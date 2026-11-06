//! The interface between the sync engine and each OS clipboard (#16).
//!
//! Implementations live in `pasta-clipboard`; [`crate::testing::FakeClipboard`]
//! stands in for them in tests.

use std::fmt;

use serde::{Deserialize, Serialize};
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
/// Persisted with the expiry deadlines (#33), so it carries no content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteReceipt {
    pub item_id: Uuid,
    /// Windows: the Win+V history entry id, if it was found after writing.
    #[serde(default)]
    pub history_id: Option<String>,
    /// macOS: `NSPasteboard.changeCount` right after the write. Windows: the
    /// clipboard sequence number.
    #[serde(default)]
    pub change_count: Option<i64>,
}

/// How an adapter writes and filters, from the user's settings (#36, #41).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterOptions {
    /// Skip copies marked concealed / transient and copies from known password managers.
    pub skip_sensitive: bool,
    /// Lowercase app ids whose copies are skipped before their text is read.
    pub excluded_apps: Vec<String>,
    /// Windows: received items also go into Win+V history.
    pub add_to_history: bool,
    /// macOS: items Pasta writes may travel on through Universal Clipboard,
    /// and Universal Clipboard items are reported as copies.
    pub icloud_bridge: bool,
}

impl Default for AdapterOptions {
    fn default() -> Self {
        Self { skip_sensitive: true, excluded_apps: Vec::new(), add_to_history: true, icloud_bridge: false }
    }
}

/// How one write should land.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WriteKind {
    /// A received item, or one picked from the history: lands in the OS history.
    #[default]
    Normal,
    /// Secrets such as the sync chain words (#46): marked concealed, kept out
    /// of every clipboard history and never synced.
    Concealed,
}

/// Facts about the platform clipboard worth showing the user.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipboardStatus {
    /// Windows: whether Win+V history is turned on. `None` where it doesn't apply.
    pub history_enabled: Option<bool>,
    /// Windows: whether "Sync across your devices" (cloud clipboard) is on, which
    /// can show items twice (#45). `None` where it doesn't apply.
    pub cloud_sync: Option<bool>,
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

    /// Like [`ClipboardAdapter::write`], for writes that must not land in any history.
    /// Adapters that can't mark a write concealed refuse it.
    fn write_concealed(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        let _ = (item_id, flavors);
        Err(ClipboardError::Unsupported)
    }

    /// Applies new privacy and write options. Called at start and on every change.
    fn configure(&self, options: &AdapterOptions) {
        let _ = options;
    }

    fn status(&self) -> ClipboardStatus {
        ClipboardStatus::default()
    }
}
