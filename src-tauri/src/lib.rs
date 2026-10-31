mod config;
mod diagnostics;
mod logging;
mod runtime;
mod tray;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use crate::config::ConfigStore;
use crate::runtime::{PeerView, Runtime};

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

pub(crate) struct AppState {
    pub runtime: OnceLock<Arc<Runtime>>,
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
    let config = Arc::new(ConfigStore::open(&data_dir)?);
    let log_guard = logging::init(&log_dir, config.get().log_level.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "Pasta starting");
    for notice in config.take_notices() {
        tracing::info!("{notice}");
    }

    app.manage(AppState {
        runtime: OnceLock::new(),
        quitting: AtomicBool::new(false),
        _log_guard: Mutex::new(log_guard),
    });
    tray::create(app.handle())?;

    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        match Runtime::start(handle.clone(), config, log_dir).await {
            Ok(runtime) => {
                runtime.refresh_tray(&handle);
                let _ = handle.state::<AppState>().runtime.set(runtime);
            }
            Err(err) => {
                tracing::error!(error = %err, "couldn't start syncing");
                tray::update(&handle, &tray::TrayView { status: format!("Not syncing: {err}"), ..Default::default() });
            }
        }
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Must be the first plugin: a second launch exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {
            tracing::info!("Pasta was launched again; the running instance stays");
        }))
        .invoke_handler(tauri::generate_handler![app_info, list_peers])
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
