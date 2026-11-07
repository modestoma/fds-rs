//! Owned uploads with explicit length and checksum semantics.
use std::{fmt, io, path::Path, pin::Pin};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue};
use tokio_util::io::ReaderStream;

use crate::{FdsError, Result};

type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

enum Source {
    Bytes(Bytes),
    Stream(ByteStream),
}

/// A known-length, owned upload body. Bytes can be explicitly replayed via
/// `try_clone`; files and streams are single-use. The SDK never retries uploads.
/// Stream producers must bound their chunk size and any upstream queues.
/// Unknown lengths are intentionally unsupported; no implicit buffering occurs.
pub struct UploadBody {
    source: Source,
    length: u64,
}

impl fmt::Debug for UploadBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadBody")
            .field("length", &self.length)
            .field("replayable", &matches!(self.source, Source::Bytes(_)))
            .finish()
    }
}

impl From<Bytes> for UploadBody {
    fn from(bytes: Bytes) -> Self {
        Self {
            length: bytes.len() as u64,
            source: Source::Bytes(bytes),
        }
    }
}
impl From<Vec<u8>> for UploadBody {
    fn from(bytes: Vec<u8>) -> Self {
        Bytes::from(bytes).into()
    }
}

impl UploadBody {
    /// Take a stream and its exact wire length. At most `length` bytes are sent.
    /// Premature EOF and a chunk exceeding the remaining length fail the upload.
    /// After the declared length, the source is dropped without polling trailing data.
    pub fn from_stream<S>(stream: S, length: u64) -> Self
    where
        S: Stream<Item = io::Result<Bytes>> + Send + 'static,
    {
        Self {
            source: Source::Stream(Box::pin(stream)),
            length,
        }
    }

    /// Open a regular file now and stream its current length in 64 KiB chunks.
    /// Keep the file unchanged until completion. Files are not automatically reopened
    /// or checksummed; use a precomputed checksum when MD5 calculation is enabled.
    pub async fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let file = tokio::fs::File::open(path).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            return Err(FdsError::invalid("upload requires a regular file"));
        }
        Ok(Self::from_stream(
            ReaderStream::with_capacity(file, 64 * 1024),
            metadata.len(),
        ))
    }

    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    /// Explicit replay is available only for immutable in-memory bytes.
    pub fn try_clone(&self) -> Option<Self> {
        match &self.source {
            Source::Bytes(bytes) => Some(bytes.clone().into()),
            Source::Stream(_) => None,
        }
    }

    pub(crate) fn prepare(
        self,
        headers: &mut HeaderMap,
        calculate_md5: bool,
    ) -> Result<reqwest::Body> {
        // Finalize body headers before signing. Never permit metadata to replace framing.
        if headers.contains_key("transfer-encoding") || headers.contains_key("content-length") {
            return Err(FdsError::invalid(
                "upload framing headers are derived from the body",
            ));
        }
        if calculate_md5 && !headers.contains_key("content-md5") {
            let Source::Bytes(bytes) = &self.source else {
                return Err(FdsError::invalid("stream/file upload requires precomputed content-md5 when MD5 calculation is enabled"));
            };
            use md5::{Digest, Md5};
            headers.insert(
                "content-md5",
                HeaderValue::from_str(&hex::encode(Md5::digest(bytes)))
                    .map_err(|_| FdsError::invalid("invalid checksum"))?,
            );
        }
        headers.insert("content-length", HeaderValue::from(self.length));
        match self.source {
            // Keep each HTTP attempt single-use even for replayable SDK bytes.
            // Callers may explicitly build another attempt via try_clone().
            Source::Bytes(bytes) => Ok(reqwest::Body::wrap_stream(futures_util::stream::once(
                async move { Ok::<_, io::Error>(bytes) },
            ))),
            Source::Stream(stream) => {
                let stream = futures_util::stream::try_unfold(
                    (stream, self.length),
                    |(mut stream, remaining)| async move {
                        if remaining == 0 {
                            return Ok(None);
                        }
                        loop {
                            let bytes = stream.next().await.ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "upload shorter than declared length",
                                )
                            })??;
                            if bytes.len() as u64 > remaining {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "upload chunk exceeds declared length",
                                ));
                            }
                            if bytes.is_empty() {
                                tokio::task::yield_now().await;
                            } else {
                                let remaining = remaining - bytes.len() as u64;
                                return Ok(Some((bytes, (stream, remaining))));
                            }
                        }
                    },
                );
                Ok(reqwest::Body::wrap_stream(stream))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_and_http_attempt_are_explicit() {
        let data = vec![42; 1024];
        let pointer = data.as_ptr();
        let body = UploadBody::from(data);
        let cloned = body.try_clone().unwrap();
        for body in [&body, &cloned] {
            let Source::Bytes(bytes) = &body.source else {
                panic!("bytes expected")
            };
            assert_eq!(bytes.as_ptr(), pointer);
        }
        let mut headers = HeaderMap::new();
        let http_body = body.prepare(&mut headers, false).unwrap();
        let request = reqwest::Client::new()
            .put("http://localhost/")
            .body(http_body)
            .build()
            .unwrap();
        assert!(request.try_clone().is_none());
        assert_eq!(headers["content-length"], "1024");
    }
}
