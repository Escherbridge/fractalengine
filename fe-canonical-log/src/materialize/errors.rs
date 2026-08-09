//! SPEC-4 §5 admission/append error taxonomy; see `src/AGENTS.md` §module-ownership and
//! `materialize/AGENTS.md` for the disposition classification rationale, including the M1/M2
//! amendments the §5 table alone did not cover.

use thiserror::Error;

use crate::envelope::Hash32;

/// Every reason a candidate operation is not appended, per §5 plus the M1/M2 amendments
/// (author equivocation and the SPEC-1 §6.2 same-verse parent rule).
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
            | Self::OpaquePayload { .. }
            | Self::AuthorEquivocation { .. } => AdmissionDisposition::Quarantine,
            Self::MaterializationFailed | Self::CheckpointMismatch => {
                AdmissionDisposition::DeferredMaterialization
            }
        }
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
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppendError {
    /// The claimed `op_id` is not `BLAKE3(candidate_bytes)`.
    #[error("claimed op_id does not equal BLAKE3 of the candidate bytes")]
    HashMismatch,
    /// `op_id` is already present with a different byte string than the candidate's.
    #[error("op_id {op_id:?} is already present with different bytes")]
    IntegrityConflict {
        /// The conflicting operation ID.
        op_id: Hash32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    #[test]
    fn every_reject_variant_reports_reject_and_nothing_else() {
        let reject_outcomes = [
            AdmissionOutcome::InvalidEnvelope {
                reason: "bad".into(),
            },
            AdmissionOutcome::UnauthorizedOperation,
            AdmissionOutcome::InvalidPayload,
            AdmissionOutcome::PreconditionFailed,
            AdmissionOutcome::SameVerseParentViolation { parent: hash(1) },
        ];
        for outcome in reject_outcomes {
            assert!(outcome.is_reject(), "{outcome:?} should be a reject");
            assert!(!outcome.is_quarantine());
            assert_eq!(outcome.disposition(), AdmissionDisposition::Reject);
        }
    }

    #[test]
    fn every_quarantine_variant_reports_quarantine_and_nothing_else() {
        let quarantine_outcomes = [
            AdmissionOutcome::MissingParent { missing: hash(1) },
            AdmissionOutcome::UnknownSchema {
                schema_hash: hash(2),
            },
            AdmissionOutcome::OpaquePayload { op_id: hash(3) },
            AdmissionOutcome::AuthorEquivocation {
                op_id: hash(4),
                conflicting_op_id: hash(5),
            },
        ];
        for outcome in quarantine_outcomes {
            assert!(outcome.is_quarantine(), "{outcome:?} should be quarantine");
            assert!(!outcome.is_reject());
            assert_eq!(outcome.disposition(), AdmissionDisposition::Quarantine);
        }
    }

    #[test]
    fn post_append_outcomes_are_neither_reject_nor_quarantine() {
        for outcome in [
            AdmissionOutcome::MaterializationFailed,
            AdmissionOutcome::CheckpointMismatch,
        ] {
            assert!(!outcome.is_reject());
            assert!(!outcome.is_quarantine());
            assert_eq!(
                outcome.disposition(),
                AdmissionDisposition::DeferredMaterialization
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
}
