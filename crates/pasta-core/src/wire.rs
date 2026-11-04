//! Fixed-size byte strings on the wire, as unpadded base64url.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// `N` bytes, serialised as unpadded base64url.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Bytes<const N: usize>(pub [u8; N]);

/// An Ed25519 public key, a nonce, an HMAC.
pub type Key32 = Bytes<32>;
/// An Ed25519 signature.
pub type Sig64 = Bytes<64>;
/// An XChaCha20-Poly1305 nonce.
pub type Nonce24 = Bytes<24>;

impl<const N: usize> Bytes<N> {
    pub fn to_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    pub fn from_base64(text: &str) -> Option<Self> {
        let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }
}

impl<const N: usize> fmt::Debug for Bytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Short prefix only: these are public values, but keep log lines readable.
        let hex: String = self.0.iter().take(4).map(|b| format!("{b:02x}")).collect();
        write!(f, "Bytes<{N}>({hex}…)")
    }
}

impl<const N: usize> Serialize for Bytes<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_base64())
    }
}

impl<'de, const N: usize> Deserialize<'de> for Bytes<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Self::from_base64(&text).ok_or_else(|| serde::de::Error::custom(format!("expected {N} bytes of base64url")))
    }
}

/// Variable-length bytes (ciphertext), serialised as unpadded base64url.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Blob(pub Vec<u8>);

impl fmt::Debug for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Blob({} bytes)", self.0.len())
    }
}

impl Serialize for Blob {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Blob {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        URL_SAFE_NO_PAD.decode(text.as_bytes()).map(Self).map_err(|_| serde::de::Error::custom("expected base64url"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_checks_length() {
        let key = Key32::from_base64(&Bytes([0xfb; 32]).to_base64()).unwrap();
        assert_eq!(key, Bytes([0xfb; 32]));
        let json = serde_json::to_string(&key).unwrap();
        assert!(!json.contains('=') && !json.contains('+') && !json.contains('/'), "{json}");
        assert_eq!(serde_json::from_str::<Key32>(&json).unwrap(), key);
        assert!(serde_json::from_str::<Sig64>(&json).is_err(), "wrong length");
        assert!(serde_json::from_str::<Key32>("\"not base64!\"").is_err());
        let blob = Blob(vec![1, 2, 3]);
        assert_eq!(serde_json::from_str::<Blob>(&serde_json::to_string(&blob).unwrap()).unwrap(), blob);
        assert_eq!(format!("{blob:?}"), "Blob(3 bytes)");
    }
}
