//! Node request/failure counters + failure-ratio: lightweight, `DashMap`-based,
//! with in-process snapshots.

use dashmap::DashMap;

use super::{CslbError, Node, NODE_FAILED_ANY, NODE_FAILED_UNLIMITED};

/// Per-node request/failure counters.
pub struct Metrics {
    min_sample_size: usize,
    // key → (total, failed)
    counters: DashMap<String, (u64, u64)>,
}

impl Metrics {
    /// Create metrics, or `None` when a failure ratio is invalid or unlimited
    /// (`ratio < 0.0 || ratio >= 1.0`).
    #[allow(clippy::manual_range_contains)] // mirror Go: `ratio < any || ratio >= unlimited`
    pub fn new(max_node_failed_ratio: f64, min_sample_size: usize) -> Option<Self> {
        if max_node_failed_ratio < NODE_FAILED_ANY || max_node_failed_ratio >= NODE_FAILED_UNLIMITED
        {
            return None;
        }
        Some(Self {
            min_sample_size,
            counters: DashMap::new(),
        })
    }

    pub fn node_inc(&self, node: &dyn Node) {
        self.counters.entry(node.to_string()).or_insert((0, 0)).0 += 1;
    }

    pub fn node_failed_inc(&self, node: &dyn Node) {
        self.counters.entry(node.to_string()).or_insert((0, 0)).1 += 1;
    }

    pub fn reset_node(&self, node: &dyn Node) {
        self.counters.remove(&node.to_string());
    }

    pub fn reset_all_nodes(&self) {
        self.counters.clear();
    }

    /// `failed / total`, clamped to `[0, 1]`; errors if the sample is too small.
    pub fn get_node_failed_ratio(&self, node: &dyn Node) -> Result<f64, CslbError> {
        let (total, failed) = self
            .counters
            .get(&node.to_string())
            .map(|e| *e.value())
            .unwrap_or((0, 0));

        if (total as f64) < self.min_sample_size as f64 {
            return Err(CslbError::SampleNotEnough);
        }
        let ratio = failed as f64 / total as f64;
        if ratio.is_nan() {
            return Err(CslbError::InvalidRatio);
        }
        Ok(ratio.clamp(NODE_FAILED_ANY, NODE_FAILED_UNLIMITED))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn node(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn new_returns_none_for_unlimited_or_invalid() {
        assert!(Metrics::new(1.0, 10).is_none()); // >= unlimited
        assert!(Metrics::new(-0.1, 10).is_none()); // < 0
        assert!(Metrics::new(0.0, 10).is_some()); // FDS uses 0.0 → created
        assert!(Metrics::new(0.5, 10).is_some());
    }

    #[test]
    fn ratio_requires_min_sample() {
        let m = Metrics::new(0.0, 10).unwrap();
        let n = node("1.1.1.1");
        for _ in 0..9 {
            m.node_inc(&n);
        }
        assert!(matches!(
            m.get_node_failed_ratio(&n),
            Err(CslbError::SampleNotEnough)
        ));
        m.node_inc(&n); // total = 10
        m.node_failed_inc(&n); // failed = 1
        assert_eq!(m.get_node_failed_ratio(&n).unwrap(), 0.1);
    }
}
