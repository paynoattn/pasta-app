//! Open at login and automatic updates (#47).
//!
//! Open at login uses `tauri-plugin-autostart` (a LaunchAgent on macOS, the
//! Run key on Windows, an XDG autostart entry on Linux) and is off until the
//! user turns it on in setup or Settings.
//!
//! Updates use `tauri-plugin-updater` against signed GitHub Releases
//! manifests, but only in builds that have an update key: the release
//! pipeline (#58) sets `PASTA_UPDATER_PUBKEY` (and optionally
//! `PASTA_UPDATER_ENDPOINT`) at build time. Without them the plugin isn't
//! loaded and the Startup & updates page says updates aren't set up.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tauri_plugin_autostart::ManagerExt as _;

use crate::views::StartupView;

/// Where signed update manifests are published (#58).
const DEFAULT_ENDPOINT: &str = "https://github.com/paynoattn/pasta-app/releases/latest/download/latest.json";
/// The Startup & updates page refreshes.
pub const UPDATE_STATUS: &str = "update-status";

#[derive(Debug, Clone, Default)]
struct UpdateState {
    status: Option<String>,
    available: Option<String>,
}

static STATE: Mutex<UpdateState> = Mutex::new(UpdateState { status: None, available: None });

/// The update key built into this binary, if any.
fn pubkey() -> Option<&'static str> {
    option_env!("PASTA_UPDATER_PUBKEY").filter(|key| !key.trim().is_empty())
}

fn endpoint() -> &'static str {
    option_env!("PASTA_UPDATER_ENDPOINT").filter(|url| !url.trim().is_empty()).unwrap_or(DEFAULT_ENDPOINT)
}

pub fn updater_configured() -> bool {
    pubkey().is_some()
}

/// The updater plugin's configuration, for builds that have a key.
pub fn updater_config() -> Option<serde_json::Value> {
    let pubkey = pubkey()?;
    Some(serde_json::json!({
        "pubkey": pubkey,
        "endpoints": [endpoint()],
        // The manifest isn't signed; only accept artifacts signed for the version it names.
        "requireSignedVersion": true,
    }))
}

pub fn open_at_login(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

pub fn set_open_at_login(app: &AppHandle, on: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if on { manager.enable() } else { manager.disable() };
    result.map_err(|err| format!("couldn't change Open at login: {err}"))?;
    tracing::info!(on, "changed Open at login");
    Ok(())
}

pub fn view(app: &AppHandle, auto_update: bool, check_hours: u32) -> StartupView {
    let state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    StartupView {
        open_at_login: open_at_login(app),
        auto_update,
        check_hours,
        updater_configured: updater_configured(),
        update_status: state
            .status
            .or_else(|| (!updater_configured()).then(|| "Updates aren't set up for this build yet.".to_owned())),
        available_version: state.available,
    }
}

fn set_state(app: &AppHandle, status: String, available: Option<String>) {
    *STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = UpdateState { status: Some(status), available };
    let _ = app.emit(UPDATE_STATUS, ());
}

/// Checks for an update now; with `install`, downloads and installs it.
pub async fn check(app: &AppHandle, install: bool) -> Result<Option<String>, String> {
    if !updater_configured() {
        return Err("Updates aren't set up for this build yet.".into());
    }
    {
        use tauri_plugin_updater::UpdaterExt as _;
        let updater = app.updater().map_err(|err| err.to_string())?;
        let update = match updater.check().await {
            Ok(update) => update,
            Err(err) => {
                let message = format!("Couldn't check for updates: {err}");
                set_state(app, message.clone(), None);
                return Err(message);
            }
        };
        let Some(update) = update else {
            set_state(app, "Pasta is up to date.".into(), None);
            return Ok(None);
        };
        let version = update.version.clone();
        tracing::info!(%version, install, "an update is available");
        if !install {
            set_state(app, format!("Pasta {version} is available."), Some(version.clone()));
            return Ok(Some(version));
        }
        set_state(app, format!("Downloading Pasta {version}…"), Some(version.clone()));
        match update.download_and_install(|_, _| {}, || {}).await {
            Ok(()) => {
                set_state(app, format!("Pasta {version} is installed; it starts next time."), None);
                Ok(Some(version))
            }
            Err(err) => {
                let message = format!("Couldn't install Pasta {version}: {err}");
                set_state(app, message.clone(), Some(version));
                Err(message)
            }
        }
    }
}

/// Checks every `check_hours` (read fresh each time) in the background.
pub fn spawn_checks(app: AppHandle, prefs: impl Fn() -> (bool, u32) + Send + 'static) {
    if !updater_configured() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        // Not right at login, when the network may not be up yet.
        tokio::time::sleep(Duration::from_secs(120)).await;
        loop {
            let (automatic, hours) = prefs();
            let _ = check(&app, automatic).await;
            tokio::time::sleep(Duration::from_secs(u64::from(hours.clamp(1, 24 * 7)) * 3600)).await;
        }
    });
}
