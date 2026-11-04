# 0010. A joining device trusts the first approval it gets

- Status: Accepted
- Date: 2026-11-02
- Issues: #22
- Supersedes: the joining device's confirmation step in ADR 0007 (Membership)

## Context

ADR 0007 asked for two confirmations to add a device: a member approves the joining
device after comparing its key fingerprint, and then the joining device accepts that
approval after comparing the approver's fingerprint. The second step was there so that
someone else who knows the words couldn't capture a new device by approving it first.

In practice, adding a device meant copying 24 words from one device to the other and then
comparing keys and confirming on both screens. Typing the words in is already a deliberate
act by the person who owns both devices, and the words are the chain's root secret.

## Decision

- Knowing the words is the joining side's approval. A waiting device adopts the first
  member list that has a single root and makes it a member under its own key, and pins
  that root; lists with another root are ignored from then on (unchanged from ADR 0007).
- The member side is unchanged: a device with the words stays _pending_ (nothing sent,
  taken or stored) until a member approves it, bound to the key fingerprint compared on
  screen.
- The offer list, the `accept` command, `accept_approval` and the "Approved by" UI are
  removed. The rate limit on member lists from non-members stays.

## Consequences

- Adding a device is: copy the words, approve it on a member. Syncing starts on its own.
- Someone else who has the words and is on the tailnet when a device joins can approve it
  first and capture it: it then syncs with their chain, not the real one, until the user
  notices (the real member keeps listing it as waiting to join) and leaves and joins
  again. This needs the words, which already let them read traffic they can capture; keep
  the words private. `the_first_approval_wins_and_later_roots_are_ignored` pins it.
- The member-side approval may go too (the setup flow, #46); that would need its own
  ADR, since it would let anyone with the words receive items.
