//! The connection manager (#9, #10, #11).
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
//! * **Handshake.** The dialer sends `hello`, the listener replies
//!   `hello_ack` once the manager accepted the connection.
//! * **One connection per pair.** If two exist (both sides dialed at once),
//!   both sides keep the one opened by the lower device id. Two from the same
//!   side means a reconnect: the newer one wins.
//! * **Heartbeats.** A `ping` every [`Timing::heartbeat`]; a connection with
//!   no traffic for [`Timing::dead_after`] is closed and redialed.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pasta_core::config::{KnownPeer, Settings};
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::engine::Outbound;
use pasta_core::protocol::{Ack, Clip, DecodeError, Envelope, Hello, Message, Ping, Pong};
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::discovery::{Candidate, Discovery, DiscoverySnapshot, TailnetState};
use crate::transport::{self, TransportError, WsStream, close};
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
    /// An item from a peer, already validated against the size limit and its hash.
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
}

pub struct NetworkOptions {
    pub device: DeviceInfo,
    pub settings: watch::Receiver<Settings>,
    pub known_peers: BTreeMap<DeviceId, KnownPeer>,
    pub timing: Timing,
}

type Senders = Arc<RwLock<HashMap<DeviceId, mpsc::Sender<Envelope>>>>;

/// Handle to the connection manager. Cheap to clone.
#[derive(Clone)]
pub struct Network {
    me: DeviceId,
    internal: mpsc::UnboundedSender<Internal>,
    senders: Senders,
    refresh: Arc<Notify>,
    peers: watch::Receiver<Vec<PeerSnapshot>>,
    status: watch::Receiver<NetStatus>,
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
        let ctx = Arc::new(Ctx {
            me,
            hello: Hello::from_device(&options.device),
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
            senders: Arc::clone(&senders),
            status_tx,
            peers_tx,
            discovery_task,
        };
        tokio::spawn(actor.run(internal_rx));
        let network = Self { me, internal: internal_tx, senders, refresh, peers: peers_rx, status: status_rx };
        (network, events_rx)
    }

    pub fn peers(&self) -> watch::Receiver<Vec<PeerSnapshot>> {
        self.peers.clone()
    }

    pub fn status(&self) -> watch::Receiver<NetStatus> {
        self.status.clone()
    }

    /// Asks discovery to run now, e.g. after the machine wakes up.
    pub fn refresh(&self) {
        self.refresh.notify_one();
    }

    /// Closes every connection and stops listening.
    pub async fn shutdown(&self) {
        let (done_tx, done_rx) = oneshot::channel();
        if self.internal.send(Internal::Shutdown(done_tx)).is_ok() {
            let _ = done_rx.await;
        }
    }
}

impl Outbound for Network {
    fn broadcast(&self, clip: &Clip) -> usize {
        let envelope = Envelope::new(self.me, Message::Clip(clip.clone()));
        let senders = self.senders.read().unwrap_or_else(std::sync::PoisonError::into_inner);
        senders
            .iter()
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
    hello: Hello,
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

enum Internal {
    Snapshot(DiscoverySnapshot),
    Handshaken(Box<Handshaken>),
    DialFailed { node_id: String, error: String },
    Latency { device_id: DeviceId, conn_id: u64, rtt: Duration },
    Closed { device_id: DeviceId, conn_id: u64, reason: String },
    Shutdown(oneshot::Sender<()>),
}

/// A connection that passed verification and the first half of the handshake.
struct Handshaken {
    ws: WsStream,
    hello: Hello,
    identity: NodeIdentity,
    addr: SocketAddr,
    /// Set for connections this side dialed: the candidate's node id.
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
    node_id: String,
    dialed: Option<String>,
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
    senders: Senders,
    status_tx: watch::Sender<NetStatus>,
    peers_tx: watch::Sender<Vec<PeerSnapshot>>,
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
                    Some(Internal::DialFailed { node_id, error }) => self.on_dial_failed(&node_id, &error),
                    Some(Internal::Latency { device_id, conn_id, rtt }) => self.on_latency(device_id, conn_id, rtt),
                    Some(Internal::Closed { device_id, conn_id, reason }) => self.on_closed(device_id, conn_id, reason),
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
        if self.tailnet != TailnetState::Running {
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
                tracing::info!(node = node_id, %error, "no Pasta found on peer; retrying rarely");
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
        let now = UnixMillis::now();
        tracing::info!(peer = %peer_id, name = %hello.name, os = %hello.os, %addr, outbound = dialed.is_some(), "peer connected");

        self.node_devices.insert(identity.node_id.clone(), peer_id);
        if let Some(node_id) = &dialed {
            self.node_devices.insert(node_id.clone(), peer_id);
        }
        let known = KnownPeer {
            name: hello.name.clone(),
            os: hello.os.clone(),
            node_id: identity.node_id.clone(),
            last_seen: now,
        };
        self.known.insert(peer_id, known.clone());
        self.senders.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(peer_id, tx);
        let _ = self.ctx.events.send(NetEvent::PeerBound { device_id: peer_id, peer: known });
        let _ = self.ctx.events.send(NetEvent::Connected {
            device_id: peer_id,
            name: hello.name.clone(),
            os: hello.os.clone(),
        });

        let link = Link { peer: peer_id, conn_id, send_ack: dialed.is_none() };
        tokio::spawn(connection(Arc::clone(&self.ctx), ws, link, rx, close_rx));
        self.conns.insert(
            peer_id,
            ActivePeer {
                conn_id,
                initiator,
                hello,
                node_id: identity.node_id,
                dialed,
                latency: None,
                last_seen: now,
                close: Some(close_tx),
            },
        );
        self.publish_peers();
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
        tracing::info!(peer = %device_id, %reason, "peer disconnected");
        // Redial soon, not instantly: the peer may be restarting.
        let next_at = Some(Instant::now() + jitter(self.ctx.timing.retry_base));
        for node_id in [Some(&peer.node_id), peer.dialed.as_ref()].into_iter().flatten() {
            let state = self.dials.entry(node_id.clone()).or_default();
            state.failures = 0;
            state.next_at = next_at;
        }
        let _ = self.ctx.events.send(NetEvent::Disconnected { device_id, reason });
        self.publish_peers();
    }

    fn publish_peers(&self) {
        let mut peers: Vec<PeerSnapshot> = self
            .known
            .iter()
            .map(|(device_id, known)| {
                let active = self.conns.get(device_id);
                PeerSnapshot {
                    device_id: *device_id,
                    name: active.map_or(&known.name, |peer| &peer.hello.name).clone(),
                    os: active.map_or(&known.os, |peer| &peer.hello.os).clone(),
                    node_id: known.node_id.clone(),
                    connected: active.is_some(),
                    latency: active.and_then(|peer| peer.latency),
                    last_seen: known.last_seen,
                }
            })
            .collect();
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
        Ok(Ok(Some(Envelope { message: Message::Hello(hello), .. }))) => {
            let handshaken = Handshaken { ws, hello, identity, addr, dialed: None };
            let _ = ctx.internal.send(Internal::Handshaken(Box::new(handshaken)));
        }
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
    transport::send(&mut ws, &Envelope::new(ctx.me, Message::Hello(ctx.hello.clone())))
        .await
        .map_err(|err| err.to_string())?;
    let reply = tokio::time::timeout(ctx.timing.handshake, transport::recv(&mut ws, max))
        .await
        .map_err(|_| "no hello_ack in time".to_owned())?;
    match reply {
        Ok(Some(Envelope { message: Message::HelloAck(hello), .. })) => {
            Ok(Handshaken { ws, hello, identity, addr, dialed: Some(candidate.node_id) })
        }
        Ok(Some(_)) => {
            transport::close(ws, close::PROTOCOL_ERROR, "expected hello_ack").await;
            Err("expected hello_ack".into())
        }
        Ok(None) => Err("closed during the handshake".into()),
        Err(TransportError::ClosedByPeer { code: close::UNSUPPORTED_VERSION, .. }) => {
            let reason = "peer speaks another protocol version".to_owned();
            ctx.reject(addr, reason.clone());
            Err(reason)
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Which connection a task drives.
struct Link {
    peer: DeviceId,
    conn_id: u64,
    /// Inbound connections still owe the `hello_ack`.
    send_ack: bool,
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

    if link.send_ack {
        send!(envelope(Message::HelloAck(ctx.hello.clone())));
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
                    Ok(Some(received)) => match received.message {
                        Message::Clip(clip) => {
                            tracing::debug!(%peer, item = %clip.item_id, bytes = clip.flavors.byte_len(), "received item");
                            send!(envelope(Message::Ack(Ack { msg_id: received.msg_id })));
                            let _ = ctx.events.send(NetEvent::Clip { from: peer, clip });
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
                        Message::Hello(_) | Message::HelloAck(_) => tracing::debug!(%peer, "ignoring repeated hello"),
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
