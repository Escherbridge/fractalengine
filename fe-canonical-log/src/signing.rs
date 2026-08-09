//! Signature preimage, Ed25519 signing, and `op_id` derivation (SPEC-1 §5.1); see `src/AGENTS.md`.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use thiserror::Error;

use crate::did_key::{did_to_public_key, public_key_to_did, DidKeyError};
use crate::envelope::{
    CompleteEnvelope, EnvelopeError, Hash32, UnsignedEnvelope, SIGNATURE_LENGTH,
};
use crate::kind::{validate_structural_rules, StructuralRuleError};

/// NUL-terminated ASCII domain separator for operation-envelope signatures (§5.1).
pub const SIGNATURE_DOMAIN: &[u8] = b"fe-oplog-v1\0";

/// Every reason signing or verifying an operation envelope fails.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SigningError {
    /// The envelope could not be encoded or decoded canonically.
    #[error("envelope: {0}")]
    Envelope(#[from] EnvelopeError),
    /// The author DID could not be decoded.
    #[error("author DID: {0}")]
    DidKey(#[from] DidKeyError),
    /// The key derived from the author DID differed from `author.public_key` (§3.2).
    #[error("author DID does not bind to author.public_key")]
    AuthorBindingMismatch,
    /// The author DID was not the canonical `did:key` text for `author.public_key` (§3.2).
    #[error("author DID is not the canonical did:key encoding of author.public_key")]
    NonCanonicalAuthorDid,
    /// The 32 bytes were not a valid Ed25519 public key.
    #[error("author public key is not a valid Ed25519 point")]
    InvalidPublicKey,
    /// The signature did not verify over the §5.1 preimage.
    #[error("Ed25519 signature verification failed")]
    SignatureVerificationFailed,
    /// The envelope broke a §6 structural rule.
    #[error("structural rule: {0}")]
    Structural(#[from] StructuralRuleError),
    /// The received bytes were not the canonical encoding of the envelope they decode to.
    #[error("received bytes are not the canonical encoding of the envelope they decode to")]
    NonCanonicalEncoding,
}

/// Builds `domain || body`, where `domain` is ASCII already terminated by its NUL byte.
pub fn domain_preimage(domain: &[u8], body: &[u8]) -> Vec<u8> {
    debug_assert!(
        domain.ends_with(&[0]),
        "a domain separator must carry its terminating NUL byte"
    );
    let mut preimage = Vec::with_capacity(domain.len() + body.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(body);
    preimage
}

/// Signs `domain || body` for any canonical-log artifact domain.
pub fn sign_domain(signing_key: &SigningKey, domain: &[u8], body: &[u8]) -> [u8; SIGNATURE_LENGTH] {
    signing_key.sign(&domain_preimage(domain, body)).to_bytes()
}

/// Verifies a signature over `domain || body` for any canonical-log artifact domain.
pub fn verify_domain(
    public_key: &[u8; 32],
    domain: &[u8],
    body: &[u8],
    signature: &[u8; SIGNATURE_LENGTH],
) -> Result<(), SigningError> {
    let verifying_key =
        VerifyingKey::from_bytes(public_key).map_err(|_| SigningError::InvalidPublicKey)?;
    verifying_key
        .verify_strict(
            &domain_preimage(domain, body),
            &Signature::from_bytes(signature),
        )
        .map_err(|_| SigningError::SignatureVerificationFailed)
}

/// The §5.1 preimage: `ASCII("fe-oplog-v1") || 0x00 || unsigned_envelope`.
pub fn signature_preimage(unsigned_bytes: &[u8]) -> Vec<u8> {
    domain_preimage(SIGNATURE_DOMAIN, unsigned_bytes)
}

/// Signs an unsigned envelope, yielding the complete eleven-key artifact.
pub fn sign_envelope(
    signing_key: &SigningKey,
    unsigned: &UnsignedEnvelope,
) -> Result<CompleteEnvelope, SigningError> {
    let unsigned_bytes = unsigned.encode_canonical()?;
    Ok(CompleteEnvelope {
        unsigned: unsigned.clone(),
        signature: sign_domain(signing_key, SIGNATURE_DOMAIN, &unsigned_bytes),
    })
}

/// Enforces §3.2: the DID decodes to `public_key` and is that key's canonical `did:key` text.
pub fn verify_author_binding(did: &str, public_key: &[u8; 32]) -> Result<(), SigningError> {
    if did_to_public_key(did)? != *public_key {
        return Err(SigningError::AuthorBindingMismatch);
    }
    if public_key_to_did(public_key) != did {
        return Err(SigningError::NonCanonicalAuthorDid);
    }
    Ok(())
}

/// Verifies the §3.2 DID-to-key binding first, then the §5.1 signature.
pub fn verify_envelope(complete: &CompleteEnvelope) -> Result<(), SigningError> {
    verify_author_binding(
        &complete.unsigned.author.did,
        &complete.unsigned.author.public_key,
    )?;
    let unsigned_bytes = complete.unsigned.encode_canonical()?;
    verify_domain(
        &complete.unsigned.author.public_key,
        SIGNATURE_DOMAIN,
        &unsigned_bytes,
        &complete.signature,
    )
}

/// Rejects bytes that are not the canonical encoding of the envelope they decoded to.
pub fn assert_canonical_bytes(
    complete: &CompleteEnvelope,
    received: &[u8],
) -> Result<(), SigningError> {
    if complete.encode_canonical()? != received {
        return Err(SigningError::NonCanonicalEncoding);
    }
    Ok(())
}

/// The mandatory ingress for bytes received from a peer; see `src/AGENTS.md` §ingress.
///
/// Decodes canonically, asserts the received bytes are their own canonical re-encoding,
/// checks the §3.2 author binding, verifies the §5.1 signature, applies the §6 structural
/// rules, refuses every non-production payload suite, and content-addresses the RECEIVED
/// bytes. [`crate::envelope::CompleteEnvelope::decode_canonical`] plus [`verify_envelope`]
/// skips the first, fourth, and fifth of those and is for locally-constructed envelopes only.
pub fn decode_and_admit(bytes: &[u8]) -> Result<(CompleteEnvelope, Hash32), SigningError> {
    let complete = CompleteEnvelope::decode_canonical(bytes)?;
    assert_canonical_bytes(&complete, bytes)?;
    verify_envelope(&complete)?;
    validate_structural_rules(&complete.unsigned)?;
    if let Some(encryption) = &complete.unsigned.payload.encryption {
        encryption.assert_production_suite()?;
    }
    Ok((complete, op_id(bytes)))
}

/// `op_id = BLAKE3(complete_envelope)` over the complete bytes, signature at key 10 included.
pub fn op_id(complete_bytes: &[u8]) -> Hash32 {
    Hash32::of(complete_bytes)
}

/// Encodes a locally-constructed complete envelope and returns its `op_id`.
///
/// Peer bytes are addressed by [`decode_and_admit`], which hashes the bytes as received.
pub fn op_id_of(complete: &CompleteEnvelope) -> Result<Hash32, SigningError> {
    Ok(op_id(&complete.encode_canonical()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::did_key::{DID_KEY_PREFIX, ED25519_MULTICODEC_PREFIX};
    use crate::envelope::{
        Author, CapabilityRef, EncryptionParams, Hlc, Identifier32, PayloadRef, Scope,
        FIXTURE_ONLY_SUITE_ID, NONCE_LENGTH, PRODUCTION_SUITE_ID, PROTOCOL_VERSION,
    };

    /// `signing_key.seed_hex` from `docs/spec/canonical-log/operation-envelope-v1.json`.
    const FIXTURE_SEED_HEX: &str =
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn fixture_signing_key() -> SigningKey {
        let seed: [u8; 32] = hex::decode(FIXTURE_SEED_HEX)
            .expect("fixture hex")
            .try_into()
            .expect("32 bytes");
        SigningKey::from_bytes(&seed)
    }

    fn sample_unsigned(signing_key: &SigningKey) -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 2,
            scope: Scope::verse_wide(Identifier32([0x11; 32])),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 0,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: Identifier32([0x55; 32]),
            parents: Vec::new(),
            hlc: Hlc::new(1, 0),
            payload: PayloadRef::empty(),
        }
    }

    #[test]
    fn signature_preimage_starts_with_the_nul_terminated_domain() {
        assert_eq!(SIGNATURE_DOMAIN, b"fe-oplog-v1\0");
        assert_eq!(hex::encode(SIGNATURE_DOMAIN), "66652d6f706c6f672d763100");
        assert_eq!(
            signature_preimage(&[0xaa, 0xbb]),
            [SIGNATURE_DOMAIN, &[0xaa, 0xbb]].concat()
        );
    }

    /// A kind-1 envelope carrying an encrypted payload under `suite_id`.
    fn encrypted_unsigned(signing_key: &SigningKey, suite_id: u16) -> UnsignedEnvelope {
        let ciphertext = [0xc0, 0xff, 0xee];
        UnsignedEnvelope {
            operation_kind: 1,
            parents: vec![Hash32([0x66; 32])],
            payload: PayloadRef {
                ciphertext_hash: Hash32::of(&ciphertext),
                ciphertext_length: ciphertext.len() as u64,
                encryption: Some(EncryptionParams {
                    suite_id,
                    key_id: Identifier32([0x77; 32]),
                    nonce: [0x88; NONCE_LENGTH],
                }),
            },
            ..sample_unsigned(signing_key)
        }
    }

    #[test]
    fn a_signed_envelope_verifies_and_addresses_its_complete_bytes() {
        let signing_key = fixture_signing_key();
        let unsigned = sample_unsigned(&signing_key);
        let complete = sign_envelope(&signing_key, &unsigned).expect("sign");
        let complete_bytes = complete.encode_canonical().expect("encode");

        assert!(verify_envelope(&complete).is_ok());
        assert_eq!(
            op_id_of(&complete).expect("op id"),
            Hash32::of(&complete_bytes),
            "op_id is BLAKE3 over the eleven-key artifact"
        );
        assert_ne!(
            op_id_of(&complete).expect("op id"),
            op_id(&unsigned.encode_canonical().expect("encode")),
            "op_id must cover the signature at key 10"
        );
    }

    #[test]
    fn admission_addresses_the_received_bytes_and_returns_the_same_envelope() {
        let signing_key = fixture_signing_key();
        let complete = sign_envelope(&signing_key, &sample_unsigned(&signing_key)).expect("sign");
        let received = complete.encode_canonical().expect("encode");

        let (admitted, admitted_op_id) = decode_and_admit(&received).expect("admit");
        assert_eq!(admitted, complete);
        assert_eq!(admitted_op_id, Hash32::of(&received));
        assert_eq!(admitted_op_id, op_id_of(&complete).expect("op id"));
    }

    #[test]
    fn admission_rejects_bytes_that_are_not_their_own_canonical_re_encoding() {
        let signing_key = fixture_signing_key();
        let complete = sign_envelope(&signing_key, &sample_unsigned(&signing_key)).expect("sign");
        let received = complete.encode_canonical().expect("encode");

        assert_eq!(assert_canonical_bytes(&complete, &received), Ok(()));

        let mut tampered = received.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        assert_eq!(
            assert_canonical_bytes(&complete, &tampered),
            Err(SigningError::NonCanonicalEncoding)
        );
        assert_eq!(
            assert_canonical_bytes(&complete, &received[..received.len() - 1]),
            Err(SigningError::NonCanonicalEncoding)
        );

        assert_eq!(
            decode_and_admit(&tampered),
            Err(SigningError::SignatureVerificationFailed),
            "a tampered signature byte still re-encodes canonically, so the signature catches it"
        );
    }

    #[test]
    fn admission_refuses_the_fixture_only_suite_that_plain_decoding_accepts() {
        let signing_key = fixture_signing_key();
        let complete = sign_envelope(
            &signing_key,
            &encrypted_unsigned(&signing_key, FIXTURE_ONLY_SUITE_ID),
        )
        .expect("sign");
        let received = complete.encode_canonical().expect("encode");

        assert_eq!(
            CompleteEnvelope::decode_canonical(&received).expect("decode"),
            complete
        );
        assert!(verify_envelope(&complete).is_ok());
        assert_eq!(
            decode_and_admit(&received),
            Err(SigningError::Envelope(EnvelopeError::FixtureOnlySuite))
        );

        let production = sign_envelope(
            &signing_key,
            &encrypted_unsigned(&signing_key, PRODUCTION_SUITE_ID),
        )
        .expect("sign");
        assert!(decode_and_admit(&production.encode_canonical().expect("encode")).is_ok());
    }

    #[test]
    fn admission_applies_the_structural_rules() {
        let signing_key = fixture_signing_key();
        let mut parentless_intent = encrypted_unsigned(&signing_key, 1);
        parentless_intent.parents = Vec::new();
        let complete = sign_envelope(&signing_key, &parentless_intent).expect("sign");

        assert!(verify_envelope(&complete).is_ok());
        assert_eq!(
            decode_and_admit(&complete.encode_canonical().expect("encode")),
            Err(SigningError::Structural(
                StructuralRuleError::NormalIntentHasNoParent
            ))
        );
    }

    #[test]
    fn verification_rejects_a_did_that_does_not_bind_to_the_public_key() {
        let signing_key = fixture_signing_key();
        let mut complete =
            sign_envelope(&signing_key, &sample_unsigned(&signing_key)).expect("sign");
        complete.unsigned.author.did = public_key_to_did(&[0x07; 32]);
        assert_eq!(
            verify_envelope(&complete),
            Err(SigningError::AuthorBindingMismatch)
        );
        assert_eq!(
            decode_and_admit(&complete.encode_canonical().expect("encode")),
            Err(SigningError::AuthorBindingMismatch)
        );
    }

    #[test]
    fn the_author_binding_admits_only_the_canonical_did_text_for_its_key() {
        let public_key = fixture_signing_key().verifying_key().to_bytes();
        let canonical = public_key_to_did(&public_key);
        assert_eq!(verify_author_binding(&canonical, &public_key), Ok(()));

        let mut multicodec = ED25519_MULTICODEC_PREFIX.to_vec();
        multicodec.extend_from_slice(&public_key);
        let alternative_spellings = [
            // The right multicodec key bytes under a different multibase.
            format!(
                "{DID_KEY_PREFIX}{}",
                multibase::encode(multibase::Base::Base64, &multicodec)
            ),
            format!(
                "{DID_KEY_PREFIX}{}",
                multibase::encode(multibase::Base::Base16Lower, &multicodec)
            ),
            // The canonical base58btc body with a base58 zero digit prepended.
            format!(
                "{DID_KEY_PREFIX}z1{}",
                &canonical[DID_KEY_PREFIX.len() + 1..]
            ),
            format!("{canonical} "),
        ];
        for spelling in alternative_spellings {
            assert_ne!(spelling, canonical);
            assert!(
                verify_author_binding(&spelling, &public_key).is_err(),
                "a non-canonical spelling of the author key must not bind: {spelling}"
            );
        }

        assert_eq!(
            verify_author_binding(&public_key_to_did(&[0x07; 32]), &public_key),
            Err(SigningError::AuthorBindingMismatch)
        );
    }

    #[test]
    fn verification_rejects_a_tampered_envelope_and_a_tampered_signature() {
        let signing_key = fixture_signing_key();
        let unsigned = sample_unsigned(&signing_key);
        let complete = sign_envelope(&signing_key, &unsigned).expect("sign");

        let mut tampered_body = complete.clone();
        tampered_body.unsigned.hlc = Hlc::new(2, 0);
        assert_eq!(
            verify_envelope(&tampered_body),
            Err(SigningError::SignatureVerificationFailed)
        );

        let mut tampered_signature = complete;
        tampered_signature.signature[0] ^= 0xff;
        assert_eq!(
            verify_envelope(&tampered_signature),
            Err(SigningError::SignatureVerificationFailed)
        );
    }

    #[test]
    fn generic_domain_helpers_separate_artifact_domains() {
        let signing_key = fixture_signing_key();
        let public_key = signing_key.verifying_key().to_bytes();
        let body = b"artifact bytes";
        let signature = sign_domain(&signing_key, b"fe-test-domain-a\0", body);

        assert!(verify_domain(&public_key, b"fe-test-domain-a\0", body, &signature).is_ok());
        assert_eq!(
            verify_domain(&public_key, b"fe-test-domain-b\0", body, &signature),
            Err(SigningError::SignatureVerificationFailed)
        );
    }
}
