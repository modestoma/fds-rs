//! Thread-safe set of distinct nodes, capped at a node limit.

use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::{PoisonError, RwLock};

/// The current node set. Iteration order is the set's (unspecified) hash
/// order, so a capped snapshot is an arbitrary subset of the resolved nodes.
pub(super) struct Group {
    nodes: RwLock<HashSet<IpAddr>>,
    max_node_count: usize,
}

impl Group {
    pub(super) fn new(max_node_count: usize) -> Self {
        Self {
            nodes: RwLock::new(HashSet::new()),
            max_node_count,
        }
    }

    /// Replace the set: insert the new nodes and drop those no longer present.
    pub(super) fn set(&self, nodes: Vec<IpAddr>) {
        let keep: HashSet<IpAddr> = nodes.into_iter().collect();
        let mut set = self.nodes.write().unwrap_or_else(PoisonError::into_inner);
        set.extend(keep.iter().copied());
        set.retain(|node| keep.contains(node));
    }

    /// Up to `max_node_count` nodes.
    pub(super) fn get(&self) -> Vec<IpAddr> {
        let set = self.nodes.read().unwrap_or_else(PoisonError::into_inner);
        set.iter().copied().take(self.max_node_count).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn set_deduplicates_replaces_and_caps() {
        let group = Group::new(2);
        group.set(vec![ip("1.1.1.1"), ip("1.1.1.1"), ip("2.2.2.2")]);
        let mut nodes = group.get();
        nodes.sort();
        assert_eq!(nodes, [ip("1.1.1.1"), ip("2.2.2.2")]);

        group.set(vec![ip("3.3.3.3"), ip("4.4.4.4"), ip("5.5.5.5")]);
        let nodes = group.get();
        assert_eq!(nodes.len(), 2);
        assert!(nodes
            .iter()
            .all(|n| *n != ip("1.1.1.1") && *n != ip("2.2.2.2")));

        group.set(Vec::new());
        assert!(group.get().is_empty());
    }
}
