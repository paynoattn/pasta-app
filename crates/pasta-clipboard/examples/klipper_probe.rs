//! Spike #49: how the KDE adapter's writes and expiries land in Klipper.
//!
//! `RUST_LOG=debug cargo run -p pasta-clipboard --example klipper_probe` writes
//! two marker items, one Klipper may record and one with the password hint,
//! and checks after each write and expiry whether Klipper's history has it.
//! Prints booleans, sizes and hashes, never clipboard text. Leaves the first
//! marker (`pasta-probe-…`) in Klipper's history: there is no per-item delete.

#![allow(clippy::print_stdout)]

#[cfg(target_os = "linux")]
fn main() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use pasta_core::clip::Flavors;
    use pasta_core::clipboard::AdapterOptions;

    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let bus = zbus::blocking::Connection::session().expect("session bus");
    let klipper = zbus::blocking::Proxy::new(&bus, "org.kde.klipper", "/klipper", "org.kde.klipper.klipper").ok();
    let history_top = || -> Option<String> { klipper.as_ref()?.call("getClipboardHistoryItem", &(0i32,)).ok() };
    let menu = || -> Vec<String> {
        klipper.as_ref().and_then(|k| k.call("getClipboardHistoryMenu", &()).ok()).unwrap_or_default()
    };

    let adapter = match pasta_clipboard::system() {
        Ok(adapter) => adapter,
        Err(err) => return println!("no clipboard adapter: {err}"),
    };
    println!("status: {:?}", adapter.status());
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let _watch = adapter
        .watch(Box::new(move |copy| {
            counter.fetch_add(1, Ordering::SeqCst);
            println!("copy reported: bytes={} hash={}", copy.flavors.plain.len(), copy.flavors.content_hash());
        }))
        .expect("watch");

    for (label, add_to_history) in [("recorded", true), ("password hint", false)] {
        adapter.configure(&AdapterOptions { add_to_history, ..AdapterOptions::default() });
        let marker = format!("pasta-probe-{}", uuid::Uuid::new_v4());
        let flavors = Flavors { plain: marker.clone(), html: Some(format!("<b>{marker}</b>")), rtf: None };
        let before = seen.load(Ordering::SeqCst);
        let receipt = adapter.write(uuid::Uuid::new_v4(), &flavors).expect("write");
        std::thread::sleep(Duration::from_millis(800));
        println!("[{label}] own write reported back: {}", seen.load(Ordering::SeqCst) > before);
        println!("[{label}] Klipper history top is the marker: {}", history_top().as_deref() == Some(&marker));
        adapter.expire(&receipt).expect("expire");
        std::thread::sleep(Duration::from_millis(800));
        println!(
            "[{label}] after expiry, marker still in Klipper history: {}",
            menu().iter().any(|item| item == &marker)
        );
        println!("[{label}] copies reported after expiry: {}", seen.load(Ordering::SeqCst) - before);
    }
    println!("watching 10 s more; copy something to see it reported");
    std::thread::sleep(Duration::from_secs(10));
}

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("the Klipper probe runs on KDE Plasma only");
}
