//! Client-side load balancing (CSLB). Ports the `cslb/` package + `fds/cslb.go`.
//! Enabled by the `cslb` feature. **Wire-format-neutral**: it only affects which
//! IP a connection targets, never the request bytes/signature.
//!
//! Integration: a reqwest custom DNS resolver
//! ([`CslbResolver`]) returns the round-robin pick plus the remaining healthy
//! IPs as fallbacks, refreshed on a TTL timer. Precise connect-failure exile
//! (Go's dialer retry) is a possible future enhancement.

mod group;
mod metrics;
mod resolver;
mod service;
mod strategy;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

pub use group::Group;
pub use metrics::Metrics;
pub use resolver::CslbResolver;
pub use service::{NodeFailedCallback, RrDnsService, Service, StaticService};
pub use strategy::{HashFunc, HashedStrategy, RoundRobinStrategy, Strategy};

// ---- Constants (option.go / cslb.go / fds/cslb.go) ------------------------

pub const NODE_COUNT_UNLIMITED: usize = usize::MAX / 2;
pub const DEFAULT_MIN_SAMPLE_SIZE: usize = 10;
/// Never refresh via the TTL timer.
pub const TTL_UNLIMITED: Duration = Duration::MAX;
pub const HEALTHY_NODE_MUST_ALL: f64 = 1.0;
pub const HEALTHY_NODE_ANY: f64 = 0.0;
pub const NODE_FAILED_UNLIMITED: f64 = 1.0;
pub const NODE_FAILED_ANY: f64 = 0.0;
/// FDS's DNS cache TTL (fds/cslb.go).
pub const DNS_CACHE_TTL: Duration = Duration::from_secs(600);

/// A load-balancer node — anything with a stable string representation used as
/// its key (Go `Node: fmt.Stringer`). The blanket impl makes `IpAddr` (etc.) a
/// `Node` directly.
pub trait Node: std::fmt::Display + Send + Sync {}
impl<T: std::fmt::Display + Send + Sync + ?Sized> Node for T {}

/// CSLB errors (the string messages match the Go `errors.New` texts).
#[derive(Debug, thiserror::Error)]
pub enum CslbError {
    #[error("empty node list")]
    EmptyNodeList,
    #[error("hashed strategy needs an input, use NextFor instead")]
    HashedNeedsInput,
    #[error("sample not enough")]
    SampleNotEnough,
    #[error("invalid ratio error")]
    InvalidRatio,
    #[error("{0}")]
    Other(String),
}

/// Load-balancer options (Go `LoadBalancerOption`).
#[derive(Clone, Debug)]
pub struct LoadBalancerOption {
    pub max_node_count: usize,
    pub ttl: Duration,
    pub min_healthy_node_ratio: f64,
    pub max_node_failed_ratio: f64,
    pub min_sample_size: usize,
}

impl Default for LoadBalancerOption {
    fn default() -> Self {
        Self {
            max_node_count: NODE_COUNT_UNLIMITED,
            ttl: TTL_UNLIMITED,
            min_healthy_node_ratio: HEALTHY_NODE_ANY,
            max_node_failed_ratio: NODE_FAILED_UNLIMITED,
            min_sample_size: DEFAULT_MIN_SAMPLE_SIZE,
        }
    }
}

struct LbInner {
    service: Arc<dyn Service>,
    strategy: Arc<dyn Strategy>,
    option: LoadBalancerOption,
    group: Group,
    metrics: Option<Metrics>,
    refresh_lock: Mutex<()>,
}

impl LbInner {
    /// service.refresh → metrics reset → group.set → strategy.set_nodes
    /// (Go `refresh`). A `Mutex` provides the singleflight-like serialization.
    async fn refresh(&self) {
        let _guard = self.refresh_lock.lock().await;
        self.service.refresh().await;
        if let Some(m) = &self.metrics {
            m.reset_all_nodes();
        }
        self.group.set(self.service.nodes().await);
        self.strategy.set_nodes(self.group.get());
    }

    async fn node_failed(&self, node: &dyn Node) {
        let metrics = match &self.metrics {
            Some(m) => m,
            None => return,
        };
        metrics.node_failed_inc(node);
        if let Ok(ratio) = metrics.get_node_failed_ratio(node) {
            if ratio > self.option.max_node_failed_ratio {
                metrics.reset_node(node);
                self.group.exile(node);
                if let Some(cb) = self.service.node_failed_callback() {
                    let key = node.to_string();
                    tokio::spawn(async move { cb(key) });
                }
                let nodes = self.group.get();
                let too_few = (self.group.original_count() as f64
                    * self.option.min_healthy_node_ratio)
                    .round()
                    > self.group.current_count() as f64;
                if nodes.is_empty() || too_few {
                    self.refresh().await;
                } else {
                    self.strategy.set_nodes(nodes);
                }
            }
        }
    }
}

/// Orchestrates a service + strategy with TTL refresh and failure-based exile
/// (Go `LoadBalancer`).
pub struct LoadBalancer {
    inner: Arc<LbInner>,
}

impl LoadBalancer {
    /// Build a load balancer and perform the first (blocking) refresh. If
    /// `ttl != TTL_UNLIMITED`, a background task refreshes on that interval and
    /// stops when the balancer is dropped.
    pub async fn new(
        service: Arc<dyn Service>,
        strategy: Arc<dyn Strategy>,
        option: LoadBalancerOption,
    ) -> Self {
        let group = Group::new(option.max_node_count);
        let metrics = Metrics::new(option.max_node_failed_ratio, option.min_sample_size);
        let ttl = option.ttl;

        let inner = Arc::new(LbInner {
            service,
            strategy,
            group,
            metrics,
            option,
            refresh_lock: Mutex::new(()),
        });

        inner.refresh().await;

        if ttl != TTL_UNLIMITED {
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
        }

        Self { inner }
    }

    /// Pick the next node (round-robin), refreshing once and retrying on error
    /// (Go `Next`).
    pub async fn next(&self) -> Result<Arc<dyn Node>, CslbError> {
        self.pick(|s| s.next()).await
    }

    /// Pick a node for a request key (Go `NextFor`).
    pub async fn next_for(&self, input: &dyn std::any::Any) -> Result<Arc<dyn Node>, CslbError> {
        self.pick(|s| s.next_for(input)).await
    }

    async fn pick<F>(&self, f: F) -> Result<Arc<dyn Node>, CslbError>
    where
        F: Fn(&dyn Strategy) -> Result<Arc<dyn Node>, CslbError>,
    {
        let inner = &self.inner;
        let mut result = f(inner.strategy.as_ref());
        if result.is_err() {
            inner.refresh().await;
            result = f(inner.strategy.as_ref());
        }
        if let (Ok(node), Some(m)) = (&result, &inner.metrics) {
            m.node_inc(node.as_ref());
        }
        result
    }

    /// Report a node failure; may exile it and/or trigger a refresh (Go
    /// `NodeFailed`).
    pub async fn node_failed(&self, node: &dyn Node) {
        self.inner.node_failed(node).await;
    }

    /// A snapshot of currently healthy nodes.
    pub fn current_nodes(&self) -> Vec<Arc<dyn Node>> {
        self.inner.group.get()
    }

    /// Force a refresh now.
    pub async fn refresh(&self) {
        self.inner.refresh().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn node(s: &str) -> Arc<dyn Node> {
        Arc::new(s.parse::<IpAddr>().unwrap())
    }

    #[tokio::test]
    async fn load_balancer_round_robin_over_static_service() {
        let service = Arc::new(StaticService::new(vec![node("1.1.1.1"), node("2.2.2.2")]));
        let strategy = Arc::new(RoundRobinStrategy::new());
        let lb = LoadBalancer::new(service, strategy, LoadBalancerOption::default()).await;
        assert_eq!(lb.current_nodes().len(), 2);

        // The node set passes through a Group (DashMap), so its concrete order
        // is unspecified — but the strategy fixes an order at refresh, so
        // round-robin cycles with period 2 over both distinct nodes.
        let picks: Vec<String> = {
            let mut v = Vec::new();
            for _ in 0..4 {
                v.push(lb.next().await.unwrap().to_string());
            }
            v
        };
        assert_eq!(picks[0], picks[2]);
        assert_eq!(picks[1], picks[3]);
        assert_ne!(picks[0], picks[1]);
        let set: std::collections::HashSet<&str> = picks.iter().map(|s| s.as_str()).collect();
        assert_eq!(set, ["1.1.1.1", "2.2.2.2"].into_iter().collect());
    }
}
