//! Streaming download types and response consumption helpers.

use crate::{Error, ObjectMetadata, Result};
use bytes::Bytes;
use reqwest::header::HeaderMap;
use std::{fmt, num::NonZeroU64};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// A single HTTP byte range. Closed bounds are inclusive.
/// Constructors reject reversed bounds and zero-length suffixes locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange(RangeKind);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeKind {
    Closed(u64, u64),
    From(u64),
    Suffix(NonZeroU64),
}
impl ByteRange {
    /// Resolve against the current object size. The end is exclusive.
    #[cfg(feature = "manager")]
    pub(crate) fn resolve(self, size: u64) -> Result<(u64, u64)> {
        let (start, end) = match self.0 {
            RangeKind::Closed(a, b) => (a, b.saturating_add(1).min(size)),
            RangeKind::From(a) => (a, size),
            RangeKind::Suffix(n) => (size.saturating_sub(n.get()), size),
        };
        if start >= end {
            return Err(Error::invalid_input("range is outside object"));
        }
        Ok((start, end))
    }
    pub fn inclusive(start: u64, end: u64) -> Result<Self> {
        if start > end {
            return Err(Error::invalid_input("range start exceeds end"));
        }
        Ok(Self(RangeKind::Closed(start, end)))
    }
    pub fn from(start: u64) -> Self {
        Self(RangeKind::From(start))
    }
    pub fn suffix(length: u64) -> Result<Self> {
        NonZeroU64::new(length)
            .map(|n| Self(RangeKind::Suffix(n)))
            .ok_or_else(|| Error::invalid_input("range suffix must be positive"))
    }
}
impl fmt::Display for ByteRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            RangeKind::Closed(start, end) => write!(f, "bytes={start}-{end}"),
            RangeKind::From(start) => write!(f, "bytes={start}-"),
            RangeKind::Suffix(length) => write!(f, "bytes=-{length}"),
        }
    }
}
impl std::str::FromStr for ByteRange {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        let invalid = || Error::invalid_input("expected a single bytes range");
        let (start, end) = value
            .strip_prefix("bytes=")
            .and_then(|s| s.split_once('-'))
            .ok_or_else(invalid)?;
        let number = |s: &str| -> Result<u64> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            s.parse().map_err(|_| invalid())
        };
        match (start.is_empty(), end.is_empty()) {
            (true, true) => Err(invalid()),
            (true, false) => Self::suffix(number(end)?),
            (false, true) => Ok(Self::from(number(start)?)),
            (false, false) => Self::inclusive(number(start)?, number(end)?),
        }
    }
}

/// Successful GET response. Body consumption is incremental unless `bytes` is used.
/// A server may ignore Range and return 200; inspect `status()` / `headers()`.
///
/// Reading the body is bounded by the client's read idle timeout, reset after
/// each read that makes progress. No total deadline applies by default; if one
/// was set via [`ClientBuilder::request_timeout`](crate::ClientBuilder::request_timeout)
/// or [`GetObjectRequest::timeout`](crate::GetObjectRequest::timeout), it keeps
/// running while [`chunk`](Self::chunk), [`bytes`](Self::bytes) or
/// [`copy_to`](Self::copy_to) consume the body and fails them once it passes.
#[non_exhaustive]
pub struct GetObjectOutput {
    response: reqwest::Response,
    metadata: ObjectMetadata,
}
impl GetObjectOutput {
    pub(crate) fn new(response: reqwest::Response) -> Self {
        let metadata = ObjectMetadata::from_headers(response.headers());
        Self { response, metadata }
    }
    pub fn status(&self) -> u16 {
        self.response.status().as_u16()
    }
    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }
    pub fn metadata(&self) -> &ObjectMetadata {
        &self.metadata
    }
    /// Read the next chunk; repeated calls after EOF return `None`.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        self.response.chunk().await.map_err(Error::body_failure)
    }
    /// Collect the remaining body in memory, not necessarily the entire object
    /// if chunks have already been consumed.
    pub async fn bytes(self) -> Result<Bytes> {
        self.response.bytes().await.map_err(Error::body_failure)
    }
    /// Copy the remaining body incrementally into a caller-owned writer.
    /// Returns bytes written by this call. Does not flush or shut down the writer.
    /// On error or cancellation, the writer may contain a partial body; no rollback
    /// or retry occurs. Inspect status/Range headers before copying when needed.
    pub async fn copy_to<W: AsyncWrite + Unpin + ?Sized>(mut self, writer: &mut W) -> Result<u64> {
        let mut written = 0;
        while let Some(chunk) = self.chunk().await? {
            writer.write_all(&chunk).await?;
            written += chunk.len() as u64;
        }
        Ok(written)
    }

    /// Escape hatch for transport-specific response features.
    pub fn into_raw(self) -> reqwest::Response {
        self.response
    }
}
impl fmt::Debug for GetObjectOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GetObjectOutput")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}
