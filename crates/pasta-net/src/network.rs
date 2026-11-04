//! The connection manager (#9, #10, #11, #20–#22).
//!
//! One actor task owns the peer table. It polls [`Discovery`], keeps a
//! listener on each of this node's tailnet addresses, dials every candidate
//! that isn't connected yet (with exponential backoff and jitter), and runs
//! one task per connection for heartbeats and messages.
//!
//! Rules every connection follows:
//!
//! * **Verified first.** The remote address is identified with the
//!   [`Verifier`] (Tailscale WhoIs) before the handshake; non-tailnet
//!   addresses are refused. A device id that connected from one tailnet node
//!   before is refused from any other node.
//! * **Same chain.** Each `hello` carries a proof that the sender knows the
//!   chain's words. Without a chain this device neither dials nor accepts;
//!   a peer in another chain is refused with [`close::CHAIN_MISMATCH`].
//! * **Signed handshake.** The dialer sends `hello`, the listener replies with
//!   a signed `hello_ack` once the manager accepted the connection, and the
//!   dialer completes it with a signed `auth`. Both sign both nonces, so each
//!   side has proven it holds the key in its `hello`. A peer that is a member
//!   must present the key it was approved with.
//! * **Members only.** Items and member lists travel `encrypted`. Items go to,
//!   and are taken from, approved members only. Other devices in the chain
//!   are listed as pending until a member approves them.
//! * **One connection per pair.** If two exist (both sides dialed at once),
//!   both sides keep the one opened by the lower device id. Two from the same
//!   side means a reconnect: the newer one wins.
//! * **Heartbeats.** A `ping` every [`Timing::heartbeat`]; a connection with
//!   no traffic for [`Timing::dead_after`] is closed and redialed.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pasta_core::chain::MemberList;
use pasta_core::config::{KnownPeer, Settings};
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::engine::Outbound;
use pasta_core::protocol::{Ack, Auth, Clip, DecodeError, Envelope, Hello, Inner, Message, Ping, Pong, Role};
use pasta_crypto::DeviceKey;
use pasta_crypto::membership::Root;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::discovery::{Candidate, Discovery, DiscoverySnapshot, TailnetState};
use crate::transport::{self, TransportError, WsStream, close};
use crate::trust::{ChainMatch, ChainSetup, ChainStatus, PeerTrust, PendingDevice, Trust};
use crate::verify::{NodeIdentity, Verifier};

/// Intervals and limits. The defaults suit a real tailnet; tests shorten them.
#[derive(Debug, Clone)]
pub struct Timing {
    pub poll: Duration,
    pub heartbeat: Duration,
    pub dead_after: Duration,
    pub handshake: Duration,
    pub retry_base: Duration,
    pub retry_max: Duration,
    /// After this many failed dials in a row a candidate is taken to be
    /// without Pasta and only retried every [`Timing::give_up_retry`].
    pub give_up_after: u32,
    pub give_up_retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(3),
            heartbeat: Duration::from_secs(10),
            dead_after: Duration::from_secs(35),
            handshake: Duration::from_secs(5),
            retry_base: Duration::from_secs(1),
            retry_max: Duration::from_secs(60),
            give_up_after: 5,
            give_up_retry: Duration::from_secs(600),
        }
    }
}

/// A row in the Devices list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerSnapshot {
    pub device_id: DeviceId,
    pub name: String,
    pub os: String,
    pub node_id: String,
    pub connected: bool,
    /// Round trip of the last `ping`.
    pub latency: Option<Duration>,
    pub last_seen: UnixMillis,
    pub trust: PeerTrust,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetStatus {
    pub tailnet: TailnetState,
    /// Addresses actually being listened on.
    pub listening: Vec<SocketAddr>,
    /// e.g. "port 47820 is already in use on 100.x.y.z".
    pub listen_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum NetEvent {
    Connected {
        device_id: DeviceId,
        name: String,
        os: String,
    },
    Disconnected {
        device_id: DeviceId,
        reason: String,
    },
    /// An item from an approved member, decrypted and validated against the
    /// size limit and its hash.
    Clip {
        from: DeviceId,
        clip: Clip,
    },
    /// A peer connected; persist this so the node binding survives restarts (#11).
    PeerBound {
        device_id: DeviceId,
        peer: KnownPeer,
    },
    /// A connection was refused during verification or the handshake.
    Rejected {
        addr: SocketAddr,
        reason: String,
    },
    /// The member list changed (approval, removal, or news from a peer); persist it.
    MembersChanged {
        /// The chain's pinned root, id and key.
        root: Option<Root>,
        list: MemberList,
    },
    /// Another member removed this device. The network has already left the chain.
    RemovedFromChain,
}

pub struct NetworkOptions {
    pub device: DeviceInfo,
    pub device_key: Arc<DeviceKey>,
    /// `None` until the device creates or joins a chain; nothing syncs without one.
    pub chain: Option<ChainSetup>,
    pub settings: watch::Receiver<Settings>,
    pub known_peers: BTreeMap<DeviceId, KnownPeer>,
    pub timing: Timing,
}

type Senders = Arc<RwLock<HashMap<DeviceId, mpsc::Sender<Envelope>>>>;

/// Handle to the connection manager. Cheap to clone.
#[derive(Clone)]
pub struct Network {
    trust: Arc<Trust>,
    internal: mpsc::UnboundedSender<Internal>,
    senders: Senders,
    refresh: Arc<Notify>,
    peers: watch::Receiver<Vec<PeerSnapshot>>,
    status: watch::Receiver<NetStatus>,
    chain: watch::Receiver<ChainStatus>,
}

impl Network {
    /// Starts the manager on the current Tokio runtime.
    pub fn start(
        options: NetworkOptions,
        discovery: Arc<dyn Discovery>,
        verifier: Arc<dyn Verifier>,
    ) -> (Self, mpsc::UnboundedReceiver<NetEvent>) {
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let initial_status =
            NetStatus { tailnet: TailnetState::Starting, listening: Vec::new(), listen_errors: Vec::new() };
        let (status_tx, status_rx) = watch::channel(initial_status);
        let (peers_tx, peers_rx) = watch::channel(Vec::new());
        let senders = Senders::default();
        let refresh = Arc::new(Notify::new());
        let me = options.device.id;
        let trust = Arc::new(Trust::new(me, options.device_key, options.chain));
        let (chain_tx, chain_rx) = watch::channel(trust.summary(Vec::new()));
        let ctx = Arc::new(Ctx {
            me,
            device: options.device,
            trust: Arc::clone(&trust),
            settings: options.settings,
            verifier,
            timing: options.timing,
            internal: internal_tx.clone(),
            events: events_tx,
        });
        let discovery_task = tokio::spawn(discovery_loop(Arc::clone(&ctx), discovery, Arc::clone(&refresh)));
        let actor = Actor {
            ctx,
            known: options.known_peers,
            conns: HashMap::new(),
            node_devices: HashMap::new(),
            dials: HashMap::new(),
            candidates: Vec::new(),
            listeners: HashMap::new(),
            tailnet: TailnetState::Starting,
            listen_errors: Vec::new(),
            next_conn_id: 0,
            declined: HashSet::new(),
            told_removed: HashSet::new(),
            list_checked: HashMap::new(),
            senders: Arc::clone(&senders),
            status_tx,
            peers_tx,
            chain_tx,
            refresh: Arc::clone(&refresh),
            discovery_task,
        };
        tokio::spawn(actor.run(internal_rx));
        let network = Self {
            trust,
            internal: internal_tx,
            senders,
            refresh,
            peers: peers_rx,
            status: status_rx,
            chain: chain_rx,
        };
        (network, events_rx)
    }

    pub fn peers(&self) -> watch::Receiver<Vec<PeerSnapshot>> {
        self.peers.clone()
    }

    pub fn status(&self) -> watch::Receiver<NetStatus> {
        self.status.clone()
    }

    pub fn chain(&self) -> watch::Receiver<ChainStatus> {
        self.chain.clone()
    }

    /// Asks discovery to run now, e.g. after the machine wakes up.
    pub fn refresh(&self) {
        self.refresh.notify_one();
    }

    /// Joins (or creates, or with `None` leaves) a chain: every connection is
    /// redone under the new chain.
    pub async fn set_chain(&self, chain: Option<ChainSetup>) {
        self.command(|done| Command::SetChain(chain, done)).await.unwrap_or(Ok(())).unwrap_or(());
    }

    /// Approves a pending device; every member learns about it. `key` is the
    /// fingerprint the user compared ([`pasta_crypto::keys::short`]); the
    /// approval fails if the device's key doesn't match it.
    pub async fn approve(&self, device: DeviceId, key: &str) -> Result<(), String> {
        let key = key.to_owned();
        self.command(|done| Command::Approve(device, key, done)).await.unwrap_or_else(|| Err(stopped()))
    }

    /// Turns a pending device away until restart.
    pub async fn decline(&self, device: DeviceId) -> Result<(), String> {
        self.command(|done| Command::Decline(device, done)).await.unwrap_or_else(|| Err(stopped()))
    }

    /// Removes a member from the chain. It stops receiving anything at once.
    pub async fn remove(&self, device: DeviceId) -> Result<(), String> {
        self.command(|done| Command::Remove(device, done)).await.unwrap_or_else(|| Err(stopped()))
    }

    /// Tells the members this device leaves, then leaves the chain.
    pub async fn leave(&self) -> Result<(), String> {
        self.command(Command::Leave).await.unwrap_or_else(|| Err(stopped()))
    }

    async fn command(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<(), String>>) -> Command,
    ) -> Option<Result<(), String>> {
        let (done_tx, done_rx) = oneshot::channel();
        self.internal.send(Internal::Command(make(done_tx))).ok()?;
        done_rx.await.ok()
    }

    /// Closes every connection and stops listening.
    pub async fn shutdown(&self) {
        let (done_tx, done_rx) = oneshot::channel();
        if self.internal.send(Internal::Shutdown(done_tx)).is_ok() {
            let _ = done_rx.await;
        }
    }
}

fn stopped() -> String {
    "the network has stopped".to_owned()
}

impl Outbound for Network {
    fn broadcast(&self, clip: &Clip) -> usize {
        if !self.trust.am_member() {
            tracing::debug!(item = %clip.item_id, "not an approved member of a chain; item stays here");
            return 0;
        }
        let Some(envelope) = self.trust.seal(&Inner::Clip(clip.clone())) else { return 0 };
        let senders = self.senders.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        senders
            .iter()
            .filter(|(device, _)| self.trust.trusts(**device))
            .filter(|(device, tx)| match tx.try_send(envelope.clone()) {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(peer = %device, item = %clip.item_id, error = %err, "dropped item for a slow peer");
                    false
                }
            })
            .count()
    }
}

/// Shared by the actor and the tasks it spawns.
struct Ctx {
    me: DeviceId,
    device: DeviceInfo,
    trust: Arc<Trust>,
    settings: watch::Receiver<Settings>,
    verifier: Arc<dyn Verifier>,
    timing: Timing,
    internal: mpsc::UnboundedSender<Internal>,
    events: mpsc::UnboundedSender<NetEvent>,
}

impl Ctx {
    fn max_item_bytes(&self) -> usize {
        self.settings.borrow().max_item_bytes
    }

    fn reject(&self, addr: SocketAddr, reason: String) {
        tracing::warn!(%addr, %reason, "refused peer connection");
        let _ = self.events.send(NetEvent::Rejected { addr, reason });
    }
}

enum Command {
    SetChain(Option<ChainSetup>, oneshot::Sender<Result<(), String>>),
    /// The device and the key fingerprint the user compared.
    Approve(DeviceId, String, oneshot::Sender<Result<(), String>>),
    Decline(DeviceId, oneshot::Sender<Result<(), String>>),
    Remove(DeviceId, oneshot::Sender<Result<(), String>>),
    Leave(oneshot::Sender<Result<(), String>>),
}

enum Internal {
    Snapshot(DiscoverySnapshot),
    Handshaken(Box<Handshaken>),
    /// An inbound connection's `auth` checked out.
    Authenticated {
        device_id: DeviceId,
        conn_id: u64,
    },
    Members {
        from: DeviceId,
        list: MemberList,
    },
    DialFailed {
        node_id: String,
        error: String,
    },
    Latency {
        device_id: DeviceId,
        conn_id: u64,
        rtt: Duration,
    },
    Closed {
        device_id: DeviceId,
        conn_id: u64,
        reason: String,
    },
    Command(Command),
    Shutdown(oneshot::Sender<()>),
}

/// A connection that passed verification and the first half of the handshake.
struct Handshaken {
    ws: WsStream,
    /// The peer's `hello` (inbound) or `hello_ack` (dialed).
    hello: Hello,
    identity: NodeIdentity,
    addr: SocketAddr,
    /// Set for connections this side dialed: the candidate's node id. These
    /// are fully authenticated already.
    dialed: Option<String>,
}

impl Handshaken {
    fn initiator(&self, me: DeviceId) -> DeviceId {
        if self.dialed.is_some() { me } else { self.hello.device_id }
    }
}

struct ActivePeer {
    conn_id: u64,
    initiator: DeviceId,
    hello: Hello,
    identity: NodeIdentity,
    dialed: Option<String>,
    /// Inbound connections wait for the peer's `auth`; until then nothing is sent to them.
    authenticated: bool,
    tx: Option<mpsc::Sender<Envelope>>,
    latency: Option<Duration>,
    last_seen: UnixMillis,
    close: Option<oneshot::Sender<(u16, &'static str)>>,
}

#[derive(Default)]
struct DialState {
    failures: u32,
    next_at: Option<Instant>,
    in_flight: bool,
}

struct Listener {
    local: SocketAddr,
    task: JoinHandle<()>,
}

struct Actor {
    ctx: Arc<Ctx>,
    known: BTreeMap<DeviceId, KnownPeer>,
    conns: HashMap<DeviceId, ActivePeer>,
    /// Tailscale node id (verified, or the dialed candidate's) → device seen on it.
    node_devices: HashMap<String, DeviceId>,
    dials: HashMap<String, DialState>,
    candidates: Vec<Candidate>,
    /// Keyed by the requested address (its port may be 0).
    listeners: HashMap<SocketAddr, Listener>,
    tailnet: TailnetState,
    listen_errors: Vec<String>,
    next_conn_id: u64,
    /// Pending devices turned away this run.
    declined: HashSet<DeviceId>,
    /// Removed devices already sent the list that tells them so.
    told_removed: HashSet<DeviceId>,
    /// When a member list from a non-member was last checked, by verified Tailscale node.
    list_checked: HashMap<String, VecDeque<Instant>>,
    senders: Senders,
    status_tx: watch::Sender<NetStatus>,
    peers_tx: watch::Sender<Vec<PeerSnapshot>>,
    chain_tx: watch::Sender<ChainStatus>,
    refresh: Arc<Notify>,
    discovery_task: JoinHandle<()>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Internal>) {
        let mut dial_tick = tokio::time::interval(Duration::from_millis(250));
        dial_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                message = rx.recv() => match message {
                    Some(Internal::Snapshot(snapshot)) => self.on_snapshot(snapshot).await,
                    Some(Internal::Handshaken(handshaken)) => self.on_handshaken(*handshaken),
                    Some(Internal::Authenticated { device_id, conn_id }) => self.on_authenticated(device_id, conn_id),
                    Some(Internal::Members { from, list }) => self.on_members(from, &list),
                    Some(Internal::DialFailed { node_id, error }) => self.on_dial_failed(&node_id, &error),
                    Some(Internal::Latency { device_id, conn_id, rtt }) => self.on_latency(device_id, conn_id, rtt),
                    Some(Internal::Closed { device_id, conn_id, reason }) => self.on_closed(device_id, conn_id, reason),
                    Some(Internal::Command(command)) => self.on_command(command).await,
                    Some(Internal::Shutdown(done)) => {
                        self.shutdown(&mut rx).await;
                        let _ = done.send(());
                        return;
                    }
                    None => return,
                },
                _ = dial_tick.tick() => self.dial_due(),
            }
        }
    }

    async fn on_snapshot(&mut self, snapshot: DiscoverySnapshot) {
        if snapshot.state != self.tailnet {
            tracing::info!(state = ?snapshot.state, "tailnet state changed");
        }
        self.tailnet = snapshot.state;
        self.reconcile_listeners(&snapshot.listen).await;
        // Forget backoff for peers that went away, so they get a fresh start when they come back.
        self.dials.retain(|node_id, _| snapshot.peers.iter().any(|peer| &peer.node_id == node_id));
        self.candidates = snapshot.peers;
        self.publish_status();
        self.dial_due();
    }

    async fn reconcile_listeners(&mut self, desired: &[SocketAddr]) {
        self.listeners.retain(|addr, listener| {
            let keep = desired.contains(addr);
            if !keep {
                tracing::info!(addr = %listener.local, "stopped listening");
                listener.task.abort();
            }
            keep
        });
        let mut errors = Vec::new();
        for addr in desired {
            if self.listeners.contains_key(addr) {
                continue;
            }
            match transport::bind(*addr).await {
                Ok(listener) => {
                    let local = listener.local_addr().unwrap_or(*addr);
                    tracing::info!(addr = %local, "listening for peers");
                    let task = tokio::spawn(accept_loop(Arc::clone(&self.ctx), listener));
                    self.listeners.insert(*addr, Listener { local, task });
                }
                Err(err) => errors.push(err.to_string()),
            }
        }
        if errors != self.listen_errors {
            for error in &errors {
                tracing::error!(%error, "can't listen for peers");
            }
            self.listen_errors = errors;
        }
    }

    fn publish_status(&self) {
        let mut listening: Vec<_> = self.listeners.values().map(|listener| listener.local).collect();
        listening.sort();
        self.status_tx.send_replace(NetStatus {
            tailnet: self.tailnet.clone(),
            listening,
            listen_errors: self.listen_errors.clone(),
        });
    }

    fn is_connected(&self, node_id: &str) -> bool {
        self.node_devices.get(node_id).is_some_and(|device| self.conns.contains_key(device))
    }

    fn dial_due(&mut self) {
        if self.tailnet != TailnetState::Running || !self.ctx.trust.in_chain() {
            return;
        }
        let now = Instant::now();
        let due: Vec<Candidate> = self
            .candidates
            .iter()
            .filter(|candidate| !self.is_connected(&candidate.node_id))
            .filter(|candidate| {
                let state = self.dials.get(&candidate.node_id);
                state.is_none_or(|state| !state.in_flight && state.next_at.is_none_or(|at| at <= now))
            })
            .cloned()
            .collect();
        for candidate in due {
            self.dials.entry(candidate.node_id.clone()).or_default().in_flight = true;
            tracing::debug!(peer = %candidate.name, addr = %candidate.addr, "dialing");
            tokio::spawn(dial(Arc::clone(&self.ctx), candidate));
        }
    }

    fn on_dial_failed(&mut self, node_id: &str, error: &str) {
        let Some(state) = self.dials.get_mut(node_id) else { return };
        let timing = &self.ctx.timing;
        state.in_flight = false;
        state.failures += 1;
        let delay = if state.failures >= timing.give_up_after {
            if state.failures == timing.give_up_after {
                tracing::info!(node = node_id, %error, "no Pasta in this chain found on peer; retrying rarely");
            }
            timing.give_up_retry
        } else {
            tracing::debug!(node = node_id, failures = state.failures, %error, "dial failed");
            let exponent = state.failures.saturating_sub(1).min(16);
            timing.retry_base.saturating_mul(1 << exponent).min(timing.retry_max)
        };
        state.next_at = Some(Instant::now() + jitter(delay));
    }

    fn on_handshaken(&mut self, handshaken: Handshaken) {
        let me = self.ctx.me;
        let peer_id = handshaken.hello.device_id;
        if let Some(node_id) = &handshaken.dialed
            && let Some(state) = self.dials.get_mut(node_id)
        {
            state.in_flight = false;
            state.failures = 0;
            state.next_at = None;
        }
        if peer_id == me {
            return self.refuse(handshaken, close::REJECTED, "connected to itself".into());
        }
        if let Some(known) = self.known.get(&peer_id)
            && known.node_id != handshaken.identity.node_id
        {
            let reason = format!(
                "device {peer_id} connected from node {} but is bound to node {}",
                handshaken.identity.node_id, known.node_id
            );
            return self.refuse(handshaken, close::REJECTED, reason);
        }
        // The chain may have changed while this connection was being set up.
        if self.ctx.trust.chain_match(&handshaken.hello) != ChainMatch::Same {
            return self.refuse(handshaken, close::CHAIN_MISMATCH, format!("device {peer_id} isn't in this chain"));
        }
        if let Some(member) = self.ctx.trust.member(peer_id) {
            if member.device_key != handshaken.hello.device_key {
                let reason = format!("device {peer_id} presented a different key than it was approved with");
                return self.refuse(handshaken, close::NOT_TRUSTED, reason);
            }
            // Empty when the approver couldn't tell (e.g. the creator's own record with Tailscale down).
            if !member.node_id.is_empty() && member.node_id != handshaken.identity.node_id {
                let reason = format!(
                    "device {peer_id} connected from node {} but was approved on node {}",
                    handshaken.identity.node_id, member.node_id
                );
                return self.refuse(handshaken, close::REJECTED, reason);
            }
        } else if self.declined.contains(&peer_id) {
            return self.refuse(handshaken, close::NOT_TRUSTED, format!("device {peer_id} was declined"));
        }
        let initiator = handshaken.initiator(me);
        if let Some(existing) = self.conns.get(&peer_id) {
            // Both sides apply the same rule, so they agree on the survivor.
            let keep_new = existing.initiator == initiator || initiator == me.min(peer_id);
            if !keep_new {
                tracing::debug!(peer = %peer_id, "keeping the existing connection");
                let ws = handshaken.ws;
                tokio::spawn(transport::close(ws, close::DUPLICATE, "duplicate connection"));
                return;
            }
            tracing::debug!(peer = %peer_id, "replacing the existing connection");
            self.close_conn(peer_id, close::DUPLICATE, "duplicate connection");
        }

        let conn_id = self.next_conn_id;
        self.next_conn_id += 1;
        let (tx, rx) = mpsc::channel(256);
        let (close_tx, close_rx) = oneshot::channel();
        let Handshaken { ws, hello, identity, addr, dialed } = handshaken;
        tracing::debug!(peer = %peer_id, %addr, outbound = dialed.is_some(), "handshake accepted");

        self.node_devices.insert(identity.node_id.clone(), peer_id);
        if let Some(node_id) = &dialed {
            self.node_devices.insert(node_id.clone(), peer_id);
        }
        // Inbound connections still owe the signed `hello_ack` and wait for `auth`.
        let ack = dialed.is_none().then(|| {
            let mut ours = self.ctx.trust.hello(&self.ctx.device);
            ours.signature = Some(self.ctx.trust.sign_handshake(Role::Listener, &hello, &ours));
            Box::new(PendingAck { theirs: hello.clone(), ours })
        });
        let authenticated = dialed.is_some();
        let link = Link { peer: peer_id, conn_id, ack };
        tokio::spawn(connection(Arc::clone(&self.ctx), ws, link, rx, close_rx));
        self.conns.insert(
            peer_id,
            ActivePeer {
                conn_id,
                initiator,
                hello,
                identity,
                dialed,
                authenticated: false,
                tx: Some(tx),
                latency: None,
                last_seen: UnixMillis::now(),
                close: Some(close_tx),
            },
        );
        if authenticated {
            self.on_authenticated(peer_id, conn_id);
        }
    }

    /// The peer proved it holds its key: it can now receive messages.
    fn on_authenticated(&mut self, device_id: DeviceId, conn_id: u64) {
        let Some(peer) = self.conns.get_mut(&device_id).filter(|peer| peer.conn_id == conn_id) else { return };
        if peer.authenticated {
            return;
        }
        if !self.ctx.trust.trusts(device_id) {
            let node = peer.identity.node_id.clone();
            let non_members: Vec<&ActivePeer> = self
                .conns
                .iter()
                .filter(|(id, other)| other.authenticated && !self.ctx.trust.trusts(**id))
                .map(|(_, other)| other)
                .collect();
            let from_node = non_members.iter().filter(|other| other.identity.node_id == node).count();
            if non_members.len() >= MAX_NON_MEMBERS || from_node >= MAX_NON_MEMBERS_PER_NODE {
                tracing::warn!(peer = %device_id, "refused a device that isn't a member: too many connected");
                self.close_conn(device_id, close::NOT_TRUSTED, "too many devices waiting");
                return;
            }
        }
        let Some(peer) = self.conns.get_mut(&device_id).filter(|peer| peer.conn_id == conn_id) else { return };
        peer.authenticated = true;
        let now = UnixMillis::now();
        peer.last_seen = now;
        let (name, os) = (peer.hello.name.clone(), peer.hello.os.clone());
        tracing::info!(
            peer = %device_id,
            %name,
            %os,
            outbound = peer.dialed.is_some(),
            trust = ?self.ctx.trust.peer_trust(device_id),
            "peer connected"
        );
        if let Some(tx) = peer.tx.take() {
            self.senders.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(device_id, tx);
        }
        let _ = self.ctx.events.send(NetEvent::Connected { device_id, name, os });
        self.bind_members(Some(device_id));
        // Members compare notes; a removed device learns (once) that it was removed.
        if self.ctx.trust.am_member() {
            let targets = self.list_targets(&[device_id]);
            self.send_members_to(&targets);
        }
        self.publish_peers();
        self.publish_chain();
    }

    /// Persists the node binding of connected members (#11). Devices that aren't
    /// members get none: they can't make this device store anything.
    fn bind_members(&mut self, just_connected: Option<DeviceId>) {
        let now = UnixMillis::now();
        for (device_id, peer) in &self.conns {
            if !peer.authenticated || !self.ctx.trust.trusts(*device_id) {
                continue;
            }
            let bound = self.known.get(device_id).is_some_and(|known| known.node_id == peer.identity.node_id);
            let known = KnownPeer {
                name: peer.hello.name.clone(),
                os: peer.hello.os.clone(),
                node_id: peer.identity.node_id.clone(),
                last_seen: now,
            };
            self.known.insert(*device_id, known.clone());
            if !bound || just_connected == Some(*device_id) {
                let _ = self.ctx.events.send(NetEvent::PeerBound { device_id: *device_id, peer: known });
            }
        }
    }

    /// Which of `candidates` get the member list: members, and removed devices
    /// that haven't been told yet.
    fn list_targets(&mut self, candidates: &[DeviceId]) -> Vec<DeviceId> {
        let mut targets = Vec::new();
        for id in candidates {
            let removed_untold = || self.ctx.trust.was_removed(*id) && !self.told_removed.contains(id);
            if self.ctx.trust.trusts(*id) || removed_untold() {
                if !self.ctx.trust.trusts(*id) {
                    self.told_removed.insert(*id);
                }
                targets.push(*id);
            }
        }
        targets
    }

    fn on_members(&mut self, from: DeviceId, list: &MemberList) {
        // Lists are taken from any device that proved the words: only events that verify
        // back to the pinned root and were signed by an active member count, and new events
        // from devices known to be removed are dropped. Checking a list costs signature
        // checks, so lists from non-members are checked once every few seconds per
        // Tailscale node (not per device id, which the sender picks).
        if !self.ctx.trust.trusts(from) && !self.list_check_allowed(from) {
            tracing::debug!(peer = %from, "ignoring a member list: too soon after the last one");
            return;
        }
        if self.ctx.trust.waiting() {
            return self.on_approval(from, list);
        }
        let was_member = self.ctx.trust.am_member();
        let outcome = self.ctx.trust.merge(list);
        if !outcome.changed {
            return;
        }
        self.members_changed();
        if was_member && !self.ctx.trust.am_member() {
            tracing::warn!(peer = %from, "this device was removed from the chain");
            let _ = self.ctx.events.send(NetEvent::RemovedFromChain);
            self.set_chain(None);
        }
    }

    fn list_check_allowed(&mut self, from: DeviceId) -> bool {
        let Some(node) = self.conns.get(&from).map(|peer| peer.identity.node_id.clone()) else { return false };
        let now = Instant::now();
        if self.list_checked.len() >= 256 {
            self.list_checked
                .retain(|_, checks| checks.back().is_some_and(|at| now.duration_since(*at) < NON_MEMBER_LIST_INTERVAL));
        }
        let checks = self.list_checked.entry(node).or_default();
        while checks.front().is_some_and(|at| now.duration_since(*at) >= NON_MEMBER_LIST_INTERVAL) {
            checks.pop_front();
        }
        if checks.len() >= NON_MEMBER_LISTS_PER_INTERVAL {
            return false;
        }
        checks.push_back(now);
        true
    }

    /// A list that would make this waiting device a member: adopted at once.
    /// Knowing the words is the approval on this side (ADR 0010); the member
    /// that approved this device was the user's choice on that side.
    fn on_approval(&mut self, from: DeviceId, list: &MemberList) {
        let Some(membership) = self.ctx.trust.offer(list) else {
            tracing::debug!(peer = %from, "a member list that doesn't approve this device");
            return;
        };
        let approver = membership.record(self.ctx.me).map(|mine| mine.approved_by);
        if !self.ctx.trust.adopt(membership) {
            return;
        }
        tracing::info!(peer = %from, approver = ?approver, "approved by a member; syncing with the chain");
        self.members_changed();
    }

    /// Persists, tells every member (and removed devices still connected), refreshes the UI.
    fn members_changed(&mut self) {
        if let Some((root, list)) = self.ctx.trust.members_list() {
            let _ = self.ctx.events.send(NetEvent::MembersChanged { root, list });
        }
        self.bind_members(None);
        if self.ctx.trust.am_member() {
            let connected: Vec<DeviceId> =
                self.conns.iter().filter(|(_, peer)| peer.authenticated).map(|(id, _)| *id).collect();
            let targets = self.list_targets(&connected);
            self.send_members_to(&targets);
        }
        self.publish_peers();
        self.publish_chain();
    }

    fn send_members_to(&self, targets: &[DeviceId]) {
        let Some((_, list)) = self.ctx.trust.members_list() else { return };
        let Some(envelope) = self.ctx.trust.seal(&Inner::Members(list)) else { return };
        let senders = self.senders.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        for device in targets {
            if let Some(tx) = senders.get(device)
                && let Err(err) = tx.try_send(envelope.clone())
            {
                tracing::warn!(peer = %device, error = %err, "couldn't queue the member list");
            }
        }
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::SetChain(chain, done) => {
                self.set_chain(chain);
                let _ = done.send(Ok(()));
            }
            Command::Approve(device, key, done) => {
                let result = self.approve(device, &key);
                let _ = done.send(result);
            }
            Command::Decline(device, done) => {
                self.declined.insert(device);
                if self.conns.get(&device).is_some_and(|_| !self.ctx.trust.trusts(device)) {
                    self.close_conn(device, close::NOT_TRUSTED, "declined");
                }
                tracing::info!(peer = %device, "declined a device");
                self.publish_chain();
                let _ = done.send(Ok(()));
            }
            Command::Remove(device, done) => {
                let result = self.ctx.trust.revoke(device).map_err(|err| err.to_string());
                if result.is_ok() {
                    tracing::info!(peer = %device, "removed a device from the chain");
                    self.members_changed();
                }
                let _ = done.send(result);
            }
            Command::Leave(done) => {
                let me = self.ctx.me;
                // Decided before revoking: afterwards this device trusts nobody.
                let members: Vec<DeviceId> =
                    self.senders_ids().into_iter().filter(|id| self.ctx.trust.trusts(*id)).collect();
                let result = match self.ctx.trust.revoke(me) {
                    Ok(()) => {
                        // Tell the members, give the messages a moment to go out, then leave.
                        self.send_members_to(&members);
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        Ok(())
                    }
                    // Not approved yet: nothing to tell anyone.
                    Err(pasta_crypto::membership::MembershipError::NotAMember) => Ok(()),
                    Err(err) => Err(err.to_string()),
                };
                if result.is_ok() {
                    tracing::info!("left the chain");
                    self.set_chain(None);
                }
                let _ = done.send(result);
            }
        }
    }

    fn senders_ids(&self) -> Vec<DeviceId> {
        self.senders.read().unwrap_or_else(std::sync::PoisonError::into_inner).keys().copied().collect()
    }

    fn approve(&mut self, device: DeviceId, key: &str) -> Result<(), String> {
        let pending = self.pending();
        let Some(candidate) = pending.iter().find(|pending| pending.device_id == device) else {
            return Err("that device isn't waiting for approval (is it connected?)".into());
        };
        if !key_matches(&candidate.device_key, key) {
            return Err("that device's key changed; compare it again".into());
        }
        self.ctx.trust.approve(candidate.candidate()).map_err(|err| err.to_string())?;
        tracing::info!(peer = %device, "approved a device");
        self.declined.remove(&device);
        self.members_changed();
        Ok(())
    }

    /// Joins, switches or leaves the chain: all connections are redone.
    fn set_chain(&mut self, chain: Option<ChainSetup>) {
        self.ctx.trust.set_chain(chain);
        self.declined.clear();
        self.told_removed.clear();
        let ids: Vec<DeviceId> = self.conns.keys().copied().collect();
        for id in ids {
            self.close_conn(id, close::CHAIN_MISMATCH, "sync chain changed");
        }
        // Fresh start for every candidate under the new chain.
        self.dials.clear();
        self.refresh.notify_one();
        self.publish_peers();
        self.publish_chain();
    }

    /// Connected, authenticated devices in the chain that aren't members.
    fn pending(&self) -> Vec<PendingDevice> {
        if !self.ctx.trust.am_member() {
            return Vec::new();
        }
        let mut pending: Vec<PendingDevice> = self
            .conns
            .iter()
            .filter(|(id, peer)| peer.authenticated && !self.ctx.trust.trusts(**id) && !self.declined.contains(id))
            .map(|(id, peer)| PendingDevice {
                device_id: *id,
                name: peer.hello.name.clone(),
                os: peer.hello.os.clone(),
                device_key: peer.hello.device_key,
                node_id: peer.identity.node_id.clone(),
                node_name: peer.identity.name.clone(),
                was_removed: self.ctx.trust.was_removed(*id),
            })
            .collect();
        pending.sort_by(|a, b| a.name.cmp(&b.name));
        pending
    }

    fn publish_chain(&self) {
        self.chain_tx.send_if_modified(|current| {
            let next = self.ctx.trust.summary(self.pending());
            if *current == next {
                return false;
            }
            *current = next;
            true
        });
    }

    fn refuse(&self, handshaken: Handshaken, code: u16, reason: String) {
        self.ctx.reject(handshaken.addr, reason);
        tokio::spawn(transport::close(handshaken.ws, code, "rejected"));
    }

    fn close_conn(&mut self, device_id: DeviceId, code: u16, reason: &'static str) {
        if let Some(mut peer) = self.conns.remove(&device_id) {
            if let Some(close) = peer.close.take() {
                let _ = close.send((code, reason));
            }
            self.senders.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&device_id);
        }
    }

    fn on_latency(&mut self, device_id: DeviceId, conn_id: u64, rtt: Duration) {
        if let Some(peer) = self.conns.get_mut(&device_id).filter(|peer| peer.conn_id == conn_id) {
            peer.latency = Some(rtt);
            peer.last_seen = UnixMillis::now();
            if let Some(known) = self.known.get_mut(&device_id) {
                known.last_seen = peer.last_seen;
            }
            self.publish_peers();
        }
    }

    fn on_closed(&mut self, device_id: DeviceId, conn_id: u64, reason: String) {
        if self.conns.get(&device_id).is_none_or(|peer| peer.conn_id != conn_id) {
            return; // a replaced duplicate
        }
        let Some(peer) = self.conns.remove(&device_id) else { return };
        self.senders.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&device_id);
        // Redial soon, not instantly: the peer may be restarting.
        let next_at = Some(Instant::now() + jitter(self.ctx.timing.retry_base));
        for node_id in [Some(&peer.identity.node_id), peer.dialed.as_ref()].into_iter().flatten() {
            let state = self.dials.entry(node_id.clone()).or_default();
            state.failures = 0;
            state.next_at = next_at;
        }
        if peer.authenticated {
            tracing::info!(peer = %device_id, %reason, "peer disconnected");
            let _ = self.ctx.events.send(NetEvent::Disconnected { device_id, reason });
        } else {
            tracing::debug!(peer = %device_id, %reason, "connection closed before the handshake finished");
        }
        self.publish_peers();
        self.publish_chain();
    }

    fn publish_peers(&self) {
        let trust = &self.ctx.trust;
        let mut peers: Vec<PeerSnapshot> = self
            .known
            .iter()
            .map(|(device_id, known)| {
                let active = self.conns.get(device_id).filter(|peer| peer.authenticated);
                PeerSnapshot {
                    device_id: *device_id,
                    name: active.map_or(&known.name, |peer| &peer.hello.name).clone(),
                    os: active.map_or(&known.os, |peer| &peer.hello.os).clone(),
                    node_id: known.node_id.clone(),
                    connected: active.is_some(),
                    latency: active.and_then(|peer| peer.latency),
                    last_seen: known.last_seen,
                    trust: trust.peer_trust(*device_id),
                }
            })
            .collect();
        // Connected devices that aren't members are listed while connected, never remembered.
        peers.extend(
            self.conns
                .iter()
                .filter(|(device_id, peer)| peer.authenticated && !self.known.contains_key(*device_id))
                .map(|(device_id, peer)| PeerSnapshot {
                    device_id: *device_id,
                    name: peer.hello.name.clone(),
                    os: peer.hello.os.clone(),
                    node_id: peer.identity.node_id.clone(),
                    connected: true,
                    latency: peer.latency,
                    last_seen: peer.last_seen,
                    trust: trust.peer_trust(*device_id),
                }),
        );
        peers.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.cmp(&b.name)));
        self.peers_tx.send_replace(peers);
    }

    async fn shutdown(&mut self, rx: &mut mpsc::UnboundedReceiver<Internal>) {
        self.discovery_task.abort();
        for listener in self.listeners.values() {
            listener.task.abort();
        }
        let mut open = self.conns.len();
        for peer in self.conns.values_mut() {
            if let Some(close) = peer.close.take() {
                let _ = close.send((1000, "shutting down"));
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while open > 0 {
                match rx.recv().await {
                    Some(Internal::Closed { .. }) => open -= 1,
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await;
        self.senders.write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
        tracing::info!("network stopped");
    }
}

/// How often a member list from a device that isn't a member is checked.
const NON_MEMBER_LIST_INTERVAL: Duration = Duration::from_secs(10);
/// How many lists from non-members one Tailscale node gets checked per interval.
const NON_MEMBER_LISTS_PER_INTERVAL: usize = 4;
/// Most connected devices that aren't members (pending, or in another state).
const MAX_NON_MEMBERS: usize = 32;
/// The same, from one Tailscale node.
const MAX_NON_MEMBERS_PER_NODE: usize = 4;

/// Whether `fingerprint` (as shown to the user) is `key`'s. At least the full
/// short fingerprint must be given, so a ground-out prefix doesn't pass.
fn key_matches(key: &pasta_core::wire::Key32, fingerprint: &str) -> bool {
    let full: String = key.0.iter().map(|b| format!("{b:02x}")).collect();
    let given = fingerprint.trim().to_ascii_lowercase();
    given.len() >= pasta_crypto::keys::short(key).len() && full.starts_with(&given)
}

/// `delay` scaled by a random factor in [0.5, 1.5).
fn jitter(delay: Duration) -> Duration {
    delay.mul_f64(0.5 + fastrand::f64())
}

async fn discovery_loop(ctx: Arc<Ctx>, discovery: Arc<dyn Discovery>, refresh: Arc<Notify>) {
    loop {
        let port = ctx.settings.borrow().port;
        let snapshot = discovery.snapshot(port).await;
        if ctx.internal.send(Internal::Snapshot(snapshot)).is_err() {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(ctx.timing.poll) => {}
            () = refresh.notified() => {}
        }
    }
}

async fn accept_loop(ctx: Arc<Ctx>, listener: tokio::net::TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                tokio::spawn(inbound(Arc::clone(&ctx), stream, addr));
            }
            Err(err) => {
                tracing::warn!(error = %err, "accept failed");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn inbound(ctx: Arc<Ctx>, stream: tokio::net::TcpStream, addr: SocketAddr) {
    let identity = match ctx.verifier.identify(addr.ip()).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return ctx.reject(addr, "not a node on this tailnet".into()),
        Err(err) => return ctx.reject(addr, format!("could not verify the address: {err}")),
    };
    let max = ctx.max_item_bytes();
    let mut ws = match transport::accept(stream, max, ctx.timing.handshake).await {
        Ok(ws) => ws,
        Err(err) => return tracing::debug!(%addr, error = %err, "websocket handshake failed"),
    };
    let first = tokio::time::timeout(ctx.timing.handshake, transport::recv(&mut ws, max)).await;
    match first {
        Ok(Ok(Some(Envelope { message: Message::Hello(hello), .. }))) => match ctx.trust.chain_match(&hello) {
            ChainMatch::Same => {
                let handshaken = Handshaken { ws, hello, identity, addr, dialed: None };
                let _ = ctx.internal.send(Internal::Handshaken(Box::new(handshaken)));
            }
            ChainMatch::NoChainHere => {
                tracing::debug!(%addr, "refused a peer: this device isn't in a sync chain");
                transport::close(ws, close::CHAIN_MISMATCH, "not in a sync chain").await;
            }
            ChainMatch::Different => {
                tracing::debug!(%addr, "refused a peer from another sync chain");
                transport::close(ws, close::CHAIN_MISMATCH, "different sync chain").await;
            }
        },
        Ok(Err(TransportError::Decode(err @ DecodeError::UnsupportedVersion { .. }))) => {
            ctx.reject(addr, err.to_string());
            transport::close(ws, close::UNSUPPORTED_VERSION, "unsupported protocol version").await;
        }
        Ok(Ok(Some(_))) => {
            ctx.reject(addr, "expected hello".into());
            transport::close(ws, close::PROTOCOL_ERROR, "expected hello").await;
        }
        Ok(Ok(None)) => {}
        Ok(Err(err)) => {
            ctx.reject(addr, format!("bad hello: {err}"));
            transport::close(ws, close::PROTOCOL_ERROR, "bad hello").await;
        }
        Err(_) => tracing::debug!(%addr, "no hello in time"),
    }
}

async fn dial(ctx: Arc<Ctx>, candidate: Candidate) {
    let node_id = candidate.node_id.clone();
    let result = dial_inner(&ctx, candidate).await;
    let _ = ctx.internal.send(match result {
        Ok(handshaken) => Internal::Handshaken(Box::new(handshaken)),
        Err(error) => Internal::DialFailed { node_id, error },
    });
}

async fn dial_inner(ctx: &Ctx, candidate: Candidate) -> Result<Handshaken, String> {
    let addr = candidate.addr;
    let identity = match ctx.verifier.identify(addr.ip()).await? {
        Some(identity) => identity,
        None => {
            ctx.reject(addr, "not a node on this tailnet".into());
            return Err("not a node on this tailnet".into());
        }
    };
    let max = ctx.max_item_bytes();
    let mut ws = transport::connect(addr, max, ctx.timing.handshake).await.map_err(|err| err.to_string())?;
    let ours = ctx.trust.hello(&ctx.device);
    transport::send(&mut ws, &Envelope::new(ctx.me, Message::Hello(ours.clone())))
        .await
        .map_err(|err| err.to_string())?;
    let reply = tokio::time::timeout(ctx.timing.handshake, transport::recv(&mut ws, max))
        .await
        .map_err(|_| "no hello_ack in time".to_owned())?;
    let theirs = match reply {
        Ok(Some(Envelope { message: Message::HelloAck(hello), .. })) => hello,
        Ok(Some(_)) => {
            transport::close(ws, close::PROTOCOL_ERROR, "expected hello_ack").await;
            return Err("expected hello_ack".into());
        }
        Ok(None) => return Err("closed during the handshake".into()),
        Err(TransportError::ClosedByPeer { code: close::UNSUPPORTED_VERSION, .. }) => {
            let reason = "peer speaks another protocol version".to_owned();
            ctx.reject(addr, reason.clone());
            return Err(reason);
        }
        // Expected for every Pasta on the tailnet outside this chain: back off quietly.
        Err(TransportError::ClosedByPeer { code: close::CHAIN_MISMATCH, .. }) => {
            return Err("peer isn't in this sync chain".into());
        }
        Err(TransportError::ClosedByPeer { code, reason }) if code == close::NOT_TRUSTED || code == close::REJECTED => {
            let reason = format!("peer refused the connection ({reason})");
            ctx.reject(addr, reason.clone());
            return Err(reason);
        }
        Err(err) => return Err(err.to_string()),
    };
    let signed =
        theirs.signature.as_ref().is_some_and(|sig| Trust::verify_handshake(Role::Listener, &ours, &theirs, sig));
    if !signed {
        transport::close(ws, close::NOT_TRUSTED, "handshake not signed").await;
        let reason = "peer's handshake signature didn't verify".to_owned();
        ctx.reject(addr, reason.clone());
        return Err(reason);
    }
    if ctx.trust.chain_match(&theirs) != ChainMatch::Same {
        transport::close(ws, close::CHAIN_MISMATCH, "different sync chain").await;
        return Err("peer isn't in this sync chain".into());
    }
    let auth = Auth { signature: ctx.trust.sign_handshake(Role::Dialer, &ours, &theirs) };
    transport::send(&mut ws, &Envelope::new(ctx.me, Message::Auth(auth))).await.map_err(|err| err.to_string())?;
    Ok(Handshaken { ws, hello: theirs, identity, addr, dialed: Some(candidate.node_id) })
}

/// The listener's half of the handshake, done by the connection task.
struct PendingAck {
    theirs: Hello,
    ours: Hello,
}

/// Which connection a task drives.
struct Link {
    peer: DeviceId,
    conn_id: u64,
    /// Inbound connections still owe the signed `hello_ack` and wait for `auth`.
    ack: Option<Box<PendingAck>>,
}

async fn connection(
    ctx: Arc<Ctx>,
    ws: WsStream,
    link: Link,
    outgoing: mpsc::Receiver<Envelope>,
    close: oneshot::Receiver<(u16, &'static str)>,
) {
    let reason = drive(&ctx, ws, &link, outgoing, close).await;
    let _ = ctx.internal.send(Internal::Closed { device_id: link.peer, conn_id: link.conn_id, reason });
}

async fn drive(
    ctx: &Ctx,
    ws: WsStream,
    link: &Link,
    mut outgoing: mpsc::Receiver<Envelope>,
    mut close: oneshot::Receiver<(u16, &'static str)>,
) -> String {
    let peer = link.peer;
    let (mut sink, mut stream) = ws.split();
    let send_timeout = ctx.timing.handshake;
    macro_rules! send {
        ($message:expr) => {
            match tokio::time::timeout(send_timeout, sink.send($message)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return format!("send failed: {err}"),
                Err(_) => return "send timed out".to_owned(),
            }
        };
    }
    let envelope = |message| WsMessage::text(Envelope::new(ctx.me, message).encode());

    if let Some(ack) = &link.ack {
        send!(envelope(Message::HelloAck(ack.ours.clone())));
        let deadline = Instant::now() + ctx.timing.handshake;
        loop {
            let frame = tokio::select! {
                biased;
                reason = &mut close => {
                    let (code, reason) = reason.unwrap_or((1000, "stopped"));
                    send!(transport::close_frame(code, reason));
                    return reason.to_owned();
                }
                frame = tokio::time::timeout_at(deadline, stream.next()) => frame,
            };
            let frame = match frame {
                Err(_) => {
                    send!(transport::close_frame(close::NOT_TRUSTED, "no auth"));
                    return "no auth in time".to_owned();
                }
                Ok(None) => return "connection closed during the handshake".to_owned(),
                Ok(Some(Err(err))) => return format!("websocket error: {err}"),
                Ok(Some(Ok(frame))) => frame,
            };
            match transport::decode_frame(frame, ctx.max_item_bytes()) {
                Ok(None) => continue,
                Ok(Some(Envelope { message: Message::Auth(auth), from_device, .. })) if from_device == peer => {
                    if Trust::verify_handshake(Role::Dialer, &ack.theirs, &ack.ours, &auth.signature) {
                        let _ = ctx.internal.send(Internal::Authenticated { device_id: peer, conn_id: link.conn_id });
                        break;
                    }
                    send!(transport::close_frame(close::NOT_TRUSTED, "handshake not signed"));
                    return "peer's auth signature didn't verify".to_owned();
                }
                Ok(Some(_)) => {
                    send!(transport::close_frame(close::PROTOCOL_ERROR, "expected auth"));
                    return "expected auth".to_owned();
                }
                Err(TransportError::ClosedByPeer { code, reason }) => {
                    return format!("closed by peer during the handshake ({code} {reason})");
                }
                Err(err) => {
                    send!(transport::close_frame(close::PROTOCOL_ERROR, "expected auth"));
                    return err.to_string();
                }
            }
        }
    }

    let mut heartbeat = tokio::time::interval(ctx.timing.heartbeat);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_rx = Instant::now();
    let mut pings: VecDeque<(Uuid, Instant)> = VecDeque::new();

    loop {
        tokio::select! {
            // A close request wins over the sender being dropped at the same moment.
            biased;
            reason = &mut close => {
                let (code, reason) = reason.unwrap_or((1000, "stopped"));
                send!(transport::close_frame(code, reason));
                return reason.to_owned();
            }
            frame = stream.next() => {
                let frame = match frame {
                    Some(Ok(frame)) => frame,
                    Some(Err(err)) => return format!("websocket error: {err}"),
                    None => return "connection closed".to_owned(),
                };
                last_rx = Instant::now();
                match transport::decode_frame(frame, ctx.max_item_bytes()) {
                    Ok(None) => {}
                    Ok(Some(received)) if received.from_device != peer => {
                        send!(transport::close_frame(close::PROTOCOL_ERROR, "sender changed"));
                        return "peer changed its device id mid-connection".to_owned();
                    }
                    Ok(Some(received)) => match &received.message {
                        Message::Encrypted(_) => {
                            if let Some(ack) = on_encrypted(ctx, peer, &received) {
                                send!(envelope(Message::Ack(ack)));
                            }
                        }
                        Message::Ping(_) => send!(envelope(Message::Pong(Pong { ping_id: received.msg_id }))),
                        Message::Pong(pong) => {
                            if let Some(index) = pings.iter().position(|(id, _)| *id == pong.ping_id) {
                                let (_, sent) = pings.remove(index).unwrap_or((pong.ping_id, Instant::now()));
                                let rtt = sent.elapsed();
                                let _ = ctx.internal.send(Internal::Latency { device_id: peer, conn_id: link.conn_id, rtt });
                            }
                        }
                        Message::Ack(ack) => tracing::trace!(%peer, msg = %ack.msg_id, "acked"),
                        Message::Hello(_) | Message::HelloAck(_) | Message::Auth(_) => {
                            tracing::debug!(%peer, kind = received.message.kind(), "ignoring a repeated handshake message");
                        }
                    },
                    Err(TransportError::Decode(err)) if err.is_ignorable() => {
                        tracing::debug!(%peer, error = %err, "ignoring message");
                    }
                    Err(TransportError::Decode(err @ (DecodeError::TooLarge { .. } | DecodeError::Invalid(_)))) => {
                        tracing::warn!(%peer, error = %err, "dropped an invalid message");
                    }
                    Err(TransportError::Decode(err @ DecodeError::UnsupportedVersion { .. })) => {
                        send!(transport::close_frame(close::UNSUPPORTED_VERSION, "unsupported protocol version"));
                        return err.to_string();
                    }
                    Err(TransportError::ClosedByPeer { code, reason }) => {
                        return format!("closed by peer ({code} {reason})");
                    }
                    Err(err) => {
                        send!(transport::close_frame(close::PROTOCOL_ERROR, "protocol error"));
                        return err.to_string();
                    }
                }
            }
            message = outgoing.recv() => match message {
                Some(message) => send!(WsMessage::text(message.encode())),
                None => {
                    send!(transport::close_frame(1000, "stopped"));
                    return "stopped".to_owned();
                }
            },
            _ = heartbeat.tick() => {
                if last_rx.elapsed() > ctx.timing.dead_after {
                    let _ = tokio::time::timeout(send_timeout, sink.send(transport::close_frame(close::TIMED_OUT, "timed out"))).await;
                    return "heartbeat timed out".to_owned();
                }
                let ping = Envelope::new(ctx.me, Message::Ping(Ping {}));
                pings.push_back((ping.msg_id, Instant::now()));
                if pings.len() > 4 {
                    pings.pop_front();
                }
                send!(WsMessage::text(ping.encode()));
            }
        }
    }
}

/// Decrypts and routes one `encrypted` message. Returns the ack to send, if any.
fn on_encrypted(ctx: &Ctx, peer: DeviceId, received: &Envelope) -> Option<Ack> {
    let inner = match ctx.trust.open(received) {
        Ok(inner) => inner,
        Err(err) => {
            tracing::warn!(%peer, msg = %received.msg_id, error = %err, "dropped an encrypted message");
            return None;
        }
    };
    if let Err(err) = inner.validate(peer, ctx.max_item_bytes()) {
        tracing::warn!(%peer, msg = %received.msg_id, kind = inner.kind(), error = %err, "dropped an invalid message");
        return None;
    }
    match inner {
        Inner::Clip(clip) => {
            if !ctx.trust.trusts(peer) {
                tracing::debug!(%peer, item = %clip.item_id, "ignoring an item from a device that isn't an approved member");
                return None;
            }
            tracing::debug!(%peer, item = %clip.item_id, bytes = clip.flavors.byte_len(), "received item");
            let _ = ctx.events.send(NetEvent::Clip { from: peer, clip });
        }
        Inner::Members(list) => {
            let _ = ctx.internal.send(Internal::Members { from: peer, list });
        }
    }
    Some(Ack { msg_id: received.msg_id })
}
