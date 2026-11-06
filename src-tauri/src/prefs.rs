//! Changing settings, on this device and on others (#29–#32, #36, #41).
//!
//! A device's mode and expiry can be set from any member: on this device the
//! config changes at once; for another device a signed `config_update` is
//! sent, and kept in `pending_updates` until that device announces settings
//! at least as new. Updates for offline devices go out when they reconnect.

use pasta_core::config::{ExcludedApp, HistoryPrefs};
use pasta_core::device::{DeviceId, SyncMode, UnixMillis};
use pasta_core::remote::{ConfigUpdate, DeviceSettings, MAX_TTL_SECS, PAUSE_FOR, SettingsVersion};

use crate::runtime::Runtime;

/// Longest excluded-app id or name, in bytes.
const MAX_APP_FIELD: usize = 256;
const MAX_EXCLUDED_APPS: usize = 200;

impl Runtime {
    fn save(&self, what: &str, change: impl FnOnce(&mut pasta_core::config::Config)) -> Result<(), String> {
        self.config.update(change).map_err(|err| format!("couldn't save {what}: {err}"))
    }

    /// Sets `device`'s mode and expiry. For another device the change is sent
    /// to it, now or when it reconnects.
    pub fn set_device_settings(&self, device: DeviceId, mode: SyncMode, ttl_secs: Option<u64>) -> Result<(), String> {
        validate_ttl(ttl_secs)?;
        if device == self.device.id {
            return self.set_own_settings(mode, ttl_secs);
        }
        let config = self.config.get();
        if !config.known_peers.contains_key(&device) && self.network.peer_settings(device).is_none() {
            return Err("that device isn't known here".into());
        }
        // Newer than anything this device knows of the target's settings, including its own queued change.
        let known = self
            .network
            .peer_settings(device)
            .or_else(|| config.peer_settings.get(&device).copied())
            .map(|settings| settings.version);
        let queued = config.pending_updates.get(&device).map(|update| update.version);
        let base = known.into_iter().chain(queued).max().unwrap_or_else(|| SettingsVersion::initial(device));
        let version = base.next(self.device.id, UnixMillis::now());
        let update = self
            .network
            .sign_config_update(device, mode, ttl_secs, version)
            .ok_or("this device isn't in a sync chain")?;
        let delivered = self.network.deliver_config_update(&update)?;
        tracing::info!(peer = %device, ?mode, ?ttl_secs, delivered, "changed another device's settings");
        self.save("the change", |config| {
            config.pending_updates.insert(device, update);
        })?;
        self.emit_peers();
        Ok(())
    }

    fn set_own_settings(&self, mode: SyncMode, ttl_secs: Option<u64>) -> Result<(), String> {
        // Read inside the serialised update, so a concurrent change can't hide a new expiry.
        let mut ttl_changed = false;
        self.save("the mode", |config| {
            if config.mode == mode && config.ttl_secs == ttl_secs && config.paused_until.is_none() {
                return;
            }
            ttl_changed = config.ttl_secs != ttl_secs;
            config.mode = mode;
            config.ttl_secs = ttl_secs;
            // Picking a mode ends "Pause for 1 hour".
            config.paused_until = None;
            config.bump_version(UnixMillis::now());
        })?;
        if ttl_changed {
            self.retime_received(ttl_secs);
        }
        Ok(())
    }

    /// "Pause for 1 hour" (#32). Resumes on its own.
    pub fn pause_for_hour(&self) -> Result<(), String> {
        let until = UnixMillis(UnixMillis::now().0.saturating_add(PAUSE_FOR.as_millis() as u64));
        self.save("the pause", |config| {
            config.paused_until = Some(until);
            config.bump_version(UnixMillis::now());
        })?;
        tracing::info!("paused syncing for an hour");
        Ok(())
    }

    /// Ends a pause, and the `Paused` mode.
    pub fn resume(&self) -> Result<(), String> {
        self.save("the mode", |config| {
            if config.paused_until.is_none() && config.mode != SyncMode::Paused {
                return;
            }
            config.paused_until = None;
            if config.mode == SyncMode::Paused {
                config.mode = SyncMode::Both;
            }
            config.bump_version(UnixMillis::now());
        })
    }

    /// The tray's pause item: pauses for an hour, or resumes.
    pub fn toggle_pause(&self) {
        let paused = self.config.get().settings().mode == SyncMode::Paused;
        let result = if paused { self.resume() } else { self.pause_for_hour() };
        if let Err(err) = result {
            tracing::warn!(error = %err, "couldn't change the pause");
        }
    }

    pub fn set_privacy(&self, skip_sensitive: bool, excluded_apps: Vec<ExcludedApp>) -> Result<(), String> {
        if excluded_apps.len() > MAX_EXCLUDED_APPS {
            return Err(format!("at most {MAX_EXCLUDED_APPS} excluded apps"));
        }
        let mut apps: Vec<ExcludedApp> = Vec::with_capacity(excluded_apps.len());
        for app in excluded_apps {
            let id = app.id.trim().to_owned();
            if id.is_empty() || id.len() > MAX_APP_FIELD || app.name.len() > MAX_APP_FIELD {
                return Err("an excluded app needs an id of at most 256 bytes".into());
            }
            if !apps.iter().any(|known| known.id.eq_ignore_ascii_case(&id)) {
                let name = if app.name.trim().is_empty() { id.clone() } else { app.name.trim().to_owned() };
                apps.push(ExcludedApp { id, name });
            }
        }
        self.save("the privacy settings", |config| {
            config.privacy.skip_sensitive = skip_sensitive;
            config.privacy.excluded_apps = apps;
        })
    }

    pub fn set_add_to_history(&self, on: bool) -> Result<(), String> {
        self.save("the setting", |config| config.add_to_history = on)
    }

    pub fn set_history_prefs(&self, max_items: usize, keep_days: Option<u32>) -> Result<(), String> {
        if !(10..=10_000).contains(&max_items) {
            return Err("keep between 10 and 10,000 items".into());
        }
        if keep_days.is_some_and(|days| days == 0 || days > 3650) {
            return Err("keep items between 1 day and 10 years".into());
        }
        self.save("the history settings", |config| config.history = HistoryPrefs { max_items, keep_days })?;
        self.prune_history();
        Ok(())
    }

    pub fn set_icloud_bridge(&self, on: bool) -> Result<(), String> {
        self.save("the iCloud setting", |config| config.icloud_bridge = on)
    }

    /// `None` goes back to the Tailscale device name (after a restart).
    pub fn set_device_name(&self, name: Option<String>) -> Result<(), String> {
        let name = name.map(|name| name.trim().to_owned()).filter(|name| !name.is_empty());
        if name.as_ref().is_some_and(|name| name.len() > pasta_core::protocol::MAX_HELLO_FIELD_BYTES) {
            return Err("that name is too long".into());
        }
        self.save("the name", |config| config.device_name = name)
    }

    pub fn finish_setup(&self) -> Result<(), String> {
        self.save("the setup state", |config| config.setup_done = true)
    }

    /// Drops queued changes for devices that are no longer members (ADR 0012), so a
    /// device approved again later doesn't get an old change.
    pub(crate) fn forget_non_members(&self) {
        let members: Vec<DeviceId> = self
            .chain_status()
            .chain
            .map(|chain| chain.members.iter().map(|member| member.device_id).collect())
            .unwrap_or_default();
        let result = self.config.update(|config| {
            config.pending_updates.retain(|device, _| members.contains(device));
        });
        if let Err(err) = result {
            tracing::warn!(error = %err, "couldn't drop queued changes for removed devices");
        }
    }

    /// Sends a queued change to a device that just connected.
    pub(crate) fn deliver_pending(&self, device: DeviceId) {
        let Some(update) = self.config.get().pending_updates.get(&device).cloned() else { return };
        match self.network.deliver_config_update(&update) {
            Ok(true) => tracing::info!(peer = %device, "sent a queued settings change"),
            Ok(false) => {}
            Err(err) => tracing::warn!(peer = %device, error = %err, "couldn't send a queued settings change"),
        }
    }

    /// A member announced its settings: remember them, and drop a queued change it has caught up with.
    pub(crate) fn on_peer_settings(&self, device: DeviceId, settings: DeviceSettings) {
        let result = self.config.update(|config| {
            config.peer_settings.insert(device, settings);
            if config.pending_updates.get(&device).is_some_and(|update| update.version <= settings.version) {
                config.pending_updates.remove(&device);
            }
        });
        if let Err(err) = result {
            tracing::warn!(error = %err, "couldn't save a device's settings");
        }
        self.emit_peers();
    }

    /// Another member changed this device's mode and expiry (#31). The
    /// network checked the sender and signature; newer versions win.
    pub(crate) fn on_config_update(&self, from: DeviceId, update: &ConfigUpdate) {
        let mut applied = false;
        let mut ttl_changed = false;
        let result = self.config.update(|config| {
            let previous = config.ttl_secs;
            applied = config.apply_update(update);
            ttl_changed = applied && previous != update.ttl_secs;
        });
        match result {
            Ok(()) if applied => {
                tracing::info!(peer = %from, mode = ?update.mode, ttl = ?update.ttl_secs, "settings changed from another device");
                if ttl_changed {
                    self.retime_received(update.ttl_secs);
                }
            }
            Ok(()) => tracing::debug!(peer = %from, "ignored an older settings change"),
            Err(err) => tracing::warn!(error = %err, "couldn't save a settings change from another device"),
        }
    }
}

fn validate_ttl(ttl_secs: Option<u64>) -> Result<(), String> {
    match ttl_secs {
        Some(0) => Err("the expiry must be at least a second".into()),
        Some(secs) if secs > MAX_TTL_SECS => Err("the expiry can be at most a year".into()),
        _ => Ok(()),
    }
}
