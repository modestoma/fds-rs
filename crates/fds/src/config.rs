//! Validated client configuration and construction.
use crate::{
    error::{Error, ErrorKind, Result},
    Client,
};
use std::{fmt, time::Duration};
use url::Url;

/// Immutable effective settings. Obtain through [`Client::configuration`].
#[derive(Debug, Clone)]
pub struct ClientConfiguration {
    pub(crate) endpoint: Url,
    pub(crate) retry_policy: crate::RetryPolicy,
    pub(crate) cdn_endpoint: Option<Url>,
    pub(crate) enable_md5_calculate: bool,
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) read_timeout: Duration,
    pub(crate) pool_idle_timeout: Duration,
    pub(crate) tcp_keepalive: Duration,
    pub(crate) pool_max_idle_per_host: usize,
    pub(crate) user_agent: String,
    #[cfg(feature = "cslb")]
    pub(crate) cslb_max_nodes: usize,
}

/// `fds-rs/<crate version>`, the User-Agent product token of every request.
const USER_AGENT: &str = concat!("fds-rs/", env!("CARGO_PKG_VERSION"));

impl ClientConfiguration {
    fn for_endpoint(endpoint: Url) -> Self {
        Self {
            endpoint,
            retry_policy: crate::RetryPolicy::default(),
            cdn_endpoint: None,
            enable_md5_calculate: false,
            connect_timeout: Duration::from_secs(50),
            request_timeout: None,
            read_timeout: Duration::from_secs(50),
            pool_idle_timeout: Duration::from_secs(90),
            tcp_keepalive: Duration::from_secs(30),
            pool_max_idle_per_host: 100,
            user_agent: USER_AGENT.to_owned(),
            #[cfg(feature = "cslb")]
            cslb_max_nodes: 20,
        }
    }
    pub fn retry_policy(&self) -> &crate::RetryPolicy {
        &self.retry_policy
    }
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }
    pub fn cdn_endpoint(&self) -> Option<&Url> {
        self.cdn_endpoint.as_ref()
    }
    pub fn enable_md5_calculate(&self) -> bool {
        self.enable_md5_calculate
    }
    /// Connection establishment timeout, including TLS. Defaults to 50 seconds.
    pub fn connect_timeout(&self) -> Duration {
        self.connect_timeout
    }
    /// Client-wide total deadline for each attempt, or `None` (the default)
    /// when no total deadline applies. See [`ClientBuilder::request_timeout`].
    pub fn request_timeout(&self) -> Option<Duration> {
        self.request_timeout
    }
    /// I/O idle timeout, reset by upload progress, by the response headers
    /// and by each response-body read. Defaults to 50 seconds. See
    /// [`ClientBuilder::read_timeout`].
    pub fn read_timeout(&self) -> Duration {
        self.read_timeout
    }
    pub fn pool_idle_timeout(&self) -> Duration {
        self.pool_idle_timeout
    }
    pub fn tcp_keepalive(&self) -> Duration {
        self.tcp_keepalive
    }
    /// Maximum idle connections kept per host. Defaults to 100.
    pub fn pool_max_idle_per_host(&self) -> usize {
        self.pool_max_idle_per_host
    }
    /// The User-Agent sent with every request: `fds-rs/<version>`, followed by
    /// the [suffix](ClientBuilder::user_agent_suffix) when one is set.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
    #[cfg(feature = "cslb")]
    pub fn cslb_max_nodes(&self) -> usize {
        self.cslb_max_nodes
    }
}

/// Builds a client without networking. Credentials are omitted from Debug.
#[derive(Default)]
pub struct ClientBuilder {
    endpoint: Option<String>,
    retry_policy: crate::RetryPolicy,
    cdn_endpoint: Option<String>,
    credentials: Option<crate::oauth::Credentials>,
    md5: bool,
    connect_timeout: Option<Duration>,
    request_timeout: Option<Duration>,
    read_timeout: Option<Duration>,
    pool_idle_timeout: Option<Duration>,
    tcp_keepalive: Option<Duration>,
    pool_max_idle_per_host: Option<usize>,
    user_agent_suffix: Option<String>,
    #[cfg(feature = "cslb")]
    cslb_max_nodes: Option<usize>,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder").finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// Opt-in retries for explicitly classified reads. Writes remain single-attempt.
    pub fn retry_policy(mut self, value: crate::RetryPolicy) -> Self {
        self.retry_policy = value;
        self
    }
    /// Full http(s) origin URL; no credentials, query, fragment or path prefix.
    pub fn endpoint(mut self, value: impl Into<String>) -> Self {
        self.endpoint = Some(value.into());
        self
    }
    /// Explicit CDN origin, used only by requests for a presigned CDN URL.
    pub fn cdn_endpoint(mut self, value: impl Into<String>) -> Self {
        self.cdn_endpoint = Some(value.into());
        self
    }
    pub fn credentials(mut self, id: impl Into<String>, secret: impl Into<String>) -> Self {
        self.credentials = Some(crate::oauth::Credentials::AccessKey(
            id.into(),
            secret.into(),
        ));
        self
    }
    /// Use an exchanged token. The last credential setter wins. Refresh by
    /// exchanging again and building a new client; existing clones stay unchanged.
    pub fn storage_token(mut self, token: crate::oauth::StorageAccessToken) -> Self {
        self.credentials = Some(crate::oauth::Credentials::OAuth(token));
        self
    }
    pub(crate) fn anonymous(mut self) -> Self {
        self.credentials = Some(crate::oauth::Credentials::Anonymous);
        self
    }
    /// Calculate hexadecimal Content-MD5 for in-memory request bodies.
    pub fn enable_md5_calculate(mut self, value: bool) -> Self {
        self.md5 = value;
        self
    }
    /// Connection establishment timeout, including TLS. Defaults to 50
    /// seconds. Must be nonzero and representable.
    pub fn connect_timeout(mut self, value: Duration) -> Self {
        self.connect_timeout = Some(value);
        self
    }
    /// Opt in to a client-wide total deadline for each request attempt.
    ///
    /// By default there is **no** total deadline: requests are bounded only by
    /// [`connect_timeout`](Self::connect_timeout) and
    /// [`read_timeout`](Self::read_timeout), so a streaming download or an
    /// upload keeps running for as long as it makes progress.
    ///
    /// Once set, the deadline runs from connecting until the response body has
    /// been fully consumed. It therefore also covers streaming reads through
    /// [`GetObjectOutput::chunk`](crate::object::GetObjectOutput::chunk),
    /// [`GetObjectOutput::copy_to`](crate::object::GetObjectOutput::copy_to) and
    /// [`GetObjectOutput::bytes`](crate::object::GetObjectOutput::bytes): a transfer that
    /// is still progressing fails with a timeout when the deadline passes. It
    /// applies to each attempt separately, not to all retries together.
    /// Requests that support a per-request `timeout` (for example
    /// [`GetObjectRequest::timeout`](crate::object::GetObjectRequest::timeout)) override
    /// this value. Must be nonzero and representable.
    pub fn request_timeout(mut self, value: Duration) -> Self {
        self.request_timeout = Some(value);
        self
    }
    /// I/O idle timeout for every request. Defaults to 50 seconds. Must be
    /// nonzero and representable.
    ///
    /// It limits how long an attempt may go without progress, never how long
    /// a transfer takes in total, so a large upload or download that keeps
    /// moving data is not cut off:
    ///
    /// - **Sending the request body:** reset each time the transport takes the
    ///   next piece of the body (at most 64 KiB), which happens as fast as the
    ///   connection drains it. An upload the peer stops reading fails after
    ///   this long.
    /// - **Waiting for the response headers:** measured from the last upload
    ///   progress, or from the start of the attempt for requests without a
    ///   body (this includes connecting, which is also bounded by
    ///   [`connect_timeout`](Self::connect_timeout)).
    /// - **Reading the response body:** bounds each read and is reset whenever
    ///   a read receives data; time the caller spends between reads does not
    ///   count.
    ///
    /// Expiry fails with [`ErrorKind::Timeout`]. Before the response headers
    /// arrive the request may already have reached the service, so the error's
    /// [`dispatch`](crate::Error::dispatch) is
    /// [`Dispatch::Unknown`](crate::Dispatch::Unknown); while reading the body
    /// it is [`Dispatch::Sent`](crate::Dispatch::Sent). To bound the total
    /// duration as well, set [`request_timeout`](Self::request_timeout) or a
    /// per-request `timeout`.
    pub fn read_timeout(mut self, value: Duration) -> Self {
        self.read_timeout = Some(value);
        self
    }
    /// Time unused connections remain eligible for pool reuse.
    pub fn pool_idle_timeout(mut self, value: Duration) -> Self {
        self.pool_idle_timeout = Some(value);
        self
    }
    /// Idle duration before TCP keepalive probes; OS scheduling applies.
    /// Must be a whole number of seconds in 1..=32767.
    pub fn tcp_keepalive(mut self, value: Duration) -> Self {
        self.tcp_keepalive = Some(value);
        self
    }
    /// Maximum idle connections kept in the pool per host. Defaults to 100;
    /// `0` disables keeping idle connections. Not a concurrency limit.
    pub fn pool_max_idle_per_host(mut self, value: usize) -> Self {
        self.pool_max_idle_per_host = Some(value);
        self
    }
    /// Identify the application: appended to the User-Agent after a space,
    /// as in `fds-rs/<version> my-app/1.2`. Must be nonempty printable ASCII
    /// (spaces allowed). The User-Agent is not signed.
    pub fn user_agent_suffix(mut self, value: impl Into<String>) -> Self {
        self.user_agent_suffix = Some(value.into());
        self
    }
    /// Maximum DNS nodes retained per CSLB host (not an HTTP concurrency limit).
    #[cfg(feature = "cslb")]
    pub fn cslb_max_nodes(mut self, value: usize) -> Self {
        self.cslb_max_nodes = Some(value);
        self
    }

    pub fn build(self) -> Result<Client> {
        let endpoint = validate_endpoint(
            self.endpoint
                .as_deref()
                .ok_or_else(|| Error::config_invalid("endpoint is required"))?,
        )?;
        let mut conf = ClientConfiguration::for_endpoint(endpoint);
        conf.cdn_endpoint = self
            .cdn_endpoint
            .as_deref()
            .map(validate_endpoint)
            .transpose()?;
        conf.enable_md5_calculate = self.md5;
        conf.retry_policy = self.retry_policy;
        for (target, value) in [
            (&mut conf.connect_timeout, self.connect_timeout),
            (&mut conf.read_timeout, self.read_timeout),
            (&mut conf.pool_idle_timeout, self.pool_idle_timeout),
            (&mut conf.tcp_keepalive, self.tcp_keepalive),
        ] {
            if let Some(value) = value {
                *target = validate_timeout(value, ErrorKind::ConfigInvalid)?;
            }
        }
        conf.request_timeout = self
            .request_timeout
            .map(|value| validate_timeout(value, ErrorKind::ConfigInvalid))
            .transpose()?;
        // Socket options use integer seconds; reject rounding and oversized
        // values instead of letting the connector silently ignore OS errors.
        if conf.tcp_keepalive.subsec_nanos() != 0 || conf.tcp_keepalive.as_secs() > 32767 {
            return Err(Error::config_invalid(
                "tcp_keepalive must be whole seconds in 1..=32767",
            ));
        }
        if let Some(value) = self.pool_max_idle_per_host {
            conf.pool_max_idle_per_host = value;
        }
        if let Some(suffix) = &self.user_agent_suffix {
            if suffix.trim().is_empty()
                || !suffix.bytes().all(|b| b == b' ' || b.is_ascii_graphic())
            {
                return Err(Error::config_invalid(
                    "user_agent_suffix must be nonempty printable ASCII",
                ));
            }
            conf.user_agent = format!("{USER_AGENT} {suffix}");
        }
        #[cfg(feature = "cslb")]
        if let Some(value) = self.cslb_max_nodes {
            if value == 0 {
                return Err(Error::config_invalid("cslb_max_nodes must be positive"));
            }
            conf.cslb_max_nodes = value;
        }
        let credentials = self
            .credentials
            .ok_or_else(|| Error::config_invalid("credentials are required"))?;
        Client::with_credentials(credentials, conf)
    }
}

/// Reject zero and deadlines which cannot be represented as an `Instant`.
/// `kind` is `ConfigInvalid` for client settings, `InvalidInput` per request.
pub(crate) fn validate_timeout(value: Duration, kind: ErrorKind) -> Result<Duration> {
    if value.is_zero() || std::time::Instant::now().checked_add(value).is_none() {
        return Err(Error::new(
            kind,
            "timeout must be nonzero and representable",
        ));
    }
    Ok(value)
}

fn validate_endpoint(value: &str) -> Result<Url> {
    // Reject normalizations which could silently remove a caller's path prefix,
    // userinfo, or whitespace before URL parsing.
    let invalid = || {
        Error::config_invalid(
            "endpoint must be an http(s) origin without userinfo, path, query or fragment",
        )
    };
    let (_, authority_path) = value.split_once("://").ok_or_else(invalid)?;
    let authority = authority_path.split('/').next().unwrap_or("");
    let path = authority_path.strip_prefix(authority).unwrap_or("");
    if value.chars().any(|c| c.is_whitespace() || c.is_control())
        || value.contains(['\\', '?', '#'])
        || authority.contains('@')
        || !(path.is_empty() || path == "/")
    {
        return Err(invalid());
    }
    let url = Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port() == Some(0)
    {
        return Err(invalid());
    }
    Ok(url)
}
