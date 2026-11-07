//! Failure-safe downloads. Only the owning future can publish the destination.
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use md5::{Digest, Md5};
use reqwest::header::{HeaderValue, CONTENT_LENGTH, CONTENT_RANGE, ETAG, RANGE};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

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

// Held by the owner AND workers: aborted workers must exit before a new owner
// can reuse the staging file. No async filesystem operations survive cancellation.
struct Destination {
    path: PathBuf,
    tmp: PathBuf,
    checkpoint: PathBuf,
    lock: PathBuf,
    _lock_file: File,
}
impl Destination {
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
            lock,
            _lock_file: file,
        })
    }
}
impl Drop for Destination {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Serialize, Deserialize)]
struct Checkpoint {
    identity: Identity,
    hashes: Vec<Option<String>>,
}
impl Checkpoint {
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

fn effective_range(range: Option<crate::ByteRange>, size: u64) -> Result<(u64, u64), ManagerError> {
    range.map_or(Ok((0, size)), |range| {
        range.resolve(size).map_err(|_| ManagerError::RangeFormat)
    })
}

async fn download_part(
    client: Client,
    id: Arc<Identity>,
    destination: Arc<Destination>,
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
    let mut file = OpenOptions::new().write(true).open(&destination.tmp)?;
    file.seek(SeekFrom::Start(start - id.start))?;
    let mut received = 0u64;
    let mut hash = Md5::new();
    while let Some(chunk) = response.chunk().await.map_err(FdsError::from)? {
        if chunk.len() as u64 > end - start - received {
            return Err(invalid("oversized download part"));
        }
        // A chunk is transport-bounded; never accumulate a part in memory.
        for bytes in chunk.chunks(64 * 1024) {
            file.write_all(bytes)?;
            hash.update(bytes);
        }
        received += chunk.len() as u64;
    }
    if received != end - start {
        return Err(invalid("short download part"));
    }
    file.sync_data()?;
    Ok((index, hex::encode(hash.finalize())))
}

impl Downloader {
    pub fn new(client: impl AsRef<Client>, options: DownloadOptions) -> Self {
        Self {
            client: client.as_ref().clone(),
            options,
            #[cfg(test)]
            panic_worker: false,
        }
    }

    /// Atomically replace the destination only after every part is verified.
    /// Concurrent calls for the same canonical parent/filename fail while locked.
    /// Without a strong ETag, restart as one streaming request; do not resume.
    pub async fn download(&self, request: &DownloadRequest) -> Result<(), ManagerError> {
        crate::object::validate_object(&request.bucket_name, &request.object_name)?;
        let destination = Arc::new(Destination::acquire(&request.file_path)?);
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
        let mut checkpoint = Checkpoint {
            identity: (*id).clone(),
            hashes: vec![None; count],
        };
        if resume {
            match read_checkpoint(&destination.checkpoint) {
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
        if let Ok(meta) = std::fs::symlink_metadata(&destination.tmp) {
            if !meta.file_type().is_file() {
                return Err(invalid("staging path is not a regular file"));
            }
        }
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(&destination.tmp) {
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
            .open(&destination.tmp)?;
        if file.metadata()?.len() != end - start {
            checkpoint.hashes.fill(None);
        }
        for (index, saved) in checkpoint.hashes.iter_mut().enumerate() {
            if let Some(expected) = saved {
                let (a, b) = part_bounds(&id, index);
                if hash_part(&mut file, a - start, b - a)? != *expected {
                    *saved = None;
                }
            }
        }
        if checkpoint.hashes.iter().all(Option::is_none) {
            file.set_len(0)?;
        }
        file.set_len(end - start)?;
        if resume {
            checkpoint.dump(&destination.checkpoint)?;
        }
        self.run_workers(id, destination.clone(), &mut checkpoint, resume)
            .await?;
        file.sync_all()?;
        // Synchronous commit: cancellation cannot leave a detached rename running.
        std::fs::rename(&destination.tmp, &destination.path)?;
        let _ = std::fs::remove_file(&destination.checkpoint);
        Ok(())
    }

    async fn run_workers(
        &self,
        id: Arc<Identity>,
        destination: Arc<Destination>,
        checkpoint: &mut Checkpoint,
        resume: bool,
    ) -> Result<(), ManagerError> {
        let mut workers = JoinSet::new();
        let mut next = 0usize;
        loop {
            while workers.len() < self.options.concurrency && next < checkpoint.hashes.len() {
                let index = next;
                next += 1;
                if checkpoint.hashes[index].is_none() {
                    #[cfg(test)]
                    if self.panic_worker {
                        workers.spawn(async { panic!("injected worker panic") });
                        continue;
                    }
                    workers.spawn(download_part(
                        self.client.clone(),
                        id.clone(),
                        destination.clone(),
                        index,
                        self.options.request_limiter.clone(),
                    ));
                }
            }
            let Some(outcome) = workers.join_next().await else {
                break;
            };
            let result = match outcome {
                Ok(result) => result,
                Err(_) => Err(invalid("download worker panicked or exited early")),
            };
            let result = result.and_then(|(index, hash)| {
                checkpoint.hashes[index] = Some(hash);
                if resume {
                    checkpoint.dump(&destination.checkpoint)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                workers.abort_all();
                while workers.join_next().await.is_some() {}
                return Err(error);
            }
        }
        if checkpoint.hashes.iter().any(Option::is_none) {
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
