//! This device's identity and sync chain, as the network uses them (#20–#22).
//!
//! [`Trust`] answers the questions every connection asks: is the peer in the
//! same chain (its `hello` carries a valid chain proof), did it sign this
//! handshake, is it an approved member, and it seals and opens `encrypted`
//! messages. The actor is the only writer of the membership; connection
//! tasks read it.

use std::sync::{Arc, Mutex, RwLock};

use pasta_core::chain::{MemberList, MemberRecord};
use pasta_core::device::{DeviceId, DeviceInfo, UnixMillis};
use pasta_core::protocol::{Envelope, Hello, Inner, Role, handshake_transcript};
use pasta_core::wire::{Key32, Sig64};
use pasta_crypto::membership::{Candidate, Membership, MembershipError, MergeOutcome, Root};
use pasta_crypto::seal::{OpenError, ReplayGuard};
use pasta_crypto::{ChainId, ChainKeys, DeviceKey};

/// A chain to sync in: its keys and what this device knows of its members.
#[derive(Clone)]
pub struct ChainSetup {
    pub keys: Arc<ChainKeys>,
    pub membership: Membership,
}

impl std::fmt::Debug for ChainSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainSetup").field("chain", &self.keys.id()).finish_non_exhaustive()
    }
}

/// The chain as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainStatus {
    /// `None`: not in a sync chain, so nothing syncs.
    pub chain: Option<ChainSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSummary {
    pub id: ChainId,
    /// This device is an approved member. If not, it waits for approval.
    pub approved: bool,
    pub members: Vec<MemberRecord>,
    /// Connected devices with the right words, waiting for someone to approve them.
    pub pending: Vec<PendingDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDevice {
    pub device_id: DeviceId,
    pub name: String,
    pub os: String,
    pub device_key: Key32,
    /// The Tailscale node it connected from: stable id and name.
    pub node_id: String,
    pub node_name: String,
    /// It was a member before and was removed (or left).
    pub was_removed: bool,
}

impl PendingDevice {
    pub(crate) fn candidate(&self) -> Candidate {
        Candidate {
            device_id: self.device_id,
            device_key: self.device_key,
            name: self.name.clone(),
            os: self.os.clone(),
            node_id: self.node_id.clone(),
        }
    }
}

/// How a peer's `hello` relates to this device's chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainMatch {
    /// This device isn't in a chain: it talks to nobody.
    NoChainHere,
    Same,
    /// No proof, or a proof for other words.
    Different,
}

/// Where a connected peer stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerTrust {
    Member,
    /// In the chain, not approved yet (or approved and removed since).
    Pending,
    /// This device itself waits for approval, so it can't tell yet.
    Unknown,
}

struct ChainState {
    keys: Arc<ChainKeys>,
    membership: Membership,
}

pub(crate) struct Trust {
    me: DeviceId,
    key: Arc<DeviceKey>,
    chain: RwLock<Option<ChainState>>,
    replay: Mutex<ReplayGuard>,
}

impl Trust {
    pub(crate) fn new(me: DeviceId, key: Arc<DeviceKey>, chain: Option<ChainSetup>) -> Self {
        let chain = chain.map(|setup| ChainState { keys: setup.keys, membership: setup.membership });
        Self { me, key, chain: RwLock::new(chain), replay: Mutex::new(ReplayGuard::default()) }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Option<ChainState>> {
        self.chain.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Option<ChainState>> {
        self.chain.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn in_chain(&self) -> bool {
        self.read().is_some()
    }

    /// A fresh `hello` for one connection: new nonce, chain proof if in a chain.
    pub(crate) fn hello(&self, device: &DeviceInfo) -> Hello {
        let mut hello = Hello::new(device, self.key.public(), pasta_crypto::random_key32());
        if let Some(chain) = self.read().as_ref() {
            hello.chain_proof = Some(chain.keys.chain_proof(self.me.0.as_bytes(), &hello.device_key, &hello.nonce));
        }
        hello
    }

    pub(crate) fn chain_match(&self, hello: &Hello) -> ChainMatch {
        let chain = self.read();
        let Some(chain) = chain.as_ref() else { return ChainMatch::NoChainHere };
        let proven = hello.chain_proof.as_ref().is_some_and(|proof| {
            chain.keys.verify_chain_proof(hello.device_id.0.as_bytes(), &hello.device_key, &hello.nonce, proof)
        });
        if proven { ChainMatch::Same } else { ChainMatch::Different }
    }

    pub(crate) fn sign_handshake(&self, role: Role, dialer: &Hello, listener: &Hello) -> Sig64 {
        self.key.sign(&handshake_transcript(role, dialer, listener))
    }

    /// Checks the other side's signature with the key from its own `hello`.
    pub(crate) fn verify_handshake(role: Role, dialer: &Hello, listener: &Hello, signature: &Sig64) -> bool {
        let signer = match role {
            Role::Dialer => &dialer.device_key,
            Role::Listener => &listener.device_key,
        };
        pasta_crypto::keys::verify(signer, &handshake_transcript(role, dialer, listener), signature)
    }

    /// The member record for `device`, if it's a current member.
    pub(crate) fn member(&self, device: DeviceId) -> Option<MemberRecord> {
        self.read().as_ref().and_then(|chain| chain.membership.member(device).cloned())
    }

    pub(crate) fn am_member(&self) -> bool {
        self.read().as_ref().is_some_and(|chain| chain.membership.member(self.me).is_some())
    }

    /// Whether this device would send `device` items and accept its items.
    pub(crate) fn trusts(&self, device: DeviceId) -> bool {
        self.read().as_ref().is_some_and(|chain| {
            chain.membership.member(self.me).is_some_and(|me| me.device_key == self.key.public())
                && chain.membership.is_member(device)
        })
    }

    pub(crate) fn peer_trust(&self, device: DeviceId) -> PeerTrust {
        let chain = self.read();
        let Some(chain) = chain.as_ref() else { return PeerTrust::Unknown };
        if chain.membership.root().is_none() {
            PeerTrust::Unknown
        } else if chain.membership.is_member(device) {
            PeerTrust::Member
        } else {
            PeerTrust::Pending
        }
    }

    pub(crate) fn was_removed(&self, device: DeviceId) -> bool {
        self.read().as_ref().is_some_and(|chain| chain.membership.is_revoked(device))
    }

    pub(crate) fn waiting(&self) -> bool {
        self.read().as_ref().is_some_and(|chain| chain.membership.root().is_none())
    }

    /// For a waiting device: the membership `list` would give it, if any.
    pub(crate) fn offer(&self, list: &MemberList) -> Option<Membership> {
        let chain = self.read();
        chain.as_ref()?.membership.offer(list, self.me, &self.key.public(), UnixMillis::now())
    }

    /// Takes the membership an approval gave this device. False if no longer waiting.
    pub(crate) fn adopt(&self, membership: Membership) -> bool {
        let mut chain = self.write();
        match chain.as_mut() {
            Some(chain)
                if chain.membership.root().is_none() && chain.membership.chain_id() == membership.chain_id() =>
            {
                chain.membership = membership;
                true
            }
            _ => false,
        }
    }

    /// Seals `inner` for the chain; `None` outside a chain.
    pub(crate) fn seal(&self, inner: &Inner) -> Option<Envelope> {
        self.read().as_ref().map(|chain| chain.keys.seal_envelope(self.me, inner))
    }

    /// Decrypts, then refuses replays. Errors carry no content.
    pub(crate) fn open(&self, envelope: &Envelope) -> Result<Inner, OpenError> {
        let inner = {
            let chain = self.read();
            let Some(chain) = chain.as_ref() else { return Err(OpenError::Decrypt) };
            chain.keys.open_envelope(envelope)?
        };
        self.replay.lock().unwrap_or_else(std::sync::PoisonError::into_inner).check(envelope, UnixMillis::now())?;
        Ok(inner)
    }

    pub(crate) fn members_list(&self) -> Option<(Option<Root>, MemberList)> {
        self.read().as_ref().map(|chain| (chain.membership.pinned_root(), chain.membership.to_list()))
    }

    pub(crate) fn merge(&self, list: &MemberList) -> MergeOutcome {
        let mut chain = self.write();
        let Some(chain) = chain.as_mut() else { return MergeOutcome::default() };
        chain.membership.merge(list, UnixMillis::now())
    }

    pub(crate) fn approve(&self, candidate: Candidate) -> Result<(), MembershipError> {
        let mut chain = self.write();
        let Some(chain) = chain.as_mut() else { return Err(MembershipError::NotAMember) };
        chain.membership.approve(&self.key, self.me, candidate, UnixMillis::now()).map(|_| ())
    }

    pub(crate) fn revoke(&self, device: DeviceId) -> Result<(), MembershipError> {
        let mut chain = self.write();
        let Some(chain) = chain.as_mut() else { return Err(MembershipError::NotAMember) };
        chain.membership.revoke(&self.key, self.me, device, UnixMillis::now())
    }

    pub(crate) fn set_chain(&self, setup: Option<ChainSetup>) {
        *self.write() = setup.map(|setup| ChainState { keys: setup.keys, membership: setup.membership });
        *self.replay.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = ReplayGuard::default();
    }

    pub(crate) fn summary(&self, pending: Vec<PendingDevice>) -> ChainStatus {
        let chain = self.read();
        ChainStatus {
            chain: chain.as_ref().map(|chain| ChainSummary {
                id: chain.keys.id(),
                approved: chain.membership.member(self.me).is_some(),
                members: chain.membership.members().cloned().collect(),
                pending,
            }),
        }
    }
}
