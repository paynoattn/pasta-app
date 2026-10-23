//! Spike #5 probe: can Pasta put text into Win+V history and delete exactly
//! that entry later?
//!
//! Run with `cargo run -p pasta-clipboard --example win_history_probe`.
//! Each case writes a unique marker string, looks for it in clipboard history,
//! prints which formats survived, deletes it and checks it is gone. Only the
//! probe's own entries are deleted, and the item that was on the clipboard
//! before the run is put back at the end. Clipboard text is never printed
//! except the probe's own markers.

#![allow(clippy::print_stdout)]

#[cfg(not(windows))]
fn main() {
    println!("win_history_probe only runs on Windows");
}

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    probe::run()
}

#[cfg(windows)]
mod probe {
    use std::thread::sleep;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows::ApplicationModel::DataTransfer::{
        Clipboard, ClipboardContentOptions, ClipboardHistoryItem, ClipboardHistoryItemsResultStatus, DataPackage,
        StandardDataFormats,
    };
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize};
    use windows::core::{HSTRING, Result};

    const APPEAR_TIMEOUT: Duration = Duration::from_secs(3);
    const GONE_TIMEOUT: Duration = Duration::from_millis(800);
    const MARKER_FORMAT: &str = "Pasta.Origin";

    struct Case {
        name: &'static str,
        write: fn(&str) -> Result<()>,
        expect_in_history: bool,
    }

    pub fn run() -> Result<()> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };

        let history = Clipboard::IsHistoryEnabled()?;
        let roaming = Clipboard::IsRoamingEnabled()?;
        println!("clipboard history enabled: {history}");
        println!("cloud clipboard (roaming) enabled: {roaming}");
        if !history {
            println!("history is off: turn it on in Settings > System > Clipboard and re-run");
            return Ok(());
        }

        let original = first_history_item()?;
        println!("existing history items present: {}", original.is_some());

        let cases = [
            Case { name: "win32: unicode text", write: write_win32_plain, expect_in_history: true },
            Case { name: "win32: text + HTML + RTF + marker", write: write_win32_rich, expect_in_history: true },
            Case {
                name: "win32: text + ExcludeClipboardContentFromMonitorProcessing",
                write: write_win32_excluded,
                expect_in_history: false,
            },
            Case {
                name: "win32: text + CanIncludeInClipboardHistory=0",
                write: write_win32_no_history,
                expect_in_history: false,
            },
            Case { name: "winrt: SetContent", write: write_winrt_plain, expect_in_history: true },
            Case {
                name: "winrt: SetContentWithOptions(history, not roamable)",
                write: write_winrt_no_roaming,
                expect_in_history: true,
            },
        ];

        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
        for (index, case) in cases.iter().enumerate() {
            let marker = format!("pasta-spike-{stamp}-{index}");
            println!("\n== {} ==", case.name);
            if let Err(err) = (case.write)(&marker) {
                println!("  write failed: {err}");
                continue;
            }
            match find_in_history(&marker, APPEAR_TIMEOUT)? {
                Some((item, elapsed)) => {
                    println!("  in history after {} ms (expected: {})", elapsed.as_millis(), case.expect_in_history);
                    println!("  formats: {}", formats(&item)?.join(", "));
                    let deleted = Clipboard::DeleteItemFromHistory(&item)?;
                    let gone = find_in_history(&marker, GONE_TIMEOUT)?.is_none();
                    println!("  DeleteItemFromHistory returned {deleted}; gone afterwards: {gone}");
                }
                None => println!("  not in history (expected: {})", case.expect_in_history),
            }
        }

        if let Some(item) = original {
            let status = Clipboard::SetHistoryItemAsContent(&item)?;
            println!("\nrestored previous clipboard item: {status:?}");
        }
        Ok(())
    }

    fn history_items() -> Result<Vec<ClipboardHistoryItem>> {
        let result = Clipboard::GetHistoryItemsAsync()?.join()?;
        let status = result.Status()?;
        if status != ClipboardHistoryItemsResultStatus::Success {
            println!("  GetHistoryItemsAsync status: {status:?}");
            return Ok(Vec::new());
        }
        Ok(result.Items()?.into_iter().collect())
    }

    fn first_history_item() -> Result<Option<ClipboardHistoryItem>> {
        Ok(history_items()?.into_iter().next())
    }

    fn find_in_history(marker: &str, timeout: Duration) -> Result<Option<(ClipboardHistoryItem, Duration)>> {
        let started = Instant::now();
        let text_format = StandardDataFormats::Text()?;
        loop {
            for item in history_items()? {
                let content = item.Content()?;
                if content.Contains(&text_format)? && content.GetTextAsync()?.join()? == marker {
                    return Ok(Some((item, started.elapsed())));
                }
            }
            if started.elapsed() >= timeout {
                return Ok(None);
            }
            sleep(Duration::from_millis(100));
        }
    }

    fn formats(item: &ClipboardHistoryItem) -> Result<Vec<String>> {
        Ok(item.Content()?.AvailableFormats()?.into_iter().map(|f| f.to_string()).collect())
    }

    fn write_win32_plain(text: &str) -> Result<()> {
        write_win32(text, &[])
    }

    fn write_win32_rich(text: &str) -> Result<()> {
        let html = cf_html(&format!("<b>{text}</b>"));
        let rtf = format!("{{\\rtf1\\ansi {{\\b {text}}}}}\0").into_bytes();
        write_win32(text, &[("HTML Format", html), ("Rich Text Format", rtf), (MARKER_FORMAT, vec![1])])
    }

    fn write_win32_excluded(text: &str) -> Result<()> {
        write_win32(text, &[("ExcludeClipboardContentFromMonitorProcessing", 0u32.to_le_bytes().to_vec())])
    }

    fn write_win32_no_history(text: &str) -> Result<()> {
        write_win32(text, &[("CanIncludeInClipboardHistory", 0u32.to_le_bytes().to_vec())])
    }

    fn write_winrt_plain(text: &str) -> Result<()> {
        let package = DataPackage::new()?;
        package.SetText(&HSTRING::from(text))?;
        retry("SetContent", || Clipboard::SetContent(&package))?;
        retry("Flush", Clipboard::Flush)
    }

    fn write_winrt_no_roaming(text: &str) -> Result<()> {
        let package = DataPackage::new()?;
        package.SetText(&HSTRING::from(text))?;
        let options = ClipboardContentOptions::new()?;
        options.SetIsAllowedInHistory(true)?;
        options.SetIsRoamable(false)?;
        retry("SetContentWithOptions", || Clipboard::SetContentWithOptions(&package, &options).map(drop))?;
        retry("Flush", Clipboard::Flush)
    }

    /// Retries a WinRT clipboard call that fails while another process holds the clipboard.
    fn retry(step: &str, mut call: impl FnMut() -> Result<()>) -> Result<()> {
        let mut attempt = 1;
        loop {
            match call() {
                Ok(()) => {
                    if attempt > 1 {
                        println!("  {step} succeeded on attempt {attempt}");
                    }
                    return Ok(());
                }
                Err(err) if attempt >= 20 => {
                    println!("  {step} still failing after {attempt} attempts");
                    return Err(err);
                }
                Err(_) => {
                    attempt += 1;
                    sleep(Duration::from_millis(50));
                }
            }
        }
    }

    /// Replaces the clipboard with `text` plus extra registered formats.
    fn write_win32(text: &str, extra: &[(&str, Vec<u8>)]) -> Result<()> {
        open_clipboard()?;
        let result = (|| unsafe {
            EmptyClipboard()?;
            let wide: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
            set_data(u32::from(CF_UNICODETEXT.0), &wide)?;
            for (name, bytes) in extra {
                let format = RegisterClipboardFormatW(&HSTRING::from(*name));
                set_data(format, bytes)?;
            }
            Ok(())
        })();
        unsafe { CloseClipboard()? };
        result
    }

    /// The clipboard is a shared lock; another app may hold it briefly.
    fn open_clipboard() -> Result<()> {
        let mut attempt = 0;
        loop {
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(()),
                Err(err) if attempt >= 10 => return Err(err),
                Err(_) => {
                    attempt += 1;
                    sleep(Duration::from_millis(20));
                }
            }
        }
    }

    unsafe fn set_data(format: u32, bytes: &[u8]) -> Result<()> {
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
            let target = GlobalLock(memory).cast::<u8>();
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            // Returns an "error" with code 0 once the lock count reaches zero.
            let _ = GlobalUnlock(memory);
            // On success the clipboard owns the memory.
            SetClipboardData(format, Some(HANDLE(memory.0)))?;
        }
        Ok(())
    }

    /// Wraps an HTML fragment in the CF_HTML header with byte offsets.
    fn cf_html(fragment: &str) -> Vec<u8> {
        const HEADER_LEN: usize = 105; // header below with 10-digit offsets
        let prefix = "<html><body><!--StartFragment-->";
        let suffix = "<!--EndFragment--></body></html>";
        let start_html = HEADER_LEN;
        let start_fragment = start_html + prefix.len();
        let end_fragment = start_fragment + fragment.len();
        let end_html = end_fragment + suffix.len();
        let header = format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
        );
        debug_assert_eq!(header.len(), HEADER_LEN);
        let mut bytes = format!("{header}{prefix}{fragment}{suffix}").into_bytes();
        bytes.push(0);
        bytes
    }
}
