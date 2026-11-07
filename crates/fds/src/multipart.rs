//! Multipart uploads: the low-level requests (create, upload part, complete,
//! abort) and the explicitly owned, bounded-memory file upload
//! [`MultipartUpload`].
//!
//! # Manual multipart
//!
//! [`Client::create_multipart_upload`] initiates an upload and returns a
//! [`MultipartUploadSession`]; its [`upload_part`](MultipartUploadSession::upload_part),
//! [`complete`](MultipartUploadSession::complete) and
//! [`abort`](MultipartUploadSession::abort) return the same operation builders
//! as [`Client::upload_part`], [`Client::complete_multipart_upload`] and
//! [`Client::abort_multipart_upload`], which take the upload ID explicitly
//! (for example after a restart).
//!
//! ```no_run
//! # async fn example(client: &fds::Client) -> fds::Result<()> {
//! use std::num::NonZeroU16;
//!
//! let session = client.create_multipart_upload("bucket", "big.bin").send().await?;
//! let part = session
//!     .upload_part(NonZeroU16::MIN, b"first part".to_vec())
//!     .send()
//!     .await?;
//! session.complete([part]).send().await?;
//! # Ok(()) }
//! ```
//!
//! # Managed file upload
//!
//! [`Client::upload_file`] configures a [`MultipartUpload`];
//! [`UploadFile::start`] validates the options and the file, initiates the
//! upload and returns the session, and [`MultipartUpload::upload`] runs it
//! once. See [`MultipartUpload`] for inspecting progress and recovering
//! manually after a failure.
use crate::object::{ObjectMetadata, PutObjectOutput};
use crate::{Client, Dispatch, Error, ErrorKind, Result};
use futures_util::future::{Fuse, FutureExt};
use futures_util::{StreamExt, TryStreamExt};
use std::future::Future;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::pin::{pin, Pin};
use std::time::SystemTime;
use tokio::io::AsyncReadExt;

pub use crate::object::multipart::{
    AbortMultipartUpload, CompleteMultipartUpload, CreateMultipartUpload, MultipartUploadSession,
    UploadPart, UploadPartOutput, MAX_PART_NUMBER,
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

const DEFAULT_PART_SIZE: u64 = 8 * 1024 * 1024;
const DEFAULT_CONCURRENCY: usize = 4;
const MAX_PART_SIZE: u64 = 64 * 1024 * 1024;
const MAX_CONCURRENCY: usize = 64;
const MAX_BUFFERED: u64 = 256 * 1024 * 1024;

/// Builder for [`Client::upload_file`]: a managed multipart upload of a local
/// file. [`start`](Self::start) returns the [`MultipartUpload`] session.
#[derive(Debug)]
#[must_use = "an upload builder does nothing until `start` is awaited"]
pub struct UploadFile {
    client: Client,
    bucket: String,
    object: String,
    path: PathBuf,
    part_size: u64,
    concurrency: usize,
    cleanup: CleanupPolicy,
    metadata: Option<ObjectMetadata>,
}

impl UploadFile {
    /// Bytes per part (default 8 MiB), at most 64 MiB. Every part but the
    /// last has exactly this size; match it to the service's part limits.
    pub fn part_size(mut self, bytes: u64) -> Self {
        self.part_size = bytes;
        self
    }

    /// Part requests in flight at once (default 4), at most 64. Part buffers
    /// (`part_size × concurrency`) may total at most 256 MiB.
    pub fn concurrency(mut self, requests: usize) -> Self {
        self.concurrency = requests;
        self
    }

    /// What happens to the unfinished upload after a failure or cancellation
    /// before completion (default [`CleanupPolicy::Preserve`]).
    pub fn cleanup(mut self, policy: CleanupPolicy) -> Self {
        self.cleanup = policy;
        self
    }

    /// Metadata sent with the initiation and again with the completion.
    pub fn metadata(mut self, metadata: ObjectMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Validate, open the file and initiate the upload exactly once.
    ///
    /// The bucket and object names and the options are checked first, then
    /// the file must be a nonempty regular file of at most
    /// [`MAX_PART_NUMBER`] parts; each of these failures is
    /// [`ErrorKind::InvalidInput`] (or [`ErrorKind::Io`] for the file) with
    /// nothing sent. Initiation errors (including a dropped future) may leave
    /// an upload whose ID was not received: never retry it blindly.
    pub async fn start(self) -> Result<MultipartUpload> {
        crate::object::validate_object(&self.bucket, &self.object)?;
        if self.part_size == 0
            || self.part_size > MAX_PART_SIZE
            || self.concurrency == 0
            || self.concurrency > MAX_CONCURRENCY
            || self.part_size * self.concurrency as u64 > MAX_BUFFERED
        {
            return Err(Error::invalid_input("invalid multipart resource limits"));
        }
        let file = tokio::fs::File::open(&self.path).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(Error::invalid_input(
                "managed multipart requires a nonempty regular file; use PUT for empty files",
            ));
        }
        let count = (metadata.len() - 1) / self.part_size + 1;
        if count > u64::from(MAX_PART_NUMBER) {
            return Err(Error::invalid_input("managed upload exceeds 10000 parts"));
        }
        let mut create = self
            .client
            .create_multipart_upload(&self.bucket, &self.object);
        if let Some(metadata) = &self.metadata {
            create = create.metadata(metadata.clone());
        }
        let session = create.send().await?;
        Ok(MultipartUpload {
            file,
            modified: metadata.modified().ok(),
            metadata: self.metadata,
            part_size: self.part_size as usize,
            concurrency: self.concurrency,
            cleanup: self.cleanup,
            progress: MultipartProgress {
                session,
                parts: Vec::new(),
                total_parts: count as usize,
                file_size: metadata.len(),
                state: MultipartState::Ready,
            },
        })
    }
}

impl Client {
    /// Upload a local file in parts with bounded memory and concurrency.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use fds::multipart::CleanupPolicy;
    ///
    /// let mut upload = client
    ///     .upload_file("bucket", "big.bin", "/data/big.bin")
    ///     .part_size(16 * 1024 * 1024)
    ///     .concurrency(4)
    ///     .cleanup(CleanupPolicy::AbortOnFailure)
    ///     .start()
    ///     .await?;
    /// // Persist `upload.progress().session.upload_id()` for recovery here.
    /// upload.upload(std::future::pending()).await?;
    /// # Ok(()) }
    /// ```
    pub fn upload_file(
        &self,
        bucket: impl Into<String>,
        object: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> UploadFile {
        UploadFile {
            client: self.clone(),
            bucket: bucket.into(),
            object: object.into(),
            path: path.into(),
            part_size: DEFAULT_PART_SIZE,
            concurrency: DEFAULT_CONCURRENCY,
            cleanup: CleanupPolicy::Preserve,
            metadata: None,
        }
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
    /// The initiated upload, bound to the requested bucket and object.
    pub session: MultipartUploadSession,
    /// Confirmed part receipts; in ascending part order once uploading ended.
    pub parts: Vec<UploadPartOutput>,
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

/// A single-run managed upload session, created by [`UploadFile::start`].
///
/// Keep it alive until success or explicit recovery. No Drop implementation
/// performs network IO. To cancel with cleanup, let the cancellation future
/// passed to [`upload`](Self::upload) complete and keep awaiting `upload`;
/// dropping the future skips cleanup. The source file must remain unchanged
/// for the entire session.
///
/// # Recovering after a failure
///
/// [`upload`](Self::upload) runs at most once. After it failed, or after its
/// future was dropped, [`progress`](Self::progress) still holds the
/// [`MultipartUploadSession`], the confirmed receipts and the
/// [`MultipartState`]. Reconcile from there with the low-level builders
/// instead of running the upload again:
///
/// - [`MultipartState::Failed`] or `Uploading`: upload the missing parts with
///   [`MultipartUploadSession::upload_part`] (part `n` covers file bytes
///   `(n - 1) × part_size ..`), then complete with every receipt in part
///   order; or give up with [`MultipartUploadSession::abort`].
/// - [`MultipartState::CompletionUnknown`]: the completion may have taken
///   effect. Check the object before completing again or aborting; a `HEAD`
///   alone cannot prove that this upload produced it.
/// - [`MultipartState::Aborted`] and `Completed` need nothing further.
///
/// ```no_run
/// # async fn example(mut upload: fds::multipart::MultipartUpload) -> fds::Result<()> {
/// use fds::multipart::MultipartState;
/// use std::num::NonZeroU16;
///
/// if let Err(error) = upload.upload(std::future::pending()).await {
///     let progress = upload.progress();
///     if progress.state == MultipartState::Failed && progress.parts.len() + 1 == progress.total_parts {
///         // Re-read and upload the last part, then complete with every receipt.
///         let last = NonZeroU16::new(progress.total_parts as u16).unwrap();
///         let body = b"bytes of the last part".to_vec();
///         let receipt = progress.session.upload_part(last, body).send().await?;
///         let mut parts = progress.parts.clone();
///         parts.push(receipt);
///         progress.session.complete(parts).send().await?;
///     } else {
///         return Err(error);
///     }
/// }
/// # Ok(()) }
/// ```
pub struct MultipartUpload {
    file: tokio::fs::File,
    modified: Option<SystemTime>,
    metadata: Option<ObjectMetadata>,
    part_size: usize,
    concurrency: usize,
    cleanup: CleanupPolicy,
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
    pub async fn upload<C>(&mut self, cancel: C) -> Result<PutObjectOutput>
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
            if self.cleanup == CleanupPolicy::AbortOnFailure {
                match self.progress.session.abort().send().await {
                    Ok(()) => self.progress.state = MultipartState::Aborted,
                    Err(error) => cause = cause.with_context("cleanup_error", error),
                }
            }
            return Err(cause);
        }
        // Set BEFORE polling the network future, so external drop is recoverable.
        self.progress.state = MultipartState::CompletionUnknown;
        let mut complete = self.progress.session.complete(self.progress.parts.clone());
        if let Some(metadata) = &self.metadata {
            complete = complete.metadata(metadata.clone());
        }
        const UNKNOWN: &str = "multipart completion outcome unknown; reconcile before retrying";
        let output = tokio::select! {
            biased;
            _ = cancel => Err(Error::new(
                ErrorKind::Cancelled,
                "cancelled while multipart completion was in flight",
            )
            .with_dispatch(Dispatch::Unknown)),
            result = complete.send() => result,
        }
        .map_err(|e| match e.dispatch() {
            Dispatch::NotSent => e,
            _ => e.into_outcome_unknown(UNKNOWN),
        })?;
        let session = &self.progress.session;
        if output.object_name != session.object()
            || (!output.bucket_name.is_empty() && output.bucket_name != session.bucket())
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
        let part_size = self.part_size;
        let session = &self.progress.session;
        let source = futures_util::stream::try_unfold(
            (&mut self.file, self.progress.file_size, NonZeroU16::MIN),
            move |(file, remaining, number)| async move {
                if remaining == 0 {
                    return Ok::<_, Error>(None);
                }
                let size = remaining.min(part_size as u64) as usize;
                let mut bytes = vec![0; size];
                file.read_exact(&mut bytes).await?;
                // At most MAX_PART_NUMBER parts (checked by `start`), so the
                // next number exists whenever another part follows.
                let next = number.saturating_add(1);
                Ok(Some((
                    (number, bytes),
                    (file, remaining - size as u64, next),
                )))
            },
        );
        let parts = source
            .map_ok(|(number, bytes)| async move {
                let size = bytes.len() as u64;
                let receipt = session.upload_part(number, bytes).send().await?;
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
            .try_buffer_unordered(self.concurrency);
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
