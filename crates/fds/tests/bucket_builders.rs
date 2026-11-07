//! Bucket and OAuth operation builders: local validation happens in `send`,
//! before anything is sent, and the per-request timeout is honored.

use fds::acl::{AccessControlList, BucketCannedAcl};
use fds::bucket::{AccessLog, LifecycleConfig, LifecycleRule};
use fds::oauth::{FdsServiceClient, OAuthProvider, StorageAccessToken};
use fds::{Client, Dispatch, ErrorKind};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Nothing listens here: a request that is actually sent fails as Transport.
const UNREACHABLE: &str = "http://127.0.0.1:1";

fn client() -> Client {
    Client::builder()
        .endpoint(UNREACHABLE)
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}

fn assert_send<T: Send>(_: &T) {}

type Op = Pin<Box<dyn Future<Output = fds::Result<()>> + Send>>;

/// Every bucket builder, created with `bucket` and, when `timeout` is set,
/// that per-request timeout.
fn bucket_ops(bucket: &str, timeout: Option<Duration>) -> Vec<(&'static str, Op)> {
    let c = client();
    let b = bucket.to_string();
    macro_rules! op {
        ($builder:expr) => {{
            let mut builder = $builder;
            if let Some(timeout) = timeout {
                builder = builder.timeout(timeout);
            }
            assert_send(&builder);
            let future = builder.send();
            assert_send(&future);
            Box::pin(async move { future.await.map(drop) }) as Op
        }};
    }
    vec![
        (
            "create_bucket",
            op!(c
                .create_bucket(&b)
                .acl([BucketCannedAcl::PublicRead])
                .org_id("org")),
        ),
        ("bucket_exists", op!(c.bucket_exists(&b))),
        ("delete_bucket", op!(c.delete_bucket(&b))),
        ("get_bucket_info", op!(c.get_bucket_info(&b))),
        ("migrate_bucket", op!(c.migrate_bucket(&b, "org", "team"))),
        ("get_bucket_acl", op!(c.get_bucket_acl(&b))),
        (
            "set_bucket_acl",
            op!(c.set_bucket_acl(&b, AccessControlList::default())),
        ),
        (
            "get_lifecycle_config",
            op!(c.get_lifecycle_config(&b).rule_id("r")),
        ),
        (
            "set_lifecycle_config",
            op!(c.set_lifecycle_config(&b, LifecycleConfig::default())),
        ),
        (
            "set_lifecycle_rule",
            op!(c.set_lifecycle_rule(&b, LifecycleRule::default())),
        ),
        ("get_access_log", op!(c.get_access_log(&b))),
        (
            "set_access_log",
            op!(c.set_access_log(&b, AccessLog::default())),
        ),
    ]
}

fn assert_not_sent(name: &str, result: fds::Result<()>, kind: ErrorKind) {
    let error = result.expect_err(name);
    assert_eq!(error.kind(), kind, "{name}: {error:?}");
    assert_eq!(error.dispatch(), Dispatch::NotSent, "{name}: {error:?}");
}

#[tokio::test]
async fn every_bucket_builder_validates_bucket_and_timeout_before_sending() {
    for (name, op) in bucket_ops("", None) {
        assert_not_sent(name, op.await, ErrorKind::InvalidInput);
    }
    for (name, op) in bucket_ops("b", Some(Duration::ZERO)) {
        assert_not_sent(name, op.await, ErrorKind::InvalidInput);
    }
    for (name, op) in bucket_ops("b", Some(Duration::MAX)) {
        assert_not_sent(name, op.await, ErrorKind::InvalidInput);
    }
    // The same builders with valid input really reach the transport.
    for (name, op) in bucket_ops("b", Some(Duration::from_secs(5))) {
        let error = op.await.expect_err(name);
        assert_eq!(error.kind(), ErrorKind::Transport, "{name}: {error:?}");
    }
    // Root listings have no bucket to validate; only the timeout is local.
    let c = client();
    assert_not_sent(
        "list_buckets",
        c.list_buckets()
            .timeout(Duration::ZERO)
            .send()
            .await
            .map(drop),
        ErrorKind::InvalidInput,
    );
    assert_not_sent(
        "list_authorized_buckets",
        c.list_authorized_buckets()
            .timeout(Duration::ZERO)
            .send()
            .await
            .map(drop),
        ErrorKind::InvalidInput,
    );
    let error = c.list_buckets().send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
}

#[tokio::test]
async fn bucket_timeout_bounds_a_silent_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let client = Client::builder()
        .endpoint(format!("http://{addr}"))
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let error = client
        .get_bucket_info("b")
        .timeout(Duration::from_millis(200))
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Timeout, "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    server.abort();
}

#[tokio::test]
async fn oauth_builders_validate_before_sending() {
    let service = FdsServiceClient::new(UNREACHABLE).unwrap();
    for (name, result) in [
        ("empty region", service.get_endpoint("", "fds").send().await),
        ("blank service", service.get_endpoint("r", " ").send().await),
        (
            "zero timeout",
            service
                .get_endpoint("r", "fds")
                .timeout(Duration::ZERO)
                .send()
                .await,
        ),
    ] {
        assert_not_sent(name, result.map(drop), ErrorKind::InvalidInput);
    }
    let exchange = |provider| service.exchange_storage_token("app", "oauth-app", "token", provider);
    for (name, builder) in [
        (
            "empty app",
            service.exchange_storage_token("", "o", "t", OAuthProvider::QQ),
        ),
        ("xiaomi without mac", exchange(OAuthProvider::XiaoMi)),
        (
            "mac for qq",
            exchange(OAuthProvider::QQ).xiaomi_mac("id", "key", "alg"),
        ),
        (
            "zero timeout",
            exchange(OAuthProvider::QQ).timeout(Duration::ZERO),
        ),
        (
            "xiaomi zero timeout",
            exchange(OAuthProvider::XiaoMi)
                .xiaomi_mac("id", "key", "alg")
                .timeout(Duration::ZERO),
        ),
    ] {
        assert!(!format!("{builder:?}").contains("token"), "{name}");
        assert_not_sent(
            name,
            builder.send().await.map(drop),
            ErrorKind::InvalidInput,
        );
    }
    // Valid inputs reach the transport.
    let error = exchange(OAuthProvider::QQ).send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
    // A non-loopback plain-HTTP origin is rejected before sending.
    let insecure = FdsServiceClient::new("http://insecure.invalid").unwrap();
    assert_not_sent(
        "insecure origin",
        insecure
            .exchange_storage_token("app", "oauth-app", "token", OAuthProvider::QQ)
            .send()
            .await
            .map(drop),
        ErrorKind::ConfigInvalid,
    );
    // An access-key client cannot list app buckets.
    assert_not_sent(
        "list_app_buckets access key",
        client().list_app_buckets().send().await.map(drop),
        ErrorKind::ConfigInvalid,
    );
}

/// Exchange a long-lived test token against a one-shot loopback server.
async fn exchanged_token() -> StorageAccessToken {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let mut head = Vec::new();
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0);
            head.extend_from_slice(&buf[..n]);
        }
        let body = r#"{"token":"t","expireTime":4102444800000}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let token = FdsServiceClient::new(format!("http://{addr}"))
        .unwrap()
        .exchange_storage_token("app", "oauth-app", "token", OAuthProvider::QQ)
        .send()
        .await
        .unwrap();
    server.await.unwrap();
    token
}

#[tokio::test]
async fn list_app_buckets_validates_timeout_before_sending() {
    let client = Client::builder()
        .endpoint(UNREACHABLE)
        .storage_token(exchanged_token().await)
        .build()
        .unwrap();
    assert_not_sent(
        "list_app_buckets zero timeout",
        client
            .list_app_buckets()
            .timeout(Duration::ZERO)
            .send()
            .await
            .map(drop),
        ErrorKind::InvalidInput,
    );
    let error = client.list_app_buckets().send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
}
