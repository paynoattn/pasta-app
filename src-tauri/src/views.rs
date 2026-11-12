//! What the UI sees over IPC (#28). Every type here derives `TS`, and
//! `cargo test -p pasta-app` writes its TypeScript definition to
//! `src/ipc/bindings/` (see `.cargo/config.toml`), so the frontend can't drift
//! from Rust. Nothing here carries clipboard text except [`HistoryItemView`],
//! which the panel shows.

use pasta_core::config::{Config, ExcludedApp};
use pasta_core::device::{DeviceId, SyncMode};
use pasta_core::remote::{ConfigUpdate, DeviceSettings, TTL_CHOICES};
use pasta_net::network::PeerSnapshot;
use pasta_net::trust::PeerTrust;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Build and protocol info shown in Settings › About.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppInfo {
    pub version: String,
    pub protocol_version: u16,
    /// `macos`, `windows` or `linux`: picks the theme (#28).
    #[ts(type = "\"macos\" | \"windows\" | \"linux\"")]
    pub os: String,
}

impl AppInfo {
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: pasta_core::PROTOCOL_VERSION,
            os: std::env::consts::OS.to_owned(),
        }
    }
}

/// `send`, `receive`, `both` or `paused`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Mode {
    Send,
    Receive,
    Both,
    Paused,
}

impl From<SyncMode> for Mode {
    fn from(mode: SyncMode) -> Self {
        match mode {
            SyncMode::Send => Self::Send,
            SyncMode::Receive => Self::Receive,
            SyncMode::Both => Self::Both,
            SyncMode::Paused => Self::Paused,
        }
    }
}

impl From<Mode> for SyncMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Send => Self::Send,
            Mode::Receive => Self::Receive,
            Mode::Both => Self::Both,
            Mode::Paused => Self::Paused,
        }
    }
}

/// One device's mode and expiry (#30, #31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceSettingsView {
    /// The chosen mode.
    pub mode: Mode,
    /// The mode in force now: `paused` while a pause runs.
    pub effective_mode: Mode,
    /// Seconds; `null` is "Never".
    #[ts(type = "number | null")]
    pub ttl_secs: Option<u64>,
    /// Milliseconds since the epoch, while "Pause for 1 hour" runs.
    #[ts(type = "number | null")]
    pub paused_until: Option<u64>,
}

impl DeviceSettingsView {
    pub fn new(settings: &DeviceSettings) -> Self {
        let now = pasta_core::device::UnixMillis::now();
        Self {
            mode: settings.mode.into(),
            effective_mode: settings.effective_mode(now).into(),
            ttl_secs: settings.ttl_secs,
            paused_until: settings.paused_until.filter(|until| *until > now).map(|until| until.0),
        }
    }

    fn from_update(update: &ConfigUpdate) -> Self {
        Self {
            mode: update.mode.into(),
            effective_mode: update.mode.into(),
            ttl_secs: update.ttl_secs,
            paused_until: None,
        }
    }
}

/// A row in the Devices list. Mirrors [`PeerSnapshot`] plus its settings.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerView {
    pub device_id: String,
    pub name: String,
    pub os: String,
    pub connected: bool,
    #[ts(type = "number | null")]
    pub latency_ms: Option<u64>,
    #[ts(type = "number")]
    pub last_seen: u64,
    /// `member`, `pending` (in the chain, not approved) or `unknown` (this device waits for approval).
    #[ts(type = "\"member\" | \"pending\" | \"unknown\"")]
    pub trust: &'static str,
    /// The tailnet address while connected.
    pub address: Option<String>,
    /// What the device last announced.
    pub settings: Option<DeviceSettingsView>,
    /// A change made here that hasn't reached the device yet (it's offline).
    pub pending_settings: Option<DeviceSettingsView>,
}

impl PeerView {
    pub fn new(peer: &PeerSnapshot, pending: Option<&ConfigUpdate>) -> Self {
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
            address: peer.address.map(|ip| ip.to_string()),
            settings: peer.settings.as_ref().map(DeviceSettingsView::new),
            pending_settings: pending.map(DeviceSettingsView::from_update),
        }
    }
}

/// An app whose copies are never synced or stored (#36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExcludedAppView {
    /// macOS bundle id, Windows executable name or Linux desktop file id.
    pub id: String,
    pub name: String,
}

impl From<&ExcludedApp> for ExcludedAppView {
    fn from(app: &ExcludedApp) -> Self {
        Self { id: app.id.clone(), name: app.name.clone() }
    }
}

/// This device's settings, for the Settings window (#29).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsView {
    pub device_id: String,
    pub device_name: Option<String>,
    pub this_device: DeviceSettingsView,
    /// The expiry choices in seconds; `null` is "Never".
    #[ts(type = "Array<number | null>")]
    pub ttl_choices: Vec<Option<u64>>,
    pub add_to_history: bool,
    pub skip_sensitive: bool,
    pub excluded_apps: Vec<ExcludedAppView>,
    /// The built-in password managers that are always skipped while `skipSensitive` is on.
    pub builtin_excluded: Vec<String>,
    #[ts(type = "number")]
    pub history_max_items: usize,
    pub history_keep_days: Option<u32>,
    pub icloud_bridge: bool,
    pub setup_done: bool,
}

impl SettingsView {
    pub fn new(config: &Config) -> Self {
        let builtin: &[&str] = match std::env::consts::OS {
            "macos" => pasta_clipboard::sensitive::MACOS_BUNDLE_IDS,
            "windows" => pasta_clipboard::sensitive::WINDOWS_EXES,
            _ => &[],
        };
        Self {
            device_id: config.device_id.to_string(),
            device_name: config.device_name.clone(),
            this_device: DeviceSettingsView::new(&config.device_settings()),
            ttl_choices: TTL_CHOICES.to_vec(),
            add_to_history: config.add_to_history,
            skip_sensitive: config.privacy.skip_sensitive,
            excluded_apps: config.privacy.excluded_apps.iter().map(ExcludedAppView::from).collect(),
            builtin_excluded: builtin.iter().map(|id| (*id).to_owned()).collect(),
            history_max_items: config.history.max_items,
            history_keep_days: config.history.keep_days,
            icloud_bridge: config.icloud_bridge,
            setup_done: config.setup_done,
        }
    }
}

/// `text`, `link` or `code` (#40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ItemKindView {
    Text,
    Link,
    Code,
}

impl From<pasta_core::clip::ItemKind> for ItemKindView {
    fn from(kind: pasta_core::clip::ItemKind) -> Self {
        match kind {
            pasta_core::clip::ItemKind::Text => Self::Text,
            pasta_core::clip::ItemKind::Link => Self::Link,
            pasta_core::clip::ItemKind::Code => Self::Code,
        }
    }
}

/// The filter chips of the panel (#38).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HistoryFilterView {
    #[default]
    All,
    Text,
    Links,
    Code,
    FromOtherDevices,
}

impl From<HistoryFilterView> for pasta_store::HistoryFilter {
    fn from(filter: HistoryFilterView) -> Self {
        match filter {
            HistoryFilterView::All => Self::All,
            HistoryFilterView::Text => Self::Text,
            HistoryFilterView::Links => Self::Links,
            HistoryFilterView::Code => Self::Code,
            HistoryFilterView::FromOtherDevices => Self::FromOtherDevices,
        }
    }
}

/// One card in the clipboard panel, or a recent item in the tray (#38, #42).
/// Carries a preview of the text; the full item stays in Rust until pasted.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryItemView {
    pub id: String,
    pub kind: ItemKindView,
    /// The start of the plain text.
    pub preview: String,
    /// The start of the HTML flavor, or HTML made from RTF. Untrusted: the UI sanitises it.
    pub html: Option<String>,
    /// The whole item is longer than the preview.
    pub truncated: bool,
    #[ts(type = "number")]
    pub bytes: usize,
    pub source_app: Option<String>,
    pub source_app_id: Option<String>,
    /// The device it was copied on, by name. `null` for this device.
    pub origin_device: Option<String>,
    pub received: bool,
    #[ts(type = "number")]
    pub created_at: u64,
    #[ts(type = "number | null")]
    pub expires_at: Option<u64>,
}

/// Longest preview sent to the UI, in bytes.
pub const PREVIEW_BYTES: usize = 4 * 1024;
/// Longest HTML preview, in bytes.
pub const HTML_PREVIEW_BYTES: usize = 32 * 1024;

/// The first `max` bytes of `text`, cut at a character boundary.
pub fn clip_str(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One received item with an expiry, for the tray's countdowns (#33, #42).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExpiryView {
    pub item_id: String,
    pub from_device: String,
    #[ts(type = "number")]
    pub written_at: u64,
    #[ts(type = "number | null")]
    pub expires_at: Option<u64>,
}

/// What the tray popover / flyout shows (#42).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatusView {
    /// One line, e.g. "Connected · 2 devices".
    pub status: String,
    /// `ok`, `warning` or `error`: the colour of the status dot.
    #[ts(type = "\"ok\" | \"warning\" | \"error\"")]
    pub level: &'static str,
    pub warnings: Vec<String>,
    pub this_device: Option<DeviceSettingsView>,
    #[ts(type = "number")]
    pub connected: usize,
    /// The keychain couldn't be opened: offer to try again.
    pub retry: bool,
}

/// The Tailscale step of the setup flow (#43).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TailscaleView {
    /// `starting`, `missing` (not installed or not reachable), `stopped` (installed, not connected) or `running`.
    #[ts(type = "\"starting\" | \"missing\" | \"stopped\" | \"running\"")]
    pub state: &'static str,
    /// Tailscale's own words, e.g. `NeedsLogin`.
    pub detail: Option<String>,
    /// This device's tailnet address.
    pub address: Option<String>,
    /// macOS: only the `tailscale` CLI is installed (e.g. Homebrew's formula),
    /// no Tailscale.app, so the fix is a Terminal command rather than the app.
    pub cli_only: bool,
}

/// Everything the setup flow checks (#43, #44, #45).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetupView {
    #[ts(type = "\"macos\" | \"windows\" | \"linux\"")]
    pub os: String,
    pub tailscale: TailscaleView,
    #[ts(type = "\"none\" | \"waiting\" | \"member\"")]
    pub chain: &'static str,
    /// macOS: Pasta may paste into other apps (#39).
    pub accessibility: Option<bool>,
    /// Windows: Win+V history is on.
    pub clipboard_history: Option<bool>,
    /// Windows: "Sync across your devices" is on (may show items twice).
    pub cloud_sync: Option<bool>,
    pub open_at_login: bool,
    pub auto_update: bool,
    pub icloud_bridge: bool,
    pub panel_shortcut: Option<String>,
    pub setup_done: bool,
}

/// The Shortcuts page (#48).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShortcutsView {
    /// Whether this OS has a panel to open (macOS only so far).
    pub supported: bool,
    /// The accelerator in use, e.g. `Shift+Command+V`.
    pub panel: Option<String>,
    pub default_panel: Option<String>,
    /// Set when the shortcut couldn't be registered.
    pub error: Option<String>,
    /// Known clashes of the current shortcut with other apps or the OS.
    pub warning: Option<String>,
}

/// The Startup & updates page (#47).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StartupView {
    pub open_at_login: bool,
    pub auto_update: bool,
    #[ts(type = "number")]
    pub check_hours: u32,
    /// Whether this build can update itself (an update key and endpoint were built in).
    pub updater_configured: bool,
    /// The last check's outcome, e.g. "Pasta is up to date".
    pub update_status: Option<String>,
    /// A newer version found by the last check.
    pub available_version: Option<String>,
}

/// One device id parsed from the UI.
pub fn device_id(id: &str) -> Result<DeviceId, String> {
    id.parse().map(DeviceId).map_err(|_| "not a device id".to_owned())
}
