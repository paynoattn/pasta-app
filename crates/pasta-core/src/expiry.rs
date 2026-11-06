//! Expiry of received items (#33).
//!
//! Every item Pasta writes for a peer is recorded with its write receipt and,
//! if this device has an expiry set, the deadline. The ledger is plain data so
//! the app can persist it (in the encrypted store) and expire items that came
//! due while it wasn't running. It also lists what "Clear from Win + V" (#34)
//! removes, and gives the tray its countdowns.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clip::ContentHash;
use crate::clipboard::WriteReceipt;
use crate::device::{DeviceId, UnixMillis};

/// Most written items without a deadline remembered; the oldest go first. Items
/// with a deadline are never dropped, or they would never expire.
pub const MAX_WRITTEN: usize = 500;

/// One received item Pasta wrote to the clipboard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Written {
    pub item_id: Uuid,
    pub from: DeviceId,
    pub written_at: UnixMillis,
    /// `None`: kept until the user clears it.
    pub expires_at: Option<UnixMillis>,
    pub content_hash: ContentHash,
    pub receipt: WriteReceipt,
}

/// The deadline for an item written at `at` under an expiry of `ttl_secs`.
pub fn deadline(at: UnixMillis, ttl_secs: Option<u64>) -> Option<UnixMillis> {
    ttl_secs.map(|secs| UnixMillis(at.0.saturating_add(secs.saturating_mul(1000))))
}

/// Received items Pasta wrote, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ledger {
    items: Vec<Written>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Written> {
        self.items.iter()
    }

    /// Records a write. Returns entries without a deadline pushed out by the
    /// size limit; they are forgotten, not expired.
    pub fn record(&mut self, written: Written) -> Vec<Written> {
        self.items.retain(|item| item.item_id != written.item_id);
        self.items.push(written);
        let mut dropped = Vec::new();
        while self.items.len() > MAX_WRITTEN {
            let Some(index) = self.items.iter().position(|item| item.expires_at.is_none()) else { break };
            dropped.push(self.items.remove(index));
        }
        dropped
    }

    /// The earliest deadline, if any item has one.
    pub fn next_deadline(&self) -> Option<UnixMillis> {
        self.items.iter().filter_map(|item| item.expires_at).min()
    }

    /// Removes and returns the items due at `now`.
    pub fn take_due(&mut self, now: UnixMillis) -> Vec<Written> {
        let (due, keep) =
            std::mem::take(&mut self.items).into_iter().partition(|item| item.expires_at.is_some_and(|at| at <= now));
        self.items = keep;
        due
    }

    /// Removes and returns every item ("Expire all now", "Clear from Win + V").
    pub fn take_all(&mut self) -> Vec<Written> {
        std::mem::take(&mut self.items)
    }

    pub fn remove(&mut self, item_id: Uuid) -> Option<Written> {
        let index = self.items.iter().position(|item| item.item_id == item_id)?;
        Some(self.items.remove(index))
    }

    /// Applies a new expiry to the items already written, measured from when
    /// each was written: shortening it can make items due at once.
    pub fn retime(&mut self, ttl_secs: Option<u64>) {
        for item in &mut self.items {
            item.expires_at = deadline(item.written_at, ttl_secs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Flavors;

    fn written(at: u64, expires_at: Option<u64>) -> Written {
        let item_id = Uuid::new_v4();
        Written {
            item_id,
            from: DeviceId::random(),
            written_at: UnixMillis(at),
            expires_at: expires_at.map(UnixMillis),
            content_hash: Flavors::plain(format!("item {at}")).content_hash(),
            receipt: WriteReceipt { item_id, history_id: None, change_count: Some(1) },
        }
    }

    #[test]
    fn deadlines_follow_the_ttl() {
        assert_eq!(deadline(UnixMillis(1_000), Some(5)), Some(UnixMillis(6_000)));
        assert_eq!(deadline(UnixMillis(1_000), None), None);
        assert_eq!(deadline(UnixMillis(u64::MAX - 1), Some(u64::MAX)), Some(UnixMillis(u64::MAX)));
    }

    #[test]
    fn takes_only_due_items() {
        let mut ledger = Ledger::new();
        let soon = written(0, Some(10));
        let later = written(0, Some(20));
        let never = written(0, None);
        ledger.record(later.clone());
        ledger.record(soon.clone());
        ledger.record(never.clone());
        assert_eq!(ledger.next_deadline(), Some(UnixMillis(10)));
        assert!(ledger.take_due(UnixMillis(9)).is_empty());
        assert_eq!(ledger.take_due(UnixMillis(10)), vec![soon]);
        assert_eq!(ledger.next_deadline(), Some(UnixMillis(20)));
        assert_eq!(ledger.take_all(), vec![later, never]);
        assert!(ledger.is_empty());
        assert_eq!(ledger.next_deadline(), None);
    }

    #[test]
    fn survives_a_restart() {
        let mut ledger = Ledger::new();
        ledger.record(written(5, Some(50)));
        let json = serde_json::to_string(&ledger).unwrap();
        let restored: Ledger = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, ledger);
    }

    #[test]
    fn bounded_dropping_items_without_a_deadline_first() {
        let mut ledger = Ledger::new();
        let keep = written(0, Some(1));
        ledger.record(keep.clone());
        for at in 1..=MAX_WRITTEN as u64 {
            ledger.record(written(at, None));
        }
        assert_eq!(ledger.len(), MAX_WRITTEN);
        assert!(ledger.iter().any(|item| item.item_id == keep.item_id), "an item with a deadline was dropped");
    }

    #[test]
    fn items_with_a_deadline_are_never_dropped_by_the_size_limit() {
        let mut ledger = Ledger::new();
        let mut expected = Vec::new();
        // Well over MAX_WRITTEN items, every one with a deadline: none may go,
        // or it would never be expired at its deadline (#33).
        for at in 0..(MAX_WRITTEN as u64 * 2) {
            let item = written(at, Some(at + 10));
            expected.push(item.item_id);
            assert!(ledger.record(item).is_empty(), "an item with a deadline was dropped");
        }
        assert_eq!(ledger.len(), expected.len());
        assert_eq!(ledger.iter().map(|item| item.item_id).collect::<Vec<_>>(), expected);
        // They still leave through their deadline, not the size limit.
        assert_eq!(ledger.take_due(UnixMillis(u64::MAX)).len(), expected.len());
    }

    #[test]
    fn the_size_limit_evicts_the_oldest_deadline_less_items() {
        let mut ledger = Ledger::new();
        let old_free = written(0, None);
        let new_free = written(1, None);
        ledger.record(old_free.clone());
        ledger.record(new_free.clone());
        // Fill the ledger to exactly MAX_WRITTEN with items that have a deadline.
        let mut keep = Vec::new();
        for at in 2..(MAX_WRITTEN as u64) {
            let item = written(at, Some(at + 10));
            keep.push(item.item_id);
            ledger.record(item);
        }
        assert_eq!(ledger.len(), MAX_WRITTEN);
        // One more pushes the oldest deadline-less item out: `old_free`, not the
        // newer `new_free` and not any item with a deadline.
        let dropped = ledger.record(written(MAX_WRITTEN as u64 + 1, None));
        assert_eq!(dropped, vec![old_free], "the oldest item without a deadline goes first");
        assert_eq!(ledger.len(), MAX_WRITTEN);
        assert!(ledger.iter().any(|item| item.item_id == new_free.item_id), "a newer deadline-less item survived");
        assert!(keep.iter().all(|id| ledger.iter().any(|item| item.item_id == *id)), "items with a deadline stayed");
    }

    #[test]
    fn retiming_measures_from_the_write() {
        let mut ledger = Ledger::new();
        ledger.record(written(1_000, None));
        ledger.retime(Some(1));
        assert_eq!(ledger.next_deadline(), Some(UnixMillis(2_000)));
        ledger.retime(None);
        assert_eq!(ledger.next_deadline(), None);
    }
}
