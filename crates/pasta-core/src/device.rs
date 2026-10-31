//! Devices and their sync modes.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable id of one Pasta install, generated on first run (#8).
///
/// Ordered so two peers can agree on which of their connections to keep (#9).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub Uuid);

impl DeviceId {
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.0)
    }
}

/// Milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixMillis(pub u64);

impl UnixMillis {
    pub fn now() -> Self {
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
        Self(u64::try_from(millis).unwrap_or(u64::MAX))
    }
}

/// What a device does with clipboard items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Broadcast local copies, ignore incoming items.
    Send,
    /// Apply incoming items, keep local copies to itself.
    Receive,
    #[default]
    Both,
    Paused,
}

impl SyncMode {
    pub fn sends(self) -> bool {
        matches!(self, Self::Send | Self::Both)
    }

    pub fn receives(self) -> bool {
        matches!(self, Self::Receive | Self::Both)
    }
}

/// How this device describes itself to peers in `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    /// `macos`, `windows` or `linux` (`std::env::consts::OS`).
    pub os: String,
    pub os_version: String,
    pub app_version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes() {
        assert!(SyncMode::Both.sends() && SyncMode::Both.receives());
        assert!(SyncMode::Send.sends() && !SyncMode::Send.receives());
        assert!(!SyncMode::Receive.sends() && SyncMode::Receive.receives());
        assert!(!SyncMode::Paused.sends() && !SyncMode::Paused.receives());
        assert_eq!(serde_json::to_string(&SyncMode::Paused).unwrap(), r#""paused""#);
    }

    #[test]
    fn device_ids_round_trip_as_plain_uuids() {
        let id = DeviceId::random();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id.0));
        assert_eq!(serde_json::from_str::<DeviceId>(&json).unwrap(), id);
    }
}
