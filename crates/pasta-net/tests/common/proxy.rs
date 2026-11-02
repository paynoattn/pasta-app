//! A small recording TCP proxy to stand in for "someone on the network" (#21).
//!
//! It sits between a dialing peer and a listening peer, records every byte in
//! both directions, and understands just enough RFC 6455 to also record the
//! WebSocket text frames (client frames are masked; the proxy unmasks them for
//! the record but forwards the original bytes). Frames travelling from the
//! listener to the dialer can be rewritten, duplicated or dropped by a
//! [`Tamper`] callback, which is how the tests play an active attacker.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Called with each text frame the listener sends the dialer; returns the
/// frames to deliver instead (none drops it, two duplicates it).
pub type Tamper = Box<dyn FnMut(&str) -> Vec<String> + Send>;

#[derive(Default)]
pub struct Capture {
    /// Everything the dialer sent, exactly as it went over the wire.
    pub raw_up: Vec<u8>,
    /// Everything the listener sent, exactly as it went over the wire.
    pub raw_down: Vec<u8>,
    /// WebSocket text frames dialer → listener (unmasked).
    pub frames_up: Vec<String>,
    /// WebSocket text frames listener → dialer, before any tampering.
    pub frames_down: Vec<String>,
    /// TCP connections the proxy accepted.
    pub connections: usize,
}

impl Capture {
    /// Whether `needle` appears anywhere in the raw traffic, either way.
    pub fn raw_contains(&self, needle: &str) -> bool {
        let needle = needle.as_bytes();
        [&self.raw_up, &self.raw_down].iter().any(|haystack| haystack.windows(needle.len()).any(|w| w == needle))
    }

    /// Whether any decoded frame contains `needle`.
    pub fn frames_contain(&self, needle: &str) -> bool {
        self.frames_up.iter().chain(&self.frames_down).any(|frame| frame.contains(needle))
    }
}

struct Shared {
    capture: Mutex<Capture>,
    tamper: Mutex<Option<Tamper>>,
}

pub struct Proxy {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Listens on loopback and forwards every connection to `target`.
    pub async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared { capture: Mutex::default(), tamper: Mutex::new(None) });
        let task = tokio::spawn({
            let shared = Arc::clone(&shared);
            async move {
                while let Ok((client, _)) = listener.accept().await {
                    shared.capture.lock().unwrap().connections += 1;
                    let Ok(server) = TcpStream::connect(target).await else { continue };
                    let (client_rx, client_tx) = client.into_split();
                    let (server_rx, server_tx) = server.into_split();
                    tokio::spawn(pump(client_rx, server_tx, true, Arc::clone(&shared)));
                    tokio::spawn(pump(server_rx, client_tx, false, Arc::clone(&shared)));
                }
            }
        });
        Self { addr, shared, task }
    }

    /// Starts rewriting listener → dialer text frames.
    pub fn set_tamper(&self, tamper: impl FnMut(&str) -> Vec<String> + Send + 'static) {
        *self.shared.tamper.lock().unwrap() = Some(Box::new(tamper));
    }

    pub fn clear_tamper(&self) {
        *self.shared.tamper.lock().unwrap() = None;
    }

    /// Runs `f` on what has been recorded so far.
    pub fn with_capture<T>(&self, f: impl FnOnce(&Capture) -> T) -> T {
        f(&self.shared.capture.lock().unwrap())
    }

    pub fn connections(&self) -> usize {
        self.with_capture(|capture| capture.connections)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Frame {
    opcode: u8,
    /// Unmasked payload.
    payload: Vec<u8>,
    /// The frame as it was on the wire.
    raw: Vec<u8>,
}

/// Takes one complete frame off the front of `buf`, if there is one.
fn parse_frame(buf: &mut Vec<u8>) -> Option<Frame> {
    if buf.len() < 2 {
        return None;
    }
    let opcode = buf[0] & 0x0f;
    let masked = buf[1] & 0x80 != 0;
    let (len, mut at) = match buf[1] & 0x7f {
        126 if buf.len() >= 4 => (usize::from(u16::from_be_bytes([buf[2], buf[3]])), 4),
        127 if buf.len() >= 10 => (usize::try_from(u64::from_be_bytes(buf[2..10].try_into().unwrap())).unwrap(), 10),
        126 | 127 => return None,
        short => (usize::from(short), 2),
    };
    let key = if masked {
        if buf.len() < at + 4 {
            return None;
        }
        let key = [buf[at], buf[at + 1], buf[at + 2], buf[at + 3]];
        at += 4;
        Some(key)
    } else {
        None
    };
    if buf.len() < at + len {
        return None;
    }
    let raw: Vec<u8> = buf.drain(..at + len).collect();
    let mut payload = raw[at..].to_vec();
    if let Some(key) = key {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= key[i % 4];
        }
    }
    Some(Frame { opcode, payload, raw })
}

/// A complete, unmasked text frame (what a server sends).
fn text_frame(payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let mut out = vec![0x81];
    match bytes.len() {
        len @ 0..=125 => out.push(u8::try_from(len).unwrap()),
        len @ 126..=65535 => {
            out.push(126);
            out.extend_from_slice(&u16::try_from(len).unwrap().to_be_bytes());
        }
        len => {
            out.push(127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(bytes);
    out
}

async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, up: bool, shared: Arc<Shared>) {
    let mut chunk = vec![0u8; 16 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    let mut in_http = true;
    loop {
        let n = match from.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        {
            let mut capture = shared.capture.lock().unwrap();
            let raw = if up { &mut capture.raw_up } else { &mut capture.raw_down };
            raw.extend_from_slice(&chunk[..n]);
        }
        pending.extend_from_slice(&chunk[..n]);
        let mut out = Vec::new();
        if in_http {
            // The HTTP upgrade request or response comes first; pass it on untouched.
            let Some(end) = pending.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
            out.extend(pending.drain(..end + 4));
            in_http = false;
        }
        while let Some(frame) = parse_frame(&mut pending) {
            if frame.opcode != 1 {
                out.extend(frame.raw);
                continue;
            }
            let text = String::from_utf8_lossy(&frame.payload).into_owned();
            {
                let mut capture = shared.capture.lock().unwrap();
                let frames = if up { &mut capture.frames_up } else { &mut capture.frames_down };
                frames.push(text.clone());
            }
            let replaced = if up { None } else { shared.tamper.lock().unwrap().as_mut().map(|tamper| tamper(&text)) };
            match replaced {
                Some(frames) => frames.iter().for_each(|frame| out.extend(text_frame(frame))),
                None => out.extend(frame.raw),
            }
        }
        if !out.is_empty() && to.write_all(&out).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
}
