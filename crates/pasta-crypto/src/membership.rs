//! Who is in the chain (#22): signed approvals and removals, replayed in order
//! from one pinned root.
//!
//! The member list is a set of signed events: approvals ([`MemberRecord`]) and
//! removals ([`Revocation`]). Every device evaluates them the same way:
//! starting from the root's own record, events are replayed in time order, and
//! an event counts only if its signer was an active member at that moment. So
//!
//! - a device is a member if its latest valid approval came after its latest
//!   valid removal: a removed device can be approved again later, and an old
//!   list can't bring it back;
//! - removal isn't retroactive: what a member signed while it was a member
//!   stays valid;
//! - a removed device can't sign anything that counts, unless it backdates it
//!   to before its removal (the limit of keys that aren't rotated; see
//!   `docs/SECURITY-MODEL.md`);
//! - timestamps more than [`MAX_FUTURE`] ahead of the receiver's clock are
//!   refused, so nobody can date an event out of reach.
//!
//! A joining device doesn't adopt a list by itself: [`Membership::offer`]
//! turns a list that would make it a member into an offer that the user
//! accepts after comparing the approver's key (the words alone must not be
//! enough to capture a new device). A list must have exactly one root.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Duration;

use pasta_core::chain::{MemberList, MemberRecord, Revocation};
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::wire::{Bytes, Key32};

use crate::keys::{ChainId, DeviceKey, verify};

/// How far ahead of the receiver's clock an event may be dated.
pub const MAX_FUTURE: Duration = Duration::from_secs(60 * 60);
pub use pasta_core::chain::MAX_EVENTS;

/// A device asking to join, as seen by the member approving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub device_id: DeviceId,
    pub device_key: Key32,
    pub name: String,
    pub os: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MembershipError {
    #[error("this device isn't an approved member of the chain")]
    NotAMember,
    #[error("that device is already a member")]
    AlreadyMember,
    #[error("no such member")]
    UnknownMember,
    #[error("the member list is full")]
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeOutcome {
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceState {
    /// Its latest valid approval.
    record: MemberRecord,
    active: bool,
}

/// The chain's root: the creator's device id and key, pinned by every device.
pub type Root = (DeviceId, Key32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    chain_id: ChainId,
    root: Option<Root>,
    /// Valid events only, in replay order.
    records: Vec<MemberRecord>,
    revocations: Vec<Revocation>,
    state: BTreeMap<DeviceId, DeviceState>,
}

impl Membership {
    /// A new chain whose only member (and root) is this device.
    pub fn create(chain_id: ChainId, me: &DeviceKey, me_as: Candidate, now: UnixMillis) -> Self {
        let root = (me_as.device_id, me.public());
        let record = sign_record(chain_id, root.1, me, me_as.device_id, me_as, now);
        Self::evaluate(chain_id, Some(root), vec![record], Vec::new())
    }

    /// A device that knows the words and waits to be approved.
    pub fn joining(chain_id: ChainId) -> Self {
        Self { chain_id, root: None, records: Vec::new(), revocations: Vec::new(), state: BTreeMap::new() }
    }

    /// Restores persisted state. Anything that doesn't verify from `root` is dropped.
    pub fn restore(chain_id: ChainId, root: Option<Root>, list: &MemberList) -> Self {
        Self::evaluate(chain_id, root, list.records.clone(), list.revocations.clone())
    }

    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    pub fn root(&self) -> Option<DeviceId> {
        self.root.map(|(id, _)| id)
    }

    /// The pinned root, id and key: what a device must persist to restore this.
    pub fn pinned_root(&self) -> Option<Root> {
        self.root
    }

    /// Approved, and not removed since.
    pub fn member(&self, device_id: DeviceId) -> Option<&MemberRecord> {
        self.state.get(&device_id).filter(|state| state.active).map(|state| &state.record)
    }

    pub fn is_member(&self, device_id: DeviceId) -> bool {
        self.member(device_id).is_some()
    }

    /// Was a member once and has been removed (or left).
    pub fn is_revoked(&self, device_id: DeviceId) -> bool {
        self.state.get(&device_id).is_some_and(|state| !state.active)
    }

    /// Any device's latest approval, member or not (e.g. who approved whom).
    pub fn record(&self, device_id: DeviceId) -> Option<&MemberRecord> {
        self.state.get(&device_id).map(|state| &state.record)
    }

    pub fn members(&self) -> impl Iterator<Item = &MemberRecord> {
        self.state.values().filter(|state| state.active).map(|state| &state.record)
    }

    pub fn to_list(&self) -> MemberList {
        MemberList { records: self.records.clone(), revocations: self.revocations.clone() }
    }

    /// `me` adds `candidate` (again, if it was removed before).
    pub fn approve(
        &mut self,
        me: &DeviceKey,
        my_id: DeviceId,
        candidate: Candidate,
        now: UnixMillis,
    ) -> Result<&MemberRecord, MembershipError> {
        self.require_member(me, my_id)?;
        if self.is_member(candidate.device_id) {
            return Err(MembershipError::AlreadyMember);
        }
        self.require_room()?;
        let device_id = candidate.device_id;
        // Strictly after anything already signed about it and after this device's own
        // approval (so the replay sees the signer active), even if the clock is behind.
        let after = self.latest_event(device_id).map_or(0, |at| at.0.saturating_add(1)).max(self.signer_floor(my_id));
        let Some((_, root_key)) = self.root else { return Err(MembershipError::NotAMember) };
        let record = sign_record(self.chain_id, root_key, me, my_id, candidate, UnixMillis(now.0.max(after)));
        let mut records = self.records.clone();
        records.push(record);
        *self = Self::evaluate(self.chain_id, self.root, records, self.revocations.clone());
        self.member(device_id).ok_or(MembershipError::NotAMember)
    }

    /// `me` removes `target`, which may be itself (leaving).
    pub fn revoke(
        &mut self,
        me: &DeviceKey,
        my_id: DeviceId,
        target: DeviceId,
        now: UnixMillis,
    ) -> Result<(), MembershipError> {
        self.require_member(me, my_id)?;
        if !self.is_member(target) {
            return Err(MembershipError::UnknownMember);
        }
        self.require_room()?;
        let after = self.latest_event(target).map_or(0, |at| at.0).max(self.signer_floor(my_id));
        let at = UnixMillis(now.0.max(after));
        let mut revocation =
            Revocation { device_id: target, revoked_at: at, revoked_by: my_id, signature: Bytes([0; 64]) };
        let Some((_, root_key)) = self.root else { return Err(MembershipError::NotAMember) };
        revocation.signature = me.sign(&revocation.signing_bytes(&self.chain_id.0, &root_key));
        let mut revocations = self.revocations.clone();
        revocations.push(revocation);
        *self = Self::evaluate(self.chain_id, self.root, self.records.clone(), revocations);
        if self.is_member(target) { Err(MembershipError::NotAMember) } else { Ok(()) }
    }

    /// Takes in a list from a peer. Does nothing before this device has a
    /// root: see [`Membership::offer`].
    ///
    /// New events signed by a device this one already knows was removed are
    /// dropped, whatever their date: a removed device keeps its key and could
    /// otherwise sign events backdated to when it was a member. New root
    /// records are dropped too; the root's own record is already here.
    pub fn merge(&mut self, list: &MemberList, now: UnixMillis) -> MergeOutcome {
        let Some(root) = self.root else { return MergeOutcome::default() };
        if !acceptable(list) {
            return MergeOutcome::default();
        }
        let incoming_records: Vec<MemberRecord> =
            list.records.iter().filter(|r| !r.is_root() && !self.is_revoked(r.approved_by)).cloned().collect();
        let incoming_revocations: Vec<Revocation> =
            list.revocations.iter().filter(|r| !self.is_revoked(r.revoked_by)).cloned().collect();
        let records = union(&self.records, &incoming_records, |r| r.signature, |r| r.added_at, now);
        let revocations = union(&self.revocations, &incoming_revocations, |r| r.signature, |r| r.revoked_at, now);
        let next = Self::evaluate(self.chain_id, Some(root), records, revocations);
        if next == *self {
            return MergeOutcome::default();
        }
        *self = next;
        MergeOutcome { changed: true }
    }

    /// For a device waiting to be approved: the membership `list` would give
    /// it, if the list has a single root and makes this device a member under
    /// its own key. The user confirms the approver before it's adopted.
    pub fn offer(&self, list: &MemberList, my_id: DeviceId, my_key: &Key32, now: UnixMillis) -> Option<Self> {
        if self.root.is_some() || !acceptable(list) {
            return None;
        }
        let roots: BTreeSet<Root> =
            list.records.iter().filter(|r| r.is_root()).map(|r| (r.device_id, r.device_key)).collect();
        let [root] = roots.into_iter().collect::<Vec<_>>()[..] else { return None };
        let records = union(&[], &list.records, |r| r.signature, |r| r.added_at, now);
        let revocations = union(&[], &list.revocations, |r| r.signature, |r| r.revoked_at, now);
        let candidate = Self::evaluate(self.chain_id, Some(root), records, revocations);
        candidate.member(my_id).is_some_and(|mine| mine.device_key == *my_key).then_some(candidate)
    }

    /// The earliest time `signer` can sign anything that counts: just after its own approval.
    fn signer_floor(&self, signer: DeviceId) -> u64 {
        self.member(signer).map_or(0, |record| record.added_at.0.saturating_add(1))
    }

    fn latest_event(&self, device: DeviceId) -> Option<UnixMillis> {
        let records = self.records.iter().filter(|r| r.device_id == device).map(|r| r.added_at);
        let revocations = self.revocations.iter().filter(|r| r.device_id == device).map(|r| r.revoked_at);
        records.chain(revocations).max()
    }

    fn require_member(&self, me: &DeviceKey, my_id: DeviceId) -> Result<(), MembershipError> {
        match self.member(my_id) {
            Some(record) if record.device_key == me.public() => Ok(()),
            _ => Err(MembershipError::NotAMember),
        }
    }

    fn require_room(&self) -> Result<(), MembershipError> {
        if self.records.len() + self.revocations.len() >= MAX_EVENTS {
            return Err(MembershipError::Full);
        }
        Ok(())
    }

    /// Replays the events in time order from `root`'s own record; keeps the valid ones.
    fn evaluate(
        chain_id: ChainId,
        root: Option<Root>,
        records: Vec<MemberRecord>,
        revocations: Vec<Revocation>,
    ) -> Self {
        let mut membership = Self::joining(chain_id);
        membership.root = root;
        let Some((root, root_key)) = root else { return membership };
        enum Event {
            Record(MemberRecord),
            Revocation(Revocation),
        }
        let mut events: Vec<(UnixMillis, u8, [u8; 64], Event)> = records
            .into_iter()
            .map(|r| (r.added_at, 0, r.signature.0, Event::Record(r)))
            .chain(revocations.into_iter().map(|r| (r.revoked_at, 1, r.signature.0, Event::Revocation(r))))
            .collect();
        // Time order; at the same instant approvals before removals (so a removal at the
        // approval's own time wins), then by signature so every device agrees.
        events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
        let active_key = |state: &BTreeMap<DeviceId, DeviceState>, id: DeviceId| {
            state.get(&id).filter(|s| s.active).map(|s| s.record.device_key)
        };
        for (_, _, _, event) in events {
            match event {
                Event::Record(record) => {
                    let signer = if record.is_root() {
                        // Only the pinned root (id and key) approves itself, and only once.
                        if record.device_id != root
                            || record.device_key != root_key
                            || membership.state.contains_key(&root)
                        {
                            continue;
                        }
                        Some(record.device_key)
                    } else {
                        active_key(&membership.state, record.approved_by)
                    };
                    let Some(signer) = signer else { continue };
                    if !verify(&signer, &record.signing_bytes(&chain_id.0, &root_key), &record.signature) {
                        continue;
                    }
                    membership.state.insert(record.device_id, DeviceState { record: record.clone(), active: true });
                    membership.records.push(record);
                }
                Event::Revocation(revocation) => {
                    let Some(signer) = active_key(&membership.state, revocation.revoked_by) else { continue };
                    let Some(target) = membership.state.get(&revocation.device_id) else { continue };
                    let bytes = revocation.signing_bytes(&chain_id.0, &root_key);
                    if !target.active || !verify(&signer, &bytes, &revocation.signature) {
                        continue;
                    }
                    if let Some(target) = membership.state.get_mut(&revocation.device_id) {
                        target.active = false;
                    }
                    membership.revocations.push(revocation);
                }
            }
        }
        membership
    }
}

/// A list this device will look at at all: bounded, so a peer can't make it do unbounded work.
fn acceptable(list: &MemberList) -> bool {
    list.within_limits()
}

/// `known` plus the new items in `incoming` (by signature), minus anything dated too far ahead.
fn union<T: Clone>(
    known: &[T],
    incoming: &[T],
    signature: impl Fn(&T) -> pasta_core::wire::Sig64,
    at: impl Fn(&T) -> UnixMillis,
    now: UnixMillis,
) -> Vec<T> {
    let limit = now.0.saturating_add(u64::try_from(MAX_FUTURE.as_millis()).unwrap_or(u64::MAX));
    let mut seen: HashSet<[u8; 64]> = known.iter().map(|item| signature(item).0).collect();
    let mut out = known.to_vec();
    for item in incoming {
        if at(item).0 <= limit && seen.insert(signature(item).0) {
            out.push(item.clone());
        }
    }
    out
}

fn sign_record(
    chain_id: ChainId,
    root_key: Key32,
    me: &DeviceKey,
    my_id: DeviceId,
    who: Candidate,
    at: UnixMillis,
) -> MemberRecord {
    let mut record = MemberRecord {
        device_id: who.device_id,
        device_key: who.device_key,
        name: who.name,
        os: who.os,
        node_id: who.node_id,
        added_at: at,
        approved_by: my_id,
        signature: Bytes([0; 64]),
    };
    record.signature = me.sign(&record.signing_bytes(&chain_id.0, &root_key));
    record
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::ChainKeys;

    struct Dev {
        id: DeviceId,
        key: DeviceKey,
    }

    impl Dev {
        fn new() -> Self {
            Self { id: DeviceId::random(), key: DeviceKey::generate() }
        }

        fn candidate(&self, name: &str) -> Candidate {
            Candidate {
                device_id: self.id,
                device_key: self.key.public(),
                name: name.into(),
                os: "test".into(),
                node_id: format!("n{name}"),
            }
        }
    }

    fn chain() -> ChainId {
        ChainKeys::generate().id()
    }

    const NOW: UnixMillis = UnixMillis(1_000_000);

    /// What a joiner does after the user accepts the offer.
    fn join(chain: ChainId, dev: &Dev, list: &MemberList) -> Membership {
        Membership::joining(chain).offer(list, dev.id, &dev.key.public(), NOW).expect("an offer")
    }

    #[test]
    fn creator_approves_and_joiner_accepts_the_offer() {
        let chain = chain();
        let (a, b, c) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        assert!(at_a.is_member(a.id));

        let at_b = Membership::joining(chain);
        assert!(at_b.offer(&at_a.to_list(), b.id, &b.key.public(), NOW).is_none(), "not in it yet");
        let mut joining = Membership::joining(chain);
        assert_eq!(joining.merge(&at_a.to_list(), NOW), MergeOutcome::default(), "no adoption without an offer");

        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        let mut at_b = join(chain, &b, &at_a.to_list());
        assert_eq!(at_b.root(), Some(a.id));
        assert!(at_b.is_member(a.id) && at_b.is_member(b.id));
        assert_eq!(at_b.record(b.id).unwrap().approved_by, a.id);

        // B approves C; A learns about C from B.
        at_b.approve(&b.key, b.id, c.candidate("c"), UnixMillis(3)).unwrap();
        assert!(at_a.merge(&at_b.to_list(), NOW).changed);
        assert!(at_a.is_member(c.id));
        assert!(!at_a.merge(&at_b.to_list(), NOW).changed, "idempotent");
        assert_eq!(at_a.approve(&a.key, a.id, c.candidate("c"), UnixMillis(4)), Err(MembershipError::AlreadyMember));
    }

    #[test]
    fn refuses_forged_foreign_and_multi_root_lists() {
        let chain = chain();
        let (a, b, mallory) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));

        // Mallory knows the words, makes her own root and "approves" B.
        let mut fake = Membership::create(chain, &mallory.key, mallory.candidate("m"), UnixMillis(1));
        fake.approve(&mallory.key, mallory.id, b.candidate("b"), UnixMillis(2)).unwrap();
        assert!(!at_a.merge(&fake.to_list(), NOW).changed, "other root ignored");
        assert!(!at_a.is_member(mallory.id));

        // A record claiming A approved it, signed by someone else.
        let mut list = at_a.to_list();
        let mut forged = fake.member(b.id).unwrap().clone();
        forged.approved_by = a.id;
        list.records.push(forged);
        assert!(!at_a.merge(&list, NOW).changed);

        // A genuine record from another chain.
        let mut other = Membership::create(ChainKeys::generate().id(), &a.key, a.candidate("a"), UnixMillis(1));
        other.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        assert!(!at_a.merge(&other.to_list(), NOW).changed);

        // A list that adds B under a different key than B's own is no offer for B.
        let mut wrong_key = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        let mut impostor = b.candidate("b");
        impostor.device_key = mallory.key.public();
        wrong_key.approve(&a.key, a.id, impostor, UnixMillis(2)).unwrap();
        assert!(Membership::joining(chain).offer(&wrong_key.to_list(), b.id, &b.key.public(), NOW).is_none());

        // A list with two roots is no offer either.
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(3)).unwrap();
        let mut two_roots = at_a.to_list();
        two_roots.records.extend(fake.to_list().records);
        assert!(Membership::joining(chain).offer(&two_roots, b.id, &b.key.public(), NOW).is_none());

        // Only members approve; oversized lists are ignored.
        let at_b = Membership::joining(chain);
        let mut at_b2 = at_b.clone();
        assert_eq!(
            at_b2.approve(&b.key, b.id, mallory.candidate("m"), UnixMillis(3)),
            Err(MembershipError::NotAMember)
        );
        let mut huge = at_a.to_list();
        let filler = huge.records[0].clone();
        huge.records.resize(MAX_EVENTS + 1, filler);
        assert!(Membership::joining(chain).offer(&huge, b.id, &b.key.public(), NOW).is_none());
        assert!(!at_a.merge(&huge, NOW).changed);
    }

    #[test]
    fn revocations_propagate_and_old_lists_cant_undo_them() {
        let chain = chain();
        let (a, b, c) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        let mut at_b = join(chain, &b, &at_a.to_list());
        at_b.approve(&b.key, b.id, c.candidate("c"), UnixMillis(3)).unwrap();
        at_a.merge(&at_b.to_list(), NOW);
        let before = at_a.to_list();

        at_a.revoke(&a.key, a.id, b.id, UnixMillis(4)).unwrap();
        assert!(!at_a.is_member(b.id) && at_a.is_revoked(b.id));
        assert!(at_a.is_member(c.id), "not retroactive");
        assert!(at_b.merge(&at_a.to_list(), NOW).changed);
        assert!(!at_b.is_member(b.id), "B learns it was removed");
        assert_eq!(at_b.revoke(&b.key, b.id, c.id, UnixMillis(5)), Err(MembershipError::NotAMember));

        // An older list without the revocation doesn't bring B back.
        assert!(!at_a.merge(&before, NOW).changed);
        assert!(!at_a.is_member(b.id));

        // A revocation signed by a stranger is ignored.
        let stranger = Dev::new();
        let mut list = at_a.to_list();
        let mut forged = Revocation {
            device_id: c.id,
            revoked_at: UnixMillis(7),
            revoked_by: stranger.id,
            signature: Bytes([0; 64]),
        };
        forged.signature = stranger.key.sign(&forged.signing_bytes(&chain.0, &a.key.public()));
        list.revocations.push(forged);
        assert!(!at_a.merge(&list, NOW).changed);
        assert!(at_a.is_member(c.id));
    }

    /// The reviewer's sequence: a removed device signs, after its removal,
    /// revocations of everyone (dated far ahead) and an approval of an
    /// accomplice, and a member that hasn't heard of the removal relays them.
    #[test]
    fn a_removed_device_cant_sign_anything_that_counts_after_its_removal() {
        let chain = chain();
        let (a, b, r, x) = (Dev::new(), Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        at_a.approve(&a.key, a.id, r.candidate("r"), UnixMillis(3)).unwrap();
        let lagging = at_a.clone(); // a member that hasn't heard of the removal yet
        at_a.revoke(&a.key, a.id, r.id, UnixMillis(10)).unwrap();

        let mut evil = lagging.to_list();
        for (target, at) in [(a.id, u64::MAX), (b.id, 20)] {
            let mut revocation = Revocation {
                device_id: target,
                revoked_at: UnixMillis(at),
                revoked_by: r.id,
                signature: Bytes([0; 64]),
            };
            revocation.signature = r.key.sign(&revocation.signing_bytes(&chain.0, &a.key.public()));
            evil.revocations.push(revocation);
        }
        let mut accomplice = sign_record(chain, a.key.public(), &r.key, r.id, x.candidate("x"), UnixMillis(21));
        accomplice.approved_by = r.id;
        evil.records.push(accomplice);

        let mut relayed = lagging.clone();
        relayed.merge(&evil, NOW);
        assert!(relayed.merge(&at_a.to_list(), NOW).changed, "the lagging member hears of the removal");
        // Now A takes the lagging member's list, with R's events in it.
        at_a.merge(&relayed.to_list(), NOW);
        assert!(at_a.is_member(a.id), "revocation dated after R's removal counted");
        assert!(at_a.is_member(b.id), "revocation dated after R's removal counted");
        assert!(!at_a.is_member(x.id), "approval signed after R's removal counted");
        assert!(!at_a.is_member(r.id));
        assert!(at_a.to_list().revocations.iter().all(|rev| rev.revoked_by != r.id || rev.device_id == r.id));
    }

    #[test]
    fn events_dated_far_ahead_are_refused() {
        let chain = chain();
        let (a, b) = (Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        let mut future = at_a.clone();
        future.approve(&a.key, a.id, b.candidate("b"), UnixMillis(NOW.0 + 2 * 3_600_000)).unwrap();
        assert!(!at_a.merge(&future.to_list(), NOW).changed, "a record two hours ahead");
        assert!(at_a.merge(&future.to_list(), UnixMillis(NOW.0 + 2 * 3_600_000)).changed, "fine once it's due");
    }

    #[test]
    fn a_removed_device_can_be_approved_again() {
        let chain = chain();
        let (a, b, c) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        at_a.approve(&a.key, a.id, c.candidate("c"), UnixMillis(3)).unwrap();
        let mut at_c = join(chain, &c, &at_a.to_list());
        at_a.revoke(&a.key, a.id, b.id, UnixMillis(10)).unwrap();
        let revoked = at_a.to_list();

        // Even with a clock behind the revocation, the new approval comes after it.
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(5)).unwrap();
        assert!(at_a.is_member(b.id));
        assert!(at_a.member(b.id).unwrap().added_at > UnixMillis(10));

        // Peers that saw the revocation accept the newer approval; the old revocation doesn't undo it.
        at_c.merge(&revoked, NOW);
        assert!(!at_c.is_member(b.id));
        assert!(at_c.merge(&at_a.to_list(), NOW).changed);
        assert!(at_c.is_member(b.id));
        assert!(!at_c.merge(&revoked, NOW).changed);
    }

    #[test]
    fn leaving_and_restoring() {
        let chain = chain();
        let (a, b) = (Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        let mut at_b = join(chain, &b, &at_a.to_list());
        at_b.revoke(&b.key, b.id, b.id, UnixMillis(3)).unwrap();
        assert!(at_a.merge(&at_b.to_list(), NOW).changed);
        assert!(!at_a.is_member(b.id), "left");

        let restored = Membership::restore(chain, at_a.pinned_root(), &at_a.to_list());
        assert_eq!(restored, at_a);
        let mut tampered = at_a.to_list();
        tampered.records.iter_mut().find(|record| record.device_id == a.id).unwrap().name = "renamed".into();
        assert!(
            Membership::restore(chain, at_a.pinned_root(), &tampered).members().next().is_none(),
            "bad root signature"
        );
    }

    #[test]
    fn an_offer_needs_a_list_that_is_current_signed_for_this_chain_and_not_revoked() {
        let chain = chain();
        let (a, b) = (Dev::new(), Dev::new());
        let joining = Membership::joining(chain);
        let hour = 3_600_000;

        // An approval dated more than an hour ahead of the joiner's clock isn't an offer, until it is due.
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(NOW.0 + 2 * hour)).unwrap();
        assert!(joining.offer(&at_a.to_list(), b.id, &b.key.public(), NOW).is_none());
        assert!(joining.offer(&at_a.to_list(), b.id, &b.key.public(), UnixMillis(NOW.0 + 2 * hour)).is_some());

        // Approved and removed since: nothing to offer.
        let mut removed = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        removed.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        removed.revoke(&a.key, a.id, b.id, UnixMillis(3)).unwrap();
        assert!(joining.offer(&removed.to_list(), b.id, &b.key.public(), NOW).is_none());

        // Signed for another chain (same people, other words).
        let mut other = Membership::create(ChainKeys::generate().id(), &a.key, a.candidate("a"), UnixMillis(1));
        other.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        assert!(joining.offer(&other.to_list(), b.id, &b.key.public(), NOW).is_none());

        // A device that already has a root doesn't take offers (it merges, from members).
        let joined = join(chain, &b, &{
            let mut list = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
            list.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
            list.to_list()
        });
        assert!(joined.offer(&removed.to_list(), b.id, &b.key.public(), NOW).is_none());
    }

    /// Replaying in time order means an event only counts if its signer was
    /// already a member at the event's own time. Clocks of a few seconds apart
    /// are normal, so a device whose clock is behind its approver's must still
    /// be able to approve others.
    #[test]
    fn a_device_whose_clock_is_behind_its_approvers_can_still_approve_others() {
        let chain = chain();
        let (a, b, c) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1_000));
        // A's clock reads 10 000 when it approves B; B's reads 5 000 a moment later.
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(10_000)).unwrap();
        let mut at_b = join(chain, &b, &at_a.to_list());
        at_b.approve(&b.key, b.id, c.candidate("c"), UnixMillis(5_000)).expect("B approving C with a slow clock");
        assert!(at_b.is_member(c.id));
        assert!(at_a.merge(&at_b.to_list(), NOW).changed && at_a.is_member(c.id), "A never learns of C");
    }

    /// An approval stamped in the same millisecond as its signer's own record
    /// is replayed in signature order, so half the time it comes first and is dropped.
    #[test]
    fn an_approval_in_the_same_millisecond_as_the_approvers_own_record_counts() {
        for _ in 0..24 {
            let chain = chain();
            let (a, b) = (Dev::new(), Dev::new());
            let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(7));
            at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(7)).expect("approved in the same millisecond");
        }
    }

    /// Review round 2, N1: a root record under the real root's id but another key.
    #[test]
    fn a_forged_root_record_with_the_roots_id_changes_nothing() {
        let chain = chain();
        let (a, b, mallory) = (Dev::new(), Dev::new(), Dev::new());
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(10));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(11)).unwrap();
        let before = at_a.clone();
        // Mallory signs a root record for A's id with her own key, dated before A's, and approves herself.
        let mut fake_root = a.candidate("a");
        fake_root.device_key = mallory.key.public();
        let forged_root = sign_record(chain, mallory.key.public(), &mallory.key, a.id, fake_root, UnixMillis(0));
        let mut list = at_a.to_list();
        list.records.push(forged_root);
        list.records.push(sign_record(
            chain,
            mallory.key.public(),
            &mallory.key,
            a.id,
            mallory.candidate("m"),
            UnixMillis(1),
        ));
        assert!(!at_a.merge(&list, NOW).changed);
        assert_eq!(at_a, before);
        let restored = Membership::restore(chain, Some((a.id, mallory.key.public())), &at_a.to_list());
        assert!(restored.members().next().is_none(), "a root pinned with another key verifies nothing");
    }

    /// Review round 2, N2: a removed device signs events backdated to when it was a member.
    #[test]
    fn a_removed_device_cant_plant_backdated_events_once_its_removal_is_known() {
        let chain = chain();
        let (a, b, r, x) = (Dev::new(), Dev::new(), Dev::new(), Dev::new());
        let root_key = a.key.public();
        let mut at_a = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        at_a.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        at_a.approve(&a.key, a.id, r.candidate("r"), UnixMillis(3)).unwrap();
        at_a.revoke(&a.key, a.id, r.id, UnixMillis(10)).unwrap();

        let mut evil = at_a.to_list();
        let accomplice = sign_record(chain, root_key, &r.key, r.id, x.candidate("x"), UnixMillis(4));
        evil.records.push(accomplice);
        for target in [a.id, b.id] {
            let mut revocation = Revocation {
                device_id: target,
                revoked_at: UnixMillis(5),
                revoked_by: r.id,
                signature: Bytes([0; 64]),
            };
            revocation.signature = r.key.sign(&revocation.signing_bytes(&chain.0, &root_key));
            evil.revocations.push(revocation);
        }
        assert!(!at_a.merge(&evil, NOW).changed);
        assert!(at_a.is_member(a.id) && at_a.is_member(b.id) && !at_a.is_member(x.id) && !at_a.is_member(r.id));
    }

    /// Review round 2, N4: a genuine record from a chain with another root doesn't count here.
    #[test]
    fn records_are_bound_to_their_chains_root() {
        let chain = chain();
        let (a, b, mallory) = (Dev::new(), Dev::new(), Dev::new());
        let mut real = Membership::create(chain, &a.key, a.candidate("a"), UnixMillis(1));
        real.approve(&a.key, a.id, b.candidate("b"), UnixMillis(2)).unwrap();
        // Mallory's root re-approves A under A's real key and appends A's genuine record of B.
        let mut fake = Membership::create(chain, &mallory.key, mallory.candidate("m"), UnixMillis(0));
        fake.approve(&mallory.key, mallory.id, a.candidate("a"), UnixMillis(1)).unwrap();
        let mut list = fake.to_list();
        list.records.push(real.record(b.id).unwrap().clone());
        assert!(
            Membership::joining(chain).offer(&list, b.id, &b.key.public(), NOW).is_none(),
            "B's record signed under A's root counted under Mallory's"
        );
    }
}
