//! Opening the keychain entry and the encrypted store at startup (#23, #25).
//!
//! The keychain can be locked or not running yet (e.g. at login); that's
//! [`StorageError::Locked`], and the caller retries. A database this device
//! can't decrypt (its keychain entry was lost) is moved aside and replaced.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pasta_core::config::Config;
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_store::secrets::{self, DeviceSecrets, SecretError, SecretStore};
use pasta_store::{Database, DbError};

pub const DB_FILE: &str = "pasta.db";
/// M1's plain config, imported once and then deleted (#25).
pub const M1_CONFIG_FILE: &str = "config.json";

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{0}")]
    Locked(SecretError),
    /// Not retried: a newer Pasta's keys, which this build mustn't overwrite.
    #[error("{0}")]
    Newer(SecretError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("couldn't access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub struct Storage {
    pub store: Arc<dyn SecretStore>,
    pub secrets: DeviceSecrets,
    pub db: Arc<Database>,
    /// What happened while opening, for the log.
    pub notices: Vec<String>,
}

pub fn open(dir: &Path, store: Arc<dyn SecretStore>) -> Result<Storage, StorageError> {
    fs::create_dir_all(dir).map_err(|source| StorageError::Io { path: dir.to_owned(), source })?;
    let mut notices = Vec::new();
    // An M1 install keeps its device id, so peers' node bindings (#11) still match.
    let m1_device_id = || m1_config(dir).map(|config| config.device_id).unwrap_or_else(DeviceId::random);
    let secrets = match secrets::load_or_create(store.as_ref(), m1_device_id) {
        Ok((secrets, created)) => {
            if created {
                notices.push(format!("created device keys in the {} for {}", store.name(), secrets.device_id));
            }
            secrets
        }
        Err(SecretError::Corrupt { store: name }) => {
            notices.push(format!("the secrets entry in the {name} was damaged; created a new device identity"));
            let secrets = DeviceSecrets::generate(DeviceId::random());
            store.save(&secrets.encode()).map_err(StorageError::Locked)?;
            secrets
        }
        Err(err @ SecretError::TooNew { .. }) => return Err(StorageError::Newer(err)),
        Err(err) => return Err(StorageError::Locked(err)),
    };
    let path = dir.join(DB_FILE);
    let db = match Database::open(&path, secrets.db_key.duplicate()) {
        Ok(db) => db,
        Err(DbError::WrongKey { .. }) => {
            let aside = path.with_extension(format!("db.unreadable-{}", UnixMillis::now().0));
            fs::rename(&path, &aside).map_err(|source| StorageError::Io { path: path.clone(), source })?;
            notices.push(format!(
                "{} couldn't be decrypted with this device's key (was the keychain entry removed?); moved it to {} and started fresh",
                path.display(),
                aside.display()
            ));
            Database::open(&path, secrets.db_key.duplicate())?
        }
        Err(err) => return Err(err.into()),
    };
    Ok(Storage { store, secrets, db: Arc::new(db), notices })
}

/// M1's `config.json`, if present and readable.
pub fn m1_config(dir: &Path) -> Option<Config> {
    let bytes = fs::read(dir.join(M1_CONFIG_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use pasta_store::MemoryStore;

    use super::*;

    #[test]
    fn first_run_keeps_the_m1_device_id() {
        let dir = tempfile::tempdir().unwrap();
        let id = DeviceId::random();
        fs::write(dir.path().join(M1_CONFIG_FILE), format!(r#"{{"device_id":"{id}"}}"#)).unwrap();
        let storage = open(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        assert_eq!(storage.secrets.device_id, id);
        assert!(dir.path().join(DB_FILE).exists());
    }

    #[test]
    fn a_locked_keychain_is_retryable_and_a_lost_key_starts_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        store.set_locked(true);
        assert!(matches!(open(dir.path(), store.clone()), Err(StorageError::Locked(_))));
        store.set_locked(false);
        let first = open(dir.path(), store.clone()).unwrap();
        first.db.put("x", &1).unwrap();
        drop(first);

        // The keychain entry is gone: new keys, and the old database is moved aside.
        let storage = open(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        assert_eq!(storage.db.get::<i32>("x").unwrap(), None);
        assert!(storage.notices.iter().any(|notice| notice.contains("moved it to")));
        let aside = fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".unreadable-"));
        assert_eq!(aside.count(), 1);
    }

    /// Replaces the table with one whose `value` column is TEXT, so reading the
    /// check row fails for a reason that has nothing to do with the key.
    fn make_rows_unreadable(path: &Path) {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch(
                "DROP TABLE kv;
                 CREATE TABLE kv (name TEXT PRIMARY KEY NOT NULL, nonce BLOB NOT NULL, value TEXT NOT NULL);
                 INSERT INTO kv VALUES ('check', zeroblob(24), 'text where a blob belongs');",
            )
            .unwrap();
    }

    fn moved_aside(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().contains(".unreadable-"))
            .count()
    }

    #[test]
    fn a_database_that_cant_be_read_is_reported_not_moved_aside_as_a_lost_key() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        drop(open(dir.path(), store.clone()).unwrap());
        let db_path = dir.path().join(DB_FILE);
        make_rows_unreadable(&db_path);

        let err = open(dir.path(), store.clone()).err().expect("an unreadable database must not open");
        assert!(matches!(err, StorageError::Db(DbError::Sqlite { .. })), "{err}");
        assert_eq!(moved_aside(dir.path()), 0, "the database was moved aside as if its key were lost");
        let rows: i64 = rusqlite::Connection::open(&db_path)
            .unwrap()
            .query_row("SELECT count(*) FROM kv WHERE name = 'check'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1, "the database file was replaced");
    }

    #[test]
    fn a_keychain_entry_from_a_newer_pasta_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let mut blob = DeviceSecrets::generate(DeviceId::random()).encode().to_vec();
        blob[7] = 2; // the format version, right after the 7-byte magic
        store.save(&blob).unwrap();

        let err = open(dir.path(), store.clone()).err().expect("a newer entry must not open");
        assert!(matches!(err, StorageError::Newer(SecretError::TooNew { .. })), "{err}");
        assert_eq!(store.load().unwrap().unwrap().to_vec(), blob, "the newer entry was overwritten");
        assert!(!dir.path().join(DB_FILE).exists(), "a database was created next to keys it doesn't know");
        // Still not retried as if the keychain were merely locked.
        assert!(!matches!(err, StorageError::Locked(_)));
    }
}
