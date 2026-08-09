//! The fifteen-key capability certificate and its `fe-capability-cert-v1` signature (§2.2).

use ed25519_dalek::SigningKey;

use crate::cbor::{decode_canonical as decode_cbor, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Scope, SIGNATURE_LENGTH};
use crate::signing::{sign_domain, verify_author_binding, verify_domain};

use super::caveats::Caveats;
use super::verbs::{ObjectClassSet, VerbSet};
use super::{
    bytes_at, optional_hash_at, optional_hash_to_cbor, principal_at, require_numeric_keys,
    scope_at, u8_at, unsigned_at, CapabilityError, Principal,
};

/// The only capability grammar version v1 admits.
pub const CAPABILITY_VERSION: u64 = 1;

/// NUL-terminated ASCII D-CL3 domain separator for capability certificates (§2.2).
pub const CERTIFICATE_SIGNATURE_DOMAIN: &[u8] = b"fe-capability-cert-v1\0";

/// The ratified protocol bound on a single certificate's lifetime (§4.5): 24 hours.
pub const MAXIMUM_CERTIFICATE_LIFETIME_MS: u64 = 24 * 60 * 60 * 1000;

/// BLAKE3 identifier of a complete certificate; derived, never serialized inside itself.
pub type CertificateId = Hash32;

const UNSIGNED_CONTEXT: &str = "unsigned_certificate";
const COMPLETE_CONTEXT: &str = "certificate";

/// Keys 0 through 13 of §2.2: exactly the bytes the certificate signature covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsignedCertificate {
    /// MUST be [`CAPABILITY_VERSION`].
    pub capability_version: u64,
    /// Signing principal.
    pub issuer: Principal,
    /// Principal allowed to exercise or attenuate this grant.
    pub audience: Principal,
    /// Parent certificate identifier; `None` only for a root certificate.
    pub parent_certificate_id: Option<CertificateId>,
    /// Admitted authority record anchoring the root issuer; delegates copy it exactly.
    pub issuer_authority_id: Hash32,
    /// Maximum target scope for this certificate.
    pub grant_scope: Scope,
    /// Scope whose epoch applies to the chain; it MUST contain `grant_scope`.
    pub epoch_scope: Scope,
    /// Epoch current when the root certificate was issued; unchanged across the chain.
    pub scope_epoch: u64,
    /// Non-empty subset of the §3.1 verb bits.
    pub verbs: VerbSet,
    /// Non-empty subset of the §3.2 object-class bits.
    pub object_classes: ObjectClassSet,
    /// Inclusive Unix millisecond start of validity.
    pub not_before_ms: u64,
    /// Exclusive Unix millisecond end of validity.
    pub not_after_ms: u64,
    /// Number of further certificates the audience may issue.
    pub delegation_depth: u8,
    /// The §2.3 caveat map.
    pub caveats: Caveats,
}

impl UnsignedCertificate {
    /// Enforces every §2.2 and §2.3 rule a single certificate can check on its own.
    ///
    /// The 24-hour bound is deliberately not checked here; see [`Self::check_lifetime`].
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.capability_version != CAPABILITY_VERSION {
            return Err(CapabilityError::UnsupportedCapabilityVersion {
                version: self.capability_version,
            });
        }
        self.grant_scope.validate()?;
        self.epoch_scope.validate()?;
        if !self.epoch_scope.contains(&self.grant_scope) {
            return Err(CapabilityError::EpochScopeDoesNotContainGrantScope);
        }
        if self.not_before_ms >= self.not_after_ms {
            return Err(CapabilityError::EmptyValidityWindow {
                not_before_ms: self.not_before_ms,
                not_after_ms: self.not_after_ms,
            });
        }
        self.caveats.validate(&self.grant_scope)
    }

    /// Declared validity window in milliseconds.
    pub const fn lifetime_ms(&self) -> u64 {
        self.not_after_ms.saturating_sub(self.not_before_ms)
    }

    /// Rejects a validity window longer than `maximum_ms` (§2.4 step 7, §4.5).
    pub fn check_lifetime(&self, maximum_ms: u64) -> Result<(), CapabilityError> {
        let lifetime_ms = self.lifetime_ms();
        if lifetime_ms > maximum_ms {
            return Err(CapabilityError::LifetimeExceedsMaximum {
                lifetime_ms,
                maximum_ms,
            });
        }
        Ok(())
    }

    /// Encodes the fourteen-key unsigned map.
    pub fn to_cbor(&self) -> Result<CborValue, CapabilityError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.capability_version)),
            (CborValue::Uint(1), self.issuer.to_cbor()),
            (CborValue::Uint(2), self.audience.to_cbor()),
            (
                CborValue::Uint(3),
                optional_hash_to_cbor(self.parent_certificate_id),
            ),
            (
                CborValue::Uint(4),
                CborValue::Bytes(self.issuer_authority_id.0.to_vec()),
            ),
            (CborValue::Uint(5), self.grant_scope.to_cbor()?),
            (CborValue::Uint(6), self.epoch_scope.to_cbor()?),
            (CborValue::Uint(7), CborValue::Uint(self.scope_epoch)),
            (
                CborValue::Uint(8),
                CborValue::Uint(u64::from(self.verbs.bits())),
            ),
            (
                CborValue::Uint(9),
                CborValue::Uint(u64::from(self.object_classes.bits())),
            ),
            (CborValue::Uint(10), CborValue::Uint(self.not_before_ms)),
            (CborValue::Uint(11), CborValue::Uint(self.not_after_ms)),
            (
                CborValue::Uint(12),
                CborValue::Uint(u64::from(self.delegation_depth)),
            ),
            (
                CborValue::Uint(13),
                self.caveats.to_cbor(&self.grant_scope)?,
            ),
        ]))
    }

    /// Decodes the fourteen-key unsigned map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CapabilityError> {
        require_numeric_keys(value, 14, UNSIGNED_CONTEXT)?;
        parse_unsigned_fields(value, UNSIGNED_CONTEXT)
    }

    /// Encodes to the canonical bytes the §2.2 signature preimage wraps.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CapabilityError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CapabilityError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// Signs this certificate, refusing a window longer than the 24-hour protocol bound.
    pub fn sign(&self, signing_key: &SigningKey) -> Result<Certificate, CapabilityError> {
        self.check_lifetime(MAXIMUM_CERTIFICATE_LIFETIME_MS)?;
        let unsigned_bytes = self.encode_canonical()?;
        Ok(Certificate {
            unsigned: self.clone(),
            signature: sign_domain(signing_key, CERTIFICATE_SIGNATURE_DOMAIN, &unsigned_bytes),
        })
    }
}

/// The complete fifteen-key §2.2 certificate: the exact artifact a chain carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// Keys 0 through 13.
    pub unsigned: UnsignedCertificate,
    /// Key 14: the Ed25519 signature over the §2.2 preimage.
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl Certificate {
    /// Encodes the fifteen-key map.
    pub fn to_cbor(&self) -> Result<CborValue, CapabilityError> {
        let mut entries = match self.unsigned.to_cbor()? {
            CborValue::Map(entries) => entries,
            _ => unreachable!("UnsignedCertificate::to_cbor always yields a map"),
        };
        entries.push((
            CborValue::Uint(14),
            CborValue::Bytes(self.signature.to_vec()),
        ));
        Ok(CborValue::Map(entries))
    }

    /// Decodes the fifteen-key map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CapabilityError> {
        require_numeric_keys(value, 15, COMPLETE_CONTEXT)?;
        Ok(Self {
            unsigned: parse_unsigned_fields(value, COMPLETE_CONTEXT)?,
            signature: bytes_at::<SIGNATURE_LENGTH>(value, 14, COMPLETE_CONTEXT)?,
        })
    }

    /// Encodes the exact complete-certificate bytes a chain stores.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CapabilityError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CapabilityError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// `certificate_id = BLAKE3(complete_certificate)`, key 14 and its signature included.
    pub fn certificate_id(&self) -> Result<CertificateId, CapabilityError> {
        Ok(certificate_id_of_bytes(&self.encode_canonical()?))
    }

    /// Enforces §2.1 rule 2 on both principals before any signature is considered.
    pub fn verify_principal_bindings(&self) -> Result<(), CapabilityError> {
        verify_author_binding(&self.unsigned.issuer.did, &self.unsigned.issuer.public_key)?;
        verify_author_binding(
            &self.unsigned.audience.did,
            &self.unsigned.audience.public_key,
        )?;
        Ok(())
    }

    /// Verifies the §2.2 Ed25519 signature under `issuer.public_key`.
    pub fn verify_issuer_signature(&self) -> Result<(), CapabilityError> {
        let unsigned_bytes = self.unsigned.encode_canonical()?;
        verify_domain(
            &self.unsigned.issuer.public_key,
            CERTIFICATE_SIGNATURE_DOMAIN,
            &unsigned_bytes,
            &self.signature,
        )?;
        Ok(())
    }

    /// Verifies both principal bindings and then the issuer signature (§2.4 step 2).
    pub fn verify_signature(&self) -> Result<(), CapabilityError> {
        self.verify_principal_bindings()?;
        self.verify_issuer_signature()
    }
}

/// `certificate_id` over bytes already in hand, so a chain never re-encodes to address a link.
pub fn certificate_id_of_bytes(complete_bytes: &[u8]) -> CertificateId {
    Hash32::of(complete_bytes)
}

/// Reads keys 0 through 13 without asserting the outer map's arity.
fn parse_unsigned_fields(
    value: &CborValue,
    context: &'static str,
) -> Result<UnsignedCertificate, CapabilityError> {
    let capability_version = unsigned_at(value, 0, context)?;
    if capability_version != CAPABILITY_VERSION {
        return Err(CapabilityError::UnsupportedCapabilityVersion {
            version: capability_version,
        });
    }
    let grant_scope = scope_at(value, 5, context)?;
    let certificate = UnsignedCertificate {
        capability_version,
        issuer: principal_at(value, 1, context)?,
        audience: principal_at(value, 2, context)?,
        parent_certificate_id: optional_hash_at(value, 3, context)?,
        issuer_authority_id: Hash32(bytes_at::<32>(value, 4, context)?),
        grant_scope,
        epoch_scope: scope_at(value, 6, context)?,
        scope_epoch: unsigned_at(value, 7, context)?,
        verbs: VerbSet::from_bits(u8_at(value, 8, context)?)?,
        object_classes: ObjectClassSet::from_bits(u8_at(value, 9, context)?)?,
        not_before_ms: unsigned_at(value, 10, context)?,
        not_after_ms: unsigned_at(value, 11, context)?,
        delegation_depth: u8_at(value, 12, context)?,
        caveats: Caveats::from_cbor(super::entry(value, 13, context)?, &grant_scope)?,
    };
    certificate.validate()?;
    Ok(certificate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::CborError;
    use crate::envelope::Identifier32;
    use crate::signing::SigningError;

    use super::super::verbs::{ObjectClass, Verb};

    fn signing_key(seed_byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed_byte; 32])
    }

    fn principal(seed_byte: u8) -> Principal {
        Principal::from_public_key(signing_key(seed_byte).verifying_key().to_bytes())
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn root_unsigned() -> UnsignedCertificate {
        UnsignedCertificate {
            capability_version: CAPABILITY_VERSION,
            issuer: principal(0x01),
            audience: principal(0x02),
            parent_certificate_id: None,
            issuer_authority_id: Hash32([0xa0; 32]),
            grant_scope: Scope::verse_wide(identifier(0x11)),
            epoch_scope: Scope::verse_wide(identifier(0x11)),
            scope_epoch: 7,
            verbs: VerbSet::from_verbs([Verb::Append, Verb::Fetch]).expect("verbs"),
            object_classes: ObjectClassSet::from_classes([ObjectClass::Operation])
                .expect("classes"),
            not_before_ms: 1_700_000_000_000,
            not_after_ms: 1_700_000_000_000 + MAXIMUM_CERTIFICATE_LIFETIME_MS,
            delegation_depth: 2,
            caveats: Caveats::unrestricted(),
        }
    }

    #[test]
    fn the_signature_domain_is_nul_terminated_and_distinct() {
        assert_eq!(CERTIFICATE_SIGNATURE_DOMAIN, b"fe-capability-cert-v1\0");
        assert_eq!(
            CERTIFICATE_SIGNATURE_DOMAIN.last(),
            Some(&0u8),
            "a D-CL3 domain terminates in NUL so no domain prefixes another"
        );
        assert_ne!(
            CERTIFICATE_SIGNATURE_DOMAIN,
            crate::signing::SIGNATURE_DOMAIN
        );
    }

    #[test]
    fn a_certificate_round_trips_and_carries_fifteen_keys() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        let bytes = certificate.encode_canonical().expect("encode");
        let decoded = Certificate::decode_canonical(&bytes).expect("decode");

        assert_eq!(decoded, certificate);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
        assert_eq!(
            certificate
                .to_cbor()
                .expect("cbor")
                .as_map()
                .map(|entries| entries.len()),
            Some(15)
        );
        assert_eq!(
            certificate
                .unsigned
                .to_cbor()
                .expect("cbor")
                .as_map()
                .map(|entries| entries.len()),
            Some(14)
        );
    }

    #[test]
    fn the_certificate_identifier_covers_the_signature() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        let identifier = certificate.certificate_id().expect("id");
        assert_eq!(
            identifier,
            Hash32::of(&certificate.encode_canonical().expect("encode"))
        );

        let mut tampered = certificate.clone();
        tampered.signature[0] ^= 0x01;
        assert_ne!(tampered.certificate_id().expect("id"), identifier);
        assert_ne!(
            identifier,
            Hash32::of(&certificate.unsigned.encode_canonical().expect("encode")),
            "certificate_id must cover key 14"
        );
    }

    #[test]
    fn a_signed_certificate_verifies_and_a_wrong_issuer_key_does_not() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        assert_eq!(certificate.verify_signature(), Ok(()));

        let wrong_issuer = root_unsigned().sign(&signing_key(0x09)).expect("sign");
        assert_eq!(
            wrong_issuer.verify_signature(),
            Err(CapabilityError::Signing(
                SigningError::SignatureVerificationFailed
            ))
        );
    }

    #[test]
    fn a_did_that_does_not_bind_to_its_public_key_is_rejected() {
        let mut unsigned = root_unsigned();
        unsigned.audience.did = principal(0x03).did;
        let certificate = unsigned.sign(&signing_key(0x01)).expect("sign");
        assert_eq!(
            certificate.verify_principal_bindings(),
            Err(CapabilityError::Signing(
                SigningError::AuthorBindingMismatch
            ))
        );
    }

    #[test]
    fn a_capability_version_other_than_one_is_rejected() {
        let mut unsigned = root_unsigned();
        unsigned.capability_version = 2;
        assert_eq!(
            unsigned.encode_canonical(),
            Err(CapabilityError::UnsupportedCapabilityVersion { version: 2 })
        );
    }

    #[test]
    fn an_epoch_scope_that_does_not_contain_the_grant_scope_is_rejected() {
        let mut unsigned = root_unsigned();
        unsigned.epoch_scope =
            Scope::new(identifier(0x11), Some(identifier(0x22)), None).expect("petal scope");
        assert_eq!(
            unsigned.encode_canonical(),
            Err(CapabilityError::EpochScopeDoesNotContainGrantScope)
        );

        unsigned.grant_scope =
            Scope::new(identifier(0x11), Some(identifier(0x22)), None).expect("petal scope");
        assert!(unsigned.encode_canonical().is_ok());
    }

    #[test]
    fn an_empty_validity_window_is_rejected() {
        let mut unsigned = root_unsigned();
        unsigned.not_after_ms = unsigned.not_before_ms;
        assert_eq!(
            unsigned.encode_canonical(),
            Err(CapabilityError::EmptyValidityWindow {
                not_before_ms: unsigned.not_before_ms,
                not_after_ms: unsigned.not_after_ms,
            })
        );
    }

    #[test]
    fn signing_refuses_a_window_longer_than_twenty_four_hours() {
        let mut unsigned = root_unsigned();
        unsigned.not_after_ms = unsigned.not_before_ms + MAXIMUM_CERTIFICATE_LIFETIME_MS + 1;
        assert_eq!(
            unsigned.sign(&signing_key(0x01)),
            Err(CapabilityError::LifetimeExceedsMaximum {
                lifetime_ms: MAXIMUM_CERTIFICATE_LIFETIME_MS + 1,
                maximum_ms: MAXIMUM_CERTIFICATE_LIFETIME_MS,
            })
        );
        assert!(
            unsigned.encode_canonical().is_ok(),
            "encoding stays permissive so peer bytes reach the §2.4 step 7 check"
        );
    }

    #[test]
    fn an_empty_verb_or_class_bitset_is_rejected_on_decode() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        let mut entries = match certificate.to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[8] = (CborValue::Uint(8), CborValue::Uint(0));
        assert_eq!(
            Certificate::from_cbor(&CborValue::Map(entries.clone())),
            Err(CapabilityError::EmptyVerbSet)
        );

        entries[8] = (
            CborValue::Uint(8),
            CborValue::Uint(u64::from(certificate.unsigned.verbs.bits())),
        );
        entries[9] = (CborValue::Uint(9), CborValue::Uint(0));
        assert_eq!(
            Certificate::from_cbor(&CborValue::Map(entries)),
            Err(CapabilityError::EmptyObjectClassSet)
        );
    }

    #[test]
    fn an_unknown_certificate_key_is_rejected() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        let mut entries = match certificate.to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries.push((CborValue::Uint(15), CborValue::Uint(0)));
        assert_eq!(
            Certificate::from_cbor(&CborValue::Map(entries)),
            Err(CapabilityError::UnexpectedMapKeys {
                context: "certificate",
                expected_key_count: 15,
            })
        );
    }

    #[test]
    fn non_canonical_certificate_bytes_are_rejected() {
        let certificate = root_unsigned().sign(&signing_key(0x01)).expect("sign");
        let mut bytes = certificate.encode_canonical().expect("encode");
        bytes.push(0x00);
        assert_eq!(
            Certificate::decode_canonical(&bytes),
            Err(CapabilityError::Cbor(CborError::TrailingBytes { count: 1 }))
        );

        // A definite-length map head replaced by the indefinite-length head 0xbf.
        let mut indefinite = certificate.encode_canonical().expect("encode");
        indefinite[0] = 0xbf;
        assert!(matches!(
            Certificate::decode_canonical(&indefinite),
            Err(CapabilityError::Cbor(CborError::IndefiniteLength { .. }))
        ));
    }
}
