mod chain;
mod config;
mod diagnostics;
mod logging;
mod runtime;
mod storage;
mod tray;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pasta_core::device::DeviceId;
use pasta_store::KeyringStore;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tokio::sync::Notify;

use crate::chain::ChainView;
use crate::config::ConfigStore;
use crate::runtime::{PeerView, Runtime};
use crate::storage::StorageError;

/// How often a locked or missing keychain is tried again on its own.
const KEYCHAIN_RETRY: Duration = Duration::from_secs(30);

/// Build and protocol info shown in Settings › About.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    protocol_version: u16,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo { version: env!("CARGO_PKG_VERSION"), protocol_version: pasta_core::PROTOCOL_VERSION }
}

/// Known devices, connected first. Updates arrive as the `peers-changed` event.
#[tauri::command]
fn list_peers(state: tauri::State<'_, AppState>) -> Vec<PeerView> {
    state.runtime.get().map(|runtime| runtime.peers().iter().map(PeerView::from).collect()).unwrap_or_default()
}

fn runtime(state: &tauri::State<'_, AppState>) -> Result<Arc<Runtime>, String> {
    state.runtime.get().cloned().ok_or_else(|| "Pasta is still starting (is the keychain locked?)".to_owned())
}

fn device_id(id: &str) -> Result<DeviceId, String> {
    id.parse().map(DeviceId).map_err(|_| "not a device id".to_owned())
}

/// The sync chain as the developer panel shows it. Updates arrive as `chain-changed`.
#[tauri::command]
fn chain_status(state: tauri::State<'_, AppState>) -> Result<ChainView, String> {
    Ok(runtime(&state)?.chain_view())
}

/// Starts a new chain. Returns its 24 words.
#[tauri::command]
async fn create_chain(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let words = runtime(&state)?.create_chain().await?;
    Ok(words.as_str().to_owned())
}

#[tauri::command]
async fn join_chain(state: tauri::State<'_, AppState>, words: String) -> Result<(), String> {
    let words = zeroize::Zeroizing::new(words);
    runtime(&state)?.join_chain(&words).await
}

#[tauri::command]
async fn leave_chain(state: tauri::State<'_, AppState>) -> Result<(), String> {
    runtime(&state)?.leave_chain().await
}

/// The chain's 24 words, for adding another device.
#[tauri::command]
fn chain_words(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(runtime(&state)?.chain_words()?.as_str().to_owned())
}

/// `key` is the fingerprint shown for the device, which the user compared with its screen.
#[tauri::command]
async fn approve_device(state: tauri::State<'_, AppState>, device_id: String, key: String) -> Result<(), String> {
    runtime(&state)?.approve_device(crate::device_id(&device_id)?, &key).await
}

#[tauri::command]
async fn decline_device(state: tauri::State<'_, AppState>, device_id: String) -> Result<(), String> {
    runtime(&state)?.decline_device(crate::device_id(&device_id)?).await
}

#[tauri::command]
async fn remove_device(state: tauri::State<'_, AppState>, device_id: String) -> Result<(), String> {
    runtime(&state)?.remove_device(crate::device_id(&device_id)?).await
}

pub(crate) struct AppState {
    pub runtime: OnceLock<Arc<Runtime>>,
    /// Wakes the startup loop to try the keychain again now.
    pub retry: Arc<Notify>,
    quitting: AtomicBool,
    _log_guard: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
}

/// Stops syncing cleanly, then exits.
pub(crate) fn quit(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.quitting.swap(true, Ordering::SeqCst) {
        return;
    }
    let runtime = state.runtime.get().cloned();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(runtime) = runtime {
            runtime.shutdown().await;
        }
        tracing::info!("Pasta stopped");
        app.exit(0);
    });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Menu-bar only: no Dock icon or app menu.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let data_dir = app.path().app_data_dir()?;
    let log_dir = app.path().app_log_dir()?;
    let log_guard = logging::init(&log_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Pasta starting");
    let hardening = pasta_crypto::hardening::harden_process();
    tracing::info!(applied = ?hardening.applied, failed = ?hardening.failed, "process hardening");

    let retry = Arc::new(Notify::new());
    app.manage(AppState {
        runtime: OnceLock::new(),
        retry: Arc::clone(&retry),
        quitting: AtomicBool::new(false),
        _log_guard: Mutex::new(log_guard),
    });
    tray::create(app.handle())?;

    let handle = app.handle().clone();
    tauri::async_runtime::spawn(start(handle, data_dir, log_dir, retry));
    Ok(())
}

/// Opens the keychain and store (retrying while the keychain is locked), then starts syncing.
async fn start(handle: AppHandle, data_dir: PathBuf, log_dir: PathBuf, retry: Arc<Notify>) {
    let store = Arc::new(KeyringStore::default());
    let storage = loop {
        let dir = data_dir.clone();
        let store: Arc<dyn pasta_store::SecretStore> = store.clone();
        let opened = tokio::task::spawn_blocking(move || storage::open(&dir, store)).await;
        match opened {
            Ok(Ok(storage)) => break storage,
            Ok(Err(StorageError::Locked(err))) => {
                tracing::warn!(error = %err, "can't open the keychain; trying again");
                tray::update(
                    &handle,
                    &tray::TrayView {
                        status: "Waiting for the keychain".into(),
                        warnings: vec![err.to_string()],
                        retry: true,
                        ..Default::default()
                    },
                );
                tokio::select! {
                    () = tokio::time::sleep(KEYCHAIN_RETRY) => {}
                    () = retry.notified() => {}
                }
            }
            Ok(Err(err)) => return not_syncing(&handle, &err.to_string()),
            Err(err) => return not_syncing(&handle, &err.to_string()),
        }
    };
    for notice in &storage.notices {
        tracing::info!("{notice}");
    }
    let config = match ConfigStore::open(Arc::clone(&storage.db), &data_dir, storage.secrets.device_id) {
        Ok(config) => Arc::new(config),
        Err(err) => return not_syncing(&handle, &err.to_string()),
    };
    logging::set_level(config.get().log_level.as_deref());
    for notice in config.take_notices() {
        tracing::info!("{notice}");
    }
    match Runtime::start(handle.clone(), config, storage, log_dir).await {
        Ok(runtime) => {
            runtime.refresh_tray(&handle);
            let view = runtime.chain_view();
            let _ = handle.state::<AppState>().runtime.set(runtime);
            // A window opened while the keychain was locked asked too early; tell it now.
            let _ = handle.emit(runtime::CHAIN_CHANGED, view);
        }
        Err(err) => not_syncing(&handle, &err),
    }
}

fn not_syncing(handle: &AppHandle, err: &str) {
    tracing::error!(error = %err, "couldn't start syncing");
    tray::update(handle, &tray::TrayView { status: format!("Not syncing: {err}"), ..Default::default() });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Must be the first plugin: a second launch exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {
            tracing::info!("Pasta was launched again; the running instance stays");
        }))
        .invoke_handler(tauri::generate_handler![
            app_info,
            list_peers,
            chain_status,
            create_chain,
            join_chain,
            leave_chain,
            chain_words,
            approve_device,
            decline_device,
            remove_device,
        ])
        .setup(setup)
        .on_window_event(|window, event| {
            // Windows only hide: Pasta keeps running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building the Pasta app");
    app.run(|app, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            // Closing the last window isn't quitting (no code). A real exit request
            // first shuts down cleanly; `quit` then exits again with `quitting` set.
            if code.is_none() {
                api.prevent_exit();
            } else if !app.state::<AppState>().quitting.load(Ordering::SeqCst) {
                api.prevent_exit();
                quit(app);
            }
        }
    });
}
