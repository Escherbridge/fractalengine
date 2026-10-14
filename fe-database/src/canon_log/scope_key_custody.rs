//! Durable scope-key custody: key material in `fe_identity::SecretStore`, a non-secret key-id
//! index in SurrealDB. Rationale: `canon_log/AGENTS.md` §durable-authority-sources.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fe_canonical_log::crypto::aead::{SealingKey, SEALING_KEY_LENGTH};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::ScopeKeyStore;
use fe_canonical_log::crypto::CryptoError;
use fe_canonical_log::envelope::{Hlc, Identifier32};
use fe_canonical_log::retention::crypto_shred::{
    crypto_shred, CryptoShredError, DestructionDisposition, ScopeKeyId, ShredLedger,
};
use fe_identity::SecretStore;

use crate::canon_log::canonical_epoch::{scope_from_key, scope_key};
use crate::canon_log::{identifier_from_hex, identifier_to_hex, StorageError};
use crate::repo::Db;

/// The `SecretStore` service every scope key is filed under (the fe-identity namespace).
pub const SCOPE_KEY_SERVICE: &str = "fractalengine";

/// The `SecretStore` account for one scope key: `canon-scope-key/<key_id hex>`.
pub fn scope_key_account(key_id: Identifier32) -> String {
    format!("canon-scope-key/{}", identifier_to_hex(key_id))
}

/// Where a key stands in the durable index. Never carries key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustodyState {
    /// Material is in the `SecretStore` and issuance continues.
    Held,
    /// Material is in the `SecretStore`; no new key or wrap is issued (§10.3.2).
    IssuanceStopped,
    /// Crypto-shredded (SPEC-5 §5.5.4): material deleted, never reloaded.
    Destroyed,
}

impl CustodyState {
    fn label(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::IssuanceStopped => "issuance_stopped",
            Self::Destroyed => "destroyed",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        match label {
            "held" => Some(Self::Held),
            "issuance_stopped" => Some(Self::IssuanceStopped),
            "destroyed" => Some(Self::Destroyed),
            _ => None,
        }
    }
}

/// Every reason a custody change is not durable.
#[derive(Debug, thiserror::Error)]
pub enum CustodyError {
    /// The custody rule refused the change (already present, issuance stopped, bad key id).
    #[error(transparent)]
    Refused(#[from] CryptoError),
    /// The crypto-shred sequence refused (the key is not controlled here).
    #[error(transparent)]
    Shred(#[from] CryptoShredError),
    /// The OS keystore refused; the change is held in-process and still queued.
    #[error("secret store: {0}")]
    SecretStore(String),
    /// The index write failed; the change is held in-process and still queued.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// One queued durable change, applied in order by [`SecretStoreScopeKeyCustody::persist`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum IndexChange {
    /// Secret first, then the index row: a crash between leaves an unindexed secret that is
    /// never loaded, so the failure direction is "key lost", never "key resurrected".
    Insert {
        key: ScopeKeyId,
        key_id: Identifier32,
    },
    /// Index only.
    StopIssuance { key: ScopeKeyId },
    /// Index row FIRST, then the secret delete: a crash between leaves a `destroyed` row whose
    /// residual secret is never reloaded and is deleted again on the next load.
    Destroy {
        key: ScopeKeyId,
        key_id: Identifier32,
    },
    /// Index only: the audit fields of an already destroyed key.
    RecordDisposition { disposition: DestructionDisposition },
}

/// The unified `ScopeKeyStore` + `ShredLedger` over the OS keystore (W4-7).
///
/// Material lives only in [`SecretStore`] (account [`scope_key_account`]) and this process's
/// zeroizing map. SurrealDB holds a non-secret index — scope, epoch, `key_id`, state — because
/// `SecretStore` cannot enumerate, and must never gain a `list` (critic C8/C10). The trait
/// methods are synchronous and change only in-process state, queueing the durable change;
/// [`Self::take_custody`] and [`Self::shred`] are the doors that also persist.
pub struct SecretStoreScopeKeyCustody {
    db: Db,
    secrets: Arc<dyn SecretStore>,
    keys: HashMap<ScopeKeyId, SealingKey>,
    key_ids: HashMap<ScopeKeyId, Identifier32>,
    issuance_stopped: HashSet<ScopeKeyId>,
    destroyed: HashSet<ScopeKeyId>,
    dispositions: Vec<DestructionDisposition>,
    pending: Vec<IndexChange>,
    load_faults: Vec<String>,
}

impl std::fmt::Debug for SecretStoreScopeKeyCustody {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretStoreScopeKeyCustody")
            .field("held", &self.keys.len())
            .field("destroyed", &self.destroyed.len())
            .field("pending", &self.pending.len())
            .finish()
    }
}

impl SecretStoreScopeKeyCustody {
    /// Reads the index and fetches each held key's material from `secrets`.
    ///
    /// A key is held only when its material is present AND derives the indexed `key_id`; any
    /// other row is recorded in [`Self::take_load_faults`] and not held, so an operation sealed
    /// under it quarantines as `PayloadKeyUnavailable`. A destroyed row is never fetched, and its
    /// secret is deleted again in case an earlier shred stopped between its two writes.
    pub async fn load(db: &Db, secrets: Arc<dyn SecretStore>) -> Result<Self, StorageError> {
        let mut response = db
            .query("SELECT scope_key, epoch, key_id_hex, state FROM canon_scope_key_index")
            .await
            .map_err(|error| StorageError::query("scope key index load", error))?
            .check()
            .map_err(|error| StorageError::query("scope key index load", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("scope key index load", error))?;

        let mut custody = Self {
            db: db.clone(),
            secrets,
            keys: HashMap::new(),
            key_ids: HashMap::new(),
            issuance_stopped: HashSet::new(),
            destroyed: HashSet::new(),
            dispositions: Vec::new(),
            pending: Vec::new(),
            load_faults: Vec::new(),
        };
        for row in rows {
            if let Err(fault) = custody.load_row(&row) {
                tracing::warn!("canon_scope_key_index row not held: {fault}");
                custody.load_faults.push(fault);
            }
        }
        Ok(custody)
    }

    /// Takes custody of fresh material and makes it durable.
    pub async fn take_custody(
        &mut self,
        key: ScopeKeyId,
        material: SealingKey,
    ) -> Result<Identifier32, CustodyError> {
        self.insert_scope_key(key.clone(), material)?;
        let key_id = self.key_ids[&key];
        self.persist().await?;
        Ok(key_id)
    }

    /// Runs the SPEC-5 §5.5.4 crypto-shred (stop issuance, destroy, record) and makes it durable.
    pub async fn shred(
        &mut self,
        key: &ScopeKeyId,
        now: Hlc,
    ) -> Result<DestructionDisposition, CustodyError> {
        let disposition = crypto_shred(self, key, now)?;
        self.persist().await?;
        Ok(disposition)
    }

    /// Applies every queued change in order; a failed step and everything after it stay queued.
    pub async fn persist(&mut self) -> Result<(), CustodyError> {
        while let Some(change) = self.pending.first().cloned() {
            self.apply(&change).await?;
            self.pending.remove(0);
        }
        Ok(())
    }

    /// Where `key` stands, or `None` when this custody has never held it.
    pub fn custody_state(&self, key: &ScopeKeyId) -> Option<CustodyState> {
        if self.destroyed.contains(key) {
            Some(CustodyState::Destroyed)
        } else if self.issuance_stopped.contains(key) {
            Some(CustodyState::IssuanceStopped)
        } else if self.keys.contains_key(key) {
            Some(CustodyState::Held)
        } else {
            None
        }
    }

    /// The committed `key_id` of a key this custody holds or destroyed.
    pub fn key_id(&self, key: &ScopeKeyId) -> Option<Identifier32> {
        self.key_ids.get(key).copied()
    }

    /// Destruction dispositions recorded in this process, oldest first.
    pub fn dispositions(&self) -> &[DestructionDisposition] {
        &self.dispositions
    }

    /// How many changes are not yet durable.
    pub fn pending_changes(&self) -> usize {
        self.pending.len()
    }

    /// Index rows [`Self::load`] refused to hold, drained.
    pub fn take_load_faults(&mut self) -> Vec<String> {
        std::mem::take(&mut self.load_faults)
    }

    fn load_row(&mut self, row: &serde_json::Value) -> Result<(), String> {
        let text = |field: &str| {
            row.get(field)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("{field} is not a string"))
        };
        let scope = scope_from_key(text("scope_key")?)?;
        let epoch = row
            .get("epoch")
            .and_then(serde_json::Value::as_i64)
            .and_then(|epoch| u64::try_from(epoch).ok())
            .ok_or_else(|| "epoch is not a non-negative int".to_string())?;
        let key_id_text = text("key_id_hex")?;
        let key_id = identifier_from_hex(key_id_text)
            .ok_or_else(|| format!("bad key id hex {key_id_text}"))?;
        let state = CustodyState::parse(text("state")?)
            .ok_or_else(|| format!("{key_id_text}: unknown custody state"))?;
        let key = ScopeKeyId { scope, epoch };
        self.key_ids.insert(key.clone(), key_id);

        if state == CustodyState::Destroyed {
            self.destroyed.insert(key.clone());
            self.issuance_stopped.insert(key);
            if let Err(error) = self
                .secrets
                .delete(SCOPE_KEY_SERVICE, &scope_key_account(key_id))
            {
                return Err(format!(
                    "{key_id_text}: residual secret not deleted: {error}"
                ));
            }
            return Ok(());
        }
        if state == CustodyState::IssuanceStopped {
            self.issuance_stopped.insert(key.clone());
        }

        let secret = self
            .secrets
            .get(SCOPE_KEY_SERVICE, &scope_key_account(key_id))
            .map_err(|error| format!("{key_id_text}: {error}"))?
            .ok_or_else(|| format!("{key_id_text}: indexed but absent from the secret store"))?;
        let material =
            decode_material(secret).map_err(|error| format!("{key_id_text}: {error}"))?;
        let derived = derive_key_id(&key.scope, key.epoch, &material)
            .map_err(|error| format!("{key_id_text}: {error}"))?;
        if derived != key_id {
            return Err(format!(
                "{key_id_text}: stored material derives {}",
                identifier_to_hex(derived)
            ));
        }
        self.keys.insert(key, material);
        Ok(())
    }

    async fn apply(&self, change: &IndexChange) -> Result<(), CustodyError> {
        match change {
            IndexChange::Insert { key, key_id } => {
                let Some(material) = self.keys.get(key) else {
                    // Destroyed before it was ever persisted: the Destroy change that follows
                    // records it, and there is no material to file.
                    return Ok(());
                };
                let mut encoded = hex::encode(material.expose_bytes());
                let filed =
                    self.secrets
                        .set(SCOPE_KEY_SERVICE, &scope_key_account(*key_id), &encoded);
                wipe(&mut encoded);
                filed.map_err(|error| CustodyError::SecretStore(error.to_string()))?;
                self.upsert_row(key, *key_id, CustodyState::Held).await?;
            }
            IndexChange::StopIssuance { key } => {
                if let Some(key_id) = self.key_ids.get(key) {
                    if !self.destroyed.contains(key) {
                        self.upsert_row(key, *key_id, CustodyState::IssuanceStopped)
                            .await?;
                    }
                }
            }
            IndexChange::Destroy { key, key_id } => {
                self.upsert_row(key, *key_id, CustodyState::Destroyed)
                    .await?;
                self.secrets
                    .delete(SCOPE_KEY_SERVICE, &scope_key_account(*key_id))
                    .map_err(|error| CustodyError::SecretStore(error.to_string()))?;
            }
            IndexChange::RecordDisposition { disposition } => {
                let index_key = index_key(&disposition.key)?;
                self.db
                    .query(
                        "UPDATE canon_scope_key_index SET destroyed_at_hlc = $at, note = $note \
                         WHERE index_key = $key",
                    )
                    .bind((
                        "at",
                        format!(
                            "{}:{}",
                            disposition.destroyed_at.wall_ms, disposition.destroyed_at.counter
                        ),
                    ))
                    .bind(("note", disposition.note.clone()))
                    .bind(("key", index_key))
                    .await
                    .map_err(|error| StorageError::query("scope key disposition", error))?
                    .check()
                    .map_err(|error| StorageError::query("scope key disposition", error))?;
            }
        }
        Ok(())
    }

    /// Writes the one index row for `key`, creating it on first use.
    async fn upsert_row(
        &self,
        key: &ScopeKeyId,
        key_id: Identifier32,
        state: CustodyState,
    ) -> Result<(), CustodyError> {
        let index_key = index_key(key)?;
        let scope_text = scope_key(&key.scope).map_err(|error| {
            StorageError::malformed("scope key index write", format!("scope encoding: {error}"))
        })?;
        let epoch = i64::try_from(key.epoch).map_err(|_| {
            StorageError::malformed("scope key index write", "epoch exceeds the int column")
        })?;
        let mut response = self
            .db
            .query(
                "UPDATE canon_scope_key_index SET state = $state WHERE index_key = $key \
                 RETURN AFTER",
            )
            .bind(("state", state.label()))
            .bind(("key", index_key.clone()))
            .await
            .map_err(|error| StorageError::query("scope key index write", error))?
            .check()
            .map_err(|error| StorageError::query("scope key index write", error))?;
        let updated: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("scope key index write", error))?;
        if !updated.is_empty() {
            return Ok(());
        }
        self.db
            .query("CREATE canon_scope_key_index CONTENT $row")
            .bind((
                "row",
                serde_json::json!({
                    "index_key": index_key,
                    "scope_key": scope_text,
                    "epoch": epoch,
                    "key_id_hex": identifier_to_hex(key_id),
                    "state": state.label(),
                }),
            ))
            .await
            .map_err(|error| StorageError::query("scope key index create", error))?
            .check()
            .map_err(|error| StorageError::query("scope key index create", error))?;
        Ok(())
    }
}

impl ScopeKeyStore for SecretStoreScopeKeyCustody {
    fn scope_key(&self, key: &ScopeKeyId) -> Option<&SealingKey> {
        self.keys.get(key)
    }

    /// Refuses a second key for one `(scope, epoch)` — including one indexed whose material is
    /// missing, so an epoch never has two keys — and any key for one whose issuance stopped,
    /// including a destroyed one: a shredded epoch is never re-keyed.
    fn insert_scope_key(
        &mut self,
        key: ScopeKeyId,
        material: SealingKey,
    ) -> Result<(), CryptoError> {
        if self.issuance_stopped.contains(&key) || self.destroyed.contains(&key) {
            return Err(CryptoError::IssuanceStopped { epoch: key.epoch });
        }
        if self.keys.contains_key(&key) || self.key_ids.contains_key(&key) {
            return Err(CryptoError::ScopeKeyAlreadyPresent { epoch: key.epoch });
        }
        let key_id = derive_key_id(&key.scope, key.epoch, &material)?;
        self.key_ids.insert(key.clone(), key_id);
        self.keys.insert(key.clone(), material);
        self.pending.push(IndexChange::Insert { key, key_id });
        Ok(())
    }

    fn stop_issuance(&mut self, key: &ScopeKeyId) {
        if self.issuance_stopped.insert(key.clone()) {
            self.pending
                .push(IndexChange::StopIssuance { key: key.clone() });
        }
    }

    fn issuance_stopped(&self, key: &ScopeKeyId) -> bool {
        self.issuance_stopped.contains(key)
    }
}

impl ShredLedger for SecretStoreScopeKeyCustody {
    /// Zeroizes the in-process copy now and queues the index-then-secret deletion.
    fn destroy_scope_key_and_wraps(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError> {
        let Some(material) = self.keys.remove(key) else {
            return Err(CryptoShredError::KeyNotControlled { key: key.clone() });
        };
        // `SealingKey` zeroizes on drop; dropping here is the erasure point SPEC-5 §5.5.4 names.
        drop(material);
        let key_id = self.key_ids[key];
        self.destroyed.insert(key.clone());
        self.pending.push(IndexChange::Destroy {
            key: key.clone(),
            key_id,
        });
        Ok(())
    }

    fn record_destruction_disposition(
        &mut self,
        disposition: DestructionDisposition,
    ) -> Result<(), CryptoShredError> {
        self.dispositions.push(disposition.clone());
        self.pending
            .push(IndexChange::RecordDisposition { disposition });
        Ok(())
    }
}

/// The unique index key of one `(scope, epoch)`.
fn index_key(key: &ScopeKeyId) -> Result<String, StorageError> {
    let scope = scope_key(&key.scope).map_err(|error| {
        StorageError::malformed("scope key index", format!("scope encoding: {error}"))
    })?;
    Ok(format!("{scope}:{}", key.epoch))
}

/// Parses filed hex material, wiping the text it was handed. The error never quotes the text:
/// `hex::FromHexError` names the offending character, which would put key bytes in the logs.
fn decode_material(mut secret: String) -> Result<SealingKey, String> {
    let decoded = hex::decode(&secret).map_err(|_| "material is not hex".to_string());
    wipe(&mut secret);
    let mut bytes = decoded?;
    let material = <[u8; SEALING_KEY_LENGTH]>::try_from(bytes.as_slice())
        .map(SealingKey::from_bytes)
        .map_err(|_| {
            format!(
                "material is {} bytes, not {SEALING_KEY_LENGTH}",
                bytes.len()
            )
        });
    bytes.fill(0);
    std::hint::black_box(&bytes);
    material
}

/// Best-effort overwrite of hex key text before it is freed; fe-database has no `zeroize`.
fn wipe(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_filed_secret_is_refused_without_quoting_any_of_its_characters() {
        // 63 valid hex digits and one marker: hex::FromHexError would name 'Q' and its index.
        let corrupt = format!("{}Q", "ab".repeat(SEALING_KEY_LENGTH).split_at(63).0);
        let error = decode_material(corrupt).expect_err("not hex");
        assert!(
            !error.contains('Q'),
            "load fault must not quote key text: {error}"
        );
        assert!(
            !error.contains("ab"),
            "load fault must not quote key text: {error}"
        );

        let wrong_length = decode_material("ab".repeat(4)).expect_err("too short");
        assert!(wrong_length.contains("4 bytes"), "{wrong_length}");
        assert!(decode_material("ab".repeat(SEALING_KEY_LENGTH)).is_ok());
    }
}
