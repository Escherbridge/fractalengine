//! The sealed segment manifest: HashSeq roots and the availability boundary (SPEC-6 §3.3).

use std::collections::{BTreeMap, BTreeSet};

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Identifier32, PROTOCOL_VERSION};

use super::artifact::LaneClass;
use super::hashseq::LaneKey;
use super::payload_shard::PayloadTopicScope;
use super::{
    assert_canonical_bytes, hash32_at, identifier32_at, map_at, require_uint_keys, uint_at,
    SegmentError,
};

/// The oldest node a lane's coverage claim depends on (§4.1.4).
///
/// The spec fixes the *requirement* (a declared availability boundary) but not its shape; this
/// per-lane single oldest node is provisional. See `segment/AGENTS.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundaryNode {
    /// The oldest HashSeq node that must be present for the lane's claim to hold.
    pub oldest_required_node: Hash32,
    /// Its exact stored byte length (§3.3.2).
    pub stored_length: u64,
}

const BOUNDARY_CONTEXT: &str = "availability boundary";
const BOUNDARY_KEYS: &[u64] = &[0, 1];

impl BoundaryNode {
    /// Encodes the boundary map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.oldest_required_node.0.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.stored_length)),
        ])
    }

    /// Decodes the boundary map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, BOUNDARY_KEYS, BOUNDARY_CONTEXT)?;
        Ok(Self {
            oldest_required_node: hash32_at(value, 0, BOUNDARY_CONTEXT)?,
            stored_length: uint_at(value, 1, BOUNDARY_CONTEXT)?,
        })
    }
}

/// HashSeq roots for one lane, mapped to their exact stored byte lengths (§3.3.2).
pub type RootSet = BTreeMap<Hash32, u64>;

/// Payload HashSeq roots per petal-affine payload-topic scope.
pub type PayloadRootMap = BTreeMap<PayloadTopicScope, RootSet>;

/// The declared availability boundary, one entry per lane the manifest claims (§4.1.4).
pub type BoundaryMap = BTreeMap<LaneKey, BoundaryNode>;

/// A sealed immutable index over one branch's HashSeq roots (§3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentManifestBody {
    protocol_version: u64,
    verse_id: Identifier32,
    branch_id: Identifier32,
    header_roots: RootSet,
    payload_roots: PayloadRootMap,
    availability_boundary: BoundaryMap,
}

const MANIFEST_CONTEXT: &str = "segment manifest body";
const MANIFEST_KEYS: &[u64] = &[0, 1, 2, 3, 4, 5, 6];
const ROOT_SET_CONTEXT: &str = "hashseq root set";

impl SegmentManifestBody {
    /// Builds a manifest and enforces the §3.3 binding rules.
    pub fn new(
        verse_id: Identifier32,
        branch_id: Identifier32,
        header_roots: RootSet,
        payload_roots: PayloadRootMap,
        availability_boundary: BoundaryMap,
    ) -> Result<Self, SegmentError> {
        let manifest = Self {
            protocol_version: PROTOCOL_VERSION,
            verse_id,
            branch_id,
            header_roots,
            payload_roots,
            availability_boundary,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Enforces the protocol version, verse affinity, and boundary-covers-every-lane rule.
    pub fn validate(&self) -> Result<(), SegmentError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(SegmentError::UnsupportedArtifactFormatVersion {
                version: self.protocol_version,
            });
        }
        let mut lanes: BTreeSet<LaneKey> = BTreeSet::new();
        if !self.header_roots.is_empty() {
            lanes.insert(LaneKey::Header {
                verse_id: self.verse_id,
            });
        }
        for (topic, roots) in &self.payload_roots {
            if topic.verse_id != self.verse_id {
                return Err(SegmentError::ManifestForeignPayloadTopic {
                    petal_id: topic.petal_id,
                });
            }
            if roots.is_empty() {
                return Err(SegmentError::EmptyLaneBody {
                    context: ROOT_SET_CONTEXT,
                });
            }
            lanes.insert(LaneKey::Payload(*topic));
        }
        let declared: BTreeSet<LaneKey> = self.availability_boundary.keys().copied().collect();
        if declared != lanes {
            return Err(SegmentError::ManifestBoundaryLaneMismatch);
        }
        Ok(())
    }

    /// Verse this manifest indexes.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// Branch this manifest indexes.
    pub fn branch_id(&self) -> Identifier32 {
        self.branch_id
    }

    /// Header HashSeq roots and their exact stored lengths, deduplicated by artifact ID.
    pub fn header_roots(&self) -> &RootSet {
        &self.header_roots
    }

    /// Payload HashSeq roots per petal-affine payload-topic scope.
    pub fn payload_roots(&self) -> &PayloadRootMap {
        &self.payload_roots
    }

    /// Every lane this manifest claims coverage for.
    pub fn lanes(&self) -> impl Iterator<Item = LaneKey> + '_ {
        self.availability_boundary.keys().copied()
    }

    /// Roots for one lane, or `None` when the manifest claims no coverage there.
    pub fn roots_for(&self, lane: &LaneKey) -> Option<&RootSet> {
        match lane {
            LaneKey::Header { verse_id } if *verse_id == self.verse_id => {
                Some(&self.header_roots).filter(|roots| !roots.is_empty())
            }
            LaneKey::Header { .. } => None,
            LaneKey::Payload(topic) => self.payload_roots.get(topic),
        }
    }

    /// The declared availability boundary for one lane (§4.1.4).
    pub fn boundary_for(&self, lane: &LaneKey) -> Option<BoundaryNode> {
        self.availability_boundary.get(lane).copied()
    }

    /// Encodes the inner body.
    pub fn to_cbor(&self) -> Result<CborValue, SegmentError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::SegmentManifest.to_wire()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.protocol_version)),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.verse_id.0.to_vec()),
            ),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.branch_id.0.to_vec()),
            ),
            (CborValue::Uint(4), encode_root_set(&self.header_roots)),
            (
                CborValue::Uint(5),
                CborValue::Map(
                    self.payload_roots
                        .iter()
                        .map(|(topic, roots)| (topic.to_cbor(), encode_root_set(roots)))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(6),
                CborValue::Map(
                    self.availability_boundary
                        .iter()
                        .map(|(lane, boundary)| (lane.to_cbor(), boundary.to_cbor()))
                        .collect(),
                ),
            ),
        ]))
    }

    /// The exact plaintext bytes that get sealed.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes a decrypted manifest body.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        require_uint_keys(&value, MANIFEST_KEYS, MANIFEST_CONTEXT)?;
        if uint_at(&value, 0, MANIFEST_CONTEXT)? != LaneClass::SegmentManifest.to_wire() {
            return Err(SegmentError::LaneBodyMismatch);
        }
        let protocol_version = uint_at(&value, 1, MANIFEST_CONTEXT)?;
        if protocol_version != PROTOCOL_VERSION {
            return Err(SegmentError::UnsupportedArtifactFormatVersion {
                version: protocol_version,
            });
        }
        let verse_id = identifier32_at(&value, 2, MANIFEST_CONTEXT)?;
        let branch_id = identifier32_at(&value, 3, MANIFEST_CONTEXT)?;
        let header_roots = decode_root_set(map_at(&value, 4, MANIFEST_CONTEXT)?)?;

        let mut payload_roots = BTreeMap::new();
        for (topic, roots) in map_at(&value, 5, MANIFEST_CONTEXT)? {
            let topic = PayloadTopicScope::from_cbor(topic)?;
            let roots =
                decode_root_set(roots.as_map().ok_or(SegmentError::FieldTypeMismatch {
                    context: MANIFEST_CONTEXT,
                    key: 5,
                })?)?;
            payload_roots.insert(topic, roots);
        }

        let mut availability_boundary = BTreeMap::new();
        for (lane, boundary) in map_at(&value, 6, MANIFEST_CONTEXT)? {
            availability_boundary.insert(
                LaneKey::from_cbor(lane)?,
                BoundaryNode::from_cbor(boundary)?,
            );
        }

        let manifest = Self::new(
            verse_id,
            branch_id,
            header_roots,
            payload_roots,
            availability_boundary,
        )?;
        assert_canonical_bytes(&value, bytes, MANIFEST_CONTEXT)?;
        Ok(manifest)
    }
}

/// Encodes an artifact-ID-to-stored-length root set.
fn encode_root_set(roots: &RootSet) -> CborValue {
    CborValue::Map(
        roots
            .iter()
            .map(|(artifact_id, stored_length)| {
                (
                    CborValue::Bytes(artifact_id.0.to_vec()),
                    CborValue::Uint(*stored_length),
                )
            })
            .collect(),
    )
}

/// Decodes an artifact-ID-to-stored-length root set, refusing a conflicting length (§3.3.4).
fn decode_root_set(entries: &[(CborValue, CborValue)]) -> Result<RootSet, SegmentError> {
    let mut roots = BTreeMap::new();
    for (artifact_id, stored_length) in entries {
        let raw = artifact_id
            .as_bytes()
            .ok_or(SegmentError::FieldTypeMismatch {
                context: ROOT_SET_CONTEXT,
                key: 0,
            })?;
        let artifact_id =
            Hash32(
                <[u8; 32]>::try_from(raw).map_err(|_| SegmentError::WrongByteLength {
                    context: ROOT_SET_CONTEXT,
                    key: 0,
                    expected: 32,
                    actual: raw.len(),
                })?,
            );
        let stored_length = stored_length
            .as_uint()
            .ok_or(SegmentError::FieldTypeMismatch {
                context: ROOT_SET_CONTEXT,
                key: 1,
            })?;
        if let Some(existing) = roots.insert(artifact_id, stored_length) {
            if existing != stored_length {
                return Err(SegmentError::ConflictingStoredLength { artifact_id });
            }
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::test_fixtures::{digest, identifier};

    fn topic(petal: u8, epoch: u64) -> PayloadTopicScope {
        PayloadTopicScope {
            verse_id: identifier(0x11),
            petal_id: identifier(petal),
            scope_epoch: epoch,
            key_id: identifier(0x41),
        }
    }

    fn roots(entries: &[(u8, u64)]) -> BTreeMap<Hash32, u64> {
        entries
            .iter()
            .map(|(filler, length)| (digest(*filler), *length))
            .collect()
    }

    fn boundary(filler: u8) -> BoundaryNode {
        BoundaryNode {
            oldest_required_node: digest(filler),
            stored_length: 512,
        }
    }

    fn manifest() -> SegmentManifestBody {
        let verse = identifier(0x11);
        let header_lane = LaneKey::Header { verse_id: verse };
        let first = topic(0x21, 7);
        let second = topic(0x22, 7);
        SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            roots(&[(0xA1, 100), (0xA2, 200)]),
            BTreeMap::from([
                (first, roots(&[(0xC1, 300)])),
                (second, roots(&[(0xC2, 400)])),
            ]),
            BTreeMap::from([
                (header_lane, boundary(0xA2)),
                (LaneKey::Payload(first), boundary(0xC1)),
                (LaneKey::Payload(second), boundary(0xC2)),
            ]),
        )
        .expect("manifest")
    }

    #[test]
    fn manifest_binds_roots_lengths_and_the_availability_boundary() {
        let manifest = manifest();
        let verse = identifier(0x11);
        assert_eq!(manifest.verse_id(), verse);
        assert_eq!(manifest.branch_id(), identifier(0xB1));
        assert_eq!(manifest.lanes().count(), 3);
        assert_eq!(
            manifest.roots_for(&LaneKey::Header { verse_id: verse }),
            Some(&roots(&[(0xA1, 100), (0xA2, 200)]))
        );
        assert_eq!(
            manifest.roots_for(&LaneKey::Payload(topic(0x21, 7))),
            Some(&roots(&[(0xC1, 300)]))
        );
        assert_eq!(manifest.roots_for(&LaneKey::Payload(topic(0x21, 8))), None);
        assert_eq!(
            manifest.boundary_for(&LaneKey::Header { verse_id: verse }),
            Some(boundary(0xA2))
        );

        let bytes = manifest.encode_canonical().expect("bytes");
        assert_eq!(
            SegmentManifestBody::decode_canonical(&bytes).expect("round trip"),
            manifest
        );
    }

    #[test]
    fn manifest_refuses_an_uncovered_lane_or_a_foreign_payload_topic() {
        let verse = identifier(0x11);
        let uncovered = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            roots(&[(0xA1, 100)]),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        assert_eq!(uncovered, Err(SegmentError::ManifestBoundaryLaneMismatch));

        let stray_boundary = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(LaneKey::Header { verse_id: verse }, boundary(0xA2))]),
        );
        assert_eq!(
            stray_boundary,
            Err(SegmentError::ManifestBoundaryLaneMismatch)
        );

        let foreign_topic = PayloadTopicScope {
            verse_id: identifier(0x99),
            ..topic(0x21, 7)
        };
        let foreign = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            BTreeMap::new(),
            BTreeMap::from([(foreign_topic, roots(&[(0xC1, 300)]))]),
            BTreeMap::from([(LaneKey::Payload(foreign_topic), boundary(0xC1))]),
        );
        assert_eq!(
            foreign,
            Err(SegmentError::ManifestForeignPayloadTopic {
                petal_id: identifier(0x21)
            })
        );
    }

    #[test]
    fn manifest_deduplicates_roots_by_artifact_id() {
        let duplicated = vec![
            (
                CborValue::Bytes(digest(0xA1).0.to_vec()),
                CborValue::Uint(100),
            ),
            (
                CborValue::Bytes(digest(0xA1).0.to_vec()),
                CborValue::Uint(101),
            ),
        ];
        assert_eq!(
            decode_root_set(&duplicated),
            Err(SegmentError::ConflictingStoredLength {
                artifact_id: digest(0xA1)
            })
        );
    }
}
