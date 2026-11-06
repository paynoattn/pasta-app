//! Wire protocol v2 (#7, #21, #22): one JSON envelope per WebSocket text frame.
//!
//! ```json
//! { "v": 2, "type": "encrypted", "msg_id": "…", "from_device": "…", "sent_at": 1730000000000, "body": { … } }
//! ```
//!
//! Clipboard items and member lists only ever travel inside `encrypted`
//! (XChaCha20-Poly1305 under the chain key, with the envelope header as AAD);
//! see [`Inner`]. The handshake (`hello`, `hello_ack`, `auth`) and the
//! heartbeat stay plaintext and carry no content. ADR 0007 has the details.
//!
//! Decoding is two-step so that peers can fail gracefully: `v` is read first
//! (a different major version is reported as [`DecodeError::UnsupportedVersion`]
//! whatever the rest looks like), then `type` picks the body. Unknown and
//! reserved types are reported as [`DecodeError::UnknownType`], which callers
//! can ignore instead of dropping the connection.
//!
//! Errors never quote the input, since a `clip` body is clipboard text.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::chain::MemberList;
use crate::clip::{ContentHash, Flavors, SourceApp};
use crate::device::{DeviceId, DeviceInfo, UnixMillis};
use crate::remote::{ConfigUpdate, DeviceSettings};
use crate::wire::{Blob, Key32, Nonce24, Sig64};

/// Major version of the wire protocol. Peers with another version refuse the handshake.
///
/// v1 was M1's plaintext protocol; v2 (M2) adds the sync chain and E2E encryption.
pub const PROTOCOL_VERSION: u16 = 2;

/// Default limit for one clipboard item, all flavors together.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 1024 * 1024;

/// Envelope types reserved for later milestones; this build rejects them as
/// unknown. None at the moment: remote config (#31), once reserved here,
/// travels inside `encrypted` as [`Inner::ConfigUpdate`].
pub const RESERVED_TYPES: &[&str] = &[];

/// Upper bound for one encoded frame given the item limit: JSON escaping can
/// grow text up to six times (`\u0000`) and base64 of the ciphertext adds a
/// third on top, plus room for the envelope.
pub fn max_frame_bytes(max_item_bytes: usize) -> usize {
    max_item_bytes.saturating_mul(8).saturating_add(64 * 1024)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Envelope {
    pub v: u16,
    pub msg_id: Uuid,
    pub from_device: DeviceId,
    pub sent_at: UnixMillis,
    #[serde(flatten)]
    pub message: Message,
}

/// Every message type. Serialised as `"type"` + `"body"` next to the envelope fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum Message {
    /// First message from the side that opened the connection.
    Hello(Hello),
    /// Reply to `hello` from the side that accepted it, signed.
    HelloAck(Hello),
    /// The dialer's signature over the handshake, completing it.
    Auth(Auth),
    /// An [`Inner`] message, sealed with the chain key.
    Encrypted(Sealed),
    /// Receipt for an `encrypted` message.
    Ack(Ack),
    Ping(Ping),
    Pong(Pong),
}

impl Message {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::HelloAck(_) => "hello_ack",
            Self::Auth(_) => "auth",
            Self::Encrypted(_) => "encrypted",
            Self::Ack(_) => "ack",
            Self::Ping(_) => "ping",
            Self::Pong(_) => "pong",
        }
    }
}

/// What travels inside `encrypted`. Never sent in the clear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "body", rename_all = "snake_case")]
pub enum Inner {
    Clip(Clip),
    /// The sender's view of the chain's members (#22).
    Members(MemberList),
    /// The sender's own mode and expiry (#31). Sent on connect and on every change.
    Settings(DeviceSettings),
    /// A member changing the receiving device's mode and expiry (#31).
    ConfigUpdate(ConfigUpdate),
}

impl Inner {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Clip(_) => "clip",
            Self::Members(_) => "members",
            Self::Settings(_) => "settings",
            Self::ConfigUpdate(_) => "config_update",
        }
    }

    /// Checks a decrypted message against its envelope's sender and the size limit.
    pub fn validate(&self, from_device: DeviceId, max_item_bytes: usize) -> Result<(), DecodeError> {
        match self {
            Self::Clip(clip) => {
                if clip.origin_device != from_device {
                    return Err(DecodeError::Invalid("clip origin_device differs from the sender"));
                }
                clip.validate(max_item_bytes)
            }
            Self::Members(list) => {
                if list.within_limits() {
                    Ok(())
                } else {
                    Err(DecodeError::Invalid("member list over the size limits"))
                }
            }
            Self::Settings(settings) => settings.validate().map_err(DecodeError::Invalid),
            Self::ConfigUpdate(update) => {
                // Updates are never relayed: the member that made the change sends it.
                if update.issuer() != from_device {
                    return Err(DecodeError::Invalid("config_update issuer differs from the sender"));
                }
                update.validate().map_err(DecodeError::Invalid)
            }
        }
    }
}

/// Longest `name`, `os` or version string a `hello` may carry, in bytes.
pub const MAX_HELLO_FIELD_BYTES: usize = 128;

/// An AEAD-sealed [`Inner`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sealed {
    pub nonce: Nonce24,
    /// Ciphertext followed by the 16-byte Poly1305 tag.
    pub ciphertext: Blob,
}

/// The bytes an `encrypted` body is bound to (AEAD associated data): the
/// envelope header, so a body can't be moved to another message or sender.
pub fn header_aad(v: u16, msg_id: &Uuid, from_device: &DeviceId, sent_at: UnixMillis) -> Vec<u8> {
    let mut aad = Vec::with_capacity(64);
    aad.extend_from_slice(b"pasta/v2/encrypted\0");
    aad.extend_from_slice(&v.to_be_bytes());
    aad.extend_from_slice(msg_id.as_bytes());
    aad.extend_from_slice(from_device.0.as_bytes());
    aad.extend_from_slice(&sent_at.0.to_be_bytes());
    aad
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub device_id: DeviceId,
    pub name: String,
    pub os: String,
    #[serde(default)]
    pub os_version: String,
    pub app_version: String,
    pub protocol_version: u16,
    /// Optional features this build supports; none yet.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The device's Ed25519 public key (#22).
    pub device_key: Key32,
    /// Fresh random challenge for this connection; the other side signs it.
    pub nonce: Key32,
    /// HMAC under the chain's auth key over (chain id, device id, key, nonce):
    /// proves the sender knows the 24 words without revealing anything that
    /// links it to its chain. Absent when the device isn't in a chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_proof: Option<Key32>,
    /// `hello_ack` only: the listener's signature over the handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Sig64>,
}

impl Hello {
    pub fn new(device: &DeviceInfo, device_key: Key32, nonce: Key32) -> Self {
        Self {
            device_id: device.id,
            name: device.name.clone(),
            os: device.os.clone(),
            os_version: device.os_version.clone(),
            app_version: device.app_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: Vec::new(),
            device_key,
            nonce,
            chain_proof: None,
            signature: None,
        }
    }
}

/// Completes the handshake from the dialer's side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Auth {
    pub signature: Sig64,
}

/// Which side of a handshake signs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Dialer,
    Listener,
}

/// What each side signs with its device key: both hellos' identities and
/// nonces, so a signature only works for this one connection.
pub fn handshake_transcript(role: Role, dialer: &Hello, listener: &Hello) -> Vec<u8> {
    let mut out = Vec::with_capacity(220);
    out.extend_from_slice(b"pasta/v2/handshake\0");
    out.push(match role {
        Role::Dialer => b'D',
        Role::Listener => b'L',
    });
    for hello in [dialer, listener] {
        out.extend_from_slice(hello.device_id.0.as_bytes());
        out.extend_from_slice(&hello.device_key.0);
        out.extend_from_slice(&hello.nonce.0);
    }
    out
}

/// One copied item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub item_id: Uuid,
    /// The device the copy happened on. In v1 this is always the sender: items
    /// are never relayed.
    pub origin_device: DeviceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_app: Option<SourceApp>,
    pub created_at: UnixMillis,
    pub flavors: Flavors,
    pub content_hash: ContentHash,
}

impl Clip {
    /// Checks a received item against the size limit and its own hash.
    pub fn validate(&self, max_item_bytes: usize) -> Result<(), DecodeError> {
        let bytes = self.flavors.byte_len();
        if bytes > max_item_bytes {
            return Err(DecodeError::TooLarge { bytes, limit: max_item_bytes });
        }
        if self.flavors.plain.is_empty() {
            return Err(DecodeError::Invalid("clip has no text/plain"));
        }
        if self.flavors.content_hash() != self.content_hash {
            return Err(DecodeError::Invalid("clip content_hash does not match its text"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ack {
    /// `msg_id` of the acknowledged message.
    pub msg_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Ping {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pong {
    /// `msg_id` of the ping this answers; the sender measures latency from it.
    pub ping_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("frame is {bytes} bytes, over the {limit} byte limit")]
    TooLarge { bytes: usize, limit: usize },
    #[error("peer speaks protocol v{theirs}, this build speaks v{PROTOCOL_VERSION}")]
    UnsupportedVersion { theirs: u16 },
    #[error("unknown message type `{0}`")]
    UnknownType(String),
    #[error("malformed message ({})", describe(.0))]
    Malformed(#[source] serde_json::Error),
    #[error("invalid message: {0}")]
    Invalid(&'static str),
}

impl DecodeError {
    /// Errors after which the connection can carry on.
    pub fn is_ignorable(&self) -> bool {
        matches!(self, Self::UnknownType(_))
    }
}

/// serde_json's messages can quote input values; keep only the kind and position.
fn describe(err: &serde_json::Error) -> String {
    let kind = match err.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    };
    format!("{kind} error at line {} column {}", err.line(), err.column())
}

#[derive(Deserialize)]
struct VersionOnly {
    v: u16,
}

#[derive(Deserialize)]
struct RawEnvelope<'a> {
    #[serde(rename = "type")]
    kind: String,
    msg_id: Uuid,
    from_device: DeviceId,
    sent_at: UnixMillis,
    #[serde(borrow, default)]
    body: Option<&'a RawValue>,
}

impl Envelope {
    pub fn new(from_device: DeviceId, message: Message) -> Self {
        Self { v: PROTOCOL_VERSION, msg_id: Uuid::new_v4(), from_device, sent_at: UnixMillis::now(), message }
    }

    pub fn encode(&self) -> String {
        // Every field serialises infallibly (strings, numbers, UUIDs, no maps with non-string keys).
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parses and validates one frame. `max_item_bytes` bounds both the frame
    /// and any `clip` it carries.
    pub fn decode(frame: &str, max_item_bytes: usize) -> Result<Self, DecodeError> {
        let limit = max_frame_bytes(max_item_bytes);
        if frame.len() > limit {
            return Err(DecodeError::TooLarge { bytes: frame.len(), limit });
        }
        let VersionOnly { v } = serde_json::from_str(frame).map_err(DecodeError::Malformed)?;
        if v != PROTOCOL_VERSION {
            return Err(DecodeError::UnsupportedVersion { theirs: v });
        }
        let raw: RawEnvelope<'_> = serde_json::from_str(frame).map_err(DecodeError::Malformed)?;
        let body = raw.body;
        let message = match raw.kind.as_str() {
            "hello" => Message::Hello(parse_body(body)?),
            "hello_ack" => Message::HelloAck(parse_body(body)?),
            "auth" => Message::Auth(parse_body(body)?),
            "encrypted" => Message::Encrypted(parse_body(body)?),
            // v1's plaintext item. Never accepted: items only travel encrypted.
            "clip" => return Err(DecodeError::Invalid("plaintext clip")),
            "ack" => Message::Ack(parse_body(body)?),
            "ping" => Message::Ping(parse_body(body)?),
            "pong" => Message::Pong(parse_body(body)?),
            other => return Err(DecodeError::UnknownType(other.chars().take(32).collect())),
        };
        let envelope = Self { v, msg_id: raw.msg_id, from_device: raw.from_device, sent_at: raw.sent_at, message };
        envelope.validate()?;
        Ok(envelope)
    }

    /// The associated data an `encrypted` body of this envelope is sealed with.
    pub fn aad(&self) -> Vec<u8> {
        header_aad(self.v, &self.msg_id, &self.from_device, self.sent_at)
    }

    fn validate(&self) -> Result<(), DecodeError> {
        match &self.message {
            Message::Hello(hello) | Message::HelloAck(hello) => {
                if hello.device_id != self.from_device {
                    return Err(DecodeError::Invalid("hello device_id differs from the envelope's from_device"));
                }
                if hello.protocol_version != PROTOCOL_VERSION {
                    return Err(DecodeError::UnsupportedVersion { theirs: hello.protocol_version });
                }
                let fields = [&hello.name, &hello.os, &hello.os_version, &hello.app_version];
                if fields.iter().any(|field| field.len() > MAX_HELLO_FIELD_BYTES)
                    || hello.capabilities.len() > 32
                    || hello.capabilities.iter().any(|cap| cap.len() > MAX_HELLO_FIELD_BYTES)
                {
                    return Err(DecodeError::Invalid("hello fields over the size limits"));
                }
            }
            Message::Auth(_) | Message::Encrypted(_) | Message::Ack(_) | Message::Ping(_) | Message::Pong(_) => {}
        }
        Ok(())
    }
}

fn parse_body<T: DeserializeOwned>(body: Option<&RawValue>) -> Result<T, DecodeError> {
    let json = body.map_or("{}", RawValue::get);
    serde_json::from_str(json).map_err(DecodeError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::SyncMode;
    use crate::wire::Bytes;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::random(),
            name: "macbook-pro".into(),
            os: "macos".into(),
            os_version: "27.2".into(),
            app_version: "0.1.0".into(),
        }
    }

    fn hello(me: &DeviceInfo) -> Hello {
        Hello::new(me, Bytes([1; 32]), Bytes([2; 32]))
    }

    fn clip(from: DeviceId, text: &str) -> Clip {
        let flavors = Flavors { plain: text.into(), html: Some(format!("<b>{text}</b>")), rtf: None };
        Clip {
            item_id: Uuid::new_v4(),
            origin_device: from,
            origin_app: Some(SourceApp { name: "Notes".into(), id: "com.apple.Notes".into() }),
            created_at: UnixMillis(1_730_000_000_000),
            content_hash: flavors.content_hash(),
            flavors,
        }
    }

    fn round_trip(envelope: &Envelope) -> Envelope {
        Envelope::decode(&envelope.encode(), DEFAULT_MAX_ITEM_BYTES).unwrap()
    }

    #[test]
    fn every_message_round_trips() {
        let me = device();
        let mut ack = hello(&me);
        ack.chain_proof = Some(Bytes([3; 32]));
        ack.signature = Some(Bytes([4; 64]));
        let messages = [
            Message::Hello(hello(&me)),
            Message::HelloAck(ack),
            Message::Auth(Auth { signature: Bytes([5; 64]) }),
            Message::Encrypted(Sealed { nonce: Bytes([6; 24]), ciphertext: crate::wire::Blob(vec![7; 40]) }),
            Message::Ack(Ack { msg_id: Uuid::new_v4() }),
            Message::Ping(Ping {}),
            Message::Pong(Pong { ping_id: Uuid::new_v4() }),
        ];
        for message in messages {
            let envelope = Envelope::new(me.id, message);
            assert_eq!(round_trip(&envelope), envelope);
        }
    }

    #[test]
    fn envelope_shape_matches_the_spec() {
        let me = device();
        let sealed = Sealed { nonce: Bytes([6; 24]), ciphertext: crate::wire::Blob(vec![7; 40]) };
        let envelope = Envelope::new(me.id, Message::Encrypted(sealed));
        let value: serde_json::Value = serde_json::from_str(&envelope.encode()).unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 6);
        for key in ["v", "type", "msg_id", "from_device", "sent_at", "body"] {
            assert!(keys.contains(&key), "{key}");
        }
        assert_eq!(value["v"], 2);
        assert_eq!(value["type"], "encrypted");
        assert!(value["body"]["nonce"].is_string() && value["body"]["ciphertext"].is_string());
    }

    #[test]
    fn inner_messages_carry_items_and_validate_them() {
        let me = device();
        let inner = Inner::Clip(clip(me.id, "hi"));
        let value = serde_json::to_value(&inner).unwrap();
        assert_eq!(value["type"], "clip");
        assert_eq!(value["body"]["flavors"]["text/plain"], "hi");
        assert!(value["body"]["flavors"].get("text/rtf").is_none(), "absent flavors are omitted");
        assert_eq!(serde_json::from_value::<Inner>(value).unwrap(), inner);
        inner.validate(me.id, DEFAULT_MAX_ITEM_BYTES).unwrap();

        let mut tampered = clip(me.id, "hello");
        tampered.flavors.plain = "goodbye".into();
        assert!(matches!(Inner::Clip(tampered).validate(me.id, DEFAULT_MAX_ITEM_BYTES), Err(DecodeError::Invalid(_))));
        let relayed = Inner::Clip(clip(DeviceId::random(), "hello"));
        assert!(matches!(relayed.validate(me.id, DEFAULT_MAX_ITEM_BYTES), Err(DecodeError::Invalid(_))));
        let big = Inner::Clip(clip(me.id, &"x".repeat(2000)));
        assert!(matches!(big.validate(me.id, 1000), Err(DecodeError::TooLarge { .. })));
    }

    #[test]
    fn plaintext_clips_are_refused() {
        let id = DeviceId::random();
        let frame = format!(
            r#"{{"v":2,"type":"clip","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{}}}}"#,
            Uuid::nil()
        );
        let err = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap_err();
        assert!(matches!(err, DecodeError::Invalid(_)) && !err.is_ignorable(), "{err}");
    }

    #[test]
    fn aad_covers_the_whole_header() {
        let me = device();
        let envelope = Envelope::new(me.id, Message::Ping(Ping {}));
        let base = envelope.aad();
        let mut other = envelope.clone();
        other.msg_id = Uuid::new_v4();
        assert_ne!(other.aad(), base);
        let mut other = envelope.clone();
        other.from_device = DeviceId::random();
        assert_ne!(other.aad(), base);
        let mut other = envelope.clone();
        other.sent_at = UnixMillis(envelope.sent_at.0 + 1);
        assert_ne!(other.aad(), base);
        let mut other = envelope;
        other.v = 3;
        assert_ne!(other.aad(), base);
    }

    #[test]
    fn transcripts_differ_by_role_and_connection() {
        let (a, b) = (device(), device());
        let (ha, hb) = (hello(&a), hello(&b));
        let dialer = handshake_transcript(Role::Dialer, &ha, &hb);
        assert_ne!(dialer, handshake_transcript(Role::Listener, &ha, &hb));
        assert_ne!(dialer, handshake_transcript(Role::Dialer, &hb, &ha));
        let mut fresh = hb.clone();
        fresh.nonce = Bytes([9; 32]);
        assert_ne!(dialer, handshake_transcript(Role::Dialer, &ha, &fresh));
    }

    #[test]
    fn decodes_a_ping_without_body() {
        let id = DeviceId::random();
        let frame = format!(r#"{{"v":2,"type":"ping","msg_id":"{}","from_device":"{id}","sent_at":5}}"#, Uuid::nil());
        let envelope = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap();
        assert_eq!(envelope.message, Message::Ping(Ping {}));
    }

    #[test]
    fn rejects_other_major_versions_before_parsing_the_rest() {
        for v in [1, 3] {
            let frame = format!(r#"{{"v":{v},"something":"entirely different"}}"#);
            let err = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap_err();
            assert!(matches!(err, DecodeError::UnsupportedVersion { theirs } if theirs == v), "{err}");
        }

        let me = device();
        let mut hello = hello(&me);
        hello.protocol_version = 9;
        let err = Envelope::decode(&Envelope::new(me.id, Message::Hello(hello)).encode(), DEFAULT_MAX_ITEM_BYTES)
            .unwrap_err();
        assert!(matches!(err, DecodeError::UnsupportedVersion { theirs: 9 }), "{err}");
    }

    #[test]
    fn unknown_and_reserved_types_are_ignorable() {
        let id = DeviceId::random();
        for kind in RESERVED_TYPES.iter().copied().chain(["from_the_future"]) {
            let frame = format!(
                r#"{{"v":2,"type":"{kind}","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{}}}}"#,
                Uuid::nil()
            );
            let err = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap_err();
            assert!(err.is_ignorable(), "{kind}: {err}");
        }
    }

    #[test]
    fn rejects_malformed_frames_without_quoting_them() {
        let id = DeviceId::random();
        let secret = "hunter2-secret";
        let frames = [
            "not json".to_owned(),
            r#"{"v":"one"}"#.to_owned(),
            format!(
                r#"{{"v":2,"type":"encrypted","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{"nonce":"{secret}"}}}}"#,
                Uuid::nil()
            ),
            format!(
                r#"{{"v":2,"type":"auth","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{"signature":"{secret}"}}}}"#,
                Uuid::nil()
            ),
        ];
        for frame in frames {
            let err = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap_err();
            assert!(matches!(err, DecodeError::Malformed(_)), "{err}");
            assert!(!err.to_string().contains(secret), "{err}");
        }
    }

    #[test]
    fn enforces_the_frame_size_limit() {
        let huge = "x".repeat(max_frame_bytes(10) + 1);
        assert!(matches!(Envelope::decode(&huge, 10), Err(DecodeError::TooLarge { .. })));
    }

    #[test]
    fn rejects_spoofed_hellos() {
        let me = device();
        let spoofed = Envelope::new(DeviceId::random(), Message::Hello(hello(&me)));
        let err = Envelope::decode(&spoofed.encode(), DEFAULT_MAX_ITEM_BYTES).unwrap_err();
        assert!(matches!(err, DecodeError::Invalid(_)));
    }

    #[test]
    fn hello_fields_over_the_limit_are_refused_and_the_limit_itself_is_fine() {
        let me = device();
        let decode = |hello: Hello, ack: bool| {
            let message = if ack { Message::HelloAck(hello) } else { Message::Hello(hello) };
            Envelope::decode(&Envelope::new(me.id, message).encode(), DEFAULT_MAX_ITEM_BYTES)
        };
        let max = MAX_HELLO_FIELD_BYTES;
        type Change = fn(&mut Hello, String);
        let fields: [(&str, Change); 4] = [
            ("name", |hello, value| hello.name = value),
            ("os", |hello, value| hello.os = value),
            ("os_version", |hello, value| hello.os_version = value),
            ("app_version", |hello, value| hello.app_version = value),
        ];
        for ack in [false, true] {
            for (label, change) in fields {
                let mut at_limit = hello(&me);
                change(&mut at_limit, "x".repeat(max));
                assert!(decode(at_limit, ack).is_ok(), "{label}: {max} bytes are allowed (ack: {ack})");
                let mut over = hello(&me);
                change(&mut over, "x".repeat(max + 1));
                let err = decode(over, ack).unwrap_err();
                assert!(matches!(err, DecodeError::Invalid(_)) && !err.is_ignorable(), "{label}: {err}");
            }
            // Bytes, not characters: 65 × "é" is 130 bytes.
            let mut wide = hello(&me);
            wide.name = "é".repeat(max / 2 + 1);
            assert!(wide.name.chars().count() <= max && matches!(decode(wide, ack), Err(DecodeError::Invalid(_))));
            let mut wide = hello(&me);
            wide.name = "é".repeat(max / 2);
            assert!(decode(wide, ack).is_ok());

            let mut caps = hello(&me);
            caps.capabilities = vec!["c".repeat(max); 32];
            assert!(decode(caps, ack).is_ok(), "32 capabilities of {max} bytes");
            let mut caps = hello(&me);
            caps.capabilities = vec!["c".into(); 33];
            assert!(matches!(decode(caps, ack), Err(DecodeError::Invalid(_))), "33 capabilities");
            let mut caps = hello(&me);
            caps.capabilities = vec!["c".repeat(max + 1)];
            assert!(matches!(decode(caps, ack), Err(DecodeError::Invalid(_))), "a long capability");
        }
    }

    #[test]
    fn settings_and_config_updates_validate_on_the_wire() {
        let issuer = DeviceId::random();
        let update = |by: DeviceId, ttl_secs: Option<u64>| {
            let version = crate::remote::SettingsVersion { counter: 1, at: UnixMillis(2), by };
            ConfigUpdate {
                target: DeviceId::random(),
                mode: SyncMode::Both,
                ttl_secs,
                version,
                signature: Bytes([1; 64]),
            }
        };

        // Both travel inside `encrypted` as their own snake_case kind.
        let settings = DeviceSettings {
            mode: SyncMode::Receive,
            ttl_secs: Some(300),
            paused_until: None,
            version: crate::remote::SettingsVersion::initial(issuer),
        };
        let value = serde_json::to_value(Inner::Settings(settings)).unwrap();
        assert_eq!(value["type"], "settings");
        assert_eq!(serde_json::from_value::<Inner>(value).unwrap(), Inner::Settings(settings));
        let sent = update(issuer, None);
        let value = serde_json::to_value(Inner::ConfigUpdate(sent.clone())).unwrap();
        assert_eq!(value["type"], "config_update");
        assert_eq!(serde_json::from_value::<Inner>(value).unwrap(), Inner::ConfigUpdate(sent));

        // The issuer (version.by) must be the sender; updates are never relayed.
        let relayed = Inner::ConfigUpdate(update(DeviceId::random(), None));
        assert!(matches!(relayed.validate(issuer, DEFAULT_MAX_ITEM_BYTES), Err(DecodeError::Invalid(_))));
        assert!(Inner::ConfigUpdate(update(issuer, None)).validate(issuer, DEFAULT_MAX_ITEM_BYTES).is_ok());

        // Bad TTLs are rejected wherever they came from.
        for ttl_secs in [Some(0), Some(crate::remote::MAX_TTL_SECS + 1)] {
            let bad = Inner::ConfigUpdate(update(issuer, ttl_secs));
            assert!(bad.validate(issuer, DEFAULT_MAX_ITEM_BYTES).is_err(), "{ttl_secs:?}");
        }

        // Settings carry the same TTL rule.
        let mut bad_settings = DeviceSettings {
            mode: SyncMode::Both,
            ttl_secs: Some(0),
            paused_until: None,
            version: crate::remote::SettingsVersion::initial(issuer),
        };
        assert!(Inner::Settings(bad_settings).validate(issuer, DEFAULT_MAX_ITEM_BYTES).is_err());
        bad_settings.ttl_secs = Some(300);
        assert!(Inner::Settings(bad_settings).validate(issuer, DEFAULT_MAX_ITEM_BYTES).is_ok());
    }

    #[test]
    fn member_lists_over_the_size_caps_fail_validation() {
        use crate::chain::{MAX_EVENTS, MAX_FIELD_BYTES, MemberRecord, Revocation};

        let from = DeviceId::random();
        let record = |name: &str| MemberRecord {
            device_id: DeviceId::random(),
            device_key: Bytes([1; 32]),
            name: name.into(),
            os: "linux".into(),
            node_id: "nCachy000CNTRL".into(),
            added_at: UnixMillis(5),
            approved_by: from,
            signature: Bytes([0; 64]),
        };
        let revocation = || Revocation {
            device_id: DeviceId::random(),
            revoked_at: UnixMillis(6),
            revoked_by: from,
            signature: Bytes([0; 64]),
        };
        let check = |list: MemberList| Inner::Members(list).validate(from, DEFAULT_MAX_ITEM_BYTES);

        let records = |n: usize| vec![record("cachy-desktop"); n];
        assert!(check(MemberList { records: records(MAX_EVENTS), revocations: Vec::new() }).is_ok());
        assert!(matches!(
            check(MemberList { records: records(MAX_EVENTS + 1), revocations: Vec::new() }),
            Err(DecodeError::Invalid(_))
        ));
        // Approvals and removals count together.
        let revocations = |n: usize| (0..n).map(|_| revocation()).collect::<Vec<_>>();
        assert!(check(MemberList { records: records(1000), revocations: revocations(MAX_EVENTS - 1000) }).is_ok());
        assert!(matches!(
            check(MemberList { records: records(1000), revocations: revocations(MAX_EVENTS - 999) }),
            Err(DecodeError::Invalid(_))
        ));

        let long = "n".repeat(MAX_FIELD_BYTES + 1);
        let exact = "n".repeat(MAX_FIELD_BYTES);
        let list = |record: MemberRecord| MemberList { records: vec![record], revocations: Vec::new() };
        assert!(check(list(record(&exact))).is_ok());
        assert!(matches!(check(list(record(&long))), Err(DecodeError::Invalid(_))), "name");
        let mut over = record("ok");
        over.os = long.clone();
        assert!(matches!(check(list(over)), Err(DecodeError::Invalid(_))), "os");
        let mut over = record("ok");
        over.node_id = long;
        assert!(matches!(check(list(over)), Err(DecodeError::Invalid(_))), "node_id");
        let mut at_limit = record("ok");
        at_limit.os = exact.clone();
        at_limit.node_id = exact;
        assert!(check(list(at_limit)).is_ok());
    }
}
