//! The sync engine (#16): broadcast local copies, apply received items, and
//! never echo an item back.
//!
//! [`SyncState`] holds the decisions and is plain data, so the dedupe and echo
//! rules are unit-testable without timers. [`spawn`] runs it on a task between
//! a [`ClipboardAdapter`] and an [`Outbound`] (the peer network).
//!
//! # Echo prevention
//!
//! The engine tracks the hash of what it believes is the current clipboard
//! item, whether it came from a local copy or from a peer. A local "copy" with
//! that same hash is not new and is never broadcast. This covers the cases
//! where an adapter's own-write marker is lost (re-selecting a Pasta item in
//! Win+V, a round trip through Universal Clipboard, apps that write twice; ADR
//! 0004, ADR 0005) without a time window. Re-copying an older item after
//! something else became current is a real copy and is sent.
//!
//! Received items are never relayed: v1 is a full mesh and each device sends
//! only its own copies.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use crate::clip::{ContentHash, Flavors};
use crate::clipboard::{ClipboardAdapter, ClipboardError, LocalCopy, WriteReceipt};
use crate::config::Settings;
use crate::device::{DeviceId, SyncMode, UnixMillis};
use crate::protocol::Clip;

/// A burst of copies closer together than this is sent once, as its last item.
pub const DEBOUNCE: Duration = Duration::from_millis(150);

/// How many received item ids are remembered to drop duplicates.
const SEEN_ITEMS: usize = 1024;

/// Why an item was not sent or not applied. Logged; never carries content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The mode doesn't send (local copy) or receive (remote item).
    Mode(SyncMode),
    /// Same text as the current clipboard item: a duplicate copy or an echo.
    Unchanged,
    /// No plain text.
    Empty,
    /// Over the item size limit even without rich flavors.
    TooLarge { bytes: usize },
    /// An item this device sent.
    OwnItem,
    /// The same item id arrived before.
    AlreadySeen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalOutcome {
    Broadcast(Clip),
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOutcome {
    Apply,
    Skip(SkipReason),
}

/// The engine's decisions, without I/O.
#[derive(Debug)]
pub struct SyncState {
    device_id: DeviceId,
    current: Option<ContentHash>,
    seen_items: RecentSet<Uuid>,
}

impl SyncState {
    pub fn new(device_id: DeviceId) -> Self {
        Self { device_id, current: None, seen_items: RecentSet::new(SEEN_ITEMS) }
    }

    /// Hash of the item the engine believes is on the clipboard.
    pub fn current(&self) -> Option<ContentHash> {
        self.current
    }

    pub fn local_copy(&mut self, copy: LocalCopy, settings: &Settings, now: UnixMillis) -> LocalOutcome {
        if copy.flavors.plain.is_empty() {
            return LocalOutcome::Skip(SkipReason::Empty);
        }
        let hash = copy.flavors.content_hash();
        if self.current == Some(hash) {
            return LocalOutcome::Skip(SkipReason::Unchanged);
        }
        // Tracked whatever the mode, so a later mode change doesn't send stale items.
        self.current = Some(hash);
        if !settings.mode.sends() {
            return LocalOutcome::Skip(SkipReason::Mode(settings.mode));
        }
        let mut flavors = copy.flavors;
        let bytes = flavors.byte_len();
        if !flavors.fit_to(settings.max_item_bytes) {
            return LocalOutcome::Skip(SkipReason::TooLarge { bytes });
        }
        LocalOutcome::Broadcast(Clip {
            item_id: Uuid::new_v4(),
            origin_device: self.device_id,
            origin_app: copy.source_app,
            created_at: now,
            flavors,
            content_hash: hash,
        })
    }

    /// Decides whether to write a received item. `clip` must already be
    /// validated (see [`Clip::validate`]).
    pub fn remote_clip(&mut self, clip: &Clip, settings: &Settings) -> RemoteOutcome {
        if clip.origin_device == self.device_id {
            return RemoteOutcome::Skip(SkipReason::OwnItem);
        }
        if !self.seen_items.insert(clip.item_id) {
            return RemoteOutcome::Skip(SkipReason::AlreadySeen);
        }
        if !settings.mode.receives() {
            return RemoteOutcome::Skip(SkipReason::Mode(settings.mode));
        }
        if self.current == Some(clip.content_hash) {
            return RemoteOutcome::Skip(SkipReason::Unchanged);
        }
        self.current = Some(clip.content_hash);
        RemoteOutcome::Apply
    }

    /// Records that Pasta itself is about to put `hash` on the clipboard.
    /// Returns the previous value so a failed write can restore it.
    pub fn set_current(&mut self, hash: Option<ContentHash>) -> Option<ContentHash> {
        std::mem::replace(&mut self.current, hash)
    }
}

/// Bounded set that forgets its oldest entries.
#[derive(Debug)]
struct RecentSet<T> {
    order: VecDeque<T>,
    items: HashSet<T>,
    capacity: usize,
}

impl<T: Copy + Eq + Hash> RecentSet<T> {
    fn new(capacity: usize) -> Self {
        Self { order: VecDeque::with_capacity(capacity), items: HashSet::with_capacity(capacity), capacity }
    }

    /// Returns `false` if `item` was already present.
    fn insert(&mut self, item: T) -> bool {
        if !self.items.insert(item) {
            return false;
        }
        self.order.push_back(item);
        if self.order.len() > self.capacity
            && let Some(old) = self.order.pop_front()
        {
            self.items.remove(&old);
        }
        true
    }
}

/// Where the engine sends local copies: every connected peer.
pub trait Outbound: Send + Sync + 'static {
    /// Queues `clip` for every connected peer; returns how many.
    fn broadcast(&self, clip: &Clip) -> usize;
}

/// What the engine did, for the UI, the Mac history (#15) and diagnostics.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    Sent { clip: Clip, peers: usize },
    Applied { clip: Clip, from: DeviceId, receipt: WriteReceipt },
    WriteFailed { item_id: Uuid, error: String },
}

enum Input {
    Local(LocalCopy),
    Remote { from: DeviceId, clip: Clip },
    WriteLocal(Flavors),
    Shutdown,
}

/// Handle to a running engine. Dropping it stops the engine.
#[derive(Debug)]
pub struct EngineHandle {
    tx: mpsc::UnboundedSender<Input>,
    task: JoinHandle<()>,
}

impl EngineHandle {
    /// Hands over an item received from `from`.
    pub fn remote(&self, from: DeviceId, clip: Clip) {
        let _ = self.tx.send(Input::Remote { from, clip });
    }

    /// Puts text on the local clipboard without sending it to peers (e.g. "Copy diagnostics").
    pub fn write_local(&self, flavors: Flavors) {
        let _ = self.tx.send(Input::WriteLocal(flavors));
    }

    /// A cloneable way to hand received items to the engine.
    pub fn sender(&self) -> EngineSender {
        EngineSender { tx: self.tx.clone() }
    }

    /// Stops watching the clipboard and waits for the engine to finish.
    pub async fn shutdown(self) {
        let _ = self.tx.send(Input::Shutdown);
        let _ = self.task.await;
    }
}

#[derive(Debug, Clone)]
pub struct EngineSender {
    tx: mpsc::UnboundedSender<Input>,
}

impl EngineSender {
    pub fn remote(&self, from: DeviceId, clip: Clip) {
        let _ = self.tx.send(Input::Remote { from, clip });
    }
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local(_) => "Local",
            Self::Remote { .. } => "Remote",
            Self::WriteLocal(_) => "WriteLocal",
            Self::Shutdown => "Shutdown",
        })
    }
}

/// Starts watching `adapter` and runs the engine on the current Tokio runtime.
pub fn spawn(
    device_id: DeviceId,
    settings: watch::Receiver<Settings>,
    adapter: Arc<dyn ClipboardAdapter>,
    outbound: Arc<dyn Outbound>,
) -> Result<(EngineHandle, mpsc::UnboundedReceiver<EngineEvent>), ClipboardError> {
    let (tx, rx) = mpsc::unbounded_channel();
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let sink_tx = tx.clone();
    let watch = adapter.watch(Box::new(move |copy| {
        let _ = sink_tx.send(Input::Local(copy));
    }))?;
    let runner = Runner { state: SyncState::new(device_id), settings, adapter, outbound, events: events_tx };
    let task = tokio::spawn(runner.run(rx, watch));
    Ok((EngineHandle { tx, task }, events_rx))
}

struct Runner {
    state: SyncState,
    settings: watch::Receiver<Settings>,
    adapter: Arc<dyn ClipboardAdapter>,
    outbound: Arc<dyn Outbound>,
    events: mpsc::UnboundedSender<EngineEvent>,
}

impl Runner {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Input>, watch: crate::clipboard::Watch) {
        let mut pending: Option<LocalCopy> = None;
        let mut deadline = Instant::now();
        loop {
            let input = if pending.is_some() {
                tokio::select! {
                    input = rx.recv() => input,
                    () = tokio::time::sleep_until(deadline) => {
                        if let Some(copy) = pending.take() {
                            self.local_copy(copy);
                        }
                        continue;
                    }
                }
            } else {
                rx.recv().await
            };
            match input {
                Some(Input::Local(copy)) => {
                    pending = Some(copy);
                    deadline = Instant::now() + DEBOUNCE;
                }
                Some(Input::Remote { from, clip }) => {
                    // Settle a copy made before this item arrived, so the item ends up current here as on the sender.
                    if let Some(copy) = pending.take() {
                        self.local_copy(copy);
                    }
                    self.remote_clip(from, clip).await;
                }
                Some(Input::WriteLocal(flavors)) => {
                    if let Some(copy) = pending.take() {
                        self.local_copy(copy);
                    }
                    let previous = self.state.set_current(Some(flavors.content_hash()));
                    if let Err(err) = self.write(Uuid::new_v4(), flavors).await {
                        tracing::warn!(error = %err, "local clipboard write failed");
                        self.state.set_current(previous);
                    }
                }
                Some(Input::Shutdown) | None => break,
            }
        }
        drop(watch);
        tracing::debug!("sync engine stopped");
    }

    fn local_copy(&mut self, copy: LocalCopy) {
        let settings = *self.settings.borrow();
        match self.state.local_copy(copy, &settings, UnixMillis::now()) {
            LocalOutcome::Broadcast(clip) => {
                let peers = self.outbound.broadcast(&clip);
                tracing::info!(
                    item = %clip.item_id,
                    hash = %clip.content_hash,
                    bytes = clip.flavors.byte_len(),
                    peers,
                    "sent local copy"
                );
                let _ = self.events.send(EngineEvent::Sent { clip, peers });
            }
            LocalOutcome::Skip(reason) => tracing::debug!(?reason, "local copy not sent"),
        }
    }

    async fn remote_clip(&mut self, from: DeviceId, clip: Clip) {
        let settings = *self.settings.borrow();
        let previous = self.state.current();
        match self.state.remote_clip(&clip, &settings) {
            RemoteOutcome::Apply => match self.write(clip.item_id, clip.flavors.clone()).await {
                Ok(receipt) => {
                    tracing::info!(
                        item = %clip.item_id,
                        hash = %clip.content_hash,
                        bytes = clip.flavors.byte_len(),
                        %from,
                        "applied item from peer"
                    );
                    let _ = self.events.send(EngineEvent::Applied { clip, from, receipt });
                }
                Err(err) => {
                    tracing::warn!(item = %clip.item_id, error = %err, "writing received item failed");
                    self.state.set_current(previous);
                    let _ =
                        self.events.send(EngineEvent::WriteFailed { item_id: clip.item_id, error: err.to_string() });
                }
            },
            RemoteOutcome::Skip(reason) => {
                tracing::debug!(item = %clip.item_id, %from, ?reason, "received item not applied");
            }
        }
    }

    async fn write(&self, item_id: Uuid, flavors: Flavors) -> Result<WriteReceipt, ClipboardError> {
        let adapter = Arc::clone(&self.adapter);
        tokio::task::spawn_blocking(move || adapter.write(item_id, &flavors))
            .await
            .unwrap_or(Err(ClipboardError::Stopped))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::testing::FakeClipboard;

    fn settings(mode: SyncMode) -> Settings {
        Settings { mode, ..Settings::default() }
    }

    fn copy(text: &str) -> LocalCopy {
        LocalCopy { flavors: Flavors::plain(text), source_app: None }
    }

    fn remote(from: DeviceId, text: &str) -> Clip {
        let flavors = Flavors::plain(text);
        Clip {
            item_id: Uuid::new_v4(),
            origin_device: from,
            origin_app: None,
            created_at: UnixMillis(1),
            content_hash: flavors.content_hash(),
            flavors,
        }
    }

    fn sent(outcome: LocalOutcome) -> Clip {
        match outcome {
            LocalOutcome::Broadcast(clip) => clip,
            LocalOutcome::Skip(reason) => panic!("not sent: {reason:?}"),
        }
    }

    #[test]
    fn broadcasts_new_copies_once() {
        let me = DeviceId::random();
        let mut state = SyncState::new(me);
        let both = settings(SyncMode::Both);
        let clip = sent(state.local_copy(copy("a"), &both, UnixMillis(5)));
        assert_eq!(clip.origin_device, me);
        assert_eq!(clip.created_at, UnixMillis(5));
        assert!(clip.validate(both.max_item_bytes).is_ok());
        // The app writing the same text twice (Teams, iTerm2 in ADR 0005).
        assert_eq!(state.local_copy(copy("a"), &both, UnixMillis(6)), LocalOutcome::Skip(SkipReason::Unchanged));
        sent(state.local_copy(copy("b"), &both, UnixMillis(7)));
        // Copying an older item again is a real copy.
        sent(state.local_copy(copy("a"), &both, UnixMillis(8)));
    }

    #[test]
    fn applied_items_are_not_echoed_back() {
        let peer = DeviceId::random();
        let mut state = SyncState::new(DeviceId::random());
        let both = settings(SyncMode::Both);
        let item = remote(peer, "from peer");
        assert_eq!(state.remote_clip(&item, &both), RemoteOutcome::Apply);
        // The watcher sees Pasta's write (marker lost), now or much later.
        assert_eq!(
            state.local_copy(copy("from peer"), &both, UnixMillis(9)),
            LocalOutcome::Skip(SkipReason::Unchanged)
        );
        // The same item arriving twice, e.g. over a replaced connection.
        assert_eq!(state.remote_clip(&item, &both), RemoteOutcome::Skip(SkipReason::AlreadySeen));
        // A different item with text we already have isn't rewritten.
        assert_eq!(state.remote_clip(&remote(peer, "from peer"), &both), RemoteOutcome::Skip(SkipReason::Unchanged));
    }

    #[test]
    fn ignores_its_own_items() {
        let me = DeviceId::random();
        let mut state = SyncState::new(me);
        let clip = remote(me, "mine");
        assert_eq!(state.remote_clip(&clip, &Settings::default()), RemoteOutcome::Skip(SkipReason::OwnItem));
    }

    #[test]
    fn follows_the_mode() {
        let peer = DeviceId::random();
        let mut state = SyncState::new(DeviceId::random());
        let receive = settings(SyncMode::Receive);
        assert_eq!(
            state.local_copy(copy("x"), &receive, UnixMillis(1)),
            LocalOutcome::Skip(SkipReason::Mode(SyncMode::Receive))
        );
        assert_eq!(state.remote_clip(&remote(peer, "y"), &receive), RemoteOutcome::Apply);

        let send = settings(SyncMode::Send);
        assert_eq!(state.remote_clip(&remote(peer, "z"), &send), RemoteOutcome::Skip(SkipReason::Mode(SyncMode::Send)));
        sent(state.local_copy(copy("w"), &send, UnixMillis(2)));

        let paused = settings(SyncMode::Paused);
        assert!(matches!(state.local_copy(copy("v"), &paused, UnixMillis(3)), LocalOutcome::Skip(_)));
        assert!(matches!(state.remote_clip(&remote(peer, "u"), &paused), RemoteOutcome::Skip(_)));
    }

    #[test]
    fn enforces_the_size_limit() {
        let mut state = SyncState::new(DeviceId::random());
        let small = Settings { max_item_bytes: 10, ..Settings::default() };
        assert_eq!(
            state.local_copy(copy(&"x".repeat(11)), &small, UnixMillis(1)),
            LocalOutcome::Skip(SkipReason::TooLarge { bytes: 11 })
        );
        let rich = LocalCopy {
            flavors: Flavors { plain: "short".into(), html: Some("<b>short</b>".into()), rtf: None },
            source_app: None,
        };
        let clip = sent(state.local_copy(rich, &small, UnixMillis(2)));
        assert_eq!(clip.flavors.html, None, "rich flavors dropped to fit");
        assert_eq!(state.local_copy(copy(""), &small, UnixMillis(3)), LocalOutcome::Skip(SkipReason::Empty));
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Clip>>);

    impl Outbound for Recorder {
        fn broadcast(&self, clip: &Clip) -> usize {
            self.0.lock().unwrap().push(clip.clone());
            1
        }
    }

    fn start(clipboard: &FakeClipboard) -> (EngineHandle, mpsc::UnboundedReceiver<EngineEvent>, Arc<Recorder>) {
        let outbound = Arc::new(Recorder::default());
        let (_tx, settings) = watch::channel(Settings::default());
        let (handle, events) =
            spawn(DeviceId::random(), settings, Arc::new(clipboard.clone()), outbound.clone() as Arc<dyn Outbound>)
                .unwrap();
        (handle, events, outbound)
    }

    #[tokio::test(start_paused = true)]
    async fn debounces_bursts_of_copies() {
        let clipboard = FakeClipboard::new();
        let (handle, _events, outbound) = start(&clipboard);
        for i in 0..5 {
            clipboard.copy(&format!("burst {i}"));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(DEBOUNCE * 2).await;
        let sent = outbound.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].flavors.plain, "burst 4");
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn writes_received_items_without_echoing_them() {
        let clipboard = FakeClipboard::new();
        let (handle, mut events, outbound) = start(&clipboard);
        let peer = DeviceId::random();
        handle.remote(peer, remote(peer, "incoming"));
        let EngineEvent::Applied { from, .. } = events.recv().await.unwrap() else { panic!("not applied") };
        assert_eq!(from, peer);
        assert_eq!(clipboard.writes().len(), 1);
        // The fake reports every write back as a copy, like Win+V without a marker.
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(outbound.0.lock().unwrap().is_empty(), "echoed a received item");

        handle.write_local(Flavors::plain("diagnostics"));
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(clipboard.writes().len(), 2);
        assert!(outbound.0.lock().unwrap().is_empty(), "sent a local-only write");
        handle.shutdown().await;
        assert!(!clipboard.is_watched(), "shutdown stops the watcher");
    }

    #[tokio::test(start_paused = true)]
    async fn failed_writes_do_not_block_the_item_later() {
        let clipboard = FakeClipboard::new();
        let (handle, mut events, _outbound) = start(&clipboard);
        let peer = DeviceId::random();
        clipboard.fail_writes(true);
        handle.remote(peer, remote(peer, "retry me"));
        assert!(matches!(events.recv().await.unwrap(), EngineEvent::WriteFailed { .. }));
        clipboard.fail_writes(false);
        handle.remote(peer, remote(peer, "retry me"));
        assert!(matches!(events.recv().await.unwrap(), EngineEvent::Applied { .. }));
        handle.shutdown().await;
    }
}
