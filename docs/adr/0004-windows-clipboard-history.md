# 0004. Windows: write and expire items in Win+V history

- Status: Accepted
- Date: 2026-10-23
- Issue: #5 (spike)

## Context

On Windows, Pasta has no history UI of its own: received text must appear in Win+V,
and when the receiving device's timer fires, exactly that entry must disappear.

## Findings

Measured with `cargo run -p pasta-clipboard --example win_history_probe` on
Windows 11, clipboard history on, cloud clipboard off, from an unpackaged desktop
process started from a non-interactive shell (no window of its own).

| Case                                                     | In Win+V?          | Formats kept in history   | `DeleteItemFromHistory` |
| -------------------------------------------------------- | ------------------ | ------------------------- | ----------------------- |
| Win32 `SetClipboardData` text                            | yes, after ~240 ms | Text, Locale              | `true`, entry gone      |
| Win32 text + `HTML Format` + RTF + custom `Pasta.Origin` | yes, after ~240 ms | Text, Locale, HTML Format | `true`, entry gone      |
| Win32 + `ExcludeClipboardContentFromMonitorProcessing`   | **no**             | n/a                       | n/a                     |
| Win32 + `CanIncludeInClipboardHistory = 0`               | **no**             | n/a                       | n/a                     |
| WinRT `Clipboard.SetContent`                             | yes, after ~240 ms | Text                      | `true`, entry gone      |
| WinRT `SetContentWithOptions` (in history, not roamable) | yes, after ~240 ms | Text                      | `true`, entry gone      |

Other observations:

- **The WinRT `Clipboard` class must be used from an STA thread.** From an MTA thread,
  activation fails with `0x8000001D` ("Activating a single-threaded class from MTA is
  not supported").
- **Writes can fail transiently** with `CLIPBRD_E_CANT_OPEN` (`0x800401D0`) while another
  process holds the clipboard. The same write succeeds on a retry.
- **History drops RTF and custom formats.** Only Text, Locale and HTML Format survive.
  So a Pasta marker format is visible on the _live_ clipboard right after Pasta writes,
  but **not** when the user re-selects that entry from Win+V. The watcher would then
  see what looks like a fresh copy.
- `GetHistoryItemsAsync` + `DeleteItemFromHistory` work from an unpackaged desktop app;
  no package identity is needed. Each history item has an `Id` and `Timestamp`.
- `SetHistoryItemAsContent` restores an earlier entry as the current clipboard.
- Unpackaged apps don't need the foreground to read history or delete entries.

## Decision

- **Write (#14):** WinRT `Clipboard.SetContentWithOptions` with `IsAllowedInHistory = true`
  and `IsRoamable = false`, carrying Text plus HTML when present, on a dedicated STA
  thread. Not roamable, so Windows' own cloud clipboard doesn't sync it a second time.
  Retry `CLIPBRD_E_CANT_OPEN` with short backoff (e.g. 20 × 50 ms).
- **Track (#14):** after writing, find the new history entry (within ~1 s, matching on
  content hash) and store its `Id` with the item's expiry deadline.
- **Expire (#34):** look up the entry by `Id` via `GetHistoryItemsAsync` and call
  `DeleteItemFromHistory`. If the item is still the current clipboard, clear it too.
  An entry that's already missing (reboot, user cleared history) counts as expired.
- **Watch (#12):** `AddClipboardFormatListener` on a message-only window. Skip items
  carrying `ExcludeClipboardContentFromMonitorProcessing` or
  `CanIncludeInClipboardHistory = 0`; that's how password managers mark secrets.
- **Echo prevention (#16) relies on content hashes, not a marker format.** The engine
  keeps hashes of recently received items and doesn't re-broadcast a "copy" whose hash
  matches, because re-selecting a Pasta entry in Win+V strips any marker.
- **Rich text:** sync HTML. RTF is not preserved by Win+V, so it isn't worth writing on
  Windows.

## Not verified here

- [ ] Behaviour after a reboot (Microsoft documents that unpinned history is cleared on
      restart). The expiry engine treats missing entries as expired either way.
- [ ] Interaction with cloud clipboard turned **on** (it was off on the test machine).
- [ ] Same calls from the packaged tray app at login (should match; re-check in #34).
