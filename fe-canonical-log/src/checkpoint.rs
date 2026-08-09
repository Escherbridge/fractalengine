//! Signed, replay-verifiable checkpoint claims (SPEC-5 §3); see `src/branch/AGENTS.md`.

use ed25519_dalek::SigningKey;
use thiserror::Error;

use crate::cbor::{
    decode_canonical as decode_cbor, encode_canonical_checked as encode_cbor_checked, CborError,
    CborValue,
};
use crate::envelope::{
    Author, CapabilityRef, EnvelopeError, Hash32, Hlc, Identifier32, Scope, SIGNATURE_LENGTH,
};
use crate::frontier::SortedFrontier;
use crate::retention::tombstone::{
    assert_no_resurrection, TombstoneError, TombstoneRetentionRecord,
};
use crate::signing::{sign_domain, verify_author_binding, verify_domain, SigningError};

/// NUL-terminated ASCII domain separator for checkpoint-claim signatures (§3.1 rule 4).
pub const CHECKPOINT_SIGNATURE_DOMAIN: &[u8] = b"fe-checkpoint-v1\0";

/// The only checkpoint claim version v1 admits (§3.1 rule 2).
pub const CHECKPOINT_VERSION: u64 = 1;

/// Field number of `checkpoint_version`.
pub const CHECKPOINT_VERSION_KEY: u64 = 0;
/// Field number of `verse_id`.
pub const VERSE_ID_KEY: u64 = 1;
/// Field number of `branch_id`.
pub const BRANCH_ID_KEY: u64 = 2;
/// Field number of `frontier_commitment`.
pub const FRONTIER_COMMITMENT_KEY: u64 = 3;
/// Field number of `segment_manifest_id`.
pub const SEGMENT_MANIFEST_ID_KEY: u64 = 4;
/// Field number of `materializer_id`.
pub const MATERIALIZER_ID_KEY: u64 = 5;
/// Field number of `materializer_version`.
pub const MATERIALIZER_VERSION_KEY: u64 = 6;
/// Field number of `projection_root_hash`.
pub const PROJECTION_ROOT_HASH_KEY: u64 = 7;
/// Field number of `authorization_view_root`.
pub const AUTHORIZATION_VIEW_ROOT_KEY: u64 = 8;
/// Field number of `snapshot_manifest_id`.
pub const SNAPSHOT_MANIFEST_ID_KEY: u64 = 9;
/// Field number of `signer`.
pub const SIGNER_KEY: u64 = 10;
/// Field number of `capability`.
pub const CAPABILITY_KEY: u64 = 11;
/// Field number of `issued_hlc`.
pub const ISSUED_HLC_KEY: u64 = 12;
/// Field number of the Ed25519 signature in the complete claim.
pub const SIGNATURE_KEY: u64 = 13;

const UNSIGNED_FIELD_COUNT: u64 = 13;
const COMPLETE_FIELD_COUNT: u64 = 14;

/// Every reason a checkpoint claim fails to encode, decode, or authenticate.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CheckpointError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A nested SPEC-1 sub-map did not parse.
    #[error("checkpoint claim: {0}")]
    Envelope(#[from] EnvelopeError),
    /// The DID binding or the signature failed.
    #[error("checkpoint claim: {0}")]
    Signing(#[from] SigningError),
    /// The claim was not a CBOR map.
    #[error("checkpoint claim is not a CBOR map")]
    ExpectedMap,
    /// The claim did not hold exactly its listed integer keys, once each, and nothing else.
    #[error(
        "checkpoint claim must hold exactly the integer keys 0..{expected_key_count} once each"
    )]
    UnexpectedMapKeys {
        /// Number of integer keys the grammar lists.
        expected_key_count: u64,
    },
    /// A listed key was absent.
    #[error("checkpoint claim key {key} is missing")]
    MissingKey {
        /// The absent key.
        key: u64,
    },
    /// A key held something other than an unsigned integer.
    #[error("checkpoint claim key {key} is not an unsigned integer")]
    ExpectedUnsignedInteger {
        /// The offending key.
        key: u64,
    },
    /// A key held something other than a byte string.
    #[error("checkpoint claim key {key} is not a byte string")]
    ExpectedByteString {
        /// The offending key.
        key: u64,
    },
    /// A nullable identifier slot held neither `null` nor a byte string.
    #[error("checkpoint claim key {key} must be null or a byte string")]
    ExpectedNullOrByteString {
        /// The offending key.
        key: u64,
    },
    /// A byte string had the wrong fixed length.
    #[error("checkpoint claim key {key} holds {actual} byte(s), expected {expected}")]
    WrongByteLength {
        /// The offending key.
        key: u64,
        /// Length the grammar requires.
        expected: usize,
        /// Length actually present.
        actual: usize,
    },
    /// `checkpoint_version` was not 1.
    #[error("checkpoint_version {version} is not 1")]
    UnsupportedCheckpointVersion {
        /// The version actually present.
        version: u64,
    },
    /// The received bytes were not the canonical encoding of the claim they decode to.
    #[error("received bytes are not the canonical encoding of the claim they decode to")]
    NonCanonicalEncoding,
}

/// The §3.1 rule 2 unsigned checkpoint claim: every binding, and no plaintext snapshot bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckpointClaimV1 {
    /// MUST be [`CHECKPOINT_VERSION`].
    pub checkpoint_version: u64,
    /// The verse this checkpoint belongs to.
    pub verse_id: Identifier32,
    /// The registry branch whose causal history was projected.
    pub branch_id: Identifier32,
    /// [`SortedFrontier::commitment`] over the exact selected frontier.
    pub frontier_commitment: Hash32,
    /// The immutable segment manifest whose closure is asserted to cover the selection.
    pub segment_manifest_id: Hash32,
    /// The deterministic projection rule set.
    pub materializer_id: Identifier32,
    /// The version of those rules; a change is a distinct projection identity.
    pub materializer_version: u64,
    /// BLAKE3 commitment to the canonical projection those rules produced.
    pub projection_root_hash: Hash32,
    /// Commitment to the epoch-bump and authority facts used for replay.
    pub authorization_view_root: Hash32,
    /// Manifest of scope-affine encrypted snapshot components, or `None`.
    pub snapshot_manifest_id: Option<Hash32>,
    /// Canonical signing principal.
    pub signer: Author,
    /// SPEC-1 capability reference the signer relied on.
    pub capability: CapabilityRef,
    /// Explicit wire HLC evidence; never the packed `fe-database` integer.
    pub issued_hlc: Hlc,
}

impl CheckpointClaimV1 {
    /// Enforces §3.1 rule 2: the version is exactly 1.
    pub fn validate(&self) -> Result<(), CheckpointError> {
        if self.checkpoint_version != CHECKPOINT_VERSION {
            return Err(CheckpointError::UnsupportedCheckpointVersion {
                version: self.checkpoint_version,
            });
        }
        Ok(())
    }

    /// True when this claim binds exactly the given selection.
    pub fn binds_frontier(&self, selected: &SortedFrontier) -> bool {
        self.frontier_commitment == selected.commitment()
    }

    /// Encodes the thirteen-key §3.1 rule 3 map.
    pub fn to_cbor(&self) -> Result<CborValue, CheckpointError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(CHECKPOINT_VERSION_KEY),
                CborValue::Uint(self.checkpoint_version),
            ),
            (
                CborValue::Uint(VERSE_ID_KEY),
                CborValue::Bytes(self.verse_id.0.to_vec()),
            ),
            (
                CborValue::Uint(BRANCH_ID_KEY),
                CborValue::Bytes(self.branch_id.0.to_vec()),
            ),
            (
                CborValue::Uint(FRONTIER_COMMITMENT_KEY),
                CborValue::Bytes(self.frontier_commitment.0.to_vec()),
            ),
            (
                CborValue::Uint(SEGMENT_MANIFEST_ID_KEY),
                CborValue::Bytes(self.segment_manifest_id.0.to_vec()),
            ),
            (
                CborValue::Uint(MATERIALIZER_ID_KEY),
                CborValue::Bytes(self.materializer_id.0.to_vec()),
            ),
            (
                CborValue::Uint(MATERIALIZER_VERSION_KEY),
                CborValue::Uint(self.materializer_version),
            ),
            (
                CborValue::Uint(PROJECTION_ROOT_HASH_KEY),
                CborValue::Bytes(self.projection_root_hash.0.to_vec()),
            ),
            (
                CborValue::Uint(AUTHORIZATION_VIEW_ROOT_KEY),
                CborValue::Bytes(self.authorization_view_root.0.to_vec()),
            ),
            (
                CborValue::Uint(SNAPSHOT_MANIFEST_ID_KEY),
                match self.snapshot_manifest_id {
                    Some(manifest) => CborValue::Bytes(manifest.0.to_vec()),
                    None => CborValue::Null,
                },
            ),
            (CborValue::Uint(SIGNER_KEY), self.signer.to_cbor()),
            (CborValue::Uint(CAPABILITY_KEY), self.capability.to_cbor()),
            (CborValue::Uint(ISSUED_HLC_KEY), self.issued_hlc.to_cbor()),
        ]))
    }

    /// Decodes the thirteen-key §3.1 rule 3 map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CheckpointError> {
        require_field_keys(value, UNSIGNED_FIELD_COUNT)?;
        parse_claim_fields(value)
    }

    /// Encodes to the canonical bytes the §3.1 rule 4 signature preimage covers.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CheckpointError> {
        Ok(encode_cbor_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CheckpointError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }
}

/// A checkpoint claim plus its `fe-checkpoint-v1` Ed25519 signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedCheckpointClaim {
    /// The signed claim.
    pub claim: CheckpointClaimV1,
    /// Ed25519 signature over `ASCII("fe-checkpoint-v1") || 0x00 || unsigned_claim`.
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl SignedCheckpointClaim {
    /// Encodes the fourteen-key complete claim; `checkpoint_id` is BLAKE3 over these bytes.
    pub fn to_cbor(&self) -> Result<CborValue, CheckpointError> {
        let mut entries = match self.claim.to_cbor()? {
            CborValue::Map(entries) => entries,
            _ => unreachable!("CheckpointClaimV1::to_cbor always yields a map"),
        };
        entries.push((
            CborValue::Uint(SIGNATURE_KEY),
            CborValue::Bytes(self.signature.to_vec()),
        ));
        Ok(CborValue::Map(entries))
    }

    /// Decodes the fourteen-key complete claim.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CheckpointError> {
        require_field_keys(value, COMPLETE_FIELD_COUNT)?;
        Ok(Self {
            claim: parse_claim_fields(value)?,
            signature: bytes_at::<SIGNATURE_LENGTH>(value, SIGNATURE_KEY)?,
        })
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CheckpointError> {
        Ok(encode_cbor_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CheckpointError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// Verifies the §3.1 rule 4 DID binding and signature; it proves authorship, never trust.
    pub fn verify_signature(&self) -> Result<(), CheckpointError> {
        verify_author_binding(&self.claim.signer.did, &self.claim.signer.public_key)?;
        verify_domain(
            &self.claim.signer.public_key,
            CHECKPOINT_SIGNATURE_DOMAIN,
            &self.claim.encode_canonical()?,
            &self.signature,
        )?;
        Ok(())
    }

    /// `checkpoint_id = BLAKE3(complete_claim)`, signature included.
    pub fn checkpoint_id(&self) -> Result<Hash32, CheckpointError> {
        Ok(Hash32::of(&self.encode_canonical()?))
    }
}

/// Signs a claim under the `fe-checkpoint-v1` domain.
pub fn sign_checkpoint_claim(
    signing_key: &SigningKey,
    claim: &CheckpointClaimV1,
) -> Result<SignedCheckpointClaim, CheckpointError> {
    let claim_bytes = claim.encode_canonical()?;
    Ok(SignedCheckpointClaim {
        claim: claim.clone(),
        signature: sign_domain(signing_key, CHECKPOINT_SIGNATURE_DOMAIN, &claim_bytes),
    })
}

/// The §3.2 rule 1 byte-level ingress: canonical re-encode, DID binding, signature, and ID.
pub fn decode_and_admit_checkpoint(
    bytes: &[u8],
) -> Result<(SignedCheckpointClaim, Hash32), CheckpointError> {
    let signed = SignedCheckpointClaim::decode_canonical(bytes)?;
    if signed.encode_canonical()? != bytes {
        return Err(CheckpointError::NonCanonicalEncoding);
    }
    signed.verify_signature()?;
    Ok((signed, Hash32::of(bytes)))
}

/// The authorization facts §3.1 rule 5 and §3.2 rule 1 require, injected by the caller.
pub trait ManagerPlusAuthorizationView {
    /// True when the signer is Manager+ in the authorization view reconstructed for the
    /// selected history, under the epoch the capability reference relies on.
    fn is_manager_plus_for_checkpoint(
        &self,
        verse_id: Identifier32,
        signer: &Author,
        capability: &CapabilityRef,
    ) -> bool;

    /// True when the signer presents an unexpired `append/checkpoint` capability for exactly
    /// this verse scope; `seed` or `materialize` authority alone is not enough.
    fn capability_permits_append_checkpoint(
        &self,
        verse_scope: &Scope,
        signer: &Author,
        capability: &CapabilityRef,
    ) -> bool;

    /// The authorization-view root this consumer derives, or `None` when it cannot derive one.
    fn authorization_view_root(
        &self,
        verse_id: Identifier32,
        branch_id: Identifier32,
        frontier_commitment: Hash32,
    ) -> Option<Hash32>;
}

/// The canonical input identity a replay must reproduce (SPEC-4 §7 rule 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplayRequest {
    /// Verse of the selection.
    pub verse_id: Identifier32,
    /// Branch of the selection.
    pub branch_id: Identifier32,
    /// D-CL19 commitment of the selected frontier.
    pub frontier_commitment: Hash32,
    /// Segment manifest whose closure covers the selection.
    pub segment_manifest_id: Hash32,
    /// Deterministic projection rule set.
    pub materializer_id: Identifier32,
    /// Version of those rules.
    pub materializer_version: u64,
}

/// The D-CL4 replay seam: derive the projection root independently of the claim.
pub trait ReplayVerifier {
    /// The projection root this consumer derives, or `None` when it cannot replay at all.
    fn replay_projection_root(&self, request: &ReplayRequest) -> Option<Hash32>;
}

/// The payload scopes a caller may decrypt, expressed as SPEC-1 scopes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessibleScopes(Vec<Scope>);

impl AccessibleScopes {
    /// Builds the accessible-scope set.
    pub fn new(scopes: impl IntoIterator<Item = Scope>) -> Self {
        Self(scopes.into_iter().collect())
    }

    /// True when some granted scope contains `required`, by SPEC-3 §1.7 containment.
    pub fn covers(&self, required: &Scope) -> bool {
        self.0.iter().any(|granted| granted.contains(required))
    }

    /// The first required scope this caller cannot decrypt.
    pub fn first_inaccessible(&self, required: &[Scope]) -> Option<Scope> {
        required.iter().find(|scope| !self.covers(scope)).copied()
    }
}

/// What the consumer independently knows about the selection it wants to accelerate.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointConsumerView<'a> {
    /// The consumer's own selected frontier, derived from immutable operation IDs.
    pub selected_frontier: &'a SortedFrontier,
    /// The consumer's own segment manifest for that selection.
    pub segment_manifest_id: Hash32,
    /// Every payload scope the selection represents.
    pub required_payload_scopes: &'a [Scope],
    /// The scopes this consumer may decrypt.
    pub accessible_scopes: &'a AccessibleScopes,
}

/// Why a checkpoint is rejected outright; §3.2 rule 5 forbids repairing any of these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointRejection {
    /// The bytes, version, DID binding, or signature failed.
    Malformed(CheckpointError),
    /// The signer is not Manager+ in the reconstructed authorization view.
    NotManagerPlus,
    /// The signer's capability does not carry unexpired `append/checkpoint` for this verse.
    CapabilityDoesNotPermitCheckpoint,
    /// The claim commits to a frontier other than the consumer's selection.
    FrontierCommitmentMismatch {
        /// Commitment the claim carries.
        claimed: Hash32,
        /// Commitment of the consumer's selection.
        selected: Hash32,
    },
    /// The claim names a segment manifest other than the consumer's.
    SegmentManifestMismatch {
        /// Manifest the claim carries.
        claimed: Hash32,
        /// Manifest the consumer derived.
        selected: Hash32,
    },
    /// Replay derived a different authorization-view root.
    AuthorizationViewRootMismatch {
        /// Root the claim carries.
        claimed: Hash32,
        /// Root the consumer derived.
        replayed: Hash32,
    },
    /// Replay derived a different projection root.
    ProjectionRootMismatch {
        /// Root the claim carries.
        claimed: Hash32,
        /// Root the consumer derived.
        replayed: Hash32,
    },
}

/// Why a well-formed, authorized claim still is not independently replay-verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UntrustedAcceleratorReason {
    /// §3.2 rule 4: the consumer cannot decrypt a scope the selection represents.
    InaccessibleScope {
        /// The first scope outside this consumer's authority.
        scope: Scope,
    },
    /// The consumer cannot reconstruct the authorization view for this selection.
    AuthorizationViewUnavailable,
    /// The consumer cannot replay the projection for this selection.
    ReplayUnavailable,
}

/// The §3.2 outcome of consuming a checkpoint claim.
///
/// Only [`verify_checkpoint_claim`] may report `Verified`; see `src/AGENTS.md` §checkpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CheckpointVerification {
    /// Signature, authority, bindings, and independent replay all agree.
    #[non_exhaustive]
    Verified {
        /// BLAKE3 of the complete claim bytes.
        checkpoint_id: Hash32,
    },
    /// Usable as an accelerator only; it MUST NOT be presented as replay-verified.
    UntrustedAccelerator {
        /// BLAKE3 of the complete claim bytes.
        checkpoint_id: Hash32,
        /// Why independent verification was not possible.
        reason: UntrustedAcceleratorReason,
    },
    /// Refused; the projection rebuilds from verified history instead.
    Rejected(CheckpointRejection),
}

impl CheckpointVerification {
    /// The checkpoint identifier when independent replay verification succeeded.
    pub fn verified_checkpoint_id(&self) -> Option<Hash32> {
        match self {
            Self::Verified { checkpoint_id } => Some(*checkpoint_id),
            _ => None,
        }
    }
}

/// Verifies a received checkpoint claim against this consumer's own view (§3.2).
///
/// A Manager+ signature alone never yields [`CheckpointVerification::Verified`]: the bindings
/// are compared against the consumer's selection, and the projection root must be derived
/// independently by [`ReplayVerifier`].
pub fn verify_checkpoint_claim(
    received_bytes: &[u8],
    consumer: &CheckpointConsumerView<'_>,
    authorization: &dyn ManagerPlusAuthorizationView,
    replay: &dyn ReplayVerifier,
) -> CheckpointVerification {
    let (signed, checkpoint_id) = match decode_and_admit_checkpoint(received_bytes) {
        Ok(admitted) => admitted,
        Err(error) => {
            return CheckpointVerification::Rejected(CheckpointRejection::Malformed(error))
        }
    };
    let claim = &signed.claim;

    if !authorization.is_manager_plus_for_checkpoint(
        claim.verse_id,
        &claim.signer,
        &claim.capability,
    ) {
        return CheckpointVerification::Rejected(CheckpointRejection::NotManagerPlus);
    }
    let verse_scope = Scope::verse_wide(claim.verse_id);
    if !authorization.capability_permits_append_checkpoint(
        &verse_scope,
        &claim.signer,
        &claim.capability,
    ) {
        return CheckpointVerification::Rejected(
            CheckpointRejection::CapabilityDoesNotPermitCheckpoint,
        );
    }

    let selected_commitment = consumer.selected_frontier.commitment();
    if claim.frontier_commitment != selected_commitment {
        return CheckpointVerification::Rejected(CheckpointRejection::FrontierCommitmentMismatch {
            claimed: claim.frontier_commitment,
            selected: selected_commitment,
        });
    }
    if claim.segment_manifest_id != consumer.segment_manifest_id {
        return CheckpointVerification::Rejected(CheckpointRejection::SegmentManifestMismatch {
            claimed: claim.segment_manifest_id,
            selected: consumer.segment_manifest_id,
        });
    }

    if let Some(scope) = consumer
        .accessible_scopes
        .first_inaccessible(consumer.required_payload_scopes)
    {
        return CheckpointVerification::UntrustedAccelerator {
            checkpoint_id,
            reason: UntrustedAcceleratorReason::InaccessibleScope { scope },
        };
    }

    match authorization.authorization_view_root(
        claim.verse_id,
        claim.branch_id,
        claim.frontier_commitment,
    ) {
        None => {
            return CheckpointVerification::UntrustedAccelerator {
                checkpoint_id,
                reason: UntrustedAcceleratorReason::AuthorizationViewUnavailable,
            }
        }
        Some(replayed) if replayed != claim.authorization_view_root => {
            return CheckpointVerification::Rejected(
                CheckpointRejection::AuthorizationViewRootMismatch {
                    claimed: claim.authorization_view_root,
                    replayed,
                },
            )
        }
        Some(_) => {}
    }

    let request = ReplayRequest {
        verse_id: claim.verse_id,
        branch_id: claim.branch_id,
        frontier_commitment: claim.frontier_commitment,
        segment_manifest_id: claim.segment_manifest_id,
        materializer_id: claim.materializer_id,
        materializer_version: claim.materializer_version,
    };
    match replay.replay_projection_root(&request) {
        None => CheckpointVerification::UntrustedAccelerator {
            checkpoint_id,
            reason: UntrustedAcceleratorReason::ReplayUnavailable,
        },
        Some(replayed) if replayed != claim.projection_root_hash => {
            CheckpointVerification::Rejected(CheckpointRejection::ProjectionRootMismatch {
                claimed: claim.projection_root_hash,
                replayed,
            })
        }
        Some(_) => CheckpointVerification::Verified { checkpoint_id },
    }
}

/// The one retention obligation §5.1 rule 5 places on the caller, not on this crate.
///
/// Only the caller knows which bootstrap paths it still stores, so this stays its assertion.
/// Suppression evidence used to be a second bool here and is not any more: `compaction_decision`
/// takes the tombstone records themselves and checks them, because that evidence IS inspectable
/// from `retention::tombstone::TombstoneRetentionRecord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootstrapCoverage {
    /// Every head of the checkpointed frontier still has a retained bootstrap path.
    pub every_head_has_retained_bootstrap_path: bool,
}

/// Why local compaction of a checkpointed selection is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionRefusal {
    /// The checkpoint was not independently replay-verified.
    CheckpointNotVerified,
    /// A head of the frontier has no retained bootstrap path.
    IncompleteBootstrapPaths,
    /// A retained bootstrap path does not prove the suppression survives replay through it.
    UnprovedSuppressionEvidence {
        /// The offending bootstrap path.
        path_id: Hash32,
    },
}

/// Whether a checkpointed selection may be compacted locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompactionDecision {
    /// Local reclamation is permitted; it authorizes no deletion on another peer.
    Permitted {
        /// The verified checkpoint the decision rests on.
        checkpoint_id: Hash32,
    },
    /// Local reclamation is refused.
    Refused(CompactionRefusal),
}

/// The single compaction gate: §2.2 rule 2 multi-head compaction and §5.4 rule 2 tombstone
/// compaction both pass through here.
///
/// `verification` is the real [`verify_checkpoint_claim`] result — signature, Manager+ authority,
/// frontier and manifest bindings, and independent replay — so a caller cannot assert a verdict.
/// `suppressions` is a REQUIRED parameter holding every tombstone whose history the compaction
/// would reclaim, and each one's retained bootstrap paths are checked here through
/// `retention::tombstone::assert_no_resurrection`. Pass an empty slice only when the compaction
/// reclaims no tombstone at all.
pub fn compaction_decision(
    verification: &CheckpointVerification,
    coverage: BootstrapCoverage,
    suppressions: &[TombstoneRetentionRecord],
) -> CompactionDecision {
    let Some(checkpoint_id) = verification.verified_checkpoint_id() else {
        return CompactionDecision::Refused(CompactionRefusal::CheckpointNotVerified);
    };
    if !coverage.every_head_has_retained_bootstrap_path {
        return CompactionDecision::Refused(CompactionRefusal::IncompleteBootstrapPaths);
    }
    for record in suppressions {
        if let Err(TombstoneError::MissingSuppressionProof { path_id }) =
            assert_no_resurrection(record)
        {
            return CompactionDecision::Refused(CompactionRefusal::UnprovedSuppressionEvidence {
                path_id,
            });
        }
    }
    CompactionDecision::Permitted { checkpoint_id }
}

fn parse_claim_fields(value: &CborValue) -> Result<CheckpointClaimV1, CheckpointError> {
    let checkpoint_version = unsigned_at(value, CHECKPOINT_VERSION_KEY)?;
    if checkpoint_version != CHECKPOINT_VERSION {
        return Err(CheckpointError::UnsupportedCheckpointVersion {
            version: checkpoint_version,
        });
    }
    Ok(CheckpointClaimV1 {
        checkpoint_version,
        verse_id: Identifier32(bytes_at::<32>(value, VERSE_ID_KEY)?),
        branch_id: Identifier32(bytes_at::<32>(value, BRANCH_ID_KEY)?),
        frontier_commitment: Hash32(bytes_at::<32>(value, FRONTIER_COMMITMENT_KEY)?),
        segment_manifest_id: Hash32(bytes_at::<32>(value, SEGMENT_MANIFEST_ID_KEY)?),
        materializer_id: Identifier32(bytes_at::<32>(value, MATERIALIZER_ID_KEY)?),
        materializer_version: unsigned_at(value, MATERIALIZER_VERSION_KEY)?,
        projection_root_hash: Hash32(bytes_at::<32>(value, PROJECTION_ROOT_HASH_KEY)?),
        authorization_view_root: Hash32(bytes_at::<32>(value, AUTHORIZATION_VIEW_ROOT_KEY)?),
        snapshot_manifest_id: optional_hash_at(value, SNAPSHOT_MANIFEST_ID_KEY)?,
        signer: Author::from_cbor(entry(value, SIGNER_KEY)?)?,
        capability: CapabilityRef::from_cbor(entry(value, CAPABILITY_KEY)?)?,
        issued_hlc: Hlc::from_cbor(entry(value, ISSUED_HLC_KEY)?)?,
    })
}

fn require_field_keys(value: &CborValue, expected_key_count: u64) -> Result<(), CheckpointError> {
    let entries = value.as_map().ok_or(CheckpointError::ExpectedMap)?;
    if entries.len() as u64 != expected_key_count {
        return Err(CheckpointError::UnexpectedMapKeys { expected_key_count });
    }
    for key in 0..expected_key_count {
        if value.get_uint_key(key).is_none() {
            return Err(CheckpointError::MissingKey { key });
        }
    }
    Ok(())
}

fn entry(value: &CborValue, key: u64) -> Result<&CborValue, CheckpointError> {
    value
        .get_uint_key(key)
        .ok_or(CheckpointError::MissingKey { key })
}

fn unsigned_at(value: &CborValue, key: u64) -> Result<u64, CheckpointError> {
    entry(value, key)?
        .as_uint()
        .ok_or(CheckpointError::ExpectedUnsignedInteger { key })
}

fn bytes_at<const LENGTH: usize>(
    value: &CborValue,
    key: u64,
) -> Result<[u8; LENGTH], CheckpointError> {
    let raw = entry(value, key)?
        .as_bytes()
        .ok_or(CheckpointError::ExpectedByteString { key })?;
    raw.try_into()
        .map_err(|_| CheckpointError::WrongByteLength {
            key,
            expected: LENGTH,
            actual: raw.len(),
        })
}

fn optional_hash_at(value: &CborValue, key: u64) -> Result<Option<Hash32>, CheckpointError> {
    let slot = entry(value, key)?;
    if slot.is_null() {
        return Ok(None);
    }
    let raw = slot
        .as_bytes()
        .ok_or(CheckpointError::ExpectedNullOrByteString { key })?;
    Ok(Some(Hash32(raw.try_into().map_err(|_| {
        CheckpointError::WrongByteLength {
            key,
            expected: 32,
            actual: raw.len(),
        }
    })?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::SIGNATURE_DOMAIN;
    use std::collections::BTreeSet;

    const VERSE: Identifier32 = Identifier32([0x0e; 32]);
    const BRANCH: Identifier32 = Identifier32([0x0a; 32]);
    const MATERIALIZER: Identifier32 = Identifier32([0x11; 32]);
    const SEGMENT_MANIFEST: Hash32 = Hash32([0x51; 32]);
    const AUTHORIZATION_ROOT: Hash32 = Hash32([0x5a; 32]);

    const MANAGER_CHAIN: Hash32 = Hash32([0x60; 32]);
    const EDITOR_CHAIN: Hash32 = Hash32([0x61; 32]);
    const RELAY_SEED_CHAIN: Hash32 = Hash32([0x62; 32]);
    const MATERIALIZER_ONLY_CHAIN: Hash32 = Hash32([0x63; 32]);
    const EXPIRED_MANAGER_CHAIN: Hash32 = Hash32([0x64; 32]);
    const CURRENT_EPOCH: u64 = 9;

    /// `signing_key.seed_hex` from `docs/spec/canonical-log/operation-envelope-v1.json`.
    const FIXTURE_SEED_HEX: &str =
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn signing_key() -> SigningKey {
        let seed: [u8; 32] = hex::decode(FIXTURE_SEED_HEX)
            .expect("fixture hex")
            .try_into()
            .expect("32 bytes");
        SigningKey::from_bytes(&seed)
    }

    fn op_id(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn petal_scope(filler: u8) -> Scope {
        Scope::new(VERSE, Some(Identifier32([filler; 32])), None).expect("petal scope")
    }

    /// One tombstone whose single retained bootstrap path either proves the suppression or not.
    fn tombstone_record(carries_suppression_proof: bool) -> TombstoneRetentionRecord {
        TombstoneRetentionRecord {
            tombstone_op_id: op_id(0x3a),
            scope: Scope::verse_wide(VERSE),
            suppressed_object_id: Identifier32([0x3c; 32]),
            retained_bootstrap_paths: vec![crate::retention::tombstone::BootstrapPath {
                path_id: Hash32(
                    [if carries_suppression_proof {
                        0x3d
                    } else {
                        0x3b
                    }; 32],
                ),
                carries_suppression_proof,
            }],
        }
    }

    /// Every consumer derives this root from the canonical input identity, so mutating any
    /// bound value makes the independent replay disagree with the claim.
    fn deterministic_projection_root(request: &ReplayRequest) -> Hash32 {
        let mut preimage = Vec::new();
        preimage.extend_from_slice(request.verse_id.as_bytes());
        preimage.extend_from_slice(request.branch_id.as_bytes());
        preimage.extend_from_slice(request.frontier_commitment.as_bytes());
        preimage.extend_from_slice(request.segment_manifest_id.as_bytes());
        preimage.extend_from_slice(request.materializer_id.as_bytes());
        preimage.extend_from_slice(&request.materializer_version.to_be_bytes());
        Hash32::of(&preimage)
    }

    struct FakeAuthorizationView {
        manager_plus_chains: BTreeSet<Hash32>,
        checkpoint_capability_chains: BTreeSet<Hash32>,
        authorization_view_root: Option<Hash32>,
    }

    impl FakeAuthorizationView {
        fn permissive() -> Self {
            Self {
                manager_plus_chains: BTreeSet::from([
                    MANAGER_CHAIN,
                    MATERIALIZER_ONLY_CHAIN,
                    EXPIRED_MANAGER_CHAIN,
                ]),
                checkpoint_capability_chains: BTreeSet::from([MANAGER_CHAIN]),
                authorization_view_root: Some(AUTHORIZATION_ROOT),
            }
        }
    }

    impl ManagerPlusAuthorizationView for FakeAuthorizationView {
        fn is_manager_plus_for_checkpoint(
            &self,
            verse_id: Identifier32,
            _signer: &Author,
            capability: &CapabilityRef,
        ) -> bool {
            verse_id == VERSE
                && capability.scope_epoch == CURRENT_EPOCH
                && self.manager_plus_chains.contains(&capability.chain_id)
        }

        fn capability_permits_append_checkpoint(
            &self,
            verse_scope: &Scope,
            _signer: &Author,
            capability: &CapabilityRef,
        ) -> bool {
            verse_scope.verse_id() == VERSE
                && verse_scope.petal_id().is_none()
                && self
                    .checkpoint_capability_chains
                    .contains(&capability.chain_id)
        }

        fn authorization_view_root(
            &self,
            _verse_id: Identifier32,
            _branch_id: Identifier32,
            _frontier_commitment: Hash32,
        ) -> Option<Hash32> {
            self.authorization_view_root
        }
    }

    struct FakeReplay {
        available: bool,
    }

    impl ReplayVerifier for FakeReplay {
        fn replay_projection_root(&self, request: &ReplayRequest) -> Option<Hash32> {
            self.available
                .then(|| deterministic_projection_root(request))
        }
    }

    fn claim_for(selected: &SortedFrontier) -> CheckpointClaimV1 {
        let frontier_commitment = selected.commitment();
        let projection_root_hash = deterministic_projection_root(&ReplayRequest {
            verse_id: VERSE,
            branch_id: BRANCH,
            frontier_commitment,
            segment_manifest_id: SEGMENT_MANIFEST,
            materializer_id: MATERIALIZER,
            materializer_version: 7,
        });
        CheckpointClaimV1 {
            checkpoint_version: CHECKPOINT_VERSION,
            verse_id: VERSE,
            branch_id: BRANCH,
            frontier_commitment,
            segment_manifest_id: SEGMENT_MANIFEST,
            materializer_id: MATERIALIZER,
            materializer_version: 7,
            projection_root_hash,
            authorization_view_root: AUTHORIZATION_ROOT,
            snapshot_manifest_id: Some(Hash32([0x71; 32])),
            signer: Author::from_public_key(signing_key().verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: MANAGER_CHAIN,
                scope_epoch: CURRENT_EPOCH,
            },
            issued_hlc: Hlc::new(1_700_000_000_000, 3),
        }
    }

    fn signed_bytes(claim: &CheckpointClaimV1) -> Vec<u8> {
        sign_checkpoint_claim(&signing_key(), claim)
            .expect("sign")
            .encode_canonical()
            .expect("encode")
    }

    fn consumer<'a>(
        selected: &'a SortedFrontier,
        required: &'a [Scope],
        accessible: &'a AccessibleScopes,
    ) -> CheckpointConsumerView<'a> {
        CheckpointConsumerView {
            selected_frontier: selected,
            segment_manifest_id: SEGMENT_MANIFEST,
            required_payload_scopes: required,
            accessible_scopes: accessible,
        }
    }

    fn verse_wide_access() -> AccessibleScopes {
        AccessibleScopes::new([Scope::verse_wide(VERSE)])
    }

    #[test]
    fn checkpoint_claim_is_canonical_and_domain_separated() {
        let selected = SortedFrontier::try_new([op_id(0x20), op_id(0x10)]).expect("frontier");
        let claim = claim_for(&selected);
        let signed = sign_checkpoint_claim(&signing_key(), &claim).expect("sign");
        let bytes = signed.encode_canonical().expect("encode");

        let decoded = SignedCheckpointClaim::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, signed);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
        assert_eq!(
            signed.to_cbor().expect("cbor").as_map().map(<[_]>::len),
            Some(14)
        );

        assert_eq!(CHECKPOINT_SIGNATURE_DOMAIN, b"fe-checkpoint-v1\0");
        assert_eq!(signed.verify_signature(), Ok(()));
        let (admitted, checkpoint_id) = decode_and_admit_checkpoint(&bytes).expect("admit");
        assert_eq!(admitted, signed);
        assert_eq!(checkpoint_id, Hash32::of(&bytes));
        assert_eq!(signed.checkpoint_id().expect("id"), checkpoint_id);
        assert_ne!(
            checkpoint_id,
            Hash32::of(&claim.encode_canonical().expect("encode")),
            "checkpoint_id covers the signature"
        );

        let claim_bytes = claim.encode_canonical().expect("encode");
        assert_eq!(
            verify_domain(
                &claim.signer.public_key,
                SIGNATURE_DOMAIN,
                &claim_bytes,
                &signed.signature,
            ),
            Err(SigningError::SignatureVerificationFailed),
            "a checkpoint signature must not verify under the operation-envelope domain"
        );

        let mut trailing = bytes.clone();
        trailing.push(0x00);
        assert!(matches!(
            decode_and_admit_checkpoint(&trailing),
            Err(CheckpointError::Cbor(_))
        ));

        let mut wrong_version = claim.clone();
        wrong_version.checkpoint_version = 2;
        assert_eq!(
            wrong_version.encode_canonical(),
            Err(CheckpointError::UnsupportedCheckpointVersion { version: 2 })
        );

        let mut no_snapshot = claim;
        no_snapshot.snapshot_manifest_id = None;
        let no_snapshot_bytes = signed_bytes(&no_snapshot);
        assert_eq!(
            decode_and_admit_checkpoint(&no_snapshot_bytes)
                .expect("admit")
                .0
                .claim
                .snapshot_manifest_id,
            None
        );
    }

    #[test]
    fn checkpoint_binds_frontier_manifest_materializer_and_authorization() {
        let selected = SortedFrontier::try_new([op_id(0x20), op_id(0x10)]).expect("frontier");
        let accessible = verse_wide_access();
        let required = [Scope::verse_wide(VERSE)];
        let view = consumer(&selected, &required, &accessible);
        let authorization = FakeAuthorizationView::permissive();
        let replay = FakeReplay { available: true };
        let claim = claim_for(&selected);

        assert!(matches!(
            verify_checkpoint_claim(&signed_bytes(&claim), &view, &authorization, &replay),
            CheckpointVerification::Verified { .. }
        ));

        let other_selection = SortedFrontier::try_new([op_id(0x20), op_id(0x99)]).expect("other");
        let mut wrong_frontier = claim.clone();
        wrong_frontier.frontier_commitment = other_selection.commitment();
        assert_eq!(
            verify_checkpoint_claim(
                &signed_bytes(&wrong_frontier),
                &view,
                &authorization,
                &replay
            ),
            CheckpointVerification::Rejected(CheckpointRejection::FrontierCommitmentMismatch {
                claimed: other_selection.commitment(),
                selected: selected.commitment(),
            })
        );

        let mut wrong_manifest = claim.clone();
        wrong_manifest.segment_manifest_id = Hash32([0x52; 32]);
        assert_eq!(
            verify_checkpoint_claim(
                &signed_bytes(&wrong_manifest),
                &view,
                &authorization,
                &replay
            ),
            CheckpointVerification::Rejected(CheckpointRejection::SegmentManifestMismatch {
                claimed: Hash32([0x52; 32]),
                selected: SEGMENT_MANIFEST,
            })
        );

        let mut wrong_authorization_root = claim.clone();
        wrong_authorization_root.authorization_view_root = Hash32([0x5b; 32]);
        assert_eq!(
            verify_checkpoint_claim(
                &signed_bytes(&wrong_authorization_root),
                &view,
                &authorization,
                &replay,
            ),
            CheckpointVerification::Rejected(CheckpointRejection::AuthorizationViewRootMismatch {
                claimed: Hash32([0x5b; 32]),
                replayed: AUTHORIZATION_ROOT,
            })
        );

        let mut other_materializer = claim.clone();
        other_materializer.materializer_id = Identifier32([0x12; 32]);
        let mut other_version = claim.clone();
        other_version.materializer_version = 8;
        let mut other_projection_root = claim;
        other_projection_root.projection_root_hash = Hash32([0x7f; 32]);
        for mutated in [other_materializer, other_version, other_projection_root] {
            assert!(
                matches!(
                    verify_checkpoint_claim(
                        &signed_bytes(&mutated),
                        &view,
                        &authorization,
                        &replay
                    ),
                    CheckpointVerification::Rejected(
                        CheckpointRejection::ProjectionRootMismatch { .. }
                    )
                ),
                "a re-signed claim whose projection identity changed must not verify"
            );
        }
    }

    #[test]
    fn checkpoint_requires_manager_plus_and_checkpoint_capability() {
        let selected = SortedFrontier::try_new([op_id(0x10)]).expect("frontier");
        let accessible = verse_wide_access();
        let required = [Scope::verse_wide(VERSE)];
        let view = consumer(&selected, &required, &accessible);
        let authorization = FakeAuthorizationView::permissive();
        let replay = FakeReplay { available: true };
        let claim = claim_for(&selected);

        for chain_id in [EDITOR_CHAIN, RELAY_SEED_CHAIN] {
            let mut refused = claim.clone();
            refused.capability = CapabilityRef {
                chain_id,
                scope_epoch: CURRENT_EPOCH,
            };
            assert_eq!(
                verify_checkpoint_claim(&signed_bytes(&refused), &view, &authorization, &replay),
                CheckpointVerification::Rejected(CheckpointRejection::NotManagerPlus)
            );
        }

        let mut stale_epoch = claim.clone();
        stale_epoch.capability = CapabilityRef {
            chain_id: MANAGER_CHAIN,
            scope_epoch: CURRENT_EPOCH - 1,
        };
        assert_eq!(
            verify_checkpoint_claim(&signed_bytes(&stale_epoch), &view, &authorization, &replay),
            CheckpointVerification::Rejected(CheckpointRejection::NotManagerPlus)
        );

        for chain_id in [MATERIALIZER_ONLY_CHAIN, EXPIRED_MANAGER_CHAIN] {
            let mut refused = claim.clone();
            refused.capability = CapabilityRef {
                chain_id,
                scope_epoch: CURRENT_EPOCH,
            };
            assert_eq!(
                verify_checkpoint_claim(&signed_bytes(&refused), &view, &authorization, &replay),
                CheckpointVerification::Rejected(
                    CheckpointRejection::CapabilityDoesNotPermitCheckpoint
                )
            );
        }

        let mut forged = sign_checkpoint_claim(&signing_key(), &claim).expect("sign");
        forged.signature[0] ^= 0xff;
        assert_eq!(
            verify_checkpoint_claim(
                &forged.encode_canonical().expect("encode"),
                &view,
                &authorization,
                &replay,
            ),
            CheckpointVerification::Rejected(CheckpointRejection::Malformed(
                CheckpointError::Signing(SigningError::SignatureVerificationFailed)
            ))
        );
    }

    #[test]
    fn checkpoint_replay_mismatch_is_not_authoritative() {
        let selected = SortedFrontier::try_new([op_id(0x10)]).expect("frontier");
        let accessible = verse_wide_access();
        let required = [Scope::verse_wide(VERSE)];
        let view = consumer(&selected, &required, &accessible);
        let authorization = FakeAuthorizationView::permissive();

        let mut disagreeing_claim = claim_for(&selected);
        disagreeing_claim.projection_root_hash = Hash32([0xaa; 32]);
        let bytes = signed_bytes(&disagreeing_claim);

        let signed = SignedCheckpointClaim::decode_canonical(&bytes).expect("decode");
        assert_eq!(
            signed.verify_signature(),
            Ok(()),
            "the Manager+ signature is valid and still buys nothing"
        );
        assert!(matches!(
            verify_checkpoint_claim(
                &bytes,
                &view,
                &authorization,
                &FakeReplay { available: true }
            ),
            CheckpointVerification::Rejected(CheckpointRejection::ProjectionRootMismatch { .. })
        ));

        let honest = signed_bytes(&claim_for(&selected));
        assert_eq!(
            verify_checkpoint_claim(
                &honest,
                &view,
                &authorization,
                &FakeReplay { available: false },
            ),
            CheckpointVerification::UntrustedAccelerator {
                checkpoint_id: Hash32::of(&honest),
                reason: UntrustedAcceleratorReason::ReplayUnavailable,
            },
            "a consumer that cannot replay holds an accelerator, not a verified projection"
        );

        let no_authorization_view = FakeAuthorizationView {
            authorization_view_root: None,
            ..FakeAuthorizationView::permissive()
        };
        assert_eq!(
            verify_checkpoint_claim(
                &honest,
                &view,
                &no_authorization_view,
                &FakeReplay { available: true },
            ),
            CheckpointVerification::UntrustedAccelerator {
                checkpoint_id: Hash32::of(&honest),
                reason: UntrustedAcceleratorReason::AuthorizationViewUnavailable,
            }
        );
    }

    #[test]
    fn scope_affine_checkpoint_snapshots_preserve_sparse_access() {
        let selected = SortedFrontier::try_new([op_id(0x10)]).expect("frontier");
        let own_petal = petal_scope(0x31);
        let other_petal = petal_scope(0x32);
        let authorization = FakeAuthorizationView::permissive();
        let replay = FakeReplay { available: true };
        let bytes = signed_bytes(&claim_for(&selected));

        let petal_access = AccessibleScopes::new([own_petal]);
        assert!(petal_access.covers(&own_petal));
        assert!(!petal_access.covers(&other_petal));
        assert!(!petal_access.covers(&Scope::verse_wide(VERSE)));

        let whole_verse_selection = [own_petal, other_petal];
        assert_eq!(
            verify_checkpoint_claim(
                &bytes,
                &consumer(&selected, &whole_verse_selection, &petal_access),
                &authorization,
                &replay,
            ),
            CheckpointVerification::UntrustedAccelerator {
                checkpoint_id: Hash32::of(&bytes),
                reason: UntrustedAcceleratorReason::InaccessibleScope { scope: other_petal },
            },
            "a petal-authorized peer cannot claim whole-verse replay verification"
        );

        let own_component = [own_petal];
        assert!(
            matches!(
                verify_checkpoint_claim(
                    &bytes,
                    &consumer(&selected, &own_component, &petal_access),
                    &authorization,
                    &replay,
                ),
                CheckpointVerification::Verified { .. }
            ),
            "the same peer still validates its own scope-affine component"
        );

        assert!(matches!(
            verify_checkpoint_claim(
                &bytes,
                &consumer(&selected, &whole_verse_selection, &verse_wide_access()),
                &authorization,
                &replay,
            ),
            CheckpointVerification::Verified { .. }
        ));
    }

    #[test]
    fn multi_head_tracking_checkpoint_and_gc_bind_sorted_frontier() {
        let heads = [op_id(0x30), op_id(0x10), op_id(0x20)];
        let selected = SortedFrontier::try_new(heads).expect("frontier");
        let accessible = verse_wide_access();
        let required = [Scope::verse_wide(VERSE)];
        let authorization = FakeAuthorizationView::permissive();
        let replay = FakeReplay { available: true };
        let bytes = signed_bytes(&claim_for(&selected));

        let permuted =
            SortedFrontier::try_new([op_id(0x20), op_id(0x30), op_id(0x10)]).expect("frontier");
        let verification = verify_checkpoint_claim(
            &bytes,
            &consumer(&permuted, &required, &accessible),
            &authorization,
            &replay,
        );
        assert_eq!(
            verification,
            CheckpointVerification::Verified {
                checkpoint_id: Hash32::of(&bytes),
            },
            "a multi-head frontier checkpoints under any arrival permutation"
        );

        let collapsed = SortedFrontier::try_new([op_id(0x30)]).expect("frontier");
        assert_eq!(
            verify_checkpoint_claim(
                &bytes,
                &consumer(&collapsed, &required, &accessible),
                &authorization,
                &replay,
            ),
            CheckpointVerification::Rejected(CheckpointRejection::FrontierCommitmentMismatch {
                claimed: selected.commitment(),
                selected: collapsed.commitment(),
            }),
            "collapsing concurrent heads breaks the D-CL19 binding"
        );

        let complete = BootstrapCoverage {
            every_head_has_retained_bootstrap_path: true,
        };
        assert_eq!(
            compaction_decision(&verification, complete, &[]),
            CompactionDecision::Permitted {
                checkpoint_id: Hash32::of(&bytes),
            }
        );
        assert_eq!(
            compaction_decision(
                &verification,
                BootstrapCoverage {
                    every_head_has_retained_bootstrap_path: false,
                },
                &[],
            ),
            CompactionDecision::Refused(CompactionRefusal::IncompleteBootstrapPaths)
        );

        // Suppression evidence is inspected, not asserted: an unproven retained bootstrap path
        // refuses compaction even though the checkpoint itself verified.
        let proven = tombstone_record(true);
        let unproven = tombstone_record(false);
        assert_eq!(
            compaction_decision(&verification, complete, std::slice::from_ref(&proven)),
            CompactionDecision::Permitted {
                checkpoint_id: Hash32::of(&bytes),
            }
        );
        assert_eq!(
            compaction_decision(&verification, complete, &[proven, unproven.clone()]),
            CompactionDecision::Refused(CompactionRefusal::UnprovedSuppressionEvidence {
                path_id: Hash32([0x3b; 32]),
            })
        );

        let unverified = verify_checkpoint_claim(
            &bytes,
            &consumer(&collapsed, &required, &accessible),
            &authorization,
            &replay,
        );
        assert_eq!(
            compaction_decision(&unverified, complete, &[]),
            CompactionDecision::Refused(CompactionRefusal::CheckpointNotVerified),
            "GC never rests on an unverified checkpoint"
        );
        assert_eq!(
            crate::retention::tombstone::may_compact_tombstone(&unproven, &unverified, complete),
            CompactionDecision::Refused(CompactionRefusal::CheckpointNotVerified),
            "the single-record spelling is the same gate, not a weaker one"
        );
    }
}
