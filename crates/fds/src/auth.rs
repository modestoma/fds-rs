//! Galaxy-V2 request signing.
//!
//! Byte-critical: one wrong byte in the `StringToSign` yields a server 403. See `tests/fixtures/golden.json` for saved signing vectors.
//!
//! Crate-internal: signing is an implementation detail, not a public API. The
//! saved Go signature vectors are checked by `tests::golden_signatures` below.
//!
//! ```text
//! StringToSign = Method + "\n" + ContentMD5 + "\n" + ContentType + "\n"
//!              + Date   + "\n" + CanonicalizedXiaomiHeaders + CanonicalizedResource
//! signature    = Base64( HMAC-SHA1( AccessSecret, StringToSign ) )
//! Authorization: Galaxy-V2 {AccessID}:{signature}
//! ```

use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use url::Url;

use crate::constant::{
    HTTP_HEADER_CONTENT_MD5, HTTP_HEADER_CONTENT_TYPE, HTTP_HEADER_DATE, XIAOMI_PREFIX,
};
use crate::error::{Error, Result};

type HmacSha1 = Hmac<Sha1>;

/// The exactly-7 subresource keys that participate in the canonical resource
/// string. Membership test only; order irrelevant.
const SUB_RESOURCES: &[&str] = &[
    "acl",
    "quota",
    "uploads",
    "partNumber",
    "uploadId",
    "storageAccessToken",
    "metadata",
];

/// Case-insensitive header abstraction the signer needs. The production impl is
/// for `reqwest::header::HeaderMap` (already lowercase-keyed).
pub(crate) trait HeaderLookup {
    /// First value for a lowercase header name; empty string if absent.
    fn get_first(&self, name_lower: &str) -> String;
    /// Visit each distinct lowercase key with its multi-value joined by `,`.
    fn for_each_lower(&self, f: &mut dyn FnMut(&str, &str));
}

impl HeaderLookup for reqwest::header::HeaderMap {
    fn get_first(&self, name_lower: &str) -> String {
        self.get(name_lower)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    fn for_each_lower(&self, f: &mut dyn FnMut(&str, &str)) {
        for key in self.keys() {
            let joined = self
                .get_all(key)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect::<Vec<_>>()
                .join(",");
            f(key.as_str(), &joined);
        }
    }
}

/// Build the `StringToSign` (before the HMAC step). Extracted
/// so it can be asserted byte-for-byte in tests without needing an HMAC oracle.
pub(crate) fn string_to_sign(
    method: &str,
    url_str: &str,
    headers: &impl HeaderLookup,
) -> Result<String> {
    let content_md5 = headers.get_first(HTTP_HEADER_CONTENT_MD5);
    let content_type = headers.get_first(HTTP_HEADER_CONTENT_TYPE);

    // Date row: a non-empty `Expires` query param (presigned URLs) takes
    // priority over the Date header; otherwise fall back to the Date header.
    let date = match expires(url_str) {
        Some(e) if !e.is_empty() => e,
        _ => headers.get_first(HTTP_HEADER_DATE),
    };

    let mut buf = String::new();
    buf.push_str(method);
    buf.push('\n');
    buf.push_str(&content_md5);
    buf.push('\n');
    buf.push_str(&content_type);
    buf.push('\n');
    buf.push_str(&date);
    buf.push('\n');
    buf.push_str(&canonical_xiaomi_headers(headers)); // each line ends with '\n' (or empty)
    buf.push_str(&canonical_resource(url_str)?); // no trailing '\n'
    Ok(buf)
}

/// Compute the Galaxy-V2 signature.
pub(crate) fn sign(
    secret: &str,
    method: &str,
    url_str: &str,
    headers: &impl HeaderLookup,
) -> Result<String> {
    let sts = string_to_sign(method, url_str, headers)?;
    let mut mac = HmacSha1::new_from_slice(secret.as_bytes())
        .map_err(|_| Error::config_invalid("invalid signing key"))?;
    mac.update(sts.as_bytes());
    Ok(STANDARD.encode(mac.finalize().into_bytes()))
}

/// First value of the exactly-cased `Expires` query param.
fn expires(url_str: &str) -> Option<String> {
    let u = Url::parse(url_str).ok()?;
    u.query_pairs()
        .find(|(k, _)| k == "Expires")
        .map(|(_, v)| v.into_owned())
}

/// CanonicalizedXiaomiHeaders: keep `x-xiaomi-` prefixed
/// keys, sort ascending, emit `key:value\n` per line.
fn canonical_xiaomi_headers(headers: &impl HeaderLookup) -> String {
    let mut kv: Vec<(String, String)> = Vec::new();
    headers.for_each_lower(&mut |k, v| {
        if k.starts_with(XIAOMI_PREFIX) {
            kv.push((k.to_string(), v.to_string()));
        }
    });
    kv.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    for (k, v) in kv {
        out.push_str(&k);
        out.push(':');
        out.push_str(&v);
        out.push('\n');
    }
    out
}

/// CanonicalizedResource: the PERCENT-DECODED path plus the
/// filtered/sorted subresource query.
fn canonical_resource(url_str: &str) -> Result<String> {
    let u = Url::parse(url_str).map_err(|_| Error::invalid_input("invalid signing URL"))?;

    // Go's `url.Parse().Path` is percent-decoded; Rust's `Url::path()` is not,
    // so decode it (space/`%2F`/etc. → literals) to match the signed resource.
    let mut path = percent_encoding::percent_decode_str(u.path())
        .decode_utf8_lossy()
        .into_owned();

    // Keep only subresource keys, first value per key.
    let mut matched: Vec<(String, String)> = Vec::new();
    for (k, v) in u.query_pairs() {
        if SUB_RESOURCES.contains(&k.as_ref()) && !matched.iter().any(|(mk, _)| mk == k.as_ref()) {
            matched.push((k.into_owned(), v.into_owned()));
        }
    }
    if matched.is_empty() {
        return Ok(path);
    }
    matched.sort_by(|a, b| a.0.cmp(&b.0));
    for (i, (k, v)) in matched.iter().enumerate() {
        path.push(if i == 0 { '?' } else { '&' });
        path.push_str(k);
        if !v.is_empty() {
            path.push('=');
            path.push_str(v);
        }
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `HeaderLookup` for tests: lowercase keys, insertion order.
    struct TestHeaders(Vec<(String, String)>);
    impl TestHeaders {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        }
    }
    impl HeaderLookup for TestHeaders {
        fn get_first(&self, name_lower: &str) -> String {
            self.0
                .iter()
                .find(|(k, _)| k == name_lower)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        }
        fn for_each_lower(&self, f: &mut dyn FnMut(&str, &str)) {
            for (k, v) in &self.0 {
                f(k, v);
            }
        }
    }

    /// Saved signature vectors captured from the Go SDK
    /// (`tests/fixtures/golden.json`). This lives here rather than in
    /// `tests/golden.rs` because [`sign`] is crate-internal; the URL and
    /// presign vectors of the same fixture still run through the public API
    /// in `tests/golden.rs`.
    #[test]
    fn golden_signatures() {
        #[derive(serde::Deserialize)]
        struct Fixtures {
            signatures: Vec<Signature>,
        }
        #[derive(serde::Deserialize)]
        struct Signature {
            secret: String,
            method: String,
            url: String,
            headers: std::collections::HashMap<String, String>,
            expected: String,
        }
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../tests/fixtures/golden.json")).unwrap();
        assert!(!fixtures.signatures.is_empty());
        for case in &fixtures.signatures {
            let headers = TestHeaders(case.headers.clone().into_iter().collect());
            let got = sign(&case.secret, &case.method, &case.url, &headers).unwrap();
            assert_eq!(
                got, case.expected,
                "signature mismatch for {} {}",
                case.method, case.url
            );
        }
    }

    /// The User-Agent (like every header outside Content-MD5, Content-Type,
    /// Date and `x-xiaomi-*`) is not part of the signature.
    #[test]
    fn user_agent_is_not_signed() {
        let url = "https://cnbj1.fds.api.xiaomi.com/b/o?uploads";
        let plain = sign("sk", "PUT", url, &dated()).unwrap();
        let with_ua =
            TestHeaders::new(&[("date", DATE), ("user-agent", "fds-rs/0.0.0 my-app/1.2")]);
        assert_eq!(sign("sk", "PUT", url, &with_ua).unwrap(), plain);
    }

    const DATE: &str = "Mon, 13 Jul 2026 10:00:00 GMT";
    fn dated() -> TestHeaders {
        TestHeaders::new(&[("date", DATE)])
    }

    // ---- Appendix A: StringToSign golden vectors (deterministic) -----------

    #[test]
    fn sts_a1_plain() {
        let sts = string_to_sign(
            "GET",
            "https://cnbj1.fds.api.xiaomi.com/mybucket/a/b.txt",
            &dated(),
        )
        .unwrap();
        assert_eq!(
            sts,
            "GET\n\n\nMon, 13 Jul 2026 10:00:00 GMT\n/mybucket/a/b.txt"
        );
    }

    #[test]
    fn sts_a2_valueless_subresource() {
        let sts = string_to_sign(
            "GET",
            "https://cnbj1.fds.api.xiaomi.com/mybucket/obj?acl",
            &dated(),
        )
        .unwrap();
        assert_eq!(
            sts,
            "GET\n\n\nMon, 13 Jul 2026 10:00:00 GMT\n/mybucket/obj?acl"
        );
    }

    #[test]
    fn sts_a3_valued_subresources_sorted_foo_dropped() {
        let sts = string_to_sign(
            "GET",
            "https://cnbj1.fds.api.xiaomi.com/b/o?uploadId=U1&partNumber=3&foo=bar",
            &dated(),
        )
        .unwrap();
        assert_eq!(
            sts,
            "GET\n\n\nMon, 13 Jul 2026 10:00:00 GMT\n/b/o?partNumber=3&uploadId=U1"
        );
    }

    #[test]
    fn sts_a4_object_name_with_space_and_slash_decoded() {
        let sts = string_to_sign(
            "GET",
            "https://cnbj1.fds.api.xiaomi.com/b/dir%20a/b%20c.txt",
            &dated(),
        )
        .unwrap();
        assert_eq!(
            sts,
            "GET\n\n\nMon, 13 Jul 2026 10:00:00 GMT\n/b/dir a/b c.txt"
        );
    }

    #[test]
    fn sts_a5_xiaomi_meta_and_content_type() {
        let h = TestHeaders::new(&[
            ("date", DATE),
            ("content-type", "image/png"),
            ("x-xiaomi-meta-uid", "1000"),
        ]);
        let sts = string_to_sign("GET", "https://cnbj1.fds.api.xiaomi.com/b/o", &h).unwrap();
        assert_eq!(
            sts,
            "GET\n\nimage/png\nMon, 13 Jul 2026 10:00:00 GMT\nx-xiaomi-meta-uid:1000\n/b/o"
        );
    }

    #[test]
    fn sts_a6_presign_expires_overrides_date() {
        // Presigned URL: the millisecond `Expires` becomes the Date row; the
        // Date header (absent here) is ignored. `Expires` is not a subresource.
        let sts = string_to_sign(
            "GET",
            "https://cnbj1.fds.api.xiaomi.com/b/o?Expires=1752400000000",
            &TestHeaders::new(&[]),
        )
        .unwrap();
        assert_eq!(sts, "GET\n\n\n1752400000000\n/b/o");
    }

    #[test]
    fn multiple_xiaomi_headers_sorted() {
        let h = TestHeaders::new(&[
            ("date", DATE),
            ("x-xiaomi-multipart-upload-mode", "MULTI_BLOB"),
            ("x-xiaomi-meta-uid", "1000"),
        ]);
        let sts = string_to_sign("PUT", "https://h/b/o?uploads", &h).unwrap();
        assert_eq!(
            sts,
            "PUT\n\n\nMon, 13 Jul 2026 10:00:00 GMT\nx-xiaomi-meta-uid:1000\nx-xiaomi-multipart-upload-mode:MULTI_BLOB\n/b/o?uploads"
        );
    }

    // ---- HMAC-SHA1 wiring sanity (RFC 2202 test case 2, no Go needed) ------

    #[test]
    fn hmac_sha1_rfc2202() {
        let mut mac = HmacSha1::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        let digest = mac.finalize().into_bytes();
        assert_eq!(
            hex::encode(digest),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    #[test]
    fn sign_produces_stable_base64() {
        // Not a Go-oracle assertion (independent golden vectors must be
        // generated from real Go); just checks sign() runs and is deterministic.
        let s1 = sign("test-secret", "GET", "https://h/b/o", &dated()).unwrap();
        let s2 = sign("test-secret", "GET", "https://h/b/o", &dated()).unwrap();
        assert_eq!(s1, s2);
        assert!(!s1.is_empty());
    }
}
