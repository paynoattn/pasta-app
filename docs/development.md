# Development

## Prerequisites

| Tool                   | Notes                                                                                                                      |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| Rust (stable)          | via [rustup](https://rustup.rs)                                                                                            |
| Node 24 + pnpm         | pnpm comes via corepack: `corepack enable` (or prefix commands with `corepack pnpm`). Version is pinned in `package.json`. |
| Tauri prerequisites    | <https://tauri.app/start/prerequisites/>                                                                                   |
| Windows                | Visual Studio Build Tools with **Desktop development with C++**, and WebView2 (preinstalled on Windows 11)                 |
| macOS                  | Xcode Command Line Tools. **The macOS build and the Mac spikes need a real Mac.**                                          |
| Linux (CachyOS / Arch) | `webkit2gtk-4.1 libappindicator-gtk3 librsvg patchelf xdotool base-devel`                                                  |
| Tailscale              | Installed and logged in on every test machine, all on the same tailnet                                                     |

Install the Windows build tools from a terminal (approve the UAC prompt):

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

## Everyday commands

```sh
pnpm install
pnpm tauri dev        # run the app
pnpm check            # everything CI runs: typecheck, lint, tests, rustfmt, clippy, cargo test
pnpm format           # prettier --write
cargo fmt --all
```

## Layout

See [ARCHITECTURE.md](ARCHITECTURE.md). Rust crates live in `crates/`, the Tauri app in
`src-tauri/`, the React UI in `src/`. There is one Cargo workspace at the repo root, so
build output goes to `/target`.

## Spike probes

These print diagnostics only, never clipboard contents:

```sh
cargo run -p pasta-net --example tailscale_probe                      # any OS (#4)
cargo run -p pasta-clipboard --example win_history_probe              # Windows (#5)
cargo run -p pasta-clipboard --example mac_pasteboard_probe -- watch  # macOS (#6)
```

`win_history_probe` writes test markers to the clipboard, deletes only those from Win+V
and puts your previous clipboard item back.

`adapter_probe` drives the real clipboard adapter for this OS (#12–#15): `watch` reports
copies with their sizes, hash and source app; `write` writes a marker item, checks the
watcher doesn't report it back, then expires it:

```sh
cargo run -p pasta-clipboard --example adapter_probe -- watch 30
cargo run -p pasta-clipboard --example adapter_probe -- write
```

## Testing across machines

- Sync needs at least two machines on one tailnet (e.g. a Mac and a Windows PC).
  Integration tests (#19) run several peers in one process without Tailscale.
- The checklist for real hardware is [`testing/manual-mvp.md`](testing/manual-mvp.md).
- `PASTA_TAILSCALE_CLI=/path/to/tailscale` overrides CLI discovery.
- Items are end-to-end encrypted (M2), but test with throwaway text anyway.
- `chain_peer` is a headless peer on the real tailnet (real discovery and WhoIs, an
  in-memory clipboard) for testing chains between machines without the UI. It writes the
  words to a file, never the terminal, and reports items by id, size and SHA-256 prefix.
  It listens on port 47820, so don't run it next to the app on the same machine:

  ```sh
  cargo run -p pasta-net --example chain_peer -- --create /tmp/words --auto-approve --for 300
  cargo run -p pasta-net --example chain_peer -- --join /tmp/words --send-every 5 --for 120
  ```

- The real keychain backends are covered by an ignored test; run it on each OS (on macOS
  from a GUI session, not over SSH):

  ```sh
  cargo test -p pasta-store -- --ignored keyring
  ```

- The M2 checklist for real hardware is [`testing/manual-m2.md`](testing/manual-m2.md);
  the security review checklist is in [`SECURITY-MODEL.md`](SECURITY-MODEL.md).

## Running the MVP

`pnpm tauri dev` runs the app. It has no window: look for the tray icon (Windows) or the
menu-bar item (macOS). A debug build without the dev server also works once `pnpm build`
has produced `dist/`:

```sh
cargo run -p pasta-app --features tauri/custom-protocol
```

| What                       | Windows                                            | macOS                                                        |
| -------------------------- | -------------------------------------------------- | ------------------------------------------------------------ |
| Encrypted store `pasta.db` | `%APPDATA%\com.cpawlukiewicz.pasta-app\`           | `~/Library/Application Support/com.cpawlukiewicz.pasta-app/` |
| Logs (daily, 7 kept)       | `%LOCALAPPDATA%\com.cpawlukiewicz.pasta-app\logs\` | `~/Library/Logs/com.cpawlukiewicz.pasta-app/`                |
| Keys (one entry)           | Credential Manager › Windows Credentials           | Keychain Access › login                                      |

- Since M2, settings, known peers and the chain's member list live in `pasta.db`, every
  value encrypted with a key from the OS keychain (service `com.cpawlukiewicz.pasta-app`,
  account `device-secrets`; ADR 0008). The first M2 start imports M1's `config.json` and
  deletes it. Settings can't be hand-edited any more until the settings UI (#29);
  `PASTA_LOG=pasta_net=debug,pasta_core=debug` overrides the log level. Logs carry item
  ids, sizes and 12-digit keyed hashes, never clipboard text.
- To start over on a machine, quit Pasta, delete `pasta.db` and the keychain entry.
- Tray → **Copy diagnostics** puts a redacted report on the clipboard (not synced): no
  device names, tailnet addresses, node ids or chain id, so it can go in a public issue.
- Tray → **Sync chain…** opens the developer panel (until #43 / #46): create a chain and
  show its 24 words, join one by typing them, approve / decline devices waiting to join,
  remove members, leave. Nothing syncs until the device is an approved member of a chain.
- A locked keychain shows "Waiting for the keychain" in the tray, with **Try the keychain
  again**; Pasta also retries every 30 s. Over SSH the macOS login keychain is
  unavailable ("User interaction is not allowed").
- Debug builds harden the process like release builds (no core dumps; on Linux no ptrace).
  `PASTA_DEBUGGABLE=1` skips that so a debugger can attach.
- On macOS, launch the app from Finder or with `open` rather than from an SSH session, so
  it runs in your login session with the menu bar and pasteboard.

### macOS: stop the keychain prompt on every rebuild

Debug builds have an ad-hoc signature that changes with every build, and the keychain
trusts an app by its signature, so each rebuild asks for the login password again
("Always Allow" lasts until the next build). Release builds signed with a Developer ID
keep the same identity across builds and don't have this problem.

To give dev builds a stable identity, once per Mac:

1. Keychain Access › Certificate Assistant › **Create a Certificate…**: name `Pasta Dev`,
   identity type **Self-Signed Root**, certificate type **Code Signing**.
2. Add `export PASTA_CODESIGN_IDENTITY="Pasta Dev"` to your shell profile and open a new
   terminal.
3. Run `pnpm tauri dev`. The first time, allow `codesign` to use the key, then answer
   Pasta's keychain prompt with **Always Allow**. Later rebuilds don't prompt.

`.cargo/config.toml` runs `scripts/macos-dev-runner.sh` for every `cargo run` on macOS. It
re-signs only the `pasta-app` binary, and only when `PASTA_CODESIGN_IDENTITY` is set; if
signing fails it warns and runs the app unsigned.

### Windows Firewall

Pasta listens on the Tailscale addresses only. If Windows asks whether to allow Pasta on
a network, allow it; if peers can reach the PC's other services over Tailscale but not
Pasta, add a rule scoped to the Tailscale interface (an elevated PowerShell):

```powershell
New-NetFirewallRule -DisplayName "Pasta (Tailscale)" -Direction Inbound -Action Allow `
  -Protocol TCP -LocalPort 47820 -InterfaceAlias Tailscale -Program "<path to pasta-app.exe>"
```

The installer (#57) should add this rule itself.
