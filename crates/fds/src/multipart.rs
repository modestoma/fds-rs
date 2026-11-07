//! Explicitly owned, bounded-memory file multipart uploads.
use crate::{
    Client, FdsError, InitMultipartUploadRequest, InitMultipartUploadResponse, ObjectMetadata,
    PutObjectResponse, Result, UploadPartRequest,
};
use futures_util::{StreamExt, TryStreamExt};
use std::{path::Path, time::SystemTime};
use tokio::io::AsyncReadExt;
pub use tokio_util::sync::CancellationToken;

/// Action after a part/read failure or cooperative cancellation before completion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CleanupPolicy {
    /// Caller owns the unfinished upload; no network cleanup is attempted.
    #[default]
    Preserve,
    /// Stop local part futures, then attempt abort once. This cannot undo an
    /// already completed object and is never applied after completion starts.
    AbortOnFailure,
}

/// Local resource limits, not assertions about server multipart constraints.
#[derive(Debug, Clone)]
pub struct MultipartOptions {
    part_size: usize,
    concurrency: usize,
    cleanup: CleanupPolicy,
}
impl Default for MultipartOptions {
    fn default() -> Self {
        Self {
            part_size: 8 * 1024 * 1024,
            concurrency: 4,
            cleanup: CleanupPolicy::Preserve,
        }
    }
}
impl MultipartOptions {
    /// At most 64 MiB per part, 64 concurrent requests, 256 MiB combined part
    /// buffers and 10,000 parts. Match part size to the actual service limits.
    pub fn new(part_size: usize, concurrency: usize, cleanup: CleanupPolicy) -> Result<Self> {
        if part_size == 0
            || part_size > 64 * 1024 * 1024
            || concurrency == 0
            || concurrency > 64
            || part_size
                .checked_mul(concurrency)
                .is_none_or(|n| n > 256 * 1024 * 1024)
        {
            return Err(FdsError::invalid("invalid multipart resource limits"));
        }
        Ok(Self {
            part_size,
            concurrency,
            cleanup,
        })
    }
    pub fn part_size(&self) -> usize {
        self.part_size
    }
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }
    pub fn cleanup_policy(&self) -> CleanupPolicy {
        self.cleanup
    }
}

/// Local knowledge, not a query of server state. Dropping a run future can leave
/// Uploading or CompletionUnknown; both require explicit recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MultipartState {
    Ready,
    Uploading,
    Failed,
    CompletionUnknown,
    Completed,
    Aborted,
}

/// Inspectable receipts and identity for manual recovery. Receipts only cover
/// confirmed parts; missing receipts do NOT prove absence on the server.
#[derive(Clone)]
pub struct MultipartProgress {
    pub upload: InitMultipartUploadResponse,
    pub parts: Vec<crate::UploadPartResponse>,
    pub total_parts: usize,
    pub file_size: u64,
    pub state: MultipartState,
}
impl std::fmt::Debug for MultipartProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultipartProgress")
            .field("state", &self.state)
            .field("confirmed_parts", &self.parts.len())
            .field("total_parts", &self.total_parts)
            .finish_non_exhaustive()
    }
}

/// Original error and optional secondary cleanup error. Identity/progress remain
/// on the session, including when the upload future is dropped without an error.
#[derive(Debug, thiserror::Error)]
#[error("managed multipart upload failed")]
pub struct MultipartUploadError {
    #[source]
    pub cause: FdsError,
    pub cleanup_error: Option<FdsError>,
}
impl From<FdsError> for MultipartUploadError {
    fn from(cause: FdsError) -> Self {
        Self {
            cause,
            cleanup_error: None,
        }
    }
}

/// A single-run session. Keep it alive until success or explicit recovery.
/// No Drop implementation performs network IO. To cancel with cleanup, cancel
/// the token and keep awaiting `upload`; dropping the future skips cleanup.
/// The source file must remain unchanged for the entire session.
pub struct MultipartUpload {
    client: Client,
    file: tokio::fs::File,
    modified: Option<SystemTime>,
    metadata: Option<ObjectMetadata>,
    options: MultipartOptions,
    progress: MultipartProgress,
}
impl std::fmt::Debug for MultipartUpload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MultipartUpload")
            .field("progress", &self.progress)
            .finish_non_exhaustive()
    }
}
impl MultipartUpload {
    /// Open and validate a nonempty regular file, then initialize exactly once.
    /// Initialization errors (including a dropped future) may leave an upload
    /// whose ID was not received. Never blindly retry initialization.
    pub async fn start(
        client: &Client,
        request: InitMultipartUploadRequest,
        path: impl AsRef<Path>,
        options: MultipartOptions,
    ) -> Result<Self> {
        if request.bucket_name.is_empty() || request.object_name.is_empty() {
            return Err(FdsError::invalid("bucket and object must be non-empty"));
        }
        let file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(FdsError::invalid(
                "managed multipart requires a nonempty regular file; use PUT for empty files",
            ));
        }
        let count = (metadata.len() - 1) / options.part_size as u64 + 1;
        if count > 10_000 {
            return Err(FdsError::invalid("managed upload exceeds 10000 parts"));
        }
        let mut upload = client.init_multipart_upload(&request).await?;
        if upload.upload_id.is_empty() {
            return Err(FdsError::invalid(
                "multipart initialization returned no upload ID; outcome unknown",
            ));
        }
        // All subsequent requests stay bound to caller identity, never server-returned paths.
        upload.bucket_name = request.bucket_name;
        upload.object_name = request.object_name;
        Ok(Self {
            client: client.clone(),
            file,
            modified: metadata.modified().ok(),
            metadata: request.metadata,
            options,
            progress: MultipartProgress {
                upload,
                parts: Vec::new(),
                total_parts: count as usize,
                file_size: metadata.len(),
                state: MultipartState::Ready,
            },
        })
    }
    pub fn progress(&self) -> &MultipartProgress {
        &self.progress
    }

    /// Upload once, with bounded concurrent part requests; complete in numeric
    /// part order. Any completion failure is conservatively outcome-unknown.
    /// After failure/drop, inspect progress and use manual APIs to reconcile;
    /// calling upload again is rejected to prevent hidden replay.
    // Preserve the public recovery error shape throughout 0.2.x.
    #[allow(clippy::result_large_err)]
    pub async fn upload(
        &mut self,
        cancel: &CancellationToken,
    ) -> std::result::Result<PutObjectResponse, MultipartUploadError> {
        if self.progress.state != MultipartState::Ready {
            return Err(FdsError::invalid(
                "multipart session already run; explicit recovery required",
            )
            .into());
        }
        self.progress.state = MultipartState::Uploading;
        let result = self.upload_parts(cancel).await;
        if let Err(cause) = result {
            self.progress.state = MultipartState::Failed;
            let cleanup_error = if self.options.cleanup == CleanupPolicy::AbortOnFailure {
                match self
                    .client
                    .abort_multipart_upload(&self.progress.upload)
                    .await
                {
                    Ok(()) => {
                        self.progress.state = MultipartState::Aborted;
                        None
                    }
                    Err(error) => Some(error),
                }
            } else {
                None
            };
            return Err(MultipartUploadError {
                cause,
                cleanup_error,
            });
        }
        // Set BEFORE polling the network future, so external drop is recoverable.
        self.progress.state = MultipartState::CompletionUnknown;
        let request = crate::CompleteMultipartUploadRequest {
            bucket_name: self.progress.upload.bucket_name.clone(),
            object_name: self.progress.upload.object_name.clone(),
            upload_id: self.progress.upload.upload_id.clone(),
            upload_parts: self.progress.parts.clone(),
            metadata: self.metadata.clone(),
            expires: None,
        };
        let output = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(FdsError::Cancelled),
            result = self.client.complete_multipart_upload(&request) => result,
        }?;
        if output.object_name != self.progress.upload.object_name
            || (!output.bucket_name.is_empty()
                && output.bucket_name != self.progress.upload.bucket_name)
        {
            return Err(FdsError::invalid(
                "multipart completion returned an unexpected object identity; outcome unknown",
            )
            .into());
        }
        self.progress.state = MultipartState::Completed;
        Ok(output)
    }

    async fn upload_parts(&mut self, cancel: &CancellationToken) -> Result<()> {
        let part_size = self.options.part_size;
        let client = &self.client;
        let upload = &self.progress.upload;
        let source = futures_util::stream::try_unfold(
            (&mut self.file, self.progress.file_size, 1i32),
            move |(file, remaining, number)| async move {
                if remaining == 0 {
                    return Ok::<_, FdsError>(None);
                }
                let size = remaining.min(part_size as u64) as usize;
                let mut bytes = vec![0; size];
                file.read_exact(&mut bytes).await?;
                Ok(Some((
                    (number, bytes),
                    (file, remaining - size as u64, number + 1),
                )))
            },
        );
        let parts = source
            .map_ok(|(number, bytes)| async move {
                let size = bytes.len() as i64;
                let receipt = UploadPartRequest::new(
                    &upload.bucket_name,
                    &upload.object_name,
                    &upload.upload_id,
                    number,
                    bytes,
                )
                .send(client)
                .await?;
                if receipt.part_number != number
                    || receipt.part_size != size
                    || receipt.etag.is_empty()
                    || receipt.etag.len() > 1024
                {
                    return Err(FdsError::invalid(
                        "multipart part receipt does not match request",
                    ));
                }
                Ok(receipt)
            })
            .try_buffer_unordered(self.options.concurrency);
        {
            futures_util::pin_mut!(parts);
            loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(FdsError::Cancelled),
                    next = parts.next() => next,
                };
                match next {
                    Some(Ok(part)) => {
                        self.progress.parts.push(part);
                    }
                    Some(Err(error)) => return Err(error),
                    None => break,
                }
            }
        }
        self.progress.parts.sort_by_key(|part| part.part_number);
        let metadata = self.file.metadata().await?;
        if metadata.len() != self.progress.file_size || metadata.modified().ok() != self.modified {
            return Err(FdsError::invalid(
                "source file changed during multipart upload",
            ));
        }
        if cancel.is_cancelled() {
            return Err(FdsError::Cancelled);
        }
        Ok(())
    }
}
