//! X25519 HPKE-style recipient-device scope-key wrap (SPEC-3 §10.2); see
//! `src/crypto/AGENTS.md` §key-wrap for the provisional numbering and the no-`hpke`-crate note.
//!
//! Protocol only. The device-enrolment *registry*, wrap delivery, and every transport remain
//! owner-gated and unbuilt: nothing here opens a socket or names `fe-network`, `fe-sync`, iroh,
//! or libp2p. [`DeviceEnrolmentView`] and [`IssuerAuthorityView`] are the seams that registry
//! and a recipient's persistent view will answer through; they are questions, not storage.

use std::fmt;

use ed25519_dalek::SigningKey;
use x25519_dalek::{PublicKey, SharedSecret, StaticSecret};
use zeroize::Zeroize;

use crate::capability::{
    bytes_at, entry, principal_at, require_numeric_keys, scope_at, unsigned_at, Principal,
};
use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{
    CapabilityRef, EncryptionParams, Identifier32, Scope, NONCE_LENGTH, PRODUCTION_SUITE_ID,
    SIGNATURE_LENGTH,
};
use crate::segment::hashseq::LaneKey;
use crate::segment::relay_policy::{
    authorize_scope_key_wrap, PeerIdentity, RelayAuthorizationView,
};
use crate::signing::{domain_preimage, sign_domain, verify_author_binding, verify_domain};

use super::aead::{self, fill_random, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
use super::key_id::derive_key_id;
use super::CryptoError;

/// NUL-terminated ASCII domain separator for the §10.2.2 wrap-key KDF and the §10.2.3 signature.
pub const SCOPE_KEY_WRAP_DOMAIN: &[u8] = b"fe-scope-key-wrap-v1\0";

/// The only key-wrap protocol version v1 admits (§10.2.3 key 0).
pub const KEY_WRAP_PROTOCOL_VERSION: u64 = 1;

/// Length of an enrolled X25519 device public key.
pub const DEVICE_PUBLIC_KEY_LENGTH: usize = 32;

/// Exact sealed length of a 32-byte scope key plus its 16-byte Poly1305 tag (§10.2.2).
pub const SEALED_SCOPE_KEY_LENGTH: usize = SEALING_KEY_LENGTH + 16;

const AAD_CONTEXT: &str = "scope key wrap associated data";
const BODY_CONTEXT: &str = "scope key wrap";

/// A recipient device's enrolled X25519 public key (§10.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DevicePublicKey(
    /// Raw Montgomery-u public key bytes.
    pub [u8; DEVICE_PUBLIC_KEY_LENGTH],
);

/// A device's X25519 private key; `StaticSecret` zeroizes its buffer on drop.
///
/// `StaticSecret` rather than `EphemeralSecret` even for the per-delivery ephemeral key, because
/// `EphemeralSecret` cannot be built from bytes and so cannot be pinned in a test vector. The
/// one-delivery discipline is enforced by [`wrap_scope_key`] generating a new one every call.
pub struct DeviceSecretKey(StaticSecret);

impl DeviceSecretKey {
    /// Draws a fresh CSPRNG device key, failing closed when the randomness source fails.
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = [0u8; DEVICE_PUBLIC_KEY_LENGTH];
        fill_random(&mut bytes)?;
        let secret = StaticSecret::from(bytes);
        bytes.zeroize();
        Ok(Self(secret))
    }

    /// Takes custody of raw private-key bytes an enrolled device already holds.
    pub fn from_bytes(bytes: [u8; DEVICE_PUBLIC_KEY_LENGTH]) -> Self {
        Self(StaticSecret::from(bytes))
    }

    /// The public key this device enrols against its principal (§10.2.1).
    pub fn public_key(&self) -> DevicePublicKey {
        DevicePublicKey(PublicKey::from(&self.0).to_bytes())
    }

    /// X25519 with a peer key, refusing the all-zero non-contributory result.
    fn diffie_hellman(&self, peer: DevicePublicKey) -> Result<SharedSecret, CryptoError> {
        let shared = self.0.diffie_hellman(&PublicKey::from(peer.0));
        if !shared.was_contributory() {
            return Err(CryptoError::NonContributoryKeyExchange);
        }
        Ok(shared)
    }
}

impl fmt::Debug for DeviceSecretKey {
    /// Never prints key material.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceSecretKey(redacted)")
    }
}

/// What a persistent view records for one §10.2.1 binding or authority question.
///
/// Three answers rather than a bool: a view that holds no record has not said "no", and keeping
/// the two apart is what stops a missing answer from being read as an allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationRecord {
    /// The view resolves the subject and currently records the binding or authority.
    Granted,
    /// The view resolves the subject and does not record it.
    Refused,
    /// The view holds no record to answer with; a missing answer is never an allow.
    Unknown,
}

impl AuthorizationRecord {
    /// Whether this is the one answer that authorizes.
    pub const fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// The recipient-side view §10.2.3 requires: "validate the issuer's authority before unwrapping".
///
/// A wrap carries its issuer's capability reference in the signed body, but a reference is a
/// claim, not an answer; only a persistent view can say whether that issuer is Manager+ for the
/// scope and epoch *now*. [`open_scope_key`] takes one, so possession of a wrap cannot be the
/// thing that opens it.
pub trait IssuerAuthorityView {
    /// Whether `issuer` currently holds Manager+ authority for `scope` at `scope_epoch` under
    /// the chain `capability` names (§10.2.1, §10.3.1).
    fn issuer_manager_authority(
        &self,
        issuer: &Principal,
        capability: &CapabilityRef,
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationRecord;
}

/// The §10.2.1 record of which X25519 device keys a principal has enrolled.
///
/// No such registry exists in this workspace: §10.2.4 delivery and device enrolment are
/// owner-gated and unbuilt. This trait is here so the absence is a view a caller must name and
/// implement rather than a check nobody notices is missing.
pub trait DeviceEnrolmentView {
    /// Whether `recipient` currently enrols exactly `device_key` as one of its devices.
    fn device_enrolment(
        &self,
        recipient: &Principal,
        device_key: DevicePublicKey,
    ) -> AuthorizationRecord;
}

/// Everything a delivery issuer MUST consult before any key material is touched (§10.2.1).
pub trait DeliveryAuthorizationView: IssuerAuthorityView + DeviceEnrolmentView {
    /// Whether `recipient` holds a currently valid `decrypt` capability for this exact scope
    /// and epoch.
    fn recipient_decrypt_authority(
        &self,
        recipient: &Principal,
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationRecord;
}

/// A device key an enrolment view confirmed, bound to the one delivery it was resolved for.
///
/// Fields are private and [`RecipientDeviceBinding::resolve`] is the only constructor, so a
/// caller cannot hand [`wrap_scope_key`] a device key the recipient never enrolled: that call
/// has no spelling. The scope and epoch travel inside the binding and are the wrap's only
/// source for them, so a binding resolved for one epoch cannot be re-aimed at another either.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecipientDeviceBinding {
    recipient: Principal,
    device_key: DevicePublicKey,
    scope: Scope,
    scope_epoch: u64,
}

impl RecipientDeviceBinding {
    /// Resolves one delivery's device binding, refusing anything but a recorded enrolment.
    pub fn resolve(
        enrolment: &impl DeviceEnrolmentView,
        recipient: &Principal,
        device_key: DevicePublicKey,
        scope: &Scope,
        scope_epoch: u64,
    ) -> Result<Self, CryptoError> {
        verify_author_binding(&recipient.did, &recipient.public_key)?;
        let record = enrolment.device_enrolment(recipient, device_key);
        if !record.is_granted() {
            return Err(CryptoError::DeviceNotEnrolled { record });
        }
        Ok(Self {
            recipient: recipient.clone(),
            device_key,
            scope: *scope,
            scope_epoch,
        })
    }

    /// The principal this device is enrolled under.
    pub fn recipient(&self) -> &Principal {
        &self.recipient
    }

    /// The enrolled X25519 device key the wrap seals to.
    pub const fn device_key(&self) -> DevicePublicKey {
        self.device_key
    }

    /// Scope the delivery seals for.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    /// Epoch the delivery seals for.
    pub const fn scope_epoch(&self) -> u64 {
        self.scope_epoch
    }
}

/// The seven-key §10.2.3 `canonical_key_wrap_aad` map.
///
/// Every binding a recipient must check lives here, and the wrap key derives from these exact
/// bytes, so altering any binding also destroys the ability to decrypt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyWrapAad {
    /// Protocol version; always [`KEY_WRAP_PROTOCOL_VERSION`].
    pub protocol_version: u64,
    /// Scope the wrapped key seals.
    pub scope: Scope,
    /// Epoch the wrapped key belongs to.
    pub scope_epoch: u64,
    /// Identifier the wrapped key derives (§10.1.3).
    pub key_id: Identifier32,
    /// Principal the wrap is addressed to.
    pub recipient: Principal,
    /// The recipient device's enrolled X25519 public key.
    pub recipient_device_key: DevicePublicKey,
    /// The issuer's fresh per-delivery ephemeral X25519 public key.
    pub ephemeral_public_key: DevicePublicKey,
}

impl KeyWrapAad {
    /// Encodes the seven-key §10.2.3 map.
    pub fn to_cbor(&self) -> Result<CborValue, CryptoError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.protocol_version)),
            (CborValue::Uint(1), self.scope.to_cbor()?),
            (CborValue::Uint(2), CborValue::Uint(self.scope_epoch)),
            (CborValue::Uint(3), CborValue::Bytes(self.key_id.0.to_vec())),
            (CborValue::Uint(4), self.recipient.to_cbor()),
            (
                CborValue::Uint(5),
                CborValue::Bytes(self.recipient_device_key.0.to_vec()),
            ),
            (
                CborValue::Uint(6),
                CborValue::Bytes(self.ephemeral_public_key.0.to_vec()),
            ),
        ]))
    }

    /// Decodes the seven-key §10.2.3 map, refusing any protocol version but 1.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CryptoError> {
        require_numeric_keys(value, 7, AAD_CONTEXT)?;
        let protocol_version = unsigned_at(value, 0, AAD_CONTEXT)?;
        if protocol_version != KEY_WRAP_PROTOCOL_VERSION {
            return Err(CryptoError::UnsupportedWrapVersion {
                version: protocol_version,
            });
        }
        Ok(Self {
            protocol_version,
            scope: scope_at(value, 1, AAD_CONTEXT)?,
            scope_epoch: unsigned_at(value, 2, AAD_CONTEXT)?,
            key_id: Identifier32(bytes_at::<32>(value, 3, AAD_CONTEXT)?),
            recipient: principal_at(value, 4, AAD_CONTEXT)?,
            recipient_device_key: DevicePublicKey(bytes_at::<DEVICE_PUBLIC_KEY_LENGTH>(
                value,
                5,
                AAD_CONTEXT,
            )?),
            ephemeral_public_key: DevicePublicKey(bytes_at::<DEVICE_PUBLIC_KEY_LENGTH>(
                value,
                6,
                AAD_CONTEXT,
            )?),
        })
    }

    /// Canonical bytes of the associated-data map.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// `ASCII("fe-scope-key-wrap-v1") || 0x00 || canonical_key_wrap_aad` (§10.2.2).
    pub fn preimage(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(domain_preimage(
            SCOPE_KEY_WRAP_DOMAIN,
            &self.encode_canonical()?,
        ))
    }
}

/// The §10.2.3 `canonical_complete_wrap`: exactly what the issuer signature covers.
///
/// The signature slot is outside this map, mirroring SPEC-1 §5.1's unsigned/complete split;
/// including it would make the signature cover itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeKeyWrapBody {
    /// The seven-key associated data the AEAD and the KDF both bind.
    pub associated_data: KeyWrapAad,
    /// Fresh CSPRNG 192-bit nonce the wrap was sealed under.
    pub wrap_nonce: [u8; NONCE_LENGTH],
    /// The 32-byte scope key sealed under the derived wrap key, tag included.
    pub sealed_key: [u8; SEALED_SCOPE_KEY_LENGTH],
    /// The Manager+ principal that issued this delivery.
    pub issuer: Principal,
    /// The issuer's capability chain reference for the epoch scope.
    pub issuer_capability: CapabilityRef,
}

impl ScopeKeyWrapBody {
    /// Encodes the five-key signed body.
    pub fn to_cbor(&self) -> Result<CborValue, CryptoError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), self.associated_data.to_cbor()?),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.wrap_nonce.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.sealed_key.to_vec()),
            ),
            (CborValue::Uint(3), self.issuer.to_cbor()),
            (CborValue::Uint(4), self.issuer_capability.to_cbor()),
        ]))
    }

    /// Decodes the five-key signed body.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CryptoError> {
        require_numeric_keys(value, 5, BODY_CONTEXT)?;
        Self::decode_fields(value)
    }

    /// Reads keys 0 through 4 without asserting the map's total key count.
    ///
    /// Shared by the five-key body and the six-key complete artifact, so one grammar change
    /// cannot alter only one of them.
    fn decode_fields(value: &CborValue) -> Result<Self, CryptoError> {
        Ok(Self {
            associated_data: KeyWrapAad::from_cbor(entry(value, 0, BODY_CONTEXT)?)?,
            wrap_nonce: bytes_at::<NONCE_LENGTH>(value, 1, BODY_CONTEXT)?,
            sealed_key: bytes_at::<SEALED_SCOPE_KEY_LENGTH>(value, 2, BODY_CONTEXT)?,
            issuer: principal_at(value, 3, BODY_CONTEXT)?,
            issuer_capability: CapabilityRef::from_cbor(entry(value, 4, BODY_CONTEXT)?)?,
        })
    }

    /// Canonical bytes of the signed body.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// `ASCII("fe-scope-key-wrap-v1") || 0x00 || canonical_complete_wrap` (§10.2.3).
    pub fn signature_preimage(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(domain_preimage(
            SCOPE_KEY_WRAP_DOMAIN,
            &self.encode_canonical()?,
        ))
    }
}

/// A complete §10.2.3 key-wrap artifact: the signed body plus the issuer's Ed25519 signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeKeyWrap {
    /// Everything the signature covers.
    pub body: ScopeKeyWrapBody,
    /// Ed25519 signature over [`ScopeKeyWrapBody::signature_preimage`].
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl ScopeKeyWrap {
    /// Encodes the six-key complete artifact.
    pub fn to_cbor(&self) -> Result<CborValue, CryptoError> {
        let CborValue::Map(mut entries) = self.body.to_cbor()? else {
            unreachable!("ScopeKeyWrapBody::to_cbor always builds a map")
        };
        entries.push((
            CborValue::Uint(5),
            CborValue::Bytes(self.signature.to_vec()),
        ));
        Ok(CborValue::Map(entries))
    }

    /// Decodes the six-key complete artifact.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CryptoError> {
        require_numeric_keys(value, 6, BODY_CONTEXT)?;
        Ok(Self {
            body: ScopeKeyWrapBody::decode_fields(value)?,
            signature: bytes_at::<SIGNATURE_LENGTH>(value, 5, BODY_CONTEXT)?,
        })
    }

    /// Canonical bytes of the complete artifact.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CryptoError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CryptoError> {
        Self::from_cbor(&decode_canonical(bytes)?)
    }

    /// Verifies the issuer DID binding and the §10.2.3 signature over the complete wrap.
    pub fn verify_issuer_signature(&self) -> Result<(), CryptoError> {
        verify_author_binding(&self.body.issuer.did, &self.body.issuer.public_key)?;
        verify_domain(
            &self.body.issuer.public_key,
            SCOPE_KEY_WRAP_DOMAIN,
            &self.body.encode_canonical()?,
            &self.signature,
        )?;
        Ok(())
    }
}

/// Everything one delivery needs, so the entry points stay a single argument.
///
/// The recipient, its device key, the scope, and the epoch all come from one resolved
/// [`RecipientDeviceBinding`]. There is no second place to state them, so no two of them can
/// disagree and no caller can address a wrap to a device the enrolment view never confirmed.
#[derive(Clone, Debug)]
pub struct ScopeKeyWrapRequest<'a> {
    /// The resolved recipient, device key, scope, and epoch this delivery is addressed to.
    pub recipient_device_binding: &'a RecipientDeviceBinding,
    /// The 32-byte scope key being delivered.
    pub scope_key: &'a SealingKey,
    /// The Manager+ principal issuing the delivery.
    pub issuer: &'a Principal,
    /// The issuer's capability chain reference for the epoch scope.
    pub issuer_capability: CapabilityRef,
}

impl ScopeKeyWrapRequest<'_> {
    /// Principal the wrap is addressed to, read from the resolved binding.
    pub fn recipient(&self) -> &Principal {
        self.recipient_device_binding.recipient()
    }

    /// Scope the wrapped key seals, read from the resolved binding.
    pub fn scope(&self) -> &Scope {
        self.recipient_device_binding.scope()
    }

    /// Epoch the wrapped key belongs to, read from the resolved binding.
    pub fn scope_epoch(&self) -> u64 {
        self.recipient_device_binding.scope_epoch()
    }
}

/// `BLAKE3_keyed(shared_secret, "fe-scope-key-wrap-v1" || 0x00 || canonical_key_wrap_aad)`.
fn derive_wrap_key(
    shared_secret: &SharedSecret,
    associated_data: &KeyWrapAad,
) -> Result<SealingKey, CryptoError> {
    let preimage = associated_data.preimage()?;
    Ok(SealingKey::from_bytes(
        *blake3::keyed_hash(shared_secret.as_bytes(), &preimage).as_bytes(),
    ))
}

/// Seals exactly the 32 scope-key bytes under the wrap key derived for `associated_data`.
///
/// The associated data is a parameter rather than being rebuilt here, so the malicious-issuer
/// test can bind a `key_id` that disagrees with the key it seals and still produce a wrap the
/// AEAD accepts. That is the only way [`open_scope_key`]'s `key_id` check is reachable.
fn seal_scope_key(
    ephemeral: &DeviceSecretKey,
    scope_key: &SealingKey,
    associated_data: &KeyWrapAad,
) -> Result<([u8; NONCE_LENGTH], [u8; SEALED_SCOPE_KEY_LENGTH]), CryptoError> {
    let shared_secret = ephemeral.diffie_hellman(associated_data.recipient_device_key)?;
    let wrap_key = derive_wrap_key(&shared_secret, associated_data)?;
    let nonce = FreshNonce::draw(associated_data.key_id)?;
    let wrap_nonce = nonce.params().nonce;
    let sealed = aead::seal(
        &wrap_key,
        nonce,
        &associated_data.preimage()?,
        scope_key.expose_bytes(),
    )?;
    let sealed_key: [u8; SEALED_SCOPE_KEY_LENGTH] = sealed
        .ciphertext
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::WrappedKeyLength {
            actual: sealed.ciphertext.len(),
        })?;
    Ok((wrap_nonce, sealed_key))
}

/// Refuses a signing key that is not the named issuer's own key.
///
/// Without this, issuance produces a well-formed wrap whose signature no recipient can verify,
/// and the defect surfaces only at the far end of a delivery channel — or, worse, is read as a
/// recipient-side fault.
fn assert_issuer_signs_as_itself(
    issuer_signing_key: &SigningKey,
    issuer: &Principal,
) -> Result<(), CryptoError> {
    verify_author_binding(&issuer.did, &issuer.public_key)?;
    if issuer_signing_key.verifying_key().to_bytes() != issuer.public_key {
        return Err(CryptoError::IssuerSigningKeyMismatch);
    }
    Ok(())
}

/// Refuses a capability reference naming any epoch but the one the wrap seals (§10.2.3).
///
/// The reference is signed over, so an issuer citing epoch `e` while delivering the `e + 1` key
/// would otherwise be authenticated by a check that never read it.
fn assert_capability_names_this_epoch(
    capability: &CapabilityRef,
    scope_epoch: u64,
) -> Result<(), CryptoError> {
    if capability.scope_epoch != scope_epoch {
        return Err(CryptoError::IssuerCapabilityEpochMismatch {
            capability_epoch: capability.scope_epoch,
            wrap_epoch: scope_epoch,
        });
    }
    Ok(())
}

/// Turns one view answer into a refusal, so `Unknown` has no path to an allow.
fn require_granted(
    record: AuthorizationRecord,
    refusal: impl FnOnce(AuthorizationRecord) -> CryptoError,
) -> Result<(), CryptoError> {
    if record.is_granted() {
        Ok(())
    } else {
        Err(refusal(record))
    }
}

/// Builds and signs one §10.2 recipient-device wrap with a fresh ephemeral key pair.
///
/// The recipient-device binding is structural here — [`ScopeKeyWrapRequest`] cannot carry an
/// unresolved one — and the issuer must hold the key it signs under. This function still asks
/// **no authority question**: it never checks whether the issuer is Manager+ or whether the
/// recipient may decrypt. [`issue_scope_key_wrap`] is the entry point a delivery issuer uses;
/// this one exists for callers that have already run §10.2.1 through a different authorization
/// surface, and for the conformance vectors. A new caller reaching for it is the thing to
/// question — this codebase's recurring defect is the gate that is built but never wired.
pub fn wrap_scope_key(
    issuer_signing_key: &SigningKey,
    request: &ScopeKeyWrapRequest<'_>,
) -> Result<ScopeKeyWrap, CryptoError> {
    let binding = request.recipient_device_binding;
    assert_issuer_signs_as_itself(issuer_signing_key, request.issuer)?;
    assert_capability_names_this_epoch(&request.issuer_capability, binding.scope_epoch())?;
    let ephemeral = DeviceSecretKey::generate()?;
    let associated_data = KeyWrapAad {
        protocol_version: KEY_WRAP_PROTOCOL_VERSION,
        scope: *binding.scope(),
        scope_epoch: binding.scope_epoch(),
        key_id: derive_key_id(binding.scope(), binding.scope_epoch(), request.scope_key)?,
        recipient: binding.recipient().clone(),
        recipient_device_key: binding.device_key(),
        ephemeral_public_key: ephemeral.public_key(),
    };
    let (wrap_nonce, sealed_key) = seal_scope_key(&ephemeral, request.scope_key, &associated_data)?;
    let body = ScopeKeyWrapBody {
        associated_data,
        wrap_nonce,
        sealed_key,
        issuer: request.issuer.clone(),
        issuer_capability: request.issuer_capability,
    };
    let signature = sign_domain(
        issuer_signing_key,
        SCOPE_KEY_WRAP_DOMAIN,
        &body.encode_canonical()?,
    );
    Ok(ScopeKeyWrap { body, signature })
}

/// The ONE issuance entry point (§10.2.1, §10.3.1): authorization first, key material second.
///
/// In order: the capability reference names the epoch being sealed, the persistent §10.2.1 view
/// records the issuer as Manager+ for that epoch scope and the recipient as currently able to
/// decrypt it, the SPEC-6 view still reports this peer as wrappable for the lane's current
/// epoch, and the key the caller supplies derives the identifier that view reports. Nothing is
/// derived until every one of them passes.
///
/// The device binding needs no check here: `request` cannot carry an unresolved one. The two
/// views are separate arguments because they are separate persistent surfaces — SPEC-6 §9.3
/// lane currency is not SPEC-3 §10.2.1 enrolment, and neither can answer the other's question.
pub fn issue_scope_key_wrap(
    view: &impl RelayAuthorizationView,
    delivery: &impl DeliveryAuthorizationView,
    lane: &LaneKey,
    issuer_signing_key: &SigningKey,
    request: &ScopeKeyWrapRequest<'_>,
) -> Result<ScopeKeyWrap, CryptoError> {
    let binding = request.recipient_device_binding;
    let scope_epoch = binding.scope_epoch();
    assert_capability_names_this_epoch(&request.issuer_capability, scope_epoch)?;
    require_granted(
        delivery.issuer_manager_authority(
            request.issuer,
            &request.issuer_capability,
            binding.scope(),
            scope_epoch,
        ),
        |record| CryptoError::IssuerNotAuthorized { record },
    )?;
    require_granted(
        delivery.recipient_decrypt_authority(binding.recipient(), binding.scope(), scope_epoch),
        |record| CryptoError::RecipientNotAuthorized { record },
    )?;
    // SPEC-6 §9.3 identifies a peer by its Ed25519 principal key; the X25519 device key is the
    // separate §10.2.1 binding above. Reading this one check as covering both was the defect.
    let peer = PeerIdentity(binding.recipient().public_key);
    let current_key_id = authorize_scope_key_wrap(view, &peer, lane, scope_epoch)?;
    let derived = derive_key_id(binding.scope(), scope_epoch, request.scope_key)?;
    if derived != current_key_id {
        return Err(CryptoError::KeyIdentifierMismatch {
            declared: current_key_id,
            derived,
        });
    }
    wrap_scope_key(issuer_signing_key, request)
}

/// Recovers the scope key from a wrap, verifying every §10.2.3 binding and the issuer's
/// authority before trusting it.
///
/// Possession proves nothing, and neither does a valid signature: a signature says only that the
/// named issuer produced these bytes, not that it was ever entitled to. `issuer_authority` is the
/// persistent view that answers the second question, and there is no way to call this without
/// one. Checked, in order: the issuer DID binding and signature, the protocol version, the
/// recipient principal binding, this device's key, that the signed capability reference names the
/// epoch being delivered, that the view records the issuer as Manager+ for that scope and epoch,
/// the contributory X25519 exchange, the AEAD tag, the recovered plaintext length, and the
/// recovered key's own derived `key_id`. The last is what stops a *properly authorized* issuer
/// from delivering key material under someone else's advertised identifier.
///
/// One binding is deliberately not checked: that the wrap's recipient principal is this device's
/// own principal. `DeviceSecretKey` carries no principal, the AAD's recipient key is what the
/// wrap key derives from, and the `key_id` check already pins the recovered material to the
/// scope and epoch — so a mis-named recipient yields the same genuine key, not a foreign one.
pub fn open_scope_key(
    wrap: &ScopeKeyWrap,
    recipient_device_secret: &DeviceSecretKey,
    issuer_authority: &impl IssuerAuthorityView,
) -> Result<SealingKey, CryptoError> {
    wrap.verify_issuer_signature()?;
    let associated_data = &wrap.body.associated_data;
    if associated_data.protocol_version != KEY_WRAP_PROTOCOL_VERSION {
        return Err(CryptoError::UnsupportedWrapVersion {
            version: associated_data.protocol_version,
        });
    }
    verify_author_binding(
        &associated_data.recipient.did,
        &associated_data.recipient.public_key,
    )?;
    if recipient_device_secret.public_key() != associated_data.recipient_device_key {
        return Err(CryptoError::RecipientDeviceMismatch);
    }
    assert_capability_names_this_epoch(&wrap.body.issuer_capability, associated_data.scope_epoch)?;
    require_granted(
        issuer_authority.issuer_manager_authority(
            &wrap.body.issuer,
            &wrap.body.issuer_capability,
            &associated_data.scope,
            associated_data.scope_epoch,
        ),
        |record| CryptoError::IssuerNotAuthorized { record },
    )?;

    let shared_secret =
        recipient_device_secret.diffie_hellman(associated_data.ephemeral_public_key)?;
    let wrap_key = derive_wrap_key(&shared_secret, associated_data)?;
    let encryption = EncryptionParams {
        suite_id: PRODUCTION_SUITE_ID,
        key_id: associated_data.key_id,
        nonce: wrap.body.wrap_nonce,
    };
    let mut recovered = aead::open(
        &wrap_key,
        &encryption,
        &associated_data.preimage()?,
        &wrap.body.sealed_key,
    )?;
    let outcome = adopt_recovered_scope_key(&recovered, associated_data);
    recovered.zeroize();
    outcome
}

/// Turns recovered plaintext into a scope key only if it derives the identifier the AAD names.
fn adopt_recovered_scope_key(
    recovered: &[u8],
    associated_data: &KeyWrapAad,
) -> Result<SealingKey, CryptoError> {
    let mut bytes: [u8; SEALING_KEY_LENGTH] =
        recovered
            .try_into()
            .map_err(|_| CryptoError::WrappedKeyLength {
                actual: recovered.len(),
            })?;
    let scope_key = SealingKey::from_bytes(bytes);
    bytes.zeroize();

    let derived = derive_key_id(
        &associated_data.scope,
        associated_data.scope_epoch,
        &scope_key,
    )?;
    if derived != associated_data.key_id {
        // `scope_key` is dropped here, and its drop zeroizes the material we refuse to trust.
        return Err(CryptoError::KeyIdentifierMismatch {
            declared: associated_data.key_id,
            derived,
        });
    }
    Ok(scope_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::envelope::Hash32;
    use crate::segment::payload_shard::PayloadTopicScope;
    use crate::segment::SegmentError;
    use crate::signing::SigningError;

    fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn principal(seed: u8) -> Principal {
        Principal::from_public_key(signing_key(seed).verifying_key().to_bytes())
    }

    fn verse(filler: u8) -> Scope {
        Scope::verse_wide(Identifier32([filler; 32]))
    }

    fn scope_key() -> SealingKey {
        SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH])
    }

    fn capability_reference(scope_epoch: u64) -> CapabilityRef {
        CapabilityRef {
            chain_id: Hash32([0x33; 32]),
            scope_epoch,
        }
    }

    fn device() -> DeviceSecretKey {
        DeviceSecretKey::from_bytes([0x0d; DEVICE_PUBLIC_KEY_LENGTH])
    }

    /// A stand-in for the persistent §10.2.1 view; every list is deny-by-default.
    ///
    /// `unrecorded` is the third answer: a principal the view cannot resolve at all, which must
    /// refuse exactly as loudly as one it resolves and denies.
    #[derive(Clone, Debug, Default)]
    struct DeliveryView {
        enrolments: Vec<(Principal, DevicePublicKey)>,
        manager_issuers: Vec<Principal>,
        decrypting_recipients: Vec<Principal>,
        unrecorded: Vec<Principal>,
    }

    impl DeliveryView {
        /// The view every happy path runs against: issuer 0x01 manages, recipient 0x02 decrypts
        /// and enrols `device()`.
        fn permissive() -> Self {
            Self {
                enrolments: vec![(principal(0x02), device().public_key())],
                manager_issuers: vec![principal(0x01)],
                decrypting_recipients: vec![principal(0x02)],
                unrecorded: Vec::new(),
            }
        }

        fn answer(&self, subject: &Principal, granted: bool) -> AuthorizationRecord {
            if self.unrecorded.contains(subject) {
                AuthorizationRecord::Unknown
            } else if granted {
                AuthorizationRecord::Granted
            } else {
                AuthorizationRecord::Refused
            }
        }
    }

    impl IssuerAuthorityView for DeliveryView {
        fn issuer_manager_authority(
            &self,
            issuer: &Principal,
            _: &CapabilityRef,
            _: &Scope,
            _: u64,
        ) -> AuthorizationRecord {
            self.answer(issuer, self.manager_issuers.contains(issuer))
        }
    }

    impl DeviceEnrolmentView for DeliveryView {
        fn device_enrolment(
            &self,
            recipient: &Principal,
            device_key: DevicePublicKey,
        ) -> AuthorizationRecord {
            let enrolled = self
                .enrolments
                .iter()
                .any(|(enrolled_by, key)| enrolled_by == recipient && *key == device_key);
            self.answer(recipient, enrolled)
        }
    }

    impl DeliveryAuthorizationView for DeliveryView {
        fn recipient_decrypt_authority(
            &self,
            recipient: &Principal,
            _: &Scope,
            _: u64,
        ) -> AuthorizationRecord {
            self.answer(recipient, self.decrypting_recipients.contains(recipient))
        }
    }

    /// Recipient 0x02 on `device()`, resolved against the permissive view.
    fn enrolled_binding(scope: &Scope, scope_epoch: u64) -> RecipientDeviceBinding {
        RecipientDeviceBinding::resolve(
            &DeliveryView::permissive(),
            &principal(0x02),
            device().public_key(),
            scope,
            scope_epoch,
        )
        .expect("the permissive view enrols this device")
    }

    fn request<'a>(
        binding: &'a RecipientDeviceBinding,
        scope_key: &'a SealingKey,
        issuer: &'a Principal,
    ) -> ScopeKeyWrapRequest<'a> {
        ScopeKeyWrapRequest {
            recipient_device_binding: binding,
            scope_key,
            issuer,
            issuer_capability: capability_reference(binding.scope_epoch()),
        }
    }

    /// A wrap addressed to `device()` under scope `verse(0x11)` epoch 4.
    fn wrap() -> ScopeKeyWrap {
        let scope = verse(0x11);
        let binding = enrolled_binding(&scope, 4);
        let material = scope_key();
        let issuer = principal(0x01);
        wrap_scope_key(&signing_key(0x01), &request(&binding, &material, &issuer)).expect("wrap")
    }

    /// A persistent SPEC-6 lane view with one wrappable device and one current key.
    struct View {
        lane: LaneKey,
        current_epoch: u64,
        current_key_id: Identifier32,
        wrappable: Vec<PeerIdentity>,
    }

    impl RelayAuthorizationView for View {
        fn current_scope_epoch(&self, lane: &LaneKey) -> Option<u64> {
            (lane == &self.lane).then_some(self.current_epoch)
        }

        fn current_key_id(&self, lane: &LaneKey) -> Option<Identifier32> {
            (lane == &self.lane).then_some(self.current_key_id)
        }

        fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }

        fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }

        fn may_wrap_scope_key_for_peer(
            &self,
            device: &PeerIdentity,
            lane: &LaneKey,
            scope_epoch: u64,
        ) -> bool {
            lane == &self.lane
                && scope_epoch == self.current_epoch
                && self.wrappable.contains(device)
        }
    }

    fn header_lane() -> LaneKey {
        LaneKey::Header {
            verse_id: Identifier32([0x11; 32]),
        }
    }

    /// The lane view every issuance happy path runs against.
    fn lane_view(current_key_id: Identifier32) -> View {
        View {
            lane: header_lane(),
            current_epoch: 4,
            current_key_id,
            wrappable: vec![PeerIdentity(principal(0x02).public_key)],
        }
    }

    #[test]
    fn the_domain_separator_is_nul_terminated_ascii() {
        assert_eq!(
            hex::encode(SCOPE_KEY_WRAP_DOMAIN),
            "66652d73636f70652d6b65792d777261702d763100"
        );
        assert!(SCOPE_KEY_WRAP_DOMAIN.ends_with(&[0]));
    }

    #[test]
    fn wrap_and_open_round_trip_the_scope_key() {
        let wrap = wrap();
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()).expect("open"),
            scope_key()
        );
    }

    #[test]
    fn a_sealed_scope_key_is_exactly_forty_eight_bytes() {
        assert_eq!(SEALED_SCOPE_KEY_LENGTH, 48);
        assert_eq!(wrap().body.sealed_key.len(), 48);
    }

    #[test]
    fn every_delivery_uses_a_fresh_ephemeral_key_and_nonce() {
        let first = wrap();
        let second = wrap();
        assert_ne!(
            first.body.associated_data.ephemeral_public_key,
            second.body.associated_data.ephemeral_public_key
        );
        assert_ne!(first.body.wrap_nonce, second.body.wrap_nonce);
        assert_ne!(first.body.sealed_key, second.body.sealed_key);
        // Both still open to the same key: the wrap is randomized, not the material.
        assert_eq!(
            open_scope_key(&first, &device(), &DeliveryView::permissive()).expect("open"),
            open_scope_key(&second, &device(), &DeliveryView::permissive()).expect("open")
        );
    }

    #[test]
    fn the_associated_data_binds_every_section_ten_two_three_field() {
        let wrap = wrap();
        let associated_data = &wrap.body.associated_data;
        assert_eq!(associated_data.protocol_version, KEY_WRAP_PROTOCOL_VERSION);
        assert_eq!(associated_data.scope, verse(0x11));
        assert_eq!(associated_data.scope_epoch, 4);
        assert_eq!(
            associated_data.key_id,
            derive_key_id(&verse(0x11), 4, &scope_key()).expect("key id")
        );
        assert_eq!(associated_data.recipient, principal(0x02));
        assert_eq!(associated_data.recipient_device_key, device().public_key());
        assert_eq!(wrap.body.issuer, principal(0x01));
        assert_eq!(wrap.body.issuer_capability, capability_reference(4));
    }

    /// The recipient, device key, scope, and epoch have exactly one source: the resolved binding.
    #[test]
    fn a_wrap_addresses_exactly_the_binding_that_was_resolved() {
        let scope = Scope::new(
            Identifier32([0x11; 32]),
            Some(Identifier32([0x22; 32])),
            None,
        )
        .expect("petal scope");
        let binding = enrolled_binding(&scope, 9);
        let material = scope_key();
        let issuer = principal(0x01);
        let wrap = wrap_scope_key(
            &signing_key(0x01),
            &ScopeKeyWrapRequest {
                recipient_device_binding: &binding,
                scope_key: &material,
                issuer: &issuer,
                issuer_capability: capability_reference(9),
            },
        )
        .expect("wrap");

        let associated_data = &wrap.body.associated_data;
        assert_eq!(&associated_data.scope, binding.scope());
        assert_eq!(associated_data.scope_epoch, binding.scope_epoch());
        assert_eq!(&associated_data.recipient, binding.recipient());
        assert_eq!(associated_data.recipient_device_key, binding.device_key());
    }

    #[test]
    fn a_wrap_encodes_and_decodes_canonically() {
        let original = wrap();
        let bytes = original.encode_canonical().expect("bytes");
        let decoded = ScopeKeyWrap::decode_canonical(&bytes).expect("decoded");
        assert_eq!(decoded, original);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
        decoded.verify_issuer_signature().expect("signature");
    }

    #[test]
    fn the_signed_body_is_the_complete_wrap_without_its_signature() {
        let wrap = wrap();
        let body_bytes = wrap.body.encode_canonical().expect("body");
        assert_eq!(
            wrap.body.signature_preimage().expect("preimage"),
            domain_preimage(SCOPE_KEY_WRAP_DOMAIN, &body_bytes)
        );
        assert_eq!(
            ScopeKeyWrapBody::from_cbor(&wrap.body.to_cbor().expect("cbor")).expect("body"),
            wrap.body
        );
    }

    // §10.2.1 device binding: sealing to an unenrolled device has no spelling.

    #[test]
    fn a_device_the_recipient_never_enrolled_cannot_be_bound() {
        let stranger_device = DeviceSecretKey::from_bytes([0x0e; DEVICE_PUBLIC_KEY_LENGTH]);
        assert_eq!(
            RecipientDeviceBinding::resolve(
                &DeliveryView::permissive(),
                &principal(0x02),
                stranger_device.public_key(),
                &verse(0x11),
                4,
            ),
            Err(CryptoError::DeviceNotEnrolled {
                record: AuthorizationRecord::Refused
            })
        );
    }

    #[test]
    fn one_principals_device_key_does_not_bind_to_another_principal() {
        // `device()` is enrolled, but by principal 0x02 and not by 0x03.
        assert_eq!(
            RecipientDeviceBinding::resolve(
                &DeliveryView::permissive(),
                &principal(0x03),
                device().public_key(),
                &verse(0x11),
                4,
            ),
            Err(CryptoError::DeviceNotEnrolled {
                record: AuthorizationRecord::Refused
            })
        );
    }

    #[test]
    fn a_view_that_cannot_answer_refuses_the_binding_rather_than_allowing_it() {
        let mut view = DeliveryView::permissive();
        view.unrecorded.push(principal(0x02));
        assert_eq!(
            RecipientDeviceBinding::resolve(
                &view,
                &principal(0x02),
                device().public_key(),
                &verse(0x11),
                4,
            ),
            Err(CryptoError::DeviceNotEnrolled {
                record: AuthorizationRecord::Unknown
            })
        );
    }

    #[test]
    fn a_binding_refuses_a_recipient_whose_did_does_not_bind_to_its_key() {
        let impostor = Principal {
            did: principal(0x02).did,
            public_key: principal(0x03).public_key,
        };
        assert!(matches!(
            RecipientDeviceBinding::resolve(
                &DeliveryView::permissive(),
                &impostor,
                device().public_key(),
                &verse(0x11),
                4,
            ),
            Err(CryptoError::Signing(_))
        ));
    }

    // §10.2.3 issuance: the issuer signs as itself, for the epoch it cites.

    #[test]
    fn wrapping_refuses_a_signing_key_that_is_not_the_issuers() {
        let scope = verse(0x11);
        let binding = enrolled_binding(&scope, 4);
        let material = scope_key();
        let issuer = principal(0x01);
        assert_eq!(
            wrap_scope_key(&signing_key(0x09), &request(&binding, &material, &issuer)),
            Err(CryptoError::IssuerSigningKeyMismatch)
        );
    }

    #[test]
    fn wrapping_refuses_a_capability_reference_for_another_epoch() {
        let scope = verse(0x11);
        let binding = enrolled_binding(&scope, 4);
        let material = scope_key();
        let issuer = principal(0x01);
        assert_eq!(
            wrap_scope_key(
                &signing_key(0x01),
                &ScopeKeyWrapRequest {
                    recipient_device_binding: &binding,
                    scope_key: &material,
                    issuer: &issuer,
                    issuer_capability: capability_reference(3),
                },
            ),
            Err(CryptoError::IssuerCapabilityEpochMismatch {
                capability_epoch: 3,
                wrap_epoch: 4,
            })
        );
    }

    // §10.2.3 opening: possession is not authority.

    #[test]
    fn open_refuses_a_wrap_addressed_to_another_device() {
        let wrap = wrap();
        let stranger = DeviceSecretKey::from_bytes([0x0e; DEVICE_PUBLIC_KEY_LENGTH]);
        assert_eq!(
            open_scope_key(&wrap, &stranger, &DeliveryView::permissive()),
            Err(CryptoError::RecipientDeviceMismatch)
        );
    }

    /// The wrap is intact and its signature verifies; only the issuer's standing is gone.
    #[test]
    fn open_refuses_a_wrap_whose_issuer_the_view_no_longer_records_as_manager_plus() {
        let wrap = wrap();
        wrap.verify_issuer_signature()
            .expect("the artifact itself is sound");

        let mut demoted = DeliveryView::permissive();
        demoted.manager_issuers.clear();
        assert_eq!(
            open_scope_key(&wrap, &device(), &demoted),
            Err(CryptoError::IssuerNotAuthorized {
                record: AuthorizationRecord::Refused
            })
        );
    }

    #[test]
    fn open_refuses_a_wrap_whose_issuer_the_view_cannot_resolve_at_all() {
        let wrap = wrap();
        let mut silent = DeliveryView::permissive();
        silent.unrecorded.push(principal(0x01));
        assert_eq!(
            open_scope_key(&wrap, &device(), &silent),
            Err(CryptoError::IssuerNotAuthorized {
                record: AuthorizationRecord::Unknown
            })
        );
    }

    /// A signed capability reference nobody reads is not a check; this is the read.
    #[test]
    fn open_refuses_a_wrap_whose_capability_reference_names_another_epoch() {
        let mut wrap = wrap();
        wrap.body.issuer_capability = capability_reference(3);
        wrap.signature = sign_domain(
            &signing_key(0x01),
            SCOPE_KEY_WRAP_DOMAIN,
            &wrap.body.encode_canonical().expect("body"),
        );
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()),
            Err(CryptoError::IssuerCapabilityEpochMismatch {
                capability_epoch: 3,
                wrap_epoch: 4,
            })
        );
    }

    #[test]
    fn open_refuses_a_tampered_issuer_signature() {
        let mut wrap = wrap();
        wrap.signature[0] ^= 0x01;
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()),
            Err(CryptoError::Signing(
                SigningError::SignatureVerificationFailed
            ))
        );
    }

    #[test]
    fn open_refuses_a_signature_from_another_issuer() {
        // The body names issuer 0x01; a different key re-signed it after the fact.
        let mut wrap = wrap();
        wrap.signature = sign_domain(
            &signing_key(0x09),
            SCOPE_KEY_WRAP_DOMAIN,
            &wrap.body.encode_canonical().expect("body"),
        );
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()),
            Err(CryptoError::Signing(
                SigningError::SignatureVerificationFailed
            ))
        );
    }

    #[test]
    fn open_refuses_a_tampered_sealed_key() {
        let mut wrap = wrap();
        wrap.body.sealed_key[0] ^= 0x01;
        // Re-sign so the tamper is caught by the AEAD, not by the signature check.
        wrap.signature = sign_domain(
            &signing_key(0x01),
            SCOPE_KEY_WRAP_DOMAIN,
            &wrap.body.encode_canonical().expect("body"),
        );
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn open_refuses_a_tampered_associated_data_binding() {
        let mut wrap = wrap();
        // Move the epoch coherently, capability reference included, so the AEAD is what refuses
        // rather than one of the cheap structural checks in front of it.
        wrap.body.associated_data.scope_epoch = 5;
        wrap.body.issuer_capability = capability_reference(5);
        wrap.signature = sign_domain(
            &signing_key(0x01),
            SCOPE_KEY_WRAP_DOMAIN,
            &wrap.body.encode_canonical().expect("body"),
        );
        // The wrap key derives from the AAD, so altering a binding also destroys decryption.
        assert_eq!(
            open_scope_key(&wrap, &device(), &DeliveryView::permissive()),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn open_refuses_a_key_whose_derived_identifier_is_not_the_one_declared() {
        // A malicious issuer: the AAD advertises one identifier, the sealed key derives another.
        let scope = verse(0x11);
        let material = scope_key();
        let honest = derive_key_id(&scope, 4, &material).expect("key id");
        let lying = Identifier32([0xff; 32]);
        assert_ne!(honest, lying);

        let ephemeral = DeviceSecretKey::from_bytes([0x0a; DEVICE_PUBLIC_KEY_LENGTH]);
        let associated_data = KeyWrapAad {
            protocol_version: KEY_WRAP_PROTOCOL_VERSION,
            scope,
            scope_epoch: 4,
            key_id: lying,
            recipient: principal(0x02),
            recipient_device_key: device().public_key(),
            ephemeral_public_key: ephemeral.public_key(),
        };
        let (wrap_nonce, sealed_key) =
            seal_scope_key(&ephemeral, &material, &associated_data).expect("seal");
        let body = ScopeKeyWrapBody {
            associated_data,
            wrap_nonce,
            sealed_key,
            issuer: principal(0x01),
            issuer_capability: capability_reference(4),
        };
        let signature = sign_domain(
            &signing_key(0x01),
            SCOPE_KEY_WRAP_DOMAIN,
            &body.encode_canonical().expect("body"),
        );

        assert_eq!(
            open_scope_key(
                &ScopeKeyWrap { body, signature },
                &device(),
                &DeliveryView::permissive()
            ),
            Err(CryptoError::KeyIdentifierMismatch {
                declared: lying,
                derived: honest,
            })
        );
    }

    #[test]
    fn the_associated_data_refuses_an_unknown_protocol_version() {
        let wrap = wrap();
        let CborValue::Map(mut entries) = wrap
            .body
            .associated_data
            .to_cbor()
            .expect("associated data cbor")
        else {
            unreachable!("the associated data is always a map")
        };
        entries[0] = (CborValue::Uint(0), CborValue::Uint(2));
        assert_eq!(
            KeyWrapAad::from_cbor(&CborValue::Map(entries)),
            Err(CryptoError::UnsupportedWrapVersion { version: 2 })
        );
    }

    // §10.2.1 issuance: three persistent views, none of them optional.

    #[test]
    fn scope_key_wrap_requires_current_recipient_authorization() {
        let scope = verse(0x11);
        let material = scope_key();
        let removed = principal(0x03);
        let issuer = principal(0x01);
        let view = lane_view(derive_key_id(&scope, 4, &material).expect("key id"));

        // The §10.2.1 view is content with both principals, so the SPEC-6 lane view is the only
        // surface that can refuse either of them.
        let mut delivery = DeliveryView::permissive();
        delivery
            .enrolments
            .push((removed.clone(), device().public_key()));
        delivery.decrypting_recipients.push(removed.clone());

        let binding = enrolled_binding(&scope, 4);
        issue_scope_key_wrap(
            &view,
            &delivery,
            &header_lane(),
            &signing_key(0x01),
            &request(&binding, &material, &issuer),
        )
        .expect("the authorized recipient receives a wrap");

        // A principal the lane view no longer reports as wrappable gets nothing.
        let removed_binding =
            RecipientDeviceBinding::resolve(&delivery, &removed, device().public_key(), &scope, 4)
                .expect("the removed member is still enrolled; only the lane view refuses");
        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &delivery,
                &header_lane(),
                &signing_key(0x01),
                &request(&removed_binding, &material, &issuer),
            ),
            Err(CryptoError::Segment(Box::new(SegmentError::Unauthorized)))
        );

        // A superseded epoch gets nothing either, even for an authorized recipient.
        let stale_binding = enrolled_binding(&scope, 3);
        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &delivery,
                &header_lane(),
                &signing_key(0x01),
                &request(&stale_binding, &material, &issuer),
            ),
            Err(CryptoError::Segment(Box::new(
                SegmentError::StaleScopeEpoch {
                    requested: 3,
                    current: 4,
                }
            )))
        );
    }

    #[test]
    fn issuance_refuses_an_issuer_without_current_manager_authority() {
        let scope = verse(0x11);
        let material = scope_key();
        let issuer = principal(0x01);
        let binding = enrolled_binding(&scope, 4);
        let view = lane_view(derive_key_id(&scope, 4, &material).expect("key id"));
        let mut demoted = DeliveryView::permissive();
        demoted.manager_issuers.clear();

        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &demoted,
                &header_lane(),
                &signing_key(0x01),
                &request(&binding, &material, &issuer),
            ),
            Err(CryptoError::IssuerNotAuthorized {
                record: AuthorizationRecord::Refused
            })
        );
    }

    #[test]
    fn issuance_refuses_a_recipient_without_a_current_decrypt_capability() {
        let scope = verse(0x11);
        let material = scope_key();
        let issuer = principal(0x01);
        let binding = enrolled_binding(&scope, 4);
        let view = lane_view(derive_key_id(&scope, 4, &material).expect("key id"));
        let mut no_decrypt = DeliveryView::permissive();
        no_decrypt.decrypting_recipients.clear();

        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &no_decrypt,
                &header_lane(),
                &signing_key(0x01),
                &request(&binding, &material, &issuer),
            ),
            Err(CryptoError::RecipientNotAuthorized {
                record: AuthorizationRecord::Refused
            })
        );
    }

    #[test]
    fn issuance_refuses_a_key_that_is_not_the_lanes_current_key() {
        let scope = verse(0x11);
        let material = scope_key();
        let issuer = principal(0x01);
        let binding = enrolled_binding(&scope, 4);
        let stale_key_id = Identifier32([0xee; 32]);
        let view = lane_view(stale_key_id);

        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &DeliveryView::permissive(),
                &header_lane(),
                &signing_key(0x01),
                &request(&binding, &material, &issuer),
            ),
            Err(CryptoError::KeyIdentifierMismatch {
                declared: stale_key_id,
                derived: derive_key_id(&scope, 4, &material).expect("key id"),
            })
        );
    }

    #[test]
    fn issuance_refuses_a_lane_the_view_does_not_know() {
        let scope = verse(0x11);
        let material = scope_key();
        let issuer = principal(0x01);
        let binding = enrolled_binding(&scope, 4);
        let view = lane_view(derive_key_id(&scope, 4, &material).expect("key id"));
        let other_lane = LaneKey::Payload(PayloadTopicScope {
            verse_id: Identifier32([0x11; 32]),
            petal_id: Identifier32([0x22; 32]),
            scope_epoch: 4,
            key_id: Identifier32([0x77; 32]),
        });

        assert_eq!(
            issue_scope_key_wrap(
                &view,
                &DeliveryView::permissive(),
                &other_lane,
                &signing_key(0x01),
                &request(&binding, &material, &issuer),
            ),
            Err(CryptoError::Segment(Box::new(SegmentError::UnknownLane)))
        );
    }

    #[test]
    fn an_authorization_record_authorizes_in_exactly_one_state() {
        assert!(AuthorizationRecord::Granted.is_granted());
        assert!(!AuthorizationRecord::Refused.is_granted());
        assert!(!AuthorizationRecord::Unknown.is_granted());
    }

    #[test]
    fn a_device_secret_key_never_prints_its_bytes() {
        assert_eq!(format!("{:?}", device()), "DeviceSecretKey(redacted)");
    }

    #[test]
    fn the_signature_slot_is_the_declared_ed25519_length() {
        assert_eq!(wrap().signature.len(), SIGNATURE_LENGTH);
    }
}
