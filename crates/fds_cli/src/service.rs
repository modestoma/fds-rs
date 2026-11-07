use super::config::Connection;
use anyhow::{bail, Context, Result};
use clap::Subcommand;
use fds::oauth::{ExchangeStorageToken, FdsServiceClient, OAuthProvider};
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
struct ExchangeFile {
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

/// A parsed OAuth input file; [`ExchangeInput::exchange`] turns it into the
/// SDK's exchange builder.
pub struct ExchangeInput {
    file: ExchangeFile,
    provider: OAuthProvider,
}

impl ExchangeInput {
    pub fn exchange(self, service: &FdsServiceClient) -> ExchangeStorageToken {
        let file = self.file;
        let request = service.exchange_storage_token(
            file.app_id,
            file.oauth_app_id,
            file.access_token,
            self.provider,
        );
        match file.xiaomi_mac {
            Some(mac) => request.xiaomi_mac(mac.open_id, mac.key, mac.algorithm),
            None => request,
        }
    }
}

pub fn read_exchange(path: &Path) -> Result<ExchangeInput> {
    // Do not include parser errors: unknown fields/variants may contain secrets.
    let bytes = std::fs::read(path).context("read OAuth input file")?;
    let file: ExchangeFile = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid OAuth JSON; see docs/cli.md for its schema"))?;
    let provider = match file.provider.as_str() {
        "XiaoMi" => OAuthProvider::XiaoMi,
        "QQ" => OAuthProvider::QQ,
        "Sina" => OAuthProvider::Sina,
        "RenRen" => OAuthProvider::RenRen,
        _ => bail!("provider must be XiaoMi, QQ, Sina or RenRen"),
    };
    Ok(ExchangeInput { file, provider })
}

pub async fn run(config: &Connection, command: &ServiceCmd) -> Result<()> {
    let service = FdsServiceClient::from_builder(config.builder()?)?;
    match command {
        ServiceCmd::Endpoint {
            region,
            service: name,
        } => println!("{}", service.get_endpoint(region, name).send().await?),
        ServiceCmd::Exchange { file } => {
            let token = read_exchange(file)?.exchange(&service).send().await?;
            super::util::print_json(
                &serde_json::json!({"app_id": token.app_id(), "expires_at": token.expires_at(), "expired": token.is_expired()}),
            )?;
        }
    }
    Ok(())
}
