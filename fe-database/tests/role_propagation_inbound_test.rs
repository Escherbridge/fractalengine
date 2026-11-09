//! role_propagation RP-3 flows: inbound role statements and deferred admission, driven
//! through the ONE inbound entry point both dispatch loops use (`InboundAdmissions::apply`)
//! over the real schema — validation (T1/T6), durable held statements and their promotion
//! (R7, T22), delivery-order convergence (T4, T22), flood resistance (T12), replay (T2), and
//! the in-process retry of deferred content rows. See fe-database/src/AGENTS.md
//! §inbound-admissions and §role-propagation.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::Duration;

use fe_database::handlers::extension_policy;
use fe_database::handlers::inbound_admissions::{
    deferred_admission_drop_count, InboundAdmissions, RowReport,
};
use fe_database::repo::Db;
use fe_database::role_grants::{role_pending_refused_count, ROLE_PENDING_PER_GRANTOR};
use fe_database::role_manager::{assign_role_checked, resolve_role};
use fe_database::{build_scope, spawn_db_thread_shared, BlobStoreHandle, DbThread, RoleLevel};
use fe_policy::RoleStatementPayload;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult, DeferralReason, ReplicatedRowOutcome};

const OWNER: &str = "did:key:z6MkInboundOwner";
const VERSE: &str = "verse-rp3";
const FRACTAL: &str = "fractal-rp3";
const PETAL_A: &str = "petal-a";
const PETAL_B: &str = "petal-b";

static HLC: Once = Once::new();
static SERIAL: Mutex<()> = Mutex::new(());

/// Tests that start a DB thread (which re-initialises the process-global HLC) or issue local
/// statements (which read it) run one at a time.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime")
}

fn verse_scope() -> String {
    build_scope(VERSE, None, None)
}

fn fractal_scope() -> String {
    build_scope(VERSE, Some(FRACTAL), None)
}

fn petal_scope(petal: &str) -> String {
    build_scope(VERSE, Some(FRACTAL), Some(petal))
}

async fn query(db: &Db, sql: &str) {
    db.query(sql)
        .await
        .expect("query")
        .check()
        .expect("query check");
}

/// An in-memory store with the real schema and NO hierarchy rows (pre-manifest).
async fn empty_db() -> Db {
    let db: Db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("mem db");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("schema");
    db
}

/// A verse owned by [`OWNER`] (default access `viewer`) with one fractal and two petals.
async fn verse_db() -> Db {
    let db = empty_db().await;
    query(
        &db,
        &format!(
            "CREATE verse CONTENT {{ verse_id: '{VERSE}', name: 'V', created_by: '{OWNER}', \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' }}; \
             CREATE fractal CONTENT {{ fractal_id: '{FRACTAL}', verse_id: '{VERSE}', \
             owner_did: '{OWNER}', name: 'F', created_at: '2026-10-15T00:00:00Z' }};"
        ),
    )
    .await;
    for petal in [PETAL_A, PETAL_B] {
        query(
            &db,
            &format!(
                "CREATE petal CONTENT {{ petal_id: '{petal}', fractal_id: '{FRACTAL}', \
                 name: 'P', node_id: '{OWNER}', created_at: '2026-10-15T00:00:00Z' }}"
            ),
        )
        .await;
    }
    db
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

/// A replicated `extension_policy` row (docs/extensions/governance.md §Policy row) by `author`.
fn policy_row(author: &str, written_at: u64) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema_version": extension_policy::POLICY_SCHEMA_VERSION,
        "verse_id": VERSE,
        "author_did": author,
        "written_at": written_at,
        "policy": {
            "mode": "allow-list",
            "allow": [{ "package_id": "org.example.fleet-sim", "versions": ">=0.1.0, <0.2.0" }],
        },
    }))
    .expect("policy json")
}

fn manifest_row() -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "verse_id": VERSE, "name": "V", "created_by": OWNER,
        "created_at": "2026-10-15T00:00:00Z", "default_access": "viewer",
    }))
    .expect("manifest json")
}

fn petal_row(petal: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "petal_id": petal, "fractal_id": FRACTAL, "name": "Late petal",
        "node_id": OWNER, "created_at": "2026-10-15T00:00:00Z",
    }))
    .expect("petal json")
}

fn node_row(node_id: &str, petal: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "node_id": node_id,
        "petal_id": petal,
        "display_name": "Deferred node",
        "position": { "type": "Point", "coordinates": [1.0, 2.0] },
        "rotation": [0.0, 0.0, 0.0, 1.0],
        "scale": [1.0, 1.0, 1.0],
        "created_at": "2026-10-15T00:00:00Z",
    }))
    .expect("node json")
}

/// [`node_row`] as the complete versioned snapshot a current publisher sends (QA-0131:
/// only a stamped row can show its author held Editor when it was written).
fn complete_node_row(node_id: &str, petal: &str, publisher: &str, hlc: u64) -> Vec<u8> {
    let mut row: serde_json::Value = serde_json::from_slice(&node_row(node_id, petal)).expect("node");
    row["elevation"] = serde_json::json!(0.0);
    row["interactive"] = serde_json::json!(true);
    row["edit_seq"] = serde_json::json!(0);
    row["_fe_scene"] = serde_json::json!({
        "version": 1, "complete": true, "hlc": hlc.to_string(), "publisher": publisher,
    });
    serde_json::to_vec(&row).expect("node json")
}

/// One role statement as its grantor publishes it: `(record_id, payload bytes)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Statement {
    grantor: String,
    record_id: String,
    bytes: Vec<u8>,
}

fn statement(
    grantor: &str,
    peer: &str,
    scope: &str,
    role: &str,
    revoked: bool,
    hlc: u64,
) -> Statement {
    let payload = RoleStatementPayload {
        scope: scope.to_string(),
        peer_did: peer.to_string(),
        grantor_did: grantor.to_string(),
        role: role.to_string(),
        revoked,
        hlc,
        editor_since: None,
        manager_since: None,
    };
    Statement {
        grantor: grantor.to_string(),
        record_id: format!("{scope}/{peer}/{grantor}"),
        bytes: serde_json::to_vec(&payload).expect("payload json"),
    }
}

fn grant(grantor: &str, peer: &str, scope: &str, role: &str, hlc: u64) -> Statement {
    statement(grantor, peer, scope, role, false, hlc)
}

fn revoke(grantor: &str, peer: &str, scope: &str, hlc: u64) -> Statement {
    statement(grantor, peer, scope, "none", true, hlc)
}

/// Deliver one statement over the inbound path (its attested author is its grantor).
async fn deliver(db: &Db, queue: &mut InboundAdmissions, stmt: &Statement) -> Vec<RowReport> {
    queue
        .apply(
            db,
            VERSE,
            "role",
            &stmt.record_id,
            &stmt.bytes,
            &stmt.grantor,
            None,
        )
        .await
}

fn author_role(did: &str) -> ReplicatedRowOutcome {
    ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
        did: did.to_string(),
    })
}

/// `(statement_key, hlc)` of every row in one statement table.
async fn statement_keys(db: &Db, table: &str) -> BTreeSet<(String, String)> {
    let mut res = db
        .query(format!("SELECT statement_key, hlc FROM {table}"))
        .await
        .expect("select statements");
    let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
    rows.iter()
        .map(|row| {
            (
                row["statement_key"].as_str().expect("key").to_string(),
                row["hlc"].as_str().expect("hlc").to_string(),
            )
        })
        .collect()
}

async fn admitted(db: &Db) -> BTreeSet<(String, String)> {
    statement_keys(db, "role_grant").await
}

async fn held(db: &Db) -> BTreeSet<(String, String)> {
    statement_keys(db, "role_statement_pending").await
}

fn keys(set: &BTreeSet<(String, String)>) -> BTreeSet<String> {
    set.iter().map(|(key, _)| key.clone()).collect()
}

/// The materialized `role` cache as `(scope, peer, role)`.
async fn role_cache(db: &Db) -> BTreeSet<(String, String, String)> {
    let mut res = db
        .query("SELECT scope, peer_did, role FROM role")
        .await
        .expect("select role cache");
    let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
    rows.iter()
        .map(|row| {
            (
                row["scope"].as_str().expect("scope").to_string(),
                row["peer_did"].as_str().expect("peer").to_string(),
                row["role"].as_str().expect("role").to_string(),
            )
        })
        .collect()
}

async fn role_of(db: &Db, peer: &str, scope: &str) -> RoleLevel {
    resolve_role(db, peer, scope).await.expect("resolve_role")
}

async fn node_exists(db: &Db, node_id: &str) -> bool {
    let mut res = db
        .query("SELECT node_id FROM node WHERE node_id = $n")
        .bind(("n", node_id.to_string()))
        .await
        .expect("select node");
    let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
    !rows.is_empty()
}

fn applied(reports: &[RowReport], record_id: &str) -> bool {
    reports
        .iter()
        .any(|r| r.record_id == record_id && r.outcome == ReplicatedRowOutcome::Applied)
}

/// The plan's acceptance flow: an Editor's node row that arrives BEFORE the owner's grant is
/// Deferred (not persisted); the owner's statement then lands and the node applies in the same
/// step — no second delivery, no snapshot replay (replaces the manual-replay test).
#[tokio::test]
async fn an_owner_role_statement_replicated_in_lets_the_editors_deferred_node_row_apply_without_replay(
) {
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    const EDITOR: &str = "did:key:z6MkEditor";

    // Stamped after the grant below (hlc 10): written once the editor held the role.
    let reports = queue
        .apply(
            &db,
            VERSE,
            "node",
            "node-early",
            &complete_node_row("node-early", PETAL_A, EDITOR, 1 << 16),
            EDITOR,
            None,
        )
        .await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, author_role(EDITOR));
    assert!(
        !node_exists(&db, "node-early").await,
        "a deferred row is not persisted"
    );
    assert!(queue
        .waiting_reason(VERSE, "node", "node-early", EDITOR)
        .is_some());

    let owner_grant = grant(OWNER, EDITOR, &verse_scope(), "editor", 10);
    let reports = deliver(&db, &mut queue, &owner_grant).await;
    assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
    assert!(
        applied(&reports, "node-early"),
        "the deferred node is retried in the same step: {reports:?}"
    );
    assert!(node_exists(&db, "node-early").await, "the node is durable");
    assert!(queue.is_empty());
}

#[derive(Debug, Clone, Copy)]
enum Promotion {
    InboundOwnerStatement,
    LocalAssignRole,
}

/// R7: a statement whose grantor is not yet Manager+ is Deferred and HELD (never stored
/// inert, never effective); when the grantor is promoted — by an inbound owner statement or
/// by the owner's local AssignRole — it is admitted with no second delivery.
#[test]
fn a_not_yet_managers_statement_is_deferred_then_admitted_on_promotion() {
    let _serial = serial();
    HLC.call_once(|| fe_database::op_log::init_hlc(0));
    runtime().block_on(promotion_flows());
}

async fn promotion_flows() {
    const MANAGER: &str = "did:key:z6MkFutureManager";
    const PEER: &str = "did:key:z6MkGrantee";
    for promotion in [Promotion::InboundOwnerStatement, Promotion::LocalAssignRole] {
        let db = verse_db().await;
        let mut queue = InboundAdmissions::new();
        let early = grant(MANAGER, PEER, &petal_scope(PETAL_A), "editor", 5);

        let reports = deliver(&db, &mut queue, &early).await;
        assert_eq!(reports[0].outcome, author_role(MANAGER), "{promotion:?}");
        assert!(
            keys(&held(&db).await).contains(&early.record_id),
            "{promotion:?}"
        );
        assert!(!keys(&admitted(&db).await).contains(&early.record_id));
        assert_eq!(
            role_of(&db, PEER, &petal_scope(PETAL_A)).await,
            RoleLevel::Viewer,
            "{promotion:?}: a held statement has no effect (the grantee keeps default access)"
        );
        assert!(
            queue.is_empty(),
            "role statements never enter the content queue"
        );

        // E10 (R18): the same not-yet-Manager's extension policy row, written after its promotion on its
        // own peer, arrives first: deferred on the author's role, never stored.
        extension_policy::apply_schema(&db).await.expect("policy table");
        let policy_written_at = (wall_ms() + 60_000) << 16;
        let reports = queue
            .apply(&db, VERSE, extension_policy::POLICY_TABLE, VERSE, &policy_row(MANAGER, policy_written_at), MANAGER, None)
            .await;
        assert_eq!(reports[0].outcome, author_role(MANAGER), "{promotion:?}");
        assert!(extension_policy::get_policy(&db, VERSE).await.expect("read policy").is_none());

        let settled = match promotion {
            Promotion::InboundOwnerStatement => {
                let promote = grant(OWNER, MANAGER, &verse_scope(), "manager", 20);
                let reports = deliver(&db, &mut queue, &promote).await;
                assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
                reports
            }
            Promotion::LocalAssignRole => {
                let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
                assign_role_checked(&db, OWNER, MANAGER, &verse_scope(), "manager", None, &store)
                    .await
                    .expect("owner assigns locally");
                queue.retry_author_roles(&db, VERSE, None).await
            }
        };
        assert!(
            applied(&settled, VERSE),
            "{promotion:?}: the deferred policy row applies on promotion: {settled:?}"
        );
        let stored = extension_policy::get_policy(&db, VERSE).await.expect("read policy").expect("policy row");
        assert_eq!((stored.author_did.as_str(), stored.policy.mode), (MANAGER, extension_policy::PolicyMode::AllowList));

        // Precondition 7 over the stored row: the allow range admits 0.1.x only, a local digest deny matches
        // under any id, and a URL whose authority ends at `?` is not loopback (G2: `?@localhost` disguise).
        fn build<'a>(
            version: &'a str,
            component_digest: &'a str,
            manifest: &'a fe_sdk::manifest_v2::ExtensionManifest,
        ) -> extension_policy::PolicyFacts<'a> {
            extension_policy::PolicyFacts {
                package_id: "org.example.fleet-sim",
                version,
                content_hash: "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12",
                component_digest: Some(component_digest),
                plaintext_digest: None,
                trust: extension_policy::TrustTier::Publisher,
                kind: fe_sdk::manifest_v2::ExtensionKind::BiConnector,
                manifest: Some(manifest),
            }
        }
        let mut bi_push: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../fe-sdk/schema/examples/bi-push.extension.json")).expect("example");
        bi_push["egress"][0]["url"] = serde_json::json!("https://evil.example?@localhost/ingest/");
        let disguised: fe_sdk::manifest_v2::ExtensionManifest = serde_json::from_value(bi_push.clone()).expect("manifest");
        bi_push["egress"][0]["url"] = serde_json::json!("http://localhost:9000/ingest/");
        let loopback: fe_sdk::manifest_v2::ExtensionManifest = serde_json::from_value(bi_push).expect("manifest");
        let digest = "c".repeat(64);
        let host_policy = extension_policy::LocalPolicy {
            deny: Vec::new(),
            ceilings: extension_policy::Ceilings {
                egress: Some(extension_policy::NetCeiling::Loopback),
                ..Default::default()
            },
        };
        let deny_digest = extension_policy::LocalPolicy { deny: vec![digest.clone()], ..host_policy.clone() };
        let verse_policy = Some(&stored.policy);
        assert!(extension_policy::evaluate(verse_policy, &host_policy, &build("0.1.3", &digest, &loopback)).is_ok());
        for (facts, local, reason) in [
            (build("0.2.0", &digest, &loopback), &host_policy, extension_policy::DenialReason::NotAllowListed),
            (build("0.1.3", &digest, &disguised), &host_policy, extension_policy::DenialReason::Ceiling("egress")),
            (build("0.1.3", &digest, &loopback), &deny_digest, extension_policy::DenialReason::DenyListed(digest.clone())),
        ] {
            assert_eq!(extension_policy::evaluate(verse_policy, local, &facts).unwrap_err().reason, reason);
        }

        // The local write path: the node (here the owner) writes as itself, log-first, one audit op per
        // action; a node below Manager is refused and that refusal is audited too.
        let local = extension_policy::ExtensionPolicy {
            mode: extension_policy::PolicyMode::TrustRoot,
            ..Default::default()
        };
        let written = extension_policy::set_policy_with_replication(&db, OWNER, VERSE, local.clone(), None, None)
            .await
            .expect("owner writes the policy");
        assert!(written.written_at > policy_written_at, "{promotion:?}: the local write orders after the row it replaces");
        assert!(matches!(
            extension_policy::set_policy_with_replication(&db, PEER, VERSE, local, None, None).await,
            Err(extension_policy::PolicyWriteError::RoleDenied(_))
        ));
        let trail = extension_policy::list_audit(&db, extension_policy::AuditFilter::Verse(VERSE), 10)
            .await
            .expect("audit trail");
        let outcomes: Vec<(&str, &str)> = trail
            .iter()
            .map(|entry| (entry["actor_did"].as_str().unwrap(), entry["outcome"].as_str().unwrap()))
            .collect();
        assert_eq!(outcomes, vec![(PEER, "refused"), (OWNER, "ok")], "{promotion:?}: newest first");

        assert!(
            keys(&admitted(&db).await).contains(&early.record_id),
            "{promotion:?}: promoted into role_grant"
        );
        assert!(
            held(&db).await.is_empty(),
            "{promotion:?}: pending is empty"
        );
        assert_eq!(
            role_of(&db, PEER, &petal_scope(PETAL_A)).await,
            RoleLevel::Editor,
            "{promotion:?}: the grantee's role materializes"
        );
    }
}

/// Every permutation of `items`, in lexicographic index order.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    fn extend(prefix: &mut Vec<usize>, used: &mut [bool], out: &mut Vec<Vec<usize>>) {
        if prefix.len() == used.len() {
            out.push(prefix.clone());
            return;
        }
        for i in 0..used.len() {
            if !used[i] {
                used[i] = true;
                prefix.push(i);
                extend(prefix, used, out);
                prefix.pop();
                used[i] = false;
            }
        }
    }
    let mut out = Vec::new();
    extend(&mut Vec::new(), &mut vec![false; n], &mut out);
    out
}

/// Clears every role-statement table and the cache: the state of a fresh store for this test
/// (the verse and hierarchy rows are identical across orders, so only these tables differ).
async fn reset_roles(db: &Db) {
    query(
        db,
        "DELETE role_grant; DELETE role_statement_pending; DELETE role;",
    )
    .await;
}

/// R7 reordering convergence (T22): six statements — a verse Manager appoints a petal
/// Manager, who grants in their petal and (ineffectively) in a sibling; the owner revokes the
/// first Manager (cascade) and re-appoints the petal Manager directly — in ALL 720 delivery
/// orders converge to identical role caches and identical admitted/held partitions.
#[tokio::test]
async fn role_statement_delivery_order_never_changes_the_final_role_set() {
    const M1: &str = "did:key:z6MkM1";
    const M2: &str = "did:key:z6MkM2";
    const X: &str = "did:key:z6MkX";
    const Y: &str = "did:key:z6MkY";
    let statements = [
        grant(OWNER, M1, &verse_scope(), "manager", 1),
        grant(M1, M2, &petal_scope(PETAL_A), "manager", 2),
        grant(M2, X, &petal_scope(PETAL_A), "editor", 3),
        grant(M2, Y, &petal_scope(PETAL_B), "editor", 4),
        revoke(OWNER, M1, &verse_scope(), 5),
        grant(OWNER, M2, &petal_scope(PETAL_A), "manager", 6),
    ];
    let db = verse_db().await;
    let mut expected = None;
    for order in permutations(statements.len()) {
        reset_roles(&db).await;
        let mut queue = InboundAdmissions::new();
        for &i in &order {
            let reports = deliver(&db, &mut queue, &statements[i]).await;
            assert_ne!(
                reports[0].outcome,
                ReplicatedRowOutcome::Denied,
                "order {order:?}: every statement is structurally valid"
            );
        }
        let state = (admitted(&db).await, held(&db).await, role_cache(&db).await);
        match &expected {
            None => expected = Some(state),
            Some(first) => assert_eq!(&state, first, "order {order:?} diverged"),
        }
    }

    let (admitted_set, held_set, _) = expected.expect("at least one order");
    let record = |i: usize| statements[i].record_id.clone();
    assert_eq!(
        keys(&admitted_set),
        BTreeSet::from([record(4), record(5), record(2)]),
        "the owner's revocation (newest for its key), its direct appointment, and M2's grant"
    );
    assert!(
        admitted_set.contains(&(record(4), "5".to_string())),
        "the revocation outranks the older grant on the shared key"
    );
    assert_eq!(
        keys(&held_set),
        BTreeSet::from([record(1), record(3)]),
        "the revoked Manager's appointment and the sibling-petal grant stay held"
    );
    assert_eq!(
        role_of(&db, X, &petal_scope(PETAL_A)).await,
        RoleLevel::Editor
    );
    assert_eq!(
        role_of(&db, Y, &petal_scope(PETAL_B)).await,
        RoleLevel::Viewer
    );
    assert_eq!(role_of(&db, M1, &verse_scope()).await, RoleLevel::Viewer);
    assert_eq!(
        role_of(&db, M2, &petal_scope(PETAL_A)).await,
        RoleLevel::Manager
    );
}

/// A DB thread over a store directory the test owns (restart = drop AND join, C8).
struct Node {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    db: Option<Db>,
    join: Option<std::thread::JoinHandle<()>>,
}

const TIMEOUT: Duration = Duration::from_secs(20);

impl Node {
    fn open(dir: &Path) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let DbThread {
            join_handle,
            shared_db,
        } = spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            Arc::new(MockBlobStore::new()),
            None,
            None,
            None,
            None,
            Some(dir.join("fe.db").to_string_lossy().to_string()),
            None,
        )
        .expect("spawn db thread");
        let started = res_rx.recv_timeout(TIMEOUT).expect("first DB result");
        assert!(matches!(started, DbResult::Started), "got {started:?}");
        Self {
            cmd_tx: Some(cmd_tx),
            res_rx,
            db: Some(shared_db),
            join: Some(join_handle),
        }
    }

    fn db(&self) -> Db {
        self.db.clone().expect("open store")
    }

    /// Deliver one replicated row through the production dispatch arm; returns its outcome.
    fn apply(
        &self,
        table: &str,
        record_id: &str,
        bytes: &[u8],
        author: &str,
    ) -> ReplicatedRowOutcome {
        self.cmd_tx
            .as_ref()
            .expect("open channel")
            .send(DbCommand::ApplyReplicatedRow {
                verse_id: VERSE.to_string(),
                table: table.to_string(),
                record_id: record_id.to_string(),
                row_bytes: bytes.to_vec(),
                author_did: author.to_string(),
            })
            .expect("send");
        loop {
            match self.res_rx.recv_timeout(TIMEOUT).expect("DB reply") {
                DbResult::ReplicatedRowApplied {
                    record_id: got,
                    outcome,
                    ..
                } if got == record_id => return outcome,
                _ => continue,
            }
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        drop(self.db.take());
        drop(self.cmd_tx.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// R7/T22 across a restart: a held statement lives in SurrealKV, survives the DB being
/// dropped and reopened (and the startup repartition), and is admitted when the owner's
/// promotion arrives afterwards — with no re-delivery of the held statement itself.
#[test]
fn held_role_statements_survive_restart() {
    let _serial = serial();
    const MANAGER: &str = "did:key:z6MkRestartManager";
    const PEER: &str = "did:key:z6MkRestartPeer";
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = runtime();
    let held_stmt = grant(MANAGER, PEER, &verse_scope(), "editor", 7);

    {
        let node = Node::open(dir.path());
        assert_eq!(
            node.apply("verse", VERSE, &manifest_row(), OWNER),
            ReplicatedRowOutcome::Applied
        );
        assert_eq!(
            node.apply("role", &held_stmt.record_id, &held_stmt.bytes, MANAGER),
            author_role(MANAGER)
        );
        assert!(keys(&rt.block_on(held(&node.db()))).contains(&held_stmt.record_id));
        // OF-4: NO extra verse_member row here — the replicated verse has no member
        // besides its owner, the exact shape reconcile's `orphaned_creator` used to claim
        // for this node at the next startup.
    }

    // The restart runs reconcile (orphaned_creator included) before any command.
    let node = Node::open(dir.path());
    let db = node.db();
    assert_eq!(
        rt.block_on(fe_database::role_grants::verse_owner(&db, VERSE))
            .expect("owner lookup")
            .as_deref(),
        Some(OWNER)
    );
    assert!(
        keys(&rt.block_on(held(&db))).contains(&held_stmt.record_id),
        "the held statement survived the restart"
    );
    let promote = grant(OWNER, MANAGER, &verse_scope(), "manager", 8);
    assert_eq!(
        node.apply("role", &promote.record_id, &promote.bytes, OWNER),
        ReplicatedRowOutcome::Applied
    );
    assert!(keys(&rt.block_on(admitted(&db))).contains(&held_stmt.record_id));
    assert!(rt.block_on(held(&db)).is_empty());
    assert_eq!(
        rt.block_on(role_of(&db, PEER, &verse_scope())),
        RoleLevel::Editor
    );
}

/// R7/T12: hostile authors flood structurally valid statements (beyond their own per-grantor
/// budget) interleaved with enough deferred content rows to overflow the content queue — yet
/// a legitimately held statement is neither evicted nor starved: the owner's promotion still
/// admits it, no hostile statement is ever admitted, and refusals are charged only to the
/// hostile grantors' own overflow.
#[tokio::test]
async fn a_hostile_flood_cannot_evict_or_starve_a_held_role_statement() {
    const MANAGER: &str = "did:key:z6MkLegitManager";
    const PEER: &str = "did:key:z6MkLegitPeer";
    const HOSTILE_AUTHORS: usize = 8;
    const STATEMENTS_EACH: usize = 1100;
    const CONTENT_ROWS: usize = 5000;
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    let refused_before = role_pending_refused_count();
    let drops_before = deferred_admission_drop_count();

    let legit = grant(MANAGER, PEER, &petal_scope(PETAL_A), "editor", 1);
    assert_eq!(
        deliver(&db, &mut queue, &legit).await[0].outcome,
        author_role(MANAGER)
    );

    let hostile: Vec<String> = (0..HOSTILE_AUTHORS)
        .map(|i| format!("did:key:z6MkHostile{i}"))
        .collect();
    let mut content_sent = 0;
    for j in 0..STATEMENTS_EACH {
        for author in &hostile {
            let stmt = grant(
                author,
                &format!("did:key:z6MkTarget{j}"),
                &verse_scope(),
                "manager",
                j as u64 + 1,
            );
            let outcome = deliver(&db, &mut queue, &stmt).await.remove(0).outcome;
            assert_eq!(
                outcome,
                author_role(author),
                "a hostile statement is never admitted"
            );
        }
        for author in &hostile {
            if content_sent >= CONTENT_ROWS {
                break;
            }
            let id = format!("flood-node-{content_sent}");
            queue
                .apply(
                    &db,
                    VERSE,
                    "node",
                    &id,
                    &node_row(&id, PETAL_A),
                    author,
                    None,
                )
                .await;
            content_sent += 1;
        }
    }
    assert!(
        deferred_admission_drop_count() > drops_before,
        "the content queue overflowed (and counted it)"
    );
    assert_eq!(
        role_pending_refused_count() - refused_before,
        (HOSTILE_AUTHORS * (STATEMENTS_EACH - ROLE_PENDING_PER_GRANTOR)) as u64,
        "only the hostile grantors' own overflow was refused"
    );
    assert!(
        keys(&held(&db).await).contains(&legit.record_id),
        "the legitimate held statement was neither evicted nor refused"
    );

    let promote = grant(OWNER, MANAGER, &verse_scope(), "manager", 2);
    assert_eq!(
        deliver(&db, &mut queue, &promote).await[0].outcome,
        ReplicatedRowOutcome::Applied
    );
    let admitted_keys = keys(&admitted(&db).await);
    assert!(admitted_keys.contains(&legit.record_id));
    assert!(
        admitted_keys
            .iter()
            .all(|key| !hostile.iter().any(|h| key.ends_with(&format!("/{h}")))),
        "no hostile statement is ever admitted"
    );
    assert_eq!(
        role_of(&db, PEER, &petal_scope(PETAL_A)).await,
        RoleLevel::Editor
    );
}

/// T4: the owner revokes a Manager concurrently with that Manager's own grant and the grants
/// of the petal Manager they appointed. Two stores receiving the statements in opposite
/// orders converge on equal role caches and equal partitions — the cascade voids them all.
#[tokio::test]
async fn concurrent_revocation_converges_in_both_delivery_orders() {
    const M: &str = "did:key:z6MkRevokedManager";
    const N: &str = "did:key:z6MkAppointedPetalManager";
    const Z: &str = "did:key:z6MkPetalGrantee";
    const W: &str = "did:key:z6MkVerseGrantee";
    let statements = [
        grant(OWNER, M, &verse_scope(), "manager", 1),
        grant(M, N, &petal_scope(PETAL_A), "manager", 2),
        grant(N, Z, &petal_scope(PETAL_A), "editor", 3),
        grant(M, W, &verse_scope(), "editor", 4),
        revoke(OWNER, M, &verse_scope(), 5),
    ];
    let mut states = Vec::new();
    for order in [[0, 1, 2, 3, 4], [4, 3, 2, 1, 0]] {
        let db = verse_db().await;
        let mut queue = InboundAdmissions::new();
        for i in order {
            deliver(&db, &mut queue, &statements[i]).await;
        }
        for peer in [M, N, Z, W] {
            for scope in [verse_scope(), petal_scope(PETAL_A)] {
                assert!(
                    role_of(&db, peer, &scope).await <= RoleLevel::Viewer,
                    "order {order:?}: {peer} keeps nothing above default access at {scope}"
                );
            }
        }
        states.push((admitted(&db).await, held(&db).await, role_cache(&db).await));
    }
    assert_eq!(states[0], states[1], "both delivery orders converge");
    assert_eq!(
        keys(&states[0].0),
        BTreeSet::from([statements[4].record_id.clone()]),
        "only the owner's revocation stays admitted"
    );
}

/// R7 subtree authority, inbound: a petal Manager's grant applies in their petal and is held
/// (never effective) in a sibling petal, at the enclosing fractal and at the verse (T19).
#[tokio::test]
async fn an_inbound_petal_manager_grant_applies_in_their_petal_and_is_held_in_a_sibling_petal() {
    const PM: &str = "did:key:z6MkPetalManager";
    const E: &str = "did:key:z6MkPetalEditor";
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    let appoint = grant(OWNER, PM, &petal_scope(PETAL_A), "manager", 1);
    assert_eq!(
        deliver(&db, &mut queue, &appoint).await[0].outcome,
        ReplicatedRowOutcome::Applied
    );

    let in_petal = grant(PM, E, &petal_scope(PETAL_A), "editor", 2);
    assert_eq!(
        deliver(&db, &mut queue, &in_petal).await[0].outcome,
        ReplicatedRowOutcome::Applied
    );
    assert_eq!(
        role_of(&db, E, &petal_scope(PETAL_A)).await,
        RoleLevel::Editor
    );

    for (scope, hlc) in [
        (petal_scope(PETAL_B), 3),
        (fractal_scope(), 4),
        (verse_scope(), 5),
    ] {
        let outside = grant(PM, E, &scope, "editor", hlc);
        assert_eq!(
            deliver(&db, &mut queue, &outside).await[0].outcome,
            author_role(PM),
            "{scope}"
        );
        assert!(
            keys(&held(&db).await).contains(&outside.record_id),
            "{scope}"
        );
        assert_eq!(role_of(&db, E, &scope).await, RoleLevel::Viewer, "{scope}");
    }
}

/// Role statements are never deferred on `Parent`: an owner statement for a petal this store
/// has not seen yet is admitted at once and takes effect when the petal row lands — with no
/// re-delivery of the statement.
#[tokio::test]
async fn an_owner_statement_for_a_petal_not_yet_known_takes_effect_when_the_petal_lands() {
    const E: &str = "did:key:z6MkLatePetalEditor";
    const LATE: &str = "petal-late";
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    let stmt = grant(OWNER, E, &petal_scope(LATE), "editor", 1);
    let reports = deliver(&db, &mut queue, &stmt).await;
    assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
    assert!(keys(&admitted(&db).await).contains(&stmt.record_id));
    assert!(queue.is_empty(), "no Parent deferral for role statements");

    let reports = queue
        .apply(&db, VERSE, "petal", LATE, &petal_row(LATE), OWNER, None)
        .await;
    assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
    assert_eq!(
        role_of(&db, E, &petal_scope(LATE)).await,
        RoleLevel::Editor,
        "the grant governs the petal as soon as it exists"
    );
}

/// Before the manifest the owner is unknown and nobody has authority: every statement is
/// held (`Deferred Manifest`). The manifest landing promotes them in authority order; a held
/// statement that targets the owner fails the full re-validation and is deleted as Denied.
#[tokio::test]
async fn role_statements_before_the_manifest_are_held_and_resolve_when_it_lands() {
    const M: &str = "did:key:z6MkEarlyManager";
    const X: &str = "did:key:z6MkEarlyEditor";
    let db = empty_db().await;
    let mut queue = InboundAdmissions::new();
    let appoint = grant(OWNER, M, &verse_scope(), "manager", 1);
    let by_manager = grant(M, X, &petal_scope(PETAL_A), "editor", 2);
    let targets_owner = grant(M, OWNER, &verse_scope(), "viewer", 3);
    for stmt in [&by_manager, &targets_owner, &appoint] {
        assert_eq!(
            deliver(&db, &mut queue, stmt).await[0].outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::Manifest),
            "{}",
            stmt.record_id
        );
    }
    assert_eq!(held(&db).await.len(), 3);
    assert!(admitted(&db).await.is_empty());

    let reports = queue
        .apply(&db, VERSE, "verse", VERSE, &manifest_row(), OWNER, None)
        .await;
    assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
    assert_eq!(
        keys(&admitted(&db).await),
        BTreeSet::from([appoint.record_id.clone(), by_manager.record_id.clone()])
    );
    assert!(
        held(&db).await.is_empty(),
        "the owner-targeting statement was deleted at promotion, not left held"
    );
    assert_eq!(
        role_of(&db, X, &petal_scope(PETAL_A)).await,
        RoleLevel::Editor
    );
    assert_eq!(role_of(&db, OWNER, &verse_scope()).await, RoleLevel::Owner);
}

/// T2: an old genuine grant replayed after the owner's newer revocation of the same key is
/// idempotent — it never undoes the revocation.
#[tokio::test]
async fn a_replayed_older_owner_grant_never_undoes_the_newer_revocation() {
    const E: &str = "did:key:z6MkReplayTarget";
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    let old_grant = grant(OWNER, E, &verse_scope(), "editor", 10);
    let newer_revoke = revoke(OWNER, E, &verse_scope(), 20);
    for stmt in [&old_grant, &newer_revoke, &old_grant] {
        assert_eq!(
            deliver(&db, &mut queue, stmt).await[0].outcome,
            ReplicatedRowOutcome::Applied
        );
    }
    assert_eq!(
        admitted(&db).await,
        BTreeSet::from([(newer_revoke.record_id.clone(), "20".to_string())])
    );
    assert_eq!(role_of(&db, E, &verse_scope()).await, RoleLevel::Viewer);
}

/// T1/T6 (and the rest of the permanent-deny list): structurally invalid statements are
/// Denied and leave no trace in either statement table.
#[tokio::test]
async fn a_forged_grantor_key_and_a_cross_verse_scope_are_denied() {
    const MALLORY: &str = "did:key:z6MkMallory";
    const P: &str = "did:key:z6MkSomePeer";
    let genuine = grant(OWNER, P, &verse_scope(), "editor", 1);
    let mismatched_payload = Statement {
        record_id: format!("{}/did:key:z6MkOtherPeer/{OWNER}", verse_scope()),
        ..genuine.clone()
    };
    let cases: Vec<(&str, Statement, &str)> = vec![
        ("forged grantor (T1)", genuine.clone(), MALLORY),
        ("payload disagrees with the key", mismatched_payload, OWNER),
        (
            "cross-verse scope (T6)",
            grant(
                OWNER,
                P,
                &build_scope("verse-other", None, None),
                "editor",
                1,
            ),
            OWNER,
        ),
        (
            "owner is never grantable (T3)",
            grant(OWNER, P, &verse_scope(), "owner", 1),
            OWNER,
        ),
        (
            "unknown role string",
            grant(OWNER, P, &verse_scope(), "admin", 1),
            OWNER,
        ),
        (
            "the owner is never a target (T5)",
            grant(MALLORY, OWNER, &verse_scope(), "viewer", 1),
            MALLORY,
        ),
        (
            "malformed key",
            Statement {
                record_id: "only/two".to_string(),
                ..genuine.clone()
            },
            OWNER,
        ),
        (
            "malformed payload",
            Statement {
                bytes: b"{not json".to_vec(),
                ..genuine.clone()
            },
            OWNER,
        ),
    ];
    for (name, stmt, author) in cases {
        let db = verse_db().await;
        let mut queue = InboundAdmissions::new();
        let reports = queue
            .apply(
                &db,
                VERSE,
                "role",
                &stmt.record_id,
                &stmt.bytes,
                author,
                None,
            )
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Denied, "{name}");
        assert!(admitted(&db).await.is_empty(), "{name}");
        assert!(held(&db).await.is_empty(), "{name}");
        assert!(queue.is_empty(), "{name}");
    }
}

/// §2.1: a `Doc::del` on the `role` table carries no HLC (and iroh's `del` is a prefix
/// delete), so it is never a revocation — revocation is a non-empty `revoked: true` statement.
#[tokio::test]
async fn an_empty_entry_on_the_role_table_is_not_applicable() {
    let db = verse_db().await;
    let mut queue = InboundAdmissions::new();
    let stmt = grant(OWNER, "did:key:z6MkKept", &verse_scope(), "editor", 1);
    deliver(&db, &mut queue, &stmt).await;
    let reports = queue
        .apply(&db, VERSE, "role", &stmt.record_id, &[], OWNER, None)
        .await;
    assert_eq!(reports[0].outcome, ReplicatedRowOutcome::NotApplicable);
    assert_eq!(keys(&admitted(&db).await), BTreeSet::from([stmt.record_id]));
}
