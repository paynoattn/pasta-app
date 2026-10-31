//! The MVP's tray / menu-bar item with a plain native menu (#17). The real
//! popover and flyout replace it in #42.

use pasta_net::network::PeerSnapshot;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::AppState;

const TRAY_ID: &str = "pasta";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayView {
    pub status: String,
    pub warnings: Vec<String>,
    pub peers: Vec<String>,
    pub paused: bool,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let view = TrayView { status: "Starting…".into(), ..TrayView::default() };
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip(&view))
        .menu(&menu(app, &view)?)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn update(app: &AppHandle, view: &TrayView) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    match menu(app, view) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(err) => tracing::warn!(error = %err, "couldn't rebuild the tray menu"),
    }
    let _ = tray.set_tooltip(Some(tooltip(view)));
}

fn tooltip(view: &TrayView) -> String {
    format!("Pasta · {}", view.status)
}

/// No latency here: it changes with every heartbeat, and rebuilding the menu
/// that often would disturb it while open.
pub fn peer_label(peer: &PeerSnapshot) -> String {
    if peer.connected { format!("● {}", peer.name) } else { format!("○ {} · offline", peer.name) }
}

fn menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<Wry>> {
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    let label = |text: &str| MenuItem::new(app, text, false, None::<&str>);
    items.push(Box::new(label(&view.status)?));
    for warning in &view.warnings {
        items.push(Box::new(label(&format!("⚠ {warning}"))?));
    }
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    if view.peers.is_empty() {
        items.push(Box::new(label("No devices yet")?));
    }
    for peer in &view.peers {
        items.push(Box::new(label(peer)?));
    }
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(CheckMenuItem::with_id(app, "pause", "Pause syncing", true, view.paused, None::<&str>)?));
    items.push(Box::new(MenuItem::with_id(app, "diagnostics", "Copy diagnostics", true, None::<&str>)?));
    items.push(Box::new(MenuItem::with_id(app, "logs", "Open log folder", true, None::<&str>)?));
    items.push(Box::new(MenuItem::with_id(app, "about", "About Pasta", true, None::<&str>)?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(app, "quit", "Quit Pasta", true, None::<&str>)?));
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item.as_ref()).collect();
    Menu::with_items(app, &refs)
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    let runtime = app.state::<AppState>().runtime.get().cloned();
    match event.id().as_ref() {
        "quit" => crate::quit(app),
        "pause" => {
            if let Some(runtime) = runtime {
                runtime.toggle_pause();
            }
        }
        "diagnostics" => {
            if let Some(runtime) = runtime {
                tauri::async_runtime::spawn(async move { runtime.copy_diagnostics().await });
            }
        }
        "logs" => {
            if let Ok(dir) = app.path().app_log_dir() {
                open_folder(&dir);
            }
        }
        "about" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        _ => {}
    }
}

fn open_folder(dir: &std::path::Path) {
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(dir).spawn() {
        Ok(mut child) => {
            // Reap it so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(err) => tracing::warn!(error = %err, "couldn't open the log folder"),
    }
}
