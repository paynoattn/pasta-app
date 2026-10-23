# 0003. WebSocket over the tailnet + sync-chain end-to-end encryption

- Status: Accepted
- Date: 2026-10-22
- Issues: #9, #11, #20, #21, #22, #26

## Context

Devices must exchange small text payloads with low latency, only with the user's own
devices. Tailscale already encrypts traffic between nodes with WireGuard. A tailnet can
also contain other people's devices and tagged servers (observed on the maintainer's
tailnet), so tailnet membership is not proof of ownership.

## Decision

1. **Transport:** a WebSocket server and client in every app, listening **only** on this
   node's Tailscale addresses (never `0.0.0.0`), on a fixed default port. Mesh, no server.
2. **Peer check:** every inbound connection is identified with Tailscale WhoIs (ADR 0002)
   and must map to the same Tailscale node it claimed before.
3. **Sync chain:** a 24-word phrase (BIP39-style, 256-bit) is the shared secret. Keys
   for payload encryption and a chain id are derived from it (HKDF). Devices join by
   typing the words or scanning a QR code.
4. **End-to-end encryption:** every `clip` / `config_update` body is sealed with an AEAD
   (XChaCha20-Poly1305 planned) under the chain key, with replay protection.
5. **Approval:** knowing the words is necessary but not enough. A new device is
   _pending_ until an existing member approves it once; the signed member list then
   syncs to every device.
6. **TLS (`tailscale cert`) is not required.** Whether to offer it as an extra layer is
   evaluated in #26.
7. **Live only for v1:** items are not stored for offline devices (#62 later).

## Consequences

- The M1 MVP runs on WireGuard alone; E2E encryption arrives in M2. Treat M1 builds as
  test builds.
- Losing the phrase means re-creating the chain; leaking it means rotating it.
- No dependency on MagicDNS or HTTPS being enabled on the tailnet.
