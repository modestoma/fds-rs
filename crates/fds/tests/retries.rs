use fds::acl::AccessControlList;
use fds::bucket::{AccessLog, LifecycleConfig, LifecycleRule};
use fds::multipart::UploadPartOutput;
use fds::object::UploadBody;
use fds::{Client, RetryMode, RetryPolicy};
use futures_util::future::{BoxFuture, FutureExt};
use mockito::Matcher;
use std::{
    num::NonZeroU16,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn client(url: String, retries: u32) -> Client {
    Client::builder()
        .endpoint(url)
        .credentials("ak", "sk")
        .retry_policy(
            RetryPolicy::new(retries, Duration::from_millis(40), Duration::from_secs(1)).unwrap(),
        )
        .request_timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

#[tokio::test]
async fn disconnect_and_unavailable_retry_with_budget_backoff_and_fresh_signature() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let c = client(format!("http://{}", listener.local_addr().unwrap()), 2);
    let times = Arc::new(Mutex::new(Vec::new()));
    let recorded = times.clone();
    let server = tokio::spawn(async move {
        for attempt in 0..3 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut b = [0; 4096];
                let n = socket.read(&mut b).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&b[..n]);
                if bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                    break;
                }
            }
            recorded.lock().unwrap().push(std::time::Instant::now());
            let request = String::from_utf8(bytes).unwrap();
            assert!(request.starts_with("GET /b/a%2Fb"));
            assert!(request.contains("authorization: Galaxy-V2 ak:"));
            assert!(request.contains("date:"));
            if attempt == 1 {
                socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            }
            if attempt == 2 {
                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
            }
        }
    });
    assert_eq!(
        c.get_object("b", "a/b")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
        "ok"
    );
    server.await.unwrap();
    let times = times.lock().unwrap();
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(20));
    assert!(times[2].duration_since(times[1]) >= Duration::from_millis(40));
}

#[tokio::test]
async fn status_budget_server_hint_and_non_transient_rejection() {
    let mut s = mockito::Server::new_async().await;
    let c = client(s.url(), 2);
    let budget = s
        .mock("HEAD", "/b/budget")
        .with_status(503)
        .expect(3)
        .create_async()
        .await;
    assert_eq!(
        c.head_object("b", "budget")
            .send()
            .await
            .unwrap_err()
            .status()
            .map(|s| s.as_u16()),
        Some(503)
    );
    budget.assert_async().await;
    for (key, status, hint) in [
        ("hint", 429, "1"),
        ("large", 503, "2"),
        ("denied", 403, "0"),
    ] {
        let count = if key == "hint" { 3 } else { 1 };
        let m = s
            .mock("HEAD", format!("/b/{key}").as_str())
            .with_status(status)
            .with_header("retry-after", hint)
            .expect(count)
            .create_async()
            .await;
        let start = std::time::Instant::now();
        assert!(c.head_object("b", key).send().await.is_err());
        if key == "hint" {
            assert!(start.elapsed() >= Duration::from_secs(2));
        }
        m.assert_async().await;
    }
}

#[tokio::test]
async fn writes_streams_and_success_decode_errors_are_never_replayed() {
    let mut s = mockito::Server::new_async().await;
    let c = client(s.url(), 3);
    let init = s
        .mock("PUT", "/b/k?uploads=")
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    assert!(c.create_multipart_upload("b", "k").send().await.is_err());
    init.assert_async().await;
    for (key, body) in [
        ("bytes", UploadBody::from(vec![1])),
        (
            "stream",
            UploadBody::from_stream(
                futures_util::stream::iter([Ok(bytes::Bytes::from_static(b"x"))]),
                1,
            ),
        ),
    ] {
        let m = s
            .mock("PUT", format!("/b/{key}").as_str())
            .with_status(503)
            .expect(1)
            .create_async()
            .await;
        assert!(c.put_object("b", key, body).send().await.is_err());
        m.assert_async().await;
    }
    let decode = s
        .mock("GET", "/")
        .with_status(200)
        .with_body("invalid")
        .expect(1)
        .create_async()
        .await;
    assert!(c.list_buckets().send().await.is_err());
    decode.assert_async().await;
    let disabled = s
        .mock("HEAD", "/b/off")
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let c = Client::builder()
        .endpoint(s.url())
        .credentials("ak", "sk")
        .build()
        .unwrap();
    assert!(c.head_object("b", "off").send().await.is_err());
    disabled.assert_async().await;
}

fn quick_policy() -> RetryPolicy {
    RetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(2)).unwrap()
}

fn client_with(url: String, policy: RetryPolicy) -> Client {
    Client::builder()
        .endpoint(url)
        .credentials("ak", "sk")
        .retry_policy(policy)
        .build()
        .unwrap()
}

fn stream_body() -> UploadBody {
    UploadBody::from_stream(
        futures_util::stream::iter([Ok(bytes::Bytes::from_static(b"x"))]),
        1,
    )
}

type Write = (
    &'static str,
    &'static str,
    BoxFuture<'static, fds::Result<()>>,
);

fn done<T>(result: fds::Result<T>) -> fds::Result<()> {
    result.map(drop)
}

/// The idempotent writes, each with a bytes body (if any) and `mode`.
fn idempotent_writes(c: &Client, mode: RetryMode) -> Vec<Write> {
    let part = NonZeroU16::MIN;
    vec![
        (
            "PUT",
            "/b/put-bytes",
            c.put_object("b", "put-bytes", b"x".to_vec())
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "DELETE",
            "/b/delete",
            c.delete_object("b", "delete")
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/metadata",
            c.set_object_metadata("b", "metadata")
                .content_type("text/plain")
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/part",
            c.upload_part("b", "part", "id", part, b"x".to_vec())
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/lifecycle",
            c.set_lifecycle_config("lifecycle", LifecycleConfig::default())
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/accesslog",
            c.set_access_log("accesslog", AccessLog::default())
                .retry(mode)
                .send()
                .map(done)
                .boxed(),
        ),
    ]
}

/// Writes that are never retried: non-idempotent operations, and idempotent
/// ones with a stream body even when the request asks for retries.
fn single_attempt_writes(c: &Client) -> Vec<Write> {
    let part = NonZeroU16::MIN;
    let receipt = UploadPartOutput {
        part_number: part,
        etag: "e".into(),
        part_size: 1,
    };
    let rule: LifecycleRule = serde_json::from_str(r#"{"id":"r","prefix":"p/"}"#).unwrap();
    vec![
        (
            "PUT",
            "/b/put-stream",
            c.put_object("b", "put-stream", stream_body())
                .retry(RetryMode::Enabled)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/part-stream",
            c.upload_part("b", "part-stream", "id", part, stream_body())
                .retry(RetryMode::Enabled)
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/copy",
            c.copy_object(("b", "source"), ("b", "copy"))
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/rename",
            c.rename_object("b", "rename", "target")
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/restore",
            c.restore_object("b", "restore").send().map(done).boxed(),
        ),
        (
            "PUT",
            "/b/init",
            c.create_multipart_upload("b", "init")
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/complete",
            c.complete_multipart_upload("b", "complete", "id", [receipt])
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "DELETE",
            "/b/abort",
            c.abort_multipart_upload("b", "abort", "id")
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/bulk",
            c.delete_objects("bulk")
                .keys(["a"])
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/acl",
            c.set_object_acl("b", "acl", AccessControlList::default())
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/public",
            c.set_object_public("b", "public").send().map(done).boxed(),
        ),
        (
            "PUT",
            "/bucketacl",
            c.set_bucket_acl("bucketacl", AccessControlList::default())
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/rule",
            c.set_lifecycle_rule("rule", rule).send().map(done).boxed(),
        ),
        (
            "PUT",
            "/create",
            c.create_bucket("create").send().map(done).boxed(),
        ),
        (
            "DELETE",
            "/drop",
            c.delete_bucket("drop").send().map(done).boxed(),
        ),
        (
            "PUT",
            "/migrate",
            c.migrate_bucket("migrate", "org", "team")
                .send()
                .map(done)
                .boxed(),
        ),
        (
            "PUT",
            "/b/prefetch",
            c.prefetch_object("b", "prefetch").send().map(done).boxed(),
        ),
        (
            "PUT",
            "/b/refresh",
            c.refresh_object("b", "refresh").send().map(done).boxed(),
        ),
    ]
}

/// Answer every write with 503 and check how many attempts each one made.
async fn assert_attempts(s: &mut mockito::Server, writes: Vec<Write>, attempts: usize) {
    for (method, path, write) in writes {
        let m = s
            .mock(method, path)
            .match_query(Matcher::Any)
            .with_status(503)
            .expect(attempts)
            .create_async()
            .await;
        let error = write.await.unwrap_err();
        assert!(
            error.status().is_some_and(|s| s.as_u16() == 503),
            "{method} {path}: {error}"
        );
        m.assert_async().await;
        m.remove_async().await;
    }
}

#[tokio::test]
async fn writes_are_single_attempt_unless_idempotent_writes_are_enabled() {
    let mut s = mockito::Server::new_async().await;
    // A read retry policy alone never repeats a write.
    let reads = client_with(s.url(), quick_policy());
    assert_attempts(&mut s, idempotent_writes(&reads, RetryMode::Policy), 1).await;
    assert_attempts(&mut s, single_attempt_writes(&reads), 1).await;

    let writes = client_with(s.url(), quick_policy().retry_idempotent_writes());
    assert_attempts(&mut s, idempotent_writes(&writes, RetryMode::Policy), 3).await;
    // Stream bodies, copy, rename, complete, bulk deletion and every other
    // non-idempotent write: one attempt each, checked one by one.
    assert_attempts(&mut s, single_attempt_writes(&writes), 1).await;
}

#[tokio::test]
async fn per_request_mode_overrides_the_policy() {
    let mut s = mockito::Server::new_async().await;
    let reads = client_with(s.url(), quick_policy());
    assert_attempts(&mut s, idempotent_writes(&reads, RetryMode::Enabled), 3).await;
    let writes = client_with(s.url(), quick_policy().retry_idempotent_writes());
    assert_attempts(&mut s, idempotent_writes(&writes, RetryMode::Disabled), 1).await;
    // The budget always comes from the policy: none, no retries.
    let none = Client::builder()
        .endpoint(s.url())
        .credentials("ak", "sk")
        .build()
        .unwrap();
    assert_attempts(&mut s, idempotent_writes(&none, RetryMode::Enabled), 1).await;
}

#[tokio::test]
async fn delete_prefix_retries_only_its_listing() {
    let mut s = mockito::Server::new_async().await;
    let c = client_with(s.url(), quick_policy().retry_idempotent_writes());
    let busy = s
        .mock("GET", "/b")
        .match_query(Matcher::Any)
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let list = s
        .mock("GET", "/b")
        .match_query(Matcher::Any)
        .with_body(r#"{"objects":[{"name":"p/a"}]}"#)
        .expect(1)
        .create_async()
        .await;
    let delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    assert!(c.delete_prefix("b", "p/").send().await.is_err());
    busy.assert_async().await;
    list.assert_async().await;
    delete.assert_async().await;
}

#[tokio::test]
async fn idempotent_put_replays_the_same_bytes_and_uses_the_shared_rule() {
    let mut s = mockito::Server::new_async().await;
    let c = client_with(s.url(), quick_policy().retry_idempotent_writes());
    let busy = s
        .mock("PUT", "/b/k")
        .match_body("payload")
        .with_status(503)
        .expect(2)
        .create_async()
        .await;
    let ok = s
        .mock("PUT", "/b/k")
        .match_body("payload")
        .with_body(r#"{"bucketName":"b","objectName":"k"}"#)
        .expect(1)
        .create_async()
        .await;
    let output = c
        .put_object("b", "k", b"payload".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(output.object_name, "k");
    busy.assert_async().await;
    ok.assert_async().await;

    // The loop stops exactly where `is_retryable` says false: an exhausted
    // 503 is retryable, a 500 is neither retried nor retryable.
    for (status, attempts) in [(503, 3), (500, 1)] {
        let path = format!("/b/{status}");
        let m = s
            .mock("PUT", path.as_str())
            .with_status(status)
            .expect(attempts)
            .create_async()
            .await;
        let error = c
            .put_object("b", status.to_string(), b"x".to_vec())
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.is_retryable(), status == 503);
        m.assert_async().await;
    }
}

#[tokio::test]
async fn idempotent_write_is_retried_after_a_dropped_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                let mut b = [0; 4096];
                let n = socket.read(&mut b).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&b[..n]);
            }
            assert!(String::from_utf8(bytes).unwrap().starts_with("DELETE /b/k"));
            // The first attempt reached the server, but no response arrives.
            if attempt == 1 {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
        }
    });
    let c = client_with(url, quick_policy().retry_idempotent_writes());
    c.delete_object("b", "k").send().await.unwrap();
    server.await.unwrap();
}
