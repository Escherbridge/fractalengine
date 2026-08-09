//! The §6.1 disavow rescind: payload grammar and the strictly-higher-authority rule.

use thiserror::Error;

use super::{bytes_at, require_numeric_keys, u16_at};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{EnvelopeError, Hash32};

const RESCIND_CONTEXT: &str = "disavow_rescind_payload";

/// Resolved authority of one principal at one scope and epoch.
///
/// This mirrors the ordering of `fe_policy::RoleLevel` without depending on `fe-policy`; see
/// `src/AGENTS.md` §Forbidden dependencies. The caller resolves the level and passes it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AuthorityLevel {
    /// No authority at this scope.
    #[default]
    None,
    /// Read-only authority.
    Viewer,
    /// Authoring authority.
    Editor,
    /// Administrative authority; the minimum a §6 disavow requires.
    Manager,
    /// Final authority; an Owner-issued disavow can never be rescinded.
    Owner,
}

/// Every reason a rescind payload fails to decode.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RescindError {
    /// A field was absent, mistyped, or the wrong width.
    #[error("rescind payload field: {0}")]
    Field(#[from] EnvelopeError),
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
}

/// Every reason a rescind is refused on authority grounds (§6.1 rule 2).
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RescindAuthorityError {
    /// The original disavow was Owner-issued, so it is final.
    #[error("an Owner-issued disavow is final and rejects every rescind")]
    OwnerIssuedDisavowIsFinal,
    /// The rescinder did not hold strictly higher authority than the original issuer.
    #[error("rescind authority {rescinder:?} is not strictly higher than issuer {issuer:?}")]
    AuthorityNotStrictlyHigher {
        /// Authority the rescinder holds.
        rescinder: AuthorityLevel,
        /// Authority the original disavow issuer held.
        issuer: AuthorityLevel,
    },
}

/// The §6.1 two-field rescind payload; it names one disavow and cannot alter its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RescindPayload {
    /// The single admitted disavow whose projection effect is withdrawn.
    pub disavow_op_id: Hash32,
    /// Registered reason code.
    pub reason_code: u16,
}

impl RescindPayload {
    /// Encodes the two-key map; the key numbering is provisional, see `author_key/AGENTS.md`.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.disavow_op_id.0.to_vec()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Uint(u64::from(self.reason_code)),
            ),
        ])
    }

    /// Decodes the two-key map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, RescindError> {
        require_numeric_keys(value, 2, RESCIND_CONTEXT)?;
        Ok(Self {
            disavow_op_id: Hash32(bytes_at::<32>(value, 0, RESCIND_CONTEXT)?),
            reason_code: u16_at(value, 1, RESCIND_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, RescindError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, RescindError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }
}

/// §6.1 rule 2: an Owner-issued disavow rejects every rescind, and otherwise the rescinder
/// must hold strictly higher authority than the original issuer.
pub fn validate_rescind_authority(
    rescind_issuer_role: AuthorityLevel,
    original_issuer_role: AuthorityLevel,
) -> Result<(), RescindAuthorityError> {
    if original_issuer_role == AuthorityLevel::Owner {
        return Err(RescindAuthorityError::OwnerIssuedDisavowIsFinal);
    }
    if rescind_issuer_role <= original_issuer_role {
        return Err(RescindAuthorityError::AuthorityNotStrictlyHigher {
            rescinder: rescind_issuer_role,
            issuer: original_issuer_role,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author_key::test_support::hash;

    #[test]
    fn rescind_payload_round_trips_canonical_bytes_byte_exactly() {
        let payload = RescindPayload {
            disavow_op_id: hash(0x0a),
            reason_code: 11,
        };
        let bytes = payload.encode_canonical().expect("encode");
        let decoded = RescindPayload::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, payload);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
    }

    #[test]
    fn rescind_payload_rejects_non_canonical_input() {
        let payload = RescindPayload {
            disavow_op_id: hash(0x0a),
            reason_code: 11,
        };
        let mut trailing = payload.encode_canonical().expect("encode");
        trailing.push(0x00);
        assert!(matches!(
            RescindPayload::decode_canonical(&trailing),
            Err(RescindError::Cbor(CborError::TrailingBytes { .. }))
        ));
    }

    #[test]
    fn authority_levels_order_from_none_to_owner() {
        assert!(AuthorityLevel::None < AuthorityLevel::Viewer);
        assert!(AuthorityLevel::Viewer < AuthorityLevel::Editor);
        assert!(AuthorityLevel::Editor < AuthorityLevel::Manager);
        assert!(AuthorityLevel::Manager < AuthorityLevel::Owner);
        assert_eq!(AuthorityLevel::default(), AuthorityLevel::None);
    }

    #[test]
    fn spec2_case_14_an_owner_issued_disavow_rejects_every_rescind() {
        for rescinder in [
            AuthorityLevel::None,
            AuthorityLevel::Viewer,
            AuthorityLevel::Editor,
            AuthorityLevel::Manager,
            AuthorityLevel::Owner,
        ] {
            assert_eq!(
                validate_rescind_authority(rescinder, AuthorityLevel::Owner),
                Err(RescindAuthorityError::OwnerIssuedDisavowIsFinal)
            );
        }
    }

    #[test]
    fn spec2_case_14_equal_or_lower_authority_cannot_rescind() {
        assert_eq!(
            validate_rescind_authority(AuthorityLevel::Manager, AuthorityLevel::Manager),
            Err(RescindAuthorityError::AuthorityNotStrictlyHigher {
                rescinder: AuthorityLevel::Manager,
                issuer: AuthorityLevel::Manager,
            })
        );
        assert_eq!(
            validate_rescind_authority(AuthorityLevel::Editor, AuthorityLevel::Manager),
            Err(RescindAuthorityError::AuthorityNotStrictlyHigher {
                rescinder: AuthorityLevel::Editor,
                issuer: AuthorityLevel::Manager,
            })
        );
        assert_eq!(
            validate_rescind_authority(AuthorityLevel::Owner, AuthorityLevel::Manager),
            Ok(())
        );
    }
}
