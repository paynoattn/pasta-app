//! Where Pasta keeps things on disk (M2).
//!
//! - [`secrets`]: device id, device key, database key and chain secret in the
//!   OS credential store (#23).
//! - [`db`]: the encrypted-at-rest SQLite store for settings, peers and the
//!   member list (#25), keyed from the credential store.
//!
//! See `docs/adr/0008-secrets-and-local-storage.md`.

pub mod db;
pub mod secrets;

pub use db::{Database, DbError};
pub use secrets::{DeviceSecrets, KeyringStore, MemoryStore, SecretError, SecretStore};
