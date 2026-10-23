//! OS clipboard adapters for Pasta.
//!
//! Each platform gets an adapter that can watch for local copies, write
//! received items into the native clipboard / clipboard history, and expire
//! them again. Adapters land in M1 (#12–#15) and M3 (#34, #35); the spike
//! probes that informed them live in `examples/`.
//!
//! See `docs/adr/0004-windows-clipboard-history.md` and
//! `docs/adr/0005-macos-pasteboard.md`.
