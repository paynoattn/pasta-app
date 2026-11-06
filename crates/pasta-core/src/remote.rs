//! Per-device settings that every member can see and change (#31, #32, #33).
//!
//! Each device announces its own [`DeviceSettings`] to the members it is
//! connected to, and any approved member can change another device's mode and
//! expiry with a signed [`ConfigUpdate`]. Changes are ordered by
//! [`SettingsVersion`]: a counter, then the time, then the device that made
//! the change, so concurrent edits resolve the same way everywhere
//! (last writer wins). See ADR 0012.

use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, SyncMode, UnixMillis};
use crate::wire::Sig64;

/// The expiry choices the UI offers, in seconds. `None` is "Never".
pub const TTL_CHOICES: &[Option<u64>] =
    &[Some(5), Some(30), Some(60), Some(300), Some(900), Some(3600), Some(86_400), Some(604_800), None];

/// Longest expiry a device accepts from a peer: a year.
pub const MAX_TTL_SECS: u64 = 365 * 86_400;

/// Largest step a change from another device may take the version counter, so a
/// buggy or hostile member can't push it to `u64::MAX` and freeze the ordering.
pub const MAX_COUNTER_STEP: u64 = 1 << 20;

/// How long "Pause for 1 hour" pauses.
pub const PAUSE_FOR: std::time::Duration = std::time::Duration::from_secs(3600);

/// Orders changes to one device's settings. Compared field by field, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SettingsVersion {
    pub counter: u64,
    pub at: UnixMillis,
    pub by: DeviceId,
}

impl SettingsVersion {
    /// The first version, before any change.
    pub fn initial(device: DeviceId) -> Self {
        Self { counter: 0, at: UnixMillis(0), by: device }
    }

    /// The version a change made now by `by` gets.
    pub fn next(&self, by: DeviceId, now: UnixMillis) -> Self {
        Self { counter: self.counter.saturating_add(1), at: now, by }
    }
}

/// What a device does with items, as it tells the other members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSettings {
    pub mode: SyncMode,
    /// How long items this device receives stay in its history. `None` keeps them.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    /// Set while "Pause for 1 hour" is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<UnixMillis>,
    pub version: SettingsVersion,
}

impl DeviceSettings {
    /// The mode in force at `now`: a running pause wins over the chosen mode.
    pub fn effective_mode(&self, now: UnixMillis) -> SyncMode {
        effective_mode(self.mode, self.paused_until, now)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        validate_ttl(self.ttl_secs)
    }
}

/// The mode in force at `now`, given the chosen mode and a running pause.
pub fn effective_mode(mode: SyncMode, paused_until: Option<UnixMillis>, now: UnixMillis) -> SyncMode {
    if paused_until.is_some_and(|until| until > now) { SyncMode::Paused } else { mode }
}

fn validate_ttl(ttl_secs: Option<u64>) -> Result<(), &'static str> {
    match ttl_secs {
        Some(0) => Err("expiry of zero seconds"),
        Some(secs) if secs > MAX_TTL_SECS => Err("expiry longer than a year"),
        _ => Ok(()),
    }
}

/// A member asking `target` to use a new mode and expiry. Signed by the
/// member that made the change (`version.by`) with its device key, over
/// [`ConfigUpdate::signing_bytes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigUpdate {
    pub target: DeviceId,
    pub mode: SyncMode,
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    pub version: SettingsVersion,
    pub signature: Sig64,
}

impl ConfigUpdate {
    pub fn issuer(&self) -> DeviceId {
        self.version.by
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        validate_ttl(self.ttl_secs)
    }

    /// What the issuer signs. Binds the update to one chain.
    pub fn signing_bytes(&self, chain_id: &[u8; 16]) -> Vec<u8> {
        signing_bytes(chain_id, self.target, self.mode, self.ttl_secs, &self.version)
    }
}

/// The bytes a [`ConfigUpdate`] with these fields is signed over.
pub fn signing_bytes(
    chain_id: &[u8; 16],
    target: DeviceId,
    mode: SyncMode,
    ttl_secs: Option<u64>,
    version: &SettingsVersion,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(112);
    out.extend_from_slice(b"pasta/v2/config\0");
    out.extend_from_slice(chain_id);
    out.extend_from_slice(target.0.as_bytes());
    out.push(match mode {
        SyncMode::Send => b's',
        SyncMode::Receive => b'r',
        SyncMode::Both => b'b',
        SyncMode::Paused => b'p',
    });
    match ttl_secs {
        Some(secs) => {
            out.push(1);
            out.extend_from_slice(&secs.to_be_bytes());
        }
        None => out.push(0),
    }
    out.extend_from_slice(&version.counter.to_be_bytes());
    out.extend_from_slice(&version.at.0.to_be_bytes());
    out.extend_from_slice(version.by.0.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_by_counter_then_time_then_device() {
        let a = DeviceId::random();
        let b = DeviceId::random();
        let v1 = SettingsVersion::initial(a).next(a, UnixMillis(10));
        assert!(v1 > SettingsVersion::initial(a));
        let later = v1.next(b, UnixMillis(5));
        assert!(later > v1, "a higher counter wins even with an older clock");
        let same_counter_newer = SettingsVersion { at: UnixMillis(11), ..v1 };
        assert!(same_counter_newer > v1);
    }

    #[test]
    fn pause_wins_until_it_runs_out() {
        let until = Some(UnixMillis(100));
        assert_eq!(effective_mode(SyncMode::Both, until, UnixMillis(99)), SyncMode::Paused);
        assert_eq!(effective_mode(SyncMode::Both, until, UnixMillis(100)), SyncMode::Both);
        assert_eq!(effective_mode(SyncMode::Receive, None, UnixMillis(1)), SyncMode::Receive);
    }

    #[test]
    fn signing_bytes_cover_every_field() {
        let chain = [7u8; 16];
        let target = DeviceId::random();
        let version = SettingsVersion::initial(target).next(DeviceId::random(), UnixMillis(3));
        let base = signing_bytes(&chain, target, SyncMode::Both, Some(60), &version);
        assert_ne!(base, signing_bytes(&[8u8; 16], target, SyncMode::Both, Some(60), &version));
        assert_ne!(base, signing_bytes(&chain, DeviceId::random(), SyncMode::Both, Some(60), &version));
        assert_ne!(base, signing_bytes(&chain, target, SyncMode::Send, Some(60), &version));
        assert_ne!(base, signing_bytes(&chain, target, SyncMode::Both, None, &version));
        assert_ne!(base, signing_bytes(&chain, target, SyncMode::Both, Some(61), &version));
        let bumped = version.next(version.by, UnixMillis(3));
        assert_ne!(base, signing_bytes(&chain, target, SyncMode::Both, Some(60), &bumped));
    }

    #[test]
    fn rejects_silly_expiry() {
        let version = SettingsVersion::initial(DeviceId::random());
        let settings = |ttl_secs| DeviceSettings { mode: SyncMode::Both, ttl_secs, paused_until: None, version };
        assert!(settings(Some(0)).validate().is_err());
        assert!(settings(Some(MAX_TTL_SECS + 1)).validate().is_err());
        assert!(settings(Some(5)).validate().is_ok());
        assert!(settings(None).validate().is_ok());
    }
}
