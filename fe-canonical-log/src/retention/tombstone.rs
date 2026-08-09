//! Tombstone retention and non-resurrection (SPEC-5 §5.4); see `src/retention/AGENTS.md`
//! §tombstone. Reproduces and supersedes, for the canonical path, the invariant
//! `fe-database/src/merge.rs:49-109` enforces for the legacy replicated-row path: a local
//! tombstone must dominate an incoming live row, never the reverse.

use thiserror::Error;

use crate::checkpoint::{
    compaction_decision, BootstrapCoverage, CheckpointVerification, CompactionDecision,
};
use crate::envelope::{Hash32, Identifier32, Scope};

/// One retained bootstrap path a peer might replay through, and whether it carries proof that
/// the suppression (the tombstone) would still be observed on that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapPath {
    /// Identifies which bootstrap path this is (e.g. a segment root or checkpoint chain link).
    pub path_id: Hash32,
    /// Whether replaying this path is provable to still surface the suppression.
    pub carries_suppression_proof: bool,
}

/// Retention bookkeeping for one tombstone: which bootstrap paths must keep proving the
/// suppression before the tombstone's own history may be compacted away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TombstoneRetentionRecord {
    /// The `op_id` of the tombstoning operation.
    pub tombstone_op_id: Hash32,
    /// Authorization scope the tombstone applies within (SPEC-3 §1.7 containment; M2).
    pub scope: Scope,
    /// The object the tombstone suppresses.
    pub suppressed_object_id: Identifier32,
    /// Bootstrap paths a fresh peer might replay through, and their suppression-proof status.
    pub retained_bootstrap_paths: Vec<BootstrapPath>,
}

/// Every reason [`assert_no_resurrection`] refuses to certify a tombstone record.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum TombstoneError {
    /// A retained bootstrap path does not carry proof the suppression survives on it.
    #[error("bootstrap path {path_id:?} does not carry a suppression proof")]
    MissingSuppressionProof {
        /// The offending path.
        path_id: Hash32,
    },
}

/// Fails if any retained bootstrap path lacks the suppression proof: a fresh peer replaying
/// that path could observe the suppressed object as if it were never deleted.
pub fn assert_no_resurrection(record: &TombstoneRetentionRecord) -> Result<(), TombstoneError> {
    if let Some(unproven) = record
        .retained_bootstrap_paths
        .iter()
        .find(|path| !path.carries_suppression_proof)
    {
        return Err(TombstoneError::MissingSuppressionProof {
            path_id: unproven.path_id,
        });
    }
    Ok(())
}

/// Whether ONE tombstone's retained history may be compacted (SPEC-5 §5.4 rule 2).
///
/// A thin single-record spelling of [`compaction_decision`], not a second gate: it runs exactly
/// the same computation over the same real [`CheckpointVerification`]. There is deliberately no
/// caller-supplied verdict enum any more — the previous two-variant stand-in was satisfiable by
/// anyone who typed the word `Verified`, with no signature, no replay and no frontier
/// commitment behind it.
pub fn may_compact_tombstone(
    record: &TombstoneRetentionRecord,
    verification: &CheckpointVerification,
    coverage: BootstrapCoverage,
) -> CompactionDecision {
    compaction_decision(verification, coverage, std::slice::from_ref(record))
}

/// Whether a tombstone's header/metadata and its ciphertext are retained. These two axes are
/// deliberately independent: releasing ciphertext (crypto-shredding or GC of the sealed
/// artifact) carries no implication for header/tombstone retention, and vice versa. No method
/// on this type couples them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadReleaseState {
    /// Whether the tombstone's own header/metadata is still retained.
    pub header_retained: bool,
    /// Whether the associated ciphertext artifact has been released (GC'd).
    pub ciphertext_released: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::checkpoint::{CheckpointRejection, CompactionRefusal};
    use crate::retention::leases::{ArtifactSetMembership, GcEligibility, GcLeaseRegistry};

    /// No lease's committed artifact set covers the tombstone under test.
    struct NoLeaseCovers;
    impl ArtifactSetMembership for NoLeaseCovers {
        fn set_contains(&self, _commitment: Hash32, _artifact_id: Hash32) -> bool {
            false
        }
    }

    /// The only `CheckpointVerification` a test can hold without a real signed claim: the
    /// refused one. There is deliberately no way to fabricate `Verified` here — obtaining it
    /// requires `verify_checkpoint_claim` over real bytes, which is the point of the change.
    fn unverified() -> CheckpointVerification {
        CheckpointVerification::Rejected(CheckpointRejection::NotManagerPlus)
    }

    fn complete_coverage() -> BootstrapCoverage {
        BootstrapCoverage {
            every_head_has_retained_bootstrap_path: true,
        }
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn scope() -> Scope {
        Scope::verse_wide(identifier(0x11))
    }

    fn record_with_proof(proven: bool) -> TombstoneRetentionRecord {
        TombstoneRetentionRecord {
            tombstone_op_id: hash(1),
            scope: scope(),
            suppressed_object_id: identifier(2),
            retained_bootstrap_paths: vec![BootstrapPath {
                path_id: hash(3),
                carries_suppression_proof: proven,
            }],
        }
    }

    #[test]
    fn assert_no_resurrection_rejects_any_unproven_bootstrap_path() {
        let unproven = record_with_proof(false);
        assert_eq!(
            assert_no_resurrection(&unproven),
            Err(TombstoneError::MissingSuppressionProof { path_id: hash(3) })
        );

        let proven = record_with_proof(true);
        assert_eq!(assert_no_resurrection(&proven), Ok(()));
    }

    #[test]
    fn may_compact_tombstone_requires_both_a_verified_checkpoint_and_proven_paths() {
        // An unverified checkpoint refuses regardless of the paths, and the refusal names the
        // checkpoint rather than the paths: there is no verdict value a caller can supply to
        // reach `Permitted` without `verify_checkpoint_claim` having returned `Verified`.
        for record in [record_with_proof(true), record_with_proof(false)] {
            assert_eq!(
                may_compact_tombstone(&record, &unverified(), complete_coverage()),
                CompactionDecision::Refused(CompactionRefusal::CheckpointNotVerified)
            );
        }

        // The single-record spelling is exactly the shared gate.
        let unproven = record_with_proof(false);
        assert_eq!(
            may_compact_tombstone(&unproven, &unverified(), complete_coverage()),
            compaction_decision(
                &unverified(),
                complete_coverage(),
                std::slice::from_ref(&unproven)
            )
        );
    }

    #[test]
    fn gc_preserves_tombstone_non_resurrection() {
        let unproven = record_with_proof(false);
        let registry = GcLeaseRegistry::new();

        // Without a suppression proof, GC must treat the artifact as still required for
        // tombstone retention.
        assert!(assert_no_resurrection(&unproven).is_err());
        assert_eq!(
            registry.gc_eligibility(
                unproven.tombstone_op_id,
                crate::envelope::Hlc::new(0, 0),
                &NoLeaseCovers,
                false,
                true,
            ),
            GcEligibility::BlockedByTombstoneRetention
        );

        // Once every retained bootstrap path proves the suppression survives, GC frees it.
        let proven = record_with_proof(true);
        assert!(assert_no_resurrection(&proven).is_ok());
        assert_eq!(
            registry.gc_eligibility(
                proven.tombstone_op_id,
                crate::envelope::Hlc::new(0, 0),
                &NoLeaseCovers,
                false,
                false,
            ),
            GcEligibility::Eligible
        );
    }

    #[test]
    fn payload_release_state_axes_are_independent() {
        for header_retained in [true, false] {
            for ciphertext_released in [true, false] {
                let state = PayloadReleaseState {
                    header_retained,
                    ciphertext_released,
                };
                assert_eq!(state.header_retained, header_retained);
                assert_eq!(state.ciphertext_released, ciphertext_released);
            }
        }
    }
}
