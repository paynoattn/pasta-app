//! Sync-chain membership data (#22): who is in the chain, who approved them,
//! who was removed. Only data and its canonical signing bytes live here; the
//! signatures are made and checked in `pasta-crypto`.
//!
//! Every record is signed by the member that approved it, back to the device
//! that created the chain (whose record approves itself). A revocation is
//! signed by any member. Lists from different devices merge by union and are
//! replayed in time order; see `pasta_crypto::membership` for the rules.

use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, UnixMillis};
use crate::wire::{Key32, Sig64};

/// Most events (approvals plus removals) one member list may carry.
pub const MAX_EVENTS: usize = 1024;
/// Longest device name, OS or node id a record may carry, in bytes.
pub const MAX_FIELD_BYTES: usize = 128;

/// One approved device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRecord {
    pub device_id: DeviceId,
    /// The device's Ed25519 public key.
    pub device_key: Key32,
    pub name: String,
    pub os: String,
    /// Tailscale stable node id the device was approved on (#11).
    pub node_id: String,
    pub added_at: UnixMillis,
    /// The member that approved this device; the creator approves itself.
    pub approved_by: DeviceId,
    /// By `approved_by`'s key over [`MemberRecord::signing_bytes`].
    pub signature: Sig64,
}

impl MemberRecord {
    pub fn is_root(&self) -> bool {
        self.approved_by == self.device_id
    }

    /// What the approver signs. Binds the record to one chain and to that
    /// chain's root key, so it can't be replayed into a list with another root.
    pub fn signing_bytes(&self, chain_id: &[u8; 16], root_key: &Key32) -> Vec<u8> {
        let mut out = Vec::with_capacity(192 + self.name.len() + self.os.len() + self.node_id.len());
        out.extend_from_slice(b"pasta/v2/member\0");
        out.extend_from_slice(chain_id);
        out.extend_from_slice(&root_key.0);
        out.extend_from_slice(self.device_id.0.as_bytes());
        out.extend_from_slice(&self.device_key.0);
        push_str(&mut out, &self.name);
        push_str(&mut out, &self.os);
        push_str(&mut out, &self.node_id);
        out.extend_from_slice(&self.added_at.0.to_be_bytes());
        out.extend_from_slice(self.approved_by.0.as_bytes());
        out
    }
}

/// A removed device (or one that left). A later approval can bring it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub device_id: DeviceId,
    pub revoked_at: UnixMillis,
    pub revoked_by: DeviceId,
    pub signature: Sig64,
}

impl Revocation {
    pub fn signing_bytes(&self, chain_id: &[u8; 16], root_key: &Key32) -> Vec<u8> {
        let mut out = Vec::with_capacity(112);
        out.extend_from_slice(b"pasta/v2/revoke\0");
        out.extend_from_slice(chain_id);
        out.extend_from_slice(&root_key.0);
        out.extend_from_slice(self.device_id.0.as_bytes());
        out.extend_from_slice(&self.revoked_at.0.to_be_bytes());
        out.extend_from_slice(self.revoked_by.0.as_bytes());
        out
    }
}

/// What members exchange (inside an `encrypted` message) and persist.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemberList {
    #[serde(default)]
    pub records: Vec<MemberRecord>,
    #[serde(default)]
    pub revocations: Vec<Revocation>,
}

impl MemberList {
    /// Within the size limits, so a peer can't make this device store or check without bound.
    pub fn within_limits(&self) -> bool {
        self.records.len() + self.revocations.len() <= MAX_EVENTS
            && self.records.iter().all(|record| {
                record.name.len() <= MAX_FIELD_BYTES
                    && record.os.len() <= MAX_FIELD_BYTES
                    && record.node_id.len() <= MAX_FIELD_BYTES
            })
    }
}

/// Length-prefixed, so `("ab", "c")` and `("a", "bc")` sign differently.
fn push_str(out: &mut Vec<u8>, value: &str) {
    let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Bytes;

    fn record(name: &str, os: &str) -> MemberRecord {
        let id = DeviceId::random();
        MemberRecord {
            device_id: id,
            device_key: Bytes([1; 32]),
            name: name.into(),
            os: os.into(),
            node_id: "nMac0000CNTRL".into(),
            added_at: UnixMillis(5),
            approved_by: id,
            signature: Bytes([0; 64]),
        }
    }

    #[test]
    fn signing_bytes_separate_fields_and_chains() {
        let a = record("ab", "c");
        let mut b = a.clone();
        b.name = "a".into();
        b.os = "bc".into();
        assert_ne!(a.signing_bytes(&[0; 16], &Bytes([1; 32])), b.signing_bytes(&[0; 16], &Bytes([1; 32])));
        assert_ne!(a.signing_bytes(&[0; 16], &Bytes([1; 32])), a.signing_bytes(&[1; 16], &Bytes([1; 32])));
        assert!(a.is_root());
    }
}
