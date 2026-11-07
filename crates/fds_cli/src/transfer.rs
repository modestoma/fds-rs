use super::output;
use super::util::{apply_metadata, print_json, read_json, BodyArgs, MetadataArgs};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use fds::{manager::*, multipart::*, object::*, *};
use std::{
    num::{NonZeroU16, NonZeroU32},
    path::PathBuf,
};

#[derive(Args)]
pub struct DownloadArgs {
    bucket: String,
    object: String,
    out: PathBuf,
    #[arg(long, default_value_t = 1024 * 1024)]
    part_size: u64,
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    #[arg(long)]
    resume: bool,
    #[arg(long)]
    range: Option<ByteRange>,
    /// Limit started part requests per second (not bytes per second)
    #[arg(long)]
    requests_per_second: Option<NonZeroU32>,
}
impl DownloadArgs {
    pub async fn run(&self, client: &Client) -> Result<()> {
        let mut download = client
            .download_file(&self.bucket, &self.object, &self.out)
            .part_size(self.part_size)
            .concurrency(self.concurrency);
        if self.resume {
            download = download.resume();
        }
        if let Some(rate) = self.requests_per_second {
            download = download.request_limiter(RequestLimiter::per_second(rate));
        }
        if let Some(range) = self.range {
            download = download.range(range);
        }
        download.send().await?;
        eprintln!("downloaded to {}", self.out.display());
        Ok(())
    }
}

#[derive(Args)]
pub struct Session {
    bucket: String,
    object: String,
    #[arg(long)]
    upload_id: String,
}

#[derive(Subcommand)]
pub enum MultipartCmd {
    /// Bounded-memory file upload with concurrency, cancellation and recovery receipt
    Upload {
        bucket: String,
        object: String,
        #[arg(long)]
        file: PathBuf,
        #[arg(long, default_value_t = 8 * 1024 * 1024)]
        part_size: usize,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
        /// Attempt SDK abort on part failure/cooperative Ctrl-C, never on ambiguous completion
        #[arg(long)]
        abort_on_failure: bool,
        /// New local JSONL file; records identity before parts and receipts after completion/failure
        #[arg(long)]
        recovery: PathBuf,
        #[command(flatten)]
        metadata: MetadataArgs,
    },
    /// Initialize once; prints bucketName/objectName/uploadId JSON
    Init {
        bucket: String,
        object: String,
        #[command(flatten)]
        metadata: MetadataArgs,
    },
    /// Upload one part; stdout is a receipt JSON object for complete --parts
    Part {
        #[command(flatten)]
        session: Session,
        #[arg(long, value_parser = clap::value_parser!(i32).range(1..))]
        number: i32,
        #[command(flatten)]
        body: BodyArgs,
        #[arg(long)]
        content_md5: Option<String>,
    },
    /// Complete with a JSON array of confirmed part receipts
    Complete {
        #[command(flatten)]
        session: Session,
        #[arg(long)]
        parts: PathBuf,
        #[command(flatten)]
        metadata: MetadataArgs,
        /// Response-signature expiration (RFC3339), not object TTL
        #[arg(long)]
        expires: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// Abort the explicitly identified session once
    Abort {
        #[command(flatten)]
        session: Session,
    },
}

fn identity(upload: &MultipartUploadSession) -> serde_json::Value {
    serde_json::json!({"bucketName": upload.bucket(), "objectName": upload.object(), "uploadId": upload.upload_id()})
}
fn progress(value: &MultipartProgress) -> serde_json::Value {
    serde_json::json!({"upload": identity(&value.session), "parts": value.parts, "state": format!("{:?}", value.state), "total_parts": value.total_parts, "file_size": value.file_size})
}
/// The SDK checks the upper bound (`MAX_PART_NUMBER`); clap already rejects
/// numbers below 1.
fn part_number(number: i32) -> Result<NonZeroU16> {
    u16::try_from(number)
        .ok()
        .and_then(NonZeroU16::new)
        .ok_or_else(|| {
            Error::new(ErrorKind::InvalidInput, "part number must be at most 10000").into()
        })
}

pub async fn run(client: &Client, command: MultipartCmd) -> Result<()> {
    match command {
        MultipartCmd::Init {
            bucket,
            object,
            metadata,
        } => {
            // The session is bound to the requested identity, not names echoed by the server.
            let upload = apply_metadata!(
                client.create_multipart_upload(&bucket, &object),
                metadata.build()?
            )
            .send()
            .await?;
            print_json(&identity(&upload))?;
        }
        MultipartCmd::Part {
            session,
            number,
            body,
            content_md5,
        } => {
            let mut request = client.upload_part(
                &session.bucket,
                &session.object,
                &session.upload_id,
                part_number(number)?,
                body.load().await?,
            );
            if let Some(md5) = content_md5 {
                request = request.content_md5(md5);
            }
            print_json(&request.send().await?)?;
        }
        MultipartCmd::Complete {
            session,
            parts,
            metadata,
            expires,
        } => {
            let parts: Vec<UploadPartOutput> = read_json(parts)?;
            let mut request = apply_metadata!(
                client.complete_multipart_upload(
                    &session.bucket,
                    &session.object,
                    &session.upload_id,
                    parts,
                ),
                metadata.build()?
            );
            if let Some(value) = expires {
                request = request.expires(value);
            }
            print_json(&output::put(&request.send().await?))?;
        }
        MultipartCmd::Abort { session } => {
            client
                .abort_multipart_upload(&session.bucket, &session.object, &session.upload_id)
                .send()
                .await?;
            output::ok()?;
        }
        MultipartCmd::Upload {
            bucket,
            object,
            file,
            part_size,
            concurrency,
            abort_on_failure,
            recovery,
            metadata,
        } => {
            // Validate before the recovery file is claimed: invalid options
            // must not leave a receipt behind (the SDK checks again in `start`).
            validate_upload_options(part_size, concurrency)?;
            let upload = apply_metadata!(
                client
                    .upload_file(&bucket, &object, file)
                    .part_size(part_size as u64)
                    .concurrency(concurrency)
                    .cleanup(if abort_on_failure {
                        CleanupPolicy::AbortOnFailure
                    } else {
                        CleanupPolicy::Preserve
                    }),
                metadata.build()?
            );
            // Claim the receipt path before any remote mutation. Never overwrite an older session.
            let mut receipt = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&recovery)
                .context("create recovery file (must not already exist)")?;
            write_receipt(
                &mut receipt,
                &serde_json::json!({"state":"Initializing", "bucketName":bucket, "objectName":object}),
            )?;
            let mut upload = upload.start().await?;
            if let Err(error) = write_receipt(&mut receipt, &progress(upload.progress())) {
                eprintln!(
                    "initialized session (receipt write failed): {}",
                    identity(&upload.progress().session)
                );
                return Err(error);
            }
            eprintln!(
                "session saved to {}; uploading {} parts",
                recovery.display(),
                upload.progress().total_parts
            );
            // Ctrl-C completes the cancellation signal; the SDK then stops,
            // runs its cleanup policy and returns the final state.
            let cancel = async {
                if let Err(error) = tokio::signal::ctrl_c().await {
                    eprintln!("Ctrl-C listener failed: {error}; cancelling upload");
                }
                eprintln!("cancelling; waiting for SDK cleanup and final state");
            };
            let result = upload.upload(cancel).await;
            let state = progress(upload.progress());
            if let Err(error) = write_receipt(&mut receipt, &state) {
                eprintln!("recovery write failed: {error}; final recovery JSON: {state}");
                if result.is_ok() {
                    return Err(error.context("upload completed but recovery write failed"));
                }
            }
            eprintln!(
                "state={:?}, confirmed_parts={}/{}",
                upload.progress().state,
                upload.progress().parts.len(),
                upload.progress().total_parts
            );
            match result {
                Ok(response) => print_json(&output::put(&response))?,
                Err(error) => {
                    if let Some((_, cleanup)) = error
                        .context()
                        .iter()
                        .find(|(key, _)| *key == "cleanup_error")
                    {
                        eprintln!("cleanup error: {cleanup}");
                    }
                    return Err(error.into());
                }
            }
        }
    }
    Ok(())
}

/// The managed-upload resource limits that `UploadFile::start` enforces
/// (part size 1 B to 64 MiB, concurrency 1 to 64, product at most 256 MiB),
/// with the same error. Checked by the CLI before it creates the recovery
/// file; `upload_limits_match_the_sdk` keeps both in sync.
fn validate_upload_options(part_size: usize, concurrency: usize) -> Result<()> {
    const MIB: u64 = 1024 * 1024;
    let (part_size, concurrency) = (part_size as u64, concurrency as u64);
    if (1..=64 * MIB).contains(&part_size)
        && (1..=64).contains(&concurrency)
        && part_size * concurrency <= 256 * MIB
    {
        Ok(())
    } else {
        Err(Error::new(ErrorKind::InvalidInput, "invalid multipart resource limits").into())
    }
}

fn write_receipt(file: &mut std::fs::File, value: &serde_json::Value) -> Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    // Append snapshots: an interrupted write cannot destroy the previous complete snapshot.
    file.seek(SeekFrom::End(0))?;
    serde_json::to_writer(&mut *file, value)?;
    writeln!(file)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_upload_options;

    // The CLI pre-check must accept exactly what the SDK accepts. With a
    // missing source file the SDK fails with InvalidInput for invalid options
    // and with Io (file not found) for valid ones; nothing is sent.
    #[tokio::test]
    async fn upload_limits_match_the_sdk() {
        const MIB: usize = 1024 * 1024;
        let client = fds::Client::builder()
            .endpoint("http://127.0.0.1:1")
            .credentials("ak", "sk")
            .build()
            .unwrap();
        let missing = std::env::temp_dir().join("fds-cli-missing-upload-source");
        let sizes = [0, 1, 4, MIB, 4 * MIB, 64 * MIB, 64 * MIB + 1, usize::MAX];
        let counts = [0, 1, 4, 16, 63, 64, 65, usize::MAX];
        for part_size in sizes {
            for concurrency in counts {
                let cli = validate_upload_options(part_size, concurrency);
                let sdk = client
                    .upload_file("b", "k", &missing)
                    .part_size(part_size as u64)
                    .concurrency(concurrency)
                    .start()
                    .await
                    .unwrap_err();
                let expected = if cli.is_ok() {
                    fds::ErrorKind::Io
                } else {
                    fds::ErrorKind::InvalidInput
                };
                assert_eq!(sdk.kind(), expected, "{part_size} x {concurrency}");
            }
        }
    }
}
