//! Desktops Pasta can't sync on: Linux without Wayland `ext-data-control`
//! (X11 sessions, GNOME; ADR 0015) and other OSes.

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{ClipboardAdapter, ClipboardError, ClipboardStatus, CopySink, Watch, WriteReceipt};
use uuid::Uuid;

/// Never reports copies; refuses writes.
pub struct Unsupported {
    reason: String,
}

impl Unsupported {
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl ClipboardAdapter for Unsupported {
    fn watch(&self, _sink: CopySink) -> Result<Watch, ClipboardError> {
        tracing::warn!(reason = %self.reason, "clipboard sync is not supported here");
        Ok(Watch::new(|| {}))
    }

    fn write(&self, _item_id: Uuid, _flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn expire(&self, _receipt: &WriteReceipt) -> Result<(), ClipboardError> {
        Ok(())
    }

    fn status(&self) -> ClipboardStatus {
        ClipboardStatus { unsupported: Some(self.reason.clone()), ..ClipboardStatus::default() }
    }
}
