use super::output;
use super::util::{print_json, read_json, BodyArgs, MetadataArgs};
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use fds::*;
use std::{num::NonZeroU32, path::PathBuf, sync::Arc};

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
        let mut options = DownloadOptions::builder()
            .part_size(self.part_size)
            .concurrency(self.concurrency)
            .resume(self.resume);
        if let Some(rate) = self.requests_per_second {
            options = options.request_limiter(Arc::new(governor::RateLimiter::direct(
                governor::Quota::per_second(rate),
            )));
        }
        let downloader = Downloader::new(client.clone(), options.build()?);
        let mut request = DownloadRequest::new(&self.bucket, &self.object, &self.out);
        if let Some(range) = self.range {
            request = request.range(range);
        }
        downloader.download(&request).await?;
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
impl Session {
    fn sdk(&self) -> InitMultipartUploadResponse {
        InitMultipartUploadResponse {
            bucket_name: self.bucket.clone(),
            object_name: self.object.clone(),
            upload_id: self.upload_id.clone(),
        }
    }
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

fn identity(upload: &InitMultipartUploadResponse) -> serde_json::Value {
    serde_json::json!({"bucketName": upload.bucket_name, "objectName": upload.object_name, "uploadId": upload.upload_id})
}
fn progress(value: &MultipartProgress) -> serde_json::Value {
    serde_json::json!({"upload": identity(&value.upload), "parts": value.parts, "state": format!("{:?}", value.state), "total_parts": value.total_parts, "file_size": value.file_size})
}

pub async fn run(client: &Client, command: MultipartCmd) -> Result<()> {
    match command {
        MultipartCmd::Init {
            bucket,
            object,
            metadata,
        } => {
            let upload = InitMultipartUploadRequest::new(&bucket, &object)
                .metadata(metadata.build()?)
                .send(client)
                .await?;
            // Bind receipts to requested identity, not paths echoed by the server.
            print_json(&identity(&InitMultipartUploadResponse {
                bucket_name: bucket,
                object_name: object,
                upload_id: upload.upload_id,
            }))?;
        }
        MultipartCmd::Part {
            session,
            number,
            body,
            content_md5,
        } => {
            let mut request = session.sdk().part(number, body.load().await?);
            if let Some(md5) = content_md5 {
                request = request.content_md5(md5);
            }
            print_json(&request.send(client).await?)?;
        }
        MultipartCmd::Complete {
            session,
            parts,
            metadata,
            expires,
        } => {
            let parts: Vec<UploadPartResponse> = read_json(parts)?;
            let mut request = session.sdk().complete(parts).metadata(metadata.build()?);
            if let Some(value) = expires {
                request = request.expires(value);
            }
            print_json(&output::put(&request.send(client).await?))?;
        }
        MultipartCmd::Abort { session } => {
            session.sdk().abort(client).await?;
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
            let options = MultipartOptions::new(
                part_size,
                concurrency,
                if abort_on_failure {
                    CleanupPolicy::AbortOnFailure
                } else {
                    CleanupPolicy::Preserve
                },
            )?;
            let request =
                InitMultipartUploadRequest::new(bucket, object).metadata(metadata.build()?);
            // Claim the receipt path before any remote mutation. Never overwrite an older session.
            let mut receipt = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&recovery)
                .context("create recovery file (must not already exist)")?;
            write_receipt(
                &mut receipt,
                &serde_json::json!({"state":"Initializing", "bucketName":request.bucket_name, "objectName":request.object_name}),
            )?;
            let mut upload = MultipartUpload::start(client, request, file, options).await?;
            if let Err(error) = write_receipt(&mut receipt, &progress(upload.progress())) {
                eprintln!(
                    "initialized session (receipt write failed): {}",
                    identity(&upload.progress().upload)
                );
                return Err(error);
            }
            eprintln!(
                "session saved to {}; uploading {} parts",
                recovery.display(),
                upload.progress().total_parts
            );
            let token = CancellationToken::new();
            let result = {
                let running = upload.upload(&token);
                tokio::pin!(running);
                tokio::select! {
                    result = &mut running => result,
                    signal = tokio::signal::ctrl_c() => {
                        if let Err(error) = signal { eprintln!("Ctrl-C listener failed: {error}; cancelling upload"); }
                        token.cancel();
                        eprintln!("cancelling; waiting for SDK cleanup and final state");
                        running.await
                    }
                }
            };
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
                    if let Some(cleanup) = &error.cleanup_error {
                        eprintln!("cleanup error: {cleanup}");
                    }
                    return Err(error.into());
                }
            }
        }
    }
    Ok(())
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
