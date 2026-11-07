use chrono::{TimeZone, Utc};
use fds::{
    ByteRange, Client, GeneratePresignedURLRequest, GetObjectRequest, HttpMethod, ObjectMetadata,
    PutObjectRequest,
};
use std::time::Duration;

fn client(endpoint: impl Into<String>) -> Client {
    Client::builder()
        .endpoint(endpoint.into())
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}
fn assert_send<T: Send>(_: T) {}

#[test]
fn ranges_validate_and_format_without_network() {
    for value in [
        "bytes=0-0",
        "bytes=10-",
        "bytes=-5",
        "bytes=0-18446744073709551615",
    ] {
        assert_eq!(value.parse::<ByteRange>().unwrap().to_string(), value);
    }
    for value in [
        "",
        "bytes=-",
        "bytes=-0",
        "bytes=2-1",
        "bytes=+1-2",
        "bytes=0-1,3-4",
        "bytes= 1-2",
        "items=1-2",
        "bytes=0-18446744073709551616",
    ] {
        assert!(value.parse::<ByteRange>().is_err(), "{value}");
    }
    assert!(ByteRange::inclusive(1, 0).is_err());
    assert!(ByteRange::suffix(0).is_err());
}

#[test]
fn metadata_preserves_empty_normalizes_and_borrows() {
    let m: ObjectMetadata = serde_json::from_str(r#"{"rawMeta":{"entry":[{"key":"Content-Type","value":"old"},{"key":"CONTENT-TYPE","value":""},{"key":"X-Xiaomi-Meta-ID","value":"42"}]}}"#).unwrap();
    assert_eq!(m.get("Content-Type"), Some(""));
    assert_eq!(m.get("absent"), None);
    assert_eq!(m.get("x-XIAOMI-meta-id"), Some("42"));
    assert_eq!(m.iter().count(), 2);
    let json: serde_json::Value = serde_json::from_slice(&m.serialize().unwrap()).unwrap();
    assert_eq!(json["rawMeta"]["content-type"], "");
    assert!(json["rawMeta"].get("Content-Type").is_none());
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("content-type", "".parse().unwrap());
    let from_headers = ObjectMetadata::from_headers(&headers);
    assert_eq!(from_headers.get("CONTENT-TYPE"), Some(""));
    assert_eq!(from_headers.get("x-xiaomi-meta-id"), None);
}

#[tokio::test]
async fn invalid_names_fail_at_both_entry_points() {
    let client = client("http://127.0.0.1:1");
    assert!(matches!(
        client.get_object(&GetObjectRequest::new("", "o")).await,
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput
    ));
    assert!(matches!(
        PutObjectRequest::new("", "o", vec![]).send(&client).await,
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput
    ));
    assert_send(GetObjectRequest::new("b", "o").send(&client));
    assert_send(PutObjectRequest::new("b", "o", vec![]).send(&client));
}

#[tokio::test]
async fn builders_preserve_encoded_keys_body_and_metadata() {
    let mut server = mockito::Server::new_async().await;
    let put = server
        .mock("PUT", "/b/a%2Fb%20%2B")
        .match_header("content-type", "text/plain")
        .match_header("x-xiaomi-meta-id", "42")
        .match_header(
            "authorization",
            mockito::Matcher::Regex("^Galaxy-V2 AKID:.+$".into()),
        )
        .match_body("hello")
        .with_status(200)
        .with_body(r#"{"objectName":"a/b +"}"#)
        .create_async()
        .await;
    let get = server
        .mock("GET", "/b/a%2Fb%20%2B")
        .match_header("range", "bytes=0-4")
        .with_status(206)
        .with_header("content-range", "bytes 0-4/10")
        .with_header("content-type", "text/plain")
        .with_body("hello")
        .create_async()
        .await;
    let client = client(server.url());
    let mut md = ObjectMetadata::new();
    md.set("X-Xiaomi-Meta-ID", "42").unwrap();
    assert_eq!(
        PutObjectRequest::new("b", "a/b +", b"hello".to_vec())
            .metadata(md)
            .content_type("text/plain")
            .send(&client)
            .await
            .unwrap()
            .object_name,
        "a/b +"
    );
    let output = GetObjectRequest::new("b", "a/b +")
        .range(ByteRange::inclusive(0, 4).unwrap())
        .send(&client)
        .await
        .unwrap();
    assert_eq!(output.status(), 206);
    assert_eq!(output.headers()["content-range"], "bytes 0-4/10");
    assert_eq!(output.metadata().get("Content-Type"), Some("text/plain"));
    assert_eq!(output.bytes().await.unwrap(), "hello");
    put.assert_async().await;
    get.assert_async().await;
}

#[tokio::test]
async fn streamed_output_does_not_wait_for_remaining_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(format!("http://{}", listener.local_addr().unwrap()));
    let (release, wait) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhello")
            .await
            .unwrap();
        wait.await.unwrap();
        socket.write_all(b"world").await.unwrap();
    });
    let mut output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    let mut prefix = Vec::new();
    while prefix.len() < 5 {
        prefix.extend(
            tokio::time::timeout(Duration::from_secs(2), output.chunk())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(prefix, b"hello");
    release.send(()).unwrap();
    assert_eq!(output.bytes().await.unwrap(), "world");
    server.await.unwrap();
}

#[tokio::test]
async fn output_preserves_errors_and_raw_escape_hatch() {
    let mut server = mockito::Server::new_async().await;
    let error = server
        .mock("GET", "/b/error")
        .with_status(403)
        .with_body("denied")
        .create_async()
        .await;
    let ok = server
        .mock("GET", "/b/ok")
        .with_status(200)
        .with_body("ok")
        .create_async()
        .await;
    let client = client(server.url());
    let err = GetObjectRequest::new("b", "error")
        .send(&client)
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(http::StatusCode::FORBIDDEN));
    assert_eq!(err.kind(), fds::ErrorKind::PermissionDenied);
    let output = GetObjectRequest::new("b", "ok")
        .send(&client)
        .await
        .unwrap();
    // The idle-timed response body keeps the raw response's length and URL.
    let raw = output.into_raw();
    assert_eq!(raw.content_length(), Some(2));
    assert_eq!(raw.url().path(), "/b/ok");
    assert_eq!(raw.bytes().await.unwrap(), "ok");
    error.assert_async().await;
    ok.assert_async().await;
}

#[test]
fn typed_presign_keeps_wire_signature_and_milliseconds() {
    let client = client("https://cnbj1.fds.api.xiaomi.com");
    let expiry = Utc.timestamp_millis_opt(1_800_000_000_123).unwrap();
    let request = GeneratePresignedURLRequest::new("b", "a/b +", HttpMethod::Head, expiry);
    let old = client.generate_presigned_url(&request).unwrap();
    assert_eq!(request.generate(&client).unwrap(), old);
    assert!(old.contains("Expires=1800000000123"));
    assert!(old.contains("metadata="));
    for ttl in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
        assert!(GeneratePresignedURLRequest::expires_in("b", "o", HttpMethod::Get, ttl).is_err());
    }
    let before = Utc::now();
    let request =
        GeneratePresignedURLRequest::expires_in("b", "o", HttpMethod::Get, Duration::from_secs(60))
            .unwrap();
    assert!(request.expiration >= before + chrono::Duration::seconds(60));
    assert!(request.expiration <= Utc::now() + chrono::Duration::seconds(60));
}

#[tokio::test]
async fn chunk_reports_truncated_body_and_eof_is_stable() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = client(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhello")
            .await
            .unwrap();
        socket.shutdown().await.unwrap();
    });
    let mut output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    let error = loop {
        match output.chunk().await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("truncated body must fail"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), fds::ErrorKind::Transport, "{error:?}");
    assert_eq!(error.dispatch(), fds::Dispatch::Sent);
    server.await.unwrap();
    let mut server = mockito::Server::new_async().await;
    let stub = server
        .mock("GET", "/b/o")
        .with_status(200)
        .with_body("")
        .create_async()
        .await;
    let client = self::client(server.url());
    let mut output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    assert!(output.chunk().await.unwrap().is_none());
    assert!(output.chunk().await.unwrap().is_none());
    stub.assert_async().await;
}

#[tokio::test]
async fn acl_builder_keeps_service_default_and_version() {
    let mut server = mockito::Server::new_async().await;
    let stub = server
        .mock("PUT", "/b/o?acl=&versionId=v1")
        .match_body(mockito::Matcher::Json(
            serde_json::json!({"accessControlList":[],"owner":{"id":"","displayName":""}}),
        ))
        .with_status(200)
        .create_async()
        .await;
    fds::SetObjectACLRequest::new("b", "o", fds::AccessControlList::default())
        .version_id("v1")
        .send(&client(server.url()))
        .await
        .unwrap();
    stub.assert_async().await;
}

#[tokio::test]
async fn copy_to_preserves_remaining_body_and_writer_errors() {
    let mut server = mockito::Server::new_async().await;
    let stub = server
        .mock("GET", "/b/o")
        .with_body("hello")
        .expect(3)
        .create_async()
        .await;
    let client = client(server.url());
    let mut destination = b"prefix:".to_vec();
    let output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    assert_eq!(output.copy_to(&mut destination).await.unwrap(), 5);
    assert_eq!(destination, b"prefix:hello");

    let mut output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    let consumed = output.chunk().await.unwrap().unwrap();
    let mut remaining = Vec::new();
    let written = output.copy_to(&mut remaining).await.unwrap();
    assert_eq!(written as usize, 5 - consumed.len());
    assert_eq!([consumed.as_ref(), &remaining].concat(), b"hello");

    let output = GetObjectRequest::new("b", "o").send(&client).await.unwrap();
    let (mut writer, reader) = tokio::io::duplex(1);
    drop(reader);
    assert!(matches!(
        output.copy_to(&mut writer).await,
        Err(e) if e.kind() == fds::ErrorKind::Io
    ));
    stub.assert_async().await;
}

#[tokio::test]
async fn file_upload_helper_keeps_body_and_options() {
    let path = std::env::temp_dir().join(format!(
        "fds-helper-{}-{}.bin",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    tokio::fs::write(&path, b"file content").await.unwrap();
    let mut server = mockito::Server::new_async().await;
    let stub = server
        .mock("PUT", "/b/o")
        .match_header("content-type", "text/plain")
        .match_header("content-length", "12")
        .match_body("file content")
        .with_body(r#"{"objectName":"o"}"#)
        .create_async()
        .await;
    let upload = fds::PutObjectRequest::from_file("b", "o", &path)
        .await
        .unwrap();
    let result = upload
        .content_type("text/plain")
        .send(&client(server.url()))
        .await;
    tokio::fs::remove_file(&path).await.unwrap();
    assert_eq!(result.unwrap().object_name, "o");
    assert!(matches!(
        fds::PutObjectRequest::from_file("b", "o", &path).await,
        Err(e) if e.kind() == fds::ErrorKind::Io
    ));
    stub.assert_async().await;
}

#[tokio::test]
async fn multipart_helpers_bind_identity_and_preserve_receipts() {
    use fds::InitMultipartUploadRequest;
    let mut server = mockito::Server::new_async().await;
    let init = server
        .mock("PUT", "/b/a%2Fb?uploads=")
        .with_body(r#"{"bucketName":"b","objectName":"a/b","uploadId":"id"}"#)
        .create_async()
        .await;
    let part = server
        .mock("PUT", "/b/a%2Fb?partNumber=1&uploadId=id")
        .match_body("hello")
        .with_body(r#"{"partNumber":1,"etag":"receipt","partSize":5}"#)
        .create_async()
        .await;
    let second_part = server
        .mock("PUT", "/b/a%2Fb?partNumber=2&uploadId=id")
        .match_body("world")
        .with_body(r#"{"partNumber":2,"etag":"receipt2","partSize":5}"#)
        .create_async()
        .await;
    let complete = server
        .mock("PUT", "/b/a%2Fb?uploadId=id")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "uploadPartResultList":[{"partNumber":1,"etag":"receipt","partSize":5},{"partNumber":2,"etag":"receipt2","partSize":5}]
        })))
        .with_body(r#"{"objectName":"a/b"}"#)
        .create_async()
        .await;
    let client = client(server.url());
    let session = InitMultipartUploadRequest::new("b", "a/b")
        .send(&client)
        .await
        .unwrap();
    let receipt = session
        .part(1, b"hello".to_vec())
        .send(&client)
        .await
        .unwrap();
    let second = session
        .part(2, b"world".to_vec())
        .send(&client)
        .await
        .unwrap();
    let output = session
        .complete(vec![receipt, second])
        .send(&client)
        .await
        .unwrap();
    assert_eq!(output.object_name, "a/b");
    init.assert_async().await;
    part.assert_async().await;
    second_part.assert_async().await;
    complete.assert_async().await;
}

#[tokio::test]
async fn direct_client_entry_points_validate_before_transport() {
    let client = client("http://127.0.0.1:1");
    for result in [
        client.head_object("", "o").await.map(|_| ()),
        client
            .copy_object(&fds::CopyObjectRequest::new("b", "", "b", "o"))
            .await,
        client
            .put_object(PutObjectRequest::new("b", "", vec![]))
            .await
            .map(|_| ()),
        client
            .list_objects(&fds::ListObjectsRequest::new("b").max_keys(0))
            .await
            .map(|_| ()),
        client
            .upload_part(fds::UploadPartRequest::new("b", "o", "id", 0, vec![]))
            .await
            .map(|_| ()),
        fds::InitMultipartUploadResponse::default()
            .abort(&client)
            .await,
    ] {
        assert!(matches!(result, Err(e) if e.kind() == fds::ErrorKind::InvalidInput));
    }
}
