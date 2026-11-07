//! The unified error model: status mapping, kinds, dispatch and redaction.
use fds::{Client, Dispatch, ErrorKind};
use mockito::Matcher;
use std::error::Error as _;

fn client(url: impl Into<String>) -> Client {
    Client::builder()
        .endpoint(url)
        .credentials("private-ak", "private-sk")
        .build()
        .unwrap()
}

/// An endpoint on which nothing listens: connecting is refused.
fn refused_endpoint() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    endpoint
}

fn chain(error: &fds::Error) -> String {
    let mut text = format!("{error} {error:?} {error:#?}");
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    text
}

#[tokio::test]
async fn rejected_statuses_map_to_kinds() {
    let mut server = mockito::Server::new_async().await;
    let c = client(server.url());
    for (status, kind) in [
        (404, ErrorKind::NotFound),
        (401, ErrorKind::Unauthenticated),
        (403, ErrorKind::PermissionDenied),
        (409, ErrorKind::Conflict),
        (412, ErrorKind::PreconditionFailed),
        (429, ErrorKind::RateLimited),
        (500, ErrorKind::ServerError),
        (503, ErrorKind::ServerError),
        (400, ErrorKind::Unexpected),
    ] {
        let mock = server
            .mock("DELETE", "/b/o")
            .with_status(status)
            .with_body(r#"{"code":"SomeCode"}"#)
            .create_async()
            .await;
        let error = c.delete_object("b", "o").send().await.unwrap_err();
        assert_eq!(error.kind(), kind, "{status}: {error:?}");
        assert_eq!(error.status().map(|s| s.as_u16()), Some(status as u16));
        assert_eq!(error.dispatch(), Dispatch::Sent);
        assert_eq!(error.service_code(), Some("SomeCode"));
        assert_eq!(error.is_not_found(), status == 404);
        assert_eq!(error.is_retryable(), matches!(status, 429 | 503));
        assert!(!error.is_outcome_unknown());
        let display = error.to_string();
        assert!(display.contains(kind.as_str()), "{display}");
        assert!(display.contains(&format!("HTTP {status}")), "{display}");
        assert!(display.contains("service code SomeCode"), "{display}");
        mock.remove_async().await;
    }
}

#[tokio::test]
async fn protocol_violations_are_classified() {
    // Empty and invalid required JSON.
    for body in ["", "{broken"] {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("PUT", "/b/o")
            .with_body(body)
            .create_async()
            .await;
        let error = client(server.url())
            .put_object("b", "o", b"x".to_vec())
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ProtocolViolation, "{error:?}");
        assert_eq!(error.dispatch(), Dispatch::Sent);
        assert_eq!(error.status(), Some(http::StatusCode::OK));
    }
    // A mismatched copy receipt: outcome unknown, caused by a protocol violation.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("PUT", "/dst/key")
        .match_query("cp=")
        .with_body(r#"{"bucketName":"dst","objectName":"other"}"#)
        .create_async()
        .await;
    let error = client(server.url())
        .copy_object(("src", "key"), ("dst", "key"))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_outcome_unknown());
    let cause = error
        .source()
        .unwrap()
        .downcast_ref::<fds::Error>()
        .unwrap();
    assert_eq!(cause.kind(), ErrorKind::ProtocolViolation);
    assert!(cause.message().contains("receipt"));
}

#[tokio::test]
async fn caller_and_configuration_errors_are_distinct() {
    let c = client("http://127.0.0.1:1");
    let error = c.delete_object("", "o").send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    assert!(error.response().is_none());
    for result in [
        Client::builder().credentials("a", "b").build().map(|_| ()),
        Client::builder()
            .endpoint("https://user@host")
            .credentials("a", "b")
            .build()
            .map(|_| ()),
        Client::builder()
            .endpoint("https://host")
            .credentials("", "b")
            .build()
            .map(|_| ()),
        fds::RetryPolicy::new(1000, std::time::Duration::ZERO, std::time::Duration::ZERO)
            .map(|_| ()),
    ] {
        assert_eq!(result.unwrap_err().kind(), ErrorKind::ConfigInvalid);
    }
}

#[tokio::test]
async fn dispatch_tracks_whether_the_request_was_sent() {
    // Local validation: nothing sent.
    let c = client(refused_endpoint());
    let error = c
        .put_object("", "o", b"x".to_vec())
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    // Connection refused: nothing sent, transport kind, retryable.
    let error = c.delete_object("b", "o").send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    assert!(error.is_retryable());
    assert!(error.source().is_some());
    // A refused copy is not outcome-unknown: it never reached the service.
    let error = c
        .copy_object(("src", "k"), ("dst", "k"))
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport, "{error:?}");
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    // A refused bulk deletion submitted nothing.
    let error = c.delete_objects("b").keys(["a"]).send().await.unwrap_err();
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    assert!(error.partial_delete_output().is_none());
    // A received response: sent.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("DELETE", "/b/o")
        .with_status(404)
        .create_async()
        .await;
    let error = client(server.url())
        .delete_object("b", "o")
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.dispatch(), Dispatch::Sent);
    // A failed bulk deletion response: outcome unknown, every name unconfirmed.
    let _d = server
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .with_status(500)
        .create_async()
        .await;
    let error = client(server.url())
        .delete_objects("b")
        .keys(["a", "b"])
        .send()
        .await
        .unwrap_err();
    assert!(error.is_outcome_unknown());
    assert_eq!(error.dispatch(), Dispatch::Sent);
    assert_eq!(
        error.partial_delete_output().unwrap().unconfirmed,
        ["a", "b"]
    );
}

#[tokio::test]
async fn debug_is_diagnostic_and_omits_credentials() {
    // Access-key client, rejected request echoing credentials into the body.
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/b/o")
        .with_status(403)
        .with_header("x-xiaomi-request-id", "private-request-id")
        .with_body(r#"{"code":"AccessDenied","message":"Galaxy-V2 private-ak:private-signature private-sk"}"#)
        .create_async()
        .await;
    let error = client(server.url())
        .get_object("b", "o")
        .send()
        .await
        .unwrap_err();
    let text = chain(&error);
    for expected in ["PermissionDenied", "403", "AccessDenied", "Sent"] {
        assert!(text.contains(expected), "{text}");
    }
    for secret in ["private-", "Galaxy-V2", "Signature"] {
        assert!(!text.contains(secret), "{text}");
    }
    assert!(error
        .response()
        .unwrap()
        .diagnostic_body()
        .contains("private-sk"));

    // Transport failure: the source chain is printed, the signed URL is not.
    let error = client(refused_endpoint())
        .get_object("b", "private-object")
        .send()
        .await
        .unwrap_err();
    let debug = format!("{error:?}");
    assert!(
        debug.contains("Transport") && debug.contains("source: ["),
        "{debug}"
    );
    assert!(error.source().is_some());
    let text = chain(&error);
    for secret in ["private-", "Galaxy-V2"] {
        assert!(!text.contains(secret), "{text}");
    }

    // OAuth: the storage token travels in the query string, never in errors.
    let mut service = mockito::Server::new_async().await;
    let _exchange = service
        .mock("GET", "/")
        .match_query(Matcher::Any)
        .with_body(
            serde_json::json!({
                "token": "private-storage-token",
                "expireTime": (chrono::Utc::now() + chrono::Duration::days(1)).timestamp_millis(),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let token = fds::oauth::FdsServiceClient::new(service.url())
        .unwrap()
        .exchange_storage_token(
            "private-app",
            "private-oauth-app",
            "private-oauth-token",
            fds::oauth::OAuthProvider::QQ,
        )
        .send()
        .await
        .unwrap();
    let oauth = Client::builder()
        .endpoint(refused_endpoint())
        .storage_token(token)
        .build()
        .unwrap();
    let error = oauth
        .get_object("b", "private-object")
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport);
    let text = chain(&error);
    for secret in ["private-", "storageAccessToken", "appId"] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[test]
fn error_stays_small_without_result_large_err() {
    assert_eq!(
        std::mem::size_of::<fds::Error>(),
        std::mem::size_of::<usize>()
    );
}
