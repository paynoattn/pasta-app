//! The tray / menu-bar icon (#17, #42, #53). A left click opens the popover
//! (macOS) or flyout (Windows, KDE) in the `tray` window; a right click shows
//! a small native menu, which also keeps Pasta usable if the webview fails.
//!
//! On Linux the icon is a StatusNotifierItem of its own: Tauri's tray goes
//! through libappindicator, which never reports a left click.

use pasta_net::network::PeerSnapshot;
use tauri::{AppHandle, Manager};

use crate::AppState;

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
    #[cfg(target_os = "linux")]
    sni::create(app, view);
    #[cfg(not(target_os = "linux"))]
    native::create(app, &view)?;
    Ok(())
}

pub fn update(app: &AppHandle, view: &TrayView) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        sni::update(view);
    }
    #[cfg(not(target_os = "linux"))]
    native::update(app, view);
}

/// No latency here: it changes with every heartbeat, and rebuilding the menu
/// that often would disturb it while open.
pub fn peer_label(peer: &PeerSnapshot) -> String {
    if peer.connected { format!("● {}", peer.name) } else { format!("○ {} · offline", peer.name) }
}

/// One menu entry, for both tray implementations. Labels are disabled items.
enum Entry {
    Label(String),
    Item(&'static str, &'static str),
    Check(&'static str, &'static str, bool),
    Separator,
}

fn entries(view: &TrayView) -> Vec<Entry> {
    let mut entries = vec![Entry::Label(view.status.clone())];
    entries.extend(view.warnings.iter().map(|warning| Entry::Label(format!("⚠ {warning}"))));
    entries.push(Entry::Separator);
    if view.peers.is_empty() {
        entries.push(Entry::Label("No devices yet".into()));
    }
    entries.extend(view.peers.iter().cloned().map(Entry::Label));
    entries.push(Entry::Separator);
    if view.retry {
        entries.push(Entry::Item("retry", "Try the keychain again"));
    }
    if cfg!(target_os = "macos") {
        entries.push(Entry::Item("panel", "Open Clipboard"));
    }
    entries.push(Entry::Item("settings", "Settings…"));
    entries.push(Entry::Check("pause", "Pause for 1 hour", view.paused));
    entries.push(Entry::Item("diagnostics", "Copy diagnostics"));
    entries.push(Entry::Item("logs", "Open log folder"));
    entries.push(Entry::Item("about", "About Pasta"));
    entries.push(Entry::Separator);
    entries.push(Entry::Item("quit", "Quit Pasta"));
    entries
}

fn on_menu(app: &AppHandle, id: &str) {
    let runtime = app.state::<AppState>().runtime.get().cloned();
    match id {
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

#[cfg(not(target_os = "linux"))]
mod native {
    use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{AppHandle, Wry};

    use super::{Entry, TrayView, entries, on_menu};

    const TRAY_ID: &str = "pasta";

    fn tooltip(view: &TrayView) -> String {
        format!("Pasta · {}", view.status)
    }

    pub fn create(app: &AppHandle, view: &TrayView) -> tauri::Result<()> {
        let mut builder = TrayIconBuilder::with_id(TRAY_ID)
            .tooltip(tooltip(view))
            .menu(&menu(app, view)?)
            .show_menu_on_left_click(false)
            .on_menu_event(|app: &AppHandle, event: MenuEvent| on_menu(app, event.id().as_ref()))
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

    fn on_tray_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
        if let TrayIconEvent::Click { rect, button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event
        {
            let app = tray.app_handle();
            let scale = app.primary_monitor().ok().flatten().map_or(1.0, |monitor| monitor.scale_factor());
            let position = rect.position.to_physical::<f64>(scale);
            let size = rect.size.to_physical::<f64>(scale);
            crate::ui::toggle_tray(app, Some((position.x, position.y, size.width, size.height)));
        }
    }

    fn menu(app: &AppHandle, view: &TrayView) -> tauri::Result<Menu<Wry>> {
        let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
        for entry in entries(view) {
            items.push(match entry {
                Entry::Label(text) => Box::new(MenuItem::new(app, text, false, None::<&str>)?),
                Entry::Item(id, text) => Box::new(MenuItem::with_id(app, id, text, true, None::<&str>)?),
                Entry::Check(id, text, on) => Box::new(CheckMenuItem::with_id(app, id, text, true, on, None::<&str>)?),
                Entry::Separator => Box::new(PredefinedMenuItem::separator(app)?),
            });
        }
        let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|item| item.as_ref()).collect();
        Menu::with_items(app, &refs)
    }
}

#[cfg(target_os = "linux")]
mod sni {
    use std::sync::OnceLock;

    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{CheckmarkItem, StandardItem};
    use ksni::{Icon, MenuItem, ToolTip};
    use tauri::AppHandle;
    use tauri::image::Image;

    use super::{Entry, TrayView, entries, on_menu};

    static HANDLE: OnceLock<Handle<PastaTray>> = OnceLock::new();

    struct PastaTray {
        app: AppHandle,
        view: TrayView,
        icons: Vec<Icon>,
    }

    pub fn create(app: &AppHandle, view: TrayView) {
        let tray = PastaTray { app: app.clone(), view, icons: icons() };
        // Plasma's panel may come up after Pasta at login; the icon appears once it does.
        match tray.assume_sni_available(true).spawn() {
            Ok(handle) => {
                let _ = HANDLE.set(handle);
            }
            Err(err) => tracing::warn!(error = %err, "couldn't show the tray icon"),
        }
    }

    pub fn update(view: &TrayView) {
        if let Some(handle) = HANDLE.get() {
            handle.update(|tray| tray.view = view.clone());
        }
    }

    /// The app icon as ARGB32 pixmaps; the panel picks the size it needs.
    fn icons() -> Vec<Icon> {
        let sources: [&[u8]; 2] = [include_bytes!("../icons/32x32.png"), include_bytes!("../icons/128x128.png")];
        sources
            .iter()
            .filter_map(|bytes| Image::from_bytes(bytes).ok())
            .map(|image| {
                let mut data = image.rgba().to_vec();
                for pixel in data.as_chunks_mut::<4>().0 {
                    pixel.rotate_right(1);
                }
                Icon { width: image.width() as i32, height: image.height() as i32, data }
            })
            .collect()
    }

    impl ksni::Tray for PastaTray {
        fn id(&self) -> String {
            "pasta".into()
        }

        fn title(&self) -> String {
            "Pasta".into()
        }

        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icons.clone()
        }

        fn tool_tip(&self) -> ToolTip {
            ToolTip { title: "Pasta".into(), description: self.view.status.clone(), ..ToolTip::default() }
        }

        fn activate(&mut self, x: i32, y: i32) {
            // Wayland only lets a window take focus with the token the panel hands over.
            let token = ksni::current_xdg_activation_token().or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                ksni::current_xdg_activation_token()
            });
            let app = self.app.clone();
            let _ = self.app.run_on_main_thread(move || {
                crate::ui::toggle_tray_activated(&app, (f64::from(x), f64::from(y)), token.as_deref());
            });
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            entries(&self.view)
                .into_iter()
                .map(|entry| match entry {
                    Entry::Label(label) => StandardItem { label, enabled: false, ..StandardItem::default() }.into(),
                    Entry::Item(id, label) => StandardItem {
                        label: label.into(),
                        activate: Box::new(move |tray: &mut Self| on_menu(&tray.app, id)),
                        ..StandardItem::default()
                    }
                    .into(),
                    Entry::Check(id, label, checked) => CheckmarkItem {
                        label: label.into(),
                        checked,
                        activate: Box::new(move |tray: &mut Self| on_menu(&tray.app, id)),
                        ..CheckmarkItem::default()
                    }
                    .into(),
                    Entry::Separator => MenuItem::Separator,
                })
                .collect()
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The label entries, in order; separators and items are skipped.
    fn labels(view: &TrayView) -> Vec<String> {
        entries(view)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Label(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// The visible text of the item or check entry with this id, if present.
    fn item_text(view: &TrayView, id: &str) -> Option<&'static str> {
        entries(view).into_iter().find_map(|entry| match entry {
            Entry::Item(item_id, text) | Entry::Check(item_id, text, _) if item_id == id => Some(text),
            _ => None,
        })
    }

    /// The checked state of the check entry with this id, if present.
    fn checked(view: &TrayView, id: &str) -> Option<bool> {
        entries(view).into_iter().find_map(|entry| match entry {
            Entry::Check(item_id, _, on) if item_id == id => Some(on),
            _ => None,
        })
    }

    #[test]
    fn a_fresh_tray_shows_the_status_and_no_devices() {
        let view = TrayView { status: "Starting…".into(), ..TrayView::default() };
        assert_eq!(labels(&view).first().map(String::as_str), Some("Starting…"));
        assert!(labels(&view).iter().any(|label| label == "No devices yet"));
        assert!(item_text(&view, "retry").is_none());
        assert_eq!(item_text(&view, "settings"), Some("Settings…"));
        assert_eq!(checked(&view, "pause"), Some(false));
        assert_eq!(item_text(&view, "quit"), Some("Quit Pasta"));
    }

    #[test]
    fn warnings_and_peers_show_as_labels_and_devices_yet_disappears() {
        let view = TrayView {
            status: "Syncing".into(),
            warnings: vec!["Tailscale is reconnecting".into()],
            peers: vec!["● macbook-pro".into(), "○ win-workstation · offline".into()],
            ..TrayView::default()
        };
        let labels = labels(&view);
        assert_eq!(labels.first().map(String::as_str), Some("Syncing"));
        assert!(labels.iter().any(|label| label == "⚠ Tailscale is reconnecting"));
        assert!(labels.iter().any(|label| label == "● macbook-pro"));
        assert!(labels.iter().any(|label| label == "○ win-workstation · offline"));
        assert!(!labels.iter().any(|label| label == "No devices yet"));
    }

    #[test]
    fn retry_and_pause_states_show_as_entries() {
        let view = TrayView { retry: true, paused: true, ..TrayView::default() };
        assert_eq!(item_text(&view, "retry"), Some("Try the keychain again"));
        assert_eq!(checked(&view, "pause"), Some(true));
    }

    #[test]
    fn the_panel_entry_only_exists_on_macos() {
        let view = TrayView::default();
        assert_eq!(item_text(&view, "panel"), if cfg!(target_os = "macos") { Some("Open Clipboard") } else { None });
    }
}
