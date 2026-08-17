//! SPEC-4 §4 deterministic causal replay against the durable apply markers.
//! Rationale: `canon_log/AGENTS.md` §replay-eligibility.

use std::collections::{HashMap, HashSet, VecDeque};

use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::identity::ProjectionIdentity;
use fe_canonical_log::materialize::ordering::{deterministic_causal_order, OrderingError};
use fe_canonical_log::materialize::traits::{CausalMaterializer, VerifiedEnvelopeMeta};

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

/// Replays the causal closure of `frontier` into one projection, resumably.
///
/// Eligibility is read from the DURABLE apply markers, never from an in-process set: a replay
/// interrupted halfway resumes from exactly where the markers say it stopped, and a child stays
/// pending until its parents carry markers for this same materializer version (§4.2). Concurrent
/// siblings are ordered by the §4.3 `(wall_ms, counter, author_public_key)` key, so the result
/// does not depend on the order operations arrived in.
pub async fn replay_to_frontier(
    store: &SurrealVerifiedLogStore,
    markers: &SurrealApplyMarkerStore,
    materializer: &dyn CausalMaterializer,
    identity: &ProjectionIdentity,
    frontier: &SortedFrontier,
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

        let mutation = materializer.reduce(&meta, &bytes).await;
        let committable = match CommittableMutation::try_from_projection(&mutation) {
            Ok(committable) => committable,
            Err(NotCommittable::ReferencedArtifactUnavailable { artifact_id }) => {
                outcome.pending.push((
                    op_id,
                    PendingReason::ReferencedArtifactUnavailable { artifact_id },
                ));
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

/// The admitted operations of a closure, and the ancestors that are not locally admitted.
type CausalClosure = (HashMap<Hash32, VerifiedEnvelopeMeta>, HashSet<Hash32>);

/// Walks `parents_of` from every selected head, returning what is locally admitted and what is
/// not. A parent that cannot be read is absent, which halts its head rather than skipping it.
async fn load_closure(
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
