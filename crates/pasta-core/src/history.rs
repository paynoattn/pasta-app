//! Pasta's own clipboard history, used on macOS where the OS has none (#15).
//!
//! In memory only for M1; the persistent, searchable store arrives in #37.
//! Generic over what an entry holds, so the app can keep the text sealed in
//! memory (`pasta_crypto::vault`, #24) rather than as plain strings.

use std::collections::VecDeque;

use uuid::Uuid;

use crate::clip::{ContentHash, SourceApp};
use crate::device::{DeviceId, UnixMillis};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry<T> {
    pub item_id: Uuid,
    pub origin_device: DeviceId,
    pub origin_app: Option<SourceApp>,
    pub at: UnixMillis,
    /// The item's text, in whatever form the owner keeps it.
    pub content: T,
    pub content_hash: ContentHash,
}

/// Newest first, bounded. Adding text that's already present moves it to the top.
#[derive(Debug)]
pub struct History<T> {
    entries: VecDeque<HistoryEntry<T>>,
    capacity: usize,
}

impl<T> History<T> {
    pub fn new(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity), capacity }
    }

    pub fn push(&mut self, entry: HistoryEntry<T>) {
        self.entries.retain(|existing| existing.content_hash != entry.content_hash);
        self.entries.push_front(entry);
        self.entries.truncate(self.capacity);
    }

    pub fn remove(&mut self, item_id: Uuid) -> Option<HistoryEntry<T>> {
        let index = self.entries.iter().position(|entry| entry.item_id == item_id)?;
        self.entries.remove(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry<T>> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Flavors;

    fn entry(text: &str) -> HistoryEntry<Flavors> {
        let flavors = Flavors::plain(text);
        HistoryEntry {
            item_id: Uuid::new_v4(),
            origin_device: DeviceId::random(),
            origin_app: None,
            at: UnixMillis(1),
            content_hash: flavors.content_hash(),
            content: flavors,
        }
    }

    #[test]
    fn keeps_newest_first_without_duplicates() {
        let mut history = History::new(2);
        history.push(entry("a"));
        history.push(entry("b"));
        history.push(entry("a"));
        let texts: Vec<_> = history.iter().map(|entry| entry.content.plain.as_str()).collect();
        assert_eq!(texts, ["a", "b"]);
        history.push(entry("c"));
        assert_eq!(history.len(), 2, "bounded");
        let newest = history.iter().next().unwrap().item_id;
        assert!(history.remove(newest).is_some());
        assert_eq!(history.len(), 1);
    }
}
