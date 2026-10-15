//! W4-8 flows (CDL-R2, DB half): the local enrol/revoke commands through the real DB thread on
//! an on-disk store, inbound enrolment rows through the production
//! `apply_replicated_row_handler`, and the ledger's three §10.2.1 delivery outcomes over the
//! durable views. Own binary; see `fe-database/src/canon_log/AGENTS.md` §device-enrolment.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use fe_canonical_log::capability::{
    Caveats, Chain, ObjectClass, ObjectClassSet, Principal, RevalidationGate, UnsignedCertificate,
    Verb, VerbSet, CAPABILITY_VERSION,
};
use fe_canonical_log::compose::{Clock, SystemClock};
use fe_canonical_log::crypto::aead::{SealingKey, SEALING_KEY_LENGTH};
use fe_canonical_log::crypto::device_enrolment::{
    enrolment_record_id, DeviceEnrolmentRevocationV1, DeviceEnrolmentV1, SignedDeviceEnrolment,
    DEVICE_ENROLMENT_DOMAIN,
};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::key_wrap::{
    issue_scope_key_wrap, open_scope_key, AuthorizationRecord, DeviceEnrolmentView,
    DevicePublicKey, DeviceSecretKey, RecipientDeviceBinding, ScopeKeyWrap, ScopeKeyWrapRequest,
};
use fe_canonical_log::crypto::CryptoError;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::kind::OperationKind;
use fe_canonical_log::segment::hashseq::LaneKey;
use fe_canonical_log::segment::relay_policy::{PeerIdentity, RelayAuthorizationView};
use fe_canonical_log::signing::{sign_domain, sign_envelope};
use fe_database::canon_log::append_store::SurrealVerifiedLogStore;
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView, SurrealScopeEpochStore,
};
use fe_database::canon_log::chain_store::ChainStore;
use fe_database::canon_log::device_enrolment::{
    DurableDeliveryView, DurableDeviceEnrolmentView, EnrolmentRow, DEVICE_ENROLMENT_TABLE,
    LOCAL_DEVICE_KEY_ACCOUNT,
};
use fe_database::canon_log::op_id_to_hex;
use fe_database::handlers::inbound_admissions::InboundAdmissions;
use fe_database::handlers::replicated_row::apply_replicated_row_handler;
use fe_database::{repo::Db, spawn_db_thread_shared, DbThread, ReplicationEvent};
use fe_identity::{InMemoryBackend, NodeKeypair, SecretStore};
use fe_policy::RoleStatementPayload;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult, DeferralReason, ReplicatedRowOutcome};

const TIMEOUT: Duration = Duration::from_secs(20);
const VERSE_ID: &str = "verse-enrol";
const NODE_SEED: [u8; 32] = [0x21; 32];
/// The canonical verse the delivery flow seals for, and its one recognised authority anchor.
const VERSE: Identifier32 = Identifier32([0x41; 32]);
const BRANCH: Identifier32 = Identifier32([0x42; 32]);
const ANCHOR: Hash32 = Hash32([0xa7; 32]);
const EPOCH: u64 = 1;
const HLC_WALL_MS: u64 = 1_700_000_000_000;
const MANAGER: u8 = 0x11;
const RECIPIENT: u8 = 0x12;
const EDITOR: u8 = 0x14;
const STRANGER: u8 = 0x15;

/// A DB thread over a store directory the TEST owns, with the replication bridge exposed.
/// `Drop` releases the shared handle, closes the command channel and joins the thread, so the
/// SurrealKV lock is free before any reopen (one open per process, `558197c`).
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    repl_rx: crossbeam::channel::Receiver<ReplicationEvent>,
    blobs: Arc<MockBlobStore>,
    db: Option<Db>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn open(
        dir: &Path,
        keypair: Option<NodeKeypair>,
        secrets: Option<Arc<dyn SecretStore>>,
    ) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let (repl_tx, repl_rx) = crossbeam::channel::bounded::<ReplicationEvent>(64);
        let blobs = Arc::new(MockBlobStore::new());
        let db_path = dir.join("fe.db").to_string_lossy().to_string();
        let DbThread {
            join_handle,
            shared_db,
        } = spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            blobs.clone(),
            Some(repl_tx),
            keypair,
            secrets,
            None,
            Some(db_path),
            None,
        )
        .expect("spawn db thread");
        let started = res_rx.recv_timeout(TIMEOUT).expect("first DB result");
        assert!(matches!(started, DbResult::Started), "got {started:?}");
        Self {
            cmd_tx: Some(cmd_tx),
            res_rx,
            repl_rx,
            blobs,
            db: Some(shared_db),
            join: Some(join_handle),
        }
    }

    fn db(&self) -> Db {
        self.db.clone().expect("open store")
    }

    /// Sends one enrolment command and returns its reply.
    fn request(&self, command: DbCommand) -> DbResult {
        self.cmd_tx
            .as_ref()
            .expect("open channel")
            .send(command)
            .expect("send command");
        loop {
            let result = self.res_rx.recv_timeout(TIMEOUT).expect("DB reply");
            if matches!(
                result,
                DbResult::LocalDeviceEnrolled { .. }
                    | DbResult::LocalDeviceRevoked { .. }
                    | DbResult::RequestFailed { .. }
                    | DbResult::Error(_)
            ) {
                return result;
            }
        }
    }

    fn enrol(&self, correlation_id: &str) -> DbResult {
        self.request(DbCommand::EnrolLocalDevice {
            verse_id: VERSE_ID.to_string(),
            correlation_id: Some(correlation_id.to_string()),
        })
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

fn node_keypair() -> NodeKeypair {
    NodeKeypair::from_bytes(&NODE_SEED).expect("node keypair")
}

fn node_principal() -> Principal {
    Principal::from_public_key(node_keypair().verifying_key().to_bytes())
}

fn signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn principal(seed: u8) -> Principal {
    Principal::from_public_key(signing_key(seed).verifying_key().to_bytes())
}

fn device(seed: u8) -> DeviceSecretKey {
    DeviceSecretKey::from_bytes([seed; 32])
}

fn in_memory_secrets() -> Arc<dyn SecretStore> {
    Arc::new(InMemoryBackend::new())
}

/// A legacy verse row, as the production create paths write it (owner = `created_by`).
async fn seed_verse_row(db: &Db, verse_id: &str) {
    let _: Option<serde_json::Value> = db
        .create("verse")
        .content(serde_json::json!({
            "verse_id": verse_id,
            "name": "Enrolment Verse",
            "created_by": "did:key:verse-owner",
            "created_at": "2026-10-14T00:00:00Z",
            "default_access": "viewer",
        }))
        .await
        .expect("seed verse");
}

async fn stored_row(db: &Db, record_id: &str) -> Option<EnrolmentRow> {
    let mut response = db
        .query(
            "SELECT record_id, principal_did, device_key_hex, artifact_cbor, revoked \
             FROM canon_device_enrolment WHERE record_id = $rid",
        )
        .bind(("rid", record_id.to_string()))
        .await
        .expect("read enrolment row");
    let rows: Vec<serde_json::Value> = response.take(0).expect("take rows");
    rows.into_iter()
        .next()
        .map(|row| serde_json::from_value(row).expect("an enrolment row"))
}

fn enrolment_bytes(principal_seed: u8, device_seed: u8, enrolled_at_ms: u64) -> Vec<u8> {
    DeviceEnrolmentV1 {
        principal: principal(principal_seed),
        device_key: device(device_seed).public_key(),
        enrolled_at_ms,
    }
    .sign(&signing_key(principal_seed))
    .expect("sign enrolment")
    .encode_canonical()
    .expect("encode enrolment")
}

fn revocation_bytes(principal_seed: u8, device_seed: u8, revoked_at_ms: u64) -> Vec<u8> {
    DeviceEnrolmentRevocationV1 {
        principal: principal(principal_seed),
        device_key: device(device_seed).public_key(),
        revoked_at_ms,
    }
    .sign(&signing_key(principal_seed))
    .expect("sign revocation")
    .encode_canonical()
    .expect("encode revocation")
}

/// The replicated row for artifact bytes, exactly as a publishing peer emits it.
fn row_for(artifact_bytes: &[u8]) -> EnrolmentRow {
    EnrolmentRow::of(
        &fe_canonical_log::crypto::VerifiedEnrolmentArtifact::decode_and_verify(artifact_bytes)
            .expect("fixture artifact verifies"),
    )
}

fn row_json(row: &EnrolmentRow) -> Vec<u8> {
    serde_json::to_vec(row).expect("row json")
}

async fn apply_inbound(
    db: &Db,
    verse_id: &str,
    row: &EnrolmentRow,
    author_did: &str,
) -> ReplicatedRowOutcome {
    apply_replicated_row_handler(
        db,
        verse_id,
        DEVICE_ENROLMENT_TABLE,
        &row.record_id,
        &row_json(row),
        author_did,
        None,
    )
    .await
    .expect("apply inbound row")
}

#[test]
fn a_local_enrolment_is_stored_signed_and_emitted_for_replication() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secrets = in_memory_secrets();
    let harness = Harness::open(dir.path(), Some(node_keypair()), Some(Arc::clone(&secrets)));
    let rt = runtime();
    rt.block_on(seed_verse_row(&harness.db(), VERSE_ID));

    let DbResult::LocalDeviceEnrolled {
        verse_id,
        record_id_hex,
        principal_did,
        device_key_hex,
        correlation_id,
    } = harness.enrol("enrol-1")
    else {
        panic!("EnrolLocalDevice must reply LocalDeviceEnrolled");
    };
    assert_eq!(verse_id, VERSE_ID);
    assert_eq!(correlation_id.as_deref(), Some("enrol-1"));
    assert_eq!(
        principal_did,
        node_keypair().to_did_key(),
        "the principal is the node identity peers attest as the entry author"
    );
    let device_key = fe_identity::load_device_keypair(&secrets, LOCAL_DEVICE_KEY_ACCOUNT)
        .expect("the device secret is persisted in the secret store")
        .public_key();
    assert_eq!(device_key_hex, hex::encode(device_key));

    // The stored row is the principal's own signed enrolment, not a placeholder.
    let stored = rt
        .block_on(stored_row(&harness.db(), &record_id_hex))
        .expect("the enrolment row is stored");
    let artifact = stored.verify().expect("the stored row verifies");
    assert!(!artifact.is_revocation());
    assert_eq!(artifact.principal(), &node_principal());
    assert_eq!(artifact.device_key(), DevicePublicKey(device_key));

    // Exactly that row rides the existing replication bridge into the verse's replica.
    let event = harness
        .repl_rx
        .recv_timeout(TIMEOUT)
        .expect("a replication event for the enrolment");
    assert_eq!(event.verse_id, VERSE_ID);
    assert_eq!(event.table, DEVICE_ENROLMENT_TABLE);
    assert_eq!(event.record_id, record_id_hex);
    let published: EnrolmentRow = serde_json::from_slice(
        &harness
            .blobs
            .bytes_for(&event.content_hash)
            .expect("the published row bytes are in the blob store"),
    )
    .expect("published row json");
    assert_eq!(published, stored);

    let view = rt
        .block_on(DurableDeviceEnrolmentView::load(&harness.db()))
        .expect("load view");
    assert_eq!(
        view.device_enrolment(&node_principal(), DevicePublicKey(device_key)),
        AuthorizationRecord::Granted
    );
}

#[test]
fn revoking_the_local_device_refuses_it_and_rotates_to_a_fresh_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let secrets = in_memory_secrets();
    let harness = Harness::open(dir.path(), Some(node_keypair()), Some(Arc::clone(&secrets)));
    let rt = runtime();
    rt.block_on(seed_verse_row(&harness.db(), VERSE_ID));

    let DbResult::LocalDeviceEnrolled {
        device_key_hex: first_key,
        ..
    } = harness.enrol("enrol-1")
    else {
        panic!("first enrolment");
    };
    let DbResult::LocalDeviceRevoked {
        device_key_hex: revoked_key,
        record_id_hex,
        ..
    } = harness.request(DbCommand::RevokeLocalDevice {
        verse_id: VERSE_ID.to_string(),
        device_key_hex: first_key.clone(),
        correlation_id: Some("revoke-1".to_string()),
    })
    else {
        panic!("RevokeLocalDevice must reply LocalDeviceRevoked");
    };
    assert_eq!(revoked_key, first_key);
    let stored = rt
        .block_on(stored_row(&harness.db(), &record_id_hex))
        .expect("the slot is kept");
    assert!(stored.revoked && stored.verify().expect("verifies").is_revocation());
    assert!(
        fe_identity::load_device_keypair(&secrets, LOCAL_DEVICE_KEY_ACCOUNT).is_err(),
        "revoking the node's own key deletes its secret"
    );

    let DbResult::LocalDeviceEnrolled {
        device_key_hex: second_key,
        ..
    } = harness.enrol("enrol-2")
    else {
        panic!("re-enrolment after revocation");
    };
    assert_ne!(second_key, first_key, "a revoked key is never re-enrolled");

    let view = rt
        .block_on(DurableDeviceEnrolmentView::load(&harness.db()))
        .expect("load view");
    let key = |hex_key: &str| {
        DevicePublicKey(
            hex::decode(hex_key)
                .expect("hex")
                .try_into()
                .expect("32 bytes"),
        )
    };
    assert_eq!(
        view.device_enrolment(&node_principal(), key(&first_key)),
        AuthorizationRecord::Refused
    );
    assert_eq!(
        view.device_enrolment(&node_principal(), key(&second_key)),
        AuthorizationRecord::Granted
    );
}

#[test]
fn an_enrolment_without_a_node_keypair_is_refused_not_placeholder_signed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, Some(in_memory_secrets()));
    let rt = runtime();
    rt.block_on(seed_verse_row(&harness.db(), VERSE_ID));

    assert!(matches!(
        harness.enrol("enrol-1"),
        DbResult::RequestFailed { ref correlation_id, .. } if correlation_id == "enrol-1"
    ));
    let view = rt
        .block_on(DurableDeviceEnrolmentView::load(&harness.db()))
        .expect("load view");
    assert!(view.registry().is_empty(), "nothing was stored");
    assert!(harness.repl_rx.try_recv().is_err(), "nothing was published");
}

#[test]
fn a_peer_enrolment_is_admitted_only_from_its_own_principal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, None);
    let db = harness.db();
    let rt = runtime();
    rt.block_on(async {
        // The manifest has converged, so the bootstrap window is closed for this verse.
        seed_verse_row(&db, VERSE_ID).await;
        let scope = fe_database::build_scope(VERSE_ID, None, None);
        for (seed, role) in [(0x31, "editor"), (0x32, "manager"), (0x34, "viewer")] {
            fe_database::rbac::assign_role(&db, &principal(seed).did, &scope, role)
                .await
                .expect("seed role");
        }
    });

    // Principal 0x33's body, signed by 0x32's key under the enrolment domain.
    let claimed = DeviceEnrolmentV1 {
        principal: principal(0x33),
        device_key: device(0x0d).public_key(),
        enrolled_at_ms: 1_000,
    };
    let forged = SignedDeviceEnrolment {
        body: claimed.clone(),
        signature: sign_domain(
            &signing_key(0x32),
            DEVICE_ENROLMENT_DOMAIN,
            &claimed.encode_canonical().expect("body"),
        ),
    }
    .encode_canonical()
    .expect("forged bytes");
    let forged_row = EnrolmentRow {
        record_id: op_id_to_hex(enrolment_record_id(&claimed.principal, claimed.device_key)),
        principal_did: claimed.principal.did.clone(),
        device_key_hex: hex::encode(claimed.device_key.0),
        artifact_cbor: BASE64.encode(&forged),
        revoked: false,
    };

    let cases: [(&str, &str, EnrolmentRow, u8, ReplicatedRowOutcome); 5] = [
        (
            "the principal publishes its own signed enrolment",
            VERSE_ID,
            row_for(&enrolment_bytes(0x31, 0x0d, 1_000)),
            0x31,
            ReplicatedRowOutcome::Applied,
        ),
        (
            "a Manager relays another principal's valid enrolment",
            VERSE_ID,
            row_for(&enrolment_bytes(0x31, 0x0e, 1_000)),
            0x32,
            ReplicatedRowOutcome::Denied,
        ),
        (
            "the named principal authors bytes another key signed",
            VERSE_ID,
            forged_row,
            0x33,
            ReplicatedRowOutcome::Denied,
        ),
        (
            "a Viewer enrols its own device",
            VERSE_ID,
            row_for(&enrolment_bytes(0x34, 0x0d, 1_000)),
            0x34,
            ReplicatedRowOutcome::Applied,
        ),
        (
            "a relayed enrolment before the verse manifest converges",
            "verse-not-yet-converged",
            row_for(&enrolment_bytes(0x35, 0x0d, 1_000)),
            0x32,
            ReplicatedRowOutcome::Denied,
        ),
    ];
    for (case, verse_id, row, author_seed, expected) in cases {
        let outcome = rt.block_on(apply_inbound(
            &db,
            verse_id,
            &row,
            &principal(author_seed).did,
        ));
        assert_eq!(outcome, expected, "{case}");
        let stored = rt.block_on(stored_row(&db, &row.record_id));
        assert_eq!(
            stored.is_some(),
            expected == ReplicatedRowOutcome::Applied,
            "{case}: stored iff applied"
        );
    }

    // The unsigned `del` marker can never revoke an enrolment.
    let held = row_for(&enrolment_bytes(0x31, 0x0d, 1_000));
    let outcome = rt.block_on(apply_replicated_row_handler(
        &db,
        VERSE_ID,
        DEVICE_ENROLMENT_TABLE,
        &held.record_id,
        &[],
        &principal(0x32).did,
        None,
    ));
    assert_eq!(outcome.expect("apply"), ReplicatedRowOutcome::NotApplicable);
    assert_eq!(rt.block_on(stored_row(&db, &held.record_id)), Some(held));
}

/// Security review (critic C8(iv)): once the manifest has converged, a principal the verse
/// resolves to role None (removed, or unknown under `default_access = none`) can no longer plant
/// enrolment rows, but its revocations always land and pre-manifest enrolments are not lost.
#[test]
fn a_principal_without_a_role_cannot_enrol_but_can_always_revoke() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, None);
    let db = harness.db();
    let rt = runtime();
    const REMOVED: u8 = 0x36;
    const DEFAULT_VIEWER: u8 = 0x37;
    const EARLY: u8 = 0x38;
    rt.block_on(async {
        seed_verse_row(&db, VERSE_ID).await;
        let scope = fe_database::build_scope(VERSE_ID, None, None);
        fe_database::rbac::assign_role(&db, &principal(REMOVED).did, &scope, "none")
            .await
            .expect("seed removed role");
    });

    let floored = row_for(&enrolment_bytes(REMOVED, 0x0d, 1_000));
    let revocation = row_for(&revocation_bytes(REMOVED, 0x0e, 1_000));
    let default_viewer = row_for(&enrolment_bytes(DEFAULT_VIEWER, 0x0d, 1_000));
    let pre_manifest = row_for(&enrolment_bytes(EARLY, 0x0d, 1_000));
    let outcomes = rt.block_on(async {
        [
            apply_inbound(&db, VERSE_ID, &floored, &principal(REMOVED).did).await,
            apply_inbound(&db, VERSE_ID, &revocation, &principal(REMOVED).did).await,
            apply_inbound(
                &db,
                VERSE_ID,
                &default_viewer,
                &principal(DEFAULT_VIEWER).did,
            )
            .await,
            apply_inbound(
                &db,
                "verse-not-yet-converged",
                &pre_manifest,
                &principal(EARLY).did,
            )
            .await,
        ]
    });
    assert_eq!(
        outcomes,
        [
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: principal(REMOVED).did
            }),
            ReplicatedRowOutcome::AppliedTombstone,
            ReplicatedRowOutcome::Applied,
            ReplicatedRowOutcome::Deferred(DeferralReason::Manifest),
        ],
        "role-None enrolment deferred until a role lands (AC-44a); its revocation and a \
         default-access Viewer's enrolment land; a pre-manifest enrolment waits for the \
         manifest (AC-44b)"
    );
    assert_eq!(rt.block_on(stored_row(&db, &floored.record_id)), None);
    assert_eq!(
        rt.block_on(stored_row(&db, &revocation.record_id)),
        Some(revocation)
    );
    assert_eq!(rt.block_on(stored_row(&db, &pre_manifest.record_id)), None);
}

/// AC-44 / T27 (critic tick 8): the W4-8 enrolment branch never needs Editor — a Viewer's
/// self-signed enrolment applies with no deferral — while a principal with no membership
/// anywhere in the verse is `Deferred AuthorRole` and applies (with no re-delivery) once ANY
/// role lands for them, even a petal-scoped one that resolves to `None` at the verse scope;
/// and a pre-manifest enrolment is `Deferred Manifest` and applies once the manifest lands.
#[test]
fn a_viewers_enrolment_never_needs_editor_and_a_pre_membership_enrolment_is_deferred_then_applied()
{
    const OWNER: &str = "did:key:z6MkEnrolmentOwner";
    const CLOSED_VERSE: &str = "verse-closed";
    const LATE_VERSE: &str = "verse-late";
    const VIEWER: u8 = 0x41;
    const OUTSIDER: u8 = 0x42;
    const EARLY: u8 = 0x43;
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, None);
    let db = harness.db();
    let rt = runtime();
    let mut admissions = InboundAdmissions::new();
    let manifest = |verse: &str, default_access: &str| {
        serde_json::to_vec(&serde_json::json!({
            "verse_id": verse, "name": "Closed", "created_by": OWNER,
            "created_at": "2026-10-15T00:00:00Z", "default_access": default_access,
        }))
        .expect("manifest json")
    };
    let owner_grant = |verse: &str, scope: &str, peer: &str, role: &str, hlc: u64| {
        let payload = RoleStatementPayload {
            scope: scope.to_string(),
            peer_did: peer.to_string(),
            grantor_did: OWNER.to_string(),
            role: role.to_string(),
            revoked: false,
            hlc,
        };
        (
            verse.to_string(),
            format!("{scope}/{peer}/{OWNER}"),
            serde_json::to_vec(&payload).expect("payload json"),
        )
    };

    rt.block_on(async {
        // A verse where nobody is a member by default.
        let reports = admissions
            .apply(
                &db,
                CLOSED_VERSE,
                "verse",
                CLOSED_VERSE,
                &manifest(CLOSED_VERSE, "none"),
                OWNER,
                None,
            )
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);

        // A verse-level Viewer enrols: no Editor needed, never deferred.
        let verse_scope = fe_database::build_scope(CLOSED_VERSE, None, None);
        let (verse, key, bytes) = owner_grant(
            CLOSED_VERSE,
            &verse_scope,
            &principal(VIEWER).did,
            "viewer",
            1,
        );
        let reports = admissions
            .apply(&db, &verse, "role", &key, &bytes, OWNER, None)
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
        let viewer_row = row_for(&enrolment_bytes(VIEWER, 0x0d, 1_000));
        let reports = admissions
            .apply(
                &db,
                CLOSED_VERSE,
                DEVICE_ENROLMENT_TABLE,
                &viewer_row.record_id,
                &row_json(&viewer_row),
                &principal(VIEWER).did,
                None,
            )
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(
            stored_row(&db, &viewer_row.record_id).await,
            Some(viewer_row)
        );

        // A principal with no membership anywhere: deferred, not denied, not stored.
        let outsider_row = row_for(&enrolment_bytes(OUTSIDER, 0x0d, 1_000));
        let reports = admissions
            .apply(
                &db,
                CLOSED_VERSE,
                DEVICE_ENROLMENT_TABLE,
                &outsider_row.record_id,
                &row_json(&outsider_row),
                &principal(OUTSIDER).did,
                None,
            )
            .await;
        assert_eq!(
            reports[0].outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: principal(OUTSIDER).did
            })
        );
        assert_eq!(stored_row(&db, &outsider_row.record_id).await, None);

        // The owner grants the outsider a PETAL-scoped role: still `None` at the verse scope,
        // but a member — the queued enrolment applies with no second delivery.
        let petal_scope =
            fe_database::build_scope(CLOSED_VERSE, Some("fractal-x"), Some("petal-x"));
        let (verse, key, bytes) = owner_grant(
            CLOSED_VERSE,
            &petal_scope,
            &principal(OUTSIDER).did,
            "viewer",
            2,
        );
        let reports = admissions
            .apply(&db, &verse, "role", &key, &bytes, OWNER, None)
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
        assert!(
            reports.iter().any(|r| r.record_id == outsider_row.record_id
                && r.outcome == ReplicatedRowOutcome::Applied),
            "the deferred enrolment is retried and applied: {reports:?}"
        );
        assert_eq!(
            stored_row(&db, &outsider_row.record_id).await,
            Some(outsider_row)
        );

        // Pre-manifest: deferred on the manifest, applied when it lands.
        let early_row = row_for(&enrolment_bytes(EARLY, 0x0d, 1_000));
        let reports = admissions
            .apply(
                &db,
                LATE_VERSE,
                DEVICE_ENROLMENT_TABLE,
                &early_row.record_id,
                &row_json(&early_row),
                &principal(EARLY).did,
                None,
            )
            .await;
        assert_eq!(
            reports[0].outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::Manifest)
        );
        assert_eq!(stored_row(&db, &early_row.record_id).await, None);
        let reports = admissions
            .apply(
                &db,
                LATE_VERSE,
                "verse",
                LATE_VERSE,
                &manifest(LATE_VERSE, "viewer"),
                OWNER,
                None,
            )
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
        assert!(
            reports.iter().any(|r| r.record_id == early_row.record_id
                && r.outcome == ReplicatedRowOutcome::Applied),
            "the pre-manifest enrolment applies once the manifest lands: {reports:?}"
        );
        assert_eq!(stored_row(&db, &early_row.record_id).await, Some(early_row));
        assert!(admissions.is_empty(), "nothing is left waiting");
    });
}

#[test]
fn a_revocation_survives_a_late_enrolment_replay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, None);
    let db = harness.db();
    let rt = runtime();
    let principal_did = principal(0x31).did;
    // The stale enrolment even claims a LATER time than the revocation: timestamps never decide.
    let enrolment = row_for(&enrolment_bytes(0x31, 0x0d, 9_000));
    let revocation = row_for(&revocation_bytes(0x31, 0x0d, 5_000));
    assert_eq!(enrolment.record_id, revocation.record_id);
    // The manifest has converged (default access viewer), so the enrolment floor passes (AC-44).
    rt.block_on(seed_verse_row(&db, VERSE_ID));

    let outcomes = rt.block_on(async {
        [
            apply_inbound(&db, VERSE_ID, &enrolment, &principal_did).await,
            apply_inbound(&db, VERSE_ID, &revocation, &principal_did).await,
            apply_inbound(&db, VERSE_ID, &enrolment, &principal_did).await,
        ]
    });
    assert_eq!(
        outcomes,
        [
            ReplicatedRowOutcome::Applied,
            ReplicatedRowOutcome::AppliedTombstone,
            ReplicatedRowOutcome::SkippedTombstoned,
        ]
    );
    assert_eq!(
        rt.block_on(stored_row(&db, &revocation.record_id)),
        Some(revocation)
    );
    let view = rt
        .block_on(DurableDeviceEnrolmentView::load(&db))
        .expect("load view");
    assert_eq!(
        view.device_enrolment(&principal(0x31), device(0x0d).public_key()),
        AuthorizationRecord::Refused
    );
}

// ---------------------------------------------------------------------------
// CDL-R2 delivery acceptance
// ---------------------------------------------------------------------------

fn verse_scope() -> Scope {
    Scope::verse_wide(VERSE)
}

/// A one-certificate chain the Manager issues to `audience` over the verse at [`EPOCH`], valid
/// for an hour around the real clock the durable view reads.
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

async fn seed_canonical_role(db: &Db, seed: u8, role: &str) {
    db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: $role }")
        .bind(("did", principal(seed).did))
        .bind(("scope", interim_authority_scope_key(&verse_scope())))
        .bind(("role", role.to_string()))
        .await
        .expect("seed role")
        .check()
        .expect("seed role check");
}

/// The Manager's signed kind-4 bump, admitted through `admit_epoch_bump`: verse 0 -> [`EPOCH`].
async fn bump_verse_to_epoch_one(db: &Db) {
    let unsigned = UnsignedEnvelope {
        protocol_version: PROTOCOL_VERSION,
        operation_kind: OperationKind::ScopeEpochBump.to_u16(),
        scope: verse_scope(),
        author: Author::from_public_key(signing_key(MANAGER).verifying_key().to_bytes()),
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
}

/// The SPEC-6 §9.3 lane view: one header lane, one current key, one wrappable peer.
struct WrappableLane {
    lane: LaneKey,
    key_id: Identifier32,
    wrappable: PeerIdentity,
}

impl RelayAuthorizationView for WrappableLane {
    fn current_scope_epoch(&self, lane: &LaneKey) -> Option<u64> {
        (*lane == self.lane).then_some(EPOCH)
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
        *lane == self.lane && epoch == EPOCH && *peer == self.wrappable
    }
}

#[test]
fn cdl_r2_delivery_acceptance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), None, None);
    let db = harness.db();
    let rt = runtime();
    let recipient = principal(RECIPIENT);
    let enrolled_device = device(0x0d);
    rt.block_on(async {
        // The manifest has converged (default access viewer): enrolments pass the floor (AC-44).
        seed_verse_row(&db, VERSE_ID).await;
        seed_canonical_role(&db, MANAGER, "manager").await;
        seed_canonical_role(&db, EDITOR, "editor").await;
        bump_verse_to_epoch_one(&db).await;
        ChainStore::load(&db)
            .await
            .expect("chain store")
            .put_verified_chain(&manager_chain(
                RECIPIENT,
                &[Verb::Decrypt],
                &[ObjectClass::Operation],
            ))
            .await
            .expect("put the recipient's decrypt chain");
        // Both principals enrol through the real inbound path; only RECIPIENT holds a chain.
        for (seed, device_seed) in [(RECIPIENT, 0x0d), (STRANGER, 0x0f)] {
            let row = row_for(&enrolment_bytes(seed, device_seed, 1_000));
            assert_eq!(
                apply_inbound(&db, VERSE_ID, &row, &principal(seed).did).await,
                ReplicatedRowOutcome::Applied
            );
        }
    });
    let delivery = rt
        .block_on(DurableDeliveryView::load(&db, Some(ANCHOR)))
        .expect("load the durable delivery view");

    let scope = verse_scope();
    let scope_key = SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH]);
    let lane = LaneKey::Header { verse_id: VERSE };
    let lane_view = |peer: &Principal| WrappableLane {
        lane,
        key_id: derive_key_id(&scope, EPOCH, &scope_key).expect("key id"),
        wrappable: PeerIdentity(peer.public_key),
    };
    let manager = principal(MANAGER);
    let capability = CapabilityRef {
        chain_id: Hash32([0x7c; 32]),
        scope_epoch: EPOCH,
    };

    // Enrolled device + Manager+ issuer + recipient decrypt chain: the wrap opens on the device.
    let binding = RecipientDeviceBinding::resolve(
        &delivery,
        &recipient,
        enrolled_device.public_key(),
        &scope,
        EPOCH,
    )
    .expect("the durable registry enrols this device");
    let request = ScopeKeyWrapRequest {
        recipient_device_binding: &binding,
        scope_key: &scope_key,
        issuer: &manager,
        issuer_capability: capability,
    };
    let wrap = issue_scope_key_wrap(
        &lane_view(&recipient),
        &delivery,
        &lane,
        &signing_key(MANAGER),
        &request,
    )
    .expect("an enrolled, authorized recipient receives a wrap");
    let received = ScopeKeyWrap::decode_canonical(&wrap.encode_canonical().expect("wrap bytes"))
        .expect("wrap decodes");
    assert_eq!(
        open_scope_key(&received, &enrolled_device, &delivery).expect("the device opens it"),
        scope_key
    );

    // A non-Manager issuer is refused before any key material is touched.
    let editor = principal(EDITOR);
    assert_eq!(
        issue_scope_key_wrap(
            &lane_view(&recipient),
            &delivery,
            &lane,
            &signing_key(EDITOR),
            &ScopeKeyWrapRequest {
                issuer: &editor,
                ..request.clone()
            },
        )
        .err(),
        Some(CryptoError::IssuerNotAuthorized {
            record: AuthorizationRecord::Refused
        })
    );

    // An unenrolled device cannot even be named in a wrap request.
    assert_eq!(
        RecipientDeviceBinding::resolve(
            &delivery,
            &recipient,
            device(0x0e).public_key(),
            &scope,
            EPOCH
        ),
        Err(CryptoError::DeviceNotEnrolled {
            record: AuthorizationRecord::Unknown
        })
    );

    // An enrolled principal holding no decrypt chain receives nothing.
    let stranger = principal(STRANGER);
    let stranger_binding = RecipientDeviceBinding::resolve(
        &delivery,
        &stranger,
        device(0x0f).public_key(),
        &scope,
        EPOCH,
    )
    .expect("the stranger's device is enrolled");
    assert_eq!(
        issue_scope_key_wrap(
            &lane_view(&stranger),
            &delivery,
            &lane,
            &signing_key(MANAGER),
            &ScopeKeyWrapRequest {
                recipient_device_binding: &stranger_binding,
                ..request.clone()
            },
        )
        .err(),
        Some(CryptoError::RecipientNotAuthorized {
            record: AuthorizationRecord::Unknown
        })
    );
}
