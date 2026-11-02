//! End-to-end encryption on the wire and the handshake's defences (#21, #22).
//!
//! A recording proxy stands in for "someone on the network" (as if WireGuard
//! were removed), and a hand-written WebSocket client plays impostors.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::proxy::Proxy;
use common::raw::{self, Incoming, Joined};
use common::{ChainChoice, Peer, PeerBuilder, TestChain, dial_via, identity, settle, sync_members};
use pasta_core::chain::{MAX_EVENTS, MAX_FIELD_BYTES};
use pasta_core::clip::Flavors;
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::protocol::{Auth, Hello, Inner, MAX_HELLO_FIELD_BYTES, Message, Ping, Role, handshake_transcript};
use pasta_core::wire::Bytes;
use pasta_crypto::membership::Candidate;
use pasta_crypto::{ChainKeys, DeviceKey, random_key32};
use pasta_net::network::NetEvent;
use pasta_net::node::NodeEvent;
use pasta_net::transport::close;
use uuid::Uuid;

const CANARY: &str = "s3cr3t-clipboard-canary";

fn member(chain: &Arc<TestChain>) -> ChainChoice {
    ChainChoice::Member(Arc::clone(chain))
}

/// Rewrites a decoded frame in place.
type Mutation = fn(&mut serde_json::Value);

fn is_encrypted(frame: &str) -> bool {
    frame.contains(r#""type":"encrypted""#)
}

fn connected_events(events: &[NodeEvent]) -> usize {
    events.iter().filter(|event| matches!(event, NodeEvent::Net(NetEvent::Connected { .. }))).count()
}

fn disconnected_events(events: &[NodeEvent]) -> usize {
    events.iter().filter(|event| matches!(event, NodeEvent::Net(NetEvent::Disconnected { .. }))).count()
}

/// Two members of one chain; `a` dials `b` through a recording proxy.
async fn pair_through_proxy() -> (Peer, Peer, Proxy) {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("win-workstation").chain(member(&chain)).start().await;
    sync_members(&[&a, &b]).await;
    let proxy = Proxy::start(b.addr).await;
    dial_via(&a, proxy.addr, &b);
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    // The member lists have crossed by now.
    settle().await;
    (a, b, proxy)
}

/// Rewrites the next `encrypted` frame the listener sends, and only that one.
/// The flag turns true once it has been rewritten.
fn tamper_next(proxy: &Proxy, mutate: impl Fn(&mut serde_json::Value) + Send + 'static) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    proxy.set_tamper(move |frame| {
        if is_encrypted(frame) && !flag.swap(true, Ordering::SeqCst) {
            let mut value: serde_json::Value = serde_json::from_str(frame).unwrap();
            mutate(&mut value);
            return vec![value.to_string()];
        }
        vec![frame.to_owned()]
    });
    done
}

/// Changes one base64 digit in the middle of a body field.
fn flip_body_field(value: &mut serde_json::Value, field: &str) {
    let mut chars: Vec<char> = value["body"][field].as_str().unwrap().chars().collect();
    let middle = chars.len() / 2;
    chars[middle] = if chars[middle] == 'A' { 'B' } else { 'A' };
    value["body"][field] = chars.into_iter().collect::<String>().into();
}

// ---------------------------------------------------------------------------
// #21: nothing readable on the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_capture_of_the_connection_shows_no_clipboard_text() {
    let (a, b, proxy) = pair_through_proxy().await;

    // A copy on each side, with every flavor carrying the canary.
    a.clipboard.copy_flavors(Flavors {
        plain: format!("{CANARY} from a"),
        html: Some(format!("<p>{CANARY} from a</p>")),
        rtf: Some(format!("{{\\rtf1 {CANARY} from a}}")),
    });
    assert_eq!(b.wait_writes(1).await, [format!("{CANARY} from a")]);
    b.clipboard.copy_flavors(Flavors {
        plain: format!("{CANARY} from b"),
        html: Some(format!("<p>{CANARY} from b</p>")),
        rtf: None,
    });
    assert_eq!(a.wait_writes(1).await, [format!("{CANARY} from b")]);
    settle().await;

    proxy.with_capture(|capture| {
        // Not vacuous: encrypted messages did cross, in both directions.
        let up = capture.frames_up.iter().filter(|frame| is_encrypted(frame)).count();
        let down = capture.frames_down.iter().filter(|frame| is_encrypted(frame)).count();
        assert!(
            up >= 1,
            "no encrypted frame went from the dialer to the listener ({} frames)",
            capture.frames_up.len()
        );
        assert!(down >= 1, "no encrypted frame went from the listener to the dialer");
        assert!(capture.raw_down.windows(18).any(|w| w == br#""type":"encrypted""#), "unmasked bytes show the type");
        assert!(capture.raw_up.len() > 100 && capture.raw_down.len() > 100);

        // The canary is nowhere: not in the raw bytes, not in any decoded frame.
        assert!(!capture.raw_contains(CANARY), "clipboard text on the wire");
        assert!(!capture.frames_contain(CANARY), "clipboard text in a WebSocket frame");
        assert!(!capture.frames_contain(r#""type":"clip""#), "a plaintext clip message");
        assert!(!capture.frames_contain("text/plain") && !capture.frames_contain("text/html"));
        // Only the handshake and heartbeat are readable; the items are not.
        for frame in capture.frames_up.iter().chain(&capture.frames_down) {
            let value: serde_json::Value = serde_json::from_str(frame).unwrap();
            let kind = value["type"].as_str().unwrap();
            assert!(["hello", "hello_ack", "auth", "encrypted", "ack", "ping", "pong"].contains(&kind), "{kind}");
        }
    });
}

#[tokio::test]
async fn a_duplicated_frame_is_applied_once_and_the_connection_survives() {
    let (mut a, b, proxy) = pair_through_proxy().await;
    a.drain_events();
    let duplicated = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&duplicated);
    proxy.set_tamper(move |frame| {
        if is_encrypted(frame) {
            counter.fetch_add(1, Ordering::SeqCst);
            return vec![frame.to_owned(), frame.to_owned()];
        }
        vec![frame.to_owned()]
    });

    b.clipboard.copy(&format!("{CANARY} once"));
    assert_eq!(a.wait_writes(1).await, [format!("{CANARY} once")]);
    settle().await;
    assert_eq!(a.clipboard.writes().len(), 1, "the replayed frame was applied again");
    assert!(duplicated.load(Ordering::SeqCst) >= 1, "the proxy never sent a duplicate");

    // Still connected, on the same TCP connection, and later items still work.
    a.wait_connected(1).await;
    b.clipboard.copy("a later item");
    assert_eq!(a.wait_writes(2).await, [format!("{CANARY} once"), "a later item".to_owned()]);
    settle().await;
    assert_eq!(a.clipboard.writes().len(), 2);
    assert_eq!(proxy.connections(), 1, "the dialer had to reconnect");
    assert_eq!(disconnected_events(&a.drain_events()), 0);
}

#[tokio::test]
async fn a_flipped_ciphertext_byte_drops_that_message_only() {
    let (mut a, b, proxy) = pair_through_proxy().await;
    a.drain_events();
    let tampered = tamper_next(&proxy, |value| flip_body_field(value, "ciphertext"));

    b.clipboard.copy(&format!("{CANARY} damaged in transit"));
    settle().await;
    assert!(tampered.load(Ordering::SeqCst), "the proxy never saw the item");
    assert!(a.clipboard.writes().is_empty(), "a tampered message was applied");
    a.wait_connected(1).await;

    b.clipboard.copy("an intact item");
    assert_eq!(a.wait_writes(1).await, ["an intact item"]);
    assert_eq!(proxy.connections(), 1, "tampering made the dialer reconnect");
    assert_eq!(disconnected_events(&a.drain_events()), 0);
}

#[tokio::test]
async fn a_changed_nonce_or_header_fails_to_decrypt_too() {
    let (a, b, proxy) = pair_through_proxy().await;
    let mutations: [(&str, Mutation); 3] = [
        ("nonce", |value| flip_body_field(value, "nonce")),
        // The header is the AAD: moving the timestamp, id or sender breaks the tag.
        ("sent_at", |value| {
            let at = value["sent_at"].as_u64().unwrap();
            value["sent_at"] = (at + 1).into();
        }),
        ("msg_id", |value| value["msg_id"] = uuid_other(value["msg_id"].as_str().unwrap())),
    ];
    let mut expected: Vec<String> = Vec::new();
    for (label, mutate) in mutations {
        let tampered = tamper_next(&proxy, mutate);
        b.clipboard.copy(&format!("{CANARY} changed {label}"));
        settle().await;
        assert!(tampered.load(Ordering::SeqCst), "{label}: the proxy never saw the item");
        assert_eq!(a.clipboard.writes().len(), expected.len(), "{label}: a tampered message was applied");
        a.wait_connected(1).await;

        expected.push(format!("intact after {label}"));
        b.clipboard.copy(expected.last().unwrap());
        assert_eq!(a.wait_writes(expected.len()).await, expected);
    }
    assert_eq!(proxy.connections(), 1);
}

/// A different, valid UUID.
fn uuid_other(original: &str) -> serde_json::Value {
    let mut chars: Vec<char> = original.chars().collect();
    chars[0] = if chars[0] == 'a' { 'b' } else { 'a' };
    chars.into_iter().collect::<String>().into()
}

// ---------------------------------------------------------------------------
// Hand-written clients: replays, plaintext, skipped authentication
// ---------------------------------------------------------------------------

/// A listener plus a chain in which `raw_id` is an approved member whose key
/// is `identity(raw_id)`, so a raw client can do everything a member can.
async fn listener_with_raw_member() -> (Peer, Arc<TestChain>, DeviceId) {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let raw_id = DeviceId::random();
    chain.admit(raw_id, "cachy-desktop");
    sync_members(&[&a]).await;
    (a, chain, raw_id)
}

#[tokio::test]
async fn a_member_resending_the_same_frame_is_applied_once() {
    let (a, chain, id) = listener_with_raw_member().await;
    let mut raw = raw::join(a.addr, id, &identity(id), &chain.keys).await;
    a.wait_connected(1).await;

    let item = raw::clip(id, &format!("{CANARY} sent twice"));
    let envelope = raw.send_sealed(&item).await;
    raw::send_envelope(&mut raw.ws, &envelope).await;
    assert_eq!(a.wait_writes(1).await, [format!("{CANARY} sent twice")]);
    settle().await;
    assert_eq!(a.clipboard.writes().len(), 1, "the replay was applied");

    raw.send_sealed(&raw::clip(id, "a different item")).await;
    assert_eq!(a.wait_writes(2).await, [format!("{CANARY} sent twice"), "a different item".to_owned()]);
    a.wait_connected(1).await;
}

#[tokio::test]
async fn a_plaintext_clip_is_refused_even_from_a_member() {
    let (a, chain, id) = listener_with_raw_member().await;
    let mut raw = raw::join(a.addr, id, &identity(id), &chain.keys).await;
    a.wait_connected(1).await;

    raw::send_text(&mut raw.ws, raw::plaintext_clip_frame(id, CANARY)).await;
    settle().await;
    assert!(a.clipboard.writes().is_empty(), "a plaintext item was applied");
    a.wait_connected(1).await;

    // The same member can still send the item sealed.
    raw.send_sealed(&raw::clip(id, "now sealed")).await;
    assert_eq!(a.wait_writes(1).await, ["now sealed"]);
}

#[tokio::test]
async fn a_member_skipping_auth_and_sending_an_item_is_closed_and_ignored() {
    let (mut a, chain, id) = listener_with_raw_member().await;
    let key = identity(id);
    let mut ws = raw::open(a.addr).await;
    let ours = raw::hello(id, &key, Some(&chain.keys));
    raw::send(&mut ws, id, Message::Hello(ours)).await;
    let Incoming::Message(ack) = raw::next(&mut ws).await else { panic!("expected hello_ack") };
    assert_eq!(ack.message.kind(), "hello_ack");

    // Straight to the item, with the right chain key and an approved identity.
    let envelope = chain.keys.seal_envelope(id, &raw::clip(id, &format!("{CANARY} without auth")));
    raw::send_envelope(&mut ws, &envelope).await;
    let seen = raw::until_closed(&mut ws).await;
    assert_eq!(seen.close, Some(close::PROTOCOL_ERROR));
    assert!(seen.kinds.is_empty(), "the unauthenticated client was sent {:?}", seen.kinds);
    settle().await;
    assert!(a.clipboard.writes().is_empty(), "an item from an unauthenticated connection was applied");
    assert_eq!(connected_events(&a.drain_events()), 0);
    assert!(a.peers().borrow().is_empty());
}

// ---------------------------------------------------------------------------
// Impersonation
// ---------------------------------------------------------------------------

/// A listener that knows `b` as a member, and `b` itself (which dials nobody).
async fn listener_and_member() -> (Peer, Peer, Arc<TestChain>) {
    let chain = TestChain::new();
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let b = PeerBuilder::new("win-workstation").chain(member(&chain)).start().await;
    sync_members(&[&a, &b]).await;
    assert!(a.member_ids().contains(&b.id()));
    (a, b, chain)
}

#[tokio::test]
async fn a_members_id_with_another_key_is_refused_even_with_a_valid_chain_proof() {
    let (mut a, b, chain) = listener_and_member().await;
    let impostor = DeviceKey::generate();
    assert_ne!(impostor.public(), b.key.public());

    let mut ws = raw::open(a.addr).await;
    // The impostor knows the words, so its proof checks out: only the key gives it away.
    let hello = raw::hello(b.id(), &impostor, Some(&chain.keys));
    assert!(chain.keys.verify_chain_proof(
        b.id().0.as_bytes(),
        &hello.device_key,
        &hello.nonce,
        hello.chain_proof.as_ref().unwrap()
    ));
    raw::send(&mut ws, b.id(), Message::Hello(hello)).await;

    let seen = raw::until_closed(&mut ws).await;
    assert_eq!(seen.close, Some(close::NOT_TRUSTED));
    assert!(seen.kinds.is_empty(), "the impostor was sent {:?}", seen.kinds);
    settle().await;
    let events = a.drain_events();
    assert_eq!(connected_events(&events), 0);
    let reasons: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            NodeEvent::Net(NetEvent::Rejected { reason, .. }) => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(reasons.iter().any(|reason| reason.contains("different key")), "{reasons:?}");
    assert!(a.peers().borrow().is_empty());
    assert!(a.pending_ids().is_empty());

    // The real device is unaffected.
    assert!(a.member_ids().contains(&b.id()));
}

#[tokio::test]
async fn a_valid_hello_followed_by_a_bad_auth_never_authenticates() {
    let (mut a, _b, chain) = listener_and_member().await;
    let id = DeviceId::random();
    let key = DeviceKey::generate();
    let other = DeviceKey::generate();

    for case in 0..5 {
        let mut ws = raw::open(a.addr).await;
        let ours = raw::hello(id, &key, Some(&chain.keys));
        raw::send(&mut ws, id, Message::Hello(ours.clone())).await;
        let Incoming::Message(reply) = raw::next(&mut ws).await else { panic!("expected hello_ack") };
        let Message::HelloAck(theirs) = reply.message else { panic!("expected hello_ack") };

        // While the handshake is half done, nothing is sent to this connection.
        a.clipboard.copy(&format!("{CANARY} for a half-open connection {case}"));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let old = raw::hello(id, &key, Some(&chain.keys));
        let (label, signature) = [
            ("all zeros", Bytes([0; 64])),
            ("garbage", Bytes([7; 64])),
            ("another device's signature", other.sign(&handshake_transcript(Role::Dialer, &ours, &theirs))),
            ("signed as the listener", key.sign(&handshake_transcript(Role::Listener, &ours, &theirs))),
            ("an old connection's signature", key.sign(&handshake_transcript(Role::Dialer, &old, &theirs))),
        ]
        .into_iter()
        .nth(case)
        .unwrap();
        raw::send(&mut ws, id, Message::Auth(Auth { signature })).await;

        let seen = raw::until_closed(&mut ws).await;
        assert_eq!(seen.close, Some(close::NOT_TRUSTED), "{label}");
        assert!(seen.kinds.is_empty(), "{label}: sent {:?} before the auth was checked", seen.kinds);
    }
    settle().await;
    assert_eq!(connected_events(&a.drain_events()), 0, "a failed auth showed up as a connection");
    assert!(a.peers().borrow().is_empty());
    assert!(a.pending_ids().is_empty(), "an unauthenticated device was listed as pending");
}

#[tokio::test]
async fn a_hello_without_a_valid_chain_proof_is_closed_as_another_chain() {
    let (mut a, b, chain) = listener_and_member().await;
    let id = DeviceId::random();
    let key = DeviceKey::generate();
    let strangers = ChainKeys::generate();

    for case in 0..7 {
        let mut hello = raw::hello(id, &key, Some(&chain.keys));
        let label = match case {
            0 => {
                hello.chain_proof = None;
                "no proof"
            }
            1 => {
                hello.chain_proof = Some(Bytes([9; 32]));
                "made-up proof"
            }
            2 => {
                hello.chain_proof = Some(strangers.chain_proof(id.0.as_bytes(), &hello.device_key, &hello.nonce));
                "proof from other words"
            }
            3 => {
                let other_id = DeviceId::random();
                hello.chain_proof =
                    Some(chain.keys.chain_proof(other_id.0.as_bytes(), &hello.device_key, &hello.nonce));
                "proof for another device id"
            }
            4 => {
                let other = DeviceKey::generate().public();
                hello.chain_proof = Some(chain.keys.chain_proof(id.0.as_bytes(), &other, &hello.nonce));
                "proof for another device key"
            }
            5 => {
                hello.chain_proof = Some(chain.keys.chain_proof(id.0.as_bytes(), &hello.device_key, &random_key32()));
                "proof for another nonce (replayed)"
            }
            _ => {
                // A member's id with the right proof for *its* key but this connection's other key.
                hello.device_id = b.id();
                hello.chain_proof = Some(chain.keys.chain_proof(id.0.as_bytes(), &hello.device_key, &hello.nonce));
                "a member's id with someone else's proof"
            }
        };
        let mut ws = raw::open(a.addr).await;
        raw::send(&mut ws, hello.device_id, Message::Hello(hello)).await;
        let seen = raw::until_closed(&mut ws).await;
        assert_eq!(seen.close, Some(close::CHAIN_MISMATCH), "{label}");
        assert!(seen.kinds.is_empty(), "{label}: sent {:?}", seen.kinds);
    }
    settle().await;
    assert_eq!(connected_events(&a.drain_events()), 0);
    assert!(a.peers().borrow().is_empty());
    assert!(a.pending_ids().is_empty(), "devices without the words aren't even pending");
}

// ---------------------------------------------------------------------------
// Size caps (#22)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_hello_with_an_oversized_field_is_closed_and_never_listed() {
    const MAX: usize = MAX_HELLO_FIELD_BYTES;
    let (mut a, chain, id) = listener_with_raw_member().await;
    let key = identity(id);

    type Change = fn(&mut Hello);
    let cases: [(&str, Change); 7] = [
        ("name", |hello| hello.name = "n".repeat(MAX + 1)),
        ("os", |hello| hello.os = "o".repeat(MAX + 1)),
        ("os_version", |hello| hello.os_version = "v".repeat(MAX + 1)),
        ("app_version", |hello| hello.app_version = "v".repeat(MAX + 1)),
        // 65 characters but 130 bytes: the limit counts bytes.
        ("a multi-byte name", |hello| hello.name = "é".repeat(MAX / 2 + 1)),
        ("33 capabilities", |hello| hello.capabilities = vec!["c".into(); 33]),
        ("a long capability", |hello| hello.capabilities = vec!["c".repeat(MAX + 1)]),
    ];
    for (label, change) in cases {
        let mut hello = raw::hello(id, &key, Some(&chain.keys));
        change(&mut hello);
        let mut ws = raw::open(a.addr).await;
        raw::send(&mut ws, id, Message::Hello(hello)).await;
        let seen = raw::until_closed(&mut ws).await;
        assert_eq!(seen.close, Some(close::PROTOCOL_ERROR), "{label}");
        assert!(seen.kinds.is_empty(), "{label}: sent {:?} to a device with an oversized hello", seen.kinds);
    }
    settle().await;
    let events = a.drain_events();
    assert_eq!(connected_events(&events), 0);
    let refused = events
        .iter()
        .filter(
            |event| matches!(event, NodeEvent::Net(NetEvent::Rejected { reason, .. }) if reason.contains("bad hello")),
        )
        .count();
    assert_eq!(refused, cases.len(), "every oversized hello should be reported as a bad hello");
    assert!(a.peers().borrow().is_empty() && a.pending_ids().is_empty(), "an oversized hello got a peer listed");

    // Exactly at the limit is fine.
    let mut hello = raw::hello(id, &key, Some(&chain.keys));
    hello.name = "n".repeat(MAX);
    hello.os = "o".repeat(MAX);
    hello.os_version = "v".repeat(MAX);
    hello.app_version = "v".repeat(MAX);
    hello.capabilities = vec!["c".repeat(MAX); 32];
    let mut ws = raw::open(a.addr).await;
    raw::send(&mut ws, id, Message::Hello(hello)).await;
    let Incoming::Message(reply) = raw::next(&mut ws).await else { panic!("expected hello_ack") };
    assert_eq!(reply.message.kind(), "hello_ack");
}

/// Did the listener acknowledge `msg_id` within `window`? (It only does for messages it accepted.)
async fn acked(raw: &mut Joined, msg_id: Uuid, window: Duration) -> bool {
    let end = Instant::now() + window;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        raw::send(&mut raw.ws, raw.id, Message::Ping(Ping {})).await;
        match tokio::time::timeout(left.min(Duration::from_millis(100)), raw::next(&mut raw.ws)).await {
            Ok(Incoming::Message(envelope)) => {
                if matches!(&envelope.message, Message::Ack(ack) if ack.msg_id == msg_id) {
                    return true;
                }
            }
            Ok(other) => panic!("the connection ended: {other:?}"),
            Err(_) => {}
        }
    }
    false
}

#[tokio::test]
async fn member_lists_over_the_size_caps_are_dropped_before_anything_looks_at_them() {
    let (a, chain, id) = listener_with_raw_member().await;
    let mut raw = raw::join(a.addr, id, &identity(id), &chain.keys).await;
    a.wait_connected(1).await;
    let signer = identity(id);
    // A list the raw member signed validly: it approves `who` under `name`.
    let approving = |who: DeviceId, name: &str| {
        let mut membership = chain.setup().membership;
        let candidate = Candidate {
            device_id: who,
            device_key: identity(who).public(),
            name: name.into(),
            os: "test".into(),
            node_id: common::LOOPBACK_NODE.into(),
        };
        membership.approve(&signer, id, candidate, UnixMillis::now()).unwrap();
        membership.to_list()
    };
    let window = Duration::from_millis(600);

    // Control: a valid list with a 128-byte name (the limit itself) is accepted.
    let (edge_name, edge_events) = (DeviceId::random(), DeviceId::random());
    let list = approving(edge_name, &"n".repeat(MAX_FIELD_BYTES));
    let sent = raw.send_sealed(&Inner::Members(list)).await;
    assert!(acked(&mut raw, sent.msg_id, window).await, "a list at the name limit was refused");
    assert!(a.member_ids().contains(&edge_name));

    // 1024 events (the limit itself) too. Repeats of one record add no event.
    let mut list = approving(edge_events, "n");
    let filler = list.records[0].clone();
    list.records.resize(MAX_EVENTS - list.revocations.len(), filler);
    assert_eq!(list.records.len() + list.revocations.len(), MAX_EVENTS);
    let sent = raw.send_sealed(&Inner::Members(list)).await;
    assert!(acked(&mut raw, sent.msg_id, window).await, "a list at the event limit was refused");
    assert!(a.member_ids().contains(&edge_events));

    // One byte over in a name: not even acknowledged, and nothing is merged.
    let long_name = DeviceId::random();
    let list = approving(long_name, &"n".repeat(MAX_FIELD_BYTES + 1));
    let sent = raw.send_sealed(&Inner::Members(list)).await;
    assert!(!acked(&mut raw, sent.msg_id, window).await, "a list with a 129-byte name was accepted");
    // One event over.
    let too_many = DeviceId::random();
    let mut list = approving(too_many, "n");
    let filler = list.records[0].clone();
    list.records.resize(MAX_EVENTS + 1 - list.revocations.len(), filler);
    let sent = raw.send_sealed(&Inner::Members(list)).await;
    assert!(!acked(&mut raw, sent.msg_id, window).await, "a list with 1025 events was accepted");

    settle().await;
    assert!(!a.member_ids().contains(&long_name) && !a.member_ids().contains(&too_many));
    // Dropping a message doesn't drop the connection, and the device still works.
    a.wait_connected(1).await;
    let sent = raw.send_sealed(&raw::clip(id, "still a member")).await;
    assert!(acked(&mut raw, sent.msg_id, window).await);
    assert_eq!(a.wait_writes(1).await, ["still a member"]);
}
