use fds::object::{DeleteObjectFailure, DeleteObjectsOutput};
use fds::{Client, Dispatch, ErrorKind};
use mockito::Matcher;
fn client(s: &mockito::ServerGuard) -> Client {
    Client::builder()
        .endpoint(s.url())
        .credentials("AKID", "secret")
        .build()
        .unwrap()
}
fn query(marker: Option<&str>) -> Matcher {
    let mut q = vec![
        Matcher::UrlEncoded("prefix".into(), "p/".into()),
        Matcher::UrlEncoded("delimiter".into(), "".into()),
        Matcher::UrlEncoded("maxKeys".into(), "1000".into()),
    ];
    if let Some(m) = marker {
        q.push(Matcher::UrlEncoded("marker".into(), m.into()));
    }
    Matcher::AllOf(q)
}
fn partial(error: &fds::Error) -> &DeleteObjectsOutput {
    error
        .partial_delete_output()
        .unwrap_or_else(|| panic!("missing partial output: {error:?}"))
}
#[tokio::test]
async fn head_metadata_status_and_encoded_key() {
    for status in [200, 404, 403, 302, 500] {
        let mut s = mockito::Server::new_async().await;
        let m = s
            .mock("HEAD", "/b/p%2Fa%20%2B")
            .match_query(Matcher::Missing)
            .with_status(status)
            .with_header("content-type", "text/plain")
            .with_header("content-length", "42")
            .with_header("x-xiaomi-meta-tag", "value")
            .create_async()
            .await;
        let result = client(&s).head_object("b", "p/a +").send().await;
        match status {
            200 => {
                let md = result.unwrap().unwrap();
                assert_eq!(md.get("content-length"), Some("42"));
                assert_eq!(md.get_content_type(), Some("text/plain"));
                assert_eq!(md.get("x-xiaomi-meta-tag"), Some("value"));
            }
            404 => assert!(result.unwrap().is_none()),
            _ => assert_eq!(
                result.unwrap_err().status().map(|s| s.as_u16()),
                Some(status as u16)
            ),
        }
        m.assert_async().await;
    }
}
#[tokio::test]
async fn bulk_results_are_never_silently_discarded() {
    for body in [
        "[]",
        r#"[{"object_name":"b","error_code":"denied","error_description":"secret"}]"#,
        "",
        "null",
        "{}",
        "[{}]",
        r#"[{"object_name":"other","error_code":"x","error_description":"x"}]"#,
    ] {
        let mut s = mockito::Server::new_async().await;
        let m = s
            .mock("PUT", "/b")
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("deleteObjects".into(), "".into()),
                Matcher::UrlEncoded("enableTrash".into(), "true".into()),
            ]))
            .match_body(r#"["a","b"]"#)
            .with_body(body)
            .create_async()
            .await;
        let r = client(&s)
            .delete_objects("b")
            .keys(["a"])
            .keys(vec![String::from("b")])
            .move_to_trash()
            .send()
            .await;
        if body == "[]" {
            assert_eq!(r.unwrap().deleted, ["a", "b"]);
        } else if body.contains("denied") {
            // Per-object failures are a normal result.
            let output = r.unwrap();
            assert_eq!(output.deleted, ["a"]);
            assert_eq!(output.failures[0].object_name, "b");
            assert_eq!(output.failures[0].error_code, "denied");
            assert_eq!(output.failures[0].error_description, "secret");
            assert!(output.unconfirmed.is_empty());
            let debug = format!("{output:?} {:?}", output.failures);
            assert!(debug.contains("denied"), "{debug}");
            assert!(
                !debug.contains("secret") && !debug.contains("\"b\""),
                "{debug}"
            );
        } else {
            let e = r.unwrap_err();
            let output = partial(&e);
            assert_eq!(e.kind(), ErrorKind::OutcomeUnknown, "{e:?}");
            assert_eq!(output.unconfirmed, ["a", "b"]);
            let source = std::error::Error::source(&e).unwrap();
            assert_eq!(
                source.downcast_ref::<fds::Error>().unwrap().kind(),
                ErrorKind::ProtocolViolation
            );
        }
        m.assert_async().await;
    }
}
#[tokio::test]
async fn empty_batch_sends_nothing() {
    let mut s = mockito::Server::new_async().await;
    let m = s.mock("PUT", Matcher::Any).expect(0).create_async().await;
    let c = client(&s);
    assert!(c
        .delete_objects("b")
        .send()
        .await
        .unwrap()
        .deleted
        .is_empty());
    assert!(c
        .delete_objects("b")
        .keys(Vec::<String>::new())
        .move_to_trash()
        .send()
        .await
        .unwrap()
        .deleted
        .is_empty());
    m.assert_async().await;
}
#[tokio::test]
async fn prefix_keeps_progress_on_later_delete_or_list_failure() {
    for fail_list in [false, true] {
        let mut s = mockito::Server::new_async().await;
        let first = s
            .mock("GET", "/b")
            .match_query(query(None))
            .with_body(r#"{"objects":[{"name":"p/a"}],"truncated":true,"nextMarker":"m"}"#)
            .create_async()
            .await;
        let delete = s
            .mock("PUT", "/b")
            .match_query(Matcher::Any)
            .match_body(r#"["p/a"]"#)
            .with_body("[]")
            .create_async()
            .await;
        let next = s
            .mock("GET", "/b")
            .match_query(query(Some("m")))
            .with_status(if fail_list { 500 } else { 200 })
            .with_body(r#"{"objects":[{"name":"p/b"}]}"#)
            .create_async()
            .await;
        let fail = s
            .mock("PUT", "/b")
            .match_query(Matcher::Any)
            .match_body(r#"["p/b"]"#)
            .with_status(500)
            .expect(if fail_list { 0 } else { 1 })
            .create_async()
            .await;
        let e = client(&s)
            .delete_prefix("b", "p/")
            .send()
            .await
            .unwrap_err();
        assert_eq!(partial(&e).deleted, ["p/a"]);
        assert_eq!(partial(&e).unconfirmed.len(), if fail_list { 0 } else { 1 });
        assert_eq!(e.status(), Some(http::StatusCode::INTERNAL_SERVER_ERROR));
        assert_eq!(
            e.kind(),
            if fail_list {
                ErrorKind::ServerError
            } else {
                ErrorKind::OutcomeUnknown
            }
        );
        first.assert_async().await;
        delete.assert_async().await;
        next.assert_async().await;
        fail.assert_async().await;
    }
}
#[tokio::test]
async fn prefix_empty_page_continues_and_partial_failure_stops() {
    let mut s = mockito::Server::new_async().await;
    let first = s
        .mock("GET", "/b")
        .match_query(query(None))
        .with_body(r#"{"objects":[],"truncated":true,"nextMarker":"m"}"#)
        .create_async()
        .await;
    let next = s
        .mock("GET", "/b")
        .match_query(query(Some("m")))
        .with_body(
            r#"{"objects":[{"name":"p/a"},{"name":"p/b"}],"truncated":true,"nextMarker":"n"}"#,
        )
        .create_async()
        .await;
    let delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/a","p/b"]"#)
        .with_body(r#"[{"object_name":"p/b","error_code":"denied","error_description":"no"}]"#)
        .create_async()
        .await;
    let later = s
        .mock("GET", "/b")
        .match_query(query(Some("n")))
        .expect(0)
        .create_async()
        .await;
    let e = client(&s)
        .delete_prefix("b", "p/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::PartialFailure);
    assert_eq!(partial(&e).deleted, ["p/a"]);
    assert_eq!(partial(&e).failures[0].object_name, "p/b");
    first.assert_async().await;
    next.assert_async().await;
    delete.assert_async().await;
    later.assert_async().await;
}
#[tokio::test]
async fn prefix_rejects_missing_marker_and_out_of_scope_names_before_deleting() {
    for body in [
        r#"{"objects":[{"name":"p/a"}],"truncated":true}"#,
        r#"{"objects":[{"name":"other"}]}"#,
    ] {
        let mut s = mockito::Server::new_async().await;
        let list = s
            .mock("GET", "/b")
            .match_query(query(None))
            .with_body(body)
            .create_async()
            .await;
        let delete = s.mock("PUT", Matcher::Any).expect(0).create_async().await;
        let error = client(&s)
            .delete_prefix("b", "p/")
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ProtocolViolation);
        assert!(partial(&error).deleted.is_empty());
        list.assert_async().await;
        delete.assert_async().await;
    }
}

#[tokio::test]
async fn prefix_repeated_marker_stops_before_next_batch() {
    let mut s = mockito::Server::new_async().await;
    let first = s
        .mock("GET", "/b")
        .match_query(query(None))
        .with_body(r#"{"objects":[{"name":"p/a"}],"truncated":true,"nextMarker":"m"}"#)
        .create_async()
        .await;
    let delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/a"]"#)
        .with_body("[]")
        .create_async()
        .await;
    let next = s
        .mock("GET", "/b")
        .match_query(query(Some("m")))
        .with_body(r#"{"objects":[{"name":"p/b"}],"truncated":true,"nextMarker":"m"}"#)
        .create_async()
        .await;
    let no_delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/b"]"#)
        .expect(0)
        .create_async()
        .await;
    let e = client(&s)
        .delete_prefix("b", "p/")
        .send()
        .await
        .unwrap_err();
    assert_eq!(e.kind(), ErrorKind::ProtocolViolation);
    assert_eq!(partial(&e).deleted, ["p/a"]);
    first.assert_async().await;
    delete.assert_async().await;
    next.assert_async().await;
    no_delete.assert_async().await;
}

#[tokio::test]
async fn prefix_success_batches_keep_caller_scope_and_finish() {
    let mut s = mockito::Server::new_async().await;
    let first = s.mock("GET", "/b").match_query(query(None)).with_body(r#"{"name":"wrong","prefix":"wrong","objects":[{"name":"p/a"}],"truncated":true,"nextMarker":"m"}"#).create_async().await;
    let delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/a"]"#)
        .with_body("[]")
        .create_async()
        .await;
    let next = s
        .mock("GET", "/b")
        .match_query(query(Some("m")))
        .with_body(r#"{"objects":[{"name":"p/b"}]}"#)
        .create_async()
        .await;
    let second = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/b"]"#)
        .with_body("[]")
        .create_async()
        .await;
    let r = client(&s).delete_prefix("b", "p/").send().await.unwrap();
    assert_eq!(r.deleted, ["p/a", "p/b"]);
    first.assert_async().await;
    delete.assert_async().await;
    next.assert_async().await;
    second.assert_async().await;
}

#[tokio::test]
async fn local_bulk_validation_retains_batch_without_unknown_outcomes() {
    let mut s = mockito::Server::new_async().await;
    let no_put = s.mock("PUT", Matcher::Any).expect(0).create_async().await;
    let c = client(&s);
    // Empty collection is a no-op, including when no target was supplied.
    assert!(c
        .delete_objects("")
        .send()
        .await
        .unwrap()
        .deleted
        .is_empty());
    for (bucket, names) in [("", vec!["a"]), ("b", vec!["a", ""])] {
        let error = c
            .delete_objects(bucket)
            .keys(names)
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput);
        assert_eq!(error.dispatch(), Dispatch::NotSent);
        assert!(error.partial_delete_output().is_none());
    }
    no_put.assert_async().await;
}

#[tokio::test]
async fn bulk_duplicates_preserve_input_and_receipt_order() {
    for body in [
        "[]",
        r#"[{"object_name":"a","error_code":"x","error_description":"x"},{"object_name":"a","error_code":"y","error_description":"y"}]"#,
    ] {
        let mut s = mockito::Server::new_async().await;
        let sent = s
            .mock("PUT", "/b")
            .match_query(Matcher::Any)
            .match_body(r#"["b","a","b","a"]"#)
            .with_body(body)
            .create_async()
            .await;
        let result = client(&s)
            .delete_objects("b")
            .keys(["b", "a", "b", "a"])
            .send()
            .await;
        if body == "[]" {
            assert_eq!(result.unwrap().deleted, ["b", "a", "b", "a"]);
        } else {
            let output = result.unwrap();
            assert_eq!(output.deleted, ["b", "b"]);
            assert_eq!(
                output
                    .failures
                    .iter()
                    .map(|f| f.error_code.as_str())
                    .collect::<Vec<_>>(),
                ["x", "y"]
            );
            assert!(output.unconfirmed.is_empty());
        }
        sent.assert_async().await;
    }
}

#[tokio::test]
async fn prefix_long_cycle_is_checked_before_delete() {
    // After a, b, c, d the bounded (Brent) detector of `into_pages` remembers
    // only d and c, so it would accept the repeated b; deletion remembers
    // every marker and must stop before submitting p/5.
    let mut s = mockito::Server::new_async().await;
    let c = client(&s);
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    for (marker, next, name) in [
        (None, "a", "p/1"),
        (Some("a"), "b", "p/2"),
        (Some("b"), "c", "p/3"),
        (Some("c"), "d", "p/4"),
        (Some("d"), "b", "p/5"),
    ] {
        let query = match marker {
            Some(marker) => format!("delimiter=&marker={marker}&maxKeys=7&prefix=p%2F"),
            None => "delimiter=&maxKeys=7&prefix=p%2F".to_owned(),
        };
        reads.push(s.mock("GET", "/b").match_query(Matcher::Exact(query))
            .with_body(serde_json::json!({"name":"evil", "prefix":"evil", "delimiter":"evil", "maxKeys":999, "objects":[{"name":name}], "truncated":true, "nextMarker":next}).to_string())
            .create_async().await);
        writes.push(
            s.mock("PUT", "/b")
                .match_query(Matcher::Exact("deleteObjects=&enableTrash=true".into()))
                .match_body(Matcher::Json(serde_json::json!([name])))
                .with_body("[]")
                .expect(if name == "p/5" { 0 } else { 1 })
                .create_async()
                .await,
        );
    }
    let error = c
        .delete_prefix("b", "p/")
        .page_size(std::num::NonZeroU32::new(7).unwrap())
        .move_to_trash()
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ProtocolViolation);
    assert_eq!(partial(&error).deleted, ["p/1", "p/2", "p/3", "p/4"]);
    assert!(partial(&error).unconfirmed.is_empty());
    for m in reads.into_iter().chain(writes) {
        m.assert_async().await;
    }
}

#[tokio::test]
async fn prefix_marker_and_delimiter_narrow_the_run_and_guard_the_start() {
    // The starting marker is sent and remembered: a page that leads back to
    // it stops the run before that page is deleted. Common prefixes are not
    // deleted, and every listing keeps the caller's delimiter.
    let mut s = mockito::Server::new_async().await;
    let first = s
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=%2F&marker=start&maxKeys=7&prefix=p%2F".into(),
        ))
        .with_body(r#"{"delimiter":"evil","objects":[{"name":"p/1"}],"commonPrefixes":["p/d/"],"truncated":true,"nextMarker":"a"}"#)
        .create_async()
        .await;
    let delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Exact("deleteObjects=&enableTrash=true".into()))
        .match_body(r#"["p/1"]"#)
        .with_body("[]")
        .create_async()
        .await;
    let second = s
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=%2F&marker=a&maxKeys=7&prefix=p%2F".into(),
        ))
        .with_body(r#"{"objects":[{"name":"p/2"}],"truncated":true,"nextMarker":"start"}"#)
        .create_async()
        .await;
    let no_delete = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["p/2"]"#)
        .expect(0)
        .create_async()
        .await;
    let error = client(&s)
        .delete_prefix("b", "p/")
        .marker("start")
        .delimiter("/")
        .page_size(std::num::NonZeroU32::new(7).unwrap())
        .move_to_trash()
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ProtocolViolation);
    assert_eq!(partial(&error).deleted, ["p/1"]);
    for m in [first, delete, second, no_delete] {
        m.assert_async().await;
    }
}

#[tokio::test]
async fn prefix_marker_is_sent_only_when_set_even_if_empty() {
    for (marker, query) in [
        (None, "delimiter=&maxKeys=1000&prefix=p%2F"),
        (Some(""), "delimiter=&marker=&maxKeys=1000&prefix=p%2F"),
    ] {
        let mut s = mockito::Server::new_async().await;
        let list = s
            .mock("GET", "/b")
            .match_query(Matcher::Exact(query.into()))
            .with_body(r#"{"objects":[{"name":"p/a"}]}"#)
            .create_async()
            .await;
        let delete = s
            .mock("PUT", "/b")
            .match_query(Matcher::Exact("deleteObjects=&enableTrash=false".into()))
            .match_body(r#"["p/a"]"#)
            .with_body("[]")
            .create_async()
            .await;
        let mut request = client(&s).delete_prefix("b", "p/").delimiter("");
        if let Some(marker) = marker {
            request = request.marker(marker);
        }
        assert_eq!(request.send().await.unwrap().deleted, ["p/a"]);
        list.assert_async().await;
        delete.assert_async().await;
    }
}

#[tokio::test]
async fn empty_prefix_does_not_allow_empty_object_names() {
    let mut s = mockito::Server::new_async().await;
    let list = s
        .mock("GET", "/b")
        .match_query(Matcher::Any)
        .with_body(r#"{"objects":[{"name":""}]}"#)
        .create_async()
        .await;
    let no_delete = s.mock("PUT", Matcher::Any).expect(0).create_async().await;
    let error = client(&s).delete_prefix("b", "").send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ProtocolViolation);
    assert!(partial(&error).unconfirmed.is_empty());
    list.assert_async().await;
    no_delete.assert_async().await;
}

#[tokio::test]
async fn bucket_targets_validate_locally_but_root_lists_remain_valid() {
    use fds::{acl::*, bucket::*};
    let mut s = mockito::Server::new_async().await;
    let c = client(&s);
    let mut forbidden = Vec::new();
    for method in ["GET", "PUT", "HEAD", "DELETE"] {
        forbidden.push(s.mock(method, Matcher::Any).expect(0).create_async().await);
    }
    let invalid = |result: fds::Result<()>| {
        assert_eq!(result.unwrap_err().kind(), fds::ErrorKind::InvalidInput)
    };
    invalid(c.create_bucket("").send().await);
    invalid(c.bucket_exists("").send().await.map(|_| ()));
    invalid(c.delete_bucket("").send().await);
    invalid(c.get_bucket_info("").send().await.map(|_| ()));
    invalid(c.migrate_bucket("", "org", "team").send().await);
    invalid(c.get_bucket_acl("").send().await.map(|_| ()));
    invalid(
        c.set_bucket_acl("", AccessControlList::default())
            .send()
            .await,
    );
    invalid(c.get_lifecycle_config("").send().await.map(|_| ()));
    invalid(
        c.set_lifecycle_config("", LifecycleConfig::default())
            .send()
            .await,
    );
    invalid(
        c.set_lifecycle_rule("", LifecycleRule::default())
            .send()
            .await,
    );
    invalid(c.get_access_log("").send().await.map(|_| ()));
    invalid(c.set_access_log("", AccessLog::default()).send().await);
    assert!(c.delete_prefix("", "").send().await.is_err());
    for m in forbidden {
        m.assert_async().await;
    }
    let root = s
        .mock("GET", "/")
        .match_query(Matcher::Missing)
        .with_body("{}")
        .create_async()
        .await;
    let authorized = s
        .mock("GET", "/")
        .match_query(Matcher::Exact("authorizedBuckets=".into()))
        .with_body("{}")
        .create_async()
        .await;
    c.list_buckets().send().await.unwrap();
    c.list_authorized_buckets().send().await.unwrap();
    root.assert_async().await;
    authorized.assert_async().await;
}

#[tokio::test]
async fn bulk_delete_outcomes_follow_dispatch() {
    // Not sent: nothing listens, the connection is refused.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let refused = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let c = Client::builder()
        .endpoint(refused)
        .credentials("AKID", "secret")
        .build()
        .unwrap();
    let error = c.delete_objects("b").keys(["a"]).send().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Transport);
    assert_eq!(error.dispatch(), Dispatch::NotSent);
    assert!(error.partial_delete_output().is_none());

    let mut s = mockito::Server::new_async().await;
    // Sent, per-object failure: Ok with the failure.
    let partial_failure = s
        .mock("PUT", "/b")
        .match_query(Matcher::Exact("deleteObjects=&enableTrash=false".into()))
        .match_body(r#"["ok","bad"]"#)
        .with_body(r#"[{"object_name":"bad","error_code":"AccessDenied","error_description":"d"}]"#)
        .create_async()
        .await;
    let output = client(&s)
        .delete_objects("b")
        .keys(["ok", "bad"])
        .send()
        .await
        .unwrap();
    assert_eq!(output.deleted, ["ok"]);
    assert_eq!(output.failures.len(), 1);
    assert!(output.unconfirmed.is_empty());
    partial_failure.assert_async().await;

    // Sent, error status: outcome unknown, every name unconfirmed.
    let unknown = s
        .mock("PUT", "/b")
        .match_query(Matcher::Any)
        .match_body(r#"["x","y"]"#)
        .with_status(503)
        .create_async()
        .await;
    let error = client(&s)
        .delete_objects("b")
        .keys(["x", "y"])
        .send()
        .await
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::OutcomeUnknown);
    assert_eq!(error.dispatch(), Dispatch::Sent);
    assert!(!error.is_retryable());
    assert_eq!(partial(&error).unconfirmed, ["x", "y"]);
    assert!(partial(&error).deleted.is_empty());
    unknown.assert_async().await;
}

#[tokio::test]
async fn delete_builders_validate_before_sending() {
    // Nothing listens here: any request that is sent fails with Transport.
    let c = Client::builder()
        .endpoint("http://127.0.0.1:1")
        .credentials("AKID", "secret")
        .build()
        .unwrap();
    let zero = std::time::Duration::ZERO;
    let second = std::time::Duration::from_secs(1);
    for (bucket, key, timeout) in [("", "a", second), ("b", "", second), ("b", "a", zero)] {
        let error = c
            .delete_objects(bucket)
            .keys([key])
            .timeout(timeout)
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput, "{error:?}");
        assert_eq!(error.dispatch(), Dispatch::NotSent);
        assert!(error.partial_delete_output().is_none());
    }
    for (bucket, timeout) in [("", second), ("b", zero)] {
        let error = c
            .delete_prefix(bucket, "p/")
            .timeout(timeout)
            .send()
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidInput, "{error:?}");
        assert_eq!(error.dispatch(), Dispatch::NotSent);
        let output = partial(&error);
        assert!(output.deleted.is_empty() && output.failures.is_empty());
    }
}

#[test]
fn delete_failure_debug_shows_only_plain_error_codes() {
    let failure: DeleteObjectFailure = serde_json::from_value(serde_json::json!({
        "object_name": "private/name",
        "error_code": "AccessDenied",
        "error_description": "private description",
    }))
    .unwrap();
    let debug = format!("{failure:?}");
    assert_eq!(
        debug,
        r#"DeleteObjectFailure { error_code: "AccessDenied", .. }"#
    );
    let failure: DeleteObjectFailure = serde_json::from_value(serde_json::json!({
        "object_name": "n",
        "error_code": "denied for private/name",
        "error_description": "",
    }))
    .unwrap();
    let debug = format!("{failure:?}");
    assert!(
        debug.contains("<withheld>") && !debug.contains("private"),
        "{debug}"
    );
}
