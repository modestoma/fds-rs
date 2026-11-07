//! SDK-managed I/O idle timing, applied with the client's `read_timeout`.
//!
//! reqwest's own `read_timeout` is a single deadline until the response headers
//! arrive and is not reset by request-body progress, so it cannot bound uploads
//! by idleness. The SDK therefore does not configure it and instead:
//!
//! - wraps every request body in [`TrackedBody`], which records the time of the
//!   last chunk handed to the transport in a shared [`ProgressClock`];
//! - races each send against [`ProgressClock::stalled`], which completes once
//!   neither upload progress nor response headers occurred for the idle period
//!   (for bodyless requests this is a deadline from the start of the attempt);
//! - wraps every response body in [`IdleBody`], which fails a read that waits
//!   longer than the idle period, reset after every received frame.

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use futures_util::Stream;
use tokio::time::{Instant, Sleep};

/// Largest piece handed to the transport at once. Splitting large chunks keeps
/// progress observable while the transport drains a big in-memory body.
pub(crate) const MAX_PIECE: usize = 64 * 1024;

/// The time of the last observed upload progress, shared between the body and
/// the send watchdog of one attempt.
pub(crate) struct ProgressClock {
    start: Instant,
    /// Nanoseconds since `start` of the last progress; only ever increases.
    last: AtomicU64,
}

impl ProgressClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            last: AtomicU64::new(0),
        })
    }

    fn touch(&self) {
        let nanos = u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.last.fetch_max(nanos, Ordering::Relaxed);
    }

    /// Completes once no progress has been recorded for `idle`.
    pub(crate) async fn stalled(&self, idle: Duration) {
        loop {
            let last = Duration::from_nanos(self.last.load(Ordering::Relaxed));
            let Some(deadline) = self
                .start
                .checked_add(last)
                .and_then(|instant| instant.checked_add(idle))
            else {
                return std::future::pending().await;
            };
            if Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep_until(deadline).await;
        }
    }
}

type ByteStream = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send>>;

/// A request body stream that yields at most [`MAX_PIECE`] bytes at a time and
/// records progress on the clock whenever the transport pulls a piece.
pub(crate) struct TrackedBody {
    inner: ByteStream,
    pending: Bytes,
    clock: Arc<ProgressClock>,
}

impl TrackedBody {
    pub(crate) fn new(inner: ByteStream, clock: Arc<ProgressClock>) -> Self {
        Self {
            inner,
            pending: Bytes::new(),
            clock,
        }
    }
}

impl Stream for TrackedBody {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        while this.pending.is_empty() {
            match ready!(this.inner.as_mut().poll_next(cx)) {
                Some(Ok(bytes)) => this.pending = bytes,
                other => return Poll::Ready(other),
            }
        }
        let piece = this.pending.split_to(this.pending.len().min(MAX_PIECE));
        this.clock.touch();
        Poll::Ready(Some(Ok(piece)))
    }
}

/// A response body whose reads fail after waiting `idle` without a frame.
/// Like reqwest's read timeout, only time spent waiting inside a poll counts:
/// a slow consumer does not trigger it.
pub(crate) struct IdleBody {
    inner: reqwest::Body,
    idle: Duration,
    sleep: Pin<Box<Sleep>>,
    armed: bool,
}

impl IdleBody {
    pub(crate) fn new(inner: reqwest::Body, idle: Duration) -> Self {
        Self {
            inner,
            idle,
            sleep: Box::pin(tokio::time::sleep(idle)),
            armed: false,
        }
    }
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl http_body::Body for IdleBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        if let Poll::Ready(frame) = Pin::new(&mut this.inner).poll_frame(cx) {
            this.armed = false;
            return Poll::Ready(frame.map(|frame| frame.map_err(Into::into)));
        }
        if !this.armed {
            // `read_timeout` is validated to be representable from now.
            this.sleep.as_mut().reset(Instant::now() + this.idle);
            this.armed = true;
        }
        ready!(this.sleep.as_mut().poll(cx));
        Poll::Ready(Some(Err(Box::new(io::Error::new(
            io::ErrorKind::TimedOut,
            "response body read idle timeout elapsed",
        )))))
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test(start_paused = true)]
    async fn tracked_body_splits_chunks_and_resets_the_clock() {
        let clock = ProgressClock::new();
        let source = futures_util::stream::iter([
            Ok(Bytes::from(vec![1; MAX_PIECE * 2 + 1])),
            Ok(Bytes::new()),
            Ok(Bytes::from_static(b"x")),
        ]);
        let mut body = TrackedBody::new(Box::pin(source), clock.clone());
        let idle = Duration::from_millis(100);
        let mut sizes = Vec::new();
        while let Some(piece) = body.next().await {
            tokio::time::advance(Duration::from_millis(60)).await;
            sizes.push(piece.unwrap().len());
            // Each pull happened 60ms ago at most: not yet stalled.
            let stalled = tokio::time::timeout(Duration::ZERO, clock.stalled(idle)).await;
            assert!(stalled.is_err());
        }
        assert_eq!(sizes, [MAX_PIECE, MAX_PIECE, 1, 1]);
        tokio::time::advance(Duration::from_millis(40)).await;
        tokio::time::timeout(Duration::ZERO, clock.stalled(idle))
            .await
            .expect("stalled once idle elapsed since the last pull");
    }
}
