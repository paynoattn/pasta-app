//! Windows adapter: watch with `WM_CLIPBOARDUPDATE` (#12), write into Win+V
//! history with WinRT (#14). See ADR 0004.
//!
//! Everything runs on one STA thread that owns a message-only window. WinRT's
//! `Clipboard` must be used from an STA, and the clipboard owner window it
//! creates must keep pumping messages, or other apps that empty the
//! clipboard would hang waiting for it. Writes and other requests reach the
//! thread as jobs plus a posted wake-up message.

use std::cell::{Cell, RefCell};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use pasta_core::clip::{ContentHash, Flavors, SourceApp};
use pasta_core::clipboard::{
    AdapterOptions, ClipboardAdapter, ClipboardError, ClipboardStatus, CopySink, LocalCopy, Watch, WriteKind,
    WriteReceipt,
};
use uuid::Uuid;
use windows::ApplicationModel::DataTransfer::{
    Clipboard, ClipboardContentOptions, ClipboardHistoryItem, ClipboardHistoryItemsResultStatus, DataPackage,
    HtmlFormatHelper, StandardDataFormats,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetForegroundWindow, GetMessageW,
    GetWindowTextLengthW, GetWindowThreadProcessId, HWND_MESSAGE, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW,
    PostMessageW, PostQuitMessage, RegisterClassExW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_CLIPBOARDUPDATE, WNDCLASSEXW,
};
use windows::core::BOOL;
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

use crate::cf_html;
use crate::sensitive::is_password_manager_exe;

const WM_JOB: u32 = WM_APP + 1;
const WM_STOP: u32 = WM_APP + 2;
/// `CLIPBRD_E_CANT_OPEN`: another process holds the clipboard (ADR 0004).
const CLIPBRD_E_CANT_OPEN: i32 = 0x800401D0_u32 as i32;
const JOB_TIMEOUT: Duration = Duration::from_secs(10);
/// How long to look for a written item in Win+V history (it appears after ~240 ms).
const HISTORY_LOOKUP: Duration = Duration::from_millis(1500);

type Sink = Arc<dyn Fn(LocalCopy) + Send + Sync>;
type Options = Arc<RwLock<AdapterOptions>>;

fn read_options(options: &Options) -> AdapterOptions {
    options.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

enum Job {
    Write { item_id: Uuid, flavors: Flavors, kind: WriteKind, reply: Sender<Result<WriteReceipt, ClipboardError>> },
    Expire { receipt: WriteReceipt, reply: Sender<Result<(), ClipboardError>> },
    Status { reply: Sender<ClipboardStatus> },
}

pub struct WindowsClipboard {
    jobs: Mutex<Sender<Job>>,
    /// The thread's window, as an integer because `HWND` isn't `Send`.
    hwnd: isize,
    sink: Arc<Mutex<Option<Sink>>>,
    options: Options,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl WindowsClipboard {
    pub fn new() -> Result<Self, ClipboardError> {
        let (jobs_tx, jobs_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let sink: Arc<Mutex<Option<Sink>>> = Arc::default();
        let options = Options::default();
        let thread_sink = Arc::clone(&sink);
        let thread_options = Arc::clone(&options);
        let thread = thread::Builder::new()
            .name("pasta-clipboard".into())
            .spawn(move || run_thread(jobs_rx, thread_sink, thread_options, ready_tx))
            .map_err(|err| ClipboardError::Os(err.to_string()))?;
        let hwnd = ready_rx.recv().map_err(|_| ClipboardError::Stopped)??;
        Ok(Self { jobs: Mutex::new(jobs_tx), hwnd, sink, options, thread: Mutex::new(Some(thread)) })
    }

    fn request<T>(&self, job: impl FnOnce(Sender<T>) -> Job) -> Result<T, ClipboardError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.jobs
            .lock()
            .map_err(|_| ClipboardError::Stopped)?
            .send(job(reply_tx))
            .map_err(|_| ClipboardError::Stopped)?;
        unsafe { PostMessageW(Some(hwnd(self.hwnd)), WM_JOB, WPARAM(0), LPARAM(0)) }
            .map_err(|err| ClipboardError::Os(err.message()))?;
        reply_rx.recv_timeout(JOB_TIMEOUT).map_err(|_| ClipboardError::Os("clipboard thread did not answer".into()))
    }
}

impl Drop for WindowsClipboard {
    fn drop(&mut self) {
        let _ = unsafe { PostMessageW(Some(hwnd(self.hwnd)), WM_STOP, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.lock().ok().and_then(|mut thread| thread.take()) {
            let _ = thread.join();
        }
    }
}

impl ClipboardAdapter for WindowsClipboard {
    fn watch(&self, sink: CopySink) -> Result<Watch, ClipboardError> {
        *self.sink.lock().map_err(|_| ClipboardError::Stopped)? = Some(Arc::from(sink));
        let slot = Arc::clone(&self.sink);
        Ok(Watch::new(move || {
            if let Ok(mut sink) = slot.lock() {
                *sink = None;
            }
        }))
    }

    fn write(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        let flavors = flavors.clone();
        self.request(|reply| Job::Write { item_id, flavors, kind: WriteKind::Normal, reply })?
    }

    fn write_concealed(&self, item_id: Uuid, flavors: &Flavors) -> Result<WriteReceipt, ClipboardError> {
        let flavors = flavors.clone();
        self.request(|reply| Job::Write { item_id, flavors, kind: WriteKind::Concealed, reply })?
    }

    fn configure(&self, options: &AdapterOptions) {
        *self.options.write().unwrap_or_else(std::sync::PoisonError::into_inner) = options.clone();
    }

    fn expire(&self, receipt: &WriteReceipt) -> Result<(), ClipboardError> {
        let receipt = receipt.clone();
        self.request(|reply| Job::Expire { receipt, reply })?
    }

    fn status(&self) -> ClipboardStatus {
        self.request(|reply| Job::Status { reply }).unwrap_or_default()
    }
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

/// Clipboard formats registered by name.
struct Formats {
    html: u32,
    exclude: u32,
    can_include_in_history: u32,
    can_upload_to_cloud: u32,
}

impl Formats {
    fn register() -> Self {
        let register = |name: PCWSTR| unsafe { RegisterClipboardFormatW(name) };
        Self {
            html: register(w!("HTML Format")),
            exclude: register(w!("ExcludeClipboardContentFromMonitorProcessing")),
            can_include_in_history: register(w!("CanIncludeInClipboardHistory")),
            can_upload_to_cloud: register(w!("CanUploadToCloudClipboard")),
        }
    }
}

/// State owned by the clipboard thread.
struct ThreadState {
    hwnd: HWND,
    jobs: Receiver<Job>,
    sink: Arc<Mutex<Option<Sink>>>,
    options: Options,
    formats: Formats,
    own_pid: u32,
}

thread_local! {
    static STATE: RefCell<Option<ThreadState>> = const { RefCell::new(None) };
    /// A clipboard change seen while a job held the state; handled when it finishes.
    static MISSED_UPDATE: Cell<bool> = const { Cell::new(false) };
}

fn run_thread(
    jobs: Receiver<Job>,
    sink: Arc<Mutex<Option<Sink>>>,
    options: Options,
    ready: Sender<Result<isize, ClipboardError>>,
) {
    let window = unsafe { create_window() };
    let hwnd = match window {
        Ok(hwnd) => hwnd,
        Err(err) => {
            let _ = ready.send(Err(ClipboardError::Os(err.message())));
            return;
        }
    };
    let formats = Formats::register();
    let own_pid = unsafe { GetCurrentProcessId() };
    STATE.with(|state| *state.borrow_mut() = Some(ThreadState { hwnd, jobs, sink, options, formats, own_pid }));
    let _ = ready.send(Ok(hwnd.0 as isize));

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unsafe {
        let _ = RemoveClipboardFormatListener(hwnd);
        let _ = DestroyWindow(hwnd);
        RoUninitialize();
    }
    STATE.with(|state| state.borrow_mut().take());
    tracing::debug!("clipboard thread stopped");
}

unsafe fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("PastaClipboardWindow");
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // Registering twice (a second adapter in one process) fails harmlessly.
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("Pasta clipboard"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )?;
        AddClipboardFormatListener(hwnd)?;
        Ok(hwnd)
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            if !with_state(on_clipboard_update) {
                MISSED_UPDATE.with(|missed| missed.set(true));
            }
            LRESULT(0)
        }
        WM_JOB => {
            with_state(run_jobs);
            LRESULT(0)
        }
        WM_STOP => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Runs `f` on the thread state unless a job is already using it (jobs pump
/// messages while they wait, so this is re-entered). Returns `false` if it
/// didn't run; the running job picks up whatever arrived meanwhile.
fn with_state(f: fn(&mut ThreadState)) -> bool {
    STATE.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return false };
        if let Some(state) = guard.as_mut() {
            f(state);
        }
        true
    })
}

fn run_jobs(state: &mut ThreadState) {
    while let Ok(job) = state.jobs.try_recv() {
        match job {
            Job::Write { item_id, flavors, kind, reply } => {
                let add_to_history = read_options(&state.options).add_to_history;
                let _ = reply.send(write(item_id, &flavors, kind, add_to_history));
            }
            Job::Expire { receipt, reply } => {
                let _ = reply.send(expire(&receipt));
            }
            Job::Status { reply } => {
                let history_enabled = Clipboard::IsHistoryEnabled().ok();
                let _ = reply.send(ClipboardStatus { history_enabled, cloud_sync: cloud_sync(), unsupported: None });
            }
        }
    }
    if MISSED_UPDATE.with(|missed| missed.replace(false)) {
        on_clipboard_update(state);
    }
}

fn on_clipboard_update(state: &mut ThreadState) {
    let Some(sink) = state.sink.lock().ok().and_then(|sink| sink.clone()) else { return };
    let owner_pid = unsafe { GetClipboardOwner() }.ok().filter(|owner| !owner.is_invalid()).map(window_pid);
    if owner_pid == Some(state.own_pid) {
        return; // Pasta's own write
    }
    // Copies with no owner window (rare) are attributed to the foreground app.
    let source_pid = owner_pid.or_else(|| Some(window_pid(unsafe { GetForegroundWindow() })));
    let source = source_pid.and_then(process_source_app);
    let options = read_options(&state.options);
    if let Some(app) = &source {
        if options.skip_sensitive && is_password_manager_exe(&app.id) {
            tracing::debug!(app = %app.id, "skipped a copy from a password manager");
            return;
        }
        if options.excluded_apps.iter().any(|id| id.eq_ignore_ascii_case(&app.id)) {
            tracing::debug!(app = %app.id, "skipped a copy from an excluded app");
            return;
        }
    }
    match read_clipboard(state, options.skip_sensitive) {
        Ok(Some(flavors)) => sink(LocalCopy { flavors, source_app: source }),
        Ok(None) => {}
        Err(err) => tracing::warn!(error = %err, "couldn't read the clipboard"),
    }
}

/// Apps with a visible, titled top-level window, except Pasta.
pub(crate) fn running_apps() -> Vec<SourceApp> {
    unsafe extern "system" fn collect(hwnd: HWND, pids: LPARAM) -> BOOL {
        // SAFETY: `pids` is the Vec passed to EnumWindows below, alive for the call.
        let pids = unsafe { &mut *(pids.0 as *mut Vec<u32>) };
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        if visible && unsafe { GetWindowTextLengthW(hwnd) } > 0 {
            let pid = window_pid(hwnd);
            if !pids.contains(&pid) {
                pids.push(pid);
            }
        }
        BOOL(1)
    }
    let mut pids: Vec<u32> = Vec::new();
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&raw mut pids as isize)) };
    let own = unsafe { GetCurrentProcessId() };
    pids.into_iter().filter(|pid| *pid != own).filter_map(process_source_app).collect()
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Executable name and path stem of a process, e.g. `notepad.exe` / `notepad`.
fn process_source_app(pid: u32) -> Option<SourceApp> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        let exe = path.rsplit(['\\', '/']).next()?.to_owned();
        let name = exe.strip_suffix(".exe").or_else(|| exe.strip_suffix(".EXE")).unwrap_or(&exe).to_owned();
        Some(SourceApp { name, id: exe })
    }
}

/// Reads text, HTML and RTF unless the item is marked sensitive. `Ok(None)`
/// for items that are sensitive or have no text.
fn read_clipboard(state: &ThreadState, skip_sensitive: bool) -> Result<Option<Flavors>, ClipboardError> {
    open_clipboard(state.hwnd)?;
    let result = (|| {
        let formats = &state.formats;
        let available = |format: u32| unsafe { IsClipboardFormatAvailable(format) }.is_ok();
        if skip_sensitive && available(formats.exclude) {
            tracing::debug!("skipped a copy marked ExcludeClipboardContentFromMonitorProcessing");
            return Ok(None);
        }
        for (format, name) in [
            (formats.can_include_in_history, "CanIncludeInClipboardHistory"),
            (formats.can_upload_to_cloud, "CanUploadToCloudClipboard"),
        ] {
            if skip_sensitive
                && available(format)
                && read_bytes(format).is_some_and(|bytes| bytes.iter().take(4).all(|&b| b == 0))
            {
                tracing::debug!(format = name, "skipped a copy marked as not for history");
                return Ok(None);
            }
        }
        let Some(plain) = read_bytes(u32::from(CF_UNICODETEXT.0)).map(|bytes| utf16_text(&bytes)) else {
            return Ok(None);
        };
        let html =
            if available(formats.html) { read_bytes(formats.html).and_then(|b| cf_html::parse(&b)) } else { None };
        let rtf_format = unsafe { RegisterClipboardFormatW(w!("Rich Text Format")) };
        let rtf = if available(rtf_format) {
            read_bytes(rtf_format)
                .map(|bytes| String::from_utf8_lossy(bytes.split(|&b| b == 0).next().unwrap_or_default()).into_owned())
        } else {
            None
        };
        Ok((!plain.is_empty()).then_some(Flavors { plain, html, rtf }))
    })();
    let _ = unsafe { CloseClipboard() };
    result
}

/// The clipboard is a shared lock; another app may hold it for a moment.
fn open_clipboard(hwnd: HWND) -> Result<(), ClipboardError> {
    for attempt in 0..10u64 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(10 * (attempt + 1)));
    }
    Err(ClipboardError::Busy)
}

/// Copies one format's data while the clipboard is open.
fn read_bytes(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let handle: HANDLE = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        let data = GlobalLock(global).cast::<u8>();
        if data.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(data, size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }
}

fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> =
        bytes.as_chunks::<2>().0.iter().map(|pair| u16::from_le_bytes(*pair)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

fn os_error(err: &windows::core::Error) -> ClipboardError {
    if err.code().0 == CLIPBRD_E_CANT_OPEN { ClipboardError::Busy } else { ClipboardError::Os(err.message()) }
}

/// Retries a WinRT clipboard call that fails while another process holds the clipboard.
fn retry<T>(mut call: impl FnMut() -> windows::core::Result<T>) -> Result<T, ClipboardError> {
    let mut attempt = 0;
    loop {
        match call() {
            Ok(value) => return Ok(value),
            Err(err) if err.code().0 == CLIPBRD_E_CANT_OPEN && attempt < 20 => {
                attempt += 1;
                pump_for(Duration::from_millis(50));
            }
            Err(err) => return Err(os_error(&err)),
        }
    }
}

/// Keeps the window responsive while a job waits.
fn pump_for(duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        sleep(Duration::from_millis(10));
    }
}

/// Whether "Sync across your devices" is on (`HKCU\Software\Microsoft\Clipboard\EnableCloudClipboard`).
fn cloud_sync() -> Option<bool> {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Clipboard"),
            w!("EnableCloudClipboard"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    // A missing value means the user never turned it on.
    Some(result.is_ok() && value != 0)
}

fn write(
    item_id: Uuid,
    flavors: &Flavors,
    kind: WriteKind,
    add_to_history: bool,
) -> Result<WriteReceipt, ClipboardError> {
    let concealed = kind == WriteKind::Concealed;
    let package = DataPackage::new().map_err(|err| os_error(&err))?;
    package.SetText(&HSTRING::from(flavors.plain.as_str())).map_err(|err| os_error(&err))?;
    if let Some(html) = flavors.html.as_ref().filter(|_| !concealed) {
        // RTF isn't written: Win+V history drops it (ADR 0004).
        let wrapped =
            HtmlFormatHelper::CreateHtmlFormat(&HSTRING::from(html.as_str())).map_err(|err| os_error(&err))?;
        package.SetHtmlFormat(&wrapped).map_err(|err| os_error(&err))?;
    }
    let tracked = add_to_history && !concealed && Clipboard::IsHistoryEnabled().unwrap_or(false);
    // Entries already in Win+V, so an older copy of the same text isn't taken for this one.
    let before: Vec<String> = if tracked { history_ids() } else { Vec::new() };
    let options = ClipboardContentOptions::new().map_err(|err| os_error(&err))?;
    // Concealed writes (the chain words, #46) never go into Win+V.
    options.SetIsAllowedInHistory(add_to_history && !concealed).map_err(|err| os_error(&err))?;
    // Windows' own cloud clipboard shouldn't sync it a second time.
    options.SetIsRoamable(false).map_err(|err| os_error(&err))?;
    retry(|| Clipboard::SetContentWithOptions(&package, &options))?;
    retry(Clipboard::Flush)?;
    let change_count = Some(i64::from(unsafe { GetClipboardSequenceNumber() }));

    let history_id = if tracked {
        let found = find_in_history(flavors.content_hash(), &before);
        if found.is_none() {
            tracing::debug!(item = %item_id, "written item not found in Win+V history");
        }
        found
    } else {
        None
    };
    Ok(WriteReceipt { item_id, history_id, change_count })
}

fn history_items() -> Vec<ClipboardHistoryItem> {
    let result = Clipboard::GetHistoryItemsAsync().and_then(|operation| operation.join());
    match result {
        Ok(result) if result.Status().ok() == Some(ClipboardHistoryItemsResultStatus::Success) => {
            result.Items().map(|items| items.into_iter().collect()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

fn history_ids() -> Vec<String> {
    history_items().into_iter().take(25).filter_map(|item| item.Id().ok().map(|id| id.to_string())).collect()
}

/// Id of the history entry Pasta's write created: the newest one whose text has
/// `hash` and that wasn't there `before` (ADR 0004: track by id). Expiry deletes
/// by this id, so an older entry with the same text must not be picked. If Win+V
/// only moved an existing entry up, that entry is the one.
fn find_in_history(hash: ContentHash, before: &[String]) -> Option<String> {
    let started = Instant::now();
    let text_format = StandardDataFormats::Text().ok()?;
    let mut existing = None;
    while started.elapsed() < HISTORY_LOOKUP {
        for (index, item) in history_items().into_iter().take(3).enumerate() {
            let Ok(content) = item.Content() else { continue };
            if !content.Contains(&text_format).unwrap_or(false) {
                continue;
            }
            let Ok(text) = content.GetTextAsync().and_then(|operation| operation.join()) else { continue };
            if Flavors::plain(text.to_string()).content_hash() != hash {
                continue;
            }
            let Some(id) = item.Id().ok().map(|id| id.to_string()) else { continue };
            if !before.contains(&id) {
                return Some(id);
            }
            // Win+V may move an older entry with the same text to the top instead of
            // adding one: accept it only once it's on top and wasn't before.
            if index == 0 && before.first() != Some(&id) {
                existing.get_or_insert(id);
            }
        }
        pump_for(Duration::from_millis(100));
    }
    existing
}

fn expire(receipt: &WriteReceipt) -> Result<(), ClipboardError> {
    if receipt.change_count == Some(i64::from(unsafe { GetClipboardSequenceNumber() })) {
        retry(Clipboard::Clear)?;
    }
    if let Some(id) = &receipt.history_id
        && let Some(item) =
            history_items().into_iter().find(|item| item.Id().is_ok_and(|item_id| item_id == id.as_str()))
    {
        Clipboard::DeleteItemFromHistory(&item).map_err(|err| os_error(&err))?;
    }
    Ok(())
}
