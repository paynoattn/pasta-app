# 0011. Search the Mac history by decrypting, not with a plaintext index

- Status: Accepted
- Date: 2026-11-04
- Issues: #37, #25

## Context

#37 asks for SQLite FTS5 full-text search over Pasta's macOS clipboard history, and for
the history to be encrypted at rest (#25, ADR 0008). The two pull against each other:
an FTS5 index is built from the plaintext and lives in the same file, so it would put
every copied item (passwords that slipped through, tokens, private messages) on disk in
the clear, which is exactly what ADR 0008 rules out. Indexing the sealed values is
useless, and SQLCipher would add a native dependency and a storage migration for a store
that holds at most a few thousand short items.

## Decision

- The history is a `history` table in the existing encrypted `pasta.db` (migration 2).
  Only `item_id`, `created_at` and `expires_at` are plain columns, for ordering and
  pruning. Text, flavors, source app, origin device and content hash are one sealed JSON
  value (XChaCha20-Poly1305 under the database key, with the item id as associated data).
- Search decrypts rows newest first, one at a time, matches case-insensitively in memory
  against the text and the source app's name, applies the panel's filter (All / Text /
  Links / Code / From other devices) and stops at the result limit. Every decrypted copy,
  including the lowercase copy used for matching, is wiped when dropped (#24).
- There is no FTS5 index, on disk or in memory: a long-lived in-memory index would keep
  every item's plaintext outside the protected-memory rules of ADR 0008.
- Retention: received items expire with the device's expiry setting (#33); items copied
  on this Mac follow "keep for N days"; everything past "keep up to N items" goes. The
  same text copied twice in a row moves the existing item up instead of adding another.

## Consequences

- Nothing about the user's history is readable from `pasta.db` without the keychain key;
  a test checks the file for a canary string.
- Search cost grows linearly with the history. At the default 500 items (max 10,000) a
  scan is a few milliseconds; if histories grow much larger, revisit with SQLCipher or an
  encrypted index rather than a plaintext one.
- No ranking: results are newest first, which is what the panel shows anyway.
