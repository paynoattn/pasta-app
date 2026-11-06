//! Pasta's clipboard history (#37) and pasting from it (#38, #39).
//!
//! On macOS every copy and received item goes into the encrypted history
//! store; elsewhere only the latest received items are kept, sealed in memory,
//! for the tray flyout. Previews sent to the UI are capped; the full item
//! stays in Rust until it is pasted.

use base64::Engine as _;
use pasta_core::clip::{Flavors, ItemKind};
use pasta_core::clipboard::WriteKind;
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::history::HistoryEntry;
use pasta_core::protocol::Clip;
use pasta_store::history::{HistoryItem, HistoryQuery};
use serde::Serialize;
use tauri::Emitter;
use ts_rs::TS;
use uuid::Uuid;

use crate::runtime::{HISTORY_CHANGED, Runtime};
use crate::views::{HTML_PREVIEW_BYTES, HistoryFilterView, HistoryItemView, PREVIEW_BYTES, clip_str};

/// What happened when the user picked an item (#39).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PasteOutcome {
    /// Written and pasted into the app that was in front. Only macOS pastes so far.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Pasted,
    /// Written to the clipboard only: Pasta may not post keystrokes (Accessibility is off).
    Copied,
}

impl Runtime {
    /// Adds a local copy or a received item to the history.
    pub(crate) fn store_item(&self, clip: &Clip, received: bool) {
        let now = UnixMillis::now();
        if let Some(history) = &self.history {
            let expires_at = if received {
                self.ledger
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .find(|item| item.item_id == clip.item_id)
                    .and_then(|item| item.expires_at)
            } else {
                None
            };
            let item = HistoryItem {
                item_id: clip.item_id,
                created_at: now,
                expires_at,
                flavors: clip.flavors.clone(),
                source_app: clip.origin_app.clone(),
                origin_device: clip.origin_device,
                content_hash: clip.content_hash,
                received,
            };
            match history.insert(&item) {
                // The replaced row's item is gone from the history; its expiry has nothing left to do.
                Ok(pasta_store::history::Inserted::Replaced(old)) => self.forget_written(old),
                Ok(pasta_store::history::Inserted::New) => {}
                Err(err) => {
                    tracing::warn!(item = %clip.item_id, error = %err, "left an item out of the history");
                    return;
                }
            }
            self.prune_history();
        } else if received {
            match self.vault.seal_flavors(&clip.flavors) {
                Ok(content) => {
                    self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(HistoryEntry {
                        item_id: clip.item_id,
                        origin_device: clip.origin_device,
                        origin_app: clip.origin_app.clone(),
                        at: now,
                        content,
                        content_hash: clip.content_hash,
                    })
                }
                Err(err) => tracing::warn!(item = %clip.item_id, error = %err, "left an item out of the recent list"),
            }
        } else {
            return;
        }
        let _ = self.app.emit(HISTORY_CHANGED, ());
    }

    /// Applies the retention settings (#35).
    pub(crate) fn prune_history(&self) {
        let Some(history) = &self.history else { return };
        let prefs = self.config.get().history;
        match history.prune(UnixMillis::now(), prefs.max_items, prefs.keep_days) {
            Ok(0) => {}
            Ok(removed) => tracing::debug!(removed, "pruned the history"),
            Err(err) => tracing::warn!(error = %err, "couldn't prune the history"),
        }
    }

    /// History items for the panel, newest first.
    pub fn history_items(
        &self,
        text: Option<String>,
        filter: HistoryFilterView,
        limit: usize,
    ) -> Result<Vec<HistoryItemView>, String> {
        let limit = limit.clamp(1, 500);
        match &self.history {
            Some(history) => {
                let query = HistoryQuery { text, filter: filter.into(), limit };
                let items = history.list(&query).map_err(|err| err.to_string())?;
                Ok(items.iter().map(|item| self.item_view(item)).collect())
            }
            None => Ok(self.recent_views(limit)),
        }
    }

    /// Received items kept in memory, for the flyout on Windows and Linux.
    fn recent_views(&self, limit: usize) -> Vec<HistoryItemView> {
        let recent = self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        recent
            .iter()
            .take(limit)
            .filter_map(|entry| {
                let flavors = self.vault.open_flavors(&entry.content).ok()?;
                let item = HistoryItem {
                    item_id: entry.item_id,
                    created_at: entry.at,
                    expires_at: self.expiry_of(entry.item_id),
                    flavors,
                    source_app: entry.origin_app.clone(),
                    origin_device: entry.origin_device,
                    content_hash: entry.content_hash,
                    received: true,
                };
                Some(self.item_view(&item))
            })
            .collect()
    }

    fn expiry_of(&self, item_id: Uuid) -> Option<UnixMillis> {
        let ledger = self.ledger.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.iter().find(|item| item.item_id == item_id).and_then(|item| item.expires_at)
    }

    fn device_label(&self, device: DeviceId) -> Option<String> {
        if device == self.device.id {
            return None;
        }
        let name = self.peers().into_iter().find(|peer| peer.device_id == device).map(|peer| peer.name);
        Some(name.unwrap_or_else(|| "another device".into()))
    }

    fn item_view(&self, item: &HistoryItem) -> HistoryItemView {
        let flavors = &item.flavors;
        let preview = clip_str(&flavors.plain, PREVIEW_BYTES).to_owned();
        let html = match (&flavors.html, &flavors.rtf) {
            (Some(html), _) => Some(clip_str(html, HTML_PREVIEW_BYTES).to_owned()),
            (None, Some(rtf)) => Some(pasta_core::rtf::to_html(clip_str(rtf, HTML_PREVIEW_BYTES * 4))),
            (None, None) => None,
        };
        let kind: ItemKind = item.kind();
        HistoryItemView {
            id: item.item_id.to_string(),
            kind: kind.into(),
            truncated: preview.len() < flavors.plain.len(),
            preview,
            html,
            bytes: flavors.byte_len(),
            source_app: item.source_app.as_ref().map(|app| app.name.clone()).filter(|name| !name.is_empty()),
            source_app_id: item.source_app.as_ref().map(|app| app.id.clone()).filter(|id| !id.is_empty()),
            origin_device: self.device_label(item.origin_device),
            received: item.received,
            created_at: item.created_at.0,
            expires_at: item.expires_at.map(|at| at.0),
        }
    }

    fn full_item(&self, item_id: Uuid) -> Result<Flavors, String> {
        match &self.history {
            Some(history) => history
                .get(item_id)
                .map_err(|err| err.to_string())?
                .map(|item| item.flavors)
                .ok_or_else(|| "that item is gone".into()),
            None => {
                let recent = self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = recent.iter().find(|entry| entry.item_id == item_id).ok_or("that item is gone")?;
                self.vault.open_flavors(&entry.content).map_err(|err| err.to_string())
            }
        }
    }

    /// Puts an item back on the clipboard without syncing it.
    pub async fn copy_item(&self, item_id: Uuid, plain_only: bool) -> Result<(), String> {
        let mut flavors = self.full_item(item_id)?;
        if plain_only {
            flavors = Flavors::plain(std::mem::take(&mut flavors.plain));
        }
        let node = self.node.lock().await;
        let node = node.as_ref().ok_or("Pasta is stopping")?;
        node.write_local_wait(flavors, WriteKind::Normal).await?;
        if let Some(history) = &self.history
            && let Err(err) = history.touch(item_id, UnixMillis::now())
        {
            tracing::debug!(item = %item_id, error = %err, "couldn't move the item up");
        }
        let _ = self.app.emit(HISTORY_CHANGED, ());
        Ok(())
    }

    /// Copies an item, hides the panel and pastes it into the app that was in
    /// front when the panel opened (#39). Without the Accessibility permission
    /// it stops after copying.
    pub async fn paste_item(&self, item_id: Uuid, plain_only: bool) -> Result<PasteOutcome, String> {
        self.copy_item(item_id, plain_only).await?;
        crate::ui::hide_panel(&self.app);
        let outcome = paste_into_front(&self.app).await;
        tracing::info!(?outcome, "picked an item from the history");
        Ok(outcome)
    }

    pub fn delete_item(&self, item_id: Uuid) -> Result<(), String> {
        if let Some(history) = &self.history {
            history.remove(item_id).map_err(|err| err.to_string())?;
        } else {
            self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(item_id);
        }
        let _ = self.app.emit(HISTORY_CHANGED, ());
        Ok(())
    }

    pub fn clear_history(&self) -> Result<usize, String> {
        let removed = match &self.history {
            Some(history) => history.clear().map_err(|err| err.to_string())?,
            None => {
                let mut recent = self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let ids: Vec<Uuid> = recent.iter().map(|entry| entry.item_id).collect();
                for id in &ids {
                    recent.remove(*id);
                }
                ids.len()
            }
        };
        tracing::info!(removed, "cleared the history");
        let _ = self.app.emit(HISTORY_CHANGED, ());
        Ok(removed)
    }

    /// A source app's icon as a `data:` URL, cached by bundle id (#37).
    pub async fn app_icon(&self, bundle_id: String) -> Option<String> {
        if let Some(cached) = self.icons.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&bundle_id) {
            return cached.clone();
        }
        let id = bundle_id.clone();
        let icon = tokio::task::spawn_blocking(move || icon_png(&id))
            .await
            .ok()
            .flatten()
            .map(|png| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)));
        let mut icons = self.icons.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if icons.len() > 256 {
            icons.clear();
        }
        icons.insert(bundle_id, icon.clone());
        icon
    }
}

/// Gives focus back to the app the panel opened over and posts ⌘V. Without
/// Accessibility, macOS would drop the keystroke, so the item is only copied.
#[cfg(target_os = "macos")]
async fn paste_into_front(app: &tauri::AppHandle) -> PasteOutcome {
    use std::time::Duration;

    use pasta_clipboard::mac_native;
    if !mac_native::accessibility_trusted(false) {
        return PasteOutcome::Copied;
    }
    let front = crate::ui::panel_front_app(app);
    let pasted = tokio::task::spawn_blocking(move || {
        if let Some(front) = &front
            && !mac_native::wait_until_front(front.pid, Duration::from_millis(80))
        {
            mac_native::activate(front.pid);
            mac_native::wait_until_front(front.pid, Duration::from_millis(600));
        }
        // The panel's key window has to be gone before the keystroke lands.
        std::thread::sleep(Duration::from_millis(40));
        mac_native::post_paste()
    })
    .await
    .unwrap_or(false);
    if pasted { PasteOutcome::Pasted } else { PasteOutcome::Copied }
}

#[cfg(not(target_os = "macos"))]
async fn paste_into_front(_app: &tauri::AppHandle) -> PasteOutcome {
    PasteOutcome::Copied
}

#[cfg(target_os = "macos")]
fn icon_png(bundle_id: &str) -> Option<Vec<u8>> {
    pasta_clipboard::mac_native::app_icon_png(bundle_id, 64.0)
}

#[cfg(not(target_os = "macos"))]
fn icon_png(_bundle_id: &str) -> Option<Vec<u8>> {
    None
}
