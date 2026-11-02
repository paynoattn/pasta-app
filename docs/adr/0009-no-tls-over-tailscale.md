# 0009. No TLS (`tailscale cert`) on top of WireGuard and E2E

- Status: Accepted
- Date: 2026-11-01
- Issue: #26

## Context

ADR 0003 left open whether peers should also speak `wss://` with certificates from
`tailscale cert`. With M2, every connection already has three layers: WireGuard between
nodes (Tailscale), WhoIs plus a signed handshake binding each connection to a device key
and a Tailscale node, and end-to-end XChaCha20-Poly1305 for every item and member list
(ADR 0007). The question is whether TLS adds meaningful protection, and at what cost.

## What TLS would add

- **Confidentiality of the handshake and heartbeats** (device names, OS versions, ids)
  from someone who can read tailnet traffic. On a tailnet that means breaking WireGuard,
  which already protects them.
- **Forward secrecy for payloads** if the chain key leaks _and_ old traffic was recorded.
  Recording it again means getting inside WireGuard, which has forward secrecy itself.
- **Server authentication by certificate.** Already covered better: WhoIs identifies the
  node, and the signed handshake proves the device key, in both directions (TLS would
  authenticate only the server side).

## What it would cost

- **The tailnet admin must enable MagicDNS and HTTPS certificates.** On a shared tailnet
  that's not the user's call, and it's tailnet-wide.
- **Names become public.** Certificates are issued by Let's Encrypt for
  `<machine>.<tailnet>.ts.net`, and every one is published in Certificate Transparency
  logs, which reveals machine names and the tailnet name to anyone. That is a privacy cost
  this project has explicitly avoided (no real hostnames in the repo).
- **Renewal and failure modes:** certificates last 90 days; each device must renew
  (`tailscale cert` or the LocalAPI) and reload. An expired certificate, a Let's Encrypt
  outage or a rate limit would break sync for reasons the user can't see.
- **Dialing by name instead of IP:** certificates name the MagicDNS host, so discovery and
  dialing would depend on MagicDNS working on every OS.
- **More code and dependencies** (rustls, certificate reload) in the most sensitive path.

## Decision

Don't adopt TLS, not even as an option. Keep `ws://` over the tailnet addresses only,
with app-layer E2E. If forward secrecy at the application layer becomes a goal, add
ephemeral X25519 keys to the signed handshake (ADR 0007) rather than TLS: it needs no
certificates, no MagicDNS and leaks no names.

## Consequences

- No dependency on MagicDNS, HTTPS certificates or Let's Encrypt; nothing published to CT
  logs.
- Metadata in the handshake relies on WireGuard alone, as recorded in
  `docs/SECURITY-MODEL.md`.
- Not tested against a live tailnet: requesting a certificate would publish this
  tailnet's machine names in public CT logs, which is exactly the cost above. The analysis
  rests on Tailscale's documentation of `tailscale cert`.
