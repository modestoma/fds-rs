//! Node source: a host's round-robin DNS records.

use std::net::IpAddr;

/// Resolves a hostname to its A/AAAA records on each refresh.
pub(super) struct RrDnsService {
    hostname: String,
    ipv4: bool,
    ipv6: bool,
}

impl RrDnsService {
    pub(super) fn new(hostname: String, ipv4: bool, ipv6: bool) -> Self {
        Self {
            hostname,
            ipv4,
            ipv6,
        }
    }

    /// The host's IPs in the enabled address families; empty if the lookup
    /// fails.
    pub(super) async fn resolve(&self) -> Vec<IpAddr> {
        // Port 0 is a placeholder; only the resolved IPs are used.
        match tokio::net::lookup_host((self.hostname.as_str(), 0)).await {
            Ok(addrs) => addrs
                .map(|addr| addr.ip())
                .filter(|ip| (ip.is_ipv4() && self.ipv4) || (ip.is_ipv6() && self.ipv6))
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}
