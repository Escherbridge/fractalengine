//! SPEC-1 §7 conformance against `docs/spec/canonical-log/operation-envelope-v1.json`.
//!
//! Every value is read from the fixture at test time; nothing here is a hardcoded copy of it.

use ed25519_dalek::SigningKey;
use fe_canonical_log::cbor::{decode_canonical, encode_canonical, CborValue};
use fe_canonical_log::did_key::public_key_to_did;
use fe_canonical_log::envelope::{
    CompleteEnvelope, EncryptionParams, EnvelopeError, Hash32, UnsignedEnvelope,
    FIXTURE_ONLY_SUITE_ID, NONCE_LENGTH,
};
use fe_canonical_log::kind::validate_structural_rules;
use fe_canonical_log::payload_aad::{payload_aad_envelope_bytes, payload_aad_preimage};
use fe_canonical_log::signing::{
    decode_and_admit, op_id, sign_envelope, signature_preimage, verify_domain, verify_envelope,
    SigningError, SIGNATURE_DOMAIN,
};
use serde_json::Value;

const FIXTURE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/spec/canonical-log/operation-envelope-v1.json"
));

fn fixture() -> Value {
    serde_json::from_str(FIXTURE_JSON).expect("golden-vector JSON parses")
}

fn text(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("field {field} is a string"))
        .to_owned()
}

fn bytes(value: &Value, field: &str) -> Vec<u8> {
    hex::decode(text(value, field)).unwrap_or_else(|_| panic!("field {field} is hex"))
}

fn optional_bytes(value: &Value, field: &str) -> Option<Vec<u8>> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(|hexadecimal| hex::decode(hexadecimal).expect("hex"))
}

fn fixture_public_key(fixture: &Value) -> [u8; 32] {
    bytes(&fixture["signing_key"], "public_key_hex")
        .try_into()
        .expect("32-byte public key")
}

fn fixture_signing_key(fixture: &Value) -> SigningKey {
    let seed: [u8; 32] = bytes(&fixture["signing_key"], "seed_hex")
        .try_into()
        .expect("32-byte seed");
    SigningKey::from_bytes(&seed)
}

#[test]
fn the_signature_domain_matches_the_fixture() {
    let fixture = fixture();
    assert_eq!(
        SIGNATURE_DOMAIN,
        bytes(&fixture["signing_key"], "signature_domain_hex").as_slice()
    );
    assert_eq!(
        public_key_to_did(&fixture_public_key(&fixture)),
        text(&fixture["signing_key"], "did")
    );
}

#[test]
fn every_vector_reproduces_all_six_conformance_points() {
    let fixture = fixture();
    let public_key = fixture_public_key(&fixture);
    let vectors = fixture["vectors"].as_array().expect("vector array");
    assert_eq!(vectors.len(), 4, "the fixture carries four golden vectors");

    for vector in vectors {
        let name = text(vector, "name");
        let unsigned_bytes = bytes(vector, "unsigned_envelope_cbor_hex");
        let complete_bytes = bytes(vector, "complete_envelope_cbor_hex");
        let signature: [u8; 64] = bytes(vector, "signature_hex")
            .try_into()
            .expect("64-byte signature");

        // (1) decode and re-encode the unsigned envelope byte-for-byte.
        let unsigned = UnsignedEnvelope::decode_canonical(&unsigned_bytes)
            .unwrap_or_else(|error| panic!("{name}: unsigned envelope decodes: {error}"));
        assert_eq!(
            unsigned.encode_canonical().expect("re-encode"),
            unsigned_bytes,
            "{name}: unsigned envelope re-encodes byte-for-byte"
        );
        assert_eq!(
            unsigned.author.public_key, public_key,
            "{name}: fixture author key"
        );
        assert!(
            validate_structural_rules(&unsigned).is_ok(),
            "{name}: structural rules hold"
        );

        // (2) build the §5.1 preimage and verify the Ed25519 signature.
        assert_eq!(
            signature_preimage(&unsigned_bytes),
            [SIGNATURE_DOMAIN, unsigned_bytes.as_slice()].concat(),
            "{name}: signature preimage shape"
        );
        assert!(
            verify_domain(&public_key, SIGNATURE_DOMAIN, &unsigned_bytes, &signature).is_ok(),
            "{name}: Ed25519 signature over the §5.1 preimage"
        );

        // (3) decode and re-encode the complete envelope byte-for-byte.
        let complete = CompleteEnvelope::decode_canonical(&complete_bytes)
            .unwrap_or_else(|error| panic!("{name}: complete envelope decodes: {error}"));
        assert_eq!(complete.signature, signature, "{name}: stored signature");
        assert_eq!(
            complete.encode_canonical().expect("re-encode"),
            complete_bytes,
            "{name}: complete envelope re-encodes byte-for-byte"
        );
        assert_eq!(
            CompleteEnvelope {
                unsigned: unsigned.clone(),
                signature,
            }
            .encode_canonical()
            .expect("reconstruct"),
            complete_bytes,
            "{name}: complete envelope reconstructs from the unsigned semantic fields"
        );
        assert!(
            verify_envelope(&complete).is_ok(),
            "{name}: DID binding then signature"
        );
        assert_eq!(
            sign_envelope(&fixture_signing_key(&fixture), &unsigned)
                .expect("sign")
                .signature,
            signature,
            "{name}: deterministic Ed25519 reproduces the fixture signature"
        );

        // (4) BLAKE3 the complete envelope and match the operation ID.
        assert_eq!(
            hex::encode(op_id(&complete_bytes).0),
            text(vector, "op_id_hex"),
            "{name}: op_id addresses the signed artifact"
        );

        // (5) BLAKE3 the fixture ciphertext and match its commitment.
        let ciphertext = bytes(vector, "ciphertext_hex");
        assert_eq!(
            hex::encode(Hash32::of(&ciphertext).0),
            text(vector, "ciphertext_hash_hex"),
            "{name}: ciphertext commitment"
        );
        assert_eq!(
            unsigned.payload.ciphertext_hash,
            Hash32::of(&ciphertext),
            "{name}: envelope commits to the fixture ciphertext"
        );
        assert_eq!(
            unsigned.payload.ciphertext_length,
            ciphertext.len() as u64,
            "{name}: envelope commits to the ciphertext length"
        );

        // (6) decode and re-encode the schema payload fixture where present.
        if let Some(plaintext) = optional_bytes(vector, "payload_plaintext_cbor_hex") {
            let payload = decode_canonical(&plaintext)
                .unwrap_or_else(|error| panic!("{name}: payload decodes: {error}"));
            assert_eq!(
                encode_canonical(&payload),
                plaintext,
                "{name}: payload re-encodes byte-for-byte"
            );
            assert_eq!(
                hex::encode(Hash32::of(&plaintext).0),
                text(vector, "payload_plaintext_hash_hex"),
                "{name}: canonical quantized payload hash"
            );
        }

        // The §5.2 payload AAD, for the one vector that commits to it.
        if let Some(stored_aad_envelope) = optional_bytes(vector, "payload_aad_envelope_cbor_hex") {
            assert_eq!(
                payload_aad_envelope_bytes(&unsigned).expect("aad envelope"),
                stored_aad_envelope,
                "{name}: payload AAD omits only ciphertext hash and length"
            );
            assert_eq!(
                hex::encode(payload_aad_preimage(&unsigned).expect("aad preimage")),
                text(vector, "payload_aad_preimage_hex"),
                "{name}: payload AAD preimage"
            );
        } else {
            assert!(
                unsigned.payload.encryption.is_none(),
                "{name}: only the no-payload vectors omit a payload AAD"
            );
            assert!(
                payload_aad_envelope_bytes(&unsigned).is_err(),
                "{name}: the no-payload form has no associated data"
            );
        }
    }
}

/// The one fixture vector that carries an encrypted payload, and so an `encryption` sub-map.
fn payload_bearing_vector(fixture: &Value) -> Value {
    fixture["vectors"]
        .as_array()
        .expect("vector array")
        .iter()
        .find(|vector| vector.get("payload_aad_envelope_cbor_hex").is_some())
        .expect("the payload-bearing vector")
        .clone()
}

/// Pulls the §3.5 `encryption` sub-map out of an unsigned envelope's decoded CBOR tree.
fn encryption_sub_map(unsigned_bytes: &[u8]) -> CborValue {
    decode_canonical(unsigned_bytes)
        .expect("unsigned envelope decodes")
        .get_uint_key(9)
        .expect("payload map")
        .get_uint_key(2)
        .expect("encryption sub-map")
        .clone()
}

#[test]
fn the_fixture_only_suite_is_rejected_while_its_encoding_still_conforms() {
    let fixture = fixture();
    let vector = payload_bearing_vector(&fixture);

    let unsigned_bytes = bytes(&vector, "unsigned_envelope_cbor_hex");
    let complete_bytes = bytes(&vector, "complete_envelope_cbor_hex");
    let unsigned = UnsignedEnvelope::decode_canonical(&unsigned_bytes).expect("decode");
    let complete = CompleteEnvelope::decode_canonical(&complete_bytes).expect("decode");
    let encryption = unsigned.payload.encryption.expect("encrypted payload");

    assert_eq!(encryption.suite_id, FIXTURE_ONLY_SUITE_ID);
    assert!(encryption.is_fixture_only_suite());
    assert!(
        encryption.assert_production_suite().is_err(),
        "suite 65535 must be rejected on production paths"
    );

    assert_eq!(
        unsigned.encode_canonical().expect("re-encode"),
        unsigned_bytes
    );
    assert_eq!(
        complete.encode_canonical().expect("re-encode"),
        complete_bytes
    );
    assert!(verify_envelope(&complete).is_ok());
    assert_eq!(
        hex::encode(op_id(&complete_bytes).0),
        text(&vector, "op_id_hex")
    );
}

#[test]
fn the_encoded_fixture_nonce_is_a_twenty_four_byte_string() {
    let fixture = fixture();
    let vector = payload_bearing_vector(&fixture);
    let encryption = encryption_sub_map(&bytes(&vector, "unsigned_envelope_cbor_hex"));

    assert_eq!(
        encryption
            .get_uint_key(2)
            .and_then(CborValue::as_bytes)
            .map(|nonce| nonce.len()),
        Some(NONCE_LENGTH),
        "the encoded nonce is unconditionally 24 bytes"
    );
}

#[test]
fn an_encryption_map_carrying_a_twelve_byte_nonce_does_not_decode() {
    let fixture = fixture();
    let vector = payload_bearing_vector(&fixture);
    let encryption = encryption_sub_map(&bytes(&vector, "unsigned_envelope_cbor_hex"));

    let mut entries = encryption.as_map().expect("encryption map").to_vec();
    entries[2] = (CborValue::Uint(2), CborValue::Bytes(vec![0x11; 12]));
    assert_eq!(
        EncryptionParams::from_cbor(&CborValue::Map(entries)),
        Err(EnvelopeError::WrongByteLength {
            context: "encryption",
            key: 2,
            expected: NONCE_LENGTH,
            actual: 12,
        })
    );
}

#[test]
fn admission_addresses_the_bytes_as_received_and_refuses_the_fixture_only_suite() {
    let fixture = fixture();
    for vector in fixture["vectors"].as_array().expect("vector array") {
        let name = text(vector, "name");
        let complete_bytes = bytes(vector, "complete_envelope_cbor_hex");
        let expected_op_id = text(vector, "op_id_hex");

        let unsigned =
            UnsignedEnvelope::decode_canonical(&bytes(vector, "unsigned_envelope_cbor_hex"))
                .expect("decode");

        if unsigned
            .payload
            .encryption
            .is_some_and(|encryption| encryption.is_fixture_only_suite())
        {
            assert!(
                CompleteEnvelope::decode_canonical(&complete_bytes).is_ok(),
                "{name}: the fixture suite still decodes"
            );
            assert_eq!(
                decode_and_admit(&complete_bytes),
                Err(SigningError::Envelope(EnvelopeError::FixtureOnlySuite)),
                "{name}: the fixture suite is never admitted"
            );
            continue;
        }

        let (admitted, admitted_op_id) = decode_and_admit(&complete_bytes)
            .unwrap_or_else(|error| panic!("{name}: admission: {error}"));
        assert_eq!(
            hex::encode(admitted_op_id.0),
            expected_op_id,
            "{name}: op_id is BLAKE3 over the bytes as received"
        );
        assert_eq!(
            admitted.encode_canonical().expect("re-encode"),
            complete_bytes,
            "{name}: the admitted envelope re-encodes to the received bytes"
        );
    }
}
