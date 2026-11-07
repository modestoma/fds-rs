//! Differential golden-vector tests: assert Rust output byte-for-byte against
//! REAL Go outputs captured in `tests/fixtures/golden.json`.
//!
//! Regenerate the fixture from the Go SDK (proves equivalence, not just
//! self-consistency):
//! ```bash
//! cd ../go-fds && go test ./fds -run TestGenerateGoldenFixtures -count=1
//! ```
//! The Go generator (`go-fds/fds/golden_export_test.go`) emits real
//! `buildRequestURL` / `signature` / `GeneratePresignedURL` values. Expected
//! signature/URL strings MUST come from Go — never hand/LLM-computed.
//!
//! The `signatures` vectors of the same fixture are checked by the
//! crate-internal unit test `auth::tests::golden_signatures`, because the
//! signer is not public API.

use std::collections::HashMap;

use fds::object::ObjectMetadata;
use fds::Client;
use http::Method;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixtures {
    access_id: String,
    access_secret: String,
    endpoint: String,
    absolute_urls: Vec<AbsUrl>,
    presigned_urls: Vec<Presigned>,
}

#[derive(Deserialize)]
struct AbsUrl {
    bucket: String,
    object: String,
    expected: String,
}

#[derive(Deserialize)]
struct Presigned {
    cdn: bool,
    bucket: String,
    object: String,
    method: String,
    expiration_millis: i64,
    metadata: Option<HashMap<String, String>>,
    expected: String,
}

fn load() -> Fixtures {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden.json");
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run the Go generator first)"));
    serde_json::from_str(&data).expect("parse golden.json")
}

fn parse_method(s: &str) -> Method {
    match s {
        "GET" => Method::GET,
        "PUT" => Method::PUT,
        "HEAD" => Method::HEAD,
        "POST" => Method::POST,
        "DELETE" => Method::DELETE,
        other => panic!("unknown method {other}"),
    }
}

fn build_metadata(m: &HashMap<String, String>) -> ObjectMetadata {
    let mut md = ObjectMetadata::new();
    for (k, v) in m {
        md.set(k, v).unwrap();
    }
    md
}

fn client(fx: &Fixtures) -> Client {
    Client::builder()
        .endpoint(format!("https://{}", fx.endpoint))
        .cdn_endpoint("https://cdn.cnbj1.fds.api.mi-img.com")
        .credentials(&fx.access_id, &fx.access_secret)
        .build()
        .unwrap()
}

#[test]
fn golden_absolute_urls() {
    let fx = load();
    let c = client(&fx);
    assert!(!fx.absolute_urls.is_empty());
    for case in &fx.absolute_urls {
        let got = c.absolute_object_url(&case.bucket, &case.object).unwrap();
        assert_eq!(
            got, case.expected,
            "absolute url mismatch for object {:?}",
            case.object
        );
    }
}

#[test]
fn golden_presigned_urls() {
    let fx = load();
    let c = client(&fx);
    assert!(!fx.presigned_urls.is_empty());
    for case in &fx.presigned_urls {
        let expiration = chrono::DateTime::from_timestamp_millis(case.expiration_millis).unwrap();
        let metadata = case.metadata.as_ref().map(build_metadata);
        let mut request = c
            .presign_object(&case.bucket, &case.object)
            .method(parse_method(&case.method))
            .expires_at(expiration);
        if case.cdn {
            request = request.cdn();
        }
        if let Some(metadata) = metadata {
            request = request.metadata(metadata);
        }
        let got = request.build().unwrap();
        assert_eq!(
            got, case.expected,
            "presigned url mismatch for {} {}/{}",
            case.method, case.bucket, case.object
        );
    }
}
