# Manual test plan: M2 trust & encryption

Run on real hardware before closing M2 (#20–#27), next to the automated tests in
`crates/pasta-net/tests/` (`chain.rs`, `wire.rs`, `sync.rs`, `no_content_in_logs.rs`).
Use at least two machines on one tailnet; copy only throwaway text.

The app's developer panel (tray › **Sync chain…**) drives the chain. Where a second app
can't run (e.g. no Linux clipboard adapter before M4, or a dev build already on the port),
`cargo run -p pasta-net --example chain_peer` stands in for a device (see
[`development.md`](../development.md#testing-across-machines)).

## Keys and storage (#23, #25)

- [ ] First start of an M2 build on an M1 install: the log says it created device keys in
      the OS keychain for the **same device id** as before and moved `config.json` into the
      encrypted store; `config.json` is gone; `pasta.db` exists and `strings pasta.db` shows
      no device names or node ids.
- [ ] Restart: the same device key (log line `starting sync … key=…`), chain and members.
- [ ] macOS over SSH (keychain unavailable): the tray says "Waiting for the keychain"; it
      starts by itself once the keychain is available, or via **Try the keychain again**.
- [ ] `cargo test -p pasta-store -- --ignored keyring` passes on each OS (macOS from the
      GUI session).

## Chain and approval (#20, #22)

- [ ] Without a chain, the tray says "Not in a sync chain" and nothing is sent or received.
- [ ] Create a chain on device A: the panel shows 24 words and a fingerprint.
- [ ] Join on device B with the words (try a typo: the panel names the word's position).
      B says "Waiting for approval"; A lists B under "Waiting to join" with its Tailscale
      name and the key fingerprint B shows.
- [ ] Before approval, copies on either side don't reach the other.
- [ ] Approve on A: B shows "Approved by" A with A's key; nothing syncs until you accept
      it on B after comparing the key with A's panel. Then both directions sync within ~1 s.
- [ ] A third device C joins and is approved on **B** (and accepts): A learns it without
      doing anything.
- [ ] A device with **other** words (or none) never appears and gets nothing.
- [ ] Remove C on A: C's panel goes back to "not in a chain"; nothing reaches C any more,
      from A or B.
- [ ] Leave on B: A shows B gone; B gets nothing.
- [ ] Removed device joins again with the words: shown as "removed before"; can be
      approved again.

## Encryption and hardening (#21, #24)

- [ ] Search every device's log folder for a phrase copied during the test: not found.
- [ ] Linux: `gdb -p $(pidof pasta-app)` as the same user fails to attach; log line
      `process hardening applied=[…PR_SET_DUMPABLE=0…]`.
- [ ] Windows: log line `process hardening applied=["crash reports exclude the heap…"]`.

## Results

| Date       | Build     | Devices                                                            | Result                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| ---------- | --------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 2026-11-01 | 0.1.0 dev | Windows 11 app; macOS 27.2 and CachyOS `chain_peer` over Tailscale | Partial, run over SSH: M1→M2 migration kept the device id; keychain on all three OSes (macOS from the GUI session; over SSH it reports unavailable as designed); create on Windows; Mac pending got nothing either way, then synced both ways after approval (SHA-256 matched); Linux approved by the Mac peer, Windows learned it by gossip; removal from Windows cut Linux off (nothing after, it left by itself); a device with other words never connected; leave and restart persisted; no copied text in any log. Not covered: Mac app UI (an M1 dev build held the port), the panel's typo message by hand. |
| 2026-11-02 | 0.1.0 dev | Windows 11 app; CachyOS `chain_peer` over Tailscale                | After the review fixes: approving with a wrong key fingerprint is refused; the joiner showed the approval with the Windows app's own key and received nothing (a copy made meanwhile never arrived) until it accepted; then both directions synced (SHA-256 matched).                                                                                                                                                                                                                                                                                                                                              |
