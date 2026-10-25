//! role_propagation RP-4 step 8 (critic tick 8 AC-46, T26): an API token's role is capped at
//! the node's own converged role on every replicated-write door —
//! `min(token max_role, role_at(local_did, scope))` — so a token minted while the node held
//! Manager cannot keep writing rows every peer would defer once the owner demotes the node.
//! Driven through the real DB-thread dispatch; the demotion arrives as the owner's replicated
//! revocation. See fe-database/src/AGENTS.md §handlers "replicated_row".

use std::sync::Arc;
use std::time::Duration;

use fe_database::{build_scope, spawn_db_thread_shared, DbThread};
use fe_policy::RoleStatementPayload;
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::blob_store::{hash_to_hex, BlobStore};
use fe_runtime::messages::{CallerAuth, DbCommand, DbResult, ReplicatedRowOutcome};
use fe_sdk::device_format::DeviceFormatDescriptor;

const VERSE: &str = "verse-api-cap";
const FRACTAL: &str = "fractal-api-cap";
const PETAL: &str = "petal-api-cap";
const TIMEOUT: Duration = Duration::from_secs(20);

/// A DB thread over a store directory the test owns; `Drop` closes AND joins it (C8: one
/// SurrealKV open per process).
struct Node {
    cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    res_rx: crossbeam::channel::Receiver<DbResult>,
    db: Option<fe_database::repo::Db>,
    join: Option<std::thread::JoinHandle<()>>,
    blobs: Arc<MockBlobStore>,
}

impl Node {
    fn open(dir: &std::path::Path, keypair: fe_identity::NodeKeypair) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<DbCommand>(64);
        let (res_tx, res_rx) = crossbeam::channel::bounded::<DbResult>(256);
        let blobs = Arc::new(MockBlobStore::new());
        let DbThread {
            join_handle,
            shared_db,
        } = spawn_db_thread_shared(
            cmd_rx,
            res_tx,
            blobs.clone(),
            None,
            Some(keypair),
            None,
            None,
            Some(dir.join("fe.db").to_string_lossy().to_string()),
            None,
        )
        .expect("spawn db thread");
        let node = Self {
            cmd_tx: Some(cmd_tx),
            res_rx,
            db: Some(shared_db),
            join: Some(join_handle),
            blobs,
        };
        node.until(|r| matches!(r, DbResult::Started));
        node
    }

    fn send(&self, cmd: DbCommand) {
        self.cmd_tx.as_ref().expect("open").send(cmd).expect("send");
    }

    /// The first result matching `wanted` (others — scene echoes, extra row reports — skipped).
    fn until(&self, wanted: impl Fn(&DbResult) -> bool) -> DbResult {
        loop {
            let result = self.res_rx.recv_timeout(TIMEOUT).expect("DB reply in time");
            if wanted(&result) {
                return result;
            }
        }
    }

    fn replicate_in(&self, table: &str, record_id: &str, row: serde_json::Value, author: &str) {
        self.send(DbCommand::ApplyReplicatedRow {
            verse_id: VERSE.to_string(),
            table: table.to_string(),
            record_id: record_id.to_string(),
            row_bytes: serde_json::to_vec(&row).expect("row json"),
            author_did: author.to_string(),
        });
        let outcome = match self.until(|r| {
            matches!(r, DbResult::ReplicatedRowApplied { record_id: got, .. } if got == record_id)
        }) {
            DbResult::ReplicatedRowApplied { outcome, .. } => outcome,
            _ => unreachable!(),
        };
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Applied,
            "{table}/{record_id}"
        );
    }

    fn set_terrain(&self, terrain: serde_json::Value, correlation: &str) -> DbResult {
        self.send(DbCommand::SetPetalTerrain {
            petal_id: PETAL.to_string(),
            terrain: Some(terrain),
            auth: manager_token(),
            correlation_id: Some(correlation.to_string()),
        });
        self.until(|r| match r {
            DbResult::PetalTerrainLoaded { correlation_id, .. } => {
                correlation_id.as_deref() == Some(correlation)
            }
            DbResult::PetalTerrainSetFailed { correlation_id, .. } => correlation_id == correlation,
            _ => false,
        })
    }

    fn register_format(&self, format_id: &str, correlation: &str) -> DbResult {
        self.send(DbCommand::RegisterDeviceFormat {
            verse_id: VERSE.to_string(),
            descriptor: DeviceFormatDescriptor {
                format_id: format_id.to_string(),
                mime: "application/octet-stream".to_string(),
                schema: None,
                description: String::new(),
            },
            declared_by: "api".to_string(),
            correlation_id: Some(correlation.to_string()),
        });
        self.until(|r| match r {
            DbResult::DeviceFormatRegistered { correlation_id, .. }
            | DbResult::DeviceFormatRejected { correlation_id, .. } => {
                correlation_id.as_deref() == Some(correlation)
            }
            DbResult::RequestFailed { correlation_id, .. } => correlation_id == correlation,
            _ => false,
        })
    }
}

impl Node {
    /// The metadata-only `CreateAsset` fe-api's upload doors send after storing the bytes
    /// (TC-7): anchored at the test petal, attributed to a client owner key.
    fn create_asset(&self, bytes: &[u8], correlation: &str) -> DbResult {
        let hash = self.blobs.add_blob(bytes).expect("blob stored API-side");
        self.send(DbCommand::CreateAsset {
            name: format!("asset-{correlation}"),
            content_type: "model/gltf-binary".to_string(),
            size_bytes: bytes.len() as u64,
            content_hash: hash_to_hex(&hash),
            owner_did: Some("client-a".to_string()),
            petal_id: Some(PETAL.to_string()),
            correlation_id: Some(correlation.to_string()),
        });
        self.until(|r| match r {
            DbResult::AssetCreated { correlation_id, .. } => {
                correlation_id.as_deref() == Some(correlation)
            }
            DbResult::RequestFailed { correlation_id, .. } => correlation_id == correlation,
            _ => false,
        })
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

/// What fe-api hands the DB thread for a token minted with `max_role = manager`.
fn manager_token() -> CallerAuth {
    CallerAuth::Identified {
        did: "api".to_string(),
        role: "manager".to_string(),
    }
}

fn statement(
    owner: &str,
    peer: &str,
    role: &str,
    revoked: bool,
    hlc: u64,
) -> (String, serde_json::Value) {
    let scope = build_scope(VERSE, None, None);
    let payload = RoleStatementPayload {
        scope: scope.clone(),
        peer_did: peer.to_string(),
        grantor_did: owner.to_string(),
        role: role.to_string(),
        revoked,
        hlc,
    };
    (
        format!("{scope}/{peer}/{owner}"),
        serde_json::to_value(payload).expect("payload json"),
    )
}

#[test]
fn an_api_token_minted_before_the_nodes_demotion_cannot_write_after_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = fe_identity::NodeKeypair::generate().to_did_key();
    let keypair = fe_identity::NodeKeypair::generate();
    let local = keypair.to_did_key();
    let node = Node::open(dir.path(), keypair);

    // The verse is the remote owner's; the owner's statement makes this node a verse Manager.
    node.replicate_in(
        "verse",
        VERSE,
        serde_json::json!({
            "verse_id": VERSE, "name": "V", "created_by": owner,
            "created_at": "2026-10-15T00:00:00Z", "default_access": "viewer",
        }),
        &owner,
    );
    node.replicate_in(
        "fractal",
        FRACTAL,
        serde_json::json!({
            "fractal_id": FRACTAL, "verse_id": VERSE, "owner_did": owner,
            "name": "F", "created_at": "2026-10-15T00:00:00Z",
        }),
        &owner,
    );
    node.replicate_in(
        "petal",
        PETAL,
        serde_json::json!({
            "petal_id": PETAL, "fractal_id": FRACTAL, "name": "P",
            "node_id": owner, "created_at": "2026-10-15T00:00:00Z",
        }),
        &owner,
    );
    let (grant_key, grant) = statement(&owner, &local, "manager", false, 1);
    node.replicate_in("role", &grant_key, grant, &owner);

    // While the node is a Manager the manager-role token writes.
    let first = serde_json::json!({ "tileset_hexon_uris": ["tileset-before"] });
    assert!(
        matches!(
            node.set_terrain(first.clone(), "before"),
            DbResult::PetalTerrainLoaded { .. }
        ),
        "the token writes while the node holds Manager"
    );
    assert!(matches!(
        node.register_format("acme.thermo/before@1", "format-before"),
        DbResult::DeviceFormatRegistered { .. }
    ));
    assert!(
        matches!(
            node.create_asset(b"glTF-before", "asset-before"),
            DbResult::AssetCreated { .. }
        ),
        "an anchored upload registers while the node holds Editor+ at the petal"
    );

    // The owner demotes the node (a replicated revocation of the owner's own grant).
    let (revoke_key, revoke) = statement(&owner, &local, "none", true, 2);
    assert_eq!(revoke_key, grant_key, "one live version per key");
    node.replicate_in("role", &revoke_key, revoke, &owner);

    // The same token now fails every replicated-write door it tries.
    let refused = node.set_terrain(
        serde_json::json!({ "tileset_hexon_uris": ["tileset-after"] }),
        "after",
    );
    assert!(
        matches!(refused, DbResult::PetalTerrainSetFailed { .. }),
        "a stale token must not outrun the node's demotion: {refused:?}"
    );
    let refused = node.register_format("acme.thermo/after@1", "format-after");
    assert!(
        matches!(refused, DbResult::RequestFailed { .. }),
        "the role-less door is capped too: {refused:?}"
    );
    // TC-7 / W6-0: the upload door is capped by the same helper (anchored asset rows replicate).
    let refused = node.create_asset(b"glTF-after", "asset-after");
    assert!(
        matches!(refused, DbResult::RequestFailed { .. }),
        "a stale token's upload must not register an anchored asset: {refused:?}"
    );

    // READ-BACK: nothing the stale token sent landed.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let db = node.db.clone().expect("open store");
    let terrain: Vec<serde_json::Value> = rt
        .block_on(async {
            db.query("SELECT terrain FROM petal WHERE petal_id = $p")
                .bind(("p", PETAL.to_string()))
                .await
                .expect("select terrain")
                .take(0)
        })
        .expect("rows");
    assert_eq!(
        terrain[0]["terrain"], first,
        "the terrain is the pre-demotion one"
    );
    let formats: Vec<serde_json::Value> = rt
        .block_on(async {
            db.query("SELECT format_id FROM device_format WHERE format_id = 'acme.thermo/after@1'")
                .await
                .expect("select formats")
                .take(0)
        })
        .expect("rows");
    assert!(formats.is_empty(), "no post-demotion format was declared");
    let assets: Vec<serde_json::Value> = rt
        .block_on(async {
            db.query("SELECT name FROM asset WHERE petal_id = $p ORDER BY name")
                .bind(("p", PETAL.to_string()))
                .await
                .expect("select assets")
                .take(0)
        })
        .expect("rows");
    let names: Vec<&str> = assets.iter().filter_map(|a| a["name"].as_str()).collect();
    assert_eq!(
        names,
        vec!["asset-asset-before"],
        "only the pre-demotion upload registered"
    );
}
