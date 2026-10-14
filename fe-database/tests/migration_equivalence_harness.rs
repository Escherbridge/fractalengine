//! W4-9 equivalence-evidence harness (CDL-W4; supports the CANON-DUAL-EMIT decision, does NOT
//! wire it). A dev-evidence run (owner R7 decision 3) on a harness-created test verse: the
//! `UpdateNodeTransform` -> `node-transform-v1` mapping runs through
//! `commit_operation_dual_emit` in `DualEmitShadow`, its candidates are admitted by the ONE
//! production verifier, replayed three times from empty, compared against the legacy SurrealDB
//! projection, and the run's `MeasurementReport` is written to
//! `target/migration-evidence/<run_id>.json`.
//! Own binary (process-global bypass counters and HLC); real DB thread over an on-disk store that
//! is closed (thread joined) and reopened mid-run. Rationale: `fe-database/src/migration/AGENTS.md`
//! §equivalence-harness and §transform-mapping.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    Caveats, Chain, ObjectClass, ObjectClassSet, Principal, RevalidationGate, UnsignedCertificate,
    Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::compose::{Clock, PayloadCiphertextSource, SystemClock};
use fe_canonical_log::crypto::aead::SealingKey;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::errors::{AdmissionOutcome, AppendOutcome};
use fe_canonical_log::materialize::traits::{verify_candidate, VerifiedEnvelopeMeta};
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::signing::sign_envelope;
use fe_database::canon_log::admission::{AdmissionRejection, NoPrecondition};
use fe_database::canon_log::append_store::SurrealVerifiedLogStore;
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView, SurrealScopeEpochStore,
};
use fe_database::canon_log::chain_store::ChainStore;
use fe_database::canon_log::scope_key_custody::SecretStoreScopeKeyCustody;
use fe_database::canon_log::verifier::{
    admit_and_append_durably, durable_candidate_verifier, DurableAdmissionError,
    DurableCandidateVerifier,
};
use fe_database::canon_log::{op_id_from_hex, op_id_to_hex};
use fe_database::migration::boundary::DualEmitOutcome;
use fe_database::migration::candidate::{
    CorrelationDisposition, IntentSnapshot, MigrationCandidateMember, MigrationCandidateSet,
};
use fe_database::migration::comparator::{
    compare_projections, ComparisonRecord, FieldDifferenceClass, QuantizedValue,
};
use fe_database::migration::mapping_inventory::{
    bypass_count, bypass_count_for, MappingInventory, MappingRegistrationError,
    D_CL20_MUTATION_KINDS,
};
use fe_database::migration::mode::MigrationMode;
use fe_database::migration::rebuild::{replay_admitted_closure_three_times, ShadowMaterializer};
use fe_database::migration::report::{MeasurementReport, RunCounters};
use fe_database::migration::shadow_store::{
    load_ledger_entries_for_run, LedgerBounds, ShadowLedgerEntry, ShadowLedgerWriter,
    SurrealShadowLedgerWriter,
};
use fe_database::migration::transform_mapping::{
    dual_emit_update_node_transform, export_legacy_transforms, in_causal_order, legacy_anchor,
    node_transform_materializer_version, node_transform_schema_hash,
    register_node_transform_schema, transform_field_path, ExactTransformQuantizer,
    QuantizationError, QuantizedTransform, RetainedCandidates, TransformCandidateDerivation,
    TransformIntent, TransformPreconditionError, TransformQuantizer, TransformRunCharter,
    TransformShadowMaterializer, TRANSFORM_MUTATION_KIND,
};
use fe_database::{build_scope, repo::Db, spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_identity::{InMemoryBackend, SecretStore};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult};
use fe_sdk::legacy_identifier::LegacyTable;

const TIMEOUT: Duration = Duration::from_secs(20);
const BRANCH: Identifier32 = Identifier32([0x42; 32]);
/// The one `issuer_authority_id` this run's deployment recognises.
const ANCHOR: Hash32 = Hash32([0xa7; 32]);
const MANAGER: u8 = 0x21;
const AUTHOR: u8 = 0x22;
const SCOPE_KEY_BYTES: [u8; 32] = [0x6c; 32];
/// The epoch the run's verse sits at after its seeded bump.
const EPOCH: u64 = 1;
const STRUCTURAL_HLC_MS: u64 = 1_700_000_000_000;

// ---------------------------------------------------------------------------
// DB thread harness (plan C8: drop AND join before reopening the same path)
// ---------------------------------------------------------------------------

struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    db: Option<Db>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn open(dir: &Path) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let db_path = dir.join("fe.db").to_string_lossy().to_string();
        // No replication sender, no sync thread: SPEC-8 §5.1 rule 7, no peer contact.
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
        let harness = Self {
            cmd_tx: Some(cmd_tx),
            res_rx,
            db: Some(shared_db),
            join: Some(join_handle),
        };
        assert!(matches!(harness.recv(), DbResult::Started));
        harness
    }

    fn db(&self) -> Db {
        self.db.clone().expect("open store")
    }

    fn send(&self, command: DbCommand) {
        self.cmd_tx
            .as_ref()
            .expect("open channel")
            .send(command)
            .expect("send command");
    }

    fn recv(&self) -> DbResult {
        self.res_rx.recv_timeout(TIMEOUT).expect("db result")
    }

    /// `UpdateNodeTransform` replies nothing on success; a trailing `Ping` orders after it.
    fn live_update_node_transform(&self, node_id: &str, transform: Transform) {
        self.send(DbCommand::UpdateNodeTransform {
            node_id: node_id.to_string(),
            position: transform.position,
            rotation: transform.rotation,
            scale: transform.scale,
        });
        self.send(DbCommand::Ping);
        assert!(matches!(self.recv(), DbResult::Pong));
    }

    fn create_node(&self, petal_id: &str, name: &str) -> String {
        self.send(DbCommand::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
            position: [0.0, 0.0, 0.0],
            correlation_id: None,
        });
        match self.recv() {
            DbResult::NodeCreated { id, .. } => id,
            other => panic!("expected NodeCreated, got {other:?}"),
        }
    }

    fn tombstone(&self, node_id: &str, auth: CallerAuth) -> DbResult {
        self.send(DbCommand::TombstoneNode {
            node_id: node_id.to_string(),
            auth,
        });
        self.recv()
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

// ---------------------------------------------------------------------------
// The harness-created test verse (SPEC-8 §5.1 rule 2: no legacy history claimed)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Transform {
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
}

fn transform(position: [f32; 3], rotation: [f32; 3], scale: [f32; 3]) -> Transform {
    Transform {
        position,
        rotation,
        scale,
    }
}

struct World {
    verse_id: String,
    petal_id: String,
    petal_scope: String,
    nodes: Vec<String>,
}

impl World {
    fn intent(&self, node_id: &str, transform: Transform) -> TransformIntent {
        TransformIntent {
            petal_scope: self.petal_scope.clone(),
            node_id: node_id.to_string(),
            position: transform.position,
            rotation: transform.rotation,
            scale: transform.scale,
        }
    }
}

/// The legacy hierarchy and `node_count` nodes, through the live `DbCommand` path. The three
/// hierarchy creates are D-CL20 kinds, so each records its per-kind bypass (W4-1).
fn create_world(harness: &Harness, tally: &mut KindTally, node_count: usize) -> World {
    harness.send(DbCommand::CreateVerse {
        name: "W4-9 harness verse".to_string(),
    });
    let verse_id = match harness.recv() {
        DbResult::VerseCreated { id, .. } => id,
        other => panic!("expected VerseCreated, got {other:?}"),
    };
    harness.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "F".to_string(),
    });
    let fractal_id = match harness.recv() {
        DbResult::FractalCreated { id, .. } => id,
        other => panic!("expected FractalCreated, got {other:?}"),
    };
    harness.send(DbCommand::CreatePetal {
        fractal_id: fractal_id.clone(),
        name: "P".to_string(),
    });
    let petal_id = match harness.recv() {
        DbResult::PetalCreated { id, .. } => id,
        other => panic!("expected PetalCreated, got {other:?}"),
    };
    for kind in ["CreateVerse", "CreateFractal", "CreatePetal"] {
        tally.legacy_without_candidate(kind);
    }
    let nodes = (0..node_count)
        .map(|index| {
            let node = harness.create_node(&petal_id, &format!("node-{index}"));
            tally.legacy_without_candidate("CreateNode");
            node
        })
        .collect();
    World {
        petal_scope: build_scope(&verse_id, Some(&fractal_id), Some(&petal_id)),
        verse_id,
        petal_id,
        nodes,
    }
}

// ---------------------------------------------------------------------------
// The run's canonical authority (the W4-7 durable sources, seeded with in-test keys)
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

struct Canon {
    verse_scope: Scope,
    genesis: Hash32,
    bump: Hash32,
    chain_id: Hash32,
}

impl Canon {
    fn verse_key(&self) -> ScopeKeyId {
        ScopeKeyId {
            scope: self.verse_scope,
            epoch: EPOCH,
        }
    }
}

/// A structural, no-payload envelope by the Manager, appended through the received door.
async fn append_structural(
    store: &SurrealVerifiedLogStore,
    verse_scope: Scope,
    kind: OperationKind,
    parents: Vec<Hash32>,
    counter: u32,
) -> Hash32 {
    let unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: kind.to_u16(),
        scope: verse_scope,
        author: Author::from_public_key(public_key(MANAGER)),
        capability: CapabilityRef {
            chain_id: Hash32([0x33; 32]),
            scope_epoch: 0,
        },
        schema_hash: Hash32([0x44; 32]),
        branch_id: BRANCH,
        parents,
        hlc: Hlc::new(STRUCTURAL_HLC_MS, counter),
        payload: PayloadRef::empty(),
    };
    let bytes = sign_envelope(&signing_key(MANAGER), &unsigned)
        .expect("sign structural operation")
        .encode_canonical()
        .expect("encode structural operation");
    let op_id = Hash32::of(&bytes);
    store
        .append_received(op_id, &bytes)
        .await
        .expect("append structural operation");
    op_id
}

/// Genesis first (SPEC-8 §5.1 rule 2), then the Manager's bump to [`EPOCH`], the author's
/// `append/op` chain, and the verse scope key in durable custody.
async fn seed_canonical(db: &Db, secrets: &Arc<dyn SecretStore>, world: &World) -> Canon {
    let verse = legacy_anchor(LegacyTable::Verse, &world.verse_id).expect("verse anchor");
    let verse_scope = Scope::verse_wide(verse);
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: 'manager' }")
        .bind(("did", principal(MANAGER).did))
        .bind(("scope", interim_authority_scope_key(&verse_scope)))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");

    let store = SurrealVerifiedLogStore::new(db.clone());
    // The SPEC-1 kind-2 branch_genesis shape of the golden `branch-genesis-empty-payload` vector
    // (zero parents, no-payload form), over THIS verse: the vector's own bytes name a fixture verse.
    let genesis =
        append_structural(&store, verse_scope, OperationKind::BranchGenesis, vec![], 0).await;
    let bump = append_structural(
        &store,
        verse_scope,
        OperationKind::ScopeEpochBump,
        vec![genesis],
        1,
    )
    .await;
    let view = DurableAuthorizationView::load(db, Some(ANCHOR))
        .await
        .expect("authorization view");
    SurrealScopeEpochStore::new(db.clone())
        .admit_epoch_bump(&store, &view, bump, &mut RevalidationGate::new())
        .await
        .expect("the Manager's bump admits");

    let now = SystemClock.now_ms();
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER),
        audience: principal(AUTHOR),
        parent_certificate_id: None,
        issuer_authority_id: ANCHOR,
        grant_scope: verse_scope,
        epoch_scope: verse_scope,
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
    let chain = Chain::from_certificates(&[certificate])
        .expect("chain")
        .encode_canonical()
        .expect("encode chain");
    let chain_id = ChainStore::load(db)
        .await
        .expect("chain store")
        .put_verified_chain(&chain)
        .await
        .expect("put the author's chain")
        .chain_id;

    let canon = Canon {
        verse_scope,
        genesis,
        bump,
        chain_id,
    };
    SecretStoreScopeKeyCustody::load(db, Arc::clone(secrets))
        .await
        .expect("custody")
        .take_custody(canon.verse_key(), SealingKey::from_bytes(SCOPE_KEY_BYTES))
        .await
        .expect("take custody of the verse key");
    canon
}

/// THE production verifier, with the run's one mapped schema registered on it.
async fn load_verifier(db: &Db, secrets: &Arc<dyn SecretStore>) -> DurableCandidateVerifier {
    let loaded = durable_candidate_verifier(db, Arc::clone(secrets), Some(ANCHOR))
        .await
        .expect("durable verifier");
    register_node_transform_schema(&loaded.verifier.parts().schemas)
        .expect("register node-transform-v1");
    loaded.verifier
}

fn run_derivation<Q: TransformQuantizer>(
    canon: &Canon,
    quantizer: Q,
    retained: Arc<RetainedCandidates>,
) -> TransformCandidateDerivation<Q, SystemClock> {
    TransformCandidateDerivation::new(
        TransformRunCharter {
            author: signing_key(AUTHOR),
            capability: CapabilityRef {
                chain_id: canon.chain_id,
                scope_epoch: EPOCH,
            },
            branch_id: BRANCH,
            verse_key_scope: canon.verse_key(),
            verse_key: SealingKey::from_bytes(SCOPE_KEY_BYTES),
            quantizer,
            clock: SystemClock,
        },
        [canon.bump],
        retained,
    )
    .expect("run charter")
}

// ---------------------------------------------------------------------------
// Corpus steps
// ---------------------------------------------------------------------------

/// Per-kind legacy outcomes and per-intent dispositions (§6.1: "by mutation kind").
#[derive(Default, Debug)]
struct KindTally {
    legacy_success: BTreeMap<String, u64>,
    with_candidate: BTreeMap<String, u64>,
    dispositions: BTreeMap<String, BTreeMap<String, u64>>,
}

impl KindTally {
    fn legacy_without_candidate(&mut self, kind: &str) {
        *self.legacy_success.entry(kind.to_string()).or_default() += 1;
        self.disposition(
            kind,
            CorrelationDisposition::LegacySucceededWithoutCandidate,
        );
    }

    fn legacy_with_candidate(&mut self, kind: &str) {
        *self.legacy_success.entry(kind.to_string()).or_default() += 1;
        *self.with_candidate.entry(kind.to_string()).or_default() += 1;
    }

    fn disposition(&mut self, kind: &str, disposition: CorrelationDisposition) {
        *self
            .dispositions
            .entry(kind.to_string())
            .or_default()
            .entry(disposition.as_token().to_string())
            .or_default() += 1;
    }

    fn total_legacy_success(&self) -> u64 {
        self.legacy_success.values().sum()
    }

    fn total_with_candidate(&self) -> u64 {
        self.with_candidate.values().sum()
    }

    fn count(&self, kind: &str, disposition: CorrelationDisposition) -> u64 {
        self.dispositions
            .get(kind)
            .and_then(|by| by.get(disposition.as_token()))
            .copied()
            .unwrap_or(0)
    }
}

/// One intent's candidate, retained exactly as derived.
struct Captured {
    intent: TransformIntent,
    set: MigrationCandidateSet,
}

impl Captured {
    fn member(&self) -> &MigrationCandidateMember {
        &self.set.members[0]
    }
}

/// One `UpdateNodeTransform` intent through the dual-emit boundary in `DualEmitShadow`.
async fn emit<Q: TransformQuantizer>(
    db: &Db,
    run_id: &str,
    derivation: &TransformCandidateDerivation<Q, SystemClock>,
    verifier: &DurableCandidateVerifier,
    intent: TransformIntent,
    tally: &mut KindTally,
) -> Captured {
    let materializer =
        TransformShadowMaterializer::new(derivation.retained(), &verifier.parts().scope_keys);
    let ledger = SurrealShadowLedgerWriter::new(db, LedgerBounds::default());
    let outcome = dual_emit_update_node_transform(
        db,
        MigrationMode::DualEmitShadow,
        run_id,
        &intent,
        derivation,
        &materializer,
        &ledger,
    )
    .await
    .expect("the intent passes the legacy precondition")
    .expect("the legacy mutation succeeds");
    match outcome {
        DualEmitOutcome::ShadowCaptured { disposition, .. } => assert_eq!(
            disposition,
            CorrelationDisposition::AppendPending,
            "the candidate reduces, so it is captured pending admission"
        ),
        other => panic!("a mapped intent must be shadow-captured, got {other:?}"),
    }
    tally.legacy_with_candidate(TRANSFORM_MUTATION_KIND);
    let set = derivation
        .retained()
        .latest_set_for(&intent.intent_digest_hex())
        .expect("derived set is retained");
    Captured { intent, set }
}

/// Admits one captured candidate through the production door, then advances the frontier.
async fn admit<Q: TransformQuantizer>(
    db: &Db,
    verifier: &DurableCandidateVerifier,
    derivation: &TransformCandidateDerivation<Q, SystemClock>,
    captured: &Captured,
) -> VerifiedEnvelopeMeta {
    let candidate = verify_candidate(&captured.member().envelope_bytes).expect("ingress");
    let ciphertext = derivation
        .retained()
        .ciphertext(&candidate.envelope.unsigned.payload.ciphertext_hash)
        .expect("ciphertext retained");
    let admitted = admit_and_append_durably(
        verifier,
        &SurrealVerifiedLogStore::new(db.clone()),
        &NoPrecondition,
        &captured.member().envelope_bytes,
        Some(&ciphertext),
    )
    .await
    .expect("the production verifier admits the candidate");
    assert_eq!(admitted.append.append_outcome, AppendOutcome::Appended);
    derivation.record_admitted(&admitted.append.meta);
    admitted.append.meta
}

/// Appends the intent's final disposition as a NEW ledger row (the ledger is append-only).
async fn record_disposition(
    db: &Db,
    run_id: &str,
    captured: &Captured,
    op_id: Hash32,
    disposition: CorrelationDisposition,
) {
    SurrealShadowLedgerWriter::new(db, LedgerBounds::default())
        .append(ShadowLedgerEntry {
            run_id: run_id.to_string(),
            intent_digest_hex: captured.intent.intent_digest_hex(),
            mutation_kind: TRANSFORM_MUTATION_KIND.to_string(),
            run_local_correlation_id: captured.set.run_local_correlation_id.clone(),
            member_op_ids_json: serde_json::to_string(&[op_id_to_hex(op_id)]).expect("json"),
            candidate_byte_hashes_json: serde_json::to_string(
                &captured.set.candidate_byte_hashes(),
            )
            .expect("json"),
            disposition,
        })
        .await
        .expect("ledger append");
}

/// The admitted-closure state of a run and every comparison it made.
#[derive(Default)]
struct Evidence {
    /// Admitted members in ARRIVAL (admission) order; replay re-orders them causally.
    admitted: Vec<MigrationCandidateMember>,
    /// Nodes with at least one admitted transform: the comparison subject set.
    covered_nodes: Vec<String>,
    comparisons: Vec<serde_json::Value>,
    normalizations: Vec<serde_json::Value>,
    counters: RunCounters,
    replay_roots: Vec<String>,
}

impl Evidence {
    fn admitted_member(&mut self, captured: &Captured) {
        self.admitted.push(captured.member().clone());
        if !self.covered_nodes.contains(&captured.intent.node_id) {
            self.covered_nodes.push(captured.intent.node_id.clone());
        }
        self.counters.candidates_expected += 1;
        self.counters.candidates_admitted_or_dispositioned += 1;
    }
}

/// Replays the admitted closure three times (causal order) and compares it with the legacy
/// export of the covered nodes; records the comparison and returns it with the replay root.
#[allow(clippy::too_many_arguments)]
async fn compare_closure(
    db: &Db,
    run_id: &str,
    evidence: &mut Evidence,
    materializer: &dyn ShadowMaterializer,
    correlation: &str,
    candidate_id: &str,
    scope: &str,
) -> (ComparisonRecord, String) {
    let closure = in_causal_order(&evidence.admitted).expect("causal order");
    evidence.counters.replay_attempts += 1;
    let replay = replay_admitted_closure_three_times(&closure, materializer)
        .expect("three byte-identical replays from empty");
    evidence.counters.replay_root_matches += 1;
    evidence.replay_roots.push(replay.root_hex.clone());

    let legacy = export_legacy_transforms(db, &evidence.covered_nodes, &ExactTransformQuantizer)
        .await
        .expect("legacy export");
    let record = compare_projections(
        run_id,
        correlation,
        scope,
        candidate_id,
        &legacy.export,
        &replay.export,
    );
    evidence.counters.unapproved_field_differences += record.differences.len() as u64;
    for normalization in &legacy.normalizations {
        evidence.normalizations.push(serde_json::json!({
            "correlation": correlation,
            "field_path": normalization.field_path,
            "source": normalization.source,
            "normalized": format!("{:?}", normalization.normalized),
        }));
    }
    evidence.comparisons.push(serde_json::json!({
        "correlation": correlation,
        "candidate_id": candidate_id,
        "legacy_export_hash": record.legacy_export_hash,
        "shadow_export_hash": record.shadow_export_hash,
        "shadow_replay_root": replay.root_hex,
        "equal": record.is_equal(),
        "differences": record.differences.iter().map(|difference| serde_json::json!({
            "field_path": difference.field_path,
            "class": format!("{:?}", difference.class),
            "legacy": format!("{:?}", difference.legacy_value),
            "shadow": format!("{:?}", difference.shadow_value),
        })).collect::<Vec<_>>(),
    }));
    (record, replay.root_hex)
}

/// One sequential transform intent: boundary, admission, prefix comparison, final disposition.
#[allow(clippy::too_many_arguments)]
async fn sequential_transform(
    db: &Db,
    run_id: &str,
    world: &World,
    derivation: &TransformCandidateDerivation<ExactTransformQuantizer, SystemClock>,
    verifier: &DurableCandidateVerifier,
    tally: &mut KindTally,
    evidence: &mut Evidence,
    node_id: &str,
    values: Transform,
) -> Captured {
    let captured = emit(
        db,
        run_id,
        derivation,
        verifier,
        world.intent(node_id, values),
        tally,
    )
    .await;
    let meta = admit(db, verifier, derivation, &captured).await;
    evidence.admitted_member(&captured);
    let materializer =
        TransformShadowMaterializer::new(derivation.retained(), &verifier.parts().scope_keys);
    let (record, _) = compare_closure(
        db,
        run_id,
        evidence,
        &materializer,
        &captured.intent.intent_digest_hex(),
        &op_id_to_hex(meta.op_id),
        &world.petal_scope,
    )
    .await;
    let disposition = if record.is_equal() {
        CorrelationDisposition::ComparedEqual
    } else {
        CorrelationDisposition::ComparedDifferent
    };
    tally.disposition(TRANSFORM_MUTATION_KIND, disposition);
    record_disposition(db, run_id, &captured, meta.op_id, disposition).await;
    assert!(
        record.is_equal(),
        "prefix parity after {node_id}: {:?}",
        record.differences
    );
    captured
}

async fn verified_op_ids(db: &Db) -> Vec<Hash32> {
    SurrealVerifiedLogStore::new(db.clone())
        .all_rows()
        .await
        .expect("verified log scan")
        .iter()
        .map(|row| {
            op_id_from_hex(row["op_id_hex"].as_str().expect("op_id_hex")).expect("op id hex")
        })
        .collect()
}

async fn edit_seq(db: &Db, node_id: &str) -> i64 {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT edit_seq FROM node WHERE node_id = $nid")
        .bind(("nid", node_id.to_string()))
        .await
        .expect("edit_seq query")
        .take(0)
        .expect("edit_seq rows");
    rows[0]["edit_seq"].as_i64().expect("edit_seq")
}

async fn ledger_rows(db: &Db, run_id: &str) -> usize {
    load_ledger_entries_for_run(db, run_id)
        .await
        .expect("ledger rows")
        .len()
}

/// The stored transform columns and latest op-log payload of one node.
async fn legacy_transform_row(db: &Db, node_id: &str) -> serde_json::Value {
    let node: Vec<serde_json::Value> = db
        .query(
            "SELECT position, elevation, rotation, scale, edit_seq FROM node WHERE node_id = $nid",
        )
        .bind(("nid", node_id.to_string()))
        .await
        .expect("node row")
        .take(0)
        .expect("node rows");
    let op_log: Vec<serde_json::Value> = db
        .query(
            "SELECT op_type, payload, lamport_clock FROM op_log WHERE payload.node_id = $nid \
             ORDER BY lamport_clock DESC LIMIT 1",
        )
        .bind(("nid", node_id.to_string()))
        .await
        .expect("op_log row")
        .take(0)
        .expect("op_log rows");
    let mut payload = op_log[0]["payload"].clone();
    payload
        .as_object_mut()
        .expect("payload object")
        .remove("node_id");
    serde_json::json!({
        "node": node[0],
        "op_type": op_log[0]["op_type"],
        "payload": payload,
    })
}

fn evidence_path(run_id: &str) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"));
    target
        .join("migration-evidence")
        .join(format!("{run_id}.json"))
}

// ---------------------------------------------------------------------------
// Flow 1: the transform corpus
// ---------------------------------------------------------------------------

#[test]
fn transform_corpus_reaches_projection_parity_across_three_replays() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let retained = Arc::new(RetainedCandidates::new());
    let run_id = format!("w4-9-transform-{}", ulid::Ulid::new());
    let mut tally = KindTally::default();
    let mut evidence = Evidence::default();
    let mut uncovered_tombstone: Option<ComparisonRecord> = None;

    // ---------------- before the restart ----------------
    let harness = Harness::open(dir.path());
    let world = create_world(&harness, &mut tally, 3);
    let (node_a, node_b, node_c) = (
        world.nodes[0].clone(),
        world.nodes[1].clone(),
        world.nodes[2].clone(),
    );
    let db = harness.db();
    let canon = runtime.block_on(seed_canonical(&db, &secrets, &world));
    let derivation = run_derivation(&canon, ExactTransformQuantizer, Arc::clone(&retained));

    let (closure_root_before, last_before, edit_seqs_before, ledger_before, log_before) = runtime
        .block_on(async {
            let verifier = load_verifier(&db, &secrets).await;
            sequential_transform(
                &db,
                &run_id,
                &world,
                &derivation,
                &verifier,
                &mut tally,
                &mut evidence,
                &node_a,
                transform([1.25, 0.5, -3.0], [0.1, 0.2, 0.3], [1.0, 1.0, 1.0]),
            )
            .await;
            sequential_transform(
                &db,
                &run_id,
                &world,
                &derivation,
                &verifier,
                &mut tally,
                &mut evidence,
                &node_b,
                transform([-7.125, 2.0, 0.333], [0.0, 1.5707964, 0.0], [2.0, 0.5, 1.0]),
            )
            .await;
            sequential_transform(
                &db,
                &run_id,
                &world,
                &derivation,
                &verifier,
                &mut tally,
                &mut evidence,
                &node_a,
                transform([1.3, 0.5, -3.0], [0.1, 0.2, 0.31], [1.0, 1.0, 1.25]),
            )
            .await;

            // Concurrent intents: both derived from the same frontier, so they are causal
            // siblings; legacy applies them in arrival order (second wins).
            let first = emit(
                &db,
                &run_id,
                &derivation,
                &verifier,
                world.intent(&node_b, transform([10.0, 0.0, 0.0], [0.0; 3], [1.0; 3])),
                &mut tally,
            )
            .await;
            let second = emit(
                &db,
                &run_id,
                &derivation,
                &verifier,
                world.intent(&node_b, transform([20.0, 0.0, 0.0], [0.0; 3], [1.0; 3])),
                &mut tally,
            )
            .await;
            assert_eq!(
                first.member().required_parent_op_ids,
                second.member().required_parent_op_ids,
                "concurrent intents name the same parents"
            );
            // Admitted in the REVERSE of their derivation order: replay must not care.
            let second_meta = admit(&db, &verifier, &derivation, &second).await;
            let first_meta = admit(&db, &verifier, &derivation, &first).await;
            evidence.admitted_member(&second);
            evidence.admitted_member(&first);
            assert_eq!(
                derivation.frontier().len(),
                2,
                "two siblings are the frontier"
            );
            let materializer = TransformShadowMaterializer::new(
                derivation.retained(),
                &verifier.parts().scope_keys,
            );
            let (record, causal_root) = compare_closure(
                &db,
                &run_id,
                &mut evidence,
                &materializer,
                "concurrent-pair",
                &op_id_to_hex(second_meta.op_id),
                &world.petal_scope,
            )
            .await;
            assert!(
                record.is_equal(),
                "causal order picks the later intent, as legacy did: {:?}",
                record.differences
            );
            let arrival_root =
                replay_admitted_closure_three_times(&evidence.admitted, &materializer)
                    .expect("arrival-order replay")
                    .root_hex;
            assert_ne!(
                arrival_root, causal_root,
                "folding in arrival order would have picked the other sibling"
            );
            for (captured, meta) in [(&first, &first_meta), (&second, &second_meta)] {
                tally.disposition(
                    TRANSFORM_MUTATION_KIND,
                    CorrelationDisposition::ComparedEqual,
                );
                record_disposition(
                    &db,
                    &run_id,
                    captured,
                    meta.op_id,
                    CorrelationDisposition::ComparedEqual,
                )
                .await;
            }

            // A malformed candidate: a captured envelope with one signature byte flipped.
            let log_count = verified_op_ids(&db).await.len();
            let mut malformed = second.member().envelope_bytes.clone();
            *malformed.last_mut().expect("bytes") ^= 0x01;
            let refused = admit_and_append_durably(
                &verifier,
                &SurrealVerifiedLogStore::new(db.clone()),
                &NoPrecondition,
                &malformed,
                None,
            )
            .await;
            assert!(
                matches!(
                    refused,
                    Err(DurableAdmissionError::Admission(
                        AdmissionRejection::Rejected {
                            outcome: AdmissionOutcome::InvalidEnvelope { .. },
                            ..
                        }
                    ))
                ),
                "{refused:?}"
            );
            assert_eq!(verified_op_ids(&db).await.len(), log_count);
            evidence.counters.candidates_expected += 1;
            evidence.counters.candidates_admitted_or_dispositioned += 1;
            tally.disposition(
                "injected-malformed-candidate",
                CorrelationDisposition::CandidateRejected,
            );

            // Denied writes leave no evidence anywhere: a policy-denied tombstone (real DB
            // thread) and a mapped intent refused by the live handler's own precondition.
            let ledger_count = ledger_rows(&db, &run_id).await;
            let retained_count = derivation.retained().all_sets().len();
            let legacy_before =
                export_legacy_transforms(&db, &evidence.covered_nodes, &ExactTransformQuantizer)
                    .await
                    .expect("legacy export");
            let denied = harness.tombstone(&node_a, CallerAuth::Anonymous);
            assert!(
                matches!(&denied, DbResult::Error(text) if text.contains("denied")),
                "{denied:?}"
            );
            let missing = world.intent(
                "01HZZZZZZZZZZZZZZZZZZZZZZZ",
                transform([1.0; 3], [0.0; 3], [1.0; 3]),
            );
            let precondition = dual_emit_update_node_transform(
                &db,
                MigrationMode::DualEmitShadow,
                &run_id,
                &missing,
                &derivation,
                &materializer,
                &SurrealShadowLedgerWriter::new(&db, LedgerBounds::default()),
            )
            .await;
            assert!(
                matches!(precondition, Err(TransformPreconditionError::Legacy(_))),
                "a missing node never reaches derivation"
            );
            assert_eq!(ledger_rows(&db, &run_id).await, ledger_count);
            assert_eq!(derivation.retained().all_sets().len(), retained_count);
            assert_eq!(verified_op_ids(&db).await.len(), log_count);
            assert_eq!(
                export_legacy_transforms(&db, &evidence.covered_nodes, &ExactTransformQuantizer)
                    .await
                    .expect("legacy export"),
                legacy_before
            );

            let mut edit_seqs = BTreeMap::new();
            for node in [&node_a, &node_b, &node_c] {
                edit_seqs.insert(node.clone(), edit_seq(&db, node).await);
            }
            (
                causal_root,
                second.member().envelope_bytes.clone(),
                edit_seqs,
                ledger_rows(&db, &run_id).await,
                verified_op_ids(&db).await,
            )
        });
    drop(db);
    drop(harness); // DB thread joined, store closed: a mid-run interruption.

    // ---------------- after the restart ----------------
    evidence.counters.interruption_points_planned += 1;
    let harness = Harness::open(dir.path());
    let db = harness.db();
    runtime.block_on(async {
        let verifier = load_verifier(&db, &secrets).await;

        // Crash recovery: the evidence is durable, exactly once, and replays to the same root.
        assert_eq!(ledger_rows(&db, &run_id).await, ledger_before);
        assert_eq!(verified_op_ids(&db).await, log_before);
        let again = admit_and_append_durably(
            &verifier,
            &SurrealVerifiedLogStore::new(db.clone()),
            &NoPrecondition,
            &last_before,
            None,
        )
        .await
        .expect("the stored ciphertext lets the same bytes re-admit");
        assert_eq!(
            again.append.append_outcome,
            AppendOutcome::AlreadyPresent,
            "no duplicate canonical effect"
        );
        for (node, before) in &edit_seqs_before {
            assert_eq!(
                edit_seq(&db, node).await,
                *before,
                "no duplicate legacy mutation"
            );
        }
        // The durable artifact store and keystore, not process memory, reproduce the root.
        let durable = TransformShadowMaterializer::new(
            &verifier.parts().ciphertexts,
            &verifier.parts().scope_keys,
        );
        let closure = in_causal_order(&evidence.admitted).expect("causal order");
        evidence.counters.replay_attempts += 1;
        let replay =
            replay_admitted_closure_three_times(&closure, &durable).expect("durable replay");
        assert_eq!(replay.root_hex, closure_root_before);
        evidence.counters.replay_root_matches += 1;
        assert!(log_before.contains(&canon.genesis) && log_before.contains(&canon.bump));
        assert!(derivation
            .frontier()
            .iter()
            .all(|head| log_before.contains(head)));
        evidence.counters.interruption_points_resolved_cleanly += 1;

        // The run continues: a merge of the two siblings, then more edits.
        let merge = sequential_transform(
            &db,
            &run_id,
            &world,
            &derivation,
            &verifier,
            &mut tally,
            &mut evidence,
            &node_c,
            transform([0.001, 0.002, 0.003], [-0.5, 0.25, 3.0], [0.75, 0.75, 0.75]),
        )
        .await;
        assert_eq!(
            merge.member().required_parent_op_ids.len(),
            2,
            "the post-restart intent merges both siblings"
        );
        sequential_transform(
            &db,
            &run_id,
            &world,
            &derivation,
            &verifier,
            &mut tally,
            &mut evidence,
            &node_a,
            transform([100.5, -0.0625, 7.0], [3.1, -3.1, 0.0], [1.0, 2.0, 3.0]),
        )
        .await;

        // A tombstone through the live path: TombstoneNode is unmapped, so it has no candidate
        // and the comparator must surface it rather than mask it (SPEC-8 §5.3 rule 2).
        let tombstoned = harness.tombstone(&node_c, CallerAuth::Local);
        assert!(
            matches!(tombstoned, DbResult::NodeDeleted { .. }),
            "{tombstoned:?}"
        );
        tally.legacy_without_candidate("TombstoneNode");
        let (record, final_root) = compare_closure(
            &db,
            &run_id,
            &mut evidence,
            &durable,
            "final-after-uncovered-tombstone",
            "final",
            &world.petal_scope,
        )
        .await;
        let tombstone_path =
            transform_field_path(legacy_anchor(LegacyTable::Node, &node_c).expect("anchor"));
        assert_eq!(record.differences.len(), 1, "{:?}", record.differences);
        let difference = &record.differences[0];
        assert_eq!(difference.field_path, tombstone_path);
        assert_eq!(difference.class, FieldDifferenceClass::ValueMismatch);
        assert_eq!(difference.legacy_value, Some(QuantizedValue::Tombstoned));
        uncovered_tombstone = Some(record);

        // Arrival order never changes the root: the closure replayed from a reversed list.
        let mut reversed = evidence.admitted.clone();
        reversed.reverse();
        evidence.counters.replay_attempts += 1;
        let reversed_replay = replay_admitted_closure_three_times(
            &in_causal_order(&reversed).expect("causal order"),
            &durable,
        )
        .expect("reversed replay");
        assert_eq!(reversed_replay.root_hex, final_root);
        evidence.counters.replay_root_matches += 1;

        // Candidate integrity over every retained candidate.
        let log_now = verified_op_ids(&db).await;
        for set in derivation.retained().all_sets() {
            evidence.counters.candidates_retained += 1;
            let member = &set.members[0];
            let candidate = verify_candidate(&member.envelope_bytes).expect("ingress");
            let ciphertext = derivation
                .retained()
                .ciphertext(&candidate.envelope.unsigned.payload.ciphertext_hash);
            if set.candidate_byte_hashes() == vec![op_id_to_hex(candidate.meta.op_id)]
                && ciphertext.is_some()
                && log_now.contains(&candidate.meta.op_id)
            {
                evidence.counters.candidates_decoded_reencoded_ok += 1;
            }
        }

        // Drift guard: the composition's legacy leg writes exactly what the live handler does.
        let live_node = harness.create_node(&world.petal_id, "drift-live");
        let composed_node = harness.create_node(&world.petal_id, "drift-composed");
        let values = transform([3.5, -1.25, 9.0], [0.7, -0.2, 1.1], [1.5, 1.0, 0.5]);
        harness.live_update_node_transform(&live_node, values);
        let legacy_only = dual_emit_update_node_transform(
            &db,
            MigrationMode::LegacyOnly,
            &run_id,
            &world.intent(&composed_node, values),
            &derivation,
            &durable,
            &SurrealShadowLedgerWriter::new(&db, LedgerBounds::default()),
        )
        .await
        .expect("precondition")
        .expect("legacy leg");
        assert!(matches!(legacy_only, DualEmitOutcome::LegacyOnlyResult(())));
        assert_eq!(
            legacy_transform_row(&db, &composed_node).await,
            legacy_transform_row(&db, &live_node).await,
            "the composed legacy leg drifted from handlers/transform.rs"
        );
    });

    // ---------------- the measurement report ----------------
    assert_eq!(
        tally.count(
            TRANSFORM_MUTATION_KIND,
            CorrelationDisposition::LegacySucceededWithoutCandidate
        ),
        0,
        "every mapped intent was captured"
    );
    assert_eq!(
        tally.with_candidate[TRANSFORM_MUTATION_KIND],
        tally.legacy_success[TRANSFORM_MUTATION_KIND]
    );
    assert_eq!(
        tally.count(
            TRANSFORM_MUTATION_KIND,
            CorrelationDisposition::ComparedEqual
        ),
        tally.legacy_success[TRANSFORM_MUTATION_KIND],
        "every transform intent compared equal at its own prefix"
    );
    assert_eq!(
        evidence.counters.replay_root_matches,
        evidence.counters.replay_attempts
    );
    assert_eq!(
        evidence.counters.candidates_decoded_reencoded_ok,
        evidence.counters.candidates_retained
    );

    let mut inventory = MappingInventory::seeded();
    let materializer_version = node_transform_materializer_version();
    inventory
        .register_mapped(
            TRANSFORM_MUTATION_KIND,
            hex::encode(node_transform_schema_hash().as_bytes()),
            format!(
                "{} (registry id {})",
                "node-transform-shadow@1",
                hex::encode(materializer_version.registry_id().as_bytes())
            ),
        )
        .expect("the run charter maps exactly one kind");
    assert!(matches!(
        inventory.register_mapped("CreateVerse", String::new(), String::new()),
        Err(MappingRegistrationError::DCl20Blocked { .. })
    ));

    evidence.counters.legacy_success = tally.total_legacy_success();
    evidence.counters.legacy_success_with_candidate = tally.total_with_candidate();
    let unmapped_bypasses = bypass_count();
    let report = MeasurementReport::generate(&evidence.counters, &inventory, unmapped_bypasses);
    for kind in ["CreateVerse", "CreateFractal", "CreatePetal"] {
        assert!(
            bypass_count_for(kind) >= 1,
            "{kind} records its D-CL20 bypass"
        );
    }
    assert!(
        report.blocks_promotion(unmapped_bypasses),
        "promotion stays blocked while the D-CL20 kinds are unmapped (migration/AGENTS.md §always-blocked)"
    );
    let measurement = |name: &str| {
        report
            .measurements
            .iter()
            .find(|measurement| measurement.name == name)
            .expect("measurement")
    };
    assert!(!measurement("ingress_coverage").meets_promotion_requirement);
    assert!(measurement("determinism").meets_promotion_requirement);
    assert!(measurement("crash_recovery").meets_promotion_requirement);
    assert!(measurement("candidate_integrity").meets_promotion_requirement);
    assert!(measurement("admission_and_replay_completeness").meets_promotion_requirement);
    assert!(
        !measurement("projection_parity").meets_promotion_requirement,
        "the uncovered tombstone is an unapproved difference"
    );

    let uncovered = uncovered_tombstone.expect("final comparison");
    let d_cl20_bypass_counts: serde_json::Map<String, serde_json::Value> = D_CL20_MUTATION_KINDS
        .iter()
        .map(|kind| (kind.to_string(), serde_json::json!(bypass_count_for(kind))))
        .collect();
    let document = serde_json::json!({
        "run_id": run_id,
        "evidence_class": "dev run on a harness-created test verse (owner R7 decision 3); not the formal SPEC-8 §6.1 validation run",
        "pinned": {
            "mapped_kinds": [TRANSFORM_MUTATION_KIND],
            "schema": "node-transform-v1",
            "schema_hash": hex::encode(node_transform_schema_hash().as_bytes()),
            "materializer": "node-transform-shadow@1",
            "materializer_registry_id": hex::encode(materializer_version.registry_id().as_bytes()),
            "comparator": "migration::comparator (quantization-only)",
            "mode": MigrationMode::DualEmitShadow.as_token(),
            "branch_genesis_op_id": op_id_to_hex(canon.genesis),
            "scope_epoch_bump_op_id": op_id_to_hex(canon.bump),
        },
        "measurements": report.measurements.iter().map(|measurement| serde_json::json!({
            "name": measurement.name,
            "numerator": measurement.numerator,
            "denominator": measurement.denominator,
            "meets_promotion_requirement": measurement.meets_promotion_requirement,
        })).collect::<Vec<_>>(),
        "blocks_promotion": report.blocks_promotion(unmapped_bypasses),
        "unmapped_bypass_count": unmapped_bypasses,
        "d_cl20_bypass_counts": d_cl20_bypass_counts,
        "by_mutation_kind": {
            "legacy_success": tally.legacy_success,
            "legacy_success_with_candidate": tally.with_candidate,
            "dispositions": tally.dispositions,
        },
        "comparisons": evidence.comparisons,
        "normalizations": evidence.normalizations,
        "replay_roots": evidence.replay_roots,
        "unresolved": [{
            "reason": "TombstoneNode is unmapped: the legacy tombstone has no candidate",
            "field_paths": uncovered.differences.iter().map(|difference| difference.field_path.clone()).collect::<Vec<_>>(),
        }],
        "not_measured_structurally_zero": ["authorization_and_privacy", "storage_accounting", "network_prohibition"],
    });
    let path = evidence_path(&run_id);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("evidence dir");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&document).expect("evidence json"),
    )
    .expect("write evidence");
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read evidence")).expect("parse");
    assert_eq!(written["blocks_promotion"], serde_json::json!(true));
}

// ---------------------------------------------------------------------------
// Flow 2: the harness can fail
// ---------------------------------------------------------------------------

/// A deliberately lossy mapping: positions rounded to whole millimetres.
struct MillimetreQuantizer;

impl TransformQuantizer for MillimetreQuantizer {
    fn quantize(
        &self,
        position: [f64; 3],
        rotation_radians: [f64; 3],
        scale: [f64; 3],
    ) -> Result<QuantizedTransform, QuantizationError> {
        let mut exact = ExactTransformQuantizer.quantize(position, rotation_radians, scale)?;
        let [x, y, z] = exact.position_nm.as_array();
        let millimetre = |nm: i64| (nm as f64 / 1_000_000.0).round() as i64 * 1_000_000;
        exact.position_nm = [millimetre(x), millimetre(y), millimetre(z)].into();
        Ok(exact)
    }
}

#[test]
fn a_lossy_transform_mapping_is_detected_as_a_difference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let run_id = format!("w4-9-lossy-{}", ulid::Ulid::new());
    let mut tally = KindTally::default();

    let harness = Harness::open(dir.path());
    let world = create_world(&harness, &mut tally, 1);
    let db = harness.db();
    runtime.block_on(async {
        let canon = seed_canonical(&db, &secrets, &world).await;
        let verifier = load_verifier(&db, &secrets).await;
        let lossy = run_derivation(
            &canon,
            MillimetreQuantizer,
            Arc::new(RetainedCandidates::new()),
        );

        let captured = emit(
            &db,
            &run_id,
            &lossy,
            &verifier,
            world.intent(
                &world.nodes[0],
                transform([1.234_567_8, 0.0, 0.0], [0.0; 3], [1.0; 3]),
            ),
            &mut tally,
        )
        .await;
        admit(&db, &verifier, &lossy, &captured).await;

        let mut evidence = Evidence::default();
        evidence.admitted_member(&captured);
        let materializer =
            TransformShadowMaterializer::new(lossy.retained(), &verifier.parts().scope_keys);
        let (record, _) = compare_closure(
            &db,
            &run_id,
            &mut evidence,
            &materializer,
            "lossy",
            "lossy",
            &world.petal_scope,
        )
        .await;
        assert!(
            !record.is_equal(),
            "a rounding mapping must not compare equal"
        );
        assert_eq!(record.differences.len(), 1, "{:?}", record.differences);
        assert_eq!(
            record.differences[0].field_path,
            transform_field_path(
                legacy_anchor(LegacyTable::Node, &world.nodes[0]).expect("anchor")
            )
        );
        assert_eq!(
            record.differences[0].class,
            FieldDifferenceClass::ValueMismatch
        );
        assert_eq!(evidence.counters.unapproved_field_differences, 1);
    });
}
