//! Sync-chain membership over real connections (#20, #22).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::proxy::Proxy;
use common::raw::{self, Incoming, Joined};
use common::{
    ChainChoice, LOOPBACK_NODE, Peer, PeerBuilder, TestChain, dial_via, fast_timing, identity, mesh, settle,
    sync_members, within,
};
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::protocol::{Inner, Message, Ping};
use pasta_crypto::membership::{Candidate, Membership};
use pasta_crypto::{ChainKeys, DeviceKey, keys};
use pasta_net::network::{NetEvent, Timing};
use pasta_net::node::NodeEvent;
use pasta_net::trust::{ChainSetup, PeerTrust};

fn connected_events(events: &[NodeEvent]) -> usize {
    events.iter().filter(|event| matches!(event, NodeEvent::Net(NetEvent::Connected { .. }))).count()
}

fn member(chain: &Arc<TestChain>) -> ChainChoice {
    ChainChoice::Member(Arc::clone(chain))
}

#[tokio::test]
async fn a_joining_device_gets_nothing_until_approved_then_everything() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(ChainChoice::Member(chain.clone())).start().await;
    let b = PeerBuilder::new("b").chain(ChainChoice::Member(chain.clone())).start().await;
    let fresh = PeerBuilder::new("fresh").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&a, &b, &fresh]).await;
    a.wait_connected(2).await;
    a.wait_pending(1).await;
    b.wait_pending(1).await;

    a.clipboard.copy("before approval");
    settle().await;
    assert!(fresh.clipboard.writes().is_empty(), "a pending device received an item");
    fresh.clipboard.copy("from the pending device");
    settle().await;
    assert_eq!(b.clipboard.writes().len(), 1, "only A's item reached B");

    a.node().network().approve(fresh.id(), &fresh.fingerprint()).await.unwrap();
    fresh.wait_approved(true).await;
    // B learns about the approval from A and starts sending too.
    b.wait_pending(0).await;
    b.clipboard.copy("after approval");
    assert_eq!(fresh.wait_writes(1).await, ["after approval"]);
}

/// The real flow: one device creates the chain, shows the 24 words, the other
/// types them in and waits for approval.
#[tokio::test]
async fn two_devices_with_the_same_words_sync_once_the_joiner_is_approved() {
    let keys = Arc::new(ChainKeys::generate());
    let creator_id = DeviceId::random();
    let creator_as = Candidate {
        device_id: creator_id,
        device_key: identity(creator_id).public(),
        name: "cachy-desktop".into(),
        os: "linux".into(),
        node_id: common::LOOPBACK_NODE.into(),
    };
    let membership = Membership::create(keys.id(), &identity(creator_id), creator_as, UnixMillis::now());
    let creator = PeerBuilder::new("creator")
        .device_id(creator_id)
        .chain(ChainChoice::Setup(ChainSetup { keys: Arc::clone(&keys), membership }))
        .start()
        .await;

    let typed_in = Arc::new(ChainKeys::from_words(&keys.words()).unwrap());
    assert_eq!(typed_in.id(), keys.id());
    let joiner = PeerBuilder::new("joiner")
        .chain(ChainChoice::Setup(ChainSetup { keys: typed_in, membership: Membership::joining(keys.id()) }))
        .start()
        .await;
    mesh(&[&creator, &joiner]).await;
    creator.wait_pending(1).await;

    creator.clipboard.copy("hello before approval");
    joiner.clipboard.copy("hello from the joiner");
    settle().await;
    assert!(joiner.clipboard.writes().is_empty() && creator.clipboard.writes().is_empty());

    creator.node().network().approve(joiner.id(), &joiner.fingerprint()).await.unwrap();
    joiner.wait_approved(true).await;
    creator.clipboard.copy("welcome");
    assert_eq!(joiner.wait_writes(1).await, ["welcome"]);
    joiner.clipboard.copy("thanks");
    assert_eq!(creator.wait_writes(1).await, ["thanks"]);
}

#[tokio::test]
async fn a_device_with_other_words_never_connects_and_shares_nothing() {
    let mut a = PeerBuilder::new("a").chain(member(&TestChain::new())).start().await;
    let mut other = PeerBuilder::new("other").chain(member(&TestChain::new())).start().await;
    mesh(&[&a, &other]).await;

    a.clipboard.copy("only for my chain");
    other.clipboard.copy("only for the other chain");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    for peer in [&mut a, &mut other] {
        let name = peer.name.clone();
        assert_eq!(connected_events(&peer.drain_events()), 0, "{name} connected to the other chain");
        assert!(peer.peers().borrow().is_empty(), "{name} lists the other chain's device");
        assert!(peer.clipboard.writes().is_empty(), "{name} received an item from another chain");
        assert!(peer.pending_ids().is_empty(), "devices from another chain aren't even pending");
    }
}

#[tokio::test]
async fn a_device_without_a_chain_neither_connects_nor_receives() {
    let chain = TestChain::new();
    let mut a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let mut loner = PeerBuilder::new("loner").chain(ChainChoice::None).start().await;
    mesh(&[&a, &loner]).await;

    a.clipboard.copy("not for the loner");
    loner.clipboard.copy("not for anyone");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(connected_events(&a.drain_events()) + connected_events(&loner.drain_events()), 0);
    assert!(a.peers().borrow().is_empty() && loner.peers().borrow().is_empty());
    assert!(loner.clipboard.writes().is_empty());
    assert!(a.clipboard.writes().is_empty());
    assert!(loner.chain_status().borrow().chain.is_none());
}

#[tokio::test]
async fn the_dialer_backs_off_from_a_peer_in_another_chain() {
    // Slow retries so a missing backoff can't hide behind the 50 ms discovery poll.
    let slow = Timing { retry_base: Duration::from_millis(500), retry_max: Duration::from_secs(2), ..fast_timing() };
    let mut a = PeerBuilder::new("a").chain(member(&TestChain::new())).timing(slow).start().await;
    let mut other = PeerBuilder::new("other").chain(member(&TestChain::new())).start().await;
    sync_members(&[&a, &other]).await;
    let proxy = Proxy::start(other.addr).await;
    // Only A dials, and it dials through the proxy, which counts the attempts.
    dial_via(&a, proxy.addr, &other);

    tokio::time::sleep(Duration::from_millis(2500)).await;
    let attempts = proxy.connections();
    assert!((2..=5).contains(&attempts), "expected a few retries with growing gaps, saw {attempts} dials in 2.5 s");
    assert_eq!(connected_events(&a.drain_events()) + connected_events(&other.drain_events()), 0);
}

#[tokio::test]
async fn any_member_can_approve_and_the_others_learn_by_gossip() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("b").chain(member(&chain)).start().await;
    let fresh = PeerBuilder::new("fresh").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&a, &b, &fresh]).await;
    a.wait_pending(1).await;
    b.wait_pending(1).await;

    // B didn't create anything; it approves just the same.
    b.node().network().approve(fresh.id(), &fresh.fingerprint()).await.unwrap();
    fresh.wait_approved(true).await;
    a.wait_pending(0).await;
    a.wait_members(4).await; // the root that never runs, A, B and the new device
    assert!(a.member_ids().contains(&fresh.id()) && b.member_ids().contains(&fresh.id()));
    assert!(fresh.member_ids().contains(&a.id()) && fresh.member_ids().contains(&b.id()));

    a.clipboard.copy("from a");
    assert_eq!(fresh.wait_writes(1).await, ["from a"]);
    fresh.clipboard.copy("from the new device");
    assert_eq!(a.wait_writes(1).await, ["from the new device"]);
    assert_eq!(b.wait_writes(2).await, ["from a", "from the new device"]);
}

#[tokio::test]
async fn approve_decline_and_remove_refuse_devices_that_are_not_there() {
    let a = PeerBuilder::new("a").chain(member(&TestChain::new())).start().await;
    let stranger = DeviceId::random();
    assert!(a.node().network().approve(stranger, "0000000000000000").await.is_err(), "nobody to approve");
    assert!(a.node().network().remove(stranger).await.is_err(), "not a member");
    // A device that isn't in a chain can't change one either.
    let loner = PeerBuilder::new("loner").chain(ChainChoice::None).start().await;
    assert!(loner.node().network().approve(stranger, "0000000000000000").await.is_err());
    assert!(loner.node().network().remove(stranger).await.is_err());
}

#[tokio::test]
async fn removing_a_member_cuts_it_off_and_it_leaves_the_chain() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("b").chain(member(&chain)).start().await;
    let mut f = PeerBuilder::new("f").chain(member(&chain)).start().await;
    mesh(&[&a, &b, &f]).await;
    for peer in [&a, &b, &f] {
        peer.wait_connected(2).await;
    }
    a.clipboard.copy("while a member");
    assert_eq!(f.wait_writes(1).await, ["while a member"]);

    a.node().network().remove(f.id()).await.unwrap();
    // F is told, and leaves by itself.
    f.wait_event(|event| matches!(event, NodeEvent::Net(NetEvent::RemovedFromChain))).await;
    f.wait_left().await;
    // The remaining members drop it and agree it's gone.
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    b.wait_members(3).await;
    for peer in [&a, &b] {
        assert!(!peer.member_ids().contains(&f.id()));
        let peers = peer.peers();
        let peers = peers.borrow();
        assert!(
            peers.iter().all(|row| row.device_id != f.id() || !row.connected),
            "{} still connected to F",
            peer.name
        );
    }
    b.wait_pending(0).await;
    a.wait_pending(0).await;

    // Nothing flows either way any more.
    a.clipboard.copy("after the removal");
    assert_eq!(b.wait_writes(2).await, ["while a member", "after the removal"]);
    f.clipboard.copy("from the removed device");
    settle().await;
    assert_eq!(f.clipboard.writes().len(), 1, "the removed device received something new");
    assert_eq!(a.clipboard.writes().len(), 0);
    assert_eq!(b.clipboard.writes().len(), 2);
}

#[tokio::test]
async fn a_removed_device_can_be_approved_again() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("b").chain(member(&chain)).start().await;
    let f = PeerBuilder::new("f").chain(member(&chain)).start().await;
    mesh(&[&a, &b, &f]).await;
    a.wait_connected(2).await;

    a.node().network().remove(f.id()).await.unwrap();
    f.wait_left().await;
    b.wait_members(3).await;

    // F types the words in again and shows up as a device that was removed before.
    f.node().network().set_chain(Some(chain.joining())).await;
    a.wait_pending(1).await;
    b.wait_pending(1).await;
    assert!(a.chain_status().borrow().chain.as_ref().unwrap().pending[0].was_removed);

    b.node().network().approve(f.id(), &f.fingerprint()).await.unwrap();
    f.wait_approved(true).await;
    a.wait_pending(0).await;
    a.wait_members(4).await;
    a.clipboard.copy("welcome back");
    assert_eq!(f.wait_writes(1).await, ["welcome back"]);
    f.clipboard.copy("glad to be back");
    assert_eq!(a.wait_writes(1).await, ["glad to be back"]);
    assert_eq!(b.wait_writes(2).await, ["welcome back", "glad to be back"]);
}

#[tokio::test]
async fn a_declined_device_is_refused_until_the_network_restarts() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let fresh = PeerBuilder::new("fresh").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&a, &fresh]).await;
    a.wait_pending(1).await;

    a.node().network().decline(fresh.id()).await.unwrap();
    a.wait_pending(0).await;
    // It keeps knocking; it stays out of the list and out of the chain.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(a.pending_ids().is_empty(), "a declined device came back as pending");
    assert!(
        a.node().network().approve(fresh.id(), &fresh.fingerprint()).await.is_err(),
        "nothing to approve while declined"
    );
    a.clipboard.copy("not for you");
    fresh.clipboard.copy("let me in");
    settle().await;
    assert!(fresh.clipboard.writes().is_empty() && a.clipboard.writes().is_empty());
    assert!(!a.member_ids().contains(&fresh.id()));

    // After a restart the decision is forgotten and the device can be approved.
    let a_id = a.id();
    let mut old = a;
    old.stop().await;
    drop(old);
    let a = PeerBuilder::new("a").device_id(a_id).chain(member(&chain)).start().await;
    mesh(&[&a, &fresh]).await;
    a.wait_pending(1).await;
    a.node().network().approve(fresh.id(), &fresh.fingerprint()).await.unwrap();
    fresh.wait_approved(true).await;
}

#[tokio::test]
async fn leaving_stops_syncing_and_the_others_see_it_gone() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("b").chain(member(&chain)).start().await;
    let c = PeerBuilder::new("c").chain(member(&chain)).start().await;
    mesh(&[&a, &b, &c]).await;
    for peer in [&a, &b, &c] {
        peer.wait_connected(2).await;
    }

    b.node().network().leave().await.unwrap();
    b.wait_left().await;
    a.wait_members(3).await;
    c.wait_members(3).await;
    a.wait_connected(1).await;
    c.wait_connected(1).await;
    for peer in [&a, &c] {
        assert!(!peer.member_ids().contains(&b.id()), "{} still lists B as a member", peer.name);
    }

    a.clipboard.copy("after b left");
    assert_eq!(c.wait_writes(1).await, ["after b left"]);
    b.clipboard.copy("from b, who left");
    settle().await;
    assert!(b.clipboard.writes().is_empty(), "a device that left received an item");
    assert_eq!(a.clipboard.writes().len(), 0);
    assert_eq!(c.clipboard.writes().len(), 1);
}

// ---------------------------------------------------------------------------
// Review follow-ups: capturing a waiting device, approving the wrong key,
// lagging members, pending devices, removed devices (#22)
// ---------------------------------------------------------------------------

fn candidate(id: DeviceId, name: &str) -> Candidate {
    Candidate {
        device_id: id,
        device_key: identity(id).public(),
        name: name.into(),
        os: "test".into(),
        node_id: LOOPBACK_NODE.into(),
    }
}

async fn wait_pending_has(peer: &Peer, id: DeviceId) {
    let mut chain = peer.chain_status();
    within(chain.wait_for(|status| {
        status.chain.as_ref().is_some_and(|chain| chain.pending.iter().any(|pending| pending.device_id == id))
    }))
    .await
    .unwrap_or_else(|_| panic!("{} never had {id} pending", peer.name));
}

/// Waits until `peer` counts `id` as a member (or not).
async fn wait_member(peer: &Peer, id: DeviceId, member: bool) {
    let mut chain = peer.chain_status();
    within(chain.wait_for(|status| {
        status.chain.as_ref().is_some_and(|chain| chain.members.iter().any(|record| record.device_id == id) == member)
    }))
    .await
    .unwrap_or_else(|_| panic!("{} never had member={member} for {id}", peer.name));
}

/// Someone who learned the 24 words: they run a chain of their own (same
/// chain id, themselves as the root) in which `victim` is already approved.
async fn start_capturer(chain: &TestChain, victim: DeviceId) -> Peer {
    let id = DeviceId::random();
    let key = identity(id);
    let long_ago = UnixMillis(UnixMillis::now().0 - 60_000);
    let mut fake = Membership::create(chain.keys.id(), &key, candidate(id, "evil-laptop"), long_ago);
    fake.approve(&key, id, candidate(victim, "win-workstation"), UnixMillis::now()).unwrap();
    PeerBuilder::new("evil-laptop")
        .device_id(id)
        .chain(ChainChoice::Setup(ChainSetup { keys: Arc::clone(&chain.keys), membership: fake }))
        .start()
        .await
}

#[tokio::test]
async fn a_waiting_device_joins_as_soon_as_a_member_approves_it() {
    let chain = TestChain::new();
    let real = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let joiner = PeerBuilder::new("win-workstation").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&real, &joiner]).await;

    // Nothing syncs while it waits.
    real.wait_pending(1).await;
    real.clipboard.copy("before the approval");
    settle().await;
    assert!(joiner.clipboard.writes().is_empty(), "a waiting device took an item");
    assert!(!joiner.chain_status().borrow().chain.as_ref().unwrap().approved);

    // Approving on the member is all it takes: the joiner adopts the list by itself.
    real.node().network().approve(joiner.id(), &joiner.fingerprint()).await.unwrap();
    joiner.wait_approved(true).await;
    assert!(joiner.member_ids().contains(&real.id()) && joiner.member_ids().contains(&joiner.id()));
    real.clipboard.copy("after the approval");
    assert_eq!(joiner.wait_writes(1).await, ["after the approval"]);
}

/// The trade-off of ADR 0010: a waiting device takes the first approval it
/// gets, so someone else who knows the words and approves it first captures
/// it, and the real chain's approval that comes later is ignored.
#[tokio::test]
async fn the_first_approval_wins_and_later_roots_are_ignored() {
    let chain = TestChain::new();
    let real = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let joiner_id = DeviceId::random();
    let attacker = start_capturer(&chain, joiner_id).await;
    let joiner = PeerBuilder::new("win-workstation")
        .device_id(joiner_id)
        .chain(ChainChoice::Joining(chain.clone()))
        .start()
        .await;
    mesh(&[&attacker, &joiner]).await;
    joiner.wait_approved(true).await;
    assert!(joiner.member_ids().contains(&attacker.id()), "joined the chain that approved it first");

    mesh(&[&real, &attacker, &joiner]).await;
    wait_pending_has(&real, joiner_id).await;
    real.node().network().approve(joiner_id, &joiner.fingerprint()).await.unwrap();
    settle().await;
    assert!(!joiner.member_ids().contains(&real.id()), "a list with another root was merged");
}

#[tokio::test]
async fn approving_needs_the_fingerprint_of_the_key_that_connected() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let fresh = PeerBuilder::new("win-workstation").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&a, &fresh]).await;
    a.wait_pending(1).await;

    let shown = fresh.fingerprint();
    assert_eq!(shown.len(), 16);
    let mut last_digit_off = shown.clone();
    last_digit_off.pop();
    last_digit_off.push(if shown.ends_with('0') { '1' } else { '0' });
    let own = keys::short(&a.key.public());
    for wrong in ["", &shown[..8], &shown[..15], "0000000000000000", &last_digit_off, &own] {
        assert!(a.node().network().approve(fresh.id(), wrong).await.is_err(), "approved with {wrong:?}");
    }
    settle().await;
    assert!(!a.member_ids().contains(&fresh.id()), "approved on a wrong fingerprint");
    assert_eq!(a.pending_ids(), [fresh.id()], "still waiting");
    assert!(!fresh.chain_status().borrow().chain.as_ref().unwrap().approved, "approved anyway");

    a.node().network().approve(fresh.id(), &shown).await.unwrap();
    fresh.wait_approved(true).await;
    assert!(a.member_ids().contains(&fresh.id()));
}

/// The key shown to the user is the key that gets approved: a pending device
/// that comes back under another key can't be approved with the old fingerprint.
#[tokio::test]
async fn a_pending_device_that_changes_key_has_to_be_compared_again() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let id = DeviceId::random();
    let (first_key, second_key) = (DeviceKey::generate(), DeviceKey::generate());
    let wait_key = |key: pasta_core::wire::Key32| {
        let mut chain = a.chain_status();
        async move {
            within(chain.wait_for(|status| {
                status.chain.as_ref().is_some_and(|chain| {
                    chain.pending.iter().any(|pending| pending.device_id == id && pending.device_key == key)
                })
            }))
            .await
            .unwrap_or_else(|_| panic!("the pending device never showed key {}", keys::short(&key)));
        }
    };

    let _first = raw::join(a.addr, id, &first_key, &chain.keys).await;
    wait_key(first_key.public()).await;
    let first_fingerprint = keys::short(&first_key.public());
    // Same device id, other key; the old connection is still open.
    let _second = raw::join(a.addr, id, &second_key, &chain.keys).await;
    wait_key(second_key.public()).await;

    let network = a.node().network();
    assert!(network.approve(id, &first_fingerprint).await.is_err(), "approved a key that isn't the one connected");
    assert!(!a.member_ids().contains(&id));
    network.approve(id, &keys::short(&second_key.public())).await.unwrap();
    let record = a.chain_status().borrow().chain.as_ref().unwrap().members.iter().find(|m| m.device_id == id).cloned();
    assert_eq!(record.unwrap().device_key, second_key.public(), "the member record has the other key");
}

// -- Lagging members ---------------------------------------------------------

/// A, B and D are members, and so is R until A removes it while D is offline.
/// R keeps its key and the words and writes a list dated after its removal:
/// revocations of B and of A, and an approval of X. Then D comes back and
/// meets R first.
struct Lagging {
    a: Peer,
    b: Peer,
    d: Peer,
    r: Peer,
    x: DeviceId,
}

#[derive(Clone, Copy, PartialEq)]
enum RevokeA {
    /// Dated two hours ahead, which nobody accepts.
    TooFarAhead,
    /// Dated just after R's removal, like the one for B.
    AfterRemoval,
}

async fn lagging_member_meets_a_removed_device(revoke_a: RevokeA) -> Lagging {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("win-workstation").chain(member(&chain)).start().await;
    let d_id = DeviceId::random();
    let mut d = PeerBuilder::new("cachy-desktop").device_id(d_id).chain(member(&chain)).start().await;
    let (r_id, x) = (DeviceId::random(), DeviceId::random());
    chain.admit(r_id, "steamdeck");
    // Everybody, D included, knows R as a member when D goes offline.
    sync_members(&[&a, &b, &d]).await;
    d.stop().await;
    drop(d);

    // A removes R and tells B.
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    a.node().network().remove(r_id).await.unwrap();
    wait_member(&b, r_id, false).await;

    // R, still holding its key and the words, writes events dated after its removal.
    let r_key = identity(r_id);
    let after_removal = UnixMillis(UnixMillis::now().0 + 5_000);
    let mut evil = chain.setup().membership;
    evil.revoke(&r_key, r_id, b.id(), after_removal).unwrap();
    let a_revoked_at = match revoke_a {
        RevokeA::TooFarAhead => UnixMillis(after_removal.0 + 2 * 3_600_000),
        RevokeA::AfterRemoval => UnixMillis(after_removal.0 + 1),
    };
    evil.revoke(&r_key, r_id, a.id(), a_revoked_at).unwrap();
    evil.approve(&r_key, r_id, candidate(x, "accomplice"), UnixMillis(after_removal.0 + 2)).unwrap();
    let r = PeerBuilder::new("steamdeck")
        .device_id(r_id)
        .chain(ChainChoice::Setup(ChainSetup { keys: Arc::clone(&chain.keys), membership: evil }))
        .start()
        .await;

    // D comes back with what it knew, and meets R before anyone else.
    let d = PeerBuilder::new("cachy-desktop").device_id(d_id).chain(member(&chain)).start().await;
    d.discovery.set_peers(vec![r.candidate()]);
    r.discovery.set_peers(vec![d.candidate()]);
    wait_member(&d, x, true).await; // R was still a member as far as D knew
    Lagging { a, b, d, r, x }
}

/// Records whether `peer` ever listed `device` as a member.
fn watch_for_member(peer: &Peer, device: DeviceId) -> Arc<AtomicBool> {
    let seen = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&seen);
    let mut chain = peer.chain_status();
    tokio::spawn(async move {
        loop {
            let listed = chain
                .borrow_and_update()
                .chain
                .as_ref()
                .is_some_and(|chain| chain.members.iter().any(|record| record.device_id == device));
            if listed {
                flag.store(true, Ordering::SeqCst);
            }
            if chain.changed().await.is_err() {
                return;
            }
        }
    });
    seen
}

#[tokio::test]
async fn a_lagging_member_that_meets_a_removed_device_first_still_ends_up_with_the_real_members() {
    let Lagging { a, b, d, r, x } = lagging_member_meets_a_removed_device(RevokeA::TooFarAhead).await;
    let a_saw_x = watch_for_member(&a, x);
    let b_saw_x = watch_for_member(&b, x);

    // D met R first: as far as D knew R was a member, so B is out and X is in for now.
    assert!(!d.member_ids().contains(&b.id()), "the premise: D took R's revocation of B");

    // Then D meets A and B, which know R was removed.
    d.discovery.set_peers(vec![r.candidate(), a.candidate(), b.candidate()]);
    a.discovery.set_peers(vec![b.candidate(), d.candidate()]);
    b.discovery.set_peers(vec![a.candidate(), d.candidate()]);
    wait_member(&d, b.id(), true).await;
    wait_member(&d, x, false).await;
    wait_member(&d, r.id(), false).await;
    for (name, peer) in [("A", &a), ("B", &b), ("D", &d)] {
        let members = peer.member_ids();
        assert!(
            members.contains(&a.id()) && members.contains(&b.id()) && members.contains(&d.id()),
            "{name}: {members:?}"
        );
        assert!(!members.contains(&x), "{name} lists the accomplice as a member");
        assert!(!members.contains(&r.id()), "{name} still lists the removed device");
    }
    assert!(!a_saw_x.load(Ordering::SeqCst) && !b_saw_x.load(Ordering::SeqCst), "A or B listed X at some point");

    // And the three really sync again, without X or R. (R may stay connected to D: it
    // never accepts that it was removed, but it gets nothing.)
    a.wait_connected(2).await;
    let mut peers = d.peers();
    within(peers.wait_for(|peers| {
        [a.id(), b.id()].iter().all(|id| peers.iter().any(|peer| peer.device_id == *id && peer.connected))
    }))
    .await
    .expect("D never connected to A and B");
    a.clipboard.copy("from a");
    assert_eq!(b.wait_writes(1).await, ["from a"]);
    assert_eq!(d.wait_writes(1).await, ["from a"]);
    d.clipboard.copy("from d");
    assert_eq!(a.wait_writes(1).await, ["from d"]);
    assert_eq!(b.wait_writes(2).await, ["from a", "from d"]);
    settle().await;
    assert!(r.clipboard.writes().is_empty(), "the removed device received an item");
}

/// The same, but R's revocations of A *and* B both count at D until D hears of
/// R's removal, and the only members D could hear it from are the two it was
/// just told to forget.
#[tokio::test]
#[ignore = "known limit (docs/SECURITY-MODEL.md, misled member): a member that takes a removed device's revocations of the others before it hears of the removal drops every list that could correct it; it has to rejoin"]
async fn a_lagging_member_that_lost_every_peer_to_a_removed_device_recovers() {
    let Lagging { a, b, d, r, x } = lagging_member_meets_a_removed_device(RevokeA::AfterRemoval).await;
    d.discovery.set_peers(vec![r.candidate(), a.candidate(), b.candidate()]);
    a.discovery.set_peers(vec![b.candidate(), d.candidate()]);
    b.discovery.set_peers(vec![a.candidate(), d.candidate()]);
    wait_member(&d, a.id(), true).await;
    wait_member(&d, b.id(), true).await;
    wait_member(&d, x, false).await;
}

// -- Pending devices store nothing -------------------------------------------

fn bound_ids(events: &[NodeEvent]) -> Vec<DeviceId> {
    events
        .iter()
        .filter_map(|event| match event {
            NodeEvent::Net(NetEvent::PeerBound { device_id, .. }) => Some(*device_id),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_pending_device_is_listed_while_connected_but_never_stored() {
    let chain = TestChain::new();
    let mut a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("win-workstation").chain(member(&chain)).start().await;
    let mut fresh = PeerBuilder::new("cachy-desktop").chain(ChainChoice::Joining(chain.clone())).start().await;
    mesh(&[&a, &b, &fresh]).await;
    a.wait_connected(2).await;
    a.wait_pending(1).await;
    settle().await;

    // Listed as a connected, pending device.
    {
        let peers = a.peers();
        let peers = peers.borrow();
        let row = peers.iter().find(|row| row.device_id == fresh.id()).expect("the pending device isn't listed");
        assert!(row.connected && row.trust == PeerTrust::Pending, "{row:?}");
    }
    // But only the member connection was persisted.
    let bound = bound_ids(&a.drain_events());
    assert!(bound.contains(&b.id()), "a member's connection must still produce PeerBound");
    assert!(!bound.contains(&fresh.id()), "a pending device was persisted");
    // The waiting device can't tell who is a member yet, so it stores nobody either.
    assert!(bound_ids(&fresh.drain_events()).is_empty(), "a waiting device persisted its peers");

    // Gone from the list once it disconnects: nothing remembers it.
    fresh.stop().await;
    let mut peers = a.peers();
    within(peers.wait_for(|peers| peers.iter().all(|row| row.device_id != fresh.id())))
        .await
        .expect("the disconnected pending device is still listed");
    a.wait_connected(1).await;
    assert!(!bound_ids(&a.drain_events()).contains(&fresh.id()));
}

// -- A removed device is told once -------------------------------------------

/// Reads for `window` (pinging so the connection stays up) and returns the
/// decrypted `encrypted` messages that arrived.
async fn read_inner(raw: &mut Joined, window: Duration) -> Vec<Inner> {
    let end = std::time::Instant::now() + window;
    let mut received = Vec::new();
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        raw::send(&mut raw.ws, raw.id, Message::Ping(Ping {})).await;
        match tokio::time::timeout(left.min(Duration::from_millis(100)), raw::next(&mut raw.ws)).await {
            Err(_) => {}
            Ok(Incoming::Message(envelope)) => {
                if matches!(envelope.message, Message::Encrypted(_)) {
                    received.push(raw.chain.open_envelope(&envelope).expect("a message sealed under the chain key"));
                }
            }
            Ok(Incoming::Close(code)) => panic!("the connection was closed ({code})"),
            Ok(Incoming::Gone) => panic!("the connection is gone"),
        }
    }
    received
}

/// The member lists among `received`. Members also announce their settings (#31) when a
/// peer connects; those are skipped.
fn member_lists(received: Vec<Inner>) -> Vec<pasta_core::chain::MemberList> {
    received
        .into_iter()
        .filter_map(|inner| match inner {
            Inner::Members(list) => Some(list),
            Inner::Settings(_) => None,
            other => panic!("expected a member list, got {}", other.kind()),
        })
        .collect()
}

#[tokio::test]
async fn a_removed_device_that_stays_connected_is_told_once() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let other = PeerBuilder::new("win-workstation").chain(ChainChoice::Joining(chain.clone())).start().await;
    let id = DeviceId::random();
    chain.admit(id, "cachy-desktop");
    mesh(&[&a, &other]).await;
    let mut raw = raw::join(a.addr, id, &identity(id), &chain.keys).await;
    wait_pending_has(&a, other.id()).await;

    // As a member it got the list when it connected.
    let lists = member_lists(read_inner(&mut raw, Duration::from_millis(400)).await);
    assert_eq!(lists.len(), 1, "expected the list sent on connecting");
    assert!(lists[0].revocations.is_empty());

    // Removed while connected: it is told, once.
    a.node().network().remove(id).await.unwrap();
    let lists = member_lists(read_inner(&mut raw, Duration::from_millis(500)).await);
    assert_eq!(lists.len(), 1, "the removed device should get the list that removes it exactly once");
    assert!(lists[0].revocations.iter().any(|revocation| revocation.device_id == id));

    // More changes to the member list (approving someone else) don't send it anything.
    a.node().network().approve(other.id(), &other.fingerprint()).await.unwrap();
    other.wait_approved(true).await;
    assert!(a.member_ids().contains(&other.id()));
    let later = read_inner(&mut raw, Duration::from_millis(600)).await;
    assert!(later.is_empty(), "the removed device was sent {} more message(s)", later.len());
    // It is listed as pending (it can still be approved again) but is not a member.
    assert!(a.pending_ids().contains(&id) && !a.member_ids().contains(&id));
}
