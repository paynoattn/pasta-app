//! KDE Plasma adapter (#50–#52): the Wayland `ext-data-control-v1` protocol
//! for watching and writing, Klipper over D-Bus for status. See ADR 0015.
//!
//! One thread owns the Wayland event queue. Writes create a data source from
//! the caller's thread (proxies are thread-safe) and the queue thread serves
//! paste requests for it until another copy replaces it.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{
    AdapterOptions, ClipboardAdapter, ClipboardError, ClipboardStatus, CopySink, LocalCopy, Watch, WriteKind,
    WriteReceipt,
};
use uuid::Uuid;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_callback, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::{self, ExtDataControlDeviceV1};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_manager_v1::ExtDataControlManagerV1;
use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::{self, ExtDataControlOfferV1};
use wayland_protocols::ext::data_control::v1::client::ext_data_control_source_v1::{self, ExtDataControlSourceV1};

/// Pasta's own-write marker. Klipper keeps only `text/*`, JSON and XML
/// formats, so an item picked again from its history comes back without it
/// and counts as a new copy; the engine's content hashes stop the echo.
pub const MARKER_MIME: &str = "application/x-pasta-origin";
/// Password managers (KeePassXC) set it to `secret`; Klipper then keeps the
/// item out of its history.
pub const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";
/// Klipper putting an item back after the clipboard emptied ("Prevent empty clipboard").
const KLIPPER_RESTORE_MIME: &str = "application/x-kde-onlyReplaceEmpty";
/// Klipper copying the mouse selection into the clipboard ("Keep the selection and clipboard the same").
const KLIPPER_SYNC_MIME: &str = "application/x-kde-syncselection";
/// Plain-text types, most preferred first.
const PLAIN_MIMES: &[&str] = &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];
const HTML_MIME: &str = "text/html";
const RTF_MIMES: &[&str] = &["text/rtf", "application/rtf"];
/// How long a copying app gets to hand over one format.
const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Larger formats are dropped; the engine's own limit is far lower.
const READ_LIMIT: usize = 32 * 1024 * 1024;

type Sink = Arc<dyn Fn(LocalCopy) + Send + Sync>;

pub struct KdeClipboard {
    conn: Connection,
    qh: QueueHandle<State>,
    manager: ExtDataControlManagerV1,
    device: ExtDataControlDeviceV1,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    options: RwLock<AdapterOptions>,
    sink: Mutex<Option<Sink>>,
    /// Pasta's source while it owns the clipboard.
    current: Mutex<Option<Current>>,
    /// The last write stamp handed out; see [`KdeClipboard::next_stamp`].
    stamp: AtomicI64,
    stopped: AtomicBool,
}

struct Current {
    source: ExtDataControlSourceV1,
    /// The receipt's `change_count`; `None` for the blank source an expiry leaves.
    stamp: Option<i64>,
}

/// What one of Pasta's sources serves. `None` serves only the marker: a
/// clipboard that pastes nothing, which Klipper doesn't refill from history.
struct SourceData {
    flavors: Option<Flavors>,
    hint: bool,
}

impl SourceData {
    fn mimes(&self) -> Vec<&'static str> {
        let mut mimes = vec![MARKER_MIME];
        if let Some(flavors) = &self.flavors {
            mimes.extend_from_slice(PLAIN_MIMES);
            if flavors.html.is_some() {
                mimes.push(HTML_MIME);
            }
            if flavors.rtf.is_some() {
                mimes.extend_from_slice(RTF_MIMES);
            }
        }
        if self.hint {
            mimes.push(PASSWORD_HINT_MIME);
        }
        mimes
    }

    fn bytes(&self, mime: &str) -> Option<&[u8]> {
        match mime {
            MARKER_MIME => Some(b"1"),
            PASSWORD_HINT_MIME if self.hint => Some(b"secret"),
            _ => {
                let flavors = self.flavors.as_ref()?;
                if PLAIN_MIMES.contains(&mime) {
                    Some(flavors.plain.as_bytes())
                } else if mime == HTML_MIME {
                    flavors.html.as_deref().map(str::as_bytes)
                } else if RTF_MIMES.contains(&mime) {
                    flavors.rtf.as_deref().map(str::as_bytes)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Default)]
struct OfferData {
    mimes: Mutex<Vec<String>>,
}

/// A clipboard item another app offers, waiting to be read.
struct Pending {
    offer: ExtDataControlOfferV1,
    mimes: Vec<String>,
}

struct State {
    shared: Arc<Shared>,
    reads: mpsc::Sender<Pending>,
}

impl KdeClipboard {
    pub fn new() -> Result<Self, ClipboardError> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Err(ClipboardError::Os("not a Wayland session".into()));
        }
        let conn = Connection::connect_to_env().map_err(|err| ClipboardError::Os(format!("wayland: {err}")))?;
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(|err| ClipboardError::Os(format!("wayland: {err}")))?;
        let qh = queue.handle();
        let manager: ExtDataControlManagerV1 = globals
            .bind(&qh, 1..=1, ())
            .map_err(|_| ClipboardError::Os("the compositor doesn't offer ext-data-control".into()))?;
        let seat: wl_seat::WlSeat =
            globals.bind(&qh, 1..=1, ()).map_err(|_| ClipboardError::Os("the compositor has no seat".into()))?;
        let device = manager.get_data_device(&seat, &qh, ());

        let shared = Arc::new(Shared::default());
        let (reads, pending) = mpsc::channel();
        let mut state = State { shared: Arc::clone(&shared), reads };
        // The first selection event is what was on the clipboard before Pasta started.
        queue.roundtrip(&mut state).map_err(|err| ClipboardError::Os(format!("wayland: {err}")))?;

        let reader_conn = conn.clone();
        let reader_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("pasta-clipboard-read".into())
            .spawn(move || read_offers(&reader_conn, &reader_shared, &pending))
            .map_err(|err| ClipboardError::Os(err.to_string()))?;
        thread::Builder::new()
            .name("pasta-wayland".into())
            .spawn(move || {
                while !state.shared.stopped.load(Ordering::SeqCst) {
                    if let Err(err) = queue.blocking_dispatch(&mut state) {
                        tracing::error!(error = %err, "lost the Wayland connection; clipboard sync stops");
                        break;
                    }
                }
                state.shared.stopped.store(true, Ordering::SeqCst);
            })
            .map_err(|err| ClipboardError::Os(err.to_string()))?;
        tracing::info!("watching the clipboard through ext-data-control");
        Ok(Self { conn, qh, manager, device, shared })
    }

    /// A receipt stamp that stays unique across restarts, so a persisted
    /// receipt never matches a later run's write.
    fn next_stamp(&self) -> i64 {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let mut last = self.shared.stamp.load(Ordering::SeqCst);
        loop {
            let next = now.max(last + 1);
            match self.shared.stamp.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return next,
                Err(actual) => last = actual,
            }
        }
    }

    /// Makes `data` the clipboard. Holds `current` across the request, so the
    /// queue thread can't see the old source cancelled before it's replaced.
    /// With `only_over`, does nothing unless the clipboard is still the write
    /// with that stamp; the check and the replace happen under one lock.
    fn set_selection(
        &self,
        data: SourceData,
        stamp: Option<i64>,
        only_over: Option<i64>,
    ) -> Result<(), ClipboardError> {
        if self.shared.stopped.load(Ordering::SeqCst) {
            return Err(ClipboardError::Stopped);
        }
        let mimes = data.mimes();
        let mut current = lock(&self.shared.current);
        if only_over.is_some() && current.as_ref().and_then(|current| current.stamp) != only_over {
            return Ok(());
        }
        let source = self.manager.create_data_source(&self.qh, Arc::new(data));
        for mime in mimes {
            source.offer(mime.to_owned());
        }
        self.device.set_selection(Some(&source));
        *current = Some(Current { source, stamp });
        self.conn.flush().map_err(|err| ClipboardError::Os(format!("wayland: {err}")))
    }

    fn write_as(&self, item_id: Uuid, flavors: &Flavors, kind: WriteKind) -> Result<WriteReceipt, ClipboardError> {
        // Without the hint Klipper records the item; with it, the item is only the current clipboard.
        let hint = kind == WriteKind::Concealed || !read_options(&self.shared.options).add_to_history;
        let stamp = self.next_stamp();
        self.set_selection(SourceData { flavors: Some(flavors.clone()), hint }, Some(stamp), None)?;
        Ok(WriteReceipt { item_id, history_id: None, change_count: Some(stamp) })
    }
}

impl Drop for KdeClipboard {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        // Wakes the queue thread so it sees the flag.
        self.conn.display().sync(&self.qh, ());
        let _ = self.conn.flush();
    }
}

impl ClipboardAdapter for KdeClipboard {
    fn watch(&self, sink: CopySink) -> Result<Watch, ClipboardError> {
        if self.shared.stopped.load(Ordering::SeqCst) {
            return Err(ClipboardError::Stopped);
        }
        *lock(&self.shared.sink) = Some(Arc::from(sink));
        let shared = Arc::clone(&self.shared);
        Ok(Watch::new(move || {
            lock(&shared.sink).take();
        }))
    }

    fn write(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        self.write_as(item_id, flavors, WriteKind::Normal)
    }

    fn write_concealed(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        self.write_as(item_id, flavors, WriteKind::Concealed)
    }

    /// Klipper has no way to delete one entry (ADR 0015), so this only clears
    /// the clipboard if the item is still on it.
    fn expire(&self, receipt: &WriteReceipt) -> Result<(), ClipboardError> {
        match receipt.change_count {
            Some(stamp) => self.set_selection(SourceData { flavors: None, hint: true }, None, Some(stamp)),
            None => Ok(()),
        }
    }

    fn configure(&self, options: &AdapterOptions) {
        *self.shared.options.write().unwrap_or_else(std::sync::PoisonError::into_inner) = options.clone();
    }

    fn status(&self) -> ClipboardStatus {
        if self.shared.stopped.load(Ordering::SeqCst) {
            return ClipboardStatus {
                unsupported: Some("Lost the connection to the desktop's clipboard; restart Pasta".into()),
                ..ClipboardStatus::default()
            };
        }
        ClipboardStatus { history_enabled: klipper_running(), ..ClipboardStatus::default() }
    }
}

/// Whether Klipper (the Plasma clipboard applet) is on the session bus.
/// `None` if the bus itself can't be reached.
pub fn klipper_running() -> Option<bool> {
    static BUS: OnceLock<Option<zbus::blocking::Connection>> = OnceLock::new();
    let bus = BUS.get_or_init(|| zbus::blocking::Connection::session().ok()).as_ref()?;
    let proxy = zbus::blocking::fdo::DBusProxy::new(bus).ok()?;
    let name = zbus::names::BusName::try_from("org.kde.klipper").ok()?;
    proxy.name_has_owner(name).ok()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn read_options(options: &RwLock<AdapterOptions>) -> AdapterOptions {
    options.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// Why a clipboard item isn't reported, or `None` if it should be.
fn skip_reason(mimes: &[String], options: &AdapterOptions) -> Option<&'static str> {
    let has = |mime: &str| mimes.iter().any(|m| m == mime);
    if has(MARKER_MIME) {
        return Some("own write");
    }
    if has(KLIPPER_RESTORE_MIME) || has(KLIPPER_SYNC_MIME) {
        return Some("Klipper restoring or syncing an item");
    }
    if options.skip_sensitive && has(PASSWORD_HINT_MIME) {
        return Some("marked as a password");
    }
    if !PLAIN_MIMES.iter().any(|mime| has(mime)) {
        return Some("no text");
    }
    None
}

/// The reader thread: reads each new clipboard item another app offers.
/// Only the newest waiting item is read; older ones were replaced already.
fn read_offers(conn: &Connection, shared: &Shared, pending: &mpsc::Receiver<Pending>) {
    while let Ok(mut next) = pending.recv() {
        while let Ok(newer) = pending.try_recv() {
            next.offer.destroy();
            next = newer;
        }
        let Pending { offer, mimes } = next;
        let sink = lock(&shared.sink).clone();
        if let Some(sink) = sink {
            let options = read_options(&shared.options);
            match skip_reason(&mimes, &options) {
                Some(reason) => tracing::debug!(reason, "skipped a clipboard item"),
                None => {
                    if let Some(flavors) = read_flavors(conn, &offer, &mimes) {
                        sink(LocalCopy { flavors, source_app: None });
                    }
                }
            }
        }
        offer.destroy();
        let _ = conn.flush();
    }
}

fn read_flavors(conn: &Connection, offer: &ExtDataControlOfferV1, mimes: &[String]) -> Option<Flavors> {
    let has = |mime: &str| mimes.iter().any(|m| m == mime);
    let plain_mime = PLAIN_MIMES.iter().find(|mime| has(mime))?;
    let plain = decode_text(read_mime(conn, offer, plain_mime)?);
    if plain.is_empty() {
        return None;
    }
    let html = has(HTML_MIME).then(|| read_mime(conn, offer, HTML_MIME)).flatten().map(decode_text);
    let rtf = RTF_MIMES
        .iter()
        .find(|mime| has(mime))
        .and_then(|mime| read_mime(conn, offer, mime))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    Some(Flavors { plain, html: html.filter(|html| !html.is_empty()), rtf })
}

/// Text in UTF-8, or UTF-16 with a byte-order mark (Firefox's `text/html`).
fn decode_text(bytes: Vec<u8>) -> String {
    let utf16 = |bytes: &[u8], from: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|&pair| from(pair)).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes.as_slice() {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => String::from_utf8(bytes).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned()),
    }
}

/// Asks the copying app for one format and reads it, with a deadline.
fn read_mime(conn: &Connection, offer: &ExtDataControlOfferV1, mime: &str) -> Option<Vec<u8>> {
    let (reader, writer) = std::io::pipe().ok()?;
    offer.receive(mime.to_owned(), writer.as_fd());
    conn.flush().ok()?;
    // Our copy of the write end must close, or the read never sees the end.
    drop(writer);
    let mut reader = File::from(OwnedFd::from(reader));
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let mut fds = [rustix::event::PollFd::new(&reader, rustix::event::PollFlags::IN)];
        let timeout = rustix::event::Timespec::try_from(left).ok()?;
        if rustix::event::poll(&mut fds, Some(&timeout)).ok()? == 0 {
            tracing::debug!(mime, "the copying app didn't hand over the clipboard in time");
            return None;
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Some(bytes),
            Ok(n) if bytes.len() + n <= READ_LIMIT => bytes.extend_from_slice(&chunk[..n]),
            Ok(_) => {
                tracing::debug!(mime, "skipped an oversized clipboard format");
                return None;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        device: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                let mimes = offer.data::<OfferData>().map(|data| lock(&data.mimes).clone()).unwrap_or_default();
                if let Err(mpsc::SendError(pending)) = state.reads.send(Pending { offer, mimes }) {
                    pending.offer.destroy();
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => offer.destroy(),
            ext_data_control_device_v1::Event::Finished => {
                tracing::warn!("the clipboard device went away");
                device.destroy();
                state.shared.stopped.store(true, Ordering::SeqCst);
            }
            _ => {}
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, OfferData::default()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, OfferData> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        data: &OfferData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            lock(&data.mimes).push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, Arc<SourceData>> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        data: &Arc<SourceData>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                // On its own thread: a slow reader must not hold up the queue.
                let data = Arc::clone(data);
                let spawned = thread::Builder::new().name("pasta-clipboard-send".into()).spawn(move || {
                    if let Some(bytes) = data.bytes(&mime_type) {
                        let _ = File::from(fd).write_all(bytes);
                    }
                });
                if let Err(err) = spawned {
                    tracing::warn!(error = %err, "couldn't hand over the clipboard");
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                let mut current = lock(&state.shared.current);
                if current.as_ref().is_some_and(|current| &current.source == source) {
                    *current = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mimes(list: &[&str]) -> Vec<String> {
        list.iter().map(|&m| m.to_owned()).collect()
    }

    #[test]
    fn skips_own_klipper_and_password_items() {
        let options = AdapterOptions::default();
        assert_eq!(skip_reason(&mimes(&["text/plain", MARKER_MIME]), &options), Some("own write"));
        assert!(skip_reason(&mimes(&["text/plain", KLIPPER_RESTORE_MIME]), &options).is_some());
        assert!(skip_reason(&mimes(&["UTF8_STRING", KLIPPER_SYNC_MIME]), &options).is_some());
        assert_eq!(skip_reason(&mimes(&["text/plain", PASSWORD_HINT_MIME]), &options), Some("marked as a password"));
        assert_eq!(skip_reason(&mimes(&["image/png"]), &options), Some("no text"));
        assert_eq!(skip_reason(&mimes(&["text/plain;charset=utf-8", "text/html"]), &options), None);
        let lax = AdapterOptions { skip_sensitive: false, ..AdapterOptions::default() };
        assert_eq!(skip_reason(&mimes(&["text/plain", PASSWORD_HINT_MIME]), &lax), None);
    }

    #[test]
    fn sources_offer_text_marker_and_hint() {
        let rich = SourceData {
            flavors: Some(Flavors { plain: "a".into(), html: Some("<b>a</b>".into()), rtf: None }),
            hint: false,
        };
        let offered = rich.mimes();
        assert!(offered.contains(&MARKER_MIME) && offered.contains(&HTML_MIME));
        assert!(!offered.contains(&PASSWORD_HINT_MIME) && !offered.contains(&"text/rtf"));
        assert_eq!(rich.bytes("UTF8_STRING"), Some(&b"a"[..]));
        assert_eq!(rich.bytes(HTML_MIME), Some(&b"<b>a</b>"[..]));
        assert_eq!(rich.bytes(PASSWORD_HINT_MIME), None);

        let blank = SourceData { flavors: None, hint: true };
        assert_eq!(blank.mimes(), vec![MARKER_MIME, PASSWORD_HINT_MIME]);
        assert_eq!(blank.bytes("text/plain"), None);
        assert_eq!(blank.bytes(PASSWORD_HINT_MIME), Some(&b"secret"[..]));
    }

    #[test]
    fn decodes_utf8_and_utf16_with_bom() {
        assert_eq!(decode_text("héllo".as_bytes().to_vec()), "héllo");
        let mut le = vec![0xFF, 0xFE];
        le.extend("<b>x</b>".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_text(le), "<b>x</b>");
        let mut be = vec![0xFE, 0xFF];
        be.extend("é".encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_text(be), "é");
    }

    #[test]
    fn decodes_invalid_utf8_lossily() {
        // A bad copy must not kill the reader thread; it reads with replacement chars.
        assert_eq!(decode_text(vec![0x80, b'a']), "\u{FFFD}a");
    }

    #[test]
    fn sources_with_rtf_offer_and_serve_it() {
        let rich = SourceData {
            flavors: Some(Flavors { plain: "b".into(), html: None, rtf: Some("{\\rtf1 b}".into()) }),
            hint: true,
        };
        let offered = rich.mimes();
        assert!(offered.contains(&"text/rtf") && offered.contains(&PASSWORD_HINT_MIME));
        assert!(!offered.contains(&HTML_MIME));
        assert_eq!(rich.bytes("text/rtf"), Some(&b"{\\rtf1 b}"[..]));
        assert_eq!(rich.bytes("text/plain"), Some(&b"b"[..]));
    }
}
