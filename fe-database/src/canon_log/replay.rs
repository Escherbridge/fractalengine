//! SPEC-4 §4 deterministic causal replay against the durable apply markers, with an optional
//! replay-time payload facts source. Rationale: `canon_log/AGENTS.md` §replay-eligibility and
//! §replay-payload-facts.

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use fe_canonical_log::compose::QuarantineReason;
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::errors::AdmissionOutcome;
use fe_canonical_log::materialize::identity::ProjectionIdentity;
use fe_canonical_log::materialize::ordering::{deterministic_causal_order, OrderingError};
use fe_canonical_log::materialize::traits::{
    verify_candidate, CandidateVerifier, CausalMaterializer, PayloadFacts, VerifiedEnvelopeMeta,
};

use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::apply_marker_store::{
    CommittableMutation, MarkerCommit, NotCommittable, SurrealApplyMarkerStore,
};
use crate::canon_log::{op_id_to_hex, StorageError};

/// Why a projection position is pending replay rather than settled.
///
/// Every variant is retryable. None of them is a decision about the operation: §5 note 2
/// forbids treating an availability gap as proof that a candidate is valid or invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingReason {
    /// A parent is not locally admitted, so the whole causal chain below it is unresolved.
    ParentMissing {
        /// The absent ancestor.
        parent: Hash32,
    },
    /// A parent is admitted but not yet processed for THIS materializer version.
    ParentPending {
        /// The unprocessed parent.
        parent: Hash32,
    },
    /// §5 `referenced_artifact_unavailable`: the reduction cannot construct its verified
    /// reference here yet. The apply marker deliberately does not advance.
    ReferencedArtifactUnavailable {
        /// The artifact the operation names.
        artifact_id: Hash32,
    },
    /// §5 `materialization_failed`: append succeeded, the atomic commit did not finish.
    MaterializationFailed {
        /// The substrate's own message.
        detail: String,
    },
    /// The reduction needs the operation's OWN payload facts and the replay facts source could
    /// not re-derive them (`canon_log/AGENTS.md` §replay-payload-facts).
    ///
    /// Distinct from [`Self::ReferencedArtifactUnavailable`] so a crypto-shredded scope key is
    /// visible as such; like it, the marker does not move, any row already projected stays, and
    /// the operation is never `Excluded`.
    PayloadFactsUnavailable {
        /// The payload ciphertext the operation's signed `PayloadRef` commits to.
        ciphertext_hash: Hash32,
        /// Why the facts could not be re-derived here.
        gap: PayloadFactsGap,
    },
}

/// Why a replay facts source could not re-derive an operation's own payload facts.
///
/// Every variant is an availability state of THIS process, never a decision about the
/// operation, which was already admitted with valid facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadFactsGap {
    /// No held scope key opens the payload: crypto-shredded (SPEC-5 §5.5.4) or never delivered.
    ScopeKeyUnavailable,
    /// The payload ciphertext is not in the local artifact store.
    CiphertextUnavailable,
    /// The verifier interprets no schema for this kind or `schema_hash` (not registered).
    SchemaNotInterpreted,
    /// The held bytes no longer authenticate or validate here; carries the verifier's reason.
    PayloadNotVerifiable {
        /// The verifier's own message.
        detail: String,
    },
}

/// What a [`ReplayPayloadFacts`] source answered for one operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayFactsLookup {
    /// The source does not re-derive facts (the legacy wrapper): `reduce` gets `None` and its
    /// pre-B2b outcome stands unchanged.
    NotConsulted,
    /// The operation is in the no-payload form; there are no facts to derive.
    NoPayload,
    /// The operation's own decrypted, schema-validated facts, bound to its `op_id`.
    Derived(PayloadFacts),
    /// The facts exist in signed bytes but cannot be re-derived here.
    Unavailable {
        /// The payload ciphertext the operation's signed `PayloadRef` commits to.
        ciphertext_hash: Hash32,
        /// Why.
        gap: PayloadFactsGap,
    },
}

/// Where a replay pass gets each operation's own payload facts (CDL-SPEC10 B2b).
///
/// An implementation may only open the operation's OWN committed payload; it never reads a
/// hexon, a statistics artifact, or an asset blob, so the G2 bright line still holds.
#[async_trait]
pub trait ReplayPayloadFacts: Send + Sync {
    /// Whether this source re-derives facts at all; `false` keeps every pre-B2b behaviour,
    /// including skipping the rebuild's re-derivability check.
    fn rederives(&self) -> bool {
        true
    }

    /// The facts of `op_id`, whose stored canonical bytes are `envelope_bytes`.
    async fn payload_facts(&self, op_id: Hash32, envelope_bytes: &[u8]) -> ReplayFactsLookup;
}

/// The legacy source: no facts, ever. [`replay_to_frontier`] and `rebuild_projection` use it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoReplayPayloadFacts;

#[async_trait]
impl ReplayPayloadFacts for NoReplayPayloadFacts {
    fn rederives(&self) -> bool {
        false
    }

    async fn payload_facts(&self, _op_id: Hash32, _envelope_bytes: &[u8]) -> ReplayFactsLookup {
        ReplayFactsLookup::NotConsulted
    }
}

/// Re-derives facts through a [`CandidateVerifier`]'s own `verify_payload`, with the
/// ciphertext resolved from the verifier's store: for the durable verifier, the W4-7
/// `SurrealArtifactStore` ciphertext, `SecretStoreScopeKeyCustody` key, and
/// `RegisteredSchemaSet`.
///
/// The verifier is a snapshot of custody: build it after the last shred
/// (`durable_candidate_verifier`, or `ComposedCandidateVerifier::new` over parts whose custody
/// ran the shred), or a stale snapshot still holds the destroyed key.
pub struct VerifierReplayFacts<'a, V: ?Sized> {
    verifier: &'a V,
}

impl<'a, V: CandidateVerifier + ?Sized> VerifierReplayFacts<'a, V> {
    /// Re-derives through `verifier`.
    pub fn new(verifier: &'a V) -> Self {
        Self { verifier }
    }
}

#[async_trait]
impl<V: CandidateVerifier + ?Sized> ReplayPayloadFacts for VerifierReplayFacts<'_, V> {
    async fn payload_facts(&self, op_id: Hash32, envelope_bytes: &[u8]) -> ReplayFactsLookup {
        // The mandatory ingress again: the facts must be bound to bytes that verify as `op_id`.
        let candidate = match verify_candidate(envelope_bytes) {
            Ok(candidate) if candidate.meta.op_id == op_id => candidate,
            Ok(_) => {
                return ReplayFactsLookup::Unavailable {
                    ciphertext_hash: op_id,
                    gap: PayloadFactsGap::PayloadNotVerifiable {
                        detail: "stored bytes are not the requested operation".to_string(),
                    },
                }
            }
            Err(outcome) => {
                return ReplayFactsLookup::Unavailable {
                    ciphertext_hash: op_id,
                    gap: PayloadFactsGap::PayloadNotVerifiable {
                        detail: outcome.to_string(),
                    },
                }
            }
        };
        let ciphertext_hash = candidate.envelope.unsigned.payload.ciphertext_hash;
        let unavailable = |gap| ReplayFactsLookup::Unavailable {
            ciphertext_hash,
            gap,
        };
        match self.verifier.verify_payload(&candidate, None).await {
            Ok(None) => ReplayFactsLookup::NoPayload,
            // The same binding `admit_candidate_with_facts` enforces: never another op's facts.
            Ok(Some(facts))
                if facts.op_id == op_id && facts.schema_hash == candidate.meta.schema_hash =>
            {
                ReplayFactsLookup::Derived(facts)
            }
            Ok(Some(_)) => unavailable(PayloadFactsGap::PayloadNotVerifiable {
                detail: "verifier returned facts bound to another operation".to_string(),
            }),
            Err(outcome) => unavailable(facts_gap(outcome)),
        }
    }
}

/// Maps a `verify_payload` refusal at replay onto the availability vocabulary.
fn facts_gap(outcome: AdmissionOutcome) -> PayloadFactsGap {
    match outcome {
        AdmissionOutcome::AuthorizationEvidenceUnavailable { reason }
            if matches!(reason.reason(), QuarantineReason::PayloadKeyUnavailable) =>
        {
            PayloadFactsGap::ScopeKeyUnavailable
        }
        AdmissionOutcome::OpaquePayload { .. } => PayloadFactsGap::CiphertextUnavailable,
        AdmissionOutcome::UnknownKind { .. } | AdmissionOutcome::UnknownSchema { .. } => {
            PayloadFactsGap::SchemaNotInterpreted
        }
        other => PayloadFactsGap::PayloadNotVerifiable {
            detail: other.to_string(),
        },
    }
}

/// A selected head that cannot be resolved because its causal closure is incomplete.
///
/// §5 note 1: an error mid-history must never let a materializer skip the candidate and carry
/// on as though the history were complete, so the head stays unresolved and is reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HaltedHead {
    /// The selected head.
    pub head: Hash32,
    /// The ancestor whose absence halts it.
    pub missing_ancestor: Hash32,
}

/// Everything one replay pass settled and everything it left pending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayOutcome {
    /// Operations reduced and committed in this pass, in causal order.
    pub applied: Vec<Hash32>,
    /// Operations that already carried a marker; `reduce` was not re-invoked for them.
    pub already_processed: Vec<Hash32>,
    /// Operations whose reduction deliberately projected nothing (§4.6), with the reason.
    pub excluded: Vec<(Hash32, String)>,
    /// Projection positions left pending replay, sorted by operation identity.
    pub pending: Vec<(Hash32, PendingReason)>,
    /// Selected heads left unresolved, sorted by head identity.
    pub halted_heads: Vec<HaltedHead>,
}

impl ReplayOutcome {
    /// Whether every selected head resolved and nothing is pending.
    ///
    /// Only a complete pass may be presented as a committed projection position (§2.6).
    pub fn is_complete(&self) -> bool {
        self.pending.is_empty() && self.halted_heads.is_empty()
    }
}

/// Every reason a replay pass cannot run to a conclusion.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// The verified log or the marker table could not be read.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The admitted operations do not form a DAG, which a valid history never does.
    #[error(transparent)]
    Ordering(#[from] OrderingError),
}

/// Replays the causal closure of `frontier` into one projection, resumably, handing every
/// reduction `None` for payload facts ([`NoReplayPayloadFacts`]).
///
/// Unchanged by CDL-SPEC10 B2b: a reduction that needs facts stays pending under
/// [`PendingReason::ReferencedArtifactUnavailable`]. Use [`replay_to_frontier_with_facts`] to
/// re-derive them.
pub async fn replay_to_frontier(
    store: &SurrealVerifiedLogStore,
    markers: &SurrealApplyMarkerStore,
    materializer: &dyn CausalMaterializer,
    identity: &ProjectionIdentity,
    frontier: &SortedFrontier,
) -> Result<ReplayOutcome, ReplayError> {
    replay_to_frontier_with_facts(
        store,
        markers,
        materializer,
        identity,
        frontier,
        &NoReplayPayloadFacts,
    )
    .await
}

/// [`replay_to_frontier`], with each not-yet-processed operation's own payload facts taken
/// from `facts` (`canon_log/AGENTS.md` §replay-payload-facts).
///
/// Facts are looked up only for operations about to be reduced; an already-marked operation
/// is skipped before any lookup, so a row projected before a crypto-shred is never touched.
/// When the reduction asks for its own payload (`ReferencedArtifactUnavailable` naming its own
/// ciphertext) and `facts` answered `Unavailable`, the position stays pending under
/// [`PendingReason::PayloadFactsUnavailable`], never `Excluded`.
///
/// Eligibility is read from the DURABLE apply markers, never from an in-process set: a replay
/// interrupted halfway resumes from exactly where the markers say it stopped, and a child stays
/// pending until its parents carry markers for this same materializer version (§4.2). Concurrent
/// siblings are ordered by the §4.3 `(wall_ms, counter, author_public_key)` key, so the result
/// does not depend on the order operations arrived in.
pub async fn replay_to_frontier_with_facts(
    store: &SurrealVerifiedLogStore,
    markers: &SurrealApplyMarkerStore,
    materializer: &dyn CausalMaterializer,
    identity: &ProjectionIdentity,
    frontier: &SortedFrontier,
    facts: &dyn ReplayPayloadFacts,
) -> Result<ReplayOutcome, ReplayError> {
    let (present, missing) = load_closure(store, frontier).await?;

    // Causal completeness in topological order: a parent is settled before its children are
    // examined, so one pass suffices and the blocking ancestor propagates downward.
    let metas: Vec<VerifiedEnvelopeMeta> = present.values().cloned().collect();
    let order = deterministic_causal_order(&metas)?;
    let mut blocked_by: HashMap<Hash32, Hash32> = HashMap::new();
    for op_id in &order {
        let meta = &present[op_id];
        for parent in &meta.parents {
            if missing.contains(parent) {
                blocked_by.insert(*op_id, *parent);
                break;
            }
            if let Some(ancestor) = blocked_by.get(parent).copied() {
                blocked_by.insert(*op_id, ancestor);
                break;
            }
        }
    }

    let mut outcome = ReplayOutcome::default();

    for head in frontier.as_slice() {
        if missing.contains(head) {
            outcome.halted_heads.push(HaltedHead {
                head: *head,
                missing_ancestor: *head,
            });
        } else if let Some(missing_ancestor) = blocked_by.get(head).copied() {
            outcome.halted_heads.push(HaltedHead {
                head: *head,
                missing_ancestor,
            });
        }
    }

    for op_id in order {
        let meta = present[&op_id].clone();

        if let Some(parent) = blocked_by.get(&op_id).copied() {
            outcome
                .pending
                .push((op_id, PendingReason::ParentMissing { parent }));
            continue;
        }

        // §4.2 against the durable marker, not memory: this is what makes replay resumable.
        let mut unprocessed_parent = None;
        for parent in &meta.parents {
            if !markers.is_applied(identity, *parent).await? {
                unprocessed_parent = Some(*parent);
                break;
            }
        }
        if let Some(parent) = unprocessed_parent {
            outcome
                .pending
                .push((op_id, PendingReason::ParentPending { parent }));
            continue;
        }

        if markers.is_applied(identity, op_id).await? {
            // §3.4: an already-marked operation is a no-op; `reduce` is not called again.
            outcome.already_processed.push(op_id);
            continue;
        }

        let Some(bytes) = store.envelope_bytes(op_id).await? else {
            return Err(StorageError::malformed(
                "replay",
                format!(
                    "{} vanished between closure and reduce",
                    op_id_to_hex(op_id)
                ),
            )
            .into());
        };
        // The closure load verified this row through `meta_of`; these are a SECOND read of it.
        // Content addressing closes the window between the two: bytes that still hash to
        // `op_id` are the bytes that were verified, so `reduce` cannot be handed a substituted
        // envelope by a writer that changed the row in between.
        if Hash32::of(&bytes) != op_id {
            return Err(StorageError::malformed(
                "replay",
                format!(
                    "{} was substituted between closure and reduce",
                    op_id_to_hex(op_id)
                ),
            )
            .into());
        }

        // The operation's OWN payload facts, re-derived from bytes that hash to `op_id`
        // (`materialize/AGENTS.md` §payload-facts); the legacy source answers `NotConsulted`.
        let lookup = facts.payload_facts(op_id, &bytes).await;
        let derived = match &lookup {
            ReplayFactsLookup::Derived(payload_facts) => Some(payload_facts),
            _ => None,
        };
        let mutation = materializer.reduce(&meta, &bytes, derived).await;
        let committable = match CommittableMutation::try_from_projection(&mutation) {
            Ok(committable) => committable,
            Err(NotCommittable::ReferencedArtifactUnavailable { artifact_id }) => {
                outcome
                    .pending
                    .push((op_id, unavailable_reason(artifact_id, lookup)));
                continue;
            }
        };

        match markers
            .commit_with_marker(identity, op_id, &committable)
            .await
        {
            Ok(MarkerCommit::Applied) => outcome.applied.push(op_id),
            Ok(MarkerCommit::Excluded) => {
                let reason = match &committable {
                    CommittableMutation::Excluded { reason } => reason.clone(),
                    CommittableMutation::Apply { .. } => String::new(),
                };
                outcome.excluded.push((op_id, reason));
            }
            Ok(MarkerCommit::AlreadyProcessed { .. }) => outcome.already_processed.push(op_id),
            Err(error) => outcome.pending.push((
                op_id,
                PendingReason::MaterializationFailed {
                    detail: error.to_string(),
                },
            )),
        }
    }

    outcome.pending.sort_by_key(|(op_id, _)| *op_id);
    outcome.halted_heads.sort_by_key(|halted| halted.head);
    Ok(outcome)
}

/// The pending reason for a reduction that could not construct `artifact_id`: the facts gap
/// when it asked for its own payload and the facts source said why it could not have it.
fn unavailable_reason(artifact_id: Hash32, lookup: ReplayFactsLookup) -> PendingReason {
    match lookup {
        ReplayFactsLookup::Unavailable {
            ciphertext_hash,
            gap,
        } if ciphertext_hash == artifact_id => PendingReason::PayloadFactsUnavailable {
            ciphertext_hash,
            gap,
        },
        _ => PendingReason::ReferencedArtifactUnavailable { artifact_id },
    }
}

/// The admitted operations of a closure, and the ancestors that are not locally admitted.
pub(crate) type CausalClosure = (HashMap<Hash32, VerifiedEnvelopeMeta>, HashSet<Hash32>);

/// Walks `parents_of` from every selected head, returning what is locally admitted and what is
/// not. A parent that cannot be read is absent, which halts its head rather than skipping it.
pub(crate) async fn load_closure(
    store: &SurrealVerifiedLogStore,
    frontier: &SortedFrontier,
) -> Result<CausalClosure, StorageError> {
    let mut present: HashMap<Hash32, VerifiedEnvelopeMeta> = HashMap::new();
    let mut missing: HashSet<Hash32> = HashSet::new();
    let mut seen: HashSet<Hash32> = HashSet::new();
    let mut queue: VecDeque<Hash32> = VecDeque::new();

    for head in frontier.as_slice() {
        if seen.insert(*head) {
            queue.push_back(*head);
        }
    }

    while let Some(op_id) = queue.pop_front() {
        match store.meta_of(op_id).await? {
            Some(meta) => {
                for parent in &meta.parents {
                    if seen.insert(*parent) {
                        queue.push_back(*parent);
                    }
                }
                present.insert(op_id, meta);
            }
            None => {
                missing.insert(op_id);
            }
        }
    }

    Ok((present, missing))
}
