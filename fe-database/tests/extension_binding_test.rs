//! Scoped extension bindings on the DB plane (docs/extensions/bindings.md §4), driven through the public
//! seams over an in-memory store: the local write path and its refusals, inbound admission, last-writer-wins
//! across authors, demotion withdraw, a re-grant that stays out, and Keep binding.
//! See fe-database/src/AGENTS.md §extension-binding.

use std::sync::{Arc, Once};

use fe_database::handlers::extension_binding::{self, binding_record_id};
use fe_database::handlers::extension_policy::{self, AllowEntry, ExtensionPolicy, PolicyMode};
use fe_database::handlers::inbound_admissions::InboundAdmissions;
use fe_database::repo::Db;
use fe_database::role_manager;
use fe_database::{build_scope, BlobStoreHandle, ReplicationEvent};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::extension_binding::{BindingMode, BindingRecord};
use fe_runtime::messages::{ReplicatedRowOutcome, StoredBinding};

const OWNER: &str = "did:key:z6MkBindingOwner";
/// This host's identity: a verse Manager.
const LOCAL: &str = "did:key:z6MkBindingHost";
/// A fractal Manager writing on another peer.
const REMOTE: &str = "did:key:z6MkBindingFractalManager";
const VIEWER: &str = "did:key:z6MkBindingViewer";
const VERSE: &str = "verse-bnd";
const FRACTAL: &str = "fractal-bnd";
const PETAL: &str = "petal-bnd";
const BI_PUSH: &str = "org.example.bi-push";

static HLC: Once = Once::new();

struct Fixture {
    db: Db,
    blobs: Arc<MockBlobStore>,
    store: BlobStoreHandle,
    repl_tx: crossbeam::channel::Sender<ReplicationEvent>,
    repl_rx: crossbeam::channel::Receiver<ReplicationEvent>,
}

async fn fixture() -> Fixture {
    // The HLC is process-global and this binary has no DB thread: one init is enough.
    HLC.call_once(|| fe_database::op_log::init_hlc(0));
    let db: Db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
        .await
        .expect("mem db");
    db.use_ns("test").use_db("test").await.expect("ns/db");
    fe_database::schema::apply_all(&db).await.expect("schema");
    db.query(format!(
        "CREATE verse CONTENT {{ verse_id: '{VERSE}', name: 'V', created_by: '{OWNER}', \
         created_at: '2026-11-09T00:00:00Z', default_access: 'viewer' }}; \
         CREATE fractal CONTENT {{ fractal_id: '{FRACTAL}', verse_id: '{VERSE}', owner_did: '{OWNER}', \
         name: 'F', created_at: '2026-11-09T00:00:00Z' }}; \
         CREATE petal CONTENT {{ petal_id: '{PETAL}', fractal_id: '{FRACTAL}', name: 'P', node_id: '{OWNER}', \
         created_at: '2026-11-09T00:00:00Z' }};"
    ))
    .await
    .expect("hierarchy")
    .check()
    .expect("hierarchy check");
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
    async fn put(
        &self,
        caller: Option<&str>,
        template: BindingRecord,
    ) -> Result<BindingRecord, extension_binding::BindingCommandError> {
        extension_binding::put_binding(
            &self.db,
            LOCAL,
            caller,
            template,
            Some(&self.store),
            Some(&self.repl_tx),
        )
        .await
    }

    async fn versions(&self, scope: &str) -> Vec<StoredBinding> {
        extension_binding::list_bindings(&self.db, scope, None)
            .await
            .expect("list bindings")
    }

    /// Binding rows published so far, decoded from their blobs; role statements are skipped.
    fn published(&self) -> Vec<(String, BindingRecord)> {
        self.repl_rx
            .try_iter()
            .filter(|event| event.table == extension_binding::BINDING_DOC_TABLE)
            .map(|event| {
                let bytes = self
                    .blobs
                    .bytes_for(&event.content_hash)
                    .expect("published blob");
                (
                    event.record_id,
                    serde_json::from_slice(&bytes).expect("published binding"),
                )
            })
            .collect()
    }
}

fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

/// A verse egress connector `warehouse` bound to `package_id` at `scope` (bindings.md §8.4, a one-peer hook).
fn connector(scope: &str, package_id: &str, config: serde_json::Value) -> BindingRecord {
    BindingRecord {
        schema_version: 1,
        scope: scope.to_string(),
        hook: "egress.connector".into(),
        key: "warehouse".into(),
        mode: BindingMode::Extension,
        package_id: Some(package_id.into()),
        version_req: Some(">=0.1.0, <0.2.0".into()),
        config,
        runs_on: None,
        author_did: String::new(),
        written_at: String::new(),
    }
}

/// Deliver `record` over the inbound path as `attested` would publish it.
async fn deliver(
    db: &Db,
    queue: &mut InboundAdmissions,
    record: &BindingRecord,
    attested: &str,
) -> ReplicatedRowOutcome {
    let record_id = binding_record_id(record).expect("record id");
    let bytes = serde_json::to_vec(record).expect("binding json");
    queue
        .apply(
            db,
            VERSE,
            extension_binding::BINDING_DOC_TABLE,
            &record_id,
            &bytes,
            attested,
            None,
        )
        .await[0]
        .outcome
        .clone()
}

/// `(author, key, config, runs_on)` of every effective version, sorted by key.
fn effective(
    versions: &[StoredBinding],
) -> Vec<(String, String, serde_json::Value, Option<String>)> {
    let mut rows: Vec<_> = versions
        .iter()
        .filter(|version| version.effective)
        .map(|version| {
            let record = &version.record;
            (
                record.author_did.clone(),
                record.key.clone(),
                record.config.clone(),
                record.runs_on.clone(),
            )
        })
        .collect();
    rows.sort_by_key(|row| row.1.clone());
    rows
}

fn version_of<'a>(versions: &'a [StoredBinding], author: &str, key: &str) -> &'a StoredBinding {
    versions
        .iter()
        .find(|version| version.record.author_did == author && version.record.key == key)
        .unwrap_or_else(|| panic!("no version by {author} at {key}: {versions:?}"))
}

/// One fractal point through its whole life: refused writes store nothing; a local write and a newer remote
/// version resolve last-writer-wins across authors; demoting the remote author withdraws its version (the
/// older one applies again, nothing is deleted); re-granting it does not revive that version nor a version
/// backdated into its old window; Keep binding re-authors it locally with its placement pinned.
#[tokio::test]
async fn a_bindings_versions_follow_their_authors_role_at_write_and_now() {
    let fx = fixture().await;
    let verse = build_scope(VERSE, None, None);
    let fractal = build_scope(VERSE, Some(FRACTAL), None);
    role_manager::assign_role_checked(
        &fx.db,
        OWNER,
        LOCAL,
        &verse,
        "manager",
        Some(&fx.repl_tx),
        &fx.store,
    )
    .await
    .expect("owner appoints the host a verse Manager");
    role_manager::assign_role_checked(
        &fx.db,
        OWNER,
        REMOTE,
        &fractal,
        "manager",
        Some(&fx.repl_tx),
        &fx.store,
    )
    .await
    .expect("owner appoints a fractal Manager");
    let policy = ExtensionPolicy {
        mode: PolicyMode::AllowList,
        allow: vec![AllowEntry {
            package_id: BI_PUSH.into(),
            content_hash: None,
            versions: Some(">=0.1.0".into()),
        }],
        deny: vec!["org.example.blocked".into()],
        ..Default::default()
    };
    extension_policy::set_policy_with_replication(&fx.db, OWNER, VERSE, policy, None, None)
        .await
        .expect("verse policy");

    // §4.2: shape (422), scope (404), role (403) and verse policy (403) refuse before anything is written.
    let refusals = [
        (
            "viewer caller",
            Some(VIEWER),
            connector(&fractal, BI_PUSH, serde_json::json!({})),
            "role-denied",
        ),
        (
            "unknown fractal",
            None,
            connector(
                &build_scope(VERSE, Some("fractal-missing"), None),
                BI_PUSH,
                serde_json::json!({}),
            ),
            "not-found",
        ),
        (
            "stretch hook",
            None,
            BindingRecord {
                hook: "asset.export".into(),
                ..connector(&fractal, BI_PUSH, serde_json::json!({}))
            },
            "unknown-hook",
        ),
        (
            "keyless hook given a key",
            None,
            BindingRecord {
                hook: "petal.map".into(),
                ..connector(&fractal, BI_PUSH, serde_json::json!({}))
            },
            "invalid-binding",
        ),
        (
            "deny-listed package",
            None,
            connector(&fractal, "org.example.blocked", serde_json::json!({})),
            "policy-denied",
        ),
        (
            "package off the allow-list",
            None,
            connector(&fractal, "org.example.unlisted", serde_json::json!({})),
            "policy-denied",
        ),
    ];
    for (case, caller, template, code) in refusals {
        let error = fx.put(caller, template).await.expect_err(case);
        assert_eq!(error.code(), code, "{case}: {error}");
    }
    assert!(
        fx.published().is_empty(),
        "a refused write publishes nothing"
    );
    assert!(
        fx.versions(&fractal).await.is_empty(),
        "a refused write stores nothing"
    );

    // The host binds; the row is published under its doc key with the op's own clock.
    let local = fx
        .put(
            None,
            connector(&fractal, BI_PUSH, serde_json::json!({ "format": "json" })),
        )
        .await
        .expect("local write");
    assert_eq!(
        fx.published(),
        vec![(binding_record_id(&local).expect("id"), local.clone())]
    );

    // A newer version by the fractal Manager replicates in and wins; the host's version stays stored and admissible.
    let local_hlc: u64 = local.written_at.parse().expect("stamped clock");
    let remote = BindingRecord {
        author_did: REMOTE.into(),
        written_at: (local_hlc + (1_000 << 16)).to_string(),
        config: serde_json::json!({ "format": "csv" }),
        ..local.clone()
    };
    let mut queue = InboundAdmissions::new();
    assert_eq!(
        deliver(&fx.db, &mut queue, &remote, REMOTE).await,
        ReplicatedRowOutcome::Applied
    );
    let versions = fx.versions(&fractal).await;
    assert_eq!(
        effective(&versions),
        vec![(
            REMOTE.to_string(),
            "warehouse".to_string(),
            serde_json::json!({ "format": "csv" }),
            None
        )]
    );
    assert!(version_of(&versions, LOCAL, "warehouse").admissible);

    // §4.3: structural failures are Denied, never stored, whatever the author's role.
    let foreign = build_scope("verse-elsewhere", Some(FRACTAL), None);
    let denied = [
        (
            "payload author is not the attested author",
            remote.clone(),
            OWNER,
        ),
        (
            "scope in another verse",
            BindingRecord {
                scope: foreign,
                ..remote.clone()
            },
            REMOTE,
        ),
        (
            "extension without a package",
            BindingRecord {
                package_id: None,
                ..remote.clone()
            },
            REMOTE,
        ),
        (
            "clock past the five-minute bound",
            BindingRecord {
                written_at: ((wall_ms() + 600_000) << 16).to_string(),
                ..remote.clone()
            },
            REMOTE,
        ),
    ];
    for (case, record, attested) in denied {
        assert_eq!(
            deliver(&fx.db, &mut queue, &record, attested).await,
            ReplicatedRowOutcome::Denied,
            "{case}"
        );
    }
    assert_eq!(
        fx.versions(&fractal).await.len(),
        2,
        "denied rows are not stored"
    );

    // §4.5 demotion: the remote version is withdrawn on read (still stored); the host's applies again.
    role_manager::revoke_role_checked(
        &fx.db,
        OWNER,
        REMOTE,
        &fractal,
        Some(&fx.repl_tx),
        &fx.store,
    )
    .await
    .expect("owner demotes the fractal Manager");
    let versions = fx.versions(&fractal).await;
    assert_eq!(versions.len(), 2, "demotion deletes nothing");
    assert!(!version_of(&versions, REMOTE, "warehouse").admissible);
    assert_eq!(
        effective(&versions),
        vec![(
            LOCAL.to_string(),
            "warehouse".to_string(),
            serde_json::json!({ "format": "json" }),
            None
        )]
    );

    // Re-grant: the old version stays out, and so does a new one backdated into the old window.
    role_manager::assign_role_checked(
        &fx.db,
        OWNER,
        REMOTE,
        &fractal,
        "manager",
        Some(&fx.repl_tx),
        &fx.store,
    )
    .await
    .expect("owner re-grants");
    let backdated = BindingRecord {
        key: "lake".into(),
        ..remote.clone()
    };
    assert_eq!(
        deliver(&fx.db, &mut queue, &backdated, REMOTE).await,
        ReplicatedRowOutcome::Applied,
        "stored: its author is Manager now"
    );
    let versions = fx.versions(&fractal).await;
    for key in ["warehouse", "lake"] {
        let version = version_of(&versions, REMOTE, key);
        assert!(
            !version.written_authorized && !version.admissible,
            "{key}: written outside the re-granted window"
        );
    }
    assert_eq!(
        effective(&versions).len(),
        1,
        "only the host's warehouse version is effective"
    );

    // Keep binding: the host re-authors the withdrawn version; a one-peer hook keeps running on REMOTE's peer.
    let kept = extension_binding::keep_binding(
        &fx.db,
        LOCAL,
        None,
        &binding_record_id(&remote).expect("id"),
        Some(&fx.store),
        Some(&fx.repl_tx),
    )
    .await
    .expect("keep binding");
    assert_eq!(
        (kept.author_did.as_str(), kept.runs_on.as_deref()),
        (LOCAL, Some(REMOTE))
    );
    assert_eq!(
        fx.published()
            .into_iter()
            .map(|(_, record)| record)
            .collect::<Vec<_>>(),
        vec![kept.clone()]
    );

    // The resolver's snapshot: judged versions, current roles, and a petal's own terrain as an implicit builtin (OQ-B6).
    fx.db
        .query(format!(
            "UPDATE petal SET terrain = {{ tileset_hexon_uris: [] }} WHERE petal_id = '{PETAL}'"
        ))
        .await
        .expect("petal terrain")
        .check()
        .expect("petal terrain check");
    let plane = extension_binding::load_binding_snapshot(&fx.db, VERSE)
        .await
        .expect("binding plane");
    assert!(
        plane.roles.is_some(),
        "the manifest is known, so roles are materialized"
    );
    assert_eq!(
        plane.stored_terrain_petals,
        std::collections::BTreeSet::from([PETAL.to_string()])
    );
    assert_eq!(
        effective(&plane.versions),
        vec![(
            LOCAL.to_string(),
            "warehouse".to_string(),
            serde_json::json!({ "format": "csv" }),
            Some(REMOTE.to_string())
        )],
        "the kept version replaced the host's own and is the point's effective version"
    );
}
