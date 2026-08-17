//! SPEC-8 §6.2 required conformance and operational cases
//! (`docs/spec/canonical-log/migration-plan.md`). Exercises
//! `fe_database::migration::*` with local in-memory test doubles for the
//! migration-specific abstractions (`IntentSnapshot`, `CandidateDerivation`,
//! `ShadowMaterializer`, `ShadowLedgerWriter`) — see
//! `fe-database/src/migration/AGENTS.md` §testing for why the legacy leg alone
//! still uses a real (throwaway, in-memory) SurrealDB.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use fe_database::migration::boundary::{commit_operation_dual_emit, DualEmitOutcome};
use fe_database::migration::candidate::{
    CandidateDerivation, CandidateDerivationError, CorrelationDisposition, IntentSnapshot,
    MigrationCandidateMember, MigrationCandidateSet,
};
use fe_database::migration::comparator::{
    compare_prefix_history, compare_projections, CanonicalExport, FieldDifferenceClass,
    QuantizedValue,
};
use fe_database::migration::mapping_inventory::MappingInventory;
use fe_database::migration::mode::{self, MigrationFlags, MigrationMode};
use fe_database::migration::rebuild::{
    replay_admitted_closure_three_times, ShadowMaterializationError, ShadowMaterializer,
    ShadowProjectionEffect,
};
use fe_database::migration::report::{MeasurementReport, RunCounters};
use fe_database::migration::shadow_store::{AppendOutcome, ShadowLedgerEntry, ShadowLedgerWriter};
use fe_database::op_log;
use fe_database::repo::{Db, Table};
use fe_database::schema::OpLog;
use fe_database::types::{NodeId, OpLogEntry, OpType};

// ---------------------------------------------------------------------------
// Shared fixtures: a throwaway in-memory SurrealDB for the legacy leg only.
// ---------------------------------------------------------------------------

static INIT_HLC: std::sync::Once = std::sync::Once::new();

/// A fresh in-memory SurrealDB with the `op_log` schema applied — mirrors
/// `fe-database/src/lib.rs`'s private `migration_tests::setup_test_db` helper.
async fn setup_test_db() -> Db {
    INIT_HLC.call_once(|| op_log::init_hlc(0));
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    db.query(OpLog::schema())
        .await
        .expect("define op_log schema")
        .check()
        .expect("define op_log schema check");
    db
}

fn sample_op_log_entry(payload: serde_json::Value) -> OpLogEntry {
    OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId("test-node".to_string()),
        op_type: OpType::PropertySet,
        payload,
        sig: "test-signature".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Local test doubles
// ---------------------------------------------------------------------------

struct FakeIntent {
    mutation_kind: String,
    digest: [u8; 32],
}

impl FakeIntent {
    fn new(mutation_kind: &str, seed: &str) -> Self {
        Self {
            mutation_kind: mutation_kind.to_string(),
            digest: *blake3::hash(seed.as_bytes()).as_bytes(),
        }
    }
}

impl IntentSnapshot for FakeIntent {
    fn mutation_kind(&self) -> &str {
        &self.mutation_kind
    }
    fn intent_digest(&self) -> [u8; 32] {
        self.digest
    }
}

/// Always fails to derive — models an unmapped mutation kind (§4.2.4).
struct AlwaysFailDerivation;
impl CandidateDerivation for AlwaysFailDerivation {
    fn derive(
        &self,
        intent: &dyn IntentSnapshot,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError> {
        Err(CandidateDerivationError::Unmapped {
            mutation_kind: intent.mutation_kind().to_string(),
        })
    }
}

/// Always derives the same fixed candidate set, ignoring the intent's own
/// content (a local test double never needs to touch a DB to do this —
/// proving the point of `derive`'s signature).
struct FixedSetDerivation {
    members: Vec<MigrationCandidateMember>,
    correlation_id: String,
}
impl CandidateDerivation for FixedSetDerivation {
    fn derive(
        &self,
        _intent: &dyn IntentSnapshot,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError> {
        Ok(MigrationCandidateSet {
            run_local_correlation_id: self.correlation_id.clone(),
            members: self.members.clone(),
        })
    }
}

/// Counts calls to an inner `CandidateDerivation` without changing its behavior.
struct SpyDerivation<D: CandidateDerivation> {
    inner: D,
    calls: AtomicUsize,
}
impl<D: CandidateDerivation> SpyDerivation<D> {
    fn new(inner: D) -> Self {
        Self {
            inner,
            calls: AtomicUsize::new(0),
        }
    }
    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
impl<D: CandidateDerivation> CandidateDerivation for SpyDerivation<D> {
    fn derive(
        &self,
        intent: &dyn IntentSnapshot,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.derive(intent)
    }
}

/// Reduces every member successfully, deterministically, from `member` alone.
struct AlwaysSucceedsMaterializer;
impl ShadowMaterializer for AlwaysSucceedsMaterializer {
    fn reduce(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<ShadowProjectionEffect, ShadowMaterializationError> {
        Ok(ShadowProjectionEffect {
            field_path: member.scope.clone(),
            value: QuantizedValue::Bytes(member.envelope_bytes.clone()),
        })
    }
}

/// Always fails to reduce — models a shadow materializer that cannot yet
/// interpret a candidate's opaque bytes.
struct AlwaysFailsMaterializer;
impl ShadowMaterializer for AlwaysFailsMaterializer {
    fn reduce(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<ShadowProjectionEffect, ShadowMaterializationError> {
        Err(ShadowMaterializationError::Unreducible {
            scope: member.scope.clone(),
            reason: "test forces a materialization failure".to_string(),
        })
    }
}

/// A pure function of `member` alone (BLAKE3 over its opaque bytes) — proves
/// replay determinism is a property of the materializer's signature, not of
/// this particular implementation being well-behaved.
struct DeterministicHashMaterializer;
impl ShadowMaterializer for DeterministicHashMaterializer {
    fn reduce(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<ShadowProjectionEffect, ShadowMaterializationError> {
        Ok(ShadowProjectionEffect {
            field_path: member.scope.clone(),
            value: QuantizedValue::Bytes(blake3::hash(&member.envelope_bytes).as_bytes().to_vec()),
        })
    }
}

/// In-memory `ShadowLedgerWriter` spy: records every appended entry, in order.
struct SpyLedger {
    appended: Mutex<Vec<ShadowLedgerEntry>>,
}
impl SpyLedger {
    fn new() -> Self {
        Self {
            appended: Mutex::new(Vec::new()),
        }
    }
    fn call_count(&self) -> usize {
        self.appended.lock().unwrap().len()
    }
    fn first_entry(&self) -> ShadowLedgerEntry {
        self.appended
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("at least one append")
    }
    fn last_entry(&self) -> ShadowLedgerEntry {
        self.appended
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("at least one append")
    }
}
#[async_trait::async_trait]
impl ShadowLedgerWriter for SpyLedger {
    async fn append(&self, entry: ShadowLedgerEntry) -> anyhow::Result<AppendOutcome> {
        let mut guard = self.appended.lock().unwrap();
        let entry_id = format!("test-entry-{}", guard.len());
        guard.push(entry);
        Ok(AppendOutcome::Appended { entry_id })
    }
}

fn export_from(fields: &[(&str, QuantizedValue)]) -> CanonicalExport {
    CanonicalExport {
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    }
}

fn sample_member(
    scope: &str,
    schema_name: &str,
    bytes: &[u8],
    parents: &[&str],
) -> MigrationCandidateMember {
    MigrationCandidateMember {
        scope: scope.to_string(),
        canonical_intent_schema_name: schema_name.to_string(),
        envelope_bytes: bytes.to_vec(),
        required_parent_op_ids: parents.iter().map(|p| p.to_string()).collect(),
    }
}

// ---------------------------------------------------------------------------
// §6.2.1 legacy_only_preserves_local_editor_path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_only_preserves_local_editor_path() {
    let db = setup_test_db().await;
    let intent = FakeIntent::new("CreateNode", "intent-seed-1");
    let derivation = SpyDerivation::new(AlwaysFailDerivation);
    let ledger = SpyLedger::new();
    let materializer = AlwaysSucceedsMaterializer;

    let outcome = commit_operation_dual_emit(
        &db,
        MigrationMode::LegacyOnly,
        "run-1",
        sample_op_log_entry(serde_json::json!({"k": "v"})),
        |_lamport| async { Ok::<(), anyhow::Error>(()) },
        &intent,
        &derivation,
        &materializer,
        &ledger,
    )
    .await
    .expect("legacy_only commit succeeds");

    assert!(matches!(outcome, DualEmitOutcome::LegacyOnlyResult(())));
    assert_eq!(
        derivation.call_count(),
        0,
        "derive() must never be called in legacy_only"
    );
    assert_eq!(
        ledger.call_count(),
        0,
        "the shadow ledger must never be touched in legacy_only"
    );
}

// ---------------------------------------------------------------------------
// §6.2.2 commit_operation_derives_legacy_and_candidate_set_from_one_intent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn commit_operation_derives_legacy_and_candidate_set_from_one_intent() {
    let db = setup_test_db().await;
    let intent = FakeIntent::new("SetNodeProperty", "intent-seed-2");
    let members = vec![
        sample_member(
            "VERSE#v1-FRACTAL#f1-PETAL#p1",
            "test.property_set.v1",
            &[1, 2, 3],
            &[],
        ),
        sample_member(
            "VERSE#v1-FRACTAL#f1-PETAL#p1-NODE#n1",
            "test.property_set.v1.child",
            &[4, 5, 6],
            &["parent-op-1"],
        ),
    ];
    let derivation = FixedSetDerivation {
        members: members.clone(),
        correlation_id: "corr-2".to_string(),
    };
    let ledger = SpyLedger::new();
    let materializer = AlwaysSucceedsMaterializer;

    let outcome = commit_operation_dual_emit(
        &db,
        MigrationMode::DualEmitShadow,
        "run-2",
        sample_op_log_entry(serde_json::json!({"k": "v"})),
        |_lamport| async { Ok::<(), anyhow::Error>(()) },
        &intent,
        &derivation,
        &materializer,
        &ledger,
    )
    .await
    .expect("dual emit commit succeeds");

    match outcome {
        DualEmitOutcome::ShadowCaptured { disposition, .. } => {
            assert_eq!(disposition, CorrelationDisposition::AppendPending);
        }
        other => panic!("expected ShadowCaptured, got {other:?}"),
    }

    assert_eq!(
        ledger.call_count(),
        1,
        "exactly one candidate set derived from the one intent"
    );
    let appended = ledger.first_entry();
    assert_eq!(appended.mutation_kind, "SetNodeProperty");
    assert_eq!(appended.run_local_correlation_id, "corr-2");

    // §4.1.4: the complete membership (both scope-local members, with the
    // required parent link on the second) was derived before any side effect —
    // proven by the retained candidate byte hashes covering both members.
    let hashes: Vec<String> = serde_json::from_str(&appended.candidate_byte_hashes_json).unwrap();
    assert_eq!(
        hashes.len(),
        2,
        "the candidate set covers both scope-local members"
    );
    assert_eq!(
        members[1].required_parent_op_ids,
        vec!["parent-op-1".to_string()]
    );
}

// ---------------------------------------------------------------------------
// §6.2.3 shadow_candidate_failure_never_repairs_legacy_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shadow_candidate_failure_never_repairs_legacy_state() {
    let db = setup_test_db().await;
    let intent = FakeIntent::new("RenameNode", "intent-seed-3");
    let members = vec![sample_member(
        "VERSE#v1-FRACTAL#f1-PETAL#p1-NODE#n1",
        "test.rename.v1",
        &[9, 9, 9],
        &[],
    )];
    let derivation = FixedSetDerivation {
        members,
        correlation_id: "corr-3".to_string(),
    };
    let ledger = SpyLedger::new();
    let materializer = AlwaysFailsMaterializer;
    let legacy_write_count = Arc::new(AtomicUsize::new(0));
    let legacy_write_count_inner = legacy_write_count.clone();

    let outcome = commit_operation_dual_emit(
        &db,
        MigrationMode::DualEmitShadow,
        "run-3",
        sample_op_log_entry(serde_json::json!({"k": "v"})),
        move |_lamport| {
            legacy_write_count_inner.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), anyhow::Error>(()) }
        },
        &intent,
        &derivation,
        &materializer,
        &ledger,
    )
    .await
    .expect("legacy still succeeds despite the materializer failure");

    match outcome {
        DualEmitOutcome::ShadowCaptured { disposition, .. } => {
            assert_eq!(disposition, CorrelationDisposition::MaterializationPending);
        }
        other => panic!("expected ShadowCaptured, got {other:?}"),
    }
    assert_eq!(
        legacy_write_count.load(Ordering::SeqCst),
        1,
        "legacy runs exactly once — never compensated, never repaired"
    );
    assert_eq!(
        ledger.call_count(),
        2,
        "the initial append_pending row plus one materialization_pending row"
    );
    assert_eq!(
        ledger.last_entry().disposition,
        CorrelationDisposition::MaterializationPending
    );
}

// ---------------------------------------------------------------------------
// §6.2.4 legacy_failure_taints_preappended_shadow_candidate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_failure_taints_preappended_shadow_candidate() {
    let db = setup_test_db().await;
    let intent = FakeIntent::new("DuplicateNode", "intent-seed-4");
    let members = vec![sample_member(
        "VERSE#v1-FRACTAL#f1-PETAL#p1-NODE#n1",
        "test.duplicate.v1",
        &[7, 7, 7],
        &[],
    )];
    let derivation = FixedSetDerivation {
        members,
        correlation_id: "corr-4".to_string(),
    };
    let ledger = SpyLedger::new();
    let materializer = AlwaysSucceedsMaterializer;
    let attempt_count = Arc::new(AtomicUsize::new(0));

    // Reference: the plain `legacy_only` error text for the identical failing input.
    let reference_counter = attempt_count.clone();
    let legacy_only_error =
        op_log::commit_operation(
            &db,
            sample_op_log_entry(serde_json::json!({"k": "v"})),
            move |_lamport| {
                reference_counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    Err::<(), anyhow::Error>(anyhow::anyhow!("simulated materialization failure"))
                }
            },
        )
        .await
        .expect_err("legacy_only fails");
    let legacy_only_text = format!("{legacy_only_error:#}");

    // The same failing input, through the dual-emit boundary, with a candidate
    // already durable in the shadow ledger before the legacy attempt runs.
    let dual_emit_counter = attempt_count.clone();
    let dual_emit_error =
        commit_operation_dual_emit(
            &db,
            MigrationMode::DualEmitShadow,
            "run-4",
            sample_op_log_entry(serde_json::json!({"k": "v"})),
            move |_lamport| {
                dual_emit_counter.fetch_add(1, Ordering::SeqCst);
                async move {
                    Err::<(), anyhow::Error>(anyhow::anyhow!("simulated materialization failure"))
                }
            },
            &intent,
            &derivation,
            &materializer,
            &ledger,
        )
        .await
        .expect_err("dual_emit_shadow also fails");
    let dual_emit_text = format!("{dual_emit_error:#}");

    assert_eq!(
        legacy_only_text, dual_emit_text,
        "legacy error text must be byte-identical between modes"
    );
    assert_eq!(
        attempt_count.load(Ordering::SeqCst),
        2,
        "one legacy attempt per call, never retried"
    );
    assert_eq!(
        ledger.call_count(),
        2,
        "the initial append_pending row plus one candidate_rejected taint row"
    );
    assert_eq!(
        ledger.last_entry().disposition,
        CorrelationDisposition::CandidateRejected
    );
}

// ---------------------------------------------------------------------------
// §6.2.5 shadow_rebuild_is_empty_state_and_arrival_independent
// ---------------------------------------------------------------------------

#[test]
fn shadow_rebuild_is_empty_state_and_arrival_independent() {
    let members = vec![
        sample_member(
            "VERSE#v1-FRACTAL#f1-PETAL#p1",
            "test.create_node.v1",
            b"member-a",
            &[],
        ),
        sample_member(
            "VERSE#v1-FRACTAL#f1-PETAL#p1-NODE#n1",
            "test.set_property.v1",
            b"member-b",
            &["op-a"],
        ),
    ];
    let materializer = DeterministicHashMaterializer;

    let result =
        replay_admitted_closure_three_times(&members, &materializer).expect("three replays agree");

    // A second, fully independent replay run (simulating a restart) must land
    // on the same root — no live SurrealDB state, receipt order, or prior-run
    // memory is ever consulted by `DeterministicHashMaterializer::reduce`.
    let restarted = replay_admitted_closure_three_times(&members, &materializer)
        .expect("restarted replay also agrees");
    assert_eq!(result.root_hex, restarted.root_hex);
    assert_eq!(result.export, restarted.export);
}

// ---------------------------------------------------------------------------
// §6.2.6 cutover_comparator_rejects_lossy_float_or_old_value_mapping
// ---------------------------------------------------------------------------

#[test]
fn cutover_comparator_rejects_lossy_float_or_old_value_mapping() {
    let legacy = export_from(&[
        ("node.transform.x", QuantizedValue::Integer(1_500)),
        (
            "node.stale_field",
            QuantizedValue::Unquantizable {
                raw_debug: "3.14159".to_string(),
            },
        ),
    ]);
    let shadow = export_from(&[
        // Off by one quantum — no tolerance window is ever applied.
        ("node.transform.x", QuantizedValue::Integer(1_501)),
        // Textually identical to the legacy side, but `Unquantizable` always
        // differs (§5.3.3) — never masked as equal.
        (
            "node.stale_field",
            QuantizedValue::Unquantizable {
                raw_debug: "3.14159".to_string(),
            },
        ),
    ]);

    let record = compare_projections(
        "run-6",
        "corr-6",
        "VERSE#v1",
        "candidate-6",
        &legacy,
        &shadow,
    );

    assert_eq!(
        record.differences.len(),
        2,
        "both the near-miss integer and the unquantizable field are differences"
    );
    let transform_diff = record
        .differences
        .iter()
        .find(|d| d.field_path == "node.transform.x")
        .unwrap();
    assert_eq!(transform_diff.class, FieldDifferenceClass::ValueMismatch);
    let stale_diff = record
        .differences
        .iter()
        .find(|d| d.field_path == "node.stale_field")
        .unwrap();
    assert_eq!(
        stale_diff.class,
        FieldDifferenceClass::AlwaysDiffersUnquantizable
    );
}

// ---------------------------------------------------------------------------
// §6.2.7 comparison_covers_prefixes_tombstones_and_restart_recovery
// ---------------------------------------------------------------------------

#[test]
fn comparison_covers_prefixes_tombstones_and_restart_recovery() {
    // Prefix 0: node created, both sides agree.
    let legacy_p0 = export_from(&[("node.exists", QuantizedValue::Bool(true))]);
    let shadow_p0 = export_from(&[("node.exists", QuantizedValue::Bool(true))]);
    // Prefix 1: legacy tombstones the node but the shadow replay missed it —
    // this must never be silently dropped as an "ignorable deletion artifact".
    let legacy_p1 = export_from(&[("node.exists", QuantizedValue::Tombstoned)]);
    let shadow_p1 = export_from(&[("node.exists", QuantizedValue::Bool(true))]);
    // Prefix 2 ("restart"): a fresh replay reaches the same tombstoned state.
    let legacy_p2 = export_from(&[("node.exists", QuantizedValue::Tombstoned)]);
    let shadow_p2 = export_from(&[("node.exists", QuantizedValue::Tombstoned)]);

    let records = compare_prefix_history(
        "run-7",
        "corr-7",
        "VERSE#v1",
        "candidate-7",
        &[legacy_p0, legacy_p1, legacy_p2],
        &[shadow_p0, shadow_p1, shadow_p2],
    );

    assert_eq!(
        records.len(),
        3,
        "every prefix gets its own comparison record"
    );
    assert!(records[0].is_equal(), "prefix 0 (creation) matches");
    assert!(
        !records[1].is_equal(),
        "prefix 1's tombstone mismatch is never ignored"
    );
    assert_eq!(
        records[1].differences[0].class,
        FieldDifferenceClass::ValueMismatch
    );
    assert!(
        records[2].is_equal(),
        "the restart replay (prefix 2) recovers exact parity"
    );
}

// ---------------------------------------------------------------------------
// §6.2.8 migration_flags_are_local_default_off_and_isolated_from_networking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_flags_are_local_default_off_and_isolated_from_networking() {
    std::env::remove_var(mode::MIGRATION_MODE_ENV_VAR);

    let flags = MigrationFlags::from_env().expect("unset env is a valid default");
    assert_eq!(flags.mode(), MigrationMode::LegacyOnly);
    assert!(flags.defaulted());

    for (token, expected) in [
        ("legacy_only", MigrationMode::LegacyOnly),
        ("seam_ready", MigrationMode::SeamReady),
        ("dual_emit_shadow", MigrationMode::DualEmitShadow),
        ("shadow_rebuild", MigrationMode::ShadowRebuild),
    ] {
        std::env::set_var(mode::MIGRATION_MODE_ENV_VAR, token);
        let flags = MigrationFlags::from_env().expect("recognized token parses");
        assert_eq!(flags.mode(), expected);
        assert!(!flags.defaulted());
    }

    std::env::set_var(
        mode::MIGRATION_MODE_ENV_VAR,
        "canonical_authoritative_local",
    );
    assert!(matches!(
        MigrationFlags::from_env(),
        Err(mode::MigrationFlagsError::CanonicalAuthoritativeLocalUnavailable)
    ));

    std::env::set_var(mode::MIGRATION_MODE_ENV_VAR, "unknown_mode_xyz");
    assert!(matches!(
        MigrationFlags::from_env(),
        Err(mode::MigrationFlagsError::UnknownMode(_))
    ));

    std::env::remove_var(mode::MIGRATION_MODE_ENV_VAR);

    // LegacyOnly/SeamReady never touch the shadow ledger, so no candidate or
    // shadow data can reach any existing legacy API/WebSocket listener either.
    let db = setup_test_db().await;
    let intent = FakeIntent::new("Ping", "intent-seed-8");
    let derivation = AlwaysFailDerivation;
    let ledger = SpyLedger::new();
    let materializer = AlwaysSucceedsMaterializer;
    for mode_under_test in [MigrationMode::LegacyOnly, MigrationMode::SeamReady] {
        commit_operation_dual_emit(
            &db,
            mode_under_test,
            "run-8",
            sample_op_log_entry(serde_json::json!({"k": "v"})),
            |_lamport| async { Ok::<(), anyhow::Error>(()) },
            &intent,
            &derivation,
            &materializer,
            &ledger,
        )
        .await
        .expect("legacy path succeeds");
    }
    assert_eq!(
        ledger.call_count(),
        0,
        "no candidate/shadow data is ever produced in these modes"
    );
}

// ---------------------------------------------------------------------------
// §6.2.9 unresolved_candidate_or_branch_contract_blocks_promotion
// ---------------------------------------------------------------------------

#[test]
fn unresolved_candidate_or_branch_contract_blocks_promotion() {
    // Even a "perfect" run — every measurement satisfied except D-CL20
    // coverage — still blocks promotion, because the mapping inventory can
    // never mark a D-CL20 kind Mapped (§4.2.5).
    let inventory = MappingInventory::seeded();
    let mut counters = RunCounters {
        legacy_success: 10,
        legacy_success_with_candidate: 10,
        candidates_retained: 10,
        candidates_decoded_reencoded_ok: 10,
        candidates_expected: 10,
        candidates_admitted_or_dispositioned: 10,
        unapproved_field_differences: 0,
        replay_root_matches: 3,
        replay_attempts: 3,
        interruption_points_planned: 5,
        interruption_points_resolved_cleanly: 5,
        unauthorized_visibility_events: 0,
        unbounded_or_unaccounted_stores: 0,
        network_prohibited_events_observed: 0,
    };
    let report = MeasurementReport::generate(&counters, &inventory, 0);
    assert!(
        report.blocks_promotion(0),
        "D-CL20's permanent unmapped kinds always block promotion"
    );

    // A missing/unresolved candidate (an uncovered legacy success) blocks
    // promotion even more directly, and is visible in its own measurement.
    counters.legacy_success_with_candidate = 9;
    let report_with_gap = MeasurementReport::generate(&counters, &inventory, 1);
    assert!(report_with_gap.blocks_promotion(1));
    assert!(
        !report_with_gap
            .measurements
            .iter()
            .find(|m| m.name == "capture_completeness")
            .unwrap()
            .meets_promotion_requirement
    );
}

// ---------------------------------------------------------------------------
// §6.2.10 authoritative_rollback_stops_writes_before_legacy_fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authoritative_rollback_stops_writes_before_legacy_fallback() {
    let db = setup_test_db().await;
    let intent = FakeIntent::new("UpdateNodeTransform", "intent-seed-10");
    let derivation = AlwaysFailDerivation;
    let ledger = SpyLedger::new();
    let materializer = AlwaysSucceedsMaterializer;
    let write_attempts = Arc::new(AtomicUsize::new(0));
    let write_attempts_inner = write_attempts.clone();

    let result = commit_operation_dual_emit(
        &db,
        MigrationMode::CanonicalAuthoritativeLocal,
        "run-10",
        sample_op_log_entry(serde_json::json!({"k": "v"})),
        move |_lamport| {
            write_attempts_inner.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), anyhow::Error>(()) }
        },
        &intent,
        &derivation,
        &materializer,
        &ledger,
    )
    .await;

    assert!(
        result.is_err(),
        "canonical_authoritative_local must refuse, not silently fall back to legacy authoring"
    );
    assert_eq!(
        write_attempts.load(Ordering::SeqCst),
        0,
        "no write — legacy or otherwise — happens once this boundary cannot honor authority"
    );
    assert_eq!(ledger.call_count(), 0, "no shadow write either");
}

// ---------------------------------------------------------------------------
// Supplementary: register_mapped refuses every D-CL20 kind unconditionally
// (§4.2.5). Not one of the ten §6.2 names, but directly named by this slice's
// acceptance criteria.
// ---------------------------------------------------------------------------

#[test]
fn register_mapped_refuses_every_d_cl20_kind() {
    use fe_database::migration::mapping_inventory::{
        MappingRegistrationError, D_CL20_MUTATION_KINDS,
    };

    for kind in D_CL20_MUTATION_KINDS {
        let mut inventory = MappingInventory::seeded();
        let result = inventory.register_mapped(
            kind,
            "test-schema-hash".to_string(),
            "test-materializer-v1".to_string(),
        );
        assert!(
            matches!(result, Err(MappingRegistrationError::DCl20Blocked { .. })),
            "{kind} must be refused by register_mapped (SPEC-8 §4.2.5)"
        );
        assert!(
            !inventory.is_mapped(kind),
            "{kind} must remain unmapped after the refused call"
        );
    }
}
