# 0014. App identifier `com.kingdomworks.pasta`

- Status: Accepted
- Date: 2026-11-11
- Issues: #65

## Context

Pasta's bundle identifier was `com.cpawlukiewicz.pasta-app`, under a personal name. The
identifier is what macOS and the App Store know the app by (bundle id, App ID, signing,
Accessibility and keychain grants), and it also names the data and log folders and the
keychain entry (ADR 0008). It can't change after the first App Store release without
shipping a new app, so it has to be settled before M5.

## Decision

- The identifier is `com.kingdomworks.pasta`, in `src-tauri/tauri.conf.json`. Tauri derives
  the data folder (`pasta.db`) and log folder from it.
- The keychain entry's service is the same string (`pasta_store::secrets::SERVICE`). This
  supersedes the service name in ADR 0008; the rest of 0008 stands.
- The macOS dev runner signs dev builds with the same identifier.
- No migration from the old identifier: no build has been released, so only development
  machines have data under the old names.

## Consequences

- A machine that ran an earlier build starts fresh: no chain, default settings, empty Mac
  history, and new permission grants (Accessibility, keychain "Always Allow"). Leave the
  chain on it first, or remove it from another device afterwards, and delete the old
  `com.cpawlukiewicz.pasta-app` folders and keychain entry by hand.
- A new device id per machine: peers see a new device that must join and be approved.
