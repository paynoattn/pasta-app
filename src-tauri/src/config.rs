//! Device settings and known peers (#8), kept in the encrypted store (#25).
//!
//! On the first M2 start, M1's plain `config.json` is imported and deleted.
//! Since the file is gone, settings are changed in the Settings window (#29).
//! `PASTA_LOG` still overrides the log level.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pasta_core::config::Config;
use pasta_core::device::DeviceId;
use pasta_store::{Database, DbError};
use tokio::sync::watch;

use crate::storage::{M1_CONFIG_FILE, m1_config};

const ROW: &str = "config";

pub type ConfigError = DbError;

pub struct ConfigStore {
    db: Arc<Database>,
    /// What happened while opening, logged once logging is up.
    notices: Mutex<Vec<String>>,
    current: watch::Sender<Config>,
    /// Serialises [`ConfigStore::update`]: commands and the runtime loop change it concurrently.
    writing: Mutex<()>,
}

impl ConfigStore {
    /// Loads the config for `device_id`, importing M1's `config.json` from `dir` on first run.
    pub fn open(db: Arc<Database>, dir: &Path, device_id: DeviceId) -> Result<Self, ConfigError> {
        let mut notices = Vec::new();
        let mut imported_from: Option<PathBuf> = None;
        let mut config = match db.get::<Config>(ROW) {
            Ok(Some(config)) => config,
            Ok(None) => match m1_config(dir) {
                Some(config) => {
                    imported_from = Some(dir.join(M1_CONFIG_FILE));
                    config
                }
                None => {
                    notices.push(format!("created settings for device {device_id}"));
                    Config::new(device_id)
                }
            },
            Err(DbError::Corrupt { .. }) => {
                notices.push("the stored settings were unreadable; started from defaults".to_owned());
                Config::new(device_id)
            }
            Err(err) => return Err(err),
        };
        if config.device_id != device_id {
            notices.push(format!("device id follows the keychain: {device_id} (was {})", config.device_id));
            config.device_id = device_id;
        }
        db.put(ROW, &config)?;
        // Only once the settings are safely stored. The file has no secrets, but names and node ids:
        // remove it after an import, and on later starts if an earlier delete failed or it was unreadable.
        let m1_file = dir.join(M1_CONFIG_FILE);
        if m1_file.exists() {
            match fs::remove_file(&m1_file) {
                Ok(()) if imported_from.is_some() => {
                    notices.push(format!("moved {} into the encrypted store", m1_file.display()));
                }
                Ok(()) => notices.push(format!("removed the old plain {}", m1_file.display())),
                Err(err) => notices.push(format!("couldn't delete {}: {err}", m1_file.display())),
            }
        }
        Ok(Self { db, notices: Mutex::new(notices), current: watch::channel(config).0, writing: Mutex::new(()) })
    }

    /// Messages from [`ConfigStore::open`], for logging once it's set up.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// The database file the config lives in.
    pub fn path(&self) -> &Path {
        self.db.path()
    }

    pub fn get(&self) -> Config {
        self.current.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Config> {
        self.current.subscribe()
    }

    /// Changes the config and saves it.
    pub fn update(&self, change: impl FnOnce(&mut Config)) -> Result<(), ConfigError> {
        let _writing = self.writing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut config = self.get();
        change(&mut config);
        if config == *self.current.borrow() {
            return Ok(());
        }
        self.db.put(ROW, &config)?;
        self.current.send_replace(config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pasta_core::device::SyncMode;
    use pasta_crypto::Secret32;

    use super::*;

    fn db(dir: &Path, key: &Secret32) -> Arc<Database> {
        Arc::new(Database::open(&dir.join("pasta.db"), key.duplicate()).unwrap())
    }

    #[test]
    fn keeps_settings_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let key = Secret32::random();
        let id = DeviceId::random();
        let first = ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap();
        first.update(|config| config.mode = SyncMode::Receive).unwrap();
        drop(first);

        let second = ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap();
        assert_eq!(second.get().device_id, id);
        assert_eq!(second.get().mode, SyncMode::Receive);
    }

    #[test]
    fn imports_and_removes_the_m1_file() {
        let dir = tempfile::tempdir().unwrap();
        let id = DeviceId::random();
        let mut m1 = Config::new(id);
        m1.port = 50000;
        m1.known_peers.insert(
            DeviceId::random(),
            pasta_core::config::KnownPeer {
                name: "win-workstation".into(),
                os: "windows".into(),
                node_id: "nWin0000CNTRL".into(),
                last_seen: pasta_core::device::UnixMillis(1),
            },
        );
        fs::write(dir.path().join(M1_CONFIG_FILE), serde_json::to_vec(&m1).unwrap()).unwrap();

        let store = ConfigStore::open(db(dir.path(), &Secret32::random()), dir.path(), id).unwrap();
        assert_eq!(store.get(), m1);
        assert!(!dir.path().join(M1_CONFIG_FILE).exists(), "plain file left behind");
        assert!(store.take_notices().iter().any(|notice| notice.contains("encrypted store")));
        let bytes = fs::read(dir.path().join("pasta.db")).unwrap();
        assert!(!bytes.windows(15).any(|window| window == b"win-workstation"), "plaintext on disk");
    }

    #[test]
    fn device_id_follows_the_keychain() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(M1_CONFIG_FILE), format!(r#"{{"device_id":"{}"}}"#, DeviceId::random())).unwrap();
        let id = DeviceId::random();
        let store = ConfigStore::open(db(dir.path(), &Secret32::random()), dir.path(), id).unwrap();
        assert_eq!(store.get().device_id, id);
    }

    /// A `config.json` with a different port, to tell "imported" from "ignored".
    fn leftover(dir: &Path, id: DeviceId) -> PathBuf {
        let mut config = Config::new(id);
        config.port = 50001;
        let path = dir.join(M1_CONFIG_FILE);
        fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_leftover_config_json_is_deleted_without_being_imported_when_the_store_has_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let key = Secret32::random();
        let id = DeviceId::random();
        let first = ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap();
        let port = first.get().port;
        assert_ne!(port, 50001);
        drop(first);

        let path = leftover(dir.path(), id);
        let second = ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap();
        assert!(!path.exists(), "the plain file was left behind");
        assert_eq!(second.get().port, port, "the leftover overwrote the stored config");
        assert!(second.take_notices().iter().any(|notice| notice.contains("removed the old plain")));
    }

    #[test]
    fn an_unreadable_config_json_is_deleted_too() {
        let id = DeviceId::random();
        let key = Secret32::random();
        // On a fresh install, and next to a stored config.
        for stored in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            if stored {
                drop(ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap());
            }
            let path = dir.path().join(M1_CONFIG_FILE);
            fs::write(&path, b"{ not json, but it names win-workstation").unwrap();
            let store = ConfigStore::open(db(dir.path(), &key), dir.path(), id).unwrap();
            assert!(!path.exists(), "an unreadable config.json stayed (stored config: {stored})");
            assert_eq!(store.get().device_id, id);
        }
    }

    #[test]
    fn a_config_json_that_cant_be_deleted_is_reported_and_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        // A directory can't be removed as a file.
        let path = dir.path().join(M1_CONFIG_FILE);
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"x").unwrap();
        let store = ConfigStore::open(db(dir.path(), &Secret32::random()), dir.path(), DeviceId::random()).unwrap();
        assert!(store.take_notices().iter().any(|notice| notice.contains("couldn't delete")));
        assert!(path.exists());
    }
}
