//! Self-signed device-enrolment artifacts and the registry that answers `DeviceEnrolmentView`
//! from them (SPEC-3 §10.2.1, owner decision R2 2026-10-13); see `src/crypto/AGENTS.md`
//! §device-enrolment.
//!
//! Protocol only, like the rest of `crypto`: no storage, no replication, no transport. The
//! durable table and the replicated-row admission branch live in fe-database
//! `canon_log::device_enrolment` and verify through [`SignedDeviceEnrolment::decode_and_verify`]
//! / [`VerifiedEnrolmentArtifact::decode_and_verify`] before anything is stored.

use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::capability::{bytes_at, principal_at, require_numeric_keys, unsigned_at, Principal};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, SIGNATURE_LENGTH};
use crate::signing::{
    domain_preimage, sign_domain, verify_author_binding, verify_domain, SigningError,
};

use super::key_wrap::{
    AuthorizationRecord, DeviceEnrolmentView, DevicePublicKey, DEVICE_PUBLIC_KEY_LENGTH,
};
use super::CryptoError;

/// NUL-terminated ASCII domain an enrolment signature is computed under (provisional, CDL-PROV).
pub const DEVICE_ENROLMENT_DOMAIN: &[u8] = b"fe-device-enrolment-v1\0";

/// NUL-terminated ASCII domain a revocation signature is computed under (provisional, CDL-PROV).
pub const DEVICE_ENROLMENT_REVOCATION_DOMAIN: &[u8] = b"fe-device-enrolment-revoke-v1\0";

/// NUL-terminated ASCII domain of [`enrolment_record_id`] (provisional, CDL-PROV).
pub const ENROLMENT_RECORD_ID_DOMAIN: &[u8] = b"fe-device-enrolment-record-id-v1\0";

/// Integer key of the signature slot in a complete artifact, one past the last body key.
const SIGNATURE_KEY: u64 = 3;

/// Which of the two artifacts a set of bytes is; only the signature domain tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    Enrolment,
    Revocation,
}

impl ArtifactKind {
    const fn domain(self) -> &'static [u8] {
        match self {
            Self::Enrolment => DEVICE_ENROLMENT_DOMAIN,
            Self::Revocation => DEVICE_ENROLMENT_REVOCATION_DOMAIN,
        }
    }

    const fn context(self) -> &'static str {
        match self {
            Self::Enrolment => "device enrolment",
            Self::Revocation => "device enrolment revocation",
        }
    }
}

/// `record_id = BLAKE3("fe-device-enrolment-record-id-v1" || 0x00 || principal_key || device_key)`.
///
/// One identifier per (principal, device) pair, shared by the enrolment and its revocation, so a
/// durable store keyed by it holds one row that a revocation overwrites. Both inputs are fixed
/// 32-byte keys, so plain concatenation is unambiguous; the DID is omitted because a verified
/// artifact's DID is a function of its key.
pub fn enrolment_record_id(principal: &Principal, device_key: DevicePublicKey) -> Hash32 {
    let mut body = Vec::with_capacity(2 * DEVICE_PUBLIC_KEY_LENGTH);
    body.extend_from_slice(&principal.public_key);
    body.extend_from_slice(&device_key.0);
    Hash32::of(&domain_preimage(ENROLMENT_RECORD_ID_DOMAIN, &body))
}

/// Refuses an X25519 key of small order, which no wrap could ever be sealed to.
///
/// X25519 clamps every scalar to a multiple of the cofactor, so any small-order point (on the
/// curve or its twist) maps to the all-zero output for any probe scalar.
fn assert_contributory_device_key(device_key: DevicePublicKey) -> Result<(), CryptoError> {
    let probe = StaticSecret::from([0x01; DEVICE_PUBLIC_KEY_LENGTH]);
    if probe
        .diffie_hellman(&PublicKey::from(device_key.0))
        .was_contributory()
    {
        Ok(())
    } else {
        Err(CryptoError::NonContributoryKeyExchange)
    }
}

/// The three-key body both artifacts share: `0` principal, `1` device key, `2` timestamp.
fn body_to_cbor(principal: &Principal, device_key: DevicePublicKey, at_ms: u64) -> CborValue {
    CborValue::Map(vec![
        (CborValue::Uint(0), principal.to_cbor()),
        (CborValue::Uint(1), CborValue::Bytes(device_key.0.to_vec())),
        (CborValue::Uint(2), CborValue::Uint(at_ms)),
    ])
}

fn encode_body(
    principal: &Principal,
    device_key: DevicePublicKey,
    at_ms: u64,
) -> Result<Vec<u8>, CryptoError> {
    Ok(encode_canonical_checked(&body_to_cbor(
        principal, device_key, at_ms,
    ))?)
}

fn encode_complete(
    principal: &Principal,
    device_key: DevicePublicKey,
    at_ms: u64,
    signature: &[u8; SIGNATURE_LENGTH],
) -> Result<Vec<u8>, CryptoError> {
    let CborValue::Map(mut entries) = body_to_cbor(principal, device_key, at_ms) else {
        unreachable!("body_to_cbor always builds a map")
    };
    entries.push((
        CborValue::Uint(SIGNATURE_KEY),
        CborValue::Bytes(signature.to_vec()),
    ));
    Ok(encode_canonical_checked(&CborValue::Map(entries))?)
}

/// Signs a body under `kind`'s domain, refusing any key but the named principal's own.
fn sign_as(
    kind: ArtifactKind,
    signing_key: &SigningKey,
    principal: &Principal,
    device_key: DevicePublicKey,
    at_ms: u64,
) -> Result<[u8; SIGNATURE_LENGTH], CryptoError> {
    verify_author_binding(&principal.did, &principal.public_key)?;
    if signing_key.verifying_key().to_bytes() != principal.public_key {
        return Err(CryptoError::EnrolmentSignerMismatch);
    }
    Ok(sign_domain(
        signing_key,
        kind.domain(),
        &encode_body(principal, device_key, at_ms)?,
    ))
}

/// A complete artifact decoded from received bytes, before its signature is checked.
struct DecodedArtifact {
    principal: Principal,
    device_key: DevicePublicKey,
    at_ms: u64,
    signature: [u8; SIGNATURE_LENGTH],
}

impl DecodedArtifact {
    /// Decodes the four-key complete artifact and refuses bytes that are not its own canonical
    /// re-encoding, so one artifact has exactly one byte form and one record content.
    fn decode(bytes: &[u8], context: &'static str) -> Result<Self, CryptoError> {
        let value = decode_canonical(bytes)?;
        require_numeric_keys(&value, SIGNATURE_KEY + 1, context)?;
        let decoded = Self {
            principal: principal_at(&value, 0, context)?,
            device_key: DevicePublicKey(bytes_at::<DEVICE_PUBLIC_KEY_LENGTH>(&value, 1, context)?),
            at_ms: unsigned_at(&value, 2, context)?,
            signature: bytes_at::<SIGNATURE_LENGTH>(&value, SIGNATURE_KEY, context)?,
        };
        let reencoded = encode_complete(
            &decoded.principal,
            decoded.device_key,
            decoded.at_ms,
            &decoded.signature,
        )?;
        if reencoded != bytes {
            return Err(SigningError::NonCanonicalEncoding.into());
        }
        verify_author_binding(&decoded.principal.did, &decoded.principal.public_key)?;
        Ok(decoded)
    }

    /// Whether the principal's own key signed this body under `kind`'s domain.
    fn signature_verifies_as(&self, kind: ArtifactKind) -> Result<bool, CryptoError> {
        let body = encode_body(&self.principal, self.device_key, self.at_ms)?;
        Ok(verify_domain(
            &self.principal.public_key,
            kind.domain(),
            &body,
            &self.signature,
        )
        .is_ok())
    }

    /// The single verification door: DID binding (in `decode`), then the signature under `kind`.
    fn verify_as(bytes: &[u8], kind: ArtifactKind) -> Result<Self, CryptoError> {
        let decoded = Self::decode(bytes, kind.context())?;
        if !decoded.signature_verifies_as(kind)? {
            return Err(SigningError::SignatureVerificationFailed.into());
        }
        if kind == ArtifactKind::Enrolment {
            assert_contributory_device_key(decoded.device_key)?;
        }
        Ok(decoded)
    }
}

/// A principal's statement that `device_key` is one of its devices (R2: self-signed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEnrolmentV1 {
    /// The principal enrolling its own device; only its key may sign this.
    pub principal: Principal,
    /// The device's X25519 public key wraps will be sealed to.
    pub device_key: DevicePublicKey,
    /// Principal-asserted enrolment time; informational, never an authority input.
    pub enrolled_at_ms: u64,
}

impl DeviceEnrolmentV1 {
    /// Canonical bytes of the three-key body.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        encode_body(&self.principal, self.device_key, self.enrolled_at_ms)
    }

    /// `ASCII("fe-device-enrolment-v1") || 0x00 || body`.
    pub fn signature_preimage(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(domain_preimage(
            DEVICE_ENROLMENT_DOMAIN,
            &self.encode_canonical()?,
        ))
    }

    /// Signs as the principal, refusing another key and a device key no wrap could seal to.
    pub fn sign(&self, signing_key: &SigningKey) -> Result<SignedDeviceEnrolment, CryptoError> {
        assert_contributory_device_key(self.device_key)?;
        let signature = sign_as(
            ArtifactKind::Enrolment,
            signing_key,
            &self.principal,
            self.device_key,
            self.enrolled_at_ms,
        )?;
        Ok(SignedDeviceEnrolment {
            body: self.clone(),
            signature,
        })
    }
}

/// A complete enrolment artifact: the body plus the principal's signature (key 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDeviceEnrolment {
    /// Everything the signature covers.
    pub body: DeviceEnrolmentV1,
    /// Ed25519 signature over [`DeviceEnrolmentV1::signature_preimage`].
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl SignedDeviceEnrolment {
    /// Canonical bytes of the four-key complete artifact; what a store or a replica holds.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        encode_complete(
            &self.body.principal,
            self.body.device_key,
            self.body.enrolled_at_ms,
            &self.signature,
        )
    }

    /// The only door to a [`VerifiedDeviceEnrolment`]: canonical re-encode, DID binding, the
    /// principal's own signature under the enrolment domain, and a contributory device key.
    pub fn decode_and_verify(bytes: &[u8]) -> Result<VerifiedDeviceEnrolment, CryptoError> {
        let decoded = DecodedArtifact::verify_as(bytes, ArtifactKind::Enrolment)?;
        Ok(VerifiedDeviceEnrolment::from_decoded(decoded, bytes))
    }
}

/// An enrolment whose bytes verified; [`SignedDeviceEnrolment::decode_and_verify`] is the only
/// constructor, so holding one is proof the principal itself signed it.
///
/// ```compile_fail
/// use fe_canonical_log::crypto::device_enrolment::VerifiedDeviceEnrolment;
/// let _forged = VerifiedDeviceEnrolment { body: todo!(), record_id: todo!(), canonical_bytes: Vec::new() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedDeviceEnrolment {
    body: DeviceEnrolmentV1,
    record_id: Hash32,
    canonical_bytes: Vec<u8>,
}

impl VerifiedDeviceEnrolment {
    fn from_decoded(decoded: DecodedArtifact, bytes: &[u8]) -> Self {
        Self {
            record_id: enrolment_record_id(&decoded.principal, decoded.device_key),
            body: DeviceEnrolmentV1 {
                principal: decoded.principal,
                device_key: decoded.device_key,
                enrolled_at_ms: decoded.at_ms,
            },
            canonical_bytes: bytes.to_vec(),
        }
    }

    /// The verified body.
    pub fn body(&self) -> &DeviceEnrolmentV1 {
        &self.body
    }

    /// The (principal, device) record this enrolment belongs to.
    pub const fn record_id(&self) -> Hash32 {
        self.record_id
    }

    /// The exact verified bytes, for storage or replication.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
}

/// A principal's signed tombstone for one of its devices (R2: revocation = signed tombstone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEnrolmentRevocationV1 {
    /// The principal revoking its own device; only its key may sign this.
    pub principal: Principal,
    /// The X25519 device key being revoked.
    pub device_key: DevicePublicKey,
    /// Principal-asserted revocation time; informational, never an authority input.
    pub revoked_at_ms: u64,
}

impl DeviceEnrolmentRevocationV1 {
    /// Canonical bytes of the three-key body.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        encode_body(&self.principal, self.device_key, self.revoked_at_ms)
    }

    /// `ASCII("fe-device-enrolment-revoke-v1") || 0x00 || body`.
    pub fn signature_preimage(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(domain_preimage(
            DEVICE_ENROLMENT_REVOCATION_DOMAIN,
            &self.encode_canonical()?,
        ))
    }

    /// Signs as the principal, refusing any other key.
    ///
    /// No contributory-key check: revoking a key nobody could seal to is pointless but harmless,
    /// and a principal must never be unable to revoke.
    pub fn sign(
        &self,
        signing_key: &SigningKey,
    ) -> Result<SignedDeviceEnrolmentRevocation, CryptoError> {
        let signature = sign_as(
            ArtifactKind::Revocation,
            signing_key,
            &self.principal,
            self.device_key,
            self.revoked_at_ms,
        )?;
        Ok(SignedDeviceEnrolmentRevocation {
            body: self.clone(),
            signature,
        })
    }
}

/// A complete revocation artifact: the body plus the principal's signature (key 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedDeviceEnrolmentRevocation {
    /// Everything the signature covers.
    pub body: DeviceEnrolmentRevocationV1,
    /// Ed25519 signature over [`DeviceEnrolmentRevocationV1::signature_preimage`].
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl SignedDeviceEnrolmentRevocation {
    /// Canonical bytes of the four-key complete artifact.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        encode_complete(
            &self.body.principal,
            self.body.device_key,
            self.body.revoked_at_ms,
            &self.signature,
        )
    }

    /// The only door to a [`VerifiedDeviceEnrolmentRevocation`]: canonical re-encode, DID
    /// binding, and the principal's own signature under the revocation domain.
    pub fn decode_and_verify(
        bytes: &[u8],
    ) -> Result<VerifiedDeviceEnrolmentRevocation, CryptoError> {
        let decoded = DecodedArtifact::verify_as(bytes, ArtifactKind::Revocation)?;
        Ok(VerifiedDeviceEnrolmentRevocation::from_decoded(
            decoded, bytes,
        ))
    }
}

/// A revocation whose bytes verified; [`SignedDeviceEnrolmentRevocation::decode_and_verify`] is
/// the only constructor.
///
/// ```compile_fail
/// use fe_canonical_log::crypto::device_enrolment::VerifiedDeviceEnrolmentRevocation;
/// let _forged = VerifiedDeviceEnrolmentRevocation { body: todo!(), record_id: todo!(), canonical_bytes: Vec::new() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedDeviceEnrolmentRevocation {
    body: DeviceEnrolmentRevocationV1,
    record_id: Hash32,
    canonical_bytes: Vec<u8>,
}

impl VerifiedDeviceEnrolmentRevocation {
    fn from_decoded(decoded: DecodedArtifact, bytes: &[u8]) -> Self {
        Self {
            record_id: enrolment_record_id(&decoded.principal, decoded.device_key),
            body: DeviceEnrolmentRevocationV1 {
                principal: decoded.principal,
                device_key: decoded.device_key,
                revoked_at_ms: decoded.at_ms,
            },
            canonical_bytes: bytes.to_vec(),
        }
    }

    /// The verified body.
    pub fn body(&self) -> &DeviceEnrolmentRevocationV1 {
        &self.body
    }

    /// The (principal, device) record this revocation tombstones.
    pub const fn record_id(&self) -> Hash32 {
        self.record_id
    }

    /// The exact verified bytes, for storage or replication.
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
}

/// Either verified artifact, as one replicated row or registry slot holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedEnrolmentArtifact {
    /// The principal enrols the device.
    Enrolment(VerifiedDeviceEnrolment),
    /// The principal has revoked the device, permanently.
    Revocation(VerifiedDeviceEnrolmentRevocation),
}

impl VerifiedEnrolmentArtifact {
    /// Verifies bytes of either artifact; the signature domain — never a caller-supplied flag
    /// such as a row's `revoked` column — decides which one it is.
    pub fn decode_and_verify(bytes: &[u8]) -> Result<Self, CryptoError> {
        let decoded = DecodedArtifact::decode(bytes, "device enrolment artifact")?;
        if decoded.signature_verifies_as(ArtifactKind::Enrolment)? {
            assert_contributory_device_key(decoded.device_key)?;
            return Ok(Self::Enrolment(VerifiedDeviceEnrolment::from_decoded(
                decoded, bytes,
            )));
        }
        if decoded.signature_verifies_as(ArtifactKind::Revocation)? {
            return Ok(Self::Revocation(
                VerifiedDeviceEnrolmentRevocation::from_decoded(decoded, bytes),
            ));
        }
        Err(SigningError::SignatureVerificationFailed.into())
    }

    /// The (principal, device) record this artifact belongs to.
    pub const fn record_id(&self) -> Hash32 {
        match self {
            Self::Enrolment(enrolment) => enrolment.record_id,
            Self::Revocation(revocation) => revocation.record_id,
        }
    }

    /// The principal that signed the artifact.
    pub fn principal(&self) -> &Principal {
        match self {
            Self::Enrolment(enrolment) => &enrolment.body.principal,
            Self::Revocation(revocation) => &revocation.body.principal,
        }
    }

    /// The X25519 device key the artifact is about.
    pub const fn device_key(&self) -> DevicePublicKey {
        match self {
            Self::Enrolment(enrolment) => enrolment.body.device_key,
            Self::Revocation(revocation) => revocation.body.device_key,
        }
    }

    /// Whether this is the tombstone.
    pub const fn is_revocation(&self) -> bool {
        matches!(self, Self::Revocation(_))
    }

    /// The exact verified bytes, for storage or replication.
    pub fn canonical_bytes(&self) -> &[u8] {
        match self {
            Self::Enrolment(enrolment) => &enrolment.canonical_bytes,
            Self::Revocation(revocation) => &revocation.canonical_bytes,
        }
    }

    /// The asserted timestamp, used only as a deterministic tie-break between same-class artifacts.
    const fn asserted_at_ms(&self) -> u64 {
        match self {
            Self::Enrolment(enrolment) => enrolment.body.enrolled_at_ms,
            Self::Revocation(revocation) => revocation.body.revoked_at_ms,
        }
    }

    /// Whether this artifact should replace `current` in the same record slot.
    ///
    /// A revocation beats every enrolment, whatever either timestamp says (auth is never LWW);
    /// within one class the earliest assertion, then the smallest bytes, wins. The order is total,
    /// so every arrival order of the same set converges on the same stored artifact.
    fn supersedes(&self, current: &Self) -> bool {
        match (self.is_revocation(), current.is_revocation()) {
            (true, false) => true,
            (false, true) => false,
            _ => {
                (self.asserted_at_ms(), self.canonical_bytes())
                    < (current.asserted_at_ms(), current.canonical_bytes())
            }
        }
    }
}

/// Pure in-memory registry of verified enrolments and revocations, one slot per
/// [`enrolment_record_id`]; the production [`DeviceEnrolmentView`] the durable fe-database view
/// loads its table into.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnrolmentRegistry {
    records: BTreeMap<Hash32, VerifiedEnrolmentArtifact>,
}

impl EnrolmentRegistry {
    /// An empty registry, which answers `Unknown` to everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one verified artifact; returns whether the slot's stored artifact changed.
    ///
    /// A revocation with no prior enrolment is retained, so it still dominates an enrolment
    /// that arrives after it.
    pub fn record(&mut self, artifact: VerifiedEnrolmentArtifact) -> bool {
        let record_id = artifact.record_id();
        if let Some(current) = self.records.get(&record_id) {
            if !artifact.supersedes(current) {
                return false;
            }
        }
        self.records.insert(record_id, artifact);
        true
    }

    /// Records a verified enrolment; see [`EnrolmentRegistry::record`].
    pub fn record_enrolment(&mut self, enrolment: VerifiedDeviceEnrolment) -> bool {
        self.record(VerifiedEnrolmentArtifact::Enrolment(enrolment))
    }

    /// Records a verified revocation; see [`EnrolmentRegistry::record`].
    pub fn record_revocation(&mut self, revocation: VerifiedDeviceEnrolmentRevocation) -> bool {
        self.record(VerifiedEnrolmentArtifact::Revocation(revocation))
    }

    /// The artifact currently held for one record slot.
    pub fn artifact(&self, record_id: &Hash32) -> Option<&VerifiedEnrolmentArtifact> {
        self.records.get(record_id)
    }

    /// Every held artifact, in record-id order.
    pub fn artifacts(&self) -> impl Iterator<Item = &VerifiedEnrolmentArtifact> {
        self.records.values()
    }

    /// Number of (principal, device) slots held.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no slot is held.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl DeviceEnrolmentView for EnrolmentRegistry {
    /// Enrolled and never revoked → `Granted`; revoked → `Refused`; nothing held → `Unknown`.
    ///
    /// A slot found by key whose signer is not exactly `recipient` (a non-canonical DID for the
    /// same key) is `Refused`: the record is about a different principal value than the one asked.
    fn device_enrolment(
        &self,
        recipient: &Principal,
        device_key: DevicePublicKey,
    ) -> AuthorizationRecord {
        match self
            .records
            .get(&enrolment_record_id(recipient, device_key))
        {
            None => AuthorizationRecord::Unknown,
            Some(artifact) if artifact.principal() != recipient => AuthorizationRecord::Refused,
            Some(VerifiedEnrolmentArtifact::Enrolment(_)) => AuthorizationRecord::Granted,
            Some(VerifiedEnrolmentArtifact::Revocation(_)) => AuthorizationRecord::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::crypto::aead::{SealingKey, SEALING_KEY_LENGTH};
    use crate::crypto::key_wrap::{
        open_scope_key, wrap_scope_key, DeviceSecretKey, IssuerAuthorityView,
        RecipientDeviceBinding, ScopeKeyWrapRequest,
    };
    use crate::envelope::{CapabilityRef, Identifier32, Scope};

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn principal(seed: u8) -> Principal {
        Principal::from_public_key(signing_key(seed).verifying_key().to_bytes())
    }

    fn device(seed: u8) -> DeviceSecretKey {
        DeviceSecretKey::from_bytes([seed; DEVICE_PUBLIC_KEY_LENGTH])
    }

    fn enrolment_bytes(principal_seed: u8, device_seed: u8, enrolled_at_ms: u64) -> Vec<u8> {
        DeviceEnrolmentV1 {
            principal: principal(principal_seed),
            device_key: device(device_seed).public_key(),
            enrolled_at_ms,
        }
        .sign(&signing_key(principal_seed))
        .expect("sign enrolment")
        .encode_canonical()
        .expect("encode enrolment")
    }

    fn revocation_bytes(principal_seed: u8, device_seed: u8, revoked_at_ms: u64) -> Vec<u8> {
        DeviceEnrolmentRevocationV1 {
            principal: principal(principal_seed),
            device_key: device(device_seed).public_key(),
            revoked_at_ms,
        }
        .sign(&signing_key(principal_seed))
        .expect("sign revocation")
        .encode_canonical()
        .expect("encode revocation")
    }

    fn verified(bytes: &[u8]) -> VerifiedEnrolmentArtifact {
        VerifiedEnrolmentArtifact::decode_and_verify(bytes).expect("artifact verifies")
    }

    /// Issuer 0x01 is the only Manager+ the recipient trusts.
    struct TrustsIssuer(Principal);

    impl IssuerAuthorityView for TrustsIssuer {
        fn issuer_manager_authority(
            &self,
            issuer: &Principal,
            _: &CapabilityRef,
            _: &Scope,
            _: u64,
        ) -> AuthorizationRecord {
            if *issuer == self.0 {
                AuthorizationRecord::Granted
            } else {
                AuthorizationRecord::Refused
            }
        }
    }

    #[test]
    fn every_domain_separator_is_nul_terminated_ascii_and_none_prefixes_another() {
        let domains = [
            DEVICE_ENROLMENT_DOMAIN,
            DEVICE_ENROLMENT_REVOCATION_DOMAIN,
            ENROLMENT_RECORD_ID_DOMAIN,
        ];
        for domain in domains {
            assert!(domain.ends_with(&[0]) && domain.is_ascii());
            for other in domains {
                if domain != other {
                    assert!(!other.starts_with(domain));
                }
            }
        }
    }

    #[test]
    fn a_wrap_to_an_enrolled_device_opens_and_a_wrap_to_an_unenrolled_one_cannot_be_requested() {
        let recipient = principal(0x02);
        let enrolled_device = device(0x0d);
        let mut registry = EnrolmentRegistry::new();
        assert!(registry.record_enrolment(
            SignedDeviceEnrolment::decode_and_verify(&enrolment_bytes(0x02, 0x0d, 1_000))
                .expect("the principal's own enrolment verifies")
        ));

        let scope = Scope::verse_wide(Identifier32([0x11; 32]));
        let binding = RecipientDeviceBinding::resolve(
            &registry,
            &recipient,
            enrolled_device.public_key(),
            &scope,
            4,
        )
        .expect("the registry enrols this device");
        let scope_key = SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH]);
        let issuer = principal(0x01);
        let wrap = wrap_scope_key(
            &signing_key(0x01),
            &ScopeKeyWrapRequest {
                recipient_device_binding: &binding,
                scope_key: &scope_key,
                issuer: &issuer,
                issuer_capability: CapabilityRef {
                    chain_id: Hash32([0x33; 32]),
                    scope_epoch: 4,
                },
            },
        )
        .expect("wrap");
        assert_eq!(
            open_scope_key(&wrap, &enrolled_device, &TrustsIssuer(issuer)).expect("opens"),
            scope_key
        );

        // A device nobody enrolled, and the enrolled device under a principal that never
        // enrolled it: neither can even be named in a wrap request.
        for (asked_principal, asked_device) in [
            (principal(0x02), device(0x0e).public_key()),
            (principal(0x03), enrolled_device.public_key()),
        ] {
            assert_eq!(
                RecipientDeviceBinding::resolve(
                    &registry,
                    &asked_principal,
                    asked_device,
                    &scope,
                    4
                ),
                Err(CryptoError::DeviceNotEnrolled {
                    record: AuthorizationRecord::Unknown
                })
            );
        }
    }

    #[test]
    fn a_revoked_device_stays_revoked_when_a_stale_enrolment_arrives_later() {
        // The enrolment even claims a LATER time than the revocation: timestamps never decide.
        let enrolment = verified(&enrolment_bytes(0x02, 0x0d, 9_000));
        let revocation = verified(&revocation_bytes(0x02, 0x0d, 5_000));
        assert_eq!(enrolment.record_id(), revocation.record_id());

        let mut enrol_first = EnrolmentRegistry::new();
        assert!(enrol_first.record(enrolment.clone()));
        assert!(enrol_first.record(revocation.clone()));

        let mut revoke_first = EnrolmentRegistry::new();
        assert!(revoke_first.record(revocation.clone()));
        assert!(
            !revoke_first.record(enrolment.clone()),
            "an enrolment never overwrites a revocation"
        );

        assert_eq!(enrol_first, revoke_first, "both arrival orders converge");
        for registry in [&enrol_first, &revoke_first] {
            assert_eq!(
                registry.device_enrolment(&principal(0x02), device(0x0d).public_key()),
                AuthorizationRecord::Refused
            );
            assert_eq!(
                registry.artifact(&revocation.record_id()),
                Some(&revocation)
            );
        }
    }

    #[test]
    fn same_class_artifacts_converge_on_one_stored_artifact_in_any_order() {
        let earlier = verified(&enrolment_bytes(0x02, 0x0d, 1_000));
        let later = verified(&enrolment_bytes(0x02, 0x0d, 2_000));
        let mut forward = EnrolmentRegistry::new();
        forward.record(earlier.clone());
        forward.record(later.clone());
        let mut backward = EnrolmentRegistry::new();
        backward.record(later);
        backward.record(earlier.clone());
        assert_eq!(forward, backward);
        assert_eq!(forward.artifact(&earlier.record_id()), Some(&earlier));
        assert!(!forward.record(earlier), "re-recording is idempotent");
    }

    #[test]
    fn an_enrolment_signed_by_another_principal_does_not_verify() {
        let claimed = DeviceEnrolmentV1 {
            principal: principal(0x02),
            device_key: device(0x0d).public_key(),
            enrolled_at_ms: 1_000,
        };
        // The signing door refuses outright: nobody can enrol a device into someone else.
        assert_eq!(
            claimed.sign(&signing_key(0x01)),
            Err(CryptoError::EnrolmentSignerMismatch)
        );

        // A forger bypassing that door still cannot get its bytes verified.
        let forged = SignedDeviceEnrolment {
            body: claimed.clone(),
            signature: sign_domain(
                &signing_key(0x01),
                DEVICE_ENROLMENT_DOMAIN,
                &claimed.encode_canonical().expect("body"),
            ),
        }
        .encode_canonical()
        .expect("encode");
        let refused = CryptoError::Signing(SigningError::SignatureVerificationFailed);
        assert_eq!(
            SignedDeviceEnrolment::decode_and_verify(&forged).err(),
            Some(refused.clone())
        );
        assert_eq!(
            VerifiedEnrolmentArtifact::decode_and_verify(&forged).err(),
            Some(refused)
        );
    }

    #[test]
    fn malformed_artifacts_are_refused_at_the_verification_door() {
        let enrolment = enrolment_bytes(0x02, 0x0d, 1_000);
        let revocation = revocation_bytes(0x02, 0x0d, 1_000);
        assert!(
            SignedDeviceEnrolmentRevocation::decode_and_verify(&enrolment).is_err(),
            "an enrolment signature does not verify under the revocation domain"
        );

        let mut tampered = enrolment.clone();
        *tampered.last_mut().expect("non-empty") ^= 0x01;

        let mut impostor_did = DeviceEnrolmentV1 {
            principal: principal(0x02),
            device_key: device(0x0d).public_key(),
            enrolled_at_ms: 1_000,
        }
        .sign(&signing_key(0x02))
        .expect("sign");
        impostor_did.body.principal.did = principal(0x03).did;

        let small_order = SignedDeviceEnrolment {
            body: DeviceEnrolmentV1 {
                principal: principal(0x02),
                device_key: DevicePublicKey([0; DEVICE_PUBLIC_KEY_LENGTH]),
                enrolled_at_ms: 1_000,
            },
            signature: [0; SIGNATURE_LENGTH],
        };
        let small_order_signed = SignedDeviceEnrolment {
            signature: sign_domain(
                &signing_key(0x02),
                DEVICE_ENROLMENT_DOMAIN,
                &small_order.body.encode_canonical().expect("body"),
            ),
            ..small_order.clone()
        };

        let cases: [(&str, Vec<u8>); 4] = [
            ("tampered signature", tampered),
            ("a revocation offered as an enrolment", revocation),
            (
                "a DID that does not bind to the key",
                impostor_did.encode_canonical().expect("encode"),
            ),
            (
                "a small-order device key",
                small_order_signed.encode_canonical().expect("encode"),
            ),
        ];
        for (case, bytes) in cases {
            assert!(
                SignedDeviceEnrolment::decode_and_verify(&bytes).is_err(),
                "{case} must not verify as an enrolment"
            );
        }
        assert_eq!(
            small_order.body.sign(&signing_key(0x02)),
            Err(CryptoError::NonContributoryKeyExchange)
        );

        // Trailing bytes after a valid artifact are not that artifact's canonical form.
        let mut padded = enrolment;
        padded.push(0x00);
        assert!(SignedDeviceEnrolment::decode_and_verify(&padded).is_err());
    }

    #[test]
    fn the_unified_door_classifies_by_signature_domain_and_keeps_the_exact_bytes() {
        let enrolment = enrolment_bytes(0x02, 0x0d, 1_000);
        let revocation = revocation_bytes(0x02, 0x0d, 1_000);
        let as_enrolment = verified(&enrolment);
        let as_revocation = verified(&revocation);
        assert!(!as_enrolment.is_revocation());
        assert!(as_revocation.is_revocation());
        assert_eq!(as_enrolment.canonical_bytes(), enrolment.as_slice());
        assert_eq!(as_revocation.canonical_bytes(), revocation.as_slice());
        assert_eq!(
            as_enrolment.record_id(),
            enrolment_record_id(&principal(0x02), device(0x0d).public_key())
        );
        assert_ne!(
            as_enrolment.record_id(),
            enrolment_record_id(&principal(0x03), device(0x0d).public_key()),
            "the same device under another principal is another record"
        );
    }
}
