//! Query-string encoding. Ports `fds/httpparser/querystring.go` +
//! `url.Values.Encode()` semantics.
//!
//! 🔴 Byte-critical: the encoded query appears in the request line, and the
//! subresource subset feeds the signature. We replicate Go's `url.QueryEscape`
//! and `url.Values.Encode()` (sort keys ascending, stable within a key).

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::error::Result;

/// Characters `url.QueryEscape` leaves unescaped are the "unreserved" set
/// `A-Z a-z 0-9 - _ . ~`. So the escape set is "everything non-alphanumeric"
/// minus those four punctuation marks. Space is escaped to `%20` here and then
/// rewritten to `+` in [`go_query_escape`] (matching Go's query-component rule).
const GO_QUERY_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Percent-encode exactly like Go `url.QueryEscape`.
///
/// ⚠️ Do NOT use `url::form_urlencoded` / `serde_urlencoded`: they follow WHATWG
/// rules and differ from Go on `*` (Go → `%2A`) and `~` (Go → unescaped),
/// which would change object-name/query encoding and break the signature.
pub fn go_query_escape(s: &str) -> String {
    // percent_encoding emits uppercase `%XX` and encodes space as `%20`;
    // Go's query component uses `+` for space. Only spaces produce `%20`
    // (other chars' `%XX` never contain that substring), so the replace is safe.
    utf8_percent_encode(s, GO_QUERY_SET)
        .to_string()
        .replace("%20", "+")
}

/// Ordered `(key, value)` pairs. Encoding sorts by key (mirroring
/// `url.Values.Encode`); insertion order is preserved among equal keys.
#[derive(Default, Debug, Clone)]
pub struct QueryValues(pub Vec<(String, String)>);

impl QueryValues {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Append a `(key, value)` pair. An empty value is kept (Go's no-`omitempty`
    /// fields emit `key=` — callers must only skip `omitempty` fields upstream).
    pub fn add(&mut self, k: impl Into<String>, v: impl Into<String>) {
        self.0.push((k.into(), v.into()));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Encode like Go `url.Values.Encode()`: stable-sort by key ascending
    /// (ASCII byte order — uppercase before lowercase), then
    /// `escape(key)=escape(value)` joined by `&`.
    pub fn encode(&self) -> String {
        let mut items = self.0.clone();
        items.sort_by(|a, b| a.0.cmp(&b.0)); // stable; keeps per-key value order
        let mut out = String::new();
        for (k, v) in items {
            if !out.is_empty() {
                out.push('&');
            }
            out.push_str(&go_query_escape(&k));
            out.push('=');
            out.push_str(&go_query_escape(&v));
        }
        out
    }
}

/// Types that can be encoded to query values (Go's `param` struct tags).
///
/// Implemented by hand per request struct. Remember the
/// `omitempty` semantics: fields WITHOUT `omitempty` emit even when
/// empty/zero; fields WITH `omitempty` are skipped when empty/zero.
pub trait ToQuery {
    fn to_query(&self) -> Result<QueryValues>;
}

/// Format a timestamp like Go `time.RFC822` (`02 Jan 06 15:04 MST`), used by the
/// reflection encoder for `time.Time` fields. Real FDS request options don't use
/// time query fields, but this keeps parity with the httpparser golden vectors.
pub fn format_rfc822(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%d %b %y %H:%M %Z").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn go_query_escape_specials() {
        // Go: space→'+', ':'→%3A, '/'→%2F, '~' unescaped, '*'→%2A.
        assert_eq!(go_query_escape("a b"), "a+b");
        assert_eq!(
            go_query_escape("01 Jan 18 01:01 UTC"),
            "01+Jan+18+01%3A01+UTC"
        );
        assert_eq!(go_query_escape("a/b"), "a%2Fb");
        assert_eq!(go_query_escape("~"), "~");
        assert_eq!(go_query_escape("*"), "%2A");
        assert_eq!(go_query_escape("x+y"), "x%2By");
    }

    #[test]
    fn rfc822_golden() {
        // httpparser test: time.Date(2018,1,1,1,1,1,1,UTC) → "01 Jan 18 01:01 UTC"
        let dt = Utc.with_ymd_and_hms(2018, 1, 1, 1, 1, 1).unwrap();
        assert_eq!(format_rfc822(&dt), "01 Jan 18 01:01 UTC");
    }

    #[test]
    fn encode_golden_vector() {
        // Reproduces querystring_test.go:36 (the fields the hand-written encoder
        // would emit for `testOptions`): sorted keys, RFC822 time, bool "false".
        let dt = Utc.with_ymd_and_hms(2018, 1, 1, 1, 1, 1).unwrap();
        let mut q = QueryValues::new();
        // Insertion order is intentionally scrambled to prove sort-by-key.
        q.add("name", "John");
        q.add("age", "12");
        q.add("last-modified", format_rfc822(&dt));
        q.add("can", "false");
        q.add("OtherOption", "helloworld");
        assert_eq!(
            q.encode(),
            "OtherOption=helloworld&age=12&can=false&last-modified=01+Jan+18+01%3A01+UTC&name=John"
        );
    }

    #[test]
    fn encode_uppercase_before_lowercase() {
        let mut q = QueryValues::new();
        q.add("age", "1");
        q.add("OtherOption", "x");
        // 'O' (0x4F) sorts before 'a' (0x61).
        assert_eq!(q.encode(), "OtherOption=x&age=1");
    }
}
