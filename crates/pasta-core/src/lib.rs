//! Platform-independent heart of Pasta.
//!
//! Owns the domain model (devices, clipboard items, sync modes), the wire
//! protocol (#7) and the sync engine (#16). Nothing in here touches the OS
//! clipboard, the network or the Tauri runtime directly: those are injected
//! through traits so the engine can be tested with fakes.
//!
//! See `docs/ARCHITECTURE.md`.

pub mod chain;
pub mod clip;
pub mod clipboard;
pub mod config;
pub mod device;
pub mod engine;
pub mod history;
pub mod protocol;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod wire;

pub use protocol::PROTOCOL_VERSION;
