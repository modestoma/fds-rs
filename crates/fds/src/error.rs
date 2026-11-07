//! The single SDK error type. Implementation module; [`Error`] and its
//! companions are re-exported at the crate root.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::fmt;

use http::StatusCode;

use crate::object::DeleteObjectsOutput;

/// Crate-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// What went wrong, independent of the message text.
///
/// The enum is `non_exhaustive`: matches need a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A caller-supplied argument or option is invalid; nothing was sent.
    InvalidInput,
    /// The client configuration (endpoint, credentials, timeouts, retry policy,
    /// CDN endpoint, credential type for the operation) is invalid or unsuitable.
    ConfigInvalid,
    /// HTTP 404: the bucket, object or upload does not exist.
    NotFound,
    /// HTTP 403: the credentials are not allowed to perform the operation.
    PermissionDenied,
    /// HTTP 401: the request was not authenticated.
    Unauthenticated,
    /// The storage access token has expired locally; exchange a new one.
    CredentialsExpired,
    /// HTTP 409: the request conflicts with the current resource state.
    Conflict,
    /// HTTP 412, or the object changed between requests (for example its ETag
    /// changed during a ranged download).
    PreconditionFailed,
    /// HTTP 429: the service is throttling requests.
    RateLimited,
    /// HTTP 5xx.
    ServerError,
    /// The service answered with a response that breaks the protocol contract:
    /// empty or invalid JSON, an oversized body, or a receipt/listing that does
    /// not match the request.
    ProtocolViolation,
    /// The connect timeout, a total request deadline or the I/O idle timeout
    /// (`read_timeout`, which also covers stalled uploads) elapsed.
    Timeout,
    /// The connection failed or broke (DNS, TCP, TLS, reset) without a timeout.
    Transport,
    /// A local file-system or reader error.
    Io,
    /// The operation was cancelled cooperatively.
    Cancelled,
    /// A mutation may or may not have taken effect (copy, multipart completion,
    /// bulk deletion). Reconcile with the service; never retry blindly.
    OutcomeUnknown,
    /// A multi-batch operation stopped because a batch reported per-item
    /// failures; later items were not attempted. The outcome so far is in
    /// [`Error::partial_delete_output`].
    /// [`DeletePrefix`](crate::object::DeletePrefix) is the only producer; a
    /// single [`DeleteObjects`](crate::object::DeleteObjects) batch returns its
    /// per-object failures in `Ok`.
    PartialFailure,
    /// Any other failure, including HTTP statuses without a dedicated kind
    /// (other 4xx, unexpected 2xx/3xx) and internal SDK failures.
    Unexpected,
}

impl ErrorKind {
    /// The variant name, e.g. `"NotFound"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidInput => "InvalidInput",
            Self::ConfigInvalid => "ConfigInvalid",
            Self::NotFound => "NotFound",
            Self::PermissionDenied => "PermissionDenied",
            Self::Unauthenticated => "Unauthenticated",
            Self::CredentialsExpired => "CredentialsExpired",
            Self::Conflict => "Conflict",
            Self::PreconditionFailed => "PreconditionFailed",
            Self::RateLimited => "RateLimited",
            Self::ServerError => "ServerError",
            Self::ProtocolViolation => "ProtocolViolation",
            Self::Timeout => "Timeout",
            Self::Transport => "Transport",
            Self::Io => "Io",
            Self::Cancelled => "Cancelled",
            Self::OutcomeUnknown => "OutcomeUnknown",
            Self::PartialFailure => "PartialFailure",
            Self::Unexpected => "Unexpected",
        }
    }

    /// Classify a rejected HTTP status.
    pub(crate) fn from_status(status: StatusCode) -> Self {
        match status.as_u16() {
            401 => Self::Unauthenticated,
            403 => Self::PermissionDenied,
            404 => Self::NotFound,
            409 => Self::Conflict,
            412 => Self::PreconditionFailed,
            429 => Self::RateLimited,
            500..=599 => Self::ServerError,
            _ => Self::Unexpected,
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether the request the error is about reached the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dispatch {
    /// Nothing was sent: local validation, signing, request construction or
    /// connection establishment failed. Retrying cannot duplicate a mutation.
    NotSent,
    /// The service received the request and answered (an HTTP status line was
    /// received), including when the response body was later rejected.
    Sent,
    /// The request may have reached the service: the connection broke or timed
    /// out while sending or before a response arrived, or the local future was
    /// cancelled while the request was in flight.
    Unknown,
}

/// HTTP response facts retained on an [`Error`].
///
/// The request ID, service code and diagnostic body are untrusted strings
/// taken from the response. They are retained only with access-key
/// credentials; with OAuth or anonymous credentials they are always empty.
#[derive(Clone)]
pub struct ResponseDetails {
    pub(crate) status: StatusCode,
    pub(crate) service_code: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) body: String,
    pub(crate) truncated: bool,
}

impl ResponseDetails {
    pub(crate) fn new(status: StatusCode) -> Self {
        Self {
            status,
            service_code: None,
            request_id: None,
            body: String::new(),
            truncated: false,
        }
    }
    /// The HTTP status. It can be a success status when the body was rejected.
    pub fn status(&self) -> StatusCode {
        self.status
    }
    /// Best-effort `code` / `errorCode` from a JSON error body (at most 256 characters).
    pub fn service_code(&self) -> Option<&str> {
        self.service_code.as_deref()
    }
    /// Best-effort request ID from response headers or the JSON error body
    /// (at most 256 characters).
    pub fn request_id(&self) -> Option<&str> {
        self.request_id.as_deref()
    }
    /// At most 8 KiB of untrusted response text. May contain sensitive data.
    pub fn diagnostic_body(&self) -> &str {
        &self.body
    }
    /// Whether [`diagnostic_body`](Self::diagnostic_body) is incomplete.
    pub fn body_truncated(&self) -> bool {
        self.truncated
    }
    /// The service code when it looks like a plain identifier; anything else
    /// (which could echo request material) is withheld from formatting.
    fn printable_service_code(&self) -> Option<&str> {
        self.service_code.as_deref().and_then(printable_code)
    }
}

/// An untrusted service code, when it looks like a plain identifier (at most
/// 64 ASCII letters, digits, `_`, `-` or `.`) and so is safe to format.
pub(crate) fn printable_code(code: &str) -> Option<&str> {
    (code.len() <= 64
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
    .then_some(code)
}

impl fmt::Debug for ResponseDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("ResponseDetails");
        s.field("status", &self.status.as_u16());
        match (self.printable_service_code(), &self.service_code) {
            (Some(code), _) => s.field("service_code", &code),
            (None, Some(_)) => s.field("service_code", &format_args!("<withheld>")),
            (None, None) => &mut s,
        };
        s.field("body_len", &self.body.len())
            .field("body_truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}

/// The error returned by every fallible SDK operation, classified by
/// [`ErrorKind`].
///
/// The type is named `fds::Error` (like `std::io::Error`), not `FdsError`: the
/// crate path already says which SDK it belongs to. There are no other public
/// error types for SDK operations: bulk deletion, managed multipart uploads and
/// the downloader all return this one.
///
/// Every fallible SDK operation returns [`Result<T>`]. An `Error` is opaque:
/// inspect it through [`Error::kind`], [`Error::dispatch`], the optional HTTP
/// [`ResponseDetails`] and the standard [`std::error::Error::source`] chain.
///
/// ```
/// fn report(error: &fds::Error) {
///     match error.kind() {
///         fds::ErrorKind::NotFound => eprintln!("missing object or bucket"),
///         fds::ErrorKind::OutcomeUnknown => eprintln!("reconcile before retrying: {error}"),
///         _ if error.is_retryable() => eprintln!("transient: {error}"),
///         _ => eprintln!("failed: {error}"),
///     }
/// }
/// # report(&fds::Error::new(fds::ErrorKind::NotFound, "example"));
/// ```
///
/// # Redaction
///
/// `Display` and `Debug` never contain credentials, signatures, storage tokens,
/// credential-bearing URLs or raw response bodies. They print the kind, the
/// SDK's own message, the HTTP status, a service code that looks like a plain
/// identifier, attached context and the `Display` text of each source (transport
/// sources have their URL removed). The request ID and the bounded diagnostic
/// body are only available through explicit accessors because they are
/// untrusted, server-controlled text. With OAuth or anonymous credentials the
/// service code, request ID and body are not retained at all.
///
/// The value is a single pointer wide; all details live behind it.
pub struct Error(Box<Inner>);

struct Inner {
    kind: ErrorKind,
    message: Cow<'static, str>,
    dispatch: Dispatch,
    response: Option<ResponseDetails>,
    context: Vec<(&'static str, String)>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
    delete_output: Option<DeleteObjectsOutput>,
}

impl Error {
    /// Create an error. Dispatch defaults to [`Dispatch::NotSent`].
    pub fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self(Box::new(Inner {
            kind,
            message: message.into(),
            dispatch: Dispatch::NotSent,
            response: None,
            context: Vec::new(),
            source: None,
            delete_output: None,
        }))
    }

    /// Attach a key/value context entry. The value is formatted with
    /// `Display` and must not contain secrets.
    pub fn with_context(mut self, key: &'static str, value: impl fmt::Display) -> Self {
        self.0.context.push((key, value.to_string()));
        self
    }

    /// Set the underlying cause, exposed through [`std::error::Error::source`].
    pub fn with_source(mut self, source: impl Into<Box<dyn StdError + Send + Sync>>) -> Self {
        self.0.source = Some(source.into());
        self
    }

    /// The classification of this error.
    pub fn kind(&self) -> ErrorKind {
        self.0.kind
    }

    /// The SDK's description, without kind, status or source.
    pub fn message(&self) -> &str {
        &self.0.message
    }

    /// Whether the request reached the service.
    pub fn dispatch(&self) -> Dispatch {
        self.0.dispatch
    }

    /// HTTP response details, when a response was received.
    pub fn response(&self) -> Option<&ResponseDetails> {
        self.0.response.as_ref()
    }

    /// HTTP status, when a response was received.
    pub fn status(&self) -> Option<StatusCode> {
        self.response().map(ResponseDetails::status)
    }

    /// Untrusted service error code; see [`ResponseDetails::service_code`].
    pub fn service_code(&self) -> Option<&str> {
        self.response()?.service_code()
    }

    /// Untrusted request ID; see [`ResponseDetails::request_id`].
    pub fn request_id(&self) -> Option<&str> {
        self.response()?.request_id()
    }

    /// Attached key/value context, in insertion order.
    pub fn context(&self) -> &[(&'static str, String)] {
        &self.0.context
    }

    /// `kind() == ErrorKind::NotFound`.
    pub fn is_not_found(&self) -> bool {
        self.kind() == ErrorKind::NotFound
    }

    /// Whether repeating the same request may succeed and cannot be harmful
    /// beyond the operation's own idempotency: rate limiting, HTTP 502/503/504,
    /// timeouts, and transport failures before a response arrived (whether or
    /// not the request reached the service). Never true for `OutcomeUnknown`,
    /// for HTTP 500 or for a transport failure while reading a received
    /// response body.
    ///
    /// This is exactly the rule the SDK's [`RetryPolicy`](crate::RetryPolicy)
    /// applies to each attempt; whether an operation is repeated at all also
    /// depends on its classification there. A `true` does not make a
    /// non-idempotent write safe to repeat when [`dispatch`](Self::dispatch)
    /// is not [`Dispatch::NotSent`].
    pub fn is_retryable(&self) -> bool {
        crate::retry::is_transient(self.kind(), self.status(), self.dispatch())
    }

    /// `kind() == ErrorKind::OutcomeUnknown`.
    pub fn is_outcome_unknown(&self) -> bool {
        self.kind() == ErrorKind::OutcomeUnknown
    }

    /// Outcomes of bulk deletion confirmed before it failed: deleted names,
    /// per-object failures and names whose outcome is unknown.
    ///
    /// Present on [`ErrorKind::OutcomeUnknown`] errors of
    /// [`DeleteObjects::send`](crate::object::DeleteObjects::send) (every name
    /// unconfirmed) and on every error of
    /// [`DeletePrefix::send`](crate::object::DeletePrefix::send) (all pages
    /// handled so far). `None` for every other error.
    pub fn partial_delete_output(&self) -> Option<&DeleteObjectsOutput> {
        self.0.delete_output.as_ref()
    }

    // ---- crate-internal construction ---------------------------------------

    pub(crate) fn invalid_input(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    pub(crate) fn config_invalid(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::ConfigInvalid, message)
    }

    /// A response violated the protocol; the request was sent.
    pub(crate) fn protocol(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::ProtocolViolation, message).with_dispatch(Dispatch::Sent)
    }

    pub(crate) fn unexpected(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Unexpected, message)
    }

    pub(crate) fn with_dispatch(mut self, dispatch: Dispatch) -> Self {
        self.0.dispatch = dispatch;
        self
    }

    pub(crate) fn with_response(mut self, response: ResponseDetails) -> Self {
        self.0.response = Some(response);
        self
    }

    pub(crate) fn response_mut(&mut self) -> Option<&mut ResponseDetails> {
        self.0.response.as_mut()
    }

    pub(crate) fn with_delete_output(mut self, output: DeleteObjectsOutput) -> Self {
        self.0.delete_output = Some(output);
        self
    }

    pub(crate) fn take_delete_output(&mut self) -> Option<DeleteObjectsOutput> {
        self.0.delete_output.take()
    }

    /// Wrap a post-dispatch failure of a mutation whose effect is unknown.
    /// Keeps dispatch and response details visible on the outer error.
    pub(crate) fn into_outcome_unknown(self, message: impl Into<Cow<'static, str>>) -> Self {
        let mut outer =
            Self::new(ErrorKind::OutcomeUnknown, message).with_dispatch(match self.dispatch() {
                Dispatch::NotSent => Dispatch::Unknown,
                other => other,
            });
        outer.0.response = self.0.response.clone();
        outer.with_source(self)
    }

    /// A `reqwest` failure while sending a request (before any response).
    pub(crate) fn send_failure(error: reqwest::Error) -> Self {
        let (kind, dispatch, message) = if error.is_builder() {
            (
                ErrorKind::InvalidInput,
                Dispatch::NotSent,
                "failed to build HTTP request",
            )
        } else if error.is_connect() {
            let kind = if error.is_timeout() {
                ErrorKind::Timeout
            } else {
                ErrorKind::Transport
            };
            (kind, Dispatch::NotSent, "failed to connect")
        } else if error.is_timeout() {
            (
                ErrorKind::Timeout,
                Dispatch::Unknown,
                "request timed out before a response arrived",
            )
        } else {
            (
                ErrorKind::Transport,
                Dispatch::Unknown,
                "request failed before a response arrived",
            )
        };
        Self::new(kind, message)
            .with_dispatch(dispatch)
            .with_source(error.without_url())
    }

    /// Neither upload progress nor response headers within `read_timeout`.
    /// The request may already have reached the service.
    pub(crate) fn idle_timeout() -> Self {
        Self::new(
            ErrorKind::Timeout,
            "request timed out before a response arrived",
        )
        .with_dispatch(Dispatch::Unknown)
        .with_source(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "no request progress or response within the read timeout",
        ))
    }

    /// A `reqwest` failure while reading a received response body.
    pub(crate) fn body_failure(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Transport
        };
        Self::new(kind, "failed to read response body")
            .with_dispatch(Dispatch::Sent)
            .with_source(error.without_url())
    }

    /// Local JSON serialization of a request body or checkpoint failed.
    pub(crate) fn json_encode(error: serde_json::Error) -> Self {
        let kind = if error.is_io() {
            ErrorKind::Io
        } else {
            ErrorKind::Unexpected
        };
        // serde's message can echo serialized values; keep only the category.
        Self::new(kind, "JSON serialization failed")
            .with_context("category", format_args!("{:?}", error.classify()))
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, "local IO failed").with_source(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.kind, self.0.message)?;
        if let Some(response) = &self.0.response {
            write!(f, " (HTTP {}", response.status.as_u16())?;
            if let Some(code) = response.printable_service_code() {
                write!(f, ", service code {code}")?;
            }
            f.write_str(")")?;
        }
        if !self.0.context.is_empty() {
            f.write_str(" [")?;
            for (index, (key, value)) in self.0.context.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{key}: {value}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

/// Formats each source in the chain with `Display`, never `Debug`: foreign
/// `Debug` output (for example a transport error) is not redaction-reviewed.
struct SourceChain<'a>(Option<&'a (dyn StdError + 'static)>);
impl fmt::Debug for SourceChain<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_list();
        let mut next = self.0;
        while let Some(error) = next {
            list.entry(&format_args!("{error}"));
            next = error.source();
        }
        list.finish()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = &*self.0;
        let mut s = f.debug_struct("Error");
        s.field("kind", &inner.kind)
            .field("message", &inner.message)
            .field("dispatch", &inner.dispatch);
        if let Some(response) = &inner.response {
            s.field("response", response);
        }
        if !inner.context.is_empty() {
            s.field("context", &inner.context);
        }
        if let Some(output) = &inner.delete_output {
            s.field("delete_output", output);
        }
        if let Some(source) = StdError::source(self) {
            s.field("source", &SourceChain(Some(source)));
        }
        s.finish()
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.0.source.as_deref().map(|e| e as _)
    }
}

// serde's error text may echo an unknown variant containing a token. Retain
// parser location/category as the source without retaining that unsafe message.
#[derive(Debug)]
pub(crate) struct JsonResponseSource {
    pub category: serde_json::error::Category,
    pub line: usize,
    pub column: usize,
}
impl fmt::Display for JsonResponseSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "JSON response decoding failed ({:?}) at line {}, column {}",
            self.category, self.line, self.column
        )
    }
}
impl StdError for JsonResponseSource {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_is_pointer_sized() {
        assert_eq!(std::mem::size_of::<Error>(), std::mem::size_of::<usize>());
        assert_eq!(
            std::mem::size_of::<Result<()>>(),
            std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn status_mapping() {
        for (status, kind) in [
            (401, ErrorKind::Unauthenticated),
            (403, ErrorKind::PermissionDenied),
            (404, ErrorKind::NotFound),
            (409, ErrorKind::Conflict),
            (412, ErrorKind::PreconditionFailed),
            (429, ErrorKind::RateLimited),
            (500, ErrorKind::ServerError),
            (503, ErrorKind::ServerError),
            (599, ErrorKind::ServerError),
            (400, ErrorKind::Unexpected),
            (405, ErrorKind::Unexpected),
            (201, ErrorKind::Unexpected),
            (302, ErrorKind::Unexpected),
        ] {
            assert_eq!(
                ErrorKind::from_status(StatusCode::from_u16(status).unwrap()),
                kind,
                "{status}"
            );
        }
    }

    fn with_status(kind: ErrorKind, status: u16) -> Error {
        Error::new(kind, "x")
            .with_dispatch(Dispatch::Sent)
            .with_response(ResponseDetails::new(StatusCode::from_u16(status).unwrap()))
    }

    #[test]
    fn retryability() {
        assert!(with_status(ErrorKind::RateLimited, 429).is_retryable());
        for status in [502, 503, 504] {
            assert!(with_status(ErrorKind::ServerError, status).is_retryable());
        }
        assert!(!with_status(ErrorKind::ServerError, 500).is_retryable());
        assert!(!with_status(ErrorKind::NotFound, 404).is_retryable());
        assert!(Error::new(ErrorKind::Timeout, "t")
            .with_dispatch(Dispatch::Unknown)
            .is_retryable());
        assert!(Error::new(ErrorKind::Transport, "t").is_retryable());
        // The request may have been sent: transient, like a timeout.
        assert!(Error::new(ErrorKind::Transport, "t")
            .with_dispatch(Dispatch::Unknown)
            .is_retryable());
        // A broken response body is not repeated by the retry loop either.
        assert!(!Error::new(ErrorKind::Transport, "t")
            .with_dispatch(Dispatch::Sent)
            .is_retryable());
        let unknown = with_status(ErrorKind::ServerError, 503).into_outcome_unknown("u");
        assert!(unknown.is_outcome_unknown() && !unknown.is_retryable());
        assert_eq!(unknown.status(), Some(StatusCode::SERVICE_UNAVAILABLE));
        assert_eq!(unknown.dispatch(), Dispatch::Sent);
        assert!(Error::new(ErrorKind::NotFound, "n").is_not_found());
    }

    #[test]
    fn formatting_is_diagnostic_and_redacted() {
        let mut response = ResponseDetails::new(StatusCode::FORBIDDEN);
        response.service_code = Some("AccessDenied".into());
        response.request_id = Some("private-request-id".into());
        response.body = "private-body Galaxy-V2 private-ak:private-signature".into();
        let error = Error::new(ErrorKind::PermissionDenied, "FDS rejected the request")
            .with_dispatch(Dispatch::Sent)
            .with_response(response)
            .with_context("bucket", "b")
            .with_source(std::io::Error::other("inner cause"));
        let display = error.to_string();
        assert_eq!(
            display,
            "PermissionDenied: FDS rejected the request (HTTP 403, service code AccessDenied) [bucket: b]"
        );
        let debug = format!("{error:?} {error:#?}");
        for expected in [
            "PermissionDenied",
            "403",
            "AccessDenied",
            "Sent",
            "inner cause",
        ] {
            assert!(debug.contains(expected), "{debug}");
        }
        for secret in ["private-", "Galaxy-V2"] {
            assert!(!format!("{display} {debug}").contains(secret), "{debug}");
        }
        // A service code that is not a plain identifier is withheld.
        let mut response = ResponseDetails::new(StatusCode::FORBIDDEN);
        response.service_code = Some("Galaxy-V2 AK:private-signature".into());
        let error = Error::new(ErrorKind::PermissionDenied, "x").with_response(response);
        let formatted = format!("{error} {error:?}");
        assert!(!formatted.contains("private-") && formatted.contains("withheld"));
        assert_eq!(error.service_code(), Some("Galaxy-V2 AK:private-signature"));
    }

    #[test]
    fn json_encode_error_omits_serde_message() {
        let mut map = std::collections::BTreeMap::new();
        map.insert(vec![1u8], "private-value");
        let error = Error::json_encode(serde_json::to_vec(&map).unwrap_err());
        assert_eq!(error.kind(), ErrorKind::Unexpected);
        assert!(!format!("{error} {error:?}").contains("private-"));
    }

    #[tokio::test]
    async fn connection_refused_is_not_sent_and_removes_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/secret-object?storageAccessToken=private-token&Signature=private-signature",
            listener.local_addr().unwrap()
        );
        drop(listener);
        let raw = reqwest::Client::new().get(url).send().await.unwrap_err();
        assert!(raw.url().is_some());
        let error = Error::send_failure(raw);
        assert_eq!(error.kind(), ErrorKind::Transport);
        assert_eq!(error.dispatch(), Dispatch::NotSent);
        assert!(error.is_retryable());
        let mut diagnostics = format!("{error} {error:?}");
        let mut source = error.source();
        while let Some(e) = source {
            diagnostics.push_str(&format!("{e} {e:?}"));
            source = e.source();
        }
        for secret in [
            "secret-object",
            "private-token",
            "private-signature",
            "storageAccessToken",
            "Signature",
        ] {
            assert!(!diagnostics.contains(secret), "{diagnostics}");
        }
    }
}
