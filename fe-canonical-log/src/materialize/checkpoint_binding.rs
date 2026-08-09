//! SPEC-4 §6.3 checkpoint binding and §6.4 consumer validation; see `src/AGENTS.md`
//! §module-ownership and `materialize/AGENTS.md` for why this module does not define a signed
//! claim type.

use thiserror::Error;

use crate::envelope::{Hash32, Identifier32};
use crate::frontier::SortedFrontier;
use crate::materialize::errors::AdmissionOutcome;
use crate::materialize::identity::MaterializerVersion;

/// The §6.3 five-field checkpoint binding: everything a checkpoint claim commits to.
///
/// This is a binding, not a signed artifact. The concrete signed claim type
/// (`SignedCheckpointClaim`) belongs to the sibling branch/checkpoint slice; binding the two
/// happens in the serial integration wave (`compose.rs`). A matching Manager+ signature over a
/// claim carrying this binding is NEVER proof by itself -- only [`CheckpointBinding::validate`]
/// recomputing the frontier commitment and comparing every other field against the caller's
/// own currently selected values is proof (§6.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointBinding {
    /// Branch this checkpoint projects.
    pub branch_id: Identifier32,
    /// D-CL19 commitment of the frontier this checkpoint replays to.
    pub frontier_commitment: Hash32,
    /// Segment manifest whose artifacts back the replay.
    pub segment_manifest_id: Hash32,
    /// Materializer identity this checkpoint's projection root was produced under.
    pub materializer_version: MaterializerVersion,
    /// Root hash of the projected state at this checkpoint.
    pub projection_root_hash: Hash32,
}

/// Every reason a [`CheckpointBinding`] does not match the caller's current selection.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CheckpointBindingMismatch {
    /// The checkpoint's `branch_id` is not the branch the caller is replaying.
    #[error("checkpoint branch_id does not match the caller's current branch")]
    BranchMismatch,
    /// Recomputing `SortedFrontier::commitment` over the caller's own frontier disagreed.
    #[error("recomputed frontier commitment does not match the checkpoint's bound commitment")]
    FrontierCommitmentMismatch,
    /// The checkpoint's `segment_manifest_id` is not the caller's current manifest.
    #[error("checkpoint's segment_manifest_id does not match the caller's current manifest")]
    SegmentManifestMismatch,
    /// The checkpoint's `materializer_version` is not the caller's current materializer.
    #[error("checkpoint's materializer_version does not match the caller's current materializer")]
    MaterializerVersionMismatch,
}

impl CheckpointBinding {
    /// SPEC-4 §6.4's consumer rule: recompute the frontier commitment from the caller's own
    /// [`SortedFrontier`] and require branch, manifest, and materializer version to match the
    /// caller's currently selected values.
    ///
    /// Deliberately does NOT compare `projection_root_hash` against an independently
    /// recomputed value: proving that requires an actual replay, which is the sibling
    /// `ReplayVerifier` seam's job (SPEC-5/SPEC-6), not a pure binding comparison this
    /// I/O-free crate can perform.
    pub fn validate(
        &self,
        expected_branch_id: Identifier32,
        expected_frontier: &SortedFrontier,
        current_segment_manifest_id: Hash32,
        current_materializer_version: &MaterializerVersion,
    ) -> Result<(), CheckpointBindingMismatch> {
        if self.branch_id != expected_branch_id {
            return Err(CheckpointBindingMismatch::BranchMismatch);
        }
        if self.frontier_commitment != expected_frontier.commitment() {
            return Err(CheckpointBindingMismatch::FrontierCommitmentMismatch);
        }
        if self.segment_manifest_id != current_segment_manifest_id {
            return Err(CheckpointBindingMismatch::SegmentManifestMismatch);
        }
        if &self.materializer_version != current_materializer_version {
            return Err(CheckpointBindingMismatch::MaterializerVersionMismatch);
        }
        Ok(())
    }
}

impl From<CheckpointBindingMismatch> for AdmissionOutcome {
    /// Collapses any specific mismatch reason into the single admission-facing
    /// `CheckpointMismatch` outcome. The specific [`CheckpointBindingMismatch`] remains
    /// available to the caller for logging before this conversion discards it.
    fn from(_mismatch: CheckpointBindingMismatch) -> Self {
        AdmissionOutcome::CheckpointMismatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_binding(
        branch_id: Identifier32,
        frontier: &SortedFrontier,
        manifest_id: Hash32,
        version: MaterializerVersion,
    ) -> CheckpointBinding {
        CheckpointBinding {
            branch_id,
            frontier_commitment: frontier.commitment(),
            segment_manifest_id: manifest_id,
            materializer_version: version,
            projection_root_hash: hash(0xaa),
        }
    }

    #[test]
    fn validate_succeeds_when_every_bound_field_matches_the_callers_current_selection() {
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10), hash(0x11)]).expect("frontier");
        let manifest_id = hash(0x20);
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, manifest_id, version.clone());

        assert_eq!(
            binding.validate(branch_id, &frontier, manifest_id, &version),
            Ok(())
        );
    }

    #[test]
    fn validate_rejects_a_branch_mismatch() {
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10)]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, hash(0x20), version.clone());

        assert_eq!(
            binding.validate(identifier(0x02), &frontier, hash(0x20), &version),
            Err(CheckpointBindingMismatch::BranchMismatch)
        );
    }

    #[test]
    fn validate_rejects_a_tampered_or_stale_frontier() {
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10), hash(0x11)]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, hash(0x20), version.clone());

        let different_frontier =
            SortedFrontier::try_new([hash(0x10), hash(0x99)]).expect("frontier");
        assert_eq!(
            binding.validate(branch_id, &different_frontier, hash(0x20), &version),
            Err(CheckpointBindingMismatch::FrontierCommitmentMismatch)
        );
    }

    #[test]
    fn validate_rejects_a_segment_manifest_mismatch() {
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10)]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, hash(0x20), version.clone());

        assert_eq!(
            binding.validate(branch_id, &frontier, hash(0x21), &version),
            Err(CheckpointBindingMismatch::SegmentManifestMismatch)
        );
    }

    #[test]
    fn validate_rejects_a_materializer_version_mismatch() {
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10)]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, hash(0x20), version.clone());

        let other_version = MaterializerVersion::new("scene-graph", 2);
        assert_eq!(
            binding.validate(branch_id, &frontier, hash(0x20), &other_version),
            Err(CheckpointBindingMismatch::MaterializerVersionMismatch)
        );
    }

    #[test]
    fn a_matching_signature_alone_is_not_representable_here_by_construction() {
        // This module has no signature field and no verifier parameter: `validate` cannot be
        // handed a signature to "trust" instead of recomputing the frontier commitment, which
        // is the load-bearing property SPEC-4 §6.4 requires.
        let branch_id = identifier(0x01);
        let frontier = SortedFrontier::try_new([hash(0x10)]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let binding = sample_binding(branch_id, &frontier, hash(0x20), version.clone());
        assert!(binding
            .validate(branch_id, &frontier, hash(0x20), &version)
            .is_ok());
    }

    #[test]
    fn mismatch_collapses_to_the_single_admission_facing_checkpoint_mismatch_outcome() {
        let converted: AdmissionOutcome =
            CheckpointBindingMismatch::FrontierCommitmentMismatch.into();
        assert_eq!(converted, AdmissionOutcome::CheckpointMismatch);
    }
}
