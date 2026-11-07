use fds::multipart::InitMultipartUploadRequest;
use fds::object::UploadBody;
use fds::{Client, RetryPolicy};
use std::{
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
    assert!(c
        .init_multipart_upload(&InitMultipartUploadRequest {
            bucket_name: "b".into(),
            object_name: "k".into(),
            metadata: None
        })
        .await
        .is_err());
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
    assert!(c.list_buckets().await.is_err());
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
