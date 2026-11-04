# Pasta security model

What Pasta protects, from whom, how, and where the protection stops. Decisions are in
[ADR 0003](adr/0003-transport-and-encryption.md), [ADR 0007](adr/0007-sync-chain-and-encryption.md),
[ADR 0008](adr/0008-secrets-and-local-storage.md) and [ADR 0009](adr/0009-no-tls-over-tailscale.md).
The [checklist](#review-checklist) at the end is run before closing a security milestone and
before the repo goes public.

## Assets

| Asset                    | Where it lives                                                                                      |
| ------------------------ | --------------------------------------------------------------------------------------------------- |
| Clipboard text           | The OS clipboard and its history; in flight between devices; briefly in Pasta's memory; Mac history |
| Chain secret (the words) | OS keychain entry; shown once on screen when creating or adding a device                            |
| Device signing key       | OS keychain entry; a locked memory page while Pasta runs                                            |
| Database key             | OS keychain entry; a locked memory page while Pasta runs                                            |
| Settings, peers, members | `pasta.db`, every value sealed with the database key                                                |
| Metadata                 | Device names, OS versions, device ids, Tailscale node ids: handshake, logs, `pasta.db`              |

## Adversaries

1. **Other people on the tailnet.** Tailnets can be shared; a tailnet node is not proof of
   ownership. They can run their own Pasta and connect to ours.
2. **Someone who learned the 24 words** (shoulder-surfing, a photo, a note) but has no
   approved device.
3. **A network attacker who can see or change traffic** between devices, as if WireGuard
   weren't there.
4. **A compromised or stolen device in the chain.**
5. **Local malware** running as the same user on a device with Pasta.
6. **Someone with the disk** of a powered-off or lost device.

## Mitigations

| Adversary                    | What stops them                                                                                                                                                                                                                                                                                                                                                  |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1. Others on the tailnet     | Pasta listens on tailnet addresses only. Every connection is identified with Tailscale WhoIs and bound to its node. Without a valid chain proof the connection is refused (4006). They never see items, member lists or the chain id.                                                                                                                            |
| 2. Words, no approval        | Knowing the words makes a device _pending_, nothing more: members send it nothing, take nothing from it and store nothing about it until a member approves it on screen, comparing its name, Tailscale device and 64-bit key fingerprint. The joining device takes the first approval it gets (ADR 0010; see the limit below). Decline refuses it until restart. |
| 3. Network attacker          | Items and member lists are XChaCha20-Poly1305 under the chain key, bound to the envelope header; tampering is detected, replays outside 5 minutes or seen before are dropped. The handshake is signed by both device keys over both nonces, so it can't be replayed or answered by an impostor.                                                                  |
| 4. Compromised / lost device | Remove it from any other member: everyone stops sending to it and ignores it; it is told and forgets the chain. Approvals and removals it signs afterwards don't count. A device can only use the key it was approved with, from the node it was approved on.                                                                                                    |
| 5. Local malware             | Keys only in the OS keychain and in locked, wiped pages. Clipboard text wiped from Pasta's memory when dropped; the Mac history sealed in memory (Windows: `CryptProtectMemory`). No core dumps; on Linux not traceable by same-user processes. Windows crash reports leave out the heap.                                                                        |
| 6. Disk access               | `pasta.db` values are encrypted; its key is in the OS keychain, which is itself encrypted with the user's login. Logs hold ids, sizes and keyed hashes, never text.                                                                                                                                                                                              |

## Known limits

Stated plainly so nobody relies on Pasta for more than it does.

- **The OS clipboard is readable by every app.** Once an item is on a device's clipboard
  (that's the point), any app running there can read it, and Windows' Win+V and KDE's
  Klipper keep their own copies. Pasta skips items marked as passwords (ADR 0004, 0005).
- **Same-user malware on macOS** can still read Pasta's memory with enough privilege, and
  on any OS can ask the keychain for Pasta's entry where the OS allows it. Memory
  protection raises the bar; it isn't a sandbox. On Windows the key behind
  `CryptProtectMemory` stays in the OS, but code injected into Pasta can call it.
- **Words plus capture.** Anyone with the words can decrypt traffic they can capture. On a
  tailnet that also means breaking WireGuard. There is no application-layer forward
  secrecy (ADR 0007).
- **No chain-key rotation yet.** A removed device still knows the words. It receives
  nothing, because nobody sends to it and its connections get nothing, but if it could
  also capture other devices' traffic (adversary 3), it could read it. Rotating the key
  after a removal needs a key per device to hand the new secret out; deferred past M2.
  Until then, if a device was stolen, create a new chain and add the others again.
- **Approval is a comparison, on one side.** The approving member compares the joining
  device's 64-bit key fingerprint by eye. The joining device trusts the first approval it
  gets (ADR 0010), so someone else with the words who approves it first captures it: it
  syncs with their chain until the user leaves and joins again. The words are the root
  secret; protect them.
- **A misled member.** A removed device that kept its key can sign events dated to when
  it was a member. Once a device knows of the removal it drops anything new from the
  removed device, so this only works on a member that hears from the removed device
  _before_ hearing of the removal. Such a member can be made to drop the other members
  (and then also drops their lists, which would correct it) or to list an accomplice;
  remove it and add it again. Rotating keys after a removal would close this; until then,
  after removing a stolen device, create a new chain.
- **Approvals the remover never saw.** If a member approves a device and is removed by
  someone who hadn't heard of that approval yet, the approval no longer counts there: the
  device shows as waiting to join again and needs approving once more.
- **Clock skew.** Devices more than 5 minutes apart drop each other's items (the log
  says so) rather than accept stale ones.
- **Metadata in the clear inside WireGuard.** `hello` carries the device name, OS and
  app version; heartbeats and acks carry ids. ADR 0009 explains why there's no TLS.
- **Copies we can't reach.** Strings the OS, the WebView or the allocator copied (e.g. on
  reallocation) aren't wiped; the words shown in the developer panel live in the WebView.
- **Restart forgets replay state.** The seen-message cache is in memory; replays across a
  restart are still bounded by the 5-minute window and the engine's item-id dedupe.

## Review checklist

Run before closing a security milestone and before making the repo public. Record the
date and result in the PR.

- [ ] `pnpm check` is green on all three OSes in CI.
- [ ] `cargo test -p pasta-store -- --ignored keyring` passes on macOS (in the GUI
      session), Windows and Linux (with a Secret Service running).
- [ ] No plaintext item, member list or chain id on the wire: `crates/pasta-net/tests`
      wire-capture test passes; frames on a real tailnet show only `encrypted` bodies.
- [ ] A device with other words, or none, never connects; a device with the right words
      receives nothing until approved; a removed device receives nothing again
      (`crates/pasta-net/tests/chain.rs` and the manual plan `docs/testing/manual-m2.md`).
- [ ] Logs on every device contain no clipboard text, words or keys: search the log
      folders for a distinctive phrase copied during testing.
- [ ] `pasta.db` contains no readable settings, names or node ids (`strings pasta.db`).
- [ ] Diagnostics ("Copy diagnostics") show no names, addresses, node ids or chain id.
- [ ] Every `unsafe` block has a `SAFETY:` comment and is in `pasta-crypto` (memory,
      hardening) or the clipboard adapters.
- [ ] No secrets, words, real hostnames, tailnet names, IPs or logins in the repo history
      (`git log -p | grep` for the tailnet suffix and known names).
- [ ] Crypto dependencies are on one RustCrypto generation and up to date; `cargo tree -d`
      shows no duplicate `digest`/`sha2` in the crypto path.
- [ ] Known limits above still match the code; new ones added.
