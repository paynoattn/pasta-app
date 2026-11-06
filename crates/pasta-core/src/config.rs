//! Device settings and known peers (#8).
//!
//! This is only the model. The app keeps it in its encrypted store (`pasta.db`,
//! #25); M1 kept it as a plain `config.json`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, SyncMode, UnixMillis};
use crate::protocol::DEFAULT_MAX_ITEM_BYTES;
use crate::remote::{ConfigUpdate, DeviceSettings, MAX_COUNTER_STEP, SettingsVersion, effective_mode};

/// TCP port Pasta listens on, on each tailnet address.
pub const DEFAULT_PORT: u16 = 47820;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Generated on first run; never changes afterwards.
    pub device_id: DeviceId,
    /// Shown to peers. `None` uses the Tailscale device name.
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub mode: SyncMode,
    /// How long received items stay in this device's history (#33). `None` keeps them.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_max_item_bytes")]
    pub max_item_bytes: usize,
    /// `tracing` filter, e.g. `info` or `pasta_net=debug`.
    #[serde(default)]
    pub log_level: Option<String>,
    /// Devices seen before, with the Tailscale node each one connected from (#11).
    #[serde(default)]
    pub known_peers: BTreeMap<DeviceId, KnownPeer>,
    /// "Pause for 1 hour" (#32): the effective mode is `Paused` until then.
    #[serde(default)]
    pub paused_until: Option<UnixMillis>,
    /// Version of `mode` and `ttl_secs`, for changes made from other devices (#31).
    /// `None` until the first change.
    #[serde(default)]
    pub settings_version: Option<SettingsVersion>,
    /// The latest settings each member announced (#31).
    #[serde(default)]
    pub peer_settings: BTreeMap<DeviceId, DeviceSettings>,
    /// Changes made here for devices that were offline, sent when they reconnect (#31).
    #[serde(default)]
    pub pending_updates: BTreeMap<DeviceId, ConfigUpdate>,
    #[serde(default)]
    pub privacy: Privacy,
    /// Received items are added to the OS clipboard history (Win+V) as well as the clipboard.
    #[serde(default = "yes")]
    pub add_to_history: bool,
    /// Retention of Pasta's own clipboard history on macOS (#35, #37).
    #[serde(default)]
    pub history: HistoryPrefs,
    /// macOS: bridge iCloud Universal Clipboard and the tailnet (#41). Off by default.
    #[serde(default)]
    pub icloud_bridge: bool,
    /// Accelerator for the clipboard panel (#48), e.g. `Shift+Command+V`. `None` uses the default.
    #[serde(default)]
    pub panel_shortcut: Option<String>,
    #[serde(default)]
    pub updates: UpdatePrefs,
    /// The setup flow (#43) was finished or skipped.
    #[serde(default)]
    pub setup_done: bool,
}

/// What is never synced or stored (#36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Privacy {
    /// Skip copies password managers mark as concealed or transient, and copies
    /// made in the built-in list of password managers.
    #[serde(default = "yes")]
    pub skip_sensitive: bool,
    #[serde(default)]
    pub excluded_apps: Vec<ExcludedApp>,
}

impl Default for Privacy {
    fn default() -> Self {
        Self { skip_sensitive: true, excluded_apps: Vec::new() }
    }
}

/// An app whose copies are never sent or stored: a macOS bundle id, a Windows
/// executable name or a Linux desktop file id, matched case-insensitively.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedApp {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryPrefs {
    /// Most items kept.
    #[serde(default = "default_history_items")]
    pub max_items: usize,
    /// Items copied on this device are kept this many days. `None` keeps them until `max_items` pushes them out.
    #[serde(default = "default_keep_days")]
    pub keep_days: Option<u32>,
}

impl Default for HistoryPrefs {
    fn default() -> Self {
        Self { max_items: default_history_items(), keep_days: default_keep_days() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdatePrefs {
    /// Download and install updates without asking (#47). Off unless chosen.
    #[serde(default)]
    pub automatic: bool,
    #[serde(default = "default_check_hours")]
    pub check_hours: u32,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self { automatic: false, check_hours: default_check_hours() }
    }
}

fn yes() -> bool {
    true
}

fn default_history_items() -> usize {
    500
}

fn default_keep_days() -> Option<u32> {
    Some(30)
}

fn default_check_hours() -> u32 {
    24
}

impl Config {
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            device_name: None,
            mode: SyncMode::default(),
            ttl_secs: None,
            port: DEFAULT_PORT,
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
            log_level: None,
            known_peers: BTreeMap::new(),
            paused_until: None,
            settings_version: None,
            peer_settings: BTreeMap::new(),
            pending_updates: BTreeMap::new(),
            privacy: Privacy::default(),
            add_to_history: true,
            history: HistoryPrefs::default(),
            icloud_bridge: false,
            panel_shortcut: None,
            updates: UpdatePrefs::default(),
            setup_done: false,
        }
    }

    /// The settings the running engine and network follow.
    pub fn settings(&self) -> Settings {
        self.settings_at(UnixMillis::now())
    }

    /// The settings in force at `now` (a pause may have run out).
    pub fn settings_at(&self, now: UnixMillis) -> Settings {
        Settings {
            mode: effective_mode(self.mode, self.paused_until, now),
            port: self.port,
            max_item_bytes: self.max_item_bytes,
            excluded_apps: self.privacy.excluded_apps.iter().map(|app| app.id.to_lowercase()).collect(),
            icloud_bridge: self.icloud_bridge,
        }
    }

    pub fn version(&self) -> SettingsVersion {
        self.settings_version.unwrap_or_else(|| SettingsVersion::initial(self.device_id))
    }

    /// What this device tells the other members about itself (#31).
    pub fn device_settings(&self) -> DeviceSettings {
        DeviceSettings {
            mode: self.mode,
            ttl_secs: self.ttl_secs,
            paused_until: self.paused_until,
            version: self.version(),
        }
    }

    /// Records a change made on this device to `mode`, `ttl_secs` or the pause.
    pub fn bump_version(&mut self, now: UnixMillis) {
        self.settings_version = Some(self.version().next(self.device_id, now));
    }

    /// Applies an update from another member if it is newer than what this
    /// device has. Returns whether anything changed.
    pub fn apply_update(&mut self, update: &ConfigUpdate) -> bool {
        let current = self.version();
        if update.target != self.device_id || update.version <= current {
            return false;
        }
        if update.version.counter > current.counter.saturating_add(MAX_COUNTER_STEP) {
            return false;
        }
        self.mode = update.mode;
        self.ttl_secs = update.ttl_secs;
        if update.mode != SyncMode::Paused {
            // Choosing a mode from another device ends a running pause too.
            self.paused_until = None;
        }
        self.settings_version = Some(update.version);
        true
    }

    /// Ends a pause that ran out. Returns whether one did.
    pub fn end_expired_pause(&mut self, now: UnixMillis) -> bool {
        if self.paused_until.is_some_and(|until| until <= now) {
            self.paused_until = None;
            return true;
        }
        false
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_max_item_bytes() -> usize {
    DEFAULT_MAX_ITEM_BYTES
}

/// The live-reloadable part of [`Config`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The mode in force: `Paused` while a pause runs.
    pub mode: SyncMode,
    pub port: u16,
    pub max_item_bytes: usize,
    /// Lowercase ids of apps whose copies are never sent or stored (#36).
    pub excluded_apps: Arc<[String]>,
    /// macOS: send Universal Clipboard items to the tailnet (#41).
    pub icloud_bridge: bool,
}

impl Settings {
    pub fn excludes(&self, app_id: &str) -> bool {
        !app_id.is_empty() && self.excluded_apps.iter().any(|id| id.eq_ignore_ascii_case(app_id))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: SyncMode::default(),
            port: DEFAULT_PORT,
            max_item_bytes: DEFAULT_MAX_ITEM_BYTES,
            excluded_apps: Arc::from(Vec::new()),
            icloud_bridge: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownPeer {
    pub name: String,
    pub os: String,
    /// Tailscale stable node id this device has always connected from.
    pub node_id: String,
    pub last_seen: UnixMillis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_gets_defaults() {
        let id = DeviceId::random();
        let config: Config = serde_json::from_str(&format!(r#"{{"device_id":"{id}"}}"#)).unwrap();
        assert_eq!(config, Config::new(id));
        assert_eq!(config.settings(), Settings::default());
    }

    #[test]
    fn round_trips_known_peers() {
        let mut config = Config::new(DeviceId::random());
        config.mode = SyncMode::Receive;
        config.known_peers.insert(
            DeviceId::random(),
            KnownPeer {
                name: "win-workstation".into(),
                os: "windows".into(),
                node_id: "nWin0000CNTRL".into(),
                last_seen: UnixMillis(1),
            },
        );
        let json = serde_json::to_string_pretty(&config).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&json).unwrap(), config);
    }

    fn update_for(
        issuer: DeviceId,
        target: DeviceId,
        counter: u64,
        mode: SyncMode,
        ttl_secs: Option<u64>,
    ) -> ConfigUpdate {
        let version = SettingsVersion { counter, at: UnixMillis(counter), by: issuer };
        ConfigUpdate { target, mode, ttl_secs, version, signature: crate::wire::Bytes([0; 64]) }
    }

    #[test]
    fn apply_update_ignores_stale_or_mistargeted_changes() {
        let me = DeviceId::random();
        let issuer = DeviceId::random();
        let mut config = Config::new(me);
        assert!(config.apply_update(&update_for(issuer, me, 1, SyncMode::Send, Some(60))));
        assert_eq!(config.mode, SyncMode::Send);
        assert_eq!(config.ttl_secs, Some(60));

        // Older and equal versions lose.
        assert!(!config.apply_update(&update_for(issuer, me, 1, SyncMode::Both, None)), "equal version");
        assert!(!config.apply_update(&update_for(issuer, me, 0, SyncMode::Both, None)), "older version");
        assert_eq!(config.mode, SyncMode::Send);

        // Wrong target loses, whatever its version.
        assert!(!config.apply_update(&update_for(issuer, DeviceId::random(), 5, SyncMode::Paused, None)));
        assert_eq!(config.mode, SyncMode::Send);
        assert_eq!(config.ttl_secs, Some(60));
    }

    #[test]
    fn apply_update_rejects_a_counter_that_jumps_too_far() {
        let me = DeviceId::random();
        let issuer = DeviceId::random();
        let mut config = Config::new(me);
        let current = config.version().counter;

        // A step above the current counter by exactly MAX_COUNTER_STEP is fine...
        let edge = update_for(issuer, me, current + MAX_COUNTER_STEP, SyncMode::Receive, None);
        assert!(config.apply_update(&edge), "a step of MAX_COUNTER_STEP is allowed");
        assert_eq!(config.mode, SyncMode::Receive);
        assert_eq!(config.settings_version, Some(edge.version));

        // ...and one past that is refused, whatever it says: a buggy or hostile
        // member must not be able to push the counter to u64::MAX and freeze
        // the ordering. Rejected updates move nothing.
        let mut refused = Config::new(me);
        for counter in [current + MAX_COUNTER_STEP + 1, u64::MAX] {
            let hostile = update_for(issuer, me, counter, SyncMode::Paused, None);
            assert!(!refused.apply_update(&hostile), "counter {counter} was accepted");
        }
        assert_eq!(refused.version().counter, current, "a rejected update moved the counter");
    }

    #[test]
    fn apply_update_ends_a_pause_unless_the_new_mode_is_paused() {
        let me = DeviceId::random();
        let mut config = Config::new(me);
        config.paused_until = Some(UnixMillis(10_000));

        assert!(config.apply_update(&update_for(DeviceId::random(), me, 1, SyncMode::Receive, None)));
        assert_eq!(config.paused_until, None, "choosing a mode from a peer ends the pause");

        config.paused_until = Some(UnixMillis(10_000));
        assert!(config.apply_update(&update_for(DeviceId::random(), me, 2, SyncMode::Paused, None)));
        assert_eq!(config.paused_until, Some(UnixMillis(10_000)), "a Paused update leaves the pause running");
        assert_eq!(config.settings_at(UnixMillis(20_000)).mode, SyncMode::Paused);
    }

    #[test]
    fn settings_at_reflects_the_pause_before_and_after_its_deadline() {
        let mut config = Config::new(DeviceId::random());
        config.mode = SyncMode::Both;
        config.paused_until = Some(UnixMillis(1000));
        assert_eq!(config.settings_at(UnixMillis(999)).mode, SyncMode::Paused);
        assert_eq!(config.settings_at(UnixMillis(1000)).mode, SyncMode::Both, "the pause ends at its deadline");
        assert!(config.end_expired_pause(UnixMillis(1000)));
        assert_eq!(config.paused_until, None);
    }

    #[test]
    fn excluded_app_ids_are_lowercased_for_matching() {
        let mut config = Config::new(DeviceId::random());
        config.privacy.excluded_apps = vec![ExcludedApp { id: "com.Example.Secrets".into(), name: "Secrets".into() }];
        let settings = config.settings_at(UnixMillis(1));
        assert_eq!(settings.excluded_apps.iter().as_slice(), ["com.example.secrets".to_owned()].as_slice());
        assert!(settings.excludes("COM.EXAMPLE.SECRETS"));
        assert!(settings.excludes("com.example.secrets"));
        assert!(!settings.excludes("com.example.other"));
        assert!(!settings.excludes(""), "an empty app id is never excluded");
    }

    #[test]
    fn an_m2_era_config_still_loads() {
        // What M2 wrote: none of the M3 fields (#29–#48) are present.
        let me = DeviceId::random();
        let m2_json = format!(
            r#"{{
                "device_id": "{me}",
                "device_name": "macbook-pro",
                "mode": "both",
                "port": 47820,
                "max_item_bytes": 1048576,
                "known_peers": {{
                    "00000000-0000-4000-8000-000000000009": {{
                        "name": "win-workstation", "os": "windows", "node_id": "nWin0000CNTRL", "last_seen": 1730000000000
                    }}
                }}
            }}"#
        );
        let config: Config = serde_json::from_str(&m2_json).unwrap();
        // Every M3 field defaults to the same value a fresh config gets.
        let mut fresh = Config::new(me);
        fresh.device_name = Some("macbook-pro".into());
        fresh.known_peers = config.known_peers.clone();
        assert_eq!(config, fresh);
        assert_eq!(config.settings(), Settings::default());
        assert_eq!(config.version(), SettingsVersion::initial(me));
        assert_eq!(config.peer_settings, BTreeMap::new());
        assert_eq!(config.pending_updates, BTreeMap::new());
        assert_eq!(config.history, HistoryPrefs::default());
        assert!(!config.icloud_bridge && !config.setup_done && config.add_to_history);
    }
}
