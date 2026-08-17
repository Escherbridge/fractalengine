//! SPEC-4 §3.1-§3.3 exactly-once durable append over SurrealDB, plus the read-only views the
//! rest of Workstream G asks of the verified log. Rationale: `canon_log/AGENTS.md`
//! §verified-log and §storage-faults.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use fe_canonical_log::branch::{HeadAdmission, VerseDagView};
use fe_canonical_log::envelope::{CompleteEnvelope, EquivocationKey, Hash32, Identifier32};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::errors::{AppendError, AppendOutcome};
use fe_canonical_log::materialize::traits::{
    verified_envelope_meta_from, EquivocationIndex, ParentVerseLookup, VerifiedEnvelopeMeta,
    VerifiedLogStore,
};

use crate::canon_log::{
    identifier_to_hex, local_stamp, op_id_from_hex, op_id_to_hex, StorageError,
};
use crate::repo::Db;

/// The wire operation kind of a `branch_genesis` (SPEC-1 §6).
const BRANCH_GENESIS_KIND: u16 = 2;

/// Every reason an append does not succeed, storage faults included.
///
/// `VerifiedLogStore::append` can only answer with [`AppendError`], which has no variant for an
/// unavailable database; this enum is the honest one and the inherent API returns it.
#[derive(Debug, thiserror::Error)]
pub enum VerifiedLogStoreError {
    /// A SPEC-4 §3.1 append refusal: the bytes do not hash to the claimed identity, or that
    /// identity already names different bytes.
    #[error(transparent)]
    Append(#[from] AppendError),
    /// The candidate bytes are not a decodable canonical envelope, so no admitted operation
    /// could have produced them.
    #[error("candidate bytes are not a decodable canonical envelope: {0}")]
    NotAnEnvelope(String),
    /// The substrate failed. §2.3 log-first-strict: the mutation fails and SurrealDB is
    /// unmodified; the caller retries the exact same immutable bytes (§3.4).
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// The durable, content-addressed verified log (SPEC-4 §3).
///
/// Exactly-once is enforced twice over: the `idx_verified_op_log_op_id` UNIQUE index makes a
/// second row for one `op_id` impossible at the storage layer, and every write path
/// read-compares the stored bytes first so a duplicate is reported as `AlreadyPresent` rather
/// than as a failure.
pub struct SurrealVerifiedLogStore {
    db: Db,
    storage_faults: Mutex<Vec<String>>,
}

impl SurrealVerifiedLogStore {
    /// Binds the store to an open database handle.
    pub fn new(db: Db) -> Self {
        Self {
            db,
            storage_faults: Mutex::new(Vec::new()),
        }
    }

    /// The handle this store reads and writes through.
    pub fn database(&self) -> &Db {
        &self.db
    }

    /// Appends already-admitted bytes under the identity their admission produced.
    ///
    /// Taking `meta` rather than a bare `op_id` is the construction that makes §2.1 unavoidable:
    /// a `VerifiedEnvelopeMeta` only exists downstream of `admit_candidate`, so there is no way
    /// to append bytes nobody verified.
    pub async fn append_admitted(
        &self,
        meta: &VerifiedEnvelopeMeta,
        bytes: &[u8],
    ) -> Result<AppendOutcome, VerifiedLogStoreError> {
        // §3.3: recompute before touching any row. A claimed identity that does not match the
        // bytes is an integrity violation, not a storage decision.
        if Hash32::of(bytes) != meta.op_id {
            return Err(AppendError::HashMismatch.into());
        }

        let op_id_hex = op_id_to_hex(meta.op_id);
        if let Some(stored) = self.stored_bytes(&op_id_hex).await? {
            return Ok(self.reconcile_existing(meta.op_id, &stored, bytes)?);
        }

        let row = serde_json::json!({
            "op_id_hex": op_id_hex,
            "envelope_bytes": BASE64.encode(bytes),
            "operation_kind": i64::from(meta.operation_kind),
            "branch_id": identifier_to_hex(meta.branch_id),
            "parent_op_ids": meta
                .parents
                .iter()
                .map(|parent| op_id_to_hex(*parent))
                .collect::<Vec<_>>(),
            "author_public_key_hex": hex::encode(meta.author_public_key),
            "wall_ms": meta.wall_ms as i64,
            "hlc_counter": i64::from(meta.counter),
            "appended_at_hlc": local_stamp(),
        });

        let insert = self
            .db
            .query("CREATE verified_op_log CONTENT $row")
            .bind(("row", row))
            .await
            .map_err(|error| StorageError::query("verified log append", error))
            .and_then(|response| {
                response
                    .check()
                    .map_err(|error| StorageError::query("verified log append", error))
            });

        match insert {
            Ok(_) => Ok(AppendOutcome::Appended),
            Err(insert_error) => {
                // A UNIQUE-index rejection and a substrate fault look alike from here, so
                // §3.4's rule applies: reconcile by looking the exact `op_id` up again.
                match self.stored_bytes(&op_id_hex).await? {
                    Some(stored) => Ok(self.reconcile_existing(meta.op_id, &stored, bytes)?),
                    None => {
                        self.record_fault(&insert_error);
                        Err(insert_error.into())
                    }
                }
            }
        }
    }

    /// §3.2/§3.3: byte-identical is `AlreadyPresent`; anything else is an integrity conflict in
    /// which neither candidate may be materialized.
    fn reconcile_existing(
        &self,
        op_id: Hash32,
        stored: &[u8],
        candidate: &[u8],
    ) -> Result<AppendOutcome, AppendError> {
        if stored == candidate {
            Ok(AppendOutcome::AlreadyPresent)
        } else {
            Err(AppendError::IntegrityConflict { op_id })
        }
    }

    /// The exact complete-envelope bytes stored under `op_id`, if any.
    pub async fn envelope_bytes(&self, op_id: Hash32) -> Result<Option<Vec<u8>>, StorageError> {
        self.stored_bytes(&op_id_to_hex(op_id)).await
    }

    /// The materializer-facing facts of an appended operation, rebuilt from its own bytes.
    ///
    /// The indexed columns are a lookup aid, never the source: `scope` and `schema_hash` are
    /// only in the signed bytes, and re-deriving every field from them means a corrupted index
    /// column can never change what a materializer sees.
    pub async fn meta_of(
        &self,
        op_id: Hash32,
    ) -> Result<Option<VerifiedEnvelopeMeta>, StorageError> {
        let Some(bytes) = self.envelope_bytes(op_id).await? else {
            return Ok(None);
        };
        if Hash32::of(&bytes) != op_id {
            return Err(StorageError::malformed(
                "verified log read",
                format!(
                    "row {} holds bytes that do not hash to its own op_id",
                    op_id_to_hex(op_id)
                ),
            ));
        }
        let complete = CompleteEnvelope::decode_canonical(&bytes).map_err(|error| {
            StorageError::malformed("verified log read", format!("{error} (canonical decode)"))
        })?;
        Ok(Some(verified_envelope_meta_from(&complete, op_id)))
    }

    /// The direct parents of an appended operation, read from the durable parent index.
    pub async fn parents(&self, op_id: Hash32) -> Result<Option<Vec<Hash32>>, StorageError> {
        let Some(row) = self.row(&op_id_to_hex(op_id)).await? else {
            return Ok(None);
        };
        let raw = row
            .get("parent_op_ids")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                StorageError::malformed("verified log parents", "parent_op_ids is not an array")
            })?;
        let mut parents = Vec::with_capacity(raw.len());
        for value in raw {
            let text = value.as_str().ok_or_else(|| {
                StorageError::malformed("verified log parents", "parent entry is not a string")
            })?;
            parents.push(op_id_from_hex(text).ok_or_else(|| {
                StorageError::malformed("verified log parents", format!("bad parent hex {text}"))
            })?);
        }
        Ok(Some(parents))
    }

    /// The verse an appended operation belongs to (SPEC-1 §6.2 same-verse parent checking).
    pub async fn verse_of(&self, op_id: Hash32) -> Result<Option<Identifier32>, StorageError> {
        Ok(self.meta_of(op_id).await?.map(|meta| meta.scope.verse_id()))
    }

    /// The already-admitted operation at this §3.4 equivocation identity, if one exists.
    pub async fn op_id_at_equivocation_key(
        &self,
        key: EquivocationKey,
    ) -> Result<Option<Hash32>, StorageError> {
        let mut response = self
            .db
            .query(
                "SELECT op_id_hex FROM verified_op_log \
                 WHERE author_public_key_hex = $author AND wall_ms = $wall AND hlc_counter = $counter \
                 LIMIT 1",
            )
            .bind(("author", hex::encode(key.author_public_key)))
            .bind(("wall", key.wall_ms as i64))
            .bind(("counter", i64::from(key.counter)))
            .await
            .map_err(|error| StorageError::query("equivocation lookup", error))?
            .check()
            .map_err(|error| StorageError::query("equivocation lookup", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("equivocation lookup", error))?;
        let Some(text) = rows
            .first()
            .and_then(|row| row.get("op_id_hex"))
            .and_then(serde_json::Value::as_str)
        else {
            return Ok(None);
        };
        op_id_from_hex(text).map(Some).ok_or_else(|| {
            StorageError::malformed("equivocation lookup", format!("bad op_id hex {text}"))
        })
    }

    /// Every appended operation, oldest identity first, for snapshot loaders and rebuilds.
    pub async fn all_rows(&self) -> Result<Vec<serde_json::Value>, StorageError> {
        let mut response = self
            .db
            .query("SELECT * FROM verified_op_log ORDER BY op_id_hex ASC")
            .await
            .map_err(|error| StorageError::query("verified log scan", error))?
            .check()
            .map_err(|error| StorageError::query("verified log scan", error))?;
        response
            .take(0)
            .map_err(|error| StorageError::query("verified log scan", error))
    }

    /// Storage faults observed since the last drain, in order.
    ///
    /// The `VerifiedLogStore` trait cannot report a substrate failure, so its impl records the
    /// real cause here and returns the most conservative protocol answer available. A caller
    /// that must distinguish "unavailable" from "invalid" uses the inherent API instead.
    pub fn take_storage_faults(&self) -> Vec<String> {
        std::mem::take(&mut *self.storage_faults.lock().expect("storage fault lock"))
    }

    /// Whether any storage fault has been recorded and not yet drained.
    pub fn has_storage_fault(&self) -> bool {
        !self
            .storage_faults
            .lock()
            .expect("storage fault lock")
            .is_empty()
    }

    fn record_fault(&self, error: &impl std::fmt::Display) {
        self.storage_faults
            .lock()
            .expect("storage fault lock")
            .push(error.to_string());
    }

    async fn stored_bytes(&self, op_id_hex: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let Some(row) = self.row(op_id_hex).await? else {
            return Ok(None);
        };
        let encoded = row
            .get("envelope_bytes")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                StorageError::malformed("verified log read", "envelope_bytes is not a string")
            })?;
        BASE64
            .decode(encoded)
            .map(Some)
            .map_err(|error| StorageError::malformed("verified log read", error))
    }

    async fn row(&self, op_id_hex: &str) -> Result<Option<serde_json::Value>, StorageError> {
        let mut response = self
            .db
            .query("SELECT * FROM verified_op_log WHERE op_id_hex = $id LIMIT 1")
            .bind(("id", op_id_hex.to_string()))
            .await
            .map_err(|error| StorageError::query("verified log read", error))?
            .check()
            .map_err(|error| StorageError::query("verified log read", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("verified log read", error))?;
        Ok(rows.into_iter().next())
    }

    fn absent_on_fault<T>(&self, outcome: Result<Option<T>, StorageError>) -> Option<T> {
        match outcome {
            Ok(value) => value,
            Err(error) => {
                self.record_fault(&error);
                // Absent is the pending-safe answer: replay halts the affected head rather
                // than applying an operation whose causal history it could not read.
                None
            }
        }
    }
}

#[async_trait]
impl VerifiedLogStore for SurrealVerifiedLogStore {
    async fn append(
        &self,
        claimed_op_id: Hash32,
        bytes: Vec<u8>,
    ) -> Result<AppendOutcome, AppendError> {
        if Hash32::of(&bytes) != claimed_op_id {
            return Err(AppendError::HashMismatch);
        }
        let complete = match CompleteEnvelope::decode_canonical(&bytes) {
            Ok(complete) => complete,
            Err(error) => {
                self.record_fault(&error);
                return Err(AppendError::IntegrityConflict {
                    op_id: claimed_op_id,
                });
            }
        };
        let meta = verified_envelope_meta_from(&complete, claimed_op_id);
        match self.append_admitted(&meta, &bytes).await {
            Ok(outcome) => Ok(outcome),
            Err(VerifiedLogStoreError::Append(error)) => Err(error),
            Err(other) => {
                self.record_fault(&other);
                // The trait has no "indeterminate" answer. Refusing is the only §2.3-compliant
                // reply, and `IntegrityConflict` is the refusal that forbids materializing
                // either candidate — conservative, and recoverable by retrying the same bytes.
                Err(AppendError::IntegrityConflict {
                    op_id: claimed_op_id,
                })
            }
        }
    }

    async fn get_bytes(&self, op_id: Hash32) -> Option<Vec<u8>> {
        self.absent_on_fault(self.envelope_bytes(op_id).await)
    }

    async fn get_meta(&self, op_id: Hash32) -> Option<VerifiedEnvelopeMeta> {
        self.absent_on_fault(self.meta_of(op_id).await)
    }

    async fn parents_of(&self, op_id: Hash32) -> Option<Vec<Hash32>> {
        self.absent_on_fault(self.parents(op_id).await)
    }
}

#[async_trait]
impl EquivocationIndex for SurrealVerifiedLogStore {
    async fn op_id_at(&self, key: EquivocationKey) -> Option<Hash32> {
        self.absent_on_fault(self.op_id_at_equivocation_key(key).await)
    }
}

#[async_trait]
impl ParentVerseLookup for SurrealVerifiedLogStore {
    async fn verse_id_of(&self, parent_op_id: Hash32) -> Option<Identifier32> {
        self.absent_on_fault(self.verse_of(parent_op_id).await)
    }
}

/// The three DAG facts SPEC-5's branch registry cannot derive, answered from the verified log.
///
/// `VerseDagView`'s methods are synchronous, so this is a snapshot loaded once by
/// [`SurrealVerseDagView::load`] rather than a live query wrapper. A stale snapshot can only
/// under-report admission, which denies rather than allows.
pub struct SurrealVerseDagView {
    verse_id: Identifier32,
    admitted: HashMap<Hash32, HeadAdmission>,
    branch_genesis: HashMap<Identifier32, Hash32>,
    replay_verified: HashSet<(Identifier32, Hash32)>,
}

impl SurrealVerseDagView {
    /// Loads every admitted operation of `verse_id` and derives its §3.4 equivocation state.
    pub async fn load(
        store: &SurrealVerifiedLogStore,
        verse_id: Identifier32,
    ) -> Result<Self, StorageError> {
        let mut operations: Vec<(Hash32, VerifiedEnvelopeMeta)> = Vec::new();
        for row in store.all_rows().await? {
            let text = row
                .get("op_id_hex")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    StorageError::malformed("verse dag load", "op_id_hex is not a string")
                })?;
            let op_id = op_id_from_hex(text).ok_or_else(|| {
                StorageError::malformed("verse dag load", format!("bad op_id hex {text}"))
            })?;
            let Some(meta) = store.meta_of(op_id).await? else {
                continue;
            };
            if meta.scope.verse_id() != verse_id {
                continue;
            }
            operations.push((op_id, meta));
        }

        // §3.4: two distinct op_ids at one equivocation identity quarantine BOTH.
        type EquivocationSlot = ([u8; 32], u64, u32);
        let mut by_equivocation_key: HashMap<EquivocationSlot, Vec<Hash32>> = HashMap::new();
        for (op_id, meta) in &operations {
            by_equivocation_key
                .entry((meta.author_public_key, meta.wall_ms, meta.counter))
                .or_default()
                .push(*op_id);
        }

        let mut admitted = HashMap::with_capacity(operations.len());
        for (op_id, meta) in &operations {
            let slot: EquivocationSlot = (meta.author_public_key, meta.wall_ms, meta.counter);
            let siblings = &by_equivocation_key[&slot];
            let conflicting = siblings.iter().find(|candidate| *candidate != op_id);
            let state = match conflicting {
                Some(conflicting_op_id) => HeadAdmission::QuarantinedEquivocation {
                    conflicting_op_id: *conflicting_op_id,
                },
                None => HeadAdmission::Admitted,
            };
            admitted.insert(*op_id, state);
        }

        // An ambiguous genesis is no genesis: two kind-2 operations for one branch is an
        // integrity problem, and answering with either would pick a winner §3.4 forbids.
        let mut genesis_candidates: HashMap<Identifier32, Vec<Hash32>> = HashMap::new();
        for (op_id, meta) in &operations {
            if meta.operation_kind == BRANCH_GENESIS_KIND
                && matches!(admitted.get(op_id), Some(HeadAdmission::Admitted))
            {
                genesis_candidates
                    .entry(meta.branch_id)
                    .or_default()
                    .push(*op_id);
            }
        }
        let branch_genesis = genesis_candidates
            .into_iter()
            .filter_map(|(branch_id, candidates)| match candidates.as_slice() {
                [only] => Some((branch_id, *only)),
                _ => None,
            })
            .collect();

        Ok(Self {
            verse_id,
            admitted,
            branch_genesis,
            replay_verified: HashSet::new(),
        })
    }

    /// The verse this snapshot answers for.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// Records that a deterministic SPEC-4 replay reproduced `frontier` for `branch_id`.
    ///
    /// Only `rebuild::rebuild_projection` calls this, and only after an actual replay: the
    /// third DAG fact is evidence this process produced, never an assumption.
    pub fn record_replay_verified(&mut self, branch_id: Identifier32, frontier: &SortedFrontier) {
        self.replay_verified
            .insert((branch_id, frontier.commitment()));
    }
}

impl VerseDagView for SurrealVerseDagView {
    fn head_admission(&self, verse_id: Identifier32, op_id: Hash32) -> HeadAdmission {
        if verse_id != self.verse_id {
            return HeadAdmission::NotInVerse;
        }
        self.admitted
            .get(&op_id)
            .copied()
            .unwrap_or(HeadAdmission::NotInVerse)
    }

    fn admitted_branch_genesis(
        &self,
        verse_id: Identifier32,
        branch_id: Identifier32,
    ) -> Option<Hash32> {
        if verse_id != self.verse_id {
            return None;
        }
        self.branch_genesis.get(&branch_id).copied()
    }

    fn frontier_is_replay_verified(
        &self,
        verse_id: Identifier32,
        branch_id: Identifier32,
        frontier: &SortedFrontier,
    ) -> bool {
        verse_id == self.verse_id
            && self
                .replay_verified
                .contains(&(branch_id, frontier.commitment()))
    }
}
