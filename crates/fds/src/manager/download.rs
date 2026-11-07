//! Failure-safe downloads. Only the owning future can publish the destination.
//!
//! No filesystem call runs on an async runtime thread: locking, staging-file
//! validation, resume hashing, part writes, checkpoints, the final fsync, the
//! rename and cleanup all run in `spawn_blocking`.
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use md5::{Digest, Md5};
use reqwest::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, ETAG, RANGE};
use serde::{Deserialize, Serialize};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::task::TaskTracker;

use super::error::ManagerError;
use crate::client::{Client, ClientRequest};
use crate::constant::HttpMethod;
use crate::error::FdsError;

/// Validated, immutable download configuration. Build with named options.
#[derive(Clone)]
pub struct DownloadOptions {
    part_size: u64,
    concurrency: usize,
    resume: bool,
    request_limiter: Option<Arc<governor::DefaultDirectRateLimiter>>,
}
impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            part_size: 1024 * 1024,
            concurrency: 4,
            resume: false,
            request_limiter: None,
        }
    }
}
impl DownloadOptions {
    pub fn builder() -> DownloadOptionsBuilder {
        DownloadOptionsBuilder(Self::default())
    }
}
/// Named configuration builder; `build` validates all numeric limits.
pub struct DownloadOptionsBuilder(DownloadOptions);
impl DownloadOptionsBuilder {
    pub fn part_size(mut self, bytes: u64) -> Self {
        self.0.part_size = bytes;
        self
    }
    pub fn concurrency(mut self, workers: usize) -> Self {
        self.0.concurrency = workers;
        self
    }
    pub fn resume(mut self, enabled: bool) -> Self {
        self.0.resume = enabled;
        self
    }
    /// One permit per started part request; this is not byte bandwidth limiting.
    pub fn request_limiter(mut self, limiter: Arc<governor::DefaultDirectRateLimiter>) -> Self {
        self.0.request_limiter = Some(limiter);
        self
    }
    pub fn build(self) -> Result<DownloadOptions, ManagerError> {
        if self.0.part_size == 0 {
            return Err(ManagerError::PartSizeSmallerThanOne);
        }
        if self.0.part_size > i64::MAX as u64 {
            return Err(invalid("part size exceeds supported file size"));
        }
        if self.0.concurrency == 0 {
            return Err(ManagerError::ConcurrencySmallerThanOne);
        }
        Ok(self.0)
    }
}

/// Concurrent streaming downloader with fixed, validated options.
pub struct Downloader {
    client: Client,
    options: DownloadOptions,
    #[cfg(test)]
    panic_worker: bool,
    #[cfg(test)]
    checkpoint_writes: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub bucket_name: String,
    pub object_name: String,
    pub range: Option<crate::ByteRange>,
    pub file_path: PathBuf,
}
impl DownloadRequest {
    pub fn new(
        bucket: impl Into<String>,
        object: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bucket_name: bucket.into(),
            object_name: object.into(),
            file_path: path.into(),
            range: None,
        }
    }
    pub fn range(mut self, range: crate::ByteRange) -> Self {
        self.range = Some(range);
        self
    }
    pub async fn send(self, downloader: &Downloader) -> Result<(), ManagerError> {
        downloader.download(&self).await
    }
}

fn invalid(message: &'static str) -> ManagerError {
    ManagerError::Fds(FdsError::invalid(message))
}

/// Network bytes buffered per part before they are handed to one blocking
/// write. A worker holds at most one buffered batch (plus the chunk that
/// filled it) and one batch being written, so a part is never accumulated in
/// memory and each part needs only `part_size / WRITE_BATCH` blocking tasks.
const WRITE_BATCH: usize = 256 * 1024;
/// With resume enabled, the checkpoint is written for the first completed
/// part, then once `CHECKPOINT_EVERY_PARTS` more parts completed or
/// `CHECKPOINT_INTERVAL` passed (whichever comes first), and a last time when
/// the download fails. Each write is a create + `sync_all` + rename.
const CHECKPOINT_EVERY_PARTS: usize = 64;
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(2);

/// Runs blocking filesystem work off the async runtime threads.
async fn blocking<T, F>(work: F) -> Result<T, ManagerError>
where
    F: FnOnce() -> Result<T, ManagerError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| invalid("blocking file operation panicked"))?
}

fn lock_flag(flag: &Mutex<bool>) -> MutexGuard<'_, bool> {
    flag.lock().unwrap_or_else(PoisonError::into_inner)
}

// Shared by the owner, workers AND their blocking writes. The lock file is
// removed only when the last holder lets go, so an aborted worker's blocking
// write always ends before a new owner can reuse the staging file.
struct Destination {
    path: PathBuf,
    tmp: PathBuf,
    checkpoint: PathBuf,
    // Opened once after validation; every part writes through this handle.
    staging: OnceLock<File>,
    lock: Option<LockFile>,
}
struct LockFile {
    path: PathBuf,
    _file: File,
}
impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Destination {
    /// Blocking: canonicalizes the parent and takes the cooperative lock.
    fn acquire(input: &Path) -> Result<Self, ManagerError> {
        let parent = input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let path = parent.canonicalize()?.join(
            input
                .file_name()
                .ok_or_else(|| invalid("missing destination filename"))?,
        );
        let suffix = |s: &str| {
            let mut p = path.as_os_str().to_os_string();
            p.push(s);
            PathBuf::from(p)
        };
        let lock = suffix(".download.lock");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)?;
        Ok(Self {
            tmp: suffix(".tmp"),
            checkpoint: suffix(".download.bp"),
            path,
            staging: OnceLock::new(),
            lock: Some(LockFile {
                path: lock,
                _file: file,
            }),
        })
    }

    fn staging(&self) -> Result<&File, ManagerError> {
        self.staging
            .get()
            .ok_or_else(|| invalid("staging file is not open"))
    }

    /// Blocking: validates the staging file, verifies resumable parts and sizes
    /// the file. Returns the checkpoint the workers start from.
    fn prepare(
        &self,
        id: &Identity,
        count: usize,
        resume: bool,
    ) -> Result<Checkpoint, ManagerError> {
        let mut checkpoint = Checkpoint {
            identity: id.clone(),
            hashes: vec![None; count],
        };
        if resume {
            match read_checkpoint(&self.checkpoint) {
                Ok(data) => {
                    if let Ok(saved) = serde_json::from_slice::<Checkpoint>(&data) {
                        if saved.identity == *id && saved.hashes.len() == count {
                            checkpoint = saved;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        // Never write through a staging symlink or other special file.
        if let Ok(meta) = std::fs::symlink_metadata(&self.tmp) {
            if !meta.file_type().is_file() {
                return Err(invalid("staging path is not a regular file"));
            }
        }
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(&self.tmp) {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() != 1 {
                return Err(invalid("staging file has other hard links"));
            }
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.tmp)?;
        let length = id.end - id.start;
        if file.metadata()?.len() != length {
            checkpoint.hashes.fill(None);
        }
        for (index, saved) in checkpoint.hashes.iter_mut().enumerate() {
            if let Some(expected) = saved {
                let (a, b) = part_bounds(id, index);
                if hash_part(&mut file, a - id.start, b - a)? != *expected {
                    *saved = None;
                }
            }
        }
        if checkpoint.hashes.iter().all(Option::is_none) {
            file.set_len(0)?;
        }
        file.set_len(length)?;
        self.staging
            .set(file)
            .map_err(|_| invalid("staging file opened twice"))?;
        Ok(checkpoint)
    }

    /// Blocking: the single fsync of the download, then the atomic publish.
    /// `abandoned` is set (under the same mutex) when the owning future is
    /// dropped, so the rename either completed before that drop returned or
    /// never happens: cancellation cannot leave a detached rename behind.
    fn commit(&self, abandoned: &Mutex<bool>) -> Result<(), ManagerError> {
        let cancelled = || invalid("download cancelled before publishing");
        if *lock_flag(abandoned) {
            return Err(cancelled());
        }
        self.staging()?.sync_all()?;
        {
            let abandoned = lock_flag(abandoned);
            if *abandoned {
                return Err(cancelled());
            }
            std::fs::rename(&self.tmp, &self.path)?;
        }
        let _ = std::fs::remove_file(&self.checkpoint);
        Ok(())
    }

    /// Blocking: closes the staging handle and removes the lock file.
    fn release(mut self) {
        drop((self.staging.take(), self.lock.take()));
    }
}
impl Drop for Destination {
    // Has work only when the owning future was cancelled (the normal paths call
    // `release` on a blocking thread). Never unlink on a runtime thread; if the
    // runtime is shutting down the closure is dropped unrun, which still closes
    // the handle and removes the lock.
    fn drop(&mut self) {
        if self.lock.is_none() && self.staging.get().is_none() {
            return;
        }
        let parts = (self.staging.take(), self.lock.take());
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => drop(handle.spawn_blocking(move || drop(parts))),
            Err(_) => drop(parts),
        }
    }
}

/// Set when the download future is dropped; see [`Destination::commit`]. The
/// drop waits for at most one in-progress rename (a metadata-only syscall).
struct Abandon(Arc<Mutex<bool>>);
impl Drop for Abandon {
    fn drop(&mut self) {
        *lock_flag(&self.0) = true;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    version: u8,
    endpoint: String,
    bucket: String,
    object: String,
    size: u64,
    etag: String,
    start: u64,
    end: u64,
    part_size: u64,
}
#[derive(Default, Serialize, Deserialize)]
struct Checkpoint {
    identity: Identity,
    hashes: Vec<Option<String>>,
}
impl Checkpoint {
    /// Blocking.
    fn dump(&self, path: &Path) -> Result<(), ManagerError> {
        let mut name = path.as_os_str().to_os_string();
        name.push(".new");
        let staging = PathBuf::from(name);
        // Never follow a stale checkpoint staging symlink.
        match std::fs::remove_file(&staging) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        serde_json::to_writer(&mut f, self)?;
        f.sync_all()?;
        std::fs::rename(staging, path)?;
        Ok(())
    }
}

/// Completed-part state plus the checkpoint write throttle.
struct Progress {
    checkpoint: Checkpoint,
    path: PathBuf,
    resume: bool,
    // Completed parts not yet in the on-disk checkpoint.
    unwritten: usize,
    last_write: Option<Instant>,
    #[cfg(test)]
    writes: Arc<std::sync::atomic::AtomicUsize>,
}
impl Progress {
    fn complete(&mut self, index: usize, hash: String) {
        self.checkpoint.hashes[index] = Some(hash);
        self.unwritten += 1;
    }
    fn write_due(&self, now: Instant) -> bool {
        self.resume
            && self.unwritten > 0
            && self.last_write.is_none_or(|last| {
                self.unwritten >= CHECKPOINT_EVERY_PARTS
                    || now.duration_since(last) >= CHECKPOINT_INTERVAL
            })
    }
    /// Unconditional final write after a failure, so resume loses nothing.
    fn final_write_due(&self) -> bool {
        self.resume && self.unwritten > 0 && !self.checkpoint.hashes.is_empty()
    }
    async fn write(&mut self) -> Result<(), ManagerError> {
        #[cfg(test)]
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Move (not clone) the checkpoint to the blocking thread and back.
        let checkpoint = std::mem::take(&mut self.checkpoint);
        let path = self.path.clone();
        let (checkpoint, result) = tokio::task::spawn_blocking(move || {
            let result = checkpoint.dump(&path);
            (checkpoint, result)
        })
        .await
        .map_err(|_| invalid("checkpoint writer panicked"))?;
        self.checkpoint = checkpoint;
        result?;
        self.unwritten = 0;
        self.last_write = Some(Instant::now());
        Ok(())
    }
}

fn read_checkpoint(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = File::open(path)?;
    // At most one million 32-character digests plus JSON overhead.
    let mut data = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 64 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "checkpoint too large",
        ));
    }
    Ok(data)
}

fn part_bounds(id: &Identity, index: usize) -> (u64, u64) {
    let start = id.start + index as u64 * id.part_size;
    (start, start.saturating_add(id.part_size).min(id.end))
}
fn hash_part(file: &mut File, offset: u64, length: u64) -> Result<String, ManagerError> {
    file.seek(SeekFrom::Start(offset))?;
    let mut left = length;
    let mut buffer = [0u8; 64 * 1024];
    let mut hash = Md5::new();
    while left > 0 {
        let count = (left as usize).min(buffer.len());
        file.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        left -= count as u64;
    }
    Ok(hex::encode(hash.finalize()))
}

/// Positional write: concurrent parts share one handle without a shared cursor.
#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}
#[cfg(windows)]
fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_write(buf, offset) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
fn write_all_at(_: &File, _: &[u8], _: u64) -> std::io::Result<()> {
    Err(std::io::ErrorKind::Unsupported.into())
}

fn effective_range(range: Option<crate::ByteRange>, size: u64) -> Result<(u64, u64), ManagerError> {
    range.map_or(Ok((0, size)), |range| {
        range.resolve(size).map_err(|_| ManagerError::RangeFormat)
    })
}

/// What a worker needs to write parts. Blocking writes are tracked so the
/// owner can wait for them after aborting workers.
#[derive(Clone)]
struct Staging {
    destination: Arc<Destination>,
    tracker: TaskTracker,
}

enum PartDigest {
    Idle(Md5),
    Writing(JoinHandle<std::io::Result<Md5>>),
    Failed,
}

/// Streams one part into the staging file in order. Batches are written and
/// hashed on a blocking thread; at most one batch per part is in flight.
struct PartWriter {
    staging: Staging,
    offset: u64,
    batch: Vec<Bytes>,
    batch_len: usize,
    digest: PartDigest,
}
impl PartWriter {
    fn new(staging: Staging, offset: u64) -> Self {
        Self {
            staging,
            offset,
            batch: Vec::new(),
            batch_len: 0,
            digest: PartDigest::Idle(Md5::new()),
        }
    }
    async fn push(&mut self, chunk: Bytes) -> Result<(), ManagerError> {
        self.batch_len += chunk.len();
        self.batch.push(chunk);
        if self.batch_len >= WRITE_BATCH {
            self.flush().await?;
        }
        Ok(())
    }
    /// Waits for the in-flight batch and takes the running digest back.
    async fn settle(&mut self) -> Result<Md5, ManagerError> {
        match std::mem::replace(&mut self.digest, PartDigest::Failed) {
            PartDigest::Idle(hash) => Ok(hash),
            PartDigest::Writing(task) => Ok(task
                .await
                .map_err(|_| invalid("download writer panicked"))??),
            PartDigest::Failed => Err(invalid("download writer failed")),
        }
    }
    async fn flush(&mut self) -> Result<(), ManagerError> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let mut hash = self.settle().await?;
        let batch = std::mem::take(&mut self.batch);
        let mut offset = self.offset;
        self.offset += self.batch_len as u64;
        self.batch_len = 0;
        let destination = self.staging.destination.clone();
        self.digest = PartDigest::Writing(self.staging.tracker.spawn_blocking(move || {
            // Holding the destination keeps the lock (and the handle) alive
            // until this write ends, even if the worker has been aborted.
            let file = destination
                .staging
                .get()
                .ok_or_else(|| std::io::Error::other("staging file is not open"))?;
            for bytes in &batch {
                write_all_at(file, bytes, offset)?;
                hash.update(bytes);
                offset += bytes.len() as u64;
            }
            Ok(hash)
        }));
        Ok(())
    }
    async fn finish(mut self) -> Result<String, ManagerError> {
        self.flush().await?;
        Ok(hex::encode(self.settle().await?.finalize()))
    }
}

async fn download_part(
    client: Client,
    id: Arc<Identity>,
    staging: Staging,
    index: usize,
    limiter: Option<Arc<governor::DefaultDirectRateLimiter>>,
) -> Result<(usize, String), ManagerError> {
    if let Some(limiter) = limiter {
        limiter.until_ready().await;
    }
    let (start, end) = part_bounds(&id, index);
    let mut request = ClientRequest::new(&id.bucket, &id.object, HttpMethod::Get);
    request.accepted_statuses = &[200, 206];
    request.headers.insert(
        RANGE,
        HeaderValue::from_str(&format!("bytes={start}-{}", end - 1))
            .map_err(|_| invalid("invalid range"))?,
    );
    let mut response = client.do_request(request).await?;
    // Even a full-object request sent with Range must be honored; no silent 200 fallback.
    if response.status().as_u16() != 206
        || response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            != Some(format!("bytes {start}-{}/{}", end - 1, id.size).as_str())
    {
        return Err(invalid("download response does not match requested range"));
    }
    if !id.etag.is_empty()
        && response.headers().get(ETAG).and_then(|v| v.to_str().ok()) != Some(id.etag.as_str())
    {
        return Err(ManagerError::ObjectStateNotMatching);
    }
    if let Some(length) = response.headers().get(CONTENT_LENGTH) {
        if length.to_str().ok().and_then(|s| s.parse::<u64>().ok()) != Some(end - start) {
            return Err(invalid("incorrect part Content-Length"));
        }
    }
    let mut writer = PartWriter::new(staging, start - id.start);
    let mut received = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(FdsError::from)? {
        if chunk.len() as u64 > end - start - received {
            return Err(invalid("oversized download part"));
        }
        received += chunk.len() as u64;
        writer.push(chunk).await?;
    }
    if received != end - start {
        return Err(invalid("short download part"));
    }
    // No per-part fsync: the single `sync_all` happens before publishing, and
    // resume re-hashes local bytes instead of trusting the checkpoint.
    Ok((index, writer.finish().await?))
}

impl Downloader {
    pub fn new(client: impl AsRef<Client>, options: DownloadOptions) -> Self {
        Self {
            client: client.as_ref().clone(),
            options,
            #[cfg(test)]
            panic_worker: false,
            #[cfg(test)]
            checkpoint_writes: Default::default(),
        }
    }

    /// Atomically replace the destination only after every part is verified.
    /// Concurrent calls for the same canonical parent/filename fail while locked.
    /// Without a strong ETag, restart as one streaming request; do not resume.
    pub async fn download(&self, request: &DownloadRequest) -> Result<(), ManagerError> {
        crate::object::validate_object(&request.bucket_name, &request.object_name)?;
        let input = request.file_path.clone();
        let destination = Arc::new(blocking(move || Destination::acquire(&input)).await?);
        let result = self.download_locked(request, &destination).await;
        // Every worker and blocking write has ended; unlock off the runtime thread.
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(destination) = Arc::try_unwrap(destination) {
                destination.release();
            }
        })
        .await;
        result
    }

    async fn download_locked(
        &self,
        request: &DownloadRequest,
        destination: &Arc<Destination>,
    ) -> Result<(), ManagerError> {
        let head = self
            .client
            .do_request(ClientRequest::new(
                &request.bucket_name,
                &request.object_name,
                HttpMethod::Head,
            ))
            .await?;
        let size = head
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| invalid("HEAD requires Content-Length"))?;
        if size > i64::MAX as u64 {
            return Err(invalid("object exceeds supported file size"));
        }
        let etag = head
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .filter(|s| s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
            .unwrap_or("")
            .to_string();
        let (start, end) = effective_range(request.range, size)?;
        let part_size = if etag.is_empty() {
            (end - start).max(1)
        } else {
            self.options.part_size
        };
        let id = Arc::new(Identity {
            version: 2,
            endpoint: self.client.configuration().endpoint().to_string(),
            bucket: request.bucket_name.clone(),
            object: request.object_name.clone(),
            size,
            etag,
            start,
            end,
            part_size,
        });
        let count = (end - start).div_ceil(part_size);
        // Bound checkpoint metadata as well as active workers, before allocation.
        if count > 1_000_000 {
            return Err(invalid("too many download parts; increase part size"));
        }
        let count = count as usize;
        let resume = self.options.resume && !id.etag.is_empty();
        let checkpoint = {
            let (destination, id) = (destination.clone(), id.clone());
            blocking(move || destination.prepare(&id, count, resume)).await?
        };
        let mut progress = Progress {
            checkpoint,
            path: destination.checkpoint.clone(),
            resume,
            unwritten: 0,
            last_write: None,
            #[cfg(test)]
            writes: self.checkpoint_writes.clone(),
        };
        let tracker = TaskTracker::new();
        let staging = Staging {
            destination: destination.clone(),
            tracker: tracker.clone(),
        };
        let workers = self.run_workers(id, staging, &mut progress).await;
        // Aborted workers cannot cancel their blocking writes: wait for them
        // before publishing, writing the checkpoint or unlocking.
        tracker.close();
        tracker.wait().await;
        let result = match workers {
            Ok(()) => {
                let abandoned = Arc::new(Mutex::new(false));
                let _abandon = Abandon(abandoned.clone());
                let destination = destination.clone();
                blocking(move || destination.commit(&abandoned)).await
            }
            Err(error) => Err(error),
        };
        if result.is_err() && progress.final_write_due() {
            // Best effort: the original error is what the caller needs.
            let _ = progress.write().await;
        }
        result
    }

    async fn run_workers(
        &self,
        id: Arc<Identity>,
        staging: Staging,
        progress: &mut Progress,
    ) -> Result<(), ManagerError> {
        let mut workers = JoinSet::new();
        let mut next = 0usize;
        let count = progress.checkpoint.hashes.len();
        loop {
            while workers.len() < self.options.concurrency && next < count {
                let index = next;
                next += 1;
                if progress.checkpoint.hashes[index].is_none() {
                    #[cfg(test)]
                    if self.panic_worker {
                        workers.spawn(async { panic!("injected worker panic") });
                        continue;
                    }
                    workers.spawn(download_part(
                        self.client.clone(),
                        id.clone(),
                        staging.clone(),
                        index,
                        self.options.request_limiter.clone(),
                    ));
                }
            }
            let Some(outcome) = workers.join_next().await else {
                break;
            };
            let result = match outcome {
                Ok(Ok((index, hash))) => {
                    progress.complete(index, hash);
                    if progress.write_due(Instant::now()) {
                        progress.write().await
                    } else {
                        Ok(())
                    }
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(invalid("download worker panicked or exited early")),
            };
            if let Err(error) = result {
                workers.abort_all();
                while workers.join_next().await.is_some() {}
                return Err(error);
            }
        }
        if progress.checkpoint.hashes.iter().any(Option::is_none) {
            return Err(invalid("download incomplete"));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn panic_does_not_publish() {
        let mut server = mockito::Server::new_async().await;
        let _head = server
            .mock("HEAD", "/b/o")
            .with_status(200)
            .with_header("content-length", "8")
            .with_header("etag", "\"v1\"")
            .create_async()
            .await;
        let root = std::env::temp_dir().join(format!("fds-panic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("out");
        std::fs::write(&path, b"original").unwrap();
        let client = Client::builder()
            .endpoint(server.url())
            .credentials("ak", "sk")
            .build()
            .unwrap();
        let mut downloader = Downloader::new(
            client,
            crate::DownloadOptions::builder()
                .part_size(4)
                .concurrency(2)
                .resume(true)
                .build()
                .unwrap(),
        );
        downloader.panic_worker = true;
        let result = downloader
            .download(&DownloadRequest {
                bucket_name: "b".into(),
                object_name: "o".into(),
                file_path: path.to_str().unwrap().into(),
                range: None,
            })
            .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"original");
        assert!(!root.join("out.download.lock").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn progress(resume: bool) -> Progress {
        Progress {
            checkpoint: Checkpoint {
                identity: Identity::default(),
                hashes: vec![None; 1000],
            },
            path: PathBuf::new(),
            resume,
            unwritten: 0,
            last_write: None,
            writes: Default::default(),
        }
    }

    #[test]
    fn checkpoint_throttle_policy() {
        let t0 = Instant::now();
        let mut p = progress(true);
        assert!(!p.write_due(t0), "nothing completed yet");
        p.complete(0, "h".into());
        assert!(p.write_due(t0), "first completion is written at once");
        p.unwritten = 0;
        p.last_write = Some(t0);
        for index in 1..CHECKPOINT_EVERY_PARTS {
            p.complete(index, "h".into());
            assert!(!p.write_due(t0 + Duration::from_millis(10)));
        }
        p.complete(CHECKPOINT_EVERY_PARTS, "h".into());
        assert!(p.write_due(t0), "every N parts");
        p.unwritten = 1;
        assert!(!p.write_due(t0 + CHECKPOINT_INTERVAL - Duration::from_millis(1)));
        assert!(p.write_due(t0 + CHECKPOINT_INTERVAL), "every T seconds");
        assert!(p.final_write_due());
        p.unwritten = 0;
        assert!(!p.write_due(t0 + CHECKPOINT_INTERVAL * 10), "nothing new");
        assert!(!p.final_write_due());

        let mut p = progress(false);
        p.complete(0, "h".into());
        assert!(!p.write_due(t0) && !p.final_write_due(), "resume disabled");
    }

    async fn counted_download(fail: Option<usize>) -> (Result<(), ManagerError>, usize) {
        const PARTS: usize = 40;
        let mut server = mockito::Server::new_async().await;
        let _head = server
            .mock("HEAD", "/b/o")
            .with_status(200)
            .with_header("content-length", &(PARTS * 4).to_string())
            .with_header("etag", "\"v1\"")
            .create_async()
            .await;
        for index in 0..PARTS {
            let (start, end) = (index * 4, index * 4 + 3);
            let mock = server
                .mock("GET", "/b/o")
                .match_header("range", format!("bytes={start}-{end}").as_str());
            let mock = if fail == Some(index) {
                mock.with_status(500)
            } else {
                mock.with_status(206)
                    .with_header("etag", "\"v1\"")
                    .with_header(
                        "content-range",
                        &format!("bytes {start}-{end}/{}", PARTS * 4),
                    )
                    .with_body("abcd")
            };
            mock.create_async().await;
        }
        let root = std::env::temp_dir().join(format!(
            "fds-cp-count-{}-{}",
            std::process::id(),
            fail.unwrap_or(usize::MAX)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let client = Client::builder()
            .endpoint(server.url())
            .credentials("ak", "sk")
            .build()
            .unwrap();
        let downloader = Downloader::new(
            client,
            crate::DownloadOptions::builder()
                .part_size(4)
                .concurrency(1)
                .resume(true)
                .build()
                .unwrap(),
        );
        let result = downloader
            .download(&DownloadRequest::new("b", "o", root.join("out")))
            .await;
        std::fs::remove_dir_all(root).unwrap();
        let writes = downloader
            .checkpoint_writes
            .load(std::sync::atomic::Ordering::Relaxed);
        (result, writes)
    }

    // The original implementation wrote (and fsynced) 1 + PARTS checkpoints.
    #[tokio::test]
    async fn checkpoint_writes_are_throttled() {
        let (result, writes) = counted_download(None).await;
        result.unwrap();
        // First completion only; a second write is possible only if the
        // download took longer than CHECKPOINT_INTERVAL on a slow host.
        assert!((1..=2).contains(&writes), "{writes} checkpoint writes");
        let (result, writes) = counted_download(Some(20)).await;
        assert!(result.is_err());
        // First completion plus the final write after the failure.
        assert!((2..=3).contains(&writes), "{writes} checkpoint writes");
    }

    // Writes keep running after their worker is dropped (spawn_blocking cannot
    // be aborted); the tracker wait covers them and they hold the destination.
    #[tokio::test]
    async fn abandoned_part_writes_finish_before_tracker_wait_returns() {
        let root = std::env::temp_dir().join(format!("fds-writer-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let destination = Arc::new(Destination::acquire(&root.join("out")).unwrap());
        let len = 8 * WRITE_BATCH as u64;
        let id = Identity {
            start: 0,
            end: len,
            part_size: len,
            ..Identity::default()
        };
        destination.prepare(&id, 1, false).unwrap();
        let tracker = TaskTracker::new();
        let mut writer = PartWriter::new(
            Staging {
                destination: destination.clone(),
                tracker: tracker.clone(),
            },
            0,
        );
        for _ in 0..8 {
            writer
                .push(Bytes::from(vec![7u8; WRITE_BATCH]))
                .await
                .unwrap();
        }
        // The last batch is in flight; drop the writer as an abort would.
        drop(writer);
        tracker.close();
        tracker.wait().await;
        assert_eq!(Arc::strong_count(&destination), 1);
        let data = std::fs::read(root.join("out.tmp")).unwrap();
        assert_eq!(data.len() as u64, len);
        assert!(data.iter().all(|&b| b == 7));
        Arc::try_unwrap(destination).ok().unwrap().release();
        assert!(!root.join("out.download.lock").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ranges() {
        assert_eq!(
            effective_range(Some(crate::ByteRange::from(2)), 10).unwrap(),
            (2, 10)
        );
        assert_eq!(
            effective_range(Some(crate::ByteRange::suffix(3).unwrap()), 10).unwrap(),
            (7, 10)
        );
        assert_eq!(
            effective_range(Some(crate::ByteRange::inclusive(0, 999).unwrap()), 10).unwrap(),
            (0, 10)
        );
        for s in ["bytes=10-", "bytes=-0", "bytes=3-2", "bytes=0-1,3-4"] {
            assert!(s
                .parse::<crate::ByteRange>()
                .map_err(ManagerError::from)
                .and_then(|r| effective_range(Some(r), 10))
                .is_err());
        }
        assert_eq!(effective_range(None, 0).unwrap(), (0, 0));
    }
}
