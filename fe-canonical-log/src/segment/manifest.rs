//! The sealed segment manifest: HashSeq roots and the availability boundary (SPEC-6 §3.3).

use std::collections::{BTreeMap, BTreeSet};

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Hlc, Identifier32, PROTOCOL_VERSION};

use super::artifact::LaneClass;
use super::hashseq::LaneKey;
use super::payload_shard::PayloadTopicScope;
use super::{
    array_at, assert_canonical_bytes, hash32_at, identifier32_at, map_at, require_uint_keys,
    uint_at, value_at, SegmentError,
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

/// Coarse statistics over the operations a manifest indexes, in the manifest's own clear text
/// (§3.3.5, D-CL28 gate G4).
///
/// **Why only these three fields.** A segment manifest is sealed under the *verse-wide header*
/// scope (§2.2.2), so everything in this struct is legible to every authorized verse member,
/// including one with no payload capability for any petal the manifest indexes. That is the
/// right visibility for segment skipping — a peer must be able to decide "this segment cannot
/// contain the range I want" without fetching it — and the wrong visibility for anything
/// derived from payload contents. A per-column minimum and maximum on a position column would
/// disclose a project's real-world location verse-wide; those statistics live in a separate
/// artifact sealed under the lane's own scope key instead (see [`SealedStatisticsRef`]).
///
/// Every field here is derivable from header-plane facts alone: HLC and scope are signed
/// envelope header fields, never payload plaintext.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentStatistics {
    /// Lowest author HLC across the indexed operations, inclusive.
    pub min_hlc: Hlc,
    /// Highest author HLC across the indexed operations, inclusive.
    pub max_hlc: Hlc,
    /// Petals the indexed operations name. Empty when every indexed operation is verse-scoped.
    pub petals: BTreeSet<Identifier32>,
    /// Number of operations the manifest indexes.
    pub operation_count: u64,
}

const STATISTICS_CONTEXT: &str = "segment statistics";
const STATISTICS_KEYS: &[u64] = &[0, 1, 2, 3];

impl SegmentStatistics {
    /// Encodes the clear statistics block.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), self.min_hlc.to_cbor()),
            (CborValue::Uint(1), self.max_hlc.to_cbor()),
            (
                CborValue::Uint(2),
                CborValue::Array(
                    self.petals
                        .iter()
                        .map(|petal| CborValue::Bytes(petal.0.to_vec()))
                        .collect(),
                ),
            ),
            (CborValue::Uint(3), CborValue::Uint(self.operation_count)),
        ])
    }

    /// Decodes the clear statistics block, refusing a non-ascending petal set.
    ///
    /// `BTreeSet` would silently absorb an out-of-order or duplicated petal, and the manifest's
    /// own re-encode pin would then reject the bytes with a generic non-canonical error. The
    /// explicit check keeps the diagnostic specific and does not rely on the pin.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, STATISTICS_KEYS, STATISTICS_CONTEXT)?;
        let min_hlc = hlc_at(value, 0)?;
        let max_hlc = hlc_at(value, 1)?;
        let entries = array_at(value, 2, STATISTICS_CONTEXT)?;

        let mut petals = BTreeSet::new();
        let mut previous: Option<Identifier32> = None;
        for entry in entries {
            let raw = entry.as_bytes().ok_or(SegmentError::FieldTypeMismatch {
                context: STATISTICS_CONTEXT,
                key: 2,
            })?;
            let petal = Identifier32(<[u8; 32]>::try_from(raw).map_err(|_| {
                SegmentError::WrongByteLength {
                    context: STATISTICS_CONTEXT,
                    key: 2,
                    expected: 32,
                    actual: raw.len(),
                }
            })?);
            if previous.is_some_and(|previous| previous >= petal) {
                return Err(SegmentError::ManifestStatisticsPetalsNotAscending);
            }
            previous = Some(petal);
            petals.insert(petal);
        }

        Ok(Self {
            min_hlc,
            max_hlc,
            petals,
            operation_count: uint_at(value, 3, STATISTICS_CONTEXT)?,
        })
    }
}

/// A reference to the fine-grained statistics artifact for one lane (§3.3.6, gate G4).
///
/// Column-level statistics — per-column minima and maxima, histograms, distinct counts, bloom
/// filters — are derived from payload contents, so they are sealed under that lane's own scope
/// key and reachable only by content address from here. A holder that can already decrypt the
/// lane learns nothing new; a holder that cannot learns only that the artifact exists and how
/// many bytes it occupies. The artifact's interior shape is deliberately unspecified by this
/// erratum: the wire slot is what is expensive to add later, not the statistics format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SealedStatisticsRef {
    /// Artifact ID of the sealed statistics artifact.
    pub artifact_id: Hash32,
    /// Its exact stored byte length (§3.3.2).
    pub stored_length: u64,
}

const SEALED_STATISTICS_CONTEXT: &str = "sealed statistics reference";
const SEALED_STATISTICS_KEYS: &[u64] = &[0, 1];

impl SealedStatisticsRef {
    /// Encodes the reference map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.artifact_id.0.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.stored_length)),
        ])
    }

    /// Decodes the reference map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, SEALED_STATISTICS_KEYS, SEALED_STATISTICS_CONTEXT)?;
        Ok(Self {
            artifact_id: hash32_at(value, 0, SEALED_STATISTICS_CONTEXT)?,
            stored_length: uint_at(value, 1, SEALED_STATISTICS_CONTEXT)?,
        })
    }
}

/// Sealed fine-grained statistics artifacts, at most one per lane the manifest claims (§3.3.6).
pub type SealedStatisticsMap = BTreeMap<LaneKey, SealedStatisticsRef>;

/// Decodes an HLC sub-map inside the statistics block.
fn hlc_at(value: &CborValue, key: u64) -> Result<Hlc, SegmentError> {
    Ok(Hlc::from_cbor(value_at(value, key, STATISTICS_CONTEXT)?)?)
}

/// A sealed immutable index over one branch's HashSeq roots (§3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentManifestBody {
    protocol_version: u64,
    verse_id: Identifier32,
    branch_id: Identifier32,
    header_roots: RootSet,
    payload_roots: PayloadRootMap,
    availability_boundary: BoundaryMap,
    statistics: SegmentStatistics,
    sealed_statistics: SealedStatisticsMap,
}

const MANIFEST_CONTEXT: &str = "segment manifest body";
const MANIFEST_KEYS: &[u64] = &[0, 1, 2, 3, 4, 5, 6, 7, 8];
const ROOT_SET_CONTEXT: &str = "hashseq root set";

impl SegmentManifestBody {
    /// Builds a manifest and enforces the §3.3 binding rules.
    ///
    /// `statistics` is a required parameter, never an `Option`: a manifest without a range is
    /// one no peer can skip, so segment skipping would silently degrade to fetch-everything
    /// wherever a publisher omitted it (§3.3.5).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        verse_id: Identifier32,
        branch_id: Identifier32,
        header_roots: RootSet,
        payload_roots: PayloadRootMap,
        availability_boundary: BoundaryMap,
        statistics: SegmentStatistics,
        sealed_statistics: SealedStatisticsMap,
    ) -> Result<Self, SegmentError> {
        let manifest = Self {
            protocol_version: PROTOCOL_VERSION,
            verse_id,
            branch_id,
            header_roots,
            payload_roots,
            availability_boundary,
            statistics,
            sealed_statistics,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Enforces the protocol version, verse affinity, boundary-covers-every-lane rule, and the
    /// §3.3.5/§3.3.6 statistics consistency rules.
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

        if self.statistics.min_hlc > self.statistics.max_hlc {
            return Err(SegmentError::ManifestStatisticsInconsistent);
        }
        // A count and a lane set must agree about whether this manifest indexes anything, so a
        // manifest cannot advertise a range it has no roots to serve, or claim emptiness while
        // carrying roots.
        if lanes.is_empty() != (self.statistics.operation_count == 0) {
            return Err(SegmentError::ManifestStatisticsInconsistent);
        }
        if !self
            .sealed_statistics
            .keys()
            .all(|lane| lanes.contains(lane))
        {
            return Err(SegmentError::ManifestStatisticsForeignLane);
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

    /// The coarse clear statistics every authorized verse member may read (§3.3.5).
    pub fn statistics(&self) -> &SegmentStatistics {
        &self.statistics
    }

    /// The sealed fine-grained statistics artifact for one lane, if the publisher wrote one
    /// (§3.3.6). Resolving its contents requires that lane's scope key.
    pub fn sealed_statistics_for(&self, lane: &LaneKey) -> Option<SealedStatisticsRef> {
        self.sealed_statistics.get(lane).copied()
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
            (CborValue::Uint(7), self.statistics.to_cbor()),
            (
                CborValue::Uint(8),
                CborValue::Map(
                    self.sealed_statistics
                        .iter()
                        .map(|(lane, reference)| (lane.to_cbor(), reference.to_cbor()))
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

        let statistics = SegmentStatistics::from_cbor(value_at(&value, 7, MANIFEST_CONTEXT)?)?;

        let mut sealed_statistics = BTreeMap::new();
        for (lane, reference) in map_at(&value, 8, MANIFEST_CONTEXT)? {
            sealed_statistics.insert(
                LaneKey::from_cbor(lane)?,
                SealedStatisticsRef::from_cbor(reference)?,
            );
        }

        let manifest = Self::new(
            verse_id,
            branch_id,
            header_roots,
            payload_roots,
            availability_boundary,
            statistics,
            sealed_statistics,
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

    fn statistics(operation_count: u64) -> SegmentStatistics {
        SegmentStatistics {
            min_hlc: Hlc::new(1_700_000_000_000, 0),
            max_hlc: Hlc::new(1_700_000_060_000, 3),
            petals: BTreeSet::from([identifier(0x21), identifier(0x22)]),
            operation_count,
        }
    }

    fn empty_statistics() -> SegmentStatistics {
        SegmentStatistics {
            min_hlc: Hlc::new(0, 0),
            max_hlc: Hlc::new(0, 0),
            petals: BTreeSet::new(),
            operation_count: 0,
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
            statistics(42),
            BTreeMap::from([(
                LaneKey::Payload(first),
                SealedStatisticsRef {
                    artifact_id: digest(0xD1),
                    stored_length: 900,
                },
            )]),
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
            statistics(1),
            BTreeMap::new(),
        );
        assert_eq!(uncovered, Err(SegmentError::ManifestBoundaryLaneMismatch));

        let stray_boundary = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::from([(LaneKey::Header { verse_id: verse }, boundary(0xA2))]),
            statistics(1),
            BTreeMap::new(),
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
            statistics(1),
            BTreeMap::new(),
        );
        assert_eq!(
            foreign,
            Err(SegmentError::ManifestForeignPayloadTopic {
                petal_id: identifier(0x21)
            })
        );
    }

    /// Gate G4: the tiering itself. Coarse range/petals/count are legible to any verse member
    /// so segments can be skipped; anything finer is only reachable by content address through
    /// an artifact sealed under the lane's own scope key.
    #[test]
    fn manifest_carries_coarse_clear_statistics_and_only_a_sealed_reference_for_fine_ones() {
        let manifest = manifest();
        let first = LaneKey::Payload(topic(0x21, 7));
        let second = LaneKey::Payload(topic(0x22, 7));

        assert_eq!(manifest.statistics().operation_count, 42);
        assert_eq!(
            manifest.statistics().min_hlc,
            Hlc::new(1_700_000_000_000, 0)
        );
        assert_eq!(
            manifest.statistics().max_hlc,
            Hlc::new(1_700_000_060_000, 3)
        );
        assert_eq!(
            manifest.statistics().petals,
            BTreeSet::from([identifier(0x21), identifier(0x22)])
        );

        // The fine-grained tier is a content address and a length -- never a value.
        assert_eq!(
            manifest.sealed_statistics_for(&first),
            Some(SealedStatisticsRef {
                artifact_id: digest(0xD1),
                stored_length: 900,
            })
        );
        assert_eq!(manifest.sealed_statistics_for(&second), None);

        let bytes = manifest.encode_canonical().expect("bytes");
        assert_eq!(
            SegmentManifestBody::decode_canonical(&bytes).expect("round trip"),
            manifest
        );
    }

    #[test]
    fn manifest_refuses_inconsistent_or_foreign_statistics() {
        let verse = identifier(0x11);
        let header_lane = LaneKey::Header { verse_id: verse };

        let inverted = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            roots(&[(0xA1, 100)]),
            BTreeMap::new(),
            BTreeMap::from([(header_lane, boundary(0xA1))]),
            SegmentStatistics {
                min_hlc: Hlc::new(1_000, 0),
                max_hlc: Hlc::new(999, 0),
                petals: BTreeSet::new(),
                operation_count: 1,
            },
            BTreeMap::new(),
        );
        assert_eq!(inverted, Err(SegmentError::ManifestStatisticsInconsistent));

        // Roots but a zero count: a manifest may not claim to index nothing while serving roots.
        let zero_count = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            roots(&[(0xA1, 100)]),
            BTreeMap::new(),
            BTreeMap::from([(header_lane, boundary(0xA1))]),
            empty_statistics(),
            BTreeMap::new(),
        );
        assert_eq!(
            zero_count,
            Err(SegmentError::ManifestStatisticsInconsistent)
        );

        // ...and the converse: no lanes but a non-zero count.
        let empty_with_count = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            statistics(7),
            BTreeMap::new(),
        );
        assert_eq!(
            empty_with_count,
            Err(SegmentError::ManifestStatisticsInconsistent)
        );

        // A wholly empty manifest with a zero count is coherent.
        SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            empty_statistics(),
            BTreeMap::new(),
        )
        .expect("an empty manifest indexing zero operations is coherent");

        // A sealed statistics artifact for a lane this manifest does not claim is refused: it
        // would advertise coverage the manifest cannot serve.
        let foreign_lane = SegmentManifestBody::new(
            verse,
            identifier(0xB1),
            roots(&[(0xA1, 100)]),
            BTreeMap::new(),
            BTreeMap::from([(header_lane, boundary(0xA1))]),
            statistics(1),
            BTreeMap::from([(
                LaneKey::Payload(topic(0x21, 7)),
                SealedStatisticsRef {
                    artifact_id: digest(0xD1),
                    stored_length: 900,
                },
            )]),
        );
        assert_eq!(
            foreign_lane,
            Err(SegmentError::ManifestStatisticsForeignLane)
        );
    }

    #[test]
    fn statistics_refuse_a_non_ascending_petal_set() {
        let descending = CborValue::Map(vec![
            (CborValue::Uint(0), Hlc::new(1, 0).to_cbor()),
            (CborValue::Uint(1), Hlc::new(2, 0).to_cbor()),
            (
                CborValue::Uint(2),
                CborValue::Array(vec![
                    CborValue::Bytes(identifier(0x22).0.to_vec()),
                    CborValue::Bytes(identifier(0x21).0.to_vec()),
                ]),
            ),
            (CborValue::Uint(3), CborValue::Uint(2)),
        ]);
        assert_eq!(
            SegmentStatistics::from_cbor(&descending),
            Err(SegmentError::ManifestStatisticsPetalsNotAscending)
        );

        let duplicated = CborValue::Map(vec![
            (CborValue::Uint(0), Hlc::new(1, 0).to_cbor()),
            (CborValue::Uint(1), Hlc::new(2, 0).to_cbor()),
            (
                CborValue::Uint(2),
                CborValue::Array(vec![
                    CborValue::Bytes(identifier(0x21).0.to_vec()),
                    CborValue::Bytes(identifier(0x21).0.to_vec()),
                ]),
            ),
            (CborValue::Uint(3), CborValue::Uint(2)),
        ]);
        assert_eq!(
            SegmentStatistics::from_cbor(&duplicated),
            Err(SegmentError::ManifestStatisticsPetalsNotAscending)
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
