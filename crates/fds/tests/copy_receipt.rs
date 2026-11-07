use fds::object::CopyObjectRequest;
use fds::{Client, Dispatch, ErrorKind, RetryPolicy};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn client(url: String) -> Client {
    Client::builder()
        .endpoint(url)
        .credentials("id", "secret")
        .retry_policy(
            RetryPolicy::new(3, Duration::from_millis(1), Duration::from_millis(2)).unwrap(),
        )
        .request_timeout(Duration::from_millis(300))
        .read_timeout(Duration::from_millis(100))
        .build()
        .unwrap()
}

#[tokio::test]
async fn receipts_are_required_matching_bounded_and_never_retried() {
    let good = r#"{"bucketName":"dst","objectName":"中文 a.tar.gz","extra":true}"#;
    let cases = vec![
        (200, good.to_owned(), true),
        (
            200,
            format!("{}{}", good, " ".repeat(65536 - good.len())),
            true,
        ),
        (200, String::new(), false),
        (200, " ".into(), false),
        (200, "{}".into(), false),
        (
            200,
            r#"{"bucketName":"dst","objectName":null}"#.into(),
            false,
        ),
        (
            200,
            r#"{"bucketName":"other","objectName":"中文 a.tar.gz"}"#.into(),
            false,
        ),
        (
            200,
            r#"{"bucketName":"dst","objectName":"other"}"#.into(),
            false,
        ),
        (200, good[..good.len() - 1].into(), false),
        (200, format!("{good}{{}}"), false),
        (200, format!("{good}{}", " ".repeat(65536)), false),
        (201, good.into(), false),
        (503, "unavailable".into(), false),
    ];
    for (status, body, succeeds) in cases {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("PUT", "/dst/%E4%B8%AD%E6%96%87%20a.tar.gz")
            .match_query("cp=")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"srcBucketName":"src","srcObjectName":"source"}),
            ))
            .with_status(status)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let result = CopyObjectRequest::new("src", "source", "dst", "中文 a.tar.gz")
            .send_with_response(&client(server.url()))
            .await;
        if succeeds {
            let receipt = result.unwrap();
            assert_eq!(receipt.bucket_name, "dst");
            assert_eq!(receipt.object_name, "中文 a.tar.gz");
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::OutcomeUnknown, "{error:?}");
            assert_eq!(error.dispatch(), Dispatch::Sent);
            // Receipt mismatches are detected after decoding and carry no status.
            if status != 200 || error.status().is_some() {
                assert_eq!(error.status().map(|s| s.as_u16()), Some(status as u16));
            }
            assert!(std::error::Error::source(&error).is_some());
        }
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn legacy_entry_point_validates_body() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("PUT", "/dst/key?cp=")
        .with_body("{}")
        .expect(1)
        .create_async()
        .await;
    let error = CopyObjectRequest::new("src", "key", "dst", "key")
        .send(&client(server.url()))
        .await
        .unwrap_err();
    assert!(error.is_outcome_unknown(), "{error:?}");
    mock.assert_async().await;
}

#[tokio::test]
async fn truncated_transport_and_stalled_body_are_unknown_without_retry() {
    for stall in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = client(format!("http://{}", listener.local_addr().unwrap()));
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"bucketName\":").await.unwrap();
            if stall {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            drop(socket);
            assert!(
                tokio::time::timeout(Duration::from_millis(150), listener.accept())
                    .await
                    .is_err()
            );
        });
        let error = c
            .copy_object_with_response(&CopyObjectRequest::new("src", "key", "dst", "key"))
            .await
            .unwrap_err();
        assert!(error.is_outcome_unknown(), "{error:?}");
        assert_eq!(error.status(), Some(http::StatusCode::OK));
        assert_eq!(error.dispatch(), Dispatch::Sent);
        task.await.unwrap();
    }
}
