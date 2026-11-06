//! One running Pasta device: the sync engine wired to the network.
//!
//! The app and the integration tests (#19) both start a [`Node`], so the
//! tests exercise the same wiring as real devices, with fakes only at the
//! edges (clipboard, discovery, verifier).

use std::sync::Arc;

use pasta_core::clip::Flavors;
use pasta_core::clipboard::{ClipboardAdapter, ClipboardError, WriteKind};
use pasta_core::engine::{self, EngineEvent, EngineHandle, ExpireRequest, Outbound};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::discovery::Discovery;
use crate::network::{NetEvent, Network, NetworkOptions};
use crate::verify::Verifier;

/// Everything a node reports, except received items (those go to the engine).
#[derive(Debug, Clone)]
pub enum NodeEvent {
    Net(NetEvent),
    Engine(EngineEvent),
}

pub struct Node {
    network: Network,
    engine: Option<EngineHandle>,
    forward: JoinHandle<()>,
}

impl Node {
    pub fn start(
        options: NetworkOptions,
        adapter: Arc<dyn ClipboardAdapter>,
        discovery: Arc<dyn Discovery>,
        verifier: Arc<dyn Verifier>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<NodeEvent>), ClipboardError> {
        let device_id = options.device.id;
        let settings = options.settings.clone();
        let (network, mut net_events) = Network::start(options, discovery, verifier);
        let outbound: Arc<dyn Outbound> = Arc::new(network.clone());
        let (engine, mut engine_events) = match engine::spawn(device_id, settings, adapter, outbound) {
            Ok(started) => started,
            Err(err) => {
                let network = network.clone();
                tokio::spawn(async move { network.shutdown().await });
                return Err(err);
            }
        };
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let to_engine = engine.sender();
        let forward = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(event) = net_events.recv() => match event {
                        NetEvent::Clip { from, clip } => to_engine.remote(from, clip),
                        other => { let _ = events_tx.send(NodeEvent::Net(other)); }
                    },
                    Some(event) = engine_events.recv() => { let _ = events_tx.send(NodeEvent::Engine(event)); }
                    else => return,
                }
            }
        });
        Ok((Self { network, engine: Some(engine), forward }, events_rx))
    }

    pub fn network(&self) -> &Network {
        &self.network
    }

    /// Puts text on this device's clipboard without sending it to peers.
    pub fn write_local(&self, flavors: Flavors) {
        if let Some(engine) = &self.engine {
            engine.write_local(flavors);
        }
    }

    /// Writes to this device's clipboard without syncing, and waits for the write.
    pub async fn write_local_wait(&self, flavors: Flavors, kind: WriteKind) -> Result<(), String> {
        match &self.engine {
            Some(engine) => engine.write_local_wait(flavors, kind).await,
            None => Err(ClipboardError::Stopped.to_string()),
        }
    }

    /// Takes received items back out of the clipboard and its history (#33, #34).
    /// Returns the ids the adapter couldn't expire.
    pub async fn expire(&self, items: Vec<ExpireRequest>) -> Result<Vec<uuid::Uuid>, String> {
        match &self.engine {
            Some(engine) => engine.expire(items).await,
            None => Err(ClipboardError::Stopped.to_string()),
        }
    }

    /// Stops the clipboard watcher first, then closes every connection.
    pub async fn shutdown(mut self) {
        if let Some(engine) = self.engine.take() {
            engine.shutdown().await;
        }
        self.network.shutdown().await;
        self.forward.abort();
    }
}
