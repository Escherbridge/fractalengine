//! Capability certificates, chains, permissions, revalidation, and blinded topics (SPEC-3).
//!
//! Rationale and the provisional wire-numbering table live in `src/capability/AGENTS.md`.

pub mod caveats;
pub mod certificate;
pub mod chain;
pub mod permissions;
pub mod revalidation;
pub mod topic;
pub mod verbs;

use thiserror::Error;

use crate::cbor::{CborError, CborValue};
use crate::envelope::{EnvelopeError, Hash32, Identifier32, Scope};
use crate::signing::SigningError;

pub use caveats::{CaveatField, Caveats};
pub use certificate::{
    Certificate, CertificateId, UnsignedCertificate, CAPABILITY_VERSION,
    CERTIFICATE_SIGNATURE_DOMAIN, MAXIMUM_CERTIFICATE_LIFETIME_MS,
};
pub use chain::{
    verify_chain, verify_chain_bytes, AttenuationAspect, AuthorityState, AuthorizationRequest,
    AuthorizationSnapshot, Chain, ChainError, ChainId, ManagerAuthorityView, PrincipalRole,
    VerificationOptions, VerifiedCapability,
};
pub use permissions::{
    permitted, requires_manager_plus, requires_operation_kind_allowlist, PermissionContext,
    PermissionError,
};
pub use revalidation::{
    AdmittedDecision, AuthorizationView, CacheKey, CacheRefusal, PinnedSession, RevalidationGate,
    SessionValidity,
};
pub use topic::{TopicLabel, TopicLane};
pub use verbs::{ObjectClass, ObjectClassSet, Verb, VerbSet};

/// A SPEC-3 principal: the SPEC-1 author map of a canonical `did:key` DID and its raw key.
pub use crate::envelope::Author as Principal;

/// Every reason a capability artifact fails to encode, decode, or satisfy §2.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CapabilityError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A shared SPEC-1 grammar rule was broken.
    #[error("envelope grammar: {0}")]
    Envelope(#[from] EnvelopeError),
    /// A DID binding or Ed25519 signature check failed.
    #[error("signing: {0}")]
    Signing(#[from] SigningError),
    /// A value that must be a map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// Map being parsed.
        context: &'static str,
    },
    /// A value that must be an array was not one.
    #[error("{context} key {key} is not a CBOR array")]
    ExpectedArray {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A map did not hold exactly the listed integer keys, once each, and nothing else.
    #[error("{context} must hold exactly the integer keys 0..{expected_key_count} once each")]
    UnexpectedMapKeys {
        /// Map being parsed.
        context: &'static str,
        /// Number of integer keys the grammar lists.
        expected_key_count: u64,
    },
    /// A listed key was absent.
    #[error("{context} key {key} is missing")]
    MissingKey {
        /// Map being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key held something other than an unsigned integer.
    #[error("{context} key {key} is not an unsigned integer")]
    ExpectedUnsignedInteger {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A key held something other than a byte string.
    #[error("{context} key {key} is not a byte string")]
    ExpectedByteString {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A nullable slot held neither `null` nor the value its row declares.
    #[error("{context} key {key} must be null or {expected}")]
    ExpectedNullOr {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// Shape the row declares.
        expected: &'static str,
    },
    /// A byte string had the wrong fixed length.
    #[error("{context} key {key} holds {actual} byte(s), expected {expected}")]
    WrongByteLength {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// Length the grammar requires.
        expected: usize,
        /// Length actually present.
        actual: usize,
    },
    /// An integer exceeded the width its grammar row declares.
    #[error("{context} key {key} value {value} exceeds its declared integer width")]
    IntegerOutOfRange {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// The out-of-range value.
        value: u64,
    },
    /// `capability_version` was not 1.
    #[error("capability_version {version} is not 1")]
    UnsupportedCapabilityVersion {
        /// The version actually present.
        version: u64,
    },
    /// A verb bitset held a bit outside §3.1.
    #[error("verb bitset holds unknown bit(s) 0x{bits:02x}")]
    UnknownVerbBits {
        /// The unknown bits.
        bits: u8,
    },
    /// A verb bitset was empty, which §2.2 key 8 forbids.
    #[error("verb bitset must be a non-empty subset of the §3.1 bits")]
    EmptyVerbSet,
    /// An object-class bitset held a bit outside §3.2.
    #[error("object-class bitset holds unknown bit(s) 0x{bits:02x}")]
    UnknownObjectClassBits {
        /// The unknown bits.
        bits: u8,
    },
    /// An object-class bitset was empty, which §2.2 key 9 forbids.
    #[error("object-class bitset must be a non-empty subset of the §3.2 bits")]
    EmptyObjectClassSet,
    /// A caveat array was not strictly ascending and duplicate-free (§2.1 rule 4).
    #[error("caveat {field:?} is not strictly ascending and duplicate-free")]
    UnsortedCaveatArray {
        /// The offending caveat.
        field: CaveatField,
    },
    /// `allowed_operation_kinds` held the reserved zero kind.
    #[error("allowed_operation_kinds must not hold the zero operation kind")]
    ZeroOperationKindInCaveat,
    /// A `resource_allowlist` entry was not inside `grant_scope` (§2.3 key 3).
    #[error("resource_allowlist holds an identifier outside grant_scope")]
    ResourceOutsideGrantScope,
    /// `epoch_scope` did not contain `grant_scope` (§2.2 key 6).
    #[error("epoch_scope must contain grant_scope")]
    EpochScopeDoesNotContainGrantScope,
    /// `not_before_ms` was not strictly before `not_after_ms`.
    #[error("not_before_ms {not_before_ms} must be strictly before not_after_ms {not_after_ms}")]
    EmptyValidityWindow {
        /// Inclusive start.
        not_before_ms: u64,
        /// Exclusive end.
        not_after_ms: u64,
    },
    /// A certificate lifetime exceeded the bound in force (§2.4 step 7, §4.5).
    #[error("certificate lifetime {lifetime_ms} ms exceeds the maximum of {maximum_ms} ms")]
    LifetimeExceedsMaximum {
        /// Lifetime the certificate declares.
        lifetime_ms: u64,
        /// Bound in force.
        maximum_ms: u64,
    },
    /// A chain named no certificate, which §2.4 key 1 forbids.
    #[error("a capability chain must hold at least one certificate")]
    EmptyChain,
    /// `leaf_certificate_id` did not equal the BLAKE3 ID of the final certificate.
    #[error("leaf_certificate_id does not equal the BLAKE3 ID of the final certificate")]
    LeafCertificateIdMismatch,
    /// A topic lane byte was outside §6.1's three lanes.
    #[error("topic lane {lane} is not one of header (0), payload (1), or availability (2)")]
    UnknownTopicLane {
        /// The rejected lane value.
        lane: u64,
    },
}

/// Requires exactly the integer keys `0..expected_key_count`, once each and nothing else.
pub(crate) fn require_numeric_keys(
    value: &CborValue,
    expected_key_count: u64,
    context: &'static str,
) -> Result<(), CapabilityError> {
    let entries = value
        .as_map()
        .ok_or(CapabilityError::ExpectedMap { context })?;
    if entries.len() as u64 != expected_key_count {
        return Err(CapabilityError::UnexpectedMapKeys {
            context,
            expected_key_count,
        });
    }
    for key in 0..expected_key_count {
        if value.get_uint_key(key).is_none() {
            return Err(CapabilityError::MissingKey { context, key });
        }
    }
    Ok(())
}

pub(crate) fn entry<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, CapabilityError> {
    value
        .get_uint_key(key)
        .ok_or(CapabilityError::MissingKey { context, key })
}

pub(crate) fn unsigned_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u64, CapabilityError> {
    entry(value, key, context)?
        .as_uint()
        .ok_or(CapabilityError::ExpectedUnsignedInteger { context, key })
}

pub(crate) fn u8_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u8, CapabilityError> {
    let raw = unsigned_at(value, key, context)?;
    u8::try_from(raw).map_err(|_| CapabilityError::IntegerOutOfRange {
        context,
        key,
        value: raw,
    })
}

pub(crate) fn bytes_at<const LENGTH: usize>(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; LENGTH], CapabilityError> {
    let raw = entry(value, key, context)?
        .as_bytes()
        .ok_or(CapabilityError::ExpectedByteString { context, key })?;
    raw.try_into()
        .map_err(|_| CapabilityError::WrongByteLength {
            context,
            key,
            expected: LENGTH,
            actual: raw.len(),
        })
}

pub(crate) fn optional_hash_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Option<Hash32>, CapabilityError> {
    let slot = entry(value, key, context)?;
    if slot.is_null() {
        return Ok(None);
    }
    Ok(Some(Hash32(bytes_at::<32>(value, key, context)?)))
}

pub(crate) fn optional_unsigned_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Option<u64>, CapabilityError> {
    let slot = entry(value, key, context)?;
    if slot.is_null() {
        return Ok(None);
    }
    Ok(Some(slot.as_uint().ok_or(
        CapabilityError::ExpectedNullOr {
            context,
            key,
            expected: "an unsigned integer",
        },
    )?))
}

/// Reads a nullable array of fixed-length byte strings.
pub(crate) fn optional_byte_array_at<const LENGTH: usize>(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Option<Vec<[u8; LENGTH]>>, CapabilityError> {
    let slot = entry(value, key, context)?;
    if slot.is_null() {
        return Ok(None);
    }
    let items = slot
        .as_array()
        .ok_or(CapabilityError::ExpectedArray { context, key })?;
    let mut decoded = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .as_bytes()
            .ok_or(CapabilityError::ExpectedByteString { context, key })?;
        decoded.push(
            raw.try_into()
                .map_err(|_| CapabilityError::WrongByteLength {
                    context,
                    key,
                    expected: LENGTH,
                    actual: raw.len(),
                })?,
        );
    }
    Ok(Some(decoded))
}

/// Reads a nullable array of unsigned integers narrowed to `u16`.
pub(crate) fn optional_u16_array_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Option<Vec<u16>>, CapabilityError> {
    let slot = entry(value, key, context)?;
    if slot.is_null() {
        return Ok(None);
    }
    let items = slot
        .as_array()
        .ok_or(CapabilityError::ExpectedArray { context, key })?;
    let mut decoded = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .as_uint()
            .ok_or(CapabilityError::ExpectedUnsignedInteger { context, key })?;
        decoded.push(
            u16::try_from(raw).map_err(|_| CapabilityError::IntegerOutOfRange {
                context,
                key,
                value: raw,
            })?,
        );
    }
    Ok(Some(decoded))
}

pub(crate) fn scope_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Scope, CapabilityError> {
    Ok(Scope::from_cbor(entry(value, key, context)?)?)
}

pub(crate) fn principal_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Principal, CapabilityError> {
    Ok(Principal::from_cbor(entry(value, key, context)?)?)
}

/// Reports whether every adjacent pair strictly ascends, which also rules out duplicates.
pub(crate) fn is_strictly_ascending<T: PartialOrd>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

/// Encodes an optional unsigned integer as its value or `null`.
pub(crate) fn optional_unsigned_to_cbor(value: Option<u64>) -> CborValue {
    match value {
        Some(value) => CborValue::Uint(value),
        None => CborValue::Null,
    }
}

/// Encodes an optional hash as a byte string or `null`.
pub(crate) fn optional_hash_to_cbor(value: Option<Hash32>) -> CborValue {
    match value {
        Some(value) => CborValue::Bytes(value.0.to_vec()),
        None => CborValue::Null,
    }
}

/// Encodes a slice of 32-byte identifiers as a CBOR array of byte strings.
pub(crate) fn identifiers_to_cbor(values: &[Identifier32]) -> CborValue {
    CborValue::Array(
        values
            .iter()
            .map(|value| CborValue::Bytes(value.0.to_vec()))
            .collect(),
    )
}

/// Encodes a slice of 32-byte hashes as a CBOR array of byte strings.
pub(crate) fn hashes_to_cbor(values: &[Hash32]) -> CborValue {
    CborValue::Array(
        values
            .iter()
            .map(|value| CborValue::Bytes(value.0.to_vec()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ed25519_dalek::SigningKey;
    use serde_json::Value;

    use super::caveats::Caveats;
    use super::certificate::{Certificate, UnsignedCertificate};
    use super::chain::{
        verify_chain_bytes, AuthorityState, AuthorizationRequest, AuthorizationSnapshot, Chain,
        ManagerAuthorityView, VerificationOptions,
    };
    use super::revalidation::AuthorizationView;
    use super::verbs::{ObjectClass, ObjectClassSet, Verb, VerbSet};
    use super::Principal;
    use crate::envelope::{Hash32, Identifier32, Scope};

    /// An in-memory authorization view that answers only for the fixture's own root anchor.
    struct FixtureAuthorityView {
        root_authority_id: Hash32,
        root_issuer: Principal,
        epoch_scope: Scope,
        current_epoch: u64,
    }

    impl AuthorizationView for FixtureAuthorityView {
        fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
            (*epoch_scope == self.epoch_scope).then_some(self.current_epoch)
        }

        fn version(&self) -> u64 {
            1
        }
    }

    impl ManagerAuthorityView for FixtureAuthorityView {
        fn authority_is_manager_plus(
            &self,
            authority_id: &Hash32,
            issuer: &Principal,
            epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            if *epoch_scope != self.epoch_scope || *authority_id != self.root_authority_id {
                return AuthorityState::Unknown;
            }
            if *issuer == self.root_issuer {
                AuthorityState::ManagerPlus
            } else {
                AuthorityState::NotManagerPlus
            }
        }

        fn principal_is_manager_plus(
            &self,
            _principal: &Principal,
            _epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            AuthorityState::NotManagerPlus
        }
    }

    /// Path of the committed SPEC-3 vector; code changes to match it, never the reverse.
    fn fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/capability-chain-v1.json")
    }

    fn fixture() -> Value {
        let text = std::fs::read_to_string(fixture_path()).expect("read the capability fixture");
        serde_json::from_str(&text).expect("parse the capability fixture")
    }

    fn bytes32(value: &Value) -> [u8; 32] {
        hex::decode(value.as_str().expect("hexadecimal string"))
            .expect("valid hexadecimal")
            .try_into()
            .expect("32 bytes")
    }

    fn optional_bytes32(value: &Value) -> Option<[u8; 32]> {
        if value.is_null() {
            None
        } else {
            Some(bytes32(value))
        }
    }

    fn unsigned_integer(value: &Value) -> u64 {
        value.as_u64().expect("unsigned integer")
    }

    fn optional_unsigned_integer(value: &Value) -> Option<u64> {
        if value.is_null() {
            None
        } else {
            Some(unsigned_integer(value))
        }
    }

    fn scope_from(value: &Value) -> Scope {
        Scope::new(
            Identifier32(bytes32(&value["verse_id_hex"])),
            optional_bytes32(&value["petal_id_hex"]).map(Identifier32),
            optional_bytes32(&value["resource_id_hex"]).map(Identifier32),
        )
        .expect("fixture scope")
    }

    fn caveats_from(value: &Value) -> Caveats {
        Caveats {
            max_payload_bytes: optional_unsigned_integer(&value["max_payload_bytes"]),
            max_segment_bytes: optional_unsigned_integer(&value["max_segment_bytes"]),
            allowed_schema_hashes: value["allowed_schema_hashes_hex"].as_array().map(|items| {
                items
                    .iter()
                    .map(|item| Hash32(bytes32(item)))
                    .collect::<Vec<Hash32>>()
            }),
            resource_allowlist: value["resource_allowlist_hex"].as_array().map(|items| {
                items
                    .iter()
                    .map(|item| Identifier32(bytes32(item)))
                    .collect::<Vec<Identifier32>>()
            }),
            allowed_operation_kinds: value["allowed_operation_kinds"].as_array().map(|items| {
                items
                    .iter()
                    .map(|item| u16::try_from(unsigned_integer(item)).expect("u16 kind"))
                    .collect::<Vec<u16>>()
            }),
            max_preview_hz: optional_unsigned_integer(&value["max_preview_hz"])
                .map(|raw| u16::try_from(raw).expect("u16 preview rate")),
        }
    }

    fn signing_key_from(value: &Value) -> SigningKey {
        SigningKey::from_bytes(&bytes32(value))
    }

    fn unsigned_from(value: &Value) -> UnsignedCertificate {
        UnsignedCertificate {
            capability_version: 1,
            issuer: Principal {
                did: value["issuer_did"].as_str().expect("did").to_owned(),
                public_key: bytes32(&value["issuer_public_key_hex"]),
            },
            audience: Principal {
                did: value["audience_did"].as_str().expect("did").to_owned(),
                public_key: bytes32(&value["audience_public_key_hex"]),
            },
            parent_certificate_id: optional_bytes32(&value["parent_certificate_id_hex"])
                .map(Hash32),
            issuer_authority_id: Hash32(bytes32(&value["issuer_authority_id_hex"])),
            grant_scope: scope_from(&value["grant_scope"]),
            epoch_scope: scope_from(&value["epoch_scope"]),
            scope_epoch: unsigned_integer(&value["scope_epoch"]),
            verbs: VerbSet::from_bits(u8::try_from(unsigned_integer(&value["verbs"])).expect("u8"))
                .expect("fixture verbs"),
            object_classes: ObjectClassSet::from_bits(
                u8::try_from(unsigned_integer(&value["object_classes"])).expect("u8"),
            )
            .expect("fixture object classes"),
            not_before_ms: unsigned_integer(&value["not_before_ms"]),
            not_after_ms: unsigned_integer(&value["not_after_ms"]),
            delegation_depth: u8::try_from(unsigned_integer(&value["delegation_depth"]))
                .expect("u8"),
            caveats: caveats_from(&value["caveats"]),
        }
    }

    #[test]
    fn capability_chain_v1_golden_vector_round_trip() {
        let fixture = fixture();
        let entries = fixture["certificates"]
            .as_array()
            .expect("certificate array");
        let mut certificates = Vec::new();

        for entry in entries {
            let unsigned = unsigned_from(entry);
            let role = entry["role"].as_str().expect("role");

            assert_eq!(
                hex::encode(unsigned.encode_canonical().expect("encode")),
                entry["unsigned_hex"].as_str().expect("hex"),
                "{role}: the unsigned certificate must encode byte-for-byte"
            );

            let signed = unsigned
                .sign(&signing_key_from(&entry["issuer_seed_hex"]))
                .expect("sign");
            assert_eq!(
                hex::encode(signed.signature),
                entry["signature_hex"].as_str().expect("hex"),
                "{role}: Ed25519 signing over the fe-capability-cert-v1 domain is deterministic"
            );

            let complete_bytes = signed.encode_canonical().expect("encode");
            assert_eq!(
                hex::encode(&complete_bytes),
                entry["complete_hex"].as_str().expect("hex"),
                "{role}: the complete certificate must encode byte-for-byte"
            );
            assert_eq!(
                hex::encode(signed.certificate_id().expect("id").0),
                entry["certificate_id_hex"].as_str().expect("hex"),
                "{role}: certificate_id is BLAKE3 over the complete certificate"
            );
            assert_eq!(
                Certificate::decode_canonical(&complete_bytes).expect("decode"),
                signed,
                "{role}: the committed bytes decode back to the same certificate"
            );
            assert_eq!(signed.verify_signature(), Ok(()), "{role}");

            certificates.push(signed);
        }

        let chain = Chain::from_certificates(&certificates).expect("chain");
        let chain_bytes = chain.encode_canonical().expect("encode");
        assert_eq!(
            hex::encode(&chain_bytes),
            fixture["chain"]["complete_hex"].as_str().expect("hex")
        );
        assert_eq!(
            hex::encode(chain.chain_id().expect("id").0),
            fixture["chain"]["chain_id_hex"].as_str().expect("hex")
        );
        assert_eq!(
            hex::encode(chain.leaf_certificate_id.0),
            fixture["chain"]["leaf_certificate_id_hex"]
                .as_str()
                .expect("hex")
        );
        assert_eq!(
            Chain::decode_canonical(&chain_bytes).expect("decode"),
            chain
        );

        let leaf = &certificates.last().expect("leaf").unsigned;
        let request = AuthorizationRequest {
            verb: Verb::Append,
            object_class: ObjectClass::Operation,
            target_scope: leaf.grant_scope,
            resource_id: None,
            requester: leaf.audience.clone(),
            operation_author: leaf.audience.clone(),
            operation_scope_epoch: leaf.scope_epoch,
            operation_kind: Some(1),
            is_structural_operation: false,
            payload_bytes: Some(1024),
            segment_bytes: None,
            schema_hash: leaf
                .caveats
                .allowed_schema_hashes
                .as_ref()
                .and_then(|hashes| hashes.first().copied()),
            preview_hz: None,
            now_ms: leaf.not_before_ms + 1_000,
        };
        let root = &certificates.first().expect("root").unsigned;
        let view = FixtureAuthorityView {
            root_authority_id: root.issuer_authority_id,
            root_issuer: root.issuer.clone(),
            epoch_scope: root.epoch_scope,
            current_epoch: leaf.scope_epoch,
        };
        let snapshot = AuthorizationSnapshot {
            epoch_causally_valid_for_operation: false,
        };
        let verified = verify_chain_bytes(
            &chain_bytes,
            &request,
            &view,
            &snapshot,
            &VerificationOptions::live(),
        )
        .expect("the committed chain authorizes its own leaf audience");

        assert_eq!(verified.chain_id, chain.chain_id().expect("id"));
        assert_eq!(verified.leaf_certificate_id, chain.leaf_certificate_id);
        assert_eq!(verified.effective_scope, leaf.grant_scope);
        assert_eq!(verified.epoch_scope, leaf.epoch_scope);
        assert_eq!(verified.scope_epoch, leaf.scope_epoch);
        assert_eq!(verified.leaf_audience, leaf.audience);
        assert_eq!(verified.effective_not_after_ms, leaf.not_after_ms);
    }

    #[test]
    fn the_fixture_records_the_domains_the_code_signs_under() {
        let fixture = fixture();
        assert_eq!(
            fixture["certificate_signature_domain"]
                .as_str()
                .expect("domain"),
            std::str::from_utf8(&super::CERTIFICATE_SIGNATURE_DOMAIN[..21]).expect("ascii")
        );
        assert_eq!(unsigned_integer(&fixture["capability_version"]), 1);
    }
}
