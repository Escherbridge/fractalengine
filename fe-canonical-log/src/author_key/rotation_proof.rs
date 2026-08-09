//! The §3.2 successor dual proof: possession by the successor over the §3.1 statement.

use ed25519_dalek::SigningKey;
use thiserror::Error;

use super::payloads::{RotationPayload, RotationPayloadError, RotationStatement};
use crate::envelope::SIGNATURE_LENGTH;
use crate::signing::{sign_domain, verify_author_binding, verify_domain, SigningError};

/// NUL-terminated ASCII domain separator for the §3.2 successor proof.
pub const ROTATION_PROOF_DOMAIN: &[u8] = b"fe-author-key-rotation-v1\0";

/// Every reason the §3.2 dual proof fails.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RotationProofError {
    /// The statement could not be encoded canonically.
    #[error("rotation payload: {0}")]
    Payload(#[from] RotationPayloadError),
    /// The predecessor DID did not bind to `predecessor_public_key`.
    #[error("predecessor DID does not bind to its public key: {0}")]
    PredecessorBinding(SigningError),
    /// The successor DID did not bind to `successor_public_key`.
    #[error("successor DID does not bind to its public key: {0}")]
    SuccessorBinding(SigningError),
    /// The successor key equalled the predecessor key (§3 rule 4).
    #[error("the successor key must differ from the predecessor key")]
    SuccessorEqualsPredecessor,
    /// The successor signature did not verify over the §3.2 preimage.
    #[error("successor proof verification failed: {0}")]
    ProofVerificationFailed(SigningError),
}

/// Signs the §3.2 preimage `ASCII("fe-author-key-rotation-v1") || 0x00 || statement`.
pub fn sign_successor_proof(
    successor_signing_key: &SigningKey,
    statement: &RotationStatement,
) -> Result<[u8; SIGNATURE_LENGTH], RotationPayloadError> {
    Ok(sign_domain(
        successor_signing_key,
        ROTATION_PROOF_DOMAIN,
        &statement.encode_canonical()?,
    ))
}

/// Verifies both DID bindings, the distinctness rule, and the §3.2 successor proof.
///
/// The predecessor's envelope signature proves authorization and is checked by SPEC-1
/// admission; this is the other half of the dual proof and is not a substitute for it.
pub fn verify_rotation_proof(payload: &RotationPayload) -> Result<(), RotationProofError> {
    let statement = &payload.statement;
    verify_author_binding(
        &statement.predecessor_did,
        &statement.predecessor_public_key,
    )
    .map_err(RotationProofError::PredecessorBinding)?;
    verify_author_binding(&statement.successor_did, &statement.successor_public_key)
        .map_err(RotationProofError::SuccessorBinding)?;
    if statement.successor_public_key == statement.predecessor_public_key {
        return Err(RotationProofError::SuccessorEqualsPredecessor);
    }
    verify_domain(
        &statement.successor_public_key,
        ROTATION_PROOF_DOMAIN,
        &statement.encode_canonical()?,
        &payload.successor_signature,
    )
    .map_err(RotationProofError::ProofVerificationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::payloads::ROTATION_PROTOCOL_VERSION;
    use crate::author_key::test_support::{hash, identifier, public_key, signing_key, verse_scope};
    use crate::did_key::public_key_to_did;
    use crate::signing::SIGNATURE_DOMAIN;

    fn statement(predecessor: u8, successor: u8) -> RotationStatement {
        RotationStatement {
            protocol_version: ROTATION_PROTOCOL_VERSION,
            scope: verse_scope(),
            branch_id: identifier(0x55),
            parent_op_id: hash(0x66),
            scope_epoch: 7,
            predecessor_did: public_key_to_did(&public_key(predecessor)),
            predecessor_public_key: public_key(predecessor),
            successor_did: public_key_to_did(&public_key(successor)),
            successor_public_key: public_key(successor),
        }
    }

    fn proved(predecessor: u8, successor: u8) -> RotationPayload {
        let statement = statement(predecessor, successor);
        let successor_signature =
            sign_successor_proof(&signing_key(successor), &statement).expect("sign");
        RotationPayload {
            statement,
            successor_signature,
        }
    }

    #[test]
    fn the_proof_domain_is_nul_terminated_and_distinct_from_the_envelope_domain() {
        assert_eq!(ROTATION_PROOF_DOMAIN, b"fe-author-key-rotation-v1\0");
        assert!(ROTATION_PROOF_DOMAIN.ends_with(&[0]));
        assert_ne!(ROTATION_PROOF_DOMAIN, SIGNATURE_DOMAIN);
    }

    #[test]
    fn spec2_case_01_a_valid_successor_proof_verifies() {
        assert_eq!(verify_rotation_proof(&proved(1, 2)), Ok(()));
    }

    #[test]
    fn spec2_case_02_changing_any_signed_field_invalidates_the_proof() {
        type Mutation = (&'static str, fn(&mut RotationPayload));
        let mutations: Vec<Mutation> = vec![
            ("branch", |payload| {
                payload.statement.branch_id = identifier(0x56)
            }),
            ("parent", |payload| {
                payload.statement.parent_op_id = hash(0x67)
            }),
            ("scope", |payload| {
                payload.statement.scope = crate::envelope::Scope::verse_wide(identifier(0x99))
            }),
            ("epoch", |payload| payload.statement.scope_epoch = 8),
        ];
        for (label, mutate) in mutations {
            let mut payload = proved(1, 2);
            mutate(&mut payload);
            assert_eq!(
                verify_rotation_proof(&payload),
                Err(RotationProofError::ProofVerificationFailed(
                    SigningError::SignatureVerificationFailed
                )),
                "a changed {label} must invalidate the successor proof"
            );
        }
    }

    #[test]
    fn spec2_case_02_a_malformed_did_key_binding_is_rejected() {
        let mut successor_mismatch = proved(1, 2);
        successor_mismatch.statement.successor_did = public_key_to_did(&public_key(9));
        assert_eq!(
            verify_rotation_proof(&successor_mismatch),
            Err(RotationProofError::SuccessorBinding(
                SigningError::AuthorBindingMismatch
            ))
        );

        let mut predecessor_mismatch = proved(1, 2);
        predecessor_mismatch.statement.predecessor_did = public_key_to_did(&public_key(9));
        assert_eq!(
            verify_rotation_proof(&predecessor_mismatch),
            Err(RotationProofError::PredecessorBinding(
                SigningError::AuthorBindingMismatch
            ))
        );
    }

    #[test]
    fn spec2_case_03_a_proof_made_by_another_key_is_rejected() {
        let statement = statement(1, 2);
        let successor_signature = sign_successor_proof(&signing_key(3), &statement).expect("sign");
        let payload = RotationPayload {
            statement,
            successor_signature,
        };
        assert_eq!(
            verify_rotation_proof(&payload),
            Err(RotationProofError::ProofVerificationFailed(
                SigningError::SignatureVerificationFailed
            ))
        );
    }

    #[test]
    fn spec2_case_03_a_repeated_key_is_rejected_structurally() {
        let payload = proved(1, 1);
        assert_eq!(
            verify_rotation_proof(&payload),
            Err(RotationProofError::SuccessorEqualsPredecessor),
            "a self-rotation is rejected even when its proof is cryptographically valid"
        );
    }

    #[test]
    fn a_proof_made_under_another_domain_does_not_verify() {
        let statement = statement(1, 2);
        let body = statement.encode_canonical().expect("statement bytes");
        let successor_signature = sign_domain(&signing_key(2), SIGNATURE_DOMAIN, &body);
        let payload = RotationPayload {
            statement,
            successor_signature,
        };
        assert_eq!(
            verify_rotation_proof(&payload),
            Err(RotationProofError::ProofVerificationFailed(
                SigningError::SignatureVerificationFailed
            ))
        );
    }
}
