# 0002. Talk to Tailscale through its CLI's JSON output

- Status: Accepted (Windows, macOS and Linux verified)
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
  not on the peer list. `Status::online_peers()` is empty unless `is_running()`.
- The `whois` errors don't contain `connect to local`; any message naming
  `local-tailscaled.sock` is also mapped to `ServiceUnreachable`.
- The ~5.2 s retry exceeded the original 5 s CLI timeout (surfacing as `Timeout`), so the
  timeout is now 10 s.

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

macOS 27.2 (Apple Silicon, same tailnet), unelevated, with all three ways of installing
Tailscale: Homebrew's open-source `tailscaled` (1.104.1), the Mac App Store app (1.104.2)
and the standalone Developer ID app from pkgs.tailscale.com (1.104.2):

- From a terminal, `status --json` and `whois --json` take **~25–35 ms** through the CLI
  inside `Tailscale.app` (both builds), and ~15–80 ms through Homebrew's CLI. Peer names,
  OS, tags and online state matched the Windows run.
- **The `Tailscale` binary inside `Tailscale.app` is the GUI and the CLI in one.** It
  runs as the CLI only if `TERM`, `TERM_PROGRAM` or `SHLVL` is set (any value). Each
  variable was tested on its own with `env -i`. Without them it starts as the GUI. It
  either prints `The Tailscale GUI failed to start: … (Tailscale.CLIError error 3.)` and
  exits **0**, or it hangs. Both builds behave the same way.
- **An app launched from Finder has none of those variables**, and its `PATH` is
  `/usr/bin:/bin:/usr/sbin:/sbin`, so discovery falls through to the bundle CLI. This was
  measured with the probe as the executable of an ad-hoc-signed `.app` that Finder opened
  through AppleScript. `open` from a terminal isn't a fair test: it passes the shell's
  environment through, including `TERM`, `SHLVL` and Homebrew's `PATH`. Before the fix,
  the Finder launch failed with `tailscale status timed out` on the App Store build and
  with a JSON parse error on the standalone build. After it, `status` and `whois` work on
  both (~50 ms and ~25–30 ms).
- Fix: on macOS, `pasta_net::tailscale` sets `TERM=dumb` on the CLI process when `TERM`
  is unset. The app isn't sandboxed, so running another app's binary works.
- How the CLI reaches the daemon (`safesocket` in the open-source CLI):
  - App Store: the network extension (`IPNExtension`, running as the user) listens on a
    random `127.0.0.1` port. The token is in the name of a `sameuserproof-<port>-<token>`
    file inside its group container. The CLI finds it with `lsof -c IPNExtension` because
    TCC blocks other apps from reading that container.
  - Standalone: a system extension running as root. `/Library/Tailscale/ipnport` is a
    symlink to the port. The token is in `/Library/Tailscale/sameuserproof-<port>`, owned
    by `root:admin` with mode `0640`, so only admin users can read it. A standard
    (non-admin) user wasn't tested.
  - Homebrew: the Unix socket `/var/run/tailscaled.socket` (`root:daemon`, `0666`). The
    CLI tries a GUI build's LocalAPI first, so with Homebrew and the App Store app both
    installed, Homebrew's CLI talked to the App Store daemon (and printed a version
    mismatch warning on stderr).
  - A direct LocalAPI client would have to copy that `lsof` and token-file lookup. That
    is more fragile than the CLI, which handles it.
- Unprivileged `tailscale down` / `up` worked on every build (Homebrew had no operator
  set).
- Quitting `Tailscale.app` through AppleScript doesn't stop it. It relaunched within a
  second, and the network extension kept serving the LocalAPI. `scutil --nc stop` on the
  VPN configuration also reconnected straight away.

macOS failure modes (CLI inside `Tailscale.app`, `TERM` set), for onboarding copy and
error mapping:

| State                                                                   | `status --json`                                                                                                                                                                            | `whois --json <ip>`                                                                                        |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------- |
| Logged out (App Store, fresh install)                                   | exit 0, ~30 ms, `NeedsLogin`; `Self` present with empty `ID` and `DNSName` and no IPs; `Peer: null`; `Health: ["Tailscale is stopped."]`                                                   | not captured                                                                                               |
| Disconnected (`tailscale down`), both builds and Homebrew               | exit 0, ~35 ms, `Stopped`; `Self` (with IPs) and the full `Peer` map still present; `Health: ["Tailscale is stopped."]`                                                                    | known peer still resolves; unknown IP exits 1 with `<date> <time> peer not found` (log-prefixed on stderr) |
| Standalone, system extension installed but not approved by the user     | **hangs with no output** (killed at 15 s); `systemextensionsctl` shows `[activated waiting for user]`                                                                                      | not captured                                                                                               |
| Homebrew daemon, socket missing or refusing (simulated with `--socket`) | exit 1 after **~5.1 s**: `failed to connect to local tailscaled (which appears to be running as tailscaled, pid …). Got error: Failed to connect to local Tailscale daemon … not running?` | exit 1 after ~5.1 s: `Failed to connect to local Tailscale daemon for /localapi/v0/whois; not running? …`  |

- `peer not found` is still matched, since `whois()` checks with `contains`.
- The unapproved extension is the likely state right after someone installs the
  standalone app and dismisses the prompt. Pasta sees it as `Timeout` after 10 s, not
  `ServiceUnreachable`, so onboarding (#54) should read a macOS `status` timeout as
  "open Tailscale and allow its system extension".
- Not captured: logged out on the standalone build, or the VPN switched off by the user
  in System Settings while the extension stays installed.

Linux (CachyOS, Tailscale 1.104.2 from the distro package, `tailscaled` under systemd),
run as a normal user via `tailscale_probe`:

- The CLI is `/usr/bin/tailscale`, found on `PATH` and also through the known-path
  fallback with `PATH` unset.
- `status --json` took **~4 ms** and `whois --json` **~3 ms**. Linux doesn't have the
  slower first call seen on Windows.
- The socket is `/run/tailscale/tailscaled.sock` (also reachable as `/var/run/…`), owned by
  `root:root` with mode `0666`, so any local user can connect.
- tailscaled lets **every** Unix-socket caller read (`status`, `whois`). Writes (`up`,
  `down`, `set`, …) need root, the operator user or tailscaled's own UID
  (`actor.Permissions` in `ipn/ipnserver`, 1.104.2). Pasta only reads, so it **needs no
  `--operator` setup**. The test machine already had an operator set, so the non-operator
  case comes from the source and the socket mode. It wasn't run as a second user.

Linux failure modes (Tailscale 1.104.2, normal user):

| State                                                                         | `status --json`                                                                                                                                                                            | `whois --json <ip>`                                                                                       |
| ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| Logged out (`NeedsLogin`, fresh state)                                        | exit 0, ~5 ms; `Self` present with empty `ID` and `DNSName` and no IPs; `Peer: null`; `Health: ["Tailscale is stopped."]`                                                                  | exit 1, ~3 ms: `peer not found`                                                                           |
| Daemon not running (no `tailscaled` process, no socket)                       | exit 1 after **~2.0 s**: `failed to connect to local tailscaled; it doesn't appear to be running (sudo systemctl start tailscaled ?)`                                                      | exit 1 after ~2.0 s: `Failed to connect to local Tailscale daemon for /localapi/v0/whois; not running? …` |
| Socket missing or refusing while a `tailscaled` process runs (starting/stale) | exit 1 after **~5.1 s**: `failed to connect to local tailscaled (which appears to be running as tailscaled, pid …). Got error: Failed to connect to local Tailscale daemon … not running?` | exit 1 after ~5.1 s: `Failed to connect to local Tailscale daemon for /localapi/v0/whois; not running? …` |

- How each state was produced, without touching the real tailnet: logged out was a separate
  `tailscaled --tun=userspace-networking` with an empty state dir and its own `--socket`.
  The other two rows point `--socket` at a missing path, or at a stale socket with no
  listener, with the real daemon either visible or hidden in a PID namespace.
- Every message contains `connect to local`, so the existing mapping reports
  `ServiceUnreachable`. The 5.1 s retry fits inside the 10 s timeout.
- Not captured: disconnected (`tailscale down` on a logged-in node), because it would take
  this machine off the shared tailnet. It's the same backend code as Windows, so expect
  `Stopped` with the cached netmap. `is_running()` gates it either way.

## Decision

- Use the **CLI with `--json`**, implemented in `pasta_net::tailscale` (`status()`,
  `whois()`, `is_tailnet_ip()`), with a 10 s timeout, no console window on Windows, and
  `TERM=dumb` added on macOS when `TERM` is unset (so the `Tailscale.app` binary runs as
  the CLI).
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
- [x] macOS, CLI inside `Tailscale.app` (standalone and App Store builds) from a
      non-sandboxed app launched from Finder: works once `TERM` is set (above). Tested
      ad-hoc signed. Re-check with the Developer ID–signed app shell (#17).
- [x] CachyOS: `status` and `whois` work for a normal user (above). Reads don't need
      the operator, so onboarding (#54) needs no `sudo tailscale set --operator=$USER`
      step unless Pasta starts changing Tailscale settings.
- [x] Windows: error text when Tailscale is disconnected or its service is stopped
      (table above). LocalAPI named pipe works unelevated (above).
- [ ] Windows: logged out (`NeedsLogin`) output; not captured, since logging out needs a
      browser re-auth.
- [x] CachyOS: error text when logged out or the daemon is unreachable (table above).
- [ ] CachyOS: disconnected (`tailscale down`) output; not captured (above).
- [x] macOS: output when logged out, disconnected or with the extension unapproved
      (table above).
- [ ] macOS: a standard (non-admin) user with the standalone build, which can't read
      the `root:admin` token file.

## Consequences

- Pasta must not use the macOS App Sandbox (it couldn't exec the CLI). Fine for
  Developer ID distribution (#56).
- Pasta depends on an undocumented heuristic in the `Tailscale.app` binary (CLI mode when
  `TERM` is set). If a Tailscale update changes it, Finder-launched Pasta breaks while
  terminal runs keep working. Test the Finder launch when Tailscale updates.
- Spawning a process per call is acceptable at this rate. If it ever isn't, switch to a
  LocalAPI client behind the same `Tailscale` API. On Windows that is cheap: the named
  pipe works unelevated, needs no token, and answers in under 1 ms vs ~30 ms for the CLI.
- A stopped or starting service costs 2–5 s per call before Pasta knows, on Windows and
  Linux alike. If that hurts onboarding, check that the pipe or socket exists first and
  fail fast.
