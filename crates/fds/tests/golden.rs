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

use std::collections::HashMap;

use fds::auth::HeaderLookup;
use fds::{Client, GeneratePresignedURLRequest, HttpMethod, ObjectMetadata};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixtures {
    access_id: String,
    access_secret: String,
    endpoint: String,
    absolute_urls: Vec<AbsUrl>,
    signatures: Vec<Sig>,
    presigned_urls: Vec<Presigned>,
}

#[derive(Deserialize)]
struct AbsUrl {
    bucket: String,
    object: String,
    expected: String,
}

#[derive(Deserialize)]
struct Sig {
    secret: String,
    method: String,
    url: String,
    headers: HashMap<String, String>,
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

/// A `HeaderLookup` over a plain lowercase-keyed map (mirrors how reqwest's
/// `HeaderMap` behaves for the signer).
struct MapHeaders(HashMap<String, String>);

impl HeaderLookup for MapHeaders {
    fn get_first(&self, name_lower: &str) -> String {
        self.0.get(name_lower).cloned().unwrap_or_default()
    }
    fn for_each_lower(&self, f: &mut dyn FnMut(&str, &str)) {
        for (k, v) in &self.0 {
            f(k, v);
        }
    }
}

fn load() -> Fixtures {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/golden.json");
    let data = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e} (run the Go generator first)"));
    serde_json::from_str(&data).expect("parse golden.json")
}

fn parse_method(s: &str) -> HttpMethod {
    match s {
        "GET" => HttpMethod::Get,
        "PUT" => HttpMethod::Put,
        "HEAD" => HttpMethod::Head,
        "POST" => HttpMethod::Post,
        "DELETE" => HttpMethod::Delete,
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
        let got = c
            .generate_absolute_object_url(&case.bucket, &case.object)
            .unwrap();
        assert_eq!(
            got, case.expected,
            "absolute url mismatch for object {:?}",
            case.object
        );
    }
}

#[test]
fn golden_signatures() {
    let fx = load();
    assert!(!fx.signatures.is_empty());
    for case in &fx.signatures {
        let headers = MapHeaders(case.headers.clone());
        let got = fds::auth::sign(&case.secret, &case.method, &case.url, &headers).unwrap();
        assert_eq!(
            got, case.expected,
            "signature mismatch for {} {}",
            case.method, case.url
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
        let got = c
            .generate_presigned_url(&GeneratePresignedURLRequest {
                cdn: case.cdn,
                bucket_name: case.bucket.clone(),
                object_name: case.object.clone(),
                method: parse_method(&case.method),
                expiration,
                metadata,
            })
            .unwrap();
        assert_eq!(
            got, case.expected,
            "presigned url mismatch for {} {}/{}",
            case.method, case.bucket, case.object
        );
    }
}
