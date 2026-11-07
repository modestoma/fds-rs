use fds::multipart::{CleanupPolicy, MultipartState, MultipartUpload};
use fds::{Client, RetryPolicy};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
struct File(PathBuf);
impl File {
    async fn new(data: &[u8]) -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fds-multipart-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::write(&path, data).await.unwrap();
        Self(path)
    }
}
impl Drop for File {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn client(url: String) -> Client {
    Client::builder()
        .endpoint(url)
        .credentials("ak", "sk")
        .retry_policy(
            RetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(2)).unwrap(),
        )
        .build()
        .unwrap()
}
/// Start a managed upload of `path` to `b/k` with 4-byte parts.
async fn start(
    client: &Client,
    path: &std::path::Path,
    concurrency: usize,
    cleanup: CleanupPolicy,
) -> fds::Result<MultipartUpload> {
    client
        .upload_file("b", "k", path)
        .part_size(4)
        .concurrency(concurrency)
        .cleanup(cleanup)
        .start()
        .await
}
async fn init(s: &mut mockito::ServerGuard) -> mockito::Mock {
    s.mock("PUT", "/b/k?uploads=")
        .with_body(r#"{"uploadId":"id","bucketName":"untrusted","objectName":"untrusted"}"#)
        .expect(1)
        .create_async()
        .await
}
async fn part(s: &mut mockito::ServerGuard, status: usize, body: &str) -> mockito::Mock {
    s.mock("PUT", "/b/k?partNumber=1&uploadId=id")
        .match_body("data")
        .with_status(status)
        .with_body(body)
        .expect(1)
        .create_async()
        .await
}

#[tokio::test]
async fn part_failure_preserves_original_cleanup_failure_and_identity() {
    for policy in [CleanupPolicy::Preserve, CleanupPolicy::AbortOnFailure] {
        let mut s = mockito::Server::new_async().await;
        let i = init(&mut s).await;
        let p = part(&mut s, 503, "part failure").await;
        let abort = s
            .mock("DELETE", "/b/k?uploadId=id")
            .with_status(500)
            .with_body("abort failure")
            .expect(if policy == CleanupPolicy::Preserve {
                0
            } else {
                1
            })
            .create_async()
            .await;
        let complete = s
            .mock("PUT", "/b/k?uploadId=id")
            .expect(0)
            .create_async()
            .await;
        let file = File::new(b"data").await;
        let mut upload = start(&client(s.url()), &file.0, 1, policy).await.unwrap();
        let error = upload.upload(std::future::pending()).await.unwrap_err();
        assert_eq!(error.status().map(|s| s.as_u16()), Some(503));
        assert_eq!(error.kind(), fds::ErrorKind::ServerError);
        let cleanup = error
            .context()
            .iter()
            .find(|(key, _)| *key == "cleanup_error")
            .map(|(_, value)| value.as_str());
        if policy == CleanupPolicy::Preserve {
            assert_eq!(cleanup, None);
        } else {
            let cleanup = cleanup.unwrap();
            assert!(
                cleanup.contains("ServerError") && cleanup.contains("HTTP 500"),
                "{cleanup}"
            );
            assert!(error.to_string().contains("cleanup_error"));
        }
        assert_eq!(upload.progress().state, MultipartState::Failed);
        assert_eq!(upload.progress().session.upload_id(), "id");
        assert_eq!(upload.progress().session.bucket(), "b");
        assert!(upload.upload(std::future::pending()).await.is_err());
        i.assert_async().await;
        p.assert_async().await;
        abort.assert_async().await;
        complete.assert_async().await;
    }
}

#[tokio::test]
async fn cooperative_cancel_aborts_once_and_bad_receipt_never_completes() {
    for cancelled in [true, false] {
        let mut s = mockito::Server::new_async().await;
        let i = init(&mut s).await;
        let p = s
            .mock("PUT", "/b/k?partNumber=1&uploadId=id")
            .with_body(r#"{"partNumber":2,"partSize":4,"etag":"e"}"#)
            .expect(if cancelled { 0 } else { 1 })
            .create_async()
            .await;
        let a = s
            .mock("DELETE", "/b/k?uploadId=id")
            .expect(1)
            .create_async()
            .await;
        let file = File::new(b"data").await;
        let mut upload = start(&client(s.url()), &file.0, 1, CleanupPolicy::AbortOnFailure)
            .await
            .unwrap();
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        }
        assert!(upload.upload(token.cancelled()).await.is_err());
        assert_eq!(upload.progress().state, MultipartState::Aborted);
        i.assert_async().await;
        p.assert_async().await;
        a.assert_async().await;
    }
}

#[tokio::test]
async fn completion_failure_is_unknown_with_all_receipts_and_no_abort_or_retry() {
    for (status, body) in [(503, "unavailable"), (200, "broken JSON"), (200, "{}")] {
        let mut s = mockito::Server::new_async().await;
        let i = init(&mut s).await;
        let p = part(&mut s, 200, r#"{"partNumber":1,"partSize":4,"etag":"e"}"#).await;
        let complete = s
            .mock("PUT", "/b/k?uploadId=id")
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let abort = s
            .mock("DELETE", "/b/k?uploadId=id")
            .expect(0)
            .create_async()
            .await;
        let file = File::new(b"data").await;
        let mut upload = start(&client(s.url()), &file.0, 1, CleanupPolicy::AbortOnFailure)
            .await
            .unwrap();
        let error = upload.upload(std::future::pending()).await.unwrap_err();
        assert!(error.is_outcome_unknown(), "{error:?}");
        assert_eq!(error.dispatch(), fds::Dispatch::Sent);
        // An identity mismatch is detected after decoding and carries no status.
        if body != "{}" {
            assert_eq!(error.status().map(|s| s.as_u16()), Some(status as u16));
        }
        assert_eq!(upload.progress().state, MultipartState::CompletionUnknown);
        assert_eq!(upload.progress().parts.len(), 1);
        assert!(upload.upload(std::future::pending()).await.is_err());
        i.assert_async().await;
        p.assert_async().await;
        complete.assert_async().await;
        abort.assert_async().await;
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, Vec<u8>) {
    let mut data = Vec::new();
    let end = loop {
        let mut bytes = [0; 4096];
        let n = socket.read(&mut bytes).await.unwrap();
        assert!(n > 0);
        data.extend_from_slice(&bytes[..n]);
        if let Some(i) = data.windows(4).position(|b| b == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let headers = String::from_utf8(data[..end].to_vec()).unwrap();
    let len = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(0);
    while data.len() < end + len {
        let mut b = [0; 4096];
        let n = socket.read(&mut b).await.unwrap();
        assert!(n > 0);
        data.extend_from_slice(&b[..n]);
    }
    (headers, data[end..].to_vec())
}
async fn respond(socket: &mut tokio::net::TcpStream, body: &str) {
    socket
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn out_of_order_parts_are_bounded_and_completed_in_order() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let c = client(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (headers, _) = read_request(&mut socket).await;
        assert!(headers.starts_with("PUT /b/k?uploads="));
        respond(&mut socket, r#"{"uploadId":"id"}"#).await;
        let mut parts = Vec::new();
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (h, b) = read_request(&mut socket).await;
            parts.push((socket, h, b));
        }
        // Exactly two requests can be in flight; the third is not read ahead/sent.
        assert!(
            tokio::time::timeout(Duration::from_millis(40), listener.accept())
                .await
                .is_err()
        );
        parts.sort_by_key(|(_, h, _)| h.clone());
        let (mut second, h, b) = parts.pop().unwrap();
        assert!(h.contains("partNumber=2"));
        assert_eq!(b, b"efgh");
        respond(&mut second, r#"{"partNumber":2,"partSize":4,"etag":"two"}"#).await;
        let (mut third, _) = listener.accept().await.unwrap();
        let (h, b) = read_request(&mut third).await;
        assert!(h.contains("partNumber=3"));
        assert_eq!(b, b"ij");
        respond(
            &mut third,
            r#"{"partNumber":3,"partSize":2,"etag":"three"}"#,
        )
        .await;
        let (mut first, h, b) = parts.pop().unwrap();
        assert!(h.contains("partNumber=1"));
        assert_eq!(b, b"abcd");
        respond(&mut first, r#"{"partNumber":1,"partSize":4,"etag":"one"}"#).await;
        let (mut complete, _) = listener.accept().await.unwrap();
        let (h, b) = read_request(&mut complete).await;
        assert!(h.starts_with("PUT /b/k?uploadId=id "));
        let value: serde_json::Value = serde_json::from_slice(&b).unwrap();
        let numbers: Vec<_> = value["uploadPartResultList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["partNumber"].as_i64().unwrap())
            .collect();
        assert_eq!(numbers, vec![1, 2, 3]);
        respond(&mut complete, r#"{"objectName":"k"}"#).await;
    });
    let file = File::new(b"abcdefghij").await;
    let mut upload = start(&c, &file.0, 2, CleanupPolicy::Preserve)
        .await
        .unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        upload.upload(std::future::pending()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output.object_name, "k");
    assert_eq!(upload.progress().state, MultipartState::Completed);
    server.await.unwrap();
}

#[tokio::test]
async fn dropping_future_keeps_session_and_completion_cancellation_never_aborts() {
    for completing in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = client(format!("http://{}", listener.local_addr().unwrap()));
        let reached = Arc::new(tokio::sync::Notify::new());
        let notify = reached.clone();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            read_request(&mut s).await;
            respond(&mut s, r#"{"uploadId":"id"}"#).await;
            let (mut s, _) = listener.accept().await.unwrap();
            read_request(&mut s).await;
            if completing {
                respond(&mut s, r#"{"partNumber":1,"partSize":4,"etag":"e"}"#).await;
                s = listener.accept().await.unwrap().0;
                read_request(&mut s).await;
            }
            notify.notify_one();
            let mut byte = [0];
            assert_eq!(s.read(&mut byte).await.unwrap(), 0);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let file = File::new(b"data").await;
        let mut upload = start(&c, &file.0, 1, CleanupPolicy::AbortOnFailure)
            .await
            .unwrap();
        let token = CancellationToken::new();
        if completing {
            let cancel = async {
                reached.notified().await;
                token.cancel();
            };
            let (result, ()) = tokio::join!(upload.upload(token.cancelled()), cancel);
            let error = result.unwrap_err();
            assert!(error.is_outcome_unknown(), "{error:?}");
            assert_eq!(error.dispatch(), fds::Dispatch::Unknown);
        } else {
            tokio::select! { result = upload.upload(token.cancelled()) => panic!("unexpected {result:?}"), _ = reached.notified() => {} }
        }
        assert_eq!(upload.progress().session.upload_id(), "id");
        assert_eq!(
            upload.progress().state,
            if completing {
                MultipartState::CompletionUnknown
            } else {
                MultipartState::Uploading
            }
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn resource_limits_and_empty_file_fail_before_network() {
    // Nothing listens here: a request that is sent fails with Transport.
    let c = client("http://127.0.0.1:1".into());
    let file = File::new(b"data").await;
    let missing = std::env::temp_dir().join("fds-multipart-missing-source");
    for (size, n) in [
        (0, 1),
        (1, 0),
        (1, 65),
        (65 * 1024 * 1024, 1),
        (64 * 1024 * 1024, 5),
    ] {
        // Options are checked before the file is opened.
        for path in [&file.0, &missing] {
            let error = c
                .upload_file("b", "k", path)
                .part_size(size)
                .concurrency(n)
                .start()
                .await
                .unwrap_err();
            assert_eq!(error.kind(), fds::ErrorKind::InvalidInput, "{size} {n}");
            assert_eq!(error.dispatch(), fds::Dispatch::NotSent);
        }
    }
    for (bucket, object) in [("", "k"), ("b", "")] {
        let error = c
            .upload_file(bucket, object, &missing)
            .start()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
    }
    let error = c.upload_file("b", "k", &missing).start().await.unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::Io);
    let empty = File::new(b"").await;
    let error = c.upload_file("b", "k", &empty.0).start().await.unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
    // 10,001 one-byte parts exceed the part-number limit.
    let large = File::new(&[0u8; 10_001]).await;
    let error = c
        .upload_file("b", "k", &large.0)
        .part_size(1)
        .start()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn initiation_without_upload_id_is_outcome_unknown() {
    let mut s = mockito::Server::new_async().await;
    let i = s
        .mock("PUT", "/b/k?uploads=")
        .with_body(r#"{"bucketName":"b","objectName":"k"}"#)
        .expect(2)
        .create_async()
        .await;
    let c = client(s.url());
    let file = File::new(b"data").await;
    let error = start(&c, &file.0, 1, CleanupPolicy::AbortOnFailure)
        .await
        .unwrap_err();
    assert!(error.is_outcome_unknown(), "{error:?}");
    let error = c
        .create_multipart_upload("b", "k")
        .send()
        .await
        .unwrap_err();
    assert!(error.is_outcome_unknown(), "{error:?}");
    i.assert_async().await;
}

// A failed run keeps everything needed to finish the upload by hand: the
// session, the confirmed receipts and the state. Only the failed part is
// re-sent, then completion carries every receipt in part order.
#[tokio::test]
async fn failed_run_is_recovered_manually_from_progress() {
    let mut s = mockito::Server::new_async().await;
    let i = init(&mut s).await;
    let first = s
        .mock("PUT", "/b/k?partNumber=1&uploadId=id")
        .match_body("abcd")
        .with_body(r#"{"partNumber":1,"partSize":4,"etag":"one"}"#)
        .expect(1)
        .create_async()
        .await;
    let failed = s
        .mock("PUT", "/b/k?partNumber=2&uploadId=id")
        .match_body("ef")
        .with_status(500)
        .expect(1)
        .create_async()
        .await;
    let no_abort = s
        .mock("DELETE", "/b/k?uploadId=id")
        .expect(0)
        .create_async()
        .await;
    let c = client(s.url());
    let file = File::new(b"abcdef").await;
    let mut upload = start(&c, &file.0, 1, CleanupPolicy::Preserve)
        .await
        .unwrap();
    let error = upload.upload(std::future::pending()).await.unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::ServerError);
    let progress = upload.progress().clone();
    assert_eq!(progress.state, MultipartState::Failed);
    assert_eq!(progress.total_parts, 2);
    assert_eq!(progress.file_size, 6);
    assert_eq!(progress.parts.len(), 1);
    assert_eq!(progress.parts[0].part_number.get(), 1);
    // Running again is refused instead of replaying hidden requests.
    let again = upload.upload(std::future::pending()).await.unwrap_err();
    assert_eq!(again.kind(), fds::ErrorKind::InvalidInput);
    failed.assert_async().await;
    failed.remove_async().await;

    let retry = s
        .mock("PUT", "/b/k?partNumber=2&uploadId=id")
        .match_body("ef")
        .with_body(r#"{"partNumber":2,"partSize":2,"etag":"two"}"#)
        .expect(1)
        .create_async()
        .await;
    let complete = s
        .mock("PUT", "/b/k?uploadId=id")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "uploadPartResultList": [
                {"partNumber": 1, "partSize": 4, "etag": "one"},
                {"partNumber": 2, "partSize": 2, "etag": "two"},
            ]
        })))
        .with_body(r#"{"bucketName":"b","objectName":"k"}"#)
        .expect(1)
        .create_async()
        .await;
    let session = &progress.session;
    let number = std::num::NonZeroU16::new(2).unwrap();
    let receipt = session
        .upload_part(number, b"ef".to_vec())
        .send()
        .await
        .unwrap();
    let mut parts = progress.parts.clone();
    parts.push(receipt);
    let output = session.complete(parts).send().await.unwrap();
    assert_eq!(output.object_name, "k");

    // The same steps work from the persisted identity alone (another process).
    let resumed_part = s
        .mock("PUT", "/b/k?partNumber=10000&uploadId=id")
        .with_body(r#"{"partNumber":10000,"partSize":1,"etag":"max"}"#)
        .expect(1)
        .create_async()
        .await;
    let abort = s
        .mock("DELETE", "/b/k?uploadId=id")
        .expect(1)
        .create_async()
        .await;
    no_abort.remove_async().await;
    let (bucket, object, id) = (session.bucket(), session.object(), session.upload_id());
    let max = std::num::NonZeroU16::new(fds::multipart::MAX_PART_NUMBER).unwrap();
    c.upload_part(bucket, object, id, max, b"x".to_vec())
        .send()
        .await
        .unwrap();
    c.abort_multipart_upload(bucket, object, id)
        .send()
        .await
        .unwrap();
    i.assert_async().await;
    first.assert_async().await;
    retry.assert_async().await;
    complete.assert_async().await;
    resumed_part.assert_async().await;
    abort.assert_async().await;
}

#[tokio::test]
async fn multipart_builders_validate_before_sending() {
    // Nothing listens here: a request that is sent fails with Transport.
    let c = client("http://127.0.0.1:1".into());
    let one = std::num::NonZeroU16::MIN;
    let too_large = std::num::NonZeroU16::new(fds::multipart::MAX_PART_NUMBER + 1).unwrap();
    let zero = std::time::Duration::ZERO;
    let results = [
        c.create_multipart_upload("", "k").send().await.map(|_| ()),
        c.create_multipart_upload("b", "k")
            .timeout(zero)
            .send()
            .await
            .map(|_| ()),
        c.upload_part("b", "k", "id", too_large, vec![])
            .send()
            .await
            .map(|_| ()),
        c.upload_part("b", "k", "", one, vec![])
            .send()
            .await
            .map(|_| ()),
        c.upload_part("b", "", "id", one, vec![])
            .send()
            .await
            .map(|_| ()),
        c.upload_part("b", "k", "id", one, vec![])
            .timeout(zero)
            .send()
            .await
            .map(|_| ()),
        c.complete_multipart_upload("b", "k", "", [])
            .send()
            .await
            .map(|_| ()),
        c.complete_multipart_upload("", "k", "id", [])
            .send()
            .await
            .map(|_| ()),
        c.complete_multipart_upload("b", "k", "id", [])
            .timeout(zero)
            .send()
            .await
            .map(|_| ()),
        c.abort_multipart_upload("b", "k", "").send().await,
        c.abort_multipart_upload("b", "", "id").send().await,
        c.abort_multipart_upload("b", "k", "id")
            .timeout(zero)
            .send()
            .await,
    ];
    for (index, result) in results.into_iter().enumerate() {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput, "case {index}");
        assert_eq!(error.dispatch(), fds::Dispatch::NotSent, "case {index}");
    }
}

#[tokio::test]
async fn cancellation_stops_inflight_parts_before_abort_and_keeps_receipts() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let c = client(format!("http://{}", listener.local_addr().unwrap()));
    let token = CancellationToken::new();
    let trigger = token.clone();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        read_request(&mut s).await;
        respond(&mut s, r#"{"uploadId":"id"}"#).await;
        let (mut s, _) = listener.accept().await.unwrap();
        read_request(&mut s).await;
        respond(&mut s, r#"{"partNumber":1,"partSize":4,"etag":"e"}"#).await;
        let (mut s, _) = listener.accept().await.unwrap();
        let (h, _) = read_request(&mut s).await;
        assert!(h.contains("partNumber=2"));
        trigger.cancel();
        let mut b = [0];
        assert_eq!(s.read(&mut b).await.unwrap(), 0);
        let (mut a, _) = listener.accept().await.unwrap();
        let (h, _) = read_request(&mut a).await;
        assert!(h.starts_with("DELETE /b/k?uploadId=id "));
        respond(&mut a, "").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), listener.accept())
                .await
                .is_err()
        );
    });
    let file = File::new(b"abcdefgh").await;
    let mut upload = start(&c, &file.0, 1, CleanupPolicy::AbortOnFailure)
        .await
        .unwrap();
    let error = upload.upload(token.cancelled()).await.unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::Cancelled);
    assert_eq!(upload.progress().parts.len(), 1);
    assert_eq!(upload.progress().state, MultipartState::Aborted);
    server.await.unwrap();
}

#[tokio::test]
async fn local_file_failure_is_preserved_and_aborted() {
    let mut s = mockito::Server::new_async().await;
    let i = init(&mut s).await;
    let a = s
        .mock("DELETE", "/b/k?uploadId=id")
        .expect(1)
        .create_async()
        .await;
    let file = File::new(b"data").await;
    let mut upload = start(&client(s.url()), &file.0, 1, CleanupPolicy::AbortOnFailure)
        .await
        .unwrap();
    tokio::fs::write(&file.0, b"").await.unwrap();
    let error = upload.upload(std::future::pending()).await.unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::Io);
    assert_eq!(upload.progress().state, MultipartState::Aborted);
    i.assert_async().await;
    a.assert_async().await;
}
