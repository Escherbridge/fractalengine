//! SPEC-4 §6 rebuild: empty-state and verified-checkpoint-plus-tail, over the same replay.
//! Rationale: `canon_log/AGENTS.md` §rebuild-and-checkpoints.

use async_trait::async_trait;
use fe_canonical_log::checkpoint::{
    decode_and_admit_checkpoint, CheckpointError, ManagerPlusAuthorizationView,
};
use fe_canonical_log::compose::{
    checkpoint_binding_for_selection, BindingSelection, CompositionError,
};
use fe_canonical_log::envelope::{Hash32, Scope};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::checkpoint_binding::CheckpointBinding;
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

/// Every reason an offered checkpoint never becomes an [`AdmittedCheckpoint`].
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum CheckpointOfferRejection {
    /// The bytes, the version, the DID binding, or the SPEC-5 §3.1 rule 4 signature failed.
    #[error("checkpoint claim is malformed or unsigned: {0}")]
    Malformed(#[from] CheckpointError),
    /// The signer is not Manager+ in the authorization view for this verse.
    #[error("checkpoint signer is not Manager+ for this verse")]
    NotManagerPlus,
    /// The signer holds no unexpired `append/checkpoint` capability for this verse scope.
    #[error("checkpoint signer's capability does not permit append/checkpoint for this verse")]
    CapabilityDoesNotPermitCheckpoint,
    /// The §6.4 comparison against the caller's own current selection failed.
    #[error(transparent)]
    Binding(#[from] CompositionError),
}

/// A checkpoint that passed byte-level admission, its signature, the Manager+ authority
/// questions, and the SPEC-4 §6.4 binding comparison against the caller's OWN selection.
///
/// Every field is private and [`AdmittedCheckpoint::admit`] is the only constructor, so
/// [`rebuild_projection`] cannot be handed a checkpoint nobody verified: the bare
/// `CheckpointBinding` that used to be this parameter carries no signature at all, and every
/// field in it is derivable by anyone who can see the branch, so offering one was free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedCheckpoint {
    checkpoint_id: Hash32,
    binding: CheckpointBinding,
}

impl AdmittedCheckpoint {
    /// Admits a received SPEC-5 checkpoint claim for one specific selection.
    ///
    /// Order: canonical re-encoding and signature (`decode_and_admit_checkpoint`), the two
    /// §3.2 authority questions, then `compose::checkpoint_binding_for_selection`, which is the
    /// one function that turns a claim into a §6.3 binding and which validates that binding
    /// against `selection` before returning it. A matching signature never substitutes for the
    /// binding comparison, and the binding comparison never substitutes for the signature.
    pub fn admit(
        received_bytes: &[u8],
        authorization: &dyn ManagerPlusAuthorizationView,
        selection: &BindingSelection<'_>,
    ) -> Result<Self, CheckpointOfferRejection> {
        let (signed, checkpoint_id) = decode_and_admit_checkpoint(received_bytes)?;
        let claim = &signed.claim;

        if !authorization.is_manager_plus_for_checkpoint(
            claim.verse_id,
            &claim.signer,
            &claim.capability,
        ) {
            return Err(CheckpointOfferRejection::NotManagerPlus);
        }
        let verse_scope = Scope::verse_wide(claim.verse_id);
        if !authorization.capability_permits_append_checkpoint(
            &verse_scope,
            &claim.signer,
            &claim.capability,
        ) {
            return Err(CheckpointOfferRejection::CapabilityDoesNotPermitCheckpoint);
        }

        let binding = checkpoint_binding_for_selection(claim, selection)?;
        Ok(Self {
            checkpoint_id,
            binding,
        })
    }

    /// BLAKE3 of the complete signed claim bytes.
    pub fn checkpoint_id(&self) -> Hash32 {
        self.checkpoint_id
    }

    /// The §6.3 projection root this checkpoint CLAIMS; a rebuild must reproduce it.
    pub fn claimed_projection_root_hash(&self) -> Hash32 {
        self.binding.projection_root_hash
    }
}

/// Which §6.1 starting point the rebuild actually used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebuildSource {
    /// Markers and projection rows were cleared and the whole closure was replayed.
    EmptyState,
    /// An admitted checkpoint whose claimed projection root the tail replay REPRODUCED, so
    /// only the tail replayed.
    VerifiedCheckpoint {
        /// BLAKE3 of the complete signed claim bytes.
        checkpoint_id: Hash32,
        /// The projection root the checkpoint claimed, which the replay then reproduced.
        claimed_projection_root_hash: Hash32,
    },
}

/// Why an admitted checkpoint was still not used as a starting point (§6.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceleratorRefusal {
    /// BLAKE3 of the complete signed claim bytes.
    pub checkpoint_id: Hash32,
    /// The projection root the checkpoint claimed.
    pub claimed_projection_root_hash: Hash32,
    /// The projection root the accelerated pass actually produced.
    pub computed_projection_root_hash: Hash32,
    /// Whether that accelerated pass even resolved every selected head.
    pub replay_was_complete: bool,
}

/// What one rebuild produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebuildReport {
    /// The starting point actually used.
    pub source: RebuildSource,
    /// The admitted checkpoint that did not reproduce, when one was offered and refused (§6.2).
    pub refused_accelerator: Option<AcceleratorRefusal>,
    /// The replay pass whose result the projection now reflects.
    pub replay: ReplayOutcome,
    /// The projection root the surface reports afterwards.
    pub projection_root_hash: Hash32,
    /// Whether this rebuild recorded `VerseDagView::frontier_is_replay_verified` evidence.
    pub replay_verified_recorded: bool,
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
/// `frontier` is the CALLER's own derived value, never a relay's assertion: §6.4 makes
/// recomputing it the whole point of validating a checkpoint. The segment manifest the caller
/// derived is compared inside [`AdmittedCheckpoint::admit`], which is why it is not repeated
/// here — one selection, compared once, in the place that can refuse.
pub struct RebuildRequest<'a> {
    /// The projection being rebuilt.
    pub identity: &'a ProjectionIdentity,
    /// The frontier the caller selected, derived from immutable operation IDs.
    pub frontier: &'a SortedFrontier,
    /// An admitted acceleration claim, if any.
    pub checkpoint: Option<&'a AdmittedCheckpoint>,
}

/// Rebuilds a projection from an admitted checkpoint when it reproduces, and from empty state
/// whenever it does not.
///
/// §6.2 makes a checkpoint an accelerator and never an authority, and this function spends that
/// sentence twice. First, an unsigned or wrongly-bound checkpoint cannot reach it at all: the
/// parameter is an [`AdmittedCheckpoint`], whose only constructor verifies the signature, asks
/// the Manager+ questions, and compares the binding to the caller's own selection. Second, even
/// an admitted checkpoint only suppresses the empty-state reset while its CLAIMED projection
/// root and the root the accelerated pass actually produced agree; when they do not, the
/// projection is thrown away and rebuilt from empty state, which is the state §6.1 guarantees.
///
/// The accelerated attempt writes its tail into the projection before that comparison can be
/// made — a projection root is a commitment to projected state, so there is nothing to compare
/// until the tail is applied. The empty-state fallback then clears exactly those rows, so a
/// refused accelerator leaves no trace in the result.
///
/// `dag_view` is a required parameter, not an option, because a completed replay is the only
/// admissible evidence for `VerseDagView::frontier_is_replay_verified`; making it a parameter
/// means the evidence is recorded wherever a rebuild happens instead of being assumed later.
/// Only the from-empty pass records it. A checkpoint-accelerated pass replayed a tail on top of
/// rows THIS process did not derive from verified operations, and §6.2 forbids presenting a
/// signature — however good — as the replay this crate did not run.
pub async fn rebuild_projection(
    store: &SurrealVerifiedLogStore,
    markers: &SurrealApplyMarkerStore,
    materializer: &dyn CausalMaterializer,
    surface: &dyn ProjectionSurface,
    dag_view: &mut SurrealVerseDagView,
    request: RebuildRequest<'_>,
) -> Result<RebuildReport, RebuildError> {
    let mut refused_accelerator = None;
    let mut accelerated: Option<(RebuildSource, ReplayOutcome, Hash32)> = None;

    if let Some(checkpoint) = request.checkpoint {
        let replay = replay_to_frontier(
            store,
            markers,
            materializer,
            request.identity,
            request.frontier,
        )
        .await?;
        let projection_root_hash = surface
            .projection_root_hash(request.identity)
            .await
            .map_err(|error| RebuildError::Surface(format!("{error:#}")))?;
        let claimed = checkpoint.claimed_projection_root_hash();

        if replay.is_complete() && projection_root_hash == claimed {
            accelerated = Some((
                RebuildSource::VerifiedCheckpoint {
                    checkpoint_id: checkpoint.checkpoint_id(),
                    claimed_projection_root_hash: claimed,
                },
                replay,
                projection_root_hash,
            ));
        } else {
            refused_accelerator = Some(AcceleratorRefusal {
                checkpoint_id: checkpoint.checkpoint_id(),
                claimed_projection_root_hash: claimed,
                computed_projection_root_hash: projection_root_hash,
                replay_was_complete: replay.is_complete(),
            });
        }
    }

    if let Some((source, replay, projection_root_hash)) = accelerated {
        return Ok(RebuildReport {
            source,
            refused_accelerator,
            replay,
            projection_root_hash,
            replay_verified_recorded: false,
        });
    }

    // §6.1: nothing but verified operations may be required for correctness, so the untrusted
    // path throws away every derived row and every marker first.
    surface
        .reset_to_empty(request.identity)
        .await
        .map_err(|error| RebuildError::Surface(format!("{error:#}")))?;
    markers.clear_projection(request.identity).await?;

    let replay = replay_to_frontier(
        store,
        markers,
        materializer,
        request.identity,
        request.frontier,
    )
    .await?;

    let replay_verified_recorded = replay.is_complete();
    if replay_verified_recorded {
        dag_view.record_replay_verified(request.identity.branch_id, request.frontier);
    }

    let projection_root_hash = surface
        .projection_root_hash(request.identity)
        .await
        .map_err(|error| RebuildError::Surface(format!("{error:#}")))?;

    Ok(RebuildReport {
        source: RebuildSource::EmptyState,
        refused_accelerator,
        replay,
        projection_root_hash,
        replay_verified_recorded,
    })
}
