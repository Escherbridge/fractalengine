//! CDL-SPEC10 B1: SPEC-10 §10 conformance tests 1-4 for the kind-5 `observation_batch`
//! operation, driven through the crate's public surface -- a real signed envelope, the one ingress
//! (`decode_and_admit` inside `ComposedCandidateVerifier`), real SPEC-3 chain verification, real
//! XChaCha20-Poly1305 payload opening, the G1 quarantine, and the reference-only reduction.

mod support;

use std::cell::Cell;

use ed25519_dalek::SigningKey;

use fe_canonical_log::capability::{ObjectClass, Verb};
use fe_canonical_log::cbor::{encode_canonical_checked, CborValue};
use fe_canonical_log::compose::{
    ComposedCandidateVerifier, ComposedVerifierParts, PayloadSchemaRegistry, QuarantineReason,
    QuarantineReasonClass, SchemaVerdict,
};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::InMemoryScopeKeyStore;
use fe_canonical_log::envelope::{Hash32, Hlc, Identifier32, PayloadRef, Scope};
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::errors::AdmissionOutcome;
use fe_canonical_log::materialize::identity::{ObservationSource, SourceClass};
use fe_canonical_log::materialize::traits::{
    admit_candidate_with_facts, AdmittedCandidate, CausalMaterializer, ParentVerseLookup,
    ProjectionMutation,
};
use fe_canonical_log::observation::{
    node_anchor, observation_batch_schema_hash, validate_observation_batch_plaintext, HlcRange,
    ObservationBatchMaterializer, ObservationBatchPayload, ObservationBatchPayloadParts,
    SensorTimeRange, SpecRegisteredKinds, HEXON_FORMAT_VERSION, OBSERVATION_BATCH_OPERATION_KIND,
    OBSERVATION_BATCH_ROW_BINDING,
};
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::retention::quarantine::{
    admit_candidate as quarantine_candidate, evict_expired_or_over_capacity,
    unknown_kind_promotion_ready, InMemoryQuarantineStore, PerReasonBudgets, QuarantineAdmitError,
    QuarantineBounds, QuarantineRecord, QuarantineStore, ReasonBudget,
};

const SCOPE_KEY: [u8; 32] = [0x6b; 32];
const HEXON_ID: Hash32 = Hash32([0xe1; 32]);
const STATS_ID: Hash32 = Hash32([0xe2; 32]);
const BRANCH: Identifier32 = Identifier32([0x41; 32]);
const KIND_ONE_SCHEMA: Hash32 = Hash32([0x90; 32]);
const NODE_A: &str = "01HZXOBSNODEA0000000000000";
const NODE_B: &str = "01HZXOBSNODEB0000000000000";

/// The registry of a build that implements SPEC-10: kind 5 under its one schema, validated by the
/// observation payload codec; kind 1 under the fixture schema as any canonical CBOR.
struct Spec10Registry;

impl PayloadSchemaRegistry for Spec10Registry {
    fn interprets_kind(&self, operation_kind: u16) -> bool {
        operation_kind == OBSERVATION_BATCH_OPERATION_KIND
    }

    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        plaintext: &[u8],
    ) -> SchemaVerdict {
        match operation_kind {
            OBSERVATION_BATCH_OPERATION_KIND if *schema_hash == observation_batch_schema_hash() => {
                if validate_observation_batch_plaintext(plaintext) {
                    SchemaVerdict::Valid
                } else {
                    SchemaVerdict::Invalid
                }
            }
            1 | OBSERVATION_BATCH_OPERATION_KIND => SchemaVerdict::UnknownSchema,
            _ => SchemaVerdict::UnknownKind,
        }
    }
}

/// The registry of a build that predates SPEC-10: it interprets no kind beyond SPEC-1's own.
struct PreSpec10Registry;

impl PayloadSchemaRegistry for PreSpec10Registry {
    fn interprets_kind(&self, _operation_kind: u16) -> bool {
        false
    }

    fn validate(
        &self,
        schema_hash: &Hash32,
        operation_kind: u16,
        _plaintext: &[u8],
    ) -> SchemaVerdict {
        match operation_kind {
            1 if *schema_hash == KIND_ONE_SCHEMA => SchemaVerdict::Valid,
            1 => SchemaVerdict::UnknownSchema,
            _ => SchemaVerdict::UnknownKind,
        }
    }
}

/// No parent is locally admitted: every parented candidate is `MissingParent`.
struct NoParentsHeld;

#[async_trait::async_trait]
impl ParentVerseLookup for NoParentsHeld {
    async fn verse_id_of(&self, _parent_op_id: Hash32) -> Option<Identifier32> {
        None
    }
}

type Verifier<R> = ComposedCandidateVerifier<
    support::ChainMap,
    support::ManagerView,
    support::UnknownLifecycle,
    InMemoryScopeKeyStore,
    support::NoCiphertexts,
    R,
    support::FixedClock,
>;

/// One signed operation and the ciphertext it commits to (empty for the no-payload form).
struct Signed {
    bytes: Vec<u8>,
    ciphertext: Vec<u8>,
    op_id: Hash32,
}

/// One author holding a verse-wide `append/op` chain and the petal's scope key.
struct World {
    author: SigningKey,
    chain_bytes: Vec<u8>,
    chain_id: Hash32,
    next_counter: Cell<u32>,
}

impl World {
    fn new() -> Self {
        let author = support::signing_key([0x71; 32]);
        let (chain_bytes, chain_id) = support::self_issued_chain(
            &author,
            &[Verb::Append],
            &[ObjectClass::Operation],
            support::verse_scope(),
            support::verse_scope(),
            support::EPOCH,
        );
        Self {
            author,
            chain_bytes,
            chain_id,
            next_counter: Cell::new(0),
        }
    }

    fn verifier<R: PayloadSchemaRegistry>(&self, schemas: R) -> Verifier<R> {
        ComposedCandidateVerifier::new(ComposedVerifierParts {
            chains: support::ChainMap::holding([self.chain_bytes.clone()]),
            authority: support::ManagerView::at_epoch(
                support::principal_of(&self.author),
                support::verse_scope(),
                support::EPOCH,
            ),
            lifecycle: support::UnknownLifecycle,
            scope_keys: support::scope_key_store(petal(), support::EPOCH, SCOPE_KEY),
            ciphertexts: support::NoCiphertexts,
            schemas,
            clock: support::FixedClock(support::NOW_MS),
        })
    }

    /// Signs one operation; `plaintext` is sealed under the petal key with the §5.2 AAD, `None`
    /// is the SPEC-1 §3.5 no-payload form.
    fn operation(
        &self,
        kind: u16,
        scope: Scope,
        parents: Vec<Hash32>,
        schema_hash: Hash32,
        plaintext: Option<&[u8]>,
    ) -> Signed {
        let counter = self.next_counter.get();
        self.next_counter.set(counter + 1);
        let mut unsigned = support::build_unsigned(
            &self.author,
            kind,
            scope,
            self.chain_id,
            support::EPOCH,
            schema_hash,
            BRANCH,
            parents,
            Hlc::new(support::NOW_MS, counter),
            PayloadRef::empty(),
        );
        let mut ciphertext = Vec::new();
        if let Some(plaintext) = plaintext {
            let key = SealingKey::from_bytes(SCOPE_KEY);
            let key_id = derive_key_id(&petal(), support::EPOCH, &key).expect("key id");
            let nonce = FreshNonce::draw(key_id).expect("nonce");
            unsigned.payload = PayloadRef {
                encryption: Some(nonce.params()),
                ..PayloadRef::empty()
            };
            let aad = payload_aad_preimage(&unsigned).expect("aad");
            let sealed = seal(&key, nonce, &aad, plaintext).expect("seal");
            unsigned.payload = sealed.payload_ref();
            ciphertext = sealed.ciphertext;
        }
        let bytes = support::sign(&self.author, &unsigned)
            .encode_canonical()
            .expect("encode");
        Signed {
            op_id: Hash32::of(&bytes),
            bytes,
            ciphertext,
        }
    }

    /// A kind-5 batch in the petal under the observation schema, sealing `plaintext`.
    fn batch(&self, plaintext: &[u8]) -> Signed {
        self.operation(
            OBSERVATION_BATCH_OPERATION_KIND,
            petal(),
            vec![support::hash(0x01)],
            observation_batch_schema_hash(),
            Some(plaintext),
        )
    }

    fn device_source(&self, label: &str) -> ObservationSource {
        ObservationSource::new(
            SourceClass::Device,
            ObservationSource::source_id_for(SourceClass::Device, label),
            support::public_key(&self.author),
            Some("acme.thermo/frame@1".to_string()),
            None,
        )
        .expect("device source")
    }

    /// A valid three-row batch over two anchors, two asset refs, and two sources.
    fn payload(&self) -> ObservationBatchPayload {
        ObservationBatchPayload::new(
            ObservationBatchPayloadParts {
                hexon_artifact_id: HEXON_ID,
                hexon_stored_length: 8_192,
                hlc_range: HlcRange {
                    min: Hlc::new(support::NOW_MS - 60_000, 0),
                    max: Hlc::new(support::NOW_MS - 1, 4),
                },
                sensor_time_range: SensorTimeRange {
                    min_ms: 1_699_999_000_000,
                    max_ms: 1_699_999_999_000,
                },
                stats_hash: STATS_ID,
                count: 3,
                anchors: vec![
                    node_anchor(NODE_B).expect("ascii"),
                    node_anchor(NODE_A).expect("ascii"),
                ],
                asset_refs: vec![support::hash(0xa2), support::hash(0xa1)],
                sources: vec![
                    self.device_source("thermo-2"),
                    self.device_source("thermo-1"),
                ],
                hexon_format_version: HEXON_FORMAT_VERSION,
                stats_stored_length: 640,
            }
            .normalized(),
        )
        .expect("valid payload")
    }
}

fn petal() -> Scope {
    support::petal_scope(support::PETAL_A)
}

async fn admit<R: PayloadSchemaRegistry>(
    verifier: &Verifier<R>,
    operation: &Signed,
) -> Result<AdmittedCandidate, AdmissionOutcome> {
    admit_candidate_with_facts(
        verifier,
        &support::NoEquivocation,
        &support::SameVerseParents,
        &operation.bytes,
        Some(&operation.ciphertext),
    )
    .await
}

fn map_entries(value: CborValue) -> Vec<(CborValue, CborValue)> {
    match value {
        CborValue::Map(entries) => entries,
        other => panic!("expected a map, got {other:?}"),
    }
}

/// `payload` with key `key` replaced by (or extended with) `value`, canonically encoded.
fn payload_with(payload: &ObservationBatchPayload, key: u64, value: CborValue) -> Vec<u8> {
    let mut entries = map_entries(payload.to_cbor());
    entries.retain(|(existing, _)| *existing != CborValue::Uint(key));
    entries.push((CborValue::Uint(key), value));
    encode_canonical_checked(&CborValue::Map(entries)).expect("encode")
}

fn reversed_array(payload: &ObservationBatchPayload, key: u64) -> CborValue {
    let mut items = payload
        .to_cbor()
        .get_uint_key(key)
        .and_then(CborValue::as_array)
        .expect("array key")
        .to_vec();
    items.reverse();
    CborValue::Array(items)
}

fn range(min: CborValue, max: CborValue) -> CborValue {
    CborValue::Map(vec![(CborValue::Uint(0), min), (CborValue::Uint(1), max)])
}

fn row_field<'a>(row: &'a CborValue, name: &str) -> &'a CborValue {
    row.get_text_key(name)
        .unwrap_or_else(|| panic!("reference row lacks {name}"))
}

/// SPEC-10 §10 test 1: build, sign, admit through the single ingress, reduce to exactly one
/// reference mutation carrying the §4.3 fields -- and the same mutation on replay.
#[tokio::test]
async fn observation_batch_op_signs_admits_and_reduces_to_reference_only() {
    let world = World::new();
    let payload = world.payload();
    let operation = world.batch(&payload.encode_canonical().expect("encode"));
    let verifier = world.verifier(Spec10Registry);

    let admitted = admit(&verifier, &operation)
        .await
        .expect("a signed, authorized, decryptable kind-5 batch admits");
    assert_eq!(admitted.candidate.meta.op_id, operation.op_id);
    assert_eq!(
        OperationKind::from_u16(admitted.candidate.meta.operation_kind),
        Ok(OperationKind::ObservationBatch)
    );
    let facts = admitted
        .payload_facts
        .as_ref()
        .expect("kind 5 carries payload facts");

    let materializer = ObservationBatchMaterializer;
    let mutation = materializer
        .reduce(&admitted.candidate.meta, &operation.bytes, Some(facts))
        .await;
    let ProjectionMutation::Apply {
        statements,
        bindings,
    } = &mutation
    else {
        panic!("a kind-5 reduction applies one reference row, got {mutation:?}");
    };
    assert_eq!(statements.len(), 1, "exactly one reference statement");
    assert_eq!(bindings.len(), 1, "exactly one reference row");
    assert_eq!(bindings[0].0, OBSERVATION_BATCH_ROW_BINDING);
    let row = &bindings[0].1;

    let bytes = |value: &[u8]| CborValue::Bytes(value.to_vec());
    assert_eq!(row_field(row, "op_id"), &bytes(&operation.op_id.0));
    assert_eq!(row_field(row, "petal_id"), &bytes(&support::PETAL_A.0));
    assert_eq!(
        row_field(row, "author_public_key"),
        &bytes(&support::public_key(&world.author))
    );
    assert_eq!(row_field(row, "hexon_artifact_id"), &bytes(&HEXON_ID.0));
    assert_eq!(row_field(row, "stats_hash"), &bytes(&STATS_ID.0));
    assert_eq!(
        row_field(row, "hexon_stored_length"),
        &CborValue::Uint(8_192)
    );
    assert_eq!(row_field(row, "stats_stored_length"), &CborValue::Uint(640));
    assert_eq!(row_field(row, "count"), &CborValue::Uint(3));
    assert_eq!(
        row_field(row, "sensor_time_min_ms"),
        &CborValue::Uint(1_699_999_000_000)
    );
    assert_eq!(row_field(row, "hlc_max_counter"), &CborValue::Uint(4));
    let mut anchors = [node_anchor(NODE_A).unwrap(), node_anchor(NODE_B).unwrap()];
    anchors.sort();
    assert_eq!(
        row_field(row, "anchors"),
        &CborValue::Array(anchors.iter().map(|anchor| bytes(&anchor.0)).collect())
    );
    assert_eq!(
        row_field(row, "asset_refs"),
        &CborValue::Array(vec![bytes(&[0xa1; 32]), bytes(&[0xa2; 32])])
    );
    let sources = row_field(row, "sources").as_array().expect("sources");
    assert_eq!(sources.len(), 2);
    assert!(
        sources[0].as_bytes() < sources[1].as_bytes(),
        "signed order"
    );

    // Replay with the same inputs is the same mutation (SPEC-4 §4 rule 5).
    assert_eq!(
        materializer
            .reduce(&admitted.candidate.meta, &operation.bytes, Some(facts))
            .await,
        mutation
    );
}

/// SPEC-10 §10 test 2: every structural (header) violation is `invalid_envelope`, every payload
/// violation is `invalid_payload`, and a foreign schema hash is `unknown_schema`.
#[tokio::test]
async fn observation_batch_structural_refusals() {
    let world = World::new();
    let verifier = world.verifier(Spec10Registry);
    let payload = world.payload();
    let valid = payload.encode_canonical().expect("encode");
    let schema = observation_batch_schema_hash();
    let kind = OBSERVATION_BATCH_OPERATION_KIND;
    let resource_scope = Scope::new(
        support::VERSE,
        Some(support::PETAL_A),
        Some(Identifier32([0x34; 32])),
    )
    .expect("resource scope");

    let header_cases = [
        (
            "zero parents",
            world.operation(kind, petal(), Vec::new(), schema, Some(&valid)),
        ),
        (
            "no-payload form",
            world.operation(kind, petal(), vec![support::hash(1)], schema, None),
        ),
        (
            "verse-wide scope",
            world.operation(
                kind,
                support::verse_scope(),
                vec![support::hash(1)],
                schema,
                Some(&valid),
            ),
        ),
        (
            "resource scope",
            world.operation(
                kind,
                resource_scope,
                vec![support::hash(1)],
                schema,
                Some(&valid),
            ),
        ),
    ];
    for (case, operation) in &header_cases {
        let outcome = admit(&verifier, operation).await;
        assert!(
            matches!(outcome, Err(AdmissionOutcome::InvalidEnvelope { .. })),
            "{case}: expected invalid_envelope, got {outcome:?}"
        );
    }

    let mut float_count = payload_with(&payload, 5, CborValue::Uint(0x0102_0304));
    let at = float_count
        .windows(6)
        .position(|window| window == [0x05, 0x1a, 0x01, 0x02, 0x03, 0x04])
        .expect("count encoding");
    float_count.splice(
        at + 1..at + 6,
        [0xfb, 0x41, 0x70, 0x20, 0x30, 0x40, 0, 0, 0],
    );
    let hlc = |wall_ms: u64| Hlc::new(wall_ms, 0).to_cbor();
    let payload_cases = [
        (
            "inverted HLC range",
            payload_with(
                &payload,
                2,
                range(hlc(support::NOW_MS), hlc(support::NOW_MS - 1)),
            ),
        ),
        (
            "inverted sensor range",
            payload_with(
                &payload,
                3,
                range(CborValue::integer(10), CborValue::integer(-10)),
            ),
        ),
        ("zero count", payload_with(&payload, 5, CborValue::Uint(0))),
        (
            "more anchors than count",
            payload_with(&payload, 5, CborValue::Uint(1)),
        ),
        (
            "unsorted anchors",
            payload_with(&payload, 6, reversed_array(&payload, 6)),
        ),
        (
            "unsorted asset refs",
            payload_with(&payload, 7, reversed_array(&payload, 7)),
        ),
        (
            "unsorted sources",
            payload_with(&payload, 8, reversed_array(&payload, 8)),
        ),
        (
            "unknown key",
            payload_with(&payload, 11, CborValue::Uint(0)),
        ),
        ("float anywhere", float_count),
    ];
    for (case, plaintext) in &payload_cases {
        let operation = world.batch(plaintext);
        assert_eq!(
            admit(&verifier, &operation).await.map(|_| ()),
            Err(AdmissionOutcome::InvalidPayload),
            "{case}"
        );
    }

    let foreign_schema = support::hash(0x77);
    let operation = world.operation(
        kind,
        petal(),
        vec![support::hash(1)],
        foreign_schema,
        Some(&valid),
    );
    assert_eq!(
        admit(&verifier, &operation).await.map(|_| ()),
        Err(AdmissionOutcome::UnknownSchema {
            schema_hash: foreign_schema
        })
    );

    // The untouched payload still admits, so each refusal above is its own mutation's doing.
    assert!(admit(&verifier, &world.batch(&valid)).await.is_ok());
}

fn record(operation: &Signed, reason: QuarantineReason, first_seen: Hlc) -> QuarantineRecord {
    QuarantineRecord {
        claimed_op_id: operation.op_id,
        derived_op_id: operation.op_id,
        candidate_bytes: operation.bytes.clone(),
        scope: petal(),
        reason,
        first_seen,
        last_validation: first_seen,
        provenance: "observation_batch_flow".to_string(),
    }
}

fn bounds(unknown_kind_entries: usize) -> QuarantineBounds {
    let wide = ReasonBudget {
        max_entries: 1_000,
        max_total_bytes: 10_000_000,
    };
    QuarantineBounds {
        max_entries: 10_000,
        max_total_bytes: 100_000_000,
        max_age_ms: 3_600_000,
        max_parent_depth: 8,
        retry_cadence_ms: 0,
        per_reason: PerReasonBudgets {
            missing_parent: ReasonBudget {
                max_entries: 8,
                max_total_bytes: 1_000_000,
            },
            unknown_schema: wide,
            unknown_kind: ReasonBudget {
                max_entries: unknown_kind_entries,
                max_total_bytes: 1_000_000,
            },
            other: wide,
        },
    }
}

/// SPEC-10 §10 test 3 (G1): a build without kind 5 quarantines kind-5 traffic at volume under
/// `unknown_kind`, charged to that class's own budget, and its missing-parent backlog survives
/// both the flood and the eviction that sheds it.
#[tokio::test]
async fn unupgraded_peer_quarantines_kind5_without_evicting_missing_parent_backlog() {
    const FLOOD: usize = 96;
    const UNKNOWN_KIND_BUDGET: usize = 16;
    let world = World::new();
    let unupgraded = world.verifier(PreSpec10Registry);
    let mut store = InMemoryQuarantineStore::default();
    let budgets = bounds(UNKNOWN_KIND_BUDGET);

    // The peer's own legitimate backlog: kind-1 work whose parent has not arrived yet.
    let missing = support::hash(0xdd);
    let mut backlog = Vec::new();
    for _ in 0..4 {
        let operation = world.operation(1, petal(), vec![missing], KIND_ONE_SCHEMA, Some(&[0xa0]));
        let outcome = admit_candidate_with_facts(
            &unupgraded,
            &support::NoEquivocation,
            &NoParentsHeld,
            &operation.bytes,
            Some(&operation.ciphertext),
        )
        .await;
        assert_eq!(
            outcome.map(|_| ()),
            Err(AdmissionOutcome::MissingParent { missing })
        );
        let reason = QuarantineReason::MissingParent {
            missing_parents: vec![missing],
        };
        quarantine_candidate(
            &mut store,
            &budgets,
            record(&operation, reason, Hlc::new(1, 0)),
        )
        .expect("backlog quarantines");
        backlog.push(operation.op_id);
    }

    // A volume of real kind-5 batches: never invalid_envelope, never unknown_schema.
    let plaintext = world.payload().encode_canonical().expect("encode");
    let mut quarantined = Vec::new();
    let mut declined = 0;
    for index in 0..FLOOD {
        let operation = world.batch(&plaintext);
        let outcome = admit(&unupgraded, &operation).await.map(|_| ());
        assert_eq!(
            outcome,
            Err(AdmissionOutcome::UnknownKind {
                operation_kind: OBSERVATION_BATCH_OPERATION_KIND
            }),
            "batch {index}"
        );
        let reason = QuarantineReason::UnknownKind {
            operation_kind: OBSERVATION_BATCH_OPERATION_KIND,
        };
        assert_eq!(reason.class(), QuarantineReasonClass::UnknownKind);
        let first_seen = Hlc::new(1_000 + index as u64, 0);
        match quarantine_candidate(&mut store, &budgets, record(&operation, reason, first_seen)) {
            Ok(()) => quarantined.push(operation),
            Err(QuarantineAdmitError::ReasonEntryCapacityExceeded { class, .. }) => {
                assert_eq!(class, QuarantineReasonClass::UnknownKind);
                declined += 1;
            }
            Err(other) => panic!("kind-5 flood declined against the wrong budget: {other:?}"),
        }
    }
    assert_eq!(quarantined.len(), UNKNOWN_KIND_BUDGET);
    assert_eq!(declined, FLOOD - UNKNOWN_KIND_BUDGET);
    for op_id in &backlog {
        assert!(store.get(op_id).is_some(), "the flood displaced backlog");
    }

    // Tightening the kind-5 budget sheds only kind-5 records, though the backlog is the oldest.
    let evicted = evict_expired_or_over_capacity(&mut store, &bounds(4), Hlc::new(2_000, 0));
    assert_eq!(evicted.len(), UNKNOWN_KIND_BUDGET - 4);
    assert!(evicted.iter().all(|op_id| !backlog.contains(op_id)));
    assert_eq!(
        store.class_len(QuarantineReasonClass::MissingParent),
        backlog.len()
    );

    // Once this build learns kind 5, the held batches promote and admit.
    let upgraded = world.verifier(Spec10Registry);
    let held = quarantined
        .iter()
        .find(|operation| store.get(&operation.op_id).is_some())
        .expect("a held batch");
    let held_reason = &store.get(&held.op_id).expect("held").reason;
    assert!(unknown_kind_promotion_ready(
        held_reason,
        &SpecRegisteredKinds
    ));
    assert!(admit(&upgraded, held).await.is_ok());
}

/// SPEC-10 §10 test 4 (G2): a peer holding the hexon and statistics artifact and a peer holding
/// nothing reduce the batch identically, and with no decrypted payload the reduction is pending
/// on the operation's own ciphertext -- never on the hexon.
#[tokio::test]
async fn reduce_is_identical_whether_or_not_the_hexon_is_present() {
    use fe_canonical_log::compose::ContentByteStore;

    let world = World::new();
    let hexon_bytes = b"sealed observation hexon".to_vec();
    let stats_bytes = b"sealed statistics artifact".to_vec();
    let mut parts = world.payload().into_parts();
    parts.hexon_artifact_id = Hash32::of(&hexon_bytes);
    parts.hexon_stored_length = hexon_bytes.len() as u64;
    parts.stats_hash = Hash32::of(&stats_bytes);
    parts.stats_stored_length = stats_bytes.len() as u64;
    let payload = ObservationBatchPayload::new(parts).expect("payload");
    let operation = world.batch(&payload.encode_canonical().expect("encode"));

    let mut holding = support::MemoryByteStore::default();
    holding.put(payload.parts().hexon_artifact_id, hexon_bytes);
    holding.put(payload.parts().stats_hash, stats_bytes);
    let empty = support::MemoryByteStore::default();
    assert!(holding.has(payload.parts().hexon_artifact_id));
    assert!(!empty.has(payload.parts().hexon_artifact_id));

    let mut reductions = Vec::new();
    for _peer in [&holding, &empty] {
        let admitted = admit(&world.verifier(Spec10Registry), &operation)
            .await
            .expect("admits on every peer");
        reductions.push(
            ObservationBatchMaterializer
                .reduce(
                    &admitted.candidate.meta,
                    &operation.bytes,
                    admitted.payload_facts.as_ref(),
                )
                .await,
        );
    }
    assert_eq!(reductions[0], reductions[1]);
    assert!(matches!(reductions[0], ProjectionMutation::Apply { .. }));

    // No decrypted payload at hand: pending on the payload ciphertext the envelope commits to.
    let meta = admit(&world.verifier(Spec10Registry), &operation)
        .await
        .expect("admits")
        .candidate
        .meta;
    let pending = ObservationBatchMaterializer
        .reduce(&meta, &operation.bytes, None)
        .await;
    assert_eq!(
        pending,
        ProjectionMutation::ReferencedArtifactUnavailable {
            artifact_id: Hash32::of(&operation.ciphertext)
        }
    );
    assert_ne!(
        Hash32::of(&operation.ciphertext),
        payload.parts().hexon_artifact_id
    );
}
