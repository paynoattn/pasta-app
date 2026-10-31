# Manual test plan: M1 Mac ↔ Windows MVP

Run once on real hardware before closing M1 (#19). Use a Mac and a Windows PC logged in
to the same tailnet, both running the same Pasta build. Copy only throwaway text: until
M2, items are protected by WireGuard alone.

Watch the logs while testing (paths in [`development.md`](../development.md#running-the-mvp)).
They show `sent local copy` / `applied item from peer` with item ids and sizes.

## Setup

- [ ] Windows: clipboard history is on (Settings › System › Clipboard). Turn it off once
      and check the tray shows "Clipboard history (Win+V) is off", then turn it back on.
- [ ] Both apps start with no window and a tray / menu-bar item; no Dock icon on the Mac.
- [ ] Each tray lists the other device within a few seconds, with no IP entered.
- [ ] Launching Pasta a second time on either machine doesn't start a second copy.
- [ ] macOS: Activity Monitor shows Pasta idle at well under 1% CPU.

## Sync

- [ ] Copy plain text on the Mac → it's at the top of Win+V on the PC within ~1 s.
- [ ] Copy plain text on the PC → ⌘V pastes it on the Mac within ~1 s.
- [ ] Copy formatted text (bold, a link) from Safari or Notes → pasting into Word or
      Outlook on the PC keeps the formatting.
- [ ] Copy formatted text from Word or Edge on the PC → pasting into Notes or Pages keeps
      the formatting.
- [ ] 20 copies in a row on each side, ~0.5 s apart: each arrives exactly once and nothing
      comes back (the sender's log has no `applied item from peer` for its own items).
- [ ] Re-select an item that came from the Mac in Win+V and paste it: the Mac doesn't
      receive it again as a new item unless something else was copied in between.
- [ ] Copy the same text twice on one side: it's sent once.

## Sensitive items

- [ ] Copy a password from 1Password, Bitwarden or KeePassXC on the PC: nothing is sent.
- [ ] Copy a password from Apple Passwords and from the Proton Pass app on the Mac:
      nothing is sent.
- [ ] Copy a password from the Proton Pass extension in a Chromium browser on the Mac:
      nothing is sent.

## Connection resilience

- [ ] Quit Pasta on one side (tray › Quit): the other side shows it offline. Start it
      again: both reconnect and sync without restarting the other.
- [ ] Sleep the Mac for a few minutes and wake it: sync works again within ~30 s.
- [ ] Sleep the PC and wake it: same.
- [ ] `tailscale down` then `tailscale up` on one side: the tray says Tailscale is
      disconnected, then reconnects and syncs.
- [ ] Stop the Tailscale service on Windows: the tray says Tailscale isn't running; start
      it again and sync resumes.

## Diagnostics and logs

- [ ] Tray › Copy diagnostics, paste into a text editor: versions, states and counts, no
      device names, tailnet IPs or node ids. The report isn't synced to the other device.
- [ ] Search both log folders for a distinctive phrase you copied during the test: it
      isn't there.

## Results

| Date       | Build     | Mac (macOS)  | PC (Windows)    | Result                                                                                                       |
| ---------- | --------- | ------------ | --------------- | ------------------------------------------------------------------------------------------------------------ |
| 2026-10-29 | 0.1.0 dev | 27.2 (arm64) | 11 (10.0.26300) | Partial, run over SSH: discovery, both directions, 20-copy bursts each way, restart reconnect. See ADR 0006. |
