# Pasta architecture

Pasta syncs copied text between your own machines over Tailscale and, on macOS,
replaces Paste.app with a native clipboard history panel.

- **Copy on one device → appears in every other device's native clipboard history**
  (Win+V on Windows, Klipper on KDE Plasma, Pasta's own panel on macOS).
- **Traffic never leaves the tailnet**, and payloads are end-to-end encrypted with a
  key only your devices hold.
- **Each device has its own mode** (send / receive / both / paused) and **expiry timer**
  (5 s → never) for items it receives.

Approved mocks: <https://claude.ai/artifact/SxDCzg1FQZJU2YckgvEcAp> ·
Roadmap: issue #65 · Decisions: [`docs/adr/`](adr/README.md)

## Scope of v1

| In                                            | Later                                    |
| --------------------------------------------- | ---------------------------------------- |
| Text: plain plus HTML / RTF flavors           | Images (#61)                             |
| Live sync between online devices              | Offline catch-up (#62)                   |
| macOS, Windows, KDE Plasma (CachyOS)          | GNOME (#60), mobile (#63)                |
| Mac panel: search, rich text, source-app icon | Pinboards, quick paste, Quick Look (#64) |

## Process model

Pasta is one Tauri 2 app per device that runs in the background (tray / menu bar).
**Rust does all the work**: watching and writing the clipboard, networking, crypto,
storage, global shortcuts and native panels. **React is only the UI**, talking to Rust
through typed Tauri commands and events.

```mermaid
flowchart LR
  subgraph Device A
    WA[Clipboard watcher] --> EA[Sync engine]
    EA --> NA[Peer transport]
    UIA[React UI] <-->|commands / events| EA
  end
  subgraph Device B
    NB[Peer transport] --> EB[Sync engine]
    EB --> XB[Clipboard writer]
    XB --> HB[(Native history<br/>Win+V / Klipper / Pasta panel)]
    EB --> TB[Expiry timers] --> XB
  end
  NA -- "WebSocket over tailnet (WireGuard)<br/>+ E2E encrypted payload" --> NB
```

Every device is both server and client: each listens on its tailnet address only and
connects out to the peers it discovers. There is no central server.

## Crates

```
crates/
  pasta-core       domain model, wire protocol, sync engine, adapter traits (no I/O)
  pasta-crypto     sync-chain words + key derivation, AEAD + replay guard, device keys,
                   signed member list, protected memory, process hardening
  pasta-net        Tailscale client (discovery, WhoIs), WebSocket transport, connection manager
                   (signed handshake, chain + membership gating), and `Node`, which wires the
                   engine to the network
  pasta-clipboard  per-OS clipboard adapters: watch / write / expire
  pasta-store      OS keychain entry for the device's secrets, encrypted SQLite store
src-tauri/         the app: wiring, Tauri commands/events, windows, tray, plugins
src/               React UI
```

Dependency direction: `pasta-core` ← `pasta-crypto` ← `pasta-net` / `pasta-store` /
`pasta-clipboard` ← `src-tauri`. `pasta-core` stays free of OS, network and Tauri code so the engine can be
tested with fake adapters (`pasta_core::testing`). The integration tests (#19) run real
`Node`s on loopback with fake clipboards and a static discovery list instead of Tailscale.

## Data flow: one copy

1. **Watch.** The platform watcher sees a new clipboard item, skips it if it's marked
   sensitive, comes from an excluded app, or was written by Pasta itself.
2. **Engine.** Builds a `clip` message (item id, origin device, source app, timestamp,
   flavors, content hash), debounces and dedupes it.
3. **Encrypt + send.** The payload is sealed with the chain key (#21) and sent to every
   connected, approved peer whose mode accepts it.
4. **Receive.** The peer verifies the sender (Tailscale WhoIs + chain membership),
   decrypts, checks the content hash against recently seen items (no ping-pong) and
   hands it to its clipboard writer.
5. **Write.** The writer puts the item on the clipboard so it lands in native history,
   tagged so the local watcher ignores it.
6. **Expire.** The receiving device starts a timer from its own TTL. When it fires, the
   platform adapter deletes that history entry (and clears the clipboard if the item is
   still current).

## Networking and trust

- **Discovery and identity** come from the local Tailscale client via its CLI's `--json`
  output ([ADR 0002](adr/0002-tailscale-access.md)).
- **Transport** is WebSocket bound to the tailnet interface only (100.64.0.0/10,
  fd7a:115c:a1e0::/48), never 0.0.0.0 ([ADR 0003](adr/0003-transport-and-encryption.md)).
- **Trust has three layers**: WireGuard (Tailscale) on the wire; a 24-word _sync chain_
  whose derived key encrypts every payload; and a one-time _approval_ of each new
  device by an existing member. A tailnet may contain other people's and tagged
  devices, so "being on the tailnet" alone never grants anything. A device that isn't
  in a chain syncs nothing ([ADR 0007](adr/0007-sync-chain-and-encryption.md)).
- **At rest**, the device's keys and the chain secret are in the OS keychain and
  everything else in an encrypted SQLite store ([ADR 0008](adr/0008-secrets-and-local-storage.md)).
  What is protected from whom, and the limits, are in [SECURITY-MODEL.md](SECURITY-MODEL.md).

## Wire protocol (v2)

One JSON envelope per WebSocket text frame, defined in `pasta_core::protocol`
([ADR 0006](adr/0006-protocol-and-sync.md), encryption and approval in
[ADR 0007](adr/0007-sync-chain-and-encryption.md)):

```json
{
  "v": 2,
  "type": "encrypted",
  "msg_id": "9b1c…",
  "from_device": "3f2a…",
  "sent_at": 1730000000000,
  "body": { "nonce": "<24 bytes, base64url>", "ciphertext": "<base64url>" }
}
```

The ciphertext is XChaCha20-Poly1305 under the chain key, with the envelope header as
associated data, over an inner message:

```json
{
  "type": "clip",
  "body": {
    "item_id": "c0de…",
    "origin_device": "3f2a…",
    "origin_app": { "name": "Safari", "id": "com.apple.Safari" },
    "created_at": 1730000000000,
    "flavors": { "text/plain": "…", "text/html": "…" },
    "content_hash": "<sha-256 of the plain text, hex>"
  }
}
```

| Type        | Body                                                                        | Sent                                       |
| ----------- | --------------------------------------------------------------------------- | ------------------------------------------ |
| `hello`     | device id, name, OS, versions, capabilities, device key, nonce, chain proof | by the dialer, first                       |
| `hello_ack` | same as `hello`, plus the listener's signature over both hellos             | by the listener, once accepted             |
| `auth`      | the dialer's signature over both hellos                                     | by the dialer, completing the handshake    |
| `encrypted` | sealed inner `clip`, `members`, `settings` or `config_update` (see below)   | items on every local copy; lists on change |
| `ack`       | `msg_id` of the `encrypted` message                                         | on accepting one                           |
| `ping`      | empty                                                                       | every 10 s                                 |
| `pong`      | `ping_id`                                                                   | in reply; gives the latency                |

Inside `encrypted`: `clip` (one copied item), `members` (the signed member list),
`settings` (the sender's own mode, expiry and pause, on connect and on change) and
`config_update` (a member changing the receiver's mode and expiry, signed with its device
key; [ADR 0012](adr/0012-remote-device-settings.md)).

- **Versioning:** `v` is the major version. A peer with another one is refused with
  close code 4001; unknown message types within v2 are ignored. A plaintext `clip` (v1)
  is dropped.
- **Limits:** 1 MiB per item by default (`max_item_bytes` in the config). Too-big items
  lose RTF, then HTML, before they're refused. Receivers check size and `content_hash`
  after decrypting.
- **Close codes:** 4001 version, 4002 refused (not a tailnet node, or a device id seen on
  another node), 4003 duplicate connection, 4004 protocol error, 4005 heartbeat timeout,
  4006 not in this sync chain, 4007 not trusted (bad signature, wrong key, declined).
- **Echo prevention:** the engine never broadcasts a copy whose hash matches the item it
  believes is current, and never relays an item from a peer.

## Platform matrix

|                | macOS                                                                                                                                                                                        | Windows                                                                                                            | KDE Plasma 6                                                        |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------- |
| Watch          | Poll `NSPasteboard.changeCount` (~250 ms)                                                                                                                                                    | `AddClipboardFormatListener` / `WM_CLIPBOARDUPDATE`                                                                | data-control protocol or Klipper D-Bus (spike #49)                  |
| Write          | `NSPasteboard` + Pasta marker type                                                                                                                                                           | Clipboard write that lands in Win+V ([ADR 0004](adr/0004-windows-clipboard-history.md))                            | data-control / Klipper (spike #49)                                  |
| Native history | Pasta's own panel (⇧⌘V)                                                                                                                                                                      | Win+V                                                                                                              | Klipper                                                             |
| Expire         | Pasta store; clear pasteboard if still ours                                                                                                                                                  | `Clipboard.DeleteItemFromHistory`                                                                                  | TBD by spike #49                                                    |
| Skip sensitive | `org.nspasteboard.ConcealedType` / `TransientType` / `AutoGeneratedType`, plus a default list of password-manager apps and browser extensions (not every manager sets the markers; ADR 0005) | `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory = 0`, plus the same default app list | `x-kde-passwordManagerHint: secret`, plus the same default app list |
| Secrets        | Keychain                                                                                                                                                                                     | Credential Manager / DPAPI                                                                                         | KWallet / Secret Service                                            |
| Tailscale CLI  | `/Applications/Tailscale.app/Contents/MacOS/Tailscale`                                                                                                                                       | `C:\Program Files\Tailscale\tailscale.exe`                                                                         | `/usr/bin/tailscale` (needs `--operator`)                           |
| iCloud bridge  | Universal Clipboard via the general pasteboard ([ADR 0005](adr/0005-macos-pasteboard.md))                                                                                                    | n/a                                                                                                                | n/a                                                                 |

## Frontend

```
src/
  App.tsx      picks the window's tree by Tauri window label
  windows/     settings/ (main), setup/, tray/ (popover / flyout), panel/ (macOS ⇧⌘V)
  components/  primitives (toggle, segmented control, InfoBar, dialog), chain + Add Device
  ipc/         typed wrappers around the commands; bindings/ is generated from Rust (ts-rs)
  state/       useResource: load from Rust, reload on Rust events
  lib/         formatting, sanitised rich-text previews, shortcut recording
  platform/    per-OS theme tokens (macOS, Fluent, Breeze), light and dark
```

The UI follows each OS's native look (see the mocks). The IPC types in
`src-tauri/src/views.rs` derive `ts_rs::TS`; `cargo test -p pasta-app` writes
`src/ipc/bindings/`, and CI fails if they're stale. The macOS panel is a non-activating
`NSPanel`, so pasting goes to the app the user was in
([ADR 0013](adr/0013-windows-ipc-types-and-the-mac-panel.md)). The Mac history is a table
in the encrypted store, searched by decrypting
([ADR 0011](adr/0011-history-search-without-a-plaintext-index.md)).

## Where things are tracked

| Milestone                   | Issues  |
| --------------------------- | ------- |
| M0 · Foundations            | #1–#6   |
| M1 · Mac ↔ Windows sync MVP | #7–#19  |
| M2 · Trust & encryption     | #20–#27 |
| M3 · Full client            | #28–#48 |
| M4 · KDE Plasma client      | #49–#55 |
| M5 · Release & distribution | #56–#59 |
| Later                       | #60–#64 |

See [`docs/development.md`](development.md) for setting up a dev machine.
