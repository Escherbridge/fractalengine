use fe_runtime::messages::{DbCommand, DbResult};
use surrealdb::engine::local::SurrealKv;

/// Error type for database thread initialization failures.
#[derive(Debug)]
pub enum DbInitError {
    RuntimeBuild(std::io::Error),
    SurrealOpen { path: String, source: surrealdb::Error },
    SurrealNsDb(surrealdb::Error),
    SchemaApply(String),
    ApiTokenSchemaApply(String),
}

impl std::fmt::Display for DbInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbInitError::RuntimeBuild(e) => write!(f, "Failed to build Tokio runtime: {e}"),
            DbInitError::SurrealOpen { path, source } => {
                write!(f, "Failed to open SurrealDB at {path}: {source}")
            }
            DbInitError::SurrealNsDb(e) => write!(f, "Failed to set SurrealDB namespace/database: {e}"),
            DbInitError::SchemaApply(e) => write!(f, "Schema application failed: {e}"),
            DbInitError::ApiTokenSchemaApply(e) => write!(f, "API token schema application failed: {e}"),
        }
    }
}

impl std::error::Error for DbInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbInitError::RuntimeBuild(e) => Some(e),
            DbInitError::SurrealOpen { source, .. } => Some(source),
            DbInitError::SurrealNsDb(e) => Some(e),
            _ => None,
        }
    }
}

// P2P Mycelium Phase A: re-export blob store types so callers (fe-ui, the
// binary crate, and tests) can depend only on `fe-database` for the full
// asset-pipeline surface.
pub use fe_runtime::blob_store::{
    hash_from_hex, hash_to_hex, BlobHash, BlobStore, BlobStoreHandle,
};

pub mod admin;
pub mod api_token_store;
pub mod atlas;
pub mod handlers;
pub mod invite;
pub mod model_url_meta;
pub mod reconcile;
pub mod op_log;
pub mod queries;
pub mod rbac;
pub mod repo;
pub mod role_level;
pub mod role_manager;
pub mod scope;
pub mod scoped_invite;
pub mod schema;
pub mod space_manager;
pub mod types;
pub mod verse;

pub use atlas::*;
pub use handlers::seed::seed_default_data;
pub use model_url_meta::ModelUrlMeta;
pub use role_level::RoleLevel;
pub use scope::{build_scope, parse_scope, parent_scope, scope_contains, ScopeParts};
pub use types::*;

/// Subdirectory under `fractalengine/assets/` where imported GLTF/GLB files are written.
/// Used by BOTH the write path (`import_gltf_handler`) and the read path
/// (`load_hierarchy_handler`) so the two stay in sync.
pub const IMPORTED_ASSETS_SUBDIR: &str = "imported";

/// Base directory (relative to CWD) where imported assets live on disk.
/// The Bevy asset server loads files relative to `fractalengine/assets/`, so the
/// path passed to `asset_server.load(...)` uses only `IMPORTED_ASSETS_SUBDIR` as the prefix.
pub fn imported_assets_dir() -> std::path::PathBuf {
    std::path::Path::new("fractalengine")
        .join("assets")
        .join(IMPORTED_ASSETS_SUBDIR)
}

#[derive(bevy::prelude::Resource, Clone)]
pub struct DbHandle(pub std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>);

/// Bevy resource wrapping an `Arc<dyn SecretStore>` so ECS systems can
/// access the secret store (e.g. for namespace secret lookups).
#[derive(bevy::prelude::Resource, Clone)]
pub struct SecretStoreRes(pub std::sync::Arc<dyn fe_identity::SecretStore>);

/// A replication event emitted by the DB thread when a replicated row is written.
/// Defined here (not in fe-sync) to avoid a circular dependency.
/// The binary crate bridges these into `SyncCommand::WriteRowEntry`.
#[derive(Debug, Clone)]
pub struct ReplicationEvent {
    pub verse_id: String,
    pub table: String,
    pub record_id: String,
    pub content_hash: BlobHash,
}

/// Sender half for replication events.
pub type ReplicationSender = crossbeam::channel::Sender<ReplicationEvent>;

/// Serialise row data as JSON, write to blob store, and send a
/// `ReplicationEvent` to be bridged to the sync thread.
///
/// # Arguments
/// * `repl_tx` — optional replication sender (None = replication disabled)
/// * `blob_store` — shared blob store for content-addressed row data
/// * `verse_id` — the verse owning this row
/// * `table` — SurrealDB table name
/// * `record_id` — the row's ULID
/// * `row_json` — serialised row as JSON bytes
pub fn replicate_row(
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_json: &[u8],
) {
    let Some(tx) = repl_tx else { return };

    let hash = match blob_store.add_blob(row_json) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!(
                "replicate_row: blob_store.add_blob failed for {table}/{record_id}: {e}"
            );
            return;
        }
    };

    tx.send(ReplicationEvent {
        verse_id: verse_id.to_string(),
        table: table.to_string(),
        record_id: record_id.to_string(),
        content_hash: hash,
    })
    .ok();
}

pub fn spawn_db_thread(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
) -> Result<std::thread::JoinHandle<()>, DbInitError> {
    spawn_db_thread_with_sync(rx, tx, blob_store, None, None, None, None)
}

/// Spawn the DB thread with optional replication, keypair, secret store,
/// and entity-change broadcast for scene streaming.
pub fn spawn_db_thread_with_sync(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
    repl_tx: Option<ReplicationSender>,
    keypair: Option<fe_identity::NodeKeypair>,
    secret_store: Option<std::sync::Arc<dyn fe_identity::SecretStore>>,
    entity_change_tx: Option<tokio::sync::broadcast::Sender<fe_runtime::messages::SceneChange>>,
) -> Result<std::thread::JoinHandle<()>, DbInitError> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_db_thread must not be called from within a Tokio runtime"
    );
    let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel::<Result<(), DbInitError>>(0);
    let handle = std::thread::spawn(move || {
        // The blob store handle is owned by the DB thread so future handlers
        // (Phase B: import refactor, Phase C: migration) can write to it
        // without crossing channel boundaries. Held live for thread lifetime.
        let blob_store: BlobStoreHandle = blob_store;
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = startup_tx.send(Err(DbInitError::RuntimeBuild(e)));
                return;
            }
        };
        rt.block_on(async {
            tracing::info!("Database thread started, initialising SurrealDB");
            let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
                match surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db").await {
                    Ok(db) => db,
                    Err(e) => {
                        let _ = startup_tx.send(Err(DbInitError::SurrealOpen {
                            path: "data/fractalengine.db".to_string(),
                            source: e,
                        }));
                        return;
                    }
                };
            if let Err(e) = db.use_ns("fractalengine").use_db("fractalengine").await {
                let _ = startup_tx.send(Err(DbInitError::SurrealNsDb(e)));
                return;
            }
            if let Err(e) = rbac::apply_schema(&db).await {
                let _ = startup_tx.send(Err(DbInitError::SchemaApply(e.to_string())));
                return;
            }
            if let Err(e) = api_token_store::apply_api_token_schema(&db).await {
                let _ = startup_tx.send(Err(DbInitError::ApiTokenSchemaApply(e.to_string())));
                return;
            }
            let _ = startup_tx.send(Ok(()));

            // Derive the local DID from the node keypair (falls back to
            // "local-node" when no keypair is available).
            let local_did = keypair.as_ref()
                .map(|kp| kp.to_did_key())
                .unwrap_or_else(|| "local-node".to_string());

            // Declarative reconciliation — runs every startup, converges to correct state.
            // Fixes ownership, backfills defaults, no tracking table needed.
            match reconcile::reconcile(&db, &local_did).await {
                Ok(n) if n > 0 => tracing::info!("Reconciliation applied {n} fix(es)"),
                Ok(_) => {}
                Err(e) => tracing::error!("Reconciliation failed: {e}"),
            }

            // Phase C: eagerly migrate any legacy base64 asset rows to the blob store.
            // Migration failure must NOT prevent startup — log and continue.
            match migrate_base64_assets_to_blob_store(&db, &blob_store).await {
                Ok((count, bytes)) => {
                    if count > 0 {
                        tracing::info!(
                            "Migrated {count} base64 assets to blob store ({bytes} bytes)"
                        );
                    }
                }
                Err(e) => tracing::error!("Asset migration failed: {e}"),
            }

            tracing::info!("SurrealDB ready");
            if tx.send(DbResult::Started).is_err() {
                tracing::warn!("Result channel closed — UI may have shut down");
            }
            use handlers::send_result;

            #[allow(clippy::while_let_loop)]
            loop {
                match rx.recv() {
                    Ok(DbCommand::Ping) => {
                        send_result(&tx, DbResult::Pong);
                    }
                    Ok(DbCommand::Seed) => match handlers::seed::seed_default_data(&db, &blob_store, &local_did).await {
                        Ok((petal_name, rooms)) => send_result(&tx, DbResult::Seeded { petal_name, rooms }),
                        Err(e) => {
                            tracing::error!("Seed failed: {e}");
                            send_result(&tx, DbResult::Error(format!("Seed failed: {e}")));
                        }
                    },
                    Ok(DbCommand::CreateVerse { name }) => {
                        match handlers::crud::create_verse_handler(&db, &blob_store, repl_tx.as_ref(), &name, &local_did, secret_store.as_ref()).await {
                            Ok(id) => send_result(&tx, DbResult::VerseCreated { id, name }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create verse failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreateFractal { verse_id, name }) => {
                        match handlers::crud::create_fractal_handler(&db, &blob_store, repl_tx.as_ref(), &verse_id, &name, &local_did).await {
                            Ok(id) => send_result(&tx, DbResult::FractalCreated { id, verse_id, name }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create fractal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreatePetal { fractal_id, name }) => {
                        match handlers::crud::create_petal_handler(&db, &fractal_id, &name, &local_did).await {
                            Ok(id) => send_result(&tx, DbResult::PetalCreated { id, fractal_id, name }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create petal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::CreateNode { petal_id, name, position }) => {
                        match handlers::crud::create_node_handler(&db, &petal_id, &name, position).await {
                            Ok(id) => {
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeAdded {
                                        node: fe_runtime::messages::NodeDto {
                                            node_id: id.clone(),
                                            petal_id: petal_id.clone(),
                                            name: name.clone(),
                                            position,
                                            rotation: [0.0, 0.0, 0.0, 1.0],
                                            scale: [1.0, 1.0, 1.0],
                                            has_asset: false,
                                            asset_path: None,
                                        },
                                    });
                                }
                                send_result(&tx, DbResult::NodeCreated { id, petal_id, name, has_asset: false });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Create node failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ImportGltf { petal_id, name, file_path, position }) => {
                        match handlers::crud::import_gltf_handler(&db, &blob_store, &petal_id, &name, &file_path, position).await {
                            Ok((node_id, asset_id, asset_path)) => send_result(&tx, DbResult::GltfImported { node_id, asset_id, petal_id, name, asset_path, position }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("GLTF import failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::LoadHierarchy) => {
                        match handlers::crud::load_hierarchy_handler(&db, &blob_store).await {
                            Ok(verses) => send_result(&tx, DbResult::HierarchyLoaded { verses }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Load hierarchy failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GenerateVerseInvite { verse_id, include_write_cap, expiry_hours }) => {
                        match handlers::invite::generate_verse_invite_handler(&db, &verse_id, include_write_cap, expiry_hours, keypair.as_ref(), secret_store.as_ref()).await {
                            Ok(invite_string) => send_result(&tx, DbResult::VerseInviteGenerated { verse_id, invite_string }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Generate invite failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::JoinVerseByInvite { invite_string }) => {
                        match handlers::invite::join_verse_by_invite_handler(&db, &invite_string, &local_did, secret_store.as_ref()).await {
                            Ok((verse_id, verse_name)) => send_result(&tx, DbResult::VerseJoined { verse_id, verse_name }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Join verse failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResetDatabase) => {
                        match handlers::admin::reset_database_handler(&db, &blob_store, &local_did).await {
                            Ok((petal_name, rooms)) => send_result(&tx, DbResult::DatabaseReset { petal_name, rooms }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Database reset failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::UpdateNodeTransform { node_id, position, rotation, scale }) => {
                        match handlers::transform::update_node_transform_handler(&db, &node_id, position, rotation, scale).await {
                            Ok(()) => {
                                if let Some(ref ect) = entity_change_tx {
                                    let _ = ect.send(fe_runtime::messages::SceneChange::NodeTransform {
                                        node_id,
                                        position,
                                        rotation,
                                        scale,
                                    });
                                }
                            }
                            Err(e) => {
                                tracing::error!("UpdateNodeTransform failed for {node_id}: {e}");
                                // Emit rollback with last-known-good values from DB
                                if let Some(ref ect) = entity_change_tx {
                                    let rollback = match handlers::crud::get_node_transform_handler(&db, &node_id).await {
                                        Ok(Some((pos, rot, sc))) => {
                                            fe_runtime::messages::SceneChange::TransformFailed {
                                                node_id,
                                                position: pos,
                                                rotation: rot,
                                                scale: sc,
                                            }
                                        }
                                        _ => {
                                            // Can't read old values — send back zeros as a signal
                                            fe_runtime::messages::SceneChange::TransformFailed {
                                                node_id,
                                                position: [0.0; 3],
                                                rotation: [0.0; 3],
                                                scale: [1.0, 1.0, 1.0],
                                            }
                                        }
                                    };
                                    let _ = ect.send(rollback);
                                }
                            }
                        }
                    }
                    Ok(DbCommand::UpdateNodeUrl { node_id, url }) => {
                        if let Err(e) = handlers::transform::update_node_url_handler(&db, &node_id, url).await {
                            tracing::error!("UpdateNodeUrl failed for {node_id}: {e}");
                        }
                    }
                    Ok(DbCommand::RenameEntity { entity_type, entity_id, new_name }) => {
                        match handlers::entity::rename_entity_handler(&db, entity_type, &entity_id, &new_name).await {
                            Ok(()) => send_result(&tx, DbResult::EntityRenamed { entity_type, entity_id, new_name }),
                            Err(e) => send_result(&tx, DbResult::Error(e)),
                        }
                    }
                    Ok(DbCommand::SetVerseDefaultAccess { verse_id, default_access }) => {
                        match crate::role_manager::set_default_access(&db, &verse_id, &default_access).await {
                            Ok(()) => send_result(&tx, DbResult::VerseDefaultAccessSet { verse_id, default_access }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Set default access failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::UpdateFractalDescription { fractal_id, description }) => {
                        match handlers::entity::update_fractal_description_handler(&db, &fractal_id, &description).await {
                            Ok(()) => send_result(&tx, DbResult::FractalDescriptionUpdated { fractal_id, description }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("{e}"))),
                        }
                    }
                    Ok(DbCommand::DeleteEntity { entity_type, entity_id }) => {
                        match handlers::entity::delete_entity_handler(&db, entity_type, &entity_id).await {
                            Ok(()) => {
                                // Emit NodeRemoved for node-level deletes (petal/fractal/verse
                                // cascades are not individually tracked — clients should
                                // re-subscribe after structural deletes).
                                if entity_type == fe_runtime::messages::EntityType::Petal {
                                    // Nodes were cascade-deleted; no individual events.
                                }
                                send_result(&tx, DbResult::EntityDeleted { entity_type, entity_id });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(e)),
                        }
                    }
                    Ok(DbCommand::ResolveRolesForPeers { scope, peer_dids }) => {
                        let roles = handlers::rbac::resolve_roles_for_peers_handler(&db, &scope, &peer_dids).await;
                        send_result(&tx, DbResult::PeerRolesResolved { scope, roles });
                    }
                    Ok(DbCommand::AssignRole { peer_did, scope, role }) => {
                        match handlers::rbac::assign_role_handler(&db, &local_did, &peer_did, &scope, &role).await {
                            Ok(()) => send_result(&tx, DbResult::RoleAssigned { peer_did, scope, role }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Assign role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::RevokeRole { peer_did, scope }) => {
                        match handlers::rbac::revoke_role_handler(&db, &local_did, &peer_did, &scope).await {
                            Ok(()) => send_result(&tx, DbResult::RoleRevoked { peer_did, scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Revoke role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GenerateScopedInvite { scope, role, expiry_hours }) => {
                        let Some(ref kp) = keypair else {
                            send_result(&tx, DbResult::Error("NodeKeypair not available for invite signing".to_string()));
                            continue;
                        };
                        match handlers::rbac::generate_scoped_invite_handler(kp, &local_did, &scope, &role, expiry_hours).await {
                            Ok(invite_link) => send_result(&tx, DbResult::ScopedInviteGenerated { invite_link }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Generate scoped invite failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveLocalRole { scope }) => {
                        match handlers::rbac::resolve_local_role_handler(&db, &local_did, &scope).await {
                            Ok(role) => send_result(&tx, DbResult::LocalRoleResolved { scope, role }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve local role failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::MintApiToken { scope, max_role, ttl_hours, label }) => {
                        let Some(ref kp) = keypair else {
                            send_result(&tx, DbResult::Error("NodeKeypair not available for API token minting".to_string()));
                            continue;
                        };
                        match handlers::api_token::mint_api_token_handler(&db, kp, &local_did, &scope, &max_role, ttl_hours, label.as_deref()).await {
                            Ok((token, jti, _created_at, expires_at)) => {
                                send_result(&tx, DbResult::ApiTokenMinted { token, jti, scope, max_role, expires_at, label });
                            }
                            Err(e) => send_result(&tx, DbResult::Error(format!("Mint API token failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::RevokeApiToken { jti }) => {
                        match handlers::api_token::revoke_api_token_handler(&db, &local_did, &jti).await {
                            Ok(true) => send_result(&tx, DbResult::ApiTokenRevoked { jti }),
                            Ok(false) => send_result(&tx, DbResult::Error(format!("API token not found or not owned: {jti}"))),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Revoke API token failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ListApiTokens { offset, limit }) => {
                        match handlers::api_token::list_api_tokens_handler(&db, &local_did, offset, limit).await {
                            Ok((tokens, total)) => send_result(&tx, DbResult::ApiTokensListed { tokens, total }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("List API tokens failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ListApiTokensByScope { scope_prefix, offset, limit }) => {
                        match handlers::api_token::list_api_tokens_by_scope_handler(&db, &scope_prefix, offset, limit).await {
                            Ok((tokens, total)) => send_result(&tx, DbResult::ScopedApiTokensListed { tokens, total }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("List API tokens by scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolvePetalScope { petal_id }) => {
                        match handlers::crud::resolve_petal_scope_handler(&db, &petal_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve petal scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::ResolveNodeScope { node_id }) => {
                        match handlers::crud::resolve_node_scope_handler(&db, &node_id).await {
                            Ok(scope) => send_result(&tx, DbResult::ScopeResolved { scope }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Resolve node scope failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::LoadNodesByPetal { petal_id }) => {
                        match handlers::crud::load_nodes_by_petal_handler(&db, &blob_store, &petal_id).await {
                            Ok(nodes) => send_result(&tx, DbResult::NodesLoaded { petal_id, nodes }),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Load nodes by petal failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::GetNodeTransform { node_id }) => {
                        match handlers::crud::get_node_transform_handler(&db, &node_id).await {
                            Ok(Some((position, rotation, scale))) => {
                                send_result(&tx, DbResult::NodeTransformLoaded { node_id, position, rotation, scale });
                            }
                            Ok(None) => send_result(&tx, DbResult::Error(format!("Node not found: {node_id}"))),
                            Err(e) => send_result(&tx, DbResult::Error(format!("Get node transform failed: {e}"))),
                        }
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Database thread shutting down");
            send_result(&tx, DbResult::Stopped);
        });
    });
    startup_rx
        .recv()
        .map_err(|_| DbInitError::RuntimeBuild(std::io::Error::new(
            std::io::ErrorKind::Other,
            "DB thread startup channel closed (thread may have panicked)",
        )))?
        .map_err(|e| e)?;
    Ok(handle)
}

// ---------------------------------------------------------------------------
// Public utilities (used by handlers and external crates)
// ---------------------------------------------------------------------------

/// Derive a deterministic 32-byte namespace ID from a namespace secret.
pub fn derive_namespace_id(secret: &[u8; 32]) -> [u8; 32] {
    *blake3::keyed_hash(b"fractalengine:verse:namespace_id", secret).as_bytes()
}

/// Retrieve a namespace secret from the secret store. Returns `None` if not found.
///
/// Validates that the stored value is exactly 64 hex characters (a 32-byte secret).
pub fn get_namespace_secret(
    store: &dyn fe_identity::SecretStore,
    verse_id: &str,
) -> anyhow::Result<Option<String>> {
    let service = format!("fractalengine:verse:{}:ns_secret", verse_id);
    match store.get(&service, "fractalengine") {
        Ok(Some(secret_hex)) => {
            if secret_hex.len() != 64 {
                anyhow::bail!(
                    "Namespace secret has invalid length: {}",
                    secret_hex.len()
                );
            }
            hex::decode(&secret_hex)
                .map_err(|e| anyhow::anyhow!("Namespace secret is not valid hex: {e}"))?;
            Ok(Some(secret_hex))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Phase C: base64 → blob store migration (startup only)
// ---------------------------------------------------------------------------

/// Migrate legacy asset rows that store file content as base64 in the `data`
/// column to the content-addressed blob store.
async fn migrate_base64_assets_to_blob_store(
    db: &crate::repo::Db,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<(usize, usize)> {
    tracing::info!("Checking for base64 assets to migrate...");

    let mut res: surrealdb::IndexedResults = db
        .query("SELECT * FROM asset WHERE content_hash IS NONE AND data IS NOT NONE")
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;

    if rows.is_empty() {
        return Ok((0, 0));
    }

    tracing::info!("Found {} legacy base64 asset(s) to migrate", rows.len());

    let mut migrated_count: usize = 0;
    let mut total_bytes: usize = 0;

    for row in &rows {
        let asset_id = match row["asset_id"].as_str() {
            Some(id) => id,
            None => {
                tracing::warn!("Skipping asset row with missing asset_id");
                continue;
            }
        };
        let data_str = match row["data"].as_str() {
            Some(d) => d,
            None => {
                tracing::warn!("Skipping asset {asset_id}: data field is not a string");
                continue;
            }
        };

        let bytes =
            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_str) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("Skipping asset {asset_id}: base64 decode failed: {e}");
                    continue;
                }
            };

        let byte_len = bytes.len();

        let hash = match blob_store.add_blob(&bytes) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("Skipping asset {asset_id}: blob_store.add_blob failed: {e}");
                continue;
            }
        };
        let hex = hash_to_hex(&hash);

        if let Err(e) = db
            .query("UPDATE asset SET content_hash = $hash, data = NONE WHERE asset_id = $aid")
            .bind(("hash", hex.clone()))
            .bind(("aid", asset_id.to_string()))
            .await
        {
            tracing::error!("Skipping asset {asset_id}: DB UPDATE failed: {e}");
            continue;
        }

        tracing::info!("Migrated asset {asset_id} ({byte_len} bytes) → content_hash={hex}");
        migrated_count += 1;
        total_bytes += byte_len;
    }

    Ok((migrated_count, total_bytes))
}

#[cfg(test)]
mod subdir_tests {
    use super::*;

    #[test]
    fn imported_assets_subdir_constant() {
        assert_eq!(IMPORTED_ASSETS_SUBDIR, "imported");
    }

    #[test]
    fn imported_assets_dir_joins_correctly() {
        let p = imported_assets_dir();
        assert!(
            p.ends_with("imported"),
            "expected path to end with 'imported', got {}",
            p.display()
        );
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.contains("fractalengine/assets/imported"), "got: {s}");
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::repo::{Db, Repo, Table};
    use crate::schema::Asset;
    use base64::Engine as _;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    /// Helper: create an in-memory SurrealDB instance with the asset schema applied.
    async fn setup_test_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        // Apply the asset table schema via the generated DDL
        db.query(schema::Asset::schema())
            .await
            .expect("define asset")
            .check()
            .expect("define asset check");
        db
    }

    /// Seed one legacy asset row with base64 data and no content_hash.
    async fn seed_legacy_asset(db: &Db, asset_id: &str, raw_bytes: &[u8]) {
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw_bytes);
        Repo::<Asset>::create_raw(db, serde_json::json!({
            "asset_id":     asset_id,
            "name":         "test-asset",
            "content_type": "model/gltf-binary",
            "size_bytes":   raw_bytes.len(),
            "data":         b64,
            "created_at":   "2025-01-01T00:00:00Z",
        }))
        .await
        .expect("seed legacy asset");
    }

    #[tokio::test]
    async fn migrate_base64_asset_to_blob_store() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());
        let raw = b"hello-glb-bytes";

        seed_legacy_asset(&db, "asset-1", raw).await;

        let (count, bytes) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("migration");
        assert_eq!(count, 1);
        assert_eq!(bytes, raw.len());

        // Verify content_hash is set in DB
        let mut res: surrealdb::IndexedResults = db
            .query("SELECT content_hash, data FROM asset WHERE asset_id = 'asset-1'")
            .await
            .expect("query");
        let rows: Vec<serde_json::Value> = res.take(0).expect("take");
        let row = &rows[0];
        assert!(
            row["content_hash"].is_string(),
            "content_hash should be set"
        );
        assert!(row["data"].is_null(), "data should be cleared (NONE)");

        // Verify blob exists in the store
        let ch = row["content_hash"].as_str().unwrap();
        let hash = hash_from_hex(ch).expect("valid hex");
        assert!(blob_store.has_blob(&hash), "blob should exist in store");
    }

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());

        seed_legacy_asset(&db, "asset-2", b"some-data").await;

        let (c1, _) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("first run");
        assert_eq!(c1, 1);

        // Second run should find nothing to migrate
        let (c2, b2) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("second run");
        assert_eq!(c2, 0);
        assert_eq!(b2, 0);
    }

    #[tokio::test]
    async fn migrate_skips_rows_with_content_hash() {
        let db = setup_test_db().await;
        let blob_store: BlobStoreHandle = Arc::new(MockBlobStore::new());

        // Insert a row that already has content_hash (Phase B style)
        let _: Option<serde_json::Value> = db
            .create("asset")
            .content(serde_json::json!({
                "asset_id": "already-migrated",
                "name": "new-asset",
                "content_type": "model/gltf-binary",
                "size_bytes": 42,
                "content_hash": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                "created_at": "2025-01-01T00:00:00Z",
            }))
            .await
            .expect("seed new-style asset");

        let (count, bytes) = migrate_base64_assets_to_blob_store(&db, &blob_store)
            .await
            .expect("migration");
        assert_eq!(count, 0);
        assert_eq!(bytes, 0);
    }
}
