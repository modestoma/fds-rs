//! Client-side load balancing (CSLB) for the [`Client`](crate::Client)'s DNS
//! resolution.
//!
//! Enabled by the `cslb` feature and entirely crate-internal: the only knob is
//! [`ClientBuilder::cslb_max_nodes`](crate::ClientBuilder::cslb_max_nodes).
//! **Wire-format-neutral**: it only affects which IP a connection targets,
//! never the request bytes/signature.
//!
//! The client installs `CslbResolver`, which keeps one `LoadBalancer` per
//! host. A balancer resolves the host's A/AAAA records (`RrDnsService`), keeps
//! up to `max_node_count` distinct IPs (`Group`), rotates the preferred IP
//! (`RoundRobinStrategy`) and re-resolves on a TTL timer. There is no
//! connect-failure feedback, so nodes are never exiled between refreshes.

mod group;
mod resolver;
mod service;
mod strategy;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use group::Group;
pub(crate) use resolver::CslbResolver;
use service::RrDnsService;
use strategy::RoundRobinStrategy;

/// The node limit used when the resolver is given zero.
const NODE_COUNT_UNLIMITED: usize = usize::MAX / 2;
/// FDS's DNS cache TTL: how often each host is re-resolved.
const DNS_CACHE_TTL: Duration = Duration::from_secs(600);

struct LbInner {
    service: RrDnsService,
    strategy: RoundRobinStrategy,
    group: Group,
    refresh_lock: Mutex<()>,
}

impl LbInner {
    /// service.resolve → group.set → strategy.set_nodes. The `Mutex`
    /// serializes concurrent refreshes.
    async fn refresh(&self) {
        let _guard = self.refresh_lock.lock().await;
        self.group.set(self.service.resolve().await);
        self.strategy.set_nodes(self.group.get());
    }
}

/// Round-robin over one host's DNS records, refreshed on a TTL timer.
struct LoadBalancer {
    inner: Arc<LbInner>,
}

impl LoadBalancer {
    /// Build a balancer and perform the first refresh. A background task
    /// refreshes every `ttl` and stops when the balancer is dropped.
    async fn new(service: RrDnsService, max_node_count: usize, ttl: Duration) -> Self {
        let inner = Arc::new(LbInner {
            service,
            strategy: RoundRobinStrategy::new(),
            group: Group::new(max_node_count),
            refresh_lock: Mutex::new(()),
        });

        inner.refresh().await;

        let weak = Arc::downgrade(&inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(ttl).await;
                match weak.upgrade() {
                    Some(inner) => inner.refresh().await,
                    None => break, // balancer dropped
                }
            }
        });

        Self { inner }
    }

    /// Pick the next node (round-robin); with no nodes, refresh once and retry.
    async fn next(&self) -> Option<IpAddr> {
        let inner = &self.inner;
        if let Some(node) = inner.strategy.next() {
            return Some(node);
        }
        inner.refresh().await;
        inner.strategy.next()
    }

    /// A snapshot of the current nodes.
    fn current_nodes(&self) -> Vec<IpAddr> {
        self.inner.group.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_balancer_rotates_over_resolved_nodes() {
        let service = RrDnsService::new("localhost".into(), true, true);
        let lb = LoadBalancer::new(service, NODE_COUNT_UNLIMITED, DNS_CACHE_TTL).await;
        let nodes = lb.current_nodes();
        assert!(!nodes.is_empty());
        assert!(nodes.iter().all(IpAddr::is_loopback));

        // The strategy fixes the group's order at refresh, so one full cycle
        // visits every node once and then wraps.
        let mut picks = Vec::new();
        for _ in 0..nodes.len() + 1 {
            picks.push(lb.next().await.unwrap());
        }
        assert_eq!(picks[..nodes.len()], nodes[..]);
        assert_eq!(picks[nodes.len()], nodes[0]);
    }
}
