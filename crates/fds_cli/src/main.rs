//! Command-line verification entry point for the public FDS SDK.
mod bucket;
mod config;
mod object;
mod service;
mod smoke;
mod transfer;
mod util;

mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "fds_cli",
    version,
    about = "Verify FDS SDK operations",
    arg_required_else_help = true
)]
struct Cli {
    #[command(flatten)]
    config: config::Connection,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Bucket operations, ACL, lifecycle and access logs
    Bucket {
        #[command(subcommand)]
        command: bucket::BucketCmd,
    },
    /// Object operations, metadata, ACL, cache and URLs
    Object {
        #[command(subcommand)]
        command: object::ObjectCmd,
    },
    /// Managed upload or individual multipart protocol operations
    Multipart {
        #[command(subcommand)]
        command: transfer::MultipartCmd,
    },
    /// Bounded-memory, concurrent, resumable file download
    Download(transfer::DownloadArgs),
    /// Unauthenticated discovery and explicit OAuth token exchange
    Service {
        #[command(subcommand)]
        command: service::ServiceCmd,
    },
    /// Opt-in read/write checks under a unique scratch prefix (not full coverage)
    Smoke {
        bucket: String,
        #[arg(long)]
        keep: bool,
        #[arg(long)]
        skip_multipart: bool,
        #[arg(long)]
        skip_download: bool,
    },
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let diagnostics = cli.config.diagnostics;
    let result = execute(cli).await;
    if diagnostics {
        if let Err(error) = &result {
            for cause in error.chain() {
                if let Some(response) = cause
                    .downcast_ref::<fds::FdsError>()
                    .and_then(fds::FdsError::response)
                {
                    eprintln!(
                        "{}",
                        serde_json::json!({"status": response.status(), "kind": format!("{:?}", response.kind()), "service_code": response.service_code(), "request_id": response.request_id(), "body": response.diagnostic_body(), "truncated": response.body_truncated()})
                    );
                    break;
                }
            }
        }
    }
    result
}

async fn execute(cli: Cli) -> Result<()> {
    if let Command::Service { command } = &cli.command {
        return service::run(&cli.config, command).await;
    }
    let client = cli.config.client().await?;
    match cli.command {
        Command::Bucket { command } => bucket::run_bucket(&client, &command).await,
        Command::Object { command } => object::run_object(&client, &command).await,
        Command::Multipart { command } => transfer::run(&client, command).await,
        Command::Download(args) => args.run(&client).await,
        Command::Smoke {
            bucket,
            keep,
            skip_multipart,
            skip_download,
        } => smoke::smoke(&client, &bucket, keep, skip_multipart, skip_download).await,
        Command::Service { .. } => unreachable!(),
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    #[test]
    fn command_schema_is_consistent() {
        Cli::command().debug_assert();
    }
}
