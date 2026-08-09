//! Rotation statement and rotation payload grammar (SPEC-2 §3, §3.1).

use thiserror::Error;

use super::{bytes_at, entry, require_numeric_keys, text_at, unsigned_at};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{
    EnvelopeError, Hash32, Identifier32, Scope, UnsignedEnvelope, SIGNATURE_LENGTH,
};

/// `protocol_version` every v1 rotation statement carries (§3.1 key 0).
pub const ROTATION_PROTOCOL_VERSION: u64 = 1;

/// `operation_kind` a rotation MUST use: a normal encrypted intent (§3.1 rule 1).
pub const ROTATION_OPERATION_KIND: u16 = 1;

const STATEMENT_CONTEXT: &str = "rotation_statement";
const PAYLOAD_CONTEXT: &str = "rotation_payload";

/// Every reason a rotation payload fails to decode or to agree with its envelope.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RotationPayloadError {
    /// A field was absent, mistyped, or the wrong width.
    #[error("rotation payload field: {0}")]
    Field(#[from] EnvelopeError),
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// `protocol_version` was not 1.
    #[error("rotation_statement.protocol_version {version} is not 1")]
    UnsupportedProtocolVersion {
        /// The version actually present.
        version: u64,
    },
    /// `scope` differed from the envelope scope.
    #[error("rotation_statement.scope differs from the envelope scope")]
    ScopeMismatch,
    /// `branch_id` differed from the envelope branch.
    #[error("rotation_statement.branch_id differs from the envelope branch_id")]
    BranchMismatch,
    /// The envelope did not name exactly one parent (§3 rule 1).
    #[error("a rotation must have exactly one parent, found {count}")]
    NotExactlyOneParent {
        /// Parents actually present.
        count: usize,
    },
    /// `parent_op_id` differed from the sole envelope parent.
    #[error("rotation_statement.parent_op_id differs from the sole envelope parent")]
    ParentMismatch,
    /// `scope_epoch` differed from the envelope capability epoch.
    #[error("rotation_statement.scope_epoch differs from the envelope capability epoch")]
    ScopeEpochMismatch,
    /// `predecessor_did` differed from the envelope author DID.
    #[error("rotation_statement.predecessor_did differs from the envelope author DID")]
    PredecessorDidMismatch,
    /// `predecessor_public_key` differed from the envelope author public key.
    #[error("rotation_statement.predecessor_public_key differs from the envelope author key")]
    PredecessorPublicKeyMismatch,
}

/// The §3.1 nine-key rotation statement: exactly the bytes the successor proof covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotationStatement {
    /// MUST be [`ROTATION_PROTOCOL_VERSION`].
    pub protocol_version: u64,
    /// Byte-for-byte the envelope scope.
    pub scope: Scope,
    /// Byte-for-byte the envelope branch identifier.
    pub branch_id: Identifier32,
    /// The sole envelope parent.
    pub parent_op_id: Hash32,
    /// The envelope capability epoch.
    pub scope_epoch: u64,
    /// The envelope author DID.
    pub predecessor_did: String,
    /// The envelope author public key.
    pub predecessor_public_key: [u8; 32],
    /// Canonical Ed25519 `did:key` of the successor.
    pub successor_did: String,
    /// Public key the successor DID encodes.
    pub successor_public_key: [u8; 32],
}

impl RotationStatement {
    /// Encodes the §3.1 nine-key map.
    pub fn to_cbor(&self) -> Result<CborValue, RotationPayloadError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.protocol_version)),
            (CborValue::Uint(1), self.scope.to_cbor()?),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.branch_id.0.to_vec()),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.parent_op_id.0.to_vec()),
            ),
            (CborValue::Uint(4), CborValue::Uint(self.scope_epoch)),
            (CborValue::Uint(5), CborValue::text(&self.predecessor_did)),
            (
                CborValue::Uint(6),
                CborValue::Bytes(self.predecessor_public_key.to_vec()),
            ),
            (CborValue::Uint(7), CborValue::text(&self.successor_did)),
            (
                CborValue::Uint(8),
                CborValue::Bytes(self.successor_public_key.to_vec()),
            ),
        ]))
    }

    /// Decodes the §3.1 nine-key map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, RotationPayloadError> {
        require_numeric_keys(value, 9, STATEMENT_CONTEXT)?;
        Ok(Self {
            protocol_version: unsigned_at(value, 0, STATEMENT_CONTEXT)?,
            scope: Scope::from_cbor(entry(value, 1, STATEMENT_CONTEXT)?)?,
            branch_id: Identifier32(bytes_at::<32>(value, 2, STATEMENT_CONTEXT)?),
            parent_op_id: Hash32(bytes_at::<32>(value, 3, STATEMENT_CONTEXT)?),
            scope_epoch: unsigned_at(value, 4, STATEMENT_CONTEXT)?,
            predecessor_did: text_at(value, 5, STATEMENT_CONTEXT)?,
            predecessor_public_key: bytes_at::<32>(value, 6, STATEMENT_CONTEXT)?,
            successor_did: text_at(value, 7, STATEMENT_CONTEXT)?,
            successor_public_key: bytes_at::<32>(value, 8, STATEMENT_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes; these are the bytes the §3.2 proof signs.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, RotationPayloadError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, RotationPayloadError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }
}

/// The §3 two-key rotation payload: the statement plus the successor's possession proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotationPayload {
    /// The §3.1 statement.
    pub statement: RotationStatement,
    /// Ed25519 signature described by §3.2.
    pub successor_signature: [u8; SIGNATURE_LENGTH],
}

impl RotationPayload {
    /// Encodes the §3 two-key map.
    pub fn to_cbor(&self) -> Result<CborValue, RotationPayloadError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), self.statement.to_cbor()?),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.successor_signature.to_vec()),
            ),
        ]))
    }

    /// Decodes the §3 two-key map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, RotationPayloadError> {
        require_numeric_keys(value, 2, PAYLOAD_CONTEXT)?;
        Ok(Self {
            statement: RotationStatement::from_cbor(entry(value, 0, PAYLOAD_CONTEXT)?)?,
            successor_signature: bytes_at::<SIGNATURE_LENGTH>(value, 1, PAYLOAD_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, RotationPayloadError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, RotationPayloadError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }

    /// Checks every §3.1 equality against the enclosing envelope of a RECEIVED payload.
    pub fn validate_against_envelope(
        &self,
        envelope: &UnsignedEnvelope,
    ) -> Result<(), RotationPayloadError> {
        let statement = &self.statement;
        if statement.protocol_version != ROTATION_PROTOCOL_VERSION {
            return Err(RotationPayloadError::UnsupportedProtocolVersion {
                version: statement.protocol_version,
            });
        }
        if statement.scope != envelope.scope {
            return Err(RotationPayloadError::ScopeMismatch);
        }
        if statement.branch_id != envelope.branch_id {
            return Err(RotationPayloadError::BranchMismatch);
        }
        let sole_parent = match envelope.parents.as_slice() {
            [only] => *only,
            other => return Err(RotationPayloadError::NotExactlyOneParent { count: other.len() }),
        };
        if statement.parent_op_id != sole_parent {
            return Err(RotationPayloadError::ParentMismatch);
        }
        if statement.scope_epoch != envelope.capability.scope_epoch {
            return Err(RotationPayloadError::ScopeEpochMismatch);
        }
        if statement.predecessor_did != envelope.author.did {
            return Err(RotationPayloadError::PredecessorDidMismatch);
        }
        if statement.predecessor_public_key != envelope.author.public_key {
            return Err(RotationPayloadError::PredecessorPublicKeyMismatch);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::rotation_proof::sign_successor_proof;
    use crate::author_key::test_support::{
        hash, identifier, intent_envelope, public_key, schemas, signing_key, verse_scope,
    };
    use crate::cbor::encode_canonical;
    use crate::did_key::public_key_to_did;
    use crate::envelope::Hlc;

    fn statement() -> RotationStatement {
        RotationStatement {
            protocol_version: ROTATION_PROTOCOL_VERSION,
            scope: verse_scope(),
            branch_id: identifier(0x55),
            parent_op_id: hash(0x66),
            scope_epoch: 7,
            predecessor_did: public_key_to_did(&public_key(1)),
            predecessor_public_key: public_key(1),
            successor_did: public_key_to_did(&public_key(2)),
            successor_public_key: public_key(2),
        }
    }

    fn payload() -> RotationPayload {
        let statement = statement();
        let successor_signature =
            sign_successor_proof(&signing_key(2), &statement).expect("sign proof");
        RotationPayload {
            statement,
            successor_signature,
        }
    }

    fn envelope() -> UnsignedEnvelope {
        intent_envelope(
            public_key(1),
            verse_scope(),
            identifier(0x55),
            vec![hash(0x66)],
            7,
            schemas().rotation,
            Hlc::new(1_700_000_000_000, 4),
        )
    }

    #[test]
    fn rotation_payload_round_trips_canonical_bytes_byte_exactly() {
        let payload = payload();
        let bytes = payload.encode_canonical().expect("encode");
        let decoded = RotationPayload::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, payload);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
    }

    #[test]
    fn rotation_payload_rejects_non_canonical_input() {
        let payload = payload();
        let canonical = payload.encode_canonical().expect("encode");

        let mut trailing = canonical.clone();
        trailing.push(0x00);
        assert!(matches!(
            RotationPayload::decode_canonical(&trailing),
            Err(RotationPayloadError::Cbor(CborError::TrailingBytes { .. }))
        ));

        // The same two entries emitted with key 1 before key 0.
        let mut unsorted = vec![0xa2u8];
        unsorted.push(0x01);
        unsorted.extend(encode_canonical(&CborValue::Bytes(
            payload.successor_signature.to_vec(),
        )));
        unsorted.push(0x00);
        unsorted.extend(
            payload
                .statement
                .encode_canonical()
                .expect("statement bytes"),
        );
        assert_ne!(unsorted, canonical);
        assert!(matches!(
            RotationPayload::decode_canonical(&unsorted),
            Err(RotationPayloadError::Cbor(
                CborError::UnsortedMapKeys { .. }
            ))
        ));
    }

    #[test]
    fn rotation_payload_rejects_an_unlisted_or_missing_key() {
        let payload = payload();
        let mut entries = match payload.to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries.push((CborValue::Uint(2), CborValue::Uint(0)));
        assert_eq!(
            RotationPayload::from_cbor(&CborValue::Map(entries)),
            Err(RotationPayloadError::Field(
                EnvelopeError::UnexpectedMapKeys {
                    context: PAYLOAD_CONTEXT,
                    expected_key_count: 2,
                }
            ))
        );

        let mut statement_entries = match payload.statement.to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        statement_entries.pop();
        assert_eq!(
            RotationStatement::from_cbor(&CborValue::Map(statement_entries)),
            Err(RotationPayloadError::Field(
                EnvelopeError::UnexpectedMapKeys {
                    context: STATEMENT_CONTEXT,
                    expected_key_count: 9,
                }
            ))
        );
    }

    #[test]
    fn spec2_case_02_every_statement_field_must_equal_its_envelope_field() {
        let envelope = envelope();
        assert_eq!(payload().validate_against_envelope(&envelope), Ok(()));

        let mut wrong_version = payload();
        wrong_version.statement.protocol_version = 2;
        assert_eq!(
            wrong_version.validate_against_envelope(&envelope),
            Err(RotationPayloadError::UnsupportedProtocolVersion { version: 2 })
        );

        let mut wrong_scope = payload();
        wrong_scope.statement.scope = crate::envelope::Scope::verse_wide(identifier(0x99));
        assert_eq!(
            wrong_scope.validate_against_envelope(&envelope),
            Err(RotationPayloadError::ScopeMismatch)
        );

        let mut wrong_branch = payload();
        wrong_branch.statement.branch_id = identifier(0x56);
        assert_eq!(
            wrong_branch.validate_against_envelope(&envelope),
            Err(RotationPayloadError::BranchMismatch)
        );

        let mut wrong_parent = payload();
        wrong_parent.statement.parent_op_id = hash(0x67);
        assert_eq!(
            wrong_parent.validate_against_envelope(&envelope),
            Err(RotationPayloadError::ParentMismatch)
        );

        let mut wrong_epoch = payload();
        wrong_epoch.statement.scope_epoch = 8;
        assert_eq!(
            wrong_epoch.validate_against_envelope(&envelope),
            Err(RotationPayloadError::ScopeEpochMismatch)
        );

        let mut wrong_did = payload();
        wrong_did.statement.predecessor_did = public_key_to_did(&public_key(9));
        assert_eq!(
            wrong_did.validate_against_envelope(&envelope),
            Err(RotationPayloadError::PredecessorDidMismatch)
        );

        let mut wrong_key = payload();
        wrong_key.statement.predecessor_public_key = public_key(9);
        assert_eq!(
            wrong_key.validate_against_envelope(&envelope),
            Err(RotationPayloadError::PredecessorPublicKeyMismatch)
        );
    }

    #[test]
    fn a_rotation_must_name_exactly_one_parent() {
        let mut parentless = envelope();
        parentless.parents = Vec::new();
        assert_eq!(
            payload().validate_against_envelope(&parentless),
            Err(RotationPayloadError::NotExactlyOneParent { count: 0 })
        );

        let mut two_parents = envelope();
        two_parents.parents = vec![hash(0x66), hash(0x68)];
        assert_eq!(
            payload().validate_against_envelope(&two_parents),
            Err(RotationPayloadError::NotExactlyOneParent { count: 2 })
        );
    }
}
