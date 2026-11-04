//! Chain keys derived from the 24 words, and each device's signing key (#20, #22).
//!
//! ```text
//! chain secret (256 bits, the words)
//!   └ HKDF-SHA256(salt = "pasta/chain/v1")
//!       ├ info "pasta/chain-id"     → chain id (16 bytes, safe to show and store)
//!       ├ info "pasta/payload-key"  → XChaCha20-Poly1305 key for message bodies (#21)
//!       ├ info "pasta/auth-key"     → HMAC key for chain proofs in `hello`
//!       └ info "pasta/log-key"      → HMAC key for content hashes in log lines
//! ```

use std::fmt;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, KeyInit, Mac};
use pasta_core::wire::{Bytes, Key32, Sig64};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::secret::Secret32;
use crate::words::{self, WordsError};

const SALT: &[u8] = b"pasta/chain/v1";

/// Identifies a chain without revealing its key. Shown as a short fingerprint
/// so two devices can be compared by eye.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChainId(pub [u8; 16]);

impl ChainId {
    /// `ABCD-EFGH`: the first 32 bits in base-32 (Crockford), easy to read aloud.
    pub fn fingerprint(&self) -> String {
        const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let value = u64::from_be_bytes([0, 0, 0, self.0[0], self.0[1], self.0[2], self.0[3], self.0[4]]);
        let chars: String = (0..8).rev().map(|i| ALPHABET[((value >> (i * 5)) & 31) as usize] as char).collect();
        format!("{}-{}", &chars[..4], &chars[4..])
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChainId({})", self.fingerprint())
    }
}

/// The chain secret and everything derived from it.
pub struct ChainKeys {
    secret: Secret32,
    id: ChainId,
    payload: Secret32,
    auth: Secret32,
    log: Secret32,
}

impl ChainKeys {
    /// A new chain with a random secret.
    pub fn generate() -> Self {
        Self::from_secret(Secret32::random())
    }

    pub fn from_words(input: &str) -> Result<Self, WordsError> {
        words::decode(input).map(Self::from_secret)
    }

    pub fn from_secret(secret: Secret32) -> Self {
        let hkdf = Hkdf::<Sha256>::new(Some(SALT), secret.expose());
        let expand = |info: &[u8]| {
            let mut key = Secret32::zeroed();
            // 32 bytes is far below HKDF-SHA256's 8160-byte limit.
            let _ = hkdf.expand(info, key.expose_mut());
            key
        };
        let mut id = [0u8; 16];
        let _ = hkdf.expand(b"pasta/chain-id", &mut id);
        Self {
            id: ChainId(id),
            payload: expand(b"pasta/payload-key"),
            auth: expand(b"pasta/auth-key"),
            log: expand(b"pasta/log-key"),
            secret,
        }
    }

    pub fn id(&self) -> ChainId {
        self.id
    }

    pub fn secret(&self) -> &Secret32 {
        &self.secret
    }

    /// The 24 words, for showing on a device that adds another one.
    pub fn words(&self) -> Zeroizing<String> {
        words::encode(&self.secret)
    }

    pub(crate) fn payload_key(&self) -> &Secret32 {
        &self.payload
    }

    /// Key for [`pasta_core::clip::set_log_key`].
    pub fn log_key(&self) -> &Secret32 {
        &self.log
    }

    /// Proves knowledge of the chain secret in a `hello`, bound to the
    /// sender's identity and its fresh nonce so it can't be replayed by
    /// anyone else.
    pub fn chain_proof(&self, device_id: &[u8; 16], device_key: &Key32, nonce: &Key32) -> Key32 {
        let mac = self.proof_mac(device_id, device_key, nonce);
        let mut out = [0u8; 32];
        out.copy_from_slice(&mac.finalize().into_bytes());
        Bytes(out)
    }

    pub fn verify_chain_proof(&self, device_id: &[u8; 16], device_key: &Key32, nonce: &Key32, proof: &Key32) -> bool {
        self.proof_mac(device_id, device_key, nonce).verify_slice(&proof.0).is_ok()
    }

    fn proof_mac(&self, device_id: &[u8; 16], device_key: &Key32, nonce: &Key32) -> Hmac<Sha256> {
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(self.auth.expose()).expect("HMAC takes any key length");
        mac.update(b"pasta/v2/chain-proof");
        mac.update(&self.id.0);
        mac.update(device_id);
        mac.update(&device_key.0);
        mac.update(&nonce.0);
        mac
    }
}

impl fmt::Debug for ChainKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainKeys").field("id", &self.id).finish_non_exhaustive()
    }
}

/// A device's Ed25519 identity. The seed stays in locked memory; the signing
/// key is rebuilt for each signature and wiped right after.
pub struct DeviceKey {
    seed: Secret32,
    public: Key32,
}

impl DeviceKey {
    pub fn generate() -> Self {
        Self::from_seed(Secret32::random())
    }

    pub fn from_seed(seed: Secret32) -> Self {
        let signing = SigningKey::from_bytes(seed.expose());
        let public = Bytes(signing.verifying_key().to_bytes());
        Self { seed, public }
    }

    pub fn seed(&self) -> &Secret32 {
        &self.seed
    }

    pub fn public(&self) -> Key32 {
        self.public
    }

    pub fn sign(&self, message: &[u8]) -> Sig64 {
        // `SigningKey` zeroizes itself on drop.
        let signing = SigningKey::from_bytes(self.seed.expose());
        Bytes(signing.sign(message).to_bytes())
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceKey({})", short(&self.public))
    }
}

/// Checks an Ed25519 signature, rejecting weak keys and malleable signatures.
pub fn verify(public: &Key32, message: &[u8], signature: &Sig64) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(&public.0) else { return false };
    if key.is_weak() {
        return false;
    }
    key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&signature.0)).is_ok()
}

/// First 16 hex digits (64 bits) of a public key, for logs and for people to compare.
pub fn short(key: &Key32) -> String {
    key.0[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_is_stable() {
        // Pinned so a change to labels or salt can't slip through: devices on
        // different builds must derive the same keys from the same words.
        let mut bytes = [0x7f; 32];
        let keys = ChainKeys::from_secret(Secret32::from_bytes(&mut bytes));
        let vectors: serde_json::Value = serde_json::from_str(include_str!("../tests/vectors.json")).unwrap();
        let chain = &vectors["chain"];
        assert_eq!(keys.id().to_hex(), chain["chain_id"].as_str().unwrap());
        assert_eq!(keys.id().fingerprint(), chain["fingerprint"].as_str().unwrap());
        let hex = |key: &Secret32| key.expose().iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(keys.payload_key()), chain["payload_key"].as_str().unwrap());
        assert_eq!(hex(&keys.auth), chain["auth_key"].as_str().unwrap());
        assert_eq!(hex(keys.log_key()), chain["log_key"].as_str().unwrap());
        assert_eq!(keys.words().as_str(), chain["words"].as_str().unwrap());
    }

    #[test]
    fn same_words_same_chain() {
        let a = ChainKeys::generate();
        let b = ChainKeys::from_words(&a.words()).unwrap();
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), ChainKeys::generate().id());
        assert_eq!(a.id().fingerprint().len(), 9);
    }

    #[test]
    fn chain_proofs_bind_identity_and_chain() {
        let chain = ChainKeys::generate();
        let device = DeviceKey::generate();
        let id = [1u8; 16];
        let nonce = Bytes([2u8; 32]);
        let proof = chain.chain_proof(&id, &device.public(), &nonce);
        assert!(chain.verify_chain_proof(&id, &device.public(), &nonce, &proof));
        assert!(!chain.verify_chain_proof(&[9u8; 16], &device.public(), &nonce, &proof), "other device id");
        assert!(!chain.verify_chain_proof(&id, &DeviceKey::generate().public(), &nonce, &proof), "other key");
        assert!(!chain.verify_chain_proof(&id, &device.public(), &Bytes([3u8; 32]), &proof), "other nonce");
        assert!(!ChainKeys::generate().verify_chain_proof(&id, &device.public(), &nonce, &proof), "other chain");
    }

    #[test]
    fn signatures_verify_only_for_their_key_and_message() {
        let device = DeviceKey::generate();
        let signature = device.sign(b"message");
        assert!(verify(&device.public(), b"message", &signature));
        assert!(!verify(&device.public(), b"other", &signature));
        assert!(!verify(&DeviceKey::generate().public(), b"message", &signature));
        assert!(!verify(&Bytes([0u8; 32]), b"message", &signature), "weak key");
        let restored = DeviceKey::from_seed(device.seed().duplicate());
        assert_eq!(restored.public(), device.public());
    }
}
