//! role_propagation RP-2 flows on the DB plane, driven through the public role seams
//! (`role_manager`, `role_grants`, `verse_settings`, the migration) over an in-memory store:
//! local statements, the materialized `role` cache, subtree authority, the partition
//! (cascade + promotion), governance gates and the legacy migration.
//! See fe-database/src/AGENTS.md §role-propagation.

use std::sync::{Arc, Once};

use fe_database::handlers::verse_settings::{
    set_verse_default_access_handler, set_verse_timeseries_settings_handler,
};
use fe_database::migration::role_grants::migrate_legacy_role_rows;
use fe_database::repo::Db;
use fe_database::role_grants::{self, Placement};
use fe_database::role_manager::{self, RoleCommandError};
use fe_database::{build_scope, BlobStoreHandle, ReplicationEvent, RoleLevel};
use fe_policy::{GrantedRole, RoleStatement, RoleStatementPayload, StatementKey};
use fe_runtime::blob_store::mock::MockBlobStore;

const OWNER: &str = "did:key:z6MkOwner";
const VERSE: &str = "verse-rp2";
const FOREIGN_VERSE: &str = "verse-foreign";
const FRACTAL: &str = "fractal-rp2";
const PETAL_A: &str = "petal-a";
const PETAL_B: &str = "petal-b";

static HLC: Once = Once::new();

/// One in-memory store holding a verse owned by [`OWNER`] with one fractal and two petals.
struct Fixture {
    db: Db,
    blobs: Arc<MockBlobStore>,
    store: BlobStoreHandle,
    repl_tx: crossbeam::channel::Sender<ReplicationEvent>,
    repl_rx: crossbeam::channel::Receiver<ReplicationEvent>,
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

async fn fixture() -> Fixture {
    // The HLC is process-global; this binary has no DB thread, so one init is enough and
    // nothing resets it mid-test.
    HLC.call_once(|| fe_database::op_log::init_hlc(0));
    let db: Db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("mem db");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("schema");
    for (verse, owner) in [(VERSE, OWNER), (FOREIGN_VERSE, "did:key:z6MkSomeoneElse")] {
        db.query(
            "CREATE verse CONTENT { verse_id: $v, name: 'V', created_by: $o, \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' }",
        )
        .bind(("v", verse))
        .bind(("o", owner))
        .await
        .expect("verse")
        .check()
        .expect("verse check");
    }
    query(
        &db,
        &format!(
            "CREATE fractal CONTENT {{ fractal_id: '{FRACTAL}', verse_id: '{VERSE}', \
             owner_did: '{OWNER}', name: 'F', created_at: '2026-10-15T00:00:00Z' }}"
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
    let blobs = Arc::new(MockBlobStore::new());
    let (repl_tx, repl_rx) = crossbeam::channel::bounded(256);
    Fixture {
        db,
        store: blobs.clone(),
        blobs,
        repl_tx,
        repl_rx,
    }
}

impl Fixture {
    async fn assign(
        &self,
        caller: &str,
        peer: &str,
        scope: &str,
        role: &str,
    ) -> anyhow::Result<()> {
        role_manager::assign_role_checked(
            &self.db,
            caller,
            peer,
            scope,
            role,
            Some(&self.repl_tx),
            &self.store,
        )
        .await
    }

    async fn revoke(
        &self,
        caller: &str,
        peer: &str,
        scope: &str,
    ) -> anyhow::Result<role_manager::RevokeReport> {
        role_manager::revoke_role_checked(
            &self.db,
            caller,
            peer,
            scope,
            Some(&self.repl_tx),
            &self.store,
        )
        .await
    }

    async fn role(&self, peer: &str, scope: &str) -> RoleLevel {
        role_manager::resolve_role(&self.db, peer, scope)
            .await
            .expect("resolve_role")
    }

    /// Every replication event so far, with role statements decoded from their blobs.
    fn drain(&self) -> Vec<(ReplicationEvent, Option<RoleStatementPayload>)> {
        let mut events = Vec::new();
        while let Ok(event) = self.repl_rx.try_recv() {
            let payload = (event.table == "role").then(|| {
                let bytes = self.blobs.bytes_for(&event.content_hash).expect("blob");
                serde_json::from_slice(&bytes).expect("role statement payload")
            });
            events.push((event, payload));
        }
        events
    }

    async fn count(&self, table: &str) -> u64 {
        let mut res = self
            .db
            .query(format!("SELECT count() AS c FROM {table} GROUP ALL"))
            .await
            .expect("count");
        let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
        rows.first().and_then(|r| r["c"].as_u64()).unwrap_or(0)
    }

    async fn verse_row(&self) -> serde_json::Value {
        let mut res = self
            .db
            .query("SELECT * FROM verse WHERE verse_id = $v")
            .bind(("v", VERSE))
            .await
            .expect("verse select");
        let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
        rows.into_iter().next().expect("verse row")
    }
}

fn held_key(stmt: &RoleStatement) -> (String, String, String) {
    (
        stmt.scope.clone(),
        stmt.peer_did.clone(),
        stmt.grantor_did.clone(),
    )
}

#[tokio::test]
async fn owner_assigns_editor_and_the_statement_is_published_and_materialized() {
    let fx = fixture().await;
    let scope = petal_scope(PETAL_A);
    fx.assign(OWNER, "did:key:z6MkEditor", &scope, "editor")
        .await
        .expect("owner assigns");

    // Materialized: the hot-path resolver reads the cache.
    assert_eq!(
        fx.role("did:key:z6MkEditor", &scope).await,
        RoleLevel::Editor
    );
    let admitted = role_grants::load_admitted(&fx.db, VERSE)
        .await
        .expect("admitted");
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].grantor_did, OWNER);

    // Published: exactly one role statement event, keyed scope/peer/grantor.
    let events = fx.drain();
    assert_eq!(events.len(), 1, "{events:?}");
    let (event, payload) = &events[0];
    assert_eq!(event.table, "role");
    assert_eq!(event.verse_id, VERSE);
    assert_eq!(
        event.record_id,
        format!("{scope}/did:key:z6MkEditor/{OWNER}")
    );
    let payload = payload.as_ref().expect("payload");
    assert_eq!(payload.role, "editor");
    assert!(!payload.revoked);
    assert_eq!(
        StatementKey::parse(&event.record_id)
            .expect("key")
            .grantor_did,
        payload.grantor_did,
        "the key's grantor is the payload's grantor"
    );
}

/// T4 locally: the owner revokes a verse Manager; the Manager's grants and the grants of the
/// petal Manager they appointed lapse down the subtree, held (not dropped) in pending.
#[tokio::test]
async fn a_manager_grant_then_owner_revocation_of_the_manager_voids_the_grant_locally() {
    let fx = fixture().await;
    let (m, pm, x, y) = (
        "did:key:z6MkM",
        "did:key:z6MkPetalManager",
        "did:key:z6MkX",
        "did:key:z6MkY",
    );
    fx.assign(OWNER, m, &verse_scope(), "manager")
        .await
        .expect("owner → M");
    fx.assign(m, x, &verse_scope(), "editor")
        .await
        .expect("M → X");
    fx.assign(m, pm, &petal_scope(PETAL_A), "manager")
        .await
        .expect("M → PM");
    fx.assign(pm, y, &petal_scope(PETAL_A), "editor")
        .await
        .expect("PM → Y");
    assert_eq!(fx.role(x, &verse_scope()).await, RoleLevel::Editor);
    assert_eq!(fx.role(y, &petal_scope(PETAL_A)).await, RoleLevel::Editor);
    fx.drain();

    fx.revoke(OWNER, m, &verse_scope())
        .await
        .expect("owner revokes M");

    assert_eq!(
        fx.role(m, &verse_scope()).await,
        RoleLevel::Viewer,
        "default access"
    );
    assert_eq!(fx.role(x, &verse_scope()).await, RoleLevel::Viewer);
    assert_eq!(fx.role(pm, &petal_scope(PETAL_A)).await, RoleLevel::Viewer);
    assert_eq!(fx.role(y, &petal_scope(PETAL_A)).await, RoleLevel::Viewer);

    let held: Vec<_> = role_grants::load_held(&fx.db, VERSE)
        .await
        .expect("held")
        .iter()
        .map(held_key)
        .collect();
    for (peer, scope, grantor) in [
        (x, verse_scope(), m),
        (pm, petal_scope(PETAL_A), m),
        (y, petal_scope(PETAL_A), pm),
    ] {
        assert!(
            held.contains(&(scope.clone(), peer.to_string(), grantor.to_string())),
            "{grantor}'s grant to {peer} must be held, not dropped: {held:?}"
        );
    }
    let admitted = role_grants::load_admitted(&fx.db, VERSE)
        .await
        .expect("admitted");
    assert!(
        admitted.iter().all(|s| s.grantor_did == OWNER),
        "only owner statements stay admitted: {admitted:?}"
    );

    let events = fx.drain();
    assert_eq!(events.len(), 1);
    let payload = events[0].1.as_ref().expect("role payload");
    assert!(payload.revoked && payload.peer_did == m && payload.grantor_did == OWNER);

    // Regranting M re-admits every held statement down the subtree, with no re-delivery.
    fx.assign(OWNER, m, &verse_scope(), "manager")
        .await
        .expect("owner → M again");
    assert_eq!(fx.role(x, &verse_scope()).await, RoleLevel::Editor);
    assert_eq!(fx.role(y, &petal_scope(PETAL_A)).await, RoleLevel::Editor);
    assert!(role_grants::load_held(&fx.db, VERSE)
        .await
        .expect("held")
        .is_empty());
}

#[tokio::test]
async fn a_manager_revoking_another_managers_grant_reports_the_role_still_held() {
    let fx = fixture().await;
    let (m1, m2, x) = ("did:key:z6MkM1", "did:key:z6MkM2", "did:key:z6MkX");
    fx.assign(OWNER, m1, &verse_scope(), "manager")
        .await
        .expect("owner → M1");
    fx.assign(OWNER, m2, &verse_scope(), "manager")
        .await
        .expect("owner → M2");
    fx.assign(m1, x, &fractal_scope(), "editor")
        .await
        .expect("M1 → X");

    let report = fx
        .revoke(m2, x, &fractal_scope())
        .await
        .expect("M2 revokes");
    assert!(report.still_held(), "{report:?}");
    assert_eq!(report.role_after, RoleLevel::Editor);
    assert_eq!(fx.role(x, &petal_scope(PETAL_B)).await, RoleLevel::Editor);

    // M1's own revoke does retract it.
    let report = fx
        .revoke(m1, x, &fractal_scope())
        .await
        .expect("M1 revokes");
    assert!(!report.still_held(), "{report:?}");
    assert_eq!(fx.role(x, &fractal_scope()).await, RoleLevel::Viewer);
}

#[tokio::test]
async fn refused_assignments_write_nothing() {
    let fx = fixture().await;
    let m = "did:key:z6MkM";
    fx.assign(OWNER, m, &verse_scope(), "manager")
        .await
        .expect("owner → M");
    fx.drain();
    let (grants, ops) = (fx.count("role_grant").await, fx.count("op_log").await);

    let cases: [(&str, &str, &str, RoleCommandError); 4] = [
        (
            OWNER,
            "did:key:z6MkX",
            "owner",
            RoleCommandError::OwnerNotAssignable,
        ),
        (
            m,
            "did:key:z6MkX",
            "owner",
            RoleCommandError::OwnerNotAssignable,
        ),
        (OWNER, OWNER, "editor", RoleCommandError::TargetIsOwner),
        (
            m,
            "did:key:z6MkX",
            "none",
            RoleCommandError::ManagerCannotAssignNone,
        ),
    ];
    for (caller, target, role, expected) in cases {
        let err = fx
            .assign(caller, target, &verse_scope(), role)
            .await
            .expect_err("refused");
        assert_eq!(
            err.downcast_ref::<RoleCommandError>(),
            Some(&expected),
            "{caller} assigning {role} to {target}: {err:#}"
        );
    }
    let err = fx
        .revoke(m, OWNER, &verse_scope())
        .await
        .expect_err("owner untouchable");
    assert_eq!(
        err.downcast_ref::<RoleCommandError>(),
        Some(&RoleCommandError::TargetIsOwner)
    );

    assert_eq!(fx.count("role_grant").await, grants, "no statement written");
    assert_eq!(
        fx.count("op_log").await,
        ops,
        "refusals precede the op-log append"
    );
    assert!(fx.drain().is_empty(), "nothing published");

    // The owner MAY assign `none`: it pins the cell below the inherited role.
    fx.assign(OWNER, m, &petal_scope(PETAL_A), "none")
        .await
        .expect("owner pins none");
    assert_eq!(fx.role(m, &petal_scope(PETAL_A)).await, RoleLevel::None);
    assert_eq!(fx.role(m, &petal_scope(PETAL_B)).await, RoleLevel::Manager);
}

/// R7 local parity, T19: a petal Manager grants in their own petal only.
#[tokio::test]
async fn a_petal_manager_assigns_in_their_petal_and_is_refused_in_a_sibling_petal_and_at_the_verse()
{
    let fx = fixture().await;
    let (pm, x) = ("did:key:z6MkPetalManager", "did:key:z6MkX");
    fx.assign(OWNER, pm, &petal_scope(PETAL_A), "manager")
        .await
        .expect("owner → PM");
    fx.drain();

    fx.assign(pm, x, &petal_scope(PETAL_A), "editor")
        .await
        .expect("in-petal grant");
    assert_eq!(fx.role(x, &petal_scope(PETAL_A)).await, RoleLevel::Editor);
    let events = fx.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].0.record_id,
        format!("{}/{x}/{pm}", petal_scope(PETAL_A))
    );

    let grants = fx.count("role_grant").await;
    for scope in [petal_scope(PETAL_B), fractal_scope(), verse_scope()] {
        let err = fx
            .assign(pm, x, &scope, "editor")
            .await
            .expect_err("outside subtree");
        assert!(
            matches!(
                err.downcast_ref::<RoleCommandError>(),
                Some(RoleCommandError::InsufficientAuthority { .. })
            ),
            "{scope}: {err:#}"
        );
    }
    assert_eq!(fx.count("role_grant").await, grants);
    assert!(fx.drain().is_empty());
    assert_eq!(fx.role(x, &petal_scope(PETAL_B)).await, RoleLevel::Viewer);
}

/// R7: a statement held because its grantor had no authority yet is admitted when a local
/// owner grant empowers the grantor — no second delivery.
#[tokio::test]
async fn a_held_statement_is_promoted_when_a_local_owner_grant_empowers_its_grantor() {
    let fx = fixture().await;
    let (m, x) = ("did:key:z6MkM", "did:key:z6MkX");
    let early = RoleStatement {
        scope: petal_scope(PETAL_B),
        peer_did: x.to_string(),
        grantor_did: m.to_string(),
        role: GrantedRole::Editor,
        revoked: false,
        hlc: 7,
        editor_since: None,
        manager_since: None,
    };
    let (placement, _) = role_grants::submit_statement(&fx.db, VERSE, &early)
        .await
        .expect("submit");
    assert_eq!(placement, Placement::Held);
    assert_eq!(fx.role(x, &petal_scope(PETAL_B)).await, RoleLevel::Viewer);

    // Re-delivery of the same statement is idempotent.
    let (again, _) = role_grants::submit_statement(&fx.db, VERSE, &early)
        .await
        .expect("resubmit");
    assert_eq!(again, Placement::Superseded);

    fx.assign(OWNER, m, &fractal_scope(), "manager")
        .await
        .expect("owner → M");
    assert_eq!(fx.role(x, &petal_scope(PETAL_B)).await, RoleLevel::Editor);
    assert!(role_grants::load_held(&fx.db, VERSE)
        .await
        .expect("held")
        .is_empty());
    assert!(role_grants::load_admitted(&fx.db, VERSE)
        .await
        .expect("admitted")
        .contains(&early));
}

/// T8, seeder path (the CreatePetal path runs through the DB thread in
/// role_propagation_durable_test.rs).
#[tokio::test]
async fn the_seeder_writes_no_implicit_petal_owner_row() {
    let fx = fixture().await;
    let local = "did:key:z6MkSeeder";
    fe_database::seed_default_data(&fx.db, &fx.store, local)
        .await
        .expect("seed");
    let mut res = fx
        .db
        .query("SELECT * FROM role WHERE role = 'owner'")
        .await
        .expect("select");
    let rows: Vec<serde_json::Value> = res.take(0).expect("rows");
    assert!(rows.is_empty(), "no implicit owner rows: {rows:?}");
}

/// R7 migration, T8: an Editor's implicit petal-owner row is dropped and they keep only the
/// role they inherit; the owner's explicit grant to them survives as an owner statement.
#[tokio::test]
async fn implicit_petal_owner_rows_are_dropped_and_the_creator_keeps_only_their_inherited_role() {
    let fx = fixture().await;
    let editor = "did:key:z6MkEditor";
    for (scope, role) in [(verse_scope(), "editor"), (petal_scope(PETAL_A), "owner")] {
        fe_database::rbac::assign_role(&fx.db, editor, &scope, role)
            .await
            .expect("legacy row");
    }
    assert_eq!(
        fx.role(editor, &petal_scope(PETAL_A)).await,
        RoleLevel::Owner
    );

    role_grants::startup(&fx.db, OWNER, Some(&fx.repl_tx), &fx.store)
        .await
        .expect("startup");

    assert_eq!(
        fx.role(editor, &petal_scope(PETAL_A)).await,
        RoleLevel::Editor
    );
    assert_eq!(fx.role(editor, &verse_scope()).await, RoleLevel::Editor);
}

#[tokio::test]
async fn legacy_role_rows_migrate_owner_rows_dropped_owner_authored_rows_become_statements() {
    let fx = fixture().await;
    let (x, y) = ("did:key:z6MkX", "did:key:z6MkY");
    let interim_scope = format!("VERSE#{}", "ab".repeat(32));
    for (peer, scope, role) in [
        (x, verse_scope(), "editor"),             // → owner statement
        (y, petal_scope(PETAL_A), "owner"),       // dropped (implicit owner)
        (y, fractal_scope(), "public"),           // dropped (unknown role)
        (OWNER, petal_scope(PETAL_B), "manager"), // dropped (targets the owner)
        (x, build_scope(FOREIGN_VERSE, None, None), "editor"), // dropped (not our verse)
        (x, interim_scope.clone(), "manager"),    // untouched (interim authority)
        (x, "VERSE#gone".to_string(), "editor"),  // untouched (no such verse)
    ] {
        fe_database::rbac::assign_role(&fx.db, peer, &scope, role)
            .await
            .expect("legacy row");
    }

    let report = migrate_legacy_role_rows(&fx.db, OWNER)
        .await
        .expect("migrate");
    assert!(!report.already_applied);
    assert_eq!(report.dropped_owner_rows, 1);
    assert_eq!(report.converted_to_statements, 1);
    assert_eq!(report.dropped_other_rows, 3);
    let again = migrate_legacy_role_rows(&fx.db, OWNER)
        .await
        .expect("rerun");
    assert!(again.already_applied, "marker-guarded: runs once per DB");

    role_grants::startup(&fx.db, OWNER, Some(&fx.repl_tx), &fx.store)
        .await
        .expect("startup");

    let admitted = role_grants::load_admitted(&fx.db, VERSE)
        .await
        .expect("admitted");
    assert_eq!(admitted.len(), 1);
    assert_eq!(
        (
            admitted[0].peer_did.as_str(),
            admitted[0].grantor_did.as_str(),
            admitted[0].role
        ),
        (x, OWNER, GrantedRole::Editor)
    );
    assert_eq!(fx.role(x, &petal_scope(PETAL_B)).await, RoleLevel::Editor);
    assert_eq!(fx.role(y, &fractal_scope()).await, RoleLevel::Viewer);
    assert_eq!(
        fe_database::rbac::get_all_roles_for_scope(&fx.db, &build_scope(FOREIGN_VERSE, None, None))
            .await
            .expect("foreign"),
        Vec::<(String, String)>::new()
    );
    for untouched in [interim_scope.as_str(), "VERSE#gone"] {
        assert_eq!(
            fe_database::rbac::get_all_roles_for_scope(&fx.db, untouched)
                .await
                .expect("untouched"),
            vec![(
                x.to_string(),
                if untouched == "VERSE#gone" {
                    "editor"
                } else {
                    "manager"
                }
                .to_string()
            )],
            "{untouched} must survive the migration and the repartition"
        );
    }

    // The converted statement is published by the startup re-emit (the owner is local).
    let events = fx.drain();
    assert!(
        events.iter().any(|(e, p)| e.table == "role"
            && p.as_ref()
                .is_some_and(|p| p.peer_did == x && p.grantor_did == OWNER)),
        "{events:?}"
    );
}

/// Gate parity (RP-2 step 5, T13 local half): verse governance needs Manager+ at the verse.
#[tokio::test]
async fn non_manager_cannot_change_default_access_or_ts_settings_locally() {
    let fx = fixture().await;
    let (viewer, editor, petal_manager, verse_manager) = (
        "did:key:z6MkViewer",
        "did:key:z6MkEditor",
        "did:key:z6MkPetalManager",
        "did:key:z6MkVerseManager",
    );
    fx.assign(OWNER, editor, &verse_scope(), "editor")
        .await
        .expect("E");
    fx.assign(OWNER, petal_manager, &petal_scope(PETAL_A), "manager")
        .await
        .expect("PM");
    fx.assign(OWNER, verse_manager, &verse_scope(), "manager")
        .await
        .expect("VM");
    fx.drain();

    for caller in [viewer, editor, petal_manager] {
        set_verse_default_access_handler(
            &fx.db,
            &fx.store,
            Some(&fx.repl_tx),
            caller,
            VERSE,
            "none",
        )
        .await
        .expect_err("default access refused");
        set_verse_timeseries_settings_handler(
            &fx.db,
            &fx.store,
            Some(&fx.repl_tx),
            caller,
            VERSE,
            "sharded",
            1,
            86_400_000,
        )
        .await
        .expect_err("ts settings refused");
    }
    let row = fx.verse_row().await;
    assert_eq!(row["default_access"], "viewer");
    assert_eq!(row["ts_mode"], "mirror");
    assert!(fx.drain().is_empty(), "a refused change publishes nothing");

    set_verse_default_access_handler(
        &fx.db,
        &fx.store,
        Some(&fx.repl_tx),
        verse_manager,
        VERSE,
        "none",
    )
    .await
    .expect("verse Manager sets default access");
    set_verse_timeseries_settings_handler(
        &fx.db,
        &fx.store,
        Some(&fx.repl_tx),
        verse_manager,
        VERSE,
        "sharded",
        1,
        86_400_000,
    )
    .await
    .expect("verse Manager sets ts settings");
    let manifests: Vec<serde_json::Value> = fx
        .drain()
        .into_iter()
        .filter(|(e, _)| e.table == "verse")
        .map(|(e, _)| {
            serde_json::from_slice(&fx.blobs.bytes_for(&e.content_hash).expect("blob"))
                .expect("manifest json")
        })
        .collect();
    assert_eq!(manifests.len(), 2, "both changes re-emit the manifest");
    assert_eq!(manifests[0]["default_access"], "none");
    assert_eq!(manifests[1]["ts_mode"], "sharded");
}
