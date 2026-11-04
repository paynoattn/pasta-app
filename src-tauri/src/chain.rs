//! Creating, joining and leaving the sync chain, and approving devices (#20, #22).
//!
//! The chain secret lives in the keychain entry (#23), the member list in the
//! encrypted store (#25). Until the real setup flow (#46) and Devices screen
//! (#43) land, the settings window has a small developer panel for these.

use std::sync::Arc;
use std::time::Duration;

use pasta_core::chain::MemberList;
use pasta_core::device::{DeviceId, UnixMillis};
use pasta_core::wire::Key32;
use pasta_crypto::membership::{Candidate, Membership, Root};
use pasta_crypto::{ChainKeys, keys};
use pasta_net::network::PeerSnapshot;
use pasta_net::trust::{ChainSetup, ChainStatus};
use pasta_store::Database;
use pasta_store::secrets::DeviceSecrets;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::runtime::Runtime;

const ROW: &str = "chain";

/// The member list as stored, tagged with the chain it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChainRecord {
    chain_id: String,
    /// The pinned root, device id and key.
    root: Option<Root>,
    members: MemberList,
}

/// The chain this device was in when it last ran. A member list that can't be
/// read is an error, not a reason to wait for approval again: a waiting device
/// trusts whoever approves it first.
pub fn load(secrets: &DeviceSecrets, db: &Database) -> Result<Option<ChainSetup>, String> {
    let Some(secret) = secrets.chain.as_ref() else { return Ok(None) };
    let keys = Arc::new(ChainKeys::from_secret(secret.duplicate()));
    let record = db.get::<ChainRecord>(ROW).map_err(|err| format!("couldn't read the member list: {err}"))?;
    let membership = match record {
        Some(record) if record.chain_id == keys.id().to_hex() => {
            Membership::restore(keys.id(), record.root, &record.members)
        }
        _ => Membership::joining(keys.id()),
    };
    Ok(Some(ChainSetup { keys, membership }))
}

/// Log hashes use the chain's key, so devices in one chain can match up their logs.
pub fn apply_log_key(keys: Option<&ChainKeys>) {
    pasta_core::clip::set_log_key(keys.map(|keys| keys.log_key().expose()));
}

/// What the developer chain panel shows. Mirrored in `src/ipc/chain.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainView {
    /// `none`, `waiting` (knows the words, not approved yet) or `member`.
    pub state: &'static str,
    /// `ABCD-EFGH`: the same on every device in the chain.
    pub fingerprint: Option<String>,
    /// First 8 hex digits of this device's key: what approvers see next to its name.
    pub this_key: String,
    pub members: Vec<MemberView>,
    pub pending: Vec<PendingView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub device_id: String,
    pub name: String,
    pub os: String,
    pub this_device: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingView {
    pub device_id: String,
    pub name: String,
    pub os: String,
    /// The Tailscale device it connected from, as Tailscale names it.
    pub node_name: String,
    /// First 8 hex digits of its device key, to compare with the joining device.
    pub key: String,
    pub was_removed: bool,
}

impl ChainView {
    pub fn new(status: &ChainStatus, peers: &[PeerSnapshot], me: DeviceId, my_key: &Key32) -> Self {
        let this_key = keys::short(my_key);
        let Some(chain) = &status.chain else {
            return Self { state: "none", fingerprint: None, this_key, members: Vec::new(), pending: Vec::new() };
        };
        let connected = |id: DeviceId| peers.iter().any(|peer| peer.device_id == id && peer.connected);
        let mut members: Vec<MemberView> = chain
            .members
            .iter()
            .map(|member| MemberView {
                device_id: member.device_id.to_string(),
                name: member.name.clone(),
                os: member.os.clone(),
                this_device: member.device_id == me,
                connected: member.device_id == me || connected(member.device_id),
            })
            .collect();
        members.sort_by(|a, b| b.this_device.cmp(&a.this_device).then_with(|| a.name.cmp(&b.name)));
        Self {
            state: if chain.approved { "member" } else { "waiting" },
            fingerprint: Some(chain.id.fingerprint()),
            this_key,
            members,
            pending: chain
                .pending
                .iter()
                .map(|pending| PendingView {
                    device_id: pending.device_id.to_string(),
                    name: pending.name.clone(),
                    os: pending.os.clone(),
                    node_name: pending.node_name.clone(),
                    key: keys::short(&pending.device_key),
                    was_removed: pending.was_removed,
                })
                .collect(),
        }
    }
}

impl Runtime {
    /// Starts a new chain with this device as its only member. Returns the words.
    pub async fn create_chain(&self) -> Result<Zeroizing<String>, String> {
        self.require_no_chain()?;
        let keys = Arc::new(ChainKeys::generate());
        let me = Candidate {
            device_id: self.device.id,
            device_key: self.device_key.public(),
            name: self.device.name.clone(),
            os: self.device.os.clone(),
            node_id: self.own_node_id().await,
        };
        let membership = Membership::create(keys.id(), &self.device_key, me, UnixMillis::now());
        let words = keys.words();
        self.adopt_chain(ChainSetup { keys, membership }).await?;
        tracing::info!("created a sync chain");
        Ok(words)
    }

    /// Joins the chain the words belong to; a member then has to approve this device.
    pub async fn join_chain(&self, words: &str) -> Result<(), String> {
        self.require_no_chain()?;
        let keys = Arc::new(ChainKeys::from_words(words).map_err(|err| err.to_string())?);
        let membership = Membership::joining(keys.id());
        self.adopt_chain(ChainSetup { keys, membership }).await?;
        tracing::info!("joined a sync chain; waiting for approval");
        Ok(())
    }

    /// Tells the other members, then forgets the chain.
    pub async fn leave_chain(&self) -> Result<(), String> {
        if self.current_chain().is_none() {
            return Err("this device isn't in a sync chain".into());
        }
        self.network.leave().await?;
        self.forget_chain().await
    }

    /// The 24 words, to add another device.
    pub fn chain_words(&self) -> Result<Zeroizing<String>, String> {
        self.current_chain().map(|keys| keys.words()).ok_or_else(|| "this device isn't in a sync chain".into())
    }

    /// `key` is the fingerprint the user compared with the joining device's screen.
    pub async fn approve_device(&self, device: DeviceId, key: &str) -> Result<(), String> {
        self.network.approve(device, key).await
    }

    pub async fn decline_device(&self, device: DeviceId) -> Result<(), String> {
        self.network.decline(device).await
    }

    pub async fn remove_device(&self, device: DeviceId) -> Result<(), String> {
        if device == self.device.id {
            return Err("use Leave to take this device out of the chain".into());
        }
        self.network.remove(device).await
    }

    pub(crate) fn save_members(&self, root: Option<Root>, members: MemberList) {
        let Some(keys) = self.current_chain() else { return };
        let record = ChainRecord { chain_id: keys.id().to_hex(), root, members };
        if let Err(err) = self.db.put(ROW, &record) {
            tracing::error!(error = %err, "couldn't save the member list");
        }
    }

    /// Wipes the chain secret and member list; the network has already left.
    pub(crate) async fn forget_chain(&self) -> Result<(), String> {
        self.save_chain_secret(None).await?;
        if let Err(err) = self.db.delete(ROW) {
            tracing::warn!(error = %err, "couldn't delete the member list");
        }
        *self.chain_keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        apply_log_key(None);
        self.network.set_chain(None).await;
        tracing::info!("forgot the sync chain");
        Ok(())
    }

    async fn adopt_chain(&self, setup: ChainSetup) -> Result<(), String> {
        self.save_chain_secret(Some(&setup.keys)).await?;
        *self.chain_keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&setup.keys));
        let record = ChainRecord {
            chain_id: setup.keys.id().to_hex(),
            root: setup.membership.pinned_root(),
            members: setup.membership.to_list(),
        };
        if let Err(err) = self.db.put(ROW, &record) {
            tracing::error!(error = %err, "couldn't save the member list");
        }
        apply_log_key(Some(&setup.keys));
        self.network.set_chain(Some(setup)).await;
        Ok(())
    }

    /// Stores (or removes) the chain secret in the keychain entry.
    async fn save_chain_secret(&self, keys: Option<&ChainKeys>) -> Result<(), String> {
        let mut secrets = self.secrets.lock().await;
        let previous = std::mem::replace(&mut secrets.chain, keys.map(|keys| keys.secret().duplicate()));
        let blob = secrets.encode();
        let store = Arc::clone(&self.secret_store);
        let saved = tokio::task::spawn_blocking(move || store.save(&blob)).await.map_err(|err| err.to_string())?;
        if let Err(err) = saved {
            secrets.chain = previous;
            return Err(format!("couldn't save to the keychain: {err}"));
        }
        Ok(())
    }

    fn current_chain(&self) -> Option<Arc<ChainKeys>> {
        self.chain_keys.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn require_no_chain(&self) -> Result<(), String> {
        match self.current_chain() {
            Some(_) => Err("this device is already in a sync chain; leave it first".into()),
            None => Ok(()),
        }
    }

    /// This machine's Tailscale stable id, for its own member record. Empty if Tailscale can't say.
    async fn own_node_id(&self) -> String {
        let Ok(tailscale) = self.tailscale.get() else { return String::new() };
        match tokio::time::timeout(Duration::from_secs(3), tailscale.status()).await {
            Ok(Ok(status)) => status.self_node.map(|node| node.stable_id).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use pasta_crypto::DeviceKey;
    use pasta_crypto::Secret32;
    use pasta_net::trust::{ChainSummary, PendingDevice};

    use super::*;

    #[test]
    fn restores_the_chain_it_belongs_to() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("pasta.db"), Secret32::random()).unwrap();
        let mut secrets = DeviceSecrets::generate(DeviceId::random());
        assert!(load(&secrets, &db).unwrap().is_none(), "no chain secret, no chain");

        let keys = ChainKeys::generate();
        secrets.chain = Some(keys.secret().duplicate());
        let key = DeviceKey::from_seed(secrets.device_key.duplicate());
        let me = Candidate {
            device_id: secrets.device_id,
            device_key: key.public(),
            name: "a".into(),
            os: "test".into(),
            node_id: String::new(),
        };
        let membership = Membership::create(keys.id(), &key, me, UnixMillis(1));
        let record =
            ChainRecord { chain_id: keys.id().to_hex(), root: membership.pinned_root(), members: membership.to_list() };
        db.put(ROW, &record).unwrap();
        let setup = load(&secrets, &db).unwrap().unwrap();
        assert_eq!(setup.membership, membership);

        // A member list from another chain isn't used.
        let other = ChainKeys::generate();
        secrets.chain = Some(other.secret().duplicate());
        let setup = load(&secrets, &db).unwrap().unwrap();
        assert_eq!(setup.membership.root(), None);
    }

    #[test]
    fn view_lists_this_device_first_and_shows_pending_keys() {
        let me = DeviceId::random();
        let key = DeviceKey::generate();
        let keys = ChainKeys::generate();
        let membership = Membership::create(
            keys.id(),
            &key,
            Candidate {
                device_id: me,
                device_key: key.public(),
                name: "zz".into(),
                os: "macos".into(),
                node_id: String::new(),
            },
            UnixMillis(1),
        );
        let pending_key = DeviceKey::generate().public();
        let status = ChainStatus {
            chain: Some(ChainSummary {
                id: keys.id(),
                approved: true,
                members: membership.members().cloned().collect(),
                pending: vec![PendingDevice {
                    device_id: DeviceId::random(),
                    name: "steamdeck".into(),
                    os: "linux".into(),
                    device_key: pending_key,
                    node_id: "nPeer000CNTRL".into(),
                    node_name: "cachy-desktop".into(),
                    was_removed: false,
                }],
            }),
        };
        let view = ChainView::new(&status, &[], me, &key.public());
        assert_eq!(view.state, "member");
        assert_eq!(view.fingerprint.as_deref(), Some(keys.id().fingerprint().as_str()));
        assert!(view.members[0].this_device && view.members[0].connected);
        assert_eq!(view.pending[0].key, keys::short(&pending_key));
        assert_eq!(view.this_key, keys::short(&key.public()));
        assert_eq!(ChainView::new(&ChainStatus::default(), &[], me, &key.public()).state, "none");
    }

    #[test]
    fn load_fails_instead_of_waiting_for_approval_when_the_chain_row_cant_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pasta.db");
        let db = Database::open(&path, Secret32::random()).unwrap();
        let mut secrets = DeviceSecrets::generate(DeviceId::random());
        let keys = ChainKeys::generate();
        secrets.chain = Some(keys.secret().duplicate());

        // No row yet: a device that just typed the words, which waits for approval.
        assert_eq!(load(&secrets, &db).unwrap().unwrap().membership.root(), None);

        // A sealed value that opens but isn't a member list.
        db.put(ROW, &"not a chain record").unwrap();
        let err = load(&secrets, &db).unwrap_err();
        assert!(err.contains("couldn't read the member list"), "{err}");

        // A sealed value that doesn't open at all.
        let record = ChainRecord { chain_id: keys.id().to_hex(), root: None, members: MemberList::default() };
        db.put(ROW, &record).unwrap();
        assert!(load(&secrets, &db).is_ok(), "the row is fine before it is damaged");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("UPDATE kv SET value = zeroblob(40) WHERE name = 'chain'", [])
            .unwrap();
        assert!(load(&secrets, &db).is_err(), "a damaged row must not look like a fresh joiner");

        // Without a chain secret there is nothing to restore, whatever the row holds.
        secrets.chain = None;
        assert!(load(&secrets, &db).unwrap().is_none());
    }
}
