//! End-to-end wire tests against a stub HTTP server (mockito). These exercise
//! the full send pipeline — URL construction, header assembly, signing, and
//! status-code handling — which the in-crate unit tests cannot reach.

use fds::{Client, CreateBucketRequest, GetObjectRequest, ListObjectsRequest, PutObjectRequest};
use mockito::Matcher;

/// Build a validated client pointed at an explicit local HTTP endpoint.
fn client_for(server: &mockito::ServerGuard) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}

#[tokio::test]
async fn put_object_signs_and_sends_body() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PUT", "/b/o.txt")
        .match_header(
            "authorization",
            Matcher::Regex(r"^Galaxy-V2 AKID:.+$".to_string()),
        )
        .match_header("date", Matcher::Regex(r".+GMT$".to_string()))
        .match_body(Matcher::Exact("hello world".to_string()))
        .with_status(200)
        .with_body(r#"{"bucketName":"b","objectName":"o.txt"}"#)
        .create_async()
        .await;

    let resp = client_for(&server)
        .put_object(PutObjectRequest::new("b", "o.txt", b"hello world".to_vec()))
        .await
        .unwrap();

    assert_eq!(resp.bucket_name, "b");
    assert_eq!(resp.object_name, "o.txt");
    m.assert_async().await;
}

#[tokio::test]
async fn get_object_range_explicitly_accepts_206() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("GET", "/b/o")
        .match_header("range", "bytes=0-4")
        .with_status(206)
        .with_body("hello")
        .create_async()
        .await;

    let bytes = client_for(&server)
        .get_object(&GetObjectRequest {
            bucket_name: "b".into(),
            object_name: "o".into(),
            range: Some("bytes=0-4".parse().unwrap()),
            timeout: None,
        })
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    assert_eq!(&bytes[..], b"hello");
    m.assert_async().await;
}

#[tokio::test]
async fn does_object_exist_maps_status() {
    let mut server = mockito::Server::new_async().await;
    let ok = server
        .mock("HEAD", "/b/present")
        .with_status(200)
        .create_async()
        .await;
    let missing = server
        .mock("HEAD", "/b/missing")
        .with_status(404)
        .create_async()
        .await;

    let c = client_for(&server);
    assert!(c.does_object_exist("b", "present").await.unwrap());
    assert!(!c.does_object_exist("b", "missing").await.unwrap());
    ok.assert_async().await;
    missing.assert_async().await;
}

#[tokio::test]
async fn list_objects_encodes_query_and_parses() {
    let mut server = mockito::Server::new_async().await;
    // Query is sorted: delimiter=&maxKeys=1000&prefix=logs%2F
    let m = server
        .mock("GET", "/mybucket")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("prefix".into(), "logs/".into()),
            Matcher::UrlEncoded("maxKeys".into(), "1000".into()),
            Matcher::UrlEncoded("delimiter".into(), "".into()),
        ]))
        .with_status(200)
        .with_body(
            r#"{"name":"mybucket","truncated":false,"objects":[{"name":"logs/a.txt","size":3}]}"#,
        )
        .create_async()
        .await;

    let listing = client_for(&server)
        .list_objects(&ListObjectsRequest {
            bucket_name: "mybucket".into(),
            prefix: "logs/".into(),
            delimiter: String::new(),
            max_keys: 1000,
            marker: None,
        })
        .await
        .unwrap();

    assert_eq!(listing.bucket_name, "mybucket");
    assert_eq!(listing.object_summaries.len(), 1);
    assert_eq!(listing.object_summaries[0].object_name, "logs/a.txt");
    m.assert_async().await;
}

#[tokio::test]
async fn create_bucket_sends_empty_put() {
    let mut server = mockito::Server::new_async().await;
    let m = server
        .mock("PUT", "/newbucket")
        .match_body(Matcher::Exact(String::new()))
        .with_status(200)
        .create_async()
        .await;

    client_for(&server)
        .create_bucket(&CreateBucketRequest {
            bucket_name: "newbucket".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    m.assert_async().await;
}

#[tokio::test]
async fn error_status_carries_body_and_code() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/b/o")
        .with_status(403)
        .with_body(r#"{"code":403,"message":"denied"}"#)
        .create_async()
        .await;

    let err = client_for(&server)
        .get_object(&GetObjectRequest {
            bucket_name: "b".into(),
            object_name: "o".into(),
            range: None,
            timeout: None,
        })
        .await
        .unwrap_err();

    assert_eq!(err.status(), Some(403));
    assert!(err
        .response()
        .map(fds::ResponseError::diagnostic_body)
        .unwrap()
        .contains("denied"));
}

#[tokio::test]
async fn object_name_with_slash_is_percent_encoded_on_the_wire() {
    let mut server = mockito::Server::new_async().await;
    // Object "dir/sub/file.log" → request line path /b/dir%2Fsub%2Ffile.log
    let m = server
        .mock("GET", "/b/dir%2Fsub%2Ffile.log")
        .with_status(200)
        .with_body("x")
        .create_async()
        .await;

    let bytes = client_for(&server)
        .get_object(&GetObjectRequest {
            bucket_name: "b".into(),
            object_name: "dir/sub/file.log".into(),
            range: None,
            timeout: None,
        })
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"x");
    m.assert_async().await;
}

#[tokio::test]
async fn explicit_status_matrix() {
    for status in [
        200, 201, 202, 204, 206, 300, 301, 302, 303, 304, 307, 308, 400, 403, 404, 429, 500, 505,
        506, 599,
    ] {
        let mut server = mockito::Server::new_async().await;
        let c = client_for(&server);
        let get = server
            .mock("GET", "/b/o")
            .with_status(status)
            .with_body("payload")
            .create_async()
            .await;
        let result = c
            .get_object(&GetObjectRequest {
                bucket_name: "b".into(),
                object_name: "o".into(),
                range: None,
                timeout: None,
            })
            .await;
        assert_eq!(result.is_ok(), status == 200, "GET {status}");
        if let Err(e) = result {
            assert_eq!(e.status(), Some(status as u16));
        }
        get.assert_async().await;
        get.remove_async().await;
        let range = server
            .mock("GET", "/b/o")
            .with_status(status)
            .with_body("payload")
            .create_async()
            .await;
        let result = c
            .get_object(&GetObjectRequest {
                bucket_name: "b".into(),
                object_name: "o".into(),
                range: Some("bytes=0-2".parse().unwrap()),
                timeout: None,
            })
            .await;
        assert_eq!(
            result.is_ok(),
            [200, 206].contains(&status),
            "Range {status}"
        );
        range.assert_async().await;
        let head = server
            .mock("HEAD", "/b/o")
            .with_status(status)
            .create_async()
            .await;
        let result = c.does_object_exist("b", "o").await;
        match status {
            200 => assert!(result.unwrap()),
            404 => assert!(!result.unwrap()),
            _ => assert_eq!(result.unwrap_err().status(), Some(status as u16)),
        }
        head.assert_async().await;
        let bucket = server
            .mock("HEAD", "/b")
            .with_status(status)
            .create_async()
            .await;
        let result = c.does_bucket_exist("b").await;
        assert_eq!(
            result.is_ok(),
            [200, 404].contains(&status),
            "bucket HEAD {status}"
        );
        bucket.assert_async().await;
        let delete = server
            .mock("DELETE", "/b/o")
            .with_status(status)
            .create_async()
            .await;
        assert_eq!(
            c.delete_object("b", "o").await.is_ok(),
            status == 200,
            "DELETE {status}"
        );
        delete.assert_async().await;
        let put = server
            .mock("PUT", "/b/o")
            .with_status(status)
            .with_body("{}")
            .create_async()
            .await;
        assert_eq!(
            c.put_object(PutObjectRequest::new("b", "o", Vec::new()))
                .await
                .is_ok(),
            status == 200,
            "JSON {status}"
        );
        put.assert_async().await;
    }
}

#[tokio::test]
async fn required_json_has_explicit_protocol_errors() {
    use fds::ResponseErrorKind;
    use std::error::Error;
    for (body, kind) in [
        ("", ResponseErrorKind::EmptyJson),
        ("  ", ResponseErrorKind::InvalidJson),
        ("{broken", ResponseErrorKind::InvalidJson),
    ] {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("PUT", "/b/o")
            .with_status(200)
            .with_header("x-xiaomi-request-id", "trace-123")
            .with_body(body)
            .create_async()
            .await;
        let error = client_for(&server)
            .put_object(PutObjectRequest::new("b", "o", Vec::new()))
            .await
            .unwrap_err();
        assert_eq!(error.status(), Some(200));
        assert_eq!(error.request_id(), Some("trace-123"));
        assert_eq!(error.response().unwrap().kind(), kind);
        assert_eq!(
            error.response().unwrap().source().is_some(),
            kind == ResponseErrorKind::InvalidJson
        );
        m.assert_async().await;
    }
}

#[tokio::test]
async fn signed_redirects_never_follow_same_or_cross_origin() {
    for status in [301, 302, 303, 307, 308] {
        for cross_origin in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let mut other = mockito::Server::new_async().await;
            let destination = if cross_origin {
                &mut other
            } else {
                &mut server
            };
            let url = format!(
                "{}/destination?storageAccessToken=secret&Signature=signed",
                destination.url()
            );
            let target = destination
                .mock("GET", "/destination")
                .match_query(Matcher::Any)
                .expect(0)
                .create_async()
                .await;
            let redirect = server
                .mock("GET", "/b/o")
                .with_status(status)
                .with_header("location", &url)
                .create_async()
                .await;
            let error = client_for(&server)
                .get_object(&GetObjectRequest {
                    bucket_name: "b".into(),
                    object_name: "o".into(),
                    range: None,
                    timeout: None,
                })
                .await
                .unwrap_err();
            assert_eq!(error.status(), Some(status as u16));
            assert!(!format!("{error} {error:?}").contains("secret"));
            target.assert_async().await;
            redirect.assert_async().await;
        }
    }
}

#[tokio::test]
async fn diagnostics_are_bounded_and_formatting_omits_untrusted_strings() {
    let secret =
        "Galaxy-V2 AKID:credential https://host/o?storageAccessToken=oauth&Signature=signed";
    for (body, truncated) in [
        (
            serde_json::json!({"code":"AccessDenied", "requestId":"body-id", "message":secret})
                .to_string(),
            false,
        ),
        (secret.repeat(200), true),
        ("雪".repeat(4000), true),
    ] {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/b/o")
            .with_status(506)
            .with_header("x-xiaomi-request-id", "header-id")
            .with_body(&body)
            .create_async()
            .await;
        let error = client_for(&server)
            .get_object(&GetObjectRequest {
                bucket_name: "b".into(),
                object_name: "o".into(),
                range: None,
                timeout: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.status(), Some(506));
        assert_eq!(error.request_id(), Some("header-id"));
        assert_eq!(
            error.service_code(),
            if truncated {
                None
            } else {
                Some("AccessDenied")
            }
        );
        let response = error.response().unwrap();
        assert_eq!(response.body_truncated(), truncated);
        assert!(response.diagnostic_body().len() <= 8192);
        let formatted = format!("{error} {error:?} {response} {response:?}");
        for sensitive in ["credential", "oauth", "signed", "header-id", "AccessDenied"] {
            assert!(!formatted.contains(sensitive));
        }
        m.assert_async().await;
    }
}

#[tokio::test]
async fn optional_error_fields_are_best_effort() {
    for (body, code, id) in [
        (
            r#"{"code":123,"requestId":"rid"}"#,
            Some("123"),
            Some("rid"),
        ),
        (r#"{"errorCode":"Denied"}"#, Some("Denied"), None),
        ("not JSON", None, None),
    ] {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("DELETE", "/b/o")
            .with_status(403)
            .with_body(body)
            .create_async()
            .await;
        let e = client_for(&server)
            .delete_object("b", "o")
            .await
            .unwrap_err();
        assert_eq!(e.service_code(), code);
        assert_eq!(e.request_id(), id);
        assert_eq!(
            e.response().map(fds::ResponseError::diagnostic_body),
            Some(body)
        );
        m.assert_async().await;
    }
}

#[tokio::test]
async fn body_read_failure_preserves_status_request_id_and_source() {
    use std::error::Error;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for status in [200, 503] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let serve = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 8192];
            assert!(
                socket.read(&mut request).await.unwrap() > 0,
                "peer closed before sending request bytes"
            );
            socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: 100\r\nx-xiaomi-request-id: read-failure\r\nConnection: close\r\n\r\n{{").as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let e = Client::builder()
            .endpoint(format!("http://{endpoint}"))
            .credentials("ak", "sk")
            .build()
            .unwrap()
            .put_object(PutObjectRequest::new("b", "o", Vec::new()))
            .await
            .unwrap_err();
        assert_eq!(e.status(), Some(status));
        assert_eq!(e.request_id(), Some("read-failure"));
        assert!(e.response().unwrap().source().is_some());
        assert_eq!(
            e.response().unwrap().kind(),
            if status == 200 {
                fds::ResponseErrorKind::BodyRead
            } else {
                fds::ResponseErrorKind::UnexpectedStatus
            }
        );
        serve.await.unwrap();
    }
}

#[tokio::test]
async fn transport_error_conversion_removes_url_from_source() {
    use std::error::Error;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/secret-object?storageAccessToken=private-token&Signature=private-signature",
        listener.local_addr().unwrap()
    );
    drop(listener);
    let raw = reqwest::Client::new().get(url).send().await.unwrap_err();
    assert!(raw.url().is_some());
    let error = fds::FdsError::from(raw);
    let mut diagnostics = format!("{error} {error:?}");
    let mut source = error.source();
    while let Some(e) = source {
        diagnostics.push_str(&format!("{e} {e:?}"));
        source = e.source();
    }
    for secret in [
        "secret-object",
        "private-token",
        "private-signature",
        "storageAccessToken",
        "Signature",
    ] {
        assert!(!diagnostics.contains(secret));
    }
}
