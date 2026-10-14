//! W4-7 flows: the ONE production `CandidateVerifier` (`canon_log::verifier`) over durable
//! authority sources, driven through the real DB thread's shared SurrealKV handle on an on-disk
//! store that is closed (thread joined) and reopened to prove persistence. Fixtures sign with
//! in-test keys and put chains through `ChainStore::put_verified_chain`.
//! Own binary (process-global HLC); see `fe-database/src/canon_log/AGENTS.md`
//! §durable-authority-sources and §production-verifier.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use fe_canonical_log::author_key::admission::AdmissionOutcome as LifecycleOutcome;
use fe_canonical_log::author_key::lineage::KeyState;
use fe_canonical_log::author_key::payloads::{
    RotationPayload, RotationStatement, ROTATION_PROTOCOL_VERSION,
};
use fe_canonical_log::author_key::rotation_proof::sign_successor_proof;
use fe_canonical_log::capability::{
    Caveats, Chain, ObjectClass, ObjectClassSet, Principal, RevalidationGate, UnsignedCertificate,
    Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::cbor::{decode_canonical, encode_canonical, CborValue};
use fe_canonical_log::checkpoint::{sign_checkpoint_claim, CheckpointClaimV1, CHECKPOINT_VERSION};
use fe_canonical_log::compose::{
    AuthorKeyLifecycleView, BindingSelection, Clock, ComposedCandidateVerifier, LifecycleDispatch,
    LifecycleKind, PayloadCiphertextSource, SystemClock,
};
use fe_canonical_log::crypto::aead::{seal, FreshNonce, SealingKey};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::CryptoError;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::materialize::errors::{
    AdmissionOutcome, AppendOutcome, AuthorizationEvidenceGap,
};
use fe_canonical_log::materialize::identity::MaterializerVersion;
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::signing::sign_envelope;
use fe_database::canon_log::admission::{AdmissionRejection, NoPrecondition};
use fe_database::canon_log::append_store::SurrealVerifiedLogStore;
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView, SurrealScopeEpochStore,
};
use fe_database::canon_log::chain_store::ChainStore;
use fe_database::canon_log::rebuild::{AdmittedCheckpoint, CheckpointOfferRejection};
use fe_database::canon_log::scope_key_custody::{
    scope_key_account, CustodyError, CustodyState, SecretStoreScopeKeyCustody, SCOPE_KEY_SERVICE,
};
use fe_database::canon_log::verifier::{
    admit_and_append_durably, durable_candidate_verifier, lifecycle_schemas, DurableAdmission,
    DurableAdmissionError, DurableCandidateVerifier,
};
use fe_database::{repo::Db, spawn_db_thread_shared, BlobStoreHandle, DbThread};
use fe_identity::{InMemoryBackend, SecretStore};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};

const TIMEOUT: Duration = Duration::from_secs(20);
const VERSE: Identifier32 = Identifier32([0x41; 32]);
const BRANCH: Identifier32 = Identifier32([0x42; 32]);
/// The one `issuer_authority_id` this deployment recognises (§interim-manager-authority).
const ANCHOR: Hash32 = Hash32([0xa7; 32]);
const MANAGER: u8 = 0x11;
const AUTHOR: u8 = 0x12;
const SUCCESSOR: u8 = 0x13;
const EDITOR: u8 = 0x14;
const SCOPE_KEY_BYTES: [u8; 32] = [0x5c; 32];
/// The epoch the verse sits at after the seeded bump.
const EPOCH: u64 = 1;
/// An application schema the tests register on the built verifier (critic C3's call).
const NOTE_SCHEMA: Hash32 = Hash32([0x70; 32]);
const HLC_WALL_MS: u64 = 1_700_000_000_000;

/// A DB thread over a store directory the TEST owns, plus the shared handle canon_log code runs
/// against. `Drop` releases the handle, closes the command channel and joins the thread, so the
/// SurrealKV lock is free before the next open (one open per process, `558197c`).
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

fn verse_key() -> ScopeKeyId {
    ScopeKeyId {
        scope: verse_scope(),
        epoch: EPOCH,
    }
}

/// A one-certificate chain the Manager issues to `audience`: `verbs` x `classes` over the
/// verse at [`EPOCH`], valid for an hour around the real clock the durable verifier reads.
fn manager_chain(audience: u8, verbs: &[Verb], classes: &[ObjectClass]) -> Vec<u8> {
    let now = SystemClock.now_ms();
    let certificate = UnsignedCertificate {
        capability_version: CAPABILITY_VERSION,
        issuer: principal(MANAGER),
        audience: principal(audience),
        parent_certificate_id: None,
        issuer_authority_id: ANCHOR,
        grant_scope: verse_scope(),
        epoch_scope: verse_scope(),
        scope_epoch: EPOCH,
        verbs: VerbSet::from_verbs(verbs.iter().copied()).expect("verbs"),
        object_classes: ObjectClassSet::from_classes(classes.iter().copied()).expect("classes"),
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

async fn seed_role(db: &Db, seed: u8, role: &str) {
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
        .bind(("did", principal(seed).did))
        .bind(("scope", interim_authority_scope_key(&verse_scope())))
        .bind(("role", role.to_string()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");
}

/// The Manager's signed kind-4 bump, appended and admitted through `admit_epoch_bump`, moving
/// the verse 0 -> [`EPOCH`]. Every later operation descends from it.
async fn bump_verse_to_epoch_one(db: &Db) -> Hash32 {
    let unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: OperationKind::ScopeEpochBump.to_u16(),
        scope: verse_scope(),
        author: Author::from_public_key(public_key(MANAGER)),
        capability: CapabilityRef {
            chain_id: Hash32([0x33; 32]),
            scope_epoch: 0,
        },
        schema_hash: Hash32([0x44; 32]),
        branch_id: BRANCH,
        parents: vec![Hash32([0x66; 32])],
        hlc: Hlc::new(HLC_WALL_MS, 0),
        payload: PayloadRef::empty(),
    };
    let bytes = sign_envelope(&signing_key(MANAGER), &unsigned)
        .expect("sign bump")
        .encode_canonical()
        .expect("encode bump");
    let op_id = Hash32::of(&bytes);
    let store = SurrealVerifiedLogStore::new(db.clone());
    store
        .append_received(op_id, &bytes)
        .await
        .expect("append bump");
    let view = DurableAuthorizationView::load(db, Some(ANCHOR))
        .await
        .expect("view");
    SurrealScopeEpochStore::new(db.clone())
        .admit_epoch_bump(&store, &view, op_id, &mut RevalidationGate::new())
        .await
        .expect("the Manager's bump admits");
    op_id
}

/// A Manager, the verse at [`EPOCH`], an `append/op` chain for [`AUTHOR`], and the verse
/// scope key in durable custody. Returns the bump (the DAG root) and the key's `key_id`.
async fn seed_verse(
    db: &Db,
    secrets: &Arc<dyn SecretStore>,
    author_chain: &[u8],
) -> (Hash32, Identifier32) {
    seed_role(db, MANAGER, "manager").await;
    let bump = bump_verse_to_epoch_one(db).await;
    ChainStore::load(db)
        .await
        .expect("chain store")
        .put_verified_chain(author_chain)
        .await
        .expect("put the author's chain");
    let key_id = SecretStoreScopeKeyCustody::load(db, Arc::clone(secrets))
        .await
        .expect("custody")
        .take_custody(verse_key(), SealingKey::from_bytes(SCOPE_KEY_BYTES))
        .await
        .expect("take custody of the verse key");
    (bump, key_id)
}

struct Sealed {
    bytes: Vec<u8>,
    ciphertext: Vec<u8>,
    op_id: Hash32,
}

/// The `append/op` chain the Manager issues [`AUTHOR`]. Minted once per test: its validity
/// window is read from the real clock, so two mints are two different chains.
fn author_chain() -> Vec<u8> {
    manager_chain(AUTHOR, &[Verb::Append], &[ObjectClass::Operation])
}

/// A kind-1 operation by `author` naming `chain_id`, its payload sealed under the verse key
/// with the SPEC-1 §5.2 AAD.
fn sealed_operation(
    author: u8,
    chain_id: Hash32,
    parents: Vec<Hash32>,
    schema_hash: Hash32,
    plaintext: &[u8],
    counter: u32,
) -> Sealed {
    let key = SealingKey::from_bytes(SCOPE_KEY_BYTES);
    let key_id = derive_key_id(&verse_scope(), EPOCH, &key).expect("key id");
    let nonce = FreshNonce::draw(key_id).expect("nonce");
    let mut unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: 1,
        scope: verse_scope(),
        author: Author::from_public_key(public_key(author)),
        capability: CapabilityRef {
            chain_id,
            scope_epoch: EPOCH,
        },
        schema_hash,
        branch_id: BRANCH,
        parents,
        hlc: Hlc::new(HLC_WALL_MS, counter),
        payload: PayloadRef {
            encryption: Some(nonce.params()),
            ..PayloadRef::empty()
        },
    };
    let aad = payload_aad_preimage(&unsigned).expect("aad");
    let sealed = seal(&key, nonce, &aad, plaintext).expect("seal");
    unsigned.payload = sealed.payload_ref();
    let bytes = sign_envelope(&signing_key(author), &unsigned)
        .expect("sign")
        .encode_canonical()
        .expect("encode");
    Sealed {
        op_id: Hash32::of(&bytes),
        bytes,
        ciphertext: sealed.ciphertext,
    }
}

fn note(value: u64) -> Vec<u8> {
    encode_canonical(&CborValue::Map(vec![(
        CborValue::Uint(0),
        CborValue::Uint(value),
    )]))
}

async fn verifier(db: &Db, secrets: &Arc<dyn SecretStore>) -> DurableCandidateVerifier {
    let loaded = durable_candidate_verifier(db, Arc::clone(secrets), Some(ANCHOR))
        .await
        .expect("durable verifier");
    loaded
        .verifier
        .parts()
        .schemas
        .register(1, NOTE_SCHEMA, |bytes| decode_canonical(bytes).is_ok())
        .expect("register the note schema");
    loaded.verifier
}

async fn admit(
    verifier: &DurableCandidateVerifier,
    db: &Db,
    operation: &Sealed,
) -> Result<DurableAdmission, DurableAdmissionError> {
    admit_and_append_durably(
        verifier,
        &SurrealVerifiedLogStore::new(db.clone()),
        &NoPrecondition,
        &operation.bytes,
        Some(&operation.ciphertext),
    )
    .await
}

async fn logged_operations(db: &Db) -> usize {
    SurrealVerifiedLogStore::new(db.clone())
        .all_rows()
        .await
        .expect("verified log scan")
        .len()
}

fn rejected_outcome(result: Result<DurableAdmission, DurableAdmissionError>) -> AdmissionRejection {
    match result {
        Err(DurableAdmissionError::Admission(rejection)) => rejection,
        other => panic!("expected an admission refusal, got {other:?}"),
    }
}

#[test]
fn a_locally_signed_operation_admits_appends_and_replays_through_the_durable_verifier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = author_chain();
    let chain_id = Hash32::of(&chain);

    let first = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            let (bump, _) = seed_verse(&db, &secrets, &chain).await;
            let verifier = verifier(&db, &secrets).await;

            let first = sealed_operation(AUTHOR, chain_id, vec![bump], NOTE_SCHEMA, &note(7), 1);
            let admitted = admit(&verifier, &db, &first).await.expect("admits");
            assert_eq!(admitted.append.append_outcome, AppendOutcome::Appended);
            assert_eq!(admitted.append.meta.op_id, first.op_id);
            assert_eq!(admitted.lifecycle, Ok(LifecycleDispatch::NotLifecycle));
            let facts = admitted.append.payload_facts.expect("payload facts");
            assert_eq!(facts.fields, decode_canonical(&note(7)).expect("note"));

            // The registry is the gate: a schema this build never registered is held, not
            // appended and not rejected.
            let unregistered = sealed_operation(
                AUTHOR,
                chain_id,
                vec![bump],
                Hash32([0x71; 32]),
                &note(8),
                2,
            );
            let held = rejected_outcome(admit(&verifier, &db, &unregistered).await);
            assert!(
                matches!(
                    held,
                    AdmissionRejection::Quarantined {
                        outcome: AdmissionOutcome::UnknownSchema { .. }
                    }
                ),
                "{held:?}"
            );
            assert_eq!(
                logged_operations(&db).await,
                2,
                "bump + the admitted operation"
            );
            first
        })
    }; // DB thread joined, store closed

    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let loaded = durable_candidate_verifier(&db, Arc::clone(&secrets), Some(ANCHOR))
            .await
            .expect("durable verifier after restart");
        assert_eq!(loaded.lifecycle_replay.operations, 2);
        assert!(loaded.lifecycle_replay.unreadable_rows.is_empty());
        let verifier = loaded.verifier;
        verifier
            .parts()
            .schemas
            .register(1, NOTE_SCHEMA, |bytes| decode_canonical(bytes).is_ok())
            .expect("register");

        // The admitted ciphertext survived in the local artifact store.
        assert_eq!(
            verifier
                .parts()
                .ciphertexts
                .ciphertext(&Hash32::of(&first.ciphertext)),
            Some(first.ciphertext.clone())
        );

        // Exactly-once across the restart: the same bytes are already present, not re-appended.
        let again = admit(&verifier, &db, &first).await.expect("re-admits");
        assert_eq!(again.append.append_outcome, AppendOutcome::AlreadyPresent);

        // And the durable verifier still admits new work descending from it.
        let child = sealed_operation(
            AUTHOR,
            chain_id,
            vec![first.op_id],
            NOTE_SCHEMA,
            &note(9),
            3,
        );
        let admitted = admit(&verifier, &db, &child).await.expect("child admits");
        assert_eq!(admitted.append.append_outcome, AppendOutcome::Appended);
        assert_eq!(logged_operations(&db).await, 3);
    });
}

/// The rotation payload `AUTHOR -> SUCCESSOR`, parented on `parent`.
fn rotation_plaintext(parent: Hash32) -> Vec<u8> {
    let statement = RotationStatement {
        protocol_version: ROTATION_PROTOCOL_VERSION,
        scope: verse_scope(),
        branch_id: BRANCH,
        parent_op_id: parent,
        scope_epoch: EPOCH,
        predecessor_did: principal(AUTHOR).did,
        predecessor_public_key: public_key(AUTHOR),
        successor_did: principal(SUCCESSOR).did,
        successor_public_key: public_key(SUCCESSOR),
    };
    RotationPayload {
        successor_signature: sign_successor_proof(&signing_key(SUCCESSOR), &statement)
            .expect("successor proof"),
        statement,
    }
    .encode_canonical()
    .expect("rotation payload")
}

#[test]
fn an_operation_after_its_authors_rotation_is_refused_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = author_chain();
    let chain_id = Hash32::of(&chain);

    let (bump, rotation) = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            let (bump, _) = seed_verse(&db, &secrets, &chain).await;
            let verifier = verifier(&db, &secrets).await;

            let rotation = sealed_operation(
                AUTHOR,
                chain_id,
                vec![bump],
                lifecycle_schemas().rotation,
                &rotation_plaintext(bump),
                1,
            );
            let admitted = admit(&verifier, &db, &rotation)
                .await
                .expect("the rotation admits");
            assert_eq!(
                admitted.lifecycle,
                Ok(LifecycleDispatch::Dispatched {
                    kind: LifecycleKind::Rotation,
                    result: Ok(LifecycleOutcome::Admitted),
                }),
                "the durable door hands the rotation to SPEC-2"
            );

            // Live: the very next admission already sees the predecessor retired.
            let after = sealed_operation(
                AUTHOR,
                chain_id,
                vec![rotation.op_id],
                NOTE_SCHEMA,
                &note(1),
                2,
            );
            let refused = rejected_outcome(admit(&verifier, &db, &after).await);
            assert!(
                matches!(
                    refused,
                    AdmissionRejection::Rejected {
                        outcome: AdmissionOutcome::UnauthorizedOperation,
                        ..
                    }
                ),
                "{refused:?}"
            );
            (bump, rotation)
        })
    };

    // Restart: nothing but the verified log, the artifact store, and the keystore carry over.
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let loaded = durable_candidate_verifier(&db, Arc::clone(&secrets), Some(ANCHOR))
            .await
            .expect("durable verifier after restart");
        assert_eq!(
            loaded.lifecycle_replay.dispatched,
            vec![(
                rotation.op_id,
                LifecycleDispatch::Dispatched {
                    kind: LifecycleKind::Rotation,
                    result: Ok(LifecycleOutcome::Admitted),
                }
            )],
            "the rotation is re-derived from the log, not remembered by the process"
        );
        assert!(loaded.lifecycle_replay.held.is_empty());
        let verifier = loaded.verifier;
        verifier
            .parts()
            .schemas
            .register(1, NOTE_SCHEMA, |bytes| decode_canonical(bytes).is_ok())
            .expect("register");
        assert!(matches!(
            verifier.parts().lifecycle.key_state(
                &public_key(AUTHOR),
                &verse_scope(),
                &[rotation.op_id]
            ),
            KeyState::Retired { .. }
        ));

        let after = sealed_operation(
            AUTHOR,
            chain_id,
            vec![rotation.op_id],
            NOTE_SCHEMA,
            &note(2),
            3,
        );
        let refused = rejected_outcome(admit(&verifier, &db, &after).await);
        assert!(
            matches!(
                refused,
                AdmissionRejection::Rejected {
                    outcome: AdmissionOutcome::UnauthorizedOperation,
                    ..
                }
            ),
            "{refused:?}"
        );

        // Control: causally concurrent with the rotation the same key is not retroactively
        // invalid (SPEC-2 §4 rule 5), so the refusal above is the lineage's, not the chain's.
        let concurrent = sealed_operation(AUTHOR, chain_id, vec![bump], NOTE_SCHEMA, &note(3), 4);
        let admitted = admit(&verifier, &db, &concurrent)
            .await
            .expect("a concurrent operation admits");
        assert_eq!(admitted.append.append_outcome, AppendOutcome::Appended);
    });
}

/// A signed checkpoint claim by `signer` naming `chain_id`, matching [`selection`].
fn checkpoint_claim(signer: u8, chain_id: Hash32, frontier: &SortedFrontier) -> Vec<u8> {
    let version = MaterializerVersion::new("scene-graph", 1);
    let claim = CheckpointClaimV1 {
        checkpoint_version: CHECKPOINT_VERSION,
        verse_id: VERSE,
        branch_id: BRANCH,
        frontier_commitment: frontier.commitment(),
        segment_manifest_id: Hash32([0x51; 32]),
        materializer_id: version.registry_id(),
        materializer_version: 1,
        projection_root_hash: Hash32([0x7a; 32]),
        authorization_view_root: Hash32([0x00; 32]),
        snapshot_manifest_id: None,
        signer: Author::from_public_key(public_key(signer)),
        capability: CapabilityRef {
            chain_id,
            scope_epoch: EPOCH,
        },
        issued_hlc: Hlc::new(HLC_WALL_MS, 0),
    };
    sign_checkpoint_claim(&signing_key(signer), &claim)
        .expect("sign claim")
        .encode_canonical()
        .expect("encode claim")
}

#[test]
fn a_checkpoint_from_a_non_manager_signer_is_not_admitted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        seed_role(&db, MANAGER, "manager").await;
        seed_role(&db, EDITOR, "editor").await;
        let bump = bump_verse_to_epoch_one(&db).await;

        let chains = ChainStore::load(&db).await.expect("chain store");
        let checkpoint = [Verb::Append];
        let classes = [ObjectClass::Checkpoint];
        let manager_chain_id = chains
            .put_verified_chain(&manager_chain(MANAGER, &checkpoint, &classes))
            .await
            .expect("manager chain")
            .chain_id;
        let editor_chain_id = chains
            .put_verified_chain(&manager_chain(EDITOR, &checkpoint, &classes))
            .await
            .expect("editor chain")
            .chain_id;
        let view = DurableAuthorizationView::load_with_chain_store(&db, Some(ANCHOR), chains)
            .await
            .expect("durable view");

        let frontier = SortedFrontier::try_new([bump]).expect("frontier");
        let version = MaterializerVersion::new("scene-graph", 1);
        let selection = BindingSelection {
            branch_id: BRANCH,
            frontier: &frontier,
            segment_manifest_id: Hash32([0x51; 32]),
            materializer_id: version.registry_id(),
            materializer_version: &version,
        };

        // An Editor holding a valid, Manager-issued append/checkpoint chain is still not
        // Manager+: the capability is necessary, never sufficient.
        assert_eq!(
            AdmittedCheckpoint::admit(
                &checkpoint_claim(EDITOR, editor_chain_id, &frontier),
                &view,
                &selection
            ),
            Err(CheckpointOfferRejection::NotManagerPlus)
        );

        // The Manager naming a chain this process does not hold, or someone else's held chain,
        // has no append/checkpoint capability here.
        for chain_id in [Hash32([0x99; 32]), editor_chain_id] {
            assert_eq!(
                AdmittedCheckpoint::admit(
                    &checkpoint_claim(MANAGER, chain_id, &frontier),
                    &view,
                    &selection
                ),
                Err(CheckpointOfferRejection::CapabilityDoesNotPermitCheckpoint),
                "chain {chain_id:?}"
            );
        }

        // Control: the Manager's own chain admits the identical claim shape.
        AdmittedCheckpoint::admit(
            &checkpoint_claim(MANAGER, manager_chain_id, &frontier),
            &view,
            &selection,
        )
        .expect("a Manager+ signer with its own append/checkpoint chain is admitted");
    });
}

#[test]
fn shredding_a_scope_key_turns_later_admission_into_quarantine_not_reject() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = runtime();
    let secrets: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
    let chain = author_chain();
    let chain_id = Hash32::of(&chain);
    let payload_key_gap = AdmissionOutcome::AuthorizationEvidenceUnavailable {
        reason: AuthorizationEvidenceGap::payload_key_unavailable(),
    };

    let (bump, key_id) = {
        let harness = Harness::open(dir.path());
        runtime.block_on(async {
            let db = harness.db();
            let (bump, key_id) = seed_verse(&db, &secrets, &chain).await;
            assert!(
                secrets
                    .get(SCOPE_KEY_SERVICE, &scope_key_account(key_id))
                    .expect("keystore")
                    .is_some(),
                "custody files the material in the keystore"
            );
            let verifier = verifier(&db, &secrets).await;
            let before = sealed_operation(AUTHOR, chain_id, vec![bump], NOTE_SCHEMA, &note(1), 1);
            admit(&verifier, &db, &before)
                .await
                .expect("admits while the key is held");

            // Shred through the verifier's own custody (SPEC-5 §5.5.4).
            let mut parts = verifier.into_parts();
            parts
                .scope_keys
                .shred(&verse_key(), Hlc::new(HLC_WALL_MS, 9))
                .await
                .expect("shred");
            let verifier = ComposedCandidateVerifier::new(parts);

            let after = sealed_operation(AUTHOR, chain_id, vec![bump], NOTE_SCHEMA, &note(2), 2);
            let held = rejected_outcome(admit(&verifier, &db, &after).await);
            assert!(
                matches!(&held, AdmissionRejection::Quarantined { outcome } if *outcome == payload_key_gap),
                "a shredded key is missing evidence, never a reject: {held:?}"
            );
            assert_eq!(logged_operations(&db).await, 2, "nothing appended");
            (bump, key_id)
        })
    };

    // The shred is durable: the keystore entry is gone and a restart does not resurrect it.
    assert_eq!(
        secrets
            .get(SCOPE_KEY_SERVICE, &scope_key_account(key_id))
            .expect("keystore"),
        None
    );
    let harness = Harness::open(dir.path());
    runtime.block_on(async {
        let db = harness.db();
        let mut custody = SecretStoreScopeKeyCustody::load(&db, Arc::clone(&secrets))
            .await
            .expect("custody after restart");
        assert_eq!(
            custody.custody_state(&verse_key()),
            Some(CustodyState::Destroyed)
        );
        // A shredded epoch is never re-keyed.
        assert!(matches!(
            custody
                .take_custody(verse_key(), SealingKey::from_bytes([0x5d; 32]))
                .await,
            Err(CustodyError::Refused(CryptoError::IssuanceStopped { epoch: EPOCH }))
        ));

        let verifier = verifier(&db, &secrets).await;
        let after = sealed_operation(AUTHOR, chain_id, vec![bump], NOTE_SCHEMA, &note(3), 3);
        let held = rejected_outcome(admit(&verifier, &db, &after).await);
        assert!(
            matches!(&held, AdmissionRejection::Quarantined { outcome } if *outcome == payload_key_gap),
            "{held:?}"
        );
        assert_eq!(logged_operations(&db).await, 2);
    });
}
