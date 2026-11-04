//! Pasta's cryptography (M2): the sync chain, end-to-end encryption, device
//! identity and membership, and keeping secrets out of reach in memory.
//!
//! - [`words`]: the 24-word chain code (#20)
//! - [`keys`]: chain keys derived from it, device signing keys (#20, #22)
//! - [`seal`]: XChaCha20-Poly1305 message bodies and replay protection (#21)
//! - [`membership`]: the signed member list (#22)
//! - [`secret`], [`vault`], [`hardening`]: protected memory (#24)
//!
//! See `docs/adr/0007-sync-chain-and-encryption.md` and `docs/SECURITY-MODEL.md`.

pub mod hardening;
pub mod keys;
pub mod membership;
pub mod seal;
pub mod secret;
pub mod vault;
pub mod words;

pub use keys::{ChainId, ChainKeys, DeviceKey};
pub use secret::Secret32;

/// Fills `buf` from the OS's CSPRNG. There's no safe way to continue without one.
pub fn fill_random(buf: &mut [u8]) {
    getrandom::fill(buf).unwrap_or_else(|err| panic!("the OS random number generator failed: {err}"));
}

/// A fresh random 32-byte value (handshake nonces).
pub fn random_key32() -> pasta_core::wire::Key32 {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes);
    pasta_core::wire::Bytes(bytes)
}
