//! Explicit endpoint discovery, OAuth exchange and the app-scoped bucket
//! listing. See docs/protocol.md for protocol evidence, provisioning
//! prerequisites and refresh semantics.
use crate::{client::ClientRequest, Client, ClientBuilder, Error, ErrorKind, Result};
use chrono::{DateTime, Utc};
use http::Method;
use reqwest::header::HeaderValue;
use std::fmt;
use std::time::Duration;

fn nonempty(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::invalid_input(
            "OAuth fields and discovery selectors must not be empty",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn secure_origin(url: &url::Url) -> Result<()> {
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() == "https" || loopback {
        Ok(())
    } else {
        Err(Error::config_invalid(
            "OAuth requires HTTPS (HTTP is allowed only for loopback IP tests)",
        ))
    }
}

/// Providers documented by the storage-token endpoint. Weixin is not included:
/// its exchange-specific required fields remain unverified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthProvider {
    XiaoMi,
    QQ,
    Sina,
    RenRen,
}
impl OAuthProvider {
    fn wire(self) -> &'static str {
        match self {
            Self::XiaoMi => "XiaoMi",
            Self::QQ => "QQ",
            Self::Sina => "Sina",
            Self::RenRen => "RenRen",
        }
    }
}

/// An FDS token bound to the app ID used in its exchange. Expiration is a UTC
/// instant decoded from Unix epoch milliseconds, not a relative TTL.
/// Cloning does not refresh it. Exchange again to obtain a replacement.
#[derive(Clone)]
pub struct StorageAccessToken {
    app_id: String,
    token: String,
    expires_at: DateTime<Utc>,
}
impl fmt::Debug for StorageAccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageAccessToken").finish_non_exhaustive()
    }
}
impl StorageAccessToken {
    pub fn app_id(&self) -> &str {
        &self.app_id
    }
    /// Sensitive bearer credential. Never log this value or authenticated URLs.
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }
    pub(crate) fn validate(&self) -> Result<()> {
        nonempty(&self.token)?;
        if self.is_expired() {
            Err(Error::new(
                ErrorKind::CredentialsExpired,
                "storage token expired; explicitly exchange a new token",
            ))
        } else {
            Ok(())
        }
    }
}

/// An unauthenticated service connection exposing only discovery and token
/// exchange. The origin is explicit; discovery never redirects a credentialed
/// client or silently adopts the returned host.
///
/// Its operations follow the crate-level "Operation builders" convention:
///
/// ```no_run
/// # async fn example() -> fds::Result<()> {
/// use fds::oauth::{FdsServiceClient, OAuthProvider};
///
/// let service = FdsServiceClient::new("https://cnbj1.fds.api.xiaomi.com")?;
/// let host = service.get_endpoint("cnbj1", "fds").send().await?;
/// let token = service
///     .exchange_storage_token("app-id", "oauth-app-id", "provider-token", OAuthProvider::XiaoMi)
///     .xiaomi_mac("open-id", "mac-key", "hmac-sha-1")
///     .send()
///     .await?;
/// let client = fds::Client::builder()
///     .endpoint(format!("https://{host}"))
///     .storage_token(token)
///     .build()?;
/// let _app_buckets = client.list_app_buckets().send().await?;
/// # Ok(()) }
/// ```
///
/// A token set with [`ClientBuilder::storage_token`] is never refreshed. To
/// exchange again automatically before it expires, give the exchange request
/// to a [`StorageTokenProvider`](crate::credentials::StorageTokenProvider)
/// instead.
#[derive(Clone, Debug)]
pub struct FdsServiceClient {
    client: Client,
}
impl FdsServiceClient {
    pub fn new(origin: impl Into<String>) -> Result<Self> {
        Self::from_builder(Client::builder().endpoint(origin))
    }
    /// Reuse timeout/pool configuration. Any credentials on the builder are
    /// discarded: this connection is exclusively for discovery/exchange.
    pub fn from_builder(builder: ClientBuilder) -> Result<Self> {
        Ok(Self {
            client: builder.anonymous().build()?,
        })
    }

    /// Discover the host serving `service` in `region`. Both are
    /// service-defined, nonempty selectors.
    pub fn get_endpoint(
        &self,
        region: impl Into<String>,
        service: impl Into<String>,
    ) -> GetEndpoint {
        GetEndpoint {
            client: self.client.clone(),
            region: region.into(),
            service: service.into(),
            timeout: None,
        }
    }

    /// Exchange a provider's OAuth access token for an FDS storage token.
    /// XiaoMi additionally requires [`ExchangeStorageToken::xiaomi_mac`];
    /// other providers reject it.
    pub fn exchange_storage_token(
        &self,
        app_id: impl Into<String>,
        oauth_app_id: impl Into<String>,
        access_token: impl Into<String>,
        provider: OAuthProvider,
    ) -> ExchangeStorageToken {
        ExchangeStorageToken {
            client: self.client.clone(),
            app_id: app_id.into(),
            oauth_app_id: oauth_app_id.into(),
            access_token: access_token.into(),
            provider,
            xiaomi: None,
            timeout: None,
        }
    }
}

/// Builder for [`FdsServiceClient::get_endpoint`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct GetEndpoint {
    client: Client,
    region: String,
    service: String,
    timeout: Option<Duration>,
}

impl GetEndpoint {
    timeout_option!();

    /// GET `/?endpoint&region=...&service=...`; returns the documented JSON
    /// host string. Empty selectors fail with
    /// [`ErrorKind::InvalidInput`] before sending. This does not validate
    /// service availability or connect to the returned host.
    pub async fn send(self) -> Result<String> {
        nonempty(&self.region)?;
        nonempty(&self.service)?;
        let mut req = ClientRequest::new("", "", Method::GET)
            .read_only()
            .with_timeout(self.timeout)?;
        req.query.add("endpoint", "");
        req.query.add("region", &self.region);
        req.query.add("service", &self.service);
        let endpoint: String = self.client.do_json(req).await?;
        // The wire contract returns a hostname, not an origin or a URL path.
        if endpoint.len() > 253
            || endpoint.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
            })
            || endpoint
                .bytes()
                .any(|b| !(b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
        {
            return Err(Error::protocol("discovery returned an invalid hostname"));
        }
        Ok(endpoint)
    }
}

/// Builder for [`FdsServiceClient::exchange_storage_token`].
///
/// Secrets are private and redacted from `Debug`; there is no serialization
/// implementation. Cloning keeps the inputs, so the same exchange can be
/// repeated; [`StorageTokenProvider`](crate::credentials::StorageTokenProvider)
/// does that to refresh a token.
#[derive(Clone)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct ExchangeStorageToken {
    client: Client,
    app_id: String,
    oauth_app_id: String,
    access_token: String,
    provider: OAuthProvider,
    xiaomi: Option<(String, String, String)>,
    timeout: Option<Duration>,
}
impl fmt::Debug for ExchangeStorageToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExchangeStorageToken")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}
impl ExchangeStorageToken {
    timeout_option!();

    /// The XiaoMi MAC inputs: open ID, MAC key and MAC algorithm. Required
    /// for [`OAuthProvider::XiaoMi`] and rejected for every other provider.
    /// Pass the algorithm returned by the provider verbatim; the SDK does not
    /// calculate a MAC or guess an algorithm allowlist.
    pub fn xiaomi_mac(
        mut self,
        open_id: impl Into<String>,
        key: impl Into<String>,
        algorithm: impl Into<String>,
    ) -> Self {
        self.xiaomi = Some((open_id.into(), key.into(), algorithm.into()));
        self
    }

    fn validate(&self) -> Result<()> {
        for value in [&self.app_id, &self.oauth_app_id, &self.access_token] {
            nonempty(value)?;
        }
        match (self.provider, &self.xiaomi) {
            (OAuthProvider::XiaoMi, Some((id, key, algorithm))) => {
                for value in [id, key, algorithm] {
                    nonempty(value)?;
                }
                Ok(())
            }
            (OAuthProvider::XiaoMi, None) => Err(Error::invalid_input(
                "XiaoMi requires open ID, MAC key and MAC algorithm",
            )),
            (_, Some(_)) => Err(Error::invalid_input(
                "MAC inputs are only supported for XiaoMi",
            )),
            (_, None) => Ok(()),
        }
    }

    /// GET `/?storageAccessToken&appId&oauthAppId&oauthAccessToken&oauthProvider`
    /// (plus the XiaoMi MAC fields) with `Authorization: OAuth`.
    ///
    /// Missing or empty inputs and MAC inputs that do not match the provider
    /// fail with [`ErrorKind::InvalidInput`], and a non-HTTPS origin (other
    /// than a loopback IP) with [`ErrorKind::ConfigInvalid`], before sending.
    /// Single attempt, even when read retries are configured. Requires
    /// application provisioning; errors discard server-controlled
    /// diagnostics.
    pub async fn send(self) -> Result<StorageAccessToken> {
        self.validate()?;
        secure_origin(self.client.configuration().endpoint())?;
        let mut req = ClientRequest::new("", "", Method::GET).with_timeout(self.timeout)?;
        req.headers
            .insert("authorization", HeaderValue::from_static("OAuth"));
        for (key, value) in [
            ("storageAccessToken", ""),
            ("appId", self.app_id.as_str()),
            ("oauthAppId", self.oauth_app_id.as_str()),
            ("oauthAccessToken", self.access_token.as_str()),
            ("oauthProvider", self.provider.wire()),
        ] {
            req.query.add(key, value);
        }
        if let Some((id, key, algorithm)) = &self.xiaomi {
            req.query.add("oauthOpenId", id);
            req.query.add("oauthMacKey", key);
            req.query.add("oauthMacAlgorithm", algorithm);
        }
        #[derive(serde::Deserialize)]
        struct WireToken {
            token: String,
            #[serde(rename = "expireTime")]
            expire_time: i64,
        }
        let wire: WireToken = self.client.do_json(req).await?;
        let token = StorageAccessToken {
            app_id: self.app_id,
            token: wire.token,
            expires_at: DateTime::from_timestamp_millis(wire.expire_time)
                .ok_or_else(|| Error::protocol("invalid storage token expiration"))?,
        };
        token.validate().map_err(|e| {
            Error::protocol("exchange returned an unusable storage token").with_source(e)
        })?;
        Ok(token)
    }
}

/// Result of [`Client::list_app_buckets`]: the app-scoped bucket listing,
/// retaining the app API's organization field. Every key is required.
#[derive(Debug, serde::Deserialize)]
pub struct AppBucketList {
    pub owner: crate::acl::Owner,
    pub buckets: Vec<AppBucket>,
}

/// One bucket of an [`AppBucketList`]. Every key is required; a negative
/// size or count, or a negative or unrepresentable timestamp, is a protocol
/// violation.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppBucket {
    #[serde(rename = "name")]
    pub bucket_name: String,
    pub org_id: String,
    /// Creation time, decoded from the Unix epoch milliseconds documented by
    /// Bucket.yaml.
    #[serde(deserialize_with = "crate::de::millis")]
    pub creation_time: DateTime<Utc>,
    /// Bytes used.
    pub used_space: u64,
    /// Number of objects (`numObjects`).
    #[serde(rename = "numObjects")]
    pub object_count: u64,
}
/// Builder for [`Client::list_app_buckets`].
#[derive(Debug)]
#[must_use = "an operation builder does nothing until `send` is awaited"]
pub struct ListAppBuckets {
    client: Client,
    timeout: Option<Duration>,
}

impl ListAppBuckets {
    timeout_option!();

    /// GET `/` authenticated by the client's storage token and its app ID.
    /// The credentials are checked for every attempt: when the client's
    /// provider returns access-key credentials instead of a storage token,
    /// this fails with [`ErrorKind::ConfigInvalid`] before sending.
    pub async fn send(self) -> Result<AppBucketList> {
        let cr = ClientRequest::new("", "", Method::GET)
            .read_only()
            .requires_storage_token()
            .with_timeout(self.timeout)?;
        self.client.do_json(cr).await
    }
}

impl Client {
    /// List buckets for the app bound to this client's exchanged OAuth token.
    /// appId is never accepted as standalone authentication or an override.
    pub fn list_app_buckets(&self) -> ListAppBuckets {
        ListAppBuckets {
            client: self.clone(),
            timeout: None,
        }
    }
}
