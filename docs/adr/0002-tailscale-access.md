# 0002. Talk to Tailscale through its CLI's JSON output

- Status: Accepted (Windows and macOS verified; Linux to confirm on hardware)
- Date: 2026-10-23
- Issue: #4 (spike)

## Context

Pasta needs, on every OS: this node's tailnet addresses, the list of online peers, and
the identity of the node behind an incoming connection. Tailscale exposes this through
its LocalAPI, whose transport and auth differ per platform (Windows named pipe, macOS
App Store vs standalone builds, Linux socket permissions), and through the `tailscale`
CLI, which hides those differences.

## Findings

Measured on Windows 11 with Tailscale 1.102, running as a **non-admin** user with no
console window (the conditions of the real app):

- `tailscale status --json` works without elevation: **~30 ms** once warm. The first call
  after a while can take ~500 ms (process start-up), so don't put it on a UI-blocking path.
- `tailscale whois --json <ip>` works without elevation: **~25–35 ms**. Accepts `ip` or
  `ip:port`. A non-tailnet address exits 1 with `peer not found`.
- Verified both through `pasta_net::tailscale` (spawned with `CREATE_NO_WINDOW`) via
  `tailscale_probe`: names, OS, tags and online state match the CLI's own output.
- Useful `status` fields: `BackendState` (`Running`, `NeedsLogin`, `Stopped`, …),
  `Self`, `Peer` (map keyed by node key), and per node `ID` (stable node id), `DNSName`,
  `HostName`, `OS`, `UserID`, `TailscaleIPs`, `Online`, `Tags`.
- Useful `whois` fields: `Node.StableID`, `Node.Name`, `Node.ComputedName`,
  `Node.Addresses` (CIDR strings), `Node.Tags`, `UserProfile.LoginName`.
- **`HostName` is unreliable for display**: iOS reports `localhost`, macOS often `Mac`.
  The first MagicDNS label of `DNSName` is a better default device name.
- **Tailnets can be shared.** The test tailnet had three users plus tagged devices. All
  tagged devices share one synthetic `tagged-devices` user, so "same Tailscale user"
  can't be the trust rule; chain membership + approval is (ADR 0003).

macOS (Apple Silicon, same tailnet), run from a terminal via `tailscale_probe`:

- The probe found the CLI on `PATH` at `/opt/homebrew/bin/tailscale` (Homebrew).
- `status --json` took **~20 ms** and `whois --json` **~14 ms**, unelevated. Peer names,
  OS, tags and online state matched the Windows run.
- **An app launched from Finder doesn't inherit the shell's `PATH`** (it gets roughly
  `/usr/bin:/bin:/usr/sbin:/sbin`), so the `PATH` lookup that worked in the terminal won't
  find Homebrew's CLI in the real app. The known-path fallback below covers it.

## Decision

- Use the **CLI with `--json`**, implemented in `pasta_net::tailscale` (`status()`,
  `whois()`, `is_tailnet_ip()`), with a 5 s timeout and no console window on Windows.
- CLI discovery order: `PASTA_TAILSCALE_CLI` env var → `PATH` → known install paths:
  - Windows: `C:\Program Files\Tailscale\tailscale.exe`
  - macOS: `/Applications/Tailscale.app/Contents/MacOS/Tailscale` (App Store and
    standalone), `/usr/local/bin/tailscale`, `/opt/homebrew/bin/tailscale`
  - Linux: `/usr/bin/tailscale`, `/usr/local/bin/tailscale`, `/usr/sbin/tailscale`
- Poll `status` every few seconds for discovery (#10); cache WhoIs per peer address (#11).
- Device names default to the first `DNSName` label.

## To confirm on hardware

Run `cargo run -p pasta-net --example tailscale_probe` and record results here:

- [x] Windows 11: works unelevated (above).
- [x] macOS, Homebrew CLI (`/opt/homebrew/bin/tailscale`) from a terminal: works
      unelevated (above).
- [ ] macOS, CLI inside `Tailscale.app` (standalone and App Store builds) from a
      non-sandboxed, Developer ID–signed app launched from Finder. Re-check when the
      app shell exists (#17), since `PATH` differs there.
- [ ] CachyOS: `tailscale status` works for a normal user. If not, onboarding (#54)
      runs `sudo tailscale set --operator=$USER` once.
- [ ] Error text when Tailscale is stopped / logged out on each OS (for onboarding copy).
      Code treats any `connect to local …` message as "service unreachable".

## Consequences

- Pasta must not use the macOS App Sandbox (it couldn't exec the CLI). Fine for
  Developer ID distribution (#56).
- Spawning a process per call is acceptable at this rate. If it ever isn't, switch to a
  LocalAPI client behind the same `Tailscale` API.
