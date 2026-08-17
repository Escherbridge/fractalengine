//! SPEC-4 §6 rebuild: empty-state and verified-checkpoint-plus-tail, over the same replay.
//! Rationale: `canon_log/AGENTS.md` §rebuild-and-checkpoints.

use async_trait::async_trait;
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::checkpoint_binding::{
    CheckpointBinding, CheckpointBindingMismatch,
};
use fe_canonical_log::materialize::identity::ProjectionIdentity;
use fe_canonical_log::materialize::traits::CausalMaterializer;

use crate::canon_log::append_store::{SurrealVerifiedLogStore, SurrealVerseDagView};
use crate::canon_log::apply_marker_store::SurrealApplyMarkerStore;
use crate::canon_log::replay::{replay_to_frontier, ReplayError, ReplayOutcome};
use crate::canon_log::StorageError;

/// The projection tables a materializer owns, as the rebuild path needs to see them.
///
/// SPEC-4 §6.1 requires a projection to be rebuildable from empty state plus verified
/// operations, which means someone has to be able to empty it and to commit to its contents.
/// Neither is knowable here — the row layout belongs to the materializer — so both are seams.
#[async_trait]
pub trait ProjectionSurface: Send + Sync {
    /// Drops every row this projection identity owns.
    async fn reset_to_empty(&self, identity: &ProjectionIdentity) -> anyhow::Result<()>;

    /// The §6.3 `projection_root_hash`: a deterministic commitment to the projected state.
    async fn projection_root_hash(&self, identity: &ProjectionIdentity) -> anyhow::Result<Hash32>;
}

/// Which §6.1 starting point the rebuild actually used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebuildSource {
    /// Markers and projection rows were cleared and the whole closure was replayed.
    EmptyState,
    /// A checkpoint validated against the caller's own selection, so only the tail replayed.
    VerifiedCheckpoint {
        /// The projection root the checkpoint claims.
        claimed_projection_root_hash: Hash32,
    },
}

/// What one rebuild produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebuildReport {
    /// The starting point actually used.
    pub source: RebuildSource,
    /// Why an offered checkpoint was not trusted, when one was offered and refused (§6.2).
    pub rejected_checkpoint: Option<CheckpointBindingMismatch>,
    /// The replay pass that ran from that starting point.
    pub replay: ReplayOutcome,
    /// The projection root the surface reports afterwards.
    pub projection_root_hash: Hash32,
}

/// Every reason a rebuild does not finish.
#[derive(Debug, thiserror::Error)]
pub enum RebuildError {
    /// The verified log or the marker table could not be read or cleared.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The replay pass itself could not run.
    #[error(transparent)]
    Replay(#[from] ReplayError),
    /// The materializer's own projection surface failed.
    #[error("projection surface failed: {0}")]
    Surface(String),
}

/// What the caller is rebuilding, and what it currently believes.
///
/// `segment_manifest_id` and `frontier` are the CALLER's own derived values, never a relay's
/// assertion: §6.4 makes recomputing them the whole point of validating a checkpoint.
pub struct RebuildRequest<'a> {
    /// The projection being rebuilt.
    pub identity: &'a ProjectionIdentity,
    /// The frontier the caller selected, derived from immutable operation IDs.
    pub frontier: &'a SortedFrontier,
    /// The segment manifest the caller derived.
    pub segment_manifest_id: Hash32,
    /// An offered acceleration claim, if any.
    pub checkpoint: Option<&'a CheckpointBinding>,
}

/// Rebuilds a projection from a verified checkpoint when its binding holds, and from empty
/// state when it does not.
///
/// §6.2 makes a checkpoint an accelerator and never an authority, so a binding that fails
/// [`CheckpointBinding::validate`] is recorded in the report and then ignored — the rebuild
/// still produces a projection, it just pays full replay for it. A matching Manager+ signature
/// could not change that: this function is never handed one.
///
/// `dag_view` is a required parameter, not an option, because a completed replay is the only
/// admissible evidence for `VerseDagView::frontier_is_replay_verified`; making it a parameter
/// means the evidence is recorded wherever a rebuild happens instead of being assumed later.
pub async fn rebuild_projection(
    store: &SurrealVerifiedLogStore,
    markers: &SurrealApplyMarkerStore,
    materializer: &dyn CausalMaterializer,
    surface: &dyn ProjectionSurface,
    dag_view: &mut SurrealVerseDagView,
    request: RebuildRequest<'_>,
) -> Result<RebuildReport, RebuildError> {
    let mut rejected_checkpoint = None;
    let mut source = RebuildSource::EmptyState;

    if let Some(binding) = request.checkpoint {
        match binding.validate(
            request.identity.branch_id,
            request.frontier,
            request.segment_manifest_id,
            &request.identity.materializer_version,
        ) {
            Ok(()) => {
                source = RebuildSource::VerifiedCheckpoint {
                    claimed_projection_root_hash: binding.projection_root_hash,
                };
            }
            Err(mismatch) => rejected_checkpoint = Some(mismatch),
        }
    }

    if source == RebuildSource::EmptyState {
        // §6.1: nothing but verified operations may be required for correctness, so the
        // untrusted path throws away every derived row and every marker first.
        surface
            .reset_to_empty(request.identity)
            .await
            .map_err(|error| RebuildError::Surface(format!("{error:#}")))?;
        markers.clear_projection(request.identity).await?;
    }

    let replay = replay_to_frontier(
        store,
        markers,
        materializer,
        request.identity,
        request.frontier,
    )
    .await?;

    if replay.is_complete() {
        dag_view.record_replay_verified(request.identity.branch_id, request.frontier);
    }

    let projection_root_hash = surface
        .projection_root_hash(request.identity)
        .await
        .map_err(|error| RebuildError::Surface(format!("{error:#}")))?;

    Ok(RebuildReport {
        source,
        rejected_checkpoint,
        replay,
        projection_root_hash,
    })
}
