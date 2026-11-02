//! A bare WebSocket client that speaks (or abuses) protocol v2 by hand, to play
//! impostors and misbehaving members against a real peer (#21, #22).

use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use pasta_core::clip::Flavors;
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::protocol::handshake_transcript;
use pasta_core::protocol::{Auth, Clip, DEFAULT_MAX_ITEM_BYTES, Envelope, Hello, Inner, Message, Role};
use pasta_crypto::{ChainKeys, DeviceKey, random_key32};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use super::within;

pub type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What arrived next on a raw connection.
#[derive(Debug)]
pub enum Incoming {
    Message(Box<Envelope>),
    Close(u16),
    Gone,
}

/// Everything a refused client saw before the connection ended.
#[derive(Debug, Default)]
pub struct Drained {
    /// `type` of each protocol frame received, in order.
    pub kinds: Vec<&'static str>,
    pub close: Option<u16>,
}

pub async fn open(addr: SocketAddr) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/pasta")).await.unwrap();
    ws
}

fn device_info(id: DeviceId) -> DeviceInfo {
    DeviceInfo { id, name: "raw-client".into(), os: "test".into(), os_version: "1".into(), app_version: "0.0.0".into() }
}

/// A `hello` with a fresh nonce and, given the chain's keys, a valid chain proof.
pub fn hello(id: DeviceId, key: &DeviceKey, chain: Option<&ChainKeys>) -> Hello {
    let mut hello = Hello::new(&device_info(id), key.public(), random_key32());
    hello.chain_proof = chain.map(|chain| chain.chain_proof(id.0.as_bytes(), &hello.device_key, &hello.nonce));
    hello
}

pub async fn send(ws: &mut Ws, from: DeviceId, message: Message) {
    send_envelope(ws, &Envelope::new(from, message)).await;
}

pub async fn send_envelope(ws: &mut Ws, envelope: &Envelope) {
    send_text(ws, envelope.encode()).await;
}

pub async fn send_text(ws: &mut Ws, text: String) {
    ws.send(WsMessage::text(text)).await.unwrap();
}

/// The next protocol frame, close frame or end of stream.
pub async fn next(ws: &mut Ws) -> Incoming {
    loop {
        match within(ws.next()).await {
            None | Some(Err(_)) => return Incoming::Gone,
            Some(Ok(WsMessage::Text(text))) => {
                let envelope = Envelope::decode(text.as_str(), DEFAULT_MAX_ITEM_BYTES).expect("a valid frame");
                return Incoming::Message(Box::new(envelope));
            }
            Some(Ok(WsMessage::Close(frame))) => return Incoming::Close(frame.map_or(1005, |frame| frame.code.into())),
            Some(Ok(_)) => {}
        }
    }
}

/// Reads until the other side closes.
pub async fn until_closed(ws: &mut Ws) -> Drained {
    let mut drained = Drained::default();
    loop {
        match next(ws).await {
            Incoming::Message(envelope) => drained.kinds.push(envelope.message.kind()),
            Incoming::Close(code) => {
                drained.close = Some(code);
                return drained;
            }
            Incoming::Gone => return drained,
        }
    }
}

/// A clip as the engine would build it.
pub fn clip(from: DeviceId, text: &str) -> Inner {
    let flavors = Flavors::plain(text);
    Inner::Clip(Clip {
        item_id: Uuid::new_v4(),
        origin_device: from,
        origin_app: None,
        created_at: UnixMillis::now(),
        content_hash: flavors.content_hash(),
        flavors,
    })
}

/// A plaintext `clip` frame, which protocol v2 never accepts.
pub fn plaintext_clip_frame(from: DeviceId, text: &str) -> String {
    serde_json::json!({
        "v": 2,
        "type": "clip",
        "msg_id": Uuid::new_v4(),
        "from_device": from.0,
        "sent_at": UnixMillis::now().0,
        "body": { "item_id": Uuid::new_v4(), "origin_device": from.0, "flavors": { "text/plain": text } },
    })
    .to_string()
}

/// A raw connection that completed the handshake correctly.
pub struct Joined {
    pub ws: Ws,
    pub id: DeviceId,
    pub chain: Arc<ChainKeys>,
    pub ours: Hello,
    pub theirs: Hello,
}

/// Dials `addr` and does the whole handshake as `id` with `key`, proving
/// knowledge of `chain`. Whether the listener then trusts the device depends
/// on whether the chain approved it.
pub async fn join(addr: SocketAddr, id: DeviceId, key: &DeviceKey, chain: &Arc<ChainKeys>) -> Joined {
    let mut ws = open(addr).await;
    let ours = hello(id, key, Some(chain));
    send(&mut ws, id, Message::Hello(ours.clone())).await;
    let Incoming::Message(reply) = next(&mut ws).await else { panic!("expected hello_ack") };
    let Message::HelloAck(theirs) = reply.message else { panic!("expected hello_ack, got {}", reply.message.kind()) };
    let transcript = handshake_transcript(Role::Listener, &ours, &theirs);
    let signature = theirs.signature.as_ref().expect("a signed hello_ack");
    assert!(pasta_crypto::keys::verify(&theirs.device_key, &transcript, signature), "hello_ack signature");
    let auth = Auth { signature: key.sign(&handshake_transcript(Role::Dialer, &ours, &theirs)) };
    send(&mut ws, id, Message::Auth(auth)).await;
    Joined { ws, id, chain: Arc::clone(chain), ours, theirs }
}

impl Joined {
    /// An `encrypted` envelope from this device carrying `inner`.
    pub fn seal(&self, inner: &Inner) -> Envelope {
        self.chain.seal_envelope(self.id, inner)
    }

    pub async fn send_sealed(&mut self, inner: &Inner) -> Envelope {
        let envelope = self.seal(inner);
        send_envelope(&mut self.ws, &envelope).await;
        envelope
    }
}
