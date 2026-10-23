//! Peer networking for Pasta.
//!
//! Everything Pasta sends travels over the tailnet. This crate wraps the
//! local Tailscale client (discovery and identity, see [`tailscale`]) and
//! will grow the WebSocket transport (#9) and connection manager (#10).
//!
//! See `docs/adr/0002-tailscale-access.md`.

pub mod tailscale;
