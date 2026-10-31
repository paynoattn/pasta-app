//! Where peers and this node's listen addresses come from (#10).
//!
//! [`TailscaleDiscovery`] reads `tailscale status`; [`StaticDiscovery`] is a
//! fixed list for tests and loopback setups.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::tailscale::{LazyTailscale, Node, is_tailnet_ip};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// State of the tailnet, as far as Pasta can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetState {
    /// No answer from discovery yet.
    Starting,
    Running,
    /// Tailscale answered but isn't connected (`Stopped`, `NeedsLogin`, …).
    NotRunning(String),
    /// Tailscale couldn't be asked: not installed, service down, timed out.
    Unavailable(String),
}

/// A machine that may run Pasta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Tailscale stable node id.
    pub node_id: String,
    pub name: String,
    pub os: String,
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySnapshot {
    pub state: TailnetState,
    /// Addresses to listen on.
    pub listen: Vec<SocketAddr>,
    pub peers: Vec<Candidate>,
}

pub trait Discovery: Send + Sync + 'static {
    /// Current tailnet state. `port` is the configured Pasta port.
    fn snapshot(&self, port: u16) -> BoxFuture<'_, DiscoverySnapshot>;
}

/// Discovery through the local Tailscale client (ADR 0002).
#[derive(Debug, Clone)]
pub struct TailscaleDiscovery {
    tailscale: Arc<LazyTailscale>,
}

/// OSes with no Pasta client yet (#63); not worth probing.
const SKIPPED_OSES: &[&str] = &["iOS", "android", "tvOS"];

impl TailscaleDiscovery {
    pub fn new(tailscale: Arc<LazyTailscale>) -> Self {
        Self { tailscale }
    }
}

impl Discovery for TailscaleDiscovery {
    fn snapshot(&self, port: u16) -> BoxFuture<'_, DiscoverySnapshot> {
        Box::pin(async move {
            let status = match self.tailscale.get() {
                Ok(tailscale) => tailscale.status().await,
                Err(err) => Err(err),
            };
            match status {
                Ok(status) if status.is_running() => {
                    let listen = status
                        .self_node
                        .iter()
                        .flat_map(|node| node.ips.iter().copied())
                        .filter(|ip| is_tailnet_ip(*ip))
                        .map(|ip| SocketAddr::new(ip, port))
                        .collect();
                    let peers = status
                        .online_peers()
                        .filter(|peer| !SKIPPED_OSES.contains(&peer.os.as_str()))
                        .filter_map(|peer| candidate(peer, port))
                        .collect();
                    DiscoverySnapshot { state: TailnetState::Running, listen, peers }
                }
                Ok(status) => DiscoverySnapshot {
                    state: TailnetState::NotRunning(format!("{:?}", status.backend_state)),
                    listen: Vec::new(),
                    peers: Vec::new(),
                },
                Err(err) => DiscoverySnapshot {
                    state: TailnetState::Unavailable(err.to_string()),
                    listen: Vec::new(),
                    peers: Vec::new(),
                },
            }
        })
    }
}

/// Dials a peer on its IPv4 address when it has one.
fn candidate(peer: &Node, port: u16) -> Option<Candidate> {
    let tailnet = || peer.ips.iter().copied().filter(|ip| is_tailnet_ip(*ip));
    let ip: IpAddr = tailnet().find(IpAddr::is_ipv4).or_else(|| tailnet().next())?;
    Some(Candidate {
        node_id: peer.stable_id.clone(),
        name: peer.name.clone(),
        os: peer.os.clone(),
        addr: SocketAddr::new(ip, port),
    })
}

/// A fixed, editable snapshot. Listen addresses keep their own ports (use port
/// 0 for an ephemeral one); the configured port is ignored.
#[derive(Debug, Clone)]
pub struct StaticDiscovery {
    snapshot: Arc<Mutex<DiscoverySnapshot>>,
}

impl StaticDiscovery {
    pub fn new(listen: Vec<SocketAddr>, peers: Vec<Candidate>) -> Self {
        Self { snapshot: Arc::new(Mutex::new(DiscoverySnapshot { state: TailnetState::Running, listen, peers })) }
    }

    pub fn set_peers(&self, peers: Vec<Candidate>) {
        self.lock().peers = peers;
    }

    pub fn set_state(&self, state: TailnetState) {
        self.lock().state = state;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DiscoverySnapshot> {
        self.snapshot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Discovery for StaticDiscovery {
    fn snapshot(&self, _port: u16) -> BoxFuture<'_, DiscoverySnapshot> {
        let mut snapshot = self.lock().clone();
        if snapshot.state != TailnetState::Running {
            snapshot.listen.clear();
            snapshot.peers.clear();
        }
        Box::pin(async move { snapshot })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(os: &str, ips: &[&str]) -> Node {
        Node {
            stable_id: "nPeer000CNTRL".into(),
            name: "peer".into(),
            dns_name: "peer.tail0000.ts.net.".into(),
            host_name: "peer".into(),
            os: os.into(),
            user_id: 1,
            ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            online: true,
            tags: Vec::new(),
        }
    }

    #[test]
    fn dials_ipv4_first() {
        let peer = node("macOS", &["fd7a:115c:a1e0::2", "100.99.1.2"]);
        assert_eq!(candidate(&peer, 47820).unwrap().addr, "100.99.1.2:47820".parse().unwrap());
        let v6_only = node("linux", &["fd7a:115c:a1e0::2"]);
        assert_eq!(candidate(&v6_only, 1).unwrap().addr, "[fd7a:115c:a1e0::2]:1".parse().unwrap());
        assert!(candidate(&node("linux", &[]), 1).is_none());
    }
}
