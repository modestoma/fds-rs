//! Opt-in retries for explicitly classified, replayable requests.
//!
//! Every operation is classified by its own semantics, never by its HTTP verb
//! ([`ReplaySafety`]): reads may be retried once a [`RetryPolicy`] is set,
//! idempotent writes only when the policy (or the request's [`RetryMode`])
//! also enables them, and every other operation is sent exactly once.
use crate::{Dispatch, Error, ErrorKind, Result};
use http::StatusCode;
use reqwest::header::HeaderMap;
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// Bounded retry policy. Disabled by default.
///
/// `max_retries` excludes the first attempt; each attempt retains the
/// client's request timeout. Success body reads are never restarted. An
/// attempt is repeated only when its failure is transient by the same rule as
/// [`Error::is_retryable`], the operation is replay-safe and its body (if
/// any) is in-memory bytes: stream and file bodies are never replayed.
///
/// Which operations are retried:
///
/// - **Reads** (GET/HEAD of objects, metadata, ACLs, listings, bucket
///   information and configuration, endpoint discovery, the app-scoped
///   bucket listing): retried whenever `max_retries > 0`.
/// - **Idempotent writes**, retried only after
///   [`retry_idempotent_writes`](Self::retry_idempotent_writes) or a
///   per-request [`RetryMode::Enabled`]: `put_object` and `upload_part` with
///   a bytes body, `delete_object`, `set_object_metadata`,
///   `set_lifecycle_config` and `set_access_log`. Repeating one of them
///   leaves the same final state, but a repeat can still be observed:
///   `delete_object` may report `NotFound` when an earlier attempt already
///   deleted the object, a repeated `put_object` may add a version in a
///   versioned bucket, and another writer's change made between two attempts
///   can be overwritten.
/// - **Everything else is never retried**, whatever the policy or the
///   request says: copy, rename, restore, create/complete/abort multipart
///   upload, bulk deletion (`delete_objects`, `delete_prefix`; the listing
///   requests of `delete_prefix` are reads), object and bucket ACL changes
///   (`set_object_acl`, `set_object_public`, `set_bucket_acl`: whether the
///   service merges a repeated `Add` grant or a repeated `Delete` is not
///   documented), `set_lifecycle_rule`, create/delete/migrate bucket, CDN
///   prefetch/refresh and the OAuth token exchange.
///
/// Transient failures are HTTP 429, 502, 503 and 504, timeouts and transport
/// failures before a response arrived. HTTP 500 is deliberately not
/// transient: it is the service's generic failure, frequently deterministic
/// for the request that caused it, so repeating the identical request mostly
/// burns the retry budget and adds load during an incident, while 502/503/504
/// explicitly signal a gateway or availability problem. Callers that want to
/// repeat a 500 can check [`Error::status`] themselves.
///
/// ```
/// # fn main() -> fds::Result<()> {
/// use std::time::Duration;
///
/// let policy = fds::RetryPolicy::new(3, Duration::from_millis(100), Duration::from_secs(2))?
///     .retry_idempotent_writes();
/// assert!(policy.retries_idempotent_writes());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct RetryPolicy {
    max_retries: u32,
    initial_delay: Duration,
    max_delay: Duration,
    idempotent_writes: bool,
}
impl RetryPolicy {
    /// Enable at most 100 retries with nonzero delays, initial <= maximum <= 60s.
    /// These are SDK resource limits, not FDS service limits.
    pub fn new(max_retries: u32, initial_delay: Duration, max_delay: Duration) -> Result<Self> {
        if max_retries > 100
            || initial_delay.is_zero()
            || initial_delay > max_delay
            || max_delay > Duration::from_secs(60)
        {
            return Err(Error::config_invalid(
                "retry limits require retries <= 100 and 0 < initial <= maximum <= 60s",
            ));
        }
        Ok(Self {
            max_retries,
            initial_delay,
            max_delay,
            idempotent_writes: false,
        })
    }

    /// Also retry idempotent writes with a bytes body (see the type-level
    /// list), not only reads. Off by default. Has no effect without retries
    /// (`max_retries == 0`). A request's [`RetryMode`] overrides it.
    pub fn retry_idempotent_writes(mut self) -> Self {
        self.idempotent_writes = true;
        self
    }

    /// Whether [`retry_idempotent_writes`](Self::retry_idempotent_writes) is
    /// set.
    pub fn retries_idempotent_writes(&self) -> bool {
        self.idempotent_writes
    }

    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }
    pub fn initial_delay(&self) -> Duration {
        self.initial_delay
    }
    pub fn max_delay(&self) -> Duration {
        self.max_delay
    }

    pub(crate) fn delay(&self, retry: u32, headers: Option<&HeaderMap>) -> Option<Duration> {
        if retry >= self.max_retries {
            return None;
        }
        let random = jitter_random();
        let cap = self
            .initial_delay
            .saturating_mul(1u32 << retry.min(31))
            .min(self.max_delay);
        // Equal jitter: [cap/2, cap], independently seeded per retry.
        let half = cap.as_nanos() as u64 / 2;
        let delay = Duration::from_nanos(half + random % (cap.as_nanos() as u64 - half + 1));
        let hint = headers
            .and_then(|h| h.get("retry-after"))
            .and_then(|v| v.to_str().ok())
            .and_then(retry_after);
        // Never shorten a server hint to the cap: stop instead of retrying early.
        if hint.is_some_and(|hint| hint > self.max_delay) {
            return None;
        }
        Some(delay.max(hint.unwrap_or_default()))
    }
}
/// A fresh, unpredictable `u64` for retry jitter, without an RNG dependency.
///
/// `RandomState::new()` hands out SipHash keys derived from per-thread keys
/// that std seeds from the OS once and then perturbs for every new state, so
/// hashing a process-wide counter with a fresh state yields a well-mixed value
/// that differs per call, per thread and per process. This is statistically
/// adequate for spreading retries; it is not a cryptographic source.
fn jitter_random() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    RandomState::new().hash_one(SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

fn retry_after(value: &str) -> Option<Duration> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) {
        // Overflow is an excessive hint, never permission for an early retry.
        return Some(Duration::from_secs(value.parse().unwrap_or(u64::MAX)));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}

/// Per-request override of the client's [`RetryPolicy`] for an idempotent
/// write, set with the operation builder's `retry` method. Only the builders
/// of idempotent writes have that method; other writes are never retried.
///
/// The number of retries and their delays always come from the client's
/// policy, so a client without retries (`max_retries == 0`) never retries,
/// whatever the mode. Stream and file bodies are never retried either.
///
/// ```no_run
/// # async fn example(client: &fds::Client) -> fds::Result<()> {
/// use fds::RetryMode;
///
/// client
///     .put_object("bucket", "key", b"same bytes every attempt".to_vec())
///     .retry(RetryMode::Enabled)
///     .send()
///     .await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RetryMode {
    /// Follow [`RetryPolicy::retries_idempotent_writes`] (the default).
    #[default]
    Policy,
    /// Retry this request by the client's policy even when the policy does
    /// not enable idempotent writes.
    Enabled,
    /// Send this request once, even when the policy enables idempotent
    /// writes.
    Disabled,
}

/// Default-deny: classification belongs to the operation, never to its verb.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) enum ReplaySafety {
    /// Sent exactly once: repeating it could change the outcome, or its
    /// semantics are not documented well enough to tell.
    #[default]
    Never,
    /// Retried whenever the policy allows retries.
    ReadOnly,
    /// Repeating it yields the same final state; retried only when the
    /// policy or the request's [`RetryMode`] opts in.
    Idempotent,
}

impl ReplaySafety {
    /// Whether a request of this class may be repeated under `policy` and the
    /// request's `mode`. Body replayability is checked separately.
    pub(crate) fn permits(self, mode: RetryMode, policy: &RetryPolicy) -> bool {
        match self {
            Self::Never => false,
            Self::ReadOnly => true,
            Self::Idempotent => match mode {
                RetryMode::Policy => policy.idempotent_writes,
                RetryMode::Enabled => true,
                RetryMode::Disabled => false,
            },
        }
    }
}

/// The single transient-failure rule, shared by [`Error::is_retryable`] and
/// the request retry loop: HTTP 429, 502, 503 and 504, timeouts, and
/// transport failures before a response arrived (the request may or may not
/// have reached the service). Body failures after a response arrived
/// (`Dispatch::Sent`) and HTTP 500 are not transient; see [`RetryPolicy`].
pub(crate) fn is_transient(
    kind: ErrorKind,
    status: Option<StatusCode>,
    dispatch: Dispatch,
) -> bool {
    match kind {
        ErrorKind::RateLimited | ErrorKind::Timeout => true,
        ErrorKind::ServerError => status.is_some_and(|s| matches!(s.as_u16(), 502..=504)),
        ErrorKind::Transport => dispatch != Dispatch::Sent,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_backoff_and_hints() {
        let policy = RetryPolicy::new(4, Duration::from_secs(2), Duration::from_secs(5)).unwrap();
        for (attempt, low, high) in [(0, 1., 2.), (1, 2., 4.), (2, 2.5, 5.)] {
            for _ in 0..30 {
                let d = policy.delay(attempt, None).unwrap().as_secs_f64();
                assert!(d >= low && d <= high);
            }
        }
        assert!(policy.delay(4, None).is_none());
        let mut h = HeaderMap::new();
        h.insert("retry-after", "4".parse().unwrap());
        assert_eq!(policy.delay(0, Some(&h)), Some(Duration::from_secs(4)));
        for hint in ["6", "9999999999999999999999999999999999"] {
            h.insert("retry-after", hint.parse().unwrap());
            assert!(policy.delay(0, Some(&h)).is_none());
        }
        assert!(retry_after("Wed, 21 Oct 2015 07:28:00 GMT")
            .unwrap()
            .is_zero());
        assert!(retry_after("nonsense").is_none());
        let future = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc2822();
        assert!(retry_after(&future).unwrap() > Duration::from_secs(28));
        assert!(RetryPolicy::default().delay(0, None).is_none());
        assert!(RetryPolicy::new(101, Duration::from_secs(1), Duration::from_secs(2)).is_err());
    }

    #[test]
    fn classification_and_mode_decide_eligibility() {
        let reads =
            RetryPolicy::new(1, Duration::from_millis(1), Duration::from_millis(1)).unwrap();
        let writes = reads.clone().retry_idempotent_writes();
        assert!(!reads.retries_idempotent_writes() && writes.retries_idempotent_writes());
        assert!(!RetryPolicy::default().retries_idempotent_writes());
        for (safety, mode, with_reads, with_writes) in [
            (ReplaySafety::Never, RetryMode::Policy, false, false),
            (ReplaySafety::Never, RetryMode::Enabled, false, false),
            (ReplaySafety::ReadOnly, RetryMode::Policy, true, true),
            (ReplaySafety::Idempotent, RetryMode::Policy, false, true),
            (ReplaySafety::Idempotent, RetryMode::Enabled, true, true),
            (ReplaySafety::Idempotent, RetryMode::Disabled, false, false),
        ] {
            assert_eq!(
                safety.permits(mode, &reads),
                with_reads,
                "{safety:?} {mode:?}"
            );
            assert_eq!(
                safety.permits(mode, &writes),
                with_writes,
                "{safety:?} {mode:?}"
            );
        }
    }

    #[test]
    fn transient_rule() {
        let status = |s| Some(StatusCode::from_u16(s).unwrap());
        for s in [502, 503, 504] {
            assert!(is_transient(
                ErrorKind::ServerError,
                status(s),
                Dispatch::Sent
            ));
        }
        assert!(!is_transient(
            ErrorKind::ServerError,
            status(500),
            Dispatch::Sent
        ));
        assert!(is_transient(
            ErrorKind::RateLimited,
            status(429),
            Dispatch::Sent
        ));
        assert!(is_transient(ErrorKind::Timeout, None, Dispatch::Unknown));
        assert!(is_transient(ErrorKind::Transport, None, Dispatch::NotSent));
        assert!(is_transient(ErrorKind::Transport, None, Dispatch::Unknown));
        assert!(!is_transient(ErrorKind::Transport, None, Dispatch::Sent));
        assert!(!is_transient(
            ErrorKind::NotFound,
            status(404),
            Dispatch::Sent
        ));
        assert!(!is_transient(
            ErrorKind::OutcomeUnknown,
            status(503),
            Dispatch::Sent
        ));
    }
}
