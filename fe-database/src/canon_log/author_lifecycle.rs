//! SPEC-2 author-key lifecycle rebuilt from the durable verified log, as the admission seam.
//! Rationale: `canon_log/AGENTS.md` §durable-authority-sources.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use fe_canonical_log::author_key::admission::{
    AuthorityState as LifecycleAuthority, AuthorizationState, CapabilityAuthorityView,
    RegisteredSchemas,
};
use fe_canonical_log::author_key::disavow_rescind::AuthorityLevel;
use fe_canonical_log::author_key::lineage::{CausalOperationView, KeyState};
use fe_canonical_log::capability::Principal;
use fe_canonical_log::compose::{
    AuthorKeyLifecycleView, LifecycleDispatch, LifecycleDispatcher, LifecycleKind,
};
use fe_canonical_log::envelope::{Hash32, Scope};
use fe_canonical_log::materialize::errors::AdmissionOutcome;
use fe_canonical_log::materialize::ordering::{deterministic_causal_order, OrderingError};
use fe_canonical_log::materialize::traits::{
    admit_candidate_with_facts, CandidateVerifier, PayloadFacts, VerifiedCandidate,
    VerifiedEnvelopeMeta,
};
use fe_policy::RoleLevel;

use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::canonical_epoch::DurableAuthorizationView;
use crate::canon_log::{op_id_from_hex, op_id_to_hex, StorageError};

/// Every reason the lifecycle could not be rebuilt or extended.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    /// The verified log could not be read.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The stored parent graph is cyclic, which no admitted DAG can be.
    #[error(transparent)]
    Ordering(#[from] OrderingError),
    /// The operation to observe is not in the verified log.
    #[error("operation {0} is not in the verified log")]
    NotAppended(String),
}

/// What one rebuild from the verified log did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifecycleReplayReport {
    /// Verified-log operations placed in the causal view.
    pub operations: usize,
    /// Lifecycle operations handed to their SPEC-2 entrypoint, with its answer, in causal order.
    pub dispatched: Vec<(Hash32, LifecycleDispatch)>,
    /// Lifecycle operations that could not be re-admitted here (for example a shredded payload
    /// key): SPEC-2 holds them, so they do not move the lineage.
    pub held: Vec<(Hash32, AdmissionOutcome)>,
    /// Stored rows that are not readable operations and were left out of the causal view.
    pub unreadable_rows: Vec<String>,
}

/// The causal DAG of the verified log: every stored operation is admitted by construction.
#[derive(Clone, Debug, Default)]
struct CausalDag {
    parents: HashMap<Hash32, Vec<Hash32>>,
}

impl CausalDag {
    fn insert(&mut self, op_id: Hash32, parents: &[Hash32]) {
        self.parents
            .entry(op_id)
            .or_insert_with(|| parents.to_vec());
    }
}

impl CausalOperationView for CausalDag {
    fn parents(&self, operation: Hash32) -> Option<Vec<Hash32>> {
        self.parents.get(&operation).cloned()
    }

    fn is_admitted(&self, operation: Hash32) -> bool {
        self.parents.contains_key(&operation)
    }

    fn reaches(&self, from: Hash32, target: Hash32) -> bool {
        let mut seen = HashSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(operation) = queue.pop_front() {
            if operation == target {
                return true;
            }
            for parent in self.parents.get(&operation).into_iter().flatten() {
                if seen.insert(*parent) {
                    queue.push_back(*parent);
                }
            }
        }
        false
    }
}

/// The operation whose SPEC-3 append authority the composed verifier (or, on replay, its
/// presence in the verified log) already established.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AdmittedAuthor {
    public_key: [u8; 32],
    scope: Scope,
    scope_epoch: u64,
}

/// The `CapabilityAuthorityView` SPEC-2 admission consults, answered from durable state.
///
/// `predecessor_authorized` is asked about exactly the dispatched operation's own author,
/// scope and epoch, so it answers `Authorized` only for that operation — whose chain the
/// composed verifier verified before it was appended — and `Unauthorized` for any other
/// question. `issuer_role` reads the interim role table (§interim-manager-authority); no row
/// is `Unavailable`, which SPEC-2 holds rather than refuses.
struct DurableLifecycleAuthority {
    roles: DurableAuthorizationView,
    admitted: Mutex<Option<AdmittedAuthor>>,
}

impl DurableLifecycleAuthority {
    fn set_admitted(&self, author: Option<AdmittedAuthor>) {
        *self.admitted.lock().expect("lifecycle authority lock") = author;
    }
}

impl CapabilityAuthorityView for DurableLifecycleAuthority {
    fn predecessor_authorized(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationState {
        let asked = AdmittedAuthor {
            public_key: *author_public_key,
            scope: *scope,
            scope_epoch,
        };
        match *self.admitted.lock().expect("lifecycle authority lock") {
            Some(admitted) if admitted == asked => AuthorizationState::Authorized,
            _ => AuthorizationState::Unauthorized,
        }
    }

    fn issuer_role(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        _scope_epoch: u64,
    ) -> LifecycleAuthority {
        let principal = Principal::from_public_key(*author_public_key);
        match self.roles.role_level(&principal, scope) {
            Some(level) => LifecycleAuthority::Resolved(match level {
                RoleLevel::Owner => AuthorityLevel::Owner,
                RoleLevel::Manager => AuthorityLevel::Manager,
                RoleLevel::Editor => AuthorityLevel::Editor,
                RoleLevel::Viewer => AuthorityLevel::Viewer,
                RoleLevel::None => AuthorityLevel::None,
            }),
            None => LifecycleAuthority::Unavailable,
        }
    }
}

struct LifecycleState {
    dispatcher: LifecycleDispatcher<DurableLifecycleAuthority>,
    dag: CausalDag,
}

/// SPEC-2 §4 lineage over the durable verified log: the production `AuthorKeyLifecycleView`.
///
/// Rebuilt by [`Self::replay_verified_log`] from every `verified_op_log` row in
/// `deterministic_causal_order`, and extended by [`Self::observe_appended`] after each live
/// append, both through the one `LifecycleDispatcher`. The lifecycle is state derived from the
/// log, never stored beside it, so a restart reproduces it exactly.
pub struct DurableAuthorKeyLifecycle {
    state: RwLock<LifecycleState>,
}

impl std::fmt::Debug for DurableAuthorKeyLifecycle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.read_state();
        formatter
            .debug_struct("DurableAuthorKeyLifecycle")
            .field("operations", &state.dag.parents.len())
            .field("rotations", &state.dispatcher.state().lineage.len())
            .finish()
    }
}

impl DurableAuthorKeyLifecycle {
    /// An empty lifecycle over `schemas`, answering issuer roles from `roles`.
    pub fn new(schemas: RegisteredSchemas, roles: DurableAuthorizationView) -> Self {
        let authority = DurableLifecycleAuthority {
            roles,
            admitted: Mutex::new(None),
        };
        Self {
            state: RwLock::new(LifecycleState {
                dispatcher: LifecycleDispatcher::new(schemas, authority),
                dag: CausalDag::default(),
            }),
        }
    }

    /// Rebuilds the causal view and the lineage from every stored operation.
    ///
    /// `opener` re-opens each lifecycle operation's payload exactly as admission did (ingress,
    /// payload key, schema); its authorization step is NOT re-run, because the row's presence
    /// in `verified_op_log` is the record that it already passed (`replay` trusts the same).
    pub async fn replay_verified_log<V>(
        &self,
        opener: &V,
        store: &SurrealVerifiedLogStore,
    ) -> Result<LifecycleReplayReport, LifecycleError>
    where
        V: CandidateVerifier + ?Sized,
    {
        let mut report = LifecycleReplayReport::default();
        let mut stored: HashMap<Hash32, (VerifiedEnvelopeMeta, Vec<u8>)> = HashMap::new();
        for row in store.all_rows().await? {
            match readable_operation(&row) {
                Ok((meta, bytes)) => {
                    stored.insert(meta.op_id, (meta, bytes));
                }
                Err(detail) => report.unreadable_rows.push(detail),
            }
        }
        let metas: Vec<VerifiedEnvelopeMeta> =
            stored.values().map(|(meta, _)| meta.clone()).collect();

        for op_id in deterministic_causal_order(&metas)? {
            let (meta, bytes) = &stored[&op_id];
            report.operations += 1;
            match self
                .place_and_dispatch(opener, store, meta, bytes, None)
                .await
            {
                Ok(LifecycleDispatch::NotLifecycle) => {}
                Ok(dispatch) => report.dispatched.push((op_id, dispatch)),
                Err(outcome) => report.held.push((op_id, outcome)),
            }
        }
        Ok(report)
    }

    /// Places one just-appended operation in the causal view and, when it names a lifecycle
    /// schema, dispatches it. `payload_ciphertext` is the ciphertext admission was handed.
    ///
    /// `Ok(Err(outcome))` is a lifecycle operation the replay admission could not re-open.
    pub async fn observe_appended<V>(
        &self,
        opener: &V,
        store: &SurrealVerifiedLogStore,
        op_id: Hash32,
        payload_ciphertext: Option<&[u8]>,
    ) -> Result<Result<LifecycleDispatch, AdmissionOutcome>, LifecycleError>
    where
        V: CandidateVerifier + ?Sized,
    {
        let bytes = store
            .envelope_bytes(op_id)
            .await?
            .ok_or_else(|| LifecycleError::NotAppended(op_id_to_hex(op_id)))?;
        let candidate = verified_stored_operation(op_id, &bytes).map_err(|detail| {
            LifecycleError::Storage(StorageError::malformed("lifecycle observe", detail))
        })?;
        Ok(self
            .place_and_dispatch(opener, store, &candidate.meta, &bytes, payload_ciphertext)
            .await)
    }

    /// Operations in the causal view.
    pub fn known_operations(&self) -> usize {
        self.read_state().dag.parents.len()
    }

    /// Rotations recorded in the lineage.
    pub fn recorded_rotations(&self) -> usize {
        self.read_state().dispatcher.state().lineage.len()
    }

    async fn place_and_dispatch<V>(
        &self,
        opener: &V,
        store: &SurrealVerifiedLogStore,
        meta: &VerifiedEnvelopeMeta,
        bytes: &[u8],
        payload_ciphertext: Option<&[u8]>,
    ) -> Result<LifecycleDispatch, AdmissionOutcome>
    where
        V: CandidateVerifier + ?Sized,
    {
        let kind = {
            let mut state = self.write_state();
            state.dag.insert(meta.op_id, &meta.parents);
            state.dispatcher.lifecycle_kind(meta)
        };
        let Some(kind) = kind else {
            return Ok(LifecycleDispatch::NotLifecycle);
        };

        let reopened = admit_candidate_with_facts(
            &StoredOperationOpener { opener },
            store,
            store,
            bytes,
            payload_ciphertext,
        )
        .await?;

        let mut guard = self.write_state();
        let state = &mut *guard;
        let unsigned = &reopened.candidate.envelope.unsigned;
        if kind == LifecycleKind::Rotation {
            // Interim lineage root (SPEC-2 has no authority history yet): the rotating key's
            // authority at this verse rests on the chain admission verified for this exact
            // operation. A key already retired on this causal path stays retired — the root
            // check runs after the rotation walk in `LineageIndex::active_key_at`.
            state.dispatcher.register_root_key(
                Scope::verse_wide(unsigned.scope.verse_id()),
                unsigned.author.public_key,
            );
        }
        state
            .dispatcher
            .authority()
            .set_admitted(Some(AdmittedAuthor {
                public_key: unsigned.author.public_key,
                scope: unsigned.scope,
                scope_epoch: unsigned.capability.scope_epoch,
            }));
        let dispatch = state.dispatcher.dispatch(&state.dag, &reopened);
        state.dispatcher.authority().set_admitted(None);
        Ok(dispatch)
    }

    fn read_state(&self) -> std::sync::RwLockReadGuard<'_, LifecycleState> {
        self.state.read().expect("lifecycle state lock")
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, LifecycleState> {
        self.state.write().expect("lifecycle state lock")
    }
}

impl AuthorKeyLifecycleView for DurableAuthorKeyLifecycle {
    fn key_state(
        &self,
        author_public_key: &[u8; 32],
        scope: &Scope,
        parents: &[Hash32],
    ) -> KeyState {
        let state = self.read_state();
        state
            .dispatcher
            .lifecycle_view(&state.dag)
            .key_state(author_public_key, scope, parents)
    }
}

/// Re-opens a STORED operation for dispatch: ingress and payload exactly as `opener` admits,
/// authorization answered by the row's presence in `verified_op_log`.
///
/// Private on purpose: it is only ever handed bytes this module read back from the verified
/// log under their own `op_id`, never a candidate.
struct StoredOperationOpener<'a, V: ?Sized> {
    opener: &'a V,
}

#[async_trait]
impl<V: CandidateVerifier + ?Sized> CandidateVerifier for StoredOperationOpener<'_, V> {
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedCandidate, AdmissionOutcome> {
        self.opener.verify_envelope(candidate_bytes).await
    }

    async fn verify_authorization(
        &self,
        _candidate: &VerifiedCandidate,
    ) -> Result<(), AdmissionOutcome> {
        Ok(())
    }

    async fn verify_payload(
        &self,
        candidate: &VerifiedCandidate,
        payload_ciphertext: Option<&[u8]>,
    ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
        self.opener
            .verify_payload(candidate, payload_ciphertext)
            .await
    }
}

/// A `verified_op_log` row re-derived from its own bytes: base64, BLAKE3 against the row's id,
/// then the signing ingress. Index columns are never read.
fn readable_operation(row: &serde_json::Value) -> Result<(VerifiedEnvelopeMeta, Vec<u8>), String> {
    let id_text = row
        .get("op_id_hex")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "op_id_hex is not a string".to_string())?;
    let op_id = op_id_from_hex(id_text).ok_or_else(|| format!("bad op_id hex {id_text}"))?;
    let encoded = row
        .get("envelope_bytes")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{id_text}: envelope_bytes is not a string"))?;
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| format!("{id_text}: {error}"))?;
    let candidate = verified_stored_operation(op_id, &bytes)?;
    Ok((candidate.meta, bytes))
}

fn verified_stored_operation(op_id: Hash32, bytes: &[u8]) -> Result<VerifiedCandidate, String> {
    if Hash32::of(bytes) != op_id {
        return Err(format!(
            "{}: bytes do not hash to the row's op_id",
            op_id_to_hex(op_id)
        ));
    }
    VerifiedCandidate::verify_from_bytes(bytes)
        .map_err(|error| format!("{}: {error}", op_id_to_hex(op_id)))
}
