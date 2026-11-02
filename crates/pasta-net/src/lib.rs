//! Peer networking for Pasta.
//!
//! Everything Pasta sends travels over the tailnet. This crate wraps the
//! local Tailscale client (discovery and identity, see [`tailscale`]), the
//! WebSocket [`transport`] (#9), the connection manager in [`network`]
//! (#10, #11) and [`node::Node`], which wires the sync engine to it.
//!
//! See `docs/adr/0002-tailscale-access.md` and
//! `docs/adr/0003-transport-and-encryption.md`.

pub mod discovery;
pub mod network;
pub mod node;
pub mod tailscale;
pub mod transport;
pub mod trust;
pub mod verify;
