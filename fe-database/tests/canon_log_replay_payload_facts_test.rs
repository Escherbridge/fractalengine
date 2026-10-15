//! CDL-SPEC10 B2b flows: replay re-derives a kind-5 operation's OWN payload facts through the
//! durable verifier (W4-7 ciphertext store + scope-key custody + `RegisteredSchemaSet`), and a
//! crypto-shredded scope key leaves the position pending while the pre-shred row is kept.
//! Real DB thread over an on-disk store, closed (thread joined) and reopened (C8: one SurrealKV
//! open per process). See `fe-database/src/canon_log/AGENTS.md` §replay-payload-facts.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    Caveats, Chain, ObjectClass, ObjectClassSet, Principal, RevalidationGate, UnsignedCertificate,
    Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::compose::{Clock, ComposedCandidateVerifier, SystemClock};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::identity::{ObservationSource, ProjectionIdentity, SourceClass};
use fe_canonical_log::observation::{
    node_anchor, observation_batch_schema_hash, validate_observation_batch_plaintext, HlcRange,
    ObservationBatchMaterializer, ObservationBatchPayload, ObservationBatchPayloadParts,
    ObservationBatchReference, SensorTimeRange, HEXON_FORMAT_VERSION,
    OBSERVATION_BATCH_OPERATION_KIND,
};
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::signing::sign_envelope;
use fe_database::canon_log::admission::NoPrecondition;
use fe_database::canon_log::append_store::{SurrealVerifiedLogStore, SurrealVerseDagView};
use fe_database::canon_log::apply_marker_store::{cbor_to_json, SurrealApplyMarkerStore};
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView, SurrealScopeEpochStore,
};
use fe_database::canon_log::chain_store::ChainStore;
use fe_database::canon_log::rebuild::{
    rebuild_projection_with_facts, ProjectionSurface, RebuildError, RebuildRequest, RebuildSource,
};
use fe_database::canon_log::replay::{
    replay_to_frontier, replay_to_frontier_with_facts, PayloadFactsGap, PendingReason,
    ReplayFactsLookup, ReplayPayloadFacts, VerifierReplayFacts,
};
use fe_database::canon_log::scope_key_custody::SecretStoreScopeKeyCustody;
use fe_database::canon_log::verifier::{
    admit_and_append_durably, durable_candidate_verifier, DurableAdmission,
    DurableCandidateVerifier,
};
use fe_database::{repo::Db, spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_identity::{InMemoryBackend, SecretStore};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};

const TIMEOUT: Duration = Duration::from_secs(20);
const VERSE: Identifier32 = Identifier32([0x51; 32]);
const PETAL: Identifier32 = Identifier32([0x52; 32]);
const BRANCH: Identifier32 = Identifier32([0x53; 32]);
/// The one `issuer_authority_id` this deployment recognises (§interim-manager-authority).
const ANCHOR: Hash32 = Hash32([0xb7; 32]);
const MANAGER: u8 = 0x21;
const AUTHOR: u8 = 0x22;
const SCOPE_KEY_BYTES: [u8; 32] = [0x6c; 32];
/// The epoch the verse sits at after the seeded bump.
const EPOCH: u64 = 1;
const HLC_WALL_MS: u64 = 1_700_000_000_000;
const NODE_A: &str = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
const NODE_B: &str = "01J9Z3K4M5N6P7Q8R9S0T1V2W4";

/// A DB thread over a store directory the TEST owns plus its shared handle. `Drop` releases the
/// handle, closes the command channel and JOINS the thread, so the SurrealKV lock is free before
/// the next open (C8; `fe-database/tests/node_view_test.rs`).
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    _res_rx: crossbeam::channel::Receiver<DbResult>,
    db: Option<Db>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn open(dir: &Path) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let db_path = dir.join("fe.db").to_string_lossy().to_string();
        let DbThread {
            join_handle,
            shared_db,
        } = spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            blob_store,
            None,
            None,
            None,
            None,
            Some(db_path),
            None,
        )
        .expect("spawn db thread");
        let started = res_rx.recv_timeout(TIMEOUT).expect("first DB result");
        assert!(matches!(started, DbResult::Started), "got {started:?}");
        Self {
            cmd_tx: Some(cmd_tx),
            _res_rx: res_rx,
            db: Some(shared_db),
            join: Some(join_handle),
        }
    }

    fn db(&self) -> Db {
        self.db.clone().expect("open store")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.db.take());
        drop(self.cmd_tx.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime")
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public_key(seed: u8) -> [u8; 32] {
    signing_key(seed).verifying_key().to_bytes()
}

fn principal(seed: u8) -> Principal {
    Principal::from_public_key(public_key(seed))
}

fn verse_scope() -> Scope {
    Scope::verse_wide(VERSE)
}

fn petal_scope() -> Scope {
    Scope::new(VERSE, Some(PETAL), None).expect("petal scope")
}

fn verse_key() -> ScopeKeyId {
    ScopeKeyId {
        scope: verse_scope(),
        epoch: EPOCH,
    }
}

/// The verse-wide `append/op` chain the Manager issues [`AUTHOR`], valid for an hour around the
/// real clock the durable verifier reads.
fn author_chain() -> Vec<u8> {
    let now = SystemClock.now_ms();
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER),
        audience: principal(AUTHOR),
        parent_certificate_id: None,
        issuer_authority_id: ANCHOR,
        grant_scope: verse_scope(),
        epoch_scope: verse_scope(),
        scope_epoch: EPOCH,
        verbs: VerbSet::from_verbs([Verb::Append]).expect("verbs"),
        object_classes: ObjectClassSet::from_classes([ObjectClass::Operation]).expect("classes"),
        not_before_ms: now - 60_000,
        not_after_ms: now + 3_600_000,
        delegation_depth: 0,
        caveats: Caveats::default(),
    }
    .sign(&signing_key(MANAGER))
    .expect("sign certificate");
    Chain::from_certificates(&[certificate])
        .expect("chain")
        .encode_canonical()
        .expect("encode chain")
}

/// A no-payload verse-scoped operation signed by the Manager, appended through the received door.
async fn append_manager_operation(
    store: &SurrealVerifiedLogStore,
    kind: OperationKind,
    parents: Vec<Hash32>,
    hlc: Hlc,
) -> Hash32 {
    let unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: kind.to_u16(),
        scope: verse_scope(),
        author: Author::from_public_key(public_key(MANAGER)),
        capability: CapabilityRef {
            chain_id: Hash32([0x33; 32]),
            scope_epoch: 0,
        },
        schema_hash: Hash32([0x44; 32]),
        branch_id: BRANCH,
        parents,
        hlc,
        payload: PayloadRef::empty(),
    };
    let bytes = sign_envelope(&signing_key(MANAGER), &unsigned)
        .expect("sign")
        .encode_canonical()
        .expect("encode");
    let op_id = Hash32::of(&bytes);
    store
        .append_received(op_id, &bytes)
        .await
        .expect("append manager operation");
    op_id
}

/// The Manager role, a branch genesis, the verse bumped 0 -> [`EPOCH`], the author's chain, and
/// the verse scope key in durable custody. Returns the bump, which every batch descends from.
async fn seed_verse(db: &Db, secrets: &Arc<dyn SecretStore>, author_chain: &[u8]) -> Hash32 {
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
        .bind(("did", principal(MANAGER).did))
        .bind(("scope", interim_authority_scope_key(&verse_scope())))
        .bind(("role", "manager".to_string()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");

    // A real root: replay walks the closure, and an absent ancestor would halt every head.
    let store = SurrealVerifiedLogStore::new(db.clone());
    let genesis = append_manager_operation(
        &store,
        OperationKind::BranchGenesis,
        Vec::new(),
        Hlc::new(HLC_WALL_MS - 1, 0),
    )
    .await;
    let bump = append_manager_operation(
        &store,
        OperationKind::ScopeEpochBump,
        vec![genesis],
        Hlc::new(HLC_WALL_MS, 0),
    )
    .await;
    let view = DurableAuthorizationView::load(db, Some(ANCHOR))
        .await
        .expect("view");
    SurrealScopeEpochStore::new(db.clone())
        .admit_epoch_bump(&store, &view, bump, &mut RevalidationGate::new())
        .await
        .expect("the Manager's bump admits");

    ChainStore::load(db)
        .await
        .expect("chain store")
        .put_verified_chain(author_chain)
        .await
        .expect("put the author's chain");
    SecretStoreScopeKeyCustody::load(db, Arc::clone(secrets))
        .await
        .expect("custody")
        .take_custody(verse_key(), SealingKey::from_bytes(SCOPE_KEY_BYTES))
        .await
        .expect("take custody of the verse key");
    bump
}

/// A valid one-anchor batch; `seed` varies the artifact ids so two batches differ.
fn batch_payload(seed: u8, count: u64) -> ObservationBatchPayload {
    let source = ObservationSource::new(
        SourceClass::Device,
        ObservationSource::source_id_for(SourceClass::Device, "thermo-1"),
        public_key(AUTHOR),
        Some("acme.thermo/frame@1".to_string()),
        None,
    )
    .expect("device source");
    ObservationBatchPayload::new(
        ObservationBatchPayloadParts {
            hexon_artifact_id: Hash32([seed; 32]),
            hexon_stored_length: 4_096,
            hlc_range: HlcRange {
                min: Hlc::new(HLC_WALL_MS - 60_000, 0),
                max: Hlc::new(HLC_WALL_MS - 1, 2),
            },
            sensor_time_range: SensorTimeRange {
                min_ms: 1_699_999_000_000,
                max_ms: 1_699_999_900_000,
            },
            stats_hash: Hash32([seed.wrapping_add(1); 32]),
            count,
            anchors: vec![
                node_anchor(NODE_B).expect("ascii id"),
                node_anchor(NODE_A).expect("ascii id"),
            ],
            asset_refs: vec![Hash32([0xa1; 32])],
            sources: vec![source],
            hexon_format_version: HEXON_FORMAT_VERSION,
            stats_stored_length: 512,
        }
        .normalized(),
    )
    .expect("valid payload")
}

struct Sealed {
    bytes: Vec<u8>,
    ciphertext: Vec<u8>,
    op_id: Hash32,
}

/// A petal-scoped kind-5 operation by [`AUTHOR`], its payload sealed under the verse key (the
/// enclosing scope the verifier's key lookup reaches) with the SPEC-1 §5.2 AAD.
fn sealed_batch(
    chain_id: Hash32,
    parents: Vec<Hash32>,
    payload: &ObservationBatchPayload,
    counter: u32,
) -> Sealed {
    let key = SealingKey::from_bytes(SCOPE_KEY_BYTES);
    let key_id = derive_key_id(&verse_scope(), EPOCH, &key).expect("key id");
    let nonce = FreshNonce::draw(key_id).expect("nonce");
    let mut unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: OBSERVATION_BATCH_OPERATION_KIND,
        scope: petal_scope(),
        author: Author::from_public_key(public_key(AUTHOR)),
        capability: CapabilityRef {
            chain_id,
            scope_epoch: EPOCH,
        },
        schema_hash: observation_batch_schema_hash(),
        branch_id: BRANCH,
        parents,
        hlc: Hlc::new(HLC_WALL_MS, counter),
        payload: PayloadRef {
            encryption: Some(nonce.params()),
            ..PayloadRef::empty()
        },
    };
    let aad = payload_aad_preimage(&unsigned).expect("aad");
    let plaintext = payload.encode_canonical().expect("payload bytes");
    let sealed = seal(&key, nonce, &aad, &plaintext).expect("seal");
    unsigned.payload = sealed.payload_ref();
    let bytes = sign_envelope(&signing_key(AUTHOR), &unsigned)
        .expect("sign")
        .encode_canonical()
        .expect("encode");
    Sealed {
        op_id: Hash32::of(&bytes),
        bytes,
        ciphertext: sealed.ciphertext,
    }
}

/// The durable verifier with the kind-5 schema registered by a call, as CDL-SPEC10 B3 does.
async fn verifier(db: &Db, secrets: &Arc<dyn SecretStore>) -> DurableCandidateVerifier {
    let loaded = durable_candidate_verifier(db, Arc::clone(secrets), Some(ANCHOR))
        .await
        .expect("durable verifier");
    loaded
        .verifier
        .parts()
        .schemas
        .register(
            OBSERVATION_BATCH_OPERATION_KIND,
            observation_batch_schema_hash(),
            validate_observation_batch_plaintext,
        )
        .expect("register the observation_batch schema");
    loaded.verifier
}

async fn admit(
    verifier: &DurableCandidateVerifier,
    db: &Db,
    operation: &Sealed,
) -> DurableAdmission {
    admit_and_append_durably(
        verifier,
        &SurrealVerifiedLogStore::new(db.clone()),
        &NoPrecondition,
        &operation.bytes,
        Some(&operation.ciphertext),
    )
    .await
    .expect("the kind-5 operation admits")
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity::new(ObservationBatchMaterializer::version(), BRANCH)
}

fn frontier(head: Hash32) -> SortedFrontier {
    SortedFrontier::try_new([head]).expect("frontier")
}

/// Every `observation_batch` reference row, without its storage record id, by `op_id`.
async fn reference_rows(db: &Db) -> Vec<serde_json::Value> {
    let mut response = db
        .query("SELECT * OMIT id FROM observation_batch ORDER BY op_id ASC")
        .await
        .expect("reference rows")
        .check()
        .expect("reference rows check");
    response.take(0).expect("reference rows take")
}

async fn disposition(db: &Db, op_id: Hash32) -> Option<String> {
    SurrealApplyMarkerStore::new(db.clone())
        .disposition_of(&identity(), op_id)
        .await
        .expect("marker read")
}

/// The reference projection's own rows, for the SPEC-4 §6 rebuild seam.
struct ReferenceSurface {
    db: Db,
}

#[async_trait]
impl ProjectionSurface for ReferenceSurface {
    async fn reset_to_empty(&self, _identity: &ProjectionIdentity) -> anyhow::Result<()> {
        self.db
            .query("DELETE FROM observation_batch")
            .await?
            .check()?;
        Ok(())
    }

    async fn projection_root_hash(&self, _identity: &ProjectionIdentity) -> anyhow::Result<Hash32> {
        let rows = reference_rows(&self.db).await;
        Ok(Hash32::of(&serde_json::to_vec(&rows)?))
    }
}

/// SPEC-10 §4.4 / B2b: a kind-5 operation replayed after a restart re-derives exactly the facts
/// it had live, through the durable verifier, and rebuilds the same reference row; the legacy
/// `None` wrapper still leaves it pending as before.
#[test]
fn replay_rederives_payload_facts_through_custody() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = author_chain();
    let chain_id = Hash32::of(&chain);

    let (batch, live_facts, live_row) = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            let bump = seed_verse(&db, &secrets, &chain).await;
            let verifier = verifier(&db, &secrets).await;

            let payload = batch_payload(0x81, 3);
            let batch = sealed_batch(chain_id, vec![bump], &payload, 1);
            let admitted = admit(&verifier, &db, &batch).await;
            let live_facts = admitted.append.payload_facts.expect("live payload facts");
            let live_row = cbor_to_json(
                &ObservationBatchReference::from_admitted(
                    &admitted.append.meta,
                    ObservationBatchPayload::from_cbor(&live_facts.fields).expect("payload"),
                )
                .to_row(),
            )
            .expect("row json");

            // The legacy wrapper is unchanged: no facts, so the position stays pending on the
            // operation's own payload ciphertext and nothing is projected.
            let store = SurrealVerifiedLogStore::new(db.clone());
            let markers = SurrealApplyMarkerStore::new(db.clone());
            let legacy = replay_to_frontier(
                &store,
                &markers,
                &ObservationBatchMaterializer,
                &identity(),
                &frontier(batch.op_id),
            )
            .await
            .expect("legacy replay");
            assert_eq!(
                legacy.pending,
                vec![(
                    batch.op_id,
                    PendingReason::ReferencedArtifactUnavailable {
                        artifact_id: Hash32::of(&batch.ciphertext),
                    }
                )]
            );
            assert!(reference_rows(&db).await.is_empty());
            assert_eq!(disposition(&db, batch.op_id).await, None);
            (batch, live_facts, live_row)
        })
    }; // DB thread joined, store closed

    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let verifier = verifier(&db, &secrets).await;
        let facts = VerifierReplayFacts::new(&verifier);

        // Custody + the retained ciphertext + the registered schema re-derive the live facts.
        assert_eq!(
            facts.payload_facts(batch.op_id, &batch.bytes).await,
            ReplayFactsLookup::Derived(live_facts)
        );

        let store = SurrealVerifiedLogStore::new(db.clone());
        let markers = SurrealApplyMarkerStore::new(db.clone());
        let mut dag_view = SurrealVerseDagView::load(&store, VERSE)
            .await
            .expect("dag view");
        let report = rebuild_projection_with_facts(
            &store,
            &markers,
            &ObservationBatchMaterializer,
            &ReferenceSurface { db: db.clone() },
            &mut dag_view,
            RebuildRequest {
                identity: &identity(),
                frontier: &frontier(batch.op_id),
                checkpoint: None,
            },
            &facts,
        )
        .await
        .expect("rebuild");

        assert_eq!(report.source, RebuildSource::EmptyState);
        assert!(report.replay.is_complete(), "{:?}", report.replay);
        assert!(report.replay_verified_recorded);
        assert_eq!(report.replay.applied, vec![batch.op_id]);
        assert_eq!(reference_rows(&db).await, vec![live_row]);
        assert_eq!(
            disposition(&db, batch.op_id).await.as_deref(),
            Some("applied")
        );
    });
}

/// B2b default (RUNBOOK §4, AC-33): after a crypto-shred the row projected before it is kept,
/// an operation not yet reduced stays pending under its own reason (never `Excluded`), and a
/// from-empty rebuild that would discard the unrederivable row is withheld — across a restart.
#[test]
fn a_shredded_scope_key_leaves_replay_pending_and_keeps_the_pre_shred_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = author_chain();
    let chain_id = Hash32::of(&chain);
    let shredded_pending = |later: &Sealed| {
        vec![(
            later.op_id,
            PendingReason::PayloadFactsUnavailable {
                ciphertext_hash: Hash32::of(&later.ciphertext),
                gap: PayloadFactsGap::ScopeKeyUnavailable,
            },
        )]
    };

    let (first, later, pre_shred_rows) = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            let bump = seed_verse(&db, &secrets, &chain).await;
            let verifier = verifier(&db, &secrets).await;
            let store = SurrealVerifiedLogStore::new(db.clone());
            let markers = SurrealApplyMarkerStore::new(db.clone());

            let first = sealed_batch(chain_id, vec![bump], &batch_payload(0x81, 3), 1);
            admit(&verifier, &db, &first).await;
            let later = sealed_batch(chain_id, vec![first.op_id], &batch_payload(0x91, 2), 2);
            admit(&verifier, &db, &later).await;

            // Only `first` is projected while the key is held.
            let projected = replay_to_frontier_with_facts(
                &store,
                &markers,
                &ObservationBatchMaterializer,
                &identity(),
                &frontier(first.op_id),
                &VerifierReplayFacts::new(&verifier),
            )
            .await
            .expect("replay before the shred");
            assert!(projected.applied.contains(&first.op_id), "{projected:?}");
            let pre_shred_rows = reference_rows(&db).await;
            assert_eq!(pre_shred_rows.len(), 1);

            // Shred through the verifier's own custody (SPEC-5 §5.5.4), then rebuild it.
            let mut parts = verifier.into_parts();
            parts
                .scope_keys
                .shred(&verse_key(), Hlc::new(HLC_WALL_MS, 9))
                .await
                .expect("shred");
            let verifier = ComposedCandidateVerifier::new(parts);
            let facts = VerifierReplayFacts::new(&verifier);
            assert_eq!(
                facts.payload_facts(later.op_id, &later.bytes).await,
                ReplayFactsLookup::Unavailable {
                    ciphertext_hash: Hash32::of(&later.ciphertext),
                    gap: PayloadFactsGap::ScopeKeyUnavailable,
                }
            );

            let after = replay_to_frontier_with_facts(
                &store,
                &markers,
                &ObservationBatchMaterializer,
                &identity(),
                &frontier(later.op_id),
                &facts,
            )
            .await
            .expect("replay after the shred");
            assert_eq!(after.pending, shredded_pending(&later));
            assert!(after.already_processed.contains(&first.op_id));
            assert!(after.excluded.is_empty(), "never Excluded: {after:?}");
            assert!(!after.is_complete());
            assert_eq!(disposition(&db, later.op_id).await, None);
            assert_eq!(reference_rows(&db).await, pre_shred_rows);

            // A from-empty rebuild would destroy `first`'s row for good: withheld, untouched.
            let mut dag_view = SurrealVerseDagView::load(&store, VERSE)
                .await
                .expect("dag view");
            let withheld = rebuild_projection_with_facts(
                &store,
                &markers,
                &ObservationBatchMaterializer,
                &ReferenceSurface { db: db.clone() },
                &mut dag_view,
                RebuildRequest {
                    identity: &identity(),
                    frontier: &frontier(later.op_id),
                    checkpoint: None,
                },
                &facts,
            )
            .await;
            match withheld {
                Err(RebuildError::WouldDiscardUnrederivableRows { unrederivable }) => {
                    assert_eq!(
                        unrederivable,
                        vec![(first.op_id, PayloadFactsGap::ScopeKeyUnavailable)]
                    );
                }
                other => panic!("expected the rebuild to be withheld, got {other:?}"),
            }
            assert_eq!(reference_rows(&db).await, pre_shred_rows);
            assert_eq!(
                disposition(&db, first.op_id).await.as_deref(),
                Some("applied")
            );
            (first, later, pre_shred_rows)
        })
    }; // DB thread joined, store closed

    // The shred is durable: a fresh verifier never reloads the key, and the outcome holds.
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let verifier = verifier(&db, &secrets).await;
        let store = SurrealVerifiedLogStore::new(db.clone());
        let markers = SurrealApplyMarkerStore::new(db.clone());
        let after_restart = replay_to_frontier_with_facts(
            &store,
            &markers,
            &ObservationBatchMaterializer,
            &identity(),
            &frontier(later.op_id),
            &VerifierReplayFacts::new(&verifier),
        )
        .await
        .expect("replay after restart");
        assert_eq!(after_restart.pending, shredded_pending(&later));
        assert!(after_restart.already_processed.contains(&first.op_id));
        assert!(after_restart.excluded.is_empty());
        assert_eq!(reference_rows(&db).await, pre_shred_rows);
    });
}
