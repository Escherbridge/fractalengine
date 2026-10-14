//! SPEC-4 §5 admission/append error taxonomy; see `src/AGENTS.md` §module-ownership and
//! `materialize/AGENTS.md` for the disposition classification rationale, including the M1/M2
//! amendments the §5 table alone did not cover.

use std::fmt;

use thiserror::Error;

use crate::compose::QuarantineReason;
use crate::envelope::Hash32;

/// The authorization evidence a candidate needs but this process cannot currently resolve
/// (SPEC-2 `author-key-lifecycle.md` rule 5, erratum E-W4-1; see `materialize/AGENTS.md`
/// §admission-vocabulary).
///
/// Wraps the crate's one [`QuarantineReason`] vocabulary rather than duplicating it, and admits
/// only the three reasons that are an evidence gap: a capability chain, an epoch state, or a
/// payload key. The field is private so [`Self::new`] and the three named constructors are the
/// only doors. Stored as a one-byte tag, so `AdmissionOutcome` stays small.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizationEvidenceGap(EvidenceGapKind);

/// The three gap reasons, private so no fourth can be named from outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvidenceGapKind {
    CapabilityChain,
    EpochState,
    PayloadKey,
}

impl AuthorizationEvidenceGap {
    /// Wraps `reason`, handing it back unchanged when it is not an authorization-evidence gap.
    ///
    /// A total match with no wildcard: a new [`QuarantineReason`] variant does not compile until
    /// it is classified here.
    pub fn new(reason: QuarantineReason) -> Result<Self, QuarantineReason> {
        match reason {
            QuarantineReason::CapabilityChainUnavailable => {
                Ok(Self::capability_chain_unavailable())
            }
            QuarantineReason::EpochStateUnavailable => Ok(Self::epoch_state_unavailable()),
            QuarantineReason::PayloadKeyUnavailable => Ok(Self::payload_key_unavailable()),
            QuarantineReason::MissingParent { .. }
            | QuarantineReason::ParentNotAdmitted { .. }
            | QuarantineReason::UnknownSchema { .. }
            | QuarantineReason::UnknownKind { .. }
            | QuarantineReason::ReferencedDisavowUnavailable { .. }
            | QuarantineReason::AuthorEquivocation { .. }
            | QuarantineReason::ArtifactIdMismatch { .. }
            | QuarantineReason::StoredLengthMismatch
            | QuarantineReason::MalformedSealedArtifact
            | QuarantineReason::FailedDecryption
            | QuarantineReason::MalformedLaneBody
            | QuarantineReason::CrossCheckFailure { .. }
            | QuarantineReason::Unauthorized => Err(reason),
        }
    }

    /// The capability chain artifact the envelope names is not locally resolvable.
    pub const fn capability_chain_unavailable() -> Self {
        Self(EvidenceGapKind::CapabilityChain)
    }

    /// The scope-epoch state the capability is checked against is not locally resolvable.
    pub const fn epoch_state_unavailable() -> Self {
        Self(EvidenceGapKind::EpochState)
    }

    /// The scope payload key the envelope's encryption map names is not held.
    pub const fn payload_key_unavailable() -> Self {
        Self(EvidenceGapKind::PayloadKey)
    }

    /// The wrapped reason, in the crate's single quarantine vocabulary.
    pub fn reason(&self) -> QuarantineReason {
        match self.0 {
            EvidenceGapKind::CapabilityChain => QuarantineReason::CapabilityChainUnavailable,
            EvidenceGapKind::EpochState => QuarantineReason::EpochStateUnavailable,
            EvidenceGapKind::PayloadKey => QuarantineReason::PayloadKeyUnavailable,
        }
    }
}

impl fmt::Display for AuthorizationEvidenceGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.reason(), f)
    }
}

/// Every reason a candidate operation is not appended, per §5 plus the M1/M2 amendments
/// (author equivocation and the SPEC-1 §6.2 same-verse parent rule), the §5 `unknown_kind`
/// row (erratum G1), and the SPEC-2 evidence-gap quarantine (erratum E-W4-1).
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AdmissionOutcome {
    /// The candidate bytes failed canonical decode, signature, or structural validation.
    #[error("envelope failed structural or grammar validation: {reason}")]
    InvalidEnvelope {
        /// Human-readable cause, sourced from the underlying `SigningError`/`EnvelopeError`.
        reason: String,
    },
    /// The presented capability does not authorize this operation.
    #[error("operation is not authorized under the presented capability")]
    UnauthorizedOperation,
    /// The decrypted payload failed schema validation.
    #[error("payload failed schema or decode validation")]
    InvalidPayload,
    /// A caller-supplied precondition (for example `require_node_exists`) was not satisfied.
    #[error("a caller-supplied precondition was not satisfied")]
    PreconditionFailed,
    /// A declared parent is not yet locally admitted; retryable once it arrives (§4.5).
    #[error("parent {missing:?} is not yet locally admitted")]
    MissingParent {
        /// The absent parent operation ID.
        missing: Hash32,
    },
    /// `schema_hash` names a schema this process does not have locally.
    #[error("schema {schema_hash:?} is not locally known")]
    UnknownSchema {
        /// The unrecognized schema hash.
        schema_hash: Hash32,
    },
    /// SPEC-4 §5 `unknown_kind` (erratum G1): this build has no registered structural rule,
    /// schema, and reduction for the candidate's `operation_kind` (SPEC-1 §6 rule 7).
    ///
    /// Quarantine, never a reject and never [`Self::InvalidEnvelope`]: the bytes may be valid to
    /// an upgraded peer, and the kind arrives through a build upgrade rather than the registry.
    #[error("operation kind {operation_kind} is not known to this build")]
    UnknownKind {
        /// The unrecognized non-zero wire operation kind.
        operation_kind: u16,
    },
    /// SPEC-2 rule 5 (erratum E-W4-1, owner-confirmed 2026-10-13): a capability chain, epoch
    /// state, or payload key the candidate needs is not locally resolvable, so authorization is
    /// undecided rather than refused.
    #[error("authorization evidence is unavailable: {reason}")]
    AuthorizationEvidenceUnavailable {
        /// Which evidence is missing; constructible only for the three gap reasons.
        reason: AuthorizationEvidenceGap,
    },
    /// The payload could not be decrypted with a currently available scope key.
    #[error("payload for operation {op_id:?} could not be decrypted with an available key")]
    OpaquePayload {
        /// The operation whose payload is opaque.
        op_id: Hash32,
    },
    /// SPEC-1 §6.2: a parent does not share this operation's verse scope. A permanent
    /// structural violation, distinct from a merely-not-yet-seen parent.
    #[error("parent {parent:?} does not share this operation's verse scope (SPEC-1 §6.2)")]
    SameVerseParentViolation {
        /// The offending cross-verse parent.
        parent: Hash32,
    },
    /// D-CL25 / SPEC-1 §3.4 (amendment M1): two distinct `op_id`s share one `EquivocationKey`.
    /// The rule is quarantine BOTH, materialize NEITHER, until an authorized resolution
    /// operation runs.
    #[error("operation {op_id:?} shares an EquivocationKey with {conflicting_op_id:?}")]
    AuthorEquivocation {
        /// The candidate operation.
        op_id: Hash32,
        /// The already-admitted operation sharing its `EquivocationKey`.
        conflicting_op_id: Hash32,
    },
    /// The operation was durably appended but its reduction step failed; retry via replay.
    #[error("append succeeded but materialization failed; retry via replay")]
    MaterializationFailed,
    /// A checkpoint's bound fields do not match the caller's currently selected values.
    #[error("checkpoint binding does not match the caller's current selection")]
    CheckpointMismatch,
}

/// The §5.1/§5.2 disposition of an [`AdmissionOutcome`]: whether the caller must reject the
/// candidate outright, quarantine it pending more information, or -- for the two post-append
/// outcomes -- neither, because the operation is already durably appended and only its
/// materialization step is unresolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionDisposition {
    /// Permanently invalid; never appended, never retried as-is.
    Reject,
    /// Retryable pending availability; retained as evidence, never materialized until resolved.
    Quarantine,
    /// Already durably appended; only the materialization step is pending.
    DeferredMaterialization,
}

impl AdmissionOutcome {
    /// The §5.1/§5.2 disposition of this outcome.
    pub fn disposition(&self) -> AdmissionDisposition {
        match self {
            Self::InvalidEnvelope { .. }
            | Self::UnauthorizedOperation
            | Self::InvalidPayload
            | Self::PreconditionFailed
            | Self::SameVerseParentViolation { .. } => AdmissionDisposition::Reject,
            Self::MissingParent { .. }
            | Self::UnknownSchema { .. }
            | Self::UnknownKind { .. }
            | Self::AuthorizationEvidenceUnavailable { .. }
            | Self::OpaquePayload { .. }
            | Self::AuthorEquivocation { .. } => AdmissionDisposition::Quarantine,
            Self::MaterializationFailed | Self::CheckpointMismatch => {
                AdmissionDisposition::DeferredMaterialization
            }
        }
    }

    /// The SPEC-2 evidence-gap quarantine for `reason`, or `reason` back unchanged when it is
    /// not one of the three gap reasons [`AuthorizationEvidenceGap::new`] admits.
    pub fn authorization_evidence_unavailable(
        reason: QuarantineReason,
    ) -> Result<Self, QuarantineReason> {
        AuthorizationEvidenceGap::new(reason)
            .map(|reason| Self::AuthorizationEvidenceUnavailable { reason })
    }

    /// Whether this outcome is a permanent, §5.1 reject.
    pub fn is_reject(&self) -> bool {
        matches!(self.disposition(), AdmissionDisposition::Reject)
    }

    /// Whether this outcome is a retryable, §5.2 quarantine.
    pub fn is_quarantine(&self) -> bool {
        matches!(self.disposition(), AdmissionDisposition::Quarantine)
    }
}

/// The result of attempting to append verified bytes under a claimed `op_id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    /// The bytes were not previously present and are now durably appended.
    Appended,
    /// The exact same bytes were already present under this `op_id`. Success, not an error --
    /// exactly-once append must be idempotent under retry.
    AlreadyPresent,
}

/// Every reason an append attempt does not succeed.
///
/// Two dispositions, and callers must not confuse them. `HashMismatch` and `IntegrityConflict`
/// are DEFINITE answers about the candidate bytes, which SPEC §3.3 treats as permanently
/// blacklisting the `op_id`. `StorageUnavailable` is INDETERMINATE: presence and integrity are
/// simply unknown, and the only correct response is to retry. See `src/AGENTS.md` §append-errors.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppendError {
    /// The claimed `op_id` is not `BLAKE3(candidate_bytes)`.
    #[error("claimed op_id does not equal BLAKE3 of the candidate bytes")]
    HashMismatch,
    /// `op_id` is already present with a different byte string than the candidate's.
    ///
    /// Definite, and permanently blacklisting. This is NOT the variant for bytes that merely
    /// failed to decode locally -- a future protocol version is undecodable here without being
    /// corrupt, and blacklisting it would refuse an operation that is valid on the network.
    /// Report that as [`AppendError::StorageUnavailable`].
    #[error("op_id {op_id:?} is already present with different bytes")]
    IntegrityConflict {
        /// The conflicting operation ID.
        op_id: Hash32,
    },
    /// The store could not answer: an I/O fault, a decode failure in already-stored bytes, or
    /// any other condition under which presence and integrity are simply unknown.
    ///
    /// Indeterminate, never a decision. The caller MUST retry the exact same immutable bytes; it
    /// MUST NOT treat this as "absent", as a SPEC §5.1 reject, or as a SPEC §5.2 quarantine.
    #[error("the log could not answer for op_id {op_id:?}: {reason}")]
    StorageUnavailable {
        /// The operation whose append could not be resolved.
        op_id: Hash32,
        /// Human-readable cause, for the operator; never a control-flow input.
        reason: String,
    },
    /// Not an admissible envelope *here*: undecodable, unsigned, structurally invalid, or a
    /// protocol version this build predates.
    ///
    /// Local and indeterminate. It says nothing about the `op_id`, which may be perfectly valid
    /// to a peer that understands it, so it MUST NOT blacklist the identity. This is the variant
    /// for a failed decode -- never [`AppendError::IntegrityConflict`].
    #[error("op_id {op_id:?} is not an envelope this build can admit: {reason}")]
    NotAnEnvelope {
        /// The operation that could not be admitted.
        op_id: Hash32,
        /// Human-readable cause, for the operator; never a control-flow input.
        reason: String,
    },
}

impl AppendError {
    /// True when the error is a definite answer about the candidate bytes, and therefore
    /// eligible for the SPEC §3.3 permanent blacklist. False for indeterminate answers.
    pub fn is_definite(&self) -> bool {
        match self {
            AppendError::HashMismatch | AppendError::IntegrityConflict { .. } => true,
            AppendError::StorageUnavailable { .. } | AppendError::NotAnEnvelope { .. } => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    /// The §5 table, E-W4-1 and erratum G1 as one table: every `AdmissionOutcome` variant with
    /// the disposition its spec row requires. A variant added without a row here still compiles,
    /// but `disposition()` itself refuses to compile until the variant is classified.
    #[test]
    fn every_admission_outcome_has_its_spec_disposition() {
        use AdmissionDisposition::{DeferredMaterialization, Quarantine, Reject};
        let table = [
            (
                AdmissionOutcome::InvalidEnvelope {
                    reason: "bad".into(),
                },
                Reject,
            ),
            (AdmissionOutcome::UnauthorizedOperation, Reject),
            (AdmissionOutcome::InvalidPayload, Reject),
            (AdmissionOutcome::PreconditionFailed, Reject),
            (
                AdmissionOutcome::SameVerseParentViolation { parent: hash(1) },
                Reject,
            ),
            (
                AdmissionOutcome::MissingParent { missing: hash(1) },
                Quarantine,
            ),
            (
                AdmissionOutcome::UnknownSchema {
                    schema_hash: hash(2),
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::UnknownKind {
                    operation_kind: 4_242,
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::AuthorizationEvidenceUnavailable {
                    reason: AuthorizationEvidenceGap::capability_chain_unavailable(),
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::AuthorizationEvidenceUnavailable {
                    reason: AuthorizationEvidenceGap::epoch_state_unavailable(),
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::AuthorizationEvidenceUnavailable {
                    reason: AuthorizationEvidenceGap::payload_key_unavailable(),
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::OpaquePayload { op_id: hash(3) },
                Quarantine,
            ),
            (
                AdmissionOutcome::AuthorEquivocation {
                    op_id: hash(4),
                    conflicting_op_id: hash(5),
                },
                Quarantine,
            ),
            (
                AdmissionOutcome::MaterializationFailed,
                DeferredMaterialization,
            ),
            (
                AdmissionOutcome::CheckpointMismatch,
                DeferredMaterialization,
            ),
        ];
        for (outcome, expected) in table {
            assert_eq!(outcome.disposition(), expected, "{outcome:?}");
            assert_eq!(outcome.is_reject(), expected == Reject, "{outcome:?}");
            assert_eq!(
                outcome.is_quarantine(),
                expected == Quarantine,
                "{outcome:?}"
            );
        }
    }

    #[test]
    fn only_the_three_evidence_gaps_wrap_into_authorization_evidence_unavailable() {
        for gap in [
            QuarantineReason::CapabilityChainUnavailable,
            QuarantineReason::EpochStateUnavailable,
            QuarantineReason::PayloadKeyUnavailable,
        ] {
            let outcome = AdmissionOutcome::authorization_evidence_unavailable(gap.clone())
                .expect("an evidence gap wraps");
            assert!(outcome.is_quarantine());
            assert!(matches!(
                &outcome,
                AdmissionOutcome::AuthorizationEvidenceUnavailable { reason }
                    if reason.reason() == gap
            ));
        }
        for not_a_gap in [
            QuarantineReason::Unauthorized,
            QuarantineReason::UnknownKind { operation_kind: 9 },
            QuarantineReason::MissingParent {
                missing_parents: vec![hash(1)],
            },
            QuarantineReason::FailedDecryption,
        ] {
            assert_eq!(
                AdmissionOutcome::authorization_evidence_unavailable(not_a_gap.clone()),
                Err(not_a_gap),
                "a refusal or a non-authorization gap must not become an evidence quarantine"
            );
        }
    }

    #[test]
    fn already_present_is_success_not_an_error() {
        let outcome: Result<AppendOutcome, AppendError> = Ok(AppendOutcome::AlreadyPresent);
        assert_eq!(outcome, Ok(AppendOutcome::AlreadyPresent));
    }

    #[test]
    fn hash_mismatch_and_integrity_conflict_are_distinct_reasons() {
        let hash_mismatch = AppendError::HashMismatch;
        let integrity_conflict = AppendError::IntegrityConflict { op_id: hash(9) };
        assert_ne!(hash_mismatch, integrity_conflict);
    }

    #[test]
    fn storage_unavailable_is_indeterminate_and_distinct_from_both_definite_answers() {
        let unavailable = AppendError::StorageUnavailable {
            op_id: hash(9),
            reason: "datastore refused the read".to_string(),
        };
        assert_ne!(unavailable, AppendError::HashMismatch);
        assert_ne!(
            unavailable,
            AppendError::IntegrityConflict { op_id: hash(9) }
        );
        assert!(!unavailable.is_definite());
        assert!(AppendError::HashMismatch.is_definite());
        assert!(AppendError::IntegrityConflict { op_id: hash(9) }.is_definite());
    }
}
