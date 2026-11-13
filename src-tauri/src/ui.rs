//! Pasta's windows (#28): Settings (`main`), the setup flow (`setup`), the
//! tray popover / flyout (`tray`, #42) and, on macOS, the clipboard panel
//! (`panel`, #38). One web bundle serves them all; `src/main.tsx` picks the
//! component tree by window label.

// The panel exists on macOS only so far.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use ts_rs::TS;

pub const MAIN: &str = "main";
pub const SETUP: &str = "setup";
pub const TRAY: &str = "tray";
pub const PANEL: &str = "panel";

/// Asks the Settings window to show a section (`devices`, `clipboard`, `chain`, …).
pub const NAVIGATE: &str = "navigate";
/// The panel was opened: the UI resets its selection and focuses the strip.
pub const PANEL_OPENED: &str = "panel-opened";
/// The tray popover was opened.
pub const TRAY_OPENED: &str = "tray-opened";

/// Points.
const PANEL_HEIGHT: f64 = 330.0;
/// A click on the tray icon right after the popover lost focus (to that click) closes it.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

static TRAY_HIDDEN_AT: Mutex<Option<Instant>> = Mutex::new(None);
const TRAY_SIZE: (f64, f64) = (360.0, 560.0);

/// Sent with [`PANEL_OPENED`].
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PanelContext {
    /// The app the panel will paste into, e.g. `Notes`.
    pub front_app: Option<String>,
    /// Whether ↩ can paste (Accessibility is granted), or only copy.
    pub can_paste: bool,
}

/// Creates the windows that must exist before they are first shown.
pub fn create_windows(app: &AppHandle) -> tauri::Result<()> {
    let tray = WebviewWindowBuilder::new(app, TRAY, WebviewUrl::App("index.html".into()))
        .title("Pasta")
        .inner_size(TRAY_SIZE.0, TRAY_SIZE.1)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .transparent(true)
        .shadow(true)
        .visible(false)
        .focused(false);
    let tray = tray.build()?;
    let handle = app.clone();
    tray.on_window_event(move |event| {
        // A popover closes when it loses focus, like the native ones.
        if let tauri::WindowEvent::Focused(false) = event
            && let Some(window) = handle.get_webview_window(TRAY)
        {
            *TRAY_HIDDEN_AT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
            let _ = window.hide();
        }
    });

    #[cfg(target_os = "macos")]
    {
        let panel = WebviewWindowBuilder::new(app, PANEL, WebviewUrl::App("index.html".into()))
            .title("Pasta clipboard")
            .inner_size(1200.0, PANEL_HEIGHT)
            .decorations(false)
            .resizable(false)
            .skip_taskbar(true)
            .always_on_top(true)
            .transparent(true)
            .shadow(false)
            .visible_on_all_workspaces(true)
            .visible(false)
            .focused(false)
            .build()?;
        crate::macos_panel::make_panel(&panel);
        let handle = app.clone();
        panel.on_window_event(move |event| {
            if let tauri::WindowEvent::Focused(false) = event {
                hide_panel(&handle);
            }
        });
    }
    Ok(())
}

/// Shows Settings, optionally at one section.
pub fn open_settings(app: &AppHandle, section: Option<&str>) {
    hide_tray(app);
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        if let Some(section) = section {
            let _ = window.emit(NAVIGATE, section);
        }
    }
}

/// Shows the setup flow (#43), creating its window the first time.
pub fn open_setup(app: &AppHandle) {
    hide_tray(app);
    let window = match app.get_webview_window(SETUP) {
        Some(window) => window,
        None => {
            let built = WebviewWindowBuilder::new(app, SETUP, WebviewUrl::App("index.html".into()))
                .title("Set up Pasta")
                .inner_size(760.0, 560.0)
                .min_inner_size(640.0, 520.0)
                .center()
                .visible(false)
                .build();
            match built {
                Ok(window) => window,
                Err(err) => return tracing::warn!(error = %err, "couldn't open the setup window"),
            }
        }
    };
    let _ = window.show();
    let _ = window.set_focus();
}

/// Closes the setup flow for good (it was finished or skipped).
pub fn close_setup(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETUP) {
        let _ = window.destroy();
    }
}

pub fn hide_tray(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(TRAY) {
        let _ = window.hide();
    }
}

/// Opens or closes the popover next to the tray icon at `anchor` (physical pixels: x, y, width, height).
pub fn toggle_tray(app: &AppHandle, anchor: Option<(f64, f64, f64, f64)>) {
    let Some(window) = app.get_webview_window(TRAY) else { return };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let just_hidden = TRAY_HIDDEN_AT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .is_some_and(|at| at.elapsed() < REOPEN_GUARD);
    if just_hidden {
        return;
    }
    if let Some(anchor) = anchor {
        place_tray(app, &window, anchor);
    }
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit(TRAY_OPENED, ());
}

/// KDE: the tray icon was clicked at `(x, y)`. `token` is the panel's XDG
/// activation token, without which KWin doesn't give the flyout focus.
#[cfg(target_os = "linux")]
pub fn toggle_tray_activated(app: &AppHandle, (x, y): (f64, f64), token: Option<&str>) {
    use gtk::prelude::GtkWindowExt;
    if let (Some(token), Some(window)) = (token, app.get_webview_window(TRAY))
        && let Ok(gtk_window) = window.gtk_window()
    {
        gtk_window.set_startup_id(token);
    }
    toggle_tray(app, Some((x, y, 0.0, 0.0)));
}

/// Below the menu-bar icon on macOS; above (or beside) the taskbar icon elsewhere.
/// Wayland doesn't let windows place themselves, so on KDE KWin decides.
fn place_tray(app: &AppHandle, window: &WebviewWindow, (x, y, width, height): (f64, f64, f64, f64)) {
    let monitor = app.monitor_from_point(x, y).ok().flatten().or_else(|| app.primary_monitor().ok().flatten());
    let scale = monitor.as_ref().map_or(1.0, tauri::Monitor::scale_factor);
    let (popover_w, popover_h) = (TRAY_SIZE.0 * scale, TRAY_SIZE.1 * scale);
    let mut left = x + width / 2.0 - popover_w / 2.0;
    let mut top = if cfg!(target_os = "macos") { y + height + 4.0 * scale } else { y - popover_h - 8.0 * scale };
    if let Some(monitor) = &monitor {
        let area = monitor.work_area();
        let (area_x, area_y) = (f64::from(area.position.x), f64::from(area.position.y));
        let (area_w, area_h) = (f64::from(area.size.width), f64::from(area.size.height));
        left = left.clamp(area_x + 8.0 * scale, (area_x + area_w - popover_w - 8.0 * scale).max(area_x));
        top = top.clamp(area_y, (area_y + area_h - popover_h).max(area_y));
    }
    let _ = window.set_size(PhysicalSize::new(popover_w as u32, popover_h as u32));
    let _ = window.set_position(PhysicalPosition::new(left as i32, top as i32));
}

#[cfg(target_os = "macos")]
static PANEL_FRONT: std::sync::Mutex<Option<pasta_clipboard::mac_native::FrontApp>> = std::sync::Mutex::new(None);

/// The app that was in front when the panel opened (#39).
#[cfg(target_os = "macos")]
pub fn panel_front_app(_app: &AppHandle) -> Option<pasta_clipboard::mac_native::FrontApp> {
    PANEL_FRONT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The global shortcut (#38): opens the panel on the screen with the pointer, or closes it.
pub fn toggle_panel(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let Some(window) = app.get_webview_window(PANEL) else { return };
        if window.is_visible().unwrap_or(false) {
            hide_panel(app);
            return;
        }
        hide_tray(app);
        let front = pasta_clipboard::mac_native::frontmost_app();
        let context = PanelContext {
            front_app: front.as_ref().map(|app| app.name.clone()).filter(|name| !name.is_empty()),
            can_paste: pasta_clipboard::mac_native::accessibility_trusted(false),
        };
        *PANEL_FRONT.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = front;
        place_panel(app, &window);
        crate::macos_panel::show(&window);
        let _ = window.emit(PANEL_OPENED, context);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
fn place_panel(app: &AppHandle, window: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|point| app.monitor_from_point(point.x, point.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let area = *monitor.work_area();
    let height = (PANEL_HEIGHT * monitor.scale_factor()) as u32;
    let top = area.position.y + i32::try_from(area.size.height.saturating_sub(height)).unwrap_or_default();
    let _ = window.set_size(PhysicalSize::new(area.size.width, height));
    let _ = window.set_position(PhysicalPosition::new(area.position.x, top));
}

pub fn hide_panel(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window(PANEL) {
        crate::macos_panel::hide(&window);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}
