//! CDL-W4 slice W4-5: cross-spec integration flows over the crate's production `compose` types
//! (`ComposedCandidateVerifier`, `ChainCapabilityVerifier`, `VerifyingCommitPipeline`,
//! `SealedProofEnvironment` + `ByteStoreSealedArtifactStore`), public API only, in-memory seams,
//! no DB and no network. See `conductor/tracks/canonical_data_log_20260808/campaign-plan-w4-20261013.md`
//! slice W4-5.
//!
//! Test 6 (`a_device_enrolment_gates_a_scope_key_wrap`) is named in the plan but explicitly
//! deferred to W4-6's executor ("placed here only after W4-6 lands") -- `DeviceEnrolmentView`
//! has no production implementor yet (`crypto/AGENTS.md:29` still reads "Not built"), so there
//! is nothing to compose it over.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use fe_canonical_log::capability::{ObjectClass, Verb};
use fe_canonical_log::cbor::{decode_canonical, encode_canonical, CborValue};
use fe_canonical_log::checkpoint::{sign_checkpoint_claim, CheckpointClaimV1, CHECKPOINT_VERSION};
use fe_canonical_log::compose::{
    ByteStoreSealedArtifactStore, ChainCapabilityVerifier, SealedProofEnvironment,
    SignedCheckpointProofView, VerifyingCommitPipeline,
};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
use fe_canonical_log::crypto::artifact_aad::StoredArtifactAad;
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::{InMemoryScopeKeyStore, LaneScopeKeyResolver};
use fe_canonical_log::envelope::{Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef};
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
use fe_canonical_log::segment::store::SealedArtifactStore;
use fe_canonical_log::segment::SegmentError;
use fe_canonical_log::wire::commit::{
    handle_commit_submit, is_delta_authorized_for_subscription, CommitAckState, CommitDeltaBody,
    CommitSubmitBody,
};
use fe_canonical_log::wire::cursor::{
    BranchRegistry, CommittedDelta, CursorTuple, ProjectionIdentity,
};
use fe_canonical_log::wire::error::WireError;
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

    // The SAME capability chain that gated SPEC-4 admission above also gates a SPEC-7 wire
    // `authorize` for this operation's own author: one capability artifact, two specs.
    let wire_verifier = ChainCapabilityVerifier::for_requester(
        support::principal_of(&author),
        support::ManagerView::at_epoch(support::principal_of(&author), verse_scope, support::EPOCH),
        support::FixedClock(support::NOW_MS),
    );
    let authorized = wire_verifier
        .verify(
            &chain_bytes,
            Verb::Append.bit(),
            ObjectClass::Operation.bit(),
            &verse_scope,
        )
        .await
        .expect("wire authorize over the same chain");
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
