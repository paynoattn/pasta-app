# 0012. Device settings travel encrypted, signed and last-writer-wins

- Status: Accepted
- Date: 2026-11-04
- Issues: #31, #32, #33
- Updates: the `config_update` type ADR 0006 and ADR 0007 reserved

## Context

Every device has its own mode (send / receive / both / paused) and expiry, and the
Devices page lets the user change them for any device from any other (#30, #31). Changes
must reach devices that are offline, every device should show every peer's current
settings, and two devices changing the same device at once must end up agreeing. ADR 0006
reserved a plaintext `config_update` envelope type for this.

## Decision

- Two new message types travel inside `encrypted` (protocol v2, no version bump: an older
  v2 peer fails to parse them, logs "dropped an encrypted message" and keeps the
  connection). The plaintext `config_update` envelope type is no longer reserved.
  - `settings`: a device's own `{mode, ttl_secs, paused_until, version}`, sent to every
    member when it connects and whenever it changes. Receivers keep the newest version
    per device and persist it.
  - `config_update`: `{target, mode, ttl_secs, version, signature}` from the member that
    made the change. The receiver accepts it only from an approved member, only for
    itself, only if `version.by` is the sender, and only if `signature` verifies with that
    member's Ed25519 key over `pasta/v2/config`, the chain id and every field. Updates are
    never relayed.
- `version` is `(counter, unix ms, device id)`, compared in that order. A device's own
  change and a remote change both bump the counter past the newest version known; ties go
  to the later clock, then the higher device id. A device applies an update only if it is
  newer than its current version, so every device converges on the same last writer.
- The device that made a change keeps it in `pending_updates` (in the encrypted store)
  until the target announces a version at least as new, and sends it again each time the
  target connects. It is re-sealed on every send, so the replay window (5 minutes) never
  rejects a change that waited for days.
- Mode enforcement is on both ends (#32): the sender skips peers whose announced mode
  doesn't receive, and the receiver drops items when its own mode doesn't. "Pause for 1
  hour" is a `paused_until` deadline next to the mode; while it runs the effective mode
  is `paused`, and it ends on its own. Choosing a mode ends it.
- The expiry setting (#33) applies to items a device receives, counted from when each
  arrived; changing it re-times the items already received.

## Consequences

- An approved member can change any member's mode and expiry. That's intended: they are
  the same user's devices. It can't add members or read anything it couldn't before.
- A removed device's queued change is dropped when it stops being a member.
- Devices on an M2 build ignore the new messages, so mixed M2/M3 chains still sync items;
  only settings stay local until every device runs M3.
