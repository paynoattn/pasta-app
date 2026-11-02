//! Clipboard items as Pasta sees them: text flavors, content hash, source app.
//!
//! The `Debug` impls here never print clipboard text, only sizes and short
//! hashes, so these types are safe to put in log lines.

use std::fmt;
use std::sync::{OnceLock, RwLock};

use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

/// The text formats of one clipboard item. `text/plain` is always present.
///
/// The text is wiped from memory when the value is dropped (#24). Copies the
/// OS, serde or the allocator made along the way are out of reach.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flavors {
    #[serde(rename = "text/plain")]
    pub plain: String,
    /// An HTML document or fragment (not Windows' `CF_HTML` wrapper).
    #[serde(rename = "text/html", default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(rename = "text/rtf", default, skip_serializing_if = "Option::is_none")]
    pub rtf: Option<String>,
}

impl Flavors {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { plain: text.into(), html: None, rtf: None }
    }

    /// Total size of all flavors in bytes.
    pub fn byte_len(&self) -> usize {
        self.plain.len() + self.html.as_ref().map_or(0, String::len) + self.rtf.as_ref().map_or(0, String::len)
    }

    /// Hash of the plain text with line endings normalised.
    ///
    /// Only the plain text is hashed: Win+V history drops RTF and rewraps HTML
    /// (ADR 0004) and Universal Clipboard strips rich types (ADR 0005), so the
    /// same copy coming back through either must still hash the same.
    pub fn content_hash(&self) -> ContentHash {
        let mut hasher = Sha256::new();
        if self.plain.contains('\r') {
            hasher.update(self.plain.replace("\r\n", "\n").as_bytes());
        } else {
            hasher.update(self.plain.as_bytes());
        }
        ContentHash(hasher.finalize().into())
    }

    /// Drops rich flavors (RTF first, then HTML) until the item fits in
    /// `max_bytes`. Returns `false` if even the plain text alone is too big.
    pub fn fit_to(&mut self, max_bytes: usize) -> bool {
        if self.byte_len() > max_bytes {
            self.rtf = None;
        }
        if self.byte_len() > max_bytes {
            self.html = None;
        }
        self.byte_len() <= max_bytes
    }
}

impl Drop for Flavors {
    fn drop(&mut self) {
        self.plain.zeroize();
        self.html.zeroize();
        self.rtf.zeroize();
    }
}

impl fmt::Debug for Flavors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flavors")
            .field("plain_bytes", &self.plain.len())
            .field("html_bytes", &self.html.as_ref().map(String::len))
            .field("rtf_bytes", &self.rtf.as_ref().map(String::len))
            .finish()
    }
}

/// SHA-256 of an item's normalised plain text; used for dedupe and echo prevention.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// 12 hex digits for log lines: a keyed hash of the content hash, so short
    /// secrets can't be brute-forced from logs. Devices in the same chain use
    /// the same key (see [`set_log_key`]), so their logs still line up.
    pub fn short(&self) -> String {
        let key = current_log_key();
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).expect("HMAC takes any key length");
        mac.update(&self.0);
        mac.finalize().into_bytes()[..6].iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != 64 || !hex.is_ascii() {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.short())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(deserializer)?;
        Self::from_hex(&hex).ok_or_else(|| serde::de::Error::custom("content_hash must be 64 hex digits"))
    }
}

static LOG_KEY: RwLock<Option<[u8; 32]>> = RwLock::new(None);

/// Sets the key for the hashes in log lines: the chain's log key, or `None`
/// for a random one per process (when this device isn't in a chain).
pub fn set_log_key(key: Option<&[u8; 32]>) {
    let mut current = LOG_KEY.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(old) = current.as_mut() {
        old.zeroize();
    }
    *current = key.copied();
}

fn current_log_key() -> [u8; 32] {
    static FALLBACK: OnceLock<[u8; 32]> = OnceLock::new();
    if let Some(key) = *LOG_KEY.read().unwrap_or_else(std::sync::PoisonError::into_inner) {
        return key;
    }
    *FALLBACK.get_or_init(|| {
        let mut key = [0u8; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key
    })
}

/// The app an item was copied from, for the panel's source icon and for
/// excluded apps (#36).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceApp {
    /// Display name, e.g. `Safari`.
    pub name: String,
    /// macOS bundle id or Windows executable name, e.g. `com.apple.Safari`, `notepad.exe`.
    #[serde(default)]
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_ignores_rich_flavors_and_line_endings() {
        let plain = Flavors::plain("one\ntwo");
        let rich =
            Flavors { plain: "one\r\ntwo".into(), html: Some("<b>one</b>".into()), rtf: Some("{\\rtf1}".into()) };
        assert_eq!(plain.content_hash(), rich.content_hash());
        assert_ne!(plain.content_hash(), Flavors::plain("one two").content_hash());
    }

    #[test]
    fn hash_hex_round_trip() {
        let hash = Flavors::plain("x").content_hash();
        assert_eq!(ContentHash::from_hex(&hash.to_hex()), Some(hash));
        assert_eq!(hash.short().len(), 12);
        assert_eq!(ContentHash::from_hex("zz"), None);
        assert_eq!(ContentHash::from_hex(&"é".repeat(32)), None);
    }

    #[test]
    fn debug_output_never_contains_text() {
        let secret = "correct horse battery staple";
        let flavors = Flavors {
            plain: secret.into(),
            html: Some(format!("<p>{secret}</p>")),
            rtf: Some(format!("{{\\rtf1 {secret}}}")),
        };
        let debug = format!("{flavors:?} {:?}", flavors.content_hash());
        assert!(!debug.contains("horse"), "{debug}");
        assert!(debug.contains("plain_bytes: 28"), "{debug}");
    }

    #[test]
    fn fit_to_drops_rich_flavors_first() {
        let mut flavors = Flavors { plain: "a".repeat(10), html: Some("b".repeat(10)), rtf: Some("c".repeat(10)) };
        assert!(flavors.fit_to(25));
        assert!(flavors.rtf.is_none() && flavors.html.is_some());
        assert!(flavors.fit_to(10));
        assert!(flavors.html.is_none());
        assert!(!flavors.fit_to(9));
    }
}
