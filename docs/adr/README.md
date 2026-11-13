# Architecture decision records

Short records of decisions that shape Pasta, newest last. Copy
[`template.md`](template.md) for a new one; never rewrite an accepted ADR, supersede it.

| #                                                        | Decision                                                 | Status   |
| -------------------------------------------------------- | -------------------------------------------------------- | -------- |
| [0001](0001-stack.md)                                    | Tauri 2 + React/TypeScript, Rust does the work           | Accepted |
| [0002](0002-tailscale-access.md)                         | Talk to Tailscale through its CLI's JSON output          | Accepted |
| [0003](0003-transport-and-encryption.md)                 | WebSocket over the tailnet + sync-chain E2E              | Accepted |
| [0004](0004-windows-clipboard-history.md)                | Windows: write and expire items in Win+V history         | Accepted |
| [0005](0005-macos-pasteboard.md)                         | macOS: pasteboard polling, markers, Universal Clipboard  | Accepted |
| [0006](0006-protocol-and-sync.md)                        | Wire protocol v1, connection rules, echo prevention      | Proposed |
| [0007](0007-sync-chain-and-encryption.md)                | Sync chain, E2E encryption, device approval (v2)         | Accepted |
| [0008](0008-secrets-and-local-storage.md)                | Keychain secrets, encrypted store, protected memory      | Accepted |
| [0009](0009-no-tls-over-tailscale.md)                    | No TLS (`tailscale cert`) on top of WireGuard and E2E    | Accepted |
| [0010](0010-joining-device-trusts-the-first-approval.md) | A joining device trusts the first approval it gets       | Accepted |
| [0011](0011-history-search-without-a-plaintext-index.md) | Search the Mac history by decrypting, no plaintext index | Accepted |
| [0012](0012-remote-device-settings.md)                   | Device settings: encrypted, signed, last writer wins     | Accepted |
| [0013](0013-windows-ipc-types-and-the-mac-panel.md)      | Windows, generated IPC types, native panel on macOS      | Accepted |
| [0014](0014-app-identifier.md)                           | App identifier `com.kingdomworks.pasta`                  | Accepted |
| [0015](0015-kde-klipper.md)                              | KDE Plasma: Wayland data-control, Klipper keeps history  | Accepted |
