//! OS clipboard adapters for Pasta.
//!
//! Each platform gets an adapter that can watch for local copies, write
//! received items into the native clipboard / clipboard history, and expire
//! them again: Windows (#12, #14), macOS (#13, #15) and KDE Plasma on
//! Wayland (#50–#52). [`system`] picks the one for the running OS. The spike
//! probes that informed them live in `examples/`.
//!
//! See `docs/adr/0004-windows-clipboard-history.md`,
//! `docs/adr/0005-macos-pasteboard.md` and `docs/adr/0015-kde-klipper.md`.

use std::sync::Arc;

use pasta_core::clipboard::{ClipboardAdapter, ClipboardError};

#[cfg(any(windows, test))]
mod cf_html;
#[cfg(target_os = "linux")]
pub mod kde;
#[cfg(target_os = "macos")]
pub mod mac_native;
#[cfg(target_os = "macos")]
mod macos;
pub mod sensitive;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

/// Apps with a window open now, for the excluded-apps picker (#36): bundle
/// ids on macOS, executable names on Windows. Sorted by name, without Pasta.
/// Empty on Linux, where Wayland doesn't tell other apps who copied (ADR 0015).
pub fn running_apps() -> Vec<pasta_core::clip::SourceApp> {
    #[cfg(target_os = "macos")]
    let mut apps = macos::running_apps();
    #[cfg(windows)]
    let mut apps = windows::running_apps();
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut apps: Vec<pasta_core::clip::SourceApp> = Vec::new();
    apps.retain(|app| !app.id.is_empty());
    apps.sort_by_key(|app| app.name.to_lowercase());
    apps.dedup_by(|a, b| a.id.eq_ignore_ascii_case(&b.id));
    apps
}

/// The clipboard adapter for this OS. On Linux without Wayland
/// `ext-data-control` (an X11 session, GNOME) it reports why it can't sync.
pub fn system() -> Result<Arc<dyn ClipboardAdapter>, ClipboardError> {
    #[cfg(windows)]
    return Ok(Arc::new(windows::WindowsClipboard::new()?));
    #[cfg(target_os = "macos")]
    return Ok(Arc::new(macos::MacClipboard::new()));
    #[cfg(target_os = "linux")]
    return match kde::KdeClipboard::new() {
        Ok(adapter) => Ok(Arc::new(adapter)),
        Err(err) => {
            tracing::warn!(error = %err, "no clipboard access on this desktop");
            Ok(Arc::new(unsupported::Unsupported::new(unsupported_reason())))
        }
    };
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    return Ok(Arc::new(unsupported::Unsupported::new("Pasta doesn't support this OS yet".into())));
}

#[cfg(target_os = "linux")]
fn unsupported_reason() -> String {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        "Pasta needs a Plasma Wayland session; X11 isn't supported".into()
    } else {
        "This desktop doesn't let Pasta watch the clipboard (needs KDE Plasma 6)".into()
    }
}
