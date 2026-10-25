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

Windows failure modes (Tailscale 1.102.2, non-admin), for onboarding copy and error mapping:

| State                                                                                       | `status --json`                                                                                                         | `whois --json <ip>`                                                                                                               |
| ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Disconnected (`tailscale down`)                                                             | exit 0, ~26 ms, `BackendState: "Stopped"`; **`Self` (with IPs) and the full `Peer` map are still present**              | known peer still resolves (cached netmap); unknown IP: `peer not found`                                                           |
| Service stopped (no `tailscaled.exe`, no pipe)                                              | exit 1 after **~2 s** of retries: `failed to connect to local tailscaled process; is the Tailscale service running?`    | exit 1 after ~2 s: `Get "http://local-tailscaled.sock/…": open \\.\pipe\…\tailscaled: The system cannot find the file specified.` |
| Pipe missing while `tailscaled.exe` runs (e.g. service starting; simulated with `--socket`) | exit 1 after **~5.2 s**: `failed to connect to local tailscaled (which appears to be running as tailscaled.exe, pid …)` | exit 1 after ~5.2 s, same `open \\.\pipe\…` text                                                                                  |

- A stopped tailnet still reports peers, so callers must gate on `BackendState == Running`,
  not on the peer list.
- The `whois` errors don't contain `connect to local`, so `pasta_net::tailscale` currently
  maps them to `Command`, not `ServiceUnreachable`.
- The ~5.2 s retry exceeds the 5 s `CLI_TIMEOUT`, so that case surfaces as `Timeout`.

Direct LocalAPI over the named pipe, from a non-admin process:

- Pipe: `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled`. A non-admin user
  can open it, and `GET /localapi/v0/status` (`Host: local-tailscaled.sock`) returns 200 in
  under 1 ms after connect.
- tailscaled identifies the caller by impersonating the pipe client, so the client must
  connect with impersonation level **Identification** or higher. .NET's default level gets
  `401 authentication failed: Unable to impersonate using a named pipe until data has been
read from that pipe`; Anonymous gets `401 … Cannot open an anonymous level security
token`. tokio's `ClientOptions` defaults to `SECURITY_IDENTIFICATION`, so a Rust client
  would work as-is.
- `whois?addr=<ip>:0` for a non-tailnet address returns `404 no match for IP:port`.
- Not tested: a second Windows user on the same PC. tailscaled serves one Windows user at
  a time (unless unattended mode is on), so Pasta running in another session should
  expect the LocalAPI or CLI to refuse it.

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
- [x] Windows: error text when Tailscale is disconnected or its service is stopped
      (table above). LocalAPI named pipe works unelevated (above).
- [ ] Windows: logged out (`NeedsLogin`) output; not captured, since logging out needs a
      browser re-auth.
- [ ] macOS / CachyOS: error text when Tailscale is stopped / logged out (for onboarding
      copy). Code treats any `connect to local …` message as "service unreachable".

## Consequences

- Pasta must not use the macOS App Sandbox (it couldn't exec the CLI). Fine for
  Developer ID distribution (#56).
- Spawning a process per call is acceptable at this rate. If it ever isn't, switch to a
  LocalAPI client behind the same `Tailscale` API. On Windows that is cheap: the named
  pipe works unelevated, needs no token, and answers in under 1 ms vs ~30 ms for the CLI.
- Error mapping must also treat `open \\.\pipe\…` and `cannot find the file specified` as
  "service unreachable", and the CLI timeout must be longer than the CLI's own ~5 s retry
  (or Pasta should check the pipe exists first and fail fast).
