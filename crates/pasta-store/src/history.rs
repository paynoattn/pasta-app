//! The macOS clipboard history (#37), in the encrypted database.
//!
//! One row per item. The creation and expiry times are plain columns so rows
//! can be ordered and pruned without decrypting them; the text, its source
//! app, origin device and hash are sealed like every other value, with the
//! item id as associated data.
//!
//! Search decrypts rows one at a time, newest first, and matches in memory
//! (ADR 0011): an FTS index would keep the text on disk in the clear. Every
//! decrypted copy is wiped when dropped.

use std::sync::Arc;

use pasta_core::clip::{ContentHash, Flavors, ItemKind, SourceApp};
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::wire::Bytes;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::db::{Database, DbError};

/// One item in the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    pub item_id: Uuid,
    pub created_at: UnixMillis,
    /// Received items expire with this device's expiry setting (#33); `None` follows retention.
    pub expires_at: Option<UnixMillis>,
    pub flavors: Flavors,
    pub source_app: Option<SourceApp>,
    pub origin_device: DeviceId,
    pub content_hash: ContentHash,
    /// Came from another device rather than a copy on this one.
    pub received: bool,
}

impl HistoryItem {
    pub fn kind(&self) -> ItemKind {
        ItemKind::of(&self.flavors)
    }
}

/// The panel's filter chips (#38).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryFilter {
    #[default]
    All,
    Text,
    Links,
    Code,
    /// Received from the user's other devices.
    FromOtherDevices,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Case-insensitive; matched against the text and the source app's name.
    pub text: Option<String>,
    pub filter: HistoryFilter,
    pub limit: usize,
}

/// What [`HistoryStore::insert`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inserted {
    New,
    /// Same text as the newest item, whose row (this id) it replaced instead of being
    /// repeated. The new item keeps its own id, so its expiry finds it.
    Replaced(Uuid),
}

/// Sealed part of a row.
#[derive(Serialize, Deserialize)]
struct Stored {
    flavors: Flavors,
    #[serde(default)]
    source_app: Option<SourceApp>,
    origin_device: DeviceId,
    content_hash: ContentHash,
    #[serde(default)]
    received: bool,
}

pub struct HistoryStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for HistoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryStore").finish_non_exhaustive()
    }
}

impl HistoryStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Adds an item at the top. Copying the newest item's text again replaces
    /// that item instead of repeating it.
    pub fn insert(&self, item: &HistoryItem) -> Result<Inserted, DbError> {
        let mut outcome = Inserted::New;
        if let Some(newest) = self.newest()?
            && newest.content_hash == item.content_hash
            && newest.item_id != item.item_id
        {
            self.remove(newest.item_id)?;
            outcome = Inserted::Replaced(newest.item_id);
        }
        let stored = Stored {
            flavors: item.flavors.clone(),
            source_app: item.source_app.clone(),
            origin_device: item.origin_device,
            content_hash: item.content_hash,
            received: item.received,
        };
        let mut json = Zeroizing::new(Vec::with_capacity(item.flavors.byte_len().saturating_mul(2) + 512));
        serde_json::to_writer(&mut *json, &stored).map_err(|_| self.corrupt(item.item_id))?;
        let (nonce, sealed) = pasta_crypto::seal::seal(self.db.key(), &aad(item.item_id), &json);
        self.db
            .lock()
            .execute(
                "INSERT INTO history (item_id, created_at, expires_at, nonce, value) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (item_id) DO UPDATE SET created_at = excluded.created_at,
                   expires_at = excluded.expires_at, nonce = excluded.nonce, value = excluded.value",
                params![
                    item.item_id.to_string(),
                    millis(item.created_at),
                    item.expires_at.map(millis),
                    &nonce.0[..],
                    sealed
                ],
            )
            .map_err(|e| self.db.sqlite(e))?;
        Ok(outcome)
    }

    pub fn get(&self, item_id: Uuid) -> Result<Option<HistoryItem>, DbError> {
        let row = self
            .db
            .lock()
            .query_row(
                "SELECT item_id, created_at, expires_at, nonce, value FROM history WHERE item_id = ?1",
                params![item_id.to_string()],
                Row::read,
            )
            .optional()
            .map_err(|e| self.db.sqlite(e))?;
        row.map(|row| self.open(row)).transpose()
    }

    fn newest(&self) -> Result<Option<HistoryItem>, DbError> {
        let row = self
            .db
            .lock()
            .query_row(
                "SELECT item_id, created_at, expires_at, nonce, value FROM history
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [],
                Row::read,
            )
            .optional()
            .map_err(|e| self.db.sqlite(e))?;
        row.map(|row| self.open(row)).transpose()
    }

    /// Items matching `query`, newest first. Rows that don't decrypt are skipped.
    pub fn list(&self, query: &HistoryQuery) -> Result<Vec<HistoryItem>, DbError> {
        let needle = query.text.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(lowercase);
        let limit = if query.limit == 0 { usize::MAX } else { query.limit };
        let conn = self.db.lock();
        let mut statement = conn
            .prepare_cached(
                "SELECT item_id, created_at, expires_at, nonce, value FROM history ORDER BY created_at DESC, rowid DESC",
            )
            .map_err(|e| self.db.sqlite(e))?;
        let rows = statement.query_map([], Row::read).map_err(|e| self.db.sqlite(e))?;
        let mut found = Vec::new();
        for row in rows {
            let row = row.map_err(|e| self.db.sqlite(e))?;
            let item_id = row.item_id.clone();
            let Ok(item) = self.open(row) else {
                tracing::warn!(item = %item_id, "skipped a history row that doesn't decrypt");
                continue;
            };
            if matches(&item, query.filter, needle.as_deref().map(String::as_str)) {
                found.push(item);
                if found.len() >= limit {
                    break;
                }
            }
        }
        Ok(found)
    }

    pub fn len(&self) -> Result<usize, DbError> {
        let count: i64 = self
            .db
            .lock()
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .map_err(|e| self.db.sqlite(e))?;
        Ok(usize::try_from(count).unwrap_or_default())
    }

    pub fn is_empty(&self) -> Result<bool, DbError> {
        self.len().map(|len| len == 0)
    }

    /// Removes one item. Returns whether it was there.
    pub fn remove(&self, item_id: Uuid) -> Result<bool, DbError> {
        let changed = self
            .db
            .lock()
            .execute("DELETE FROM history WHERE item_id = ?1", params![item_id.to_string()])
            .map_err(|e| self.db.sqlite(e))?;
        Ok(changed > 0)
    }

    /// Ids of every item received from another device.
    pub fn received_ids(&self) -> Result<Vec<Uuid>, DbError> {
        let query = HistoryQuery { text: None, filter: HistoryFilter::FromOtherDevices, limit: 0 };
        Ok(self.list(&query)?.into_iter().map(|item| item.item_id).collect())
    }

    /// Removes every item received from another device. Returns how many.
    pub fn remove_received(&self) -> Result<usize, DbError> {
        let query = HistoryQuery { text: None, filter: HistoryFilter::FromOtherDevices, limit: 0 };
        let mut removed = 0;
        for item in self.list(&query)? {
            if self.remove(item.item_id)? {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Removes everything. Returns how many items there were.
    pub fn clear(&self) -> Result<usize, DbError> {
        self.db.lock().execute("DELETE FROM history", []).map_err(|e| self.db.sqlite(e))
    }

    /// Moves an item to the top, e.g. after it was pasted again.
    pub fn touch(&self, item_id: Uuid, now: UnixMillis) -> Result<bool, DbError> {
        let changed = self
            .db
            .lock()
            .execute("UPDATE history SET created_at = ?2 WHERE item_id = ?1", params![item_id.to_string(), millis(now)])
            .map_err(|e| self.db.sqlite(e))?;
        Ok(changed > 0)
    }

    /// Sets a received item's expiry, e.g. after the expiry setting changed.
    pub fn set_expiry(&self, item_id: Uuid, expires_at: Option<UnixMillis>) -> Result<(), DbError> {
        self.db
            .lock()
            .execute(
                "UPDATE history SET expires_at = ?2 WHERE item_id = ?1",
                params![item_id.to_string(), expires_at.map(millis)],
            )
            .map_err(|e| self.db.sqlite(e))?;
        Ok(())
    }

    /// Applies retention (#35): drops items past their expiry, items without
    /// one older than `keep_days`, and everything past the newest `max_items`.
    /// Returns how many items went.
    pub fn prune(&self, now: UnixMillis, max_items: usize, keep_days: Option<u32>) -> Result<usize, DbError> {
        let conn = self.db.lock();
        let sqlite = |e| self.db.sqlite(e);
        let mut removed = conn
            .execute("DELETE FROM history WHERE expires_at IS NOT NULL AND expires_at <= ?1", params![millis(now)])
            .map_err(sqlite)?;
        if let Some(days) = keep_days {
            let cutoff = now.0.saturating_sub(u64::from(days).saturating_mul(86_400_000));
            removed += conn
                .execute(
                    "DELETE FROM history WHERE expires_at IS NULL AND created_at < ?1",
                    params![i64::try_from(cutoff).unwrap_or(i64::MAX)],
                )
                .map_err(sqlite)?;
        }
        removed += conn
            .execute(
                "DELETE FROM history WHERE item_id NOT IN
                   (SELECT item_id FROM history ORDER BY created_at DESC, rowid DESC LIMIT ?1)",
                params![i64::try_from(max_items).unwrap_or(i64::MAX)],
            )
            .map_err(sqlite)?;
        Ok(removed)
    }

    fn open(&self, row: Row) -> Result<HistoryItem, DbError> {
        let item_id: Uuid = row.item_id.parse().map_err(|_| DbError::Corrupt { name: "history".into() })?;
        let corrupt = || self.corrupt(item_id);
        let nonce = Bytes(<[u8; 24]>::try_from(row.nonce.as_slice()).map_err(|_| corrupt())?);
        let json = pasta_crypto::seal::open(self.db.key(), &nonce, &aad(item_id), &row.value).map_err(|_| corrupt())?;
        let stored: Stored = serde_json::from_slice(&json).map_err(|_| corrupt())?;
        Ok(HistoryItem {
            item_id,
            created_at: UnixMillis(u64::try_from(row.created_at).unwrap_or_default()),
            expires_at: row.expires_at.map(|at| UnixMillis(u64::try_from(at).unwrap_or_default())),
            flavors: stored.flavors,
            source_app: stored.source_app,
            origin_device: stored.origin_device,
            content_hash: stored.content_hash,
            received: stored.received,
        })
    }

    fn corrupt(&self, item_id: Uuid) -> DbError {
        DbError::Corrupt { name: format!("history/{item_id}") }
    }
}

struct Row {
    item_id: String,
    created_at: i64,
    expires_at: Option<i64>,
    nonce: Vec<u8>,
    value: Vec<u8>,
}

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            item_id: row.get(0)?,
            created_at: row.get(1)?,
            expires_at: row.get(2)?,
            nonce: row.get(3)?,
            value: row.get(4)?,
        })
    }
}

fn millis(at: UnixMillis) -> i64 {
    i64::try_from(at.0).unwrap_or(i64::MAX)
}

fn aad(item_id: Uuid) -> Vec<u8> {
    let mut aad = b"pasta/store/v1/history/".to_vec();
    aad.extend_from_slice(item_id.as_bytes());
    aad
}

/// A lowercase copy that is wiped when dropped.
fn lowercase(text: &str) -> Zeroizing<String> {
    Zeroizing::new(text.to_lowercase())
}

fn matches(item: &HistoryItem, filter: HistoryFilter, needle: Option<&str>) -> bool {
    let kind_ok = match filter {
        HistoryFilter::All => true,
        HistoryFilter::Text => item.kind() == ItemKind::Text,
        HistoryFilter::Links => item.kind() == ItemKind::Link,
        HistoryFilter::Code => item.kind() == ItemKind::Code,
        HistoryFilter::FromOtherDevices => item.received,
    };
    if !kind_ok {
        return false;
    }
    let Some(needle) = needle else { return true };
    if item.source_app.as_ref().is_some_and(|app| app.name.to_lowercase().contains(needle)) {
        return true;
    }
    lowercase(&item.flavors.plain).contains(needle)
}

#[cfg(test)]
mod tests {
    use pasta_crypto::Secret32;

    use super::*;

    fn store(dir: &std::path::Path) -> HistoryStore {
        HistoryStore::new(Arc::new(Database::open(&dir.join("pasta.db"), Secret32::random()).unwrap()))
    }

    fn item(text: &str, at: u64) -> HistoryItem {
        let flavors = Flavors::plain(text);
        HistoryItem {
            item_id: Uuid::new_v4(),
            created_at: UnixMillis(at),
            expires_at: None,
            content_hash: flavors.content_hash(),
            flavors,
            source_app: Some(SourceApp { name: "Notes".into(), id: "com.apple.Notes".into() }),
            origin_device: DeviceId::random(),
            received: false,
        }
    }

    fn texts(items: &[HistoryItem]) -> Vec<&str> {
        items.iter().map(|item| item.flavors.plain.as_str()).collect()
    }

    #[test]
    fn keeps_items_sealed_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let canary = "history-canary-s3cr3t";
        let history = store(dir.path());
        history.insert(&item(canary, 1)).unwrap();
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), [canary]);
        drop(history);
        let bytes = std::fs::read(dir.path().join("pasta.db")).unwrap();
        assert!(!bytes.windows(canary.len()).any(|window| window == canary.as_bytes()), "plaintext on disk");
        assert!(!bytes.windows(5).any(|window| window == b"Notes"), "source app on disk");
    }

    #[test]
    fn newest_first_with_consecutive_copies_merged() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let first = item("a", 1);
        history.insert(&first).unwrap();
        history.insert(&item("b", 2)).unwrap();
        let replaced = history.list(&HistoryQuery::default()).unwrap()[0].item_id;
        let again = item("b", 3);
        assert_eq!(history.insert(&again).unwrap(), Inserted::Replaced(replaced));
        assert!(history.get(again.item_id).unwrap().is_some(), "the new item keeps its id");
        assert!(history.get(replaced).unwrap().is_none());
        assert_eq!(history.insert(&item("a", 4)).unwrap(), Inserted::New, "only consecutive copies merge");
        let all = history.list(&HistoryQuery::default()).unwrap();
        assert_eq!(texts(&all), ["a", "b", "a"]);
        assert_eq!(all[1].created_at, UnixMillis(3), "the repeated copy moved up");
        assert!(history.remove(first.item_id).unwrap());
        assert_eq!(history.len().unwrap(), 2);
    }

    #[test]
    fn replacing_the_newest_item_keeps_the_new_ones_id_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let local = item("same text", 1);
        history.insert(&local).unwrap();

        // The same text arrives from a peer: the newest row is replaced, the new
        // item keeps its own id and its own metadata (received, expiry).
        let received = HistoryItem {
            item_id: Uuid::new_v4(),
            created_at: UnixMillis(2),
            expires_at: Some(UnixMillis(5_000)),
            received: true,
            origin_device: DeviceId::random(),
            ..item("same text", 2)
        };
        assert_eq!(history.insert(&received).unwrap(), Inserted::Replaced(local.item_id));
        assert!(history.get(local.item_id).unwrap().is_none(), "the old id is gone");
        let stored = history.get(received.item_id).unwrap().unwrap();
        assert!(stored.received);
        assert_eq!(stored.expires_at, Some(UnixMillis(5_000)));
        assert_eq!(stored.origin_device, received.origin_device, "the new item's metadata won");
        assert_eq!(history.len().unwrap(), 1);
    }

    #[test]
    fn remove_received_only_takes_items_from_other_devices() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let mine = item("copied here", 1);
        history.insert(&mine).unwrap();
        let from_peer = {
            let mut from_peer = item("from the other machine", 2);
            from_peer.received = true;
            from_peer
        };
        history.insert(&from_peer).unwrap();

        // "Expire all now" (#33): only received items go, and the count is right.
        assert_eq!(history.remove_received().unwrap(), 1);
        assert!(history.get(from_peer.item_id).unwrap().is_none());
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), ["copied here"]);
        assert_eq!(history.remove_received().unwrap(), 0, "nothing left to remove");
    }

    #[test]
    fn received_ids_lists_only_received_items() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let mine = item("copied here", 1);
        history.insert(&mine).unwrap();
        let older = {
            let mut older = item("from the phone", 2);
            older.received = true;
            older
        };
        let newer = {
            let mut newer = item("from the laptop", 3);
            newer.received = true;
            newer
        };
        history.insert(&older).unwrap();
        history.insert(&newer).unwrap();

        // Exactly the received items, newest first, never a local copy
        // ("expire all now", #33, works from this list).
        assert_eq!(history.received_ids().unwrap(), [newer.item_id, older.item_id]);
        assert!(history.remove(newer.item_id).unwrap());
        assert_eq!(history.received_ids().unwrap(), [older.item_id], "a removed item's id is gone");
        assert_eq!(history.len().unwrap(), 2, "the local copy stays");
    }

    #[test]
    fn searches_text_and_app_names_and_filters_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        history.insert(&item("Quarterly REPORT draft", 1)).unwrap();
        history.insert(&item("https://example.com/report", 2)).unwrap();
        history.insert(&item("let total = 1;", 3)).unwrap();
        let mut remote = item("from the other machine", 4);
        remote.received = true;
        history.insert(&remote).unwrap();

        let query = |text: Option<&str>, filter| HistoryQuery { text: text.map(Into::into), filter, limit: 0 };
        assert_eq!(texts(&history.list(&query(Some("report"), HistoryFilter::All)).unwrap()).len(), 2);
        assert_eq!(
            texts(&history.list(&query(Some("report"), HistoryFilter::Links)).unwrap()),
            ["https://example.com/report"]
        );
        assert_eq!(texts(&history.list(&query(None, HistoryFilter::Code)).unwrap()), ["let total = 1;"]);
        assert_eq!(
            texts(&history.list(&query(None, HistoryFilter::FromOtherDevices)).unwrap()),
            ["from the other machine"]
        );
        assert_eq!(history.list(&query(Some("notes"), HistoryFilter::All)).unwrap().len(), 4, "matches the app name");
        let limited = HistoryQuery { limit: 1, ..HistoryQuery::default() };
        assert_eq!(texts(&history.list(&limited).unwrap()), ["from the other machine"]);
    }

    #[test]
    fn prunes_expired_old_and_surplus_items() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let day = 86_400_000;
        let now = UnixMillis(40 * day);
        history.insert(&item("old local", day)).unwrap();
        let mut expiring = item("received, expired", 39 * day);
        expiring.expires_at = Some(UnixMillis(40 * day - 1));
        history.insert(&expiring).unwrap();
        let mut kept = item("received, still valid", 39 * day + 1);
        kept.expires_at = Some(UnixMillis(41 * day));
        history.insert(&kept).unwrap();
        for n in 0..3 {
            history.insert(&item(&format!("recent {n}"), 39 * day + 2 + n)).unwrap();
        }
        assert_eq!(history.prune(now, 3, Some(30)).unwrap(), 3);
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), ["recent 2", "recent 1", "recent 0"]);
    }

    #[test]
    fn touch_moves_an_item_to_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let a = item("a", 1);
        let b = item("b", 2);
        let c = item("c", 3);
        history.insert(&a).unwrap();
        history.insert(&b).unwrap();
        history.insert(&c).unwrap();

        // Pasting an older item makes it the newest again, without a new row.
        assert!(history.touch(a.item_id, UnixMillis(10)).unwrap());
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), ["a", "c", "b"]);
        assert_eq!(history.len().unwrap(), 3);

        // Touching something that was deleted reports that it changed nothing.
        history.remove(c.item_id).unwrap();
        assert!(!history.touch(c.item_id, UnixMillis(11)).unwrap());
    }

    #[test]
    fn set_expiry_and_prune_follow_the_new_setting() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let now = UnixMillis(1000);
        let kept = item("kept forever", now.0);
        let received = item("received", now.0);
        history.insert(&kept).unwrap();
        history.insert(&received).unwrap();

        // The expiry setting changed: received items are retimed, not dropped yet.
        history.set_expiry(received.item_id, Some(UnixMillis(now.0 + 5_000))).unwrap();
        assert_eq!(history.get(received.item_id).unwrap().unwrap().expires_at, Some(UnixMillis(now.0 + 5_000)));
        assert_eq!(history.get(kept.item_id).unwrap().unwrap().expires_at, None, "other items keep theirs");
        assert_eq!(history.prune(now, 500, None).unwrap(), 0, "nothing is due yet");

        // Past the deadline the item goes; a `None` expiry keeps its item.
        assert_eq!(history.prune(UnixMillis(now.0 + 5_001), 500, None).unwrap(), 1);
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), ["kept forever"]);

        // Back to "Never": the same call clears the deadline instead.
        let another = item("another received", now.0 + 10);
        history.insert(&another).unwrap();
        history.set_expiry(another.item_id, Some(UnixMillis(now.0 + 20_000))).unwrap();
        history.set_expiry(another.item_id, None).unwrap();
        assert_eq!(history.get(another.item_id).unwrap().unwrap().expires_at, None);
        assert_eq!(history.prune(UnixMillis(now.0 + 30_000), 500, None).unwrap(), 0);
    }

    #[test]
    fn a_tampered_row_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let history = store(dir.path());
        let a = item("a", 1);
        let b = item("b", 2);
        history.insert(&a).unwrap();
        history.insert(&b).unwrap();
        // Move b's sealed value onto a: the item id (AAD) no longer matches.
        history
            .db
            .lock()
            .execute(
                "UPDATE history SET nonce = (SELECT nonce FROM history WHERE item_id = ?2),
                   value = (SELECT value FROM history WHERE item_id = ?2) WHERE item_id = ?1",
                params![a.item_id.to_string(), b.item_id.to_string()],
            )
            .unwrap();
        assert_eq!(texts(&history.list(&HistoryQuery::default()).unwrap()), ["b"]);
        assert!(history.get(a.item_id).is_err());
    }
}
