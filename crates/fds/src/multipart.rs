//! Multipart uploads: the low-level requests (initiate, upload part,
//! complete, abort) and the explicitly owned, bounded-memory file upload
//! [`MultipartUpload`].
use crate::object::{ObjectMetadata, PutObjectResponse};
use crate::{Client, Dispatch, Error, ErrorKind, Result};
use futures_util::future::{Fuse, FutureExt};
use futures_util::{StreamExt, TryStreamExt};
use std::future::Future;
use std::pin::{pin, Pin};
use std::{path::Path, time::SystemTime};
use tokio::io::AsyncReadExt;

pub use crate::object::multipart::{
    CompleteMultipartUploadRequest, InitMultipartUploadRequest, InitMultipartUploadResponse,
    UploadPartRequest, UploadPartResponse,
};

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
            return Err(Error::invalid_input("invalid multipart resource limits"));
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
    pub parts: Vec<UploadPartResponse>,
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

/// A single-run session. Keep it alive until success or explicit recovery.
/// No Drop implementation performs network IO. To cancel with cleanup, let the
/// cancellation future passed to [`upload`](Self::upload) complete and keep
/// awaiting `upload`; dropping the future skips cleanup.
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
        crate::object::validate_object(&request.bucket_name, &request.object_name)?;
        let file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(Error::invalid_input(
                "managed multipart requires a nonempty regular file; use PUT for empty files",
            ));
        }
        let count = (metadata.len() - 1) / options.part_size as u64 + 1;
        if count > 10_000 {
            return Err(Error::invalid_input("managed upload exceeds 10000 parts"));
        }
        let mut upload = client.init_multipart_upload(&request).await?;
        if upload.upload_id.is_empty() {
            return Err(
                Error::protocol("multipart initialization returned no upload ID")
                    .into_outcome_unknown(
                        "multipart initialization outcome unknown; an upload may have been created",
                    ),
            );
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
    /// part order. After failure/drop, inspect progress and use manual APIs to
    /// reconcile; calling upload again is rejected to prevent hidden replay.
    ///
    /// Errors before completion are returned as they occurred. When
    /// [`CleanupPolicy::AbortOnFailure`] is set and the abort itself fails,
    /// the abort error's `Display` text is attached to the original error as
    /// the `cleanup_error` [context](Error::context) entry, and the state stays
    /// [`MultipartState::Failed`] instead of `Aborted`.
    ///
    /// Any completion failure after the completion request may have been sent
    /// (including cancellation while it is in flight) is conservatively
    /// [`ErrorKind::OutcomeUnknown`], with the cause in the source chain.
    ///
    /// # Cancellation
    ///
    /// `cancel` is any future that completes when the upload should stop
    /// cooperatively; completion yields [`ErrorKind::Cancelled`] (or
    /// `OutcomeUnknown` once completion is in flight) and still runs the
    /// configured [`CleanupPolicy`]. Pass [`std::future::pending()`] for no
    /// cancellation. A `tokio_util::sync::CancellationToken` works through
    /// `token.cancelled()`, a timeout through `tokio::time::sleep(..)`, and a
    /// oneshot receiver through `async { let _ = rx.await; }`.
    ///
    /// ```no_run
    /// # async fn example(upload: &mut fds::multipart::MultipartUpload) -> fds::Result<()> {
    /// let deadline = tokio::time::sleep(std::time::Duration::from_secs(3600));
    /// upload.upload(deadline).await?;
    /// # Ok(()) }
    /// ```
    pub async fn upload<C>(&mut self, cancel: C) -> Result<PutObjectResponse>
    where
        C: Future<Output = ()>,
    {
        if self.progress.state != MultipartState::Ready {
            return Err(Error::invalid_input(
                "multipart session already run; explicit recovery required",
            ));
        }
        // Fused: once the signal fires it is never polled again.
        let mut cancel = pin!(cancel.fuse());
        self.progress.state = MultipartState::Uploading;
        let result = self.upload_parts(cancel.as_mut()).await;
        if let Err(mut cause) = result {
            self.progress.state = MultipartState::Failed;
            if self.options.cleanup == CleanupPolicy::AbortOnFailure {
                match self
                    .client
                    .abort_multipart_upload(&self.progress.upload)
                    .await
                {
                    Ok(()) => self.progress.state = MultipartState::Aborted,
                    Err(error) => cause = cause.with_context("cleanup_error", error),
                }
            }
            return Err(cause);
        }
        // Set BEFORE polling the network future, so external drop is recoverable.
        self.progress.state = MultipartState::CompletionUnknown;
        let request = CompleteMultipartUploadRequest {
            bucket_name: self.progress.upload.bucket_name.clone(),
            object_name: self.progress.upload.object_name.clone(),
            upload_id: self.progress.upload.upload_id.clone(),
            upload_parts: self.progress.parts.clone(),
            metadata: self.metadata.clone(),
            expires: None,
        };
        const UNKNOWN: &str = "multipart completion outcome unknown; reconcile before retrying";
        let output = tokio::select! {
            biased;
            _ = cancel => Err(Error::new(
                ErrorKind::Cancelled,
                "cancelled while multipart completion was in flight",
            )
            .with_dispatch(Dispatch::Unknown)),
            result = self.client.complete_multipart_upload(&request) => result,
        }
        .map_err(|e| match e.dispatch() {
            Dispatch::NotSent => e,
            _ => e.into_outcome_unknown(UNKNOWN),
        })?;
        if output.object_name != self.progress.upload.object_name
            || (!output.bucket_name.is_empty()
                && output.bucket_name != self.progress.upload.bucket_name)
        {
            return Err(Error::protocol(
                "multipart completion returned an unexpected object identity",
            )
            .into_outcome_unknown(UNKNOWN));
        }
        self.progress.state = MultipartState::Completed;
        Ok(output)
    }

    async fn upload_parts<C>(&mut self, mut cancel: Pin<&mut Fuse<C>>) -> Result<()>
    where
        C: Future<Output = ()>,
    {
        let part_size = self.options.part_size;
        let client = &self.client;
        let upload = &self.progress.upload;
        let source = futures_util::stream::try_unfold(
            (&mut self.file, self.progress.file_size, 1i32),
            move |(file, remaining, number)| async move {
                if remaining == 0 {
                    return Ok::<_, Error>(None);
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
                    return Err(Error::protocol(
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
                    _ = cancel.as_mut() => return Err(cancelled()),
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
            return Err(Error::invalid_input(
                "source file changed during multipart upload",
            ));
        }
        if cancel.now_or_never().is_some() {
            return Err(cancelled());
        }
        Ok(())
    }
}

/// Part requests may have been in flight when the cancellation signal fired.
fn cancelled() -> Error {
    Error::new(ErrorKind::Cancelled, "multipart upload cancelled").with_dispatch(Dispatch::Unknown)
}
