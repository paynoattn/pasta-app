//! Device settings and known peers (#8).
//!
//! This is only the model. The app keeps it in its encrypted store (`pasta.db`,
//! #25); M1 kept it as a plain `config.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, SyncMode, UnixMillis};
use crate::protocol::DEFAULT_MAX_ITEM_BYTES;

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
        }
    }

    /// The settings the running engine and network follow.
    pub fn settings(&self) -> Settings {
        Settings { mode: self.mode, port: self.port, max_item_bytes: self.max_item_bytes }
    }
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_max_item_bytes() -> usize {
    DEFAULT_MAX_ITEM_BYTES
}

/// The live-reloadable part of [`Config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub mode: SyncMode,
    pub port: u16,
    pub max_item_bytes: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self { mode: SyncMode::default(), port: DEFAULT_PORT, max_item_bytes: DEFAULT_MAX_ITEM_BYTES }
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
}
