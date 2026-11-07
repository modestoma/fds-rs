//! Node-selection strategies. Ports `cslb/strategy*.go`.

use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;

use super::{CslbError, Node};

/// Controls how a node is chosen (Go `Strategy`). Thread-safe.
pub trait Strategy: Send + Sync {
    fn set_nodes(&self, nodes: Vec<Arc<dyn Node>>);
    fn next(&self) -> Result<Arc<dyn Node>, CslbError>;
    fn next_for(&self, input: &dyn Any) -> Result<Arc<dyn Node>, CslbError>;
}

/// Round-robin strategy (Go `roundRobinStrategy`).
pub struct RoundRobinStrategy {
    index: AtomicU64,
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl RoundRobinStrategy {
    /// The index starts at `u64::MAX` so the first `next()` wraps to index 0
    /// (matching Go's `atomic.AddUint64(MaxUint64, 1) == 0`).
    pub fn new() -> Self {
        Self {
            index: AtomicU64::new(u64::MAX),
            nodes: ArcSwap::from_pointee(Vec::new()),
        }
    }
}

impl Default for RoundRobinStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl Strategy for RoundRobinStrategy {
    fn set_nodes(&self, nodes: Vec<Arc<dyn Node>>) {
        self.nodes.store(Arc::new(nodes));
    }

    fn next(&self) -> Result<Arc<dyn Node>, CslbError> {
        let nodes = self.nodes.load();
        if nodes.is_empty() {
            return Err(CslbError::EmptyNodeList);
        }
        // fetch_add returns the OLD value; wrapping_add(1) reproduces Go's
        // post-increment semantics (first call → 0).
        let i = self.index.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        Ok(nodes[(i % nodes.len() as u64) as usize].clone())
    }

    fn next_for(&self, _input: &dyn Any) -> Result<Arc<dyn Node>, CslbError> {
        self.next()
    }
}

/// A hash function mapping a request key to a `u64` (Go `HashFunc`).
pub type HashFunc = Arc<dyn Fn(&dyn Any) -> Result<u64, CslbError> + Send + Sync>;

/// Hashed strategy (Go `hashedStrategy`): requires an input via `next_for`.
pub struct HashedStrategy {
    hash_func: HashFunc,
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl HashedStrategy {
    pub fn new(hash_func: HashFunc) -> Self {
        Self {
            hash_func,
            nodes: ArcSwap::from_pointee(Vec::new()),
        }
    }
}

impl Strategy for HashedStrategy {
    fn set_nodes(&self, nodes: Vec<Arc<dyn Node>>) {
        self.nodes.store(Arc::new(nodes));
    }

    fn next(&self) -> Result<Arc<dyn Node>, CslbError> {
        Err(CslbError::HashedNeedsInput)
    }

    fn next_for(&self, input: &dyn Any) -> Result<Arc<dyn Node>, CslbError> {
        let nodes = self.nodes.load();
        if nodes.is_empty() {
            return Err(CslbError::EmptyNodeList);
        }
        let hash = (self.hash_func)(input)?;
        Ok(nodes[(hash % nodes.len() as u64) as usize].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn node(s: &str) -> Arc<dyn Node> {
        Arc::new(s.parse::<IpAddr>().unwrap())
    }

    #[test]
    fn round_robin_starts_at_index_zero() {
        let s = RoundRobinStrategy::new();
        s.set_nodes(vec![node("1.1.1.1"), node("2.2.2.2"), node("3.3.3.3")]);
        assert_eq!(s.next().unwrap().to_string(), "1.1.1.1"); // index 0 first
        assert_eq!(s.next().unwrap().to_string(), "2.2.2.2");
        assert_eq!(s.next().unwrap().to_string(), "3.3.3.3");
        assert_eq!(s.next().unwrap().to_string(), "1.1.1.1"); // wraps
    }

    #[test]
    fn round_robin_empty_errors() {
        let s = RoundRobinStrategy::new();
        assert!(matches!(s.next(), Err(CslbError::EmptyNodeList)));
    }

    #[test]
    fn hashed_requires_input() {
        let hf: HashFunc = Arc::new(|_| Ok(0));
        let s = HashedStrategy::new(hf);
        s.set_nodes(vec![node("1.1.1.1"), node("2.2.2.2")]);
        assert!(matches!(s.next(), Err(CslbError::HashedNeedsInput)));
        // hash 3 % 2 == 1 → second node
        let hf3: HashFunc = Arc::new(|_| Ok(3));
        let s3 = HashedStrategy::new(hf3);
        s3.set_nodes(vec![node("1.1.1.1"), node("2.2.2.2")]);
        assert_eq!(s3.next_for(&()).unwrap().to_string(), "2.2.2.2");
    }
}
