//! reqwest custom DNS resolver backed by per-host [`LoadBalancer`]s: return
//! the round-robin pick plus the remaining nodes as fallbacks; TTL refresh
//! handles staleness.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::sync::Mutex;

use super::{LoadBalancer, RrDnsService, DNS_CACHE_TTL, NODE_COUNT_UNLIMITED};

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
    let service = RrDnsService::new(host.to_string(), inner.ipv4, inner.ipv6);
    let lb = Arc::new(LoadBalancer::new(service, inner.max_node_count, DNS_CACHE_TTL).await);
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
            if let Some(primary) = lb.next().await {
                addrs.push(SocketAddr::new(primary, 0));
            }
            // Fallbacks: the rest of the nodes (reqwest tries in order).
            for node in lb.current_nodes() {
                let sa = SocketAddr::new(node, 0);
                if !addrs.contains(&sa) {
                    addrs.push(sa);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    async fn resolve(resolver: &CslbResolver, host: &str) -> Vec<SocketAddr> {
        let name = Name::from_str(host).unwrap();
        resolver.resolve(name).await.unwrap().collect()
    }

    #[tokio::test]
    async fn returns_distinct_nodes_with_rotating_primary() {
        let resolver = CslbResolver::new(20, true, true);
        let first = resolve(&resolver, "localhost").await;
        assert!(!first.is_empty());
        for (i, addr) in first.iter().enumerate() {
            assert!(addr.ip().is_loopback() && addr.port() == 0, "{addr}");
            assert!(!first[..i].contains(addr), "duplicate {addr}");
        }

        let second = resolve(&resolver, "localhost").await;
        let mut sorted = (first.clone(), second.clone());
        sorted.0.sort();
        sorted.1.sort();
        assert_eq!(sorted.0, sorted.1, "same node set between refreshes");
        if first.len() > 1 {
            assert_ne!(first[0], second[0], "the primary rotates");
        }
    }

    #[tokio::test]
    async fn node_limit_and_family_filter_apply() {
        let limited = resolve(&CslbResolver::new(1, true, true), "localhost").await;
        assert_eq!(limited.len(), 1);

        let ipv4_only = resolve(&CslbResolver::new(0, true, false), "localhost").await;
        assert!(!ipv4_only.is_empty());
        assert!(ipv4_only.iter().all(SocketAddr::is_ipv4), "{ipv4_only:?}");
    }
}
