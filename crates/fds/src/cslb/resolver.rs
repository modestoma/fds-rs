//! reqwest custom DNS resolver backed by per-host [`LoadBalancer`]s: return
//! the round-robin pick plus remaining healthy IPs as fallbacks; TTL refresh
//! handles staleness.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::sync::Mutex;

use super::{
    LoadBalancer, LoadBalancerOption, RoundRobinStrategy, RrDnsService, DEFAULT_MIN_SAMPLE_SIZE,
    DNS_CACHE_TTL, NODE_COUNT_UNLIMITED,
};

struct Inner {
    max_node_count: usize,
    ipv4: bool,
    ipv6: bool,
    lbs: Mutex<HashMap<String, Arc<LoadBalancer>>>,
}

/// A reqwest DNS resolver that load-balances across a host's A/AAAA records.
/// Crate-internal so that `reqwest::dns::Resolve` stays out of the public API;
/// the client installs it via `reqwest::ClientBuilder::dns_resolver`.
#[derive(Clone)]
pub(crate) struct CslbResolver {
    inner: Arc<Inner>,
}

impl CslbResolver {
    pub(crate) fn new(max_node_count: usize, ipv4: bool, ipv6: bool) -> Self {
        let max_node_count = if max_node_count == 0 {
            NODE_COUNT_UNLIMITED
        } else {
            max_node_count
        };
        Self {
            inner: Arc::new(Inner {
                max_node_count,
                ipv4,
                ipv6,
                lbs: Mutex::new(HashMap::new()),
            }),
        }
    }
}

async fn get_or_create(inner: &Inner, host: &str) -> Arc<LoadBalancer> {
    let mut map = inner.lbs.lock().await;
    if let Some(lb) = map.get(host) {
        return lb.clone();
    }
    let service = Arc::new(RrDnsService::new(
        vec![host.to_string()],
        inner.ipv4,
        inner.ipv6,
    ));
    let strategy = Arc::new(RoundRobinStrategy::new());
    let option = LoadBalancerOption {
        max_node_count: inner.max_node_count,
        ttl: DNS_CACHE_TTL,
        min_healthy_node_ratio: 0.5,
        max_node_failed_ratio: 0.0,
        min_sample_size: DEFAULT_MIN_SAMPLE_SIZE,
    };
    let lb = Arc::new(LoadBalancer::new(service, strategy, option).await);
    map.insert(host.to_string(), lb.clone());
    lb
}

impl Resolve for CslbResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let inner = self.inner.clone();
        Box::pin(async move {
            let host = name.as_str().to_string();
            let lb = get_or_create(&inner, &host).await;

            let mut addrs: Vec<SocketAddr> = Vec::new();
            // Primary: the round-robin pick (advances the rotation).
            if let Ok(primary) = lb.next().await {
                if let Ok(ip) = primary.to_string().parse::<IpAddr>() {
                    addrs.push(SocketAddr::new(ip, 0));
                }
            }
            // Fallbacks: the rest of the healthy nodes (reqwest tries in order).
            for node in lb.current_nodes() {
                if let Ok(ip) = node.to_string().parse::<IpAddr>() {
                    let sa = SocketAddr::new(ip, 0);
                    if !addrs.contains(&sa) {
                        addrs.push(sa);
                    }
                }
            }

            // Fall back to the system resolver if CSLB has nothing.
            if addrs.is_empty() {
                let sys = tokio::net::lookup_host((host.as_str(), 0))
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
                addrs.extend(sys);
            }

            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}
