//! Payload AEAD associated-data construction (SPEC-1 §5.2); see `src/AGENTS.md`.

use thiserror::Error;

use crate::cbor::{encode_canonical_checked as encode_cbor_checked, CborValue};
use crate::envelope::{EnvelopeError, UnsignedEnvelope};
use crate::signing::domain_preimage;

/// NUL-terminated ASCII domain separator for payload AEAD associated data (§5.2).
pub const PAYLOAD_AAD_DOMAIN: &[u8] = b"fe-oplog-payload-aad-v1\0";

/// Every reason payload associated data cannot be constructed.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PayloadAadError {
    /// The envelope could not be encoded canonically.
    #[error("envelope: {0}")]
    Envelope(#[from] EnvelopeError),
    /// The operation has the no-payload form, so it has no associated data.
    #[error("payload AAD requires encryption metadata; this operation has the no-payload form")]
    MissingEncryption,
}

/// Keys 0 through 8 verbatim plus a key-9 map holding only the `encryption` sub-map.
///
/// Ciphertext hash and length are deliberately omitted to avoid a circular dependency; the
/// envelope signature is what binds those two values.
pub fn payload_aad_envelope_cbor(
    unsigned: &UnsignedEnvelope,
) -> Result<CborValue, PayloadAadError> {
    let encryption = unsigned
        .payload
        .encryption
        .as_ref()
        .ok_or(PayloadAadError::MissingEncryption)?;

    let envelope = unsigned.to_cbor()?;
    let mut entries: Vec<(CborValue, CborValue)> = envelope
        .as_map()
        .expect("UnsignedEnvelope::to_cbor always yields a map")
        .iter()
        .filter(|(key, _)| key != &CborValue::Uint(9))
        .cloned()
        .collect();
    entries.push((
        CborValue::Uint(9),
        CborValue::Map(vec![(CborValue::Uint(2), encryption.to_cbor())]),
    ));
    Ok(CborValue::Map(entries))
}

/// Canonical bytes of [`payload_aad_envelope_cbor`].
pub fn payload_aad_envelope_bytes(unsigned: &UnsignedEnvelope) -> Result<Vec<u8>, PayloadAadError> {
    encode_cbor_checked(&payload_aad_envelope_cbor(unsigned)?)
        .map_err(|error| PayloadAadError::Envelope(EnvelopeError::Cbor(error)))
}

/// The §5.2 preimage: `ASCII("fe-oplog-payload-aad-v1") || 0x00 || payload_aad_envelope`.
pub fn payload_aad_preimage(unsigned: &UnsignedEnvelope) -> Result<Vec<u8>, PayloadAadError> {
    Ok(domain_preimage(
        PAYLOAD_AAD_DOMAIN,
        &payload_aad_envelope_bytes(unsigned)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{
        Author, CapabilityRef, EncryptionParams, Hash32, Hlc, Identifier32, PayloadRef, Scope,
        NONCE_LENGTH, PROTOCOL_VERSION,
    };

    fn encrypted_unsigned() -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 1,
            scope: Scope::verse_wide(Identifier32([0x11; 32])),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 1,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: Identifier32([0x55; 32]),
            parents: vec![Hash32([0x66; 32])],
            hlc: Hlc::new(9, 1),
            payload: PayloadRef {
                ciphertext_hash: Hash32::of(&[0xc0, 0xff, 0xee]),
                ciphertext_length: 3,
                encryption: Some(EncryptionParams {
                    suite_id: 1,
                    key_id: Identifier32([0x77; 32]),
                    nonce: [0x88; NONCE_LENGTH],
                }),
            },
        }
    }

    #[test]
    fn aad_envelope_keeps_keys_zero_through_eight_and_trims_key_nine() {
        let unsigned = encrypted_unsigned();
        let aad = payload_aad_envelope_cbor(&unsigned).expect("aad envelope");
        let full = unsigned.to_cbor().expect("envelope");

        assert_eq!(aad.as_map().map(|entries| entries.len()), Some(10));
        for key in 0..=8u64 {
            assert_eq!(aad.get_uint_key(key), full.get_uint_key(key), "key {key}");
        }

        let trimmed = aad.get_uint_key(9).expect("trimmed payload map");
        assert_eq!(trimmed.as_map().map(|entries| entries.len()), Some(1));
        assert_eq!(
            trimmed.get_uint_key(2),
            full.get_uint_key(9)
                .and_then(|payload| payload.get_uint_key(2))
        );
        assert_eq!(trimmed.get_uint_key(0), None);
        assert_eq!(trimmed.get_uint_key(1), None);
    }

    #[test]
    fn aad_preimage_prefixes_the_nul_terminated_domain() {
        let unsigned = encrypted_unsigned();
        assert_eq!(
            hex::encode(PAYLOAD_AAD_DOMAIN),
            "66652d6f706c6f672d7061796c6f61642d6161642d763100"
        );

        let preimage = payload_aad_preimage(&unsigned).expect("preimage");
        let (domain, body) = preimage.split_at(PAYLOAD_AAD_DOMAIN.len());
        assert_eq!(domain, PAYLOAD_AAD_DOMAIN);
        assert_eq!(
            crate::cbor::decode_canonical(body).expect("the AAD body is canonical CBOR"),
            payload_aad_envelope_cbor(&unsigned).expect("aad envelope")
        );
    }

    #[test]
    fn the_no_payload_form_has_no_associated_data() {
        let mut unsigned = encrypted_unsigned();
        unsigned.payload = PayloadRef::empty();
        assert_eq!(
            payload_aad_envelope_cbor(&unsigned),
            Err(PayloadAadError::MissingEncryption)
        );
        assert_eq!(
            payload_aad_envelope_bytes(&unsigned),
            Err(PayloadAadError::MissingEncryption)
        );
        assert_eq!(
            payload_aad_preimage(&unsigned),
            Err(PayloadAadError::MissingEncryption)
        );
    }
}
