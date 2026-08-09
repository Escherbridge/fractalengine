//! The five-step checkpoint reachability proof (SPEC-6 §4.2).
//!
//! A Manager+ signature makes a claim; it never makes history valid. Everything the proof
//! trusts is re-hashed, decrypted under an authorized scope key, and canonically validated
//! first, and every shortfall becomes an explicit `Unresolved` rather than a silent skip.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use crate::envelope::{Hash32, Identifier32};
use crate::frontier::SortedFrontier;

use super::hashseq::{walk_to_boundary, ArtifactLookup, HashSeqNode, HashSeqNodeSource, LaneKey};
use super::header_lane::{AdmittedEnvelope, EnvelopeView, HeaderSegmentBody};
use super::manifest::{RootSet, SegmentManifestBody};
use super::payload_shard::{PayloadShardBody, PayloadShardRecord, PayloadTopicScope};
use super::SegmentError;

/// Headers collected from the manifest's header lane, indexed by `op_id`.
type HeaderIndex = BTreeMap<Hash32, AdmittedEnvelope>;

/// Payload records from one topic's shards, indexed by `op_id`.
type RecordIndex = BTreeMap<Hash32, PayloadShardRecord>;

/// Artifacts a lane's HashSeq walk covers, mapped to their exact stored byte lengths.
type ArtifactLengths = BTreeMap<Hash32, u64>;

/// The signed checkpoint under proof; the branch slice owns the concrete artifact.
pub trait CheckpointView {
    /// Verse the checkpoint covers.
    fn verse_id(&self) -> Identifier32;

    /// Branch the checkpoint covers.
    fn branch_id(&self) -> Identifier32;

    /// The exact manifest the checkpoint binds (§4.2.1).
    fn segment_manifest_id(&self) -> Hash32;

    /// The selected sorted frontier the projection was taken at.
    fn selected_frontier(&self) -> &SortedFrontier;

    /// The D-CL19 frontier commitment the signed claim itself carries (§4.2.1).
    ///
    /// A required method with no default on purpose: a default returning
    /// `selected_frontier().commitment()` would make the [`run_proof`] cross-check vacuous, and
    /// the whole point is that the heads the proof walks must be the heads the signature covers.
    /// `compose::SignedCheckpointProofView` is the concrete SPEC-5 implementation.
    fn frontier_commitment(&self) -> Hash32;

    /// The declared replay base, or `None` when the closure runs to genesis.
    fn replay_base(&self) -> Option<Hash32>;

    /// Verifies the Manager+ signature over the signed checkpoint bytes (§4.2.2).
    fn verify_manager_signature(&self) -> Result<(), SegmentError>;
}

/// Opens sealed artifacts under the authorized scope key; wave 3 supplies the AEAD.
///
/// Every implementation MUST re-hash before decrypting. This module re-hashes independently
/// through [`ArtifactLookup::fetch_verified`] so the guarantee does not rest on that promise.
pub trait ProofEnvironment {
    /// Opens a sealed segment manifest.
    fn open_manifest(&self, artifact_id: Hash32) -> Result<SegmentManifestBody, SegmentError>;

    /// Opens a sealed HashSeq node.
    fn open_hashseq_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError>;

    /// Opens a sealed header segment.
    fn open_header_segment(&self, artifact_id: Hash32) -> Result<HeaderSegmentBody, SegmentError>;

    /// Opens a sealed payload shard.
    fn open_payload_shard(&self, artifact_id: Hash32) -> Result<PayloadShardBody, SegmentError>;
}

/// Reports which lanes the verifier currently holds decrypt authority for (§4.2.4).
pub trait ScopeKeyResolver {
    /// Whether the current authorized scope key for this lane is available.
    fn has_decrypt_authority(&self, lane: &LaneKey) -> bool;
}

/// Supplies parents for operations already admitted locally, outside the manifest's coverage.
pub trait ParentWalk {
    /// Parents of a locally admitted operation, or `None` when it is unknown here.
    fn parents_of(&self, op_id: Hash32) -> Option<Vec<Hash32>>;

    /// Whether the operation is a parentless branch genesis.
    fn is_genesis(&self, op_id: Hash32) -> bool;
}

/// The SPEC-1/SPEC-3 payload admission the projection depends on (§4.2.3 step 5).
pub trait PayloadAdmission {
    /// Whether the projection actually needs this operation's payload.
    fn projection_needs_payload(&self, op_id: Hash32) -> bool;

    /// Runs the SPEC-1 §5.2 AAD and SPEC-3 authorization checks over a matched record.
    fn admit_payload(&self, op_id: Hash32, record: &PayloadShardRecord)
        -> Result<(), SegmentError>;
}

/// Traversal ceilings for one proof.
///
/// Both are caller policy under D-CL24 and have no default: the right ceiling depends on the
/// deployment's sequence length and branch depth, not on this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofBounds {
    /// Largest number of HashSeq nodes one root-to-boundary walk may traverse.
    pub maximum_hashseq_nodes: usize,
    /// Largest number of operations the selected-frontier parent closure may contain.
    pub maximum_parent_walk_operations: usize,
}

impl ProofBounds {
    /// Builds bounds from the caller's two explicit numbers.
    pub const fn new(maximum_hashseq_nodes: usize, maximum_parent_walk_operations: usize) -> Self {
        Self {
            maximum_hashseq_nodes,
            maximum_parent_walk_operations,
        }
    }
}

/// Everything one proof needs, so the entry point stays a single argument.
pub struct ProofInputs<'a, C, L, E, K, P, A> {
    /// The signed checkpoint under proof.
    pub checkpoint: &'a C,
    /// Raw sealed-artifact access, used for the independent re-hash of everything fetched.
    pub lookup: &'a L,
    /// Decryption and canonical decoding of sealed bodies.
    pub environment: &'a E,
    /// Which lanes the verifier may decrypt.
    pub scope_keys: &'a K,
    /// Parents for operations already admitted locally.
    pub parents: &'a P,
    /// Payload admission for the projection.
    pub admission: &'a A,
    /// Traversal ceilings.
    pub bounds: ProofBounds,
}

/// What the proof established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointProofOutcome {
    /// The full closure verified, payloads included.
    Verified {
        /// Operations in the selected-frontier closure whose headers were verified.
        headers_verified: usize,
        /// Payloads matched, re-hashed, and admitted.
        payloads_verified: usize,
    },
    /// The verse-wide header closure verified, but a required petal payload was unreadable.
    ///
    /// Never a verified projection root and never complete materialization (§4.2.4).
    HeaderReachableOnly {
        /// Operations in the closure whose headers were verified.
        headers_verified: usize,
        /// Lanes the verifier holds no current decrypt authority for.
        unauthorized_lanes: Vec<LaneKey>,
    },
    /// A head could not be resolved; the checkpoint is not an accelerator (§4.2.5).
    Unresolved {
        /// Why resolution stopped.
        reason: SegmentError,
    },
}

/// Re-hashes every artifact before it is decrypted, per §4.2.3 step 1.
struct RehashingNodeSource<'a, L, E> {
    lookup: &'a L,
    environment: &'a E,
}

impl<L: ArtifactLookup, E: ProofEnvironment> HashSeqNodeSource for RehashingNodeSource<'_, L, E> {
    fn open_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError> {
        self.lookup.fetch_verified(artifact_id)?;
        self.environment.open_hashseq_node(artifact_id)
    }
}

/// Runs the §4.2.3 five steps in order and never walks outside the declared parent closure.
pub fn verify_checkpoint<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
) -> CheckpointProofOutcome
where
    C: CheckpointView,
    L: ArtifactLookup,
    E: ProofEnvironment,
    K: ScopeKeyResolver,
    P: ParentWalk,
    A: PayloadAdmission,
{
    match run_proof(inputs) {
        Ok(outcome) => outcome,
        Err(reason) => CheckpointProofOutcome::Unresolved { reason },
    }
}

fn run_proof<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
) -> Result<CheckpointProofOutcome, SegmentError>
where
    C: CheckpointView,
    L: ArtifactLookup,
    E: ProofEnvironment,
    K: ScopeKeyResolver,
    P: ParentWalk,
    A: PayloadAdmission,
{
    let checkpoint = inputs.checkpoint;
    checkpoint.verify_manager_signature()?;

    // The signature covers a frontier COMMITMENT; the walk needs actual heads. Recomputing the
    // D-CL19 commitment over the heads about to be walked is what stops a caller pairing a valid
    // signature with a frontier it never covered.
    let computed = inputs.checkpoint.selected_frontier().commitment();
    let claimed = checkpoint.frontier_commitment();
    if computed != claimed {
        return Err(SegmentError::FrontierCommitmentMismatch { claimed, computed });
    }

    let manifest_id = checkpoint.segment_manifest_id();
    inputs.lookup.fetch_verified(manifest_id)?;

    let header_lane = LaneKey::Header {
        verse_id: checkpoint.verse_id(),
    };
    if !inputs.scope_keys.has_decrypt_authority(&header_lane) {
        return Err(SegmentError::ScopeKeyUnavailable);
    }
    let manifest = inputs.environment.open_manifest(manifest_id)?;
    manifest.validate()?;
    if manifest.verse_id() != checkpoint.verse_id()
        || manifest.branch_id() != checkpoint.branch_id()
    {
        return Err(SegmentError::CheckpointBindingMismatch);
    }

    let headers = collect_headers(inputs, &manifest, &header_lane)?;
    let reachable = walk_selected_closure(inputs, &headers)?;
    resolve_payloads(inputs, &manifest, &headers, &reachable)
}

/// Step 3: walks every declared header root to the boundary and collects exact headers.
fn collect_headers<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
    manifest: &SegmentManifestBody,
    header_lane: &LaneKey,
) -> Result<HeaderIndex, SegmentError>
where
    L: ArtifactLookup,
    E: ProofEnvironment,
{
    let mut headers: HeaderIndex = BTreeMap::new();
    let Some(roots) = manifest.roots_for(header_lane) else {
        return Ok(headers);
    };
    let boundary = manifest
        .boundary_for(header_lane)
        .ok_or(SegmentError::ManifestBoundaryLaneMismatch)?;
    let segments = walk_lane(inputs, header_lane, roots, boundary.oldest_required_node)?;

    for (artifact_id, declared_length) in segments {
        let bytes = inputs.lookup.fetch_verified(artifact_id)?;
        if bytes.len() as u64 != declared_length {
            return Err(SegmentError::StoredLengthMismatch {
                declared: declared_length,
                received: bytes.len() as u64,
            });
        }
        let segment = inputs.environment.open_header_segment(artifact_id)?;
        if segment.verse_id() != header_lane.verse_id() {
            return Err(SegmentError::ForeignLaneArtifact { artifact_id });
        }
        for record in segment.records() {
            let op_id = record.op_id();
            if let Some(existing) = headers.get(&op_id) {
                if existing.canonical_bytes() != record.canonical_bytes() {
                    return Err(SegmentError::ConflictingDuplicateOperationId { op_id });
                }
                continue;
            }
            headers.insert(op_id, record.clone());
        }
    }

    assert_no_equivocation(&headers)?;
    Ok(headers)
}

/// Refuses to materialize either candidate when one equivocation key names two operations.
fn assert_no_equivocation(headers: &HeaderIndex) -> Result<(), SegmentError> {
    let mut slots = BTreeMap::new();
    for (op_id, header) in headers {
        let equivocation_key = header.equivocation_key();
        if let Some(first_op_id) = slots.insert(equivocation_key, *op_id) {
            return Err(SegmentError::AuthorEquivocation {
                equivocation_key,
                first_op_id,
                second_op_id: *op_id,
            });
        }
    }
    Ok(())
}

/// Walks every root of one lane to its boundary, merging the artifacts they cover.
fn walk_lane<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
    lane: &LaneKey,
    roots: &RootSet,
    boundary: Hash32,
) -> Result<ArtifactLengths, SegmentError>
where
    L: ArtifactLookup,
    E: ProofEnvironment,
{
    let source = RehashingNodeSource {
        lookup: inputs.lookup,
        environment: inputs.environment,
    };
    let mut covered: ArtifactLengths = BTreeMap::new();
    for (root, declared_length) in roots {
        let bytes = inputs.lookup.fetch_verified(*root)?;
        if bytes.len() as u64 != *declared_length {
            return Err(SegmentError::StoredLengthMismatch {
                declared: *declared_length,
                received: bytes.len() as u64,
            });
        }
        let walk = walk_to_boundary(
            *root,
            lane,
            boundary,
            &source,
            inputs.bounds.maximum_hashseq_nodes,
        )?;
        for (artifact_id, stored_length) in walk.entries {
            match covered.insert(artifact_id, stored_length) {
                Some(existing) if existing != stored_length => {
                    return Err(SegmentError::ConflictingStoredLength { artifact_id })
                }
                _ => {}
            }
        }
    }
    Ok(covered)
}

/// Step 4: walks SPEC-1 parents from every frontier member to genesis or the replay base.
///
/// Only the declared closure is walked. An unrelated concurrent operation is not a missing
/// ancestor, and every selected multi-parent merge makes all of its parents required (§4.2.6).
fn walk_selected_closure<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
    headers: &HeaderIndex,
) -> Result<BTreeSet<Hash32>, SegmentError>
where
    C: CheckpointView,
    P: ParentWalk,
{
    let replay_base = inputs.checkpoint.replay_base();
    let mut reachable: BTreeSet<Hash32> = BTreeSet::new();
    let mut pending: Vec<Hash32> = inputs.checkpoint.selected_frontier().as_slice().to_vec();

    while let Some(op_id) = pending.pop() {
        if !reachable.insert(op_id) {
            continue;
        }
        if reachable.len() > inputs.bounds.maximum_parent_walk_operations {
            return Err(SegmentError::TraversalBoundExceeded {
                maximum: inputs.bounds.maximum_parent_walk_operations,
            });
        }
        if Some(op_id) == replay_base {
            continue;
        }
        let parents = match headers.get(&op_id) {
            Some(header) => header.parents().to_vec(),
            None if inputs.parents.is_genesis(op_id) => Vec::new(),
            None => inputs
                .parents
                .parents_of(op_id)
                .ok_or(SegmentError::MissingRequiredHeader { op_id })?,
        };
        pending.extend(parents);
    }
    Ok(reachable)
}

/// Step 5: matches, re-hashes, and admits the payload of every payload-bearing reachable header.
fn resolve_payloads<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
    manifest: &SegmentManifestBody,
    headers: &HeaderIndex,
    reachable: &BTreeSet<Hash32>,
) -> Result<CheckpointProofOutcome, SegmentError>
where
    L: ArtifactLookup,
    E: ProofEnvironment,
    K: ScopeKeyResolver,
    A: PayloadAdmission,
{
    let mut unauthorized_lanes: Vec<LaneKey> = Vec::new();
    let mut opened_topics: BTreeMap<PayloadTopicScope, RecordIndex> = BTreeMap::new();
    let mut payloads_verified = 0usize;
    let mut headers_verified = 0usize;

    for op_id in reachable {
        let Some(header) = headers.get(op_id) else {
            continue;
        };
        headers_verified += 1;
        if !header.has_payload() || !inputs.admission.projection_needs_payload(*op_id) {
            continue;
        }
        let scope = header.scope();
        let topic = manifest
            .payload_roots()
            .keys()
            .find(|topic| topic.admits(&scope))
            .copied()
            .ok_or(SegmentError::MissingRequiredPayload { op_id: *op_id })?;
        let lane = LaneKey::Payload(topic);
        if !inputs.scope_keys.has_decrypt_authority(&lane) {
            if !unauthorized_lanes.contains(&lane) {
                unauthorized_lanes.push(lane);
            }
            continue;
        }
        let records = match opened_topics.entry(topic) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(open_topic_records(inputs, manifest, &lane)?),
        };
        let record = records
            .get(op_id)
            .ok_or(SegmentError::MissingRequiredPayload { op_id: *op_id })?;
        record.verify_self()?;
        let payload = header.payload_ref();
        if payload.ciphertext_hash != record.ciphertext_hash
            || payload.ciphertext_length != record.ciphertext_length
        {
            return Err(SegmentError::PayloadRecordHeaderMismatch { op_id: *op_id });
        }
        inputs.admission.admit_payload(*op_id, record)?;
        payloads_verified += 1;
    }

    if !unauthorized_lanes.is_empty() {
        return Ok(CheckpointProofOutcome::HeaderReachableOnly {
            headers_verified,
            unauthorized_lanes,
        });
    }
    Ok(CheckpointProofOutcome::Verified {
        headers_verified,
        payloads_verified,
    })
}

/// Walks one payload lane to its boundary and indexes every record the shards carry.
fn open_topic_records<C, L, E, K, P, A>(
    inputs: &ProofInputs<'_, C, L, E, K, P, A>,
    manifest: &SegmentManifestBody,
    lane: &LaneKey,
) -> Result<RecordIndex, SegmentError>
where
    L: ArtifactLookup,
    E: ProofEnvironment,
{
    let roots = manifest
        .roots_for(lane)
        .ok_or(SegmentError::ManifestBoundaryLaneMismatch)?;
    let boundary = manifest
        .boundary_for(lane)
        .ok_or(SegmentError::ManifestBoundaryLaneMismatch)?;
    let shards = walk_lane(inputs, lane, roots, boundary.oldest_required_node)?;

    let mut records = BTreeMap::new();
    for (artifact_id, declared_length) in shards {
        let bytes = inputs.lookup.fetch_verified(artifact_id)?;
        if bytes.len() as u64 != declared_length {
            return Err(SegmentError::StoredLengthMismatch {
                declared: declared_length,
                received: bytes.len() as u64,
            });
        }
        let shard = inputs.environment.open_payload_shard(artifact_id)?;
        for record in shard.records() {
            if let Some(existing) = records.insert(record.op_id, record.clone()) {
                if existing != *record {
                    return Err(SegmentError::ConflictingDuplicateOperationId {
                        op_id: record.op_id,
                    });
                }
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::envelope::{Scope, NONCE_LENGTH, PRODUCTION_SUITE_ID};
    use crate::segment::artifact::{EncryptionDescriptor, LaneClass, SealedArtifact};
    use crate::segment::hashseq::HashSeqEntry;
    use crate::segment::manifest::BoundaryNode;
    use crate::segment::test_fixtures::{genesis_header, identifier, intent_header, HeaderFixture};

    #[derive(Default)]
    struct World {
        artifacts: BTreeMap<Hash32, Vec<u8>>,
        manifests: BTreeMap<Hash32, SegmentManifestBody>,
        nodes: BTreeMap<Hash32, HashSeqNode>,
        header_segments: BTreeMap<Hash32, HeaderSegmentBody>,
        shards: BTreeMap<Hash32, PayloadShardBody>,
    }

    impl World {
        fn publish(&mut self, lane: LaneClass, plaintext: &[u8]) -> (Hash32, u64) {
            let sealed = SealedArtifact::seal(
                lane,
                EncryptionDescriptor::new(
                    PRODUCTION_SUITE_ID,
                    identifier(0x41),
                    [plaintext.len() as u8; NONCE_LENGTH],
                ),
                plaintext.to_vec(),
            )
            .expect("sealed");
            let bytes = sealed.encode_canonical().expect("bytes");
            let artifact_id = sealed.artifact_id().expect("id");
            let length = bytes.len() as u64;
            self.artifacts.insert(artifact_id, bytes);
            (artifact_id, length)
        }

        fn add_header_segment(&mut self, body: HeaderSegmentBody) -> (Hash32, u64) {
            let published = self.publish(
                LaneClass::HeaderSegment,
                &body.encode_canonical().expect("bytes"),
            );
            self.header_segments.insert(published.0, body);
            published
        }

        fn add_shard(&mut self, body: PayloadShardBody) -> (Hash32, u64) {
            let published = self.publish(
                LaneClass::PayloadShard,
                &body.encode_canonical().expect("bytes"),
            );
            self.shards.insert(published.0, body);
            published
        }

        fn add_node(&mut self, node: HashSeqNode) -> (Hash32, u64) {
            let published = self.publish(
                LaneClass::HashSeqNode,
                &node.encode_canonical().expect("bytes"),
            );
            self.nodes.insert(published.0, node);
            published
        }

        fn add_manifest(&mut self, body: SegmentManifestBody) -> (Hash32, u64) {
            let published = self.publish(
                LaneClass::SegmentManifest,
                &body.encode_canonical().expect("bytes"),
            );
            self.manifests.insert(published.0, body);
            published
        }
    }

    impl ArtifactLookup for World {
        fn fetch(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
            self.artifacts.get(&artifact_id).cloned()
        }
    }

    impl ProofEnvironment for World {
        fn open_manifest(&self, artifact_id: Hash32) -> Result<SegmentManifestBody, SegmentError> {
            self.manifests
                .get(&artifact_id)
                .cloned()
                .ok_or(SegmentError::ArtifactUnavailable { artifact_id })
        }

        fn open_hashseq_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError> {
            self.nodes
                .get(&artifact_id)
                .cloned()
                .ok_or(SegmentError::ArtifactUnavailable { artifact_id })
        }

        fn open_header_segment(
            &self,
            artifact_id: Hash32,
        ) -> Result<HeaderSegmentBody, SegmentError> {
            self.header_segments
                .get(&artifact_id)
                .cloned()
                .ok_or(SegmentError::ArtifactUnavailable { artifact_id })
        }

        fn open_payload_shard(
            &self,
            artifact_id: Hash32,
        ) -> Result<PayloadShardBody, SegmentError> {
            self.shards
                .get(&artifact_id)
                .cloned()
                .ok_or(SegmentError::ArtifactUnavailable { artifact_id })
        }
    }

    struct Checkpoint {
        verse_id: Identifier32,
        branch_id: Identifier32,
        segment_manifest_id: Hash32,
        selected_frontier: SortedFrontier,
        frontier_commitment: Hash32,
        signature_valid: bool,
    }

    impl CheckpointView for Checkpoint {
        fn verse_id(&self) -> Identifier32 {
            self.verse_id
        }

        fn branch_id(&self) -> Identifier32 {
            self.branch_id
        }

        fn segment_manifest_id(&self) -> Hash32 {
            self.segment_manifest_id
        }

        fn selected_frontier(&self) -> &SortedFrontier {
            &self.selected_frontier
        }

        fn frontier_commitment(&self) -> Hash32 {
            self.frontier_commitment
        }

        fn replay_base(&self) -> Option<Hash32> {
            None
        }

        fn verify_manager_signature(&self) -> Result<(), SegmentError> {
            if self.signature_valid {
                Ok(())
            } else {
                Err(SegmentError::Unauthorized)
            }
        }
    }

    struct Keys {
        authorized: Vec<LaneKey>,
    }

    impl ScopeKeyResolver for Keys {
        fn has_decrypt_authority(&self, lane: &LaneKey) -> bool {
            self.authorized.contains(lane)
        }
    }

    struct NoLocalHistory;

    impl ParentWalk for NoLocalHistory {
        fn parents_of(&self, _op_id: Hash32) -> Option<Vec<Hash32>> {
            None
        }

        fn is_genesis(&self, _op_id: Hash32) -> bool {
            false
        }
    }

    struct NeedsEveryPayload;

    impl PayloadAdmission for NeedsEveryPayload {
        fn projection_needs_payload(&self, _op_id: Hash32) -> bool {
            true
        }

        fn admit_payload(
            &self,
            _op_id: Hash32,
            _record: &PayloadShardRecord,
        ) -> Result<(), SegmentError> {
            Ok(())
        }
    }

    struct Scenario {
        world: World,
        checkpoint: Checkpoint,
        header_lane: LaneKey,
        payload_lane: LaneKey,
        genesis: HeaderFixture,
        first: HeaderFixture,
        second: HeaderFixture,
        concurrent: HeaderFixture,
    }

    /// Genesis -> first -> second is the selected closure; `concurrent` hangs off genesis.
    fn scenario(include_first_header: bool, include_second_payload: bool) -> Scenario {
        let verse = identifier(0x11);
        let branch = identifier(0xB1);
        let petal = Scope::new(verse, Some(identifier(0x21)), None).expect("scope");

        let genesis = genesis_header(1, Scope::verse_wide(verse), 1);
        let first = intent_header(2, petal, vec![genesis.op_id], 1, b"ciphertext-first");
        let second = intent_header(3, petal, vec![first.op_id], 2, b"ciphertext-second");
        let concurrent = intent_header(4, petal, vec![genesis.op_id], 3, b"ciphertext-other");

        let mut header_bytes = vec![genesis.bytes.clone()];
        if include_first_header {
            header_bytes.push(first.bytes.clone());
        }
        header_bytes.push(second.bytes.clone());
        header_bytes.push(concurrent.bytes.clone());

        let mut world = World::default();
        let header_body =
            HeaderSegmentBody::seal(verse, header_bytes.iter().map(|bytes| bytes.as_slice()))
                .expect("header body");
        let (header_segment_id, header_segment_length) = world.add_header_segment(header_body);
        let (header_root, header_root_length) = world.add_node(
            HashSeqNode::new(
                LaneKey::Header { verse_id: verse },
                None,
                vec![HashSeqEntry {
                    artifact_id: header_segment_id,
                    stored_length: header_segment_length,
                }],
            )
            .expect("node"),
        );

        let topic = PayloadTopicScope {
            verse_id: verse,
            petal_id: identifier(0x21),
            scope_epoch: 7,
            key_id: identifier(0x41),
        };
        let mut records = vec![PayloadShardRecord::of(
            first.op_id,
            first.ciphertext.clone(),
        )];
        if include_second_payload {
            records.push(PayloadShardRecord::of(
                second.op_id,
                second.ciphertext.clone(),
            ));
        }
        let (shard_id, shard_length) =
            world.add_shard(PayloadShardBody::new(topic, records).expect("shard"));
        let (payload_root, payload_root_length) = world.add_node(
            HashSeqNode::new(
                LaneKey::Payload(topic),
                None,
                vec![HashSeqEntry {
                    artifact_id: shard_id,
                    stored_length: shard_length,
                }],
            )
            .expect("node"),
        );

        let manifest = SegmentManifestBody::new(
            verse,
            branch,
            BTreeMap::from([(header_root, header_root_length)]),
            BTreeMap::from([(topic, BTreeMap::from([(payload_root, payload_root_length)]))]),
            BTreeMap::from([
                (
                    LaneKey::Header { verse_id: verse },
                    BoundaryNode {
                        oldest_required_node: header_root,
                        stored_length: header_root_length,
                    },
                ),
                (
                    LaneKey::Payload(topic),
                    BoundaryNode {
                        oldest_required_node: payload_root,
                        stored_length: payload_root_length,
                    },
                ),
            ]),
        )
        .expect("manifest");
        let (segment_manifest_id, _) = world.add_manifest(manifest);
        let selected_frontier = SortedFrontier::try_new([second.op_id]).expect("frontier");

        Scenario {
            world,
            checkpoint: Checkpoint {
                verse_id: verse,
                branch_id: branch,
                frontier_commitment: selected_frontier.commitment(),
                selected_frontier,
                segment_manifest_id,
                signature_valid: true,
            },
            header_lane: LaneKey::Header { verse_id: verse },
            payload_lane: LaneKey::Payload(topic),
            genesis,
            first,
            second,
            concurrent,
        }
    }

    fn prove(scenario: &Scenario, keys: &Keys) -> CheckpointProofOutcome {
        verify_checkpoint(&ProofInputs {
            checkpoint: &scenario.checkpoint,
            lookup: &scenario.world,
            environment: &scenario.world,
            scope_keys: keys,
            parents: &NoLocalHistory,
            admission: &NeedsEveryPayload,
            bounds: ProofBounds::new(64, 64),
        })
    }

    #[test]
    fn checkpoint_proof_covers_selected_parent_closure() {
        let complete = scenario(true, true);
        let both_lanes = Keys {
            authorized: vec![complete.header_lane, complete.payload_lane],
        };
        assert_eq!(
            prove(&complete, &both_lanes),
            CheckpointProofOutcome::Verified {
                headers_verified: 3,
                payloads_verified: 2,
            }
        );
        assert_ne!(complete.concurrent.op_id, complete.second.op_id);
        assert_ne!(complete.genesis.op_id, complete.first.op_id);

        let missing_ancestor = scenario(false, true);
        assert_eq!(
            prove(
                &missing_ancestor,
                &Keys {
                    authorized: vec![missing_ancestor.header_lane, missing_ancestor.payload_lane],
                }
            ),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::MissingRequiredHeader {
                    op_id: missing_ancestor.first.op_id
                }
            }
        );

        let missing_payload = scenario(true, false);
        assert_eq!(
            prove(
                &missing_payload,
                &Keys {
                    authorized: vec![missing_payload.header_lane, missing_payload.payload_lane],
                }
            ),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::MissingRequiredPayload {
                    op_id: missing_payload.second.op_id
                }
            }
        );

        let header_only = scenario(true, true);
        assert_eq!(
            prove(
                &header_only,
                &Keys {
                    authorized: vec![header_only.header_lane]
                }
            ),
            CheckpointProofOutcome::HeaderReachableOnly {
                headers_verified: 3,
                unauthorized_lanes: vec![header_only.payload_lane],
            }
        );

        let no_header_key = scenario(true, true);
        assert_eq!(
            prove(&no_header_key, &Keys { authorized: vec![] }),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::ScopeKeyUnavailable
            }
        );
    }

    #[test]
    fn checkpoint_signature_is_not_history_authority() {
        let mut unsigned = scenario(true, true);
        unsigned.checkpoint.signature_valid = false;
        assert_eq!(
            prove(
                &unsigned,
                &Keys {
                    authorized: vec![unsigned.header_lane, unsigned.payload_lane]
                }
            ),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::Unauthorized
            }
        );

        let mut substituted = scenario(true, true);
        let manifest_id = substituted.checkpoint.segment_manifest_id;
        substituted
            .world
            .artifacts
            .insert(manifest_id, b"substituted-bytes".to_vec());
        let keys = Keys {
            authorized: vec![substituted.header_lane, substituted.payload_lane],
        };
        assert_eq!(
            prove(&substituted, &keys),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::ArtifactIdMismatch {
                    claimed: manifest_id,
                    computed: Hash32::of(b"substituted-bytes"),
                }
            }
        );

        // A valid Manager+ signature over one frontier commitment cannot be paired with a
        // different set of heads: the proof recomputes the D-CL19 commitment over the heads it is
        // about to walk before it fetches anything.
        let mut substituted_frontier = scenario(true, true);
        let other_heads =
            SortedFrontier::try_new([substituted_frontier.genesis.op_id]).expect("frontier");
        let signed_commitment = substituted_frontier.checkpoint.frontier_commitment;
        substituted_frontier.checkpoint.selected_frontier = other_heads.clone();
        assert_eq!(
            prove(
                &substituted_frontier,
                &Keys {
                    authorized: vec![
                        substituted_frontier.header_lane,
                        substituted_frontier.payload_lane
                    ]
                }
            ),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::FrontierCommitmentMismatch {
                    claimed: signed_commitment,
                    computed: other_heads.commitment(),
                }
            }
        );

        let mut wrong_branch = scenario(true, true);
        wrong_branch.checkpoint.branch_id = identifier(0xB2);
        assert_eq!(
            prove(
                &wrong_branch,
                &Keys {
                    authorized: vec![wrong_branch.header_lane, wrong_branch.payload_lane]
                }
            ),
            CheckpointProofOutcome::Unresolved {
                reason: SegmentError::CheckpointBindingMismatch
            }
        );

        let mut absent_segment = scenario(true, true);
        let segment_ids: Vec<Hash32> = absent_segment
            .world
            .header_segments
            .keys()
            .copied()
            .collect();
        for id in segment_ids {
            absent_segment.world.artifacts.remove(&id);
        }
        assert!(matches!(
            prove(
                &absent_segment,
                &Keys {
                    authorized: vec![absent_segment.header_lane, absent_segment.payload_lane]
                }
            ),
            CheckpointProofOutcome::Unresolved { .. }
        ));
    }
}
