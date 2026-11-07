use super::config::Connection;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use fds::{FdsServiceClient, OAuthProvider, StorageTokenRequest};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ServiceCmd {
    /// Discover a host; never automatically redirects credentials to that host
    Endpoint { region: String, service: String },
    /// Exchange and verify a token; prints app ID and expiration, never the secret
    Exchange {
        #[arg(long)]
        file: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeInput {
    app_id: String,
    oauth_app_id: String,
    access_token: String,
    provider: String,
    xiaomi_mac: Option<XiaomiMac>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XiaomiMac {
    open_id: String,
    key: String,
    algorithm: String,
}

pub fn read_exchange(path: &Path) -> Result<StorageTokenRequest> {
    // Do not include parser errors: unknown fields/variants may contain secrets.
    let bytes = std::fs::read(path).context("read OAuth input file")?;
    let input: ExchangeInput = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid OAuth JSON; see docs/cli.md for its schema"))?;
    let provider = match input.provider.as_str() {
        "XiaoMi" => OAuthProvider::XiaoMi,
        "QQ" => OAuthProvider::QQ,
        "Sina" => OAuthProvider::Sina,
        "RenRen" => OAuthProvider::RenRen,
        _ => bail!("provider must be XiaoMi, QQ, Sina or RenRen"),
    };
    let mut request = StorageTokenRequest::new(
        input.app_id,
        input.oauth_app_id,
        input.access_token,
        provider,
    );
    if let Some(mac) = input.xiaomi_mac {
        request = request.xiaomi_mac(mac.open_id, mac.key, mac.algorithm);
    }
    Ok(request)
}

pub async fn run(config: &Connection, command: &ServiceCmd) -> Result<()> {
    let service = FdsServiceClient::from_builder(config.builder()?)?;
    match command {
        ServiceCmd::Endpoint {
            region,
            service: name,
        } => println!("{}", service.get_endpoint(region, name).await?),
        ServiceCmd::Exchange { file } => {
            let token = service
                .exchange_storage_token(&read_exchange(file)?)
                .await?;
            super::util::print_json(
                &serde_json::json!({"app_id": token.app_id(), "expires_at": token.expires_at(), "expired": token.is_expired()}),
            )?;
        }
    }
    Ok(())
}
