//! WebSocket transport between peers (#9).
//!
//! Each device listens on its tailnet addresses only, never `0.0.0.0`, and
//! dials the peers discovery finds. One JSON [`Envelope`] per text frame.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use pasta_core::protocol::{DecodeError, Envelope, max_frame_bytes};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

use crate::tailscale::is_tailnet_ip;

pub type WsStream = WebSocketStream<TcpStream>;

/// Close codes Pasta sends, in the private-use range 4000–4999.
pub mod close {
    /// The peer speaks another protocol version.
    pub const UNSUPPORTED_VERSION: u16 = 4001;
    /// The peer failed verification (not a tailnet node, or a device id seen on another node).
    pub const REJECTED: u16 = 4002;
    /// Another connection to the same device is kept instead.
    pub const DUPLICATE: u16 = 4003;
    /// The peer sent something that isn't valid protocol.
    pub const PROTOCOL_ERROR: u16 = 4004;
    /// No traffic within the heartbeat timeout.
    pub const TIMED_OUT: u16 = 4005;
    /// Not in the same sync chain (or this side isn't in one at all).
    pub const CHAIN_MISMATCH: u16 = 4006;
    /// The handshake signature or device key doesn't check out, or the device was declined.
    pub const NOT_TRUSTED: u16 = 4007;
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("refusing to listen on {0}: not a tailnet or loopback address")]
    NotTailnet(IpAddr),
    #[error("port {port} is already in use on {ip}")]
    PortInUse { ip: IpAddr, port: u16 },
    #[error("timed out")]
    Timeout,
    #[error("closed by peer ({code} {reason})")]
    ClosedByPeer { code: u16, reason: String },
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tungstenite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Listens on `addr`, which must be a tailnet address (or loopback, for tests).
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, TransportError> {
    if !is_tailnet_ip(addr.ip()) && !addr.ip().is_loopback() {
        return Err(TransportError::NotTailnet(addr.ip()));
    }
    TcpListener::bind(addr).await.map_err(|err| match err.kind() {
        std::io::ErrorKind::AddrInUse => TransportError::PortInUse { ip: addr.ip(), port: addr.port() },
        _ => TransportError::Io(err),
    })
}

fn config(max_item_bytes: usize) -> WebSocketConfig {
    let limit = max_frame_bytes(max_item_bytes);
    WebSocketConfig::default().max_message_size(Some(limit)).max_frame_size(Some(limit))
}

/// Completes the server side of the WebSocket handshake.
pub async fn accept(stream: TcpStream, max_item_bytes: usize, timeout: Duration) -> Result<WsStream, TransportError> {
    let _ = stream.set_nodelay(true);
    let handshake = tokio_tungstenite::accept_async_with_config(stream, Some(config(max_item_bytes)));
    Ok(tokio::time::timeout(timeout, handshake).await.map_err(|_| TransportError::Timeout)??)
}

/// Opens a WebSocket to a peer.
pub async fn connect(addr: SocketAddr, max_item_bytes: usize, timeout: Duration) -> Result<WsStream, TransportError> {
    let connecting = async {
        let stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        let url = format!("ws://{addr}/pasta");
        let (ws, _response) =
            tokio_tungstenite::client_async_with_config(url, stream, Some(config(max_item_bytes))).await?;
        Ok::<_, TransportError>(ws)
    };
    tokio::time::timeout(timeout, connecting).await.map_err(|_| TransportError::Timeout)?
}

pub async fn send(ws: &mut WsStream, envelope: &Envelope) -> Result<(), TransportError> {
    ws.send(WsMessage::text(envelope.encode())).await?;
    Ok(())
}

/// Next protocol frame, skipping WebSocket control frames. `Ok(None)` when closed.
pub async fn recv(ws: &mut WsStream, max_item_bytes: usize) -> Result<Option<Envelope>, TransportError> {
    while let Some(frame) = ws.next().await {
        if let Some(envelope) = decode_frame(frame?, max_item_bytes)? {
            return Ok(Some(envelope));
        }
    }
    Ok(None)
}

/// Decodes one WebSocket message. Control frames give `Ok(None)`.
pub fn decode_frame(frame: WsMessage, max_item_bytes: usize) -> Result<Option<Envelope>, TransportError> {
    match frame {
        WsMessage::Text(text) => Ok(Some(Envelope::decode(text.as_str(), max_item_bytes)?)),
        WsMessage::Binary(_) => Err(TransportError::Decode(DecodeError::Invalid("binary frames are not used"))),
        WsMessage::Close(frame) => Err(match frame {
            Some(frame) => TransportError::ClosedByPeer { code: frame.code.into(), reason: frame.reason.to_string() },
            None => TransportError::ClosedByPeer { code: 1005, reason: String::new() },
        }),
        WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => Ok(None),
    }
}

pub fn close_frame(code: u16, reason: &'static str) -> WsMessage {
    WsMessage::Close(Some(CloseFrame { code: CloseCode::from(code), reason: reason.into() }))
}

/// Sends a close frame and gives the peer a moment to see it.
pub async fn close(mut ws: WsStream, code: u16, reason: &'static str) {
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let _ = ws.send(close_frame(code, reason)).await;
        // Drain until the peer acknowledges the close.
        while let Some(Ok(_)) = ws.next().await {}
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn refuses_to_listen_on_lan_or_wildcard_addresses() {
        for addr in ["0.0.0.0:0", "192.168.1.10:0", "[::]:0"] {
            let err = bind(addr.parse().unwrap()).await.unwrap_err();
            assert!(matches!(err, TransportError::NotTailnet(_)), "{addr}: {err}");
        }
    }

    #[tokio::test]
    async fn reports_a_port_in_use() {
        let first = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let err = bind(first.local_addr().unwrap()).await.unwrap_err();
        assert!(matches!(err, TransportError::PortInUse { .. }), "{err}");
    }
}
