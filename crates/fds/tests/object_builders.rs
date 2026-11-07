use chrono::{TimeZone, Utc};
use fds::object::{ByteRange, UploadBody, UserMetadata};
use fds::Client;
use http::Method;
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
    let info: fds::object::ObjectBasicInfo = serde_json::from_str(r#"{"metadataBean":{"rawMeta":{"entry":[{"key":"Content-Type","value":"old"},{"key":"CONTENT-TYPE","value":""},{"key":"X-Xiaomi-Meta-ID","value":"42"}]}}}"#).unwrap();
    let m = info.metadata;
    assert_eq!(m.attributes.content_type.as_deref(), Some(""));
    assert_eq!(m.attributes.cache_control, None);
    assert_eq!(m.user_metadata.get("ID"), Some("42"));
    assert_eq!(m.user_metadata.get("absent"), None);
    assert_eq!(m.entries().count(), 2);
    let mut user = UserMetadata::new();
    user.insert("Key", "");
    assert_eq!(user.get("KEY"), Some(""));
    assert_eq!(user.iter().collect::<Vec<_>>(), [("key", "")]);
}

#[tokio::test]
async fn invalid_names_fail_at_send() {
    let client = client("http://127.0.0.1:1");
    assert!(matches!(
        client.get_object("", "o").send().await,
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput
    ));
    assert!(matches!(
        client.put_object("", "o", vec![]).send().await,
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput
    ));
    assert_send(client.get_object("b", "o").send());
    assert_send(client.put_object("b", "o", vec![]).send());
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
    let mut md = UserMetadata::new();
    md.insert("ID", "42");
    assert_eq!(
        client
            .put_object("b", "a/b +", b"hello".to_vec())
            .user_metadata(md)
            .content_type("text/plain")
            .send()
            .await
            .unwrap()
            .object_name,
        "a/b +"
    );
    let output = client
        .get_object("b", "a/b +")
        .range(ByteRange::inclusive(0, 4).unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(output.status(), http::StatusCode::PARTIAL_CONTENT);
    assert!(output.is_partial());
    assert_eq!(output.headers().get("Content-Range"), Some("bytes 0-4/10"));
    assert_eq!(
        output.metadata().attributes.content_type.as_deref(),
        Some("text/plain")
    );
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
    let mut output = client.get_object("b", "o").send().await.unwrap();
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
async fn output_preserves_errors_and_body_length() {
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
    let err = client.get_object("b", "error").send().await.unwrap_err();
    assert_eq!(err.status(), Some(http::StatusCode::FORBIDDEN));
    assert_eq!(err.kind(), fds::ErrorKind::PermissionDenied);
    let output = client.get_object("b", "ok").send().await.unwrap();
    // The idle-timed response body keeps the response's length.
    assert_eq!(output.content_length(), Some(2));
    assert!(!output.is_partial());
    assert_eq!(output.bytes().await.unwrap(), "ok");
    error.assert_async().await;
    ok.assert_async().await;
}

#[test]
fn presign_builder_keeps_milliseconds_and_validates_lifetimes() {
    let client = client("https://cnbj1.fds.api.xiaomi.com");
    let expiry = Utc.timestamp_millis_opt(1_800_000_000_123).unwrap();
    let url = client
        .presign_object("b", "a/b +")
        .method(Method::HEAD)
        .expires_at(expiry)
        .build()
        .unwrap()
        .to_string();
    assert!(url.contains("Expires=1800000000123"));
    assert!(url.contains("metadata="));
    for ttl in [Duration::ZERO, Duration::from_nanos(1), Duration::MAX] {
        let error = client
            .presign_object("b", "o")
            .expires_in(ttl)
            .build()
            .unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
    }
    let missing = client.presign_object("b", "o").build().unwrap_err();
    assert_eq!(missing.kind(), fds::ErrorKind::InvalidInput);
    // A relative lifetime is resolved when `build` runs.
    let before = Utc::now().timestamp_millis();
    let url = client
        .presign_object("b", "o")
        .expires_in(Duration::from_secs(60))
        .build()
        .unwrap();
    let after = Utc::now().timestamp_millis();
    let expires: i64 = url
        .as_str()
        .split("Expires=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .parse()
        .unwrap();
    assert!((before + 60_000..=after + 60_000).contains(&expires));
    // The CDN host requires a configured CDN endpoint.
    let error = client
        .presign_object("b", "o")
        .expires_at(expiry)
        .cdn()
        .build()
        .unwrap_err();
    assert_eq!(error.kind(), fds::ErrorKind::ConfigInvalid);
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
    let mut output = client.get_object("b", "o").send().await.unwrap();
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
    let mut output = client.get_object("b", "o").send().await.unwrap();
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
    client(server.url())
        .set_object_acl("b", "o", fds::acl::AccessControlList::default())
        .version_id("v1")
        .send()
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
    let output = client.get_object("b", "o").send().await.unwrap();
    assert_eq!(output.copy_to(&mut destination).await.unwrap(), 5);
    assert_eq!(destination, b"prefix:hello");

    let mut output = client.get_object("b", "o").send().await.unwrap();
    let consumed = output.chunk().await.unwrap().unwrap();
    let mut remaining = Vec::new();
    let written = output.copy_to(&mut remaining).await.unwrap();
    assert_eq!(written as usize, 5 - consumed.len());
    assert_eq!([consumed.as_ref(), &remaining].concat(), b"hello");

    let output = client.get_object("b", "o").send().await.unwrap();
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
    let body = UploadBody::from_file(&path).await.unwrap();
    let result = client(server.url())
        .put_object("b", "o", body)
        .content_type("text/plain")
        .send()
        .await;
    tokio::fs::remove_file(&path).await.unwrap();
    assert_eq!(result.unwrap().object_name, "o");
    assert!(matches!(
        UploadBody::from_file(&path).await,
        Err(e) if e.kind() == fds::ErrorKind::Io
    ));
    stub.assert_async().await;
}

#[tokio::test]
async fn multipart_helpers_bind_identity_and_preserve_receipts() {
    use std::num::NonZeroU16;
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
    let session = client
        .create_multipart_upload("b", "a/b")
        .send()
        .await
        .unwrap();
    assert_eq!(
        (session.bucket(), session.object(), session.upload_id()),
        ("b", "a/b", "id")
    );
    let receipt = session
        .upload_part(NonZeroU16::MIN, b"hello".to_vec())
        .send()
        .await
        .unwrap();
    let second = session
        .upload_part(NonZeroU16::new(2).unwrap(), b"world".to_vec())
        .send()
        .await
        .unwrap();
    let output = session
        .complete(vec![receipt, second])
        .send()
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
        client.head_object("", "o").send().await.map(|_| ()),
        client
            .copy_object(("b", ""), ("b", "o"))
            .send()
            .await
            .map(|_| ()),
        client.put_object("b", "", vec![]).send().await.map(|_| ()),
        client.list_objects("").send().await.map(|_| ()),
        client
            .upload_part("b", "o", "", std::num::NonZeroU16::MIN, vec![])
            .send()
            .await
            .map(|_| ()),
        client.abort_multipart_upload("b", "o", "").send().await,
    ] {
        assert!(matches!(result, Err(e) if e.kind() == fds::ErrorKind::InvalidInput));
    }
}

#[tokio::test]
async fn every_object_builder_validates_names_and_timeout_before_sending() {
    // Nothing listens here: any request that is sent fails with Transport.
    let client = client("http://127.0.0.1:1");
    let acl = fds::acl::AccessControlList::default;
    for (bucket, object, timeout) in [
        ("", "o", Duration::from_secs(1)),
        ("b", "", Duration::from_secs(1)),
        ("b", "o", Duration::ZERO),
    ] {
        let t = timeout;
        let results = [
            client
                .get_object(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .head_object(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .object_exists(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .put_object(bucket, object, vec![])
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .copy_object((bucket, object), ("b", "o"))
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .copy_object(("b", "o"), (bucket, object))
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .rename_object(bucket, object, "o")
                .timeout(t)
                .send()
                .await,
            client
                .rename_object(bucket, "o", object)
                .timeout(t)
                .send()
                .await,
            client
                .restore_object(bucket, object)
                .timeout(t)
                .send()
                .await,
            client
                .prefetch_object(bucket, object)
                .timeout(t)
                .send()
                .await,
            client
                .refresh_object(bucket, object)
                .timeout(t)
                .send()
                .await,
            client.delete_object(bucket, object).timeout(t).send().await,
            client
                .get_object_metadata(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .set_object_metadata(bucket, object)
                .timeout(t)
                .send()
                .await,
            client
                .get_object_basic_info(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .get_object_acl(bucket, object)
                .timeout(t)
                .send()
                .await
                .map(drop),
            client
                .set_object_acl(bucket, object, acl())
                .timeout(t)
                .send()
                .await,
            client
                .set_object_public(bucket, object)
                .timeout(t)
                .send()
                .await,
        ];
        for (index, result) in results.into_iter().enumerate() {
            let error = result.unwrap_err();
            assert_eq!(
                (error.kind(), error.dispatch()),
                (fds::ErrorKind::InvalidInput, fds::Dispatch::NotSent),
                "operation #{index} with {bucket:?}/{object:?}/{timeout:?}"
            );
        }
    }
}

/// A user metadata key named like a system header is prefixed and never
/// replaces the typed header; a reserved system key (`storage-class`) is sent
/// as given and read back as an attribute.
#[tokio::test]
async fn user_metadata_and_system_headers_never_conflict() {
    let mut server = mockito::Server::new_async().await;
    let user: UserMetadata = [
        ("Content-Type", "user/type"),
        ("storage-class", "STANDARD_IA"),
        ("owner", "me"),
    ]
    .into_iter()
    .collect();
    let put = server
        .mock("PUT", "/b/o")
        .match_header("content-type", "text/plain")
        .match_header("x-xiaomi-meta-content-type", "user/type")
        .match_header("x-xiaomi-meta-storage-class", "STANDARD_IA")
        .match_header("x-xiaomi-meta-owner", "me")
        .with_body("{}")
        .create_async()
        .await;
    let set = server
        .mock("PUT", "/b/o")
        .match_query("setMetaData=")
        .match_body(mockito::Matcher::Exact(
            r#"{"rawMeta":{"content-type":"text/plain","x-xiaomi-meta-content-type":"user/type","x-xiaomi-meta-owner":"me","x-xiaomi-meta-storage-class":"STANDARD_IA"}}"#.into(),
        ))
        .create_async()
        .await;
    let head = server
        .mock("HEAD", "/b/o")
        .with_header("content-type", "text/plain")
        .with_header("x-xiaomi-meta-content-type", "user/type")
        .with_header("x-xiaomi-meta-storage-class", "STANDARD_IA")
        .with_header("x-xiaomi-meta-owner", "me")
        .create_async()
        .await;
    let client = client(server.url());
    client
        .put_object("b", "o", b"x".to_vec())
        .content_type("text/plain")
        .user_metadata(user.clone())
        .send()
        .await
        .unwrap();
    client
        .set_object_metadata("b", "o")
        .content_type("text/plain")
        .user_metadata(user)
        .send()
        .await
        .unwrap();
    let md = client.head_object("b", "o").send().await.unwrap().unwrap();
    assert_eq!(md.attributes.content_type.as_deref(), Some("text/plain"));
    assert_eq!(
        md.attributes.storage_class.as_ref().map(|c| c.as_str()),
        Some("STANDARD_IA")
    );
    let expected: UserMetadata = [("content-type", "user/type"), ("owner", "me")]
        .into_iter()
        .collect();
    assert_eq!(md.user_metadata, expected);
    put.assert_async().await;
    set.assert_async().await;
    head.assert_async().await;
}

/// Header-borne user metadata is checked at send, before anything is sent;
/// the set-metadata JSON body accepts any string.
#[tokio::test]
async fn user_metadata_is_validated_per_transport_at_send() {
    let mut server = mockito::Server::new_async().await;
    let set = server
        .mock("PUT", "/b/o")
        .match_query("setMetaData=")
        .match_body(mockito::Matcher::Exact(
            r#"{"rawMeta":{"x-xiaomi-meta-name":"line\nbreak"}}"#.into(),
        ))
        .create_async()
        .await;
    let client = client(server.url());
    for user in [[("bad key", "v")], [("name", "line\nbreak")]] {
        let user: UserMetadata = user.into_iter().collect();
        let error = client
            .put_object("b", "o", b"x".to_vec())
            .user_metadata(user.clone())
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
        assert_eq!(error.dispatch(), fds::Dispatch::NotSent);
        let error = client
            .presign_object("b", "o")
            .expires_in(std::time::Duration::from_secs(60))
            .user_metadata(user)
            .build()
            .unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
    }
    client
        .set_object_metadata("b", "o")
        .user_metadata([("name", "line\nbreak")].into_iter().collect())
        .send()
        .await
        .unwrap();
    set.assert_async().await;
}

/// An empty key or one that already carries the prefix (any case) fails
/// every builder that takes user metadata, before anything is sent.
#[tokio::test]
async fn empty_and_prefixed_user_metadata_keys_are_never_sent() {
    let mut server = mockito::Server::new_async().await;
    let any = server
        .mock("PUT", mockito::Matcher::Any)
        .match_query(mockito::Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let client = client(server.url());
    let path = std::env::temp_dir().join(format!("fds-user-metadata-{}.bin", std::process::id()));
    std::fs::write(&path, b"abcdefgh").unwrap();
    let check = |result: fds::Result<()>, builder: &str, key: &str| {
        let error = result.unwrap_err();
        assert_eq!(
            error.kind(),
            fds::ErrorKind::InvalidInput,
            "{builder} {key:?}"
        );
        assert_eq!(
            error.dispatch(),
            fds::Dispatch::NotSent,
            "{builder} {key:?}"
        );
    };
    for key in ["", "x-xiaomi-meta-a", "X-XIAOMI-META-a", "x-xiaomi-meta-"] {
        let user: UserMetadata = [(key, "v")].into_iter().collect();
        let put = client
            .put_object("b", "o", b"x".to_vec())
            .user_metadata(user.clone())
            .send()
            .await;
        check(put.map(drop), "put", key);
        let create = client
            .create_multipart_upload("b", "o")
            .user_metadata(user.clone())
            .send()
            .await;
        check(create.map(drop), "create", key);
        let complete = client
            .complete_multipart_upload("b", "o", "u", [])
            .user_metadata(user.clone())
            .send()
            .await;
        check(complete.map(drop), "complete", key);
        let upload = client
            .upload_file("b", "o", &path)
            .part_size(4)
            .user_metadata(user.clone())
            .start()
            .await;
        check(upload.map(drop), "upload_file", key);
        let set = client
            .set_object_metadata("b", "o")
            .user_metadata(user.clone())
            .send()
            .await;
        check(set, "set_object_metadata", key);
        let presign = client
            .presign_object("b", "o")
            .expires_in(std::time::Duration::from_secs(60))
            .user_metadata(user)
            .build();
        check(presign.map(drop), "presign", key);
    }
    let _ = std::fs::remove_file(&path);
    any.assert_async().await;
}
