//! The running sync service: config, keys, clipboard adapter and [`Node`],
//! plus the loop that keeps the tray, the store and the UI in step with them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{AdapterOptions, ClipboardAdapter, ClipboardStatus};
use pasta_core::config::{Config, Settings};
use pasta_core::device::{DeviceInfo, SyncMode, UnixMillis};
use pasta_core::engine::EngineEvent;
use pasta_core::expiry::Ledger;
use pasta_core::history::History;
use pasta_core::remote::DeviceSettings;
use pasta_crypto::vault::{self, Vault};
use pasta_crypto::{ChainKeys, DeviceKey};
use pasta_net::discovery::TailscaleDiscovery;
use pasta_net::network::{NetEvent, NetStatus, Network, NetworkOptions, PeerSnapshot, Timing};
use pasta_net::node::{Node, NodeEvent};
use pasta_net::tailscale::LazyTailscale;
use pasta_net::trust::{ChainStatus, PeerTrust};
use pasta_net::verify::TailscaleVerifier;
use pasta_store::secrets::{DeviceSecrets, SecretStore};
use pasta_store::{Database, HistoryStore};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Notify, watch};

use crate::chain::{self, ChainView};
use crate::config::ConfigStore;
use crate::storage::Storage;
use crate::views::{DeviceSettingsView, PeerView, SettingsView, StatusView};
use crate::{diagnostics, tray};

/// Event the UI listens to for the Devices list (#10, #30).
pub const PEERS_CHANGED: &str = "peers-changed";
/// Event for the sync chain (#20, #22).
pub const CHAIN_CHANGED: &str = "chain-changed";
/// This device's settings changed, here or from another device (#29, #31).
pub const SETTINGS_CHANGED: &str = "settings-changed";
/// The clipboard history changed (#37); the panel asks for the items again.
pub const HISTORY_CHANGED: &str = "history-changed";
/// What the tray popover shows changed (#42).
pub const STATUS_CHANGED: &str = "status-changed";
/// Received items in the tray (macOS: the Mac history) that are newer than this many go.
const RECENT_ITEMS: usize = 20;
/// A tick that arrives this late means the machine slept.
const WAKE_GAP: Duration = Duration::from_secs(15);
/// How often retention runs over the Mac history besides on every insert.
const PRUNE_EVERY_TICKS: u32 = 1800;

pub struct Runtime {
    pub config: Arc<ConfigStore>,
    pub device: DeviceInfo,
    pub log_dir: PathBuf,
    pub(crate) app: AppHandle,
    pub(crate) device_key: Arc<DeviceKey>,
    pub(crate) secret_store: Arc<dyn SecretStore>,
    pub(crate) secrets: tokio::sync::Mutex<DeviceSecrets>,
    pub(crate) db: Arc<Database>,
    pub(crate) chain_keys: Mutex<Option<Arc<ChainKeys>>>,
    pub(crate) tailscale: Arc<LazyTailscale>,
    pub(crate) clipboard: Arc<dyn ClipboardAdapter>,
    clipboard_status: Mutex<ClipboardStatus>,
    pub(crate) network: Network,
    pub(crate) node: tokio::sync::Mutex<Option<Node>>,
    /// macOS: Pasta's own clipboard history, in the encrypted store (#37).
    pub(crate) history: Option<HistoryStore>,
    /// Elsewhere: the latest received items for the tray flyout (#42), sealed in memory (#24).
    pub(crate) recent: Mutex<History<vault::Sealed>>,
    pub(crate) vault: Vault,
    /// Received items and their expiry deadlines (#33), persisted.
    pub(crate) ledger: Mutex<Ledger>,
    pub(crate) expiry_changed: Notify,
    /// Source app icons as data URLs, by bundle id (#37).
    pub(crate) icons: Mutex<HashMap<String, Option<String>>>,
    tray: Mutex<Option<tray::TrayView>>,
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
        clipboard.configure(&adapter_options(&current));
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
        let (device_settings_tx, device_settings_rx) = watch::channel(current.device_settings());
        let options = NetworkOptions {
            device: device.clone(),
            device_key: Arc::clone(&device_key),
            chain,
            settings: settings_rx,
            device_settings: device_settings_rx,
            known_peers: current.known_peers.clone(),
            peer_settings: current.peer_settings.clone(),
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
        let history = cfg!(target_os = "macos").then(|| HistoryStore::new(Arc::clone(&db)));
        let ledger = crate::expiry::load(&db);
        let runtime = Arc::new(Self {
            config,
            device,
            log_dir,
            app: app.clone(),
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
            history,
            recent: Mutex::new(History::new(RECENT_ITEMS)),
            vault: Vault::new(),
            ledger: Mutex::new(ledger),
            expiry_changed: Notify::new(),
            icons: Mutex::new(HashMap::new()),
            tray: Mutex::new(None),
        });
        runtime.prune_history();
        tokio::spawn(Arc::clone(&runtime).run(app, events, settings_tx, device_settings_tx));
        Ok(runtime)
    }

    async fn run(
        self: Arc<Self>,
        app: AppHandle,
        mut events: tokio::sync::mpsc::UnboundedReceiver<NodeEvent>,
        settings: watch::Sender<Settings>,
        device_settings: watch::Sender<DeviceSettings>,
    ) {
        let mut peers = self.network.peers();
        let mut status = self.network.status();
        let mut chain = self.network.chain();
        let mut config = self.config.subscribe();
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        let mut last_tick = tokio::time::Instant::now();
        let mut ticks: u32 = 0;
        let mut adapter = adapter_options(&self.config.get());
        self.refresh_tray(&app);
        // Items that came due while Pasta wasn't running.
        self.expire_due().await;
        loop {
            let next_expiry = self.next_expiry();
            tokio::select! {
                event = events.recv() => match event {
                    Some(event) => self.on_event(event).await,
                    None => return,
                },
                Ok(()) = peers.changed() => {
                    peers.borrow_and_update();
                    self.emit_peers();
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
                    let current = config.borrow_and_update().clone();
                    let next = current.settings();
                    settings.send_if_modified(|old| std::mem::replace(old, next.clone()) != next);
                    let announced = current.device_settings();
                    device_settings.send_if_modified(|old| std::mem::replace(old, announced) != announced);
                    let options = adapter_options(&current);
                    if options != adapter {
                        self.clipboard.configure(&options);
                        adapter = options;
                    }
                    let _ = app.emit(SETTINGS_CHANGED, SettingsView::new(&current));
                    self.emit_peers();
                    self.refresh_tray(&app);
                }
                () = sleep_until_millis(next_expiry) => self.expire_due().await,
                () = self.expiry_changed.notified() => {}
                now = tick.tick() => {
                    if now.duration_since(last_tick) > WAKE_GAP {
                        tracing::info!("woke from sleep; looking for peers");
                        self.network.refresh();
                    }
                    last_tick = now;
                    ticks = ticks.wrapping_add(1);
                    // A pause that ran out: the engine and network follow at once.
                    let current = self.config.get();
                    let next = current.settings();
                    if settings.send_if_modified(|old| std::mem::replace(old, next.clone()) != next) {
                        tracing::info!("the pause ran out; syncing again");
                        let _ = app.emit(SETTINGS_CHANGED, SettingsView::new(&current));
                        self.refresh_tray(&app);
                    }
                    if ticks.is_multiple_of(15) {
                        self.refresh_clipboard_status(&app).await;
                    }
                    if ticks.is_multiple_of(PRUNE_EVERY_TICKS) {
                        self.prune_history();
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
            NodeEvent::Net(NetEvent::Connected { device_id, .. }) => self.deliver_pending(device_id),
            NodeEvent::Net(NetEvent::PeerSettings { device_id, settings }) => {
                self.on_peer_settings(device_id, settings)
            }
            NodeEvent::Net(NetEvent::ConfigUpdate { from, update }) => self.on_config_update(from, &update),
            NodeEvent::Net(NetEvent::MembersChanged { root, list }) => {
                self.save_members(root, list);
                self.forget_non_members();
            }
            NodeEvent::Net(NetEvent::RemovedFromChain) => {
                if let Err(err) = self.forget_chain().await {
                    tracing::error!(error = %err, "couldn't remove the chain key after this device was removed");
                }
            }
            NodeEvent::Engine(EngineEvent::Applied { clip, from, receipt }) => {
                self.record_received(&clip, from, receipt);
                self.store_item(&clip, true);
            }
            NodeEvent::Engine(EngineEvent::Sent { clip, .. } | EngineEvent::Copied { clip }) => {
                self.store_item(&clip, false);
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

    /// Sends the Devices list to every window.
    pub(crate) fn emit_peers(&self) {
        let _ = self.app.emit(PEERS_CHANGED, self.peer_views());
    }

    /// Rebuilds the tray menu if what it shows changed, and tells the popover.
    pub fn refresh_tray(&self, app: &AppHandle) {
        let view = self.tray_view();
        let mut shown = self.tray.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if shown.as_ref() != Some(&view) {
            tray::update(app, &view);
            *shown = Some(view);
        }
        drop(shown);
        let _ = app.emit(STATUS_CHANGED, self.status_view());
    }

    fn tray_view(&self) -> tray::TrayView {
        let peers = self.peers();
        let status = self.network.status().borrow().clone();
        let chain = self.chain_status();
        let mode = self.config.get().settings().mode;
        let connected = peers.iter().filter(|peer| peer.connected && peer.trust == PeerTrust::Member).count();
        tray::TrayView {
            status: status_line(&status, &chain, connected, mode),
            warnings: self.warnings(&status, &chain),
            peers: peers.iter().filter(|peer| peer.trust != PeerTrust::Pending).map(tray::peer_label).collect(),
            paused: mode == SyncMode::Paused,
            retry: false,
        }
    }

    fn warnings(&self, status: &NetStatus, chain: &ChainStatus) -> Vec<String> {
        let clipboard = self.clipboard_status();
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
        warnings
    }

    /// What the tray popover / flyout shows (#42).
    pub fn status_view(&self) -> StatusView {
        let view = self.tray_view();
        let status = self.network.status().borrow().clone();
        let chain = self.chain_status();
        let level = match &status.tailnet {
            pasta_net::discovery::TailnetState::Running if chain.chain.as_ref().is_some_and(|c| c.approved) => {
                if view.warnings.is_empty() { "ok" } else { "warning" }
            }
            pasta_net::discovery::TailnetState::Starting => "warning",
            pasta_net::discovery::TailnetState::Running => "warning",
            _ => "error",
        };
        let connected = self.peers().iter().filter(|peer| peer.connected && peer.trust == PeerTrust::Member).count();
        StatusView {
            status: view.status,
            level,
            warnings: view.warnings,
            this_device: Some(DeviceSettingsView::new(&self.config.get().device_settings())),
            connected,
            retry: false,
        }
    }

    pub fn peers(&self) -> Vec<PeerSnapshot> {
        self.network.peers().borrow().clone()
    }

    pub fn peer_views(&self) -> Vec<PeerView> {
        let config = self.config.get();
        self.peers().iter().map(|peer| PeerView::new(peer, config.pending_updates.get(&peer.device_id))).collect()
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
        match &self.history {
            Some(history) => history.len().unwrap_or_default(),
            None => self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len(),
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

/// What the clipboard adapter needs from the config (#36, #41).
pub(crate) fn adapter_options(config: &Config) -> AdapterOptions {
    AdapterOptions {
        skip_sensitive: config.privacy.skip_sensitive,
        excluded_apps: config.privacy.excluded_apps.iter().map(|app| app.id.to_lowercase()).collect(),
        add_to_history: config.add_to_history,
        icloud_bridge: config.icloud_bridge,
    }
}

/// Sleeps until the wall-clock time `at`, or forever for `None`.
async fn sleep_until_millis(at: Option<UnixMillis>) {
    match at {
        Some(at) => {
            let wait = at.0.saturating_sub(UnixMillis::now().0);
            tokio::time::sleep(Duration::from_millis(wait)).await;
        }
        None => std::future::pending().await,
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
