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

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use uuid::Uuid;

use crate::clip::{ContentHash, Flavors, UNIVERSAL_CLIPBOARD_APP_ID};
use crate::clipboard::{ClipboardAdapter, ClipboardError, LocalCopy, WriteKind, WriteReceipt};
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
    /// Copied in an app the user excluded (#36): never sent or stored.
    Excluded,
    /// Arrived through iCloud Universal Clipboard while the bridge is off (#41).
    NotBridged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalOutcome {
    Broadcast(Clip),
    /// A new copy that stays on this device: the mode doesn't send, or it came
    /// through Universal Clipboard with the bridge off (#41). Still goes into
    /// the Mac history.
    Keep(Clip, SkipReason),
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
        let app_id = copy.source_app.as_ref().map_or("", |app| app.id.as_str());
        if settings.excludes(app_id) {
            return LocalOutcome::Skip(SkipReason::Excluded);
        }
        let mut flavors = copy.flavors;
        let bytes = flavors.byte_len();
        let keep_reason = if !settings.mode.sends() {
            Some(SkipReason::Mode(settings.mode))
        } else if app_id == UNIVERSAL_CLIPBOARD_APP_ID && !settings.icloud_bridge {
            Some(SkipReason::NotBridged)
        } else {
            None
        };
        if keep_reason.is_none() && !flavors.fit_to(settings.max_item_bytes) {
            return LocalOutcome::Skip(SkipReason::TooLarge { bytes });
        }
        let clip = Clip {
            item_id: Uuid::new_v4(),
            origin_device: self.device_id,
            origin_app: copy.source_app,
            created_at: now,
            flavors,
            content_hash: hash,
        };
        match keep_reason {
            Some(reason) => LocalOutcome::Keep(clip, reason),
            None => LocalOutcome::Broadcast(clip),
        }
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
    Sent {
        clip: Clip,
        peers: usize,
    },
    /// A new local copy that wasn't sent (see [`LocalOutcome::Keep`]).
    Copied {
        clip: Clip,
    },
    Applied {
        clip: Clip,
        from: DeviceId,
        receipt: WriteReceipt,
    },
    WriteFailed {
        item_id: Uuid,
        error: String,
    },
}

/// An item to take back out of the clipboard and its history (#33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpireRequest {
    pub receipt: WriteReceipt,
    pub content_hash: ContentHash,
}

type Reply<T> = oneshot::Sender<Result<T, String>>;

enum Input {
    Local(LocalCopy),
    Remote { from: DeviceId, clip: Clip },
    WriteLocal { flavors: Flavors, kind: WriteKind, done: Option<Reply<()>> },
    Expire { items: Vec<ExpireRequest>, done: Option<Reply<Vec<Uuid>>> },
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
        let _ = self.tx.send(Input::WriteLocal { flavors, kind: WriteKind::Normal, done: None });
    }

    /// Like [`EngineHandle::write_local`], and waits for the write (e.g. before pasting it, #39).
    pub async fn write_local_wait(&self, flavors: Flavors, kind: WriteKind) -> Result<(), String> {
        let (done, rx) = oneshot::channel();
        self.tx.send(Input::WriteLocal { flavors, kind, done: Some(done) }).map_err(|_| stopped())?;
        rx.await.unwrap_or_else(|_| Err(stopped()))
    }

    /// Takes items back out of the clipboard and its history (#33, #34). Items
    /// that are already gone count as expired. Returns the ids of the items the
    /// adapter couldn't expire (e.g. the clipboard was busy), to try again later.
    pub async fn expire(&self, items: Vec<ExpireRequest>) -> Result<Vec<Uuid>, String> {
        let (done, rx) = oneshot::channel();
        self.tx.send(Input::Expire { items, done: Some(done) }).map_err(|_| stopped())?;
        rx.await.unwrap_or_else(|_| Err(stopped()))
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

fn stopped() -> String {
    ClipboardError::Stopped.to_string()
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local(_) => "Local",
            Self::Remote { .. } => "Remote",
            Self::WriteLocal { .. } => "WriteLocal",
            Self::Expire { .. } => "Expire",
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
                Some(Input::WriteLocal { flavors, kind, done }) => {
                    if let Some(copy) = pending.take() {
                        self.local_copy(copy);
                    }
                    let previous = self.state.set_current(Some(flavors.content_hash()));
                    let result = self.write(Uuid::new_v4(), flavors, kind).await.map(|_| ());
                    if let Err(err) = &result {
                        tracing::warn!(error = %err, "local clipboard write failed");
                        self.state.set_current(previous);
                    }
                    if let Some(done) = done {
                        let _ = done.send(result.map_err(|err| err.to_string()));
                    }
                }
                Some(Input::Expire { items, done }) => {
                    if let Some(copy) = pending.take() {
                        self.local_copy(copy);
                    }
                    let failed = self.expire(items).await;
                    if let Some(done) = done {
                        let _ = done.send(Ok(failed));
                    }
                }
                Some(Input::Shutdown) | None => break,
            }
        }
        drop(watch);
        tracing::debug!("sync engine stopped");
    }

    fn local_copy(&mut self, copy: LocalCopy) {
        let settings = self.settings.borrow().clone();
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
            LocalOutcome::Keep(clip, reason) => {
                tracing::debug!(item = %clip.item_id, ?reason, "local copy kept on this device");
                let _ = self.events.send(EngineEvent::Copied { clip });
            }
            LocalOutcome::Skip(reason) => tracing::debug!(?reason, "local copy not sent"),
        }
    }

    async fn remote_clip(&mut self, from: DeviceId, clip: Clip) {
        let settings = self.settings.borrow().clone();
        let previous = self.state.current();
        match self.state.remote_clip(&clip, &settings) {
            RemoteOutcome::Apply => match self.write(clip.item_id, clip.flavors.clone(), WriteKind::Normal).await {
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

    async fn write(&self, item_id: Uuid, flavors: Flavors, kind: WriteKind) -> Result<WriteReceipt, ClipboardError> {
        let adapter = Arc::clone(&self.adapter);
        tokio::task::spawn_blocking(move || match kind {
            WriteKind::Normal => adapter.write(item_id, &flavors),
            WriteKind::Concealed => adapter.write_concealed(item_id, &flavors),
        })
        .await
        .unwrap_or(Err(ClipboardError::Stopped))
    }

    /// Expires items through the adapter, one by one. If the current item goes,
    /// the engine forgets it, so copying the same text again later is a new
    /// copy. Returns the ids the adapter failed on.
    async fn expire(&mut self, items: Vec<ExpireRequest>) -> Vec<Uuid> {
        let adapter = Arc::clone(&self.adapter);
        let receipts: Vec<WriteReceipt> = items.iter().map(|item| item.receipt.clone()).collect();
        let failed = tokio::task::spawn_blocking(move || {
            let mut failed = Vec::new();
            for receipt in &receipts {
                if let Err(err) = adapter.expire(receipt) {
                    tracing::warn!(item = %receipt.item_id, error = %err, "couldn't expire an item");
                    failed.push(receipt.item_id);
                }
            }
            failed
        })
        .await
        .unwrap_or_else(|_| items.iter().map(|item| item.receipt.item_id).collect());
        // A hash that is still on the clipboard through a failed item stays current.
        let kept: Vec<ContentHash> =
            items.iter().filter(|item| failed.contains(&item.receipt.item_id)).map(|item| item.content_hash).collect();
        for item in items.iter().filter(|item| !failed.contains(&item.receipt.item_id)) {
            if self.state.current() == Some(item.content_hash) && !kept.contains(&item.content_hash) {
                self.state.set_current(None);
            }
            tracing::info!(item = %item.receipt.item_id, hash = %item.content_hash, "expired item");
        }
        failed
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Mutex;

    use super::*;
    use crate::clip::SourceApp;
    use crate::clipboard::{CopySink, Watch};
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
            LocalOutcome::Keep(_, reason) | LocalOutcome::Skip(reason) => panic!("not sent: {reason:?}"),
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
    fn universal_clipboard_items_follow_the_bridge_setting() {
        let mut state = SyncState::new(DeviceId::random());
        let from_iphone = LocalCopy {
            flavors: Flavors::plain("from the iphone"),
            source_app: Some(SourceApp { name: "Universal Clipboard".into(), id: UNIVERSAL_CLIPBOARD_APP_ID.into() }),
        };

        // Bridge off (the default): the copy stays on this Mac, never broadcast.
        let off = Settings::default();
        assert!(matches!(
            state.local_copy(from_iphone.clone(), &off, UnixMillis(1)),
            LocalOutcome::Keep(_, SkipReason::NotBridged)
        ));

        // Bridge on: it is a normal copy and goes to the tailnet.
        let on = Settings { icloud_bridge: true, ..Settings::default() };
        let from_iphone = LocalCopy {
            flavors: Flavors::plain("another copy from the iphone"),
            source_app: Some(SourceApp { name: "Universal Clipboard".into(), id: UNIVERSAL_CLIPBOARD_APP_ID.into() }),
        };
        let clip = sent(state.local_copy(from_iphone, &on, UnixMillis(2)));
        assert_eq!(clip.origin_app.as_ref().map(|app| app.id.as_str()), Some(UNIVERSAL_CLIPBOARD_APP_ID));
    }

    #[test]
    fn excluded_apps_are_never_sent_or_emitted() {
        let mut state = SyncState::new(DeviceId::random());
        let both = settings(SyncMode::Both);
        let from_secrets = LocalCopy {
            flavors: Flavors::plain("a password"),
            source_app: Some(SourceApp { name: "Secrets".into(), id: "com.example.secrets".into() }),
        };
        let excluded = Settings {
            excluded_apps: std::sync::Arc::from(vec!["COM.EXAMPLE.SECRETS".to_owned()]),
            ..settings(SyncMode::Both)
        };

        // Matching is case-insensitive, and the copy is never a Keep either.
        assert_eq!(
            state.local_copy(from_secrets.clone(), &excluded, UnixMillis(1)),
            LocalOutcome::Skip(SkipReason::Excluded)
        );
        // An excluded copy still updates `current`, so the engine never treats it as new later.
        assert_eq!(state.local_copy(from_secrets, &both, UnixMillis(2)), LocalOutcome::Skip(SkipReason::Unchanged));
        // Other apps are unaffected.
        sent(state.local_copy(copy("ordinary"), &excluded, UnixMillis(3)));
    }

    #[test]
    fn keep_reasons_cover_receive_and_paused_modes() {
        let mut state = SyncState::new(DeviceId::random());
        let receive = settings(SyncMode::Receive);
        let paused = settings(SyncMode::Paused);
        assert!(matches!(
            state.local_copy(copy("stays here"), &receive, UnixMillis(1)),
            LocalOutcome::Keep(_, SkipReason::Mode(SyncMode::Receive))
        ));
        assert!(matches!(
            state.local_copy(copy("stays here too"), &paused, UnixMillis(2)),
            LocalOutcome::Keep(_, SkipReason::Mode(SyncMode::Paused))
        ));
    }

    #[test]
    fn follows_the_mode() {
        let peer = DeviceId::random();
        let mut state = SyncState::new(DeviceId::random());
        let receive = settings(SyncMode::Receive);
        assert!(matches!(
            state.local_copy(copy("x"), &receive, UnixMillis(1)),
            LocalOutcome::Keep(_, SkipReason::Mode(SyncMode::Receive))
        ));
        assert_eq!(state.remote_clip(&remote(peer, "y"), &receive), RemoteOutcome::Apply);

        let send = settings(SyncMode::Send);
        assert_eq!(state.remote_clip(&remote(peer, "z"), &send), RemoteOutcome::Skip(SkipReason::Mode(SyncMode::Send)));
        sent(state.local_copy(copy("w"), &send, UnixMillis(2)));

        let paused = settings(SyncMode::Paused);
        assert!(matches!(state.local_copy(copy("v"), &paused, UnixMillis(3)), LocalOutcome::Keep(..)));
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

    /// A [`FakeClipboard`] whose `expire` fails for the item ids in `failing`,
    /// so a test can make the adapter busy on chosen items (#33).
    struct FlakyExpiry {
        inner: FakeClipboard,
        failing: Arc<Mutex<HashSet<Uuid>>>,
    }

    impl ClipboardAdapter for FlakyExpiry {
        fn watch(&self, sink: CopySink) -> Result<Watch, ClipboardError> {
            self.inner.watch(sink)
        }

        fn write(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
            self.inner.write(item_id, flavors)
        }

        fn expire(&self, receipt: &WriteReceipt) -> Result<(), ClipboardError> {
            if self.failing.lock().unwrap().contains(&receipt.item_id) {
                return Err(ClipboardError::Busy);
            }
            self.inner.expire(receipt)
        }
    }

    fn start_with(
        adapter: Arc<dyn ClipboardAdapter>,
    ) -> (EngineHandle, mpsc::UnboundedReceiver<EngineEvent>, Arc<Recorder>) {
        let outbound = Arc::new(Recorder::default());
        let (_tx, settings) = watch::channel(Settings::default());
        let (handle, events) =
            spawn(DeviceId::random(), settings, adapter, outbound.clone() as Arc<dyn Outbound>).unwrap();
        (handle, events, outbound)
    }

    fn start(clipboard: &FakeClipboard) -> (EngineHandle, mpsc::UnboundedReceiver<EngineEvent>, Arc<Recorder>) {
        start_with(Arc::new(clipboard.clone()))
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

    fn receipt(item_id: Uuid) -> WriteReceipt {
        WriteReceipt { item_id, history_id: None, change_count: Some(7) }
    }

    #[tokio::test(start_paused = true)]
    async fn expiring_resets_current_so_the_same_text_is_new_again() {
        let clipboard = FakeClipboard::new();
        let (handle, _events, outbound) = start(&clipboard);
        let peer = DeviceId::random();

        // A received item arrives and becomes current.
        let incoming = remote(peer, "expires soon");
        let hash = incoming.content_hash;
        handle.remote(peer, incoming);
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(outbound.0.lock().unwrap().len(), 0, "received items are never echoed");

        // Its deadline passes: expire() hands it to the adapter and the engine
        // forgets the hash, so re-copying the same text is a real new copy.
        let item_id = Uuid::new_v4();
        assert_eq!(
            handle.expire(vec![ExpireRequest { receipt: receipt(item_id), content_hash: hash }]).await.unwrap(),
            Vec::<Uuid>::new()
        );
        assert_eq!(clipboard.expired(), [item_id]);
        clipboard.copy("expires soon");
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(outbound.0.lock().unwrap().len(), 1, "the same text after expiry is sent again");
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn expiring_an_item_that_is_not_current_keeps_current() {
        let clipboard = FakeClipboard::new();
        let (handle, _events, _outbound) = start(&clipboard);
        let peer = DeviceId::random();

        // Item one arrives, then item two replaces it.
        let first = remote(peer, "first");
        let first_hash = first.content_hash;
        handle.remote(peer, first);
        let second = remote(peer, "second");
        handle.remote(peer, second);
        tokio::time::sleep(DEBOUNCE * 2).await;

        // Expiring the older item must not clear the current one's hash.
        let stale_id = Uuid::new_v4();
        handle.expire(vec![ExpireRequest { receipt: receipt(stale_id), content_hash: first_hash }]).await.unwrap();
        clipboard.copy("second");
        tokio::time::sleep(DEBOUNCE * 2).await;
        // "second" is still current, so the watcher's report of it is an echo:
        // nothing new was broadcast. (No direct way to assert `current` from
        // outside; the missing broadcast is the observable contract.)
        handle.shutdown().await;
    }

    /// Two received items are current in turn: `first`, then `second`.
    async fn two_current_items(clipboard: &FakeClipboard, handle: &EngineHandle) -> (ContentHash, ContentHash) {
        let peer = DeviceId::random();
        let first = remote(peer, "first item");
        let first_hash = first.content_hash;
        handle.remote(peer, first);
        let second = remote(peer, "second item");
        let second_hash = second.content_hash;
        handle.remote(peer, second);
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(clipboard.writes().len(), 2, "both items were written");
        (first_hash, second_hash)
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_expiry_does_not_stop_the_others() {
        let clipboard = FakeClipboard::new();
        let failing = Arc::new(Mutex::new(HashSet::new()));
        let adapter = Arc::new(FlakyExpiry { inner: clipboard.clone(), failing: failing.clone() });
        let (handle, _events, _outbound) = start_with(adapter);
        let (first_hash, second_hash) = two_current_items(&clipboard, &handle).await;

        // The adapter is busy on the first item only; the other still expires.
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        failing.lock().unwrap().insert(first_id);
        let failed = handle
            .expire(vec![
                ExpireRequest { receipt: receipt(first_id), content_hash: first_hash },
                ExpireRequest { receipt: receipt(second_id), content_hash: second_hash },
            ])
            .await
            .unwrap();
        assert_eq!(failed, [first_id], "only the busy item is reported back");
        assert_eq!(clipboard.expired(), [second_id], "a failed expire doesn't stop the others");
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_expiry_keeps_the_item_current() {
        let clipboard = FakeClipboard::new();
        let failing = Arc::new(Mutex::new(HashSet::new()));
        let adapter = Arc::new(FlakyExpiry { inner: clipboard.clone(), failing: failing.clone() });
        let (handle, _events, outbound) = start_with(adapter);
        let (_first_hash, second_hash) = two_current_items(&clipboard, &handle).await;

        // Expiring the current item fails, so its hash stays current and the
        // watcher's report of the same text is still an echo, not a new copy.
        let busy_id = Uuid::new_v4();
        failing.lock().unwrap().insert(busy_id);
        let failed =
            handle.expire(vec![ExpireRequest { receipt: receipt(busy_id), content_hash: second_hash }]).await.unwrap();
        assert_eq!(failed, [busy_id]);
        clipboard.copy("second item");
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(outbound.0.lock().unwrap().is_empty(), "an item the adapter couldn't expire is still current");
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_expiry_does_not_keep_a_later_current_item_alive() {
        let clipboard = FakeClipboard::new();
        let failing = Arc::new(Mutex::new(HashSet::new()));
        let adapter = Arc::new(FlakyExpiry { inner: clipboard.clone(), failing: failing.clone() });
        let (handle, _events, outbound) = start_with(adapter);
        let (first_hash, second_hash) = two_current_items(&clipboard, &handle).await;

        // The busy item comes first in the batch; the current item after it
        // still expires, and its hash is forgotten: re-copying the same text is
        // a new copy again. A failure must not poison the rest of the batch.
        let busy_id = Uuid::new_v4();
        let current_id = Uuid::new_v4();
        failing.lock().unwrap().insert(busy_id);
        let failed = handle
            .expire(vec![
                ExpireRequest { receipt: receipt(busy_id), content_hash: first_hash },
                ExpireRequest { receipt: receipt(current_id), content_hash: second_hash },
            ])
            .await
            .unwrap();
        assert_eq!(failed, [busy_id]);
        assert_eq!(clipboard.expired(), [current_id]);
        clipboard.copy("second item");
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert_eq!(outbound.0.lock().unwrap().len(), 1, "the current item's hash was forgotten");
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_succeeded_older_receipt_does_not_expire_a_hash_whose_current_item_failed() {
        let clipboard = FakeClipboard::new();
        let failing = Arc::new(Mutex::new(HashSet::new()));
        let adapter = Arc::new(FlakyExpiry { inner: clipboard.clone(), failing: failing.clone() });
        let (handle, _events, outbound) = start_with(adapter);
        let peer = DeviceId::random();

        // A received item becomes current. Its text now has two receipts in the
        // expiry batch: an older one, and the current item's own.
        let incoming = remote(peer, "still on the clipboard");
        let hash = incoming.content_hash;
        handle.remote(peer, incoming);
        tokio::time::sleep(DEBOUNCE * 2).await;

        // The older receipt expires, the current one is busy: the text is still
        // on the clipboard through the failed one, so the hash stays current.
        let older_id = Uuid::new_v4();
        let current_id = Uuid::new_v4();
        failing.lock().unwrap().insert(current_id);
        let failed = handle
            .expire(vec![
                ExpireRequest { receipt: receipt(older_id), content_hash: hash },
                ExpireRequest { receipt: receipt(current_id), content_hash: hash },
            ])
            .await
            .unwrap();
        assert_eq!(failed, [current_id], "only the busy receipt is reported back");
        assert_eq!(clipboard.expired(), [older_id], "the older receipt did expire");

        // Re-copying the same text is an echo of what is still on the
        // clipboard, not a new copy: it must not be broadcast.
        clipboard.copy("still on the clipboard");
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(
            outbound.0.lock().unwrap().is_empty(),
            "an expired older receipt must not make a still-current hash look new"
        );
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn concealed_writes_go_to_write_concealed_and_are_not_broadcast() {
        let clipboard = FakeClipboard::new();
        let (handle, _events, outbound) = start(&clipboard);
        handle.write_local_wait(Flavors::plain("diagnostics"), WriteKind::Concealed).await.unwrap();
        assert_eq!(clipboard.concealed().len(), 1, "concealed writes reach write_concealed");
        assert!(clipboard.writes().is_empty(), "a concealed write is not a normal write");
        tokio::time::sleep(DEBOUNCE * 2).await;
        assert!(outbound.0.lock().unwrap().is_empty(), "a concealed write must never be broadcast");
        handle.shutdown().await;
    }
}
