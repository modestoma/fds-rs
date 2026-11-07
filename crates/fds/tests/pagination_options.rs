use chrono::{TimeZone, Utc};
use fds::acl::{AccessControlList, AclAction, BucketCannedAcl};
use fds::object::ListObjects;
use fds::Client;
use futures_util::{pin_mut, StreamExt};
use mockito::{Matcher, Server};

fn client(server: &Server) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials("AKID", "sk")
        .build()
        .unwrap()
}
fn query(marker: &str) -> Matcher {
    Matcher::AllOf(vec![
        Matcher::UrlEncoded("prefix".into(), "logs/".into()),
        Matcher::UrlEncoded("delimiter".into(), "/".into()),
        Matcher::UrlEncoded("maxKeys".into(), "2".into()),
        Matcher::UrlEncoded("marker".into(), marker.into()),
    ])
}
fn request(c: &Client) -> ListObjects {
    c.list_objects("b")
        .prefix("logs/")
        .delimiter("/")
        .max_keys(std::num::NonZeroU32::new(2).unwrap())
}

#[tokio::test]
async fn lazy_pages_preserve_scope_initial_marker_and_empty_pages() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let first = s.mock("GET", "/b").match_query(query("b/start +"))
        .with_body(r#"{"name":"wrong","prefix":"wrong","delimiter":"!","maxKeys":99,"truncated":true,"nextMarker":"opaque","objects":[],"commonPrefixes":["logs/dir/"]}"#)
        .create_async().await;
    let second = s
        .mock("GET", "/b")
        .match_query(query("opaque"))
        .with_body(r#"{"objects":[{"name":"logs/a"}],"truncated":false}"#)
        .create_async()
        .await;
    let pages = request(&c).marker("b/start +").into_pages();
    pin_mut!(pages);
    assert!(!first.matched_async().await);
    assert_eq!(
        pages.next().await.unwrap().unwrap().common_prefixes,
        ["logs/dir/"]
    );
    assert!(!second.matched_async().await);
    assert_eq!(
        pages.next().await.unwrap().unwrap().object_summaries.len(),
        1
    );
    assert!(pages.next().await.is_none());
    assert!(pages.next().await.is_none());
    first.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn objects_skip_empty_pages_and_do_not_prefetch() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let a = s
        .mock("GET", "/b")
        .match_query(query("start"))
        .with_body(r#"{"truncated":true,"nextMarker":"a"}"#)
        .create_async()
        .await;
    let b = s
        .mock("GET", "/b")
        .match_query(query("a"))
        .with_body(
            r#"{"truncated":true,"nextMarker":"b","objects":[{"name":"one"},{"name":"two"}]}"#,
        )
        .create_async()
        .await;
    let last = s
        .mock("GET", "/b")
        .match_query(query("b"))
        .with_body(r#"{"objects":[{"name":"three"}]}"#)
        .create_async()
        .await;
    let objects = request(&c).marker("start").into_objects();
    pin_mut!(objects);
    assert_eq!(objects.next().await.unwrap().unwrap().object_name, "one");
    assert_eq!(objects.next().await.unwrap().unwrap().object_name, "two");
    assert!(!last.matched_async().await);
    assert_eq!(objects.next().await.unwrap().unwrap().object_name, "three");
    assert!(objects.next().await.is_none());
    a.assert_async().await;
    b.assert_async().await;
    last.assert_async().await;
}

#[tokio::test]
async fn missing_and_repeated_markers_end_with_one_error() {
    for body in [
        r#"{"truncated":true}"#,
        r#"{"truncated":true,"nextMarker":"start"}"#,
    ] {
        let mut s = Server::new_async().await;
        let c = client(&s);
        let m = s
            .mock("GET", "/b")
            .match_query(query("start"))
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let pages = request(&c).marker("start").into_pages();
        pin_mut!(pages);
        assert!(matches!(
            pages.next().await.unwrap(),
            Err(e) if e.kind() == fds::ErrorKind::ProtocolViolation
        ));
        assert!(pages.next().await.is_none());
        m.assert_async().await;
    }
}

#[tokio::test]
async fn opaque_marker_cycle_terminates_in_constant_space() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let a = s
        .mock("GET", "/b")
        .match_query(query("a"))
        .with_body(r#"{"truncated":true,"nextMarker":"b"}"#)
        .expect(2)
        .create_async()
        .await;
    let b = s
        .mock("GET", "/b")
        .match_query(query("b"))
        .with_body(r#"{"truncated":true,"nextMarker":"a"}"#)
        .expect(1)
        .create_async()
        .await;
    let pages = request(&c).marker("a").into_pages();
    pin_mut!(pages);
    assert!(pages.next().await.unwrap().is_ok());
    assert!(pages.next().await.unwrap().is_ok());
    assert!(pages.next().await.unwrap().is_err());
    assert!(pages.next().await.is_none());
    a.assert_async().await;
    b.assert_async().await;
}

#[tokio::test]
async fn service_and_decode_errors_propagate_after_progress() {
    for (status, body) in [(403, "denied"), (200, "invalid json")] {
        let mut s = Server::new_async().await;
        let c = client(&s);
        let first = s
            .mock("GET", "/b")
            .match_query(query("start"))
            .with_body(r#"{"truncated":true,"nextMarker":"a","objects":[{"name":"one"}]}"#)
            .create_async()
            .await;
        let bad = s
            .mock("GET", "/b")
            .match_query(query("a"))
            .with_status(status)
            .with_body(body)
            .create_async()
            .await;
        let objects = request(&c).marker("start").into_objects();
        pin_mut!(objects);
        assert_eq!(objects.next().await.unwrap().unwrap().object_name, "one");
        assert!(objects.next().await.unwrap().is_err());
        assert!(objects.next().await.is_none());
        first.assert_async().await;
        bad.assert_async().await;
    }
}

#[tokio::test]
async fn one_page_initial_marker_and_default_omission() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let initial = s
        .mock("GET", "/b")
        .match_query(query("start"))
        .with_body("{}")
        .create_async()
        .await;
    request(&c).marker("start").send().await.unwrap();
    let default = s
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=%2F&maxKeys=2&prefix=logs%2F".into(),
        ))
        .with_body("{}")
        .create_async()
        .await;
    let pages = request(&c).into_pages();
    pin_mut!(pages);
    pages.next().await.unwrap().unwrap();
    assert!(pages.next().await.is_none());
    initial.assert_async().await;
    default.assert_async().await;
}

#[tokio::test]
async fn typed_acl_add_delete_and_bucket_creation_headers() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let mut acl = AccessControlList::default();
    acl.add_grant(fds::acl::Grant {
        grantee: fds::acl::GrantKey {
            id: "user".into(),
            display_name: String::new(),
        },
        permission: fds::acl::GrantPermission::Read,
        grant_type: fds::acl::GrantType::User,
    });
    let body = serde_json::to_value(&acl).unwrap();
    for (action, query) in [
        (AclAction::Add, "acl="),
        (AclAction::Delete, "acl=&action=delete"),
    ] {
        let bucket = s
            .mock("PUT", "/b")
            .match_query(Matcher::Exact(query.into()))
            .match_body(Matcher::Json(body.clone()))
            .create_async()
            .await;
        c.set_bucket_acl("b", acl.clone())
            .action(action)
            .send()
            .await
            .unwrap();
        bucket.assert_async().await;
        let object = s
            .mock("PUT", "/b/a%2Fb")
            .match_query(Matcher::Exact(format!("{query}&versionId=v")))
            .match_body(Matcher::Json(body.clone()))
            .create_async()
            .await;
        c.set_object_acl("b", "a/b", acl.clone())
            .version_id("v")
            .action(action)
            .send()
            .await
            .unwrap();
        object.assert_async().await;
    }
    let create = s
        .mock("PUT", "/b")
        .match_header("x-xiaomi-meta-acl", "PUBLIC_READ,AUTHENTICATED_WRITE")
        .match_body("")
        .create_async()
        .await;
    c.create_bucket("b")
        .acl([
            BucketCannedAcl::PublicRead,
            BucketCannedAcl::AuthenticatedWrite,
        ])
        .send()
        .await
        .unwrap();
    create.assert_async().await;
    let omitted = s
        .mock("PUT", "/b")
        .match_header("x-xiaomi-meta-acl", Matcher::Missing)
        .create_async()
        .await;
    c.create_bucket("b").send().await.unwrap();
    omitted.assert_async().await;
}

#[tokio::test]
async fn signature_expiry_is_milliseconds_and_omitted_by_default() {
    let mut s = Server::new_async().await;
    let c = client(&s);
    let timestamp = 1_900_000_000_123i64;
    let expires = Utc.timestamp_millis_opt(timestamp).unwrap();
    for expiry in [Some(expires), None] {
        let query = expiry
            .map(|_| format!("expires={timestamp}"))
            .unwrap_or_default();
        let upload = s
            .mock("PUT", "/b/a%2Fb")
            .match_query(Matcher::Exact(query.clone()))
            .match_body("payload")
            .with_body("{}")
            .create_async()
            .await;
        let req = c.put_object("b", "a/b", b"payload".to_vec());
        if let Some(expiry) = expiry {
            req.expires(expiry).send().await.unwrap();
        } else {
            req.send().await.unwrap();
        }
        upload.assert_async().await;
        let query = if query.is_empty() {
            "uploadId=id".into()
        } else {
            format!("{query}&uploadId=id")
        };
        let complete = s
            .mock("PUT", "/b/a%2Fb")
            .match_query(Matcher::Exact(query))
            .match_body(Matcher::Json(
                serde_json::json!({"uploadPartResultList":[]}),
            ))
            .with_body("{}")
            .create_async()
            .await;
        let mut complete_upload = c.complete_multipart_upload("b", "a/b", "id", []);
        if let Some(expiry) = expiry {
            complete_upload = complete_upload.expires(expiry);
        }
        complete_upload.send().await.unwrap();
        complete.assert_async().await;
    }
}

#[tokio::test]
async fn transport_and_local_validation_errors_terminate() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let c = Client::builder()
        .endpoint(endpoint)
        .credentials("AKID", "sk")
        .build()
        .unwrap();
    let pages = c.list_objects("b").into_pages();
    pin_mut!(pages);
    assert!(pages.next().await.unwrap().is_err());
    assert!(pages.next().await.is_none());
    let pages = c.list_objects("").into_pages();
    pin_mut!(pages);
    assert!(matches!(
        pages.next().await.unwrap(),
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput
    ));
    assert!(pages.next().await.is_none());
    for result in [
        c.list_objects("").send().await,
        c.list_objects("b")
            .timeout(std::time::Duration::ZERO)
            .send()
            .await,
    ] {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), fds::ErrorKind::InvalidInput);
        assert_eq!(error.dispatch(), fds::Dispatch::NotSent);
    }
    let pages = c
        .list_objects("b")
        .timeout(std::time::Duration::ZERO)
        .into_objects();
    pin_mut!(pages);
    assert!(matches!(
        pages.next().await.unwrap(),
        Err(e) if e.kind() == fds::ErrorKind::InvalidInput && e.dispatch() == fds::Dispatch::NotSent
    ));
    assert!(pages.next().await.is_none());
}
