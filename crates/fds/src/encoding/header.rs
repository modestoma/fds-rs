//! Request-header encoding. Ports `fds/httpparser/header.go`.
//!
//! Implemented by hand per request struct. The `Option`
//! fields sidestep BUG-5 (Go's nil-pointer `break` aborting the field loop) —
//! no real option struct triggers it.

use reqwest::header::HeaderMap;

use crate::error::Result;

/// Types that can be encoded to HTTP request headers (Go's `header` struct
/// tags). Header names keep Go's literal casing (`Content-Md5`, `Cache-Control`,
/// …); reqwest lowercases them on the wire, which is signature-neutral.
pub trait ToHeaders {
    fn to_headers(&self) -> Result<HeaderMap>;
}
