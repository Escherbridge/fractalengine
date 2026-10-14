//! SPEC-10 §4.5 legacy identifiers: the ONE mapping from a hot-plane text id to a canonical
//! 32-byte identifier. Rationale and the single-implementation rule: `src/AGENTS.md`
//! §legacy-identifier.

/// The NUL-terminated domain prefixed to every legacy-identifier preimage (provisional, CDL-PROV).
pub const LEGACY_IDENTIFIER_DOMAIN: &[u8] = b"fe-legacy-identifier-v1\0";

/// The hot-plane table a legacy id belongs to; part of the hashed bytes, so tables never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LegacyTable {
    /// A `verse` row id.
    Verse,
    /// A `petal` row id.
    Petal,
    /// A `node` row id.
    Node,
}

impl LegacyTable {
    /// The exact text string SPEC-10 §4.5 rule 1 hashes for this table.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Verse => "verse",
            Self::Petal => "petal",
            Self::Node => "node",
        }
    }
}

/// Why an id has no legacy identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyIdentifierError {
    /// The id holds a non-ASCII byte. Canonical CBOR text must be Unicode NFC and this crate
    /// cannot check NFC, so it refuses every non-ASCII id rather than hash a non-canonical one.
    NonAsciiId {
        /// Byte offset of the first non-ASCII byte.
        offset: usize,
    },
}

impl std::fmt::Display for LegacyIdentifierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonAsciiId { offset } => write!(
                formatter,
                "legacy id holds a non-ASCII byte at offset {offset}; only ASCII ids map"
            ),
        }
    }
}

impl std::error::Error for LegacyIdentifierError {}

/// `BLAKE3(ASCII("fe-legacy-identifier-v1") || 00 || canonical_cbor([table, id]))` (SPEC-10 §4.5
/// rule 1): the canonical-log `Identifier32` bytes of one hot-plane row id.
///
/// Deterministic and enumerable (rule 3): the result may appear only inside encrypted payloads,
/// sealed hexons, blinded topics, or a header scope — never in clear manifest statistics.
pub fn legacy_identifier(table: LegacyTable, id: &str) -> Result<[u8; 32], LegacyIdentifierError> {
    if let Some(offset) = id.bytes().position(|byte| !byte.is_ascii()) {
        return Err(LegacyIdentifierError::NonAsciiId { offset });
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(LEGACY_IDENTIFIER_DOMAIN);
    // Canonical CBOR (RFC 8949 §4.2.1) of the two-element array `[table, id]`.
    hasher.update(&[0x82]);
    hasher.update(&cbor_text_header(table.as_str().len()));
    hasher.update(table.as_str().as_bytes());
    hasher.update(&cbor_text_header(id.len()));
    hasher.update(id.as_bytes());
    Ok(*hasher.finalize().as_bytes())
}

/// The shortest-form major-type-3 (text string) header for a string of `length` bytes.
fn cbor_text_header(length: usize) -> Vec<u8> {
    const TEXT: u8 = 0x60;
    let length = length as u64;
    if length < 24 {
        vec![TEXT | length as u8]
    } else if length <= u64::from(u8::MAX) {
        vec![TEXT | 24, length as u8]
    } else if length <= u64::from(u16::MAX) {
        let mut header = vec![TEXT | 25];
        header.extend_from_slice(&(length as u16).to_be_bytes());
        header
    } else if length <= u64::from(u32::MAX) {
        let mut header = vec![TEXT | 26];
        header.extend_from_slice(&(length as u32).to_be_bytes());
        header
    } else {
        let mut header = vec![TEXT | 27];
        header.extend_from_slice(&length.to_be_bytes());
        header
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLES: [LegacyTable; 3] = [LegacyTable::Verse, LegacyTable::Petal, LegacyTable::Node];

    #[test]
    fn legacy_identifier_is_stable_and_collision_free_across_tables() {
        let corpus = [
            "",
            "0",
            "01J9Z3K4M5N6P7Q8R9S0T1V2W3",
            "01J9Z3K4M5N6P7Q8R9S0T1V2W4",
            "verse",
            "node",
            &"x".repeat(24),
            &"x".repeat(256),
        ];
        let mut seen = std::collections::BTreeMap::new();
        for table in TABLES {
            for id in corpus {
                let first = legacy_identifier(table, id).expect("ASCII ids map");
                assert_eq!(
                    legacy_identifier(table, id),
                    Ok(first),
                    "the same (table, id) always maps to the same identifier"
                );
                if let Some(previous) = seen.insert(first, (table, id)) {
                    panic!("{previous:?} and {:?} collide", (table, id));
                }
            }
        }
        assert_eq!(seen.len(), TABLES.len() * corpus.len());
    }

    #[test]
    fn the_hashed_bytes_are_the_spec_preimage() {
        // "node" -> 64 6e 6f 64 65; "ab" -> 62 61 62; the array header is 82.
        let mut preimage = LEGACY_IDENTIFIER_DOMAIN.to_vec();
        preimage.extend_from_slice(&[0x82, 0x64, b'n', b'o', b'd', b'e', 0x62, b'a', b'b']);
        assert_eq!(
            legacy_identifier(LegacyTable::Node, "ab"),
            Ok(*blake3::hash(&preimage).as_bytes())
        );
    }

    #[test]
    fn a_non_ascii_id_is_refused_rather_than_hashed_non_canonically() {
        assert_eq!(
            legacy_identifier(LegacyTable::Petal, "abc\u{e9}"),
            Err(LegacyIdentifierError::NonAsciiId { offset: 3 })
        );
    }

    #[test]
    fn text_headers_use_the_shortest_form_at_every_length_boundary() {
        let cases: [(usize, &[u8]); 8] = [
            (0, &[0x60]),
            (23, &[0x77]),
            (24, &[0x78, 24]),
            (255, &[0x78, 0xff]),
            (256, &[0x79, 0x01, 0x00]),
            (65_535, &[0x79, 0xff, 0xff]),
            (65_536, &[0x7a, 0x00, 0x01, 0x00, 0x00]),
            (1, &[0x61]),
        ];
        for (length, header) in cases {
            assert_eq!(cbor_text_header(length), header, "length {length}");
        }
    }
}
