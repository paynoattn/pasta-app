//! Where Pasta keeps things on disk (M2).
//!
//! - [`secrets`]: device id, device key, database key and chain secret in the
//!   OS credential store (#23).
//! - [`db`]: the encrypted-at-rest SQLite store for settings, peers and the
//!   member list (#25), keyed from the credential store.
//! - [`history`]: the macOS clipboard history (#37), in the same database.
//!
//! See `docs/adr/0008-secrets-and-local-storage.md`.

pub mod db;
pub mod history;
pub mod secrets;

pub use db::{Database, DbError};
pub use history::{HistoryFilter, HistoryItem, HistoryQuery, HistoryStore};
pub use secrets::{DeviceSecrets, KeyringStore, MemoryStore, SecretError, SecretStore};
