//! Byte-level wire snapshot of the single-object operations.
//!
//! Every request is captured from a raw TCP socket exactly as the SDK sends
//! it: request line, header lines in send order and the raw body. Only the
//! values that legitimately differ between runs are normalized: the `host`
//! port, the `date` header and the signature inside `authorization` (a
//! function of the captured method, URL and headers plus that date; the signer
//! itself is pinned by the Go golden vectors). Presigned URLs carry no clock
//! and are compared in full, signature included.
//!
//! The expected text is `tests/fixtures/wire_object_ops.txt`. Set
//! `FDS_UPDATE_WIRE_SNAPSHOT=1` to rewrite it after an intended wire change.

use fds::acl::{AccessControlList, AclAction, Grant, GrantKey, GrantPermission, GrantType};
use fds::object::{ObjectMetadata, UploadBody};
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Serve exactly one request, answer it with `status` / `body`, and return
/// the normalized request text.
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
    let client = configure(
        Client::builder()
            .endpoint(format!("http://{addr}"))
            .credentials("AKID", "sk"),
    )
    .build()
    .unwrap();
    op(client).await;
    let (head, body) = server.await.unwrap();
    normalize(&head, &body)
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

fn meta(key: &str, value: &str) -> ObjectMetadata {
    let mut metadata = ObjectMetadata::new();
    metadata.set(key, value).unwrap();
    metadata
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
                .metadata(meta("x-xiaomi-meta-k", "v"))
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
            c.set_object_metadata("b", ODD, meta("x-xiaomi-meta-k", "v"))
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

fn urls() -> Vec<(&'static str, String)> {
    let client = Client::builder()
        .endpoint("https://cnbj1.fds.api.xiaomi.com")
        .cdn_endpoint("https://cdn.cnbj1.fds.api.mi-img.com")
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    let expiration = at(1_752_400_000_123);
    let presign = |name, method: Method, cdn, metadata: Option<ObjectMetadata>| {
        let mut request = client
            .presign_object("b", ODD)
            .method(method)
            .expires_at(expiration);
        if cdn {
            request = request.cdn();
        }
        if let Some(metadata) = metadata {
            request = request.metadata(metadata);
        }
        (name, request.build().unwrap())
    };
    vec![
        presign("presign GET", Method::GET, false, None),
        presign("presign HEAD", Method::HEAD, false, None),
        presign("presign GET cdn", Method::GET, true, None),
        presign(
            "presign PUT metadata",
            Method::PUT,
            false,
            Some(meta("content-type", "text/plain")),
        ),
        presign(
            "presign DELETE cdn metadata",
            Method::DELETE,
            true,
            Some(meta("x-xiaomi-meta-k", "v")),
        ),
        (
            "absolute object",
            client.absolute_object_url("b", ODD).unwrap(),
        ),
        (
            "absolute bucket",
            client.absolute_object_url("b", "").unwrap(),
        ),
        ("absolute root", client.absolute_object_url("", "").unwrap()),
    ]
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
    // A single final newline keeps `git diff --check` quiet.
    let actual = format!("{}\n", actual.trim_end());
    if std::env::var_os("FDS_UPDATE_WIRE_SNAPSHOT").is_some() {
        std::fs::write(SNAPSHOT, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(SNAPSHOT)
        .unwrap_or_else(|e| panic!("read {SNAPSHOT}: {e}; set FDS_UPDATE_WIRE_SNAPSHOT=1"));
    assert_eq!(actual, expected, "wire bytes changed");
}
