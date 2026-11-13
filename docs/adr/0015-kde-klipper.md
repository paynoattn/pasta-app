# 0015. KDE Plasma: Wayland data-control, Klipper keeps history

- Status: Accepted
- Date: 2026-11-12
- Issues: #49 (spike), #50–#55

## Context

On KDE Plasma, received text must show up in Klipper (Meta+V), and when the receiving
device's timer fires, the item should go away. Plasma 6 runs on Wayland, where an
ordinary client can only see the clipboard while it has keyboard focus, and Pasta has no
window most of the time. Klipper's D-Bus interface (`org.kde.klipper`, `/klipper`) is the
other obvious way in.

## Findings

Measured with `cargo run -p pasta-clipboard --example klipper_probe` on CachyOS, Plasma
6 Wayland, Klipper running with default settings. The probe reports booleans only.

| Case                                                       | Reported to Pasta's watcher? | Top of Klipper history? | Still in history after expiry? |
| ---------------------------------------------------------- | ---------------------------- | ----------------------- | ------------------------------ |
| Pasta writes text + `application/x-pasta-origin` marker    | no (own source)              | yes                     | **yes**                        |
| Pasta writes text + `x-kde-passwordManagerHint: secret`    | no (own source)              | **no**                  | n/a                            |
| Another client copies (Klipper `setClipboardContents`)     | yes                          | yes                     | n/a                            |
| Expiry: blank source offering only the marker and the hint | no                           | not added               | Klipper doesn't restore        |

Other observations:

- **`ext-data-control-v1` works without focus.** KWin implements it (and the older
  `zwlr_data_control_v1`). A client gets every selection change and can set the
  selection from a background thread. GNOME/Mutter doesn't offer it.
- **Klipper has no per-item delete over D-Bus.** It has `clearClipboardHistory`
  (everything) and `clearClipboardContents`; `getClipboardHistoryMenu` returns text only,
  with no ids. Editing its SQLite history while it runs doesn't change its live model.
- **Klipper honours the password-manager hint**: an offer with
  `x-kde-passwordManagerHint` = `secret` never enters history (KeePassXC uses this).
- **Klipper re-offers items itself**: when the clipboard is emptied it may restore the
  previous entry, and it syncs the clipboard and the primary selection. Those offers
  carry `application/x-kde-onlyReplaceEmpty` and `application/x-kde-syncselection`.
- **Wayland doesn't say which app copied.** There's no source pid or app id.
- **X11 is on its way out.** Plasma 6.8 drops the X11 session; there is no X11 backend.
- **Secrets:** KWallet answers the Secret Service API, so the `keyring` backend from
  ADR 0008 works unchanged.
- **Tray:** Tauri's tray on Linux (libappindicator) never reports a left click, so the
  flyout couldn't open. KWin centres new windows and ignores client positioning on
  Wayland.

## Decision

- **Watch (#50):** `ext-data-control-v1` on its own Wayland queue thread
  (`pasta_clipboard::kde`). Skip offers that carry Pasta's marker, Klipper's
  restore/sync types, no text, or (when skipping sensitive items) the password hint.
  The source app is always unknown, so excluded apps aren't offered on Plasma.
- **Write (#51):** set a data-control source offering UTF-8 text (several mime
  aliases), HTML and RTF when present, and the marker. Klipper picks it up into history.
  With **Add received items to Klipper** off, or for concealed items, the source also
  offers the password hint, so Klipper never records it.
- **Expire (#52):** if the item is still the current clipboard, replace it with a blank
  source that offers only the marker and the hint. Klipper ignores it and the clipboard
  is empty. Klipper's history entry stays: Pasta doesn't clear or rewrite Klipper's
  history, since the only D-Bus call that could do that removes everything. The Settings
  page says so, and points to turning off Add to Klipper for complete expiry.
- **Echo prevention** relies on the engine's content hashes (ADR 0006), as on Windows:
  re-selecting an entry in Klipper drops Pasta's marker.
- **History check:** "history is on" means `org.kde.klipper` has an owner on the
  session bus.
- **Unsupported sessions:** on X11 or a compositor without data-control, the adapter
  reports why and setup shows it; sync of other devices' items still works elsewhere.
- **Tray (#53):** a StatusNotifierItem through `ksni` (async-io, so zbus stays in one
  runtime mode). A left click opens the flyout, with the XDG activation token passed on
  so KWin focuses it. A small KWin script, loaded over D-Bus while Pasta runs, places the
  flyout above the tray in the panel's corner. The flyout is opaque: KWin doesn't blur
  behind it.
- **Packaging (#55):** CI builds the .deb and AppImage (Tauri bundler, which needs
  libayatana-appindicator even though Pasta's Linux tray doesn't use it). The AUR
  `pasta-bin` repackages the .deb; `packaging/arch/PKGBUILD` builds a checkout with
  `tauri build --no-bundle`. The desktop file is `pasta-app.desktop`, matching the
  Wayland app id.

## Consequences

- With Add to Klipper on, expired items stay in Klipper's history until Klipper drops
  them (its history size) or the user clears it. Only the live clipboard is cleared.
- No excluded-apps list on Plasma; password managers are caught by the hint only.
- GNOME (#60) needs a different approach (an extension or the portal), since Mutter has
  no data-control.
- The probe leaves its marker entries in Klipper's history; clear them from Klipper.

## Not verified here

- [ ] Settings with staged Apply / Reset (#53): settings still apply instantly.
- [ ] Klipper set to "ignore selection" off/on, and with history disabled.
- [ ] Multi-seat or a second Wayland seat.
- [ ] The .deb and AppImage on a clean machine (built in CI only).
