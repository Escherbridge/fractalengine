//! Storage-side compositions over the crate's injected seams: a byte-store-backed
//! `SealedArtifactStore`, a decrypting `ProofEnvironment`, and the production `wire::commit`
//! pipeline (W4-4). See `src/AGENTS.md` §composed-verifiers and §trait-composition-map.

use async_trait::async_trait;

use crate::crypto::aead;
use crate::crypto::key_id::derive_key_id;
use crate::crypto::scope_key::{LaneScopeKeyResolver, ScopeKeyStore};
use crate::envelope::{Hash32, Identifier32};
use crate::materialize::errors::{AdmissionOutcome, AppendError, AppendOutcome};
use crate::materialize::traits::{
    admit_candidate, CandidateVerifier, EquivocationIndex, ParentVerseLookup, VerifiedLogStore,
};
use crate::segment::artifact::{admit_sealed, verify_artifact_id, LaneClass};
use crate::segment::checkpoint_proof::ProofEnvironment;
use crate::segment::hashseq::{ArtifactLookup, HashSeqNode, LaneKey};
use crate::segment::header_lane::HeaderSegmentBody;
use crate::segment::manifest::SegmentManifestBody;
use crate::segment::payload_shard::{PayloadShardBody, PayloadTopicScope};
use crate::segment::store::{assert_immutable_write, SealedArtifactStore};
use crate::segment::SegmentError;
use crate::wire::commit::{CanonicalCommitPipeline, PipelineResult};
use crate::wire::error::ProtocolErrorCategory;

/// Raw content-addressed byte storage, injected into [`ByteStoreSealedArtifactStore`].
///
/// A production implementation MUST be local-only (an `fe-database` table, say) and MUST NOT be
/// the iroh blob store: iroh blobs are served to peers, and that is the canonical network
/// exposure this crate's sealed-artifact lanes stay NO_GO for.
pub trait ContentByteStore {
    /// The exact stored bytes for `artifact_id`, if present.
    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>>;
    /// Whether `artifact_id` is present.
    fn has(&self, artifact_id: Hash32) -> bool;
    /// Stores `bytes` under `artifact_id`. The caller has already verified the id and the
    /// immutable-rewrite rule; this is the raw write.
    fn put(&mut self, artifact_id: Hash32, bytes: Vec<u8>);
}

/// [`SealedArtifactStore`] over an injected [`ContentByteStore`] (SPEC-6 §2.1.6, §5.1).
///
/// Mirrors `segment::store::InMemorySealedArtifactStore`'s own `put_verified` body exactly:
/// recompute the id, then [`assert_immutable_write`] BEFORE delegating to the byte store, so a
/// caller cannot reach a conflicting write through this adapter without both checks running
/// first — the backing store's own `put` never sees a rejected rewrite.
#[derive(Debug)]
pub struct ByteStoreSealedArtifactStore<B> {
    store: B,
}

impl<B: ContentByteStore> ByteStoreSealedArtifactStore<B> {
    /// Wraps a byte store.
    pub fn new(store: B) -> Self {
        Self { store }
    }

    /// Borrows the underlying byte store.
    pub fn store(&self) -> &B {
        &self.store
    }

    /// Unwraps the underlying byte store.
    pub fn into_store(self) -> B {
        self.store
    }
}

impl<B: ContentByteStore> SealedArtifactStore for ByteStoreSealedArtifactStore<B> {
    fn put_verified(&mut self, claimed_id: Hash32, bytes: &[u8]) -> Result<Hash32, SegmentError> {
        let artifact_id = verify_artifact_id(bytes, claimed_id)?;
        assert_immutable_write(self.store.get(artifact_id).as_deref(), artifact_id, bytes)?;
        self.store.put(artifact_id, bytes.to_vec());
        Ok(artifact_id)
    }

    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.store.get(artifact_id)
    }

    fn has(&self, artifact_id: Hash32) -> bool {
        self.store.has(artifact_id)
    }
}

impl<B: ContentByteStore> ArtifactLookup for ByteStoreSealedArtifactStore<B> {
    fn fetch(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.store.get(artifact_id)
    }
}

/// Decrypting [`ProofEnvironment`] over verified bytes and real scope-key custody.
///
/// Pure composition; no segment producer is added. Every fetch re-hashes independently through
/// [`ArtifactLookup::fetch_verified`] before anything is decrypted — the guarantee
/// `ProofEnvironment`'s own doc comment asks for, rather than one resting on a promise.
///
/// A sealed artifact's outer form commits only to its `key_id` (SPEC-6 §2.1.3), never to the
/// `(scope, epoch)` pair that produced it — and a payload shard's own topic scope lives INSIDE
/// the body this type is in the middle of decrypting, so which lane an artifact belongs to
/// cannot be assumed from the call alone. `payload_topics` is the small, caller-known set of
/// topics this environment currently holds (or cares to try) a key for — realistically the
/// petals the local principal has payload capability over, never the whole verse. Resolution
/// tries the header lane first (fully determined by `verse_id` and `header_scope_epoch`, both
/// fixed at construction), then each of those topics, accepting a lane only when the scope key
/// held there derives the exact `key_id` the artifact committed to.
#[derive(Debug)]
pub struct SealedProofEnvironment<'a, A, K> {
    lookup: &'a A,
    keys: &'a K,
    verse_id: Identifier32,
    header_scope_epoch: u64,
    payload_topics: &'a [PayloadTopicScope],
}

impl<'a, A: ArtifactLookup, K: ScopeKeyStore> SealedProofEnvironment<'a, A, K> {
    /// Binds an artifact lookup and scope-key custody to one verse's header epoch and the
    /// caller's known candidate payload topics.
    pub fn new(
        lookup: &'a A,
        keys: &'a K,
        verse_id: Identifier32,
        header_scope_epoch: u64,
        payload_topics: &'a [PayloadTopicScope],
    ) -> Self {
        Self {
            lookup,
            keys,
            verse_id,
            header_scope_epoch,
            payload_topics,
        }
    }

    /// Whether `lane`'s current scope key derives `key_id` (SPEC-6 §9.1 commitment).
    fn lane_key_matches(&self, lane: &LaneKey, key_id: Identifier32) -> bool {
        let resolver = LaneScopeKeyResolver::new(self.keys, self.header_scope_epoch);
        let scope_key_id = resolver.lane_scope_key_id(lane);
        self.keys
            .scope_key(&scope_key_id)
            .and_then(|key| derive_key_id(&lane.topic_scope(), scope_key_id.epoch, key).ok())
            == Some(key_id)
    }

    /// The lane whose current scope key derives `key_id`, header lane tried first.
    fn resolve_lane(&self, key_id: Identifier32) -> Option<LaneKey> {
        let header_lane = LaneKey::Header {
            verse_id: self.verse_id,
        };
        if self.lane_key_matches(&header_lane, key_id) {
            return Some(header_lane);
        }
        self.payload_topics
            .iter()
            .copied()
            .map(LaneKey::Payload)
            .find(|lane| self.lane_key_matches(lane, key_id))
    }

    /// Re-hashes, decodes the sealed outer form, resolves the sealing lane, decrypts, and
    /// decodes the plaintext under its OWN declared lane class -- never a class the caller
    /// merely hoped for.
    fn open_and_decode(&self, artifact_id: Hash32) -> Result<DecodedSealedBody, SegmentError> {
        let bytes = self.lookup.fetch_verified(artifact_id)?;
        let (sealed, _) = admit_sealed(&bytes)?;
        let lane = self
            .resolve_lane(sealed.encryption.key_id())
            .ok_or(SegmentError::ScopeKeyUnavailable)?;
        let resolver = LaneScopeKeyResolver::new(self.keys, self.header_scope_epoch);
        let scope_key_id = resolver.lane_scope_key_id(&lane);
        let key = self
            .keys
            .scope_key(&scope_key_id)
            .ok_or(SegmentError::ScopeKeyUnavailable)?;
        let aad = sealed.associated_data()?;
        let plaintext = aead::open(key, sealed.encryption.params(), &aad, &sealed.ciphertext)
            .map_err(|_| SegmentError::DecryptionFailed)?;
        decode_sealed_body(sealed.lane_class, &plaintext)
    }
}

/// Every lane body a sealed artifact's OWN declared [`LaneClass`] (SPEC-6 §2.1.5) can decode to.
enum DecodedSealedBody {
    HeaderSegment(HeaderSegmentBody),
    PayloadShard(PayloadShardBody),
    HashSeqNode(HashSeqNode),
    SegmentManifest(SegmentManifestBody),
}

/// Picks the one decoder a sealed artifact's own declared lane class names.
///
/// A total match with **no wildcard arm**, per `src/AGENTS.md` §composed-verifiers: when
/// CDL-SPEC10 A2 adds a fifth [`LaneClass`] (`BulkArtifact`), this function refuses to compile
/// until that slice adds its own decoding arm here -- A2 owns that arm, not a silent fallthrough
/// that would otherwise let an unrecognized lane decode as whichever variant happened to be
/// requested.
fn decode_sealed_body(
    lane_class: LaneClass,
    plaintext: &[u8],
) -> Result<DecodedSealedBody, SegmentError> {
    match lane_class {
        LaneClass::HeaderSegment => {
            HeaderSegmentBody::decode_canonical(plaintext).map(DecodedSealedBody::HeaderSegment)
        }
        LaneClass::PayloadShard => {
            PayloadShardBody::decode_canonical(plaintext).map(DecodedSealedBody::PayloadShard)
        }
        LaneClass::HashSeqNode => {
            HashSeqNode::decode_canonical(plaintext).map(DecodedSealedBody::HashSeqNode)
        }
        LaneClass::SegmentManifest => {
            SegmentManifestBody::decode_canonical(plaintext).map(DecodedSealedBody::SegmentManifest)
        }
    }
}

impl<A: ArtifactLookup, K: ScopeKeyStore> ProofEnvironment for SealedProofEnvironment<'_, A, K> {
    fn open_manifest(&self, artifact_id: Hash32) -> Result<SegmentManifestBody, SegmentError> {
        match self.open_and_decode(artifact_id)? {
            DecodedSealedBody::SegmentManifest(body) => Ok(body),
            _ => Err(SegmentError::LaneBodyMismatch),
        }
    }

    fn open_hashseq_node(&self, artifact_id: Hash32) -> Result<HashSeqNode, SegmentError> {
        match self.open_and_decode(artifact_id)? {
            DecodedSealedBody::HashSeqNode(node) => Ok(node),
            _ => Err(SegmentError::LaneBodyMismatch),
        }
    }

    fn open_header_segment(&self, artifact_id: Hash32) -> Result<HeaderSegmentBody, SegmentError> {
        match self.open_and_decode(artifact_id)? {
            DecodedSealedBody::HeaderSegment(body) => Ok(body),
            _ => Err(SegmentError::LaneBodyMismatch),
        }
    }

    fn open_payload_shard(&self, artifact_id: Hash32) -> Result<PayloadShardBody, SegmentError> {
        match self.open_and_decode(artifact_id)? {
            DecodedSealedBody::PayloadShard(body) => Ok(body),
            _ => Err(SegmentError::LaneBodyMismatch),
        }
    }
}

/// The production [`CanonicalCommitPipeline`]: `admit_candidate` then `VerifiedLogStore::append`.
///
/// Appends only; it materializes nothing and names no projection, so a successful append — a
/// fresh write or a byte-identical resubmission alike — always reports
/// [`PipelineResult::AcceptedPendingMaterialization`]: the one wire state that claims nothing
/// about a cursor or binding this pipeline cannot see. A caller that also materializes wraps
/// this pipeline to upgrade that into `Committed`/`AlreadyCommitted`; this slice does not own
/// that wrapper (it needs a projection view no trait here supplies).
#[derive(Debug)]
pub struct VerifyingCommitPipeline<V, S, E, P> {
    verifier: V,
    store: S,
    equivocation_index: E,
    parent_lookup: P,
}

impl<V, S, E, P> VerifyingCommitPipeline<V, S, E, P> {
    /// Composes the pipeline from its seams.
    pub fn new(verifier: V, store: S, equivocation_index: E, parent_lookup: P) -> Self {
        Self {
            verifier,
            store,
            equivocation_index,
            parent_lookup,
        }
    }
}

/// Maps an admission refusal onto the closest SPEC-7 `protocol_error` category `wire/error.rs`
/// (not owned by this slice) already defines. SPEC-4's own quarantine/reject vocabulary predates
/// several of these outcomes (`UnknownKind` is D-CL28 gate G1; the three
/// `AuthorizationEvidenceUnavailable` reasons are erratum E-W4-1), so three mappings below are
/// judgment calls, not 1:1 correspondences, flagged here for the owner to ratify a dedicated
/// category:
///
/// - `UnknownKind` reuses `UnknownSchema` (same "retry once this build/registry catches up" shape).
/// - `AuthorizationEvidenceUnavailable` (any of its three reasons) reuses `MissingParent` (same
///   "retry once this process can resolve the missing evidence locally" shape).
/// - `AuthorEquivocation` reuses `ConflictingOpBytes` (both are "two candidates conflict at one
///   identity", even though equivocation quarantines both rather than rejecting a substitution).
///
/// `PreconditionFailed` and `SameVerseParentViolation` reuse `InvalidEnvelope` as the nearest
/// permanent-reject category; `compose::ComposedCandidateVerifier` never constructs the former,
/// so it is unreachable through the production verifier today.
fn admission_outcome_to_pipeline_result(outcome: AdmissionOutcome) -> PipelineResult {
    match outcome {
        AdmissionOutcome::InvalidEnvelope { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidEnvelope)
        }
        AdmissionOutcome::UnauthorizedOperation => {
            PipelineResult::Rejected(ProtocolErrorCategory::UnauthorizedOperation)
        }
        AdmissionOutcome::InvalidPayload => {
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidPayload)
        }
        AdmissionOutcome::PreconditionFailed => {
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidEnvelope)
        }
        AdmissionOutcome::MissingParent { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::MissingParent)
        }
        AdmissionOutcome::UnknownSchema { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::UnknownSchema)
        }
        AdmissionOutcome::UnknownKind { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::UnknownSchema)
        }
        AdmissionOutcome::AuthorizationEvidenceUnavailable { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::MissingParent)
        }
        AdmissionOutcome::OpaquePayload { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::OpaquePayload)
        }
        AdmissionOutcome::SameVerseParentViolation { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidEnvelope)
        }
        AdmissionOutcome::AuthorEquivocation { .. } => {
            PipelineResult::Rejected(ProtocolErrorCategory::ConflictingOpBytes)
        }
        AdmissionOutcome::MaterializationFailed | AdmissionOutcome::CheckpointMismatch => {
            PipelineResult::AcceptedPendingMaterialization
        }
    }
}

#[async_trait]
impl<V, S, E, P> CanonicalCommitPipeline for VerifyingCommitPipeline<V, S, E, P>
where
    V: CandidateVerifier,
    S: VerifiedLogStore,
    E: EquivocationIndex,
    P: ParentVerseLookup,
{
    fn derive_op_id(&self, complete_envelope_bytes: &[u8]) -> Hash32 {
        Hash32::of(complete_envelope_bytes)
    }

    async fn submit_candidate(
        &self,
        complete_envelope_bytes: &[u8],
        payload_ciphertext: Option<&[u8]>,
    ) -> PipelineResult {
        let op_id = self.derive_op_id(complete_envelope_bytes);
        let admitted = match admit_candidate(
            &self.verifier,
            &self.equivocation_index,
            &self.parent_lookup,
            complete_envelope_bytes,
            payload_ciphertext,
        )
        .await
        {
            Ok(meta) => meta,
            Err(outcome) => return admission_outcome_to_pipeline_result(outcome),
        };
        // `admit_candidate` judges whatever candidate the verifier returned; only append the
        // bytes it actually judged, whatever `V` is.
        if admitted.op_id != op_id {
            return PipelineResult::Rejected(ProtocolErrorCategory::InvalidOpId);
        }

        match self
            .store
            .append(op_id, complete_envelope_bytes.to_vec())
            .await
        {
            Ok(AppendOutcome::Appended) | Ok(AppendOutcome::AlreadyPresent) => {
                PipelineResult::AcceptedPendingMaterialization
            }
            Err(AppendError::IntegrityConflict { .. }) => PipelineResult::IntegrityConflict,
            Err(AppendError::HashMismatch) => {
                PipelineResult::Rejected(ProtocolErrorCategory::InvalidOpId)
            }
            // Indeterminate (`AppendError::is_definite() == false`):
            // `docs/spec/canonical-log/log-first-materialization.md` point 4 makes retry the
            // WIRE CALLER's job ("A caller MAY retry the same immutable bytes"), never this
            // pipeline's; point 3 forbids reporting success, and a permanent reject would
            // wrongly blacklist a perfectly valid `op_id` over a transient local fault.
            Err(AppendError::StorageUnavailable { .. })
            | Err(AppendError::NotAnEnvelope { .. }) => {
                PipelineResult::Rejected(ProtocolErrorCategory::MaterializationPending)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use crate::crypto::aead::{FreshNonce, SealingKey, SEALING_KEY_LENGTH};
    use crate::crypto::artifact_aad::StoredArtifactAad;
    use crate::crypto::scope_key::InMemoryScopeKeyStore;
    use crate::envelope::{
        Author, CapabilityRef, EquivocationKey, Hlc, PayloadRef, Scope, UnsignedEnvelope,
        NONCE_LENGTH, PRODUCTION_SUITE_ID, PROTOCOL_VERSION,
    };
    use crate::materialize::traits::{verify_candidate, PayloadFacts, VerifiedCandidate};
    use crate::retention::crypto_shred::{crypto_shred, ScopeKeyId};
    use crate::segment::artifact::{EncryptionDescriptor, SealedArtifact};
    use crate::segment::hashseq::HashSeqEntry;
    use crate::signing::sign_envelope;
    use ed25519_dalek::SigningKey;

    #[derive(Default)]
    struct TestByteStore {
        bytes: BTreeMap<Hash32, Vec<u8>>,
        put_calls: u32,
    }

    impl ContentByteStore for TestByteStore {
        fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
            self.bytes.get(&artifact_id).cloned()
        }

        fn has(&self, artifact_id: Hash32) -> bool {
            self.bytes.contains_key(&artifact_id)
        }

        fn put(&mut self, artifact_id: Hash32, bytes: Vec<u8>) {
            self.put_calls += 1;
            self.bytes.insert(artifact_id, bytes);
        }
    }

    fn sealed_bytes(body: &[u8]) -> (Hash32, Vec<u8>) {
        let artifact = SealedArtifact::seal(
            LaneClass::HeaderSegment,
            EncryptionDescriptor::new(
                PRODUCTION_SUITE_ID,
                Identifier32([1; 32]),
                [7; NONCE_LENGTH],
            ),
            body.to_vec(),
        )
        .expect("sealed");
        (
            artifact.artifact_id().expect("id"),
            artifact.encode_canonical().expect("bytes"),
        )
    }

    #[test]
    fn a_rewrite_of_an_existing_artifact_id_is_refused_by_the_byte_store_adapter() {
        let mut store = ByteStoreSealedArtifactStore::new(TestByteStore::default());
        let (artifact_id, bytes) = sealed_bytes(b"first-body");
        let (other_id, other_bytes) = sealed_bytes(b"other-body");

        assert_eq!(store.put_verified(artifact_id, &bytes), Ok(artifact_id));
        assert!(store.has(artifact_id));
        assert_eq!(store.get(artifact_id).as_deref(), Some(bytes.as_slice()));
        assert_eq!(store.store().put_calls, 1);

        // The identical bytes again: idempotent, not a rewrite, but still a dedup write.
        assert_eq!(store.put_verified(artifact_id, &bytes), Ok(artifact_id));
        assert_eq!(store.store().put_calls, 2);

        // A different byte string under the SAME claimed id is refused before the adapter ever
        // calls the backing store's own `put` -- the call count must not move.
        assert_eq!(
            store.put_verified(artifact_id, &other_bytes),
            Err(SegmentError::ArtifactIdMismatch {
                claimed: artifact_id,
                computed: other_id,
            })
        );
        assert_eq!(
            store.store().put_calls,
            2,
            "the refused rewrite must never reach `put`"
        );
        assert_eq!(store.get(artifact_id).as_deref(), Some(bytes.as_slice()));
        assert!(!store.has(other_id));
    }

    #[test]
    fn crypto_shred_through_the_custody_store_makes_the_payload_unopenable() {
        let verse_id = Identifier32([0x11; 32]);
        let scope = Scope::verse_wide(verse_id);
        let header_epoch = 1;
        let scope_key_id = ScopeKeyId {
            scope,
            epoch: header_epoch,
        };
        let key_bytes = [0x5a; SEALING_KEY_LENGTH];

        let mut keys = InMemoryScopeKeyStore::new();
        keys.insert_scope_key(scope_key_id.clone(), SealingKey::from_bytes(key_bytes))
            .expect("insert");
        let key_id = derive_key_id(&scope, header_epoch, &SealingKey::from_bytes(key_bytes))
            .expect("key id");

        let node = HashSeqNode::new(
            LaneKey::Header { verse_id },
            None,
            vec![HashSeqEntry {
                artifact_id: Hash32([0x02; 32]),
                stored_length: 10,
            }],
        )
        .expect("node");
        let plaintext = node.encode_canonical().expect("plaintext");

        let nonce = FreshNonce::draw(key_id).expect("nonce");
        let encryption_params = nonce.params();
        let aad = StoredArtifactAad::hashseq_node(plaintext.len(), encryption_params)
            .preimage()
            .expect("aad");
        let sealed_payload =
            aead::seal(&SealingKey::from_bytes(key_bytes), nonce, &aad, &plaintext).expect("seal");
        let descriptor = EncryptionDescriptor::new(
            encryption_params.suite_id,
            encryption_params.key_id,
            encryption_params.nonce,
        );
        let sealed = SealedArtifact::seal(
            LaneClass::HashSeqNode,
            descriptor,
            sealed_payload.ciphertext,
        )
        .expect("sealed");
        let bytes = sealed.encode_canonical().expect("bytes");
        let artifact_id = sealed.artifact_id().expect("id");

        let mut store = ByteStoreSealedArtifactStore::new(TestByteStore::default());
        store.put_verified(artifact_id, &bytes).expect("stored");

        {
            let environment =
                SealedProofEnvironment::new(&store, &keys, verse_id, header_epoch, &[]);
            assert_eq!(
                environment
                    .open_hashseq_node(artifact_id)
                    .expect("opens before shred"),
                node
            );
        }

        crypto_shred(&mut keys, &scope_key_id, Hlc::new(1_000, 0)).expect("shred");

        let environment = SealedProofEnvironment::new(&store, &keys, verse_id, header_epoch, &[]);
        assert_eq!(
            environment.open_hashseq_node(artifact_id),
            Err(SegmentError::ScopeKeyUnavailable),
            "the unified custody store no longer holds a key to open the payload with"
        );
    }

    struct AlwaysAdmits;

    #[async_trait]
    impl CandidateVerifier for AlwaysAdmits {
        async fn verify_envelope(
            &self,
            candidate_bytes: &[u8],
        ) -> Result<VerifiedCandidate, AdmissionOutcome> {
            verify_candidate(candidate_bytes)
        }

        async fn verify_authorization(
            &self,
            _candidate: &VerifiedCandidate,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            _candidate: &VerifiedCandidate,
            _payload_ciphertext: Option<&[u8]>,
        ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
            Ok(None)
        }
    }

    struct NoEquivocation;

    #[async_trait]
    impl EquivocationIndex for NoEquivocation {
        async fn op_id_at(&self, _key: EquivocationKey) -> Option<Hash32> {
            None
        }
    }

    struct NoParents;

    #[async_trait]
    impl ParentVerseLookup for NoParents {
        async fn verse_id_of(&self, _parent_op_id: Hash32) -> Option<Identifier32> {
            None
        }
    }

    #[derive(Default)]
    struct RecordingLogStoreState {
        rows: BTreeMap<Hash32, Vec<u8>>,
        fresh_appends: u32,
        already_present_hits: u32,
    }

    #[derive(Default)]
    struct RecordingLogStore {
        state: Mutex<RecordingLogStoreState>,
    }

    #[async_trait]
    impl VerifiedLogStore for RecordingLogStore {
        async fn append(
            &self,
            claimed_op_id: Hash32,
            bytes: Vec<u8>,
        ) -> Result<AppendOutcome, AppendError> {
            let mut state = self.state.lock().expect("lock");
            if let Some(existing) = state.rows.get(&claimed_op_id) {
                if *existing == bytes {
                    state.already_present_hits += 1;
                    return Ok(AppendOutcome::AlreadyPresent);
                }
                return Err(AppendError::IntegrityConflict {
                    op_id: claimed_op_id,
                });
            }
            state.rows.insert(claimed_op_id, bytes);
            state.fresh_appends += 1;
            Ok(AppendOutcome::Appended)
        }

        async fn get_bytes(&self, op_id: Hash32) -> Option<Vec<u8>> {
            self.state.lock().expect("lock").rows.get(&op_id).cloned()
        }

        async fn get_meta(
            &self,
            _op_id: Hash32,
        ) -> Option<crate::materialize::traits::VerifiedEnvelopeMeta> {
            None
        }

        async fn parents_of(&self, _op_id: Hash32) -> Option<Vec<Hash32>> {
            None
        }
    }

    /// Answers every candidate with ONE fixed, validly signed envelope: a verifier that judges
    /// bytes other than the ones it was handed.
    struct JudgesOtherBytes(Vec<u8>);

    #[async_trait]
    impl CandidateVerifier for JudgesOtherBytes {
        async fn verify_envelope(
            &self,
            _candidate_bytes: &[u8],
        ) -> Result<VerifiedCandidate, AdmissionOutcome> {
            verify_candidate(&self.0)
        }

        async fn verify_authorization(
            &self,
            _candidate: &VerifiedCandidate,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            _candidate: &VerifiedCandidate,
            _payload_ciphertext: Option<&[u8]>,
        ) -> Result<Option<PayloadFacts>, AdmissionOutcome> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn commit_submit_never_appends_bytes_the_verifier_did_not_judge() {
        let judged = signed_sample_bytes_at(Hlc::new(42, 7));
        let submitted = signed_sample_bytes_at(Hlc::new(43, 0));
        assert_ne!(judged, submitted);
        let pipeline = VerifyingCommitPipeline::new(
            JudgesOtherBytes(judged),
            RecordingLogStore::default(),
            NoEquivocation,
            NoParents,
        );

        assert_eq!(
            pipeline.submit_candidate(&submitted, None).await,
            PipelineResult::Rejected(ProtocolErrorCategory::InvalidOpId)
        );
        let state = pipeline.store.state.lock().expect("lock");
        assert_eq!(
            state.fresh_appends, 0,
            "unjudged bytes must never be appended"
        );
        assert!(state.rows.is_empty());
    }

    fn signed_sample_bytes() -> Vec<u8> {
        signed_sample_bytes_at(Hlc::new(42, 7))
    }

    fn signed_sample_bytes_at(hlc: Hlc) -> Vec<u8> {
        let signing_key = SigningKey::from_bytes(&[0x42; 32]);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 2,
            scope: Scope::verse_wide(Identifier32([0x11; 32])),
            author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 5,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: Identifier32([0x55; 32]),
            parents: Vec::new(),
            hlc,
            payload: PayloadRef::empty(),
        };
        sign_envelope(&signing_key, &unsigned)
            .expect("sign")
            .encode_canonical()
            .expect("encode")
    }

    #[tokio::test]
    async fn commit_submit_resubmitted_byte_identical_acks_already_present_not_a_second_append() {
        let bytes = signed_sample_bytes();
        let pipeline = VerifyingCommitPipeline::new(
            AlwaysAdmits,
            RecordingLogStore::default(),
            NoEquivocation,
            NoParents,
        );

        let first = pipeline.submit_candidate(&bytes, None).await;
        assert_eq!(first, PipelineResult::AcceptedPendingMaterialization);

        let second = pipeline.submit_candidate(&bytes, None).await;
        assert_eq!(
            second,
            PipelineResult::AcceptedPendingMaterialization,
            "a byte-identical resubmission acks the same way, not as a fresh append"
        );

        let state = pipeline.store.state.lock().expect("lock");
        assert_eq!(
            state.fresh_appends, 1,
            "the resubmission must not perform a second append"
        );
        assert_eq!(state.already_present_hits, 1);
        assert_eq!(state.rows.len(), 1);
    }
}
