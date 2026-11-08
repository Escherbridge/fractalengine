//! role_propagation RP-2 flows through the real DB thread over a persisted SurrealKV store:
//! the CreatePetal path writes no implicit owner row (T8), the AC-42 durable re-emit of local
//! role statements across a restart and a lost publish (T23), and the AC-45 guarantee that the
//! role migration and rematerialization never touch an interim `VERSE#<hex>` authority row
//! (T18). Restarts drop AND join the DB thread before reopening (C8: one SurrealKV open per
//! store); tests run one at a time because the HLC is process-global and every DB-thread start
//! re-initialises it. See fe-database/src/AGENTS.md §role-propagation.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use fe_canonical_log::capability::{AuthorityState, ManagerAuthorityView, Principal};
use fe_canonical_log::envelope::{Hash32, Identifier32, Scope};
use fe_database::canon_log::canonical_epoch::{
    interim_authority_scope_key, DurableAuthorizationView,
};
use fe_database::{build_scope, repo::Db, spawn_db_thread_shared, DbThread, ReplicationEvent};
use fe_identity::NodeKeypair;
use fe_policy::RoleStatementPayload;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::messages::{DbCommand, DbResult};

const TIMEOUT: Duration = Duration::from_secs(20);
const NODE_SEED: [u8; 32] = [0x5a; 32];
const PEER: &str = "did:key:z6MkRoleTarget";
const ANCHOR: Hash32 = Hash32([0xa7; 32]);

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn node_keypair() -> NodeKeypair {
    NodeKeypair::from_bytes(&NODE_SEED).expect("node keypair")
}

fn local_did() -> String {
    node_keypair().to_did_key()
}

/// A DB thread over a store directory the TEST owns, with the replication bridge exposed.
struct Harness {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    repl_rx: crossbeam::channel::Receiver<ReplicationEvent>,
    blobs: Arc<MockBlobStore>,
    db: Option<Db>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Harness {
    fn open(dir: &Path, replication_capacity: usize) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(64);
        let (repl_tx, repl_rx) =
            crossbeam::channel::bounded::<ReplicationEvent>(replication_capacity);
        let blobs = Arc::new(MockBlobStore::new());
        let DbThread {
            join_handle,
            shared_db,
        } = spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            blobs.clone(),
            Some(repl_tx),
            Some(node_keypair()),
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
            repl_rx,
            blobs,
            db: Some(shared_db),
            join: Some(join_handle),
        }
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

    /// Sends a command and returns the first reply `accept` matches (an `Error` panics).
    fn ask(&self, command: DbCommand, accept: impl Fn(&DbResult) -> bool) -> DbResult {
        self.send(command);
        loop {
            let result = self.res_rx.recv_timeout(TIMEOUT).expect("DB reply");
            if let DbResult::Error(e) = &result {
                panic!("DB command failed: {e}");
            }
            if accept(&result) {
                return result;
            }
        }
    }

    /// verse → fractal → petal through the real commands; returns `(verse, petal scope)`.
    fn create_hierarchy(&self) -> (String, String) {
        let DbResult::VerseCreated { id: verse, .. } = self
            .ask(DbCommand::CreateVerse { name: "V".into() }, |r| {
                matches!(r, DbResult::VerseCreated { .. })
            })
        else {
            unreachable!()
        };
        let fractal = self.create_fractal(&verse);
        let DbResult::PetalCreated { id: petal, .. } = self.ask(
            DbCommand::CreatePetal {
                fractal_id: fractal.clone(),
                name: "P".into(),
            },
            |r| matches!(r, DbResult::PetalCreated { .. }),
        ) else {
            unreachable!()
        };
        let scope = build_scope(&verse, Some(&fractal), Some(&petal));
        (verse, scope)
    }

    fn create_fractal(&self, verse: &str) -> String {
        let DbResult::FractalCreated { id, .. } = self.ask(
            DbCommand::CreateFractal {
                verse_id: verse.to_string(),
                name: "F".into(),
            },
            |r| matches!(r, DbResult::FractalCreated { .. }),
        ) else {
            unreachable!()
        };
        id
    }

    fn assign(&self, scope: &str, role: &str) {
        self.ask(
            DbCommand::AssignRole {
                peer_did: PEER.into(),
                scope: scope.into(),
                role: role.into(),
            },
            |r| matches!(r, DbResult::RoleAssigned { .. }),
        );
    }

    fn revoke(&self, scope: &str) {
        self.ask(
            DbCommand::RevokeRole {
                peer_did: PEER.into(),
                scope: scope.into(),
            },
            |r| matches!(r, DbResult::RoleRevoked { .. }),
        );
    }

    fn peer_role(&self, scope: &str) -> String {
        let DbResult::PeerRolesResolved { roles, .. } = self.ask(
            DbCommand::ResolveRolesForPeers {
                scope: scope.into(),
                peer_dids: vec![PEER.into()],
            },
            |r| matches!(r, DbResult::PeerRolesResolved { .. }),
        ) else {
            unreachable!()
        };
        roles.into_iter().next().expect("one role").1
    }

    /// Every role statement published so far, decoded from its blob.
    fn role_statements(&self) -> Vec<(ReplicationEvent, RoleStatementPayload)> {
        let mut out = Vec::new();
        while let Ok(event) = self.repl_rx.try_recv() {
            if event.table != "role" {
                continue;
            }
            let bytes = self.blobs.bytes_for(&event.content_hash).expect("blob");
            let payload = serde_json::from_slice(&bytes).expect("role statement payload");
            out.push((event, payload));
        }
        out
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

/// T8 through the real CreatePetal path: a non-owner creating a petal (petal creation needs
/// Manager) gets no `owner` row there; demoted to Editor it resolves to Editor, never Owner.
#[test]
fn a_non_owner_creating_a_petal_gets_no_implicit_owner_row() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), 64);
    let db = harness.db();
    let rt = runtime();
    let verse_scope = build_scope("verse-of-another", None, None);
    rt.block_on(async {
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-of-another', name: 'V', \
             created_by: 'did:key:z6MkSomeoneElse', created_at: '2026-10-15T00:00:00Z', \
             default_access: 'viewer' }; \
             CREATE fractal CONTENT { fractal_id: 'f-1', verse_id: 'verse-of-another', \
             owner_did: 'did:key:z6MkSomeoneElse', name: 'F', created_at: '2026-10-15T00:00:00Z' };",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed check");
        // The materialized cache entry an inbound owner grant would produce for this node.
        fe_database::rbac::assign_role(&db, &local_did(), &verse_scope, "manager")
            .await
            .expect("manager cache row");
    });

    let DbResult::PetalCreated { id: petal, .. } = harness.ask(
        DbCommand::CreatePetal {
            fractal_id: "f-1".into(),
            name: "Editor's petal".into(),
        },
        |r| matches!(r, DbResult::PetalCreated { .. }),
    ) else {
        unreachable!()
    };
    let petal_scope = build_scope("verse-of-another", Some("f-1"), Some(&petal));

    let owner_rows: Vec<serde_json::Value> = rt.block_on(async {
        db.query("SELECT * FROM role WHERE role = 'owner'")
            .await
            .expect("select")
            .take(0)
            .expect("rows")
    });
    assert!(owner_rows.is_empty(), "{owner_rows:?}");
    rt.block_on(fe_database::rbac::assign_role(&db, &local_did(), &verse_scope, "editor"))
        .expect("demoted to editor");
    let DbResult::LocalRoleResolved { role, .. } = harness
        .ask(DbCommand::ResolveLocalRole { scope: petal_scope.clone() }, |r| {
            matches!(r, DbResult::LocalRoleResolved { .. })
        })
    else {
        unreachable!()
    };
    assert_eq!(role, "editor");

    use fe_runtime::entity_access::{EntityAccessAction as Access,EntityAccessTarget};
    use fe_runtime::messages::EntityType;
    let denied=EntityAccessTarget {entity_type:EntityType::Petal,entity_id:petal,scope:petal_scope.clone()};
    let DbResult::EntityAccessFinished {request_id,result,..}=harness.ask(DbCommand::EntityAccess {target:denied,action:Access::Load {peer_dids:vec![PEER.into()]},request_id:"access-editor-denied".into()},|r|matches!(r,DbResult::EntityAccessFinished {..})) else {unreachable!()};
    assert_eq!(request_id,"access-editor-denied");assert!(result.is_err(),"Editor cannot inspect Manager-only access metadata");
    let (_,owned_scope)=harness.create_hierarchy();
    let owned_parts=fe_database::parse_scope(&owned_scope).unwrap();
    harness.ask(DbCommand::SetVerseDefaultAccess {verse_id:owned_parts.verse_id.clone(),default_access:"viewer".into()},|reply|matches!(reply,DbResult::VerseDefaultAccessSet {..}));
    let target=EntityAccessTarget {entity_type:EntityType::Petal,entity_id:owned_parts.petal_id.clone().unwrap(),scope:owned_scope};
    let access=|id:&str,action:Access| {
        let DbResult::EntityAccessFinished {target:returned,request_id,result}=harness.ask(DbCommand::EntityAccess {target:target.clone(),action,request_id:id.into()},|r|matches!(r,DbResult::EntityAccessFinished {..})) else {unreachable!()};
        assert_eq!(returned,target);assert_eq!(request_id,id);result
    };
    let initial=access("access-owner-load",Access::Load {peer_dids:vec![PEER.into()]}).expect("Owner reads roles");
    assert_eq!(initial.local_role,"owner");assert!(initial.roles.iter().any(|(did,role)|did==PEER&&role=="viewer"));
    let assigned=access("access-assign",Access::Assign {peer_did:PEER.into(),role:"editor".into()}).expect("checked assignment");
    assert!(assigned.roles.iter().any(|(did,role)|did==PEER&&role=="editor"));
    let revoked=access("access-revoke",Access::Revoke {peer_did:PEER.into()}).expect("checked revoke");
    assert!(revoked.roles.iter().any(|(did,role)|did==PEER&&role=="viewer"),"an owner revoke blocks the cell: the peer keeps only the verse default, never the revoked grant");
    let reopened=access("access-reopen",Access::Load {peer_dids:vec![]}).expect("reopened Access tab");
    assert!(reopened.roles.iter().any(|(did,role)|did==PEER&&role=="viewer"),"QA-0162: a revoked member absent from the peer registry stays listed so it can be re-granted");
    let regranted=access("access-regrant",Access::Assign {peer_did:PEER.into(),role:"editor".into()}).expect("re-grant after revoke");
    assert!(regranted.roles.iter().any(|(did,role)|did==PEER&&role=="editor"));
    access("access-rerevoke",Access::Revoke {peer_did:PEER.into()}).expect("revoke again");
    // A grantee holds one statement per (scope, grantor): 1100 revoked ones must not overflow the listing.
    let statements:Vec<serde_json::Value>=(0..1100).map(|i|{
        let scope=format!("VERSE#{}-FRACTAL#{}-PETAL#ghost-{i}",owned_parts.verse_id,owned_parts.fractal_id.as_deref().unwrap());
        serde_json::json!({"statement_key":format!("{scope}/{PEER}/{}",local_did()),"verse_id":owned_parts.verse_id,"scope":scope,
            "peer_did":PEER,"grantor_did":local_did(),"role":"editor","revoked":true,"hlc":"1","updated_at":"2026-11-08T00:00:00Z"})
    }).collect();
    rt.block_on(async {db.query("INSERT INTO role_grant $statements").bind(("statements",statements)).await.expect("seed statements").check().expect("seeded statements")});
    let crowded=access("access-many-statements",Access::Load {peer_dids:vec![]}).expect("a grantee's many statements list it once");
    assert_eq!(crowded.roles.iter().filter(|(did,_)|did==PEER).count(),1);
    let minted=access("access-token-mint",Access::MintToken {max_role:"viewer".into(),ttl_hours:1,client_did:None}).expect("scoped mint");
    assert!(minted.minted.is_some());assert!(!format!("{minted:?}").contains(&minted.minted.as_ref().unwrap().token),"Debug never reveals token");
    let token=minted.tokens.first().expect("readback token");
    let verse_target = EntityAccessTarget {
        entity_type: EntityType::Verse,
        entity_id: owned_parts.verse_id.clone(),
        scope: build_scope(&owned_parts.verse_id, None, None),
    };
    let fractal_target = EntityAccessTarget {
        entity_type: EntityType::Fractal,
        entity_id: owned_parts.fractal_id.clone().unwrap(),
        scope: build_scope(
            &owned_parts.verse_id,
            owned_parts.fractal_id.as_deref(),
            None,
        ),
    };
    for (boundary, foreign_scopes) in [
        (
            verse_target,
            vec![
                build_scope(&format!("{}-other", owned_parts.verse_id), None, None),
                build_scope(
                    &format!("{}-other", owned_parts.verse_id),
                    owned_parts.fractal_id.as_deref(),
                    owned_parts.petal_id.as_deref(),
                ),
            ],
        ),
        (
            fractal_target,
            vec![
                build_scope(
                    &owned_parts.verse_id,
                    Some(&format!(
                        "{}-other",
                        owned_parts.fractal_id.as_deref().unwrap()
                    )),
                    None,
                ),
                build_scope(
                    &owned_parts.verse_id,
                    Some(&format!(
                        "{}-other",
                        owned_parts.fractal_id.as_deref().unwrap()
                    )),
                    owned_parts.petal_id.as_deref(),
                ),
            ],
        ),
        (
            target.clone(),
            vec![build_scope(
                &owned_parts.verse_id,
                owned_parts.fractal_id.as_deref(),
                Some(&format!(
                    "{}-other",
                    owned_parts.petal_id.as_deref().unwrap()
                )),
            )],
        ),
    ] {
        let malformed_scopes = [
            format!("{}-FRACTAL#misplaced", target.scope),
            format!("{}-PETAL#misplaced", target.scope),
        ];
        for foreign_scope in foreign_scopes.into_iter().chain(malformed_scopes) {
            let foreign = fe_database::api_token_store::ApiTokenRecord {
                jti: format!("foreign-{}", ulid::Ulid::new()),
                scope: foreign_scope.clone(),
                max_role: "viewer".into(),
                label: None,
                sub: local_did(),
                created_at: "2026-01-01T00:00:00Z".into(),
                expires_at: "2099-01-01T00:00:00Z".into(),
                revoked: false,
                client: None,
            };
            rt.block_on(fe_database::api_token_store::store_api_token(&db, &foreign))
                .expect("foreign token fixture");
            let DbResult::EntityAccessFinished { result, .. } = harness.ask(
                DbCommand::EntityAccess {
                    target: boundary.clone(),
                    action: Access::Load { peer_dids: vec![] },
                    request_id: "access-hyphen-boundary-load".into(),
                },
                |r| matches!(r, DbResult::EntityAccessFinished { .. }),
            ) else {
                unreachable!()
            };
            let loaded = result.expect("boundary target loads");
            assert!(
                loaded.tokens.iter().any(|row| row.jti == token.jti),
                "Real descendants remain visible"
            );
            assert!(
                loaded.tokens.iter().all(|row| row.jti != foreign.jti),
                "Foreign or malformed scope must not expose token metadata: {} / {}",
                boundary.scope,
                foreign.scope
            );
            let DbResult::EntityAccessFinished { result, .. } = harness.ask(
                DbCommand::EntityAccess {
                    target: boundary.clone(),
                    action: Access::RevokeToken {
                        jti: foreign.jti.clone(),
                        sub: foreign.sub.clone(),
                        scope: foreign.scope.clone(),
                    },
                    request_id: "access-hyphen-boundary-revoke".into(),
                },
                |r| matches!(r, DbResult::EntityAccessFinished { .. }),
            ) else {
                unreachable!()
            };
            assert!(
                result.is_err(),
                "Foreign or malformed token is outside the parsed target components"
            );
            assert!(
                !rt.block_on(fe_database::api_token_store::is_token_revoked(
                    &db,
                    &foreign.jti
                ))
                .unwrap(),
                "Refused foreign token remains active"
            );
        }
    }

    assert!(access("access-token-wrong-sub",Access::RevokeToken {jti:token.jti.clone(),sub:"wrong".into(),scope:token.scope.clone()}).is_err());
    let refreshed=access("access-token-still-present",Access::Load {peer_dids:vec![]}).unwrap();assert!(refreshed.tokens.iter().any(|row|row.jti==token.jti));
    let revoked=access("access-token-revoke",Access::RevokeToken {jti:token.jti.clone(),sub:token.sub.clone(),scope:token.scope.clone()}).expect("exact target revoke");
    assert!(revoked.tokens.iter().all(|row|row.jti!=token.jti));
    rt.block_on(async {db.query("UPDATE petal SET fractal_id='moved' WHERE petal_id=$id").bind(("id",target.entity_id.clone())).await.unwrap().check().unwrap();});
    assert!(access("access-stale-parent",Access::Load {peer_dids:vec![]}).is_err(),"Stale target cannot read a reparented branch");
}

/// AC-42, T23: a revocation published while the verse's replica was closed (the event never
/// reached a peer) is re-emitted — newest version only — at the next DB startup, and again
/// when the replica opens.
#[test]
fn a_revocation_issued_while_the_replica_is_closed_reaches_peers_after_restart() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("tempdir");
    let (verse, scope, revocation) = {
        let harness = Harness::open(dir.path(), 64);
        let (verse, scope) = harness.create_hierarchy();
        let petal = fe_database::parse_scope(&scope).unwrap().petal_id.unwrap();
        let emitted: Vec<_> = harness.repl_rx.try_iter().collect();
        let created = emitted
            .iter()
            .find(|event| event.table == "petal")
            .expect("committed Petal is published");
        assert_eq!(created.record_id, petal);
        assert_eq!(created.verse_id, verse);
        let row: serde_json::Value =
            serde_json::from_slice(&harness.blobs.bytes_for(&created.content_hash).unwrap())
                .unwrap();
        assert_eq!(row["petal_id"], petal);
        assert!(row.get("id").is_none() && row.get("view_hidden").is_none());
        harness.assign(&scope, "editor");
        harness.revoke(&scope);
        assert_eq!(harness.peer_role(&scope), "viewer");
        // Replica closed: these publishes never reach a peer.
        let lost = harness.role_statements();
        assert_eq!(lost.len(), 2, "the grant then the revocation: {lost:?}");
        let revocation = lost[1].1.clone();
        assert!(revocation.revoked);
        (verse, scope, revocation)
    }; // DB thread joined, store closed

    let harness = Harness::open(dir.path(), 64);
    let expected_key = format!("{scope}/{PEER}/{}", local_did());
    let reemitted = harness.role_statements();
    assert_eq!(
        reemitted.len(),
        1,
        "one live version per key: {reemitted:?}"
    );
    assert_eq!(reemitted[0].0.record_id, expected_key);
    assert_eq!(reemitted[0].0.verse_id, verse);
    assert_eq!(
        reemitted[0].1, revocation,
        "the startup re-emit carries the newest statement: the revocation, never the grant"
    );
    assert_eq!(harness.peer_role(&scope), "viewer");

    // The replica-open signal from the sync thread re-emits it once more.
    harness.send(DbCommand::ReemitLocalRoleStatements {
        verse_id: verse.clone(),
    });
    let deadline = std::time::Instant::now() + TIMEOUT;
    let on_open = loop {
        let found = harness.role_statements();
        if !found.is_empty() || std::time::Instant::now() > deadline {
            break found;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(on_open.len(), 1);
    assert_eq!(on_open[0].1, revocation);

    harness.send(DbCommand::ReemitLocalVerseCatalog {
        verse_id: verse.clone(),
    });
    harness.ask(DbCommand::LoadHierarchy, |result| {
        matches!(result, DbResult::HierarchyLoaded { .. })
    });
    let catalog: Vec<_> = harness.repl_rx.try_iter().collect();
    assert_eq!(
        catalog
            .iter()
            .map(|event| event.table.as_str())
            .collect::<Vec<_>>(),
        vec!["verse", "fractal", "petal"],
        "startup repair restores ancestry order without ingest"
    );
    for event in &catalog {
        assert_eq!(event.verse_id, verse);
        let row: serde_json::Value =
            serde_json::from_slice(&harness.blobs.bytes_for(&event.content_hash).unwrap()).unwrap();
        assert!(row.get("id").is_none() && row.get("view_hidden").is_none());
    }
    let db = harness.db();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        db.query("UPDATE verse SET created_by='did:key:foreign-owner', default_access='viewer' WHERE verse_id=$verse")
            .bind(("verse", verse.clone())).await.unwrap().check().unwrap();
    });
    harness.send(DbCommand::ReemitLocalVerseCatalog { verse_id: verse });
    harness.ask(DbCommand::LoadHierarchy, |result| {
        matches!(result, DbResult::HierarchyLoaded { .. })
    });
    assert!(
        harness.repl_rx.try_recv().is_err(),
        "Viewer startup must not sign or publish catalog rows"
    );
}

/// AC-42, T23: a revocation whose publish was dropped by a flooded replication path is
/// recovered by the replica-open re-emit.
#[test]
fn a_pending_writes_flood_cannot_drop_a_local_revocation() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Harness::open(dir.path(), 4);
    let (verse, scope) = harness.create_hierarchy(); // verse + fractal manifests: 2 events
    harness.create_fractal(&verse);
    harness.create_fractal(&verse); // the bridge is now full

    let dropped_before = fe_database::replication_drop_count();
    harness.assign(&scope, "editor");
    harness.revoke(&scope);
    assert!(
        fe_database::replication_drop_count() >= dropped_before + 2,
        "both role publishes were dropped by the flood"
    );
    assert!(
        harness.role_statements().is_empty(),
        "nothing about the role reached the bridge"
    );

    harness.send(DbCommand::ReemitLocalRoleStatements {
        verse_id: verse.clone(),
    });
    let deadline = std::time::Instant::now() + TIMEOUT;
    let recovered = loop {
        let found = harness.role_statements();
        if !found.is_empty() || std::time::Instant::now() > deadline {
            break found;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(
        recovered[0].0.record_id,
        format!("{scope}/{PEER}/{}", local_did())
    );
    assert!(
        recovered[0].1.revoked,
        "the revocation, not the superseded grant"
    );
    assert_eq!(recovered[0].0.verse_id, verse);
}

/// AC-45, T18: hex-keyed interim authority rows (as the canon_log fixtures seed them) are
/// byte-identical after the role migration and a rematerialization, across restarts, and
/// still answer `DurableAuthorizationView`'s Manager+ question.
#[test]
fn interim_authority_rows_survive_the_role_migration_and_rematerialize_across_a_restart() {
    let _serial = serial();
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = runtime();
    let interim_scope = Scope::verse_wide(Identifier32([0x41; 32]));
    let interim_key = interim_authority_scope_key(&interim_scope);
    let manager = Principal::from_public_key([0x11; 32]);
    let owner_principal = Principal::from_public_key([0x12; 32]);
    let legacy_verse_scope = build_scope("verse-legacy", None, None);

    let snapshot = |db: &Db| -> Vec<serde_json::Value> {
        rt.block_on(async {
            db.query(
                "SELECT * FROM role WHERE string::starts_with(scope, 'VERSE#4141') ORDER BY id",
            )
            .await
            .expect("select interim")
            .take(0)
            .expect("rows")
        })
    };

    let seeded = {
        let harness = Harness::open(dir.path(), 64);
        let db = harness.db();
        rt.block_on(async {
            db.query(
                "CREATE verse CONTENT { verse_id: 'verse-legacy', name: 'V', created_by: $owner, \
                 created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' }; \
                 CREATE role CONTENT { peer_did: $manager, scope: $interim, role: 'manager' }; \
                 CREATE role CONTENT { peer_did: $owner_principal, scope: $interim, role: 'owner' }; \
                 CREATE role CONTENT { peer_did: $peer, scope: $legacy, role: 'owner' }; \
                 CREATE role CONTENT { peer_did: $manager, scope: $legacy, role: 'editor' }; \
                 DELETE local_migration_marker;",
            )
            .bind(("owner", local_did()))
            .bind(("manager", manager.did.clone()))
            .bind(("owner_principal", owner_principal.did.clone()))
            .bind(("peer", PEER))
            .bind(("interim", interim_key.clone()))
            .bind(("legacy", legacy_verse_scope.clone()))
            .await
            .expect("seed legacy store")
            .check()
            .expect("seed check");
        });
        let seeded = snapshot(&db);
        assert_eq!(seeded.len(), 2, "{seeded:?}");
        seeded
    }; // a legacy, never-migrated store

    for restart in 0..2 {
        let harness = Harness::open(dir.path(), 64);
        let db = harness.db();
        assert_eq!(
            snapshot(&db),
            seeded,
            "restart {restart}: interim rows byte-identical"
        );
        let view = rt
            .block_on(DurableAuthorizationView::load(&db, Some(ANCHOR)))
            .expect("load view");
        assert_eq!(
            view.authority_is_manager_plus(&ANCHOR, &manager, &interim_scope, 0),
            AuthorityState::ManagerPlus,
            "restart {restart}"
        );
        // The legacy hierarchy rows did migrate: the implicit owner row is gone, and the
        // owner-local explicit row is an owner statement in the rebuilt cache.
        let legacy: Vec<serde_json::Value> = rt.block_on(async {
            db.query("SELECT peer_did, role FROM role WHERE scope = $s")
                .bind(("s", legacy_verse_scope.clone()))
                .await
                .expect("select legacy")
                .take(0)
                .expect("rows")
        });
        assert_eq!(
            legacy,
            vec![serde_json::json!({ "peer_did": manager.did, "role": "editor" })],
            "restart {restart}"
        );
        let statements = rt
            .block_on(fe_database::role_grants::load_admitted(&db, "verse-legacy"))
            .expect("statements");
        assert_eq!(statements.len(), 1, "restart {restart}: {statements:?}");
    }
}
