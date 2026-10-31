//! Placeholder for platforms without an adapter yet (KDE Plasma arrives in M4).

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{ClipboardAdapter, ClipboardError, ClipboardStatus, CopySink, Watch, WriteReceipt};
use uuid::Uuid;

/// Never reports copies; refuses writes.
pub struct Unsupported;

impl ClipboardAdapter for Unsupported {
    fn watch(&self, _sink: CopySink) -> Result<Watch, ClipboardError> {
        tracing::warn!("clipboard sync is not supported on this platform yet");
        Ok(Watch::new(|| {}))
    }

    fn write(&self, _item_id: Uuid, _flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn expire(&self, _receipt: &WriteReceipt) -> Result<(), ClipboardError> {
        Ok(())
    }

    fn status(&self) -> ClipboardStatus {
        ClipboardStatus { history_enabled: None, unsupported: Some("KDE Plasma support arrives in M4".into()) }
    }
}
