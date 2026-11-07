//! Opt-in retries for explicitly classified, replayable reads.
use crate::{Error, Result};
use reqwest::header::HeaderMap;
use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// Bounded retry policy. Disabled by default. `max_retries` excludes the first
/// attempt; each attempt retains the client's request timeout. Success body reads
/// are never restarted. Mutating operations are never retried.
#[derive(Debug, Clone, Default)]
pub struct RetryPolicy {
    max_retries: u32,
    initial_delay: Duration,
    max_delay: Duration,
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
        })
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

/// Default-deny: classification belongs to the operation, never to its verb.
#[derive(Clone, Copy, Default)]
pub(crate) enum ReplaySafety {
    #[default]
    Never,
    ReadOnly,
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
}
