//! The interface between the sync engine and each OS clipboard (#16).
//!
//! Implementations live in `pasta-clipboard`; [`crate::testing::FakeClipboard`]
//! stands in for them in tests.

use std::fmt;

use uuid::Uuid;

use crate::clip::{Flavors, SourceApp};

/// A copy the platform watcher saw, after it skipped sensitive items and the
/// adapter's own writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCopy {
    pub flavors: Flavors,
    pub source_app: Option<SourceApp>,
}

/// Receives local copies. Called from the adapter's own thread.
pub type CopySink = Box<dyn Fn(LocalCopy) + Send + Sync>;

/// What an adapter needs to find a written item again when it expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteReceipt {
    pub item_id: Uuid,
    /// Windows: the Win+V history entry id, if it was found after writing.
    pub history_id: Option<String>,
    /// macOS: `NSPasteboard.changeCount` right after the write.
    pub change_count: Option<i64>,
}

/// Facts about the platform clipboard worth showing the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardStatus {
    /// Windows: whether Win+V history is turned on. `None` where it doesn't apply.
    pub history_enabled: Option<bool>,
    /// Set when the adapter can't work on this platform yet.
    pub unsupported: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("the clipboard is held by another app")]
    Busy,
    #[error("clipboard access is not supported on this platform yet")]
    Unsupported,
    #[error("the clipboard adapter has stopped")]
    Stopped,
    /// An OS error. Must never contain clipboard text.
    #[error("clipboard error: {0}")]
    Os(String),
}

/// Stops watching when dropped.
pub struct Watch {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Watch {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self { stop: Some(Box::new(stop)) }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

impl fmt::Debug for Watch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Watch")
    }
}

/// One OS clipboard. Methods may block (the engine calls them off its own task).
pub trait ClipboardAdapter: Send + Sync + 'static {
    /// Starts reporting local copies to `sink` until the returned guard is dropped.
    fn watch(&self, sink: CopySink) -> Result<Watch, ClipboardError>;

    /// Makes `flavors` the current clipboard item, so it lands in the OS history.
    /// The watcher must not report this write back as a local copy.
    fn write(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError>;

    /// Removes a written item from the OS history, and clears the clipboard if
    /// it is still current. An item that is already gone counts as expired.
    fn expire(&self, receipt: &WriteReceipt) -> Result<(), ClipboardError>;

    fn status(&self) -> ClipboardStatus {
        ClipboardStatus::default()
    }
}
