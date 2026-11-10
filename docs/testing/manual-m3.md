# Manual test plan: M3 full client

Run on real hardware before closing M3 (#28–#48), next to the automated tests (`pnpm
check`). Use a Mac and a Windows PC on one tailnet, both on an M3 build; copy only
throwaway text.

## Windows and look (#28, #29)

- [ ] Settings, setup, the tray popover / flyout and (macOS) the panel open, each with the
      OS's look: macOS grouped lists and segmented controls, Windows Fluent cards and
      toggles, light and dark following the OS (switch it while Pasta runs).
- [ ] Settings search finds "expiry", "words", "login" and jumps to the section.
- [ ] Closing Settings hides it; Pasta keeps running in the tray / menu bar.

## Devices and remote settings (#30, #31, #32)

- [ ] Devices shows the chain card ("Home sync chain · end-to-end encrypted · N
      devices"), each device with OS, tailnet address, online/offline and latency, and
      "This device".
- [ ] Set the PC to **Receive** from the Mac: the PC's Settings shows Receive within a
      few seconds; copies on the PC no longer reach the Mac; the Mac's row for the PC
      says Receive.
- [ ] Quit Pasta on the PC, change its expiry on the Mac: the row says "changes reach it
      when it reconnects". Start the PC: the change arrives, and the note goes away.
- [ ] Change the same device from both machines at nearly the same time: both end up
      showing the same value.
- [ ] Pause for 1 hour (tray or Settings): nothing goes out or comes in; the tray says
      paused; Resume, or wait (set the clock forward), and syncing resumes by itself.
- [ ] Rename this device; remove another (it stops receiving at once).

## Expiry (#33, #34, #35)

- [ ] PC expiry 30 s: copy on the Mac, it shows in Win+V, and is gone from Win+V and the
      clipboard after 30 s. The flyout shows "expires in … s" while it waits.
- [ ] Quit Pasta on the PC with an item waiting; start it after the deadline: the item
      is expired right away.
- [ ] Reboot the PC with an item waiting: no error; the item is marked expired.
- [ ] **Clear from Win + V** removes every Pasta item from Win+V and nothing else.
- [ ] Mac expiry 1 min: a received item leaves the panel (and the pasteboard, if still
      current) after a minute; local copies stay until the retention limit.
- [ ] **Expire all now** removes every received item on that device.

## Privacy (#36)

- [ ] Copy from 1Password / Apple Passwords / Proton Pass: not sent, not in the panel.
- [ ] Turn off "Skip passwords…": those copies sync (turn it back on).
- [ ] Exclude an app (picker and typed id): its copies are neither sent nor stored.

## Mac history and panel (#37–#40)

- [ ] ⇧⌘V opens the strip at the bottom of the screen under the pointer, over a
      full-screen app too, without the app in front losing focus.
- [ ] ←/→ move, ↩ pastes into the app in front, ⇧↩ pastes plain text, esc closes,
      clicking elsewhere closes, ⌘F and typing search, ⌫ deletes the selected card.
- [ ] Filters: Text, Links (link card with host), Code (monospace), From other devices.
- [ ] Rich text from Pages/Safari shows bold/lists; a copied web page with images or
      scripts shows text only (no network requests in the console).
- [ ] RTF-only items (TextEdit) show formatted.
- [ ] Without Accessibility: ↩ copies and the hint says to allow Accessibility.
- [ ] `strings ~/Library/Application\ Support/com.kingdomworks.pasta/pasta.db`
      shows no copied text.
- [ ] Retention: lower "Keep up to" to 100: older items go.

## iCloud bridge (#41)

- [ ] Off (default): an iPhone copy shows in the panel labelled "iPhone · iCloud" but
      doesn't reach the PC; PC copies don't reach the iPhone.
- [ ] On: the iPhone copy reaches the PC; a PC copy can be pasted on the iPhone; no
      ping-pong (each item once).

## Tray (#42)

- [ ] macOS popover under the menu-bar icon; Windows flyout above the taskbar icon; both
      close when clicking elsewhere.
- [ ] Status, mode, recent / received items with countdowns, Open Clipboard (macOS),
      Pause 1h, Clear from Win + V (Windows), Add Device, Settings, Quit.
- [ ] Right click shows the fallback menu.

## Setup and adding devices (#43–#46)

- [ ] First start without a chain opens setup. Tailscale stopped / logged out / not
      installed each show the right fix-it text; starting Tailscale updates the step.
- [ ] Create a chain on the Mac; join on the PC with the words; approve it in Add Device
      ("Looking for new devices…" turns into the request).
- [ ] Add Device: words behind a confirmation, QR code, **Copy words** (doesn't show in
      Win+V / the panel and doesn't sync).
- [ ] macOS: Accessibility detected live after granting it; shortcut note; bridge opt-in.
- [ ] Windows: clipboard history off → "Turn it on…" opens Settings › Clipboard; cloud
      sync on → warning; firewall link opens Windows Security.
- [ ] Settings › About › Run setup again.

## Startup, updates, shortcuts (#47, #48)

- [ ] Open at login on/off survives a log-out (LaunchAgent / Run key).
- [ ] A build without `PASTA_UPDATER_PUBKEY` says updates aren't set up.
- [ ] Click the shortcut (Settings › Shortcuts, and setup on macOS) and press a new one;
      Esc cancels; ⌘Space is refused; ⇧⌘V shows the Paste and Match
      Style note; Reset goes back to ⇧⌘V. The old shortcut stops working.
- [ ] Windows/KDE: the Shortcuts page shows the placeholder.
