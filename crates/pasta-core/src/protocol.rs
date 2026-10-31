//! Wire protocol v1 (#7): one JSON envelope per WebSocket text frame.
//!
//! ```json
//! { "v": 1, "type": "clip", "msg_id": "…", "from_device": "…", "sent_at": 1730000000000, "body": { … } }
//! ```
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

use crate::clip::{ContentHash, Flavors, SourceApp};
use crate::device::{DeviceId, DeviceInfo, UnixMillis};

/// Major version of the wire protocol. Peers with another version refuse the handshake.
pub const PROTOCOL_VERSION: u16 = 1;

/// Default limit for one clipboard item, all flavors together.
pub const DEFAULT_MAX_ITEM_BYTES: usize = 1024 * 1024;

/// Message types reserved for later milestones: remote config (#31), device
/// approval (#22) and the E2E-encrypted wrapper (#21). This build rejects them
/// as unknown.
pub const RESERVED_TYPES: &[&str] = &["config_update", "device_approval", "encrypted"];

/// Upper bound for one encoded frame given the item limit: JSON escaping can
/// grow text up to six times (`\u0000`), plus room for the envelope.
pub fn max_frame_bytes(max_item_bytes: usize) -> usize {
    max_item_bytes.saturating_mul(6).saturating_add(64 * 1024)
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
    /// Reply to `hello` from the side that accepted it.
    HelloAck(Hello),
    Clip(Clip),
    /// Receipt for a `clip`.
    Ack(Ack),
    Ping(Ping),
    Pong(Pong),
}

impl Message {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::HelloAck(_) => "hello_ack",
            Self::Clip(_) => "clip",
            Self::Ack(_) => "ack",
            Self::Ping(_) => "ping",
            Self::Pong(_) => "pong",
        }
    }
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
}

impl Hello {
    pub fn from_device(device: &DeviceInfo) -> Self {
        Self {
            device_id: device.id,
            name: device.name.clone(),
            os: device.os.clone(),
            os_version: device.os_version.clone(),
            app_version: device.app_version.clone(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: Vec::new(),
        }
    }
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
            "clip" => Message::Clip(parse_body(body)?),
            "ack" => Message::Ack(parse_body(body)?),
            "ping" => Message::Ping(parse_body(body)?),
            "pong" => Message::Pong(parse_body(body)?),
            other => return Err(DecodeError::UnknownType(other.chars().take(32).collect())),
        };
        let envelope = Self { v, msg_id: raw.msg_id, from_device: raw.from_device, sent_at: raw.sent_at, message };
        envelope.validate(max_item_bytes)?;
        Ok(envelope)
    }

    fn validate(&self, max_item_bytes: usize) -> Result<(), DecodeError> {
        match &self.message {
            Message::Hello(hello) | Message::HelloAck(hello) => {
                if hello.device_id != self.from_device {
                    return Err(DecodeError::Invalid("hello device_id differs from the envelope's from_device"));
                }
                if hello.protocol_version != PROTOCOL_VERSION {
                    return Err(DecodeError::UnsupportedVersion { theirs: hello.protocol_version });
                }
            }
            Message::Clip(clip) => {
                if clip.origin_device != self.from_device {
                    return Err(DecodeError::Invalid("clip origin_device differs from the sender"));
                }
                clip.validate(max_item_bytes)?;
            }
            Message::Ack(_) | Message::Ping(_) | Message::Pong(_) => {}
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

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::random(),
            name: "macbook-pro".into(),
            os: "macos".into(),
            os_version: "27.2".into(),
            app_version: "0.1.0".into(),
        }
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
        let messages = [
            Message::Hello(Hello::from_device(&me)),
            Message::HelloAck(Hello::from_device(&me)),
            Message::Clip(clip(me.id, "hello")),
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
        let envelope = Envelope::new(me.id, Message::Clip(clip(me.id, "hi")));
        let value: serde_json::Value = serde_json::from_str(&envelope.encode()).unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 6);
        for key in ["v", "type", "msg_id", "from_device", "sent_at", "body"] {
            assert!(keys.contains(&key), "{key}");
        }
        assert_eq!(value["type"], "clip");
        assert_eq!(value["body"]["flavors"]["text/plain"], "hi");
        assert!(value["body"]["flavors"].get("text/rtf").is_none(), "absent flavors are omitted");
    }

    #[test]
    fn decodes_a_ping_without_body() {
        let id = DeviceId::random();
        let frame = format!(r#"{{"v":1,"type":"ping","msg_id":"{}","from_device":"{id}","sent_at":5}}"#, Uuid::nil());
        let envelope = Envelope::decode(&frame, DEFAULT_MAX_ITEM_BYTES).unwrap();
        assert_eq!(envelope.message, Message::Ping(Ping {}));
    }

    #[test]
    fn rejects_other_major_versions_before_parsing_the_rest() {
        let err = Envelope::decode(r#"{"v":2,"something":"entirely different"}"#, DEFAULT_MAX_ITEM_BYTES).unwrap_err();
        assert!(matches!(err, DecodeError::UnsupportedVersion { theirs: 2 }), "{err}");

        let me = device();
        let mut hello = Hello::from_device(&me);
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
                r#"{{"v":1,"type":"{kind}","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{}}}}"#,
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
                r#"{{"v":1,"type":"clip","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{"flavors":{{"text/plain":"{secret}"}}}}}}"#,
                Uuid::nil()
            ),
            format!(
                r#"{{"v":1,"type":"clip","msg_id":"{}","from_device":"{id}","sent_at":5,"body":{{"item_id":"{secret}"}}}}"#,
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
    fn enforces_the_item_size_limit() {
        let me = device();
        let envelope = Envelope::new(me.id, Message::Clip(clip(me.id, &"x".repeat(2000))));
        let err = Envelope::decode(&envelope.encode(), 1000).unwrap_err();
        assert!(matches!(err, DecodeError::TooLarge { .. }), "{err}");

        let huge = "x".repeat(max_frame_bytes(10) + 1);
        assert!(matches!(Envelope::decode(&huge, 10), Err(DecodeError::TooLarge { .. })));
    }

    #[test]
    fn rejects_inconsistent_clips() {
        let me = device();
        let mut tampered = clip(me.id, "hello");
        tampered.flavors.plain = "goodbye".into();
        let err = round_trip_err(Envelope::new(me.id, Message::Clip(tampered)));
        assert!(matches!(err, DecodeError::Invalid(_)), "{err}");

        let relayed = clip(DeviceId::random(), "hello");
        let err = round_trip_err(Envelope::new(me.id, Message::Clip(relayed)));
        assert!(matches!(err, DecodeError::Invalid(_)), "{err}");

        let spoofed = Envelope::new(DeviceId::random(), Message::Hello(Hello::from_device(&me)));
        assert!(matches!(round_trip_err(spoofed), DecodeError::Invalid(_)));
    }

    fn round_trip_err(envelope: Envelope) -> DecodeError {
        Envelope::decode(&envelope.encode(), DEFAULT_MAX_ITEM_BYTES).unwrap_err()
    }
}
