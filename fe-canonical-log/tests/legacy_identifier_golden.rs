//! Drift guard for SPEC-10 §4.5: `fe_sdk::legacy_identifier` hand-encodes `[table, id]` as CBOR,
//! and this test recomputes the same preimage with this crate's own canonical encoder, so the
//! two encoders cannot silently diverge (campaign-plan critic C10; `fe-sdk/src/AGENTS.md`
//! §legacy-identifier).

use fe_canonical_log::cbor::{encode_canonical, CborValue};
use fe_canonical_log::envelope::Hash32;
use fe_sdk::legacy_identifier::{legacy_identifier, LegacyTable, LEGACY_IDENTIFIER_DOMAIN};

/// `BLAKE3(domain || canonical_cbor([table, id]))` built from this crate's encoder.
fn reference_identifier(table: &str, id: &str) -> [u8; 32] {
    let value = CborValue::Array(vec![CborValue::text(table), CborValue::text(id)]);
    let mut preimage = LEGACY_IDENTIFIER_DOMAIN.to_vec();
    preimage.extend_from_slice(&encode_canonical(&value));
    Hash32::of(&preimage).0
}

#[test]
fn legacy_identifier_matches_the_canonical_log_cbor_encoder() {
    // Each length sits on an RFC 8949 §4.2.1 text-header boundary: inline (< 24), one-byte
    // (< 256), two-byte (< 65 536) and four-byte lengths.
    for length in [0usize, 1, 23, 24, 255, 256, 65_535, 65_536] {
        let id: String = (0..length)
            .map(|index| char::from(b'0' + (index % 10) as u8))
            .collect();
        for table in [LegacyTable::Verse, LegacyTable::Petal, LegacyTable::Node] {
            assert_eq!(
                legacy_identifier(table, &id).expect("an ASCII id maps"),
                reference_identifier(table.as_str(), &id),
                "{table:?} id of length {length}"
            );
        }
    }
}
