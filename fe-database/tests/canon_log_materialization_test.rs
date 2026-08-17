//! SPEC-4 §8 required conformance tests against the embedded SurrealDB engine.
//!
//! Eleven of the thirteen §8 tests live here under their exact spec names. Tests 12 and 13 —
//! `reduction_writes_references_and_never_reads_external_artifacts` and
//! `referenced_artifact_unavailable_is_expressible_and_distinct_from_exclusion` — are about
//! what a reduction may read rather than about storage, and live in `fe-canonical-log`
//! (`src/materialize/traits.rs`). Rationale: `fe-database/src/canon_log/AGENTS.md`.

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::SigningKey;

use fe_canonical_log::cbor::CborValue;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::{
    verified_envelope_meta_from, AdmissionOutcome, AppendOutcome, CandidateVerifier,
    CausalMaterializer, CheckpointBinding, CheckpointBindingMismatch, MaterializerVersion,
    ProjectionIdentity, ProjectionMutation, VerifiedEnvelopeMeta,
};
use fe_canonical_log::signing::{decode_and_admit, op_id as op_id_of_bytes, sign_envelope};

use fe_database::canon_log::admission::{
    admit_and_append, AdmissionRejection, AdmittedAppend, NoPrecondition,
};
use fe_database::canon_log::append_store::{SurrealVerifiedLogStore, SurrealVerseDagView};
use fe_database::canon_log::apply_marker_store::SurrealApplyMarkerStore;
use fe_database::canon_log::rebuild::{
    rebuild_projection, ProjectionSurface, RebuildRequest, RebuildSource,
};
use fe_database::canon_log::replay::{replay_to_frontier, PendingReason, ReplayOutcome};

type Db = surrealdb::Surreal<surrealdb::engine::local::Db>;

/// The Application operation kind used by every fixture: SPEC-1 §6 imposes no extra structural
/// rule on kinds 5..=65535, so a fixture can carry any parent shape without inventing payload
/// encryption the storage layer is not the subject of.
const FIXTURE_KIND: u16 = 100;

const VERSE: Identifier32 = Identifier32([0x11; 32]);
const BRANCH: Identifier32 = Identifier32([0x22; 32]);
const SEGMENT_MANIFEST: Hash32 = Hash32([0x33; 32]);

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// One signed, canonical, admissible operation.
#[derive(Clone)]
struct Operation {
    op_id: Hash32,
    bytes: Vec<u8>,
    meta: VerifiedEnvelopeMeta,
}

fn build_operation(author_seed: u8, parents: &[Hash32], wall_ms: u64, counter: u32) -> Operation {
    let signing_key = SigningKey::from_bytes(&[author_seed; 32]);
    let mut parents = parents.to_vec();
    parents.sort();
    parents.dedup();
    let unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: FIXTURE_KIND,
        scope: Scope::verse_wide(VERSE),
        author: Author::from_public_key(signing_key.verifying_key().to_bytes()),
        capability: CapabilityRef {
            chain_id: Hash32([0x77; 32]),
            scope_epoch: 0,
        },
        schema_hash: Hash32([0x88; 32]),
        branch_id: BRANCH,
        parents,
        hlc: Hlc::new(wall_ms, counter),
        payload: PayloadRef::empty(),
    };
    let complete = sign_envelope(&signing_key, &unsigned).expect("sign fixture envelope");
    let bytes = complete
        .encode_canonical()
        .expect("encode fixture envelope");
    let op_id = op_id_of_bytes(&bytes);
    let meta = verified_envelope_meta_from(&complete, op_id);
    Operation { op_id, bytes, meta }
}

/// A verifier that runs the REAL canonical admission for envelope structure, and injects the
/// §5 authorization/payload outcomes a test wants to observe.
#[derive(Default)]
struct FixtureVerifier {
    unauthorized: HashSet<Hash32>,
    invalid_payload: HashSet<Hash32>,
    unknown_schema: HashSet<Hash32>,
    opaque_payload: HashSet<Hash32>,
}

#[async_trait]
impl CandidateVerifier for FixtureVerifier {
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome> {
        match decode_and_admit(candidate_bytes) {
            Ok((complete, op_id)) => Ok(verified_envelope_meta_from(&complete, op_id)),
            Err(error) => Err(AdmissionOutcome::InvalidEnvelope {
                reason: error.to_string(),
            }),
        }
    }

    async fn verify_authorization(
        &self,
        meta: &VerifiedEnvelopeMeta,
    ) -> Result<(), AdmissionOutcome> {
        if self.unauthorized.contains(&meta.op_id) {
            return Err(AdmissionOutcome::UnauthorizedOperation);
        }
        Ok(())
    }

    async fn verify_payload(&self, meta: &VerifiedEnvelopeMeta) -> Result<(), AdmissionOutcome> {
        if self.unknown_schema.contains(&meta.op_id) {
            return Err(AdmissionOutcome::UnknownSchema {
                schema_hash: meta.schema_hash,
            });
        }
        if self.opaque_payload.contains(&meta.op_id) {
            return Err(AdmissionOutcome::OpaquePayload { op_id: meta.op_id });
        }
        if self.invalid_payload.contains(&meta.op_id) {
            return Err(AdmissionOutcome::InvalidPayload);
        }
        Ok(())
    }
}

/// A deterministic reduction that reads ONLY `meta` and `envelope_bytes` (§4 rule 8).
#[derive(Default)]
struct FixtureMaterializer {
    reduce_calls: Mutex<Vec<Hash32>>,
    failing: Mutex<HashSet<Hash32>>,
    excluded: Mutex<HashSet<Hash32>>,
    unavailable: Mutex<HashSet<Hash32>>,
}

impl FixtureMaterializer {
    fn reduce_count(&self, op_id: Hash32) -> usize {
        self.reduce_calls
            .lock()
            .expect("reduce log")
            .iter()
            .filter(|recorded| **recorded == op_id)
            .count()
    }

    fn set_failing(&self, op_id: Hash32, failing: bool) {
        let mut set = self.failing.lock().expect("failing set");
        if failing {
            set.insert(op_id);
        } else {
            set.remove(&op_id);
        }
    }
}

#[async_trait]
impl CausalMaterializer for FixtureMaterializer {
    async fn reduce(
        &self,
        meta: &VerifiedEnvelopeMeta,
        envelope_bytes: &[u8],
    ) -> ProjectionMutation {
        self.reduce_calls
            .lock()
            .expect("reduce log")
            .push(meta.op_id);

        if self
            .unavailable
            .lock()
            .expect("unavailable set")
            .contains(&meta.op_id)
        {
            return ProjectionMutation::ReferencedArtifactUnavailable {
                artifact_id: meta.schema_hash,
            };
        }
        if self
            .excluded
            .lock()
            .expect("excluded set")
            .contains(&meta.op_id)
        {
            return ProjectionMutation::Excluded {
                reason: "fixture exclusion".to_string(),
            };
        }
        if self
            .failing
            .lock()
            .expect("failing set")
            .contains(&meta.op_id)
        {
            return ProjectionMutation::Apply {
                statements: vec!["THROW 'induced reduction failure'".to_string()],
                bindings: Vec::new(),
            };
        }

        // Every input comes from the operation itself: no external artifact is dereferenced,
        // no clock is read, no pre-existing projection row is consulted.
        ProjectionMutation::Apply {
            statements: vec![
                "CREATE canon_projection CONTENT { op_id_hex: $op_id_hex, digest_hex: $digest_hex }"
                    .to_string(),
            ],
            bindings: vec![
                (
                    "op_id_hex".to_string(),
                    CborValue::Text(hex::encode(meta.op_id.as_bytes())),
                ),
                (
                    "digest_hex".to_string(),
                    CborValue::Bytes(blake3::hash(envelope_bytes).as_bytes().to_vec()),
                ),
            ],
        }
    }
}

/// The projection tables the fixture materializer owns.
struct FixtureSurface {
    db: Db,
}

#[async_trait]
impl ProjectionSurface for FixtureSurface {
    async fn reset_to_empty(&self, _identity: &ProjectionIdentity) -> anyhow::Result<()> {
        self.db
            .query("DELETE FROM canon_projection")
            .await?
            .check()?;
        Ok(())
    }

    async fn projection_root_hash(&self, _identity: &ProjectionIdentity) -> anyhow::Result<Hash32> {
        Ok(projection_root(&self.db).await)
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    db: Db,
    store: SurrealVerifiedLogStore,
    markers: SurrealApplyMarkerStore,
}

async fn harness() -> Harness {
    let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("in-memory SurrealDB");
    db.use_ns("canon_log")
        .use_db("canon_log")
        .await
        .expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("schema");
    db.query("DEFINE TABLE IF NOT EXISTS canon_projection SCHEMALESS")
        .await
        .expect("projection table")
        .check()
        .expect("projection table check");
    Harness {
        store: SurrealVerifiedLogStore::new(db.clone()),
        markers: SurrealApplyMarkerStore::new(db.clone()),
        db,
    }
}

fn identity() -> ProjectionIdentity {
    ProjectionIdentity::new(MaterializerVersion::new("scene-graph", 1), BRANCH)
}

async fn projection_rows(db: &Db) -> Vec<serde_json::Value> {
    let mut response = db
        .query("SELECT op_id_hex, digest_hex FROM canon_projection ORDER BY op_id_hex ASC")
        .await
        .expect("projection select");
    response.take(0).expect("projection rows")
}

async fn projection_root(db: &Db) -> Hash32 {
    let mut hasher = blake3::Hasher::new();
    for row in projection_rows(db).await {
        hasher.update(
            row.get("op_id_hex")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .as_bytes(),
        );
        hasher.update(
            row.get("digest_hex")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .as_bytes(),
        );
    }
    Hash32(*hasher.finalize().as_bytes())
}

async fn verified_log_row_count(db: &Db) -> usize {
    let mut response = db
        .query("SELECT op_id_hex FROM verified_op_log")
        .await
        .expect("log select");
    let rows: Vec<serde_json::Value> = response.take(0).expect("log rows");
    rows.len()
}

async fn marker_row_count(db: &Db) -> usize {
    let mut response = db
        .query("SELECT marker_key FROM materializer_apply_marker")
        .await
        .expect("marker select");
    let rows: Vec<serde_json::Value> = response.take(0).expect("marker rows");
    rows.len()
}

/// genesis -> {sibling_a, sibling_b concurrent at the same HLC} -> merge.
fn sample_dag() -> Vec<Operation> {
    let genesis = build_operation(0x10, &[], 100, 0);
    let sibling_a = build_operation(0x20, &[genesis.op_id], 200, 0);
    let sibling_b = build_operation(0x30, &[genesis.op_id], 200, 0);
    let merge = build_operation(0x40, &[sibling_a.op_id, sibling_b.op_id], 300, 0);
    vec![genesis, sibling_a, sibling_b, merge]
}

fn frontier_of(heads: &[Hash32]) -> SortedFrontier {
    SortedFrontier::try_new(heads.iter().copied()).expect("frontier")
}

/// The whole §2 pipeline, with the store standing in for its own equivocation index and
/// parent-verse lookup, as production would wire it.
async fn admit_bytes(
    store: &SurrealVerifiedLogStore,
    verifier: &FixtureVerifier,
    bytes: &[u8],
) -> Result<AdmittedAppend, AdmissionRejection> {
    admit_and_append(verifier, store, store, &NoPrecondition, store, bytes).await
}

// ---------------------------------------------------------------------------
// §8.1
// ---------------------------------------------------------------------------

#[tokio::test]
async fn append_before_projection() {
    let harness = harness().await;
    let verifier = FixtureVerifier::default();
    let operation = build_operation(0x10, &[], 100, 0);

    let admitted = admit_bytes(&harness.store, &verifier, &operation.bytes)
        .await
        .expect("admission");
    assert_eq!(admitted.append_outcome, AppendOutcome::Appended);

    // Recoverable by op_id, byte for byte, with no projection write anywhere.
    assert_eq!(
        harness
            .store
            .envelope_bytes(operation.op_id)
            .await
            .expect("read"),
        Some(operation.bytes.clone())
    );
    assert_eq!(
        harness.store.meta_of(operation.op_id).await.expect("meta"),
        Some(operation.meta.clone())
    );
    assert!(projection_rows(&harness.db).await.is_empty());
    assert_eq!(marker_row_count(&harness.db).await, 0);
}

// ---------------------------------------------------------------------------
// §8.2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_append_failure_leaves_projection_unchanged() {
    let harness = harness().await;
    let mut verifier = FixtureVerifier::default();
    let operation = build_operation(0x10, &[], 100, 0);
    let rejected = build_operation(0x11, &[], 110, 0);
    verifier.unauthorized.insert(rejected.op_id);

    // A rejected candidate never reaches the append.
    let rejection = admit_bytes(&harness.store, &verifier, &rejected.bytes)
        .await
        .expect_err("unauthorized candidate must not append");
    assert!(matches!(rejection, AdmissionRejection::Rejected { .. }));
    assert_eq!(verified_log_row_count(&harness.db).await, 0);

    // Bytes that do not hash to their claimed identity are refused before any row is touched.
    let mut tampered = operation.meta.clone();
    tampered.op_id = Hash32([0xfe; 32]);
    harness
        .store
        .append_admitted(&tampered, &operation.bytes)
        .await
        .expect_err("a mismatched claim must be refused");
    assert_eq!(verified_log_row_count(&harness.db).await, 0);
    assert!(projection_rows(&harness.db).await.is_empty());

    // An indeterminate acknowledgement is reconciled by retrying the exact same bytes: the
    // second attempt deduplicates instead of minting a second entry (§3.2, §3.4).
    let first = harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect("first append");
    let retry = harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect("exact-byte retry");
    assert_eq!(first, AppendOutcome::Appended);
    assert_eq!(retry, AppendOutcome::AlreadyPresent);
    assert_eq!(verified_log_row_count(&harness.db).await, 1);
    assert!(projection_rows(&harness.db).await.is_empty());
    assert_eq!(marker_row_count(&harness.db).await, 0);

    // §3.3: a claimed identity that maps to different bytes is an integrity violation and
    // neither candidate may be materialized. Simulate a substituted stored row.
    let decoy = build_operation(0x12, &[], 120, 0);
    harness
        .db
        .query("UPDATE verified_op_log SET envelope_bytes = $bytes WHERE op_id_hex = $id")
        .bind(("bytes", BASE64.encode(&decoy.bytes)))
        .bind(("id", hex::encode(operation.op_id.as_bytes())))
        .await
        .expect("substitute stored bytes")
        .check()
        .expect("substitute check");
    let conflict = harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect_err("an integrity conflict must fail");
    assert!(
        format!("{conflict}").contains("different bytes"),
        "{conflict}"
    );
    assert_eq!(verified_log_row_count(&harness.db).await, 1);
    assert!(projection_rows(&harness.db).await.is_empty());
}

// ---------------------------------------------------------------------------
// §8.3
// ---------------------------------------------------------------------------

#[tokio::test]
async fn append_and_apply_idempotence() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let operation = build_operation(0x10, &[], 100, 0);
    let identity = identity();
    let frontier = frontier_of(&[operation.op_id]);

    for _ in 0..2 {
        harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    assert_eq!(verified_log_row_count(&harness.db).await, 1);

    let first = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("first replay");
    let second = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("second replay");

    assert_eq!(first.applied, vec![operation.op_id]);
    assert_eq!(second.applied, Vec::<Hash32>::new());
    assert_eq!(second.already_processed, vec![operation.op_id]);
    assert_eq!(projection_rows(&harness.db).await.len(), 1);
    assert_eq!(marker_row_count(&harness.db).await, 1);
    assert_eq!(materializer.reduce_count(operation.op_id), 1);
}

// ---------------------------------------------------------------------------
// §8.4
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_between_append_and_apply_replays() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let operation = build_operation(0x10, &[], 100, 0);
    let identity = identity();

    harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect("append");

    // The crash: the process stops here, between a durable append and any atomic apply.
    assert!(!harness
        .markers
        .is_applied(&identity, operation.op_id)
        .await
        .expect("marker read"));
    assert!(projection_rows(&harness.db).await.is_empty());

    // Recovery is replay alone. No row is hand-written and no operation is re-minted.
    let outcome = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier_of(&[operation.op_id]),
    )
    .await
    .expect("replay after crash");

    assert_eq!(outcome.applied, vec![operation.op_id]);
    assert!(outcome.is_complete());
    assert_eq!(projection_rows(&harness.db).await.len(), 1);
    assert_eq!(
        harness
            .markers
            .disposition_of(&identity, operation.op_id)
            .await
            .expect("marker read"),
        Some("applied".to_string())
    );
}

// ---------------------------------------------------------------------------
// §8.5
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_after_atomic_apply_does_not_double_apply() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let operation = build_operation(0x10, &[], 100, 0);
    let identity = identity();
    let frontier = frontier_of(&[operation.op_id]);

    harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect("append");
    replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("apply");

    let root_before = projection_root(&harness.db).await;

    // The crash lands after the projection/marker commit; replay must observe the marker.
    let after_crash = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("replay after crash");

    assert!(after_crash.applied.is_empty());
    assert_eq!(after_crash.already_processed, vec![operation.op_id]);
    assert_eq!(projection_root(&harness.db).await, root_before);
    assert_eq!(projection_rows(&harness.db).await.len(), 1);
    assert_eq!(marker_row_count(&harness.db).await, 1);
    assert_eq!(
        materializer.reduce_count(operation.op_id),
        1,
        "an already-marked operation must not be reduced again"
    );
}

// ---------------------------------------------------------------------------
// §8.6
// ---------------------------------------------------------------------------

#[tokio::test]
async fn causal_and_concurrent_order_is_arrival_independent() {
    let dag = sample_dag();
    let identity = identity();
    let frontier = frontier_of(&[dag[3].op_id]);

    let permutations: [[usize; 4]; 4] = [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1], [1, 3, 0, 2]];

    let mut roots = Vec::new();
    let mut orders = Vec::new();
    for permutation in permutations {
        let harness = harness().await;
        let materializer = FixtureMaterializer::default();

        // Arrival order varies; the store is content-addressed, so nothing else does.
        for index in permutation {
            harness
                .store
                .append_admitted(&dag[index].meta, &dag[index].bytes)
                .await
                .expect("append");
        }

        // Replay repeatedly: eligibility is per-parent, so a single pass settles the whole DAG.
        let outcome = replay_to_frontier(
            &harness.store,
            &harness.markers,
            &materializer,
            &identity,
            &frontier,
        )
        .await
        .expect("replay");

        assert!(outcome.is_complete(), "{outcome:?}");
        assert_eq!(outcome.applied.len(), 4);
        roots.push(projection_root(&harness.db).await);
        orders.push(outcome.applied);
    }

    for root in &roots {
        assert_eq!(
            *root, roots[0],
            "the canonical projection root must not depend on insertion order"
        );
    }
    for order in &orders {
        assert_eq!(
            *order, orders[0],
            "the causal order must not depend on insertion order"
        );
    }

    // The §4.3 tie-break actually decided the concurrent pair: both siblings share
    // (wall_ms, counter), so the smaller author public key must come first every time.
    let applied = &orders[0];
    assert_eq!(applied[0], dag[0].op_id, "genesis is causally first");
    assert_eq!(applied[3], dag[3].op_id, "the merge is causally last");
    let expected_first_sibling = if dag[1].meta.author_public_key < dag[2].meta.author_public_key {
        dag[1].op_id
    } else {
        dag[2].op_id
    };
    assert_eq!(
        applied[1], expected_first_sibling,
        "concurrent siblings order by (wall_ms, counter, author_public_key)"
    );
}

// ---------------------------------------------------------------------------
// §8.7
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_midstream_candidate_blocks_complete_claim() {
    let harness = harness().await;
    let mut verifier = FixtureVerifier::default();

    let genesis = build_operation(0x10, &[], 100, 0);
    let unauthorized = build_operation(0x11, &[genesis.op_id], 110, 0);
    let unknown_schema = build_operation(0x12, &[genesis.op_id], 120, 0);
    let opaque = build_operation(0x13, &[genesis.op_id], 130, 0);
    let orphan_parent = build_operation(0x14, &[], 140, 0);
    let orphan = build_operation(0x15, &[orphan_parent.op_id], 150, 0);

    verifier.unauthorized.insert(unauthorized.op_id);
    verifier.unknown_schema.insert(unknown_schema.op_id);
    verifier.opaque_payload.insert(opaque.op_id);

    admit_bytes(&harness.store, &verifier, &genesis.bytes)
        .await
        .expect("genesis admits");

    // 1. invalid envelope: corrupted bytes are rejected, never appended.
    let mut corrupted = genesis.bytes.clone();
    *corrupted.last_mut().expect("non-empty") ^= 0xff;
    let outcome = admit_bytes(&harness.store, &verifier, &corrupted)
        .await
        .expect_err("corrupted bytes must be rejected");
    assert!(matches!(outcome, AdmissionRejection::Rejected { .. }));

    // 2. unauthorized.
    let outcome = admit_bytes(&harness.store, &verifier, &unauthorized.bytes)
        .await
        .expect_err("unauthorized must be rejected");
    assert_eq!(
        outcome.outcome(),
        Some(&AdmissionOutcome::UnauthorizedOperation)
    );

    // 3. missing parent: quarantined, not appended, not applied.
    let outcome = admit_bytes(&harness.store, &verifier, &orphan.bytes)
        .await
        .expect_err("a missing parent must quarantine");
    assert!(matches!(outcome, AdmissionRejection::Quarantined { .. }));
    assert_eq!(
        outcome.outcome(),
        Some(&AdmissionOutcome::MissingParent {
            missing: orphan_parent.op_id
        })
    );

    // 4. unknown schema.
    let outcome = admit_bytes(&harness.store, &verifier, &unknown_schema.bytes)
        .await
        .expect_err("an unknown schema must quarantine");
    assert!(matches!(outcome, AdmissionRejection::Quarantined { .. }));

    // 5. opaque payload.
    let outcome = admit_bytes(&harness.store, &verifier, &opaque.bytes)
        .await
        .expect_err("an opaque payload must quarantine");
    assert!(matches!(outcome, AdmissionRejection::Quarantined { .. }));

    // Only the genesis is in the verified log; nothing was provisionally applied.
    assert_eq!(verified_log_row_count(&harness.db).await, 1);
    assert!(projection_rows(&harness.db).await.is_empty());

    // A candidate whose parent never arrived HALTS its head rather than being skipped, even
    // when its own bytes reach the store through a segment rather than through admission.
    harness
        .store
        .append_admitted(&orphan.meta, &orphan.bytes)
        .await
        .expect("segment delivery");
    let materializer = FixtureMaterializer::default();
    let outcome = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity(),
        &frontier_of(&[orphan.op_id]),
    )
    .await
    .expect("replay");

    assert!(
        !outcome.is_complete(),
        "an incomplete history must never be claimed as complete"
    );
    assert_eq!(outcome.halted_heads.len(), 1);
    assert_eq!(outcome.halted_heads[0].head, orphan.op_id);
    assert_eq!(
        outcome.halted_heads[0].missing_ancestor,
        orphan_parent.op_id
    );
    assert!(outcome.applied.is_empty());
    assert_eq!(materializer.reduce_count(orphan.op_id), 0);
}

// ---------------------------------------------------------------------------
// §8.8
// ---------------------------------------------------------------------------

#[tokio::test]
async fn materializer_failure_stays_pending_until_replay() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let operation = build_operation(0x10, &[], 100, 0);
    let identity = identity();
    let frontier = frontier_of(&[operation.op_id]);

    harness
        .store
        .append_admitted(&operation.meta, &operation.bytes)
        .await
        .expect("append");
    materializer.set_failing(operation.op_id, true);

    let failed = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("replay runs even when a reduction fails");

    assert!(failed.applied.is_empty());
    assert!(!failed.is_complete(), "a failed reduction is not projected");
    assert_eq!(failed.pending.len(), 1);
    assert_eq!(failed.pending[0].0, operation.op_id);
    assert!(matches!(
        failed.pending[0].1,
        PendingReason::MaterializationFailed { .. }
    ));
    assert!(projection_rows(&harness.db).await.is_empty());
    assert_eq!(marker_row_count(&harness.db).await, 0);

    // The appended operation is still valid. Deterministic retry converges without minting a
    // replacement: the same op_id, the same bytes, the same identity.
    materializer.set_failing(operation.op_id, false);
    let recovered = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("retry");

    assert_eq!(recovered.applied, vec![operation.op_id]);
    assert!(recovered.is_complete());
    assert_eq!(verified_log_row_count(&harness.db).await, 1);
    assert_eq!(projection_rows(&harness.db).await.len(), 1);
}

// ---------------------------------------------------------------------------
// §8.9
// ---------------------------------------------------------------------------

#[tokio::test]
async fn checkpoint_binds_frontier_manifest_and_materializer() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let dag = sample_dag();
    let identity = identity();
    let frontier = frontier_of(&[dag[3].op_id]);

    for operation in &dag {
        harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier,
    )
    .await
    .expect("replay");
    let projection_root_hash = projection_root(&harness.db).await;

    let honest = CheckpointBinding {
        branch_id: BRANCH,
        frontier_commitment: frontier.commitment(),
        segment_manifest_id: SEGMENT_MANIFEST,
        materializer_version: identity.materializer_version.clone(),
        projection_root_hash,
    };

    let mut dag_view = SurrealVerseDagView::load(&harness.store, VERSE)
        .await
        .expect("dag view");
    let surface = FixtureSurface {
        db: harness.db.clone(),
    };

    let accepted = rebuild_projection(
        &harness.store,
        &harness.markers,
        &materializer,
        &surface,
        &mut dag_view,
        RebuildRequest {
            identity: &identity,
            frontier: &frontier,
            segment_manifest_id: SEGMENT_MANIFEST,
            checkpoint: Some(&honest),
        },
    )
    .await
    .expect("rebuild");
    assert_eq!(accepted.rejected_checkpoint, None);
    assert_eq!(
        accepted.source,
        RebuildSource::VerifiedCheckpoint {
            claimed_projection_root_hash: projection_root_hash
        }
    );

    // Changing any bound field invalidates the checkpoint for reuse.
    let other_frontier = frontier_of(&[dag[1].op_id, dag[2].op_id]);
    let cases: Vec<(CheckpointBinding, CheckpointBindingMismatch)> = vec![
        (
            CheckpointBinding {
                frontier_commitment: other_frontier.commitment(),
                ..honest.clone()
            },
            CheckpointBindingMismatch::FrontierCommitmentMismatch,
        ),
        (
            CheckpointBinding {
                segment_manifest_id: Hash32([0x34; 32]),
                ..honest.clone()
            },
            CheckpointBindingMismatch::SegmentManifestMismatch,
        ),
        (
            CheckpointBinding {
                materializer_version: MaterializerVersion::new("scene-graph", 2),
                ..honest.clone()
            },
            CheckpointBindingMismatch::MaterializerVersionMismatch,
        ),
        (
            CheckpointBinding {
                materializer_version: MaterializerVersion::new("terrain-index", 1),
                ..honest.clone()
            },
            CheckpointBindingMismatch::MaterializerVersionMismatch,
        ),
        (
            CheckpointBinding {
                branch_id: Identifier32([0x23; 32]),
                ..honest.clone()
            },
            CheckpointBindingMismatch::BranchMismatch,
        ),
    ];

    for (tampered, expected) in cases {
        let report = rebuild_projection(
            &harness.store,
            &harness.markers,
            &materializer,
            &surface,
            &mut dag_view,
            RebuildRequest {
                identity: &identity,
                frontier: &frontier,
                segment_manifest_id: SEGMENT_MANIFEST,
                checkpoint: Some(&tampered),
            },
        )
        .await
        .expect("rebuild");
        assert_eq!(report.rejected_checkpoint, Some(expected));
        assert_eq!(
            report.source,
            RebuildSource::EmptyState,
            "a rejected checkpoint must fall back to a full replay"
        );
    }

    // A checkpoint whose projection root disagrees with the replayed one is detectable: the
    // binding validates on its five fields, but the accelerator's claim does not reproduce.
    let lying = CheckpointBinding {
        projection_root_hash: Hash32([0x99; 32]),
        ..honest.clone()
    };
    let report = rebuild_projection(
        &harness.store,
        &harness.markers,
        &materializer,
        &surface,
        &mut dag_view,
        RebuildRequest {
            identity: &identity,
            frontier: &frontier,
            segment_manifest_id: SEGMENT_MANIFEST,
            checkpoint: Some(&lying),
        },
    )
    .await
    .expect("rebuild");
    assert_eq!(
        report.source,
        RebuildSource::VerifiedCheckpoint {
            claimed_projection_root_hash: Hash32([0x99; 32])
        }
    );
    assert_ne!(
        report.projection_root_hash,
        Hash32([0x99; 32]),
        "replay must be able to contradict a checkpoint's claimed projection root"
    );
}

// ---------------------------------------------------------------------------
// §8.10
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_rebuild_matches_checkpoint_replay() {
    let dag = sample_dag();
    let identity = identity();
    let frontier = frontier_of(&[dag[3].op_id]);

    // Pass one: empty-state rebuild, no checkpoint offered at all.
    let empty_harness = harness().await;
    let empty_materializer = FixtureMaterializer::default();
    for operation in &dag {
        empty_harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    let mut empty_dag_view = SurrealVerseDagView::load(&empty_harness.store, VERSE)
        .await
        .expect("dag view");
    let empty_surface = FixtureSurface {
        db: empty_harness.db.clone(),
    };
    let empty_report = rebuild_projection(
        &empty_harness.store,
        &empty_harness.markers,
        &empty_materializer,
        &empty_surface,
        &mut empty_dag_view,
        RebuildRequest {
            identity: &identity,
            frontier: &frontier,
            segment_manifest_id: SEGMENT_MANIFEST,
            checkpoint: None,
        },
    )
    .await
    .expect("empty rebuild");
    assert_eq!(empty_report.source, RebuildSource::EmptyState);
    assert!(empty_report.replay.is_complete());

    // Pass two: an independent database, the genesis prefix already checkpointed, and the tail
    // replayed on top of a verified checkpoint binding.
    let checkpoint_harness = harness().await;
    let checkpoint_materializer = FixtureMaterializer::default();
    for operation in &dag {
        checkpoint_harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    let prefix_frontier = frontier_of(&[dag[0].op_id]);
    replay_to_frontier(
        &checkpoint_harness.store,
        &checkpoint_harness.markers,
        &checkpoint_materializer,
        &identity,
        &prefix_frontier,
    )
    .await
    .expect("prefix replay");

    let binding = CheckpointBinding {
        branch_id: BRANCH,
        frontier_commitment: frontier.commitment(),
        segment_manifest_id: SEGMENT_MANIFEST,
        materializer_version: identity.materializer_version.clone(),
        projection_root_hash: empty_report.projection_root_hash,
    };
    let mut checkpoint_dag_view = SurrealVerseDagView::load(&checkpoint_harness.store, VERSE)
        .await
        .expect("dag view");
    let checkpoint_surface = FixtureSurface {
        db: checkpoint_harness.db.clone(),
    };
    let checkpoint_report = rebuild_projection(
        &checkpoint_harness.store,
        &checkpoint_harness.markers,
        &checkpoint_materializer,
        &checkpoint_surface,
        &mut checkpoint_dag_view,
        RebuildRequest {
            identity: &identity,
            frontier: &frontier,
            segment_manifest_id: SEGMENT_MANIFEST,
            checkpoint: Some(&binding),
        },
    )
    .await
    .expect("checkpoint rebuild");
    assert_eq!(checkpoint_report.rejected_checkpoint, None);
    assert!(checkpoint_report.replay.is_complete());

    assert_eq!(
        empty_report.projection_root_hash, checkpoint_report.projection_root_hash,
        "empty-state replay and checkpoint-plus-tail replay must agree"
    );
    // Both rebuilds are of the same projection identity, so their roots are comparable at all
    // (§6.5: a materializer version change would make them different projections entirely).
    assert_eq!(
        checkpoint_report.source,
        RebuildSource::VerifiedCheckpoint {
            claimed_projection_root_hash: empty_report.projection_root_hash
        }
    );
}

// ---------------------------------------------------------------------------
// §8.11
// ---------------------------------------------------------------------------

/// The §7.1 canonical input identity of an analytics computation.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AnalyticsSourceIdentity {
    branch_id: Identifier32,
    frontier_commitment: Hash32,
    segment_manifest_id: Hash32,
    materializer_id: String,
    materializer_version: u32,
}

#[tokio::test]
async fn analytics_source_identity_is_reproducible() {
    let dag = sample_dag();
    let identity = identity();
    let frontier = frontier_of(&[dag[3].op_id]);
    let declared = AnalyticsSourceIdentity {
        branch_id: BRANCH,
        frontier_commitment: frontier.commitment(),
        segment_manifest_id: SEGMENT_MANIFEST,
        materializer_id: identity.materializer_version.materializer_id.clone(),
        materializer_version: identity.materializer_version.version,
    };

    // Two independent materializations, given the same declared source identity, over stores
    // that received the operations in different orders.
    let mut relations = Vec::new();
    let mut roots = Vec::new();
    for order in [[0usize, 1, 2, 3], [3, 1, 0, 2]] {
        let harness = harness().await;
        let materializer = FixtureMaterializer::default();
        for index in order {
            harness
                .store
                .append_admitted(&dag[index].meta, &dag[index].bytes)
                .await
                .expect("append");
        }
        let outcome = replay_to_frontier(
            &harness.store,
            &harness.markers,
            &materializer,
            &identity,
            &frontier,
        )
        .await
        .expect("replay");
        assert!(outcome.is_complete());

        relations.push(projection_rows(&harness.db).await);
        roots.push(projection_root(&harness.db).await);
    }

    assert_eq!(
        relations[0], relations[1],
        "the canonical analytics input relation must be identical"
    );
    assert_eq!(roots[0], roots[1]);

    // The identity a result would record is itself reproducible from the same inputs.
    let recomputed = AnalyticsSourceIdentity {
        branch_id: BRANCH,
        frontier_commitment: frontier_of(&[dag[3].op_id]).commitment(),
        segment_manifest_id: SEGMENT_MANIFEST,
        materializer_id: identity.materializer_version.materializer_id.clone(),
        materializer_version: identity.materializer_version.version,
    };
    assert_eq!(declared, recomputed);

    // A materializer version bump is a different source identity, not the same one relabelled.
    let bumped = AnalyticsSourceIdentity {
        materializer_version: identity.materializer_version.version + 1,
        ..declared.clone()
    };
    assert_ne!(declared, bumped);
}

// ---------------------------------------------------------------------------
// Supporting evidence for the pending/exclusion states the eleven tests rely on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unavailable_reference_leaves_the_marker_alone_and_its_children_pending() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let parent = build_operation(0x10, &[], 100, 0);
    let child = build_operation(0x20, &[parent.op_id], 200, 0);
    let identity = identity();

    for operation in [&parent, &child] {
        harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    materializer
        .unavailable
        .lock()
        .expect("unavailable set")
        .insert(parent.op_id);

    let outcome = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier_of(&[child.op_id]),
    )
    .await
    .expect("replay");

    assert!(outcome.applied.is_empty());
    assert!(!outcome.is_complete());
    assert_eq!(marker_row_count(&harness.db).await, 0);
    assert!(outcome.pending.iter().any(|(op_id, reason)| {
        *op_id == parent.op_id
            && matches!(reason, PendingReason::ReferencedArtifactUnavailable { .. })
    }));
    assert!(outcome.pending.iter().any(|(op_id, reason)| {
        *op_id == child.op_id && matches!(reason, PendingReason::ParentPending { .. })
    }));
    assert_eq!(materializer.reduce_count(child.op_id), 0);
}

#[tokio::test]
async fn an_exclusion_is_recorded_as_evidence_and_still_makes_children_eligible() {
    let harness = harness().await;
    let materializer = FixtureMaterializer::default();
    let parent = build_operation(0x10, &[], 100, 0);
    let child = build_operation(0x20, &[parent.op_id], 200, 0);
    let identity = identity();

    for operation in [&parent, &child] {
        harness
            .store
            .append_admitted(&operation.meta, &operation.bytes)
            .await
            .expect("append");
    }
    materializer
        .excluded
        .lock()
        .expect("excluded set")
        .insert(parent.op_id);

    let outcome: ReplayOutcome = replay_to_frontier(
        &harness.store,
        &harness.markers,
        &materializer,
        &identity,
        &frontier_of(&[child.op_id]),
    )
    .await
    .expect("replay");

    assert!(outcome.is_complete());
    assert_eq!(outcome.applied, vec![child.op_id]);
    assert_eq!(
        outcome.excluded,
        vec![(parent.op_id, "fixture exclusion".to_string())]
    );
    assert_eq!(
        harness
            .markers
            .disposition_of(&identity, parent.op_id)
            .await
            .expect("marker read"),
        Some("excluded:fixture exclusion".to_string())
    );
    assert_eq!(projection_rows(&harness.db).await.len(), 1);
}
