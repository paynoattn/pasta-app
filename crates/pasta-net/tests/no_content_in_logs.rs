//! #18: clipboard text never reaches the logs, at any level.
//!
//! Runs a sync with secret text through every path that logs (send, receive,
//! apply, oversize, invalid, echo) while capturing all `tracing` output at
//! TRACE, then checks the secret isn't in it.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::raw;
use common::{ChainChoice, Peer, PeerBuilder, TestChain, identity, mesh, settle};
use pasta_core::clip::Flavors;
use pasta_core::device::DeviceId;
use pasta_core::protocol::Message;
use pasta_crypto::DeviceKey;

const SECRET: &str = "s3cr3t-clipboard-canary";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// A current-thread runtime keeps every task on this thread, inside the subscriber's scope.
#[tokio::test(flavor = "current_thread")]
async fn clipboard_text_never_appears_in_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let a = Peer::start("a").await;
    let b = PeerBuilder::new("b").max_item_bytes(4096).start().await;
    mesh(&[&a, &b]).await;
    a.wait_connected(1).await;
    b.wait_connected(1).await;

    a.clipboard.copy_flavors(Flavors {
        plain: format!("{SECRET} plain"),
        html: Some(format!("<p>{SECRET}</p>")),
        rtf: Some(format!("{{\\rtf1 {SECRET}}}")),
    });
    b.wait_writes(1).await;
    // Echo of the received item, a duplicate copy, and one too big for B.
    b.clipboard.copy(&format!("{SECRET} plain"));
    a.clipboard.copy(&format!("{SECRET} plain"));
    a.clipboard.copy(&format!("{SECRET} {}", "x".repeat(8192)));
    settle().await;
    b.clipboard.copy(&format!("{SECRET} reply"));
    a.wait_writes(1).await;
    settle().await;

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("sent local copy"), "logging wasn't captured:\n{logs}");
    assert!(logs.contains("applied item from peer"), "logging wasn't captured:\n{logs}");
    assert!(!logs.contains(SECRET), "clipboard text leaked into the logs:\n{logs}");
}

/// The paths that exist to refuse content: a sealed message that was damaged
/// or replayed, a plaintext item, items from a device that is pending or was
/// removed, and a removed device copying locally. Each must log what happened
/// (ids, errors) and nothing of the text.
#[tokio::test(flavor = "current_thread")]
async fn refused_content_never_appears_in_logs() {
    let capture = Capture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let chain = TestChain::new();
    let member = |chain: &Arc<TestChain>| ChainChoice::Member(Arc::clone(chain));
    let a = PeerBuilder::new("macbook-pro").chain(member(&chain)).start().await;
    let c = PeerBuilder::new("win-workstation").chain(member(&chain)).start().await;
    // Two hand-made members that `a` learns about, and a device that only knows the words.
    let (member_id, removed_id, pending_id) = (DeviceId::random(), DeviceId::random(), DeviceId::random());
    chain.admit(member_id, "cachy-desktop");
    chain.admit(removed_id, "cachy-laptop");
    mesh(&[&a, &c]).await;
    a.wait_connected(1).await;

    let mut honest = raw::join(a.addr, member_id, &identity(member_id), &chain.keys).await;
    a.wait_connected(2).await;
    let mut pending = raw::join(a.addr, pending_id, &DeviceKey::generate(), &chain.keys).await;
    a.wait_connected(3).await;
    a.wait_pending(1).await;
    let mut removed = raw::join(a.addr, removed_id, &identity(removed_id), &chain.keys).await;
    a.wait_connected(4).await;

    // A sealed item with one damaged byte, then a sound one sent twice.
    let mut damaged = honest.seal(&raw::clip(member_id, &format!("{SECRET} damaged")));
    if let Message::Encrypted(sealed) = &mut damaged.message {
        sealed.ciphertext.0[3] ^= 1;
    }
    raw::send_envelope(&mut honest.ws, &damaged).await;
    let sound = honest.send_sealed(&raw::clip(member_id, &format!("{SECRET} sound"))).await;
    raw::send_envelope(&mut honest.ws, &sound).await;
    // A plaintext item in a frame the protocol doesn't allow.
    raw::send_text(&mut honest.ws, raw::plaintext_clip_frame(member_id, &format!("{SECRET} plaintext"))).await;
    // A device that isn't approved yet sends a properly sealed item.
    pending.send_sealed(&raw::clip(pending_id, &format!("{SECRET} from a pending device"))).await;
    settle().await;
    assert_eq!(a.clipboard.writes().len(), 1, "only the sound item from the member should have been applied");

    // A member is removed and keeps talking on its open connection.
    a.node().network().remove(removed_id).await.unwrap();
    removed.send_sealed(&raw::clip(removed_id, &format!("{SECRET} from a removed device"))).await;
    // A real peer is removed, learns of it, and its next copy stays on the device.
    a.node().network().remove(c.id()).await.unwrap();
    c.wait_left().await;
    c.clipboard.copy(&format!("{SECRET} after being removed"));
    settle().await;
    assert_eq!(a.clipboard.writes().len(), 1, "something from a removed or pending device was applied");

    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    let logged = |needle: &str, id: Option<DeviceId>| {
        logs.lines().any(|line| line.contains(needle) && id.is_none_or(|id| line.contains(&id.to_string())))
    };
    assert!(logged("dropped an encrypted message", Some(member_id)), "no line for the damaged item:\n{logs}");
    assert!(logged("replay", Some(member_id)), "no line for the replayed item:\n{logs}");
    assert!(logged("dropped an invalid message", Some(member_id)), "no line for the plaintext item:\n{logs}");
    for id in [pending_id, removed_id] {
        assert!(logged("isn't an approved member", Some(id)), "no refusal line for {id}:\n{logs}");
    }
    assert!(logged("this device was removed from the chain", None), "no line for the removal:\n{logs}");
    assert!(logged("item stays here", None), "no line for the removed device's copy:\n{logs}");
    assert!(!logs.contains(SECRET), "clipboard text leaked into the logs:\n{logs}");
}
