//! End-to-end encryption of message bodies (#21).
//!
//! An [`Inner`] message is serialised and sealed with XChaCha20-Poly1305 under
//! the chain's payload key and a random 192-bit nonce (random nonces are safe
//! at this size). The envelope header (`v`, `msg_id`, `from_device`,
//! `sent_at`) is the associated data, so a sealed body only opens under the
//! header it was sent with. Plaintext buffers are wiped after use.
//!
//! [`ReplayGuard`] then refuses anything outside the timestamp window or
//! seen before.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::protocol::{Envelope, Inner, Message, PROTOCOL_VERSION, Sealed, header_aad};
use pasta_core::wire::{Blob, Bytes, Nonce24};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::keys::ChainKeys;
use crate::secret::Secret32;

/// How far a message's `sent_at` may be from the receiver's clock.
pub const REPLAY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Why an `encrypted` message was dropped. Never carries content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenError {
    #[error("not an encrypted message")]
    NotEncrypted,
    #[error("failed to decrypt (wrong chain key or tampered)")]
    Decrypt,
    #[error("decrypted body isn't a known message")]
    Malformed,
    #[error("sent {skew_secs}s away from this device's clock, outside the replay window; check both clocks")]
    Stale { skew_secs: i64 },
    #[error("already received (replay)")]
    Replayed,
}

pub fn seal(key: &Secret32, aad: &[u8], plaintext: &[u8]) -> (Nonce24, Vec<u8>) {
    let mut nonce = [0u8; 24];
    crate::fill_random(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    let mut buffer = Vec::with_capacity(plaintext.len() + 16);
    buffer.extend_from_slice(plaintext);
    // Only fails for messages over ~256 GiB; never send the buffer then, it may still be plaintext.
    if cipher.encrypt_in_place(&XNonce::from(nonce), aad, &mut buffer).is_err() {
        zeroize::Zeroize::zeroize(&mut buffer);
        panic!("message too large to encrypt");
    }
    (Bytes(nonce), buffer)
}

pub fn open(key: &Secret32, nonce: &Nonce24, aad: &[u8], ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, OpenError> {
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    let mut buffer = Zeroizing::new(ciphertext.to_vec());
    cipher.decrypt_in_place(&XNonce::from(nonce.0), aad, &mut *buffer).map_err(|_| OpenError::Decrypt)?;
    Ok(buffer)
}

impl ChainKeys {
    /// An `encrypted` envelope from `from` carrying `inner`.
    pub fn seal_envelope(&self, from: DeviceId, inner: &Inner) -> Envelope {
        let msg_id = Uuid::new_v4();
        let sent_at = UnixMillis::now();
        let aad = header_aad(PROTOCOL_VERSION, &msg_id, &from, sent_at);
        // Sized up front so serde doesn't reallocate and leave plaintext copies behind.
        let estimate = match inner {
            Inner::Clip(clip) => clip.flavors.byte_len().saturating_mul(2).saturating_add(4096),
            Inner::Members(list) => list.records.len().saturating_mul(512).saturating_add(4096),
            Inner::Settings(_) | Inner::ConfigUpdate(_) => 1024,
        };
        let mut plaintext = Zeroizing::new(Vec::with_capacity(estimate));
        // Serialising plain data into a Vec can't fail.
        let _ = serde_json::to_writer(&mut *plaintext, inner);
        let (nonce, ciphertext) = seal(self.payload_key(), &aad, &plaintext);
        Envelope {
            v: PROTOCOL_VERSION,
            msg_id,
            from_device: from,
            sent_at,
            message: Message::Encrypted(Sealed { nonce, ciphertext: Blob(ciphertext) }),
        }
    }

    /// Decrypts an `encrypted` envelope. Replay checks are separate ([`ReplayGuard`]).
    pub fn open_envelope(&self, envelope: &Envelope) -> Result<Inner, OpenError> {
        let Message::Encrypted(sealed) = &envelope.message else { return Err(OpenError::NotEncrypted) };
        let plaintext = open(self.payload_key(), &sealed.nonce, &envelope.aad(), &sealed.ciphertext.0)?;
        serde_json::from_slice(&plaintext).map_err(|_| OpenError::Malformed)
    }
}

/// Refuses messages outside [`REPLAY_WINDOW`] or seen before. Ids are kept
/// for twice the window, so anything older than what's remembered is stale.
#[derive(Debug)]
pub struct ReplayGuard {
    window_ms: u64,
    capacity: usize,
    seen: HashSet<(DeviceId, Uuid)>,
    order: VecDeque<(UnixMillis, DeviceId, Uuid)>,
}

impl Default for ReplayGuard {
    fn default() -> Self {
        Self::new(REPLAY_WINDOW, 20_000)
    }
}

impl ReplayGuard {
    pub fn new(window: Duration, capacity: usize) -> Self {
        let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
        Self { window_ms, capacity, seen: HashSet::new(), order: VecDeque::new() }
    }

    /// Call only for messages that decrypted. Records the id on success.
    pub fn check(&mut self, envelope: &Envelope, now: UnixMillis) -> Result<(), OpenError> {
        let skew_ms = i128::from(envelope.sent_at.0) - i128::from(now.0);
        if skew_ms.unsigned_abs() > u128::from(self.window_ms) {
            let skew_secs = i64::try_from(skew_ms / 1000).unwrap_or(i64::MAX);
            return Err(OpenError::Stale { skew_secs });
        }
        let horizon = now.0.saturating_sub(self.window_ms.saturating_mul(2));
        while let Some(&(at, device, id)) = self.order.front() {
            if at.0 >= horizon && self.order.len() < self.capacity {
                break;
            }
            self.order.pop_front();
            self.seen.remove(&(device, id));
        }
        if !self.seen.insert((envelope.from_device, envelope.msg_id)) {
            return Err(OpenError::Replayed);
        }
        self.order.push_back((now, envelope.from_device, envelope.msg_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pasta_core::chain::MemberList;
    use pasta_core::clip::Flavors;
    use pasta_core::protocol::{Clip, DEFAULT_MAX_ITEM_BYTES};

    use super::*;

    fn clip(from: DeviceId, text: &str) -> Inner {
        let flavors = Flavors::plain(text);
        Inner::Clip(Clip {
            item_id: Uuid::new_v4(),
            origin_device: from,
            origin_app: None,
            created_at: UnixMillis(1),
            content_hash: flavors.content_hash(),
            flavors,
        })
    }

    #[test]
    fn seals_and_opens_through_the_wire_format() {
        let chain = ChainKeys::generate();
        let me = DeviceId::random();
        let canary = "s3cr3t-clipboard-canary";
        let envelope = chain.seal_envelope(me, &clip(me, canary));
        let frame = envelope.encode();
        assert!(!frame.contains(canary), "plaintext on the wire");
        let received = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap();
        let inner = chain.open_envelope(&received).unwrap();
        assert_eq!(inner, clip_with_id(&inner, me, canary));
        inner.validate(me, DEFAULT_MAX_ITEM_BYTES).unwrap();

        let members = chain.seal_envelope(me, &Inner::Members(MemberList::default()));
        assert_eq!(chain.open_envelope(&members).unwrap(), Inner::Members(MemberList::default()));
    }

    #[test]
    fn seals_and_opens_settings_and_config_updates() {
        use pasta_core::device::SyncMode;
        use pasta_core::remote::{ConfigUpdate, DeviceSettings, SettingsVersion};
        use pasta_core::wire::Bytes;

        let chain = ChainKeys::generate();
        let me = DeviceId::random();
        let issuer = DeviceId::random();
        let settings = Inner::Settings(DeviceSettings {
            mode: SyncMode::Receive,
            ttl_secs: Some(300),
            paused_until: Some(UnixMillis(1_730_000_000_000)),
            version: SettingsVersion { counter: 4, at: UnixMillis(9), by: me },
        });
        let update = Inner::ConfigUpdate(ConfigUpdate {
            target: DeviceId::random(),
            mode: SyncMode::Send,
            ttl_secs: None,
            version: SettingsVersion { counter: 5, at: UnixMillis(10), by: issuer },
            signature: Bytes([8; 64]),
        });

        for inner in [&settings, &update] {
            let envelope = chain.seal_envelope(me, inner);
            let frame = envelope.encode();
            let decoded = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap();
            assert_eq!(decoded.message.kind(), "encrypted");
            assert_eq!(chain.open_envelope(&decoded).unwrap(), *inner, "{} round-trips", inner.kind());
            // Nothing about the mode, expiry or signature travels in the clear.
            assert!(!frame.contains("receive") && !frame.contains("config_update"), "kind on the wire");
        }
    }

    fn clip_with_id(inner: &Inner, me: DeviceId, text: &str) -> Inner {
        let Inner::Clip(received) = inner else { panic!("not a clip") };
        let Inner::Clip(mut expected) = clip(me, text) else { unreachable!() };
        expected.item_id = received.item_id;
        Inner::Clip(expected)
    }

    #[test]
    fn refuses_other_keys_and_any_tampering() {
        let chain = ChainKeys::generate();
        let me = DeviceId::random();
        let envelope = chain.seal_envelope(me, &clip(me, "hello"));
        assert_eq!(ChainKeys::generate().open_envelope(&envelope), Err(OpenError::Decrypt), "other chain");

        let mut moved = envelope.clone();
        moved.from_device = DeviceId::random();
        assert_eq!(chain.open_envelope(&moved), Err(OpenError::Decrypt), "header is bound");
        let mut renumbered = envelope.clone();
        renumbered.msg_id = Uuid::new_v4();
        assert_eq!(chain.open_envelope(&renumbered), Err(OpenError::Decrypt));
        let mut redated = envelope.clone();
        redated.sent_at = UnixMillis(redated.sent_at.0 + 1);
        assert_eq!(chain.open_envelope(&redated), Err(OpenError::Decrypt));

        let mut flipped = envelope.clone();
        if let Message::Encrypted(sealed) = &mut flipped.message {
            sealed.ciphertext.0[3] ^= 1;
        }
        assert_eq!(chain.open_envelope(&flipped), Err(OpenError::Decrypt));
        let plain = Envelope::new(me, Message::Ping(pasta_core::protocol::Ping {}));
        assert_eq!(chain.open_envelope(&plain), Err(OpenError::NotEncrypted));
    }

    #[test]
    fn replay_guard_refuses_repeats_and_stale_messages() {
        let chain = ChainKeys::generate();
        let me = DeviceId::random();
        let envelope = chain.seal_envelope(me, &clip(me, "hello"));
        let now = envelope.sent_at;
        let mut guard = ReplayGuard::default();
        guard.check(&envelope, now).unwrap();
        assert_eq!(guard.check(&envelope, now), Err(OpenError::Replayed));
        let window = u64::try_from(REPLAY_WINDOW.as_millis()).unwrap();
        let late = UnixMillis(now.0 + window + 1000);
        assert!(matches!(guard.check(&envelope, late), Err(OpenError::Stale { skew_secs }) if skew_secs < 0));
        let early = UnixMillis(now.0 - window - 1000);
        let fresh = chain.seal_envelope(me, &clip(me, "hello"));
        assert!(matches!(guard.check(&fresh, early), Err(OpenError::Stale { .. })), "from the future");

        // Old ids are forgotten once they're past the window anyway.
        let mut small = ReplayGuard::new(Duration::from_secs(1), 2);
        for _ in 0..5 {
            let message = chain.seal_envelope(me, &clip(me, "x"));
            small.check(&message, message.sent_at).unwrap();
        }
        assert!(small.order.len() <= 2 && small.seen.len() <= 2);
    }

    #[test]
    fn matches_the_pinned_vector() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!("../tests/vectors.json")).unwrap();
        let v = &vectors["aead"];
        let hex = |name: &str| -> Vec<u8> {
            let text = v[name].as_str().unwrap();
            (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap()).collect()
        };
        let key = Secret32::from_slice(&hex("key")).unwrap();
        let nonce = Bytes(<[u8; 24]>::try_from(hex("nonce")).unwrap());
        let opened = open(&key, &nonce, &hex("aad"), &hex("ciphertext")).unwrap();
        assert_eq!(opened.as_slice(), v["plaintext"].as_str().unwrap().as_bytes());
    }
}
