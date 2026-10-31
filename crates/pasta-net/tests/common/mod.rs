//! In-process peers on loopback for the integration tests (#19).
//!
//! Each [`Peer`] is a real [`Node`] (engine, network, WebSocket transport)
//! with a fake clipboard, a static discovery list instead of Tailscale, and a
//! verifier that treats loopback as one tailnet node.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use pasta_core::config::{KnownPeer, Settings};
use pasta_core::device::{DeviceId, DeviceInfo};
use pasta_core::testing::FakeClipboard;
use pasta_net::discovery::{Candidate, StaticDiscovery};
use pasta_net::network::{NetworkOptions, PeerSnapshot, Timing};
use pasta_net::node::{Node, NodeEvent};
use pasta_net::verify::{NodeIdentity, StaticVerifier};
use tokio::sync::{mpsc, watch};

pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
pub const LOOPBACK_NODE: &str = "nLoopbackCNTRL";
const WAIT: Duration = Duration::from_secs(10);

pub fn fast_timing() -> Timing {
    Timing {
        poll: Duration::from_millis(50),
        heartbeat: Duration::from_millis(200),
        dead_after: Duration::from_secs(2),
        handshake: Duration::from_secs(2),
        retry_base: Duration::from_millis(50),
        retry_max: Duration::from_millis(500),
        give_up_after: 1000,
        give_up_retry: Duration::from_secs(1),
    }
}

pub struct Peer {
    pub name: String,
    pub device: DeviceInfo,
    pub clipboard: FakeClipboard,
    pub discovery: StaticDiscovery,
    pub settings: watch::Sender<Settings>,
    pub addr: SocketAddr,
    pub events: mpsc::UnboundedReceiver<NodeEvent>,
    node: Option<Node>,
}

pub struct PeerBuilder {
    name: String,
    device_id: DeviceId,
    settings: Settings,
    known: BTreeMap<DeviceId, KnownPeer>,
}

impl PeerBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            device_id: DeviceId::random(),
            settings: Settings::default(),
            known: BTreeMap::new(),
        }
    }

    pub fn device_id(mut self, id: DeviceId) -> Self {
        self.device_id = id;
        self
    }

    pub fn max_item_bytes(mut self, bytes: usize) -> Self {
        self.settings.max_item_bytes = bytes;
        self
    }

    pub fn known(mut self, id: DeviceId, node_id: &str) -> Self {
        let peer =
            KnownPeer { name: "old".into(), os: "test".into(), node_id: node_id.into(), last_seen: Default::default() };
        self.known.insert(id, peer);
        self
    }

    pub async fn start(self) -> Peer {
        let device = DeviceInfo {
            id: self.device_id,
            name: self.name.clone(),
            os: "test".into(),
            os_version: "1".into(),
            app_version: "0.0.0".into(),
        };
        let clipboard = FakeClipboard::new();
        let discovery = StaticDiscovery::new(vec![SocketAddr::new(LOOPBACK, 0)], Vec::new());
        let verifier = StaticVerifier::new()
            .with(LOOPBACK, NodeIdentity { node_id: LOOPBACK_NODE.into(), name: "loopback".into() });
        let (settings_tx, settings_rx) = watch::channel(self.settings);
        let options = NetworkOptions {
            device: device.clone(),
            settings: settings_rx,
            known_peers: self.known,
            timing: fast_timing(),
        };
        let (node, events) =
            Node::start(options, Arc::new(clipboard.clone()), Arc::new(discovery.clone()), Arc::new(verifier)).unwrap();
        let mut status = node.network().status();
        let status = within(status.wait_for(|status| !status.listening.is_empty())).await.unwrap().clone();
        Peer {
            name: self.name,
            device,
            clipboard,
            discovery,
            settings: settings_tx,
            addr: status.listening[0],
            events,
            node: Some(node),
        }
    }
}

impl Peer {
    pub async fn start(name: &str) -> Self {
        PeerBuilder::new(name).start().await
    }

    pub fn id(&self) -> DeviceId {
        self.device.id
    }

    pub fn candidate(&self) -> Candidate {
        Candidate { node_id: format!("n{}", self.name), name: self.name.clone(), os: "test".into(), addr: self.addr }
    }

    pub fn node(&self) -> &Node {
        self.node.as_ref().expect("node stopped")
    }

    pub fn peers(&self) -> watch::Receiver<Vec<PeerSnapshot>> {
        self.node().network().peers()
    }

    /// Waits until exactly `n` peers are connected.
    pub async fn wait_connected(&self, n: usize) {
        let mut peers = self.peers();
        within(peers.wait_for(|peers| peers.iter().filter(|peer| peer.connected).count() == n))
            .await
            .unwrap_or_else(|_| panic!("{} never had {n} connected peers", self.name));
    }

    /// Waits until the clipboard received `count` writes from peers.
    pub async fn wait_writes(&self, count: usize) -> Vec<String> {
        within(async {
            loop {
                let writes = self.clipboard.writes();
                if writes.len() >= count {
                    return writes.into_iter().map(|flavors| flavors.plain).collect();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    pub async fn stop(&mut self) {
        if let Some(node) = self.node.take() {
            node.shutdown().await;
        }
    }

    /// Drains events received so far.
    pub fn drain_events(&mut self) -> Vec<NodeEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }
}

/// Points every peer at all the others.
pub fn mesh(peers: &[&Peer]) {
    for peer in peers {
        let others = peers.iter().filter(|other| other.id() != peer.id()).map(|other| other.candidate()).collect();
        peer.discovery.set_peers(others);
    }
}

pub async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future).await.expect("timed out")
}

/// Lets in-flight messages land.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}
