//! CDL-SPEC10 B3 flows: the SPEC-10 observation plane's storage half end to end — door-gated
//! window membership, the shadow sealer, the kind-5 reference projection, the hydrator, the
//! parity comparison, and the G6 analytics read. Real DB thread over an on-disk store per test,
//! closed (thread joined) before any reopen (C8). Conformance names: SPEC-10 §10 tests 10-16, 19.
//! See `fe-database/src/canon_log/AGENTS.md` §observation.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    Caveats, Chain, ObjectClass, ObjectClassSet, Principal, RevalidationGate, UnsignedCertificate,
    Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::compose::{Clock, ContentByteStore, SystemClock};
use fe_canonical_log::crypto::aead::SealingKey;
use fe_canonical_log::did_key::public_key_to_did;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::identity::{ObservationSource, SourceClass};
use fe_canonical_log::observation::{
    node_anchor, HlcRange, ObservationBatchMaterializer, ObservationBatchPayload,
    ObservationBatchPayloadParts, SensorTimeRange,
};
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::segment::artifact::NonceLedger;
use fe_canonical_log::signing::sign_envelope;
use fe_database::canon_log::admission::NoPrecondition;
use fe_database::canon_log::append_store::SurrealVerifiedLogStore;
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView, SurrealScopeEpochStore,
};
use fe_database::canon_log::chain_store::ChainStore;
use fe_database::canon_log::observation::{
    branch_heads, compare_catalog_with_legacy, hydrate_observation_catalog,
    observation_reference_rows, observation_source, read_observation_batches,
    rebuild_observation_references, seal_shard, sealing_context, sign_observation_batch,
    AnalyticsReadBase, AnalyticsReadError, HydrationOutcome, HydrationReport, HydrationRequest,
    ObservationPlaneError, ObservationSealer, ObservationSealerConfig, ObservationTable,
    PayloadRefRow, ReadingRow, SealPolicy,
};
use fe_database::canon_log::rebuild::RebuildSource;
use fe_database::canon_log::scope_key_custody::SecretStoreScopeKeyCustody;
use fe_database::canon_log::verifier::{
    admit_and_append_durably, durable_observation_verifier, DurableCandidateVerifier,
};
use fe_database::handlers::device_payload::{insert_device_payloads, register_format};
use fe_database::handlers::iot_reading::insert_readings_with_replication;
use fe_database::handlers::replicated_row::apply_replicated_row_handler;
use fe_database::migration::mode::{MigrationMode, MIGRATION_MODE_ENV_VAR};
use fe_database::{repo::Db, spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_identity::{InMemoryBackend, SecretStore};
use fe_query::columnar::observation::write_observation_hexon;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{
    DbCommand, DbResult, DevicePayloadInput, IotReadingInput, ReplicatedRowOutcome,
};
use fe_sdk::device_format::DeviceFormatDescriptor;
use fe_sdk::legacy_identifier::{legacy_identifier, LegacyTable};

const TIMEOUT: Duration = Duration::from_secs(20);
const VERSE_ID: &str = "verse-obs";
const FRACTAL_ID: &str = "fractal-obs";
const PETAL_ID: &str = "petal-obs";
const NODE_A: &str = "node-a";
const NODE_B: &str = "node-b";
/// A third anchor (RP-0): `seed_legacy` seeds only `NODE_A`/`NODE_B`.
const NODE_C: &str = "node-c";
const FORMAT_ID: &str = "acme.thermo/frame@1";
const FORMAT_MIME: &str = "application/octet-stream";
const BRANCH: Identifier32 = Identifier32([0x53; 32]);
/// The one `issuer_authority_id` this deployment recognises (§interim-manager-authority).
const ANCHOR: Hash32 = Hash32([0xb7; 32]);
const MANAGER: u8 = 0x31;
/// The node key: batch author and hydrator.
const NODE: u8 = 0x32;
/// The authenticated device behind every door accept.
const DEVICE: u8 = 0x33;
const EPOCH: u64 = 1;
const PETAL_KEY_BYTES: [u8; 32] = [0x6d; 32];
const HLC_WALL_MS: u64 = 1_700_000_000_000;

// ---------------------------------------------------------------------------
// Harness: one DB thread per store, dropped AND joined before a reopen (C8)
// ---------------------------------------------------------------------------

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

/// `FE_MIGRATION_MODE` is process-global: every test that ingests through the door holds this
/// lock for its whole body, so no two tests ever see each other's mode.
static MODE_LOCK: Mutex<()> = Mutex::new(());

struct ModeGuard {
    _lock: MutexGuard<'static, ()>,
}

fn migration_mode(token: Option<&str>) -> ModeGuard {
    let lock = MODE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    match token {
        Some(token) => std::env::set_var(MIGRATION_MODE_ENV_VAR, token),
        None => std::env::remove_var(MIGRATION_MODE_ENV_VAR),
    }
    ModeGuard { _lock: lock }
}

impl Drop for ModeGuard {
    fn drop(&mut self) {
        std::env::remove_var(MIGRATION_MODE_ENV_VAR);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime")
}

// ---------------------------------------------------------------------------
// Identities and canonical seeding
// ---------------------------------------------------------------------------

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn public_key(seed: u8) -> [u8; 32] {
    signing_key(seed).verifying_key().to_bytes()
}

fn principal(seed: u8) -> Principal {
    Principal::from_public_key(public_key(seed))
}

fn did(seed: u8) -> String {
    public_key_to_did(&public_key(seed))
}

fn verse() -> Identifier32 {
    Identifier32(legacy_identifier(LegacyTable::Verse, VERSE_ID).expect("ascii verse id"))
}

fn petal() -> Identifier32 {
    Identifier32(legacy_identifier(LegacyTable::Petal, PETAL_ID).expect("ascii petal id"))
}

fn verse_scope() -> Scope {
    Scope::verse_wide(verse())
}

fn petal_scope() -> Scope {
    Scope::new(verse(), Some(petal()), None).expect("petal scope")
}

/// The Manager's verse-wide chain for [`NODE`]: `append` (op + shard) to seal batches, and
/// `fetch`/`decrypt`/`materialize` on `shard` to hydrate them (SPEC-10 §4.2, §5.1).
fn node_chain() -> Vec<u8> {
    let now = SystemClock.now_ms();
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER),
        audience: principal(NODE),
        parent_certificate_id: None,
        issuer_authority_id: ANCHOR,
        grant_scope: verse_scope(),
        epoch_scope: verse_scope(),
        scope_epoch: EPOCH,
        verbs: VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Decrypt, Verb::Materialize])
            .expect("verbs"),
        object_classes: ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Shard])
            .expect("classes"),
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

/// [`node_chain`], but with a `max_segment_bytes` caveat (SPEC-10 §3.2.5):
/// small enough that an outsized sealed shard trips `ChainError::SegmentTooLarge`
/// at `check_append_shard` (`sealer.rs:385`), the gate `seal_closed_windows`
/// reads as `Prepared::Oversized` (RP-0).
fn node_chain_with_segment_cap(max_segment_bytes: u64) -> Vec<u8> {
    let now = SystemClock.now_ms();
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER),
        audience: principal(NODE),
        parent_certificate_id: None,
        issuer_authority_id: ANCHOR,
        grant_scope: verse_scope(),
        epoch_scope: verse_scope(),
        scope_epoch: EPOCH,
        verbs: VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Decrypt, Verb::Materialize])
            .expect("verbs"),
        object_classes: ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Shard])
            .expect("classes"),
        not_before_ms: now - 60_000,
        not_after_ms: now + 3_600_000,
        delegation_depth: 0,
        caveats: Caveats {
            max_segment_bytes: Some(max_segment_bytes),
            ..Caveats::default()
        },
    }
    .sign(&signing_key(MANAGER))
    .expect("sign capped certificate");
    Chain::from_certificates(&[certificate])
        .expect("chain")
        .encode_canonical()
        .expect("encode chain")
}

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

/// The Manager role, a branch genesis, the verse bumped 0 -> [`EPOCH`], the node's chain, and the
/// petal payload-topic key in durable custody.
async fn seed_canon(db: &Db, secrets: &Arc<dyn SecretStore>, chain: &[u8]) {
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
        .bind(("did", principal(MANAGER).did))
        .bind(("scope", interim_authority_scope_key(&verse_scope())))
        .bind(("role", "manager".to_string()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");
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
        .put_verified_chain(chain)
        .await
        .expect("put the node's chain");
    SecretStoreScopeKeyCustody::load(db, Arc::clone(secrets))
        .await
        .expect("custody")
        .take_custody(
            ScopeKeyId {
                scope: petal_scope(),
                epoch: EPOCH,
            },
            SealingKey::from_bytes(PETAL_KEY_BYTES),
        )
        .await
        .expect("take custody of the petal key");
}

// ---------------------------------------------------------------------------
// Legacy hot plane seeding and the two ingest doors
// ---------------------------------------------------------------------------

async fn seed_legacy(db: &Db) {
    db.query(
        "CREATE verse CONTENT { verse_id: $vid, name: 'v', created_by: 'did:key:z6MkOwner', \
         created_at: '2026-10-14T00:00:00Z', default_access: 'viewer' };
         CREATE fractal CONTENT { fractal_id: $fid, verse_id: $vid, owner_did: 'did:key:z6MkOwner', \
         name: 'f', created_at: '2026-10-14T00:00:00Z' };
         CREATE petal CONTENT { petal_id: $pid, fractal_id: $fid, name: 'p', \
         node_id: 'did:key:z6MkOwner', created_at: '2026-10-14T00:00:00Z' };",
    )
    .bind(("vid", VERSE_ID.to_string()))
    .bind(("fid", FRACTAL_ID.to_string()))
    .bind(("pid", PETAL_ID.to_string()))
    .await
    .expect("seed hierarchy")
    .check()
    .expect("seed hierarchy check");
    for node_id in [NODE_A, NODE_B] {
        db.query(
            "CREATE node CONTENT { node_id: $nid, petal_id: $pid, display_name: 'anchor', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
             scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-14T00:00:00Z' }",
        )
        .bind(("nid", node_id.to_string()))
        .bind(("pid", PETAL_ID.to_string()))
        .await
        .expect("seed node")
        .check()
        .expect("seed node check");
    }
    register_format(
        db,
        VERSE_ID,
        &DeviceFormatDescriptor {
            format_id: FORMAT_ID.to_string(),
            mime: FORMAT_MIME.to_string(),
            schema: None,
            description: String::new(),
        },
        "did:key:z6MkManager",
    )
    .await
    .expect("declare format");
}

async fn seed_asset(db: &Db, bytes: &[u8]) -> String {
    let asset_id = ulid::Ulid::new().to_string();
    db.query(
        "CREATE asset CONTENT { asset_id: $aid, name: 'f.bin', content_type: $ct, \
         size_bytes: $size, content_hash: $hash, owner_did: $owner, \
         petal_id: $pid, created_at: '2026-10-14T00:00:00Z' }",
    )
    .bind(("aid", asset_id.clone()))
    .bind(("ct", FORMAT_MIME.to_string()))
    .bind(("size", bytes.len() as i64))
    .bind(("hash", blake3::hash(bytes).to_hex().to_string()))
    .bind(("owner", did(DEVICE)))
    .bind(("pid", PETAL_ID.to_string()))
    .await
    .expect("seed asset")
    .check()
    .expect("seed asset check");
    asset_id
}

/// One reading through the real door; returns its server-minted `reading_id`.
async fn ingest_reading(db: &Db, node_id: &str, value: f64, payload_id: Option<String>) -> String {
    insert_readings_with_replication(
        db,
        PETAL_ID,
        Some(VERSE_ID),
        &did(DEVICE),
        SourceClass::Device,
        &[IotReadingInput {
            node_id: node_id.to_string(),
            metric: "temperature_c".to_string(),
            value,
            units: "C".to_string(),
            recorded_at: Some("2026-10-14T00:00:00Z".to_string()),
            payload_id,
        }],
        None,
        None,
    )
    .await
    .expect("door accepts the reading");
    let ids: Vec<String> = db
        .query("SELECT VALUE reading_id FROM iot_reading WHERE node_id = $node AND value = $value")
        .bind(("node", node_id.to_string()))
        .bind(("value", value))
        .await
        .expect("reading id")
        .take(0)
        .expect("reading id take");
    ids.into_iter().next().expect("the reading was written")
}

/// [`ingest_reading`], with an explicit `units` instead of the fixed `"C"`
/// (RP-0: inflating `units` inflates the sealed hexon's stored length —
/// SPEC-10 §3.2 rule 1 pins the Parquet members `UNCOMPRESSED` with
/// dictionary encoding off, so stored length tracks content length directly,
/// with no compression trick to defeat).
async fn ingest_reading_with_units(db: &Db, node_id: &str, value: f64, units: &str) -> String {
    insert_readings_with_replication(
        db,
        PETAL_ID,
        Some(VERSE_ID),
        &did(DEVICE),
        SourceClass::Device,
        &[IotReadingInput {
            node_id: node_id.to_string(),
            metric: "temperature_c".to_string(),
            value,
            units: units.to_string(),
            recorded_at: Some("2026-10-14T00:00:00Z".to_string()),
            payload_id: None,
        }],
        None,
        None,
    )
    .await
    .expect("door accepts the reading");
    let ids: Vec<String> = db
        .query("SELECT VALUE reading_id FROM iot_reading WHERE node_id = $node AND value = $value")
        .bind(("node", node_id.to_string()))
        .bind(("value", value))
        .await
        .expect("reading id")
        .take(0)
        .expect("reading id take");
    ids.into_iter().next().expect("the reading was written")
}

/// One device payload of `assets` through the real door; returns its `payload_id`.
async fn ingest_payload(db: &Db, node_id: &str, assets: Vec<String>) -> String {
    let (payload_id, _) = insert_device_payloads(
        db,
        PETAL_ID,
        Some(VERSE_ID),
        &did(DEVICE),
        SourceClass::Device,
        &DevicePayloadInput {
            node_id: node_id.to_string(),
            format_id: FORMAT_ID.to_string(),
            asset_ids: assets,
            recorded_at: Some("2026-10-14T00:00:01Z".to_string()),
            payload_id: None,
        },
        None,
        None,
    )
    .await
    .expect("door accepts the payload");
    payload_id
}

async fn count_rows(db: &Db, statement: &str) -> usize {
    let rows: Vec<serde_json::Value> = db
        .query(statement)
        .await
        .expect("count query")
        .take(0)
        .expect("count take");
    rows.len()
}

async fn window_member_count(db: &Db) -> usize {
    count_rows(db, "SELECT row_id FROM observation_window_member").await
}

async fn kind5_count(db: &Db) -> usize {
    count_rows(
        db,
        "SELECT op_id_hex FROM verified_op_log WHERE operation_kind = 5",
    )
    .await
}

// ---------------------------------------------------------------------------
// The observation plane under test
// ---------------------------------------------------------------------------

async fn verifier(db: &Db, secrets: &Arc<dyn SecretStore>) -> DurableCandidateVerifier {
    durable_observation_verifier(db, Arc::clone(secrets), Some(ANCHOR))
        .await
        .expect("durable observation verifier")
        .verifier
}

fn sealer<'a>(
    mode: MigrationMode,
    db: &'a Db,
    verifier: &'a DurableCandidateVerifier,
    author: &'a SigningKey,
    chain_id: Hash32,
) -> Result<ObservationSealer<'a>, ObservationPlaneError> {
    ObservationSealer::new(
        mode,
        db,
        verifier,
        ObservationSealerConfig {
            verse_id: VERSE_ID,
            branch_id: BRANCH,
            chain_id,
            author,
            policy: SealPolicy::default(),
        },
    )
}

async fn hydrate(
    db: &Db,
    verifier: &DurableCandidateVerifier,
    chain_id: Hash32,
) -> HydrationReport {
    hydrate_observation_catalog(
        MigrationMode::ShadowRebuild,
        db,
        verifier,
        HydrationRequest {
            verse_id: verse(),
            branch_id: BRANCH,
            hydrator_chain_id: chain_id,
            hydrator: &principal(NODE),
        },
    )
    .await
    .expect("hydration runs")
}

async fn catalog_reading_ids(db: &Db) -> Vec<String> {
    db.query("SELECT VALUE reading_id FROM observation_catalog_reading ORDER BY reading_id ASC")
        .await
        .expect("catalog read")
        .take(0)
        .expect("catalog take")
}

fn device_source() -> ObservationSource {
    observation_source(SourceClass::Device, &did(DEVICE), None).expect("device source")
}

/// A reading row as the sealer would build it from a door accept.
fn reading_row(reading_id: &str, node_id: &str, value: f64, wall_offset: u64) -> ReadingRow {
    ReadingRow {
        reading_id: reading_id.to_string(),
        node_id: node_id.to_string(),
        metric: "temperature_c".to_string(),
        value,
        units: "C".to_string(),
        recorded_at: "2026-10-14T00:00:00+00:00".to_string(),
        recorded_at_ms: 1_760_400_000_000 + wall_offset as i64,
        hlc_timestamp: (HLC_WALL_MS + 10_000 + wall_offset) << 16,
        payload_id: None,
        source_index: 0,
    }
}

async fn stored_artifact(verifier: &DurableCandidateVerifier, artifact_id: Hash32) -> Vec<u8> {
    ContentByteStore::get(&verifier.parts().ciphertexts, artifact_id).expect("artifact is held")
}

async fn forget_artifact(db: &Db, artifact_id: Hash32) {
    db.query("DELETE FROM canon_sealed_artifact WHERE artifact_id_hex = $id")
        .bind(("id", hex::encode(artifact_id.as_bytes())))
        .await
        .expect("forget artifact")
        .check()
        .expect("forget artifact check");
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 13
// ---------------------------------------------------------------------------

/// SPEC-10 §8 rule 2: in `legacy_only` (and `seam_ready`) the door records no window, nothing is
/// sealed, and nothing is appended; the sealer and hydrator refuse to run at all.
#[test]
fn legacy_only_ingest_creates_no_observation_candidate() {
    let _mode = migration_mode(None);
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        seed_legacy(&db).await;
        let asset = seed_asset(&db, b"frame").await;
        ingest_payload(&db, NODE_A, vec![asset]).await;
        ingest_reading(&db, NODE_A, 21.5, None).await;

        assert_eq!(
            count_rows(&db, "SELECT reading_id FROM iot_reading").await,
            1
        );
        assert_eq!(window_member_count(&db).await, 0, "no window record");

        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        for mode in [MigrationMode::LegacyOnly, MigrationMode::SeamReady] {
            assert!(matches!(
                sealer(mode, &db, &verifier, &author, Hash32::of(&chain)),
                Err(ObservationPlaneError::ModeDisabled { .. })
            ));
        }
        assert!(matches!(
            hydrate_observation_catalog(
                MigrationMode::DualEmitShadow,
                &db,
                &verifier,
                HydrationRequest {
                    verse_id: verse(),
                    branch_id: BRANCH,
                    hydrator_chain_id: Hash32::of(&chain),
                    hydrator: &principal(NODE),
                },
            )
            .await,
            Err(ObservationPlaneError::ModeDisabled { .. })
        ));
        assert_eq!(kind5_count(&db).await, 0, "no observation candidate");
        assert_eq!(
            count_rows(&db, "SELECT artifact_id_hex FROM canon_sealed_artifact").await,
            0,
            "nothing sealed"
        );
        assert!(observation_reference_rows(&db)
            .await
            .expect("rows")
            .is_empty());
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 14
// ---------------------------------------------------------------------------

/// Readings and a device payload through the real doors in `shadow_rebuild`: the sealer appends
/// kind-5 batches, their reference rows materialize, and three hydrations from empty state give
/// the same catalog, which equals the legacy rows with zero differences.
#[test]
fn shadow_mode_seals_window_commits_batch_and_hydrates_parity_with_legacy() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        seed_legacy(&db).await;
        let first = seed_asset(&db, b"frame one").await;
        let second = seed_asset(&db, b"frame two").await;
        let payload_id = ingest_payload(&db, NODE_A, vec![first, second]).await;
        ingest_reading(&db, NODE_A, 21.5, Some(payload_id)).await;
        ingest_reading(&db, NODE_A, 0.125, None).await;
        ingest_reading(&db, NODE_B, 19.25, None).await;
        assert_eq!(
            window_member_count(&db).await,
            4,
            "the door recorded every accept"
        );

        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let mut sealer = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("shadow sealer");
        // A window still open is not sealed.
        let open = sealer.seal_closed_windows(0).await.expect("nothing closed");
        assert!(open.batches.is_empty());

        let report = sealer
            .seal_closed_windows(i64::MAX)
            .await
            .expect("seal closed windows");
        assert!(report.uncovered.is_empty(), "{:?}", report.uncovered);
        assert!(!report.batches.is_empty());
        assert_eq!(
            report.batches.iter().map(|batch| batch.count).sum::<u64>(),
            4
        );
        assert!(report
            .batches
            .iter()
            .all(|batch| batch.reference_materialized));
        assert_eq!(kind5_count(&db).await, report.batches.len());

        let references = observation_reference_rows(&db).await.expect("rows");
        assert_eq!(references.len(), report.batches.len());
        assert_eq!(
            references
                .iter()
                .map(|row| row["count"].as_u64().expect("count"))
                .sum::<u64>(),
            4
        );

        // Everything is sealed now; a second pass appends nothing.
        let again = sealer.seal_closed_windows(i64::MAX).await.expect("re-run");
        assert!(again.batches.is_empty());

        let hydrations = [
            hydrate(&db, &verifier, chain_id).await,
            hydrate(&db, &verifier, chain_id).await,
            hydrate(&db, &verifier, chain_id).await,
        ];
        for hydration in &hydrations {
            assert!(hydration
                .outcomes
                .values()
                .all(|outcome| *outcome == HydrationOutcome::Hydrated));
            assert_eq!((hydration.readings, hydration.payloads), (3, 1));
            assert!(hydration.conflicts.is_empty());
        }
        assert_eq!(hydrations[0].catalog_root, hydrations[1].catalog_root);
        assert_eq!(hydrations[1].catalog_root, hydrations[2].catalog_root);

        let parity = compare_catalog_with_legacy(MigrationMode::ShadowRebuild, &db, "b3-parity")
            .await
            .expect("comparison");
        assert!(
            parity.comparison.is_equal(),
            "{:?}",
            parity.comparison.differences
        );
        assert_eq!((parity.members_total, parity.members_sealed), (4, 4));
        assert!(!parity.blocks_promotion());
        assert_eq!(parity.run_counters().legacy_success_with_candidate, 4);
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 15
// ---------------------------------------------------------------------------

/// SPEC-10 §3.4 rule 5: a replicated row stamped with this node's own DID is in the hot table
/// but never in window membership, so the sealer cannot select it — by construction, not by a
/// `source_did` filter.
#[test]
fn sealer_never_signs_replicated_observations() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        seed_legacy(&db).await;
        db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: 'editor' }")
            .bind(("did", did(NODE)))
            .bind(("scope", format!("VERSE#{VERSE_ID}")))
            .await
            .expect("seed editor")
            .check()
            .expect("seed editor check");

        let replicated = serde_json::json!({
            "reading_id": "reading-replicated", "node_id": NODE_A, "petal_id": PETAL_ID,
            "metric": "temperature_c", "value": 99.0, "units": "C",
            "recorded_at": "2026-10-14T00:00:00Z", "recorded_at_ms": 1_760_400_000_000_i64,
            "hlc_timestamp": ((HLC_WALL_MS + 5) << 16) as i64,
            "source_did": did(NODE), "payload_id": null,
        });
        let outcome = apply_replicated_row_handler(
            &db,
            VERSE_ID,
            "iot_reading",
            "reading-replicated",
            &serde_json::to_vec(&replicated).expect("row bytes"),
            &did(NODE),
            None,
        )
        .await
        .expect("inbound row");
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Applied,
            "union applies the row"
        );
        let local = ingest_reading(&db, NODE_A, 20.0, None).await;

        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let report = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("shadow sealer")
        .seal_closed_windows(i64::MAX)
        .await
        .expect("seal");
        assert_eq!(
            report.batches.iter().map(|batch| batch.count).sum::<u64>(),
            1
        );

        hydrate(&db, &verifier, chain_id).await;
        assert_eq!(catalog_reading_ids(&db).await, vec![local]);
        assert_eq!(window_member_count(&db).await, 1);
    });
}

/// SPEC-10 §3.4 rules 5-6 (Phase B security review): membership binds the exact row the door
/// accepted. A hot row whose columns change under the same id after the door (an admin clear
/// followed by union re-replication of author-chosen columns) is marked uncovered and never
/// signed; the untouched member next to it still seals and hydrates.
#[test]
fn sealer_never_signs_a_hot_row_replaced_after_the_door_accepted_it() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        seed_legacy(&db).await;
        let replaced = ingest_reading(&db, NODE_A, 20.0, None).await;
        let kept = ingest_reading(&db, NODE_B, 21.0, None).await;
        db.query("UPDATE iot_reading SET value = 99.0, source_did = $did WHERE reading_id = $id")
            .bind(("did", did(NODE)))
            .bind(("id", replaced.clone()))
            .await
            .expect("substitute the hot row")
            .check()
            .expect("substitute check");

        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let report = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("shadow sealer")
        .seal_closed_windows(i64::MAX)
        .await
        .expect("seal");
        assert_eq!(
            report.batches.iter().map(|batch| batch.count).sum::<u64>(),
            1,
            "only the untouched member is signed: {report:?}"
        );
        assert_eq!(report.uncovered.len(), 1, "{report:?}");
        assert_eq!(report.uncovered[0].row_id, replaced);
        assert!(
            report.uncovered[0]
                .reason
                .contains("not the observation the door accepted"),
            "{report:?}"
        );

        hydrate(&db, &verifier, chain_id).await;
        assert_eq!(catalog_reading_ids(&db).await, vec![kept]);
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 16 (C8 restart)
// ---------------------------------------------------------------------------

/// An empty rebuild from the verified log, after a restart, reproduces every reference row the
/// sealer materialized live (B2b facts through durable custody).
#[test]
fn rebuild_from_log_reproduces_observation_reference_rows() {
    let _mode = migration_mode(Some("dual_emit_shadow"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);

    let live_rows = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            seed_canon(&db, &secrets, &chain).await;
            seed_legacy(&db).await;
            ingest_reading(&db, NODE_A, 1.5, None).await;
            ingest_reading(&db, NODE_B, 2.5, None).await;
            let verifier = verifier(&db, &secrets).await;
            let author = signing_key(NODE);
            let report = sealer(
                MigrationMode::DualEmitShadow,
                &db,
                &verifier,
                &author,
                chain_id,
            )
            .expect("shadow sealer")
            .seal_closed_windows(i64::MAX)
            .await
            .expect("seal");
            assert!(!report.batches.is_empty());
            let rows = observation_reference_rows(&db).await.expect("rows");
            assert_eq!(rows.len(), report.batches.len());
            rows
        })
    }; // DB thread joined, store closed

    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let verifier = verifier(&db, &secrets).await;
        let heads = branch_heads(&db, BRANCH).await.expect("heads");
        let frontier = SortedFrontier::try_new(heads).expect("frontier");
        let report = rebuild_observation_references(
            MigrationMode::DualEmitShadow,
            &db,
            &verifier,
            verse(),
            BRANCH,
            &frontier,
        )
        .await
        .expect("mode")
        .expect("rebuild");
        assert_eq!(report.source, RebuildSource::EmptyState);
        assert!(report.replay.is_complete(), "{:?}", report.replay);
        assert_eq!(
            observation_reference_rows(&db).await.expect("rows"),
            live_rows
        );
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 10
// ---------------------------------------------------------------------------

/// Seals `readings`/`payload_refs` honestly, then signs a batch whose payload `tamper` bends
/// while it still passes every §4.3 rule. Admits and appends it; returns its `op_id`.
#[allow(clippy::too_many_arguments)]
async fn admit_bent_batch(
    db: &Db,
    verifier: &DurableCandidateVerifier,
    chain_id: Hash32,
    readings: Vec<ReadingRow>,
    payload_refs: Vec<PayloadRefRow>,
    written_source_count: usize,
    sources: Vec<ObservationSource>,
    tamper: impl FnOnce(&mut ObservationBatchPayloadParts),
) -> Hash32 {
    let context = sealing_context(db, verifier, chain_id, VERSE_ID, PETAL_ID)
        .await
        .expect("sealing context");
    let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(64).expect("capacity"));
    let written =
        write_observation_hexon(&did(NODE), &readings, &payload_refs, written_source_count)
            .expect("hexon");
    let summary = written.summary.clone();
    let hexon = seal_shard(&context, &mut ledger, written.hexon_bytes).expect("hexon seal");
    let stats = seal_shard(&context, &mut ledger, written.stats_bytes).expect("stats seal");
    let mut parts = ObservationBatchPayloadParts {
        hexon_artifact_id: hexon.artifact_id,
        hexon_stored_length: hexon.stored.len() as u64,
        hlc_range: HlcRange {
            min: Hlc::new(summary.hlc_range.0 .0 as u64, summary.hlc_range.0 .1 as u32),
            max: Hlc::new(summary.hlc_range.1 .0 as u64, summary.hlc_range.1 .1 as u32),
        },
        sensor_time_range: SensorTimeRange {
            min_ms: summary.sensor_time_range_ms.0,
            max_ms: summary.sensor_time_range_ms.1,
        },
        stats_hash: stats.artifact_id,
        count: summary.count,
        anchors: summary.anchors.iter().copied().map(Identifier32).collect(),
        asset_refs: summary.asset_refs.iter().copied().map(Hash32).collect(),
        sources,
        hexon_format_version: summary.hexon_format_version,
        stats_stored_length: stats.stored.len() as u64,
    };
    tamper(&mut parts);
    let payload = ObservationBatchPayload::new(parts.normalized()).expect("bent but valid payload");
    let (packed, _) = fe_database::op_log::next_hlc_timestamp();
    let signed = sign_observation_batch(
        &context,
        &signing_key(NODE),
        BRANCH,
        branch_heads(db, BRANCH).await.expect("heads"),
        Hlc::new(packed >> 16, (packed & 0xFFFF) as u32),
        &payload,
    )
    .expect("sign");
    for shard in [&hexon, &stats] {
        verifier
            .parts()
            .ciphertexts
            .put_sealed_artifact(shard.artifact_id, &shard.stored)
            .await
            .expect("store artifact");
    }
    admit_and_append_durably(
        verifier,
        &SurrealVerifiedLogStore::new(db.clone()),
        &NoPrecondition,
        &signed.bytes,
        Some(&signed.ciphertext),
    )
    .await
    .expect("admission cannot see the hexon, so the bent batch admits");
    signed.op_id
}

/// SPEC-10 §5.2 step 4: a hexon that disagrees with its signed batch is
/// `observation_batch_invalid`, and none of its rows enters the catalog; an honest batch in the
/// same pass hydrates.
#[test]
fn hydration_rejects_a_hexon_that_disagrees_with_its_signed_batch() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let honest = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("shadow sealer")
        .seal_observations(
            PETAL_ID,
            vec![reading_row("honest", NODE_A, 1.0, 1)],
            Vec::new(),
            vec![device_source()],
        )
        .await
        .expect("honest batch");

        let other_source =
            observation_source(SourceClass::Simulation, &did(DEVICE), None).expect("sim source");
        let two_rows = |tag: &str| {
            vec![
                reading_row(&format!("{tag}-1"), NODE_A, 2.0, 2),
                reading_row(&format!("{tag}-2"), NODE_A, 3.0, 3),
            ]
        };
        let payload_ref = PayloadRefRow {
            payload_id: "assets-payload".to_string(),
            node_id: NODE_A.to_string(),
            format_id: FORMAT_ID.to_string(),
            recorded_at: "2026-10-14T00:00:01+00:00".to_string(),
            recorded_at_ms: 1_760_400_001_000,
            hlc_timestamp: (HLC_WALL_MS + 10_009) << 16,
            asset_ids: vec!["asset-1".to_string()],
            asset_content_hashes: vec![[0xa1; 32]],
            source_index: 0,
        };
        let mut out_of_range = two_rows("range");
        out_of_range[1].source_index = 1;

        let bent = [
            (
                "row count",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    two_rows("count"),
                    Vec::new(),
                    1,
                    vec![device_source()],
                    |parts| parts.count += 1,
                )
                .await,
                vec!["count-1", "count-2"],
            ),
            (
                "anchor set",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    two_rows("anchors"),
                    Vec::new(),
                    1,
                    vec![device_source()],
                    |parts| parts.anchors.push(node_anchor(NODE_B).expect("anchor")),
                )
                .await,
                vec!["anchors-1", "anchors-2"],
            ),
            (
                "hlc range",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    two_rows("hlc"),
                    Vec::new(),
                    1,
                    vec![device_source()],
                    |parts| {
                        parts.hlc_range.max =
                            Hlc::new(parts.hlc_range.max.wall_ms + 1, parts.hlc_range.max.counter)
                    },
                )
                .await,
                vec!["hlc-1", "hlc-2"],
            ),
            (
                "asset set",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    two_rows("assets"),
                    vec![payload_ref],
                    1,
                    vec![device_source()],
                    |parts| parts.asset_refs.push(Hash32([0xee; 32])),
                )
                .await,
                vec!["assets-1", "assets-2"],
            ),
            (
                "unused source",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    two_rows("unused"),
                    Vec::new(),
                    2,
                    vec![device_source(), other_source.clone()],
                    |_| {},
                )
                .await,
                vec!["unused-1", "unused-2"],
            ),
            (
                "out-of-range source index",
                admit_bent_batch(
                    &db,
                    &verifier,
                    chain_id,
                    out_of_range,
                    Vec::new(),
                    2,
                    vec![device_source()],
                    |_| {},
                )
                .await,
                vec!["range-1", "range-2"],
            ),
        ];

        let report = hydrate(&db, &verifier, chain_id).await;
        assert_eq!(report.outcomes[&honest.op_id], HydrationOutcome::Hydrated);
        let catalog = catalog_reading_ids(&db).await;
        assert_eq!(catalog, vec!["honest".to_string()]);
        for (case, op_id, reading_ids) in bent {
            assert!(
                matches!(
                    report.outcomes[&op_id],
                    HydrationOutcome::ObservationBatchInvalid { .. }
                ),
                "{case}: {:?}",
                report.outcomes[&op_id]
            );
            for reading_id in reading_ids {
                assert!(!catalog.contains(&reading_id.to_string()), "{case}");
            }
        }
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 12
// ---------------------------------------------------------------------------

/// SPEC-10 §5.3 rules 3-4: two hydrated batches carrying `reading_id` "dup" with different
/// values contribute neither copy and leave evidence naming both batches; an identical copy in
/// both collapses into one row.
#[test]
fn conflicting_duplicate_observation_is_retained_as_evidence_not_resolved() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let mut sealer = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("shadow sealer");
        let first = sealer
            .seal_observations(
                PETAL_ID,
                vec![
                    reading_row("dup", NODE_A, 1.0, 1),
                    reading_row("same", NODE_A, 5.0, 2),
                ],
                Vec::new(),
                vec![device_source()],
            )
            .await
            .expect("first batch");
        let second = sealer
            .seal_observations(
                PETAL_ID,
                vec![
                    reading_row("dup", NODE_A, 2.0, 1),
                    reading_row("same", NODE_A, 5.0, 2),
                ],
                Vec::new(),
                vec![device_source()],
            )
            .await
            .expect("second batch");

        let report = hydrate(&db, &verifier, chain_id).await;
        assert_eq!(report.outcomes[&first.op_id], HydrationOutcome::Hydrated);
        assert_eq!(report.outcomes[&second.op_id], HydrationOutcome::Hydrated);
        let mut both = vec![first.op_id, second.op_id];
        both.sort();
        assert_eq!(
            report.conflicts,
            vec![(ObservationTable::Reading, "dup".to_string(), both.clone())]
        );
        assert_eq!(catalog_reading_ids(&db).await, vec!["same".to_string()]);

        let same_op_ids: Vec<Vec<String>> = db
            .query("SELECT VALUE op_ids FROM observation_catalog_reading WHERE reading_id = 'same'")
            .await
            .expect("same row")
            .take(0)
            .expect("same take");
        let both_hex: Vec<String> = both.iter().map(|op| hex::encode(op.as_bytes())).collect();
        assert_eq!(same_op_ids, vec![both_hex.clone()]);
        let evidence: Vec<Vec<String>> = db
            .query("SELECT VALUE op_ids FROM observation_catalog_conflict WHERE row_id = 'dup'")
            .await
            .expect("conflict row")
            .take(0)
            .expect("conflict take");
        assert_eq!(evidence, vec![both_hex]);
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 11
// ---------------------------------------------------------------------------

/// SPEC-10 §4.4 / §5.3: with its hexon gone the batch still reduces (its reference row is
/// rebuilt from the log alone), hydration reports `referenced_artifact_unavailable`, and the
/// batch hydrates once the artifact arrives.
#[test]
fn missing_hexon_surfaces_unavailable_at_hydration_not_reduce() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        let (batch, hexon_bytes) = {
            let verifier = verifier(&db, &secrets).await;
            let author = signing_key(NODE);
            let batch = sealer(
                MigrationMode::ShadowRebuild,
                &db,
                &verifier,
                &author,
                chain_id,
            )
            .expect("shadow sealer")
            .seal_observations(
                PETAL_ID,
                vec![reading_row("r1", NODE_A, 7.0, 1)],
                Vec::new(),
                vec![device_source()],
            )
            .await
            .expect("batch");
            let bytes = stored_artifact(&verifier, batch.hexon_artifact_id).await;
            (batch, bytes)
        };
        assert!(batch.reference_materialized);
        forget_artifact(&db, batch.hexon_artifact_id).await;

        let verifier = verifier(&db, &secrets).await;
        let rows_before = observation_reference_rows(&db).await.expect("rows");
        let frontier = SortedFrontier::try_new([batch.op_id]).expect("frontier");
        rebuild_observation_references(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            verse(),
            BRANCH,
            &frontier,
        )
        .await
        .expect("mode")
        .expect("the reduction never needs the hexon");
        assert_eq!(
            observation_reference_rows(&db).await.expect("rows"),
            rows_before
        );

        let report = hydrate(&db, &verifier, chain_id).await;
        assert!(matches!(
            report.outcomes[&batch.op_id],
            HydrationOutcome::ReferencedArtifactUnavailable { .. }
        ));
        assert!(
            catalog_reading_ids(&db).await.is_empty(),
            "nothing substituted"
        );

        verifier
            .parts()
            .ciphertexts
            .put_sealed_artifact(batch.hexon_artifact_id, &hexon_bytes)
            .await
            .expect("the hexon arrives");
        let report = hydrate(&db, &verifier, chain_id).await;
        assert_eq!(report.outcomes[&batch.op_id], HydrationOutcome::Hydrated);
        assert_eq!(catalog_reading_ids(&db).await, vec!["r1".to_string()]);
    });
}

// ---------------------------------------------------------------------------
// SPEC-10 §10 test 19
// ---------------------------------------------------------------------------

/// G6: an analytics read records the sorted, deduplicated batches it read whatever the order,
/// and a listed batch that is not hydrated fails the read instead of shrinking the set.
#[test]
fn analytics_identity_records_the_batches_it_read() {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = node_chain();
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        let (first, second) = {
            let verifier = verifier(&db, &secrets).await;
            let author = signing_key(NODE);
            let mut sealer = sealer(
                MigrationMode::ShadowRebuild,
                &db,
                &verifier,
                &author,
                chain_id,
            )
            .expect("shadow sealer");
            let first = sealer
                .seal_observations(
                    PETAL_ID,
                    vec![
                        reading_row("a1", NODE_A, 1.0, 1),
                        reading_row("a2", NODE_A, 2.0, 2),
                    ],
                    Vec::new(),
                    vec![device_source()],
                )
                .await
                .expect("first");
            let second = sealer
                .seal_observations(
                    PETAL_ID,
                    vec![reading_row("b1", NODE_B, 3.0, 3)],
                    Vec::new(),
                    vec![device_source()],
                )
                .await
                .expect("second");
            hydrate(&db, &verifier, chain_id).await;
            (first, second)
        };
        let base = AnalyticsReadBase {
            branch_id: BRANCH,
            frontier_commitment: Hash32([0x0f; 32]),
            segment_manifest_id: Hash32([0x0e; 32]),
            materializer_version: ObservationBatchMaterializer::version(),
        };
        let mut both = vec![first.op_id, second.op_id];
        both.sort();

        let shuffled =
            read_observation_batches(&db, base.clone(), [second.op_id, first.op_id, second.op_id])
                .await
                .expect("read");
        assert_eq!(shuffled.identity.observation_input_set(), both.as_slice());
        let ids: Vec<&str> = shuffled
            .readings
            .iter()
            .map(|row| row.reading_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a1", "a2", "b1"]);
        let ordered = read_observation_batches(&db, base.clone(), [first.op_id, second.op_id])
            .await
            .expect("read");
        assert_eq!(ordered.identity, shuffled.identity);

        forget_artifact(&db, first.hexon_artifact_id).await;
        let verifier = verifier(&db, &secrets).await;
        hydrate(&db, &verifier, chain_id).await;
        match read_observation_batches(&db, base.clone(), [first.op_id, second.op_id]).await {
            Err(AnalyticsReadError::BatchNotHydrated { op_id, .. }) => {
                assert_eq!(op_id, first.op_id)
            }
            other => panic!("expected the read to fail, got {other:?}"),
        }
        let narrowed = read_observation_batches(&db, base, [second.op_id])
            .await
            .expect("an explicitly narrower read is its own computation");
        assert_eq!(narrowed.identity.observation_input_set(), &[second.op_id]);
    });
}

// ---------------------------------------------------------------------------
// RP-0 (critic tick 8, AC-49): oversized-window split path
// ---------------------------------------------------------------------------

/// SPEC-10 §3.4 rule 4: a closed window whose encoded artifact exceeds the
/// chain's `max_segment_bytes` caveat is never refused outright — it splits
/// by anchor, then by HLC within an anchor, until every member either seals
/// or is individually too large to fit. A window of 3 anchors (one anchor
/// carrying both a small and an oversized reading) exercises both splits: the
/// anchor split separates the 3 anchors, and the HLC split then separates the
/// two same-anchor readings. The oversized reading is marked `uncovered`,
/// never silently dropped; the honest readings still seal and hydrate.
///
/// Mutation check: deleting the `Prepared::Oversized if group.len() == 1` arm
/// (`sealer.rs:300`) makes a 1-member oversized group split forever instead
/// of terminating (`split_group` on a 1-element group returns `(empty,
/// original)`, so the same group is re-pushed and re-evaluated unchanged) —
/// this test then hangs, which is why the seal call is itself time-bounded.
#[test]
fn an_oversized_window_splits_by_anchor_then_hlc_and_a_single_oversized_observation_is_marked_uncovered(
) {
    let _mode = migration_mode(Some("shadow_rebuild"));
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    // Comfortably above ordinary per-reading Parquet/zip overhead (a handful
    // of short string/int columns, one row) and comfortably below a reading
    // whose `units` alone is `HUGE_UNITS_LEN` bytes.
    const SEGMENT_CAP: u64 = 65_536;
    const HUGE_UNITS_LEN: usize = 300_000;
    let chain = node_chain_with_segment_cap(SEGMENT_CAP);
    let chain_id = Hash32::of(&chain);
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_canon(&db, &secrets, &chain).await;
        seed_legacy(&db).await;
        // A third anchor: `seed_legacy` seeds only NODE_A/NODE_B.
        db.query(
            "CREATE node CONTENT { node_id: $nid, petal_id: $pid, display_name: 'anchor', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
             scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-14T00:00:00Z' }",
        )
        .bind(("nid", NODE_C.to_string()))
        .bind(("pid", PETAL_ID.to_string()))
        .await
        .expect("seed node-c")
        .check()
        .expect("seed node-c check");

        let huge_units = "x".repeat(HUGE_UNITS_LEN);
        // Two readings share NODE_A's anchor (one small, one oversized) so
        // the window needs both an anchor split (3 distinct anchors) and,
        // within NODE_A's pair, an HLC split (SPEC-10 §3.4 rule 4).
        let small_a = ingest_reading_with_units(&db, NODE_A, 1.0, "C").await;
        let huge = ingest_reading_with_units(&db, NODE_A, 2.0, &huge_units).await;
        let small_b = ingest_reading_with_units(&db, NODE_B, 3.0, "C").await;
        let small_c = ingest_reading_with_units(&db, NODE_C, 4.0, "C").await;
        assert_eq!(
            window_member_count(&db).await,
            4,
            "the door recorded every accept"
        );

        let verifier = verifier(&db, &secrets).await;
        let author = signing_key(NODE);
        let mut sealer = sealer(
            MigrationMode::ShadowRebuild,
            &db,
            &verifier,
            &author,
            chain_id,
        )
        .expect("capped sealer");

        let report = tokio::time::timeout(TIMEOUT, sealer.seal_closed_windows(i64::MAX))
            .await
            .expect("seal_closed_windows must terminate, not hang")
            .expect("seal closed windows");

        // Every member ends sealed or uncovered and no member is in two batches.
        // `split_group` halves by anchor HASH order, so a half holding two small
        // anchors that fits the cap legitimately seals as one 2-member batch —
        // batch sizes depend on hash order; only the sealed total is invariant.
        assert_eq!(report.uncovered.len(), 1, "{:?}", report.uncovered);
        assert_eq!(report.uncovered[0].table, ObservationTable::Reading);
        assert_eq!(report.uncovered[0].row_id, huge);
        assert_eq!(
            report.uncovered[0].reason,
            "a single observation exceeds the sealed artifact bound"
        );
        assert!(
            report.batches.iter().all(|b| b.count >= 1),
            "{:?}",
            report.batches
        );
        assert_eq!(
            report.batches.iter().map(|b| b.count).sum::<u64>(),
            3,
            "every honest member seals exactly once: {:?}",
            report.batches
        );

        hydrate(&db, &verifier, chain_id).await;
        let mut expected = vec![small_a, small_b, small_c];
        expected.sort();
        assert_eq!(
            catalog_reading_ids(&db).await,
            expected,
            "the oversized reading never hydrates; the honest readings do"
        );

        // A second pass seals nothing new: the honest members are already
        // sealed and the oversized member stays uncovered, not retried.
        let again = tokio::time::timeout(TIMEOUT, sealer.seal_closed_windows(i64::MAX))
            .await
            .expect("re-run must terminate, not hang")
            .expect("re-run");
        assert!(again.batches.is_empty());
        assert!(again.uncovered.is_empty());
    });
}
