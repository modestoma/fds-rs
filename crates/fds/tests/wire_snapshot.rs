//! Byte-level wire snapshot of the single-object, bucket and OAuth operations.
//!
//! Every request is captured from a raw TCP socket exactly as the SDK sends
//! it: request line, header lines in send order and the raw body. Only the
//! values that legitimately differ between runs are normalized: the `host`
//! port, the `date` header and the signature inside `authorization` (a
//! function of the captured method, URL and headers plus that date; the signer
//! itself is pinned by the Go golden vectors). Presigned URLs carry no clock
//! and are compared in full, signature included.
//!
//! The expected texts are `tests/fixtures/wire_object_ops.txt` and
//! `tests/fixtures/wire_bucket_ops.txt`. Set `FDS_UPDATE_WIRE_SNAPSHOT=1` to
//! rewrite them after an intended wire change. The OAuth exchange and the
//! storage-token requests carry the fixed `authorization: OAuth` header and a
//! fixed test token, so they are kept verbatim.

use fds::acl::{
    AccessControlList, AclAction, BucketCannedAcl, Grant, GrantKey, GrantPermission, GrantType,
};
use fds::bucket::{AccessLog, LifecycleAction, LifecycleBaseItem, LifecycleConfig, LifecycleRule};
use fds::oauth::{FdsServiceClient, OAuthProvider};
use fds::object::{StorageClass, UploadBody, UserMetadata};
use fds::Client;
use http::Method;
use std::future::Future;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wire_object_ops.txt"
);
const BUCKET_SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wire_bucket_ops.txt"
);

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Serve exactly one request on a fresh loopback origin, answer it with
/// `status` / `body`, and return the normalized request text together with
/// what `op` returned. `op` receives the origin URL.
async fn capture_at<F, Fut, T>(status: u16, body: &'static str, op: F) -> (String, T)
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = T>,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "peer closed before the request head");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find(&buf, b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
        assert!(
            !head.to_ascii_lowercase().contains("transfer-encoding"),
            "bodies are sent with a known length"
        );
        let length = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        while buf.len() < head_end + length {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "peer closed before the request body");
            buf.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(buf.len(), head_end + length, "no bytes after the body");
        let response = format!(
            "HTTP/1.1 {status} Snapshot\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.ok();
        (head, buf[head_end..].to_vec())
    });
    let output = op(format!("http://{addr}")).await;
    let (head, body) = server.await.unwrap();
    (normalize(&head, &body), output)
}

/// [`capture_at`] with an access-key client built from `configure`.
async fn capture<F, Fut>(
    configure: impl FnOnce(fds::ClientBuilder) -> fds::ClientBuilder,
    status: u16,
    body: &'static str,
    op: F,
) -> String
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = ()>,
{
    capture_at(status, body, |origin| async move {
        let client = configure(Client::builder().endpoint(origin).credentials("AKID", "sk"))
            .build()
            .unwrap();
        op(client).await
    })
    .await
    .0
}

fn normalize(head: &str, body: &[u8]) -> String {
    let mut out = String::new();
    for line in head.trim_end_matches("\r\n").split("\r\n") {
        let normalized = match line.split_once(": ") {
            Some(("host", _)) => "host: <host>".to_string(),
            Some(("date", value)) => {
                assert!(value.ends_with(" GMT"), "{line}");
                "date: <date>".to_string()
            }
            Some(("authorization", "OAuth")) => line.to_string(),
            Some(("authorization", value)) => {
                let signature = value
                    .strip_prefix("Galaxy-V2 AKID:")
                    .unwrap_or_else(|| panic!("unexpected authorization {value}"));
                assert!(!signature.is_empty());
                "authorization: Galaxy-V2 AKID:<signature>".to_string()
            }
            _ => line.to_string(),
        };
        out.push_str(&normalized);
        out.push('\n');
    }
    out.push_str("--- body ---\n");
    match std::str::from_utf8(body) {
        Ok(text) => out.push_str(text),
        Err(_) => out.push_str(&format!("<hex {}>", hex(body))),
    }
    out.push('\n');
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn plain(builder: fds::ClientBuilder) -> fds::ClientBuilder {
    builder
}

fn md5(builder: fds::ClientBuilder) -> fds::ClientBuilder {
    builder.enable_md5_calculate(true)
}

/// One user metadata entry `x-xiaomi-meta-{key}`.
fn user(key: &str, value: &str) -> UserMetadata {
    [(key, value)].into_iter().collect()
}

fn grant(id: &str, permission: GrantPermission, grant_type: GrantType) -> AccessControlList {
    let mut acl = AccessControlList::default();
    acl.add_grant(Grant {
        grantee: GrantKey {
            id: id.into(),
            display_name: String::new(),
        },
        permission,
        grant_type,
    });
    acl
}

fn at(millis: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(millis).unwrap()
}

const ACL_BODY: &str = r#"{"accessControlList":[],"owner":{"id":"o","displayName":""}}"#;
const ODD: &str = "dir/a b+c.txt";

async fn requests() -> Vec<(&'static str, String)> {
    let mut cases = Vec::new();

    cases.push((
        "get_object",
        capture(plain, 200, "x", |c| async move {
            c.get_object("b", ODD)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "get_object range timeout",
        capture(plain, 206, "x", |c| async move {
            c.get_object("b", "o")
                .range("bytes=0-4".parse().unwrap())
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "head_object",
        capture(plain, 200, "", |c| async move {
            assert!(c.head_object("b", ODD).send().await.unwrap().is_some());
        })
        .await,
    ));
    cases.push((
        "object_exists missing",
        capture(plain, 404, "", |c| async move {
            assert!(!c.object_exists("b", "o").send().await.unwrap());
        })
        .await,
    ));
    cases.push((
        "get_object_metadata",
        capture(plain, 200, "", |c| async move {
            c.get_object_metadata("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "put_object bytes all options",
        capture(plain, 200, "{}", |c| async move {
            c.put_object("b", ODD, b"hello world".to_vec())
                .cache_control("no-cache")
                .content_disposition("attachment")
                .content_encoding("identity")
                .content_type("text/plain")
                .content_md5("XrY7u+Ae7tCTyyK7j1rNww==")
                .expect("100-continue")
                .user_metadata(user("k", "v"))
                .expires(at(1_752_400_000_123))
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "put_object empty options",
        capture(plain, 200, "{}", |c| async move {
            c.put_object("b", "o", Vec::new())
                .content_type("")
                .cache_control("")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "put_object stream precomputed md5",
        capture(md5, 200, "{}", |c| async move {
            let chunks = [
                Ok(bytes::Bytes::from_static(b"hello ")),
                Ok(bytes::Bytes::from_static(b"stream")),
            ];
            let body = UploadBody::from_stream(futures_util::stream::iter(chunks), 12);
            c.put_object("b", "o", body)
                .content_md5("precomputed-md5-value")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "put_object bytes md5",
        capture(md5, 200, "{}", |c| async move {
            c.put_object("b", "o", b"md5 me".to_vec())
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("wire_snapshot_put.txt");
    std::fs::write(&path, b"from a file").unwrap();
    cases.push((
        "put_object from_file",
        capture(plain, 200, "{}", |c| async move {
            let body = UploadBody::from_file(&path).await.unwrap();
            c.put_object("b", "o", body)
                .content_type("text/plain")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "copy_object",
        capture(
            plain,
            200,
            r#"{"bucketName":"tb","objectName":"dir/a b+c.txt"}"#,
            |c| async move {
                let receipt = c
                    .copy_object(("sb", "src/x y"), ("tb", ODD))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(receipt.object_name, ODD);
            },
        )
        .await,
    ));
    cases.push((
        "rename_object",
        capture(plain, 200, "", |c| async move {
            c.rename_object("b", ODD, "new/x y+z").send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "restore_object",
        capture(plain, 200, "", |c| async move {
            c.restore_object("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "prefetch_object",
        capture(plain, 200, "", |c| async move {
            c.prefetch_object("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "refresh_object",
        capture(plain, 200, "", |c| async move {
            c.refresh_object("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "delete_object",
        capture(plain, 200, "", |c| async move {
            c.delete_object("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "set_object_metadata",
        capture(plain, 200, "", |c| async move {
            c.set_object_metadata("b", ODD)
                .user_metadata(user("k", "v"))
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "get_object_basic_info",
        capture(plain, 200, "{}", |c| async move {
            c.get_object_basic_info("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_object_acl",
        capture(plain, 200, ACL_BODY, |c| async move {
            c.get_object_acl("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_object_acl version",
        capture(plain, 200, ACL_BODY, |c| async move {
            c.get_object_acl("b", "o")
                .version_id("v 1")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "get_object_acl empty version",
        capture(plain, 200, ACL_BODY, |c| async move {
            c.get_object_acl("b", "o")
                .version_id("")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "set_object_acl add",
        capture(plain, 200, "", |c| async move {
            let acl = grant("user1", GrantPermission::FullControl, GrantType::User);
            c.set_object_acl("b", ODD, acl).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "set_object_acl delete version",
        capture(plain, 200, "", |c| async move {
            let acl = grant("user1", GrantPermission::Write, GrantType::User);
            c.set_object_acl("b", "o", acl)
                .action(AclAction::Delete)
                .version_id("v1")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "set_object_public",
        capture(plain, 200, "", |c| async move {
            c.set_object_public("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases
}

const BUCKET_INFO: &str = r#"{"name":"b","creationTime":1,"numObjects":2,"usedSpace":3}"#;
const BUCKETS: &str = r#"{"owner":{"id":"o","displayName":""},"buckets":[]}"#;
const LIFECYCLE: &str = r#"{"rules":[]}"#;
const ACCESS_LOG: &str =
    r#"{"bucketName":"b","enabled":true,"logBucketName":"logs","logPrefix":"p/"}"#;
/// Far in the future, so the exchanged test token never expires.
const TOKEN: &str = r#"{"token":"storage-token +&%","expireTime":4102444800000}"#;
const ODD_BUCKET: &str = "b-1";

fn lifecycle_rule(id: &str, prefix: &str, actions: &[(LifecycleAction, f64)]) -> LifecycleRule {
    LifecycleRule {
        id: id.into(),
        prefix: prefix.into(),
        enabled: true,
        action: actions
            .iter()
            .map(|(action, days)| (action.clone(), LifecycleBaseItem { days: *days }))
            .collect(),
    }
}

async fn bucket_requests() -> Vec<(&'static str, String)> {
    let mut cases = Vec::new();

    cases.push((
        "create_bucket",
        capture(plain, 200, "", |c| async move {
            c.create_bucket(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "create_bucket all options",
        capture(plain, 200, "", |c| async move {
            c.create_bucket("b")
                .acl([
                    BucketCannedAcl::PublicRead,
                    BucketCannedAcl::AuthenticatedWrite,
                ])
                .storage_class(StorageClass::new(StorageClass::STANDARD_IA))
                .org_id("org +&1")
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "create_bucket empty org and acl",
        capture(plain, 200, "", |c| async move {
            c.create_bucket("b")
                .acl([])
                .org_id("")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "bucket_exists",
        capture(plain, 200, "", |c| async move {
            assert!(c.bucket_exists(ODD_BUCKET).send().await.unwrap());
        })
        .await,
    ));
    cases.push((
        "bucket_exists missing",
        capture(plain, 404, "", |c| async move {
            assert!(!c.bucket_exists("b").send().await.unwrap());
        })
        .await,
    ));
    cases.push((
        "delete_bucket",
        capture(plain, 200, "", |c| async move {
            c.delete_bucket(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_bucket_info",
        capture(plain, 200, BUCKET_INFO, |c| async move {
            c.get_bucket_info(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "list_buckets",
        capture(plain, 200, BUCKETS, |c| async move {
            c.list_buckets().send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "list_authorized_buckets",
        capture(plain, 200, BUCKETS, |c| async move {
            c.list_authorized_buckets().send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "migrate_bucket",
        capture(plain, 200, "", |c| async move {
            c.migrate_bucket(ODD_BUCKET, "org +&1", "team 2")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "get_bucket_acl",
        capture(plain, 200, ACL_BODY, |c| async move {
            c.get_bucket_acl(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "set_bucket_acl add",
        capture(plain, 200, "", |c| async move {
            let acl = grant("user1", GrantPermission::Read, GrantType::User);
            c.set_bucket_acl(ODD_BUCKET, acl).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "set_bucket_acl delete",
        capture(plain, 200, "", |c| async move {
            let acl = grant("ALL_USERS", GrantPermission::Write, GrantType::Group);
            c.set_bucket_acl("b", acl)
                .action(AclAction::Delete)
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "get_lifecycle_config",
        capture(plain, 200, LIFECYCLE, |c| async move {
            c.get_lifecycle_config(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_lifecycle_config rule",
        capture(plain, 200, LIFECYCLE, |c| async move {
            c.get_lifecycle_config("b")
                .rule_id("rule +&1")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "set_lifecycle_config",
        capture(plain, 200, "", |c| async move {
            let config = LifecycleConfig {
                rules: vec![
                    lifecycle_rule(
                        "r1",
                        "logs/",
                        &[
                            (LifecycleAction::Expiration, 30.0),
                            (LifecycleAction::AbortIncompleteMultipartUpload, 7.0),
                            (LifecycleAction::NonCurrentVersionExpiration, 1.5),
                        ],
                    ),
                    lifecycle_rule(
                        "r2",
                        "",
                        &[(LifecycleAction::from("abortIncompleteMultipartUpload"), 2.0)],
                    ),
                ],
            };
            c.set_lifecycle_config(ODD_BUCKET, config)
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "set_lifecycle_rule",
        capture(plain, 200, "", |c| async move {
            let rule = lifecycle_rule(
                "r 3",
                "tmp/",
                &[
                    (LifecycleAction::AbortIncompleteMultipartUpload, 1.0),
                    (LifecycleAction::from("transition"), 0.0),
                ],
            );
            c.set_lifecycle_rule(ODD_BUCKET, rule).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_access_log",
        capture(plain, 200, ACCESS_LOG, |c| async move {
            c.get_access_log(ODD_BUCKET).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "set_access_log",
        capture(plain, 200, "", |c| async move {
            let config = AccessLog {
                bucket_name: ODD_BUCKET.into(),
                enabled: true,
                log_bucket_name: "logs".into(),
                log_prefix: "access/ +&".into(),
            };
            c.set_access_log(ODD_BUCKET, config).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "get_endpoint",
        capture_at(200, r#""cnbj1.fds.api.xiaomi.com""#, |origin| async move {
            let service = FdsServiceClient::new(origin).unwrap();
            let host = service.get_endpoint("sg +&1", "fds").send().await.unwrap();
            assert_eq!(host, "cnbj1.fds.api.xiaomi.com");
        })
        .await
        .0,
    ));
    cases.push((
        "exchange_storage_token qq",
        capture_at(200, TOKEN, |origin| async move {
            FdsServiceClient::new(origin)
                .unwrap()
                .exchange_storage_token(
                    "app +&1",
                    "oauth-app",
                    "oauth-token +&%",
                    OAuthProvider::QQ,
                )
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .unwrap();
        })
        .await
        .0,
    ));
    let (exchange, token) = capture_at(200, TOKEN, |origin| async move {
        FdsServiceClient::from_builder(Client::builder().endpoint(origin))
            .unwrap()
            .exchange_storage_token(
                "app +&1",
                "oauth-app",
                "oauth-token +&%",
                OAuthProvider::XiaoMi,
            )
            .xiaomi_mac("open+id", "mac+key", "HmacSHA1")
            .send()
            .await
            .unwrap()
    })
    .await;
    cases.push(("exchange_storage_token xiaomi mac", exchange));
    cases.push((
        "list_app_buckets",
        capture_at(200, BUCKETS, |origin| async move {
            let client = Client::builder()
                .endpoint(origin)
                .storage_token(token)
                .build()
                .unwrap();
            client.list_app_buckets().send().await.unwrap();
        })
        .await
        .0,
    ));
    cases
}

fn urls() -> Vec<(&'static str, String)> {
    let client = Client::builder()
        .endpoint("https://cnbj1.fds.api.xiaomi.com")
        .cdn_endpoint("https://cdn.cnbj1.fds.api.mi-img.com")
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    let expiration = at(1_752_400_000_123);
    enum Signed {
        ContentType(&'static str),
        User(UserMetadata),
    }
    let presign = |name, method: Method, cdn, signed: Option<Signed>| {
        let mut request = client
            .presign_object("b", ODD)
            .method(method)
            .expires_at(expiration);
        if cdn {
            request = request.cdn();
        }
        request = match signed {
            Some(Signed::ContentType(value)) => request.content_type(value),
            Some(Signed::User(metadata)) => request.user_metadata(metadata),
            None => request,
        };
        // The snapshot records the Url's exact string form.
        (name, request.build().unwrap().to_string())
    };
    vec![
        presign("presign GET", Method::GET, false, None),
        presign("presign HEAD", Method::HEAD, false, None),
        presign("presign GET cdn", Method::GET, true, None),
        presign(
            "presign PUT metadata",
            Method::PUT,
            false,
            Some(Signed::ContentType("text/plain")),
        ),
        presign(
            "presign DELETE cdn metadata",
            Method::DELETE,
            true,
            Some(Signed::User(user("k", "v"))),
        ),
        (
            "absolute object",
            client.absolute_object_url("b", ODD).unwrap().to_string(),
        ),
        (
            "absolute bucket",
            client.absolute_object_url("b", "").unwrap().to_string(),
        ),
        (
            "absolute root",
            client.absolute_object_url("", "").unwrap().to_string(),
        ),
    ]
}

/// Compare `actual` with the fixture at `path`, rewriting it first when
/// `FDS_UPDATE_WIRE_SNAPSHOT` is set.
fn assert_snapshot(path: &str, actual: &str) {
    // A single final newline keeps `git diff --check` quiet.
    let actual = format!("{}\n", actual.trim_end());
    if std::env::var_os("FDS_UPDATE_WIRE_SNAPSHOT").is_some() {
        std::fs::write(path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}; set FDS_UPDATE_WIRE_SNAPSHOT=1"));
    assert_eq!(actual, expected, "wire bytes changed");
}

#[tokio::test]
async fn single_object_operations_match_the_wire_snapshot() {
    let mut actual = String::new();
    for (name, request) in requests().await {
        actual.push_str(&format!("=== {name} ===\n{request}\n"));
    }
    for (name, url) in urls() {
        actual.push_str(&format!("=== {name} ===\n{url}\n\n"));
    }
    assert_snapshot(SNAPSHOT, &actual);
}

#[tokio::test]
async fn bucket_and_oauth_operations_match_the_wire_snapshot() {
    let mut actual = String::new();
    for (name, request) in bucket_requests().await {
        actual.push_str(&format!("=== {name} ===\n{request}\n"));
    }
    assert_snapshot(BUCKET_SNAPSHOT, &actual);
}

// ---- Listing and bulk deletion ----------------------------------------------
//
// Same capture and normalization as above, written to its own fixture,
// `tests/fixtures/wire_listing_delete.txt`. Paginating operations send several
// requests; each answer closes its connection, so every request is captured
// from a fresh socket, in send order.

const LISTING_SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wire_listing_delete.txt"
);

/// Read one request from `socket`, answer `200` with `body` and close.
async fn serve_one(mut socket: tokio::net::TcpStream, body: &str) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0, "peer closed before the request head");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0, "peer closed before the request body");
        buf.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(buf.len(), head_end + length, "no bytes after the body");
    let response = format!(
        "HTTP/1.1 200 Snapshot\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await.unwrap();
    socket.shutdown().await.ok();
    (head, buf[head_end..].to_vec())
}

/// Serve one request per entry of `bodies`, in order, and return the
/// normalized requests.
async fn capture_all<F, Fut>(bodies: &'static [&'static str], op: F) -> String
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = ()>,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for body in bodies {
            let (socket, _) = listener.accept().await.unwrap();
            captured.push(serve_one(socket, body).await);
        }
        captured
    });
    let client = Client::builder()
        .endpoint(format!("http://{addr}"))
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    op(client).await;
    let captured = server.await.unwrap();
    let mut out = String::new();
    for (index, (head, body)) in captured.iter().enumerate() {
        out.push_str(&format!("--- request {} ---\n", index + 1));
        out.push_str(&normalize(head, body));
    }
    out
}

async fn listing_delete_requests() -> Vec<(&'static str, String)> {
    use futures_util::TryStreamExt;
    use std::num::NonZeroU32;

    let keys = |n| NonZeroU32::new(n).unwrap();

    let mut cases = Vec::new();
    cases.push((
        "list_objects defaults",
        capture_all(&["{}"], |c| async move {
            c.list_objects("b").send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "list_objects all options",
        capture_all(&["{}"], |c| async move {
            c.list_objects("b")
                .prefix(ODD)
                .delimiter("/")
                .max_keys(keys(7))
                .marker("m k+1/2")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "list_objects empty marker",
        capture_all(&["{}"], |c| async move {
            c.list_objects("b").marker("").send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "list_objects pages",
        capture_all(
            &[
                r#"{"truncated":true,"nextMarker":"n x+1","objects":[]}"#,
                "{}",
            ],
            |c| async move {
                let pages: Vec<_> = c
                    .list_objects("b")
                    .prefix("p/")
                    .into_pages()
                    .try_collect()
                    .await
                    .unwrap();
                assert_eq!(pages.len(), 2);
            },
        )
        .await,
    ));
    cases.push((
        "list_objects objects from marker",
        capture_all(
            &[
                r#"{"truncated":true,"nextMarker":"n","objects":[{"name":"p/a"}]}"#,
                r#"{"objects":[{"name":"p/b"}]}"#,
            ],
            |c| async move {
                let objects: Vec<_> = c
                    .list_objects("b")
                    .max_keys(keys(2))
                    .marker("start")
                    .into_objects()
                    .try_collect()
                    .await
                    .unwrap();
                assert_eq!(objects.len(), 2);
            },
        )
        .await,
    ));
    cases.push((
        "delete_objects",
        capture_all(&["[]"], |c| async move {
            c.delete_objects("b")
                .keys(["a", ODD, "a"])
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "delete_objects trash",
        capture_all(&["[]"], |c| async move {
            c.delete_objects("b")
                .keys(["x"])
                .move_to_trash()
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "delete_prefix",
        capture_all(
            &[
                r#"{"objects":[{"name":"p/a"}],"truncated":true,"nextMarker":"m"}"#,
                "[]",
                r#"{"objects":[{"name":"p/b c"}]}"#,
                "[]",
            ],
            |c| async move {
                c.delete_prefix("b", "p/").send().await.unwrap();
            },
        )
        .await,
    ));
    cases.push((
        "delete_prefix trash page size",
        capture_all(&[r#"{"objects":[{"name":"p/a"}]}"#, "[]"], |c| async move {
            c.delete_prefix("b", "p/")
                .page_size(keys(2))
                .move_to_trash()
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "delete_prefix empty marker",
        capture_all(&[r#"{"objects":[{"name":"p/a"}]}"#, "[]"], |c| async move {
            c.delete_prefix("b", "p/").marker("").send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "delete_prefix marker delimiter",
        capture_all(
            &[
                r#"{"objects":[{"name":"p/a"}],"commonPrefixes":["p/d/"],"truncated":true,"nextMarker":"n x+1"}"#,
                "[]",
                r#"{"objects":[{"name":"p/b"}]}"#,
                "[]",
            ],
            |c| async move {
                c.delete_prefix("b", "p/")
                    .marker("m k+1/2")
                    .delimiter("/")
                    .send()
                    .await
                    .unwrap();
            },
        )
        .await,
    ));
    cases
}

#[tokio::test]
async fn listing_and_bulk_delete_match_the_wire_snapshot() {
    let mut actual = String::new();
    for (name, requests) in listing_delete_requests().await {
        actual.push_str(&format!("=== {name} ===\n{requests}\n"));
    }
    let actual = format!("{}\n", actual.trim_end());
    if std::env::var_os("FDS_UPDATE_WIRE_SNAPSHOT").is_some() {
        std::fs::write(LISTING_SNAPSHOT, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(LISTING_SNAPSHOT)
        .unwrap_or_else(|e| panic!("read {LISTING_SNAPSHOT}: {e}; set FDS_UPDATE_WIRE_SNAPSHOT=1"));
    assert_eq!(actual, expected, "wire bytes changed");
}

// ---------------------------------------------------------------------------
// Multipart requests and the managed transfers (upload and download).
//
// Same normalization as above; the expected text is
// `tests/fixtures/wire_multipart_ops.txt`, rewritten by the same
// `FDS_UPDATE_WIRE_SNAPSHOT=1`. Managed transfers send several requests, so
// `capture_sequence` serves one reply per connection in order and joins the
// normalized requests.

const MULTIPART_SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/wire_multipart_ops.txt"
);

/// One scripted reply: status, extra header lines (each ending in `\r\n`)
/// and body. `Content-Length` is the body length unless `headers` sets it.
struct Reply {
    status: u16,
    headers: &'static str,
    body: &'static str,
}

fn reply(status: u16, body: &'static str) -> Reply {
    Reply {
        status,
        headers: "",
        body,
    }
}

async fn read_one(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0, "peer closed before the request head");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "bodies are sent with a known length"
    );
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let n = socket.read(&mut chunk).await.unwrap();
        assert!(n > 0, "peer closed before the request body");
        buf.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(buf.len(), head_end + length, "no bytes after the body");
    (head, buf[head_end..].to_vec())
}

/// Serve `replies.len()` sequential requests (one per connection) and return
/// the normalized requests in arrival order.
async fn capture_sequence<F, Fut>(
    configure: impl FnOnce(fds::ClientBuilder) -> fds::ClientBuilder,
    replies: Vec<Reply>,
    op: F,
) -> String
where
    F: FnOnce(Client) -> Fut,
    Fut: Future<Output = ()>,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, body) = read_one(&mut socket).await;
            let length = if reply.headers.contains("Content-Length:") {
                String::new()
            } else {
                format!("Content-Length: {}\r\n", reply.body.len())
            };
            let response = format!(
                "HTTP/1.1 {} Snapshot\r\n{}{length}Connection: close\r\n\r\n{}",
                reply.status, reply.headers, reply.body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
            captured.push((head, body));
        }
        // Nothing else may be sent.
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "unexpected extra request"
        );
        captured
    });
    let client = configure(
        Client::builder()
            .endpoint(format!("http://{addr}"))
            .credentials("AKID", "sk"),
    )
    .build()
    .unwrap();
    op(client).await;
    let captured = server.await.unwrap();
    captured
        .iter()
        .enumerate()
        .map(|(index, (head, body))| {
            format!("--- request {} ---\n{}", index + 1, normalize(head, body))
        })
        .collect()
}

fn tmp_file(name: &str, data: &[u8]) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, data).unwrap();
    path
}

#[cfg(feature = "manager")]
fn tmp_dir(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

const INIT_BODY: &str = r#"{"bucketName":"b","objectName":"dir/a b+c.txt","uploadId":"u/1 +2"}"#;
const PART1_BODY: &str = r#"{"partNumber":1,"etag":"e1","partSize":4}"#;
const PART2_BODY: &str = r#"{"partNumber":2,"etag":"e2","partSize":4}"#;
const COMPLETE_BODY: &str = r#"{"bucketName":"b","objectName":"dir/a b+c.txt"}"#;

fn receipt(number: u16, etag: &str, size: u64) -> fds::multipart::UploadPartOutput {
    fds::multipart::UploadPartOutput {
        part_number: std::num::NonZeroU16::new(number).unwrap(),
        etag: etag.into(),
        part_size: size,
    }
}

async fn multipart_requests() -> Vec<(&'static str, String)> {
    use fds::multipart::CleanupPolicy;
    use std::num::NonZeroU16;
    let n = |number| NonZeroU16::new(number).unwrap();
    let mut cases = Vec::new();

    cases.push((
        "init_multipart_upload",
        capture(plain, 200, INIT_BODY, |c| async move {
            c.create_multipart_upload("b", ODD).send().await.unwrap();
        })
        .await,
    ));
    cases.push((
        "init_multipart_upload metadata",
        capture(plain, 200, INIT_BODY, |c| async move {
            c.create_multipart_upload("b", ODD)
                .user_metadata(user("k", "v"))
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "upload_part bytes",
        capture(plain, 200, PART2_BODY, |c| async move {
            c.upload_part("b", ODD, "u/1 +2", n(2), b"efgh".to_vec())
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "upload_part content_md5",
        capture(plain, 200, PART1_BODY, |c| async move {
            c.upload_part("b", "o", "u", n(1), b"abcd".to_vec())
                .content_md5("4vxxTEcn7pOV8yTNLn8zHw==")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "upload_part bytes md5",
        capture(md5, 200, PART1_BODY, |c| async move {
            c.upload_part("b", "o", "u", n(1), b"abcd".to_vec())
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "upload_part stream",
        capture(plain, 200, PART1_BODY, |c| async move {
            let chunks = [
                Ok(bytes::Bytes::from_static(b"ab")),
                Ok(bytes::Bytes::from_static(b"cd")),
            ];
            let body = UploadBody::from_stream(futures_util::stream::iter(chunks), 4);
            c.upload_part("b", "o", "u", n(1), body)
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    let part_file = tmp_file("wire_snapshot_part.txt", b"abcd");
    cases.push((
        "upload_part from_file",
        capture(plain, 200, PART1_BODY, |c| async move {
            let body = UploadBody::from_file(&part_file).await.unwrap();
            c.upload_part("b", "o", "u", n(1), body)
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "complete_multipart_upload",
        capture(plain, 200, COMPLETE_BODY, |c| async move {
            c.complete_multipart_upload(
                "b",
                ODD,
                "u/1 +2",
                [receipt(1, "e1", 4), receipt(2, "e2", 4)],
            )
            .send()
            .await
            .unwrap();
        })
        .await,
    ));
    cases.push((
        "complete_multipart_upload metadata expires",
        capture(plain, 200, COMPLETE_BODY, |c| async move {
            c.complete_multipart_upload("b", "o", "u", [receipt(1, "e1", 4)])
                .content_type("text/plain")
                .expires(at(1_752_400_000_123))
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "complete_multipart_upload no parts",
        capture(plain, 200, COMPLETE_BODY, |c| async move {
            c.complete_multipart_upload("b", "o", "u", [])
                .send()
                .await
                .unwrap();
        })
        .await,
    ));
    cases.push((
        "abort_multipart_upload",
        capture(plain, 200, "", |c| async move {
            c.abort_multipart_upload("b", ODD, "u/1 +2")
                .send()
                .await
                .unwrap();
        })
        .await,
    ));

    let source = tmp_file("wire_snapshot_managed.txt", b"abcdefgh");
    let path = source.clone();
    cases.push((
        "managed upload",
        capture_sequence(
            plain,
            vec![
                reply(200, INIT_BODY),
                reply(200, PART1_BODY),
                reply(200, PART2_BODY),
                reply(200, COMPLETE_BODY),
            ],
            |c| async move {
                let mut upload = c
                    .upload_file("b", ODD, &path)
                    .part_size(4)
                    .concurrency(1)
                    .cleanup(CleanupPolicy::Preserve)
                    .user_metadata(user("k", "v"))
                    .start()
                    .await
                    .unwrap();
                upload.upload(std::future::pending()).await.unwrap();
            },
        )
        .await,
    ));
    let path = source.clone();
    cases.push((
        "managed upload abort on failure",
        capture_sequence(
            plain,
            vec![
                reply(200, INIT_BODY),
                reply(503, "unavailable"),
                reply(200, ""),
            ],
            |c| async move {
                let mut upload = c
                    .upload_file("b", ODD, &path)
                    .part_size(4)
                    .concurrency(1)
                    .cleanup(CleanupPolicy::AbortOnFailure)
                    .start()
                    .await
                    .unwrap();
                upload.upload(std::future::pending()).await.unwrap_err();
            },
        )
        .await,
    ));
    cases
}

#[cfg(feature = "manager")]
async fn download_requests() -> Vec<(&'static str, String)> {
    let mut cases = Vec::new();
    const HEAD: &str = "Content-Length: 8\r\nETag: \"v1\"\r\n";
    let dir = tmp_dir("wire_snapshot_download");
    let out = dir.join("out");
    cases.push((
        "download parts",
        capture_sequence(
            plain,
            vec![
                Reply {
                    status: 200,
                    headers: HEAD,
                    body: "",
                },
                Reply {
                    status: 206,
                    headers: "Content-Range: bytes 0-3/8\r\nETag: \"v1\"\r\n",
                    body: "abcd",
                },
                Reply {
                    status: 206,
                    headers: "Content-Range: bytes 4-7/8\r\nETag: \"v1\"\r\n",
                    body: "efgh",
                },
            ],
            |c| async move {
                c.download_file("b", ODD, &out)
                    .part_size(4)
                    .concurrency(1)
                    .send()
                    .await
                    .unwrap();
            },
        )
        .await,
    ));
    let out = dir.join("out");
    assert_eq!(std::fs::read(&out).unwrap(), b"abcdefgh");
    cases.push((
        "download range",
        capture_sequence(
            plain,
            vec![
                Reply {
                    status: 200,
                    headers: HEAD,
                    body: "",
                },
                Reply {
                    status: 206,
                    headers: "Content-Range: bytes 2-5/8\r\nETag: \"v1\"\r\n",
                    body: "cdef",
                },
            ],
            |c| async move {
                c.download_file("b", "o", &out)
                    .range(fds::object::ByteRange::inclusive(2, 5).unwrap())
                    .part_size(4)
                    .send()
                    .await
                    .unwrap();
            },
        )
        .await,
    ));
    assert_eq!(std::fs::read(dir.join("out")).unwrap(), b"cdef");
    cases
}

#[cfg(not(feature = "manager"))]
async fn download_requests() -> Vec<(&'static str, String)> {
    Vec::new()
}

#[tokio::test]
async fn multipart_and_transfer_operations_match_the_wire_snapshot() {
    let mut actual = String::new();
    for (name, request) in multipart_requests().await {
        actual.push_str(&format!("=== {name} ===\n{request}\n"));
    }
    let downloads = download_requests().await;
    for (name, request) in &downloads {
        actual.push_str(&format!("=== {name} ===\n{request}\n"));
    }
    let actual = format!("{}\n", actual.trim_end());
    if std::env::var_os("FDS_UPDATE_WIRE_SNAPSHOT").is_some() && !downloads.is_empty() {
        std::fs::write(MULTIPART_SNAPSHOT, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(MULTIPART_SNAPSHOT).unwrap_or_else(|e| {
        panic!("read {MULTIPART_SNAPSHOT}: {e}; set FDS_UPDATE_WIRE_SNAPSHOT=1")
    });
    if downloads.is_empty() {
        // Without the manager feature only the multipart prefix is compared.
        let prefix = expected
            .split("=== download parts ===")
            .next()
            .unwrap()
            .trim_end();
        assert_eq!(actual.trim_end(), prefix, "wire bytes changed");
    } else {
        assert_eq!(actual, expected, "wire bytes changed");
    }
}
