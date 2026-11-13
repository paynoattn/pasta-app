mod chain;
mod clip_history;
mod commands;
mod config;
mod diagnostics;
mod expiry;
#[cfg(target_os = "linux")]
mod kwin;
mod logging;
#[cfg(target_os = "macos")]
mod macos_panel;
mod prefs;
mod runtime;
mod setup;
mod shortcuts;
mod startup;
mod storage;
mod tray;
mod ui;
mod views;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pasta_store::KeyringStore;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tokio::sync::Notify;

use crate::config::ConfigStore;
use crate::runtime::Runtime;
use crate::storage::StorageError;
use crate::views::StatusView;

/// How often a locked or missing keychain is tried again on its own.
const KEYCHAIN_RETRY: Duration = Duration::from_secs(30);

pub(crate) struct AppState {
    pub runtime: OnceLock<Arc<Runtime>>,
    /// Wakes the startup loop to try the keychain again now.
    pub retry: Arc<Notify>,
    /// What the tray shows before the runtime is up: the line, and whether to offer a retry.
    boot: Mutex<(String, bool)>,
    quitting: AtomicBool,
    _log_guard: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
}

impl AppState {
    pub(crate) fn boot_status(&self) -> StatusView {
        let (status, retry) = self.boot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        StatusView {
            level: if retry || status.starts_with("Not syncing") { "error" } else { "warning" },
            status,
            warnings: Vec::new(),
            this_device: None,
            connected: 0,
            retry,
        }
    }

    fn set_boot(&self, app: &AppHandle, status: &str, retry: bool) {
        *self.boot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = (status.to_owned(), retry);
        let _ = app.emit(runtime::STATUS_CHANGED, self.boot_status());
    }
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
        #[cfg(target_os = "linux")]
        kwin::uninstall().await;
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
    startup::log_login_repair();
    let hardening = pasta_crypto::hardening::harden_process();
    tracing::info!(applied = ?hardening.applied, failed = ?hardening.failed, "process hardening");

    let retry = Arc::new(Notify::new());
    app.manage(AppState {
        runtime: OnceLock::new(),
        retry: Arc::clone(&retry),
        boot: Mutex::new(("Starting…".to_owned(), false)),
        quitting: AtomicBool::new(false),
        _log_guard: Mutex::new(log_guard),
    });
    tray::create(app.handle())?;
    ui::create_windows(app.handle())?;
    #[cfg(target_os = "linux")]
    kwin::install();

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
                handle.state::<AppState>().set_boot(&handle, "Waiting for the keychain", true);
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
            let current = runtime.config.get();
            shortcuts::register_configured(&handle, current.panel_shortcut.as_deref());
            let checks = Arc::clone(&runtime);
            startup::spawn_checks(handle.clone(), move || {
                let updates = checks.config.get().updates;
                (updates.automatic, updates.check_hours)
            });
            let first_run = !current.setup_done && view.state == "none";
            let _ = handle.state::<AppState>().runtime.set(runtime);
            // A window opened while the keychain was locked asked too early; tell it now.
            let _ = handle.emit(runtime::CHAIN_CHANGED, view);
            let _ = handle.emit(runtime::SETTINGS_CHANGED, views::SettingsView::new(&current));
            if first_run {
                ui::open_setup(&handle);
            }
        }
        Err(err) => not_syncing(&handle, &err),
    }
}

fn not_syncing(handle: &AppHandle, err: &str) {
    tracing::error!(error = %err, "couldn't start syncing");
    let status = format!("Not syncing: {err}");
    handle.state::<AppState>().set_boot(handle, &status, false);
    tray::update(handle, &tray::TrayView { status, ..Default::default() });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();
    // Before the single-instance plugin, which ends a second launch (#47).
    #[cfg(windows)]
    startup::repoint_login_entry(&context.package_info().name);
    let mut builder = tauri::Builder::default()
        // Must be the first plugin: a second launch exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tracing::info!("Pasta was launched again; the running instance stays");
            ui::open_settings(app, None);
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build());
    // Only builds with an update key can update themselves (#47, #58).
    if let Some(updater) = startup::updater_config() {
        context.config_mut().plugins.0.insert("updater".into(), updater);
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let app = builder
        .invoke_handler(all_commands!())
        .setup(setup)
        .on_window_event(|window, event| {
            // Windows only hide: Pasta keeps running in the tray. The setup window is
            // made again when needed.
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() != ui::SETUP
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(context)
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
