//! XChaCha20-Poly1305 sealing under a 32-byte scope key (SPEC-1 §3.5/§5.2, SPEC-3 §10.1,
//! SPEC-6 §9.1); see `src/crypto/AGENTS.md` §aead.

use std::fmt;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use zeroize::Zeroize;

use crate::envelope::{
    EncryptionParams, Hash32, Identifier32, PayloadRef, NONCE_LENGTH, PRODUCTION_SUITE_ID,
};

use super::CryptoError;

/// Length in bytes of every symmetric key this module seals under.
pub const SEALING_KEY_LENGTH: usize = 32;

/// Poly1305 authentication-tag length every ciphertext carries on top of its plaintext.
pub const AEAD_TAG_LENGTH: usize = 16;

/// Fills `buffer` from the operating-system CSPRNG, failing closed rather than degrading.
pub(crate) fn fill_random(buffer: &mut [u8]) -> Result<(), CryptoError> {
    OsRng
        .try_fill_bytes(buffer)
        .map_err(|_| CryptoError::RandomnessUnavailable)
}

/// Exact ciphertext length a plaintext of `plaintext_length` bytes seals to.
pub const fn ciphertext_length_for(plaintext_length: usize) -> usize {
    plaintext_length + AEAD_TAG_LENGTH
}

/// A 32-byte symmetric key: a scope key, a derived wrap key, or a derived topic key.
///
/// Zeroized on drop, redacted by `Debug`, and compared in constant time. `expose_bytes` is the
/// one disclosure point, named so every call site reads as a deliberate one.
#[derive(Clone)]
pub struct SealingKey([u8; SEALING_KEY_LENGTH]);

impl SealingKey {
    /// Takes custody of raw key bytes the caller already holds.
    pub fn from_bytes(bytes: [u8; SEALING_KEY_LENGTH]) -> Self {
        Self(bytes)
    }

    /// Draws a fresh CSPRNG key, failing closed when the randomness source fails (§10.1.2).
    pub fn generate() -> Result<Self, CryptoError> {
        let mut bytes = [0u8; SEALING_KEY_LENGTH];
        fill_random(&mut bytes)?;
        let key = Self(bytes);
        bytes.zeroize();
        Ok(key)
    }

    /// Borrows the raw key bytes.
    pub fn expose_bytes(&self) -> &[u8; SEALING_KEY_LENGTH] {
        &self.0
    }
}

impl fmt::Debug for SealingKey {
    /// Never prints key material; a `Debug` derive here would put keys in every error log.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealingKey(redacted)")
    }
}

impl PartialEq for SealingKey {
    /// Constant-time comparison; a short-circuiting byte compare on keys is a timing oracle.
    fn eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |accumulator, (left, right)| {
                accumulator | (left ^ right)
            })
            == 0
    }
}

impl Eq for SealingKey {}

impl Zeroize for SealingKey {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SealingKey {
    /// A bare array drop leaves the bytes readable on the stack or heap; this does not.
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A one-shot sealing grant: one CSPRNG nonce and the §3.5 parameters naming it.
///
/// Neither `Clone` nor `Copy`, and [`seal`] consumes it, so one grant seals at most one
/// plaintext. Nonce reuse under one XChaCha20-Poly1305 key is a keystream-recovery break, so
/// the type system refuses it rather than a ledger detecting it after the fact.
#[derive(Debug)]
pub struct FreshNonce {
    params: EncryptionParams,
}

impl FreshNonce {
    /// Draws a fresh 24-byte CSPRNG nonce for `key_id` under the production suite.
    pub fn draw(key_id: Identifier32) -> Result<Self, CryptoError> {
        let mut nonce = [0u8; NONCE_LENGTH];
        fill_random(&mut nonce)?;
        Ok(Self {
            params: EncryptionParams {
                suite_id: PRODUCTION_SUITE_ID,
                key_id,
                nonce,
            },
        })
    }

    /// Wraps caller-supplied parameters, for the golden-vector oracle and negative tests only.
    ///
    /// [`seal`] still runs `assert_production_suite`, so this cannot smuggle suite 65535 or any
    /// other suite onto a production path.
    pub fn from_params(params: EncryptionParams) -> Self {
        Self { params }
    }

    /// The parameters the associated data must bind before sealing.
    pub fn params(&self) -> EncryptionParams {
        self.params
    }
}

/// One sealed plaintext: the §3.5 parameters it was sealed under and its ciphertext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedPayload {
    /// Suite, key identifier, and the nonce actually used.
    pub encryption: EncryptionParams,
    /// Ciphertext with its appended 16-byte Poly1305 tag.
    pub ciphertext: Vec<u8>,
}

impl SealedPayload {
    /// The §3.5 payload reference committing to this ciphertext, never to its plaintext.
    pub fn payload_ref(&self) -> PayloadRef {
        PayloadRef {
            ciphertext_hash: Hash32::of(&self.ciphertext),
            ciphertext_length: self.ciphertext.len() as u64,
            encryption: Some(self.encryption),
        }
    }
}

/// One authenticated-encryption suite; `suite_id` is the SPEC-1 §3.5 wire selector.
///
/// V1 admits exactly one implementation. The trait exists so a future `protocol_version` can
/// introduce a second suite without a second cipher call site, never so an operation may
/// negotiate one: §9 fixes the suite for the whole protocol version.
///
/// Both methods take the whole `EncryptionParams` rather than a bare nonce so an implementation
/// can refuse a suite it does not answer to. Every implementation MUST call
/// `EncryptionParams::assert_production_suite` (or its own equivalent) first, because this trait
/// is public: were the check to live only in [`seal`] and [`open`], a caller could reach the
/// cipher through the trait and skip it.
pub trait AeadSuite {
    /// The §3.5 `suite_id` this implementation answers to.
    fn suite_id(&self) -> u16;

    /// Seals `plaintext` under `key`, `encryption`, and `associated_data`.
    fn seal(
        &self,
        key: &SealingKey,
        encryption: &EncryptionParams,
        associated_data: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError>;

    /// Opens `ciphertext`, refusing it unless the tag authenticates `associated_data`.
    fn open(
        &self,
        key: &SealingKey,
        encryption: &EncryptionParams,
        associated_data: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError>;
}

/// The one v1 production suite: XChaCha20-Poly1305 with a 192-bit nonce (`suite_id = 1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XChaCha20Poly1305Suite;

impl XChaCha20Poly1305Suite {
    /// Builds the cipher, refusing a key of the wrong length.
    fn cipher(key: &SealingKey) -> Result<XChaCha20Poly1305, CryptoError> {
        XChaCha20Poly1305::new_from_slice(key.expose_bytes()).map_err(|_| {
            CryptoError::InvalidKeyLength {
                actual: key.expose_bytes().len(),
            }
        })
    }
}

impl AeadSuite for XChaCha20Poly1305Suite {
    fn suite_id(&self) -> u16 {
        PRODUCTION_SUITE_ID
    }

    fn seal(
        &self,
        key: &SealingKey,
        encryption: &EncryptionParams,
        associated_data: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        encryption.assert_production_suite()?;
        Self::cipher(key)?
            .encrypt(
                XNonce::from_slice(&encryption.nonce),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| CryptoError::SealFailed)
    }

    fn open(
        &self,
        key: &SealingKey,
        encryption: &EncryptionParams,
        associated_data: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        encryption.assert_production_suite()?;
        Self::cipher(key)?
            .decrypt(
                XNonce::from_slice(&encryption.nonce),
                Payload {
                    msg: ciphertext,
                    aad: associated_data,
                },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)
    }
}

/// The ONE seal entry point in this crate; it refuses every non-production suite first.
///
/// `associated_data` is opaque here on purpose. An operation payload supplies
/// [`crate::payload_aad::payload_aad_preimage`]; a stored artifact supplies
/// [`super::artifact_aad::StoredArtifactAad::preimage`]. Keeping the construction outside means
/// this function cannot be specialized into a variant that skips one of them, and the
/// `assert_production_suite` call below is unconditional rather than a caller's option.
pub fn seal(
    key: &SealingKey,
    nonce: FreshNonce,
    associated_data: &[u8],
    plaintext: &[u8],
) -> Result<SealedPayload, CryptoError> {
    let encryption = nonce.params;
    encryption.assert_production_suite()?;
    let ciphertext = XChaCha20Poly1305Suite.seal(key, &encryption, associated_data, plaintext)?;
    Ok(SealedPayload {
        encryption,
        ciphertext,
    })
}

/// The ONE open entry point in this crate; it refuses every non-production suite first.
///
/// Suite 65535 remains decodable by [`EncryptionParams::from_cbor`] so the golden-vector oracle
/// still reads its fixture, and is rejected here so it never reaches a cipher.
pub fn open(
    key: &SealingKey,
    encryption: &EncryptionParams,
    associated_data: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    encryption.assert_production_suite()?;
    XChaCha20Poly1305Suite.open(key, encryption, associated_data, ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::envelope::{
        Author, CapabilityRef, EnvelopeError, Hlc, Scope, UnsignedEnvelope, FIXTURE_ONLY_SUITE_ID,
        PROTOCOL_VERSION,
    };
    use crate::payload_aad::payload_aad_preimage;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn scope_key() -> SealingKey {
        SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH])
    }

    /// A payload-bearing envelope whose encryption map names `params`.
    ///
    /// The §5.2 AAD omits ciphertext hash and length, so the placeholder values here cannot
    /// affect the preimage; the envelope signature is what binds those two.
    fn unsigned_with(params: EncryptionParams) -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 1,
            scope: Scope::verse_wide(identifier(0x11)),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 1,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: identifier(0x55),
            parents: vec![Hash32([0x66; 32])],
            hlc: Hlc::new(9, 1),
            payload: PayloadRef {
                ciphertext_hash: Hash32::of(&[0xc0, 0xff, 0xee]),
                ciphertext_length: 3,
                encryption: Some(params),
            },
        }
    }

    /// The §5.2 flow: draw the nonce, bind it into the envelope, then build the AAD.
    fn payload_associated_data(params: EncryptionParams) -> Vec<u8> {
        payload_aad_preimage(&unsigned_with(params)).expect("payload AAD")
    }

    #[test]
    fn payload_round_trips_under_its_spec1_associated_data() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let params = nonce.params();
        let associated_data = payload_associated_data(params);

        let sealed = seal(&key, nonce, &associated_data, b"canonical payload").expect("seal");
        assert_eq!(sealed.encryption, params);
        assert_eq!(
            open(
                &key,
                &sealed.encryption,
                &associated_data,
                &sealed.ciphertext
            )
            .expect("open"),
            b"canonical payload".to_vec()
        );
    }

    #[test]
    fn the_payload_reference_commits_to_ciphertext_not_plaintext() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let params = nonce.params();
        let associated_data = payload_associated_data(params);
        let sealed = seal(&key, nonce, &associated_data, b"payload").expect("seal");

        let reference = sealed.payload_ref();
        assert_eq!(reference.ciphertext_hash, Hash32::of(&sealed.ciphertext));
        assert_ne!(reference.ciphertext_hash, Hash32::of(b"payload"));
        assert_eq!(reference.ciphertext_length, sealed.ciphertext.len() as u64);
        assert_eq!(reference.encryption, Some(params));
    }

    #[test]
    fn ciphertext_carries_exactly_the_poly1305_tag_overhead() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let associated_data = payload_associated_data(nonce.params());
        let sealed = seal(&key, nonce, &associated_data, b"twelve bytes").expect("seal");
        assert_eq!(sealed.ciphertext.len(), ciphertext_length_for(12));
    }

    #[test]
    fn open_refuses_a_tampered_ciphertext() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let associated_data = payload_associated_data(nonce.params());
        let mut sealed = seal(&key, nonce, &associated_data, b"canonical payload").expect("seal");
        sealed.ciphertext[0] ^= 0x01;

        assert_eq!(
            open(
                &key,
                &sealed.encryption,
                &associated_data,
                &sealed.ciphertext
            ),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn open_refuses_a_tampered_authentication_tag() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let associated_data = payload_associated_data(nonce.params());
        let mut sealed = seal(&key, nonce, &associated_data, b"canonical payload").expect("seal");
        let last = sealed.ciphertext.len() - 1;
        sealed.ciphertext[last] ^= 0x80;

        assert_eq!(
            open(
                &key,
                &sealed.encryption,
                &associated_data,
                &sealed.ciphertext
            ),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn open_refuses_different_associated_data() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let associated_data = payload_associated_data(nonce.params());
        let sealed = seal(&key, nonce, &associated_data, b"canonical payload").expect("seal");

        let mut other = associated_data.clone();
        other.push(0x00);
        assert_eq!(
            open(&key, &sealed.encryption, &other, &sealed.ciphertext),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn open_refuses_a_different_scope_key() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let associated_data = payload_associated_data(nonce.params());
        let sealed = seal(&key, nonce, &associated_data, b"canonical payload").expect("seal");

        let other = SealingKey::from_bytes([0x5b; SEALING_KEY_LENGTH]);
        assert_eq!(
            open(
                &other,
                &sealed.encryption,
                &associated_data,
                &sealed.ciphertext
            ),
            Err(CryptoError::AuthenticationFailed)
        );
    }

    #[test]
    fn repeated_seals_draw_distinct_nonces() {
        let key_id = identifier(0x77);
        let mut nonces = std::collections::BTreeSet::new();
        for _ in 0..64 {
            let nonce = FreshNonce::draw(key_id).expect("nonce");
            let params = nonce.params();
            assert_eq!(params.suite_id, PRODUCTION_SUITE_ID);
            assert_eq!(params.key_id, key_id);
            assert!(
                nonces.insert(params.nonce),
                "the CSPRNG repeated a 192-bit nonce under one key"
            );
            // Consume the grant so the one-shot contract is exercised, not just the draw.
            let sealed = seal(&scope_key(), nonce, b"aad", b"plaintext").expect("seal");
            assert_eq!(sealed.encryption.nonce, params.nonce);
        }
        assert_eq!(nonces.len(), 64);
    }

    #[test]
    fn seal_refuses_the_fixture_only_suite() {
        let fixture = EncryptionParams {
            suite_id: FIXTURE_ONLY_SUITE_ID,
            key_id: identifier(0x77),
            nonce: [0x88; NONCE_LENGTH],
        };
        assert_eq!(
            seal(
                &scope_key(),
                FreshNonce::from_params(fixture),
                b"aad",
                b"plaintext"
            ),
            Err(CryptoError::Envelope(EnvelopeError::FixtureOnlySuite))
        );
    }

    #[test]
    fn open_refuses_the_fixture_only_suite() {
        let key = scope_key();
        let nonce = FreshNonce::draw(identifier(0x77)).expect("nonce");
        let sealed = seal(&key, nonce, b"aad", b"plaintext").expect("seal");

        let fixture = EncryptionParams {
            suite_id: FIXTURE_ONLY_SUITE_ID,
            ..sealed.encryption
        };
        assert_eq!(
            open(&key, &fixture, b"aad", &sealed.ciphertext),
            Err(CryptoError::Envelope(EnvelopeError::FixtureOnlySuite))
        );
    }

    #[test]
    fn seal_and_open_refuse_an_unregistered_suite() {
        let unknown = EncryptionParams {
            suite_id: 2,
            key_id: identifier(0x77),
            nonce: [0x88; NONCE_LENGTH],
        };
        let expected =
            CryptoError::Envelope(EnvelopeError::UnsupportedPayloadSuite { suite_id: 2 });
        assert_eq!(
            seal(
                &scope_key(),
                FreshNonce::from_params(unknown),
                b"aad",
                b"plaintext"
            )
            .unwrap_err(),
            expected
        );
        assert_eq!(
            open(&scope_key(), &unknown, b"aad", b"ciphertext").unwrap_err(),
            expected
        );
    }

    #[test]
    fn the_production_suite_answers_to_suite_id_one() {
        assert_eq!(XChaCha20Poly1305Suite.suite_id(), PRODUCTION_SUITE_ID);
    }

    /// The trait is public, so reaching the cipher through it must not skip the suite check.
    #[test]
    fn the_trait_surface_refuses_the_fixture_only_suite_too() {
        let fixture = EncryptionParams {
            suite_id: FIXTURE_ONLY_SUITE_ID,
            key_id: identifier(0x77),
            nonce: [0x88; NONCE_LENGTH],
        };
        let expected = CryptoError::Envelope(EnvelopeError::FixtureOnlySuite);
        assert_eq!(
            XChaCha20Poly1305Suite
                .seal(&scope_key(), &fixture, b"aad", b"plaintext")
                .unwrap_err(),
            expected
        );
        assert_eq!(
            XChaCha20Poly1305Suite
                .open(&scope_key(), &fixture, b"aad", b"ciphertext")
                .unwrap_err(),
            expected
        );
    }

    #[test]
    fn a_sealing_key_zeroizes_and_never_prints_its_bytes() {
        let mut key = SealingKey::from_bytes([0xab; SEALING_KEY_LENGTH]);
        assert_eq!(format!("{key:?}"), "SealingKey(redacted)");
        assert!(!format!("{key:?}").contains("171"));

        key.zeroize();
        assert_eq!(key.expose_bytes(), &[0u8; SEALING_KEY_LENGTH]);
    }

    #[test]
    fn sealing_keys_compare_by_value() {
        assert_eq!(
            SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH])
        );
        assert_ne!(
            SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            SealingKey::from_bytes([0x02; SEALING_KEY_LENGTH])
        );
    }

    #[test]
    fn generated_keys_are_distinct_and_non_trivial() {
        let first = SealingKey::generate().expect("key");
        let second = SealingKey::generate().expect("key");
        assert_ne!(first, second);
        assert_ne!(first.expose_bytes(), &[0u8; SEALING_KEY_LENGTH]);
    }
}
