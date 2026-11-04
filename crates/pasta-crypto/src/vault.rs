//! Clipboard text Pasta holds on to, kept encrypted in memory (#24).
//!
//! The Mac history (#15) keeps up to 200 items for the panel. Rather than as
//! plain strings, each item is sealed and opened only while it's used:
//!
//! - **Windows:** `CryptProtectMemory(SAME_PROCESS)`. The key lives in the
//!   kernel, so a dump or read of Pasta's memory doesn't reveal it.
//! - **macOS / Linux:** XChaCha20-Poly1305 under a random per-run session key
//!   in a locked page. This keeps the text out of swap, core dumps and naive
//!   memory scans, but a process that can read all of Pasta's memory can
//!   find the key too (see `docs/SECURITY-MODEL.md`).

use zeroize::{Zeroize, Zeroizing};

#[cfg(not(windows))]
use crate::secret::Secret32;

/// Encrypts and decrypts [`Sealed`] buffers for this process.
pub struct Vault {
    #[cfg(not(windows))]
    key: Secret32,
}

/// Text sealed by a [`Vault`]. Useless outside the process that sealed it.
pub struct Sealed {
    #[cfg(windows)]
    data: Vec<u8>,
    #[cfg(windows)]
    len: usize,
    #[cfg(not(windows))]
    nonce: pasta_core::wire::Nonce24,
    #[cfg(not(windows))]
    data: Vec<u8>,
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sealed({} bytes)", self.data.len())
    }
}

impl Drop for Sealed {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("couldn't protect clipboard text in memory: {0}")]
pub struct VaultError(String);

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl Vault {
    pub fn new() -> Self {
        Self {
            #[cfg(not(windows))]
            key: Secret32::random(),
        }
    }

    #[cfg(windows)]
    pub fn seal(&self, plaintext: &[u8]) -> Result<Sealed, VaultError> {
        use windows::Win32::Security::Cryptography::{
            CRYPTPROTECTMEMORY_BLOCK_SIZE, CRYPTPROTECTMEMORY_SAME_PROCESS, CryptProtectMemory,
        };
        let block = CRYPTPROTECTMEMORY_BLOCK_SIZE as usize;
        let padded = plaintext.len().div_ceil(block).max(1) * block;
        let mut data = Vec::with_capacity(padded);
        data.extend_from_slice(plaintext);
        data.resize(padded, 0);
        let size = u32::try_from(padded).map_err(|_| VaultError("item too large".into()))?;
        // SAFETY: `data` is a live, writable buffer of exactly `size` bytes, a
        // multiple of the block size as the API requires.
        let result = unsafe { CryptProtectMemory(data.as_mut_ptr().cast(), size, CRYPTPROTECTMEMORY_SAME_PROCESS) };
        if let Err(err) = result {
            data.zeroize();
            return Err(VaultError(err.to_string()));
        }
        Ok(Sealed { data, len: plaintext.len() })
    }

    #[cfg(windows)]
    pub fn open(&self, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        use windows::Win32::Security::Cryptography::{CRYPTPROTECTMEMORY_SAME_PROCESS, CryptUnprotectMemory};
        let mut data = Zeroizing::new(sealed.data.clone());
        let size = u32::try_from(data.len()).map_err(|_| VaultError("item too large".into()))?;
        // SAFETY: as in `seal`; the copy has the same, block-aligned length.
        let result = unsafe { CryptUnprotectMemory(data.as_mut_ptr().cast(), size, CRYPTPROTECTMEMORY_SAME_PROCESS) };
        result.map_err(|err| VaultError(err.to_string()))?;
        data.truncate(sealed.len);
        Ok(data)
    }

    #[cfg(not(windows))]
    pub fn seal(&self, plaintext: &[u8]) -> Result<Sealed, VaultError> {
        let (nonce, data) = crate::seal::seal(&self.key, b"pasta/vault", plaintext);
        Ok(Sealed { nonce, data })
    }

    #[cfg(not(windows))]
    pub fn open(&self, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        crate::seal::open(&self.key, &sealed.nonce, b"pasta/vault", &sealed.data)
            .map_err(|err| VaultError(err.to_string()))
    }

    /// Seals clipboard flavors (as JSON).
    pub fn seal_flavors(&self, flavors: &pasta_core::clip::Flavors) -> Result<Sealed, VaultError> {
        let mut json = Zeroizing::new(Vec::with_capacity(flavors.byte_len().saturating_mul(2).saturating_add(64)));
        serde_json::to_writer(&mut *json, flavors).map_err(|err| VaultError(err.to_string()))?;
        self.seal(&json)
    }

    pub fn open_flavors(&self, sealed: &Sealed) -> Result<pasta_core::clip::Flavors, VaultError> {
        let json = self.open(sealed)?;
        serde_json::from_slice(&json).map_err(|_| VaultError("not clipboard flavors".into()))
    }
}

#[cfg(test)]
mod tests {
    use pasta_core::clip::Flavors;

    use super::*;

    #[test]
    fn round_trips_without_keeping_plaintext() {
        let vault = Vault::new();
        let canary = b"s3cr3t-clipboard-canary";
        for text in [&canary[..], b"", &[0u8; 16][..], &[1u8; 17][..]] {
            let sealed = vault.seal(text).unwrap();
            assert_eq!(vault.open(&sealed).unwrap().as_slice(), text);
            if !text.is_empty() {
                assert!(!sealed.data.windows(text.len()).any(|window| window == text), "plaintext left in sealed data");
            }
        }
        let flavors = Flavors { plain: "hello".into(), html: Some("<b>hello</b>".into()), rtf: None };
        let sealed = vault.seal_flavors(&flavors).unwrap();
        assert_eq!(vault.open_flavors(&sealed).unwrap(), flavors);
        assert!(format!("{sealed:?}").starts_with("Sealed("));
    }

    #[cfg(not(windows))]
    #[test]
    fn another_vault_cant_open_it() {
        let sealed = Vault::new().seal(b"hello").unwrap();
        assert!(Vault::new().open(&sealed).is_err());
    }
}
