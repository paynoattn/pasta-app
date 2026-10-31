//! Who is on the other end of a connection (#11).
//!
//! Every connection, inbound or outbound, is identified by its remote
//! address before the handshake counts. [`TailscaleVerifier`] asks Tailscale
//! WhoIs; anything that isn't a node on this tailnet is refused.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::discovery::BoxFuture;
use crate::tailscale::{LazyTailscale, is_tailnet_ip};

/// The tailnet node behind an address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentity {
    /// Tailscale stable node id; survives IP changes.
    pub node_id: String,
    pub name: String,
}

pub trait Verifier: Send + Sync + 'static {
    /// `Ok(None)` when `ip` is not a node this device may talk to.
    fn identify(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<NodeIdentity>, String>>;
}

const POSITIVE_TTL: Duration = Duration::from_secs(300);
const NEGATIVE_TTL: Duration = Duration::from_secs(30);

/// Tailscale WhoIs with a short cache per address.
#[derive(Debug)]
pub struct TailscaleVerifier {
    tailscale: Arc<LazyTailscale>,
    cache: Mutex<HashMap<IpAddr, (Instant, Option<NodeIdentity>)>>,
}

impl TailscaleVerifier {
    pub fn new(tailscale: Arc<LazyTailscale>) -> Self {
        Self { tailscale, cache: Mutex::new(HashMap::new()) }
    }

    fn cached(&self, ip: IpAddr) -> Option<Option<NodeIdentity>> {
        let cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (at, identity) = cache.get(&ip)?;
        let ttl = if identity.is_some() { POSITIVE_TTL } else { NEGATIVE_TTL };
        (at.elapsed() < ttl).then(|| identity.clone())
    }
}

impl Verifier for TailscaleVerifier {
    fn identify(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<NodeIdentity>, String>> {
        Box::pin(async move {
            if !is_tailnet_ip(ip) {
                return Ok(None);
            }
            if let Some(identity) = self.cached(ip) {
                return Ok(identity);
            }
            let identity = self
                .tailscale
                .get()
                .map_err(|err| err.to_string())?
                .whois(ip)
                .await
                .map_err(|err| err.to_string())?
                .map(|whois| NodeIdentity { node_id: whois.stable_id, name: whois.name });
            self.cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(ip, (Instant::now(), identity.clone()));
            Ok(identity)
        })
    }
}

/// Fixed answers, for tests and loopback setups.
#[derive(Debug, Default)]
pub struct StaticVerifier {
    nodes: Mutex<HashMap<IpAddr, NodeIdentity>>,
}

impl StaticVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Treats every connection from `ip` as coming from `identity`.
    pub fn with(self, ip: IpAddr, identity: NodeIdentity) -> Self {
        self.nodes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(ip, identity);
        self
    }
}

impl Verifier for StaticVerifier {
    fn identify(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<NodeIdentity>, String>> {
        let identity = self.nodes.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&ip).cloned();
        Box::pin(async move { Ok(identity) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_tailnet_addresses_are_refused_without_asking_tailscale() {
        // A CLI path that doesn't exist: any call to it would fail with an error.
        let tailscale = LazyTailscale::with(crate::tailscale::Tailscale::with_cli("/nonexistent/tailscale"));
        let verifier = TailscaleVerifier::new(Arc::new(tailscale));
        for ip in ["192.168.1.10", "10.0.0.1", "127.0.0.1", "8.8.8.8"] {
            assert_eq!(verifier.identify(ip.parse().unwrap()).await, Ok(None), "{ip}");
        }
        assert!(verifier.identify("100.64.0.1".parse().unwrap()).await.is_err());
    }
}
