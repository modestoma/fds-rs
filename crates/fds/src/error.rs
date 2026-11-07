//! Structured response errors with opt-in, bounded diagnostics.
//! HTTP status, service code and bounded response diagnostics remain distinct.

use thiserror::Error;

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, FdsError>;

/// The unified error type for the FDS SDK.
#[derive(Error)]
#[non_exhaustive]
pub enum FdsError {
    /// Corresponds to Go `ErrorEndpoint` ("wrong endpoint").
    #[error("wrong endpoint")]
    Endpoint,

    /// Transport-layer error (connect/timeout/TLS/…), from `reqwest`.
    #[error("http transport error: {0}")]
    Http(#[source] reqwest::Error),

    /// Rejected HTTP status or invalid success response.
    #[error("{0}")]
    Response(#[from] ResponseError),

    /// Copy was attempted but no valid matching receipt was obtained.
    /// The destination may have been written; do not blindly retry.
    #[error(
        "Copy outcome may be unknown; destination may have been written; do not blindly retry: {0}"
    )]
    CopyOutcomeUnknown(#[source] Box<FdsError>),

    /// Local IO (download file writes, breakpoint files, …).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Invalid argument/format (Range format, invalid metadata key, marshal
    /// failure, …).
    #[error("{0}")]
    Invalid(String),

    /// The request was cancelled (corresponds to Go `ctx.Err()`). In practice
    /// timeouts/cancellations usually surface as `FdsError::Http`.
    #[error("request cancelled")]
    Cancelled,
}

impl FdsError {
    /// Build an `Invalid` error.
    pub fn invalid(msg: impl Into<String>) -> Self {
        FdsError::Invalid(msg.into())
    }
}

/// The reason an HTTP response was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseErrorKind {
    UnexpectedStatus,
    EmptyJson,
    InvalidJson,
    BodyRead,
    BodyTooLarge,
}

/// Response diagnostics. Display/Debug deliberately omit all server-controlled
/// strings: even a request ID or service code can echo credentials.
/// Explicit string accessors return untrusted data and must not be logged blindly.
pub struct ResponseError {
    pub(crate) status: u16,
    pub(crate) kind: ResponseErrorKind,
    pub(crate) service_code: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) body: String,
    pub(crate) truncated: bool,
    pub(crate) source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ResponseError {
    pub fn status(&self) -> u16 {
        self.status
    }
    pub fn kind(&self) -> ResponseErrorKind {
        self.kind
    }
    pub fn service_code(&self) -> Option<&str> {
        self.service_code.as_deref()
    }
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }
    /// At most 8 KiB of untrusted response text. May contain sensitive data.
    pub fn diagnostic_body(&self) -> &str {
        &self.body
    }
    pub fn body_truncated(&self) -> bool {
        self.truncated
    }
}
impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FDS response {}: {:?}", self.status, self.kind)
    }
}
impl std::fmt::Debug for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for ResponseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|e| e as _)
    }
}

// serde's error text may echo an unknown variant containing a token. Retain
// parser location/category as the source without retaining that unsafe message.
#[derive(Debug, Error)]
#[error("JSON response decoding failed ({category:?}) at line {line}, column {column}")]
pub(crate) struct JsonResponseSource {
    pub category: serde_json::error::Category,
    pub line: usize,
    pub column: usize,
}
impl From<reqwest::Error> for FdsError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}
impl std::fmt::Debug for FdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CopyOutcomeUnknown(e) => f.debug_tuple("CopyOutcomeUnknown").field(e).finish(),
            Self::Response(e) => std::fmt::Debug::fmt(e, f),
            Self::Http(_) => f.write_str("FdsError::Http"),
            Self::Json(_) => f.write_str("FdsError::Json"),
            Self::Invalid(_) => f.write_str("FdsError::Invalid"),
            Self::Io(_) => f.write_str("FdsError::Io"),
            Self::Endpoint => f.write_str("FdsError::Endpoint"),
            Self::Cancelled => f.write_str("FdsError::Cancelled"),
        }
    }
}
impl FdsError {
    pub fn response(&self) -> Option<&ResponseError> {
        match self {
            Self::CopyOutcomeUnknown(e) => e.response(),
            Self::Response(e) => Some(e),
            _ => None,
        }
    }
    pub fn status(&self) -> Option<u16> {
        self.response().map(ResponseError::status)
    }
    pub fn service_code(&self) -> Option<&str> {
        self.response()?.service_code()
    }
    pub fn request_id(&self) -> Option<&str> {
        self.response()?.request_id()
    }
}
