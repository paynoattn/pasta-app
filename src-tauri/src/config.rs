//! `config.json` in the app data dir (#8): device id, settings, known peers.
//!
//! Written atomically (temp file + rename). Edits made to the file while Pasta
//! runs are picked up by [`ConfigStore::reload_if_changed`].

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use pasta_core::config::Config;
use pasta_core::device::DeviceId;
use tokio::sync::watch;

const FILE_NAME: &str = "config.json";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("couldn't access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct ConfigStore {
    path: PathBuf,
    /// What happened while opening, logged once logging is up.
    notices: Mutex<Vec<String>>,
    current: watch::Sender<Config>,
    /// Modification time of the file as Pasta last wrote or read it.
    seen_mtime: Mutex<Option<SystemTime>>,
}

impl ConfigStore {
    /// Loads the config from `dir`, creating it (and a new device id) on first run.
    pub fn open(dir: &Path) -> Result<Self, ConfigError> {
        fs::create_dir_all(dir).map_err(|source| ConfigError::Io { path: dir.to_owned(), source })?;
        let path = dir.join(FILE_NAME);
        let mut notices = Vec::new();
        let loaded = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Config>(&bytes) {
                Ok(config) => Some(config),
                Err(err) => {
                    let backup = path.with_extension(format!("json.bad-{}", pasta_core::device::UnixMillis::now().0));
                    notices.push(format!("config.json is invalid ({err}); moved it to {}", backup.display()));
                    let _ = fs::rename(&path, &backup);
                    None
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(source) => return Err(ConfigError::Io { path, source }),
        };
        let fresh = loaded.is_none();
        let config = loaded.unwrap_or_else(|| Config::new(DeviceId::random()));
        if fresh {
            notices.push(format!("created a new device identity {}", config.device_id));
        }
        let store = Self {
            path,
            notices: Mutex::new(notices),
            current: watch::channel(config).0,
            seen_mtime: Mutex::new(None),
        };
        if fresh {
            store.save(&store.get())?;
        } else {
            *store.seen_mtime.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = store.mtime();
        }
        Ok(store)
    }

    /// Messages from [`ConfigStore::open`], for logging once it's set up.
    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.notices.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> Config {
        self.current.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Config> {
        self.current.subscribe()
    }

    /// Changes the config and saves it.
    pub fn update(&self, change: impl FnOnce(&mut Config)) -> Result<(), ConfigError> {
        let mut config = self.get();
        change(&mut config);
        if config == *self.current.borrow() {
            return Ok(());
        }
        self.save(&config)?;
        self.current.send_replace(config);
        Ok(())
    }

    /// Re-reads the file if something else changed it. Returns whether it did.
    pub fn reload_if_changed(&self) -> bool {
        let mtime = self.mtime();
        let mut seen = self.seen_mtime.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if mtime.is_none() || mtime == *seen {
            return false;
        }
        *seen = mtime;
        drop(seen);
        let config = match fs::read(&self.path).map(|bytes| serde_json::from_slice::<Config>(&bytes)) {
            Ok(Ok(config)) => config,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "ignoring an invalid edit to config.json");
                return false;
            }
            Err(err) => {
                tracing::warn!(error = %err, "couldn't re-read config.json");
                return false;
            }
        };
        let current = self.get();
        let config = if config.device_id == current.device_id {
            config
        } else {
            tracing::warn!("device_id in config.json can't be changed while Pasta runs; keeping the old one");
            Config { device_id: current.device_id, ..config }
        };
        if config == current {
            return false;
        }
        tracing::info!("reloaded config.json");
        self.current.send_replace(config);
        true
    }

    fn mtime(&self) -> Option<SystemTime> {
        fs::metadata(&self.path).and_then(|meta| meta.modified()).ok()
    }

    fn save(&self, config: &Config) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io { path: self.path.clone(), source };
        let tmp = self.path.with_extension("json.tmp");
        let mut json = serde_json::to_vec_pretty(config).unwrap_or_default();
        json.push(b'\n');
        let mut file = fs::File::create(&tmp).map_err(io)?;
        file.write_all(&json).and_then(|()| file.sync_all()).map_err(io)?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(io)?;
        *self.seen_mtime.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = self.mtime();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pasta_core::device::SyncMode;

    use super::*;

    #[test]
    fn keeps_the_device_id_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let first = ConfigStore::open(dir.path()).unwrap();
        let id = first.get().device_id;
        first.update(|config| config.mode = SyncMode::Receive).unwrap();
        drop(first);

        let second = ConfigStore::open(dir.path()).unwrap();
        assert_eq!(second.get().device_id, id);
        assert_eq!(second.get().mode, SyncMode::Receive);
    }

    #[test]
    fn picks_up_edits_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::open(dir.path()).unwrap();
        let changes = store.subscribe();
        assert!(!store.reload_if_changed(), "own writes aren't reloads");

        let mut edited = store.get();
        edited.port = 50000;
        edited.device_id = DeviceId::random();
        // Make sure the modification time moves even on coarse-grained filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(store.path(), serde_json::to_vec(&edited).unwrap()).unwrap();
        let file = fs::File::options().write(true).open(store.path()).unwrap();
        file.set_modified(SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();

        assert!(store.reload_if_changed());
        assert!(changes.has_changed().unwrap());
        assert_eq!(store.get().port, 50000);
        assert_ne!(store.get().device_id, edited.device_id, "device id is kept");
    }

    #[test]
    fn replaces_an_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), b"{ not json").unwrap();
        let store = ConfigStore::open(dir.path()).unwrap();
        assert_eq!(store.get().port, pasta_core::config::DEFAULT_PORT);
        let backups = fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".bad-"));
        assert_eq!(backups.count(), 1);
    }
}
