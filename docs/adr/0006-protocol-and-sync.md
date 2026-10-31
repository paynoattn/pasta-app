# 0006. Wire protocol v1, connection rules and echo prevention

- Status: Proposed
- Date: 2026-10-29
- Issues: #7, #9, #10, #11, #13, #16

## Context

M1 needs a protocol two clients can keep speaking as encryption (M2) and remote config
(M3) arrive, a way for a mesh of peers to end up with one connection per pair, and a rule
that stops an item from bouncing between devices. ADR 0004 and ADR 0005 showed that the
own-write markers on both OSes get lost (re-selecting an item in Win+V, a round trip
through Universal Clipboard), so the marker can't be the only echo guard.

## Decision

**Protocol** (`pasta_core::protocol`)

- One JSON envelope per WebSocket text frame:
  `{ v, type, msg_id, from_device, sent_at, body }`.
- Types: `hello`, `hello_ack`, `clip`, `ack`, `ping`, `pong`. `config_update`,
  `device_approval` and `encrypted` are reserved and, like any unknown type, decode to an
  ignorable error, so a newer peer's extra messages don't drop the connection.
- `v` is read before anything else. Another major version gets close code 4001.
- A `clip` carries `text/plain` (required) plus optional `text/html` and `text/rtf`, the
  origin device and app, and `content_hash`: SHA-256 of the plain text with `\r\n`
  normalised to `\n`. Rich flavors aren't hashed because Win+V rewraps HTML and drops RTF,
  and Universal Clipboard strips both.
- Size limit 1 MiB per item (configurable). An item that's too big drops RTF, then HTML,
  before being refused. The receiver re-checks size and hash.
- Decode errors never quote input: serde's messages can contain field values.

**Connections** (`pasta_net::network`)

- Listen only on this node's tailnet addresses (loopback allowed for tests), default port 47820. Rebind when the addresses or the port change; stop listening when Tailscale stops.
- Every connection, inbound and outbound, is identified with WhoIs before the handshake.
  A device id that connected from one Tailscale node is refused from any other (close code
  4002). The binding is persisted with the known peers.
- Both sides dial. If two connections exist for a pair, both keep the one opened by the
  lower device id; two from the same side means a reconnect, and the newer one wins.
- `ping` every 10 s; no traffic for 35 s closes the connection. Failed dials back off
  exponentially (1 s to 60 s, ±50% jitter); after 5 failures a node is taken to have no
  Pasta and is retried every 10 minutes, or as soon as it goes offline and comes back.
- Tailscale's CLI is located lazily, so installing Tailscale while Pasta runs works.

**Echo prevention** (`pasta_core::engine`)

- The engine tracks the hash of the item it believes is current, whether it was copied
  here or applied from a peer. A local copy with that hash is never broadcast. This needs
  no time window: Pasta's own write, an app writing the same text twice, and re-selecting
  an applied item in Win+V all hit it. Copying an older item after something else became
  current is a real copy and is sent.
- Items are never relayed (v1 is a full mesh), an item from this device is ignored, and
  the last 1024 received item ids are remembered to drop duplicates.
- Copies closer than 150 ms are sent once, as the last of the burst. A pending copy is
  settled before a received item is applied, so both sides end on the same item.
- Adapters also skip their own writes (Windows: clipboard owner is Pasta's process;
  macOS: `changeCount` after the write plus the `dev.pasta.origin` marker), which saves a
  round through the engine.

**macOS polling** (amends ADR 0005)

- The watcher thread sleeps between polls. ADR 0005 spun `NSRunLoop` for 250 ms, which
  worked on the probe's main thread but on a background thread with no run-loop sources
  returns at once: in the app it became a busy loop at ~99% CPU and missed every change
  after the first. With `thread::sleep` it uses ~0.2% CPU. The app's main thread keeps
  `NSWorkspace.frontmostApplication` current.

## Consequences

- Measured on a Mac and a Windows PC on the same tailnet: peers found and connected within
  ~25 ms of starting, items applied about a second after copying, and 20 copies 400 ms
  apart in each direction arrived once each with nothing echoed.
- Hashes in the logs are 12 hex digits of an unkeyed SHA-256, which a determined reader
  could brute-force for very short secrets. Revisit with a keyed hash when the chain key
  exists (#21).
- Both peers dialing at once causes one brief disconnect/reconnect on the side whose
  connection loses the tie-break. The UI should treat a disconnect shorter than a second
  as noise.
- The Windows firewall wasn't in the way on the test machine (the Mac reached port 47820
  on the Windows PC). The rule an installer should add is in `docs/development.md` (#57).
