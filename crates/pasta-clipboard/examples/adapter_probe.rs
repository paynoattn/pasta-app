//! Exercises this OS's clipboard adapter against the real clipboard (#12–#15).
//!
//! `cargo run -p pasta-clipboard --example adapter_probe -- watch [seconds]`
//! reports each copy; `-- write` writes a marker item, checks the watcher
//! doesn't report it back, then expires it again (on Windows: deletes it from
//! Win+V history). Prints sizes, hashes and source apps, never clipboard text.

#![allow(clippy::print_stdout)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pasta_core::clip::Flavors;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "watch".to_owned());
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);

    let adapter = match pasta_clipboard::system() {
        Ok(adapter) => adapter,
        Err(err) => return println!("no clipboard adapter: {err}"),
    };
    println!("status: {:?}", adapter.status());

    let seen = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&seen);
    let watch = adapter.watch(Box::new(move |copy| {
        counter.fetch_add(1, Ordering::SeqCst);
        let app = copy.source_app.map(|app| format!("{} ({})", app.name, app.id)).unwrap_or_else(|| "unknown".into());
        println!("copy: {:?} hash={} from {app}", copy.flavors, copy.flavors.content_hash());
    }));
    let _watch = match watch {
        Ok(watch) => watch,
        Err(err) => return println!("watch failed: {err}"),
    };

    if mode == "write" {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
        let marker = format!("pasta-probe-{stamp}");
        let flavors = Flavors { plain: marker.clone(), html: Some(format!("<b>{marker}</b>")), rtf: None };
        let started = std::time::Instant::now();
        match adapter.write(uuid::Uuid::new_v4(), &flavors) {
            Ok(receipt) => {
                println!(
                    "wrote marker in {} ms: history entry found={} change_count={:?}",
                    started.elapsed().as_millis(),
                    receipt.history_id.is_some(),
                    receipt.change_count
                );
                std::thread::sleep(Duration::from_secs(2));
                println!("own write reported back as a copy: {}", seen.load(Ordering::SeqCst) > 0);
                match adapter.expire(&receipt) {
                    Ok(()) => println!("expired the marker"),
                    Err(err) => println!("expire failed: {err}"),
                }
            }
            Err(err) => println!("write failed: {err}"),
        }
    } else {
        println!("watching for {seconds}s; copy something");
        std::thread::sleep(Duration::from_secs(seconds));
    }
    println!("copies seen: {}", seen.load(Ordering::SeqCst));
}
