//! Durable, local-only content-addressed bytes: sealed SPEC-6 artifacts and operation payload
//! ciphertexts. Rationale: `canon_log/AGENTS.md` §durable-authority-sources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use fe_canonical_log::compose::{
    ByteStoreSealedArtifactStore, ContentByteStore, PayloadCiphertextSource,
};
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::segment::store::SealedArtifactStore;
use fe_canonical_log::segment::SegmentError;

use crate::canon_log::{create_if_absent, op_id_from_hex, op_id_to_hex, StorageError};
use crate::repo::Db;

/// What a stored row holds; diagnostic only, never an input to a lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentClass {
    /// A SPEC-6 sealed artifact written through [`ByteStoreSealedArtifactStore`].
    SealedArtifact,
    /// An operation payload ciphertext, addressed by its `PayloadRef::ciphertext_hash`.
    PayloadCiphertext,
}

impl ContentClass {
    fn label(self) -> &'static str {
        match self {
            Self::SealedArtifact => "sealed_artifact",
            Self::PayloadCiphertext => "payload_ciphertext",
        }
    }
}

/// Every reason a sealed artifact is not durably stored.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactStoreError {
    /// SPEC-6 refused the bytes (id mismatch or a conflicting rewrite); nothing was written.
    #[error(transparent)]
    Refused(#[from] SegmentError),
    /// The bytes are held in-process but the durable write failed; retry the same bytes.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Local-only bytes in `canon_sealed_artifact`, keyed by `BLAKE3(bytes)`.
///
/// The production [`ContentByteStore`] and [`PayloadCiphertextSource`]. It is deliberately NOT
/// the iroh blob store: iroh blobs are served to peers, which is the canonical network exposure
/// that stays NO_GO. The seams are synchronous, so reads answer from the cache [`Self::load`]
/// fills; [`ContentByteStore::put`] queues a durable write that [`Self::persist_pending`] drains,
/// and the `async` doors below do both. Clones share one cache.
#[derive(Clone)]
pub struct SurrealArtifactStore {
    db: Db,
    cache: Arc<RwLock<HashMap<Hash32, Vec<u8>>>>,
    pending: Arc<Mutex<Vec<(Hash32, ContentClass)>>>,
    load_faults: Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for SurrealArtifactStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurrealArtifactStore")
            .field("held", &self.read_cache().len())
            .finish()
    }
}

impl SurrealArtifactStore {
    /// Reads every stored row, holding only rows whose bytes hash to their own id.
    pub async fn load(db: &Db) -> Result<Self, StorageError> {
        let mut response = db
            .query("SELECT artifact_id_hex, content_bytes FROM canon_sealed_artifact")
            .await
            .map_err(|error| StorageError::query("artifact store load", error))?
            .check()
            .map_err(|error| StorageError::query("artifact store load", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("artifact store load", error))?;

        let mut cache = HashMap::with_capacity(rows.len());
        let mut faults = Vec::new();
        for row in rows {
            match bytes_from_row(&row) {
                Ok((artifact_id, bytes)) => {
                    cache.insert(artifact_id, bytes);
                }
                Err(detail) => faults.push(detail),
            }
        }
        for fault in &faults {
            tracing::warn!("canon_sealed_artifact row not held: {fault}");
        }
        Ok(Self {
            db: db.clone(),
            cache: Arc::new(RwLock::new(cache)),
            pending: Arc::new(Mutex::new(Vec::new())),
            load_faults: Arc::new(Mutex::new(faults)),
        })
    }

    /// Durably stores an admitted operation's payload ciphertext under its BLAKE3, so a later
    /// replay can reopen it; returns that address.
    pub async fn put_payload_ciphertext(&self, ciphertext: &[u8]) -> Result<Hash32, StorageError> {
        let address = Hash32::of(ciphertext);
        self.write_row(address, ciphertext, ContentClass::PayloadCiphertext)
            .await?;
        self.write_cache().insert(address, ciphertext.to_vec());
        Ok(address)
    }

    /// The SPEC-6 §5.1 door: recomputes the id and refuses a conflicting rewrite BEFORE any
    /// byte is held, then writes durably.
    pub async fn put_sealed_artifact(
        &self,
        claimed_id: Hash32,
        bytes: &[u8],
    ) -> Result<Hash32, ArtifactStoreError> {
        let artifact_id = self.sealed().put_verified(claimed_id, bytes)?;
        self.persist_pending().await?;
        Ok(artifact_id)
    }

    /// A [`SealedArtifactStore`] over this store; writes through it are durable after
    /// [`Self::persist_pending`].
    pub fn sealed(&self) -> ByteStoreSealedArtifactStore<Self> {
        ByteStoreSealedArtifactStore::new(self.clone())
    }

    /// Writes every queued [`ContentByteStore::put`] to the table; returns how many rows this
    /// call created. A failed write stays queued for the next call.
    pub async fn persist_pending(&self) -> Result<usize, StorageError> {
        let queued = std::mem::take(&mut *self.pending_lock());
        let mut created = 0;
        for (index, (artifact_id, class)) in queued.iter().enumerate() {
            let Some(bytes) = self.read_cache().get(artifact_id).cloned() else {
                continue;
            };
            match self.write_row(*artifact_id, &bytes, *class).await {
                Ok(true) => created += 1,
                Ok(false) => {}
                Err(error) => {
                    self.pending_lock()
                        .splice(0..0, queued[index..].iter().copied());
                    return Err(error);
                }
            }
        }
        Ok(created)
    }

    /// How many byte strings are held.
    pub fn len(&self) -> usize {
        self.read_cache().len()
    }

    /// Whether nothing is held.
    pub fn is_empty(&self) -> bool {
        self.read_cache().is_empty()
    }

    /// Rows [`Self::load`] refused to hold, drained.
    pub fn take_load_faults(&self) -> Vec<String> {
        std::mem::take(&mut *self.load_faults.lock().expect("artifact load fault lock"))
    }

    async fn write_row(
        &self,
        artifact_id: Hash32,
        bytes: &[u8],
        class: ContentClass,
    ) -> Result<bool, StorageError> {
        create_if_absent(
            &self.db,
            "canon_sealed_artifact",
            "artifact_id_hex",
            &op_id_to_hex(artifact_id),
            serde_json::json!({
                "artifact_id_hex": op_id_to_hex(artifact_id),
                "content_class": class.label(),
                "content_bytes": BASE64.encode(bytes),
            }),
        )
        .await
    }

    fn read_cache(&self) -> std::sync::RwLockReadGuard<'_, HashMap<Hash32, Vec<u8>>> {
        self.cache.read().expect("artifact cache lock")
    }

    fn write_cache(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<Hash32, Vec<u8>>> {
        self.cache.write().expect("artifact cache lock")
    }

    fn pending_lock(&self) -> std::sync::MutexGuard<'_, Vec<(Hash32, ContentClass)>> {
        self.pending.lock().expect("artifact pending lock")
    }
}

impl ContentByteStore for SurrealArtifactStore {
    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.read_cache().get(&artifact_id).cloned()
    }

    fn has(&self, artifact_id: Hash32) -> bool {
        self.read_cache().contains_key(&artifact_id)
    }

    /// Holds the bytes in-process and queues their durable write for [`Self::persist_pending`].
    fn put(&mut self, artifact_id: Hash32, bytes: Vec<u8>) {
        self.write_cache().insert(artifact_id, bytes);
        self.pending_lock()
            .push((artifact_id, ContentClass::SealedArtifact));
    }
}

impl PayloadCiphertextSource for SurrealArtifactStore {
    fn ciphertext(&self, ciphertext_hash: &Hash32) -> Option<Vec<u8>> {
        self.read_cache().get(ciphertext_hash).cloned()
    }
}

/// A stored row, re-derived from its own bytes; the id column is a claim, never the source.
fn bytes_from_row(row: &serde_json::Value) -> Result<(Hash32, Vec<u8>), String> {
    let id_text = row
        .get("artifact_id_hex")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "artifact_id_hex is not a string".to_string())?;
    let claimed =
        op_id_from_hex(id_text).ok_or_else(|| format!("bad artifact id hex {id_text}"))?;
    let encoded = row
        .get("content_bytes")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{id_text}: content_bytes is not a string"))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| format!("{id_text}: {error}"))?;
    if Hash32::of(&bytes) != claimed {
        return Err(format!("{id_text}: bytes do not hash to the row's id"));
    }
    Ok((claimed, bytes))
}
