//! Canonical CBOR codec for the RFC 8949 §4.2.1 profile; see `src/AGENTS.md` §cbor.

use thiserror::Error;
use unicode_normalization::{is_nfc, UnicodeNormalization};

/// Maximum nesting depth accepted by [`decode_canonical`].
pub const MAXIMUM_NESTING_DEPTH: usize = 128;

/// Closed canonical CBOR value model: floats, tags, undefined, and other simple values cannot exist.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CborValue {
    /// Major type 0 unsigned integer.
    Uint(u64),
    /// Major type 1 negative integer, held as its own negative value.
    NegInt(i64),
    /// Major type 2 definite-length byte string.
    Bytes(Vec<u8>),
    /// Major type 3 definite-length UTF-8 text string in Unicode NFC.
    Text(String),
    /// Major type 4 definite-length array.
    Array(Vec<CborValue>),
    /// Major type 5 definite-length map, canonically ordered on encode.
    Map(Vec<(CborValue, CborValue)>),
    /// Simple value 22 (`0xf6`), the only simple value in the profile.
    Null,
}

/// Every reason canonical CBOR decoding rejects an input.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CborError {
    /// The input ended in the middle of an item.
    #[error("truncated CBOR: {needed} more byte(s) required at offset {offset}")]
    Truncated {
        /// Bytes the item still required.
        needed: usize,
        /// Offset at which the read failed.
        offset: usize,
    },
    /// Additional information 31 requested an indefinite length.
    #[error("indefinite length (additional information 31) at offset {offset}")]
    IndefiniteLength {
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// Additional information 28, 29, or 30 is reserved.
    #[error("reserved additional information {additional} at offset {offset}")]
    ReservedAdditionalInformation {
        /// The reserved additional-information value.
        additional: u8,
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// An integer or length argument used more bytes than the shortest permitted form.
    #[error("non-minimal CBOR argument {value} in {width} argument byte(s) at offset {offset}")]
    NonMinimalArgument {
        /// The decoded argument value.
        value: u64,
        /// Argument byte width actually used.
        width: u8,
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// Major type 7 carried a float or a simple value other than `null`.
    #[error("float or unsupported simple value {additional} at offset {offset}")]
    UnsupportedSimpleValue {
        /// The additional-information value seen under major type 7.
        additional: u8,
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// Major type 6 tags are forbidden in signed bytes.
    #[error("tag (major type 6) at offset {offset}")]
    TagNotAllowed {
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// A major type 1 argument encoded a value below `i64::MIN`.
    #[error("negative integer at offset {offset} is below i64::MIN")]
    NegativeIntegerOutOfRange {
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// A declared length exceeds the bytes actually remaining.
    #[error("declared length {length} at offset {offset} exceeds the remaining input")]
    LengthOutOfRange {
        /// The declared length.
        length: u64,
        /// Offset of the offending head byte.
        offset: usize,
    },
    /// A text string was not valid UTF-8.
    #[error("text string at offset {offset} is not valid UTF-8")]
    InvalidUtf8 {
        /// Offset of the offending text head byte.
        offset: usize,
    },
    /// A text string differed from its own NFC normalization.
    #[error("text string at offset {offset} is not Unicode NFC")]
    NotNormalizedNfc {
        /// Offset of the offending text head byte.
        offset: usize,
    },
    /// Two map keys had identical canonical encodings.
    #[error("duplicate map key at offset {offset}")]
    DuplicateMapKey {
        /// Offset of the repeated key.
        offset: usize,
    },
    /// Map keys were not in strictly ascending canonical key-byte order.
    #[error("map key at offset {offset} breaks ascending canonical key-byte order")]
    UnsortedMapKeys {
        /// Offset of the out-of-order key.
        offset: usize,
    },
    /// Bytes remained after the top-level value.
    #[error("{count} trailing byte(s) after the top-level CBOR value")]
    TrailingBytes {
        /// Number of unconsumed bytes.
        count: usize,
    },
    /// Nesting exceeded [`MAXIMUM_NESTING_DEPTH`].
    #[error("CBOR nesting at offset {offset} exceeds the depth limit of {limit}")]
    DepthLimitExceeded {
        /// The enforced limit.
        limit: usize,
        /// Offset at which the limit was crossed.
        offset: usize,
    },
    /// A value handed to [`encode_canonical_checked`] held a text string that was not NFC.
    #[error("cannot encode a text string that is not Unicode NFC")]
    UnnormalizedTextInValue,
    /// A value handed to [`encode_canonical_checked`] held a map with two identical keys.
    #[error("cannot encode a map holding two keys with identical canonical encodings")]
    DuplicateKeyInValue,
}

impl CborValue {
    /// Builds a text value, normalizing to Unicode NFC first.
    pub fn text(value: impl AsRef<str>) -> Self {
        let value = value.as_ref();
        if is_nfc(value) {
            CborValue::Text(value.to_owned())
        } else {
            CborValue::Text(value.nfc().collect())
        }
    }

    /// Builds the correct integer variant for a signed value.
    pub fn integer(value: i64) -> Self {
        if value < 0 {
            CborValue::NegInt(value)
        } else {
            CborValue::Uint(value as u64)
        }
    }

    /// Returns the unsigned integer value.
    pub fn as_uint(&self) -> Option<u64> {
        match self {
            CborValue::Uint(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns either integer variant as `i64` when it fits.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            CborValue::Uint(value) => i64::try_from(*value).ok(),
            CborValue::NegInt(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the byte string contents.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            CborValue::Bytes(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the text string contents.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            CborValue::Text(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the array elements.
    pub fn as_array(&self) -> Option<&[CborValue]> {
        match self {
            CborValue::Array(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the map entries in their current order.
    pub fn as_map(&self) -> Option<&[(CborValue, CborValue)]> {
        match self {
            CborValue::Map(value) => Some(value),
            _ => None,
        }
    }

    /// Reports whether this is the `null` simple value.
    pub fn is_null(&self) -> bool {
        matches!(self, CborValue::Null)
    }

    /// Looks up a map entry by exact key value.
    pub fn get(&self, key: &CborValue) -> Option<&CborValue> {
        self.as_map()?
            .iter()
            .find(|(entry_key, _)| entry_key == key)
            .map(|(_, entry_value)| entry_value)
    }

    /// Looks up a map entry under an unsigned integer key.
    pub fn get_uint_key(&self, key: u64) -> Option<&CborValue> {
        self.get(&CborValue::Uint(key))
    }

    /// Looks up a map entry under a text key, normalizing the probe to NFC.
    pub fn get_text_key(&self, key: &str) -> Option<&CborValue> {
        self.get(&CborValue::text(key))
    }
}

/// Encodes a value into the RFC 8949 §4.2.1 canonical form; for internal round-trip use only.
///
/// Signing and hashing paths use [`encode_canonical_checked`]; see `src/AGENTS.md` §cbor.
pub fn encode_canonical(value: &CborValue) -> Vec<u8> {
    debug_assert!(
        validate_encodable(value).is_ok(),
        "encode_canonical was handed a value whose bytes its own decoder would reject"
    );
    encode_item(value)
}

/// Encodes a value, first rejecting the two shapes that encode to bytes the decoder rejects.
pub fn encode_canonical_checked(value: &CborValue) -> Result<Vec<u8>, CborError> {
    validate_encodable(value)?;
    Ok(encode_item(value))
}

/// Rejects non-NFC text and duplicate map keys anywhere in the tree.
fn validate_encodable(value: &CborValue) -> Result<(), CborError> {
    match value {
        CborValue::Text(text) => {
            if !is_nfc(text) {
                return Err(CborError::UnnormalizedTextInValue);
            }
        }
        CborValue::Array(items) => {
            for item in items {
                validate_encodable(item)?;
            }
        }
        CborValue::Map(entries) => {
            let mut encoded_keys: Vec<Vec<u8>> = Vec::with_capacity(entries.len());
            for (key, entry_value) in entries {
                validate_encodable(key)?;
                validate_encodable(entry_value)?;
                encoded_keys.push(encode_item(key));
            }
            encoded_keys.sort();
            if encoded_keys.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(CborError::DuplicateKeyInValue);
            }
        }
        _ => {}
    }
    Ok(())
}

fn encode_item(value: &CborValue) -> Vec<u8> {
    let mut output = Vec::new();
    encode_into(value, &mut output);
    output
}

fn encode_into(value: &CborValue, output: &mut Vec<u8>) {
    match value {
        CborValue::Uint(unsigned) => encode_argument(0, *unsigned, output),
        CborValue::NegInt(signed) => {
            if *signed < 0 {
                encode_argument(1, (-1i128 - i128::from(*signed)) as u64, output);
            } else {
                encode_argument(0, *signed as u64, output);
            }
        }
        CborValue::Bytes(bytes) => {
            encode_argument(2, bytes.len() as u64, output);
            output.extend_from_slice(bytes);
        }
        CborValue::Text(text) => {
            encode_argument(3, text.len() as u64, output);
            output.extend_from_slice(text.as_bytes());
        }
        CborValue::Array(items) => {
            encode_argument(4, items.len() as u64, output);
            for item in items {
                encode_into(item, output);
            }
        }
        CborValue::Map(entries) => {
            let mut encoded: Vec<(Vec<u8>, Vec<u8>)> = entries
                .iter()
                .map(|(key, entry_value)| (encode_item(key), encode_item(entry_value)))
                .collect();
            encoded.sort_by(|left, right| left.0.cmp(&right.0));
            encode_argument(5, encoded.len() as u64, output);
            for (key, entry_value) in encoded {
                output.extend_from_slice(&key);
                output.extend_from_slice(&entry_value);
            }
        }
        CborValue::Null => output.push(0xf6),
    }
}

fn encode_argument(major_type: u8, argument: u64, output: &mut Vec<u8>) {
    let head = major_type << 5;
    if argument < 24 {
        output.push(head | argument as u8);
    } else if argument <= u64::from(u8::MAX) {
        output.push(head | 24);
        output.push(argument as u8);
    } else if argument <= u64::from(u16::MAX) {
        output.push(head | 25);
        output.extend_from_slice(&(argument as u16).to_be_bytes());
    } else if argument <= u64::from(u32::MAX) {
        output.push(head | 26);
        output.extend_from_slice(&(argument as u32).to_be_bytes());
    } else {
        output.push(head | 27);
        output.extend_from_slice(&argument.to_be_bytes());
    }
}

/// Decodes canonical CBOR, rejecting every non-canonical form in the profile.
pub fn decode_canonical(bytes: &[u8]) -> Result<CborValue, CborError> {
    let mut decoder = Decoder { bytes, offset: 0 };
    let value = decoder.decode_item(0)?;
    if decoder.offset != bytes.len() {
        return Err(CborError::TrailingBytes {
            count: bytes.len() - decoder.offset,
        });
    }
    Ok(value)
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn read_fixed(&mut self, count: usize) -> Result<&'a [u8], CborError> {
        let source: &'a [u8] = self.bytes;
        let remaining = source.len() - self.offset;
        if remaining < count {
            return Err(CborError::Truncated {
                needed: count - remaining,
                offset: self.offset,
            });
        }
        let slice = &source[self.offset..self.offset + count];
        self.offset += count;
        Ok(slice)
    }

    fn take_declared(&mut self, length: u64, head_offset: usize) -> Result<&'a [u8], CborError> {
        let remaining = (self.bytes.len() - self.offset) as u64;
        if length > remaining {
            return Err(CborError::LengthOutOfRange {
                length,
                offset: head_offset,
            });
        }
        self.read_fixed(length as usize)
    }

    fn read_argument(&mut self, additional: u8, head_offset: usize) -> Result<u64, CborError> {
        match additional {
            0..=23 => Ok(u64::from(additional)),
            24 => {
                let value = u64::from(self.read_fixed(1)?[0]);
                if value < 24 {
                    Err(CborError::NonMinimalArgument {
                        value,
                        width: 1,
                        offset: head_offset,
                    })
                } else {
                    Ok(value)
                }
            }
            25 => {
                let mut raw = [0u8; 2];
                raw.copy_from_slice(self.read_fixed(2)?);
                let value = u64::from(u16::from_be_bytes(raw));
                if value <= u64::from(u8::MAX) {
                    Err(CborError::NonMinimalArgument {
                        value,
                        width: 2,
                        offset: head_offset,
                    })
                } else {
                    Ok(value)
                }
            }
            26 => {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(self.read_fixed(4)?);
                let value = u64::from(u32::from_be_bytes(raw));
                if value <= u64::from(u16::MAX) {
                    Err(CborError::NonMinimalArgument {
                        value,
                        width: 4,
                        offset: head_offset,
                    })
                } else {
                    Ok(value)
                }
            }
            27 => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(self.read_fixed(8)?);
                let value = u64::from_be_bytes(raw);
                if value <= u64::from(u32::MAX) {
                    Err(CborError::NonMinimalArgument {
                        value,
                        width: 8,
                        offset: head_offset,
                    })
                } else {
                    Ok(value)
                }
            }
            31 => Err(CborError::IndefiniteLength {
                offset: head_offset,
            }),
            _ => Err(CborError::ReservedAdditionalInformation {
                additional,
                offset: head_offset,
            }),
        }
    }

    fn decode_item(&mut self, depth: usize) -> Result<CborValue, CborError> {
        let head_offset = self.offset;
        if depth > MAXIMUM_NESTING_DEPTH {
            return Err(CborError::DepthLimitExceeded {
                limit: MAXIMUM_NESTING_DEPTH,
                offset: head_offset,
            });
        }
        let head = self.read_fixed(1)?[0];
        let major_type = head >> 5;
        let additional = head & 0x1f;

        match major_type {
            0 => Ok(CborValue::Uint(
                self.read_argument(additional, head_offset)?,
            )),
            1 => {
                let argument = self.read_argument(additional, head_offset)?;
                if argument > i64::MAX as u64 {
                    return Err(CborError::NegativeIntegerOutOfRange {
                        offset: head_offset,
                    });
                }
                Ok(CborValue::NegInt((-1i128 - argument as i128) as i64))
            }
            2 => {
                let length = self.read_argument(additional, head_offset)?;
                Ok(CborValue::Bytes(
                    self.take_declared(length, head_offset)?.to_vec(),
                ))
            }
            3 => {
                let length = self.read_argument(additional, head_offset)?;
                let raw = self.take_declared(length, head_offset)?;
                let text = std::str::from_utf8(raw).map_err(|_| CborError::InvalidUtf8 {
                    offset: head_offset,
                })?;
                if !is_nfc(text) {
                    return Err(CborError::NotNormalizedNfc {
                        offset: head_offset,
                    });
                }
                Ok(CborValue::Text(text.to_owned()))
            }
            4 => {
                let length = self.read_argument(additional, head_offset)?;
                self.reject_impossible_container(length, head_offset, 1)?;
                let mut items = Vec::with_capacity(length as usize);
                for _ in 0..length {
                    items.push(self.decode_item(depth + 1)?);
                }
                Ok(CborValue::Array(items))
            }
            5 => {
                let length = self.read_argument(additional, head_offset)?;
                self.reject_impossible_container(length, head_offset, 2)?;
                let source: &'a [u8] = self.bytes;
                let mut entries = Vec::with_capacity(length as usize);
                let mut previous_key: Option<&'a [u8]> = None;
                for _ in 0..length {
                    let key_start = self.offset;
                    let key = self.decode_item(depth + 1)?;
                    let key_bytes = &source[key_start..self.offset];
                    if let Some(previous) = previous_key {
                        match key_bytes.cmp(previous) {
                            std::cmp::Ordering::Greater => {}
                            std::cmp::Ordering::Equal => {
                                return Err(CborError::DuplicateMapKey { offset: key_start })
                            }
                            std::cmp::Ordering::Less => {
                                return Err(CborError::UnsortedMapKeys { offset: key_start })
                            }
                        }
                    }
                    previous_key = Some(key_bytes);
                    let value = self.decode_item(depth + 1)?;
                    entries.push((key, value));
                }
                Ok(CborValue::Map(entries))
            }
            6 => Err(CborError::TagNotAllowed {
                offset: head_offset,
            }),
            _ if additional == 22 => Ok(CborValue::Null),
            _ => Err(CborError::UnsupportedSimpleValue {
                additional,
                offset: head_offset,
            }),
        }
    }

    /// Rejects a container whose declared item count cannot fit in the remaining bytes.
    fn reject_impossible_container(
        &self,
        length: u64,
        head_offset: usize,
        minimum_bytes_per_item: u64,
    ) -> Result<(), CborError> {
        let remaining = (self.bytes.len() - self.offset) as u64;
        if length.saturating_mul(minimum_bytes_per_item) > remaining {
            return Err(CborError::LengthOutOfRange {
                length,
                offset: head_offset,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden vector `merge-empty-payload-multi-parent-hlc-edges`, unsigned envelope.
    const MERGE_UNSIGNED_ENVELOPE_HEX: &str = concat!(
        "aa0001010302a3005820000102030405060708090a0b0c0d0e0f1011121314151617",
        "18191a1b1c1d1e1f015820202122232425262728292a2b2c2d2e2f30313233343536",
        "3738393a3b3c3d3e3f02f603a20078386469643a6b65793a7a364d6b656852676637",
        "794a62676147665973646f41734b6442504533646a324359686f775164636a71534a",
        "6776566401582003a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc86",
        "64125531b804a2005820404142434445464748494a4b4c4d4e4f5051525354555657",
        "58595a5b5c5d5e5f0100055820606162636465666768696a6b6c6d6e6f7071727374",
        "75767778797a7b7c7d7e7f065820808182838485868788898a8b8c8d8e8f90919293",
        "9495969798999a9b9c9d9e9f07825820a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1",
        "b2b3b4b5b6b7b8b9babbbcbdbebf5820c0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1",
        "d2d3d4d5d6d7d8d9dadbdcdddedf08a2001bffffffffffffffff011affffffff09a3",
        "005820af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f32",
        "62010002f6",
    );

    /// Golden vector `intent-test-profile-unicode-and-nano-quantization`, payload plaintext.
    const PAYLOAD_PLAINTEXT_HEX: &str = concat!(
        "a4000101697472616e73666f726d0258204142434445464748494a4b4c4d4e4f5051",
        "52535455565758595a5b5c5d5e5f6003a4008320001b7fffffffffffffff01833b7f",
        "ffffffffffffff011affffffff028301010103a164ceba657920",
    );

    /// Golden vector `intent-test-profile-unicode-and-nano-quantization`, `encryption` sub-map.
    const ENCRYPTION_MAP_HEX: &str = concat!(
        "a30019ffff015820e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9fa",
        "fbfcfdfeff000258181112131415161718191a1b1c1d1e1f202122232425262728",
    );

    /// Golden vector fixture DID, asserted to survive a text round-trip inside the envelope.
    const FIXTURE_DID: &str = "did:key:z6MkehRgf7yJbgaGfYsdoAsKdBPE3dj2CYhowQdcjqSJgvVd";

    fn bytes_of(hexadecimal: &str) -> Vec<u8> {
        hex::decode(hexadecimal).expect("fixture hex")
    }

    fn assert_round_trips(hexadecimal: &str) {
        let original = bytes_of(hexadecimal);
        let decoded = decode_canonical(&original).expect("golden bytes must decode");
        assert_eq!(
            encode_canonical(&decoded),
            original,
            "re-encoding must be byte-exact"
        );
    }

    /// Encodes a text string without normalizing it, so non-NFC inputs can be tested.
    fn raw_text_item(text: &str) -> Vec<u8> {
        let mut item = Vec::new();
        encode_argument(3, text.len() as u64, &mut item);
        item.extend_from_slice(text.as_bytes());
        item
    }

    #[test]
    fn golden_merge_unsigned_envelope_round_trips_byte_exactly() {
        assert_round_trips(MERGE_UNSIGNED_ENVELOPE_HEX);
    }

    #[test]
    fn golden_payload_plaintext_round_trips_byte_exactly() {
        assert_round_trips(PAYLOAD_PLAINTEXT_HEX);
    }

    #[test]
    fn golden_encryption_map_carries_a_twenty_four_byte_nonce() {
        assert_round_trips(ENCRYPTION_MAP_HEX);
        let decoded = decode_canonical(&bytes_of(ENCRYPTION_MAP_HEX)).expect("decode");
        assert_eq!(
            decoded.get_uint_key(0).and_then(CborValue::as_uint),
            Some(65535)
        );
        assert_eq!(
            decoded
                .get_uint_key(1)
                .and_then(CborValue::as_bytes)
                .map(|key_id| key_id.len()),
            Some(32)
        );
        assert_eq!(
            decoded
                .get_uint_key(2)
                .and_then(CborValue::as_bytes)
                .map(|nonce| nonce.len()),
            Some(24)
        );
    }

    #[test]
    fn golden_envelope_exposes_its_semantic_fields_through_accessors() {
        let decoded = decode_canonical(&bytes_of(MERGE_UNSIGNED_ENVELOPE_HEX)).expect("decode");
        assert_eq!(decoded.as_map().map(|entries| entries.len()), Some(10));
        assert_eq!(
            decoded.get_uint_key(0).and_then(CborValue::as_uint),
            Some(1)
        );
        assert_eq!(
            decoded.get_uint_key(1).and_then(CborValue::as_uint),
            Some(3)
        );

        let author = decoded.get_uint_key(3).expect("author map");
        assert_eq!(
            author.get_uint_key(0).and_then(CborValue::as_text),
            Some(FIXTURE_DID)
        );
        assert_eq!(
            author
                .get_uint_key(1)
                .and_then(CborValue::as_bytes)
                .map(|key| key.len()),
            Some(32)
        );

        let hlc = decoded.get_uint_key(8).expect("hlc map");
        assert_eq!(
            hlc.get_uint_key(0).and_then(CborValue::as_uint),
            Some(u64::MAX)
        );
        assert_eq!(
            hlc.get_uint_key(1).and_then(CborValue::as_uint),
            Some(u64::from(u32::MAX))
        );

        let parents = decoded
            .get_uint_key(7)
            .and_then(CborValue::as_array)
            .expect("parents array");
        assert_eq!(parents.len(), 2);
        assert!(parents[0].as_bytes().unwrap() < parents[1].as_bytes().unwrap());

        let payload = decoded.get_uint_key(9).expect("payload map");
        assert!(payload.get_uint_key(2).expect("encryption slot").is_null());
        assert_eq!(
            payload.get_uint_key(1).and_then(CborValue::as_uint),
            Some(0)
        );

        let scope = decoded.get_uint_key(2).expect("scope map");
        assert_eq!(
            scope
                .get_uint_key(1)
                .and_then(CborValue::as_bytes)
                .map(|petal| petal.len()),
            Some(32)
        );
        assert!(scope.get_uint_key(2).expect("resource slot").is_null());
    }

    #[test]
    fn golden_payload_exposes_its_unicode_text_key_and_nano_edges() {
        let decoded = decode_canonical(&bytes_of(PAYLOAD_PLAINTEXT_HEX)).expect("decode");
        let transform = decoded.get_uint_key(3).expect("transform map");

        assert_eq!(
            transform
                .get_uint_key(3)
                .and_then(|nested| nested.get_text_key("\u{03ba}ey")),
            Some(&CborValue::NegInt(-1))
        );

        let position = transform
            .get_uint_key(0)
            .and_then(CborValue::as_array)
            .expect("position vector");
        assert_eq!(position[0].as_int(), Some(-1));
        assert_eq!(position[1].as_int(), Some(0));
        assert_eq!(position[2].as_int(), Some(i64::MAX));

        let rotation = transform
            .get_uint_key(1)
            .and_then(CborValue::as_array)
            .expect("rotation vector");
        assert_eq!(rotation[0].as_int(), Some(i64::MIN));
        assert_eq!(rotation[1].as_int(), Some(1));
        assert_eq!(rotation[2].as_int(), Some(i64::from(u32::MAX)));

        assert_eq!(
            decoded.get_uint_key(1).and_then(CborValue::as_text),
            Some("transform")
        );
    }

    #[test]
    fn integer_edges_encode_to_the_golden_forms() {
        let cases: &[(CborValue, &str)] = &[
            (CborValue::Uint(0), "00"),
            (CborValue::Uint(1), "01"),
            (CborValue::Uint(23), "17"),
            (CborValue::Uint(24), "1818"),
            (CborValue::Uint(255), "18ff"),
            (CborValue::Uint(256), "190100"),
            (CborValue::Uint(65535), "19ffff"),
            (CborValue::Uint(65536), "1a00010000"),
            (CborValue::Uint(u64::from(u32::MAX)), "1affffffff"),
            (
                CborValue::Uint(u64::from(u32::MAX) + 1),
                "1b0000000100000000",
            ),
            (CborValue::Uint(i64::MAX as u64), "1b7fffffffffffffff"),
            (CborValue::Uint(u64::MAX), "1bffffffffffffffff"),
            (CborValue::NegInt(-1), "20"),
            (CborValue::NegInt(-24), "37"),
            (CborValue::NegInt(-25), "3818"),
            (CborValue::NegInt(-256), "38ff"),
            (CborValue::NegInt(-257), "390100"),
            (CborValue::NegInt(i64::MIN), "3b7fffffffffffffff"),
            (CborValue::Null, "f6"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                hex::encode(encode_canonical(value)),
                *expected,
                "encoding {value:?}"
            );
            assert_eq!(
                decode_canonical(&bytes_of(expected)).expect("decode"),
                *value,
                "decoding {expected}"
            );
        }
    }

    #[test]
    fn empty_containers_and_strings_round_trip() {
        let cases: &[(CborValue, &str)] = &[
            (CborValue::Bytes(Vec::new()), "40"),
            (CborValue::Text(String::new()), "60"),
            (CborValue::Array(Vec::new()), "80"),
            (CborValue::Map(Vec::new()), "a0"),
        ];
        for (value, expected) in cases {
            assert_eq!(hex::encode(encode_canonical(value)), *expected);
            assert_eq!(
                decode_canonical(&bytes_of(expected)).expect("decode"),
                *value
            );
        }
    }

    #[test]
    fn nested_maps_round_trip_byte_exactly() {
        let value = CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Map(vec![(
                    CborValue::Uint(0),
                    CborValue::Map(vec![(
                        CborValue::Uint(7),
                        CborValue::Bytes(vec![0xab, 0xcd]),
                    )]),
                )]),
            ),
            (
                CborValue::Uint(1),
                CborValue::Array(vec![
                    CborValue::text("alpha"),
                    CborValue::NegInt(-1000),
                    CborValue::Null,
                    CborValue::Map(Vec::new()),
                ]),
            ),
        ]);
        let encoded = encode_canonical(&value);
        let decoded = decode_canonical(&encoded).expect("decode");
        assert_eq!(decoded, value);
        assert_eq!(encode_canonical(&decoded), encoded);
    }

    #[test]
    fn encoding_sorts_map_keys_by_complete_key_bytes() {
        let value = CborValue::Map(vec![
            (CborValue::text("zeta"), CborValue::Uint(3)),
            (CborValue::Uint(1000), CborValue::Uint(1)),
            (CborValue::Uint(0), CborValue::Uint(0)),
        ]);
        assert_eq!(
            hex::encode(encode_canonical(&value)),
            "a300001903e801647a65746103"
        );
        assert!(decode_canonical(&encode_canonical(&value)).is_ok());
    }

    #[test]
    fn map_key_order_is_plain_bytewise_not_length_first() {
        // Uint(1000) is `19 03 e8`; Text("") is `60`. Bytewise ascending puts the
        // three-byte key first, which a length-first comparison would reject.
        let canonical = bytes_of("a21903e8006000");
        let decoded = decode_canonical(&canonical).expect("bytewise-ascending keys are canonical");
        assert_eq!(encode_canonical(&decoded), canonical);
        assert_eq!(
            decode_canonical(&bytes_of("a260001903e800")),
            Err(CborError::UnsortedMapKeys { offset: 3 })
        );
    }

    #[test]
    fn rejects_indefinite_lengths() {
        let cases: &[&str] = &[
            "5f4161ff", // indefinite byte string
            "7f6161ff", // indefinite text string
            "9fff",     // indefinite array
            "bf0000ff", // indefinite map
        ];
        for hexadecimal in cases {
            assert_eq!(
                decode_canonical(&bytes_of(hexadecimal)),
                Err(CborError::IndefiniteLength { offset: 0 }),
                "{hexadecimal}"
            );
        }
    }

    #[test]
    fn rejects_reserved_additional_information() {
        for hexadecimal in ["1c", "1d", "1e", "5c", "9d", "be"] {
            let error = decode_canonical(&bytes_of(hexadecimal)).expect_err("reserved");
            assert!(
                matches!(error, CborError::ReservedAdditionalInformation { .. }),
                "{hexadecimal} gave {error:?}"
            );
        }
    }

    #[test]
    fn rejects_non_minimal_integer_arguments() {
        let cases: &[(&str, u64, u8)] = &[
            ("1800", 0, 1),
            ("1817", 23, 1),
            ("190000", 0, 2),
            ("1900ff", 255, 2),
            ("1a0000ffff", 65535, 4),
            ("1b00000000ffffffff", u64::from(u32::MAX), 8),
            ("3817", 23, 1),
            ("3900ff", 255, 2),
        ];
        for (hexadecimal, value, width) in cases {
            assert_eq!(
                decode_canonical(&bytes_of(hexadecimal)),
                Err(CborError::NonMinimalArgument {
                    value: *value,
                    width: *width,
                    offset: 0,
                }),
                "{hexadecimal}"
            );
        }
    }

    #[test]
    fn rejects_non_minimal_length_arguments() {
        let cases: &[(&str, u64, u8)] = &[
            ("5800", 0, 1),           // byte string, length 0 in one argument byte
            ("79000161", 1, 2),       // text string, length 1 in two argument bytes
            ("980100", 1, 1),         // array, length 1 in one argument byte
            ("ba000000010000", 1, 4), // map, length 1 in four argument bytes
        ];
        for (hexadecimal, value, width) in cases {
            assert_eq!(
                decode_canonical(&bytes_of(hexadecimal)),
                Err(CborError::NonMinimalArgument {
                    value: *value,
                    width: *width,
                    offset: 0,
                }),
                "{hexadecimal}"
            );
        }
    }

    #[test]
    fn rejects_floats_and_every_simple_value_except_null() {
        let cases: &[&str] = &[
            "f93c00",             // half-precision 1.0
            "fa3f800000",         // single-precision 1.0
            "fb3ff0000000000000", // double-precision 1.0
            "f4",                 // false
            "f5",                 // true
            "f7",                 // undefined
            "f0",                 // simple value 16
            "f818",               // one-byte simple value
            "ff",                 // break stop code
        ];
        for hexadecimal in cases {
            let error = decode_canonical(&bytes_of(hexadecimal)).expect_err("major type 7");
            assert!(
                matches!(error, CborError::UnsupportedSimpleValue { .. }),
                "{hexadecimal} gave {error:?}"
            );
        }
        assert_eq!(decode_canonical(&bytes_of("f6")), Ok(CborValue::Null));
    }

    #[test]
    fn rejects_tags() {
        for hexadecimal in ["c06161", "d8646161", "d901026161"] {
            assert_eq!(
                decode_canonical(&bytes_of(hexadecimal)),
                Err(CborError::TagNotAllowed { offset: 0 }),
                "{hexadecimal}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_map_keys() {
        assert_eq!(
            decode_canonical(&bytes_of("a201010102")),
            Err(CborError::DuplicateMapKey { offset: 3 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("a2616100616101")),
            Err(CborError::DuplicateMapKey { offset: 4 })
        );
    }

    #[test]
    fn rejects_unsorted_map_keys() {
        assert_eq!(
            decode_canonical(&bytes_of("a202000100")),
            Err(CborError::UnsortedMapKeys { offset: 3 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("a3000002000100")),
            Err(CborError::UnsortedMapKeys { offset: 5 })
        );
        // Text("zeta") is `64 7a 65 74 61`; Text("aa") is `62 61 61`. The shorter key
        // sorts first bytewise, so this longer-key-first map is not canonical.
        assert_eq!(
            decode_canonical(&bytes_of("a2647a6574610062616100")),
            Err(CborError::UnsortedMapKeys { offset: 7 })
        );
    }

    #[test]
    fn rejects_trailing_bytes() {
        assert_eq!(
            decode_canonical(&bytes_of("0000")),
            Err(CborError::TrailingBytes { count: 1 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("a0f6f6")),
            Err(CborError::TrailingBytes { count: 2 })
        );
    }

    #[test]
    fn rejects_truncated_items() {
        assert_eq!(
            decode_canonical(&[]),
            Err(CborError::Truncated {
                needed: 1,
                offset: 0
            })
        );
        assert_eq!(
            decode_canonical(&bytes_of("18")),
            Err(CborError::Truncated {
                needed: 1,
                offset: 1
            })
        );
        assert_eq!(
            decode_canonical(&bytes_of("1b0000000100")),
            Err(CborError::Truncated {
                needed: 3,
                offset: 1
            })
        );
        assert!(
            decode_canonical(&bytes_of("8101")).is_ok(),
            "a one-element array is complete"
        );
    }

    #[test]
    fn rejects_lengths_larger_than_the_remaining_input() {
        assert_eq!(
            decode_canonical(&bytes_of("582000010203")),
            Err(CborError::LengthOutOfRange {
                length: 32,
                offset: 0
            })
        );
        assert_eq!(
            decode_canonical(&bytes_of("5bffffffffffffffff")),
            Err(CborError::LengthOutOfRange {
                length: u64::MAX,
                offset: 0
            })
        );
        assert_eq!(
            decode_canonical(&bytes_of("9bffffffffffffffff")),
            Err(CborError::LengthOutOfRange {
                length: u64::MAX,
                offset: 0
            })
        );
        assert_eq!(
            decode_canonical(&bytes_of("bbffffffffffffffff")),
            Err(CborError::LengthOutOfRange {
                length: u64::MAX,
                offset: 0
            })
        );
        // The same eight-byte argument is a legal integer, not a length.
        assert_eq!(
            decode_canonical(&bytes_of("1bffffffffffffffff")),
            Ok(CborValue::Uint(u64::MAX))
        );
    }

    #[test]
    fn rejects_invalid_utf8_text() {
        assert_eq!(
            decode_canonical(&bytes_of("62ffff")),
            Err(CborError::InvalidUtf8 { offset: 0 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("61c3")),
            Err(CborError::InvalidUtf8 { offset: 0 })
        );
    }

    #[test]
    fn rejects_text_that_is_not_already_nfc() {
        // "e" followed by U+0301 COMBINING ACUTE ACCENT is the NFD form of "e-acute".
        assert_eq!(
            decode_canonical(&raw_text_item("e\u{0301}")),
            Err(CborError::NotNormalizedNfc { offset: 0 })
        );
    }

    #[test]
    fn rejects_non_nfc_text_anywhere_in_the_tree() {
        let text_item = raw_text_item("\u{03ba}e\u{0301}y");

        let mut nested_in_array = vec![0x81];
        nested_in_array.extend_from_slice(&text_item);
        assert!(matches!(
            decode_canonical(&nested_in_array),
            Err(CborError::NotNormalizedNfc { .. })
        ));

        let mut as_map_key = vec![0xa1];
        as_map_key.extend_from_slice(&text_item);
        as_map_key.push(0x00);
        assert!(matches!(
            decode_canonical(&as_map_key),
            Err(CborError::NotNormalizedNfc { .. })
        ));

        let mut as_map_value = vec![0xa1, 0x00];
        as_map_value.extend_from_slice(&text_item);
        assert!(matches!(
            decode_canonical(&as_map_value),
            Err(CborError::NotNormalizedNfc { .. })
        ));

        let mut deeply_nested = vec![0x81, 0xa1, 0x00, 0x81];
        deeply_nested.extend_from_slice(&text_item);
        assert!(matches!(
            decode_canonical(&deeply_nested),
            Err(CborError::NotNormalizedNfc { .. })
        ));
    }

    #[test]
    fn text_constructor_normalizes_to_nfc() {
        let normalized = CborValue::text("e\u{0301}");
        assert_eq!(normalized.as_text(), Some("\u{e9}"));
        let encoded = encode_canonical(&normalized);
        assert_eq!(decode_canonical(&encoded).expect("decode"), normalized);
        assert_eq!(CborValue::text("\u{03ba}ey").as_text(), Some("\u{03ba}ey"));
    }

    #[test]
    fn rejects_negative_integers_below_i64_min() {
        assert_eq!(
            decode_canonical(&bytes_of("3bffffffffffffffff")),
            Err(CborError::NegativeIntegerOutOfRange { offset: 0 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("3b8000000000000000")),
            Err(CborError::NegativeIntegerOutOfRange { offset: 0 })
        );
        assert_eq!(
            decode_canonical(&bytes_of("3b7fffffffffffffff")),
            Ok(CborValue::NegInt(i64::MIN))
        );
    }

    #[test]
    fn rejects_nesting_beyond_the_depth_limit() {
        let mut too_deep = vec![0x81; MAXIMUM_NESTING_DEPTH + 4];
        too_deep.push(0x00);
        let error = decode_canonical(&too_deep).expect_err("depth limit");
        assert!(
            matches!(error, CborError::DepthLimitExceeded { .. }),
            "{error:?}"
        );

        let mut at_limit = vec![0x81; MAXIMUM_NESTING_DEPTH];
        at_limit.push(0x00);
        assert!(decode_canonical(&at_limit).is_ok());
    }

    #[test]
    fn checked_encoding_rejects_text_that_is_not_already_nfc() {
        let unnormalized = CborValue::Text("e\u{0301}".to_owned());
        assert_eq!(
            encode_canonical_checked(&unnormalized),
            Err(CborError::UnnormalizedTextInValue)
        );
        assert_eq!(
            encode_canonical_checked(&CborValue::Array(vec![
                CborValue::Uint(0),
                unnormalized.clone(),
            ])),
            Err(CborError::UnnormalizedTextInValue)
        );
        assert_eq!(
            encode_canonical_checked(&CborValue::Map(vec![(unnormalized, CborValue::Uint(0))])),
            Err(CborError::UnnormalizedTextInValue)
        );
        assert_eq!(
            encode_canonical_checked(&CborValue::text("e\u{0301}")),
            Ok(bytes_of("62c3a9"))
        );
    }

    #[test]
    fn checked_encoding_rejects_a_map_holding_duplicate_keys() {
        let duplicated = CborValue::Map(vec![
            (CborValue::Uint(1), CborValue::Uint(0)),
            (CborValue::Uint(1), CborValue::Uint(2)),
        ]);
        assert_eq!(
            encode_canonical_checked(&duplicated),
            Err(CborError::DuplicateKeyInValue)
        );

        let nested = CborValue::Array(vec![CborValue::Map(vec![
            (CborValue::text("key"), CborValue::Uint(0)),
            (CborValue::text("key"), CborValue::Uint(1)),
        ])]);
        assert_eq!(
            encode_canonical_checked(&nested),
            Err(CborError::DuplicateKeyInValue)
        );
    }

    #[test]
    fn checked_encoding_accepts_every_value_the_decoder_accepts() {
        for hexadecimal in [
            MERGE_UNSIGNED_ENVELOPE_HEX,
            PAYLOAD_PLAINTEXT_HEX,
            ENCRYPTION_MAP_HEX,
        ] {
            let original = bytes_of(hexadecimal);
            let decoded = decode_canonical(&original).expect("golden bytes must decode");
            assert_eq!(
                encode_canonical_checked(&decoded),
                Ok(original),
                "{hexadecimal}"
            );
        }
    }

    #[test]
    fn integer_constructor_picks_the_correct_major_type() {
        assert_eq!(CborValue::integer(0), CborValue::Uint(0));
        assert_eq!(
            CborValue::integer(i64::MAX),
            CborValue::Uint(i64::MAX as u64)
        );
        assert_eq!(CborValue::integer(-1), CborValue::NegInt(-1));
        assert_eq!(CborValue::integer(i64::MIN), CborValue::NegInt(i64::MIN));
        assert_eq!(hex::encode(encode_canonical(&CborValue::integer(-1))), "20");
    }

    #[test]
    fn accessors_return_none_for_the_wrong_variant() {
        let text = CborValue::text("value");
        assert_eq!(text.as_uint(), None);
        assert_eq!(text.as_int(), None);
        assert_eq!(text.as_bytes(), None);
        assert_eq!(text.as_array(), None);
        assert_eq!(text.as_map(), None);
        assert!(!text.is_null());
        assert_eq!(text.get_uint_key(0), None);
        assert_eq!(CborValue::Uint(u64::MAX).as_int(), None);
        assert_eq!(CborValue::Null.as_text(), None);
    }
}
