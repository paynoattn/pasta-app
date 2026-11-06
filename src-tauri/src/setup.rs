//! The setup flow's checks (#43–#45) and the system settings it links to.

use pasta_net::discovery::TailnetState;
use serde::Deserialize;
use ts_rs::TS;

use crate::runtime::Runtime;
use crate::views::{SetupView, TailscaleView};

/// A system settings page or download the setup flow can open. A fixed list,
/// so the UI can't make Pasta open arbitrary URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SettingsLink {
    /// macOS › Privacy & Security › Accessibility.
    Accessibility,
    /// macOS › Keyboard › Keyboard Shortcuts.
    Keyboard,
    /// Windows › System › Clipboard.
    Clipboard,
    /// Windows › Privacy & security › Windows Security › Firewall.
    Firewall,
    /// Opens the Tailscale app.
    TailscaleApp,
    /// tailscale.com/download.
    TailscaleDownload,
}

impl SettingsLink {
    fn target(self) -> Option<&'static str> {
        let target = match (self, std::env::consts::OS) {
            (Self::Accessibility, "macos") => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            (Self::Keyboard, "macos") => "x-apple.systempreferences:com.apple.Keyboard-Settings.extension",
            (Self::Clipboard, "windows") => "ms-settings:clipboard",
            (Self::Firewall, "windows") => "windowsdefender://network",
            (Self::TailscaleApp, "macos") => "/Applications/Tailscale.app",
            (Self::TailscaleDownload, _) => "https://tailscale.com/download",
            _ => return None,
        };
        Some(target)
    }
}

/// Opens one of the fixed [`SettingsLink`]s with the OS.
pub fn open_link(link: SettingsLink) -> Result<(), String> {
    let target = link.target().ok_or("that setting doesn't exist on this OS")?;
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        // `start` needs an empty title before a target with a colon.
        command.args(["/C", "start", ""]);
        command
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    let mut child = command.arg(target).spawn().map_err(|err| format!("couldn't open it: {err}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

impl Runtime {
    pub fn setup_view(&self) -> SetupView {
        let config = self.config.get();
        let status = self.net_status();
        let tailscale = match &status.tailnet {
            TailnetState::Starting => TailscaleView { state: "starting", detail: None, address: None },
            TailnetState::Unavailable(detail) => {
                TailscaleView { state: "missing", detail: Some(detail.clone()), address: None }
            }
            TailnetState::NotRunning(detail) => {
                TailscaleView { state: "stopped", detail: Some(detail.clone()), address: None }
            }
            TailnetState::Running => TailscaleView {
                state: "running",
                detail: None,
                address: status.listening.first().map(|addr| addr.ip().to_string()),
            },
        };
        let clipboard = self.clipboard_status();
        SetupView {
            os: std::env::consts::OS.to_owned(),
            tailscale,
            chain: self.chain_view().state,
            accessibility: accessibility(),
            clipboard_history: clipboard.history_enabled,
            cloud_sync: clipboard.cloud_sync,
            open_at_login: crate::startup::open_at_login(&self.app),
            auto_update: config.updates.automatic,
            icloud_bridge: config.icloud_bridge,
            panel_shortcut: crate::shortcuts::supported()
                .then(|| config.panel_shortcut.clone().unwrap_or_else(|| crate::shortcuts::DEFAULT_PANEL.to_owned())),
            setup_done: config.setup_done,
        }
    }
}

#[cfg(target_os = "macos")]
fn accessibility() -> Option<bool> {
    Some(pasta_clipboard::mac_native::accessibility_trusted(false))
}

#[cfg(not(target_os = "macos"))]
fn accessibility() -> Option<bool> {
    None
}

/// Asks macOS to show its Accessibility prompt (once per app); elsewhere a no-op.
pub fn request_accessibility() -> Option<bool> {
    #[cfg(target_os = "macos")]
    return Some(pasta_clipboard::mac_native::accessibility_trusted(true));
    #[cfg(not(target_os = "macos"))]
    None
}
