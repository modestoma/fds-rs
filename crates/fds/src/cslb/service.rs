//! Node sources. Ports `cslb/service.go`, `service_static.go`, `service_rrdns.go`.

use std::net::IpAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;

use super::Node;

/// A callback fired (in a spawned task) when a node is exiled. Takes the node's
/// string key (adapted from Go's `func(node Node)` — the real services return
/// `None`).
pub type NodeFailedCallback = Arc<dyn Fn(String) + Send + Sync>;

/// A group of interchangeable nodes (Go `Service`). Thread-safe.
#[async_trait]
pub trait Service: Send + Sync {
    /// A snapshot of the currently available nodes.
    async fn nodes(&self) -> Vec<Arc<dyn Node>>;
    /// Update the node set.
    async fn refresh(&self);
    /// Optional callback fired when a node is exiled.
    fn node_failed_callback(&self) -> Option<NodeFailedCallback> {
        None
    }
}

/// A fixed list of nodes (Go `staticService`).
pub struct StaticService {
    static_nodes: Vec<Arc<dyn Node>>,
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl StaticService {
    pub fn new(nodes: Vec<Arc<dyn Node>>) -> Self {
        Self {
            static_nodes: nodes,
            nodes: ArcSwap::from_pointee(Vec::new()),
        }
    }
}

#[async_trait]
impl Service for StaticService {
    async fn nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes.load().as_ref().clone()
    }

    async fn refresh(&self) {
        self.nodes.store(Arc::new(self.static_nodes.clone()));
    }
}

/// Round-robin DNS service (Go `rrDNSService`): resolves hostnames to A/AAAA
/// records each refresh.
pub struct RrDnsService {
    hostnames: Vec<String>,
    ipv4: bool,
    ipv6: bool,
    nodes: ArcSwap<Vec<Arc<dyn Node>>>,
}

impl RrDnsService {
    pub fn new(hostnames: Vec<String>, ipv4: bool, ipv6: bool) -> Self {
        Self {
            hostnames,
            ipv4,
            ipv6,
            nodes: ArcSwap::from_pointee(Vec::new()),
        }
    }
}

#[async_trait]
impl Service for RrDnsService {
    async fn nodes(&self) -> Vec<Arc<dyn Node>> {
        self.nodes.load().as_ref().clone()
    }

    async fn refresh(&self) {
        let mut ips: Vec<Arc<dyn Node>> = Vec::new();
        for host in &self.hostnames {
            // port 0 is a placeholder; we only use the resolved IPs.
            if let Ok(addrs) = tokio::net::lookup_host((host.as_str(), 0)).await {
                for addr in addrs {
                    let ip: IpAddr = addr.ip();
                    let keep = (ip.is_ipv4() && self.ipv4) || (ip.is_ipv6() && self.ipv6);
                    if keep {
                        let node: Arc<dyn Node> = Arc::new(ip);
                        ips.push(node);
                    }
                }
            }
        }
        self.nodes.store(Arc::new(ips));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_service_refresh_exposes_nodes() {
        let svc = StaticService::new(vec![
            Arc::new("1.1.1.1".parse::<IpAddr>().unwrap()) as Arc<dyn Node>,
            Arc::new("2.2.2.2".parse::<IpAddr>().unwrap()) as Arc<dyn Node>,
        ]);
        assert_eq!(svc.nodes().await.len(), 0); // before refresh
        svc.refresh().await;
        assert_eq!(svc.nodes().await.len(), 2);
    }
}
