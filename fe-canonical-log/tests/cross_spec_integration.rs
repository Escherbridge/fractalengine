//! CDL-W4 slice W4-5: cross-spec integration flows over the crate's production `compose` types
//! (`ComposedCandidateVerifier`, `ChainCapabilityVerifier`, `VerifyingCommitPipeline`,
//! `SealedProofEnvironment` + `ByteStoreSealedArtifactStore`), public API only, in-memory seams,
//! no DB and no network. See `conductor/tracks/canonical_data_log_20260808/campaign-plan-w4-20261013.md`
//! slice W4-5.
//!
//! Test 6 (`a_device_enrolment_gates_a_scope_key_wrap`) was appended by W4-6, once
//! `crypto::device_enrolment::EnrolmentRegistry` gave `DeviceEnrolmentView` a production
//! implementor to compose over.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use fe_canonical_log::capability::{
    AuthorityState, ManagerAuthorityView, ObjectClass, Principal, Verb,
};
use fe_canonical_log::cbor::{decode_canonical, encode_canonical, CborValue};
use fe_canonical_log::checkpoint::{sign_checkpoint_claim, CheckpointClaimV1, CHECKPOINT_VERSION};
use fe_canonical_log::compose::{
    ByteStoreSealedArtifactStore, ChainCapabilityVerifier, SealedProofEnvironment,
    SignedCheckpointProofView, VerifyingCommitPipeline,
};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
use fe_canonical_log::crypto::artifact_aad::StoredArtifactAad;
use fe_canonical_log::crypto::device_enrolment::{
    DeviceEnrolmentRevocationV1, DeviceEnrolmentV1, EnrolmentRegistry, VerifiedEnrolmentArtifact,
};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::key_wrap::{
    issue_scope_key_wrap, open_scope_key, AuthorizationRecord, DeliveryAuthorizationView,
    DeviceEnrolmentView, DevicePublicKey, DeviceSecretKey, IssuerAuthorityView,
    RecipientDeviceBinding, ScopeKeyWrap, ScopeKeyWrapRequest,
};
use fe_canonical_log::crypto::scope_key::{InMemoryScopeKeyStore, LaneScopeKeyResolver};
use fe_canonical_log::crypto::CryptoError;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::errors::{AdmissionDisposition, AdmissionOutcome};
use fe_canonical_log::materialize::traits::{admit_candidate_with_facts, CandidateVerifier};
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::segment::artifact::{
    seal_artifact, EncryptionDescriptor, LaneClass, NonceLedger, SealRequest,
};
use fe_canonical_log::segment::checkpoint_proof::{
    verify_checkpoint, CheckpointProofOutcome, ParentWalk, PayloadAdmission, ProofBounds,
    ProofInputs,
};
use fe_canonical_log::segment::hashseq::{HashSeqEntry, HashSeqNode, LaneKey};
use fe_canonical_log::segment::header_lane::HeaderSegmentBody;
use fe_canonical_log::segment::manifest::{BoundaryNode, SegmentManifestBody, SegmentStatistics};
use fe_canonical_log::segment::payload_shard::PayloadShardRecord;
use fe_canonical_log::segment::relay_policy::{PeerIdentity, RelayAuthorizationView};
use fe_canonical_log::segment::store::SealedArtifactStore;
use fe_canonical_log::segment::SegmentError;
use fe_canonical_log::wire::commit::{
    handle_commit_submit, is_delta_authorized_for_subscription, CommitAckState, CommitDeltaBody,
    CommitSubmitBody,
};
use fe_canonical_log::wire::cursor::{
    BranchRegistry, CommittedDelta, CursorTuple, ProjectionIdentity,
};
use fe_canonical_log::wire::error::{ProtocolErrorCategory, WireError};
use fe_canonical_log::wire::session::{CapabilityVerifier, SessionAuthorizationTable};
use fe_canonical_log::wire::subscription::SubscriptionRecord;

/// The SPEC-1 golden-vector fixture, read only for its signing identity: this file's own
/// `signing_key.seed_hex`/`public_key_hex`/`did` are byte-identical to
/// `tests/fixtures/capability-chain-v1.json`'s root-certificate issuer, which is what ties this
/// test's author to "the golden vector" and "its fixture chain" (see
/// `support::self_issued_chain`'s doc comment for why a fresh chain is minted rather than the
/// fixture's own frozen bytes).
const GOLDEN_ENVELOPE_FIXTURE_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../docs/spec/canonical-log/operation-envelope-v1.json"
));

fn golden_vector_signing_seed() -> [u8; 32] {
    let fixture: serde_json::Value =
        serde_json::from_str(GOLDEN_ENVELOPE_FIXTURE_JSON).expect("golden envelope fixture parses");
    let seed_hex = fixture["signing_key"]["seed_hex"]
        .as_str()
        .expect("seed_hex is a string");
    hex::decode(seed_hex)
        .expect("seed_hex is hex")
        .try_into()
        .expect("32-byte seed")
}

#[tokio::test]
async fn golden_vector_envelope_admits_through_the_composed_verifier() {
    let author = support::signing_key(golden_vector_signing_seed());
    let scope = support::verse_scope();
    let (chain_bytes, chain_id) = support::self_issued_chain(
        &author,
        &[Verb::Append],
        &[ObjectClass::Operation],
        scope,
        scope,
        support::EPOCH,
    );

    // Kind 2 (branch_genesis): zero parents, no-payload form allowed -- the simplest structurally
    // valid operation this author's chain can admit.
    let unsigned = support::build_unsigned(
        &author,
        2,
        scope,
        chain_id,
        support::EPOCH,
        support::hash(0x70),
        Identifier32([0x40; 32]),
        Vec::new(),
        Hlc::new(support::NOW_MS, 0),
        PayloadRef::empty(),
    );
    let complete = support::sign(&author, &unsigned);
    let bytes = complete.encode_canonical().expect("encode");
    let op_id = Hash32::of(&bytes);

    let verifier = support::build_verifier(
        chain_bytes,
        support::ManagerView::at_epoch(support::principal_of(&author), scope, support::EPOCH),
        InMemoryScopeKeyStore::new(),
    );

    let admitted = admit_candidate_with_facts(
        &verifier,
        &support::NoEquivocation,
        &support::SameVerseParents,
        &bytes,
        None,
    )
    .await
    .expect("the golden-vector-identity envelope admits through its self-issued chain");
    assert_eq!(admitted.candidate.meta.op_id, op_id);
    assert!(
        admitted.payload_facts.is_none(),
        "no-payload form extracts no facts"
    );

    // Flipping one signature byte is caught at ingress and never reaches authorization.
    let mut forged = complete.clone();
    forged.signature[0] ^= 0xff;
    let forged_bytes = forged.encode_canonical().expect("encode");
    assert!(
        matches!(
            verifier.verify_envelope(&forged_bytes).await,
            Err(AdmissionOutcome::InvalidEnvelope { .. })
        ),
        "a forged signature must be rejected at the mandatory ingress, not admitted"
    );
}

#[tokio::test]
async fn a_capability_gates_append_and_is_refused_after_a_scope_epoch_bump() {
    let author = support::signing_key([0x61; 32]);
    let scope = support::verse_scope();
    let (chain_bytes, chain_id) = support::self_issued_chain(
        &author,
        &[Verb::Append],
        &[ObjectClass::Operation],
        scope,
        scope,
        support::EPOCH,
    );
    let unsigned = support::build_unsigned(
        &author,
        2,
        scope,
        chain_id,
        support::EPOCH,
        support::hash(0x71),
        Identifier32([0x41; 32]),
        Vec::new(),
        Hlc::new(support::NOW_MS, 0),
        PayloadRef::empty(),
    );
    let bytes = support::sign(&author, &unsigned)
        .encode_canonical()
        .expect("encode");

    let principal = support::principal_of(&author);
    let live = support::build_verifier(
        chain_bytes.clone(),
        support::ManagerView::at_epoch(principal.clone(), scope, support::EPOCH),
        InMemoryScopeKeyStore::new(),
    );
    let candidate = live.verify_envelope(&bytes).await.expect("ingress");
    assert_eq!(live.verify_authorization(&candidate).await, Ok(()));

    // The persistent view moves to EPOCH + 1 with the chain itself unchanged: the chain still
    // verifies byte-for-byte, but SPEC-3 §2.4 step 9 now refuses it -- a permanent reject, never
    // a retryable quarantine, because the chain artifact and the epoch state both resolved; they
    // simply disagree.
    let bumped = support::build_verifier(
        chain_bytes,
        support::ManagerView::at_epoch(principal, scope, support::EPOCH + 1),
        InMemoryScopeKeyStore::new(),
    );
    let refused = bumped.verify_authorization(&candidate).await;
    assert_eq!(refused, Err(AdmissionOutcome::UnauthorizedOperation));
    assert_eq!(
        refused.unwrap_err().disposition(),
        AdmissionDisposition::Reject
    );
}

/// Seals `plaintext` under `lane_class` into `lane` at `epoch`, through the crate's ONE public
/// seal door (`segment::artifact::seal_artifact` -- `SealedArtifact::seal` itself is
/// `pub(crate)` precisely so nothing else can reach it, per that function's own doc comment),
/// and stores the result, returning its artifact id and exact stored byte length.
fn seal_and_store(
    store: &mut ByteStoreSealedArtifactStore<support::MemoryByteStore>,
    key: &SealingKey,
    lane: LaneKey,
    epoch: u64,
    lane_class: LaneClass,
    plaintext: &[u8],
) -> (Hash32, u64) {
    let key_id = derive_key_id(&lane.topic_scope(), epoch, key).expect("key id");
    let nonce = FreshNonce::draw(key_id).expect("nonce");
    let encryption = nonce.params();
    let aad = StoredArtifactAad::for_plaintext(lane_class, plaintext.len(), encryption)
        .preimage()
        .expect("aad");
    let sealed_payload = seal(key, nonce, &aad, plaintext).expect("seal");
    let descriptor =
        EncryptionDescriptor::new(encryption.suite_id, encryption.key_id, encryption.nonce);
    let view = support::SealingView::declaring(lane, epoch, key_id);
    let mut ledger = NonceLedger::with_capacity(std::num::NonZeroUsize::new(4).unwrap());
    let sealed = seal_artifact(
        &view,
        &mut ledger,
        SealRequest {
            lane: &lane,
            scope_epoch: epoch,
            lane_class,
            descriptor,
            ciphertext: sealed_payload.ciphertext,
        },
    )
    .expect("sealed artifact");
    let bytes = sealed.encode_canonical().expect("encode");
    let artifact_id = sealed.artifact_id().expect("artifact id");
    let length = bytes.len() as u64;
    store.put_verified(artifact_id, &bytes).expect("stored");
    (artifact_id, length)
}

const HEADER_EPOCH: u64 = 3;
const CHECKPOINT_BRANCH: Identifier32 = Identifier32([0x42; 32]);

/// One admitted genesis header, sealed and HashSeq/manifest-indexed under `header_epoch`;
/// `withhold_header_segment` skips storing the header-segment artifact itself while still
/// building and storing everything that references it, so its own real artifact id is unchanged.
fn build_checkpoint_world(
    header_epoch: u64,
    key_bytes: [u8; SEALING_KEY_LENGTH],
    withhold_header_segment: bool,
) -> (
    ByteStoreSealedArtifactStore<support::MemoryByteStore>,
    Hash32,
    Hash32,
) {
    let verse_id = support::VERSE;
    let header_lane_scope = support::verse_scope();
    let scope_key = SealingKey::from_bytes(key_bytes);
    let mut store = ByteStoreSealedArtifactStore::new(support::MemoryByteStore::default());

    let author = support::signing_key([0x62; 32]);
    let genesis_unsigned = support::build_unsigned(
        &author,
        2,
        header_lane_scope,
        support::hash(0),
        0,
        support::hash(0x72),
        CHECKPOINT_BRANCH,
        Vec::new(),
        Hlc::new(support::NOW_MS, 0),
        PayloadRef::empty(),
    );
    let genesis_bytes = support::sign(&author, &genesis_unsigned)
        .encode_canonical()
        .expect("encode genesis");
    let genesis_op_id = Hash32::of(&genesis_bytes);

    let header_body = HeaderSegmentBody::seal(verse_id, [genesis_bytes.as_slice()])
        .expect("header body admits the genesis envelope");
    let header_plain = header_body.encode_canonical().expect("header body bytes");

    let header_lane = LaneKey::Header { verse_id };
    let mut scratch_store = ByteStoreSealedArtifactStore::new(support::MemoryByteStore::default());
    let (header_segment_id, header_segment_len) = seal_and_store(
        &mut scratch_store,
        &scope_key,
        header_lane,
        header_epoch,
        LaneClass::HeaderSegment,
        &header_plain,
    );
    if !withhold_header_segment {
        let header_segment_bytes = scratch_store
            .get(header_segment_id)
            .expect("just sealed and stored");
        store
            .put_verified(header_segment_id, &header_segment_bytes)
            .expect("stored header segment");
    }

    let header_node = HashSeqNode::new(
        header_lane,
        None,
        vec![HashSeqEntry {
            artifact_id: header_segment_id,
            stored_length: header_segment_len,
        }],
    )
    .expect("hashseq node");
    let header_node_bytes = header_node.encode_canonical().expect("encode node");
    let (header_root_id, header_root_len) = seal_and_store(
        &mut store,
        &scope_key,
        header_lane,
        header_epoch,
        LaneClass::HashSeqNode,
        &header_node_bytes,
    );

    let manifest = SegmentManifestBody::new(
        verse_id,
        CHECKPOINT_BRANCH,
        BTreeMap::from([(header_root_id, header_root_len)]),
        BTreeMap::new(),
        BTreeMap::from([(
            LaneKey::Header { verse_id },
            BoundaryNode {
                oldest_required_node: header_root_id,
                stored_length: header_root_len,
            },
        )]),
        SegmentStatistics {
            min_hlc: genesis_unsigned.hlc,
            max_hlc: genesis_unsigned.hlc,
            petals: BTreeSet::new(),
            operation_count: 1,
        },
        BTreeMap::new(),
    )
    .expect("manifest");
    let manifest_bytes = manifest.encode_canonical().expect("encode manifest");
    let (manifest_id, _) = seal_and_store(
        &mut store,
        &scope_key,
        header_lane,
        header_epoch,
        LaneClass::SegmentManifest,
        &manifest_bytes,
    );

    (store, genesis_op_id, manifest_id)
}

struct GenesisOnly(Hash32);

impl ParentWalk for GenesisOnly {
    fn parents_of(&self, _op_id: Hash32) -> Option<Vec<Hash32>> {
        None
    }

    fn is_genesis(&self, op_id: Hash32) -> bool {
        op_id == self.0
    }
}

struct NoPayloadNeeded;

impl PayloadAdmission for NoPayloadNeeded {
    fn projection_needs_payload(&self, _op_id: Hash32) -> bool {
        false
    }

    fn admit_payload(
        &self,
        _op_id: Hash32,
        _record: &PayloadShardRecord,
    ) -> Result<(), SegmentError> {
        Ok(())
    }
}

/// Signs a fresh Manager+ checkpoint claim over `genesis_op_id`/`manifest_id` and runs the
/// SPEC-6 §4.2 proof against `store` through the production `SealedProofEnvironment` and
/// `LaneScopeKeyResolver` (reused for both the coarse `ScopeKeyResolver` seam and the actual key
/// material -- it implements both roles).
fn run_checkpoint_proof(
    store: &ByteStoreSealedArtifactStore<support::MemoryByteStore>,
    genesis_op_id: Hash32,
    manifest_id: Hash32,
    scope_keys: &InMemoryScopeKeyStore,
    header_epoch: u64,
) -> CheckpointProofOutcome {
    let verse_id = support::VERSE;
    let manager_key = support::signing_key([0x63; 32]);
    let selected_frontier = SortedFrontier::try_new([genesis_op_id]).expect("frontier");
    let claim = CheckpointClaimV1 {
        checkpoint_version: CHECKPOINT_VERSION,
        verse_id,
        branch_id: CHECKPOINT_BRANCH,
        frontier_commitment: selected_frontier.commitment(),
        segment_manifest_id: manifest_id,
        materializer_id: Identifier32([0x01; 32]),
        materializer_version: 1,
        projection_root_hash: Hash32([0x02; 32]),
        authorization_view_root: Hash32([0x03; 32]),
        snapshot_manifest_id: None,
        signer: Author::from_public_key(support::public_key(&manager_key)),
        capability: CapabilityRef {
            chain_id: Hash32([0x04; 32]),
            scope_epoch: 1,
        },
        issued_hlc: Hlc::new(support::NOW_MS, 0),
    };
    let signed_claim = sign_checkpoint_claim(&manager_key, &claim).expect("sign claim");
    let view = SignedCheckpointProofView::new(&signed_claim, &selected_frontier, None);
    let environment = SealedProofEnvironment::new(store, scope_keys, verse_id, header_epoch, &[]);
    let resolver = LaneScopeKeyResolver::new(scope_keys, header_epoch);
    verify_checkpoint(&ProofInputs {
        checkpoint: &view,
        lookup: store,
        environment: &environment,
        scope_keys: &resolver,
        parents: &GenesisOnly(genesis_op_id),
        admission: &NoPayloadNeeded,
        bounds: ProofBounds::new(64, 64),
    })
}

#[test]
fn a_valid_claim_still_reports_unresolved_reachability_without_header_or_decrypt_authority() {
    let key_bytes = [0x5a; SEALING_KEY_LENGTH];

    // Baseline: the Manager+ signature verifies, the header segment is held, and the verifier
    // holds the header lane's decrypt key -- Verified. This is the harness's own mutation check:
    // without a passing baseline, the two failure cases below could not be told apart from a
    // proof that always reports Unresolved regardless of setup.
    let (full_store, genesis_op_id, manifest_id) =
        build_checkpoint_world(HEADER_EPOCH, key_bytes, false);
    let keys_present = support::scope_key_store(support::verse_scope(), HEADER_EPOCH, key_bytes);
    assert_eq!(
        run_checkpoint_proof(
            &full_store,
            genesis_op_id,
            manifest_id,
            &keys_present,
            HEADER_EPOCH
        ),
        CheckpointProofOutcome::Verified {
            headers_verified: 1,
            payloads_verified: 0,
        }
    );

    // Without the header segment itself: the claim's own signature still verifies -- "a
    // Manager+ signature makes a claim; it never makes history valid" -- but the header closure
    // cannot be walked, so reachability is Unresolved, not a verified, materializable root.
    let (withheld_store, withheld_genesis, withheld_manifest) =
        build_checkpoint_world(HEADER_EPOCH, key_bytes, true);
    let outcome = run_checkpoint_proof(
        &withheld_store,
        withheld_genesis,
        withheld_manifest,
        &keys_present,
        HEADER_EPOCH,
    );
    assert!(
        matches!(outcome, CheckpointProofOutcome::Unresolved { .. }),
        "withholding the header segment must report Unresolved, got {outcome:?}"
    );

    // Without ANY decrypt authority for the header lane: refused before the manifest is even
    // opened -- also Unresolved, for a distinct, named reason (`ScopeKeyUnavailable`).
    let no_keys = InMemoryScopeKeyStore::new();
    assert_eq!(
        run_checkpoint_proof(
            &full_store,
            genesis_op_id,
            manifest_id,
            &no_keys,
            HEADER_EPOCH
        ),
        CheckpointProofOutcome::Unresolved {
            reason: SegmentError::ScopeKeyUnavailable
        }
    );
}

#[tokio::test]
async fn a_sealed_payload_round_trips_and_fails_closed_when_its_aad_envelope_is_mutated() {
    let author = support::signing_key([0x64; 32]);
    let scope = support::verse_scope();
    let (chain_bytes, chain_id) = support::self_issued_chain(
        &author,
        &[Verb::Append],
        &[ObjectClass::Operation],
        scope,
        scope,
        support::EPOCH,
    );
    let key_bytes = [0x5b; SEALING_KEY_LENGTH];
    let scope_keys = support::scope_key_store(scope, support::EPOCH, key_bytes);
    let key = SealingKey::from_bytes(key_bytes);

    let parent = support::hash(0x80);
    let key_id = derive_key_id(&scope, support::EPOCH, &key).expect("key id");
    let nonce = FreshNonce::draw(key_id).expect("nonce");
    let plaintext = encode_canonical(&CborValue::Map(vec![(
        CborValue::Uint(0),
        CborValue::Uint(99),
    )]));

    let branch_v1 = Identifier32([0x50; 32]);
    let mut unsigned_v1 = support::build_unsigned(
        &author,
        1, // normal_intent: needs >=1 parent and an encrypted non-empty payload
        scope,
        chain_id,
        support::EPOCH,
        support::SCHEMA,
        branch_v1,
        vec![parent],
        Hlc::new(support::NOW_MS, 0),
        PayloadRef {
            encryption: Some(nonce.params()),
            ..PayloadRef::empty()
        },
    );
    let aad_v1 = payload_aad_preimage(&unsigned_v1).expect("aad v1");
    let sealed = seal(&key, nonce, &aad_v1, &plaintext).expect("seal");
    unsigned_v1.payload = sealed.payload_ref();
    let bytes_v1 = support::sign(&author, &unsigned_v1)
        .encode_canonical()
        .expect("encode v1");

    let verifier = support::build_verifier(
        chain_bytes,
        support::ManagerView::at_epoch(support::principal_of(&author), scope, support::EPOCH),
        scope_keys,
    );

    // Round trip: the ciphertext opens under exactly the envelope it was sealed for.
    let candidate_v1 = verifier
        .verify_envelope(&bytes_v1)
        .await
        .expect("ingress v1");
    assert_eq!(verifier.verify_authorization(&candidate_v1).await, Ok(()));
    let facts = verifier
        .verify_payload(&candidate_v1, Some(&sealed.ciphertext))
        .await
        .expect("opens")
        .expect("payload-bearing operation has facts");
    assert_eq!(facts.fields, decode_canonical(&plaintext).expect("decode"));

    // Fails closed: the SAME ciphertext (same committed hash/length/encryption) now travels
    // inside a DIFFERENT, independently and validly signed envelope whose branch_id differs.
    // `branch_id` is covered by the §5.2 payload AAD but is not a SPEC-3 authorization input, so
    // verify_authorization still succeeds on its own -- only the AEAD open, bound to the whole
    // envelope, must refuse.
    let branch_v2 = Identifier32([0x51; 32]);
    let mut unsigned_v2 = unsigned_v1.clone();
    unsigned_v2.branch_id = branch_v2;
    let bytes_v2 = support::sign(&author, &unsigned_v2)
        .encode_canonical()
        .expect("encode v2");

    let candidate_v2 = verifier
        .verify_envelope(&bytes_v2)
        .await
        .expect("ingress v2");
    assert_eq!(
        verifier.verify_authorization(&candidate_v2).await,
        Ok(()),
        "branch_id is not a SPEC-3 authorization input"
    );
    assert_eq!(
        verifier
            .verify_payload(&candidate_v2, Some(&sealed.ciphertext))
            .await,
        Err(AdmissionOutcome::InvalidPayload),
        "the ciphertext travelled unchanged but its AAD envelope's branch_id differs from what \
         it was sealed under, so authenticated decryption must fail closed"
    );
}

#[tokio::test]
async fn commit_submit_ack_delta_flows_over_the_composed_verifier() {
    let author = support::signing_key([0x65; 32]);
    let verse_scope = support::verse_scope();
    let (chain_bytes, chain_id) = support::self_issued_chain(
        &author,
        &[Verb::Append, Verb::Fetch],
        &[ObjectClass::Operation],
        verse_scope,
        verse_scope,
        support::EPOCH,
    );

    let petal_a = support::petal_scope(support::PETAL_A);
    let branch_id = Identifier32([0x60; 32]);
    let unsigned = support::build_unsigned(
        &author,
        2,
        petal_a,
        chain_id,
        support::EPOCH,
        support::hash(0x73),
        branch_id,
        Vec::new(),
        Hlc::new(support::NOW_MS, 0),
        PayloadRef::empty(),
    );
    let bytes = support::sign(&author, &unsigned)
        .encode_canonical()
        .expect("encode");
    let claimed_op_id = Hash32::of(&bytes);

    let verifier = support::build_verifier(
        chain_bytes.clone(),
        support::ManagerView::at_epoch(support::principal_of(&author), verse_scope, support::EPOCH),
        InMemoryScopeKeyStore::new(),
    );
    let pipeline = VerifyingCommitPipeline::new(
        verifier,
        support::MemoryLogStore::default(),
        support::NoEquivocation,
        support::SameVerseParents,
    );

    let mut sessions = SessionAuthorizationTable::new();
    let generation = sessions
        .accept_binding([0x11; 16], chain_id, support::public_key(&author))
        .expect("accept binding");

    let body = CommitSubmitBody {
        session_generation: generation,
        authorization_binding_id: [0x11; 16],
        claimed_op_id,
        complete_envelope: bytes,
        payload_ciphertext: None,
    };
    let ack = handle_commit_submit(&pipeline, &sessions, &body).await;
    assert_eq!(
        ack.state,
        CommitAckState::AcceptedPendingMaterialization,
        "VerifyingCommitPipeline materializes nothing, so a successful append is always \
         pending, never Committed -- see its own doc comment"
    );

    // Mutation check: the ack above is the composed verifier's verdict, not the pipeline's
    // default. The same body through a pipeline whose authorization view has moved one epoch
    // on is refused by SPEC-3 and acked as a rejection, not as an append.
    let bumped_pipeline = VerifyingCommitPipeline::new(
        support::build_verifier(
            chain_bytes.clone(),
            support::ManagerView::at_epoch(
                support::principal_of(&author),
                verse_scope,
                support::EPOCH + 1,
            ),
            InMemoryScopeKeyStore::new(),
        ),
        support::MemoryLogStore::default(),
        support::NoEquivocation,
        support::SameVerseParents,
    );
    assert_eq!(
        handle_commit_submit(&bumped_pipeline, &sessions, &body)
            .await
            .state,
        CommitAckState::Rejected {
            category: ProtocolErrorCategory::UnauthorizedOperation,
        },
        "the commit pipeline must route through the composed verifier's SPEC-3 check"
    );

    // SPEC-7 wire `authorize` runs the same SPEC-3 chain walk for this author. A structural
    // append needs a kind allowlist (§4.7), and a kind-less session request against a kind
    // allowlist is refused (§2.4 step 8, "absence is refusal") — so the chain that admitted the
    // kind-2 op above is refused at the wire, while the same author's unrestricted chain over
    // the same grant is authorized.
    let wire_verifier = ChainCapabilityVerifier::for_requester(
        support::principal_of(&author),
        support::ManagerView::at_epoch(support::principal_of(&author), verse_scope, support::EPOCH),
        support::FixedClock(support::NOW_MS),
    );
    assert!(
        wire_verifier
            .verify(
                &chain_bytes,
                Verb::Append.bit(),
                ObjectClass::Operation.bit(),
                &verse_scope,
            )
            .await
            .is_err(),
        "a kind-allowlisted chain never authorizes a kind-less wire session"
    );
    let (unrestricted_chain, _) = support::self_issued_chain_with_caveats(
        &author,
        &[Verb::Append, Verb::Fetch],
        &[ObjectClass::Operation],
        verse_scope,
        verse_scope,
        support::EPOCH,
        fe_canonical_log::capability::Caveats::default(),
    );
    let authorized = wire_verifier
        .verify(
            &unrestricted_chain,
            Verb::Append.bit(),
            ObjectClass::Operation.bit(),
            &verse_scope,
        )
        .await
        .expect("wire authorize over the author's unrestricted chain");
    assert_eq!(authorized.leaf_principal, support::principal_of(&author));

    // A subscriber of a SIBLING petal must never receive this delta (§4.3.3): the committed
    // op's exact scope is `petal_a`; a subscription scoped to the disjoint `petal_b` does not
    // contain it.
    let petal_b = support::petal_scope(support::PETAL_B);
    let projection = ProjectionIdentity {
        materializer_id: "fe-test".to_owned(),
        version: 1,
    };
    let delta = CommittedDelta {
        op_id: claimed_op_id,
        branch_id,
        scope: petal_a,
        projection_identity: projection.clone(),
        cursor: support::StubBranchRegistry.issue_cursor(
            CursorTuple {
                verse_id: support::VERSE,
                branch_id,
                subscription_scope: petal_a,
                projection_identity: projection.clone(),
            },
            SortedFrontier::try_new([claimed_op_id])
                .expect("frontier")
                .commitment(),
            vec![1, 2, 3],
        ),
        change_summary: vec![9, 9],
    };
    let sibling_subscription = SubscriptionRecord {
        authorization_binding_id: [0x11; 16],
        branch_id,
        scope: petal_b,
        projection_identity: projection.clone(),
    };
    assert!(!is_delta_authorized_for_subscription(
        &sibling_subscription.scope,
        &delta.scope
    ));
    assert_eq!(
        CommitDeltaBody::for_subscription([0x22; 16], &sibling_subscription, generation, &delta),
        Err(WireError::ScopeNotAuthorized),
        "a sibling petal's subscriber must never receive this delta"
    );

    // Positive control: the same delta IS authorized for a verse-wide subscription.
    let verse_subscription = SubscriptionRecord {
        authorization_binding_id: [0x11; 16],
        branch_id,
        scope: verse_scope,
        projection_identity: projection.clone(),
    };
    assert!(is_delta_authorized_for_subscription(
        &verse_subscription.scope,
        &delta.scope
    ));
    assert!(
        CommitDeltaBody::for_subscription([0x22; 16], &verse_subscription, generation, &delta)
            .is_ok()
    );
}

/// Test 6's §10.2.1 delivery view, one question per source: SPEC-3 issuer Manager+ authority
/// from the same `ManagerView` the admission tests use, R2 device enrolment from the production
/// [`EnrolmentRegistry`], and a `decrypt` grant to exactly one recipient.
struct EnrolmentGatedDelivery {
    authority: support::ManagerView,
    enrolments: EnrolmentRegistry,
    decrypting_recipient: Principal,
}

impl IssuerAuthorityView for EnrolmentGatedDelivery {
    fn issuer_manager_authority(
        &self,
        issuer: &Principal,
        _capability: &CapabilityRef,
        scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorizationRecord {
        match self
            .authority
            .principal_is_manager_plus(issuer, scope, scope_epoch)
        {
            AuthorityState::ManagerPlus => AuthorizationRecord::Granted,
            AuthorityState::NotManagerPlus => AuthorizationRecord::Refused,
            AuthorityState::Unknown => AuthorizationRecord::Unknown,
        }
    }
}

impl DeviceEnrolmentView for EnrolmentGatedDelivery {
    fn device_enrolment(
        &self,
        recipient: &Principal,
        device_key: DevicePublicKey,
    ) -> AuthorizationRecord {
        self.enrolments.device_enrolment(recipient, device_key)
    }
}

impl DeliveryAuthorizationView for EnrolmentGatedDelivery {
    fn recipient_decrypt_authority(
        &self,
        recipient: &Principal,
        _scope: &Scope,
        _scope_epoch: u64,
    ) -> AuthorizationRecord {
        if *recipient == self.decrypting_recipient {
            AuthorizationRecord::Granted
        } else {
            AuthorizationRecord::Refused
        }
    }
}

/// The SPEC-6 §9.3 lane view: one header lane, one current key, one wrappable peer.
struct WrappableLane {
    lane: LaneKey,
    epoch: u64,
    key_id: Identifier32,
    wrappable: PeerIdentity,
}

impl RelayAuthorizationView for WrappableLane {
    fn current_scope_epoch(&self, lane: &LaneKey) -> Option<u64> {
        (*lane == self.lane).then_some(self.epoch)
    }

    fn current_key_id(&self, lane: &LaneKey) -> Option<Identifier32> {
        (*lane == self.lane).then_some(self.key_id)
    }

    fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
        false
    }

    fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
        false
    }

    fn may_wrap_scope_key_for_peer(&self, peer: &PeerIdentity, lane: &LaneKey, epoch: u64) -> bool {
        *lane == self.lane && epoch == self.epoch && *peer == self.wrappable
    }
}

#[test]
fn a_device_enrolment_gates_a_scope_key_wrap() {
    let manager = support::signing_key([0x70; 32]);
    let recipient_key = support::signing_key([0x71; 32]);
    let recipient = support::principal_of(&recipient_key);
    let device = DeviceSecretKey::from_bytes([0x0d; 32]);
    let verse_scope = support::verse_scope();
    let lane = LaneKey::Header {
        verse_id: support::VERSE,
    };
    let scope_key = SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH]);
    let lane_view = WrappableLane {
        lane,
        epoch: support::EPOCH,
        key_id: derive_key_id(&verse_scope, support::EPOCH, &scope_key).expect("key id"),
        wrappable: PeerIdentity(recipient.public_key),
    };

    // The recipient signs its own enrolment; the bytes enter the registry only through the
    // same verification door a replicated row uses.
    let enrolment_bytes = DeviceEnrolmentV1 {
        principal: recipient.clone(),
        device_key: device.public_key(),
        enrolled_at_ms: support::NOW_MS,
    }
    .sign(&recipient_key)
    .expect("sign enrolment")
    .encode_canonical()
    .expect("encode enrolment");
    let mut delivery = EnrolmentGatedDelivery {
        authority: support::ManagerView::at_epoch(
            support::principal_of(&manager),
            verse_scope,
            support::EPOCH,
        ),
        enrolments: EnrolmentRegistry::new(),
        decrypting_recipient: recipient.clone(),
    };
    assert!(delivery.enrolments.record(
        VerifiedEnrolmentArtifact::decode_and_verify(&enrolment_bytes).expect("enrolment verifies")
    ));

    // Enrolled device + Manager+ issuer + decrypting recipient: the wrap is issued, survives its
    // canonical byte form, and opens to the exact scope key on the enrolled device.
    let issuer = support::principal_of(&manager);
    let binding = RecipientDeviceBinding::resolve(
        &delivery,
        &recipient,
        device.public_key(),
        &verse_scope,
        support::EPOCH,
    )
    .expect("the registry enrols this device");
    let request = ScopeKeyWrapRequest {
        recipient_device_binding: &binding,
        scope_key: &scope_key,
        issuer: &issuer,
        issuer_capability: CapabilityRef {
            chain_id: support::hash(0x7c),
            scope_epoch: support::EPOCH,
        },
    };
    let wrap = issue_scope_key_wrap(&lane_view, &delivery, &lane, &manager, &request)
        .expect("an enrolled, authorized recipient receives a wrap");
    let received = ScopeKeyWrap::decode_canonical(&wrap.encode_canonical().expect("wrap bytes"))
        .expect("wrap decodes");
    assert_eq!(
        open_scope_key(&received, &device, &delivery).expect("the enrolled device opens it"),
        scope_key
    );

    // A non-Manager issuer is refused before any key material is touched.
    let outsider_key = support::signing_key([0x72; 32]);
    let outsider = support::principal_of(&outsider_key);
    assert_eq!(
        issue_scope_key_wrap(
            &lane_view,
            &delivery,
            &lane,
            &outsider_key,
            &ScopeKeyWrapRequest {
                issuer: &outsider,
                ..request.clone()
            },
        )
        .err(),
        Some(CryptoError::IssuerNotAuthorized {
            record: AuthorizationRecord::Refused
        })
    );

    // A device the recipient never enrolled cannot even be named in a wrap request.
    let unenrolled = DeviceSecretKey::from_bytes([0x0e; 32]);
    assert_eq!(
        RecipientDeviceBinding::resolve(
            &delivery,
            &recipient,
            unenrolled.public_key(),
            &verse_scope,
            support::EPOCH,
        ),
        Err(CryptoError::DeviceNotEnrolled {
            record: AuthorizationRecord::Unknown
        })
    );

    // The recipient's signed tombstone revokes the device, and replaying the old enrolment
    // afterwards (a late replica row) does not restore it.
    let revocation_bytes = DeviceEnrolmentRevocationV1 {
        principal: recipient.clone(),
        device_key: device.public_key(),
        revoked_at_ms: support::NOW_MS - 1,
    }
    .sign(&recipient_key)
    .expect("sign revocation")
    .encode_canonical()
    .expect("encode revocation");
    assert!(delivery.enrolments.record(
        VerifiedEnrolmentArtifact::decode_and_verify(&revocation_bytes)
            .expect("revocation verifies")
    ));
    assert!(!delivery.enrolments.record(
        VerifiedEnrolmentArtifact::decode_and_verify(&enrolment_bytes).expect("still verifies")
    ));
    assert_eq!(
        RecipientDeviceBinding::resolve(
            &delivery,
            &recipient,
            device.public_key(),
            &verse_scope,
            support::EPOCH,
        ),
        Err(CryptoError::DeviceNotEnrolled {
            record: AuthorizationRecord::Refused
        })
    );
}

/// Track `relay_record_warehouse_20261120` slice P2: custody receipts, the D-CL39 durability
/// floor, and the compaction gate that requires it (SPEC-9 §3-§4, SPEC-5 §5.2 rule 6).
mod durability_floor_journey {
    use fe_canonical_log::capability::Principal;
    use fe_canonical_log::cbor::{encode_canonical_checked, CborValue};
    use fe_canonical_log::checkpoint::{
        compaction_decision, sign_checkpoint_claim, verify_checkpoint_claim, AccessibleScopes,
        BootstrapCoverage, CheckpointClaimV1, CheckpointConsumerView, CheckpointVerification,
        CompactionDecision, CompactionRefusal, ManagerPlusAuthorizationView, ReplayRequest,
        ReplayVerifier, CHECKPOINT_VERSION,
    };
    use fe_canonical_log::envelope::{Author, CapabilityRef, Hash32, Hlc, Identifier32, Scope};
    use fe_canonical_log::frontier::SortedFrontier;
    use fe_canonical_log::retention::durability_floor::{
        DurabilityFloorEvidence, FloorShortfall, FloorSubject, FloorUnknownReason,
        ManifestCustody, ManifestFloorState, ReceiptDisqualification, RejectedReceipt,
        RelayQualificationView,
    };
    use fe_canonical_log::retention::tombstone::{BootstrapPath, TombstoneRetentionRecord};
    use fe_canonical_log::segment::custody_receipt::{
        issue_custody_receipt, verify_custody_receipt, CustodyReceiptError, CustodySet,
        RecordRelayBinding, CUSTODY_RECEIPT_DOMAIN, WAREHOUSE_FORMAT_EPOCH,
    };
    use fe_canonical_log::segment::store::{InMemorySealedArtifactStore, SealedArtifactStore};
    use fe_canonical_log::signing::sign_domain;

    use super::support::{hash, signing_key, VERSE};

    const OTHER_VERSE: Identifier32 = Identifier32([0x7f; 32]);
    const PROJECTION_ROOT: Hash32 = Hash32([0xd1; 32]);
    const AUTHORIZATION_ROOT: Hash32 = Hash32([0xd2; 32]);

    /// The durable record-relay list (Q6) and current `seed` holders (Q7) of [`VERSE`].
    struct RecordRelays {
        listed: Vec<String>,
        seeders: Vec<[u8; 32]>,
    }

    impl RelayQualificationView for RecordRelays {
        fn is_listed_record_relay(&self, verse_id: Identifier32, relay_did: &str) -> bool {
            verse_id == VERSE && self.listed.iter().any(|did| did == relay_did)
        }

        fn holds_current_segment_seed(&self, verse_id: Identifier32, relay: &Principal) -> bool {
            verse_id == VERSE && self.seeders.contains(&relay.public_key)
        }
    }

    struct ManagerSigned;

    impl ManagerPlusAuthorizationView for ManagerSigned {
        fn is_manager_plus_for_checkpoint(
            &self,
            verse_id: Identifier32,
            _signer: &Author,
            _capability: &CapabilityRef,
        ) -> bool {
            verse_id == VERSE
        }

        fn capability_permits_append_checkpoint(
            &self,
            verse_scope: &Scope,
            _signer: &Author,
            _capability: &CapabilityRef,
        ) -> bool {
            verse_scope.verse_id() == VERSE
        }

        fn authorization_view_root(
            &self,
            _verse_id: Identifier32,
            _branch_id: Identifier32,
            _frontier_commitment: Hash32,
        ) -> Option<Hash32> {
            Some(AUTHORIZATION_ROOT)
        }
    }

    struct Replays;

    impl ReplayVerifier for Replays {
        fn replay_projection_root(&self, _request: &ReplayRequest) -> Option<Hash32> {
            Some(PROJECTION_ROOT)
        }
    }

    fn principal(seed: u8) -> Principal {
        Principal::from_public_key(signing_key([seed; 32]).verifying_key().to_bytes())
    }

    fn receipt(
        seed: u8,
        binding: RecordRelayBinding,
        set: &CustodySet,
        store: &InMemorySealedArtifactStore,
        held_at_ms: u64,
    ) -> Vec<u8> {
        issue_custody_receipt(&signing_key([seed; 32]), &binding, set, store, held_at_ms)
            .expect("relay holds the whole set")
            .encode_canonical()
            .expect("receipt bytes")
    }

    /// A receipt an older binary signed before its §6 reset, hand-built from the SPEC-9 §3.1 table.
    fn pre_reset_receipt(seed: u8, set: &CustodySet, epoch: u64) -> Vec<u8> {
        let key = signing_key([seed; 32]);
        let relay = Principal::from_public_key(key.verifying_key().to_bytes());
        let custody_set_id = set.custody_set_id().expect("custody set id");
        let mut entries = vec![
            (CborValue::Uint(0), CborValue::Uint(1)),
            (CborValue::Uint(1), CborValue::Bytes(VERSE.0.to_vec())),
            (CborValue::Uint(2), CborValue::Bytes(set.manifest_id().0.to_vec())),
            (CborValue::Uint(3), CborValue::Bytes(custody_set_id.0.to_vec())),
            (CborValue::Uint(4), relay.to_cbor()),
            (CborValue::Uint(5), CborValue::Uint(17)),
            (CborValue::Uint(6), CborValue::Uint(epoch)),
        ];
        let unsigned =
            encode_canonical_checked(&CborValue::Map(entries.clone())).expect("unsigned");
        let signature = sign_domain(&key, CUSTODY_RECEIPT_DOMAIN, &unsigned);
        entries.push((CborValue::Uint(7), CborValue::Bytes(signature.to_vec())));
        let bytes = encode_canonical_checked(&CborValue::Map(entries)).expect("receipt");
        verify_custody_receipt(&bytes).expect("an old-epoch receipt still verifies (Q1)");
        bytes
    }

    fn verified_checkpoint(manifest_id: Hash32) -> CheckpointVerification {
        let selected = SortedFrontier::try_new([hash(0xe1)]).expect("frontier");
        let manager = signing_key([0x4d; 32]);
        let claim = CheckpointClaimV1 {
            checkpoint_version: CHECKPOINT_VERSION,
            verse_id: VERSE,
            branch_id: Identifier32([0xb1; 32]),
            frontier_commitment: selected.commitment(),
            segment_manifest_id: manifest_id,
            materializer_id: Identifier32([0x11; 32]),
            materializer_version: 1,
            projection_root_hash: PROJECTION_ROOT,
            authorization_view_root: AUTHORIZATION_ROOT,
            snapshot_manifest_id: None,
            signer: Author::from_public_key(manager.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: hash(0x60),
                scope_epoch: 1,
            },
            issued_hlc: Hlc::new(1_700_000_000_000, 1),
        };
        let bytes = sign_checkpoint_claim(&manager, &claim)
            .expect("sign")
            .encode_canonical()
            .expect("claim bytes");
        let required = [Scope::verse_wide(VERSE)];
        let accessible = AccessibleScopes::new([Scope::verse_wide(VERSE)]);
        let verification = verify_checkpoint_claim(
            &bytes,
            &CheckpointConsumerView {
                selected_frontier: &selected,
                segment_manifest_id: manifest_id,
                required_payload_scopes: &required,
                accessible_scopes: &accessible,
            },
            &ManagerSigned,
            &Replays,
        );
        assert!(verification.verified_checkpoint_id().is_some());
        verification
    }

    fn floor_refusal(shortfall: FloorShortfall) -> CompactionDecision {
        CompactionDecision::Refused(CompactionRefusal::DurabilityFloorNotMet(shortfall))
    }

    #[test]
    fn durability_floor_counts_distinct_qualifying_relays_and_gates_prune() {
        // Manifest M covers itself plus payload shard P; manifest M2 covers only itself.
        let manifest = b"sealed manifest M".to_vec();
        let shard = b"sealed payload shard P".to_vec();
        let second = b"sealed manifest M2".to_vec();
        let (m, p, m2) = (Hash32::of(&manifest), Hash32::of(&shard), Hash32::of(&second));
        let bulk = hash(0xb7);
        let mut store = InMemorySealedArtifactStore::new();
        for bytes in [&manifest, &shard, &second] {
            store.put_verified(Hash32::of(bytes), bytes).expect("held");
        }
        let set = CustodySet::for_manifest(
            m,
            [(p, shard.len() as u64), (m, manifest.len() as u64)],
        )
        .expect("custody set");
        let partial = CustodySet::for_manifest(m, [(m, manifest.len() as u64)]).expect("subset");
        let second_set =
            CustodySet::for_manifest(m2, [(m2, second.len() as u64)]).expect("second set");
        let current = RecordRelayBinding::current(VERSE);
        // A and B are listed seeders; N is listed without `seed`; U holds `seed` but is unlisted.
        let (a, b, n, u) = (0xa1u8, 0xb2u8, 0xc3u8, 0xd4u8);
        let view = RecordRelays {
            listed: [a, b, n].iter().map(|seed| principal(*seed).did).collect(),
            seeders: [a, b, u]
                .iter()
                .map(|seed| principal(*seed).public_key)
                .collect(),
        };

        // SPEC-9 §8 test 1: byte-exact round trip, receipt_id over the complete bytes.
        let first = receipt(a, current, &set, &store, 10);
        let verified = verify_custody_receipt(&first).expect("verifies");
        assert_eq!(verified.receipt_id(), Hash32::of(&first));
        assert_eq!(verified.manifest_id(), m);
        assert_eq!(verified.custody_set_id(), set.custody_set_id().expect("id"));
        assert_eq!(verified.warehouse_format_epoch(), WAREHOUSE_FORMAT_EPOCH);
        // A relay's decoded seed commitment carries the same members and id, but stays marked as
        // a peer's statement, so it can be receipted and never stands in for the walk (§2 rule 2).
        let received = CustodySet::decode_canonical(m, &set.encode_canonical().expect("set bytes"))
            .expect("seed commitment decodes");
        assert_eq!(received.members(), set.members());
        assert_eq!(received.custody_set_id(), set.custody_set_id());
        assert!(set.is_walked() && !received.is_walked());

        // SPEC-9 §8 test 7: no receipt without every member re-hashed at its exact length.
        let missing = CustodySet::for_manifest(m, [(m, manifest.len() as u64), (bulk, 4)])
            .expect("set naming an unheld artifact");
        let wrong_length =
            CustodySet::for_manifest(m, [(m, manifest.len() as u64 + 1)]).expect("set");
        let stale = RecordRelayBinding::stamped(VERSE, WAREHOUSE_FORMAT_EPOCH + 1);
        for (binding, bad_set, expected) in [
            (
                current,
                &missing,
                CustodyReceiptError::CustodyMemberMissing { artifact_id: bulk },
            ),
            (
                current,
                &wrong_length,
                CustodyReceiptError::CustodyMemberMismatch { artifact_id: m },
            ),
            (
                stale,
                &set,
                CustodyReceiptError::StaleFormatEpoch {
                    stamped: WAREHOUSE_FORMAT_EPOCH + 1,
                    current: WAREHOUSE_FORMAT_EPOCH,
                },
            ),
        ] {
            assert_eq!(
                issue_custody_receipt(&signing_key([a; 32]), &binding, bad_set, &store, 1),
                Err(expected)
            );
        }

        // AC-5.2 / SPEC-9 §8 test 4: every row but the first adds nothing to the count.
        let mut tampered = receipt(b, current, &set, &store, 11);
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        let rows: Vec<(&str, Vec<u8>, Option<ReceiptDisqualification>)> = vec![
            ("qualifying relay A", first.clone(), None),
            (
                "duplicate from A",
                receipt(a, current, &set, &store, 99),
                Some(ReceiptDisqualification::DuplicateRelay),
            ),
            (
                "listed relay without seed",
                receipt(n, current, &set, &store, 12),
                Some(ReceiptDisqualification::NoSeedCapability),
            ),
            (
                "seeder not on the list",
                receipt(u, current, &set, &store, 13),
                Some(ReceiptDisqualification::RelayNotListed),
            ),
            (
                "receipt naming another verse",
                receipt(b, RecordRelayBinding::current(OTHER_VERSE), &set, &store, 14),
                Some(ReceiptDisqualification::ForeignVerse),
            ),
            (
                "receipt for a manifest outside the release",
                receipt(b, current, &second_set, &store, 15),
                Some(ReceiptDisqualification::UnrelatedManifest),
            ),
            (
                "receipt attesting a different custody set",
                receipt(b, current, &partial, &store, 16),
                Some(ReceiptDisqualification::CustodySetMismatch),
            ),
            (
                "receipt from a stale format epoch",
                pre_reset_receipt(b, &set, WAREHOUSE_FORMAT_EPOCH + 1),
                Some(ReceiptDisqualification::FormatEpochMismatch),
            ),
            (
                "bad signature",
                tampered,
                Some(ReceiptDisqualification::NotVerified),
            ),
        ];
        let receipts: Vec<Vec<u8>> = rows.iter().map(|(_, bytes, _)| bytes.clone()).collect();
        let expected_rejections: Vec<RejectedReceipt> = rows
            .iter()
            .enumerate()
            .filter_map(|(receipt_index, (_, _, reason))| {
                reason.map(|reason| RejectedReceipt {
                    receipt_index,
                    reason,
                })
            })
            .collect();
        let manifests = [ManifestCustody::Computed(set.clone())];
        let released = [m, p];
        let subject = FloorSubject {
            verse_id: VERSE,
            peer_prune_after_floor: true,
            manifests: &manifests,
            released_artifacts: &released,
        };
        let one_relay =
            DurabilityFloorEvidence::assess(&subject, receipts.as_slice(), Some(2), &view);
        assert_eq!(one_relay.rejected_receipts(), expected_rejections.as_slice());
        assert_eq!(one_relay.manifests()[0].qualifying_relays, 1);
        assert_eq!(one_relay.manifests()[0].state, ManifestFloorState::BelowFloor);

        // A malicious seeder's list handed in as the assessor's set is unknown, never counted.
        let laundered = [ManifestCustody::Computed(received.clone())];
        let from_seed_commitment = DurabilityFloorEvidence::assess(
            &FloorSubject {
                manifests: &laundered,
                ..subject
            },
            receipts.as_slice(),
            Some(1),
            &view,
        );
        assert_eq!(
            from_seed_commitment.manifests()[0].state,
            ManifestFloorState::Unknown(FloorUnknownReason::CustodySetUnavailable)
        );
        assert!(!from_seed_commitment.is_met());

        // AC-5.1: R = 2 with one qualifying relay refuses; a second distinct relay releases.
        let checkpoint = verified_checkpoint(m);
        let complete = BootstrapCoverage {
            every_head_has_retained_bootstrap_path: true,
        };
        assert_eq!(
            compaction_decision(&checkpoint, &one_relay, complete, &[]),
            floor_refusal(FloorShortfall::BelowFloor {
                artifact_id: m,
                manifest_id: m,
                qualifying_relays: 1,
                required: 2,
            })
        );
        let mut with_b = receipts.clone();
        with_b.push(receipt(b, current, &set, &store, 20));
        let met = DurabilityFloorEvidence::assess(&subject, with_b.as_slice(), Some(2), &view);
        assert!(met.is_met());
        assert_eq!(
            compaction_decision(&checkpoint, &met, complete, &[]),
            CompactionDecision::Permitted {
                checkpoint_id: checkpoint.verified_checkpoint_id().expect("verified"),
                // The permit carries exactly the assessed release, ascending, never wider.
                released_artifacts: {
                    let mut expected = released.to_vec();
                    expected.sort();
                    expected
                },
            }
        );

        // AC-5.3: a met floor never releases a tombstone no-resurrection still needs.
        let unproven = TombstoneRetentionRecord {
            tombstone_op_id: hash(0x3a),
            scope: Scope::verse_wide(VERSE),
            suppressed_object_id: Identifier32([0x3c; 32]),
            retained_bootstrap_paths: vec![BootstrapPath {
                path_id: hash(0x3b),
                carries_suppression_proof: false,
            }],
        };
        assert_eq!(
            compaction_decision(&checkpoint, &met, complete, &[unproven]),
            CompactionDecision::Refused(CompactionRefusal::UnprovedSuppressionEvidence {
                path_id: hash(0x3b),
            })
        );

        // Release-shape rows: each refuses with its own shortfall over the same met receipts.
        let two_manifests = [
            ManifestCustody::Computed(set.clone()),
            ManifestCustody::Computed(second_set.clone()),
        ];
        let unknown_set = [ManifestCustody::Unavailable { manifest_id: m }];
        let with_bulk = [m, bulk];
        let both_manifests = [m, m2];
        let release_rows: Vec<(&str, FloorSubject<'_>, Option<u64>, FloorShortfall)> = vec![
            ("R missing", subject, None, FloorShortfall::ReplicationFactorUnknown),
            ("R zero", subject, Some(0), FloorShortfall::ReplicationFactorUnknown),
            (
                "peer_prune_after_floor off",
                FloorSubject {
                    peer_prune_after_floor: false,
                    ..subject
                },
                Some(2),
                FloorShortfall::PeerPruneAfterFloorOff,
            ),
            (
                "release covering nothing",
                FloorSubject {
                    released_artifacts: &[],
                    ..subject
                },
                Some(2),
                FloorShortfall::NothingReleased,
            ),
            (
                "bulk artifact in no custody set",
                FloorSubject {
                    released_artifacts: &with_bulk,
                    ..subject
                },
                Some(2),
                FloorShortfall::ArtifactNotCovered { artifact_id: bulk },
            ),
            (
                "receipts for M never cover M2",
                FloorSubject {
                    manifests: &two_manifests,
                    released_artifacts: &both_manifests,
                    ..subject
                },
                Some(2),
                // B's one M2 receipt counts for M2 only; M's two receipts never pool into it.
                FloorShortfall::BelowFloor {
                    artifact_id: m2,
                    manifest_id: m2,
                    qualifying_relays: 1,
                    required: 2,
                },
            ),
            (
                "assessor without the custody set",
                FloorSubject {
                    manifests: &unknown_set,
                    ..subject
                },
                Some(2),
                FloorShortfall::ArtifactNotCovered { artifact_id: m },
            ),
        ];
        for (row, row_subject, r, shortfall) in release_rows {
            let evidence =
                DurabilityFloorEvidence::assess(&row_subject, with_b.as_slice(), r, &view);
            assert_eq!(
                compaction_decision(&checkpoint, &evidence, complete, &[]),
                floor_refusal(shortfall),
                "{row}"
            );
        }

        // SPEC-9 §8 test 5: without the custody set the floor is unknown, never met.
        let blind = DurabilityFloorEvidence::assess(
            &FloorSubject {
                manifests: &unknown_set,
                ..subject
            },
            with_b.as_slice(),
            Some(2),
            &view,
        );
        assert_eq!(
            blind.manifests()[0].state,
            ManifestFloorState::Unknown(FloorUnknownReason::CustodySetUnavailable)
        );
    }
}
