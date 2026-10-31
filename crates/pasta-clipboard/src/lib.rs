//! OS clipboard adapters for Pasta.
//!
//! Each platform gets an adapter that can watch for local copies, write
//! received items into the native clipboard / clipboard history, and expire
//! them again: Windows (#12, #14) and macOS (#13, #15) here, KDE Plasma in
//! M4. [`system`] picks the one for the running OS. The spike probes that
//! informed them live in `examples/`.
//!
//! See `docs/adr/0004-windows-clipboard-history.md` and
//! `docs/adr/0005-macos-pasteboard.md`.

use std::sync::Arc;

use pasta_core::clipboard::{ClipboardAdapter, ClipboardError};

#[cfg(any(windows, test))]
mod cf_html;
#[cfg(target_os = "macos")]
mod macos;
pub mod sensitive;
#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;
#[cfg(windows)]
mod windows;

/// The clipboard adapter for this OS.
pub fn system() -> Result<Arc<dyn ClipboardAdapter>, ClipboardError> {
    #[cfg(windows)]
    return Ok(Arc::new(windows::WindowsClipboard::new()?));
    #[cfg(target_os = "macos")]
    return Ok(Arc::new(macos::MacClipboard::new()));
    #[cfg(not(any(windows, target_os = "macos")))]
    return Ok(Arc::new(unsupported::Unsupported));
}
