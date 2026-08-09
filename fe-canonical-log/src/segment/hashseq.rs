//! HashSeq nodes and the reachability walk (SPEC-6 §4.1).
//!
//! A HashSeq is delivery structure. It proves which immutable artifacts a manifest claims to
//! cover; SPEC-1 parent links, never node order, establish operation causality.

use std::collections::{BTreeMap, BTreeSet};

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Identifier32, Scope};

use super::artifact::{verify_artifact_id, LaneClass};
use super::payload_shard::PayloadTopicScope;
use super::store::{InMemorySealedArtifactStore, SealedArtifactStore};
use super::{
    array_at, assert_canonical_bytes, hash32_at, require_uint_keys, uint_at, value_at, SegmentError,
};

/// The exact scope binding of one delivery lane (§2.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaneKey {
    /// The verse-wide header lane.
    Header {
        /// Verse the header lane covers.
        verse_id: Identifier32,
    },
    /// One petal-affine payload-topic lane.
    Payload(PayloadTopicScope),
}

const LANE_KEY_CONTEXT: &str = "lane key";

impl LaneKey {
    /// Which lane class the artifacts in this lane belong to.
    pub fn lane_class(&self) -> LaneClass {
        match self {
            Self::Header { .. } => LaneClass::HeaderSegment,
            Self::Payload(_) => LaneClass::PayloadShard,
        }
    }

    /// The verse this lane belongs to.
    pub fn verse_id(&self) -> Identifier32 {
        match self {
            Self::Header { verse_id } => *verse_id,
            Self::Payload(topic) => topic.verse_id,
        }
    }

    /// The SPEC-1 scope the lane's SPEC-3 §6.1 topic label blinds.
    ///
    /// Verse-wide for the header lane, petal-affine for a payload lane, matching §6.1 rule 4;
    /// this is the only place a delivery lane becomes a capability scope.
    pub fn topic_scope(&self) -> Scope {
        match self {
            Self::Header { verse_id } => Scope::verse_wide(*verse_id),
            Self::Payload(topic) => topic.scope(),
        }
    }

    /// The scope epoch in force, or `None` for the header lane, whose epoch is verse-wide.
    pub fn scope_epoch(&self) -> Option<u64> {
        match self {
            Self::Header { .. } => None,
            Self::Payload(topic) => Some(topic.scope_epoch),
        }
    }

    /// Encodes the lane key as a two-element array so it can serve as a canonical map key.
    pub fn to_cbor(&self) -> CborValue {
        match self {
            Self::Header { verse_id } => CborValue::Array(vec![
                CborValue::Uint(0),
                CborValue::Bytes(verse_id.0.to_vec()),
            ]),
            Self::Payload(topic) => CborValue::Array(vec![CborValue::Uint(1), topic.to_cbor()]),
        }
    }

    /// Decodes the lane key.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        let entries = value.as_array().ok_or(SegmentError::ExpectedArray {
            context: LANE_KEY_CONTEXT,
        })?;
        let [discriminant, binding] = entries else {
            return Err(SegmentError::ExpectedArray {
                context: LANE_KEY_CONTEXT,
            });
        };
        match discriminant.as_uint() {
            Some(0) => {
                let raw = binding.as_bytes().ok_or(SegmentError::FieldTypeMismatch {
                    context: LANE_KEY_CONTEXT,
                    key: 0,
                })?;
                Ok(Self::Header {
                    verse_id: Identifier32(<[u8; 32]>::try_from(raw).map_err(|_| {
                        SegmentError::WrongByteLength {
                            context: LANE_KEY_CONTEXT,
                            key: 0,
                            expected: 32,
                            actual: raw.len(),
                        }
                    })?),
                })
            }
            Some(1) => Ok(Self::Payload(PayloadTopicScope::from_cbor(binding)?)),
            _ => Err(SegmentError::UnknownField {
                context: LANE_KEY_CONTEXT,
            }),
        }
    }
}

/// One `(artifact_id, stored_length)` entry in a HashSeq node (§4.1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HashSeqEntry {
    /// The immutable artifact this entry points at.
    pub artifact_id: Hash32,
    /// Its exact stored byte length.
    pub stored_length: u64,
}

const ENTRY_CONTEXT: &str = "hashseq entry";
const ENTRY_KEYS: &[u64] = &[0, 1];

impl HashSeqEntry {
    /// Encodes the entry map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.artifact_id.0.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.stored_length)),
        ])
    }

    /// Decodes the entry map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, ENTRY_KEYS, ENTRY_CONTEXT)?;
        Ok(Self {
            artifact_id: hash32_at(value, 0, ENTRY_CONTEXT)?,
            stored_length: uint_at(value, 1, ENTRY_CONTEXT)?,
        })
    }
}

/// One immutable link in a delivery sequence (§4.1.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashSeqNode {
    lane: LaneKey,
    predecessor_id: Option<Hash32>,
    entries: Vec<HashSeqEntry>,
}

const NODE_CONTEXT: &str = "hashseq node body";
const NODE_KEYS: &[u64] = &[0, 1, 2, 3];

impl HashSeqNode {
    /// Builds a node, refusing an empty entry list or a repeated artifact ID (§4.1.1, §4.1.2).
    ///
    /// Entry order is sealing order and is preserved verbatim; it is never causal order.
    pub fn new(
        lane: LaneKey,
        predecessor_id: Option<Hash32>,
        entries: Vec<HashSeqEntry>,
    ) -> Result<Self, SegmentError> {
        if entries.is_empty() {
            return Err(SegmentError::EmptyHashSeqNode);
        }
        let mut seen = BTreeSet::new();
        for entry in &entries {
            if !seen.insert(entry.artifact_id) {
                return Err(SegmentError::DuplicateHashSeqEntry {
                    artifact_id: entry.artifact_id,
                });
            }
        }
        Ok(Self {
            lane,
            predecessor_id,
            entries,
        })
    }

    /// The lane and exact scope binding this node is fixed to.
    pub fn lane(&self) -> LaneKey {
        self.lane
    }

    /// The previous node in the sequence, or `None` at the sequence origin.
    pub fn predecessor_id(&self) -> Option<Hash32> {
        self.predecessor_id
    }

    /// Entries in sealing order.
    pub fn entries(&self) -> &[HashSeqEntry] {
        &self.entries
    }

    /// Encodes the inner body.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::HashSeqNode.to_wire()),
            ),
            (CborValue::Uint(1), self.lane.to_cbor()),
            (
                CborValue::Uint(2),
                self.predecessor_id
                    .map_or(CborValue::Null, |id| CborValue::Bytes(id.0.to_vec())),
            ),
            (
                CborValue::Uint(3),
                CborValue::Array(self.entries.iter().map(HashSeqEntry::to_cbor).collect()),
            ),
        ])
    }

    /// The exact plaintext bytes that get sealed.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes a decrypted node body, refusing a lane mismatch and a non-canonical re-encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        require_uint_keys(&value, NODE_KEYS, NODE_CONTEXT)?;
        if uint_at(&value, 0, NODE_CONTEXT)? != LaneClass::HashSeqNode.to_wire() {
            return Err(SegmentError::LaneBodyMismatch);
        }
        let lane = LaneKey::from_cbor(value_at(&value, 1, NODE_CONTEXT)?)?;
        let predecessor_slot = value_at(&value, 2, NODE_CONTEXT)?;
        let predecessor_id = if predecessor_slot.is_null() {
            None
        } else {
            Some(hash32_at(&value, 2, NODE_CONTEXT)?)
        };
        let mut entries = Vec::new();
        for entry in array_at(&value, 3, NODE_CONTEXT)? {
            entries.push(HashSeqEntry::from_cbor(entry)?);
        }
        let node = Self::new(lane, predecessor_id, entries)?;
        assert_canonical_bytes(&value, bytes, NODE_CONTEXT)?;
        Ok(node)
    }
}

/// Fetches sealed artifact bytes by ID; wave 3 backs this with the real store.
pub trait ArtifactLookup {
    /// The exact stored bytes, or `None` when the artifact is not available here.
    fn fetch(&self, artifact_id: Hash32) -> Option<Vec<u8>>;

    /// Fetches and re-hashes before returning, per §5.1; never override to skip the re-hash.
    fn fetch_verified(&self, artifact_id: Hash32) -> Result<Vec<u8>, SegmentError> {
        let bytes = self
            .fetch(artifact_id)
            .ok_or(SegmentError::ArtifactUnavailable { artifact_id })?;
        verify_artifact_id(&bytes, artifact_id)?;
        Ok(bytes)
    }
}

impl ArtifactLookup for InMemorySealedArtifactStore {
    fn fetch(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.get(artifact_id)
    }
}

/// Opens a sealed HashSeq node: re-hash, decrypt under the authorized scope key, decode.
///
/// Wave 3 supplies the AEAD; this crate only states the ordering the implementation owes.
pub trait HashSeqNodeSource {
    /// Yields the decoded node, or the reason it could not be opened.
    fn open_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError>;
}

/// What one root-to-boundary walk established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashSeqWalk {
    /// Nodes traversed, newest first, ending at the boundary node.
    pub nodes_visited: Vec<Hash32>,
    /// Artifacts the path covers, deduplicated by ID across nodes (§4.1.2).
    pub entries: BTreeMap<Hash32, u64>,
}

/// Walks one root to the manifest's declared boundary, failing closed (§4.1.3, §4.1.4).
///
/// Refuses a cycle, a lane or scope-binding change, a missing predecessor, and a path that
/// ends before the boundary. `maximum_nodes` is a caller bound under D-CL24; there is no
/// default, because the right ceiling depends on the deployment's sequence length.
pub fn walk_to_boundary(
    root: Hash32,
    lane: &LaneKey,
    boundary: Hash32,
    nodes: &impl HashSeqNodeSource,
    maximum_nodes: usize,
) -> Result<HashSeqWalk, SegmentError> {
    let mut nodes_visited = Vec::new();
    let mut seen = BTreeSet::new();
    let mut entries: BTreeMap<Hash32, u64> = BTreeMap::new();
    let mut current = Some(root);

    while let Some(node_id) = current {
        if nodes_visited.len() == maximum_nodes {
            return Err(SegmentError::TraversalBoundExceeded {
                maximum: maximum_nodes,
            });
        }
        if !seen.insert(node_id) {
            return Err(SegmentError::HashSeqCycle { node_id });
        }
        let node = match nodes.open_node(node_id) {
            Ok(node) => node,
            Err(SegmentError::ArtifactUnavailable { .. }) => {
                return Err(SegmentError::MissingHashSeqPredecessor { node_id })
            }
            Err(other) => return Err(other),
        };
        if node.lane() != *lane {
            return Err(SegmentError::HashSeqBindingMismatch { node_id });
        }
        for entry in node.entries() {
            match entries.get(&entry.artifact_id) {
                Some(existing) if *existing != entry.stored_length => {
                    return Err(SegmentError::ConflictingStoredLength {
                        artifact_id: entry.artifact_id,
                    })
                }
                _ => {
                    entries.insert(entry.artifact_id, entry.stored_length);
                }
            }
        }
        nodes_visited.push(node_id);
        if node_id == boundary {
            return Ok(HashSeqWalk {
                nodes_visited,
                entries,
            });
        }
        current = node.predecessor_id();
    }

    Err(SegmentError::BoundaryUnreachable { boundary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;

    use crate::segment::test_fixtures::{digest, identifier};

    fn header_lane() -> LaneKey {
        LaneKey::Header {
            verse_id: identifier(0x11),
        }
    }

    fn payload_lane(epoch: u64) -> LaneKey {
        LaneKey::Payload(PayloadTopicScope {
            verse_id: identifier(0x11),
            petal_id: identifier(0x21),
            scope_epoch: epoch,
            key_id: identifier(0x41),
        })
    }

    fn entry(filler: u8, stored_length: u64) -> HashSeqEntry {
        HashSeqEntry {
            artifact_id: digest(filler),
            stored_length,
        }
    }

    #[derive(Default)]
    struct FakeNodes(Map<Hash32, HashSeqNode>);

    impl FakeNodes {
        fn with(mut self, id: Hash32, node: HashSeqNode) -> Self {
            self.0.insert(id, node);
            self
        }
    }

    impl HashSeqNodeSource for FakeNodes {
        fn open_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError> {
            self.0
                .get(&artifact_id)
                .cloned()
                .ok_or(SegmentError::ArtifactUnavailable { artifact_id })
        }
    }

    #[test]
    fn hashseq_reachability_requires_complete_predecessor_walk() {
        let lane = header_lane();
        let (newest, middle, oldest) = (digest(0xA1), digest(0xA2), digest(0xA3));

        let complete = FakeNodes::default()
            .with(
                newest,
                HashSeqNode::new(lane, Some(middle), vec![entry(1, 10), entry(2, 20)])
                    .expect("node"),
            )
            .with(
                middle,
                HashSeqNode::new(lane, Some(oldest), vec![entry(2, 20), entry(3, 30)])
                    .expect("node"),
            )
            .with(
                oldest,
                HashSeqNode::new(lane, None, vec![entry(4, 40)]).expect("node"),
            );

        let walk = walk_to_boundary(newest, &lane, oldest, &complete, 16).expect("complete walk");
        assert_eq!(walk.nodes_visited, vec![newest, middle, oldest]);
        assert_eq!(walk.entries.len(), 4);
        assert_eq!(walk.entries.get(&digest(2)), Some(&20));

        let missing_predecessor = FakeNodes::default().with(
            newest,
            HashSeqNode::new(lane, Some(middle), vec![entry(1, 10)]).expect("node"),
        );
        assert_eq!(
            walk_to_boundary(newest, &lane, oldest, &missing_predecessor, 16),
            Err(SegmentError::MissingHashSeqPredecessor { node_id: middle })
        );

        let cyclic = FakeNodes::default()
            .with(
                newest,
                HashSeqNode::new(lane, Some(middle), vec![entry(1, 10)]).expect("node"),
            )
            .with(
                middle,
                HashSeqNode::new(lane, Some(newest), vec![entry(2, 20)]).expect("node"),
            );
        assert_eq!(
            walk_to_boundary(newest, &lane, oldest, &cyclic, 16),
            Err(SegmentError::HashSeqCycle { node_id: newest })
        );

        let cross_lane = FakeNodes::default()
            .with(
                newest,
                HashSeqNode::new(lane, Some(middle), vec![entry(1, 10)]).expect("node"),
            )
            .with(
                middle,
                HashSeqNode::new(payload_lane(7), None, vec![entry(2, 20)]).expect("node"),
            );
        assert_eq!(
            walk_to_boundary(newest, &lane, oldest, &cross_lane, 16),
            Err(SegmentError::HashSeqBindingMismatch { node_id: middle })
        );

        let cross_scope = FakeNodes::default()
            .with(
                newest,
                HashSeqNode::new(payload_lane(7), Some(middle), vec![entry(1, 10)]).expect("node"),
            )
            .with(
                middle,
                HashSeqNode::new(payload_lane(8), None, vec![entry(2, 20)]).expect("node"),
            );
        assert_eq!(
            walk_to_boundary(newest, &payload_lane(7), oldest, &cross_scope, 16),
            Err(SegmentError::HashSeqBindingMismatch { node_id: middle })
        );

        let truncated = FakeNodes::default().with(
            newest,
            HashSeqNode::new(lane, None, vec![entry(1, 10)]).expect("node"),
        );
        assert_eq!(
            walk_to_boundary(newest, &lane, oldest, &truncated, 16),
            Err(SegmentError::BoundaryUnreachable { boundary: oldest })
        );

        assert_eq!(
            walk_to_boundary(newest, &lane, oldest, &complete, 2),
            Err(SegmentError::TraversalBoundExceeded { maximum: 2 })
        );
    }

    #[test]
    fn node_body_round_trips_and_refuses_empty_or_repeated_entries() {
        for lane in [header_lane(), payload_lane(7)] {
            let node = HashSeqNode::new(lane, Some(digest(0xA2)), vec![entry(1, 10), entry(2, 20)])
                .expect("node");
            let bytes = node.encode_canonical().expect("bytes");
            assert_eq!(
                HashSeqNode::decode_canonical(&bytes).expect("round trip"),
                node
            );

            let origin = HashSeqNode::new(lane, None, vec![entry(1, 10)]).expect("node");
            let bytes = origin.encode_canonical().expect("bytes");
            assert_eq!(
                HashSeqNode::decode_canonical(&bytes).expect("round trip"),
                origin
            );
        }

        assert_eq!(
            HashSeqNode::new(header_lane(), None, Vec::new()),
            Err(SegmentError::EmptyHashSeqNode)
        );
        assert_eq!(
            HashSeqNode::new(header_lane(), None, vec![entry(1, 10), entry(1, 11)]),
            Err(SegmentError::DuplicateHashSeqEntry {
                artifact_id: digest(1)
            })
        );
    }

    /// A lookup that hands back whatever it was told, so the re-hash guard is what fails.
    struct SubstitutingLookup {
        answers: Map<Hash32, Vec<u8>>,
    }

    impl ArtifactLookup for SubstitutingLookup {
        fn fetch(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
            self.answers.get(&artifact_id).cloned()
        }
    }

    #[test]
    fn artifact_lookup_rehashes_before_returning() {
        let honest = b"sealed-node".to_vec();
        let artifact_id = Hash32::of(&honest);
        let lookup = SubstitutingLookup {
            answers: Map::from([
                (artifact_id, honest.clone()),
                (digest(0xEE), b"substituted".to_vec()),
            ]),
        };

        assert_eq!(lookup.fetch_verified(artifact_id), Ok(honest));
        assert_eq!(
            lookup.fetch_verified(digest(0xEE)),
            Err(SegmentError::ArtifactIdMismatch {
                claimed: digest(0xEE),
                computed: Hash32::of(b"substituted"),
            })
        );
        assert_eq!(
            lookup.fetch_verified(digest(0xFF)),
            Err(SegmentError::ArtifactUnavailable {
                artifact_id: digest(0xFF)
            })
        );
    }
}
