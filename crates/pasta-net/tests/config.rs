//! Per-device settings over real connections (#31, #32): announcements,
//! signed changes, and the sender side of mode enforcement.

mod common;

use std::time::Duration;

use common::raw;
use common::{ChainChoice, Peer, PeerBuilder, TestChain, identity, mesh, settle, within};
use pasta_core::device::{DeviceId, SyncMode, UnixMillis};
use pasta_core::protocol::Inner;
use pasta_core::remote::{ConfigUpdate, DeviceSettings, SettingsVersion, signing_bytes};
use pasta_net::network::NetEvent;
use pasta_net::node::NodeEvent;
use uuid::Uuid;

/// What `peer`'s network last heard that `device` announced.
fn announced(peer: &Peer, device: DeviceId) -> Option<DeviceSettings> {
    peer.node().network().peer_settings(device)
}

/// Bumps `peer`'s announced settings to `mode`.
fn announce(peer: &Peer, mode: SyncMode) {
    let now = UnixMillis::now();
    peer.device_settings.send_modify(|settings| {
        settings.mode = mode;
        settings.version = settings.version.next(peer.id(), now);
    });
}

/// Waits until `peer` knows `device` announced something matching `check`.
async fn await_announcement(peer: &Peer, device: DeviceId, check: impl Fn(&DeviceSettings) -> bool + Clone) {
    within(async {
        loop {
            if announced(peer, device).is_some_and(|settings| check(&settings)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

/// Whether `peer` saw any `ConfigUpdate` in `events` so far.
fn saw_update(events: &[NodeEvent]) -> bool {
    events.iter().any(|event| matches!(event, NodeEvent::Net(NetEvent::ConfigUpdate { .. })))
}

#[tokio::test]
async fn members_see_each_others_announcements_and_reannounce_on_change() {
    let a = PeerBuilder::new("a").start().await;
    let mut b = PeerBuilder::new("b").start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // Each device's initial settings (Both, never expiring) arrive on connect.
    await_announcement(&a, b.id(), |settings| settings.mode == SyncMode::Both).await;
    await_announcement(&b, a.id(), |settings| settings.mode == SyncMode::Both).await;

    // Changing the settings re-announces them to everyone connected.
    let change = DeviceSettings {
        mode: SyncMode::Receive,
        ttl_secs: Some(300),
        paused_until: None,
        version: announced(&b, a.id()).unwrap().version.next(a.id(), UnixMillis::now()),
    };
    a.device_settings.send_replace(change);
    await_announcement(&b, a.id(), |settings| settings.version == change.version).await;
    assert_eq!(announced(&b, a.id()), Some(change), "b learned a's new settings");
    assert_eq!(announced(&a, b.id()).unwrap().mode, SyncMode::Both, "b's settings are untouched");

    // The change also surfaces as an event on the other side, for persisting.
    let NodeEvent::Net(NetEvent::PeerSettings { device_id, settings: seen }) =
        b.wait_event(|event| {
            matches!(event, NodeEvent::Net(NetEvent::PeerSettings { settings, .. }) if settings.version == change.version)
        })
        .await
    else {
        unreachable!()
    };
    assert_eq!(device_id, a.id(), "the event names the device that changed");
    assert_eq!(seen, change);
}

#[tokio::test]
async fn a_signed_change_reaches_its_target() {
    let a = PeerBuilder::new("a").start().await;
    let mut b = PeerBuilder::new("b").start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // A changes b's mode and expiry; b sees a NetEvent with the same update.
    let base = announced(&a, b.id()).map(|settings| settings.version);
    let version = base.unwrap_or_else(|| SettingsVersion::initial(b.id())).next(a.id(), UnixMillis::now());
    assert!(a.node().network().send_config_update(b.id(), SyncMode::Send, Some(3600), version).unwrap());

    let NodeEvent::Net(NetEvent::ConfigUpdate { from, update }) =
        b.wait_event(|event| matches!(event, NodeEvent::Net(NetEvent::ConfigUpdate { .. }))).await
    else {
        unreachable!()
    };
    assert_eq!(from, a.id());
    assert_eq!(update.target, b.id());
    assert_eq!(update.mode, SyncMode::Send);
    assert_eq!(update.ttl_secs, Some(3600));
    assert_eq!(update.version, version);
    // What arrived is exactly what a signed over this chain.
    let signed = a.node().network().sign_config_update(b.id(), SyncMode::Send, Some(3600), version).unwrap();
    assert_eq!(signed, update);
}

#[tokio::test]
async fn broadcasts_skip_devices_that_announced_send_only() {
    let a = PeerBuilder::new("a").start().await;
    let b = PeerBuilder::new("b").start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    await_announcement(&a, b.id(), |settings| settings.mode == SyncMode::Both).await;

    // b announces Send: it still sends but receives nothing.
    announce(&b, SyncMode::Send);
    await_announcement(&a, b.id(), |settings| settings.mode == SyncMode::Send).await;
    settle().await;
    a.clipboard.copy("while b only sends");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(b.clipboard.writes().is_empty(), "a sent an item to a device that doesn't receive");
    b.clipboard.copy("b still sends");
    assert_eq!(a.wait_writes(1).await, ["b still sends"], "b's own copies still flow");

    // Back to Both: delivery resumes.
    announce(&b, SyncMode::Both);
    await_announcement(&a, b.id(), |settings| settings.mode == SyncMode::Both).await;
    settle().await;
    a.clipboard.copy("welcome back");
    assert_eq!(b.wait_writes(1).await, ["welcome back"]);
}

#[tokio::test]
async fn forged_or_foreign_updates_are_ignored() {
    let chain = TestChain::new();
    let a = PeerBuilder::new("a").chain(ChainChoice::Member(chain.clone())).start().await;
    let mut b = PeerBuilder::new("b").chain(ChainChoice::Member(chain.clone())).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // An admitted member whose update is signed by a key that isn't its member key.
    let imposter = DeviceId::random();
    chain.admit(imposter, "imposter");
    let wrong_key = pasta_crypto::DeviceKey::generate();
    let mut joined = raw::join(a.addr, imposter, &wrong_key, &chain.keys).await;
    let chain_id = chain.keys.id();

    let make = |target: DeviceId, by: DeviceId, counter: u64, key: &pasta_crypto::DeviceKey| {
        let version = SettingsVersion { counter, at: UnixMillis::now(), by };
        let bytes = signing_bytes(&chain_id.0, target, SyncMode::Paused, None, &version);
        ConfigUpdate { target, mode: SyncMode::Paused, ttl_secs: None, version, signature: key.sign(&bytes) }
    };

    // 1. Signed by a key that isn't the issuer's member key.
    let bad_signature = make(b.id(), imposter, 1, &wrong_key);
    joined.send_sealed(&Inner::ConfigUpdate(bad_signature)).await;
    // 2. Correctly signed by the imposter, but for another device.
    let for_other = make(DeviceId::random(), imposter, 2, &identity(imposter));
    joined.send_sealed(&Inner::ConfigUpdate(for_other)).await;
    settle().await;
    assert!(!saw_update(&b.drain_events()), "b accepted a forged or mistargeted update");

    // 3. Sent by a device that isn't a member: its `encrypted` frames never
    //    reach ConfigUpdate handling.
    let stranger = DeviceId::random();
    let stranger_key = pasta_crypto::DeviceKey::generate();
    let mut unknown = raw::join(b.addr, stranger, &stranger_key, &chain.keys).await;
    // Signed with a's real key over the real chain, so only membership can save b.
    let forged = make(b.id(), a.id(), 3, &identity(a.id()));
    unknown.send_sealed(&Inner::ConfigUpdate(forged)).await;
    settle().await;
    assert!(!saw_update(&b.drain_events()), "b took an update from a non-member");
}

#[tokio::test]
async fn a_same_version_announcement_with_different_fields_is_ignored() {
    // Deterministic ids, so the raw takeover dial always wins the duplicate-
    // connection rule (`initiator == me.min(peer_id)`): the raw client dials
    // with a_id < b_id, so b (the listener) keeps the raw connection and drops
    // the real a's. Random ids made this a coin flip that lost half the runs.
    let a_id = DeviceId(Uuid::from_u64_pair(1, 1));
    let b_id = DeviceId(Uuid::from_u64_pair(2, 2));
    let a = PeerBuilder::new("a").device_id(a_id).start().await;
    let mut b = PeerBuilder::new("b").device_id(b_id).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // b first learns a's real settings (Both, never expiring) on connect.
    await_announcement(&b, a.id(), |settings| settings.mode == SyncMode::Both).await;
    let known = announced(&b, a.id()).unwrap();
    b.drain_events();

    // Take over a's connection from a second socket speaking as a (its key,
    // its chain), and stop the real a from redialing, so the frames below are
    // the only ones that arrive.
    let chain = TestChain::shared();
    let mut as_a = raw::join(b.addr, a.id(), &identity(a.id()), &chain.keys).await;
    a.discovery.set_peers(Vec::new());

    // First a repeat of the known version with different fields — a replay or
    // a buggy peer — then a genuinely newer announcement. Both travel on one
    // connection in order, so once the newer one lands, the same-version frame
    // was processed too.
    let tampered = DeviceSettings { mode: SyncMode::Send, ttl_secs: Some(60), ..known };
    as_a.send_sealed(&Inner::Settings(tampered)).await;
    let newer = DeviceSettings {
        mode: SyncMode::Both,
        ttl_secs: None,
        paused_until: None,
        version: known.version.next(a.id(), UnixMillis::now()),
    };
    as_a.send_sealed(&Inner::Settings(newer)).await;
    await_announcement(&b, a.id(), |settings| settings.version == newer.version).await;

    // The same-version frame changed nothing: no event, no stored settings.
    assert_eq!(announced(&b, a.id()), Some(newer), "b took the newer announcement");
    assert!(
        !b.drain_events().iter().any(
            |event| matches!(event, NodeEvent::Net(NetEvent::PeerSettings { settings, .. }) if *settings == tampered)
        ),
        "b surfaced a settings change that wasn't one"
    );
    let row_settings = {
        let peers = b.peers();
        peers.borrow().iter().find(|row| row.device_id == a.id()).and_then(|row| row.settings)
    };
    assert_eq!(row_settings, Some(newer), "the devices list took the phantom settings");
}

#[tokio::test]
async fn a_send_only_announcement_does_not_survive_a_redone_connection() {
    // b announces Send on its connection to a, so a stops sending items to it.
    // The connection is then replaced: a raw client dials a as b, from the same
    // side as b's existing connection, so a's duplicate rule (`existing.initiator
    // == initiator`) deterministically keeps the new one and closes the old via
    // the actor's `close_conn` path — the same path `set_chain` and a reconnect
    // take. The raw client never announces, so "b hasn't announced on the new
    // connection" holds by construction, and whatever a broadcasts next must
    // reach it: the announcement from the old connection must not stick.
    let chain = TestChain::shared();
    // Fixed ids with b < a: the duplicate rule then resolves both the initial
    // mesh and the takeover to b-initiated connections, deterministically.
    let b_id = DeviceId(Uuid::from_u64_pair(3, 3));
    let a_id = DeviceId(Uuid::from_u64_pair(4, 4));
    let send = DeviceSettings {
        mode: SyncMode::Send,
        ttl_secs: None,
        paused_until: None,
        version: SettingsVersion::initial(b_id).next(b_id, UnixMillis(1)),
    };
    let a = PeerBuilder::new("a").device_id(a_id).start().await;
    let b = PeerBuilder::new("b").device_id(b_id).announces(send).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    await_announcement(&a, b_id, |settings| settings.mode == SyncMode::Send).await;

    // a sends nothing while b announced Send on this connection. The sleep also
    // lets the copy clear the engine's debounce before the connection is redone.
    a.clipboard.copy("held back");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(b.clipboard.writes().is_empty(), "a sent to a device that announced Send");

    // Park the real b before the takeover, so it neither re-dials nor announces
    // again once its connection is replaced.
    b.discovery.set_peers(Vec::new());
    settle().await;

    // Take over b's connection from a second socket speaking as b (its key, its
    // chain). Same-side dial, so a's duplicate rule closes the old connection
    // through `close_conn` and keeps the new one.
    let mut as_b = raw::join(a.addr, b_id, &identity(b_id), &chain.keys).await;
    a.wait_connected(1).await;

    // The announcement died with the old connection: a's next copy must reach
    // the new one. a also sends its member list and settings here; skip those
    // and take the first item.
    a.clipboard.copy("flows again");
    let clip = within(async {
        loop {
            match raw::next(&mut as_b.ws).await {
                raw::Incoming::Message(envelope) => {
                    if envelope.message.kind() == "encrypted"
                        && let Ok(Inner::Clip(clip)) = as_b.chain.open_envelope(&envelope)
                    {
                        return clip;
                    }
                }
                raw::Incoming::Close(_) | raw::Incoming::Gone => panic!("the raw connection closed"),
            }
        }
    })
    .await;
    assert_eq!(clip.flavors.plain, "flows again", "the item reached the replaced connection");
}

#[tokio::test]
async fn a_stale_stored_send_only_peer_still_receives_until_it_announces() {
    // b restores, from its store, an announcement in which a was Send-only —
    // left over from an earlier connection (#31). Stored values may be stale,
    // so the sender side of enforcement must not trust one: only an
    // announcement on the current connection may stop items.
    //
    // a's fresh announcement never lands here: it carries an invalid expiry, so
    // it is dropped before it can mark a as having announced — the same state
    // as "its announcement hasn't arrived yet".
    let a_id = DeviceId::random();
    let stale = DeviceSettings {
        mode: SyncMode::Send,
        ttl_secs: None,
        paused_until: None,
        version: SettingsVersion::initial(a_id).next(a_id, UnixMillis(1)),
    };
    let never_lands = DeviceSettings {
        mode: SyncMode::Both,
        ttl_secs: Some(0),
        paused_until: None,
        version: SettingsVersion::initial(a_id),
    };
    let a = PeerBuilder::new("a").device_id(a_id).announces(never_lands).start().await;
    let b = PeerBuilder::new("b").stored_peer_settings([(a_id, stale)]).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    settle().await;
    assert_eq!(announced(&b, a_id), Some(stale), "b should still hold the stored settings");

    // b sends anyway: a hasn't announced Send on this connection.
    b.clipboard.copy("sent despite the stale stored Send");
    assert_eq!(a.wait_writes(1).await, ["sent despite the stale stored Send"]);
}
