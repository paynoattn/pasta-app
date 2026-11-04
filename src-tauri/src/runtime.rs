//! The running sync service: config, keys, clipboard adapter and [`Node`],
//! plus the loop that keeps the tray, the store and the UI in step with them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{ClipboardAdapter, ClipboardStatus};
use pasta_core::config::Settings;
use pasta_core::device::{DeviceInfo, SyncMode};
use pasta_core::engine::EngineEvent;
use pasta_core::history::{History, HistoryEntry};
use pasta_crypto::vault::{self, Vault};
use pasta_crypto::{ChainKeys, DeviceKey};
use pasta_net::discovery::TailscaleDiscovery;
use pasta_net::network::{NetEvent, NetStatus, Network, NetworkOptions, PeerSnapshot, Timing};
use pasta_net::node::{Node, NodeEvent};
use pasta_net::tailscale::LazyTailscale;
use pasta_net::trust::{ChainStatus, PeerTrust};
use pasta_net::verify::TailscaleVerifier;
use pasta_store::Database;
use pasta_store::secrets::{DeviceSecrets, SecretStore};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::chain::{self, ChainView};
use crate::config::ConfigStore;
use crate::storage::Storage;
use crate::{diagnostics, tray};

/// Event the UI listens to for the Devices list (#10).
pub const PEERS_CHANGED: &str = "peers-changed";
/// Event for the sync chain panel (#20, #22).
pub const CHAIN_CHANGED: &str = "chain-changed";
const HISTORY_ITEMS: usize = 200;
/// A tick that arrives this late means the machine slept.
const WAKE_GAP: Duration = Duration::from_secs(15);

pub struct Runtime {
    pub config: Arc<ConfigStore>,
    pub device: DeviceInfo,
    pub log_dir: PathBuf,
    pub(crate) device_key: Arc<DeviceKey>,
    pub(crate) secret_store: Arc<dyn SecretStore>,
    pub(crate) secrets: tokio::sync::Mutex<DeviceSecrets>,
    pub(crate) db: Arc<Database>,
    pub(crate) chain_keys: Mutex<Option<Arc<ChainKeys>>>,
    pub(crate) tailscale: Arc<LazyTailscale>,
    clipboard: Arc<dyn ClipboardAdapter>,
    clipboard_status: Mutex<ClipboardStatus>,
    pub(crate) network: Network,
    node: tokio::sync::Mutex<Option<Node>>,
    /// macOS has no OS clipboard history, so Pasta keeps its own (#15), sealed in memory (#24).
    history: Mutex<History<vault::Sealed>>,
    vault: Vault,
    tray: Mutex<Option<tray::TrayView>>,
}

/// A row in the Devices list, as the UI sees it. Mirrored in `src/ipc/commands.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub device_id: String,
    pub name: String,
    pub os: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub last_seen: u64,
    /// `member`, `pending` (in the chain, not approved) or `unknown` (this device waits for approval).
    pub trust: &'static str,
}

impl From<&PeerSnapshot> for PeerView {
    fn from(peer: &PeerSnapshot) -> Self {
        Self {
            device_id: peer.device_id.to_string(),
            name: peer.name.clone(),
            os: peer.os.clone(),
            connected: peer.connected,
            latency_ms: peer.latency.map(|latency| u64::try_from(latency.as_millis()).unwrap_or(u64::MAX)),
            last_seen: peer.last_seen.0,
            trust: match peer.trust {
                PeerTrust::Member => "member",
                PeerTrust::Pending => "pending",
                PeerTrust::Unknown => "unknown",
            },
        }
    }
}

impl Runtime {
    pub async fn start(
        app: AppHandle,
        config: Arc<ConfigStore>,
        storage: Storage,
        log_dir: PathBuf,
    ) -> Result<Arc<Self>, String> {
        let clipboard = tokio::task::spawn_blocking(pasta_clipboard::system)
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| format!("clipboard: {err}"))?;
        let tailscale = Arc::new(LazyTailscale::new());
        let current = config.get();
        let device = DeviceInfo {
            id: current.device_id,
            name: device_name(current.device_name.as_deref(), &tailscale).await,
            os: std::env::consts::OS.to_owned(),
            os_version: os_info::get().version().to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
        };
        let Storage { store, secrets, db, .. } = storage;
        let device_key = Arc::new(DeviceKey::from_seed(secrets.device_key.duplicate()));
        let chain = chain::load(&secrets, &db)?;
        let chain_keys = chain.as_ref().map(|setup| Arc::clone(&setup.keys));
        chain::apply_log_key(chain_keys.as_deref());
        tracing::info!(
            device = %device.id,
            key = %pasta_crypto::keys::short(&device_key.public()),
            os = %device.os,
            os_version = %device.os_version,
            chain = ?chain_keys.as_ref().map(|keys| keys.id()),
            "starting sync"
        );

        let (settings_tx, settings_rx) = watch::channel(current.settings());
        let options = NetworkOptions {
            device: device.clone(),
            device_key: Arc::clone(&device_key),
            chain,
            settings: settings_rx,
            known_peers: current.known_peers.clone(),
            timing: Timing::default(),
        };
        let discovery = Arc::new(TailscaleDiscovery::new(Arc::clone(&tailscale)));
        let verifier = Arc::new(TailscaleVerifier::new(Arc::clone(&tailscale)));
        let (node, events) = Node::start(options, Arc::clone(&clipboard), discovery, verifier)
            .map_err(|err| format!("clipboard: {err}"))?;
        let clipboard_status = clipboard.status();
        if clipboard_status.history_enabled == Some(false) {
            tracing::warn!("Windows clipboard history is off; received items won't show in Win+V");
        }
        let runtime = Arc::new(Self {
            config,
            device,
            log_dir,
            device_key,
            secret_store: store,
            secrets: tokio::sync::Mutex::new(secrets),
            db,
            chain_keys: Mutex::new(chain_keys),
            tailscale,
            clipboard,
            clipboard_status: Mutex::new(clipboard_status),
            network: node.network().clone(),
            node: tokio::sync::Mutex::new(Some(node)),
            history: Mutex::new(History::new(HISTORY_ITEMS)),
            vault: Vault::new(),
            tray: Mutex::new(None),
        });
        tokio::spawn(Arc::clone(&runtime).run(app, events, settings_tx));
        Ok(runtime)
    }

    async fn run(
        self: Arc<Self>,
        app: AppHandle,
        mut events: tokio::sync::mpsc::UnboundedReceiver<NodeEvent>,
        settings: watch::Sender<Settings>,
    ) {
        let mut peers = self.network.peers();
        let mut status = self.network.status();
        let mut chain = self.network.chain();
        let mut config = self.config.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        let mut last_tick = tokio::time::Instant::now();
        let mut ticks: u32 = 0;
        self.refresh_tray(&app);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.on_event(event).await,
                    None => return,
                },
                Ok(()) = peers.changed() => {
                    let views: Vec<PeerView> = peers.borrow_and_update().iter().map(PeerView::from).collect();
                    let _ = app.emit(PEERS_CHANGED, views);
                    let _ = app.emit(CHAIN_CHANGED, self.chain_view());
                    self.refresh_tray(&app);
                }
                Ok(()) = chain.changed() => {
                    chain.borrow_and_update();
                    let _ = app.emit(CHAIN_CHANGED, self.chain_view());
                    self.refresh_tray(&app);
                }
                Ok(()) = status.changed() => self.refresh_tray(&app),
                Ok(()) = config.changed() => {
                    let next = config.borrow_and_update().settings();
                    settings.send_if_modified(|current| std::mem::replace(current, next) != next);
                    self.refresh_tray(&app);
                }
                now = tick.tick() => {
                    if now.duration_since(last_tick) > WAKE_GAP {
                        tracing::info!("woke from sleep; looking for peers");
                        self.network.refresh();
                    }
                    last_tick = now;
                    ticks = ticks.wrapping_add(1);
                    if ticks.is_multiple_of(15) {
                        self.refresh_clipboard_status(&app).await;
                    }
                }
            }
        }
    }

    async fn on_event(&self, event: NodeEvent) {
        match event {
            NodeEvent::Net(NetEvent::PeerBound { device_id, peer }) => {
                if let Err(err) = self.config.update(|config| {
                    config.known_peers.insert(device_id, peer);
                }) {
                    tracing::warn!(error = %err, "couldn't save the peer list");
                }
            }
            NodeEvent::Net(NetEvent::MembersChanged { root, list }) => self.save_members(root, list),
            NodeEvent::Net(NetEvent::RemovedFromChain) => {
                if let Err(err) = self.forget_chain().await {
                    tracing::error!(error = %err, "couldn't remove the chain key after this device was removed");
                }
            }
            NodeEvent::Engine(EngineEvent::Applied { clip, .. } | EngineEvent::Sent { clip, .. })
                if cfg!(target_os = "macos") =>
            {
                match self.vault.seal_flavors(&clip.flavors) {
                    Ok(content) => {
                        self.history.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(HistoryEntry {
                            item_id: clip.item_id,
                            origin_device: clip.origin_device,
                            origin_app: clip.origin_app.clone(),
                            at: clip.created_at,
                            content,
                            content_hash: clip.content_hash,
                        });
                    }
                    Err(err) => tracing::warn!(item = %clip.item_id, error = %err, "left an item out of the history"),
                }
            }
            _ => {}
        }
    }

    async fn refresh_clipboard_status(&self, app: &AppHandle) {
        let clipboard = Arc::clone(&self.clipboard);
        if let Ok(status) = tokio::task::spawn_blocking(move || clipboard.status()).await {
            let changed = {
                let mut current = self.clipboard_status.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::replace(&mut *current, status.clone()) != status
            };
            if changed {
                self.refresh_tray(app);
            }
        }
    }

    /// Rebuilds the tray menu if what it shows changed.
    pub fn refresh_tray(&self, app: &AppHandle) {
        let view = self.tray_view();
        let mut shown = self.tray.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if shown.as_ref() != Some(&view) {
            tray::update(app, &view);
            *shown = Some(view);
        }
    }

    fn tray_view(&self) -> tray::TrayView {
        let peers = self.peers();
        let status = self.network.status().borrow().clone();
        let chain = self.chain_status();
        let mode = self.config.get().mode;
        let clipboard = self.clipboard_status();
        let connected = peers.iter().filter(|peer| peer.connected && peer.trust == PeerTrust::Member).count();
        let mut warnings = Vec::new();
        if clipboard.history_enabled == Some(false) {
            warnings.push("Clipboard history (Win+V) is off".to_owned());
        }
        if let Some(reason) = clipboard.unsupported {
            warnings.push(reason);
        }
        warnings.extend(status.listen_errors.iter().cloned());
        if let Some(summary) = &chain.chain {
            warnings.extend(summary.pending.iter().map(|pending| format!("{} wants to join", pending.name)));
        }
        tray::TrayView {
            status: status_line(&status, &chain, connected, mode),
            warnings,
            peers: peers.iter().filter(|peer| peer.trust != PeerTrust::Pending).map(tray::peer_label).collect(),
            paused: mode == SyncMode::Paused,
            retry: false,
        }
    }

    pub fn peers(&self) -> Vec<PeerSnapshot> {
        self.network.peers().borrow().clone()
    }

    pub fn chain_status(&self) -> ChainStatus {
        self.network.chain().borrow().clone()
    }

    pub fn chain_view(&self) -> ChainView {
        ChainView::new(&self.chain_status(), &self.peers(), self.device.id, &self.device_key.public())
    }

    pub fn net_status(&self) -> NetStatus {
        self.network.status().borrow().clone()
    }

    pub fn clipboard_status(&self) -> ClipboardStatus {
        self.clipboard_status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn history_len(&self) -> usize {
        self.history.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
    }

    pub fn toggle_pause(&self) {
        let result = self.config.update(|config| {
            config.mode = if config.mode == SyncMode::Paused { SyncMode::Both } else { SyncMode::Paused };
        });
        if let Err(err) = result {
            tracing::warn!(error = %err, "couldn't save the mode");
        }
    }

    /// Puts a redacted diagnostics report on the clipboard, without syncing it.
    pub async fn copy_diagnostics(&self) {
        let report = diagnostics::report(self);
        if let Some(node) = self.node.lock().await.as_ref() {
            node.write_local(Flavors::plain(report));
            tracing::info!("copied diagnostics to the clipboard");
        }
    }

    /// Stops the watcher, then closes every connection.
    pub async fn shutdown(&self) {
        if let Some(node) = self.node.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), node.shutdown()).await;
        }
    }
}

fn status_line(status: &NetStatus, chain: &ChainStatus, connected: usize, mode: SyncMode) -> String {
    use pasta_net::discovery::TailnetState;
    match &status.tailnet {
        TailnetState::Starting => "Starting…".to_owned(),
        TailnetState::Unavailable(_) => "Tailscale isn't running".to_owned(),
        TailnetState::NotRunning(_) => "Tailscale is disconnected".to_owned(),
        TailnetState::Running => match &chain.chain {
            None => "Not in a sync chain".to_owned(),
            Some(summary) if !summary.approved => "Waiting for approval".to_owned(),
            Some(_) if mode == SyncMode::Paused => "Paused".to_owned(),
            Some(_) if !status.listen_errors.is_empty() => "Can't accept connections".to_owned(),
            Some(_) => match connected {
                0 => "Looking for devices…".to_owned(),
                1 => "Connected · 1 device".to_owned(),
                n => format!("Connected · {n} devices"),
            },
        },
    }
}

/// The configured name, else the Tailscale device name, else the OS hostname.
async fn device_name(configured: Option<&str>, tailscale: &LazyTailscale) -> String {
    if let Some(name) = configured.filter(|name| !name.trim().is_empty()) {
        return name.to_owned();
    }
    if let Ok(tailscale) = tailscale.get()
        && let Ok(Ok(status)) = tokio::time::timeout(Duration::from_secs(5), tailscale.status()).await
        && let Some(me) = status.self_node.filter(|me| !me.name.is_empty())
    {
        return me.name;
    }
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|var| std::env::var(var).ok().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| "Pasta device".to_owned())
}
