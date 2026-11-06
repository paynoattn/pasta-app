//! Test doubles for the engine and the integration tests (#19). Enabled with
//! the `testing` feature.

use std::sync::{Arc, Mutex, MutexGuard};

use uuid::Uuid;

use crate::clip::{Flavors, SourceApp};
use crate::clipboard::{AdapterOptions, ClipboardAdapter, ClipboardError, CopySink, LocalCopy, Watch, WriteReceipt};

type Sink = Arc<dyn Fn(LocalCopy) + Send + Sync>;

/// An in-memory clipboard.
///
/// By default every write is reported back to the watcher as a local copy,
/// the worst case real adapters can hit (e.g. re-selecting a Pasta item in
/// Win+V strips its marker), so tests exercise the engine's echo prevention.
#[derive(Clone, Default)]
pub struct FakeClipboard {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    sink: Option<Sink>,
    current: Option<Flavors>,
    writes: Vec<Flavors>,
    expired: Vec<Uuid>,
    concealed: Vec<Uuid>,
    options: Option<AdapterOptions>,
    quiet_writes: bool,
    fail_writes: bool,
}

impl FakeClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stop reporting writes back to the watcher, like an adapter whose marker works.
    pub fn quiet_writes(&self, quiet: bool) {
        self.lock().quiet_writes = quiet;
    }

    pub fn fail_writes(&self, fail: bool) {
        self.lock().fail_writes = fail;
    }

    /// Simulates the user copying `text`.
    pub fn copy(&self, text: &str) {
        self.copy_flavors(Flavors::plain(text));
    }

    pub fn copy_flavors(&self, flavors: Flavors) {
        self.copy_from(flavors, None);
    }

    /// Simulates a copy made in `app`.
    pub fn copy_from(&self, flavors: Flavors, source_app: Option<SourceApp>) {
        let sink = {
            let mut inner = self.lock();
            inner.current = Some(flavors.clone());
            inner.sink.clone()
        };
        if let Some(sink) = sink {
            sink(LocalCopy { flavors, source_app });
        }
    }

    /// Item ids written with [`ClipboardAdapter::write_concealed`].
    pub fn concealed(&self) -> Vec<Uuid> {
        self.lock().concealed.clone()
    }

    /// The options last passed to [`ClipboardAdapter::configure`].
    pub fn options(&self) -> Option<AdapterOptions> {
        self.lock().options.clone()
    }

    pub fn current(&self) -> Option<Flavors> {
        self.lock().current.clone()
    }

    pub fn writes(&self) -> Vec<Flavors> {
        self.lock().writes.clone()
    }

    pub fn expired(&self) -> Vec<Uuid> {
        self.lock().expired.clone()
    }

    pub fn is_watched(&self) -> bool {
        self.lock().sink.is_some()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ClipboardAdapter for FakeClipboard {
    fn watch(&self, sink: CopySink) -> Result<Watch, ClipboardError> {
        self.lock().sink = Some(Arc::from(sink));
        let inner = Arc::clone(&self.inner);
        Ok(Watch::new(move || {
            inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).sink = None;
        }))
    }

    fn write(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        let echo = {
            let mut inner = self.lock();
            if inner.fail_writes {
                return Err(ClipboardError::Busy);
            }
            inner.current = Some(flavors.clone());
            inner.writes.push(flavors.clone());
            if inner.quiet_writes { None } else { inner.sink.clone() }
        };
        if let Some(sink) = echo {
            sink(LocalCopy { flavors: flavors.clone(), source_app: None });
        }
        Ok(WriteReceipt { item_id, history_id: None, change_count: None })
    }

    fn expire(&self, receipt: &WriteReceipt) -> Result<(), ClipboardError> {
        self.lock().expired.push(receipt.item_id);
        Ok(())
    }

    /// Never echoed back: real adapters mark these so their watchers skip them.
    fn write_concealed(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        let mut inner = self.lock();
        if inner.fail_writes {
            return Err(ClipboardError::Busy);
        }
        inner.current = Some(flavors.clone());
        inner.concealed.push(item_id);
        Ok(WriteReceipt { item_id, history_id: None, change_count: None })
    }

    fn configure(&self, options: &AdapterOptions) {
        self.lock().options = Some(options.clone());
    }
}
