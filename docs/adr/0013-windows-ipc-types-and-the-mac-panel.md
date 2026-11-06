# 0013. One web bundle for every window, generated IPC types, a native panel on macOS

- Status: Accepted
- Date: 2026-11-04
- Issues: #28, #38, #39, #42, #47

## Context

M3 adds four windows (Settings, setup, the tray popover / flyout and the macOS clipboard
panel) on top of the background app, all talking to Rust over Tauri commands. The UI must
follow each OS (macOS, Windows 11 Fluent, KDE Breeze, light and dark), the panel must take
keystrokes without stealing focus from the app the user pastes into, and updates must be
signed, before the release pipeline that signs them exists (#58).

## Decision

- **Windows.** One Vite bundle; `src/main.tsx` reads the Tauri window label (`main`,
  `setup`, `tray`, `panel`) and lazy-loads that tree. Settings is declared in
  `tauri.conf.json`; the tray and panel windows are built in Rust at start
  (`src-tauri/src/ui.rs`), transparent and undecorated, and setup is built on demand.
  Closing hides a window, except setup, which is destroyed when finished.
- **Types.** Every IPC view type in `src-tauri/src/views.rs` (and the chain views) derives
  `ts_rs::TS`; `cargo test -p pasta-app` writes `src/ipc/bindings/*.ts`, CI fails if they
  differ from what's committed, and `src/ipc/commands.ts` wraps each command with those
  types. ts-rs over tauri-specta: it is stable, derive-only, and leaves the commands as
  plain Tauri commands; tauri-specta is still a release candidate.
- **State.** No state library: a small `useResource(load, events)` hook loads from Rust
  and loads again when a Rust event fires. Rust stays the source of truth.
- **Theme.** CSS custom properties keyed on `data-platform` (from the webview's user
  agent, so the first paint is right) and `prefers-color-scheme`. Primitives (toggle,
  segmented control, InfoBar, sections) restyle per OS.
- **The panel (#38).** The panel window's `NSWindow` becomes a non-activating `NSPanel`
  at runtime (`src-tauri/src/macos_panel.rs`), as `tauri-nspanel` does, but without the
  git dependency: the object's class is swapped for an `NSPanel` subclass that can become
  key. The subclass reserves the same bytes as Tao's window class, so the instance size
  matches (objc2 checks it) and Tao still finds its own state at its fixed offset; if Tao's
  layout changes the swap is skipped and the panel stays a normal window. The panel is
  shown with `orderFrontRegardless` + `makeKeyWindow`, so Pasta never becomes the active
  app, floats above full-screen apps on every Space, and hides when it loses focus.
- **Paste (#39).** Picking an item writes it to the pasteboard (not synced), hides the
  panel, gives focus back to the app that was in front if needed, and posts ⌘V with
  `CGEventPost`. Without the Accessibility permission (`AXIsProcessTrustedWithOptions`)
  the keystroke would be dropped, so Pasta only copies and the panel says so.
- **Updates (#47).** `tauri-plugin-updater` is loaded only in builds that have
  `PASTA_UPDATER_PUBKEY` (and optionally `PASTA_UPDATER_ENDPOINT`) at compile time, with
  `requireSignedVersion` so a forged manifest can't downgrade. Without them the page says
  updates aren't set up. Open at login uses `tauri-plugin-autostart`, off by default.

## Consequences

- One bundle means the tray and panel load the shared runtime chunk; each window's own
  code is a separate lazy chunk.
- The class swap relies on Tao keeping its window state in one pointer-sized variable; a
  Tao upgrade that changes that degrades the panel (it activates Pasta) instead of
  crashing. Recheck after Tao upgrades.
- The Accessibility grant is tied to the app's signature: dev builds lose it on rebuild
  unless signed with a stable identity (docs/development.md), and release builds keep it
  only with consistent signing (#56).
- #58 has to set the updater key and publish `latest.json` before updates work.
