use anyhow::{anyhow, bail, Result};
use clap::Args;
use fds::oauth::FdsServiceClient;
use fds::{Client, ClientBuilder, RetryPolicy};
use std::{path::PathBuf, time::Duration};

#[derive(Args)]
pub struct Connection {
    /// Print bounded, untrusted SDK response diagnostics to stderr on failure
    #[arg(long, global = true)]
    pub diagnostics: bool,
    /// FDS origin URL (required except for --help)
    #[arg(long, env = "FDS_ENDPOINT", global = true)]
    pub endpoint: Option<String>,
    #[arg(long, env = "FDS_CDN_ENDPOINT", global = true)]
    cdn_endpoint: Option<String>,
    #[arg(
        long,
        env = "FDS_ACCESS_KEY_ID",
        global = true,
        conflicts_with = "oauth"
    )]
    access_key_id: Option<String>,
    #[arg(
        long,
        env = "FDS_ACCESS_KEY_SECRET",
        hide_env_values = true,
        global = true,
        conflicts_with = "oauth"
    )]
    access_key_secret: Option<String>,
    /// JSON OAuth exchange inputs; explicitly exchange once before the operation
    #[arg(long, env = "FDS_OAUTH_FILE", global = true)]
    oauth: Option<PathBuf>,
    /// Explicit origin for OAuth exchange (defaults to --endpoint)
    #[arg(long, env = "FDS_AUTH_ENDPOINT", global = true, requires = "oauth")]
    auth_endpoint: Option<String>,
    /// Calculate MD5 for bytes; file/stream bodies need a supplied content-md5
    #[arg(long, global = true)]
    md5: bool,
    #[arg(long, global = true)]
    connect_timeout_ms: Option<u64>,
    // Unset keeps the SDK default of no total deadline; set opts in to one.
    #[arg(long, global = true)]
    request_timeout_ms: Option<u64>,
    #[arg(long, global = true)]
    read_timeout_ms: Option<u64>,
    #[arg(long, global = true)]
    pool_idle_timeout_ms: Option<u64>,
    #[arg(long, global = true)]
    tcp_keepalive_secs: Option<u64>,
    /// Read retries only; excludes initial attempt. Mutations are never retried
    #[arg(long, global = true, default_value_t = 0)]
    retries: u32,
    #[arg(long, global = true, default_value_t = 100)]
    retry_initial_ms: u64,
    #[arg(long, global = true, default_value_t = 5000)]
    retry_max_ms: u64,
    /// DNS node limit, available only with the cslb Cargo feature
    #[cfg(feature = "cslb")]
    #[arg(long, global = true)]
    cslb_max_nodes: Option<usize>,
}

impl Connection {
    pub fn builder(&self) -> Result<ClientBuilder> {
        let mut b = Client::builder()
            .endpoint(
                self.endpoint
                    .as_ref()
                    .ok_or_else(|| anyhow!("provide --endpoint or FDS_ENDPOINT"))?,
            )
            .enable_md5_calculate(self.md5)
            .retry_policy(RetryPolicy::new(
                self.retries,
                Duration::from_millis(self.retry_initial_ms),
                Duration::from_millis(self.retry_max_ms),
            )?);
        if let Some(v) = &self.cdn_endpoint {
            b = b.cdn_endpoint(v);
        }
        if let Some(v) = self.connect_timeout_ms {
            b = b.connect_timeout(Duration::from_millis(v));
        }
        if let Some(v) = self.request_timeout_ms {
            b = b.request_timeout(Duration::from_millis(v));
        }
        if let Some(v) = self.read_timeout_ms {
            b = b.read_timeout(Duration::from_millis(v));
        }
        if let Some(v) = self.pool_idle_timeout_ms {
            b = b.pool_idle_timeout(Duration::from_millis(v));
        }
        if let Some(v) = self.tcp_keepalive_secs {
            b = b.tcp_keepalive(Duration::from_secs(v));
        }
        #[cfg(feature = "cslb")]
        if let Some(v) = self.cslb_max_nodes {
            b = b.cslb_max_nodes(v);
        }
        Ok(b)
    }

    pub async fn client(&self) -> Result<Client> {
        let builder = self.builder()?;
        if let Some(path) = &self.oauth {
            if self.access_key_id.is_some() || self.access_key_secret.is_some() {
                bail!("choose access keys or OAuth, not both");
            }
            let request = super::service::read_exchange(path)?;
            let mut auth = self.builder()?;
            if let Some(origin) = &self.auth_endpoint {
                auth = auth.endpoint(origin);
            }
            let token = request
                .exchange(&FdsServiceClient::from_builder(auth)?)
                .send()
                .await?;
            return Ok(builder.storage_token(token).build()?);
        }
        Ok(builder
            .credentials(
                self.access_key_id
                    .as_ref()
                    .ok_or_else(|| anyhow!("provide FDS_ACCESS_KEY_ID or --oauth"))?,
                self.access_key_secret
                    .as_ref()
                    .ok_or_else(|| anyhow!("provide FDS_ACCESS_KEY_SECRET or --oauth"))?,
            )
            .build()?)
    }
}
