//! SPEC-10 §4.3 `observation_batch` payload codec; see `src/observation/AGENTS.md` §payload.

use thiserror::Error;

use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{Hash32, Hlc, Identifier32};
use crate::materialize::identity::{ObservationSource, ObservationSourceError};

/// The only `hexon_format_version` SPEC-10 v1 defines (§4.3 key 9).
pub const HEXON_FORMAT_VERSION: u16 = 1;

const PAYLOAD_CONTEXT: &str = "observation_batch payload";
const RANGE_CONTEXT: &str = "observation_batch range";
const PAYLOAD_KEYS: &[u64] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
const RANGE_KEYS: &[u64] = &[0, 1];

/// Every reason a kind-5 payload is refused (SPEC-10 §4.3; `invalid_payload` under SPEC-4 §5).
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ObservationPayloadError {
    /// The bytes were not canonical CBOR: a float, a duplicate key, a non-shortest length...
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A `sources` entry broke SPEC-10 §7.1.
    #[error("observation source: {0}")]
    Source(#[from] ObservationSourceError),
    /// A value that must be a map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// Map being parsed.
        context: &'static str,
    },
    /// A key outside the grammar, a non-integer key, or a repeated key was present (rule 1).
    #[error("{context} carries an unknown, repeated, or non-integer key")]
    UnknownKey {
        /// Map being parsed.
        context: &'static str,
    },
    /// A listed key was absent (rule 1).
    #[error("{context} key {key} is missing")]
    MissingKey {
        /// Map being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key held the wrong CBOR type, an out-of-range integer, or a wrong-length byte string.
    #[error("{context} key {key} holds the wrong CBOR type or length")]
    WrongType {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// `hlc_range` had its minimum after its maximum (rule 2).
    #[error("hlc_range minimum is after its maximum")]
    InvertedHlcRange,
    /// `sensor_time_range_ms` had its minimum after its maximum (rule 2).
    #[error("sensor_time_range_ms minimum is after its maximum")]
    InvertedSensorTimeRange,
    /// `count` was zero: a batch with nothing in it is never sealed (rule 3).
    #[error("count is zero")]
    ZeroCount,
    /// `anchors` was empty (rule 4).
    #[error("anchors is empty")]
    EmptyAnchors,
    /// `anchors` named more anchors than there are rows (rule 4).
    #[error("{anchors} anchors exceed count {count}")]
    MoreAnchorsThanCount {
        /// Anchors listed.
        anchors: usize,
        /// Rows counted.
        count: u64,
    },
    /// `anchors` was not strictly ascending (rule 4).
    #[error("anchors are not strictly ascending")]
    AnchorsNotStrictlyAscending,
    /// `asset_refs` was not strictly ascending (rule 5).
    #[error("asset_refs are not strictly ascending")]
    AssetRefsNotStrictlyAscending,
    /// `sources` was empty (rule 6).
    #[error("sources is empty")]
    EmptySources,
    /// `sources` was not strictly ascending by canonical encoding (rule 6).
    #[error("sources are not strictly ascending by canonical encoding")]
    SourcesNotStrictlyAscending,
    /// `hexon_format_version` was not [`HEXON_FORMAT_VERSION`] (§4.3 key 9).
    #[error("hexon_format_version {version} is not supported")]
    UnsupportedHexonFormatVersion {
        /// The refused version.
        version: u16,
    },
    /// `hexon_artifact_id` equalled `stats_hash` (rule 7).
    #[error("hexon_artifact_id and stats_hash name the same artifact")]
    HexonIsStatisticsArtifact,
    /// A stored length was zero (rule 7).
    #[error("{field} is zero")]
    ZeroStoredLength {
        /// The zero field.
        field: &'static str,
    },
    /// The bytes decoded but were not their own canonical re-encoding.
    #[error("observation_batch payload bytes are not their own canonical re-encoding")]
    NonCanonicalEncoding,
}

/// An inclusive ingestion-HLC range, ordered by `(wall_ms, counter)` (§4.3 key 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HlcRange {
    /// Earliest ingestion HLC over every row.
    pub min: Hlc,
    /// Latest ingestion HLC over every row.
    pub max: Hlc,
}

/// An inclusive `recorded_at_ms` (sensor time) range (§4.3 key 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SensorTimeRange {
    /// Earliest sensor time, Unix milliseconds.
    pub min_ms: i64,
    /// Latest sensor time, Unix milliseconds.
    pub max_ms: i64,
}

/// The fields of an [`ObservationBatchPayload`] before validation; [`ObservationBatchPayload::new`]
/// is the only door from these to a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationBatchPayloadParts {
    /// Key 0: SPEC-6 `artifact_id` of the sealed observation hexon.
    pub hexon_artifact_id: Hash32,
    /// Key 1: exact stored byte length of that artifact.
    pub hexon_stored_length: u64,
    /// Key 2: inclusive ingestion-HLC range over every row.
    pub hlc_range: HlcRange,
    /// Key 3: inclusive sensor-time range over every row.
    pub sensor_time_range: SensorTimeRange,
    /// Key 4: `artifact_id` of the sealed statistics artifact.
    pub stats_hash: Hash32,
    /// Key 5: rows across both Parquet members.
    pub count: u64,
    /// Key 6: the anchors (SPEC-10 §4.5 node identifiers, see `observation::node_anchor`).
    pub anchors: Vec<Identifier32>,
    /// Key 7: the asset content hashes.
    pub asset_refs: Vec<Hash32>,
    /// Key 8: the observation sources.
    pub sources: Vec<ObservationSource>,
    /// Key 9: the hexon's Parquet profile version.
    pub hexon_format_version: u16,
    /// Key 10: exact stored byte length of the statistics artifact.
    pub stats_stored_length: u64,
}

impl ObservationBatchPayloadParts {
    /// Sorts and deduplicates `anchors`, `asset_refs`, and `sources` into their signed order, so
    /// a sealer can collect them in ingest order and still hand [`ObservationBatchPayload::new`]
    /// strictly ascending sets.
    pub fn normalized(mut self) -> Self {
        self.anchors.sort_unstable();
        self.anchors.dedup();
        self.asset_refs.sort_unstable();
        self.asset_refs.dedup();
        self.sources.sort();
        self.sources.dedup();
        self
    }
}

/// The decrypted, validated payload of one kind-5 operation (SPEC-10 §4.3). Every §4.3 rule holds
/// by construction: the fields are private and [`Self::new`], [`Self::from_cbor`], and
/// [`Self::decode_canonical`] are the only doors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationBatchPayload {
    parts: ObservationBatchPayloadParts,
}

impl ObservationBatchPayload {
    /// Validates `parts` against every §4.3 rule; it never sorts or repairs anything.
    pub fn new(parts: ObservationBatchPayloadParts) -> Result<Self, ObservationPayloadError> {
        if parts.hlc_range.min > parts.hlc_range.max {
            return Err(ObservationPayloadError::InvertedHlcRange);
        }
        if parts.sensor_time_range.min_ms > parts.sensor_time_range.max_ms {
            return Err(ObservationPayloadError::InvertedSensorTimeRange);
        }
        if parts.count == 0 {
            return Err(ObservationPayloadError::ZeroCount);
        }
        if parts.anchors.is_empty() {
            return Err(ObservationPayloadError::EmptyAnchors);
        }
        if parts.anchors.len() as u64 > parts.count {
            return Err(ObservationPayloadError::MoreAnchorsThanCount {
                anchors: parts.anchors.len(),
                count: parts.count,
            });
        }
        if !strictly_ascending(&parts.anchors) {
            return Err(ObservationPayloadError::AnchorsNotStrictlyAscending);
        }
        if !strictly_ascending(&parts.asset_refs) {
            return Err(ObservationPayloadError::AssetRefsNotStrictlyAscending);
        }
        if parts.sources.is_empty() {
            return Err(ObservationPayloadError::EmptySources);
        }
        let encoded_sources = parts
            .sources
            .iter()
            .map(ObservationSource::encode_canonical)
            .collect::<Result<Vec<_>, _>>()?;
        if !strictly_ascending(&encoded_sources) {
            return Err(ObservationPayloadError::SourcesNotStrictlyAscending);
        }
        if parts.hexon_format_version != HEXON_FORMAT_VERSION {
            return Err(ObservationPayloadError::UnsupportedHexonFormatVersion {
                version: parts.hexon_format_version,
            });
        }
        if parts.hexon_artifact_id == parts.stats_hash {
            return Err(ObservationPayloadError::HexonIsStatisticsArtifact);
        }
        if parts.hexon_stored_length == 0 {
            return Err(ObservationPayloadError::ZeroStoredLength {
                field: "hexon_stored_length",
            });
        }
        if parts.stats_stored_length == 0 {
            return Err(ObservationPayloadError::ZeroStoredLength {
                field: "stats_stored_length",
            });
        }
        Ok(Self { parts })
    }

    /// Every field, read-only.
    pub fn parts(&self) -> &ObservationBatchPayloadParts {
        &self.parts
    }

    /// Releases the validated fields.
    pub fn into_parts(self) -> ObservationBatchPayloadParts {
        self.parts
    }

    /// Encodes the eleven-key §4.3 map.
    pub fn to_cbor(&self) -> CborValue {
        let parts = &self.parts;
        let uint = |key: u64, value: u64| (CborValue::Uint(key), CborValue::Uint(value));
        let hash =
            |key: u64, value: &Hash32| (CborValue::Uint(key), CborValue::Bytes(value.0.to_vec()));
        CborValue::Map(vec![
            hash(0, &parts.hexon_artifact_id),
            uint(1, parts.hexon_stored_length),
            (
                CborValue::Uint(2),
                range_map(parts.hlc_range.min.to_cbor(), parts.hlc_range.max.to_cbor()),
            ),
            (
                CborValue::Uint(3),
                range_map(
                    CborValue::integer(parts.sensor_time_range.min_ms),
                    CborValue::integer(parts.sensor_time_range.max_ms),
                ),
            ),
            hash(4, &parts.stats_hash),
            uint(5, parts.count),
            (
                CborValue::Uint(6),
                CborValue::Array(
                    parts
                        .anchors
                        .iter()
                        .map(|anchor| CborValue::Bytes(anchor.0.to_vec()))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(7),
                CborValue::Array(
                    parts
                        .asset_refs
                        .iter()
                        .map(|asset| CborValue::Bytes(asset.0.to_vec()))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(8),
                CborValue::Array(
                    parts
                        .sources
                        .iter()
                        .map(ObservationSource::to_cbor)
                        .collect(),
                ),
            ),
            uint(9, u64::from(parts.hexon_format_version)),
            uint(10, parts.stats_stored_length),
        ])
    }

    /// The exact canonical plaintext the kind-5 operation seals.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ObservationPayloadError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes an already-decoded payload map (for example `PayloadFacts::fields`), enforcing
    /// every §4.3 rule.
    pub fn from_cbor(value: &CborValue) -> Result<Self, ObservationPayloadError> {
        require_exact_keys(value, PAYLOAD_KEYS, PAYLOAD_CONTEXT)?;
        let hlc = field(value, 2, PAYLOAD_CONTEXT)?;
        require_exact_keys(hlc, RANGE_KEYS, RANGE_CONTEXT)?;
        let sensor = field(value, 3, PAYLOAD_CONTEXT)?;
        require_exact_keys(sensor, RANGE_KEYS, RANGE_CONTEXT)?;
        let hexon_format_version =
            u16::try_from(uint(value, 9, PAYLOAD_CONTEXT)?).map_err(|_| {
                ObservationPayloadError::WrongType {
                    context: PAYLOAD_CONTEXT,
                    key: 9,
                }
            })?;
        Self::new(ObservationBatchPayloadParts {
            hexon_artifact_id: Hash32(bytes32(value, 0, PAYLOAD_CONTEXT)?),
            hexon_stored_length: uint(value, 1, PAYLOAD_CONTEXT)?,
            hlc_range: HlcRange {
                min: hlc_at(hlc, 0)?,
                max: hlc_at(hlc, 1)?,
            },
            sensor_time_range: SensorTimeRange {
                min_ms: int(sensor, 0, RANGE_CONTEXT)?,
                max_ms: int(sensor, 1, RANGE_CONTEXT)?,
            },
            stats_hash: Hash32(bytes32(value, 4, PAYLOAD_CONTEXT)?),
            count: uint(value, 5, PAYLOAD_CONTEXT)?,
            anchors: array(value, 6)?
                .iter()
                .map(|item| item_bytes32(item, 6).map(Identifier32))
                .collect::<Result<_, _>>()?,
            asset_refs: array(value, 7)?
                .iter()
                .map(|item| item_bytes32(item, 7).map(Hash32))
                .collect::<Result<_, _>>()?,
            sources: array(value, 8)?
                .iter()
                .map(ObservationSource::from_cbor)
                .collect::<Result<_, _>>()?,
            hexon_format_version,
            stats_stored_length: uint(value, 10, PAYLOAD_CONTEXT)?,
        })
    }

    /// Decodes standalone plaintext: canonical CBOR first, then every §4.3 rule.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ObservationPayloadError> {
        let value = decode_canonical(bytes)?;
        let payload = Self::from_cbor(&value)?;
        if encode_canonical_checked(&value)? != bytes {
            return Err(ObservationPayloadError::NonCanonicalEncoding);
        }
        Ok(payload)
    }
}

/// The schema validator a payload schema registry registers for kind 5 under
/// `observation::observation_batch_schema_hash()`: true exactly when [`ObservationBatchPayload::
/// decode_canonical`] accepts `plaintext`.
pub fn validate_observation_batch_plaintext(plaintext: &[u8]) -> bool {
    ObservationBatchPayload::decode_canonical(plaintext).is_ok()
}

fn strictly_ascending<T: Ord>(items: &[T]) -> bool {
    items.windows(2).all(|pair| pair[0] < pair[1])
}

fn range_map(min: CborValue, max: CborValue) -> CborValue {
    CborValue::Map(vec![(CborValue::Uint(0), min), (CborValue::Uint(1), max)])
}

/// Requires exactly `expected` integer keys, each once, and nothing else.
fn require_exact_keys(
    value: &CborValue,
    expected: &[u64],
    context: &'static str,
) -> Result<(), ObservationPayloadError> {
    let entries = value
        .as_map()
        .ok_or(ObservationPayloadError::ExpectedMap { context })?;
    for (key, _) in entries {
        match key.as_uint() {
            Some(key) if expected.contains(&key) => {}
            _ => return Err(ObservationPayloadError::UnknownKey { context }),
        }
    }
    for &key in expected {
        if value.get_uint_key(key).is_none() {
            return Err(ObservationPayloadError::MissingKey { context, key });
        }
    }
    if entries.len() != expected.len() {
        return Err(ObservationPayloadError::UnknownKey { context });
    }
    Ok(())
}

fn field<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, ObservationPayloadError> {
    value
        .get_uint_key(key)
        .ok_or(ObservationPayloadError::MissingKey { context, key })
}

fn uint(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u64, ObservationPayloadError> {
    field(value, key, context)?
        .as_uint()
        .ok_or(ObservationPayloadError::WrongType { context, key })
}

fn int(value: &CborValue, key: u64, context: &'static str) -> Result<i64, ObservationPayloadError> {
    field(value, key, context)?
        .as_int()
        .ok_or(ObservationPayloadError::WrongType { context, key })
}

fn bytes32(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; 32], ObservationPayloadError> {
    item_bytes32(field(value, key, context)?, key)
}

fn item_bytes32(item: &CborValue, key: u64) -> Result<[u8; 32], ObservationPayloadError> {
    item.as_bytes()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or(ObservationPayloadError::WrongType {
            context: PAYLOAD_CONTEXT,
            key,
        })
}

fn array(value: &CborValue, key: u64) -> Result<&[CborValue], ObservationPayloadError> {
    field(value, key, PAYLOAD_CONTEXT)?
        .as_array()
        .ok_or(ObservationPayloadError::WrongType {
            context: PAYLOAD_CONTEXT,
            key,
        })
}

fn hlc_at(range: &CborValue, key: u64) -> Result<Hlc, ObservationPayloadError> {
    Hlc::from_cbor(field(range, key, RANGE_CONTEXT)?).map_err(|_| {
        ObservationPayloadError::WrongType {
            context: RANGE_CONTEXT,
            key,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materialize::identity::SourceClass;

    fn source(label: &str) -> ObservationSource {
        ObservationSource::new(
            SourceClass::Device,
            ObservationSource::source_id_for(SourceClass::Device, label),
            [0x42; 32],
            None,
            None,
        )
        .expect("device source")
    }

    fn unsorted_parts() -> ObservationBatchPayloadParts {
        ObservationBatchPayloadParts {
            hexon_artifact_id: Hash32([0x01; 32]),
            hexon_stored_length: 4_096,
            hlc_range: HlcRange {
                min: Hlc::new(1_000, 0),
                max: Hlc::new(2_000, 3),
            },
            sensor_time_range: SensorTimeRange {
                min_ms: -5,
                max_ms: 1_900,
            },
            stats_hash: Hash32([0x02; 32]),
            count: 6,
            anchors: vec![
                Identifier32([0x30; 32]),
                Identifier32([0x10; 32]),
                Identifier32([0x30; 32]),
            ],
            asset_refs: vec![Hash32([0x90; 32]), Hash32([0x80; 32])],
            sources: vec![source("sensor-b"), source("sensor-a"), source("sensor-b")],
            hexon_format_version: HEXON_FORMAT_VERSION,
            stats_stored_length: 512,
        }
    }

    /// Ingest-order sets are refused until normalized, and a normalized payload survives the
    /// canonical round trip byte for byte (the bytes the kind-5 operation signs).
    #[test]
    fn normalized_parts_round_trip_canonically_and_raw_ingest_order_is_refused() {
        assert_eq!(
            ObservationBatchPayload::new(unsorted_parts()),
            Err(ObservationPayloadError::AnchorsNotStrictlyAscending)
        );

        let payload =
            ObservationBatchPayload::new(unsorted_parts().normalized()).expect("normalized");
        assert_eq!(payload.parts().anchors.len(), 2);
        assert_eq!(
            payload.parts().asset_refs,
            vec![Hash32([0x80; 32]), Hash32([0x90; 32])]
        );
        assert_eq!(payload.parts().sources.len(), 2);
        assert!(payload.parts().sources[0] < payload.parts().sources[1]);

        let bytes = payload.encode_canonical().expect("encode");
        assert_eq!(
            ObservationBatchPayload::decode_canonical(&bytes).expect("decode"),
            payload
        );
        assert!(validate_observation_batch_plaintext(&bytes));
        assert!(!validate_observation_batch_plaintext(
            &bytes[..bytes.len() - 1]
        ));
    }
}
