//! Cryptography for Pasta.
//!
//! Will hold the sync-chain phrase and key derivation (#20), payload
//! encryption (#21), device keypairs for approvals (#22) and protected-memory
//! helpers (#24). Empty until M2; kept as a crate now so the dependency
//! graph (core ← crypto ← net / app) is fixed from the start.
//!
//! See `docs/ARCHITECTURE.md` and `docs/adr/0003-transport-and-encryption.md`.
