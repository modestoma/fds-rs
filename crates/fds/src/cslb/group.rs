//! Thread-safe node set.

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

use super::Node;

/// A thread-safe collection of nodes keyed by `node.to_string()`.
pub struct Group {
    m: DashMap<String, Arc<dyn Node>>,
    original_count: AtomicI64,
    current_count: AtomicI64,
    max_node_count: usize,
}

impl Group {
    pub fn new(max_node_count: usize) -> Self {
        Self {
            m: DashMap::new(),
            original_count: AtomicI64::new(0),
            current_count: AtomicI64::new(0),
            max_node_count,
        }
    }

    /// Replace the set: insert the new nodes and drop keys no longer present.
    pub fn set(&self, nodes: Vec<Arc<dyn Node>>) {
        let mut keep: HashSet<String> = HashSet::with_capacity(nodes.len());
        for node in &nodes {
            let key = node.to_string();
            keep.insert(key.clone());
            self.m.insert(key, node.clone());
        }
        self.m.retain(|k, _| keep.contains(k));
        self.original_count
            .store(nodes.len() as i64, Ordering::SeqCst);
        self.current_count
            .store(nodes.len() as i64, Ordering::SeqCst);
    }

    /// Up to `max_node_count` nodes.
    pub fn get(&self) -> Vec<Arc<dyn Node>> {
        let mut result = Vec::new();
        for entry in self.m.iter() {
            result.push(entry.value().clone());
            if result.len() >= self.max_node_count {
                break;
            }
        }
        result
    }

    /// Remove a node; decrement `current_count` if it was present.
    pub fn exile(&self, node: &dyn Node) -> bool {
        let removed = self.m.remove(&node.to_string()).is_some();
        if removed {
            self.current_count.fetch_sub(1, Ordering::SeqCst);
        }
        removed
    }

    pub fn original_count(&self) -> i64 {
        self.original_count.load(Ordering::SeqCst)
    }

    pub fn current_count(&self) -> i64 {
        self.current_count.load(Ordering::SeqCst)
    }
}
