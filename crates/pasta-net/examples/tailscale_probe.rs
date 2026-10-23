//! Spike #4 probe: checks what Pasta can learn from the local Tailscale client.
//!
//! Run on each OS with `cargo run -p pasta-net --example tailscale_probe`.
//! Prints names, OS and timings only; no keys or account details.

#![allow(clippy::print_stdout)]

use std::time::Instant;

use pasta_net::tailscale::{Tailscale, is_tailnet_ip};

#[tokio::main]
async fn main() {
    let ts = match Tailscale::locate() {
        Ok(ts) => ts,
        Err(err) => {
            println!("locate: {err}");
            return;
        }
    };
    println!("cli: {}", ts.cli_path().display());

    let started = Instant::now();
    let status = match ts.status().await {
        Ok(status) => status,
        Err(err) => {
            println!("status: {err}");
            return;
        }
    };
    println!("status: {:?} in {} ms", status.backend_state, started.elapsed().as_millis());

    if let Some(me) = &status.self_node {
        println!("self: {} ({}) ips={:?}", me.name, me.os, me.ips);
        let all_tailnet = me.ips.iter().all(|ip| is_tailnet_ip(*ip));
        println!("self ips all in tailnet ranges: {all_tailnet}");
    }
    println!("peers: {} total, {} online", status.peers.len(), status.online_peers().count());
    for peer in &status.peers {
        let tagged = if peer.tags.is_empty() { "" } else { " [tagged]" };
        println!("  {:<28} {:<8} online={:<5} hostname={:?}{tagged}", peer.name, peer.os, peer.online, peer.host_name);
    }

    if let Some(ip) = status.online_peers().find_map(|p| p.ips.first().copied()) {
        let started = Instant::now();
        match ts.whois(ip).await {
            Ok(Some(who)) => println!("whois {ip}: {} in {} ms", who.name, started.elapsed().as_millis()),
            Ok(None) => println!("whois {ip}: not on tailnet"),
            Err(err) => println!("whois {ip}: {err}"),
        }
    }

    let lan_ip = "192.168.1.1".parse().expect("valid ip");
    match ts.whois(lan_ip).await {
        Ok(None) => println!("whois {lan_ip}: not on tailnet (expected)"),
        other => println!("whois {lan_ip}: unexpected {other:?}"),
    }
}
