//! Durable, local-only SPEC-3 §2.4 capability-chain artifacts by `chain_id`.
//! Rationale: `canon_log/AGENTS.md` §durable-authority-sources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use fe_canonical_log::capability::{Certificate, Chain, ChainId};
use fe_canonical_log::compose::CapabilityChainSource;
use fe_canonical_log::envelope::Hash32;

use crate::canon_log::canonical_epoch::scope_key;
use crate::canon_log::{create_if_absent, op_id_from_hex, op_id_to_hex, StorageError};
use crate::repo::Db;

/// One held chain and the two lookup facts derived from its own bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredChain {
    /// The exact canonical chain bytes `chain_id` addresses.
    pub chain_bytes: Vec<u8>,
    /// DID of the leaf certificate's audience.
    pub leaf_audience_did: String,
    /// Durable key (`canonical_epoch::scope_key`) of the leaf certificate's epoch scope.
    pub epoch_scope_key: String,
}

/// What [`ChainStore::put_verified_chain`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainPut {
    /// `BLAKE3` of the canonical chain bytes.
    pub chain_id: ChainId,
    /// `false` when byte-identical bytes were already held.
    pub newly_stored: bool,
}

/// Every reason a chain is not stored.
#[derive(Debug, thiserror::Error)]
pub enum ChainStoreError {
    /// The bytes are not a canonical SPEC-3 §2.4 chain map of canonical certificates.
    #[error("not a canonical capability chain: {0}")]
    NotACanonicalChain(String),
    /// The substrate failed; nothing new is held.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Capability chains this process holds, written through to `canon_capability_chain`.
///
/// Access-controlled local data (SPEC-3 privacy table): never replicated, never served through
/// fe-api. Clones share one cache, so a chain put through any clone answers through all of them.
/// [`CapabilityChainSource`] is synchronous, so reads answer from the cache that [`Self::load`]
/// fills and [`Self::put_verified_chain`] writes through.
#[derive(Clone)]
pub struct ChainStore {
    db: Db,
    cache: Arc<RwLock<HashMap<ChainId, StoredChain>>>,
    load_faults: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for ChainStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainStore")
            .field("held", &self.read_cache().len())
            .finish()
    }
}

impl ChainStore {
    /// Reads every stored chain, re-verifying each row's bytes against its own id.
    ///
    /// A row whose bytes are not a canonical chain or do not hash to its id is not held: it is
    /// recorded in [`Self::take_load_faults`] and an operation naming it quarantines as
    /// `CapabilityChainUnavailable`, never admits.
    pub async fn load(db: &Db) -> Result<Self, StorageError> {
        let mut response = db
            .query("SELECT chain_id_hex, chain_bytes FROM canon_capability_chain")
            .await
            .map_err(|error| StorageError::query("chain store load", error))?
            .check()
            .map_err(|error| StorageError::query("chain store load", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("chain store load", error))?;

        let mut cache = HashMap::with_capacity(rows.len());
        let mut faults = Vec::new();
        for row in rows {
            match chain_from_row(&row) {
                Ok((chain_id, stored)) => {
                    cache.insert(chain_id, stored);
                }
                Err(detail) => faults.push(detail),
            }
        }
        for fault in &faults {
            tracing::warn!("canon_capability_chain row not held: {fault}");
        }
        Ok(Self {
            db: db.clone(),
            cache: Arc::new(RwLock::new(cache)),
            load_faults: Arc::new(Mutex::new(faults)),
        })
    }

    /// Stores chain bytes after a canonical round-trip of the chain and every certificate, with
    /// `chain_id` recomputed from the bytes; the caller never names the id.
    ///
    /// Holding a chain authorizes nothing: every use re-runs SPEC-3 §2.4 against the view.
    pub async fn put_verified_chain(
        &self,
        chain_bytes: &[u8],
    ) -> Result<ChainPut, ChainStoreError> {
        let (chain_id, stored) =
            inspect_chain(chain_bytes).map_err(ChainStoreError::NotACanonicalChain)?;
        if let Some(existing) = self.read_cache().get(&chain_id) {
            // Content addressing: equal ids are equal bytes, so a repeat is a no-op.
            debug_assert_eq!(existing.chain_bytes, chain_bytes);
            return Ok(ChainPut {
                chain_id,
                newly_stored: false,
            });
        }

        let newly_stored = create_if_absent(
            &self.db,
            "canon_capability_chain",
            "chain_id_hex",
            &op_id_to_hex(chain_id),
            serde_json::json!({
                "chain_id_hex": op_id_to_hex(chain_id),
                "leaf_audience_did": stored.leaf_audience_did,
                "epoch_scope_key": stored.epoch_scope_key,
                "chain_bytes": BASE64.encode(&stored.chain_bytes),
            }),
        )
        .await?;

        self.write_cache().insert(chain_id, stored);
        Ok(ChainPut {
            chain_id,
            newly_stored,
        })
    }

    /// The chain held under `chain_id`, if any.
    pub fn held_chain(&self, chain_id: &ChainId) -> Option<StoredChain> {
        self.read_cache().get(chain_id).cloned()
    }

    /// Every held chain whose leaf audience is `did`, in `chain_id` order.
    pub fn chains_for_leaf_audience(&self, did: &str) -> Vec<(ChainId, Vec<u8>)> {
        let mut chains: Vec<(ChainId, Vec<u8>)> = self
            .read_cache()
            .iter()
            .filter(|(_, stored)| stored.leaf_audience_did == did)
            .map(|(chain_id, stored)| (*chain_id, stored.chain_bytes.clone()))
            .collect();
        chains.sort_by_key(|(chain_id, _)| *chain_id);
        chains
    }

    /// How many chains are held.
    pub fn len(&self) -> usize {
        self.read_cache().len()
    }

    /// Whether no chain is held.
    pub fn is_empty(&self) -> bool {
        self.read_cache().is_empty()
    }

    /// Rows [`Self::load`] refused to hold, drained.
    pub fn take_load_faults(&self) -> Vec<String> {
        std::mem::take(&mut *self.load_faults.lock().expect("chain load fault lock"))
    }

    fn read_cache(&self) -> std::sync::RwLockReadGuard<'_, HashMap<ChainId, StoredChain>> {
        self.cache.read().expect("chain cache lock")
    }

    fn write_cache(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<ChainId, StoredChain>> {
        self.cache.write().expect("chain cache lock")
    }
}

impl CapabilityChainSource for ChainStore {
    fn chain_bytes(&self, chain_id: &Hash32) -> Option<Vec<u8>> {
        self.read_cache()
            .get(chain_id)
            .map(|stored| stored.chain_bytes.clone())
    }
}

/// The canonical round-trip, the recomputed id, and the leaf facts of `chain_bytes`.
fn inspect_chain(chain_bytes: &[u8]) -> Result<(ChainId, StoredChain), String> {
    let chain = Chain::decode_canonical(chain_bytes).map_err(|error| error.to_string())?;
    let canonical = chain
        .encode_canonical()
        .map_err(|error| error.to_string())?;
    if canonical.as_slice() != chain_bytes {
        return Err("chain bytes are not their own canonical encoding".to_string());
    }
    let mut leaf = None;
    for (index, certificate_bytes) in chain.certificates.iter().enumerate() {
        let certificate = Certificate::decode_canonical(certificate_bytes)
            .map_err(|error| format!("certificate {index}: {error}"))?;
        let reencoded = certificate
            .encode_canonical()
            .map_err(|error| format!("certificate {index}: {error}"))?;
        if reencoded != *certificate_bytes {
            return Err(format!("certificate {index} is not canonical"));
        }
        leaf = Some(certificate);
    }
    let leaf = leaf.ok_or_else(|| "chain carries no certificate".to_string())?;
    Ok((
        Hash32::of(chain_bytes),
        StoredChain {
            chain_bytes: chain_bytes.to_vec(),
            leaf_audience_did: leaf.unsigned.audience.did.clone(),
            epoch_scope_key: scope_key(&leaf.unsigned.epoch_scope)?,
        },
    ))
}

/// A stored row, re-derived from its own bytes; the index columns are never trusted.
fn chain_from_row(row: &serde_json::Value) -> Result<(ChainId, StoredChain), String> {
    let id_text = row
        .get("chain_id_hex")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "chain_id_hex is not a string".to_string())?;
    let claimed = op_id_from_hex(id_text).ok_or_else(|| format!("bad chain id hex {id_text}"))?;
    let encoded = row
        .get("chain_bytes")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{id_text}: chain_bytes is not a string"))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| format!("{id_text}: {error}"))?;
    let (chain_id, stored) =
        inspect_chain(&bytes).map_err(|error| format!("{id_text}: {error}"))?;
    if chain_id != claimed {
        return Err(format!(
            "{id_text}: bytes hash to {}",
            op_id_to_hex(chain_id)
        ));
    }
    Ok((chain_id, stored))
}
