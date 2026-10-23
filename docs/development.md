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

## Testing across machines

- Sync needs at least two machines on one tailnet (e.g. a Mac and a Windows PC).
  Integration tests (#19) run several peers in one process without Tailscale.
- `PASTA_TAILSCALE_CLI=/path/to/tailscale` overrides CLI discovery.
- Never paste real secrets while testing sync; until M2 lands, payloads are protected
  only by WireGuard.
