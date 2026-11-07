//! HTTP client core and request pipeline.
//!
//! Byte-critical, like signing: URL construction and the exact ordering of
//! "assemble headers → set Date → sign → set Authorization → send". See the
//! request pipeline below.

use crate::upload::UploadBody;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::auth;
use crate::config::{ClientBuilder, ClientConfiguration};
use crate::constant::{HTTP_HEADER_AUTHORIZATION, HTTP_HEADER_DATE};
use crate::credentials::{Credentials, CredentialsProvider};
use crate::encoding::{go_query_escape, QueryValues};
use crate::error::{Dispatch, Error, ErrorKind, JsonResponseSource, ResponseDetails, Result};
use crate::object::UserMetadata;
use http::Method;
use std::{borrow::Cow, fmt, sync::Arc, time::Duration};

/// A validated FDS client. Clones share immutable configuration, the private
/// credentials provider and a connection pool; no caller-owned Arc is
/// required.
///
/// ```
/// # fn main() -> fds::Result<()> {
/// let client = fds::Client::builder()
///     .endpoint("https://cnbj1.fds.api.xiaomi.com")
///     .credentials("access-id", "secret")
///     .build()?;
/// let shared = client.clone();
/// assert_eq!(shared.configuration().endpoint(), client.configuration().endpoint());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    http: reqwest::Client,
    configuration: ClientConfiguration,
    auth: Auth,
}

/// Where a client's credentials come from.
pub(crate) enum Auth {
    /// Validated at build time; the same for every request.
    Static(Credentials),
    /// Asked before every attempt and validated each time.
    Provider(Arc<dyn CredentialsProvider>),
    /// The internal discovery and token-exchange channel: no authentication.
    Anonymous,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("configuration", &self.inner.configuration)
            .finish_non_exhaustive()
    }
}

/// Normalized internal request with an owned, known-length body.
pub(crate) struct ClientRequest<'a> {
    pub bucket: &'a str,
    pub object: &'a str,
    pub method: Method,
    pub query: QueryValues,
    pub headers: HeaderMap,
    /// Appended as `x-xiaomi-meta-*` headers after `headers`.
    pub user_metadata: Option<&'a UserMetadata>,
    pub body: Option<UploadBody>,
    pub replay_safety: crate::retry::ReplaySafety,
    pub accepted_statuses: &'static [u16],
    /// Per-attempt total deadline overriding the client-wide `request_timeout`;
    /// covers consumption of the response body. Callers validate it first.
    pub timeout: Option<Duration>,
    /// Fail before sending unless the attempt's credentials are a storage
    /// token (the app-scoped listing).
    pub requires_storage_token: bool,
}

impl<'a> ClientRequest<'a> {
    pub(crate) fn requires_storage_token(mut self) -> Self {
        self.requires_storage_token = true;
        self
    }

    pub(crate) fn read_only(mut self) -> Self {
        self.replay_safety = crate::retry::ReplaySafety::ReadOnly;
        self
    }

    /// Apply an operation builder's per-request deadline; zero or
    /// unrepresentable values are an invalid-input error.
    pub(crate) fn with_timeout(mut self, timeout: Option<Duration>) -> Result<Self> {
        self.timeout = timeout
            .map(|t| crate::config::validate_timeout(t, ErrorKind::InvalidInput))
            .transpose()?;
        Ok(self)
    }

    /// A bare request with empty query/headers/metadata/body.
    pub(crate) fn new(bucket: &'a str, object: &'a str, method: Method) -> Self {
        Self {
            bucket,
            object,
            method,
            query: QueryValues::new(),
            headers: HeaderMap::new(),
            user_metadata: None,
            body: None,
            accepted_statuses: &[200],
            replay_safety: crate::retry::ReplaySafety::Never,
            timeout: None,
            requires_storage_token: false,
        }
    }
}

impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    pub fn configuration(&self) -> &ClientConfiguration {
        &self.inner.configuration
    }

    /// The access key used for presigning: presigning is synchronous, so only
    /// static access-key credentials can sign.
    pub(crate) fn access_keys(&self) -> Result<(&str, &str)> {
        match &self.inner.auth {
            Auth::Static(credentials) => credentials.access_key_pair(),
            _ => None,
        }
        .ok_or_else(|| {
            Error::config_invalid("Galaxy-V2 presigning requires access key credentials")
        })
    }

    pub(crate) fn with_credentials(auth: Auth, conf: ClientConfiguration) -> Result<Self> {
        if let Auth::Static(credentials) = &auth {
            credentials.validate(&conf)?;
        }
        // `read_timeout` is deliberately not given to reqwest: its version is
        // a fixed deadline until the response headers, which upload progress
        // does not reset. `send` enforces it as an idle timeout instead.
        let mut builder = reqwest::Client::builder()
            .connect_timeout(conf.connect_timeout)
            .pool_idle_timeout(conf.pool_idle_timeout)
            .tcp_keepalive(conf.tcp_keepalive)
            .pool_max_idle_per_host(conf.pool_max_idle_per_host)
            // Added by reqwest after signing; never part of the signature.
            .user_agent(conf.user_agent.as_str())
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never());
        // No total deadline unless explicitly configured: reqwest's timeout
        // also covers body consumption and would cut off long transfers.
        if let Some(timeout) = conf.request_timeout {
            builder = builder.timeout(timeout);
        }
        #[cfg(feature = "cslb")]
        {
            let resolver = crate::cslb::CslbResolver::new(conf.cslb_max_nodes, true, true);
            builder = builder.dns_resolver(Arc::new(resolver));
        }
        Ok(Self {
            inner: Arc::new(ClientInner {
                http: builder.build().map_err(|error| {
                    Error::unexpected("failed to build HTTP client")
                        .with_source(error.without_url())
                })?,
                configuration: conf,
                auth,
            }),
        })
    }

    /// Run a request and return the raw response. `cdn` is always false in the
    /// request pipeline (only presigned URLs may use the CDN host).
    pub(crate) async fn do_request(&self, req: ClientRequest<'_>) -> Result<reqwest::Response> {
        self.do_request_redacted(req).await.map(|(resp, _)| resp)
    }

    /// [`do_request`](Self::do_request), also returning the redaction policy
    /// of the credentials the successful attempt used, for body errors.
    async fn do_request_redacted(
        &self,
        req: ClientRequest<'_>,
    ) -> Result<(reqwest::Response, Redaction)> {
        // Only `Sync` parts of the request may be held across the provider's
        // await: the body is not.
        let target = Target {
            bucket: req.bucket,
            object: req.object,
            query: &req.query,
            requires_storage_token: req.requires_storage_token,
        };
        let mut attempt = self.prepare_attempt(&target).await?;

        let mut headers = req.headers;
        if let Some(metadata) = req.user_metadata {
            metadata.append_to_headers(&mut headers)?;
        }

        let mut body = req.body;
        let eligible = matches!(req.replay_safety, crate::retry::ReplaySafety::ReadOnly)
            && body.as_ref().is_none_or(|body| body.try_clone().is_some());
        let policy = &self.inner.configuration.retry_policy;
        let mut retries = 0;
        loop {
            let replay = if eligible {
                body.as_ref().and_then(UploadBody::try_clone)
            } else {
                None
            };
            let redaction = Redaction::for_credentials(attempt.credentials.as_deref());
            let result = self
                .send(
                    req.method.clone(),
                    attempt.url,
                    headers.clone(),
                    body.take(),
                    req.timeout,
                    attempt.credentials.as_deref(),
                )
                .await;
            let delay = if eligible {
                match &result {
                    Ok(resp)
                        if !req.accepted_statuses.contains(&resp.status().as_u16())
                            && matches!(resp.status().as_u16(), 429 | 502 | 503 | 504) =>
                    {
                        policy.delay(retries, Some(resp.headers()))
                    }
                    Err(SendFailure::Transport(error))
                        if !error.is_body()
                            && (error.is_connect() || error.is_timeout() || error.is_request()) =>
                    {
                        policy.delay(retries, None)
                    }
                    Err(SendFailure::IdleTimeout) => policy.delay(retries, None),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(delay) = delay {
                drop(result);
                tokio::time::sleep(delay).await;
                body = replay;
                retries += 1;
                // Every attempt asks for the then-current credentials: a
                // refreshed token changes the URL, a rotated key the signature.
                attempt = self.prepare_attempt(&target).await?;
            } else {
                let resp = result.map_err(SendFailure::into_error)?;
                return match check_response_status(resp, req.accepted_statuses).await {
                    Ok(resp) => Ok((resp, redaction)),
                    Err(error) => Err(redaction.error(error)),
                };
            }
        }
    }

    /// The credentials for one attempt (`None`: anonymous), validated, and
    /// the URL carrying a storage token's query parameters. Fails before
    /// sending.
    async fn prepare_attempt(&self, req: &Target<'_>) -> Result<Attempt<'_>> {
        let credentials = match &self.inner.auth {
            Auth::Static(credentials) => Some(Cow::Borrowed(credentials)),
            Auth::Provider(provider) => {
                let credentials = provider.provide_credentials().await?;
                credentials.validate(&self.inner.configuration)?;
                Some(Cow::Owned(credentials))
            }
            Auth::Anonymous => None,
        };
        let token = credentials
            .as_deref()
            .and_then(Credentials::storage_access_token);
        if req.requires_storage_token && token.is_none() {
            return Err(Error::config_invalid(
                "app bucket listing requires an exchanged storage token",
            ));
        }
        let mut query = Cow::Borrowed(req.query);
        if let Some(token) = token {
            // Static tokens were validated at build time; check expiry again.
            token.validate()?;
            let query = query.to_mut();
            query.add("appId", token.app_id());
            query.add("storageAccessToken", token.token());
        }
        let url = self.build_request_url(req.bucket, req.object, &query.encode(), false)?;
        Ok(Attempt { credentials, url })
    }

    /// Decode a required JSON response; an empty body is a protocol error.
    pub(crate) async fn do_json<T>(&self, req: ClientRequest<'_>) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        self.do_json_limited(req, usize::MAX).await
    }

    pub(crate) async fn do_json_limited<T: serde::de::DeserializeOwned>(
        &self,
        req: ClientRequest<'_>,
        limit: usize,
    ) -> Result<T> {
        let (mut resp, redaction) = self.do_request_redacted(req).await?;
        let details = response_details(&resp);
        let mut bytes = Vec::new();
        while let Some(chunk) = match resp.chunk().await {
            Ok(chunk) => chunk,
            Err(source) => {
                return Err(redaction.error(Error::body_failure(source).with_response(details)));
            }
        } {
            if chunk.len() > limit.saturating_sub(bytes.len()) {
                return Err(redaction.error(
                    Error::protocol("response body exceeds the operation's limit")
                        .with_response(details),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(redaction.error(
                Error::protocol("required JSON response body is empty").with_response(details),
            ));
        }
        serde_json::from_slice(&bytes).map_err(|source| {
            let mut details = details;
            set_diagnostic_body(&mut details, &bytes, bytes.len() > DIAGNOSTIC_LIMIT);
            redaction.error(
                Error::protocol("invalid JSON response body")
                    .with_response(details)
                    .with_source(JsonResponseSource {
                        category: source.classify(),
                        line: source.line(),
                        column: source.column(),
                    }),
            )
        })
    }

    /// Convenience: run a request and discard the response body.
    pub(crate) async fn do_discard(&self, req: ClientRequest<'_>) -> Result<()> {
        let _ = self.do_request(req).await?;
        Ok(())
    }

    /// The core send: body handling → Date → sign → Authorization → send.
    /// Ordering is load-bearing.
    async fn send(
        &self,
        method: Method,
        url: url::Url,
        mut headers: HeaderMap,
        body: Option<UploadBody>,
        timeout: Option<Duration>,
        credentials: Option<&Credentials>,
    ) -> std::result::Result<reqwest::Response, SendFailure> {
        // (3) body: optional Content-MD5 (must be set BEFORE signing). The
        // clock starts now: upload progress and the header wait are measured
        // against `read_timeout` from here.
        let idle = self.inner.configuration.read_timeout;
        let clock = crate::idle::ProgressClock::new();
        let body = body
            .map(|body| {
                body.prepare(
                    &mut headers,
                    self.inner.configuration.enable_md5_calculate,
                    clock.clone(),
                )
            })
            .transpose()?;

        // (6) Date — GMT; the SAME string is signed and sent.
        let date = chrono::Utc::now()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        headers.insert(
            HeaderName::from_static(HTTP_HEADER_DATE),
            header_value(&date)?,
        );

        // (7) Authorization from this attempt's credentials, signed over the
        // Date set above.
        if let Some(credentials) = credentials {
            let value = match credentials.access_key_pair() {
                Some((id, secret)) => {
                    let sig = auth::sign(secret, method.as_str(), url.as_str(), &headers)?;
                    let mut value = HeaderValue::from_str(&format!("Galaxy-V2 {id}:{sig}"))
                        .map_err(|_| Error::config_invalid("invalid authorization header"))?;
                    value.set_sensitive(true);
                    value
                }
                None => HeaderValue::from_static("OAuth"),
            };
            headers.insert(HeaderName::from_static(HTTP_HEADER_AUTHORIZATION), value);
        }

        tracing::debug!(method = method.as_str(), ">>> FDS request");

        // (9) send. `.body()` (never `.json()`) so reqwest does not inject a
        // Content-Type that would corrupt the signature.
        let mut rb = self.inner.http.request(method, url).headers(headers);
        if let Some(b) = body {
            rb = rb.body(b);
        }
        // Overrides the client-wide deadline for this attempt only.
        if let Some(timeout) = timeout {
            rb = rb.timeout(timeout);
        }
        // Idle watchdog until the response headers arrive: fires once neither
        // a body piece was pulled nor headers arrived for `read_timeout`.
        // Dropping the send future aborts the attempt.
        let resp = tokio::select! {
            biased;
            result = rb.send() => result.map_err(SendFailure::Transport)?,
            () = clock.stalled(idle) => return Err(SendFailure::IdleTimeout),
        };

        // Rebuild the response around an idle-timed body, retaining the
        // headers, status and transport extensions. The request URL (which
        // carries the storage token in OAuth mode) is not carried over: no
        // returned type exposes a response URL.
        let raw: http::Response<reqwest::Body> = resp.into();
        let (parts, body) = raw.into_parts();
        let body = reqwest::Body::wrap(crate::idle::IdleBody::new(body, idle));
        let resp = reqwest::Response::from(http::Response::from_parts(parts, body));

        // Status/retry decisions belong to the operation-aware caller.
        Ok(resp)
    }

    /// Build the request URL. The object name is `go_query_escape`d then
    /// `+`→`%20` (so `/`→`%2F`, space→`%20`); the bucket is not encoded. The
    /// pre-encoded string is parsed once and never re-encoded downstream.
    pub(crate) fn build_request_url(
        &self,
        bucket: &str,
        object: &str,
        params: &str,
        cdn: bool,
    ) -> Result<url::Url> {
        let endpoint = if cdn {
            self.inner
                .configuration
                .cdn_endpoint
                .as_ref()
                .ok_or_else(|| Error::config_invalid("CDN endpoint is not configured"))?
        } else {
            &self.inner.configuration.endpoint
        };

        let escaped = go_query_escape(object).replace('+', "%20");

        let mut s = endpoint.as_str().to_owned();
        if !bucket.is_empty() {
            s.push_str(bucket);
        }
        if !object.is_empty() {
            s.push('/');
            s.push_str(&escaped);
        }
        if !params.is_empty() {
            s.push('?');
            s.push_str(params);
        }

        url::Url::parse(&s).map_err(|e| Error::invalid_input(format!("build_request_url: {e}")))
    }
}

const DIAGNOSTIC_LIMIT: usize = 8192;

/// A header value built by the SDK or from caller input; bytes that are not
/// valid in a header are an invalid-input error.
pub(crate) fn header_value(value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|e| Error::invalid_input(e.to_string()))
}

/// The single redaction policy, decided per request by the credentials that
/// request actually used.
///
/// Access-key requests are signed in headers, so response URLs carry no
/// secrets and server diagnostics are kept. Every other request redacts: a
/// storage token travels in the query string, and the anonymous
/// discovery/exchange channel carries OAuth inputs in the query and must not
/// echo server text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Redaction {
    Keep,
    Redact,
}

impl Redaction {
    fn for_credentials(credentials: Option<&Credentials>) -> Self {
        match credentials.and_then(Credentials::access_key_pair) {
            Some(_) => Self::Keep,
            None => Self::Redact,
        }
    }

    /// Server-controlled response strings are dropped when redacting.
    fn error(self, mut error: Error) -> Error {
        if self == Self::Redact {
            if let Some(response) = error.response_mut() {
                response.body.clear();
                response.truncated = false;
                response.service_code = None;
                response.request_id = None;
            }
        }
        error
    }
}

/// What [`Client::prepare_attempt`] needs from a request.
struct Target<'r> {
    bucket: &'r str,
    object: &'r str,
    query: &'r QueryValues,
    requires_storage_token: bool,
}

/// One attempt's credentials and the URL built for them.
struct Attempt<'c> {
    credentials: Option<Cow<'c, Credentials>>,
    url: url::Url,
}

/// A failed attempt: local preparation errors are final, transport errors and
/// idle timeouts may be retried by `do_request` for eligible requests.
pub(crate) enum SendFailure {
    Local(Error),
    Transport(reqwest::Error),
    /// No upload progress and no response headers within `read_timeout`.
    IdleTimeout,
}
impl From<Error> for SendFailure {
    fn from(error: Error) -> Self {
        Self::Local(error)
    }
}
impl SendFailure {
    fn into_error(self) -> Error {
        match self {
            Self::Local(error) => error,
            Self::Transport(error) => Error::send_failure(error),
            Self::IdleTimeout => Error::idle_timeout(),
        }
    }
}

fn response_details(resp: &reqwest::Response) -> ResponseDetails {
    let mut details = ResponseDetails::new(resp.status());
    // Best-effort header conventions, not a required FDS response schema.
    details.request_id = ["x-xiaomi-request-id", "x-request-id"]
        .iter()
        .find_map(|name| {
            resp.headers()
                .get(*name)?
                .to_str()
                .ok()
                .map(|s| s.chars().take(256).collect())
        });
    details
}

fn set_diagnostic_body(details: &mut ResponseDetails, bytes: &[u8], truncated: bool) {
    let end = bytes.len().min(DIAGNOSTIC_LIMIT);
    details.body = String::from_utf8_lossy(&bytes[..end]).into_owned();
    // Lossy UTF-8 replacement can expand the byte count.
    let mut end = details.body.len().min(DIAGNOSTIC_LIMIT);
    while !details.body.is_char_boundary(end) {
        end -= 1;
    }
    details.truncated = truncated || end < details.body.len();
    details.body.truncate(end);
    if !details.truncated {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) {
            let text = |key: &str| {
                value.get(key).and_then(|v| match v {
                    serde_json::Value::String(s) => Some(s.chars().take(256).collect::<String>()),
                    serde_json::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
            };
            details.service_code = text("code").or_else(|| text("errorCode"));
            if details.request_id.is_none() {
                details.request_id = text("requestId");
            }
        }
    }
}

/// Only an operation's explicit allowlist may produce success. A rejected
/// status maps to its [`ErrorKind`]; the request was dispatched.
async fn check_response_status(
    mut resp: reqwest::Response,
    allowed: &[u16],
) -> Result<reqwest::Response> {
    if allowed.contains(&resp.status().as_u16()) {
        return Ok(resp);
    }
    let status = resp.status();
    let mut details = response_details(&resp);
    let mut bytes = Vec::new();
    let mut read_failure = None;
    // Read only a bounded prefix, including one byte to detect truncation.
    while bytes.len() <= DIAGNOSTIC_LIMIT {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let take = chunk.len().min(DIAGNOSTIC_LIMIT + 1 - bytes.len());
                bytes.extend_from_slice(&chunk[..take]);
            }
            Ok(None) => break,
            Err(source) => {
                read_failure = Some(source.without_url());
                break;
            }
        }
    }
    let incomplete = bytes.len() > DIAGNOSTIC_LIMIT || read_failure.is_some();
    set_diagnostic_body(&mut details, &bytes, incomplete);
    let message = if status.as_u16() >= 400 {
        "FDS rejected the request"
    } else {
        "unexpected HTTP status for this operation"
    };
    let mut error = Error::new(ErrorKind::from_status(status), message)
        .with_dispatch(Dispatch::Sent)
        .with_response(details);
    if let Some(source) = read_failure {
        error = error.with_source(source);
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Client {
        Client::builder()
            .endpoint("https://cnbj1.fds.api.xiaomi.com")
            .cdn_endpoint("https://cdn.cnbj1.fds.api.mi-img.com")
            .credentials("ak", "sk")
            .build()
            .unwrap()
    }

    #[test]
    fn url_plain_object() {
        let c = test_client();
        let u = c.build_request_url("b", "a/b.txt", "", false).unwrap();
        assert_eq!(u.as_str(), "https://cnbj1.fds.api.xiaomi.com/b/a%2Fb.txt");
    }

    #[test]
    fn url_object_with_dirs_encodes_slash() {
        let c = test_client();
        let u = c
            .build_request_url("hello", "dir1/dir2/build.log", "", false)
            .unwrap();
        assert_eq!(
            u.as_str(),
            "https://cnbj1.fds.api.xiaomi.com/hello/dir1%2Fdir2%2Fbuild.log"
        );
    }

    #[test]
    fn url_object_with_space_and_plus() {
        let c = test_client();
        let u = c.build_request_url("b", "a b/c.txt", "", false).unwrap();
        assert_eq!(
            u.as_str(),
            "https://cnbj1.fds.api.xiaomi.com/b/a%20b%2Fc.txt"
        );
        let u2 = c.build_request_url("b", "x+y", "", false).unwrap();
        assert_eq!(u2.as_str(), "https://cnbj1.fds.api.xiaomi.com/b/x%2By");
    }

    #[test]
    fn url_unicode_object() {
        let c = test_client();
        let u = c
            .build_request_url("b", "中文/文件.txt", "", false)
            .unwrap();
        assert_eq!(
            u.as_str(),
            "https://cnbj1.fds.api.xiaomi.com/b/%E4%B8%AD%E6%96%87%2F%E6%96%87%E4%BB%B6.txt"
        );
    }

    #[test]
    fn url_bucket_only_and_root() {
        let c = test_client();
        let u = c.build_request_url("b", "", "", false).unwrap();
        assert_eq!(u.as_str(), "https://cnbj1.fds.api.xiaomi.com/b");
        let root = c.build_request_url("", "", "", false).unwrap();
        assert_eq!(root.as_str(), "https://cnbj1.fds.api.xiaomi.com/");
    }

    #[test]
    fn url_with_params() {
        let c = test_client();
        let u = c.build_request_url("b", "o", "acl=", false).unwrap();
        assert_eq!(u.as_str(), "https://cnbj1.fds.api.xiaomi.com/b/o?acl=");
    }

    #[test]
    fn url_cdn_host() {
        let c = test_client();
        let u = c.build_request_url("b", "o", "", true).unwrap();
        assert_eq!(u.as_str(), "https://cdn.cnbj1.fds.api.mi-img.com/b/o");
    }

    #[test]
    fn url_encoding_survives_roundtrip_to_signed_resource() {
        // The pre-encoded path must survive url::parse so the signer decodes it
        // back to the literal object name.
        let c = test_client();
        let u = c
            .build_request_url("b", "dir a/b c.txt", "", false)
            .unwrap();
        let decoded = percent_encoding::percent_decode_str(u.path())
            .decode_utf8_lossy()
            .into_owned();
        assert_eq!(decoded, "/b/dir a/b c.txt");
    }
}

#[cfg(test)]
mod response_security_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Capture(Arc<Mutex<String>>);
    impl tracing::field::Visit for Capture {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .push_str(&format!("{}={value:?}\n", field.name()));
        }
    }
    impl tracing::Subscriber for Capture {
        fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
            metadata.target().starts_with("fds::")
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            values.record(&mut self.clone());
        }
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut self.clone());
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn request_trace_omits_tokens_keys_and_signatures() {
        const CHILD_ENV: &str = "FDS_REQUEST_TRACE_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            // Callsite interests are process-global. Run this private-request
            // test alone so parallel tests cannot initialize its callsites with
            // a different subscriber. Keep arbitrary sensitive query coverage.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "client::response_security_tests::request_trace_omits_tokens_keys_and_signatures",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .expect("start isolated tracing test");
            assert!(
                output.status.success(),
                "isolated tracing test failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }
        let captured = Capture(Arc::new(Mutex::new(String::new())));
        // Installed before the runtime/HTTP stack; every thread in this child
        // observes the same subscriber, including first callsite registration.
        tracing::subscriber::set_global_default(captured.clone()).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(assert_request_trace_is_redacted(captured));
    }

    async fn assert_request_trace_is_redacted(captured: Capture) {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/b/private-object")
            .expect(3)
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .create_async()
            .await;
        let client = Client::builder()
            .endpoint(server.url())
            .credentials("private-ak", "private-sk")
            .build()
            .unwrap();
        let mut req = ClientRequest::new("b", "private-object", Method::GET);
        req.query.add("storageAccessToken", "private-token");
        req.query.add("Signature", "private-signature");
        client.do_request(req).await.unwrap();
        assert!(captured.0.lock().unwrap().contains("FDS request"));
        let exchange = server.mock("GET", "/")
            .match_query(mockito::Matcher::Any)
            .expect(2)
            .with_body(serde_json::json!({"token":"private-storage-token", "expireTime":(chrono::Utc::now()+chrono::Duration::days(1)).timestamp_millis()}).to_string())
            .create_async().await;
        let request = crate::oauth::FdsServiceClient::new(server.url())
            .unwrap()
            .exchange_storage_token(
                "private-app",
                "private-oauth-app",
                "private-oauth-token",
                crate::oauth::OAuthProvider::XiaoMi,
            )
            .xiaomi_mac("private-openid", "private-mac-key", "HmacSHA1");
        let token = request.clone().send().await.unwrap();
        let oauth = Client::builder()
            .endpoint(server.url())
            .storage_token(token)
            .build()
            .unwrap();
        oauth
            .do_request(ClientRequest::new("b", "private-object", Method::GET))
            .await
            .unwrap();
        // A refreshing provider: its exchange runs inside the request.
        let provider = crate::credentials::StorageTokenProvider::new(request);
        assert!(!format!("{provider:?}").contains("private-"));
        let refreshing = Client::builder()
            .endpoint(server.url())
            .credentials_provider(provider)
            .build()
            .unwrap();
        refreshing
            .do_request(ClientRequest::new("b", "private-object", Method::GET))
            .await
            .unwrap();
        exchange.assert_async().await;
        let output = captured.0.lock().unwrap().clone();
        // Access-key GET, OAuth token exchange, storage-token GET, and the
        // provider's exchange plus its storage-token GET.
        assert_eq!(output.matches("FDS request").count(), 5);
        for sensitive in [
            "private-",
            "http://",
            "storageAccessToken",
            "Signature",
            "Galaxy-V2",
        ] {
            assert!(
                !output.contains(sensitive),
                "unexpected diagnostic: {output}"
            );
        }
        m.assert_async().await;
    }
}

#[cfg(test)]
mod retry_safety_tests {
    use super::*;
    use std::time::Duration;
    #[tokio::test]
    async fn semantic_default_deny_and_body_replayability_are_independent() {
        let mut server = mockito::Server::new_async().await;
        let client = Client::builder()
            .endpoint(server.url())
            .credentials("ak", "sk")
            .retry_policy(
                crate::RetryPolicy::new(2, Duration::from_millis(1), Duration::from_millis(2))
                    .unwrap(),
            )
            .build()
            .unwrap();
        for (key, method, read, stream, attempts) in [
            ("unclassified_get", Method::GET, false, false, 1),
            ("unsafe_post", Method::POST, false, false, 1),
            ("read_stream", Method::GET, true, true, 1),
            ("read_bytes", Method::GET, true, false, 3),
        ] {
            let m = server
                .mock(method.as_str(), format!("/b/{key}").as_str())
                .with_status(503)
                .expect(attempts)
                .create_async()
                .await;
            let mut req = ClientRequest::new("b", key, method);
            if read {
                req = req.read_only();
            }
            req.body = Some(if stream {
                UploadBody::from_stream(
                    futures_util::stream::iter([Ok(bytes::Bytes::from_static(b"x"))]),
                    1,
                )
            } else {
                vec![b'x'].into()
            });
            assert!(client.do_request(req).await.is_err());
            m.assert_async().await;
        }
    }
}
