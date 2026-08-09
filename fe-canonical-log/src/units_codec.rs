//! Nano-base-unit transform-vector codec (SPEC-1 §4); see `src/AGENTS.md` §module-ownership.

use fe_sdk::units::NanoVec3;
use thiserror::Error;

use crate::cbor::CborValue;

/// Number of axes in a canonical transform vector.
pub const TRANSFORM_VECTOR_LENGTH: usize = 3;

/// Every reason a canonical transform vector fails to decode.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum UnitsCodecError {
    /// The value was not a CBOR array.
    #[error("a canonical transform vector must be a CBOR array")]
    ExpectedArray,
    /// The array did not hold exactly three axes.
    #[error("a canonical transform vector holds {length} element(s), expected 3")]
    WrongLength {
        /// Elements actually present.
        length: usize,
    },
    /// An axis was not a CBOR integer inside the `i64` range.
    #[error("transform vector element {index} is not an i64 CBOR integer")]
    ExpectedInteger {
        /// Index of the offending axis.
        index: usize,
    },
}

/// Encodes a vector as three canonical CBOR signed integers in X, Y, Z order; never floats.
pub fn nano_vec3_to_cbor(vector: NanoVec3) -> CborValue {
    CborValue::Array(
        vector
            .as_array()
            .into_iter()
            .map(CborValue::integer)
            .collect(),
    )
}

/// Decodes three canonical CBOR signed integers in X, Y, Z order.
pub fn nano_vec3_from_cbor(value: &CborValue) -> Result<NanoVec3, UnitsCodecError> {
    let axes = value.as_array().ok_or(UnitsCodecError::ExpectedArray)?;
    if axes.len() != TRANSFORM_VECTOR_LENGTH {
        return Err(UnitsCodecError::WrongLength { length: axes.len() });
    }
    let mut decoded = [0i64; TRANSFORM_VECTOR_LENGTH];
    for (index, axis) in axes.iter().enumerate() {
        decoded[index] = axis
            .as_int()
            .ok_or(UnitsCodecError::ExpectedInteger { index })?;
    }
    Ok(NanoVec3::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical};

    fn round_trip(vector: NanoVec3) {
        let encoded = encode_canonical(&nano_vec3_to_cbor(vector));
        let decoded = decode_canonical(&encoded).expect("canonical bytes");
        assert_eq!(nano_vec3_from_cbor(&decoded).expect("decode"), vector);
        assert_eq!(encode_canonical(&decoded), encoded);
    }

    #[test]
    fn round_trips_the_full_i64_range() {
        round_trip(NanoVec3::new(0, 0, 0));
        round_trip(NanoVec3::new(-1, 1, 0));
        round_trip(NanoVec3::new(i64::MIN, 0, i64::MAX));
        round_trip(NanoVec3::new(i64::MAX, i64::MIN, -1));
        round_trip(NanoVec3::new(
            i64::from(i32::MIN),
            i64::from(u32::MAX),
            1_000_000_000,
        ));
    }

    #[test]
    fn encodes_the_golden_vector_axis_forms() {
        // The `intent-test-profile-unicode-and-nano-quantization` position vector.
        assert_eq!(
            hex::encode(encode_canonical(&nano_vec3_to_cbor(NanoVec3::new(
                -1,
                0,
                i64::MAX
            )))),
            "8320001b7fffffffffffffff"
        );
        // Its rotation vector.
        assert_eq!(
            hex::encode(encode_canonical(&nano_vec3_to_cbor(NanoVec3::new(
                i64::MIN,
                1,
                i64::from(u32::MAX)
            )))),
            "833b7fffffffffffffff011affffffff"
        );
        // Its scale vector, in integer parts per billion.
        assert_eq!(
            hex::encode(encode_canonical(&nano_vec3_to_cbor(NanoVec3::new(1, 1, 1)))),
            "83010101"
        );
    }

    #[test]
    fn rejects_a_non_array_a_wrong_length_and_a_non_integer_axis() {
        assert_eq!(
            nano_vec3_from_cbor(&CborValue::Uint(0)),
            Err(UnitsCodecError::ExpectedArray)
        );
        assert_eq!(
            nano_vec3_from_cbor(&CborValue::Array(vec![
                CborValue::Uint(0),
                CborValue::Uint(1)
            ])),
            Err(UnitsCodecError::WrongLength { length: 2 })
        );
        assert_eq!(
            nano_vec3_from_cbor(&CborValue::Array(vec![
                CborValue::Uint(0),
                CborValue::text("one"),
                CborValue::Uint(2),
            ])),
            Err(UnitsCodecError::ExpectedInteger { index: 1 })
        );
    }

    #[test]
    fn rejects_an_unsigned_axis_beyond_i64_max() {
        assert_eq!(
            nano_vec3_from_cbor(&CborValue::Array(vec![
                CborValue::Uint(u64::MAX),
                CborValue::Uint(0),
                CborValue::Uint(0),
            ])),
            Err(UnitsCodecError::ExpectedInteger { index: 0 })
        );
    }
}
