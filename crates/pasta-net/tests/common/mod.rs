//! In-process peers on loopback for the integration tests (#19).
//!
//! Each [`Peer`] is a real [`Node`] (engine, network, WebSocket transport)
//! with a fake clipboard, a static discovery list instead of Tailscale, and a
//! verifier that treats loopback as one tailnet node.
//!
//! By default every peer is an approved member of one shared [`TestChain`],
//! whose root is a device that never runs. [`PeerBuilder::chain`] puts a peer
//! in another chain, in none, or makes it a device that knows the words but
//! waits for approval.

#![allow(dead_code)]

pub mod proxy;
pub mod raw;

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use pasta_core::config::{KnownPeer, Settings};
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::testing::FakeClipboard;
use pasta_crypto::membership::{Candidate, Membership};
use pasta_crypto::{ChainKeys, DeviceKey};
use pasta_net::discovery::{Candidate as DiscoveryCandidate, StaticDiscovery};
use pasta_net::network::{NetworkOptions, PeerSnapshot, Timing};
use pasta_net::node::{Node, NodeEvent};
use pasta_net::trust::{ChainSetup, ChainStatus};
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

/// Device keys by device id, so a restarted peer keeps its identity.
static IDENTITIES: LazyLock<Mutex<HashMap<DeviceId, Arc<DeviceKey>>>> = LazyLock::new(Default::default);

pub fn identity(id: DeviceId) -> Arc<DeviceKey> {
    Arc::clone(IDENTITIES.lock().unwrap().entry(id).or_insert_with(|| Arc::new(DeviceKey::generate())))
}

/// A chain whose root (a device that never runs) approves whoever it's told to.
pub struct TestChain {
    pub keys: Arc<ChainKeys>,
    root_id: DeviceId,
    root_key: DeviceKey,
    membership: Mutex<Membership>,
}

impl TestChain {
    pub fn new() -> Arc<Self> {
        let keys = Arc::new(ChainKeys::generate());
        let root_id = DeviceId::random();
        let root_key = DeviceKey::generate();
        let root = Candidate {
            device_id: root_id,
            device_key: root_key.public(),
            name: "root".into(),
            os: "test".into(),
            node_id: String::new(),
        };
        let membership = Membership::create(keys.id(), &root_key, root, UnixMillis::now());
        Arc::new(Self { keys, root_id, root_key, membership: Mutex::new(membership) })
    }

    /// The chain every peer is in unless told otherwise.
    pub fn shared() -> Arc<Self> {
        static SHARED: LazyLock<Arc<TestChain>> = LazyLock::new(TestChain::new);
        Arc::clone(&SHARED)
    }

    /// The root approves `id`.
    pub fn admit(&self, id: DeviceId, name: &str) {
        let mut membership = self.membership.lock().unwrap();
        if membership.is_member(id) {
            return;
        }
        let candidate = Candidate {
            device_id: id,
            device_key: identity(id).public(),
            name: name.into(),
            os: "test".into(),
            node_id: LOOPBACK_NODE.into(),
        };
        membership.approve(&self.root_key, self.root_id, candidate, UnixMillis::now()).unwrap();
    }

    /// The root removes `id`.
    pub fn remove(&self, id: DeviceId) {
        self.membership.lock().unwrap().revoke(&self.root_key, self.root_id, id, UnixMillis::now()).unwrap();
    }

    /// What a member knows: the keys and the member list as of now.
    pub fn setup(&self) -> ChainSetup {
        ChainSetup { keys: Arc::clone(&self.keys), membership: self.membership.lock().unwrap().clone() }
    }

    /// What a device that only typed the words knows.
    pub fn joining(&self) -> ChainSetup {
        ChainSetup { keys: Arc::clone(&self.keys), membership: Membership::joining(self.keys.id()) }
    }
}

/// Which chain a test peer is in.
#[derive(Clone)]
pub enum ChainChoice {
    /// An approved member of this chain.
    Member(Arc<TestChain>),
    /// Knows this chain's words, waits for approval.
    Joining(Arc<TestChain>),
    /// Exactly this setup (e.g. one created by the peer itself).
    Setup(ChainSetup),
    /// Not in a chain.
    None,
}

pub struct Peer {
    pub name: String,
    pub device: DeviceInfo,
    pub key: Arc<DeviceKey>,
    pub chain: ChainChoice,
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
    chain: ChainChoice,
    timing: Timing,
}

impl PeerBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            device_id: DeviceId::random(),
            settings: Settings::default(),
            known: BTreeMap::new(),
            chain: ChainChoice::Member(TestChain::shared()),
            timing: fast_timing(),
        }
    }

    pub fn timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
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

    pub fn chain(mut self, chain: ChainChoice) -> Self {
        self.chain = chain;
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
        let key = identity(device.id);
        let chain = setup_for(&self.chain, device.id, &self.name);
        let clipboard = FakeClipboard::new();
        let discovery = StaticDiscovery::new(vec![SocketAddr::new(LOOPBACK, 0)], Vec::new());
        let verifier = StaticVerifier::new()
            .with(LOOPBACK, NodeIdentity { node_id: LOOPBACK_NODE.into(), name: "loopback".into() });
        let (settings_tx, settings_rx) = watch::channel(self.settings);
        let options = NetworkOptions {
            device: device.clone(),
            device_key: Arc::clone(&key),
            chain,
            settings: settings_rx,
            known_peers: self.known,
            timing: self.timing,
        };
        let (node, events) =
            Node::start(options, Arc::new(clipboard.clone()), Arc::new(discovery.clone()), Arc::new(verifier)).unwrap();
        let mut status = node.network().status();
        let status = within(status.wait_for(|status| !status.listening.is_empty())).await.unwrap().clone();
        Peer {
            name: self.name,
            device,
            key,
            chain: self.chain,
            clipboard,
            discovery,
            settings: settings_tx,
            addr: status.listening[0],
            events,
            node: Some(node),
        }
    }
}

fn setup_for(choice: &ChainChoice, id: DeviceId, name: &str) -> Option<ChainSetup> {
    match choice {
        ChainChoice::Member(chain) => {
            chain.admit(id, name);
            Some(chain.setup())
        }
        ChainChoice::Joining(chain) => Some(chain.joining()),
        ChainChoice::Setup(setup) => Some(setup.clone()),
        ChainChoice::None => None,
    }
}

impl Peer {
    pub async fn start(name: &str) -> Self {
        PeerBuilder::new(name).start().await
    }

    pub fn id(&self) -> DeviceId {
        self.device.id
    }

    pub fn candidate(&self) -> DiscoveryCandidate {
        DiscoveryCandidate {
            node_id: format!("n{}", self.name),
            name: self.name.clone(),
            os: "test".into(),
            addr: self.addr,
        }
    }

    pub fn node(&self) -> &Node {
        self.node.as_ref().expect("node stopped")
    }

    pub fn peers(&self) -> watch::Receiver<Vec<PeerSnapshot>> {
        self.node().network().peers()
    }

    pub fn chain_status(&self) -> watch::Receiver<ChainStatus> {
        self.node().network().chain()
    }

    /// Waits until exactly `n` peers are connected.
    pub async fn wait_connected(&self, n: usize) {
        let mut peers = self.peers();
        within(peers.wait_for(|peers| peers.iter().filter(|peer| peer.connected).count() == n))
            .await
            .unwrap_or_else(|_| panic!("{} never had {n} connected peers", self.name));
    }

    /// Waits until `n` devices wait for approval here.
    pub async fn wait_pending(&self, n: usize) {
        let mut chain = self.chain_status();
        within(chain.wait_for(|status| status.chain.as_ref().is_some_and(|chain| chain.pending.len() == n)))
            .await
            .unwrap_or_else(|_| panic!("{} never had {n} pending devices", self.name));
    }

    /// Waits until this peer is (or isn't) an approved member.
    pub async fn wait_approved(&self, approved: bool) {
        let mut chain = self.chain_status();
        within(chain.wait_for(|status| status.chain.as_ref().is_some_and(|chain| chain.approved) == approved))
            .await
            .unwrap_or_else(|_| panic!("{} never became approved={approved}", self.name));
    }

    /// The fingerprint of this device's key, as the approving user compares it.
    pub fn fingerprint(&self) -> String {
        pasta_crypto::keys::short(&self.key.public())
    }

    /// Waits until the clipboard received `count` writes from peers.
    pub async fn wait_writes(&self, count: usize) -> Vec<String> {
        within(async {
            loop {
                let writes = self.clipboard.writes();
                if writes.len() >= count {
                    return writes.into_iter().map(|mut flavors| std::mem::take(&mut flavors.plain)).collect();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
    }

    /// Waits until this peer is no longer in any sync chain.
    pub async fn wait_left(&self) {
        let mut chain = self.chain_status();
        within(chain.wait_for(|status| status.chain.is_none()))
            .await
            .unwrap_or_else(|_| panic!("{} never left the chain", self.name));
    }

    /// Waits until the member list here has exactly `n` entries (the never-running root counts).
    pub async fn wait_members(&self, n: usize) {
        let mut chain = self.chain_status();
        within(chain.wait_for(|status| status.chain.as_ref().is_some_and(|chain| chain.members.len() == n)))
            .await
            .unwrap_or_else(|_| panic!("{} never had {n} members", self.name));
    }

    /// The devices this peer currently counts as members.
    pub fn member_ids(&self) -> Vec<DeviceId> {
        let status = self.chain_status();
        let status = status.borrow();
        status
            .chain
            .as_ref()
            .map(|chain| chain.members.iter().map(|member| member.device_id).collect())
            .unwrap_or_default()
    }

    /// Devices waiting for approval here.
    pub fn pending_ids(&self) -> Vec<DeviceId> {
        let status = self.chain_status();
        let status = status.borrow();
        status
            .chain
            .as_ref()
            .map(|chain| chain.pending.iter().map(|device| device.device_id).collect())
            .unwrap_or_default()
    }

    /// Waits for the next event matching `matches`, skipping the others.
    pub async fn wait_event(&mut self, matches: impl Fn(&NodeEvent) -> bool) -> NodeEvent {
        within(async {
            loop {
                let event = self.events.recv().await.expect("event channel closed");
                if matches(&event) {
                    return event;
                }
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

/// Points every peer at all the others. Members first learn the current
/// member list of their chain, so peers started earlier know the later ones.
pub async fn mesh(peers: &[&Peer]) {
    sync_members(peers).await;
    for peer in peers {
        let others = peers.iter().filter(|other| other.id() != peer.id()).map(|other| other.candidate()).collect();
        peer.discovery.set_peers(others);
    }
}

/// Members learn the current member list of their chain, without any discovery.
pub async fn sync_members(peers: &[&Peer]) {
    for peer in peers {
        if let ChainChoice::Member(chain) = &peer.chain {
            peer.node().network().set_chain(Some(chain.setup())).await;
        }
    }
}

/// `from` will dial `to` at `via` (a proxy, say) and nobody else dials anything.
pub fn dial_via(from: &Peer, via: SocketAddr, to: &Peer) {
    from.discovery.set_peers(vec![DiscoveryCandidate { addr: via, ..to.candidate() }]);
}

pub async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WAIT, future).await.expect("timed out")
}

/// Lets in-flight messages land.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}
