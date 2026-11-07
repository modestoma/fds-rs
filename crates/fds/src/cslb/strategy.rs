//! Round-robin node selection.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{PoisonError, RwLock};

/// Round-robin over the node list fixed by the last `set_nodes`. Thread-safe.
pub(super) struct RoundRobinStrategy {
    index: AtomicU64,
    nodes: RwLock<Vec<IpAddr>>,
}

impl RoundRobinStrategy {
    /// The index starts at `u64::MAX` so the first `next()` wraps to index 0.
    pub(super) fn new() -> Self {
        Self {
            index: AtomicU64::new(u64::MAX),
            nodes: RwLock::new(Vec::new()),
        }
    }

    pub(super) fn set_nodes(&self, nodes: Vec<IpAddr>) {
        *self.nodes.write().unwrap_or_else(PoisonError::into_inner) = nodes;
    }

    /// The next node, or `None` when the list is empty.
    pub(super) fn next(&self) -> Option<IpAddr> {
        let nodes = self.nodes.read().unwrap_or_else(PoisonError::into_inner);
        if nodes.is_empty() {
            return None;
        }
        // fetch_add returns the OLD value; wrapping_add(1) makes the first
        // call pick index 0.
        let i = self.index.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        Some(nodes[(i % nodes.len() as u64) as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn round_robin_starts_at_index_zero() {
        let s = RoundRobinStrategy::new();
        s.set_nodes(vec![ip("1.1.1.1"), ip("2.2.2.2"), ip("3.3.3.3")]);
        assert_eq!(s.next(), Some(ip("1.1.1.1"))); // index 0 first
        assert_eq!(s.next(), Some(ip("2.2.2.2")));
        assert_eq!(s.next(), Some(ip("3.3.3.3")));
        assert_eq!(s.next(), Some(ip("1.1.1.1"))); // wraps
    }

    #[test]
    fn round_robin_empty_returns_none() {
        let s = RoundRobinStrategy::new();
        assert_eq!(s.next(), None);
    }
}
