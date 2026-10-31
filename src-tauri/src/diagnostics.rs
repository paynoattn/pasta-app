//! "Copy diagnostics" for bug reports (#18).
//!
//! Reports end up in public issues, so this lists versions, states and counts
//! but no device names, tailnet addresses, node ids or clipboard content.

use std::fmt::Write;
use std::path::Path;

use pasta_core::device::UnixMillis;
use pasta_net::discovery::TailnetState;

use crate::runtime::Runtime;

pub fn report(runtime: &Runtime) -> String {
    let config = runtime.config.get();
    let device = &runtime.device;
    let status = runtime.net_status();
    let peers = runtime.peers();
    let clipboard = runtime.clipboard_status();
    let now = UnixMillis::now().0;

    let mut out = String::new();
    let _ = writeln!(out, "Pasta {} · protocol v{}", device.app_version, pasta_core::PROTOCOL_VERSION);
    let _ = writeln!(out, "OS: {} {} ({})", device.os, device.os_version, std::env::consts::ARCH);
    let _ =
        writeln!(out, "Device: {} · mode {:?} · port {}", short_id(&device.id.to_string()), config.mode, config.port);
    let tailnet = match &status.tailnet {
        TailnetState::Starting => "starting".to_owned(),
        TailnetState::Running => "running".to_owned(),
        TailnetState::NotRunning(state) => format!("not running ({state})"),
        // Error text can name paths but not tailnet details.
        TailnetState::Unavailable(error) => format!("unavailable ({error})"),
    };
    let _ = writeln!(out, "Tailscale: {tailnet}");
    let _ = writeln!(out, "Listening on: {} address(es)", status.listening.len());
    for error in &status.listen_errors {
        // "port N is already in use on <ip>": keep the port, drop the address.
        let _ = writeln!(out, "  listen error: {}", error.split(" on ").next().unwrap_or(error));
    }
    let history = match clipboard.history_enabled {
        Some(true) => "on",
        Some(false) => "OFF",
        None => "n/a",
    };
    let _ = writeln!(out, "Clipboard history: {history}");
    if let Some(reason) = &clipboard.unsupported {
        let _ = writeln!(out, "Clipboard: {reason}");
    }
    let connected = peers.iter().filter(|peer| peer.connected).count();
    let _ = writeln!(out, "Peers: {} known, {connected} connected", peers.len());
    for peer in &peers {
        let state = match (peer.connected, peer.latency) {
            (true, Some(latency)) => format!("connected, {} ms", latency.as_millis()),
            (true, None) => "connected".to_owned(),
            (false, _) => format!("offline, last seen {}", ago(now.saturating_sub(peer.last_seen.0))),
        };
        let _ = writeln!(out, "  {} {}: {state}", short_id(&peer.device_id.to_string()), peer.os);
    }
    if cfg!(target_os = "macos") {
        let _ = writeln!(out, "History items: {}", runtime.history_len());
    }
    let _ = writeln!(out, "Config: {}", home_relative(runtime.config.path()));
    let _ = writeln!(out, "Logs: {}", home_relative(&runtime.log_dir));
    out
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn ago(millis: u64) -> String {
    let secs = millis / 1000;
    match secs {
        0..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}

/// Replaces the home directory (which contains the user name) with `~`.
fn home_relative(path: &Path) -> String {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_ages() {
        assert_eq!(ago(5_000), "5s ago");
        assert_eq!(ago(120_000), "2m ago");
        assert_eq!(ago(7_200_000), "2h ago");
        assert_eq!(ago(172_800_000), "2d ago");
    }

    #[test]
    fn hides_the_home_directory() {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).unwrap();
        let path = Path::new(&home).join("logs");
        assert_eq!(home_relative(&path), format!("~{}logs", std::path::MAIN_SEPARATOR));
    }
}
