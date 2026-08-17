//! Cross-module composition: the crate's single quarantine vocabulary and the bindings that
//! join SPEC-4's checkpoint binding, SPEC-5's signed claim, and SPEC-6's reachability proof.
//! See `src/AGENTS.md` §unified-vocabularies.

use thiserror::Error;

use crate::checkpoint::{CheckpointClaimV1, SignedCheckpointClaim};
use crate::envelope::{EquivocationKey, Hash32, Identifier32};
use crate::frontier::SortedFrontier;
use crate::materialize::checkpoint_binding::{CheckpointBinding, CheckpointBindingMismatch};
use crate::materialize::identity::MaterializerVersion;
use crate::segment::checkpoint_proof::CheckpointView;
use crate::segment::SegmentError;

/// Why a candidate operation or received artifact is withheld rather than admitted.
///
/// One vocabulary for all three quarantine paths — SPEC-2 lifecycle admission
/// (`author_key::admission`), SPEC-5 pre-admission holding (`retention::quarantine`), and SPEC-6
/// receipt refusal (`segment::store`). Each of those modules re-exports this type; none defines
/// its own. The reason never carries the candidate's own identifier: every store keys the reason
/// by that identifier already.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum QuarantineReason {
    /// Declared parents that are not locally present.
    #[error("{} declared parent(s) are not locally present", missing_parents.len())]
    MissingParent {
        /// Parents the candidate names that are absent here.
        missing_parents: Vec<Hash32>,
    },
    /// A declared parent is present but has not completed SPEC-1 admission.
    #[error("parent {parent_op_id:?} is present but not admitted")]
    ParentNotAdmitted {
        /// The parent the candidate names.
        parent_op_id: Hash32,
    },
    /// The declared payload schema is not one this process can resolve.
    #[error("schema {schema_hash:?} is not locally known")]
    UnknownSchema {
        /// The unrecognized schema hash.
        schema_hash: Hash32,
    },
    /// The declared `operation_kind` is not one this build can interpret (SPEC-1 §6 rule 7).
    ///
    /// Distinct from [`Self::UnknownSchema`] because the two have different producers and
    /// different promotion paths: a schema arrives through the registry, a kind arrives through
    /// a build upgrade. Under D-CL2 headers replicate verse-wide with no version gate, so an
    /// un-upgraded peer receives every operation of a kind it cannot yet interpret. D-CL28 gate
    /// G1 gives that traffic its own budget class so it cannot consume the room the same peer
    /// needs for its own [`Self::MissingParent`] backlog.
    #[error("operation kind {operation_kind} is not known to this build")]
    UnknownKind {
        /// The unrecognized non-zero wire operation kind.
        operation_kind: u16,
    },
    /// The scope payload key could not be resolved, so the payload stayed encrypted.
    #[error("the scope payload key is unavailable")]
    PayloadKeyUnavailable,
    /// The capability chain artifact could not be resolved.
    #[error("the capability chain artifact is unavailable")]
    CapabilityChainUnavailable,
    /// The scope-epoch state could not be resolved.
    #[error("the scope-epoch state is unavailable")]
    EpochStateUnavailable,
    /// The disavow a rescind names is not admitted here yet.
    #[error("the referenced disavow {disavow_op_id:?} is unavailable")]
    ReferencedDisavowUnavailable {
        /// The disavow the rescind names.
        disavow_op_id: Hash32,
    },
    /// SPEC-1 §3.4 author equivocation: two `op_id`s share one `(key, wall_ms, counter)`.
    ///
    /// Per D-CL25 the rule is quarantine BOTH candidates and materialize NEITHER, so the variant
    /// names both conflicting operations and a receiver never picks a winner.
    #[error(
        "author equivocation: {first_op_id:?} and {second_op_id:?} share {equivocation_key:?}"
    )]
    AuthorEquivocation {
        /// The author, wall clock, and counter both operations claim.
        equivocation_key: EquivocationKey,
        /// The first observed operation.
        first_op_id: Hash32,
        /// The conflicting operation.
        second_op_id: Hash32,
    },
    /// The recomputed digest disagreed with the requested artifact ID (SPEC-6 §5.1).
    #[error("artifact ID mismatch: claimed {claimed:?}, recomputed {computed:?}")]
    ArtifactIdMismatch {
        /// The requested identifier.
        claimed: Hash32,
        /// The identifier the bytes actually have.
        computed: Hash32,
    },
    /// The declared stored byte length disagreed with the bytes received (SPEC-6 §5.1).
    #[error("declared stored length does not match the bytes received")]
    StoredLengthMismatch,
    /// The sealed outer form was malformed, non-canonical, or used a non-production suite.
    #[error("the sealed outer form is malformed, non-canonical, or a non-production suite")]
    MalformedSealedArtifact,
    /// Authenticated decryption failed under the resolved scope key (SPEC-6 §5.3).
    #[error("the sealed body failed authenticated decryption")]
    FailedDecryption,
    /// The decrypted body was malformed or did not match its declared lane (SPEC-6 §2.1.5).
    #[error("the decrypted body is malformed or does not match its declared lane")]
    MalformedLaneBody,
    /// A header and shard reference did not cross-check in both directions (SPEC-6 §5.4).
    #[error("header and shard references for {op_id:?} do not cross-check")]
    CrossCheckFailure {
        /// The operation whose references disagreed.
        op_id: Hash32,
    },
    /// No current capability or scope key covered the candidate.
    #[error("no current capability or scope key covers this candidate")]
    Unauthorized,
}

/// The bounded budget a [`QuarantineReason`] draws against (SPEC-5 §4 rule 3, D-CL28 gate G1).
///
/// Each class carries its own entry and byte budget in `retention::QuarantineBounds`, so one
/// retryable reason cannot exhaust the room another needs. The three named classes are the ones
/// a remote sender can drive independently of the receiver; `Other` is the shared residual. The
/// pool-wide bounds still apply on top of every class budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuarantineReasonClass {
    /// Retryable once the declared parent closure is locally admitted.
    MissingParent,
    /// Retryable once the exact registered schema resolves.
    UnknownSchema,
    /// Retryable once this build learns to interpret the operation kind.
    UnknownKind,
    /// Every reason outside the three independently driven retry classes.
    Other,
}

impl QuarantineReasonClass {
    /// Every class, for iterating budgets. Exhaustive by construction.
    pub const ALL: [Self; 4] = [
        Self::MissingParent,
        Self::UnknownSchema,
        Self::UnknownKind,
        Self::Other,
    ];
}

impl QuarantineReason {
    /// The budget class this reason spends.
    ///
    /// A total match with no wildcard arm on purpose: a new [`QuarantineReason`] variant does
    /// not compile until it declares which budget it draws against, so no future reason can
    /// silently join — and dilute — the residual class.
    pub fn class(&self) -> QuarantineReasonClass {
        match self {
            Self::MissingParent { .. } => QuarantineReasonClass::MissingParent,
            Self::UnknownSchema { .. } => QuarantineReasonClass::UnknownSchema,
            Self::UnknownKind { .. } => QuarantineReasonClass::UnknownKind,
            Self::ParentNotAdmitted { .. }
            | Self::PayloadKeyUnavailable
            | Self::CapabilityChainUnavailable
            | Self::EpochStateUnavailable
            | Self::ReferencedDisavowUnavailable { .. }
            | Self::AuthorEquivocation { .. }
            | Self::ArtifactIdMismatch { .. }
            | Self::StoredLengthMismatch
            | Self::MalformedSealedArtifact
            | Self::FailedDecryption
            | Self::MalformedLaneBody
            | Self::CrossCheckFailure { .. }
            | Self::Unauthorized => QuarantineReasonClass::Other,
        }
    }
}

/// Every reason a signed SPEC-5 claim does not describe the caller's own selected projection.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CompositionError {
    /// The SPEC-4 §6.4 binding comparison against the caller's own selection failed.
    #[error("{0}")]
    Binding(#[from] CheckpointBindingMismatch),
    /// The claim names a materializer other than the caller's.
    #[error("the signed claim names a different materializer identifier")]
    MaterializerIdentifierMismatch,
    /// The claim names a materializer version other than the caller's.
    #[error("the signed claim names materializer version {claimed}, caller runs {current}")]
    MaterializerVersionMismatch {
        /// Version the claim carries.
        claimed: u64,
        /// Version the caller runs.
        current: u64,
    },
}

/// The caller's own currently selected projection identity.
///
/// `materializer_id` is separate because the two specs name a materializer differently: a SPEC-5
/// claim carries the 32-byte registry identifier, a SPEC-4 binding carries the hand-authored
/// name/number pair, and only the caller holds the mapping between them.
#[derive(Clone, Copy, Debug)]
pub struct BindingSelection<'a> {
    /// Branch the caller is replaying.
    pub branch_id: Identifier32,
    /// The caller's own selected frontier, from immutable operation IDs.
    pub frontier: &'a SortedFrontier,
    /// The caller's own segment manifest for that selection.
    pub segment_manifest_id: Hash32,
    /// Registry identifier of `materializer_version`.
    pub materializer_id: Identifier32,
    /// The caller's current materializer identity.
    pub materializer_version: &'a MaterializerVersion,
}

/// The SPEC-4 §6.3 binding a signed SPEC-5 claim carries, validated against the caller's own
/// selection before it is handed back (§6.4).
///
/// The ONE function that turns a SPEC-5 claim into a SPEC-4 binding, and it is fallible. The
/// binding is DERIVED from the claim's own fields, so the two vocabularies cannot disagree by
/// construction; a caller then cannot obtain that binding without `CheckpointBinding::validate`
/// having recomputed the frontier commitment over the caller's own frontier and compared the
/// branch, manifest and materializer against the caller's current values. A separate
/// "also check these agree" helper would be a gate a future author can forget, so there is none.
pub fn checkpoint_binding_for_selection(
    claim: &CheckpointClaimV1,
    selection: &BindingSelection<'_>,
) -> Result<CheckpointBinding, CompositionError> {
    if claim.materializer_id != selection.materializer_id {
        return Err(CompositionError::MaterializerIdentifierMismatch);
    }
    let current = u64::from(selection.materializer_version.version);
    if claim.materializer_version != current {
        return Err(CompositionError::MaterializerVersionMismatch {
            claimed: claim.materializer_version,
            current,
        });
    }
    let binding = CheckpointBinding {
        branch_id: claim.branch_id,
        frontier_commitment: claim.frontier_commitment,
        segment_manifest_id: claim.segment_manifest_id,
        materializer_version: selection.materializer_version.clone(),
        projection_root_hash: claim.projection_root_hash,
    };
    binding.validate(
        selection.branch_id,
        selection.frontier,
        selection.segment_manifest_id,
        selection.materializer_version,
    )?;
    Ok(binding)
}

/// Presents a signed SPEC-5 checkpoint claim to the SPEC-6 §4.2 reachability proof.
///
/// The claim commits to a frontier *commitment*; the proof has to walk actual heads. Pairing the
/// two here is what lets `segment::checkpoint_proof::verify_checkpoint` recompute
/// `SortedFrontier::commitment` over the heads it walks and refuse a frontier the signature never
/// covered.
#[derive(Clone, Copy, Debug)]
pub struct SignedCheckpointProofView<'a> {
    claim: &'a SignedCheckpointClaim,
    selected_frontier: &'a SortedFrontier,
    replay_base: Option<Hash32>,
}

impl<'a> SignedCheckpointProofView<'a> {
    /// Binds a signed claim to the frontier the verifier itself selected.
    pub const fn new(
        claim: &'a SignedCheckpointClaim,
        selected_frontier: &'a SortedFrontier,
        replay_base: Option<Hash32>,
    ) -> Self {
        Self {
            claim,
            selected_frontier,
            replay_base,
        }
    }

    /// The signed claim under proof.
    pub const fn claim(&self) -> &SignedCheckpointClaim {
        self.claim
    }
}

impl CheckpointView for SignedCheckpointProofView<'_> {
    fn verse_id(&self) -> Identifier32 {
        self.claim.claim.verse_id
    }

    fn branch_id(&self) -> Identifier32 {
        self.claim.claim.branch_id
    }

    fn segment_manifest_id(&self) -> Hash32 {
        self.claim.claim.segment_manifest_id
    }

    fn selected_frontier(&self) -> &SortedFrontier {
        self.selected_frontier
    }

    fn frontier_commitment(&self) -> Hash32 {
        self.claim.claim.frontier_commitment
    }

    fn replay_base(&self) -> Option<Hash32> {
        self.replay_base
    }

    fn verify_manager_signature(&self) -> Result<(), SegmentError> {
        Ok(self.claim.verify_signature()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ed25519_dalek::SigningKey;

    use crate::checkpoint::{sign_checkpoint_claim, CHECKPOINT_VERSION};
    use crate::envelope::{Author, CapabilityRef, Hlc};

    const VERSE: Identifier32 = Identifier32([0x0e; 32]);
    const BRANCH: Identifier32 = Identifier32([0x0a; 32]);
    const MATERIALIZER: Identifier32 = Identifier32([0x11; 32]);
    const SEGMENT_MANIFEST: Hash32 = Hash32([0x51; 32]);

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x21; 32])
    }

    fn frontier() -> SortedFrontier {
        SortedFrontier::try_new([Hash32([0x10; 32]), Hash32([0x20; 32])]).expect("frontier")
    }

    fn claim(selected: &SortedFrontier) -> CheckpointClaimV1 {
        CheckpointClaimV1 {
            checkpoint_version: CHECKPOINT_VERSION,
            verse_id: VERSE,
            branch_id: BRANCH,
            frontier_commitment: selected.commitment(),
            segment_manifest_id: SEGMENT_MANIFEST,
            materializer_id: MATERIALIZER,
            materializer_version: 3,
            projection_root_hash: Hash32([0x7a; 32]),
            authorization_view_root: Hash32([0x5a; 32]),
            snapshot_manifest_id: None,
            signer: Author::from_public_key(signing_key().verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: Hash32([0x60; 32]),
                scope_epoch: 9,
            },
            issued_hlc: Hlc::new(1_700_000_000_000, 1),
        }
    }

    fn selection<'a>(
        frontier: &'a SortedFrontier,
        version: &'a MaterializerVersion,
    ) -> BindingSelection<'a> {
        BindingSelection {
            branch_id: BRANCH,
            frontier,
            segment_manifest_id: SEGMENT_MANIFEST,
            materializer_id: MATERIALIZER,
            materializer_version: version,
        }
    }

    #[test]
    fn a_claim_derived_binding_is_only_returned_after_it_validates() {
        let selected = frontier();
        let claim = claim(&selected);
        let version = MaterializerVersion::new("scene-graph", 3);

        let binding = checkpoint_binding_for_selection(&claim, &selection(&selected, &version))
            .expect("the claim matches the caller's selection");
        assert_eq!(binding.branch_id, BRANCH);
        assert_eq!(binding.frontier_commitment, selected.commitment());
        assert_eq!(binding.segment_manifest_id, SEGMENT_MANIFEST);
        assert_eq!(binding.projection_root_hash, claim.projection_root_hash);
        assert_eq!(binding.materializer_version, version);

        // Every disagreement returns an error INSTEAD of a binding: there is no way to hold a
        // claim-derived binding that was not validated against the caller's own selection.
        let collapsed = SortedFrontier::try_new([Hash32([0x10; 32])]).expect("frontier");
        assert_eq!(
            checkpoint_binding_for_selection(&claim, &selection(&collapsed, &version)),
            Err(CompositionError::Binding(
                CheckpointBindingMismatch::FrontierCommitmentMismatch
            ))
        );

        let other_branch = BindingSelection {
            branch_id: Identifier32([0x0b; 32]),
            ..selection(&selected, &version)
        };
        assert_eq!(
            checkpoint_binding_for_selection(&claim, &other_branch),
            Err(CompositionError::Binding(
                CheckpointBindingMismatch::BranchMismatch
            ))
        );

        let other_manifest = BindingSelection {
            segment_manifest_id: Hash32([0x52; 32]),
            ..selection(&selected, &version)
        };
        assert_eq!(
            checkpoint_binding_for_selection(&claim, &other_manifest),
            Err(CompositionError::Binding(
                CheckpointBindingMismatch::SegmentManifestMismatch
            ))
        );

        let bumped = MaterializerVersion::new("scene-graph", 4);
        assert_eq!(
            checkpoint_binding_for_selection(&claim, &selection(&selected, &bumped)),
            Err(CompositionError::MaterializerVersionMismatch {
                claimed: 3,
                current: 4,
            })
        );

        let foreign_materializer = BindingSelection {
            materializer_id: Identifier32([0x12; 32]),
            ..selection(&selected, &version)
        };
        assert_eq!(
            checkpoint_binding_for_selection(&claim, &foreign_materializer),
            Err(CompositionError::MaterializerIdentifierMismatch)
        );
    }

    #[test]
    fn the_proof_view_exposes_the_signed_claims_own_commitment() {
        let selected = frontier();
        let signed = sign_checkpoint_claim(&signing_key(), &claim(&selected)).expect("sign");
        let view = SignedCheckpointProofView::new(&signed, &selected, None);

        assert_eq!(view.verse_id(), VERSE);
        assert_eq!(view.branch_id(), BRANCH);
        assert_eq!(view.segment_manifest_id(), SEGMENT_MANIFEST);
        assert_eq!(view.replay_base(), None);
        assert_eq!(view.frontier_commitment(), selected.commitment());
        assert_eq!(view.selected_frontier(), &selected);
        assert_eq!(view.verify_manager_signature(), Ok(()));
        assert_eq!(view.claim(), &signed);

        let mut forged = signed.clone();
        forged.signature[0] ^= 0xff;
        let forged_view = SignedCheckpointProofView::new(&forged, &selected, None);
        assert!(forged_view.verify_manager_signature().is_err());

        // The view reports the SIGNED commitment, never a recomputation of whatever frontier it
        // was handed: that is what lets the SPEC-6 proof detect the two disagreeing.
        let other = SortedFrontier::try_new([Hash32([0x99; 32])]).expect("frontier");
        let mismatched = SignedCheckpointProofView::new(&signed, &other, None);
        assert_ne!(
            mismatched.frontier_commitment(),
            mismatched.selected_frontier().commitment()
        );
    }
}
