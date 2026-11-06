//! The encrypted-at-rest local store (#25): SQLite with every value sealed.
//!
//! Each value is serialised to JSON and sealed with XChaCha20-Poly1305 under
//! the database key from the OS keyring (#23), with the table and row name as
//! associated data, so a value can't be moved to another row. Names, row
//! counts and sizes are visible on disk; contents aren't.
//!
//! Migrations are numbered SQL scripts applied in order, tracked in
//! `PRAGMA user_version`. A `check` row written on creation detects a wrong
//! key (e.g. after the keychain entry was lost) before anything else is read.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pasta_core::wire::Bytes;
use pasta_crypto::Secret32;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

/// Schema migrations, in order. Never edit one that shipped; add another.
const MIGRATIONS: &[&str] = &[
    // 1: key/value rows for settings, peers and the chain's member list.
    "CREATE TABLE kv (name TEXT PRIMARY KEY NOT NULL, nonce BLOB NOT NULL, value BLOB NOT NULL) STRICT;",
    // 2: the macOS clipboard history (#37). Times are plain for ordering and pruning;
    // everything else is in the sealed value. See `crate::history`.
    "CREATE TABLE history (
        item_id TEXT PRIMARY KEY NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER,
        nonce BLOB NOT NULL,
        value BLOB NOT NULL
     ) STRICT;
     CREATE INDEX history_by_time ON history (created_at DESC);",
];

const CHECK_ROW: &str = "check";
const CHECK_VALUE: &str = "pasta";

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error in {path}: {source}")]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error("{path} can't be decrypted with this device's key")]
    WrongKey { path: PathBuf },
    #[error("{path} was written by a newer Pasta (schema {found}, this build knows {known})")]
    TooNew { path: PathBuf, found: u32, known: u32 },
    #[error("row `{name}` doesn't decrypt or parse")]
    Corrupt { name: String },
}

pub struct Database {
    path: PathBuf,
    conn: Mutex<Connection>,
    key: Secret32,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").field("path", &self.path).finish_non_exhaustive()
    }
}

impl Database {
    /// Opens (or creates) the database at `path`, migrating it to the latest schema.
    pub fn open(path: &Path, key: Secret32) -> Result<Self, DbError> {
        let sqlite = |source| DbError::Sqlite { path: path.to_owned(), source };
        let conn = Connection::open(path).map_err(sqlite)?;
        // Deleted rows are overwritten, not left in free pages.
        conn.execute_batch("PRAGMA secure_delete = ON; PRAGMA journal_mode = DELETE; PRAGMA foreign_keys = ON;")
            .map_err(sqlite)?;
        let db = Self { path: path.to_owned(), conn: Mutex::new(conn), key };
        let fresh = db.migrate()?;
        if fresh {
            db.put(CHECK_ROW, &CHECK_VALUE)?;
        } else {
            // Only a check row that is missing or doesn't decrypt means another key;
            // a read error (busy, I/O) is reported as such, never as a wrong key.
            match db.get::<String>(CHECK_ROW) {
                Ok(Some(value)) if value == CHECK_VALUE => {}
                Ok(_) | Err(DbError::Corrupt { .. }) => return Err(DbError::WrongKey { path: path.to_owned() }),
                Err(err) => return Err(err),
            }
        }
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn key(&self) -> &Secret32 {
        &self.key
    }

    pub(crate) fn sqlite(&self, source: rusqlite::Error) -> DbError {
        DbError::Sqlite { path: self.path.clone(), source }
    }

    /// Applies pending migrations. Returns whether the database was new.
    fn migrate(&self) -> Result<bool, DbError> {
        let mut conn = self.lock();
        let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0)).map_err(|e| self.sqlite(e))?;
        let known = u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX);
        if version > known {
            return Err(DbError::TooNew { path: self.path.clone(), found: version, known });
        }
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = conn.transaction().map_err(|e| self.sqlite(e))?;
            tx.execute_batch(sql).map_err(|e| self.sqlite(e))?;
            tx.pragma_update(None, "user_version", u32::try_from(index + 1).unwrap_or(u32::MAX))
                .map_err(|e| self.sqlite(e))?;
            tx.commit().map_err(|e| self.sqlite(e))?;
            tracing::info!(schema = index + 1, "migrated the local database");
        }
        Ok(version == 0)
    }

    pub fn schema_version(&self) -> Result<u32, DbError> {
        self.lock().query_row("PRAGMA user_version", [], |row| row.get(0)).map_err(|e| self.sqlite(e))
    }

    pub fn put<T: Serialize + ?Sized>(&self, name: &str, value: &T) -> Result<(), DbError> {
        let mut json = Zeroizing::new(Vec::with_capacity(1024));
        serde_json::to_writer(&mut *json, value).map_err(|_| DbError::Corrupt { name: name.to_owned() })?;
        let (nonce, sealed) = pasta_crypto::seal::seal(&self.key, &aad(name), &json);
        self.lock()
            .execute(
                "INSERT INTO kv (name, nonce, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (name) DO UPDATE SET nonce = excluded.nonce, value = excluded.value",
                params![name, &nonce.0[..], sealed],
            )
            .map_err(|e| self.sqlite(e))?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, DbError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = self
            .lock()
            .query_row("SELECT nonce, value FROM kv WHERE name = ?1", params![name], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(|e| self.sqlite(e))?;
        let Some((nonce, sealed)) = row else { return Ok(None) };
        let corrupt = || DbError::Corrupt { name: name.to_owned() };
        let nonce = Bytes(<[u8; 24]>::try_from(nonce.as_slice()).map_err(|_| corrupt())?);
        let json = pasta_crypto::seal::open(&self.key, &nonce, &aad(name), &sealed).map_err(|_| corrupt())?;
        serde_json::from_slice(&json).map(Some).map_err(|_| corrupt())
    }

    pub fn delete(&self, name: &str) -> Result<(), DbError> {
        self.lock().execute("DELETE FROM kv WHERE name = ?1", params![name]).map_err(|e| self.sqlite(e))?;
        Ok(())
    }
}

fn aad(name: &str) -> Vec<u8> {
    let mut aad = b"pasta/store/v1/kv/".to_vec();
    aad.extend_from_slice(name.as_bytes());
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Secret32 {
        let mut bytes = [9u8; 32];
        Secret32::from_bytes(&mut bytes)
    }

    #[test]
    fn stores_sealed_values_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        let canary = "s3cr3t-clipboard-canary";
        {
            let db = Database::open(&path, key()).unwrap();
            assert_eq!(db.schema_version().unwrap(), 2);
            db.put("config", &serde_json::json!({ "name": canary })).unwrap();
            assert_eq!(db.get::<serde_json::Value>("missing").unwrap(), None);
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(canary.len()).any(|window| window == canary.as_bytes()), "plaintext on disk");

        let db = Database::open(&path, key()).unwrap();
        let value: serde_json::Value = db.get("config").unwrap().unwrap();
        assert_eq!(value["name"], canary);
        db.delete("config").unwrap();
        assert_eq!(db.get::<serde_json::Value>("config").unwrap(), None);
    }

    #[test]
    fn refuses_a_wrong_key_and_swapped_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        let db = Database::open(&path, key()).unwrap();
        db.put("a", &1).unwrap();
        db.put("b", &2).unwrap();
        // Copy row b's sealed value over row a: the AAD (row name) no longer matches.
        db.lock()
            .execute("UPDATE kv SET nonce = (SELECT nonce FROM kv WHERE name = 'b'), value = (SELECT value FROM kv WHERE name = 'b') WHERE name = 'a'", [])
            .unwrap();
        assert!(matches!(db.get::<i32>("a"), Err(DbError::Corrupt { .. })));
        drop(db);
        assert!(matches!(Database::open(&path, Secret32::random()), Err(DbError::WrongKey { .. })));
    }

    #[test]
    fn refuses_a_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        drop(Database::open(&path, key()).unwrap());
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Database::open(&path, key()), Err(DbError::TooNew { found: 99, .. })));
    }

    /// Replaces the table with one whose `value` column is TEXT, so reading any
    /// row fails (not the key's fault), and puts a `check` row in it.
    fn make_rows_unreadable(path: &Path) {
        Connection::open(path)
            .unwrap()
            .execute_batch(
                "DROP TABLE kv;
                 CREATE TABLE kv (name TEXT PRIMARY KEY NOT NULL, nonce BLOB NOT NULL, value TEXT NOT NULL);
                 INSERT INTO kv VALUES ('check', zeroblob(24), 'text where a blob belongs');",
            )
            .unwrap();
    }

    #[test]
    fn a_read_error_on_the_check_row_is_not_reported_as_a_wrong_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        drop(Database::open(&path, key()).unwrap());
        make_rows_unreadable(&path);

        // The right key and an unrelated one give the same answer: the file can't be read.
        for key in [key(), Secret32::random()] {
            let err = Database::open(&path, key).unwrap_err();
            assert!(matches!(err, DbError::Sqlite { .. }), "expected a database error, got: {err}");
        }
        assert!(path.exists());
    }

    #[test]
    fn a_missing_or_undecryptable_check_row_means_another_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        let edit = |sql: &str| Connection::open(&path).unwrap().execute_batch(sql).unwrap();

        drop(Database::open(&path, key()).unwrap());
        edit("DELETE FROM kv WHERE name = 'check'");
        assert!(matches!(Database::open(&path, key()), Err(DbError::WrongKey { .. })), "no check row");

        edit("INSERT INTO kv VALUES ('check', zeroblob(24), zeroblob(40))");
        assert!(matches!(Database::open(&path, key()), Err(DbError::WrongKey { .. })), "doesn't decrypt");
        edit("UPDATE kv SET nonce = zeroblob(5) WHERE name = 'check'");
        assert!(matches!(Database::open(&path, key()), Err(DbError::WrongKey { .. })), "nonce of the wrong length");
    }

    #[test]
    fn a_file_that_is_not_a_database_is_a_database_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        std::fs::write(&path, b"this is not a SQLite file, and it is long enough to be read as a header....").unwrap();
        let err = Database::open(&path, key()).unwrap_err();
        assert!(matches!(err, DbError::Sqlite { .. }), "{err}");
        assert!(path.exists());
    }
}
