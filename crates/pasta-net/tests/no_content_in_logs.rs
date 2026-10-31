//! #18: clipboard text never reaches the logs, at any level.
//!
//! Runs a sync with secret text through every path that logs (send, receive,
//! apply, oversize, invalid, echo) while capturing all `tracing` output at
//! TRACE, then checks the secret isn't in it.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::{Peer, PeerBuilder, mesh, settle};
use pasta_core::clip::Flavors;

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
    mesh(&[&a, &b]);
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
