//! The §9 D-CL18 Owner-countersigned continuity grant: attribution and display only.
//!
//! **NOT WIRE-FINAL.** SPEC-2 §9.2 describes this payload in prose and gives no normative
//! integer-key table, so the seven statement fields below carry provisional key numbers
//! assigned by this crate under D-CL24. They are recorded in `author_key/AGENTS.md`
//! §"Provisional wire numbering" and await owner ratification. No cross-implementation
//! interop is claimed for these bytes.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use thiserror::Error;

use super::{bytes_at, entry, require_numeric_keys, text_at, u16_at, unsigned_at};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{EnvelopeError, Hash32, Scope, UnsignedEnvelope, SIGNATURE_LENGTH};
use crate::signing::{sign_domain, verify_author_binding, verify_domain, SigningError};

/// NUL-terminated ASCII domain separator for the §9.2 new-principal countersignature.
pub const CONTINUITY_GRANT_DOMAIN: &[u8] = b"fe-owner-continuity-grant-v1\0";

/// `protocol_version` every v1 continuity-grant statement carries.
pub const CONTINUITY_GRANT_PROTOCOL_VERSION: u64 = 1;

const STATEMENT_CONTEXT: &str = "continuity_grant_statement";
const PAYLOAD_CONTEXT: &str = "continuity_grant_payload";

/// Every reason a continuity grant fails to decode or to satisfy §9.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ContinuityGrantError {
    /// A field was absent, mistyped, or the wrong width.
    #[error("continuity grant field: {0}")]
    Field(#[from] EnvelopeError),
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// `protocol_version` was not 1.
    #[error("continuity grant protocol_version {version} is not 1")]
    UnsupportedProtocolVersion {
        /// The version actually present.
        version: u64,
    },
    /// The lost principal DID did not bind to its public key.
    #[error("lost principal DID does not bind to its public key: {0}")]
    LostPrincipalBinding(SigningError),
    /// The new principal DID did not bind to its public key.
    #[error("new principal DID does not bind to its public key: {0}")]
    NewPrincipalBinding(SigningError),
    /// The two principals were the same key.
    #[error("a continuity grant must link two different principals")]
    PrincipalsAreIdentical,
    /// The grant scope was narrower than a whole verse (§9.2 binds a verse scope).
    #[error("a continuity grant scope must be verse-wide")]
    ScopeIsNotVerseWide,
    /// The grant scope differed from the envelope scope.
    #[error("continuity grant scope differs from the envelope scope")]
    ScopeMismatch,
    /// The new principal's countersignature did not verify.
    #[error("new principal countersignature failed: {0}")]
    CountersignatureFailed(SigningError),
}

/// The §9.2 statement: both principals, the affected verse scope, and a reason code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuityGrantStatement {
    /// MUST be [`CONTINUITY_GRANT_PROTOCOL_VERSION`].
    pub protocol_version: u64,
    /// Verse-wide scope the attribution link applies to.
    pub verse_scope: Scope,
    /// Canonical DID of the lost predecessor principal.
    pub lost_principal_did: String,
    /// Public key the lost principal DID encodes.
    pub lost_principal_public_key: [u8; 32],
    /// Canonical DID of the new principal.
    pub new_principal_did: String,
    /// Public key the new principal DID encodes.
    pub new_principal_public_key: [u8; 32],
    /// Registered reason code.
    pub reason_code: u16,
}

impl ContinuityGrantStatement {
    /// Encodes the seven-key statement map.
    pub fn to_cbor(&self) -> Result<CborValue, ContinuityGrantError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.protocol_version)),
            (CborValue::Uint(1), self.verse_scope.to_cbor()?),
            (
                CborValue::Uint(2),
                CborValue::text(&self.lost_principal_did),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.lost_principal_public_key.to_vec()),
            ),
            (CborValue::Uint(4), CborValue::text(&self.new_principal_did)),
            (
                CborValue::Uint(5),
                CborValue::Bytes(self.new_principal_public_key.to_vec()),
            ),
            (
                CborValue::Uint(6),
                CborValue::Uint(u64::from(self.reason_code)),
            ),
        ]))
    }

    /// Decodes the seven-key statement map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, ContinuityGrantError> {
        require_numeric_keys(value, 7, STATEMENT_CONTEXT)?;
        Ok(Self {
            protocol_version: unsigned_at(value, 0, STATEMENT_CONTEXT)?,
            verse_scope: Scope::from_cbor(entry(value, 1, STATEMENT_CONTEXT)?)?,
            lost_principal_did: text_at(value, 2, STATEMENT_CONTEXT)?,
            lost_principal_public_key: bytes_at::<32>(value, 3, STATEMENT_CONTEXT)?,
            new_principal_did: text_at(value, 4, STATEMENT_CONTEXT)?,
            new_principal_public_key: bytes_at::<32>(value, 5, STATEMENT_CONTEXT)?,
            reason_code: u16_at(value, 6, STATEMENT_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes; these are the bytes the new principal countersigns.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ContinuityGrantError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ContinuityGrantError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }
}

/// The §9.2 dual-signed grant: the Owner signs the envelope, the new principal signs this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuityGrantPayload {
    /// The statement both signatures bind to.
    pub statement: ContinuityGrantStatement,
    /// Ed25519 signature by `new_principal_public_key` under [`CONTINUITY_GRANT_DOMAIN`].
    pub new_principal_signature: [u8; SIGNATURE_LENGTH],
}

impl ContinuityGrantPayload {
    /// Encodes the two-key payload map.
    pub fn to_cbor(&self) -> Result<CborValue, ContinuityGrantError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), self.statement.to_cbor()?),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.new_principal_signature.to_vec()),
            ),
        ]))
    }

    /// Decodes the two-key payload map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, ContinuityGrantError> {
        require_numeric_keys(value, 2, PAYLOAD_CONTEXT)?;
        Ok(Self {
            statement: ContinuityGrantStatement::from_cbor(entry(value, 0, PAYLOAD_CONTEXT)?)?,
            new_principal_signature: bytes_at::<SIGNATURE_LENGTH>(value, 1, PAYLOAD_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ContinuityGrantError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ContinuityGrantError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }

    /// Verifies both DID bindings, the distinctness rule, and the new-principal proof.
    pub fn verify(&self) -> Result<(), ContinuityGrantError> {
        let statement = &self.statement;
        if statement.protocol_version != CONTINUITY_GRANT_PROTOCOL_VERSION {
            return Err(ContinuityGrantError::UnsupportedProtocolVersion {
                version: statement.protocol_version,
            });
        }
        verify_author_binding(
            &statement.lost_principal_did,
            &statement.lost_principal_public_key,
        )
        .map_err(ContinuityGrantError::LostPrincipalBinding)?;
        verify_author_binding(
            &statement.new_principal_did,
            &statement.new_principal_public_key,
        )
        .map_err(ContinuityGrantError::NewPrincipalBinding)?;
        if statement.lost_principal_public_key == statement.new_principal_public_key {
            return Err(ContinuityGrantError::PrincipalsAreIdentical);
        }
        if statement.verse_scope.petal_id().is_some()
            || statement.verse_scope.resource_id().is_some()
        {
            return Err(ContinuityGrantError::ScopeIsNotVerseWide);
        }
        verify_domain(
            &statement.new_principal_public_key,
            CONTINUITY_GRANT_DOMAIN,
            &statement.encode_canonical()?,
            &self.new_principal_signature,
        )
        .map_err(ContinuityGrantError::CountersignatureFailed)
    }

    /// Verifies the payload and checks it against the enclosing envelope of a RECEIVED grant.
    pub fn validate_against_envelope(
        &self,
        envelope: &UnsignedEnvelope,
    ) -> Result<(), ContinuityGrantError> {
        self.verify()?;
        if self.statement.verse_scope != envelope.scope {
            return Err(ContinuityGrantError::ScopeMismatch);
        }
        Ok(())
    }
}

/// Signs the §9.2 preimage `ASCII("fe-owner-continuity-grant-v1") || 0x00 || statement`.
pub fn sign_continuity_grant(
    new_principal_signing_key: &SigningKey,
    statement: &ContinuityGrantStatement,
) -> Result<[u8; SIGNATURE_LENGTH], ContinuityGrantError> {
    Ok(sign_domain(
        new_principal_signing_key,
        CONTINUITY_GRANT_DOMAIN,
        &statement.encode_canonical()?,
    ))
}

/// A materialized §9.2 link. It is display metadata: it grants no authority, no lineage, and
/// no disavow override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributionLink {
    /// Operation that carried the grant.
    pub grant_op_id: Hash32,
    /// Verse the link applies to.
    pub verse_scope: Scope,
    /// The lost predecessor principal.
    pub lost_principal_public_key: [u8; 32],
    /// The new principal.
    pub new_principal_public_key: [u8; 32],
}

/// Attribution links, kept deliberately separate from lineage and disavow state.
#[derive(Clone, Debug, Default)]
pub struct AttributionIndex {
    links: BTreeMap<Hash32, AttributionLink>,
}

impl AttributionIndex {
    /// Builds an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one attribution link.
    pub fn record(&mut self, link: AttributionLink) -> Option<AttributionLink> {
        self.links.insert(link.grant_op_id, link)
    }

    /// Links naming `public_key` as either principal, in grant `op_id` order.
    pub fn links_for(&self, public_key: &[u8; 32]) -> Vec<&AttributionLink> {
        self.links
            .values()
            .filter(|link| {
                link.lost_principal_public_key == *public_key
                    || link.new_principal_public_key == *public_key
            })
            .collect()
    }

    /// Number of recorded links.
    pub fn len(&self) -> usize {
        self.links.len()
    }

    /// Reports whether no link is recorded.
    pub fn is_empty(&self) -> bool {
        self.links.is_empty()
    }

    /// Removes a link, for the §3.4 equivocation rule that materializes neither candidate.
    pub fn retract(&mut self, grant_op_id: Hash32) -> bool {
        self.links.remove(&grant_op_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::test_support::{
        hash, identifier, petal_scope, public_key, signing_key, verse_scope,
    };
    use crate::did_key::public_key_to_did;
    use crate::signing::SIGNATURE_DOMAIN;

    fn statement(lost: u8, new: u8, scope: Scope) -> ContinuityGrantStatement {
        ContinuityGrantStatement {
            protocol_version: CONTINUITY_GRANT_PROTOCOL_VERSION,
            verse_scope: scope,
            lost_principal_did: public_key_to_did(&public_key(lost)),
            lost_principal_public_key: public_key(lost),
            new_principal_did: public_key_to_did(&public_key(new)),
            new_principal_public_key: public_key(new),
            reason_code: 2,
        }
    }

    fn grant(lost: u8, new: u8, scope: Scope) -> ContinuityGrantPayload {
        let statement = statement(lost, new, scope);
        let new_principal_signature =
            sign_continuity_grant(&signing_key(new), &statement).expect("countersign");
        ContinuityGrantPayload {
            statement,
            new_principal_signature,
        }
    }

    #[test]
    fn the_grant_domain_is_nul_terminated_and_distinct_from_every_other_domain() {
        assert_eq!(CONTINUITY_GRANT_DOMAIN, b"fe-owner-continuity-grant-v1\0");
        assert!(CONTINUITY_GRANT_DOMAIN.ends_with(&[0]));
        assert_ne!(CONTINUITY_GRANT_DOMAIN, SIGNATURE_DOMAIN);
        assert_ne!(
            CONTINUITY_GRANT_DOMAIN,
            crate::author_key::rotation_proof::ROTATION_PROOF_DOMAIN
        );
    }

    #[test]
    fn continuity_grant_round_trips_canonical_bytes_byte_exactly() {
        let grant = grant(1, 2, verse_scope());
        let bytes = grant.encode_canonical().expect("encode");
        let decoded = ContinuityGrantPayload::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, grant);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
    }

    #[test]
    fn continuity_grant_rejects_non_canonical_input() {
        let mut trailing = grant(1, 2, verse_scope())
            .encode_canonical()
            .expect("encode");
        trailing.push(0x00);
        assert!(matches!(
            ContinuityGrantPayload::decode_canonical(&trailing),
            Err(ContinuityGrantError::Cbor(CborError::TrailingBytes { .. }))
        ));
    }

    #[test]
    fn a_valid_grant_verifies_and_a_countersignature_by_another_key_does_not() {
        assert_eq!(grant(1, 2, verse_scope()).verify(), Ok(()));

        let statement = statement(1, 2, verse_scope());
        let impostor = ContinuityGrantPayload {
            new_principal_signature: sign_continuity_grant(&signing_key(3), &statement)
                .expect("countersign"),
            statement,
        };
        assert_eq!(
            impostor.verify(),
            Err(ContinuityGrantError::CountersignatureFailed(
                SigningError::SignatureVerificationFailed
            ))
        );
    }

    #[test]
    fn a_grant_must_link_two_different_verse_wide_principals() {
        assert_eq!(
            grant(1, 1, verse_scope()).verify(),
            Err(ContinuityGrantError::PrincipalsAreIdentical)
        );
        assert_eq!(
            grant(1, 2, petal_scope()).verify(),
            Err(ContinuityGrantError::ScopeIsNotVerseWide)
        );

        let mut mismatched = grant(1, 2, verse_scope());
        mismatched.statement.new_principal_did = public_key_to_did(&public_key(9));
        assert_eq!(
            mismatched.verify(),
            Err(ContinuityGrantError::NewPrincipalBinding(
                SigningError::AuthorBindingMismatch
            ))
        );
    }

    #[test]
    fn a_grant_scope_must_equal_its_envelope_scope() {
        use crate::author_key::test_support::{intent_envelope, schemas};
        use crate::envelope::Hlc;

        let envelope = intent_envelope(
            public_key(8),
            verse_scope(),
            identifier(0x55),
            vec![hash(0x01)],
            0,
            schemas().continuity_grant,
            Hlc::new(60, 0),
        );
        assert_eq!(
            grant(1, 2, verse_scope()).validate_against_envelope(&envelope),
            Ok(())
        );

        let other_verse = Scope::verse_wide(identifier(0x21));
        assert_eq!(
            grant(1, 2, other_verse).validate_against_envelope(&envelope),
            Err(ContinuityGrantError::ScopeMismatch)
        );
    }

    #[test]
    fn the_attribution_index_records_and_retracts_links() {
        let mut index = AttributionIndex::new();
        assert!(index.is_empty());
        let link = AttributionLink {
            grant_op_id: hash(0x0e),
            verse_scope: verse_scope(),
            lost_principal_public_key: public_key(1),
            new_principal_public_key: public_key(2),
        };
        assert_eq!(index.record(link), None);
        assert_eq!(index.len(), 1);
        assert_eq!(index.links_for(&public_key(1)), vec![&link]);
        assert_eq!(index.links_for(&public_key(2)), vec![&link]);
        assert!(index.links_for(&public_key(3)).is_empty());
        assert!(index.retract(hash(0x0e)));
        assert!(index.is_empty());
    }
}
