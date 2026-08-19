//! SPEC-3 §5.1/§5.2 persistent epoch state, the durable authorization view read at startup,
//! and the §5.2 causal-validity snapshot. Rationale: `canon_log/AGENTS.md` §epoch-state and
//! §interim-manager-authority.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use fe_canonical_log::capability::{
    AuthorityState, AuthorizationSnapshot, AuthorizationView, ManagerAuthorityView, Principal,
    RevalidationGate,
};
use fe_canonical_log::cbor;
use fe_canonical_log::envelope::{Hash32, Scope};
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::signing::decode_and_admit;
use fe_policy::RoleLevel;

use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::{identifier_to_hex, op_id_from_hex, op_id_to_hex, StorageError};
use crate::repo::Db;

/// Monotonic within one process; every reload of the durable view gets a strictly greater
/// number so a §5.3 cache key recorded against an older view can never satisfy a newer one.
static VIEW_VERSION: AtomicU64 = AtomicU64::new(0);

/// The durable key for one epoch scope: hex of its canonical CBOR scope map.
pub fn scope_key(scope: &Scope) -> Result<String, String> {
    let value = scope.to_cbor().map_err(|error| error.to_string())?;
    Ok(hex::encode(cbor::encode_canonical(&value)))
}

/// Parses a durable scope key back into the scope it encodes.
pub fn scope_from_key(key: &str) -> Result<Scope, String> {
    let bytes = hex::decode(key).map_err(|error| error.to_string())?;
    let value = cbor::decode_canonical(&bytes).map_err(|error| error.to_string())?;
    Scope::from_cbor(&value).map_err(|error| error.to_string())
}

/// The persisted §5.1 state for one epoch scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeEpochRecord {
    /// The epoch currently materialized at this scope; absent state reads as 0.
    pub current_epoch: u64,
    /// Every admitted `scope_epoch_bump`, retained as auditable evidence (§5.1 rule 6).
    pub admitted_bump_op_ids: Vec<Hash32>,
}

/// What admitting a `scope_epoch_bump` did to the persistent state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EpochBumpAdmission {
    /// `e -> e + 1`, exactly.
    Bumped {
        /// The epoch before this bump.
        previous_epoch: u64,
        /// The epoch after it.
        current_epoch: u64,
    },
    /// §5.1 rule 6: a second valid bump from the same `e`, neither in the other's parent
    /// closure. The epoch does not move; the operation is retained as evidence.
    ConcurrentDuplicate {
        /// The unchanged epoch.
        current_epoch: u64,
    },
    /// This exact operation was already admitted; re-admitting is a no-op.
    AlreadyAdmitted {
        /// The unchanged epoch.
        current_epoch: u64,
    },
}

/// Every reason a `scope_epoch_bump` is refused.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum EpochBumpRejection {
    /// The operation is not in the verified log, so it cannot move authorization state.
    #[error("operation {0} is not in the verified log")]
    NotAppended(String),
    /// The stored bytes are not an admissible signed envelope, so they authorize nothing.
    ///
    /// A row can only be read back through `signing::decode_and_admit` here, so an unsigned or
    /// tampered envelope cannot move an epoch even if some other writer got it into the table.
    #[error("operation {op_id} is not an admissible signed envelope: {detail}")]
    Unverified {
        /// Hex of the operation identity that was asked for.
        op_id: String,
        /// Why the stored bytes were not admissible.
        detail: String,
    },
    /// SPEC-3 §5.1: only a Manager+ identity at THIS epoch scope may move its epoch.
    ///
    /// `AuthorityState::Unknown` lands here too: an unanswered authority question is a refusal,
    /// never an allow.
    #[error("author {author_did} may not bump this epoch scope: {authority:?}")]
    NotManagerPlus {
        /// The DID of the operation's own author.
        author_did: String,
        /// What the authorization view actually answered.
        authority: AuthorityState,
    },
    /// Another admission moved this scope between the read and the write; nothing changed.
    #[error("scope {scope_key} was modified concurrently; retry the same operation")]
    ConcurrentModification {
        /// The durable scope key whose compare-and-swap failed.
        scope_key: String,
    },
    /// SPEC-1 reserves kind 4 for `scope_epoch_bump`; nothing else may bump an epoch.
    #[error("operation_kind {operation_kind} is not a scope_epoch_bump (kind 4)")]
    NotAnEpochBump {
        /// The kind actually presented.
        operation_kind: u16,
    },
    /// §5.1 rule 1: exactly one parent.
    #[error("a scope_epoch_bump must have exactly one parent, found {count}")]
    ParentCountInvalid {
        /// Parents actually present.
        count: usize,
    },
    /// §5.1 rule 6: a bump from an epoch that a already-admitted bump superseded.
    #[error("epoch {declared} is stale; this scope is already at {current}")]
    StaleEpoch {
        /// The epoch the capability declared.
        declared: u64,
        /// The epoch actually materialized.
        current: u64,
    },
    /// §5.1 rule 3: arbitrary skips are invalid.
    #[error("epoch {declared} skips ahead of the materialized epoch {current}")]
    EpochSkip {
        /// The epoch the capability declared.
        declared: u64,
        /// The epoch actually materialized.
        current: u64,
    },
    /// The scope, the envelope, or a stored row could not be encoded or read.
    #[error("{0}")]
    Encoding(String),
}

impl From<StorageError> for EpochBumpRejection {
    fn from(error: StorageError) -> Self {
        Self::Encoding(error.to_string())
    }
}

/// SPEC-3 §5.1 persistent epoch state over SurrealDB.
pub struct SurrealScopeEpochStore {
    db: Db,
}

impl SurrealScopeEpochStore {
    /// Binds the store to an open database handle.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// The handle this store reads and writes through.
    pub fn database(&self) -> &Db {
        &self.db
    }

    /// The persisted record for one epoch scope; an unknown scope reads as epoch 0.
    pub async fn record(&self, scope: &Scope) -> Result<ScopeEpochRecord, EpochBumpRejection> {
        let key = scope_key(scope).map_err(EpochBumpRejection::Encoding)?;
        self.record_by_key(&key).await
    }

    /// The epoch currently materialized at one scope, or `None` when the view knows none.
    ///
    /// `None` is never an allow: §5.3 rule 5 makes an unanswerable question a cache miss.
    pub async fn current_epoch(&self, scope: &Scope) -> Result<Option<u64>, EpochBumpRejection> {
        let key = scope_key(scope).map_err(EpochBumpRejection::Encoding)?;
        match self.row_by_key(&key).await? {
            Some(row) => Ok(Some(record_from_row(&row)?.current_epoch)),
            None => Ok(None),
        }
    }

    /// Admits an already-appended `scope_epoch_bump` into the persistent authorization view.
    ///
    /// `gate` is a required parameter and `RevalidationGate::on_epoch_bump` is called here, so
    /// `capability/AGENTS.md` §revalidation obligation 2 cannot be forgotten at a call site:
    /// there is no admission path that does not take the gate.
    ///
    /// `authority` is a required parameter and the Manager+ question is asked here, so SPEC-3
    /// §5.1's authority rule cannot be forgotten at a call site: there is no admission path
    /// that does not take the view. `AuthorityState::Unknown` refuses, exactly like
    /// `NotManagerPlus` — an unanswerable authority question is never an allow (§5.3 rule 5).
    ///
    /// `op_id` names an operation that must already be in the verified log; the declared epoch
    /// is read from its own signed capability reference rather than from a caller argument, so
    /// nobody can bump an epoch with a number the author never signed. The stored bytes go back
    /// through `signing::decode_and_admit`, so an unsigned envelope authorizes nothing here
    /// even if it reached the table by some other route.
    pub async fn admit_epoch_bump(
        &self,
        store: &SurrealVerifiedLogStore,
        authority: &dyn ManagerAuthorityView,
        op_id: Hash32,
        gate: &mut RevalidationGate,
    ) -> Result<EpochBumpAdmission, EpochBumpRejection> {
        let Some(bytes) = store.envelope_bytes(op_id).await? else {
            return Err(EpochBumpRejection::NotAppended(op_id_to_hex(op_id)));
        };
        let (complete, computed_op_id) =
            decode_and_admit(&bytes).map_err(|error| EpochBumpRejection::Unverified {
                op_id: op_id_to_hex(op_id),
                detail: error.to_string(),
            })?;
        if computed_op_id != op_id {
            return Err(EpochBumpRejection::Unverified {
                op_id: op_id_to_hex(op_id),
                detail: format!("bytes address {}", op_id_to_hex(computed_op_id)),
            });
        }
        let unsigned = &complete.unsigned;

        match OperationKind::from_u16(unsigned.operation_kind) {
            Ok(OperationKind::ScopeEpochBump) => {}
            _ => {
                return Err(EpochBumpRejection::NotAnEpochBump {
                    operation_kind: unsigned.operation_kind,
                })
            }
        }
        if unsigned.parents.len() != 1 {
            return Err(EpochBumpRejection::ParentCountInvalid {
                count: unsigned.parents.len(),
            });
        }

        // §5.1 rule 1: the header scope IS the epoch scope being bumped.
        let scope = unsigned.scope;
        let declared = unsigned.capability.scope_epoch;

        // SPEC-3 §5.1: only a Manager+ identity at this exact epoch scope may move it. Asked
        // BEFORE any state is read or written, so an unauthorized bump touches nothing at all
        // and cannot even invalidate a revalidation cache.
        let authority_state =
            authority.principal_is_manager_plus(&unsigned.author, &scope, declared);
        if !authority_state.is_manager_plus() {
            return Err(EpochBumpRejection::NotManagerPlus {
                author_did: unsigned.author.did.clone(),
                authority: authority_state,
            });
        }

        let key = scope_key(&scope).map_err(EpochBumpRejection::Encoding)?;
        // One read, not two: re-reading the row for the record would open a window in which the
        // epoch the compare-and-swap below expects is not the epoch this decision used.
        let existing_row = self.row_by_key(&key).await?;
        let state = match &existing_row {
            Some(row) => record_from_row(row)?,
            None => ScopeEpochRecord::default(),
        };

        if state.admitted_bump_op_ids.contains(&op_id) {
            gate.on_epoch_bump(&scope, state.current_epoch);
            return Ok(EpochBumpAdmission::AlreadyAdmitted {
                current_epoch: state.current_epoch,
            });
        }

        let (next_epoch, admission) = if declared == state.current_epoch {
            let next = declared + 1;
            (
                next,
                EpochBumpAdmission::Bumped {
                    previous_epoch: state.current_epoch,
                    current_epoch: next,
                },
            )
        } else if declared + 1 == state.current_epoch {
            // §5.1 rule 6: a second bump from the same `e`. It is auditable evidence only when
            // it is concurrent with the bump that already applied; one that FOLLOWS that bump
            // is stale, and only the DAG can tell the two apart.
            let already: HashSet<Hash32> = state.admitted_bump_op_ids.iter().copied().collect();
            if ancestor_within(store, op_id, &already).await?.is_some() {
                return Err(EpochBumpRejection::StaleEpoch {
                    declared,
                    current: state.current_epoch,
                });
            }
            (
                state.current_epoch,
                EpochBumpAdmission::ConcurrentDuplicate {
                    current_epoch: state.current_epoch,
                },
            )
        } else if declared < state.current_epoch {
            return Err(EpochBumpRejection::StaleEpoch {
                declared,
                current: state.current_epoch,
            });
        } else {
            return Err(EpochBumpRejection::EpochSkip {
                declared,
                current: state.current_epoch,
            });
        };

        let mut admitted = state.admitted_bump_op_ids.clone();
        admitted.push(op_id);
        admitted.sort();
        admitted.dedup();
        let admitted_hex: Vec<String> = admitted.iter().map(|id| op_id_to_hex(*id)).collect();

        if existing_row.is_some() {
            // Compare-and-swap on the epoch this decision was made against. A plain UPDATE
            // would let two concurrent admissions each read `e`, each write `e + 1`, and the
            // second silently discard the first one's evidence row (§5.1 rule 6 keeps every
            // admitted bump). Matching zero rows means the state moved underneath us.
            let mut response = self
                .db
                .query(
                    "UPDATE scope_epoch_state SET current_epoch = $epoch, \
                     admitted_bump_op_ids = $ids \
                     WHERE scope_key = $key AND current_epoch = $expected RETURN AFTER",
                )
                .bind(("epoch", next_epoch as i64))
                .bind(("ids", admitted_hex))
                .bind(("key", key.clone()))
                .bind(("expected", state.current_epoch as i64))
                .await
                .map_err(|error| StorageError::query("epoch state write", error))?
                .check()
                .map_err(|error| StorageError::query("epoch state write", error))?;
            let updated: Vec<serde_json::Value> = response
                .take(0)
                .map_err(|error| StorageError::query("epoch state write", error))?;
            if updated.is_empty() {
                return Err(EpochBumpRejection::ConcurrentModification { scope_key: key });
            }
        } else {
            self.db
                .query("CREATE scope_epoch_state CONTENT $row")
                .bind((
                    "row",
                    serde_json::json!({
                        "scope_key": key.clone(),
                        "current_epoch": next_epoch as i64,
                        "admitted_bump_op_ids": admitted_hex,
                    }),
                ))
                .await
                .map_err(|error| StorageError::query("epoch state create", error))?
                .check()
                .map_err(|error| StorageError::query("epoch state create", error))?;
        }

        // §5.3 obligation 2: every admitted bump invalidates its cached allow decisions.
        gate.on_epoch_bump(&scope, next_epoch);
        Ok(admission)
    }

    /// Every persisted epoch scope and its record, for the startup view load.
    pub async fn all_records(&self) -> Result<Vec<(String, ScopeEpochRecord)>, StorageError> {
        let mut response = self
            .db
            .query("SELECT * FROM scope_epoch_state ORDER BY scope_key ASC")
            .await
            .map_err(|error| StorageError::query("epoch state scan", error))?
            .check()
            .map_err(|error| StorageError::query("epoch state scan", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("epoch state scan", error))?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let key = row
                .get("scope_key")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    StorageError::malformed("epoch state scan", "scope_key is not a string")
                })?
                .to_string();
            records.push((key, record_from_row(&row)?));
        }
        Ok(records)
    }

    async fn record_by_key(&self, key: &str) -> Result<ScopeEpochRecord, EpochBumpRejection> {
        match self.row_by_key(key).await? {
            Some(row) => Ok(record_from_row(&row)?),
            None => Ok(ScopeEpochRecord::default()),
        }
    }

    async fn row_by_key(&self, key: &str) -> Result<Option<serde_json::Value>, StorageError> {
        let mut response = self
            .db
            .query("SELECT * FROM scope_epoch_state WHERE scope_key = $key LIMIT 1")
            .bind(("key", key.to_string()))
            .await
            .map_err(|error| StorageError::query("epoch state read", error))?
            .check()
            .map_err(|error| StorageError::query("epoch state read", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("epoch state read", error))?;
        Ok(rows.into_iter().next())
    }
}

fn record_from_row(row: &serde_json::Value) -> Result<ScopeEpochRecord, StorageError> {
    let current_epoch = row
        .get("current_epoch")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            StorageError::malformed("epoch state read", "current_epoch is not an int")
        })?;
    let raw = row
        .get("admitted_bump_op_ids")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut admitted_bump_op_ids = Vec::with_capacity(raw.len());
    for value in raw {
        let text = value.as_str().ok_or_else(|| {
            StorageError::malformed("epoch state read", "bump entry is not a string")
        })?;
        admitted_bump_op_ids.push(op_id_from_hex(text).ok_or_else(|| {
            StorageError::malformed("epoch state read", format!("bad op_id hex {text}"))
        })?);
    }
    Ok(ScopeEpochRecord {
        current_epoch: current_epoch.max(0) as u64,
        admitted_bump_op_ids,
    })
}

/// The first member of `targets` reachable from `start` through parent edges, if any.
async fn ancestor_within(
    store: &SurrealVerifiedLogStore,
    start: Hash32,
    targets: &HashSet<Hash32>,
) -> Result<Option<Hash32>, StorageError> {
    let mut seen: HashSet<Hash32> = HashSet::from([start]);
    let mut queue: VecDeque<Hash32> = VecDeque::from([start]);
    while let Some(op_id) = queue.pop_front() {
        let Some(parents) = store.parents(op_id).await? else {
            continue;
        };
        for parent in parents {
            if targets.contains(&parent) {
                return Ok(Some(parent));
            }
            if seen.insert(parent) {
                queue.push_back(parent);
            }
        }
    }
    Ok(None)
}

/// SPEC-3 §5.2 rule 2: an operation with a lower epoch stays historically valid only when it
/// sits inside an admitted bump's transitive parent closure.
///
/// The only injected input `verify_chain` still takes, because it is DAG state no chain field
/// identifies. Everything else the verifier asks the view for itself.
pub async fn build_authorization_snapshot(
    store: &SurrealVerifiedLogStore,
    epochs: &SurrealScopeEpochStore,
    epoch_scope: &Scope,
    operation_op_id: Hash32,
) -> Result<AuthorizationSnapshot, EpochBumpRejection> {
    let record = epochs.record(epoch_scope).await?;
    let target = HashSet::from([operation_op_id]);
    for bump in &record.admitted_bump_op_ids {
        if *bump == operation_op_id || ancestor_within(store, *bump, &target).await?.is_some() {
            return Ok(AuthorizationSnapshot {
                epoch_causally_valid_for_operation: true,
            });
        }
    }
    Ok(AuthorizationSnapshot {
        epoch_causally_valid_for_operation: false,
    })
}

/// The interim role-table key for an epoch scope.
///
/// SPEC-2's authority history is not implemented, so Manager+ status is read from the legacy
/// `role` table through this documented projection: `VERSE#<verse_hex>`, then `-PETAL#<hex>`,
/// then `-RESOURCE#<hex>`, all lowercase hex of the 32-byte identifiers. It is deliberately NOT
/// `crate::build_scope`'s VERSE/FRACTAL/PETAL string — the two hierarchies are different
/// shapes, and quietly reusing one for the other would make an unrelated fractal row answer an
/// epoch-scope question. Replace this whole function when SPEC-2 lands.
pub fn interim_authority_scope_key(scope: &Scope) -> String {
    let mut key = format!("VERSE#{}", identifier_to_hex(scope.verse_id()));
    if let Some(petal_id) = scope.petal_id() {
        key.push_str(&format!("-PETAL#{}", identifier_to_hex(petal_id)));
    }
    if let Some(resource_id) = scope.resource_id() {
        key.push_str(&format!("-RESOURCE#{}", identifier_to_hex(resource_id)));
    }
    key
}

/// The persistent authorization view, read from durable state at startup (§5.3 rule 5).
///
/// A snapshot rather than a live query wrapper because `AuthorizationView` and
/// `ManagerAuthorityView` are synchronous. Every question it cannot answer returns `None` or
/// `AuthorityState::Unknown`, which denies: a missed notification is a cache miss, never an
/// approval.
pub struct DurableAuthorizationView {
    epochs: HashMap<Scope, u64>,
    roles: HashMap<(String, String), String>,
    interim_authority_anchor: Option<Hash32>,
    version: u64,
}

impl DurableAuthorizationView {
    /// Reads durable epoch state and the interim role table into a fresh view.
    ///
    /// Call at startup and after any admitted epoch bump. `interim_authority_anchor` is the one
    /// `issuer_authority_id` this deployment recognises while SPEC-2's authority history is
    /// unimplemented; `None` means no authority anchor resolves, so every root-issuer question
    /// answers `Unknown` and every chain verification denies.
    pub async fn load(
        db: &Db,
        interim_authority_anchor: Option<Hash32>,
    ) -> Result<Self, StorageError> {
        let epoch_store = SurrealScopeEpochStore::new(db.clone());
        let mut epochs = HashMap::new();
        for (key, record) in epoch_store.all_records().await? {
            match scope_from_key(&key) {
                Ok(scope) => {
                    epochs.insert(scope, record.current_epoch);
                }
                Err(detail) => {
                    return Err(StorageError::malformed(
                        "authorization view load",
                        format!("scope_key {key}: {detail}"),
                    ))
                }
            }
        }

        let mut response = db
            .query("SELECT peer_did, scope, role FROM role")
            .await
            .map_err(|error| StorageError::query("authorization view load", error))?
            .check()
            .map_err(|error| StorageError::query("authorization view load", error))?;
        let rows: Vec<serde_json::Value> = response
            .take(0)
            .map_err(|error| StorageError::query("authorization view load", error))?;
        let mut roles = HashMap::with_capacity(rows.len());
        for row in rows {
            let (Some(peer_did), Some(scope), Some(role)) = (
                row.get("peer_did").and_then(serde_json::Value::as_str),
                row.get("scope").and_then(serde_json::Value::as_str),
                row.get("role").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            roles.insert((peer_did.to_string(), scope.to_string()), role.to_string());
        }

        Ok(Self {
            epochs,
            roles,
            interim_authority_anchor,
            version: VIEW_VERSION.fetch_add(1, Ordering::SeqCst) + 1,
        })
    }

    /// The number of epoch scopes this view knows.
    pub fn known_scope_count(&self) -> usize {
        self.epochs.len()
    }

    fn role_level(&self, principal: &Principal, epoch_scope: &Scope) -> Option<RoleLevel> {
        let key = (
            principal.did.clone(),
            interim_authority_scope_key(epoch_scope),
        );
        self.roles
            .get(&key)
            .map(|role| RoleLevel::from(role.as_str()))
    }
}

impl AuthorizationView for DurableAuthorizationView {
    fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
        self.epochs.get(epoch_scope).copied()
    }

    fn version(&self) -> u64 {
        self.version
    }
}

impl ManagerAuthorityView for DurableAuthorizationView {
    fn authority_is_manager_plus(
        &self,
        authority_id: &Hash32,
        issuer: &Principal,
        epoch_scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorityState {
        // An unrecognised authority anchor is an unanswered question, and an unanswered
        // question refuses. It is NOT "not Manager+": the two are different outcomes and
        // `AuthorityState` keeps them apart on purpose.
        match self.interim_authority_anchor {
            Some(anchor) if anchor == *authority_id => {
                self.principal_is_manager_plus(issuer, epoch_scope, scope_epoch)
            }
            _ => AuthorityState::Unknown,
        }
    }

    fn principal_is_manager_plus(
        &self,
        principal: &Principal,
        epoch_scope: &Scope,
        _scope_epoch: u64,
    ) -> AuthorityState {
        match self.role_level(principal, epoch_scope) {
            Some(level) if level.is_at_least(RoleLevel::Manager) => AuthorityState::ManagerPlus,
            Some(_) => AuthorityState::NotManagerPlus,
            None => AuthorityState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;
    use fe_canonical_log::envelope::{
        Author, CapabilityRef, CompleteEnvelope, Hlc, Identifier32, PayloadRef, UnsignedEnvelope,
        PROTOCOL_VERSION,
    };
    use fe_canonical_log::signing::sign_envelope;

    /// An authorization view that answers every Manager+ question the same way.
    struct FixtureAuthority(AuthorityState);

    impl AuthorizationView for FixtureAuthority {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            None
        }

        fn version(&self) -> u64 {
            1
        }
    }

    impl ManagerAuthorityView for FixtureAuthority {
        fn authority_is_manager_plus(
            &self,
            _authority_id: &Hash32,
            _issuer: &Principal,
            _epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            self.0
        }

        fn principal_is_manager_plus(
            &self,
            _principal: &Principal,
            _epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            self.0
        }
    }

    /// A signed kind-4 `scope_epoch_bump` for [`verse_scope`] declaring `scope_epoch`.
    fn epoch_bump(author_seed: u8, scope_epoch: u64) -> (Hash32, Vec<u8>, String) {
        let signing_key = SigningKey::from_bytes(&[author_seed; 32]);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: OperationKind::ScopeEpochBump.to_u16(),
            scope: verse_scope(),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: Identifier32([0x55; 32]),
            parents: vec![Hash32([0x66; 32])],
            hlc: Hlc::new(1_000, 0),
            payload: PayloadRef::empty(),
        };
        let complete = sign_envelope(&signing_key, &unsigned).expect("sign bump");
        let bytes = complete.encode_canonical().expect("encode bump");
        let did = complete.unsigned.author.did.clone();
        (Hash32::of(&bytes), bytes, did)
    }

    fn verse_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    fn petal_scope() -> Scope {
        Scope::new(
            Identifier32([0x11; 32]),
            Some(Identifier32([0x22; 32])),
            None,
        )
        .expect("petal scope")
    }

    #[test]
    fn the_durable_scope_key_round_trips_and_separates_scopes() {
        let verse = scope_key(&verse_scope()).expect("verse key");
        let petal = scope_key(&petal_scope()).expect("petal key");
        assert_ne!(verse, petal);
        assert_eq!(scope_from_key(&verse).expect("decode"), verse_scope());
        assert_eq!(scope_from_key(&petal).expect("decode"), petal_scope());
        assert!(scope_from_key("not-hex").is_err());
    }

    #[test]
    fn the_interim_role_key_is_not_the_legacy_fractal_scope_string() {
        let key = interim_authority_scope_key(&petal_scope());
        assert!(key.starts_with("VERSE#1111"));
        assert!(key.contains("-PETAL#2222"));
        assert!(
            !key.contains("FRACTAL"),
            "the epoch hierarchy is verse/petal/resource, not verse/fractal/petal"
        );
        assert_eq!(
            interim_authority_scope_key(&verse_scope()),
            format!("VERSE#{}", "11".repeat(32))
        );
    }

    async fn memory_database() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("canon_log")
            .use_db("canon_log")
            .await
            .expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");
        db
    }

    #[tokio::test]
    async fn an_unknown_scope_reads_as_epoch_zero_with_no_evidence() {
        let db = memory_database().await;
        let store = SurrealScopeEpochStore::new(db);
        let record = store.record(&verse_scope()).await.expect("record");
        assert_eq!(record, ScopeEpochRecord::default());
        assert_eq!(record.current_epoch, 0);
    }

    #[tokio::test]
    async fn the_startup_view_denies_every_question_it_cannot_answer() {
        let db = memory_database().await;
        let view = DurableAuthorizationView::load(&db, None)
            .await
            .expect("view load");

        assert_eq!(view.current_epoch(&verse_scope()), None);
        assert_eq!(view.known_scope_count(), 0);
        assert_eq!(
            view.authority_is_manager_plus(
                &Hash32([0x01; 32]),
                &Principal::from_public_key([0x02; 32]),
                &verse_scope(),
                0
            ),
            AuthorityState::Unknown
        );
        assert_eq!(
            view.principal_is_manager_plus(
                &Principal::from_public_key([0x02; 32]),
                &verse_scope(),
                0
            ),
            AuthorityState::Unknown
        );
    }

    #[tokio::test]
    async fn the_interim_manager_source_reads_the_role_table_through_the_anchor() {
        let db = memory_database().await;
        let manager = Principal::from_public_key([0x03; 32]);
        let viewer = Principal::from_public_key([0x04; 32]);
        for (principal, role) in [(&manager, "manager"), (&viewer, "viewer")] {
            db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
                .bind(("did", principal.did.clone()))
                .bind(("scope", interim_authority_scope_key(&verse_scope())))
                .bind(("role", role.to_string()))
                .await
                .expect("seed role")
                .check()
                .expect("seed role check");
        }

        let anchor = Hash32([0xa0; 32]);
        let view = DurableAuthorizationView::load(&db, Some(anchor))
            .await
            .expect("view load");

        assert_eq!(
            view.authority_is_manager_plus(&anchor, &manager, &verse_scope(), 0),
            AuthorityState::ManagerPlus
        );
        assert_eq!(
            view.authority_is_manager_plus(&anchor, &viewer, &verse_scope(), 0),
            AuthorityState::NotManagerPlus
        );
        assert_eq!(
            view.authority_is_manager_plus(&Hash32([0xa1; 32]), &manager, &verse_scope(), 0),
            AuthorityState::Unknown,
            "a chain rooted at an unrecognised authority anchor is never allowed"
        );
        assert_eq!(
            view.principal_is_manager_plus(&manager, &petal_scope(), 0),
            AuthorityState::Unknown,
            "a verse-scoped role does not answer a petal-scoped question"
        );
    }

    #[tokio::test]
    async fn an_epoch_bump_without_manager_authority_changes_nothing() {
        let db = memory_database().await;
        let store = SurrealVerifiedLogStore::new(db.clone());
        let epochs = SurrealScopeEpochStore::new(db);
        let (op_id, bytes, author_did) = epoch_bump(0x21, 0);
        store
            .append_received(op_id, &bytes)
            .await
            .expect("append the bump");

        let mut gate = RevalidationGate::new();
        for state in [AuthorityState::Unknown, AuthorityState::NotManagerPlus] {
            let rejection = epochs
                .admit_epoch_bump(&store, &FixtureAuthority(state), op_id, &mut gate)
                .await
                .expect_err("only a Manager+ identity may bump an epoch");
            assert_eq!(
                rejection,
                EpochBumpRejection::NotManagerPlus {
                    author_did: author_did.clone(),
                    authority: state,
                }
            );
            assert_eq!(
                epochs.current_epoch(&verse_scope()).await.expect("read"),
                None,
                "an unauthorized bump must not even create the scope row"
            );
        }

        // The identical operation, once the view can answer Manager+.
        let admission = epochs
            .admit_epoch_bump(
                &store,
                &FixtureAuthority(AuthorityState::ManagerPlus),
                op_id,
                &mut gate,
            )
            .await
            .expect("an authorized bump admits");
        assert_eq!(
            admission,
            EpochBumpAdmission::Bumped {
                previous_epoch: 0,
                current_epoch: 1,
            }
        );
        assert_eq!(
            epochs.current_epoch(&verse_scope()).await.expect("read"),
            Some(1)
        );
    }

    #[tokio::test]
    async fn an_unsigned_row_cannot_move_an_epoch() {
        let db = memory_database().await;
        let store = SurrealVerifiedLogStore::new(db.clone());
        let epochs = SurrealScopeEpochStore::new(db.clone());
        let (op_id, bytes, _) = epoch_bump(0x21, 0);
        store
            .append_received(op_id, &bytes)
            .await
            .expect("append the bump");

        // Substitute the stored bytes with a canonically-encoded envelope nobody signed. It
        // still decodes; only the signature check can tell the difference.
        let mut complete = CompleteEnvelope::decode_canonical(&bytes).expect("decode");
        complete.signature[0] ^= 0xff;
        let forged = complete.encode_canonical().expect("encode");
        db.query("UPDATE verified_op_log SET envelope_bytes = $bytes WHERE op_id_hex = $id")
            .bind(("bytes", BASE64.encode(&forged)))
            .bind(("id", op_id_to_hex(op_id)))
            .await
            .expect("substitute stored bytes")
            .check()
            .expect("substitute check");

        let mut gate = RevalidationGate::new();
        let rejection = epochs
            .admit_epoch_bump(
                &store,
                &FixtureAuthority(AuthorityState::ManagerPlus),
                op_id,
                &mut gate,
            )
            .await
            .expect_err("an unsigned envelope authorizes nothing");
        assert!(
            matches!(rejection, EpochBumpRejection::Unverified { .. }),
            "{rejection:?}"
        );
        assert_eq!(
            epochs.current_epoch(&verse_scope()).await.expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn a_concurrent_second_bump_is_evidence_and_does_not_move_the_epoch() {
        let db = memory_database().await;
        let store = SurrealVerifiedLogStore::new(db.clone());
        let epochs = SurrealScopeEpochStore::new(db);
        let (first_op_id, first_bytes, _) = epoch_bump(0x21, 0);
        let (second_op_id, second_bytes, _) = epoch_bump(0x22, 0);
        for (op_id, bytes) in [(first_op_id, &first_bytes), (second_op_id, &second_bytes)] {
            store
                .append_received(op_id, bytes)
                .await
                .expect("append the bump");
        }

        let authority = FixtureAuthority(AuthorityState::ManagerPlus);
        let mut gate = RevalidationGate::new();
        assert_eq!(
            epochs
                .admit_epoch_bump(&store, &authority, first_op_id, &mut gate)
                .await
                .expect("first bump"),
            EpochBumpAdmission::Bumped {
                previous_epoch: 0,
                current_epoch: 1,
            }
        );
        assert_eq!(
            epochs
                .admit_epoch_bump(&store, &authority, second_op_id, &mut gate)
                .await
                .expect("second bump"),
            EpochBumpAdmission::ConcurrentDuplicate { current_epoch: 1 }
        );

        // §5.1 rule 6: the epoch moved once, and BOTH bumps survive as auditable evidence.
        // The compare-and-swap on `current_epoch` is what keeps the second write from
        // overwriting the first one's evidence list wholesale.
        let record = epochs.record(&verse_scope()).await.expect("record");
        assert_eq!(record.current_epoch, 1);
        assert_eq!(record.admitted_bump_op_ids.len(), 2);
        assert!(record.admitted_bump_op_ids.contains(&first_op_id));
        assert!(record.admitted_bump_op_ids.contains(&second_op_id));
    }

    #[tokio::test]
    async fn each_view_reload_reports_a_strictly_greater_version() {
        let db = memory_database().await;
        let first = DurableAuthorizationView::load(&db, None)
            .await
            .expect("first load");
        let second = DurableAuthorizationView::load(&db, None)
            .await
            .expect("second load");
        assert!(second.version() > first.version());
    }
}
