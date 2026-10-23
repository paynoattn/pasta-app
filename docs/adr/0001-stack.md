# 0001. Tauri 2 + React/TypeScript, Rust does the work

- Status: Accepted
- Date: 2026-10-22
- Issue: #1

## Context

Pasta runs in the background all day on macOS, Windows and Linux. Its real work is OS
integration: clipboard hooks, global shortcuts, a non-activating panel on macOS,
keychains and networking. The UI is settings screens, a tray popover and the Mac
clipboard panel. The maintainer's stack is TypeScript + React, and mobile clients may
come later.

Options considered: Electron (~150 MB RAM for a clipboard watcher, native modules needed
anyway), fully native Rust UI (slower UI work), native app per OS (three codebases),
Tauri 2.

## Decision

- **Tauri 2** with a **React + TypeScript** frontend (Vite).
- **All OS, network, crypto and storage work lives in Rust** crates; React only renders
  and calls typed commands. The background process stays small and the core is reusable
  by a future mobile app.
- Stay on Tauri 2 stable; Tauri 3 is in alpha as of this writing.

## Consequences

- Platform features need Rust FFI (`windows`, `objc2`, D-Bus); the UI stays fast to build.
- Building the macOS target needs a Mac; CI covers all three OSes (#3).
- iOS and Android can't watch the clipboard in the background, so mobile (#63) would be
  receive + manual send; it is not a design driver for v1.
