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
- Never paste real secrets while testing sync; until M2 lands, payloads are protected
  only by WireGuard.

## Running the MVP

`pnpm tauri dev` runs the app. It has no window: look for the tray icon (Windows) or the
menu-bar item (macOS). A debug build without the dev server also works once `pnpm build`
has produced `dist/`:

```sh
cargo run -p pasta-app --features tauri/custom-protocol
```

| What                   | Windows                                            | macOS                                                        |
| ---------------------- | -------------------------------------------------- | ------------------------------------------------------------ |
| Config (`config.json`) | `%APPDATA%\com.cpawlukiewicz.pasta-app\`           | `~/Library/Application Support/com.cpawlukiewicz.pasta-app/` |
| Logs (daily, 7 kept)   | `%LOCALAPPDATA%\com.cpawlukiewicz.pasta-app\logs\` | `~/Library/Logs/com.cpawlukiewicz.pasta-app/`                |

- `config.json` holds the device id, `mode` (`send`, `receive`, `both`, `paused`),
  `port` (default 47820), `max_item_bytes`, `log_level` and the known peers. Edits are
  picked up within a few seconds; `log_level` and `port` changes need a restart and a
  rediscovery respectively.
- `PASTA_LOG=pasta_net=debug,pasta_core=debug` overrides the log level. Logs carry item
  ids, sizes and 12-digit hashes, never clipboard text.
- Tray → **Copy diagnostics** puts a redacted report on the clipboard (not synced): no
  device names, tailnet addresses or node ids, so it can go in a public issue.
- On macOS, launch the app from Finder or with `open` rather than from an SSH session, so
  it runs in your login session with the menu bar and pasteboard.

### Windows Firewall

Pasta listens on the Tailscale addresses only. If Windows asks whether to allow Pasta on
a network, allow it; if peers can reach the PC's other services over Tailscale but not
Pasta, add a rule scoped to the Tailscale interface (an elevated PowerShell):

```powershell
New-NetFirewallRule -DisplayName "Pasta (Tailscale)" -Direction Inbound -Action Allow `
  -Protocol TCP -LocalPort 47820 -InterfaceAlias Tailscale -Program "<path to pasta-app.exe>"
```

The installer (#57) should add this rule itself.
