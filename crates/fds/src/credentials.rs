//! Credentials and the providers a [`Client`](crate::Client) asks for them.
//!
//! A client holds one [`CredentialsProvider`] and asks it for
//! [`Credentials`] before **every attempt** of every request, retries
//! included, so the provider can rotate access keys or refresh a storage
//! token without rebuilding the client or its clones:
//!
//! - [`ClientBuilder::credentials`](crate::ClientBuilder::credentials) and
//!   [`ClientBuilder::storage_token`](crate::ClientBuilder::storage_token) are
//!   shortcuts for a [`StaticCredentials`] provider; they never change.
//! - [`StorageTokenProvider`] repeats an OAuth storage-token exchange shortly
//!   before the current token expires.
//! - Any other source implements [`CredentialsProvider`] and is installed with
//!   [`ClientBuilder::credentials_provider`](crate::ClientBuilder::credentials_provider).
//!
//! Discovering credentials from environment variables or configuration files
//! is not provided.
//!
//! ```no_run
//! # async fn example() -> fds::Result<()> {
//! use fds::credentials::StorageTokenProvider;
//! use fds::oauth::{FdsServiceClient, OAuthProvider};
//! use std::time::Duration;
//!
//! let service = FdsServiceClient::new("https://cnbj1.fds.api.xiaomi.com")?;
//! let provider = StorageTokenProvider::new(service.exchange_storage_token(
//!     "app-id",
//!     "oauth-app-id",
//!     "provider-token",
//!     OAuthProvider::QQ,
//! ))
//! .refresh_margin(Duration::from_secs(600));
//! let client = fds::Client::builder()
//!     .endpoint("https://cnbj1.fds.api.xiaomi.com")
//!     .credentials_provider(provider)
//!     .build()?;
//! // The first request exchanges a token; later requests reuse it and
//! // exchange again shortly before it expires.
//! let _app_buckets = client.list_app_buckets().send().await?;
//! # Ok(()) }
//! ```
use crate::oauth::{ExchangeStorageToken, StorageAccessToken};
use crate::{ClientConfiguration, Error, ErrorKind, Result};
use chrono::{DateTime, Utc};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::{fmt, time::Duration};

/// The future returned by [`CredentialsProvider::provide_credentials`].
pub type CredentialsFuture<'a> = Pin<Box<dyn Future<Output = Result<Credentials>> + Send + 'a>>;

/// A source of [`Credentials`], asked before every request attempt.
///
/// The trait is object safe: implementations return a boxed future, usually
/// written as `Box::pin(async move { .. })`. A provider is shared by every
/// clone of the client and may be called concurrently, so it should cache
/// and share expensive work itself (as [`StorageTokenProvider`] does).
///
/// An error is returned to the caller as is; the request is not sent. Return
/// [`ErrorKind::Unauthenticated`] or [`ErrorKind::CredentialsExpired`] when
/// no usable credentials can be obtained.
///
/// ```
/// use fds::credentials::{Credentials, CredentialsFuture, CredentialsProvider};
/// use std::sync::RwLock;
///
/// /// Access keys that another task rotates.
/// struct Rotating(RwLock<(String, String)>);
///
/// impl CredentialsProvider for Rotating {
///     fn provide_credentials(&self) -> CredentialsFuture<'_> {
///         let (id, secret) = self.0.read().unwrap().clone();
///         Box::pin(async move { Ok(Credentials::access_key(id, secret)) })
///     }
/// }
///
/// # fn main() -> fds::Result<()> {
/// let client = fds::Client::builder()
///     .endpoint("https://cnbj1.fds.api.xiaomi.com")
///     .credentials_provider(Rotating(RwLock::new(("id".into(), "secret".into()))))
///     .build()?;
/// # let _ = client; Ok(()) }
/// ```
pub trait CredentialsProvider: Send + Sync {
    /// The credentials for the next request attempt.
    fn provide_credentials(&self) -> CredentialsFuture<'_>;
}

impl<T: CredentialsProvider + ?Sized> CredentialsProvider for Arc<T> {
    fn provide_credentials(&self) -> CredentialsFuture<'_> {
        (**self).provide_credentials()
    }
}

/// Credentials for one request: a Galaxy-V2 access key, or an exchanged
/// OAuth storage token.
///
/// `Debug` prints only the kind (and a token's expiration), never the access
/// key ID, secret, app ID or token.
#[derive(Clone)]
pub struct Credentials(Repr);

#[derive(Clone)]
enum Repr {
    AccessKey { id: String, secret: String },
    StorageToken(StorageAccessToken),
}

impl Credentials {
    /// An access key ID and secret. Requests are signed with Galaxy-V2.
    pub fn access_key(id: impl Into<String>, secret: impl Into<String>) -> Self {
        Self(Repr::AccessKey {
            id: id.into(),
            secret: secret.into(),
        })
    }

    /// An exchanged storage token. Requests carry the token and its app ID in
    /// the query string; they cannot be presigned.
    pub fn storage_token(token: StorageAccessToken) -> Self {
        Self(Repr::StorageToken(token))
    }

    /// The access key ID, for access-key credentials.
    pub fn access_key_id(&self) -> Option<&str> {
        match &self.0 {
            Repr::AccessKey { id, .. } => Some(id),
            Repr::StorageToken(_) => None,
        }
    }

    /// The storage token, for storage-token credentials.
    pub fn storage_access_token(&self) -> Option<&StorageAccessToken> {
        match &self.0 {
            Repr::StorageToken(token) => Some(token),
            Repr::AccessKey { .. } => None,
        }
    }

    pub(crate) fn access_key_pair(&self) -> Option<(&str, &str)> {
        match &self.0 {
            Repr::AccessKey { id, secret } => Some((id, secret)),
            Repr::StorageToken(_) => None,
        }
    }

    /// Checked at build time for static credentials and before every attempt
    /// for credentials from a provider.
    pub(crate) fn validate(&self, conf: &ClientConfiguration) -> Result<()> {
        match &self.0 {
            Repr::AccessKey { id, secret }
                if id.is_empty()
                    || secret.is_empty()
                    || !id.bytes().all(|b| b.is_ascii_graphic() && b != b':') =>
            {
                Err(Error::config_invalid(
                    "credentials require a nonempty header-safe access ID and nonempty secret",
                ))
            }
            Repr::AccessKey { .. } => Ok(()),
            Repr::StorageToken(token) => {
                crate::oauth::secure_origin(conf.endpoint())?;
                token.validate()
            }
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Credentials");
        match &self.0 {
            Repr::AccessKey { .. } => s.field("kind", &"AccessKey"),
            Repr::StorageToken(token) => s
                .field("kind", &"StorageToken")
                .field("expires_at", &token.expires_at()),
        };
        s.finish_non_exhaustive()
    }
}

/// A provider that always returns the same [`Credentials`].
///
/// [`ClientBuilder::credentials`](crate::ClientBuilder::credentials) and
/// [`ClientBuilder::storage_token`](crate::ClientBuilder::storage_token) are
/// shortcuts for it. Static credentials are validated when the client is
/// built, and a static storage token is never refreshed: once it expires,
/// requests fail with [`ErrorKind::CredentialsExpired`] before sending.
#[derive(Clone)]
pub struct StaticCredentials {
    pub(crate) credentials: Credentials,
}

impl StaticCredentials {
    pub fn new(credentials: Credentials) -> Self {
        Self { credentials }
    }
}

impl From<Credentials> for StaticCredentials {
    fn from(credentials: Credentials) -> Self {
        Self::new(credentials)
    }
}

impl fmt::Debug for StaticCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StaticCredentials")
            .field(&self.credentials)
            .finish()
    }
}

impl CredentialsProvider for StaticCredentials {
    fn provide_credentials(&self) -> CredentialsFuture<'_> {
        let credentials = self.credentials.clone();
        Box::pin(async move { Ok(credentials) })
    }
}

/// The default [`StorageTokenProvider::refresh_margin`]: five minutes.
pub const DEFAULT_REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// A storage token that is exchanged again shortly before it expires.
///
/// It wraps an [`ExchangeStorageToken`] request (built with
/// [`FdsServiceClient::exchange_storage_token`](crate::oauth::FdsServiceClient::exchange_storage_token))
/// and repeats it whenever a token is needed and the cached one is missing or
/// due for refresh:
///
/// - **When:** a token is due once the time left before it expires is at most
///   the [refresh margin](Self::refresh_margin) (default
///   [`DEFAULT_REFRESH_MARGIN`]). For a token this provider exchanged itself,
///   the margin is capped at half of the token's lifetime, so that a
///   short-lived token is not exchanged again on every request.
/// - **Single flight:** concurrent requests that find the token due share one
///   exchange; they all wait for it and use its result. Clones of the
///   provider share the cached token and the exchange.
/// - **Failure:** when the exchange fails, every request waiting for it fails
///   before sending, with [`ErrorKind::CredentialsExpired`] if the cached
///   token has expired and [`ErrorKind::Unauthenticated`] otherwise (no token
///   yet, or the token is still valid but due for refresh). The exchange error
///   is the [`source`](std::error::Error::source) of the error returned to the
///   request that ran the exchange. A failure is not cached: the next request
///   tries again. An expired token is never sent.
///
/// The exchange itself is a single attempt with its own timeout and is not
/// retried. Cancelling the request that runs it cancels the exchange; the
/// next waiting request starts a new one. Refreshing the provider's OAuth
/// access token (the input of the exchange) is the application's
/// responsibility: implement [`CredentialsProvider`] for that.
///
/// `Debug` shows the margin and the cached token's expiration only.
#[derive(Clone)]
pub struct StorageTokenProvider {
    shared: Arc<Shared>,
    refresh_margin: Duration,
}

struct Shared {
    exchange: ExchangeStorageToken,
    state: Mutex<State>,
    /// Held across the exchange: the single flight.
    refresh: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    token: Option<Cached>,
    /// Counts finished exchanges, so that a request waiting for the single
    /// flight can tell whether one completed while it waited.
    generation: u64,
    /// The kind of the most recent exchange failure, if it failed.
    failure: Option<ErrorKind>,
}

struct Cached {
    token: StorageAccessToken,
    /// When this provider exchanged it; `None` for a seeded token.
    obtained_at: Option<DateTime<Utc>>,
}

impl Cached {
    fn due(&self, margin: Duration, now: DateTime<Utc>) -> bool {
        let expires_at = self.token.expires_at();
        let mut margin = chrono::Duration::from_std(margin).unwrap_or(chrono::Duration::MAX);
        if let Some(obtained_at) = self.obtained_at {
            margin = margin.min((expires_at - obtained_at) / 2);
        }
        expires_at
            .checked_sub_signed(margin)
            .is_none_or(|refresh_at| now >= refresh_at)
    }
}

const REFRESH_FAILED: &str = "storage token exchange failed; the request was not sent";

impl StorageTokenProvider {
    /// A provider that runs `exchange` for its first token, on the first
    /// request.
    pub fn new(exchange: ExchangeStorageToken) -> Self {
        Self {
            shared: Arc::new(Shared {
                exchange,
                state: Mutex::new(State::default()),
                refresh: tokio::sync::Mutex::new(()),
            }),
            refresh_margin: DEFAULT_REFRESH_MARGIN,
        }
    }

    /// Start with an already exchanged `token` (for example one obtained from
    /// the same `exchange` request) instead of exchanging on the first
    /// request. Its lifetime is unknown, so the full margin applies to it.
    pub fn with_token(self, token: StorageAccessToken) -> Self {
        self.lock_state().token = Some(Cached {
            token,
            obtained_at: None,
        });
        self
    }

    /// Exchange again once the token expires within `margin`. Defaults to
    /// [`DEFAULT_REFRESH_MARGIN`]. Applies to this value and the clones made
    /// from it afterwards; the cached token is shared regardless.
    pub fn refresh_margin(mut self, margin: Duration) -> Self {
        self.refresh_margin = margin;
        self
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    async fn token(&self) -> Result<StorageAccessToken> {
        let observed = {
            let state = self.lock_state();
            match &state.token {
                Some(cached) if !cached.due(self.refresh_margin, Utc::now()) => {
                    return Ok(cached.token.clone());
                }
                _ => state.generation,
            }
        };
        let _flight = self.shared.refresh.lock().await;
        {
            let state = self.lock_state();
            if state.generation != observed {
                // An exchange finished while this request waited: share it.
                if let Some(kind) = state.failure {
                    return Err(Error::new(kind, REFRESH_FAILED));
                }
                if let Some(cached) = state.token.as_ref().filter(|c| !c.token.is_expired()) {
                    return Ok(cached.token.clone());
                }
            }
        }
        let result = self.shared.exchange.clone().send().await;
        let mut state = self.lock_state();
        state.generation = state.generation.wrapping_add(1);
        match result {
            Ok(token) => {
                state.failure = None;
                state.token = Some(Cached {
                    token: token.clone(),
                    obtained_at: Some(Utc::now()),
                });
                Ok(token)
            }
            Err(error) => {
                let kind = match &state.token {
                    Some(cached) if cached.token.is_expired() => ErrorKind::CredentialsExpired,
                    _ => ErrorKind::Unauthenticated,
                };
                state.failure = Some(kind);
                Err(Error::new(kind, REFRESH_FAILED).with_source(error))
            }
        }
    }
}

impl fmt::Debug for StorageTokenProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let expires_at = self
            .lock_state()
            .token
            .as_ref()
            .map(|cached| cached.token.expires_at());
        f.debug_struct("StorageTokenProvider")
            .field("refresh_margin", &self.refresh_margin)
            .field("expires_at", &expires_at)
            .finish_non_exhaustive()
    }
}

impl CredentialsProvider for StorageTokenProvider {
    fn provide_credentials(&self) -> CredentialsFuture<'_> {
        Box::pin(async move { self.token().await.map(Credentials::storage_token) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_secrets() {
        let key = Credentials::access_key("private-id", "private-secret");
        let provider = StaticCredentials::new(key.clone());
        for output in [format!("{key:?}"), format!("{provider:?}")] {
            assert!(output.contains("AccessKey"), "{output}");
            assert!(!output.contains("private"), "{output}");
        }
    }
}
