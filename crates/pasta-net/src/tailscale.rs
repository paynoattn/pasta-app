//! Client for the local Tailscale daemon, driven through the `tailscale` CLI's
//! `--json` output.
//!
//! The CLI is used instead of talking to the LocalAPI directly because it
//! already handles each platform's transport and auth (Windows named pipe,
//! macOS App Store vs standalone builds, Linux socket permissions). Calls take
//! roughly 35–55 ms, which is fine for periodic discovery and for a cached
//! WhoIs on each new connection. See `docs/adr/0002-tailscale-access.md`.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

/// Environment variable that overrides CLI discovery with an explicit path.
pub const CLI_PATH_ENV: &str = "PASTA_TAILSCALE_CLI";

/// Longer than the CLI's own retries (~5.2 s on Windows while the service starts),
/// so an unreachable daemon is reported as such rather than as a timeout.
const CLI_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(windows)]
const CLI_FILE_NAME: &str = "tailscale.exe";
#[cfg(not(windows))]
const CLI_FILE_NAME: &str = "tailscale";

/// Install locations checked after `PATH`, in order.
#[cfg(windows)]
const KNOWN_CLI_PATHS: &[&str] = &[r"C:\Program Files\Tailscale\tailscale.exe"];
#[cfg(target_os = "macos")]
const KNOWN_CLI_PATHS: &[&str] = &[
    // Both the App Store and standalone builds ship the CLI inside the bundle.
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/usr/local/bin/tailscale",
    "/opt/homebrew/bin/tailscale",
];
#[cfg(all(unix, not(target_os = "macos")))]
const KNOWN_CLI_PATHS: &[&str] = &["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/usr/sbin/tailscale"];

#[derive(Debug, thiserror::Error)]
pub enum TailscaleError {
    #[error("the Tailscale CLI was not found; is Tailscale installed?")]
    NotInstalled,
    #[error("the Tailscale service is not reachable; is Tailscale running? ({0})")]
    ServiceUnreachable(String),
    #[error("`tailscale {command}` failed: {stderr}")]
    Command { command: &'static str, stderr: String },
    #[error("`tailscale {command}` timed out")]
    Timeout { command: &'static str },
    #[error("unexpected output from `tailscale {command}`: {source}")]
    Parse {
        command: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to run the Tailscale CLI: {0}")]
    Io(#[from] std::io::Error),
}

/// Handle to the local Tailscale CLI.
#[derive(Debug, Clone)]
pub struct Tailscale {
    cli: PathBuf,
}

impl Tailscale {
    /// Finds the CLI: [`CLI_PATH_ENV`], then `PATH`, then known install locations.
    pub fn locate() -> Result<Self, TailscaleError> {
        if let Some(path) = std::env::var_os(CLI_PATH_ENV) {
            let path = PathBuf::from(path);
            return if path.is_file() { Ok(Self::with_cli(path)) } else { Err(TailscaleError::NotInstalled) };
        }
        let on_path = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .map(|dir| dir.join(CLI_FILE_NAME));
        on_path
            .chain(KNOWN_CLI_PATHS.iter().map(PathBuf::from))
            .find(|candidate| candidate.is_file())
            .map(Self::with_cli)
            .ok_or(TailscaleError::NotInstalled)
    }

    pub fn with_cli(cli: impl Into<PathBuf>) -> Self {
        Self { cli: cli.into() }
    }

    pub fn cli_path(&self) -> &Path {
        &self.cli
    }

    /// This node, its peers and the daemon's state (`tailscale status --json`).
    pub async fn status(&self) -> Result<Status, TailscaleError> {
        let out = self.run("status", ["status", "--json"]).await?;
        parse_status(&out)
    }

    /// Identifies the tailnet node behind `ip` (`tailscale whois --json`).
    ///
    /// Returns `Ok(None)` when the address is not a node on this tailnet.
    pub async fn whois(&self, ip: IpAddr) -> Result<Option<WhoIs>, TailscaleError> {
        let ip = ip.to_string();
        match self.run("whois", ["whois", "--json", ip.as_str()]).await {
            Ok(out) => parse_whois(&out).map(Some),
            Err(TailscaleError::Command { stderr, .. }) if stderr.contains("peer not found") => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn run<const N: usize>(&self, command: &'static str, args: [&str; N]) -> Result<Vec<u8>, TailscaleError> {
        let mut cmd = Command::new(&self.cli);
        cmd.args(args.iter().map(OsStr::new)).kill_on_drop(true);
        #[cfg(windows)]
        {
            // CREATE_NO_WINDOW: don't flash a console from the GUI app.
            cmd.creation_flags(0x0800_0000);
        }
        #[cfg(target_os = "macos")]
        if std::env::var_os("TERM").is_none() {
            // The App Store bundle binary only acts as the CLI when it sees a terminal
            // (`TERM`, `TERM_PROGRAM` or `SHLVL`). An app launched from Finder has none,
            // so it starts as the GUI instead and hangs or prints non-JSON.
            cmd.env("TERM", "dumb");
        }
        let output = tokio::time::timeout(CLI_TIMEOUT, cmd.output())
            .await
            .map_err(|_| TailscaleError::Timeout { command })??;
        if output.status.success() {
            return Ok(output.stdout);
        }
        // Some errors (e.g. whois "peer not found") are printed to stdout.
        let mut message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if message.is_empty() {
            message = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        }
        if is_unreachable(&message) {
            return Err(TailscaleError::ServiceUnreachable(message));
        }
        Err(TailscaleError::Command { command, stderr: message })
    }
}

/// True for CLI errors meaning the daemon couldn't be reached. `status` says
/// "failed to connect to local tailscaled …"; other commands print the raw
/// transport error, e.g. `Get "http://local-tailscaled.sock/…": open \\.\pipe\…`.
fn is_unreachable(message: &str) -> bool {
    message.contains("connect to local") || message.contains("local-tailscaled.sock")
}

/// State of the local `tailscaled` backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendState {
    Running,
    Starting,
    Stopped,
    NeedsLogin,
    NeedsMachineAuth,
    NoState,
    Other(String),
}

impl From<String> for BackendState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "Running" => Self::Running,
            "Starting" => Self::Starting,
            "Stopped" => Self::Stopped,
            "NeedsLogin" => Self::NeedsLogin,
            "NeedsMachineAuth" => Self::NeedsMachineAuth,
            "NoState" => Self::NoState,
            _ => Self::Other(value),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    pub backend_state: BackendState,
    /// This machine. Logged out, it is still present but has an empty
    /// `stable_id` and no IPs.
    pub self_node: Option<Node>,
    pub peers: Vec<Node>,
    pub magic_dns_suffix: String,
}

impl Status {
    /// True when the tailnet is up. A stopped backend still reports its last
    /// netmap (own IPs and peers), so check this before trusting either.
    pub fn is_running(&self) -> bool {
        self.backend_state == BackendState::Running
    }

    /// Peers that are online; empty unless the backend is running.
    pub fn online_peers(&self) -> impl Iterator<Item = &Node> {
        let running = self.is_running();
        self.peers.iter().filter(move |peer| running && peer.online)
    }
}

/// A machine on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Tailscale's stable node id (e.g. `nXXXXCNTRL`); survives IP changes.
    pub stable_id: String,
    /// Short device name: the first MagicDNS label, falling back to the OS hostname.
    pub name: String,
    pub dns_name: String,
    /// Hostname the OS reports. Unreliable for display: iOS reports `localhost`.
    pub host_name: String,
    pub os: String,
    /// Owner. All tagged devices share a single synthetic "tagged-devices" user.
    pub user_id: i64,
    pub ips: Vec<IpAddr>,
    pub online: bool,
    pub tags: Vec<String>,
}

/// Result of a WhoIs lookup for a tailnet address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoIs {
    pub stable_id: String,
    pub name: String,
    pub dns_name: String,
    pub user_id: i64,
    pub user_login: String,
    pub tags: Vec<String>,
    pub ips: Vec<IpAddr>,
}

/// True for addresses Tailscale assigns: 100.64.0.0/10 and fd7a:115c:a1e0::/48.
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 100 && (b & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => matches!(v6.segments(), [0xfd7a, 0x115c, 0xa1e0, ..]),
    }
}

fn short_name(dns_name: &str, host_name: &str) -> String {
    match dns_name.split('.').next() {
        Some(label) if !label.is_empty() => label.to_owned(),
        _ => host_name.to_owned(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawStatus {
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<RawPeer>,
    #[serde(default)]
    peer: Option<HashMap<String, RawPeer>>,
    #[serde(rename = "MagicDNSSuffix", default)]
    magic_dns_suffix: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawPeer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "OS", default)]
    os: String,
    #[serde(rename = "UserID", default)]
    user_id: i64,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Option<Vec<IpAddr>>,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

impl From<RawPeer> for Node {
    fn from(raw: RawPeer) -> Self {
        Self {
            name: short_name(&raw.dns_name, &raw.host_name),
            stable_id: raw.id,
            dns_name: raw.dns_name,
            host_name: raw.host_name,
            os: raw.os,
            user_id: raw.user_id,
            ips: raw.tailscale_ips.unwrap_or_default(),
            online: raw.online,
            tags: raw.tags.unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawWhoIs {
    node: RawWhoIsNode,
    user_profile: RawUserProfile,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawWhoIsNode {
    #[serde(rename = "StableID")]
    stable_id: String,
    name: String,
    #[serde(default)]
    computed_name: String,
    #[serde(default)]
    user: i64,
    #[serde(default)]
    addresses: Option<Vec<String>>,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawUserProfile {
    #[serde(default)]
    login_name: String,
}

fn parse_status(json: &[u8]) -> Result<Status, TailscaleError> {
    let raw: RawStatus =
        serde_json::from_slice(json).map_err(|source| TailscaleError::Parse { command: "status", source })?;
    let mut peers: Vec<Node> = raw.peer.unwrap_or_default().into_values().map(Node::from).collect();
    peers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Status {
        backend_state: raw.backend_state.into(),
        self_node: raw.self_node.map(Node::from),
        peers,
        magic_dns_suffix: raw.magic_dns_suffix,
    })
}

fn parse_whois(json: &[u8]) -> Result<WhoIs, TailscaleError> {
    let raw: RawWhoIs =
        serde_json::from_slice(json).map_err(|source| TailscaleError::Parse { command: "whois", source })?;
    let node = raw.node;
    let ips = node
        .addresses
        .unwrap_or_default()
        .iter()
        // Addresses come as single-host prefixes, e.g. "100.101.102.103/32".
        .filter_map(|cidr| cidr.split('/').next()?.parse().ok())
        .collect();
    let name = if node.computed_name.is_empty() { short_name(&node.name, "") } else { node.computed_name };
    Ok(WhoIs {
        stable_id: node.stable_id,
        name,
        dns_name: node.name,
        user_id: node.user,
        user_login: raw.user_profile.login_name,
        tags: node.tags.unwrap_or_default(),
        ips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &[u8] = include_bytes!("../tests/fixtures/status.json");
    const STATUS_LOGGED_OUT: &[u8] = include_bytes!("../tests/fixtures/status-logged-out.json");
    const WHOIS: &[u8] = include_bytes!("../tests/fixtures/whois.json");

    #[test]
    fn parses_status() {
        let status = parse_status(STATUS).unwrap();
        assert_eq!(status.backend_state, BackendState::Running);
        assert_eq!(status.magic_dns_suffix, "tail0000.ts.net");

        let me = status.self_node.as_ref().unwrap();
        assert_eq!(me.name, "win-workstation");
        assert_eq!(me.stable_id, "nSelf000CNTRL");
        assert_eq!(me.ips, vec!["100.101.4.12".parse::<IpAddr>().unwrap(), "fd7a:115c:a1e0::1".parse().unwrap()]);

        let names: Vec<_> = status.peers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["cachy-desktop", "iphone", "macbook-pro"]);
        let online: Vec<_> = status.online_peers().map(|p| p.name.as_str()).collect();
        assert_eq!(online, ["cachy-desktop", "macbook-pro"]);
    }

    #[test]
    fn device_name_prefers_magic_dns_over_hostname() {
        let status = parse_status(STATUS).unwrap();
        let iphone = status.peers.iter().find(|p| p.os == "iOS").unwrap();
        assert_eq!(iphone.host_name, "localhost");
        assert_eq!(iphone.name, "iphone");
    }

    #[test]
    fn tagged_peers_keep_their_tags() {
        let status = parse_status(STATUS).unwrap();
        let cachy = status.peers.iter().find(|p| p.name == "cachy-desktop").unwrap();
        assert_eq!(cachy.tags, ["tag:server"]);
        assert!(status.peers.iter().find(|p| p.name == "macbook-pro").unwrap().tags.is_empty());
    }

    #[test]
    fn parses_logged_out_status() {
        let status = parse_status(STATUS_LOGGED_OUT).unwrap();
        assert_eq!(status.backend_state, BackendState::NeedsLogin);
        assert!(status.peers.is_empty());
    }

    #[test]
    fn stopped_backend_has_no_online_peers() {
        let json = String::from_utf8(STATUS.to_vec()).unwrap().replace(r#""Running""#, r#""Stopped""#);
        let status = parse_status(json.as_bytes()).unwrap();
        assert!(!status.is_running());
        assert!(!status.peers.is_empty(), "stopped status still lists peers");
        assert_eq!(status.online_peers().count(), 0);
    }

    #[test]
    fn recognises_unreachable_daemon_errors() {
        // Messages captured on Windows with the service stopped or starting.
        assert!(is_unreachable("failed to connect to local tailscaled process; is the Tailscale service running?"));
        assert!(is_unreachable(
            r#"failed to connect to local tailscaled (which appears to be running as tailscaled.exe, pid 1234). Got error: Get "http://local-tailscaled.sock/localapi/v0/status": open \\.\pipe\x: The system cannot find the file specified."#
        ));
        assert!(is_unreachable(
            r#"Get "http://local-tailscaled.sock/localapi/v0/whois?proto=&addr=100.64.0.1": open \\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled: The system cannot find the file specified."#
        ));
        // Linux, with tailscaled not running and with its socket refusing connections.
        assert!(is_unreachable(
            "failed to connect to local tailscaled; it doesn't appear to be running (sudo systemctl start tailscaled ?)"
        ));
        assert!(is_unreachable(
            "Failed to connect to local Tailscale daemon for /localapi/v0/whois; not running? Error: dial unix /var/run/tailscale/tailscaled.sock: connect: connection refused"
        ));
        assert!(!is_unreachable("peer not found"));
    }

    #[test]
    fn parses_whois() {
        let whois = parse_whois(WHOIS).unwrap();
        assert_eq!(whois.stable_id, "nMac0000CNTRL");
        assert_eq!(whois.name, "macbook-pro");
        assert_eq!(whois.user_login, "user@example.com");
        assert_eq!(whois.ips, vec!["100.99.1.2".parse::<IpAddr>().unwrap(), "fd7a:115c:a1e0::2".parse().unwrap()]);
    }

    #[test]
    fn recognises_tailnet_addresses() {
        for ip in ["100.64.0.1", "100.127.255.254", "100.101.4.12", "fd7a:115c:a1e0::1"] {
            assert!(is_tailnet_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["100.63.255.255", "100.128.0.1", "192.168.1.10", "10.0.0.1", "fd7a:115c:a1e1::1", "::1"] {
            assert!(!is_tailnet_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn unknown_backend_state_is_preserved() {
        assert_eq!(BackendState::from("Weird".to_owned()), BackendState::Other("Weird".to_owned()));
    }
}
