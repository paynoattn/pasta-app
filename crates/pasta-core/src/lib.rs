//! Platform-independent heart of Pasta.
//!
//! Owns the domain model (devices, clipboard items, sync modes), the wire
//! protocol (#7) and the sync engine (#16). Nothing in here touches the OS
//! clipboard, the network or the Tauri runtime directly: those are injected
//! through traits so the engine can be tested with fakes.
//!
//! See `docs/ARCHITECTURE.md`.

/// Version of the peer-to-peer wire protocol this build speaks.
///
/// Bumped on breaking changes; peers with a different major version refuse
/// the handshake.
pub const PROTOCOL_VERSION: u16 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_starts_at_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
