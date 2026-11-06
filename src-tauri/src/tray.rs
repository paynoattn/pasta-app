//! The tray / menu-bar icon (#17, #42). A left click opens the popover
//! (macOS) or flyout (Windows) in the `tray` window; a right click shows a
//! small native menu, which also keeps Pasta usable if the webview fails.

use pasta_net::network::PeerSnapshot;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::AppState;

const TRAY_ID: &str = "pasta";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayView {
    pub status: String,
    pub warnings: Vec<String>,
    pub peers: Vec<String>,
    pub paused: bool,
    /// The keychain couldn't be opened: offer to try again now.
    pub retry: bool,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let view = TrayView { status: "Starting…".into(), ..TrayView::default() };
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip(&view))
        .menu(&menu(app, &view)?)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_event);
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

fn on_tray_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click { rect, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
        let app = tray.app_handle();
        let scale = app.primary_monitor().ok().flatten().map_or(1.0, |monitor| monitor.scale_factor());
        let position = rect.position.to_physical::<f64>(scale);
        let size = rect.size.to_physical::<f64>(scale);
        crate::ui::toggle_tray(app, Some((position.x, position.y, size.width, size.height)));
    }
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
    if view.retry {
        items.push(Box::new(MenuItem::with_id(app, "retry", "Try the keychain again", true, None::<&str>)?));
    }
    if cfg!(target_os = "macos") {
        items.push(Box::new(MenuItem::with_id(app, "panel", "Open Clipboard", true, None::<&str>)?));
    }
    items.push(Box::new(MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?));
    items.push(Box::new(CheckMenuItem::with_id(app, "pause", "Pause for 1 hour", true, view.paused, None::<&str>)?));
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
        "retry" => app.state::<AppState>().retry.notify_one(),
        "panel" => crate::ui::toggle_panel(app),
        "settings" => crate::ui::open_settings(app, None),
        "about" => crate::ui::open_settings(app, Some("about")),
        _ => {}
    }
}

pub(crate) fn open_folder(dir: &std::path::Path) {
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
