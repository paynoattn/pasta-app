//! MVP integration tests (#19): real nodes on loopback with fake clipboards.

mod common;

use std::time::Duration;

use common::{LOOPBACK_NODE, Peer, PeerBuilder, mesh, settle, within};
use futures_util::{SinkExt, StreamExt};
use pasta_core::clip::Flavors;
use pasta_core::device::DeviceInfo;
use pasta_core::engine::EngineEvent;
use pasta_core::protocol::{Envelope, Hello, Message};
use pasta_net::network::NetEvent;
use pasta_net::node::NodeEvent;
use pasta_net::transport::close;
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn sent_count(events: &[NodeEvent]) -> usize {
    events.iter().filter(|event| matches!(event, NodeEvent::Engine(EngineEvent::Sent { .. }))).count()
}

#[tokio::test]
async fn copies_reach_the_other_peer() {
    let a = Peer::start("a").await;
    let b = Peer::start("b").await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    a.clipboard.copy_flavors(Flavors { plain: "hello".into(), html: Some("<b>hello</b>".into()), rtf: None });
    let writes = within(async {
        loop {
            if let Some(flavors) = b.clipboard.writes().pop() {
                return flavors;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(writes.plain, "hello");
    assert_eq!(writes.html.as_deref(), Some("<b>hello</b>"), "rich flavors travel along");

    b.clipboard.copy("world");
    assert_eq!(a.wait_writes(1).await, ["world"]);
}

#[tokio::test]
async fn alternating_copies_arrive_exactly_once_without_ping_pong() {
    let mut a = Peer::start("a").await;
    let mut b = Peer::start("b").await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // Fake clipboards report every write back as a copy, the worst case for echoes.
    const COPIES: usize = 30;
    let (mut to_a, mut to_b) = (Vec::new(), Vec::new());
    for i in 0..COPIES {
        let text = format!("item {i}");
        if i % 2 == 0 {
            a.clipboard.copy(&text);
            to_b.push(text);
            b.wait_writes(to_b.len()).await;
        } else {
            b.clipboard.copy(&text);
            to_a.push(text);
            a.wait_writes(to_a.len()).await;
        }
    }
    settle().await;
    assert_eq!(b.wait_writes(0).await, to_b);
    assert_eq!(a.wait_writes(0).await, to_a);
    let sent = sent_count(&a.drain_events()) + sent_count(&b.drain_events());
    assert_eq!(sent, COPIES, "something was broadcast twice or echoed back");
}

#[tokio::test]
async fn three_peers_each_get_a_copy_once() {
    let a = Peer::start("a").await;
    let b = Peer::start("b").await;
    let c = Peer::start("c").await;
    mesh(&[&a, &b, &c]).await;
    for peer in [&a, &b, &c] {
        peer.wait_connected(2).await;
    }
    a.clipboard.copy("to everyone");
    assert_eq!(b.wait_writes(1).await, ["to everyone"]);
    assert_eq!(c.wait_writes(1).await, ["to everyone"]);
    settle().await;
    assert_eq!(b.clipboard.writes().len(), 1);
    assert_eq!(c.clipboard.writes().len(), 1);
    assert!(a.clipboard.writes().is_empty(), "the sender got its own item back");
}

#[tokio::test]
async fn simultaneous_dials_settle_on_one_connection() {
    let mut a = Peer::start("a").await;
    let mut b = Peer::start("b").await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    // After the tie-break, nothing keeps churning.
    a.drain_events();
    b.drain_events();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let churn = |events: Vec<NodeEvent>| {
        events.iter().filter(|event| matches!(event, NodeEvent::Net(NetEvent::Connected { .. }))).count()
    };
    assert_eq!(churn(a.drain_events()) + churn(b.drain_events()), 0);
    a.wait_connected(1).await;
}

#[tokio::test]
async fn reconnects_after_a_peer_restarts() {
    let a = Peer::start("a").await;
    let mut b = Peer::start("b").await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;

    let id = b.id();
    b.stop().await;
    a.wait_connected(0).await;

    // Same device, new process and port.
    let b = PeerBuilder::new("b").device_id(id).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;
    a.clipboard.copy("after restart");
    assert_eq!(b.wait_writes(1).await, ["after restart"]);
}

#[tokio::test]
async fn a_peer_dropping_mid_send_does_not_affect_the_others() {
    let a = Peer::start("a").await;
    let b = Peer::start("b").await;
    let mut c = Peer::start("c").await;
    mesh(&[&a, &b, &c]).await;
    a.wait_connected(2).await;

    a.clipboard.copy("first");
    c.stop().await;
    assert_eq!(b.wait_writes(1).await, ["first"]);
    a.wait_connected(1).await;
    a.clipboard.copy("second");
    assert_eq!(b.wait_writes(2).await, ["first", "second"]);
}

#[tokio::test]
async fn oversize_items_are_not_sent_or_applied() {
    let mut a = Peer::start("a").await;
    let b = PeerBuilder::new("b").max_item_bytes(1024).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    // Over A's own limit: never sent.
    a.settings.send_modify(|settings| settings.max_item_bytes = 4096);
    a.clipboard.copy(&"x".repeat(5000));
    settle().await;
    assert_eq!(sent_count(&a.drain_events()), 0);

    // Within A's limit but over B's: B drops it and stays connected.
    a.clipboard.copy(&"y".repeat(2000));
    settle().await;
    assert!(b.clipboard.writes().is_empty());
    a.clipboard.copy("small");
    assert_eq!(b.wait_writes(1).await, ["small"]);
    b.wait_connected(1).await;
}

#[tokio::test]
async fn refuses_other_protocol_versions() {
    let mut a = Peer::start("a").await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{}/pasta", a.addr)).await.unwrap();
    let device = DeviceInfo {
        id: pasta_core::device::DeviceId::random(),
        name: "future".into(),
        os: "test".into(),
        os_version: "1".into(),
        app_version: "9.0.0".into(),
    };
    let hello = Hello::new(&device, pasta_core::wire::Bytes([1; 32]), pasta_core::wire::Bytes([2; 32]));
    let mut hello: serde_json::Value =
        serde_json::from_str(&Envelope::new(device.id, Message::Hello(hello)).encode()).unwrap();
    hello["v"] = 3.into();
    ws.send(WsMessage::text(hello.to_string())).await.unwrap();

    let reply = within(ws.next()).await.unwrap().unwrap();
    let WsMessage::Close(Some(frame)) = reply else { panic!("expected a close frame, got {reply:?}") };
    assert_eq!(u16::from(frame.code), close::UNSUPPORTED_VERSION);
    settle().await;
    assert!(a.drain_events().iter().any(|event| matches!(event, NodeEvent::Net(NetEvent::Rejected { .. }))));
    assert!(a.peers().borrow().is_empty());
}

#[tokio::test]
async fn refuses_a_device_id_seen_on_another_node() {
    let b = Peer::start("b").await;
    // A remembers B's device id from a different tailnet node.
    let mut a = PeerBuilder::new("a").known(b.id(), "nSomewhereElseCNTRL").start().await;
    assert_ne!(LOOPBACK_NODE, "nSomewhereElseCNTRL");
    mesh(&[&a, &b]).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let events = a.drain_events();
    assert!(events.iter().any(|event| matches!(event, NodeEvent::Net(NetEvent::Rejected { .. }))));
    assert!(!events.iter().any(|event| matches!(event, NodeEvent::Net(NetEvent::Connected { .. }))));
    b.clipboard.copy("should not arrive");
    settle().await;
    assert!(a.clipboard.writes().is_empty());
}

#[tokio::test]
async fn refuses_addresses_that_are_not_tailnet_nodes() {
    // The test verifier vouches for 127.0.0.1 only, so 127.0.0.2 stands in for a
    // non-tailnet address. It is refused before anything is sent to it.
    let mut a = Peer::start("a").await;
    let unknown = PeerBuilder::new("x").start().await;
    a.discovery.set_peers(vec![pasta_net::discovery::Candidate {
        addr: std::net::SocketAddr::new("127.0.0.2".parse().unwrap(), unknown.addr.port()),
        ..unknown.candidate()
    }]);
    settle().await;
    assert!(a.drain_events().iter().any(|event| matches!(event, NodeEvent::Net(NetEvent::Rejected { .. }))));
    assert!(a.peers().borrow().is_empty());
}
