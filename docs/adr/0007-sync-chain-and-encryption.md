# 0007. Sync chain, end-to-end encryption and device approval (protocol v2)

- Status: Accepted; the joining device's confirmation step superseded by 0010
- Date: 2026-11-01
- Issues: #20, #21, #22

## Context

ADR 0003 decided the shape of M2's trust model (a 24-word chain code, AEAD payloads,
one-time approval of each device, a signed member list) without fixing the details.
ADR 0006's protocol v1 sent items in plaintext over WireGuard and synced with any Pasta on
the tailnet that passed the WhoIs node binding. This ADR fixes the M2 wire protocol and
the membership rules. It refines ADR 0003 and replaces the parts of ADR 0006 that changed
(protocol version, message types, the "sync with every Pasta on the tailnet" rule); the
connection rules and echo prevention of ADR 0006 still hold.

## Decision

**Chain code and keys** (`pasta_crypto::words`, `pasta_crypto::keys`)

- The chain secret is 256 random bits, shown as 24 BIP39 English words (8-bit SHA-256
  checksum). The words only encode the secret; no BIP39 seed or passphrase is involved.
- Typing is forgiving: case, punctuation, numbering and line breaks are ignored, and a word
  may be cut to its first four letters (unique in the BIP39 list). The checksum catches a
  wrong or swapped word; errors name the position, never the word.
- HKDF-SHA256 with salt `pasta/chain/v1` derives: the chain id (16 bytes,
  `pasta/chain-id`), the payload key (`pasta/payload-key`), the auth key
  (`pasta/auth-key`) and the log-hash key (`pasta/log-key`). The chain id is shown as an
  8-character fingerprint (`ABCD-EFGH`, Crockford base-32) that matches on every device.
  `crates/pasta-crypto/tests/vectors.json` pins the derivation (checked against an
  independent HKDF).
- Each device has an Ed25519 key. Its seed, the chain secret and the database key live in
  the OS keychain (ADR 0008).

**Protocol v2** (`pasta_core::protocol`; v1 peers are refused with close code 4001)

- `hello` and `hello_ack` add `device_key`, a fresh 32-byte `nonce` per connection, and a
  `chain_proof`: HMAC-SHA256 under the auth key over (chain id, device id, device key,
  nonce). The proof shows the sender knows the words without revealing anything that links
  connections or devices to a chain, and can't be replayed under another identity.
- Handshake: dialer `hello` → listener `hello_ack` signed with its device key → dialer
  `auth` signed with its key. Both sign the same transcript (both device ids, keys and
  nonces) with a role byte, so each side has proven it holds the key in its own `hello`,
  for this connection only. Until the `auth` checks out, the listener sends nothing but the
  `hello_ack`.
- A device with no chain neither dials nor accepts. A peer without a valid proof for this
  chain is refused with close code **4006** (`CHAIN_MISMATCH`); the dialer treats it like
  any failed dial and backs off. A bad signature, a member presenting a key other than the
  one it was approved with, or a declined device gets **4007** (`NOT_TRUSTED`).
- `clip` is no longer a top-level type; a plaintext `clip` is dropped as invalid. Items
  and member lists travel only inside `encrypted`: `{nonce, ciphertext}` with the inner
  message (`{"type":"clip"|"members","body":…}`) sealed by **XChaCha20-Poly1305** under
  the payload key, a random 192-bit nonce, and the envelope header (`v`, `msg_id`,
  `from_device`, `sent_at`) as associated data, so a body can't be moved to another
  message or sender. `config_update` (#31) will travel the same way.
- Replay protection: after decryption, a message whose `sent_at` is more than 5 minutes
  from the receiver's clock is dropped (the log says how far off the clock is), and
  `(from_device, msg_id)` pairs are remembered for twice that window. The engine's item-id
  dedupe (ADR 0006) stays as a second layer.
- Anything that fails to decrypt, validate or pass the replay check is dropped and logged
  with the peer and message id only. Hashes in log lines are keyed with the log-hash key,
  so devices in one chain can line up their logs but nobody can brute-force short items
  from them (closes the consequence noted in ADR 0006).

**Membership** (`pasta_crypto::membership`, `pasta_net::trust`)

- The member list is a set of records `{device_id, device_key, name, os, node_id,
added_at, approved_by, signature}`, each signed by its approver over the chain id and
  every field. The creator's record approves itself and is the chain's **root**.
  Revocations `{device_id, revoked_at, revoked_by, signature}` are signed by any member.
- A device trusts only events that verify back to its pinned root, which is the creator's
  device id **and key**; every record and revocation signs the chain id and the root's
  key, so it counts only in the chain it was made for. The creator pins itself. A joining device never adopts a list by itself: a list that has exactly one root
  and makes it a member under its own key becomes an **offer**, shown with the approver's
  name, Tailscale device and key fingerprint, and the user accepts it after comparing that
  key with the one the approving device shows. Knowing the words is therefore not enough
  to capture a joining device by approving it first. After that, lists with another root
  are ignored.
- The events are replayed in time order, starting from the root's own record, and each
  counts only if its signer was an active member at that moment. A device is a member if
  its latest valid approval came after its latest valid removal, so a removed device can
  be approved again later with the same identity, while an old list can't bring it back.
  Removal isn't retroactive (what a member signed while it was one stays valid), and a
  removed device can't sign anything that counts, short of backdating it to before its
  removal. Events dated more than an hour ahead of the receiver's clock are ignored, so
  nobody can date one out of reach, and a list carries at most 1024 events with names and
  ids of at most 128 bytes.
- Lists are exchanged inside `encrypted` when two members connect and whenever the list
  changes (approve, remove, leave, or news from a peer), and merged by these rules. A
  member takes a list from any device that proved the words, but drops new events signed by
  a device it already knows was removed (whatever their date: a removed device keeps its
  key and could backdate them) and any new root record. Checking a list costs signature
  checks, so lists from non-members are checked at most 4 times per 10 s per Tailscale
  node. A waiting device keeps one offer per Tailscale node (8 at most), so one node can't
  crowd out the real approver, and at most 32 non-members stay connected, 4 per node.
- A connected device with a valid chain proof that isn't a member is **pending**: members
  list it with its name, OS, Tailscale device name and a 64-bit key fingerprint, and send it
  nothing, take nothing from it and store nothing about it (no node binding). Approve is
  bound to the fingerprint the user compared, signs a record and gossips the list. Decline
  refuses it until restart. Remove signs a revocation; the removed device is sent the list
  once so it learns, then leaves the chain by itself. Leave is a self-revocation sent to
  the members before the device forgets the chain.
- A member must connect with the key it was approved with, and from the Tailscale node it
  was approved on (when the approver knew it), on top of ADR 0006's per-device node binding.

**Product decisions** (agreed 2026-11-01)

- A device that isn't in a chain syncs nothing; M1 installs create or join a chain once.
- Rotating the chain key after removing a device is deferred (see the security model).
- Until the setup flow (#46) and Devices screen (#43) land, the settings window has a small
  developer panel for create / join / approve / decline / remove / leave.

## Consequences

- A packet capture on the tailnet interface shows no clipboard text or member data even
  without WireGuard; the handshake, heartbeats and acks carry device names, OS versions and
  ids in the clear (inside WireGuard on a real tailnet).
- Knowing the words is necessary but not enough to receive anything; a member must approve.
  Words alone do let a device read traffic it can capture, which on a tailnet means breaking
  WireGuard as well (see `docs/SECURITY-MODEL.md`).
- No forward secrecy at the application layer: the payload key is static for the life of
  the chain. WireGuard provides it on the wire. Ephemeral keys in the handshake would add
  it; tracked as a later improvement, not needed for v1.
- Clocks more than 5 minutes apart stop items from syncing; the log says so.
- Every M2 device needs the chain set up once; M1 and M2 builds can't talk to each other.
