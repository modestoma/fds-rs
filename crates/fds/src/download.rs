//! Streaming download types and response consumption helpers.

use crate::object::ObjectMetadata;
use crate::{Error, ErrorKind, Result};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::{Stream, TryStreamExt};
use http::header::{
    HeaderMap, HeaderName, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, LAST_MODIFIED,
};
use http::StatusCode;
use std::{fmt, num::NonZeroU64};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::io::StreamReader;

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

/// The `Content-Range` of a partial response: an inclusive byte range and,
/// when the service states it, the complete object size.
///
/// Returned by [`GetObjectOutput::content_range`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    start: u64,
    end: u64,
    complete_length: Option<u64>,
}

impl ContentRange {
    /// First byte position.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// Last byte position, inclusive.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The complete object size, or `None` when the service sent `*`.
    pub fn complete_length(&self) -> Option<u64> {
        self.complete_length
    }

    /// Parse `bytes <start>-<end>/<length or *>`; `None` for anything else,
    /// including reversed bounds and an end beyond the stated length.
    fn parse(value: &str) -> Option<Self> {
        let number = |s: &str| -> Option<u64> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse().ok()
        };
        let (range, length) = value.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = range.split_once('-')?;
        let (start, end) = (number(start)?, number(end)?);
        let complete_length = match length {
            "*" => None,
            length => Some(number(length)?),
        };
        if start > end || complete_length.is_some_and(|length| end >= length) {
            return None;
        }
        Some(Self {
            start,
            end,
            complete_length,
        })
    }
}

/// The headers of a response, as received; a borrowed view returned by
/// [`GetObjectOutput::headers`]. Names are lowercase.
///
/// Prefer the typed accessors of [`GetObjectOutput`] and its
/// [`metadata`](GetObjectOutput::metadata); this view is for diagnostics and
/// for headers the SDK does not model. `Debug` prints the names only.
#[derive(Clone, Copy)]
pub struct ResponseHeaders<'a>(&'a HeaderMap);

impl<'a> ResponseHeaders<'a> {
    /// The first value of a header, looked up ignoring case; `None` when it
    /// is missing or not visible ASCII.
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.0.get(name)?.to_str().ok()
    }

    /// Every value of a header, looked up ignoring case, as raw bytes in the
    /// order received.
    pub fn get_all(&self, name: &str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.0
            .get_all(name)
            .into_iter()
            .map(|value| value.as_bytes())
    }

    /// Every header value with its name, as raw bytes: names in the order
    /// first received, the values of a repeated name together.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a [u8])> + 'a {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_bytes()))
    }

    /// The number of header values, counting each value of a repeated name.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for ResponseHeaders<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// Successful GET response. Body consumption is incremental unless `bytes` is used.
/// A server may ignore Range and return 200; check [`is_partial`](Self::is_partial)
/// and [`content_range`](Self::content_range) before relying on a range.
///
/// The typed accessors read the response headers. They return `None` for a
/// missing header and for a value that does not parse, so an unusual header
/// never fails the download; [`headers`](Self::headers) keeps the raw values.
/// [`metadata`](Self::metadata) parses the same headers into the object's
/// attributes and user metadata.
///
/// Reading the body is bounded by the client's read idle timeout, reset after
/// each read that makes progress. No total deadline applies by default; if one
/// was set via [`ClientBuilder::request_timeout`](crate::ClientBuilder::request_timeout)
/// or [`GetObject::timeout`](crate::object::GetObject::timeout), it keeps
/// running while [`chunk`](Self::chunk), [`bytes`](Self::bytes),
/// [`copy_to`](Self::copy_to), [`into_stream`](Self::into_stream) or
/// [`into_async_read`](Self::into_async_read) consume the body and fails them
/// once it passes.
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
    /// The response status: 200, or 206 for a ranged request the service
    /// honored.
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }
    /// Whether the service answered with 206 Partial Content.
    pub fn is_partial(&self) -> bool {
        self.status() == StatusCode::PARTIAL_CONTENT
    }
    /// Every response header as received.
    pub fn headers(&self) -> ResponseHeaders<'_> {
        ResponseHeaders(self.response.headers())
    }
    /// The object's attributes and user metadata, parsed from the response
    /// headers. Its [`content_length`](crate::object::ObjectAttributes::content_length)
    /// is this response's, like [`content_length`](Self::content_length).
    pub fn metadata(&self) -> &ObjectMetadata {
        &self.metadata
    }
    /// `Content-Length`: the number of body bytes this response carries (the
    /// range length for a partial response).
    pub fn content_length(&self) -> Option<u64> {
        crate::metadata::parse_decimal(self.header(CONTENT_LENGTH)?)
    }
    /// `Content-Type`, verbatim.
    pub fn content_type(&self) -> Option<&str> {
        self.header(CONTENT_TYPE)
    }
    /// `ETag`, verbatim (including any quotes).
    pub fn etag(&self) -> Option<&str> {
        self.header(ETAG)
    }
    /// `Last-Modified`, parsed as an HTTP date in the IMF-fixdate form
    /// (`Sun, 06 Nov 1994 08:49:37 GMT`); the obsolete RFC 850 and asctime
    /// forms read as `None`.
    pub fn last_modified(&self) -> Option<DateTime<Utc>> {
        crate::metadata::parse_http_date(self.header(LAST_MODIFIED)?)
    }
    /// `Content-Range` of a partial response, parsed.
    pub fn content_range(&self) -> Option<ContentRange> {
        ContentRange::parse(self.header(CONTENT_RANGE)?)
    }
    fn header(&self, name: HeaderName) -> Option<&str> {
        self.response.headers().get(name)?.to_str().ok()
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
    /// The remaining body as a stream of chunks, with the same errors as
    /// [`chunk`](Self::chunk). The stream ends after the last chunk; after an
    /// error, do not rely on further items.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// use futures_util::TryStreamExt;
    ///
    /// let output = client.get_object("bucket", "key").send().await?;
    /// let mut body = output.into_stream();
    /// while let Some(chunk) = body.try_next().await? {
    ///     let _ = chunk;
    /// }
    /// # Ok(()) }
    /// ```
    pub fn into_stream(self) -> impl Stream<Item = Result<Bytes>> + Send + Unpin + 'static {
        self.response.bytes_stream().map_err(Error::body_failure)
    }
    /// The remaining body as an [`AsyncRead`], for example for
    /// [`tokio::io::copy`] or a decoder.
    ///
    /// A failure is reported as a [`std::io::Error`] (kind `TimedOut` for a
    /// timeout, `Other` otherwise) wrapping the [`Error`] that
    /// [`chunk`](Self::chunk) would return; recover it with
    /// `io_error.into_inner()` and `downcast::<fds::Error>()`.
    ///
    /// ```no_run
    /// # async fn example(client: &fds::Client) -> fds::Result<()> {
    /// let output = client.get_object("bucket", "key").send().await?;
    /// let mut reader = output.into_async_read();
    /// let mut file = tokio::fs::File::create("local.bin").await?;
    /// tokio::io::copy(&mut reader, &mut file).await?;
    /// # Ok(()) }
    /// ```
    pub fn into_async_read(self) -> impl AsyncRead + Send + Unpin + 'static {
        StreamReader::new(self.into_stream().map_err(|error| {
            let kind = match error.kind() {
                ErrorKind::Timeout => std::io::ErrorKind::TimedOut,
                _ => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, error)
        }))
    }
}
impl fmt::Debug for GetObjectOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GetObjectOutput")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}
