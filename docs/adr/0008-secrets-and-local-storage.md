# 0008. Secrets in the OS keychain, an encrypted local store, protected memory

- Status: Accepted
- Date: 2026-11-01
- Issues: #23, #24, #25

## Context

M2 adds secrets (the chain secret, each device's signing key) and asks that everything
Pasta keeps at rest is encrypted, and that what it holds in memory is hard for other
processes to scrape. M1 kept settings and known peers in a plain `config.json`.

## Decision

**OS secret storage (#23)** (`pasta_store::secrets`)

- One keychain entry per OS user (service `com.cpawlukiewicz.pasta-app`, account
  `device-secrets`) holds a small binary blob: device id, Ed25519 seed, database key and,
  in a chain, the chain secret. One entry means one keychain prompt, not three, when an
  unsigned dev build changes.
- Backends via `keyring-core` and the native stores the `keyring` crate's v1 mode uses:
  macOS Keychain, Windows Credential Manager, Secret Service on Linux (KWallet's
  `ksecretd` or GNOME Keyring, via zbus, pure Rust). The store is built fresh for each
  attempt rather than through `keyring`'s process-wide default, which caches the first
  result and so could never recover from a keychain that was locked at login.
- A locked, missing or refusing keychain is a retryable state: the tray says "Waiting for
  the keychain" with a "Try the keychain again" item, and Pasta retries every 30 s. Over SSH
  the macOS login keychain answers "User interaction is not allowed", which lands here too.
- The device id lives with its key. An M1 install keeps its device id (read from
  `config.json` on the first M2 start), so peers' node bindings still match.
- If the entry is damaged, Pasta starts a new identity. If the database can't be decrypted
  (its key was lost), it's moved aside as `pasta.db.unreadable-<ms>` and a new one created.

**Encrypted-at-rest store (#25)** (`pasta_store::db`)

- SQLite (`rusqlite`, bundled, no OpenSSL) with **app-level AEAD per value**: each value is
  JSON sealed with XChaCha20-Poly1305 under the database key, with the row name as
  associated data, so rows can't be swapped. Table and row names and value sizes are
  visible on disk; contents aren't. Chosen over SQLCipher, whose Windows build needs a
  vendored OpenSSL (full Perl, nmake) on all three CI runners.
- `PRAGMA secure_delete` is on. A sealed `check` row detects a wrong key before anything
  else is read; only a check row that is missing or doesn't decrypt counts as a wrong key,
  a read error (busy, I/O) is reported as such and nothing is moved. A member list that
  can't be read stops startup rather than putting the device back to waiting for
  approval. A keychain entry from a newer Pasta is left alone, not overwritten.
- Migrations are numbered SQL scripts applied in order in a transaction and tracked in
  `PRAGMA user_version`; a database from a newer build is refused rather than downgraded.
- Rows today: `config` (settings, known peers) and `chain` (root and member list). The Mac
  history store (#37) will add tables the same way.
- On the first M2 start, `config.json` is imported and then deleted. Settings are no longer
  hand-edited (the settings UI is M3); `PASTA_LOG` still overrides the log level, which is
  applied once the store is open.

**Protected memory (#24)** (`pasta_crypto::secret`, `vault`, `hardening`)

- Keys live in `Secret32`: one OS page each, locked in RAM (`mlock` / `VirtualLock`) so
  they never reach swap or the pagefile, wiped before release, compared in constant time,
  redacted in `Debug`. Signing, AEAD and HMAC keys are rebuilt from them for each use and
  wiped by their crates on drop (`zeroize` features); `hkdf` has no such feature, so its
  short-lived state while deriving chain keys is not wiped. A page of its own per secret means
  unlocking one never unlocks another. We use `zeroize` and this type rather than
  `secrecy`, which adds no locking.
- Clipboard text: `Flavors` wipes its strings on drop, and plaintext buffers around
  encryption and the store are `Zeroizing`. The Mac history keeps items sealed in memory:
  `CryptProtectMemory(SAME_PROCESS)` on Windows (key held by the OS), XChaCha20-Poly1305
  under a per-run session key in a locked page on macOS and Linux.
- Process hardening at startup: Linux `PR_SET_DUMPABLE=0` (no core dumps, no same-user
  ptrace or `/proc/<pid>/mem`) and `RLIMIT_CORE=0`; macOS `RLIMIT_CORE=0`; Windows crash
  reports exclude the heap (`WerSetFlags(NOHEAP)`). Debug builds skip it with
  `PASTA_DEBUGGABLE=1` so a debugger can attach.

## Consequences

- Release builds keep `panic = "abort"`, so wiping on unwind doesn't apply; hardening
  keeps a crash from writing memory out.
- The limits (the OS clipboard is readable by every app, copies made by the OS or the
  WebView, same-user malware on macOS) are spelled out in `docs/SECURITY-MODEL.md`.
- Tests use an in-memory secret store; the real stores are covered by an ignored test run
  by hand on each OS.
