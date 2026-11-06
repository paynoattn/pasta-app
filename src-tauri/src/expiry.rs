//! Expiring received items (#33–#35): the persisted ledger, the timer in the
//! runtime loop, "Expire all now" and "Clear from Win + V".

use pasta_core::clipboard::WriteReceipt;
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::engine::ExpireRequest;
use pasta_core::expiry::{Ledger, Written, deadline};
use pasta_core::protocol::Clip;
use pasta_store::Database;
use tauri::Emitter;
use uuid::Uuid;

use crate::runtime::{HISTORY_CHANGED, Runtime};
use crate::views::ExpiryView;

const ROW: &str = "expiry";
/// When an item the adapter couldn't expire is tried again.
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
/// The tray's countdowns change.
pub const EXPIRY_CHANGED: &str = "expiry-changed";

/// The ledger as stored; a missing or unreadable row starts empty.
pub fn load(db: &Database) -> Ledger {
    match db.get::<Ledger>(ROW) {
        Ok(Some(ledger)) => ledger,
        Ok(None) => Ledger::new(),
        Err(err) => {
            tracing::warn!(error = %err, "couldn't read the expiry deadlines; starting over");
            Ledger::new()
        }
    }
}

impl Runtime {
    fn lock_ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save_ledger(&self, ledger: &Ledger) {
        if let Err(err) = self.db.put(ROW, ledger) {
            tracing::warn!(error = %err, "couldn't save the expiry deadlines");
        }
    }

    /// A received item was written: remember it, with a deadline if this device has an expiry.
    pub(crate) fn record_received(&self, clip: &Clip, from: DeviceId, receipt: WriteReceipt) {
        let now = UnixMillis::now();
        let ttl = self.config.get().ttl_secs;
        let written = Written {
            item_id: clip.item_id,
            from,
            written_at: now,
            expires_at: deadline(now, ttl),
            content_hash: clip.content_hash,
            receipt,
        };
        {
            let mut ledger = self.lock_ledger();
            ledger.record(written);
            self.save_ledger(&ledger);
        }
        self.expiry_changed.notify_one();
        let _ = self.app.emit(EXPIRY_CHANGED, self.expiry_views());
    }

    pub(crate) fn next_expiry(&self) -> Option<UnixMillis> {
        self.lock_ledger().next_deadline()
    }

    /// Expires every item that is due.
    pub(crate) async fn expire_due(&self) {
        let due = {
            let mut ledger = self.lock_ledger();
            let due = ledger.take_due(UnixMillis::now());
            if !due.is_empty() {
                self.save_ledger(&ledger);
            }
            due
        };
        if !due.is_empty() {
            self.expire_items(due).await;
        }
    }

    /// "Expire all now" (#33) and "Clear from Win + V" (#34): every received
    /// item goes, deadline or not. Returns how many went.
    pub async fn expire_all(&self) -> Result<usize, String> {
        // Received items in the Mac history now, including any the ledger no longer
        // lists (past its size limit). Items arriving meanwhile aren't part of this.
        let received = match &self.history {
            Some(history) => history.received_ids().map_err(|err| err.to_string())?,
            None => Vec::new(),
        };
        let all = {
            let mut ledger = self.lock_ledger();
            let all = ledger.take_all();
            self.save_ledger(&ledger);
            all
        };
        let batch: Vec<Uuid> = all.iter().map(|item| item.item_id).collect();
        let (mut expired, failed) = self.expire_items(all).await;
        if let Some(history) = &self.history {
            for id in received.iter().filter(|id| !batch.contains(id) && !failed.contains(id)) {
                if history.remove(*id).map_err(|err| err.to_string())? {
                    expired += 1;
                }
            }
            let _ = self.app.emit(HISTORY_CHANGED, ());
        }
        Ok(expired)
    }

    /// Expires `items` through the adapter. Items it fails on (e.g. the
    /// clipboard was busy) go back into the ledger and are tried again later.
    /// Returns how many expired, and the ids that will be retried.
    async fn expire_items(&self, items: Vec<Written>) -> (usize, Vec<Uuid>) {
        let requests: Vec<ExpireRequest> = items
            .iter()
            .map(|item| ExpireRequest { receipt: item.receipt.clone(), content_hash: item.content_hash })
            .collect();
        let failed: Vec<Uuid> = match self.node.lock().await.as_ref() {
            Some(node) => node.expire(requests).await.unwrap_or_else(|err| {
                tracing::warn!(error = %err, "couldn't expire items");
                items.iter().map(|item| item.item_id).collect()
            }),
            None => items.iter().map(|item| item.item_id).collect(),
        };
        let (retry, done): (Vec<Written>, Vec<Written>) =
            items.into_iter().partition(|item| failed.contains(&item.item_id));
        if !retry.is_empty() {
            let at = UnixMillis(UnixMillis::now().0.saturating_add(RETRY_AFTER.as_millis() as u64));
            let mut ledger = self.lock_ledger();
            for mut item in retry {
                item.expires_at = Some(at);
                ledger.record(item);
            }
            self.save_ledger(&ledger);
            self.expiry_changed.notify_one();
        }
        if let Some(history) = &self.history {
            for item in &done {
                if let Err(err) = history.remove(item.item_id) {
                    tracing::warn!(item = %item.item_id, error = %err, "couldn't remove an expired item from the history");
                }
            }
        }
        {
            let mut recent = self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for item in &done {
                recent.remove(item.item_id);
            }
        }
        let _ = self.app.emit(HISTORY_CHANGED, ());
        let _ = self.app.emit(EXPIRY_CHANGED, self.expiry_views());
        (done.len(), failed)
    }

    /// Forgets a written item without expiring it, e.g. when a newer copy of the
    /// same text replaced it in the Mac history.
    pub(crate) fn forget_written(&self, item_id: Uuid) {
        let mut ledger = self.lock_ledger();
        if ledger.remove(item_id).is_some() {
            self.save_ledger(&ledger);
            drop(ledger);
            self.expiry_changed.notify_one();
        }
    }

    /// The expiry setting changed: received items follow the new one, counted from when each arrived.
    pub(crate) fn retime_received(&self, ttl_secs: Option<u64>) {
        let items: Vec<(uuid::Uuid, Option<UnixMillis>)> = {
            let mut ledger = self.lock_ledger();
            ledger.retime(ttl_secs);
            self.save_ledger(&ledger);
            ledger.iter().map(|item| (item.item_id, item.expires_at)).collect()
        };
        if let Some(history) = &self.history {
            for (id, expires_at) in items {
                if let Err(err) = history.set_expiry(id, expires_at) {
                    tracing::warn!(item = %id, error = %err, "couldn't update an item's expiry");
                }
            }
        }
        self.expiry_changed.notify_one();
        let _ = self.app.emit(EXPIRY_CHANGED, self.expiry_views());
    }

    /// Received items, newest first, for the tray's countdowns.
    pub fn expiry_views(&self) -> Vec<ExpiryView> {
        let names: std::collections::HashMap<DeviceId, String> =
            self.peers().into_iter().map(|peer| (peer.device_id, peer.name)).collect();
        let ledger = self.lock_ledger();
        ledger
            .iter()
            .rev()
            .map(|item| ExpiryView {
                item_id: item.item_id.to_string(),
                from_device: names.get(&item.from).cloned().unwrap_or_else(|| "another device".into()),
                written_at: item.written_at.0,
                expires_at: item.expires_at.map(|at| at.0),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use pasta_core::clip::ContentHash;
    use pasta_core::device::UnixMillis;
    use pasta_core::expiry::Written;
    use pasta_crypto::Secret32;

    use super::*;

    #[test]
    fn load_returns_an_empty_ledger_for_a_missing_or_corrupt_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        let db = Database::open(&path, Secret32::random()).unwrap();

        // No row yet.
        assert_eq!(load(&db), Ledger::new());

        // A healthy row loads back.
        let item_id = uuid::Uuid::new_v4();
        let mut ledger = Ledger::new();
        ledger.record(Written {
            item_id,
            from: DeviceId::random(),
            written_at: UnixMillis(5),
            expires_at: Some(UnixMillis(6)),
            content_hash: ContentHash([1; 32]),
            receipt: WriteReceipt { item_id, history_id: None, change_count: Some(2) },
        });
        db.put(ROW, &ledger).unwrap();
        assert_eq!(load(&db), ledger);

        // A row that doesn't decrypt starts over rather than failing startup.
        // Flip the nonce of another row onto it: the AAD (row name) stops matching.
        db.put("other", &1u32).unwrap();
        let raw = rusqlite::Connection::open(&path).unwrap();
        raw.execute(
            "UPDATE kv SET nonce = (SELECT nonce FROM kv WHERE name = 'other'),
               value = (SELECT value FROM kv WHERE name = 'other') WHERE name = ?1",
            rusqlite::params![ROW],
        )
        .unwrap();
        assert_eq!(load(&db), Ledger::new(), "a corrupt ledger starts empty");
    }
}
