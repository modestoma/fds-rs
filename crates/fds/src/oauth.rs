//! Explicit endpoint discovery and OAuth exchange. See docs/protocol.md
//! for protocol evidence, provisioning prerequisites and refresh semantics.
use crate::{
    client::ClientRequest, Client, ClientBuilder, ClientConfiguration, Error, ErrorKind, Result,
};
use chrono::{DateTime, Utc};
use http::Method;
use reqwest::header::HeaderValue;
use std::fmt;

pub(crate) enum Credentials {
    AccessKey(String, String),
    OAuth(StorageAccessToken),
    Anonymous,
}
impl Credentials {
    pub(crate) fn validate(&self, conf: &ClientConfiguration) -> Result<()> {
        match self {
            Self::AccessKey(id, secret)
                if id.is_empty()
                    || secret.is_empty()
                    || !id.bytes().all(|b| b.is_ascii_graphic() && b != b':') =>
            {
                Err(Error::config_invalid(
                    "credentials require a nonempty header-safe access ID and nonempty secret",
                ))
            }
            Self::OAuth(token) => {
                secure_origin(conf.endpoint())?;
                token.validate()
            }
            _ => Ok(()),
        }
    }
}

fn nonempty(value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::invalid_input(
            "OAuth fields and discovery selectors must not be empty",
        ))
    } else {
        Ok(())
    }
}
fn secure_origin(url: &url::Url) -> Result<()> {
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

/// Secrets are private and redacted from Debug; no serialization implementation.
/// XiaoMi requires `xiaomi_mac` before sending. Other providers reject MAC fields.
#[derive(Clone)]
pub struct StorageTokenRequest {
    app_id: String,
    oauth_app_id: String,
    access_token: String,
    provider: OAuthProvider,
    xiaomi: Option<(String, String, String)>,
}
impl fmt::Debug for StorageTokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageTokenRequest")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}
impl StorageTokenRequest {
    pub fn new(
        app_id: impl Into<String>,
        oauth_app_id: impl Into<String>,
        access_token: impl Into<String>,
        provider: OAuthProvider,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            oauth_app_id: oauth_app_id.into(),
            access_token: access_token.into(),
            provider,
            xiaomi: None,
        }
    }
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
    /// GET /?endpoint&region=...&service=...; returns the documented JSON host
    /// string. Service/region values are service-defined, nonempty selectors.
    /// This does not validate service availability or connect to the returned host.
    pub async fn get_endpoint(&self, region: &str, service: &str) -> Result<String> {
        nonempty(region)?;
        nonempty(service)?;
        let mut req = ClientRequest::new("", "", Method::GET).read_only();
        req.query.add("endpoint", "");
        req.query.add("region", region);
        req.query.add("service", service);
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
    /// Single-attempt exchange, even when read retries are configured. Requires
    /// application provisioning; errors discard server-controlled diagnostics.
    pub async fn exchange_storage_token(
        &self,
        input: &StorageTokenRequest,
    ) -> Result<StorageAccessToken> {
        input.validate()?;
        secure_origin(self.client.configuration().endpoint())?;
        let mut req = ClientRequest::new("", "", Method::GET);
        req.headers
            .insert("authorization", HeaderValue::from_static("OAuth"));
        for (key, value) in [
            ("storageAccessToken", ""),
            ("appId", input.app_id.as_str()),
            ("oauthAppId", input.oauth_app_id.as_str()),
            ("oauthAccessToken", input.access_token.as_str()),
            ("oauthProvider", input.provider.wire()),
        ] {
            req.query.add(key, value);
        }
        if let Some((id, key, algorithm)) = &input.xiaomi {
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
            app_id: input.app_id.clone(),
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

/// App-scoped bucket listing, retaining the app API's organization field.
#[derive(Debug, serde::Deserialize)]
pub struct AppBuckets {
    pub owner: crate::acl::Owner,
    pub buckets: Vec<AppBucket>,
}
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppBucket {
    pub name: String,
    pub org_id: String,
    /// Unix epoch milliseconds, as documented by Bucket.yaml.
    pub creation_time: i64,
    pub used_space: i64,
    pub num_objects: i64,
}
impl Client {
    /// List buckets for the app bound to this client's exchanged OAuth token.
    /// appId is never accepted as standalone authentication or an override.
    pub async fn list_app_buckets(&self) -> Result<AppBuckets> {
        self.require_oauth()?;
        self.do_json(ClientRequest::new("", "", Method::GET).read_only())
            .await
    }
}
