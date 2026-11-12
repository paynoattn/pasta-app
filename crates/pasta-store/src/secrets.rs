//! The device's secrets in the OS credential store (#23): macOS Keychain,
//! Windows Credential Manager, Secret Service (KWallet / GNOME Keyring) on Linux.
//!
//! One entry holds everything, so a keychain prompt (e.g. after an update of
//! an unsigned build) comes up once, not once per key:
//!
//! - the device id and its Ed25519 seed (#22),
//! - the key of the encrypted local database (#25),
//! - the chain secret, when the device is in a chain (#20).
//!
//! The device id lives here, next to its key, so the two can't drift apart.

use std::sync::{Arc, Mutex};

use pasta_core::device::DeviceId;
use pasta_crypto::Secret32;
use uuid::Uuid;
use zeroize::Zeroizing;

/// Keychain service and account names. One entry per OS user.
pub const SERVICE: &str = "com.kingdomworks.pasta";
pub const ACCOUNT: &str = "device-secrets";

const MAGIC: &[u8; 8] = b"PASTA\x00S\x01";
const LEN_NO_CHAIN: usize = MAGIC.len() + 16 + 32 + 32 + 1;
const LEN_WITH_CHAIN: usize = LEN_NO_CHAIN + 32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// Locked, no credential service running, access denied: worth retrying.
    #[error("the {store} isn't available: {reason}")]
    Unavailable { store: &'static str, reason: String },
    /// The entry exists but isn't Pasta's format.
    #[error("the secrets entry in the {store} is damaged")]
    Corrupt { store: &'static str },
    /// Written by a newer Pasta. Left alone: overwriting it would lose the chain.
    #[error("the secrets entry in the {store} was written by a newer version of Pasta")]
    TooNew { store: &'static str },
}

/// Where the secrets blob lives. The real one is [`KeyringStore`]; tests use [`MemoryStore`].
pub trait SecretStore: Send + Sync {
    /// The blob, or `None` if there is none yet.
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError>;
    fn save(&self, blob: &[u8]) -> Result<(), SecretError>;
    /// What to call it in messages, e.g. "macOS Keychain".
    fn name(&self) -> &'static str;
}

/// What the keychain holds for this device.
#[derive(Debug)]
pub struct DeviceSecrets {
    pub device_id: DeviceId,
    pub device_key: Secret32,
    pub db_key: Secret32,
    pub chain: Option<Secret32>,
}

impl DeviceSecrets {
    pub fn generate(device_id: DeviceId) -> Self {
        Self { device_id, device_key: Secret32::random(), db_key: Secret32::random(), chain: None }
    }

    pub fn encode(&self) -> Zeroizing<Vec<u8>> {
        let mut out = Zeroizing::new(Vec::with_capacity(LEN_WITH_CHAIN));
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(self.device_id.0.as_bytes());
        out.extend_from_slice(self.device_key.expose());
        out.extend_from_slice(self.db_key.expose());
        match &self.chain {
            Some(chain) => {
                out.push(1);
                out.extend_from_slice(chain.expose());
            }
            None => out.push(0),
        }
        out
    }

    pub fn decode(blob: &[u8]) -> Option<Self> {
        if !blob.starts_with(MAGIC) || !(blob.len() == LEN_NO_CHAIN || blob.len() == LEN_WITH_CHAIN) {
            return None;
        }
        let rest = &blob[MAGIC.len()..];
        let device_id = DeviceId(Uuid::from_slice(&rest[..16]).ok()?);
        let device_key = Secret32::from_slice(&rest[16..48])?;
        let db_key = Secret32::from_slice(&rest[48..80])?;
        let chain = match (rest[80], blob.len()) {
            (0, LEN_NO_CHAIN) => None,
            (1, LEN_WITH_CHAIN) => Some(Secret32::from_slice(&rest[81..113])?),
            _ => return None,
        };
        Some(Self { device_id, device_key, db_key, chain })
    }
}

/// Loads the secrets, or creates them (with `new_device_id`) on first run.
/// Returns whether they were created.
pub fn load_or_create(
    store: &dyn SecretStore,
    new_device_id: impl FnOnce() -> DeviceId,
) -> Result<(DeviceSecrets, bool), SecretError> {
    if let Some(blob) = store.load()? {
        if blob.len() >= MAGIC.len()
            && blob[..MAGIC.len() - 1] == MAGIC[..MAGIC.len() - 1]
            && blob[MAGIC.len() - 1] > MAGIC[MAGIC.len() - 1]
        {
            return Err(SecretError::TooNew { store: store.name() });
        }
        let secrets = DeviceSecrets::decode(&blob).ok_or(SecretError::Corrupt { store: store.name() })?;
        return Ok((secrets, false));
    }
    let secrets = DeviceSecrets::generate(new_device_id());
    store.save(&secrets.encode())?;
    Ok((secrets, true))
}

/// The OS credential store. A fresh connection per call, so a store that
/// was locked or not running yet is picked up on retry.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    service: String,
    account: String,
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new(SERVICE, ACCOUNT)
    }
}

impl KeyringStore {
    pub fn new(service: &str, account: &str) -> Self {
        Self { service: service.to_owned(), account: account.to_owned() }
    }

    fn entry(&self) -> Result<keyring_core::Entry, SecretError> {
        let store = platform_store().map_err(|err| self.unavailable(&err))?;
        store.build(&self.service, &self.account, None).map_err(|err| self.unavailable(&err))
    }

    fn unavailable(&self, err: &keyring_core::Error) -> SecretError {
        SecretError::Unavailable { store: self.name(), reason: describe(err) }
    }

    /// Removes the entry (for tests and probes).
    pub fn delete(&self) -> Result<(), SecretError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(err) => Err(self.unavailable(&err)),
        }
    }
}

impl SecretStore for KeyringStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        match self.entry()?.get_secret() {
            Ok(blob) => Ok(Some(Zeroizing::new(blob))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(keyring_core::Error::BadDataFormat(mut blob, _)) => {
                zeroize::Zeroize::zeroize(&mut blob);
                Err(SecretError::Corrupt { store: self.name() })
            }
            Err(err) => Err(self.unavailable(&err)),
        }
    }

    fn save(&self, blob: &[u8]) -> Result<(), SecretError> {
        self.entry()?.set_secret(blob).map_err(|err| self.unavailable(&err))
    }

    fn name(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "macOS Keychain"
        } else if cfg!(windows) {
            "Windows Credential Manager"
        } else {
            "Secret Service (KWallet / GNOME Keyring)"
        }
    }
}

/// keyring errors can carry the secret bytes (`BadEncoding`); describe without them.
fn describe(err: &keyring_core::Error) -> String {
    match err {
        keyring_core::Error::PlatformFailure(inner) | keyring_core::Error::NoStorageAccess(inner) => inner.to_string(),
        keyring_core::Error::BadEncoding(_) | keyring_core::Error::BadDataFormat(..) => "unreadable entry".into(),
        other => format!("{other:?}").chars().take(200).collect(),
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

/// An in-memory store for tests, optionally "locked".
#[derive(Default)]
pub struct MemoryStore {
    blob: Mutex<Option<Vec<u8>>>,
    locked: Mutex<bool>,
}

impl MemoryStore {
    pub fn set_locked(&self, locked: bool) {
        *self.locked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = locked;
    }

    fn check(&self) -> Result<(), SecretError> {
        if *self.locked.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            return Err(SecretError::Unavailable { store: "test store", reason: "locked".into() });
        }
        Ok(())
    }
}

impl SecretStore for MemoryStore {
    fn load(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretError> {
        self.check()?;
        Ok(self.blob.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().map(Zeroizing::new))
    }

    fn save(&self, blob: &[u8]) -> Result<(), SecretError> {
        self.check()?;
        *self.blob.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(blob.to_vec());
        Ok(())
    }

    fn name(&self) -> &'static str {
        "test store"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_and_without_a_chain() {
        let mut secrets = DeviceSecrets::generate(DeviceId::random());
        let decoded = DeviceSecrets::decode(&secrets.encode()).unwrap();
        assert_eq!(decoded.device_id, secrets.device_id);
        assert_eq!(decoded.device_key, secrets.device_key);
        assert_eq!(decoded.db_key, secrets.db_key);
        assert!(decoded.chain.is_none());

        secrets.chain = Some(Secret32::random());
        let decoded = DeviceSecrets::decode(&secrets.encode()).unwrap();
        assert_eq!(decoded.chain, secrets.chain);

        let blob = secrets.encode();
        assert!(DeviceSecrets::decode(&blob[..blob.len() - 1]).is_none(), "truncated");
        assert!(DeviceSecrets::decode(b"not pasta").is_none());
        assert_eq!(format!("{secrets:?}").matches("redacted").count(), 3, "{secrets:?}");
    }

    #[test]
    fn creates_once_then_loads_and_reports_a_locked_store() {
        let store = MemoryStore::default();
        let id = DeviceId::random();
        let (first, created) = load_or_create(&store, || id).unwrap();
        assert!(created);
        assert_eq!(first.device_id, id);
        let (second, created) = load_or_create(&store, DeviceId::random).unwrap();
        assert!(!created);
        assert_eq!(second.device_key, first.device_key);

        store.set_locked(true);
        assert!(matches!(load_or_create(&store, DeviceId::random), Err(SecretError::Unavailable { .. })));
        store.set_locked(false);
        let mut newer = first.encode().to_vec();
        newer[MAGIC.len() - 1] = 2;
        store.save(&newer).unwrap();
        assert!(matches!(load_or_create(&store, DeviceId::random), Err(SecretError::TooNew { .. })));
        store.save(b"garbage").unwrap();
        assert!(matches!(load_or_create(&store, DeviceId::random), Err(SecretError::Corrupt { .. })));
    }

    /// Against the real OS store. Run with `cargo test -p pasta-store -- --ignored keyring`;
    /// needs an unlocked keychain (no CI runner has one).
    #[test]
    #[ignore = "uses the real OS credential store"]
    fn keyring_round_trip() {
        let store = KeyringStore::new("com.kingdomworks.pasta.test", &format!("probe-{}", Uuid::new_v4()));
        let secrets = DeviceSecrets::generate(DeviceId::random());
        store.save(&secrets.encode()).unwrap();
        let loaded = DeviceSecrets::decode(&store.load().unwrap().unwrap()).unwrap();
        assert_eq!(loaded.device_key, secrets.device_key);
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn any_higher_version_is_too_new_and_left_alone_while_other_blobs_are_corrupt() {
        let store = MemoryStore::default();
        let good = DeviceSecrets::generate(DeviceId::random()).encode().to_vec();
        let version_at = MAGIC.len() - 1;

        let mut longer = good.clone();
        longer[version_at] = 2;
        longer.extend_from_slice(&[0; 64]);
        let mut header_only = MAGIC[..version_at].to_vec();
        header_only.push(9);
        let mut newest = good.clone();
        newest[version_at] = 255;
        for blob in [longer, header_only, newest] {
            store.save(&blob).unwrap();
            assert!(matches!(load_or_create(&store, DeviceId::random), Err(SecretError::TooNew { .. })));
            assert_eq!(store.load().unwrap().unwrap().to_vec(), blob, "a newer entry was overwritten");
        }

        let mut older = good.clone();
        older[version_at] = 0;
        let mut foreign = good.clone();
        foreign[0] = b'Q';
        foreign[version_at] = 2;
        for blob in [older, foreign, good[..version_at].to_vec(), Vec::new()] {
            store.save(&blob).unwrap();
            assert!(matches!(load_or_create(&store, DeviceId::random), Err(SecretError::Corrupt { .. })), "{blob:?}");
        }
    }
}
