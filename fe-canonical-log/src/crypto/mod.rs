//! Scope-key AEAD sealing, X25519 HPKE-style key wrap, and scope-key lifecycle
//! (SPEC-1 §5.2/§9, SPEC-3 §10, SPEC-6 §9.1).
//!
//! The only cipher-executing module in this crate: every other module hands ciphertext around
//! as opaque bytes. Rationale, the provisional wrap numbering, and the crypto-shredding limits
//! live in `src/crypto/AGENTS.md`.
//!
//! Pure local key handling: no socket, no listener, no iroh, no libp2p, no `fe-network` and no
//! `fe-sync`. The device-enrolment registry and wrap delivery transport are deliberately absent;
//! `key_wrap`'s view traits are the seams they will answer through, and every one of them is
//! deny-by-default in the meantime.

pub mod aead;
pub mod artifact_aad;
pub mod key_id;
pub mod key_wrap;
pub mod scope_key;

use thiserror::Error;

use crate::capability::CapabilityError;
use crate::cbor::CborError;
use crate::envelope::{EnvelopeError, Identifier32};
use crate::segment::SegmentError;
use crate::signing::SigningError;

pub use aead::{
    AeadSuite, FreshNonce, SealedPayload, SealingKey, XChaCha20Poly1305Suite, AEAD_TAG_LENGTH,
    SEALING_KEY_LENGTH,
};
pub use artifact_aad::StoredArtifactAad;
pub use key_id::{derive_key_id, SCOPE_KEY_ID_DOMAIN};
pub use key_wrap::{
    issue_scope_key_wrap, open_scope_key, wrap_scope_key, AuthorizationRecord,
    DeliveryAuthorizationView, DeviceEnrolmentView, DevicePublicKey, DeviceSecretKey,
    IssuerAuthorityView, KeyWrapAad, RecipientDeviceBinding, ScopeKeyWrap, ScopeKeyWrapBody,
    ScopeKeyWrapRequest, DEVICE_PUBLIC_KEY_LENGTH, KEY_WRAP_PROTOCOL_VERSION,
    SCOPE_KEY_WRAP_DOMAIN, SEALED_SCOPE_KEY_LENGTH,
};
pub use scope_key::{
    EpochBump, InMemoryScopeKeyStore, LaneScopeKeyResolver, ScopeKeyLifecycle, ScopeKeyStore,
};

/// Every reason a sealing, unsealing, key-wrap, or scope-key lifecycle step is refused.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A shared SPEC-1 grammar rule was broken, fixture suite 65535 on a production path included.
    #[error("envelope grammar: {0}")]
    Envelope(#[from] EnvelopeError),
    /// A DID binding or Ed25519 signature check failed.
    #[error("signing: {0}")]
    Signing(#[from] SigningError),
    /// A SPEC-3 scope-key context or topic derivation was refused.
    #[error("capability: {0}")]
    Capability(Box<CapabilityError>),
    /// The SPEC-6 authorization view refused the wrap, seal, or lane.
    #[error("segment: {0}")]
    Segment(Box<SegmentError>),
    /// The CSPRNG failed; SPEC-3 §10.1.2 requires failing closed rather than degrading.
    #[error("the operating-system CSPRNG is unavailable; sealing fails closed")]
    RandomnessUnavailable,
    /// The AEAD refused to produce a ciphertext.
    #[error("authenticated encryption failed")]
    SealFailed,
    /// The Poly1305 tag did not authenticate the ciphertext under this key, nonce, and AAD.
    #[error("authenticated decryption failed")]
    AuthenticationFailed,
    /// A key of the wrong length reached the cipher.
    #[error("a symmetric key of {actual} byte(s) reached the cipher; 32 are required")]
    InvalidKeyLength {
        /// Length actually supplied.
        actual: usize,
    },
    /// A key-wrap artifact declared a protocol version this build does not implement.
    #[error("key-wrap protocol version {version} is not 1")]
    UnsupportedWrapVersion {
        /// The version actually present.
        version: u64,
    },
    /// The device holding this secret is not the device the wrap's associated data names (§10.2.4).
    #[error("this device is not the recipient the key wrap is addressed to")]
    RecipientDeviceMismatch,
    /// The recipient principal does not enrol this X25519 device key (§10.2.1).
    #[error("the recipient principal does not enrol this X25519 device key: {record:?}")]
    DeviceNotEnrolled {
        /// What the enrolment view actually recorded.
        record: AuthorizationRecord,
    },
    /// The persistent view does not record the issuer as Manager+ for this epoch scope (§10.2.1).
    #[error("the key-wrap issuer holds no current Manager+ authority here: {record:?}")]
    IssuerNotAuthorized {
        /// What the authority view actually recorded.
        record: AuthorizationRecord,
    },
    /// The view records no current `decrypt` capability for the wrap recipient (§10.2.1).
    #[error("the key-wrap recipient holds no current decrypt capability here: {record:?}")]
    RecipientNotAuthorized {
        /// What the authority view actually recorded.
        record: AuthorizationRecord,
    },
    /// The signed issuer capability reference names an epoch the wrap does not seal (§10.2.3).
    #[error("the issuer capability names epoch {capability_epoch}; the wrap seals {wrap_epoch}")]
    IssuerCapabilityEpochMismatch {
        /// Epoch the capability reference cites.
        capability_epoch: u64,
        /// Epoch the wrap's associated data actually seals.
        wrap_epoch: u64,
    },
    /// The signing key offered for issuance is not the named issuer principal's key (§10.2.3).
    #[error("the offered signing key is not the issuer principal's own key")]
    IssuerSigningKeyMismatch,
    /// An X25519 exchange produced the all-zero shared secret, so a small-order point was supplied.
    #[error("the X25519 exchange was non-contributory")]
    NonContributoryKeyExchange,
    /// A wrapped scope key did not have the exact 32-byte plaintext / 48-byte sealed length.
    #[error("a wrapped scope key of {actual} byte(s) is not the required length")]
    WrappedKeyLength {
        /// Length actually present.
        actual: usize,
    },
    /// The recovered key does not derive the identifier the artifact declares (§10.1.3).
    #[error("declared key identifier {declared:?} is not the one {derived:?} the key derives")]
    KeyIdentifierMismatch {
        /// Identifier the artifact or authorization view declared.
        declared: Identifier32,
        /// Identifier the key material actually derives.
        derived: Identifier32,
    },
    /// A store already holds a controlled key for this scope and epoch.
    #[error("a controlled scope key for epoch {epoch} already exists; overwriting one is refused")]
    ScopeKeyAlreadyPresent {
        /// The epoch already in custody.
        epoch: u64,
    },
    /// No controlled key exists for this scope and epoch.
    #[error("no controlled scope key for epoch {epoch}")]
    ScopeKeyUnavailable {
        /// The epoch with no controlled key.
        epoch: u64,
    },
    /// Issuance for this scope and epoch was stopped by a bump or a crypto-shred (§10.3.2/§10.3.3).
    #[error("issuance for epoch {epoch} has stopped; no new key or wrap is produced for it")]
    IssuanceStopped {
        /// The epoch whose issuance stopped.
        epoch: u64,
    },
    /// A scope epoch could not be bumped because `u64` has no successor for it.
    #[error("scope epoch {epoch} has no successor")]
    EpochOverflow {
        /// The epoch that could not be bumped.
        epoch: u64,
    },
}

impl From<CapabilityError> for CryptoError {
    /// Boxed so one large sibling taxonomy does not widen every `Result` in this module.
    fn from(error: CapabilityError) -> Self {
        Self::Capability(Box::new(error))
    }
}

impl From<SegmentError> for CryptoError {
    /// Boxed for the same reason as [`CryptoError::Capability`].
    fn from(error: SegmentError) -> Self {
        Self::Segment(Box::new(error))
    }
}
