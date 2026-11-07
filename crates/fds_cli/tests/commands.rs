use mockito::{Matcher, Server};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        static ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fds-cli-test-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> String {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(endpoint: Option<&str>) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fds_cli"));
    for (key, _) in std::env::vars() {
        if key.starts_with("FDS_") {
            cmd.env_remove(key);
        }
    }
    if let Some(endpoint) = endpoint {
        cmd.env("FDS_ENDPOINT", endpoint)
            .env("FDS_ACCESS_KEY_ID", "id")
            .env("FDS_ACCESS_KEY_SECRET", "secret");
    }
    cmd
}
async fn run(endpoint: &str, args: &[&str]) -> Output {
    let mut cmd = command(Some(endpoint));
    cmd.args(args);
    tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn value(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn help_and_conflicting_input_need_no_credentials() {
    for args in [
        vec!["--help"],
        vec!["multipart", "--help"],
        vec!["object", "put", "--help"],
    ] {
        success(&command(None).args(args).output().unwrap());
    }
    for args in [
        vec!["object", "put", "b", "k"],
        vec!["object", "put", "b", "k", "--file", "x", "--text", "y"],
        vec!["object", "put", "b", "k", "--stdin"],
        vec!["object", "put", "b", "k", "--text", "x", "--length", "1"],
        vec!["object", "delete-many", "b"],
        vec!["object", "presign", "b", "k", "--expires", "0"],
    ] {
        assert_eq!(
            command(None).args(args).output().unwrap().status.code(),
            Some(2)
        );
    }
}

#[tokio::test]
async fn file_and_stdin_uploads_forward_metadata_and_exact_body() {
    let mut server = Server::new_async().await;
    let files = Files::new();
    let path = files.write("source", b"hello");
    let upload = server
        .mock("PUT", "/b/k")
        .match_header("content-length", "5")
        .match_header("content-type", "text/plain")
        .match_header("cache-control", "no-cache")
        .match_header("content-disposition", "attachment")
        .match_body("hello")
        .with_body(r#"{"bucketName":"b","objectName":"k"}"#)
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &[
            "object",
            "put",
            "b",
            "k",
            "--file",
            &path,
            "--content-type",
            "text/plain",
            "--cache-control",
            "no-cache",
            "--content-disposition",
            "attachment",
        ],
    )
    .await;
    success(&result);
    assert_eq!(value(&result)["objectName"], "k");
    upload.assert_async().await;
    let stdin = server
        .mock("PUT", "/b/stream")
        .match_header("content-length", "5")
        .match_body("hello")
        .with_body("{}")
        .create_async()
        .await;
    let mut cmd = command(Some(&server.url()));
    cmd.args(["object", "put", "b", "stream", "--stdin", "--length", "5"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = tokio::task::spawn_blocking(move || {
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(b"hello").unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap();
    success(&result);
    stdin.assert_async().await;
}

#[tokio::test]
async fn acl_actions_versions_and_canned_grants_are_exposed() {
    let mut server = Server::new_async().await;
    let files = Files::new();
    let acl = json!({"owner":{"id":"owner","displayName":""},"accessControlList":[{"grantee":{"id":"user","displayName":""},"permission":"READ","type":"USER"}]});
    let path = files.write("acl.json", acl.to_string());
    let bucket = server
        .mock("PUT", "/b?acl=&action=delete")
        .match_body(Matcher::Json(acl.clone()))
        .create_async()
        .await;
    success(
        &run(
            &server.url(),
            &[
                "bucket", "set-acl", "b", "--file", &path, "--action", "delete",
            ],
        )
        .await,
    );
    bucket.assert_async().await;
    let object = server
        .mock("PUT", "/b/k?acl=&action=delete&versionId=v1")
        .match_body(Matcher::Json(acl))
        .create_async()
        .await;
    success(
        &run(
            &server.url(),
            &[
                "object",
                "set-acl",
                "b",
                "k",
                "--file",
                &path,
                "--action",
                "delete",
                "--version-id",
                "v1",
            ],
        )
        .await,
    );
    object.assert_async().await;
    let create = server
        .mock("PUT", "/new")
        .match_header("x-xiaomi-meta-acl", "PUBLIC_READ,PUBLIC_WRITE")
        .create_async()
        .await;
    success(
        &run(
            &server.url(),
            &[
                "bucket",
                "create",
                "new",
                "--acl",
                "public-read",
                "--acl",
                "public-write",
            ],
        )
        .await,
    );
    create.assert_async().await;
}

#[tokio::test]
async fn cache_and_head_are_distinct_operations() {
    let mut server = Server::new_async().await;
    for operation in ["prefetch", "refresh"] {
        let mock = server
            .mock("PUT", format!("/b/k?{operation}=").as_str())
            .create_async()
            .await;
        let result = run(&server.url(), &["object", operation, "b", "k"]).await;
        success(&result);
        assert_eq!(value(&result)["completion_confirmed"], false);
        mock.assert_async().await;
    }
    let head = server
        .mock("HEAD", "/b/k")
        .with_status(404)
        .create_async()
        .await;
    let result = run(&server.url(), &["object", "head", "b", "k"]).await;
    success(&result);
    assert!(value(&result).is_null());
    head.assert_async().await;
}

#[tokio::test]
async fn pagination_exposes_markers_and_follows_all_pages() {
    let mut server = Server::new_async().await;
    let one = server
        .mock("GET", "/b")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("marker".into(), "start".into()),
            Matcher::UrlEncoded("maxKeys".into(), "2".into()),
        ]))
        .with_body(r#"{"truncated":true,"nextMarker":"next","objects":[{"name":"one"}]}"#)
        .create_async()
        .await;
    let two = server
        .mock("GET", "/b")
        .match_query(Matcher::UrlEncoded("marker".into(), "next".into()))
        .with_body(r#"{"truncated":false,"objects":[{"name":"two"}]}"#)
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &[
            "object",
            "list",
            "b",
            "--marker",
            "start",
            "--max-keys",
            "2",
            "--all",
        ],
    )
    .await;
    success(&result);
    let pages: Vec<Value> = serde_json::Deserializer::from_slice(&result.stdout)
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0]["nextMarker"], "next");
    one.assert_async().await;
    two.assert_async().await;
}

#[tokio::test]
async fn partial_delete_reports_confirmed_and_failed_keys_and_exits_nonzero() {
    let mut server = Server::new_async().await;
    let delete = server
        .mock("PUT", "/b?deleteObjects=&enableTrash=false")
        .with_body(
            r#"[{"object_name":"bad","error_code":"denied","error_description":"permission"}]"#,
        )
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &["object", "delete-many", "b", "good", "bad"],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(value(&result)["output"]["deleted"], json!(["good"]));
    assert_eq!(
        value(&result)["output"]["failures"][0]["object_name"],
        "bad"
    );
    delete.assert_async().await;
}

#[tokio::test]
async fn manual_multipart_receipts_round_trip() {
    let mut server = Server::new_async().await;
    let files = Files::new();
    let init = server
        .mock("PUT", "/b/k?uploads=")
        .with_body(r#"{"uploadId":"session","bucketName":"wrong"}"#)
        .create_async()
        .await;
    let result = run(&server.url(), &["multipart", "init", "b", "k"]).await;
    success(&result);
    assert_eq!(value(&result)["bucketName"], "b");
    init.assert_async().await;
    let part = server
        .mock("PUT", "/b/k?partNumber=1&uploadId=session")
        .match_body("data")
        .with_body(r#"{"partNumber":1,"partSize":4,"etag":"e"}"#)
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &[
            "multipart",
            "part",
            "b",
            "k",
            "--upload-id",
            "session",
            "--number",
            "1",
            "--text",
            "data",
        ],
    )
    .await;
    success(&result);
    let path = files.write("parts.json", json!([value(&result)]).to_string());
    part.assert_async().await;
    let complete = server
        .mock("PUT", "/b/k?uploadId=session")
        .match_body(Matcher::Json(
            json!({"uploadPartResultList":[{"partNumber":1,"partSize":4,"etag":"e"}]}),
        ))
        .with_body("{}")
        .create_async()
        .await;
    success(
        &run(
            &server.url(),
            &[
                "multipart",
                "complete",
                "b",
                "k",
                "--upload-id",
                "session",
                "--parts",
                &path,
            ],
        )
        .await,
    );
    complete.assert_async().await;
    let abort = server
        .mock("DELETE", "/b/k?uploadId=other")
        .create_async()
        .await;
    success(
        &run(
            &server.url(),
            &["multipart", "abort", "b", "k", "--upload-id", "other"],
        )
        .await,
    );
    abort.assert_async().await;
}

#[tokio::test]
async fn managed_failure_records_receipts_and_never_aborts_ambiguous_completion() {
    let mut server = Server::new_async().await;
    let files = Files::new();
    let source = files.write("source", b"data");
    let recovery = files.path("recovery.jsonl");
    let init = server
        .mock("PUT", "/b/k?uploads=")
        .with_body(r#"{"uploadId":"session"}"#)
        .expect(1)
        .create_async()
        .await;
    let part = server
        .mock("PUT", "/b/k?partNumber=1&uploadId=session")
        .with_body(r#"{"partNumber":1,"partSize":4,"etag":"e"}"#)
        .create_async()
        .await;
    let complete = server
        .mock("PUT", "/b/k?uploadId=session")
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let abort = server
        .mock("DELETE", "/b/k?uploadId=session")
        .expect(0)
        .create_async()
        .await;
    let args = [
        "multipart",
        "upload",
        "b",
        "k",
        "--file",
        &source,
        "--part-size",
        "4",
        "--concurrency",
        "1",
        "--abort-on-failure",
        "--recovery",
        &recovery,
    ];
    assert_eq!(run(&server.url(), &args).await.status.code(), Some(1));
    let snapshots: Vec<Value> = std::fs::read_to_string(&recovery)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(snapshots[1]["state"], "Ready");
    assert_eq!(snapshots.last().unwrap()["state"], "CompletionUnknown");
    assert_eq!(
        snapshots.last().unwrap()["parts"].as_array().unwrap().len(),
        1
    );
    // Existing recovery paths fail before another initialization.
    assert_eq!(run(&server.url(), &args).await.status.code(), Some(1));
    init.assert_async().await;
    part.assert_async().await;
    complete.assert_async().await;
    abort.assert_async().await;
}

#[tokio::test]
async fn invalid_managed_upload_options_fail_before_the_recovery_file() {
    let mut server = Server::new_async().await;
    let init = server
        .mock("PUT", Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    let files = Files::new();
    let source = files.write("source", b"data");
    for (part_size, concurrency) in [
        ("0", "1"),
        ("4", "0"),
        ("4", "65"),
        ("67108865", "1"),
        ("67108864", "5"),
    ] {
        let recovery = files.path("recovery.jsonl");
        let result = run(
            &server.url(),
            &[
                "multipart",
                "upload",
                "b",
                "k",
                "--file",
                &source,
                "--part-size",
                part_size,
                "--concurrency",
                concurrency,
                "--recovery",
                &recovery,
            ],
        )
        .await;
        assert_eq!(result.status.code(), Some(1), "{part_size} x {concurrency}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("invalid multipart resource limits"),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            !std::path::Path::new(&recovery).exists(),
            "recovery file created for {part_size} x {concurrency}"
        );
    }
    init.assert_async().await;
}

#[tokio::test]
async fn discovery_needs_no_keys_and_oauth_exchange_authenticates_app_listing() {
    let mut server = Server::new_async().await;
    let discovery = server
        .mock("GET", "/?endpoint=&region=r&service=s")
        .match_header("authorization", Matcher::Missing)
        .with_body("\"fds.example.com\"")
        .create_async()
        .await;
    let mut cmd = command(None);
    cmd.args(["--endpoint", &server.url(), "service", "endpoint", "r", "s"]);
    success(
        &tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap(),
    );
    discovery.assert_async().await;
    let files = Files::new();
    let input=files.write("oauth.json",json!({"app_id":"app","oauth_app_id":"provider-app","access_token":"provider-secret","provider":"QQ"}).to_string());
    let exchange=server.mock("GET","/").match_query(Matcher::UrlEncoded("storageAccessToken".into(),"".into())).match_header("authorization","OAuth")
        .with_body(json!({"token":"storage-secret","expireTime":(chrono::Utc::now()+chrono::Duration::hours(1)).timestamp_millis()}).to_string()).create_async().await;
    let list = server
        .mock("GET", "/")
        .match_query(Matcher::UrlEncoded("appId".into(), "app".into()))
        .with_body(r#"{"owner":{"id":"o","displayName":""},"buckets":[]}"#)
        .create_async()
        .await;
    let mut cmd = command(None);
    cmd.args([
        "--endpoint",
        &server.url(),
        "--oauth",
        &input,
        "bucket",
        "list-app",
    ]);
    let result = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    success(&result);
    assert_eq!(value(&result)["owner"]["id"], "o");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("secret"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("secret"));
    exchange.assert_async().await;
    list.assert_async().await;
}

#[tokio::test]
async fn failed_stream_preserves_existing_destination_and_removes_staging() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            let count = stream.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort")
            .await
            .unwrap();
    });
    let files = Files::new();
    let out = files.write("destination", b"keep me");
    let result = run(
        &endpoint,
        &[
            "object",
            "get",
            "b",
            "k",
            "--out",
            &out,
            "--request-timeout-ms",
            "1000",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
    assert_eq!(std::fs::read_dir(&files.0).unwrap().count(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn read_retries_and_presign_metadata_reach_the_sdk() {
    let mut server = Server::new_async().await;
    let request = server
        .mock("HEAD", "/b/k")
        .with_status(503)
        .expect(3)
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &[
            "--retries",
            "2",
            "--retry-initial-ms",
            "1",
            "--retry-max-ms",
            "2",
            "object",
            "exists",
            "b",
            "k",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    request.assert_async().await;
    let result = run(
        &server.url(),
        &[
            "object",
            "presign",
            "b",
            "k",
            "--method",
            "PUT",
            "--expires",
            "600",
            "--content-type",
            "text/plain",
        ],
    )
    .await;
    success(&result);
    let url = String::from_utf8(result.stdout).unwrap();
    let expiration = url
        .split('?')
        .nth(1)
        .unwrap()
        .split('&')
        .find_map(|part| part.strip_prefix("Expires="))
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let client = fds::Client::builder()
        .endpoint(server.url())
        .credentials("id", "secret")
        .build()
        .unwrap();
    let expected = client
        .presign_object("b", "k")
        .method(http::Method::PUT)
        .expires_at(chrono::DateTime::from_timestamp_millis(expiration).unwrap())
        .content_type("text/plain")
        .build()
        .unwrap();
    assert_eq!(url.trim(), expected.as_str());
}

#[tokio::test]
async fn manager_download_forwards_range_and_resource_options() {
    let mut server = Server::new_async().await;
    let head = server
        .mock("HEAD", "/b/k")
        .with_header("content-length", "8")
        .with_header("etag", "identity")
        .create_async()
        .await;
    let get = server
        .mock("GET", "/b/k")
        .match_header("range", "bytes=2-5")
        .with_status(206)
        .with_header("content-range", "bytes 2-5/8")
        .with_header("etag", "identity")
        .with_body("cdef")
        .create_async()
        .await;
    let files = Files::new();
    let out = files.path("out");
    let result = run(
        &server.url(),
        &[
            "download",
            "b",
            "k",
            &out,
            "--range",
            "bytes=2-5",
            "--part-size",
            "4",
            "--concurrency",
            "1",
            "--requests-per-second",
            "100",
            "--resume",
        ],
    )
    .await;
    success(&result);
    assert_eq!(std::fs::read(out).unwrap(), b"cdef");
    head.assert_async().await;
    get.assert_async().await;
}

#[tokio::test]
async fn managed_part_failure_applies_abort_policy_and_reports_final_state() {
    let mut server = Server::new_async().await;
    let init = server
        .mock("PUT", "/b/k?uploads=")
        .with_body(r#"{"uploadId":"id"}"#)
        .create_async()
        .await;
    let part = server
        .mock("PUT", "/b/k?partNumber=1&uploadId=id")
        .with_status(503)
        .expect(1)
        .create_async()
        .await;
    let abort = server
        .mock("DELETE", "/b/k?uploadId=id")
        .expect(1)
        .create_async()
        .await;
    let files = Files::new();
    let source = files.write("source", b"data");
    let recovery = files.path("session.jsonl");
    let result = run(
        &server.url(),
        &[
            "multipart",
            "upload",
            "b",
            "k",
            "--file",
            &source,
            "--recovery",
            &recovery,
            "--abort-on-failure",
            "--part-size",
            "4",
            "--concurrency",
            "1",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    let text = std::fs::read_to_string(recovery).unwrap();
    let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert_eq!(last["state"], "Aborted");
    init.assert_async().await;
    part.assert_async().await;
    abort.assert_async().await;
}

#[tokio::test]
async fn headers_and_opt_in_response_diagnostics_keep_stdout_usable() {
    let mut server = Server::new_async().await;
    let get = server
        .mock("GET", "/b/k")
        .with_header("x-xiaomi-meta-tag", "value")
        .with_body("hello")
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &["object", "get", "b", "k", "--show-headers"],
    )
    .await;
    success(&result);
    assert_eq!(result.stdout, b"hello");
    let headers: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(headers["status"], 200);
    assert!(headers["headers"]
        .as_array()
        .unwrap()
        .contains(&json!(["x-xiaomi-meta-tag", "value"])));
    get.assert_async().await;
    let failure = server
        .mock("GET", "/b/k")
        .with_status(403)
        .with_body("sensitive-diagnostic")
        .expect(2)
        .create_async()
        .await;
    let plain = run(&server.url(), &["object", "get", "b", "k"]).await;
    assert!(!plain.status.success());
    assert!(!String::from_utf8_lossy(&plain.stderr).contains("sensitive-diagnostic"));
    let diagnostic = run(&server.url(), &["--diagnostics", "object", "get", "b", "k"]).await;
    assert!(!diagnostic.status.success());
    assert!(diagnostic.stdout.is_empty());
    assert!(String::from_utf8_lossy(&diagnostic.stderr).contains("sensitive-diagnostic"));
    failure.assert_async().await;
}

#[tokio::test]
async fn delete_prefix_keeps_listing_flags_and_stops_on_a_repeated_marker() {
    let mut server = Server::new_async().await;
    let first = server
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=%2F&marker=start&maxKeys=7&prefix=p%2F".into(),
        ))
        .with_body(r#"{"truncated":true,"nextMarker":"a","objects":[{"name":"p/1"}]}"#)
        .create_async()
        .await;
    let delete = server
        .mock("PUT", "/b?deleteObjects=&enableTrash=true")
        .match_body(r#"["p/1"]"#)
        .with_body("[]")
        .create_async()
        .await;
    // Back to the starting marker: nothing from this page is deleted.
    let second = server
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=%2F&marker=a&maxKeys=7&prefix=p%2F".into(),
        ))
        .with_body(r#"{"truncated":true,"nextMarker":"start","objects":[{"name":"p/2"}]}"#)
        .create_async()
        .await;
    let no_delete = server
        .mock("PUT", "/b?deleteObjects=&enableTrash=true")
        .match_body(r#"["p/2"]"#)
        .expect(0)
        .create_async()
        .await;
    let result = run(
        &server.url(),
        &[
            "object",
            "delete-prefix",
            "b",
            "p/",
            "--delimiter",
            "/",
            "--marker",
            "start",
            "--max-keys",
            "7",
            "--trash",
        ],
    )
    .await;
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(value(&result)["output"]["deleted"], json!(["p/1"]));
    assert_eq!(
        String::from_utf8_lossy(&result.stderr),
        "error: ProtocolViolation: invalid prefix deletion page or pagination marker\n"
    );
    first.assert_async().await;
    delete.assert_async().await;
    second.assert_async().await;
    no_delete.assert_async().await;
}

#[tokio::test]
async fn delete_prefix_without_flags_sends_an_empty_marker() {
    let mut server = Server::new_async().await;
    let first = server
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=&marker=&maxKeys=1000&prefix=p%2F".into(),
        ))
        .with_body(r#"{"truncated":true,"nextMarker":"m","objects":[{"name":"p/1"}]}"#)
        .create_async()
        .await;
    let second = server
        .mock("GET", "/b")
        .match_query(Matcher::Exact(
            "delimiter=&marker=m&maxKeys=1000&prefix=p%2F".into(),
        ))
        .with_body(r#"{"objects":[{"name":"q/2"}]}"#)
        .create_async()
        .await;
    let delete = server
        .mock("PUT", "/b?deleteObjects=&enableTrash=false")
        .match_body(r#"["p/1"]"#)
        .with_body("[]")
        .create_async()
        .await;
    // A listed name outside the prefix: nothing from that page is deleted.
    let no_delete = server
        .mock("PUT", "/b?deleteObjects=&enableTrash=false")
        .match_body(r#"["q/2"]"#)
        .expect(0)
        .create_async()
        .await;
    let result = run(&server.url(), &["object", "delete-prefix", "b", "p/"]).await;
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(value(&result)["output"]["deleted"], json!(["p/1"]));
    assert_eq!(
        String::from_utf8_lossy(&result.stderr),
        "error: ProtocolViolation: invalid prefix deletion page or pagination marker\n"
    );
    for mock in [first, second, delete, no_delete] {
        mock.assert_async().await;
    }
}

#[tokio::test]
async fn delete_prefix_keeps_the_client_error_for_a_broken_listing() {
    let mut server = Server::new_async().await;
    let list = server
        .mock("GET", "/b")
        .match_query(Matcher::Any)
        .with_body("not json")
        .create_async()
        .await;
    let result = run(&server.url(), &["object", "delete-prefix", "b", "p/"]).await;
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(value(&result)["output"]["deleted"], json!([]));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.starts_with("error: ProtocolViolation: invalid JSON response body (HTTP 200)"),
        "{stderr}"
    );
    list.assert_async().await;
}

#[tokio::test]
async fn empty_and_prefixed_user_metadata_keys_fail_before_sending() {
    let mut server = Server::new_async().await;
    let any = server
        .mock("PUT", Matcher::Any)
        .match_query(Matcher::Any)
        .expect(0)
        .create_async()
        .await;
    for (meta, message) in [
        (
            "x-xiaomi-meta-=v",
            "error: InvalidInput: user metadata key must not be empty",
        ),
        (
            "x-xiaomi-meta-X-Xiaomi-Meta-a=v",
            "error: InvalidInput: user metadata key \"x-xiaomi-meta-a\" must not include the x-xiaomi-meta- prefix",
        ),
    ] {
        for args in [
            &["object", "put", "b", "k", "--text", "hi", "--meta", meta][..],
            &["object", "set-metadata", "b", "k", "--meta", meta][..],
            &["object", "presign", "b", "k", "--meta", meta][..],
            &["multipart", "init", "b", "k", "--meta", meta][..],
        ] {
            let result = run(&server.url(), args).await;
            assert_eq!(result.status.code(), Some(1), "{args:?}");
            assert!(result.stdout.is_empty(), "{args:?}");
            assert_eq!(
                String::from_utf8(result.stderr).unwrap().trim_end(),
                message,
                "{args:?}"
            );
        }
    }
    any.assert_async().await;
}
