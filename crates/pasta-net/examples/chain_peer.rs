//! A headless Pasta peer on the real tailnet, for testing the sync chain
//! between machines (#20–#22) without the app's UI.
//!
//! It runs the same [`Node`] as the app, with real Tailscale discovery and
//! WhoIs, and an in-memory clipboard. It never prints clipboard text: items
//! are reported by id, size and the first 16 hex digits of their SHA-256.
//!
//! ```sh
//! # Start a chain; the words go to a file (0600), not the terminal.
//! cargo run -p pasta-net --example chain_peer -- --create /tmp/words --auto-approve --for 300
//! # Join it from another machine, accept the approval, send a test item every 5 s.
//! cargo run -p pasta-net --example chain_peer -- --join /tmp/words --auto-accept --send-every 5 --for 120
//! ```
//!
//! Each run is a new device (fresh id and key). Don't run it next to the app
//! on the same machine: both want port 47820.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pasta_core::config::Settings;
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::engine::EngineEvent;
use pasta_core::testing::FakeClipboard;
use pasta_crypto::membership::{Candidate, Membership};
use pasta_crypto::{ChainKeys, DeviceKey};
use pasta_net::discovery::TailscaleDiscovery;
use pasta_net::network::{NetEvent, NetworkOptions, Timing};
use pasta_net::node::{Node, NodeEvent};
use pasta_net::tailscale::LazyTailscale;
use pasta_net::trust::ChainSetup;
use pasta_net::verify::TailscaleVerifier;
use tokio::sync::watch;

#[derive(Default)]
struct Args {
    name: Option<String>,
    create: Option<PathBuf>,
    join: Option<PathBuf>,
    auto_approve: bool,
    auto_accept: bool,
    accept_after: u64,
    send_every: Option<u64>,
    run_for: u64,
}

fn parse() -> Args {
    let mut args = Args { run_for: 120, ..Args::default() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--name" => args.name = it.next(),
            "--create" => args.create = it.next().map(PathBuf::from),
            "--join" => args.join = it.next().map(PathBuf::from),
            "--auto-approve" => args.auto_approve = true,
            "--auto-accept" => args.auto_accept = true,
            // Accept approvals only this many seconds after they arrive (shows nothing syncs before).
            "--accept-after" => {
                args.auto_accept = true;
                args.accept_after = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
            "--send-every" => args.send_every = it.next().and_then(|s| s.parse().ok()),
            "--for" => args.run_for = it.next().and_then(|s| s.parse().ok()).unwrap_or(120),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    args
}

fn hash16(flavors: &pasta_core::clip::Flavors) -> String {
    flavors.content_hash().to_hex()[..16].to_owned()
}

#[allow(clippy::print_stdout)]
#[tokio::main]
async fn main() {
    let args = parse();
    let tailscale = Arc::new(LazyTailscale::new());
    let status = tailscale.get().expect("tailscale CLI").status().await.expect("tailscale status");
    let me_node = status.self_node.expect("this machine on the tailnet");
    let device = DeviceInfo {
        id: DeviceId::random(),
        name: args.name.clone().unwrap_or_else(|| format!("{}-probe", me_node.name)),
        os: std::env::consts::OS.to_owned(),
        os_version: String::new(),
        app_version: format!("{}-probe", env!("CARGO_PKG_VERSION")),
    };
    let key = Arc::new(DeviceKey::generate());
    let chain = if let Some(path) = &args.create {
        let keys = Arc::new(ChainKeys::generate());
        write_private(path, keys.words().as_bytes());
        let me = Candidate {
            device_id: device.id,
            device_key: key.public(),
            name: device.name.clone(),
            os: device.os.clone(),
            node_id: me_node.stable_id.clone(),
        };
        let membership = Membership::create(keys.id(), &key, me, UnixMillis::now());
        println!("created chain {} (words in {})", keys.id().fingerprint(), path.display());
        Some(ChainSetup { keys, membership })
    } else if let Some(path) = &args.join {
        let words = zeroize::Zeroizing::new(std::fs::read_to_string(path).expect("read the words file"));
        let keys = Arc::new(ChainKeys::from_words(&words).expect("valid words"));
        println!("joining chain {}", keys.id().fingerprint());
        Some(ChainSetup { membership: Membership::joining(keys.id()), keys })
    } else {
        println!("no chain");
        None
    };
    println!("device {} key {}", device.id, pasta_crypto::keys::short(&key.public()));

    let (_settings_tx, settings_rx) = watch::channel(Settings::default());
    let options = NetworkOptions {
        device: device.clone(),
        device_key: key,
        chain,
        settings: settings_rx,
        known_peers: Default::default(),
        timing: Timing::default(),
    };
    let clipboard = FakeClipboard::new();
    clipboard.quiet_writes(true);
    let discovery = Arc::new(TailscaleDiscovery::new(Arc::clone(&tailscale)));
    let verifier = Arc::new(TailscaleVerifier::new(tailscale));
    let (node, mut events) =
        Node::start(options, Arc::new(clipboard.clone()), discovery, verifier).expect("start the node");
    let network = node.network().clone();
    let mut chain_status = network.chain();

    let deadline = tokio::time::sleep(Duration::from_secs(args.run_for));
    tokio::pin!(deadline);
    let mut send = tokio::time::interval(Duration::from_secs(args.send_every.unwrap_or(3600)));
    send.tick().await;
    let mut sent = 0u32;
    loop {
        tokio::select! {
            () = &mut deadline => break,
            _ = send.tick(), if args.send_every.is_some() => {
                sent += 1;
                let text = format!("pasta-probe-canary {} #{sent}", device.name);
                println!("copy #{sent}: {} bytes, sha256 {}", text.len(), hash16(&pasta_core::clip::Flavors::plain(text.clone())));
                clipboard.copy(&text);
            }
            Ok(()) = chain_status.changed() => {
                let status = chain_status.borrow_and_update().clone();
                if let Some(chain) = &status.chain {
                    println!(
                        "chain {}: approved={} members={} pending={} offers={}",
                        chain.id.fingerprint(),
                        chain.approved,
                        chain.members.len(),
                        chain.pending.len(),
                        chain.offers.len()
                    );
                    if args.auto_accept && !chain.offers.is_empty() {
                        tokio::time::sleep(Duration::from_secs(args.accept_after)).await;
                        for offer in &chain.offers {
                            let key = pasta_crypto::keys::short(&offer.approver_key);
                            println!("accepting the approval from {} (key {key})", offer.approver_name);
                            if let Err(err) = network.accept(&key).await {
                                println!("accept failed: {err}");
                            }
                        }
                    }
                    if args.auto_approve {
                        for pending in &chain.pending {
                            println!("approving {} ({}, key {})", pending.device_id, pending.name, pasta_crypto::keys::short(&pending.device_key));
                            let key = pasta_crypto::keys::short(&pending.device_key);
                            if let Err(err) = network.approve(pending.device_id, &key).await {
                                println!("approve failed: {err}");
                            }
                        }
                    }
                } else {
                    println!("not in a chain");
                }
            }
            Some(event) = events.recv() => match event {
                NodeEvent::Net(NetEvent::Connected { device_id, name, .. }) => println!("connected {device_id} ({name})"),
                NodeEvent::Net(NetEvent::Disconnected { device_id, reason }) => println!("disconnected {device_id}: {reason}"),
                NodeEvent::Net(NetEvent::Rejected { reason, .. }) => println!("rejected: {reason}"),
                NodeEvent::Net(NetEvent::RemovedFromChain) => println!("REMOVED from the chain"),
                NodeEvent::Engine(EngineEvent::Applied { clip, from, .. }) => {
                    println!("RECEIVED item {} from {from}: {} bytes, sha256 {}", clip.item_id, clip.flavors.byte_len(), hash16(&clip.flavors));
                }
                NodeEvent::Engine(EngineEvent::Sent { clip, peers }) => println!("sent item {} to {peers} peer(s)", clip.item_id),
                _ => {}
            },
        }
    }
    println!("received {} item(s) in total", clipboard.writes().len());
    node.shutdown().await;
}

fn write_private(path: &PathBuf, bytes: &[u8]) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .expect("words file");
        file.write_all(bytes).expect("write the words");
    }
    #[cfg(not(unix))]
    std::fs::write(path, bytes).expect("write the words");
}
