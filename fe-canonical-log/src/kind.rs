//! Structural operation kinds (SPEC-1 §6); see `src/AGENTS.md` §module-ownership.

use thiserror::Error;

use crate::envelope::UnsignedEnvelope;

/// The v1 structural operation kinds, plus the open application range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    /// Kind 1: an intent operation with parents and an encrypted non-empty payload.
    NormalIntent,
    /// Kind 2: the parentless genesis of a branch.
    BranchGenesis,
    /// Kind 3: a detached-to-tracking merge.
    DetachedMerge,
    /// Kind 4: an epoch bump for an exact revoked scope.
    ScopeEpochBump,
    /// Kinds 5..=65535, whose payload rules live in a schema registry that does not exist yet.
    Application(u16),
}

/// Every reason an operation fails the §6 rules derivable from one envelope.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum StructuralRuleError {
    /// `operation_kind` was zero.
    #[error("operation_kind must be non-zero")]
    ZeroOperationKind,
    /// A `branch_genesis` named a parent.
    #[error("branch_genesis (kind 2) must have zero parents, found {count}")]
    BranchGenesisHasParents {
        /// Parents actually present.
        count: usize,
    },
    /// A normal intent named no parent.
    #[error("normal intent (kind 1) must have at least one parent")]
    NormalIntentHasNoParent,
    /// A normal intent lacked an encrypted non-empty payload.
    #[error("normal intent (kind 1) requires an encrypted non-empty payload reference")]
    NormalIntentPayloadNotEncrypted,
    /// A detached merge named fewer than two parents.
    #[error("detached merge (kind 3) must have at least two parents, found {count}")]
    DetachedMergeNeedsTwoParents {
        /// Parents actually present.
        count: usize,
    },
    /// A detached merge carried a payload.
    #[error("detached merge (kind 3) requires the no-payload form")]
    DetachedMergeNeedsNoPayload,
    /// A scope-epoch bump did not name exactly one parent.
    #[error("scope_epoch_bump (kind 4) must have exactly one parent, found {count}")]
    ScopeEpochBumpNeedsOneParent {
        /// Parents actually present.
        count: usize,
    },
    /// A scope-epoch bump carried a payload.
    #[error("scope_epoch_bump (kind 4) requires the no-payload form")]
    ScopeEpochBumpNeedsNoPayload,
}

impl OperationKind {
    /// Classifies a wire `operation_kind`, rejecting zero.
    pub fn from_u16(value: u16) -> Result<Self, StructuralRuleError> {
        match value {
            0 => Err(StructuralRuleError::ZeroOperationKind),
            1 => Ok(Self::NormalIntent),
            2 => Ok(Self::BranchGenesis),
            3 => Ok(Self::DetachedMerge),
            4 => Ok(Self::ScopeEpochBump),
            other => Ok(Self::Application(other)),
        }
    }

    /// The wire `operation_kind` for this classification.
    pub const fn to_u16(self) -> u16 {
        match self {
            Self::NormalIntent => 1,
            Self::BranchGenesis => 2,
            Self::DetachedMerge => 3,
            Self::ScopeEpochBump => 4,
            Self::Application(value) => value,
        }
    }
}

/// Checks every §6 rule derivable from ONE envelope, with no DAG or registry lookup.
///
/// Kinds 5 and above pass through structurally unvalidated because no schema registry
/// exists yet: an unknown kind or unknown `schema_hash` MUST be quarantined by the
/// admission layer (§6.7), never here and never by silent reinterpretation.
pub fn validate_structural_rules(
    envelope: &UnsignedEnvelope,
) -> Result<OperationKind, StructuralRuleError> {
    let kind = OperationKind::from_u16(envelope.operation_kind)?;
    let parent_count = envelope.parents.len();

    match kind {
        OperationKind::NormalIntent => {
            if parent_count == 0 {
                return Err(StructuralRuleError::NormalIntentHasNoParent);
            }
            if envelope.payload.encryption.is_none() || envelope.payload.ciphertext_length == 0 {
                return Err(StructuralRuleError::NormalIntentPayloadNotEncrypted);
            }
        }
        OperationKind::BranchGenesis => {
            if parent_count != 0 {
                return Err(StructuralRuleError::BranchGenesisHasParents {
                    count: parent_count,
                });
            }
        }
        OperationKind::DetachedMerge => {
            if parent_count < 2 {
                return Err(StructuralRuleError::DetachedMergeNeedsTwoParents {
                    count: parent_count,
                });
            }
            if !envelope.payload.is_no_payload_form() {
                return Err(StructuralRuleError::DetachedMergeNeedsNoPayload);
            }
        }
        OperationKind::ScopeEpochBump => {
            if parent_count != 1 {
                return Err(StructuralRuleError::ScopeEpochBumpNeedsOneParent {
                    count: parent_count,
                });
            }
            if !envelope.payload.is_no_payload_form() {
                return Err(StructuralRuleError::ScopeEpochBumpNeedsNoPayload);
            }
        }
        OperationKind::Application(_) => {}
    }

    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{
        Author, CapabilityRef, EncryptionParams, Hash32, Hlc, Identifier32, PayloadRef, Scope,
        NONCE_LENGTH, PROTOCOL_VERSION,
    };

    fn envelope(
        operation_kind: u16,
        parents: Vec<Hash32>,
        payload: PayloadRef,
    ) -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind,
            scope: Scope::verse_wide(Identifier32([0x11; 32])),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 0,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: Identifier32([0x55; 32]),
            parents,
            hlc: Hlc::new(1, 0),
            payload,
        }
    }

    fn parent(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn encrypted_payload() -> PayloadRef {
        PayloadRef {
            ciphertext_hash: Hash32::of(&[0xc0, 0xff, 0xee]),
            ciphertext_length: 3,
            encryption: Some(EncryptionParams {
                suite_id: 1,
                key_id: Identifier32([0x77; 32]),
                nonce: [0x88; NONCE_LENGTH],
            }),
        }
    }

    #[test]
    fn wire_values_map_to_kinds_and_back() {
        for (value, kind) in [
            (1u16, OperationKind::NormalIntent),
            (2, OperationKind::BranchGenesis),
            (3, OperationKind::DetachedMerge),
            (4, OperationKind::ScopeEpochBump),
            (5, OperationKind::Application(5)),
            (65535, OperationKind::Application(65535)),
        ] {
            assert_eq!(OperationKind::from_u16(value), Ok(kind));
            assert_eq!(kind.to_u16(), value);
        }
        assert_eq!(
            OperationKind::from_u16(0),
            Err(StructuralRuleError::ZeroOperationKind)
        );
    }

    #[test]
    fn branch_genesis_must_be_parentless() {
        assert_eq!(
            validate_structural_rules(&envelope(2, Vec::new(), PayloadRef::empty())),
            Ok(OperationKind::BranchGenesis)
        );
        assert_eq!(
            validate_structural_rules(&envelope(2, vec![parent(1)], PayloadRef::empty())),
            Err(StructuralRuleError::BranchGenesisHasParents { count: 1 })
        );
    }

    #[test]
    fn normal_intent_needs_a_parent_and_an_encrypted_non_empty_payload() {
        assert_eq!(
            validate_structural_rules(&envelope(1, vec![parent(1)], encrypted_payload())),
            Ok(OperationKind::NormalIntent)
        );
        assert_eq!(
            validate_structural_rules(&envelope(1, Vec::new(), encrypted_payload())),
            Err(StructuralRuleError::NormalIntentHasNoParent)
        );
        assert_eq!(
            validate_structural_rules(&envelope(1, vec![parent(1)], PayloadRef::empty())),
            Err(StructuralRuleError::NormalIntentPayloadNotEncrypted)
        );

        let mut zero_length = encrypted_payload();
        zero_length.ciphertext_length = 0;
        assert_eq!(
            validate_structural_rules(&envelope(1, vec![parent(1)], zero_length)),
            Err(StructuralRuleError::NormalIntentPayloadNotEncrypted)
        );
    }

    #[test]
    fn detached_merge_needs_two_parents_and_no_payload() {
        assert_eq!(
            validate_structural_rules(&envelope(
                3,
                vec![parent(1), parent(2)],
                PayloadRef::empty()
            )),
            Ok(OperationKind::DetachedMerge)
        );
        assert_eq!(
            validate_structural_rules(&envelope(3, vec![parent(1)], PayloadRef::empty())),
            Err(StructuralRuleError::DetachedMergeNeedsTwoParents { count: 1 })
        );
        assert_eq!(
            validate_structural_rules(&envelope(
                3,
                vec![parent(1), parent(2)],
                encrypted_payload()
            )),
            Err(StructuralRuleError::DetachedMergeNeedsNoPayload)
        );
    }

    #[test]
    fn scope_epoch_bump_needs_one_parent_and_no_payload() {
        assert_eq!(
            validate_structural_rules(&envelope(4, vec![parent(1)], PayloadRef::empty())),
            Ok(OperationKind::ScopeEpochBump)
        );
        assert_eq!(
            validate_structural_rules(&envelope(
                4,
                vec![parent(1), parent(2)],
                PayloadRef::empty()
            )),
            Err(StructuralRuleError::ScopeEpochBumpNeedsOneParent { count: 2 })
        );
        assert_eq!(
            validate_structural_rules(&envelope(4, vec![parent(1)], encrypted_payload())),
            Err(StructuralRuleError::ScopeEpochBumpNeedsNoPayload)
        );
    }

    #[test]
    fn application_kinds_pass_through_unvalidated() {
        for kind in [5u16, 1000, 65535] {
            assert_eq!(
                validate_structural_rules(&envelope(kind, Vec::new(), PayloadRef::empty())),
                Ok(OperationKind::Application(kind))
            );
            assert_eq!(
                validate_structural_rules(&envelope(
                    kind,
                    vec![parent(1), parent(2)],
                    encrypted_payload()
                )),
                Ok(OperationKind::Application(kind))
            );
        }
    }

    #[test]
    fn a_zero_kind_is_rejected_before_any_structural_rule() {
        assert_eq!(
            validate_structural_rules(&envelope(0, Vec::new(), PayloadRef::empty())),
            Err(StructuralRuleError::ZeroOperationKind)
        );
    }
}
