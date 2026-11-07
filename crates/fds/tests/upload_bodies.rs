use bytes::Bytes;
use chrono::{TimeZone, Utc};
use fds::multipart::UploadPartRequest;
use fds::object::UploadBody;
use fds::Client;
use futures_util::stream;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn client(endpoint: String, md5: bool) -> Client {
    Client::builder()
        .endpoint(endpoint)
        .credentials("AKID", "sk")
        .enable_md5_calculate(md5)
        .build()
        .unwrap()
}

#[tokio::test]
async fn owned_put_and_stream_preserve_wire_headers_and_expiry() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("PUT", "/bucket/a%2Fb%20%2B?expires=1700000000000")
        .match_header("content-length", "5")
        .match_header("content-type", "text/plain")
        .match_header("content-md5", "5d41402abc4b2a76b9719d911017c592")
        .match_header("cache-control", "no-cache")
        .match_body("hello")
        .with_status(200)
        .with_body(r#"{"objectName":"a/b +"}"#)
        .expect(2)
        .create_async()
        .await;
    let client = client(server.url(), true);
    let expiry = Utc.timestamp_millis_opt(1700000000000).unwrap();
    let result = client
        .put_object("bucket", "a/b +", b"hello".to_vec())
        .content_type("text/plain")
        .cache_control("no-cache")
        .expires(expiry)
        .send()
        .await
        .unwrap();
    assert_eq!(result.object_name, "a/b +");
    let body = UploadBody::from_stream(
        stream::iter([
            Ok(Bytes::from_static(b"he")),
            Ok(Bytes::from_static(b"llo")),
        ]),
        5,
    );
    assert!(body.try_clone().is_none());
    client
        .put_object("bucket", "a/b +", body)
        .content_type("text/plain")
        .content_md5("5d41402abc4b2a76b9719d911017c592")
        .cache_control("no-cache")
        .expires(expiry)
        .send()
        .await
        .unwrap();
    mock.assert_async().await;
}

#[tokio::test]
async fn part_file_empty_and_owned_owned_send() {
    let mut server = mockito::Server::new_async().await;
    let part = server
        .mock("PUT", "/b/k?partNumber=1&uploadId=id")
        .match_body("part")
        .match_header("content-length", "4")
        .with_body(r#"{"partNumber":1,"etag":"e","partSize":4}"#)
        .create_async()
        .await;
    let empty = server
        .mock("PUT", "/b/empty")
        .match_header("content-length", "0")
        .with_body("{}")
        .create_async()
        .await;
    let owned = server
        .mock("PUT", "/b/owned")
        .match_body("owned")
        .with_body("{}")
        .create_async()
        .await;
    let path = std::env::temp_dir().join(format!("fds-upload-test-{}", std::process::id()));
    tokio::fs::write(&path, b"part").await.unwrap();
    let body = UploadBody::from_file(&path).await.unwrap();
    assert!(body.try_clone().is_none());
    let client = client(server.url(), false);
    let result = UploadPartRequest::new("b", "k", "id", 1, body)
        .send(&client)
        .await
        .unwrap();
    assert_eq!(result.part_number, 1);
    tokio::fs::remove_file(path).await.unwrap();
    client
        .put_object("b", "empty", Bytes::new())
        .send()
        .await
        .unwrap();
    client
        .put_object("b", "owned", b"owned".to_vec())
        .send()
        .await
        .unwrap();
    part.assert_async().await;
    empty.assert_async().await;
    owned.assert_async().await;
}

#[tokio::test]
async fn checksum_requirement_fails_before_polling_source() {
    let body = UploadBody::from_stream(
        stream::poll_fn(|_| -> std::task::Poll<Option<io::Result<Bytes>>> {
            panic!("must not poll without precomputed MD5")
        }),
        5,
    );
    let error = client("http://127.0.0.1:1".into(), true)
        .put_object("b", "k", body)
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
}

#[tokio::test]
async fn invalid_stream_lengths_and_source_errors_fail() {
    let mut server = mockito::Server::new_async().await;
    let never = server
        .mock("PUT", "/b/k")
        .with_body("{}")
        .expect(0)
        .create_async()
        .await;
    let client = client(server.url(), false);
    for (chunks, length) in [
        (vec![Ok(Bytes::from_static(b"a"))], 2),
        (vec![Ok(Bytes::from_static(b"abc"))], 2),
        (vec![Err(io::Error::other("source failed"))], 2),
    ] {
        let body = UploadBody::from_stream(stream::iter(chunks), length);
        assert!(client.put_object("b", "k", body).send().await.is_err());
    }
    never.assert_async().await;
}

struct Generated {
    count: Arc<AtomicUsize>,
    dropped: Arc<AtomicBool>,
}
impl futures_util::Stream for Generated {
    type Item = io::Result<Bytes>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.count.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok(Bytes::from(vec![b'x'; 64 * 1024]))))
    }
}
impl Drop for Generated {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn large_generated_upload_is_backpressured_and_cancellation_drops_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(format!("http://{}", listener.local_addr().unwrap()), false);
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let body = UploadBody::from_stream(
        Generated {
            count: count.clone(),
            dropped: dropped.clone(),
        },
        1_u64 << 40,
    );
    let task = tokio::spawn(async move { client.put_object("b", "k", body).send().await });
    let (mut socket, _) = listener.accept().await.unwrap();
    let mut buffer = [0; 4096];
    assert!(
        socket.read(&mut buffer).await.unwrap() > 0,
        "peer closed before sending request bytes"
    );
    // Stop draining: a 1 TiB producer must not be eagerly consumed.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let produced = count.load(Ordering::SeqCst);
    assert!(
        produced > 0 && produced < 2048,
        "produced {produced} chunks while peer stalled"
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let stopped = count.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(stopped, count.load(Ordering::SeqCst));
}

#[tokio::test]
async fn generated_upload_completes_without_collecting_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(format!("http://{}", listener.local_addr().unwrap()), false);
    let length = 32 * 1024 * 1024;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(socket.read_u8().await.unwrap());
            assert!(header.len() < 8192);
        }
        assert!(String::from_utf8(header)
            .unwrap()
            .contains(&format!("content-length: {length}\r\n")));
        let mut received = 0;
        let mut buffer = [0; 64 * 1024];
        while received < length {
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            assert!(buffer[..n].iter().all(|b| *b == b'x'));
            received += n;
        }
        assert_eq!(received, length);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
    });
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let body = UploadBody::from_stream(
        Generated {
            count: count.clone(),
            dropped: dropped.clone(),
        },
        length as u64,
    );
    client.put_object("b", "k", body).send().await.unwrap();
    server.await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), length / (64 * 1024));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn bytes_replay_shares_storage_and_debug_hides_payload() {
    let bytes = Bytes::from_static(b"secret payload");
    let body = UploadBody::from(bytes);
    assert_eq!(body.try_clone().unwrap().len(), 14);
    assert!(!format!("{body:?}").contains("secret"));
}
