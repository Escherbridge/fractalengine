use fe_runtime::messages::{
    DbCommand, DbResult, FractalHierarchyData, NodeHierarchyData, PetalHierarchyData,
    VerseHierarchyData,
};
use crate::repo::Repo;
use crate::schema::{Asset, Fractal, Role, Verse, VerseMemberRow};
use surrealdb::engine::local::{Db, SurrealKv};

// P2P Mycelium Phase A: re-export blob store types so callers (fe-ui, the
// binary crate, and tests) can depend only on `fe-database` for the full
// asset-pipeline surface.
pub use fe_runtime::blob_store::{
    hash_from_hex, hash_to_hex, BlobHash, BlobStore, BlobStoreHandle,
};

pub mod admin;
pub mod atlas;
pub mod invite;
pub mod migrations;
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
pub use model_url_meta::ModelUrlMeta;
pub use role_level::RoleLevel;
pub use scope::{build_scope, parse_scope, parent_scope, ScopeParts};
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
) -> std::thread::JoinHandle<()> {
    spawn_db_thread_with_sync(rx, tx, blob_store, None, None)
}

/// Spawn the DB thread with an optional replication sender and node keypair.
///
/// When `repl_tx` is `Some`, DB write handlers can send `ReplicationEvent`s
/// that the binary crate bridges into `SyncCommand::WriteRowEntry`. When
/// `None`, replication is silently disabled.
///
/// When `keypair` is `Some`, invite generation/verification is enabled.
pub fn spawn_db_thread_with_sync(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
    blob_store: BlobStoreHandle,
    repl_tx: Option<ReplicationSender>,
    keypair: Option<fe_identity::NodeKeypair>,
) -> std::thread::JoinHandle<()> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_db_thread must not be called from within a Tokio runtime"
    );
    std::thread::spawn(move || {
        // The blob store handle is owned by the DB thread so future handlers
        // (Phase B: import refactor, Phase C: migration) can write to it
        // without crossing channel boundaries. Held live for thread lifetime.
        let blob_store: BlobStoreHandle = blob_store;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build database Tokio runtime");
        rt.block_on(async {
            tracing::info!("Database thread started, initialising SurrealDB");
            let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
                surrealdb::Surreal::new::<SurrealKv>("data/fractalengine.db")
                    .await
                    .expect("SurrealDB init");
            db.use_ns("fractalengine")
                .use_db("fractalengine")
                .await
                .expect("SurrealDB ns/db");
            rbac::apply_schema(&db)
                .await
                .expect("Schema application failed");

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
            #[allow(clippy::while_let_loop)]
            loop {
                match rx.recv() {
                    Ok(DbCommand::Ping) => {
                        if tx.send(DbResult::Pong).is_err() {
                            tracing::warn!("Result channel closed — UI may have shut down");
                        }
                    }
                    Ok(DbCommand::Seed) => match seed_default_data(&db, &blob_store, &local_did).await {
                        Ok((petal_name, rooms)) => {
                            if tx.send(DbResult::Seeded { petal_name, rooms }).is_err() {
                                tracing::warn!("Result channel closed — UI may have shut down");
                            }
                        }
                        Err(e) => {
                            tracing::error!("Seed failed: {e}");
                            if tx.send(DbResult::Error(format!("Seed failed: {e}"))).is_err() {
                                tracing::warn!("Result channel closed — UI may have shut down");
                            }
                        }
                    },
                    Ok(DbCommand::CreateVerse { name }) => {
                        match create_verse_handler(&db, &blob_store, repl_tx.as_ref(), &name, &local_did).await
                        {
                            Ok(id) => {
                                if tx.send(DbResult::VerseCreated { id, name }).is_err() {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Create verse failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::CreateFractal { verse_id, name }) => {
                        match create_fractal_handler(
                            &db,
                            &blob_store,
                            repl_tx.as_ref(),
                            &verse_id,
                            &name,
                            &local_did,
                        )
                        .await
                        {
                            Ok(id) => {
                                if tx
                                    .send(DbResult::FractalCreated { id, verse_id, name })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Create fractal failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::CreatePetal { fractal_id, name }) => {
                        match create_petal_handler(&db, &fractal_id, &name, &local_did).await {
                            Ok(id) => {
                                if tx
                                    .send(DbResult::PetalCreated {
                                        id,
                                        fractal_id,
                                        name,
                                    })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Create petal failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::CreateNode {
                        petal_id,
                        name,
                        position,
                    }) => match create_node_handler(&db, &petal_id, &name, position).await {
                        Ok(id) => {
                            if tx
                                .send(DbResult::NodeCreated {
                                    id,
                                    petal_id,
                                    name,
                                    has_asset: false,
                                })
                                .is_err()
                            {
                                tracing::warn!("Result channel closed — UI may have shut down");
                            }
                        }
                        Err(e) => {
                            if tx
                                .send(DbResult::Error(format!("Create node failed: {e}")))
                                .is_err()
                            {
                                tracing::warn!("Result channel closed — UI may have shut down");
                            }
                        }
                    },
                    Ok(DbCommand::ImportGltf {
                        petal_id,
                        name,
                        file_path,
                        position,
                    }) => {
                        match import_gltf_handler(
                            &db,
                            &blob_store,
                            &petal_id,
                            &name,
                            &file_path,
                            position,
                        )
                        .await
                        {
                            Ok((node_id, asset_id, asset_path)) => {
                                if tx
                                    .send(DbResult::GltfImported {
                                        node_id,
                                        asset_id,
                                        petal_id,
                                        name,
                                        asset_path,
                                        position,
                                    })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("GLTF import failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::LoadHierarchy) => {
                        match load_hierarchy_handler(&db, &blob_store).await {
                            Ok(verses) => {
                                if tx.send(DbResult::HierarchyLoaded { verses }).is_err() {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Load hierarchy failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::GenerateVerseInvite {
                        verse_id,
                        include_write_cap,
                        expiry_hours,
                    }) => {
                        match generate_verse_invite_handler(
                            &db,
                            &verse_id,
                            include_write_cap,
                            expiry_hours,
                            keypair.as_ref(),
                        )
                        .await
                        {
                            Ok(invite_string) => {
                                if tx
                                    .send(DbResult::VerseInviteGenerated {
                                        verse_id,
                                        invite_string,
                                    })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Generate invite failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::JoinVerseByInvite { invite_string }) => {
                        match join_verse_by_invite_handler(&db, &invite_string, &local_did).await {
                            Ok((verse_id, verse_name)) => {
                                if tx
                                    .send(DbResult::VerseJoined {
                                        verse_id,
                                        verse_name,
                                    })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Join verse failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::ResetDatabase) => {
                        match reset_database_handler(&db, &blob_store, &local_did).await {
                            Ok((petal_name, rooms)) => {
                                if tx
                                    .send(DbResult::DatabaseReset { petal_name, rooms })
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                            Err(e) => {
                                if tx
                                    .send(DbResult::Error(format!("Database reset failed: {e}")))
                                    .is_err()
                                {
                                    tracing::warn!("Result channel closed — UI may have shut down");
                                }
                            }
                        }
                    }
                    Ok(DbCommand::UpdateNodeTransform { node_id, position, rotation, scale }) => {
                        match update_node_transform_handler(&db, &node_id, position, rotation, scale).await {
                            Ok(()) => {}
                            Err(e) => {
                                tracing::error!("UpdateNodeTransform failed for {node_id}: {e}");
                            }
                        }
                    }
                    Ok(DbCommand::UpdateNodeUrl { node_id, url }) => {
                        match update_node_url_handler(&db, &node_id, url).await {
                            Ok(()) => {}
                            Err(e) => {
                                tracing::error!("UpdateNodeUrl failed for {node_id}: {e}");
                            }
                        }
                    }
                    Ok(DbCommand::RenameEntity { entity_type, entity_id, new_name }) => {
                        let result = match entity_type.as_str() {
                            "verse" => db.query("UPDATE verse SET name = $name WHERE verse_id = $id")
                                .bind(("name", new_name.clone()))
                                .bind(("id", entity_id.clone()))
                                .await,
                            "fractal" => db.query("UPDATE fractal SET name = $name WHERE fractal_id = $id")
                                .bind(("name", new_name.clone()))
                                .bind(("id", entity_id.clone()))
                                .await,
                            "petal" => db.query("UPDATE petal SET name = $name WHERE petal_id = $id")
                                .bind(("name", new_name.clone()))
                                .bind(("id", entity_id.clone()))
                                .await,
                            other => {
                                tx.send(DbResult::Error(format!("Unknown entity type: {other}"))).ok();
                                continue;
                            }
                        };
                        match result {
                            Ok(_) => { tx.send(DbResult::EntityRenamed { entity_type, entity_id, new_name }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Rename failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::SetVerseDefaultAccess { verse_id, default_access }) => {
                        match crate::role_manager::set_default_access(&db, &verse_id, &default_access).await {
                            Ok(()) => { tx.send(DbResult::VerseDefaultAccessSet { verse_id, default_access }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Set default access failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::UpdateFractalDescription { fractal_id, description }) => {
                        match db.query("UPDATE fractal SET description = $desc WHERE fractal_id = $id")
                            .bind(("desc", description.clone()))
                            .bind(("id", fractal_id.clone()))
                            .await
                        {
                            Ok(_) => { tx.send(DbResult::FractalDescriptionUpdated { fractal_id, description }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Update description failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::DeleteEntity { entity_type, entity_id }) => {
                        let result = match entity_type.as_str() {
                            "verse" => async {
                                // Cascade: delete nodes -> petals -> fractals -> roles -> verse
                                db.query("DELETE FROM node WHERE petal_id IN (SELECT petal_id FROM petal WHERE fractal_id IN (SELECT fractal_id FROM fractal WHERE verse_id = $id))")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM petal WHERE fractal_id IN (SELECT fractal_id FROM fractal WHERE verse_id = $id)")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM fractal WHERE verse_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM role WHERE string::starts_with(scope, $prefix)")
                                    .bind(("prefix", format!("VERSE#{}", entity_id))).await?;
                                db.query("DELETE FROM verse_member WHERE verse_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM verse WHERE verse_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                Ok::<(), surrealdb::Error>(())
                            }.await,
                            "fractal" => async {
                                db.query("DELETE FROM node WHERE petal_id IN (SELECT petal_id FROM petal WHERE fractal_id = $id)")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM petal WHERE fractal_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM fractal WHERE fractal_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                Ok(())
                            }.await,
                            "petal" => async {
                                db.query("DELETE FROM node WHERE petal_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                db.query("DELETE FROM petal WHERE petal_id = $id")
                                    .bind(("id", entity_id.clone())).await?;
                                Ok(())
                            }.await,
                            other => {
                                tx.send(DbResult::Error(format!("Unknown entity type: {other}"))).ok();
                                continue;
                            }
                        };
                        match result {
                            Ok(()) => { tx.send(DbResult::EntityDeleted { entity_type, entity_id }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Delete failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::ResolveRolesForPeers { scope, peer_dids }) => {
                        let mut roles = Vec::new();
                        for did in &peer_dids {
                            match crate::role_manager::resolve_role(&db, did, &scope).await {
                                Ok(level) => roles.push((did.clone(), level.to_string())),
                                Err(e) => {
                                    tracing::warn!("resolve_role failed for {did}: {e}");
                                    roles.push((did.clone(), "none".to_string()));
                                }
                            }
                        }
                        tx.send(DbResult::PeerRolesResolved { scope, roles }).ok();
                    }
                    Ok(DbCommand::AssignRole { peer_did, scope, role }) => {
                        let role_level = crate::RoleLevel::from(role.as_str());
                        match crate::role_manager::assign_role_checked(&db, &local_did, &peer_did, &scope, role_level).await {
                            Ok(()) => { tx.send(DbResult::RoleAssigned { peer_did, scope, role }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Assign role failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::RevokeRole { peer_did, scope }) => {
                        match crate::role_manager::revoke_role_checked(&db, &local_did, &peer_did, &scope).await {
                            Ok(()) => { tx.send(DbResult::RoleRevoked { peer_did, scope }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Revoke role failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::GenerateScopedInvite { scope, role, expiry_hours }) => {
                        let Some(ref kp) = keypair else {
                            tx.send(DbResult::Error("NodeKeypair not available for invite signing".to_string())).ok();
                            continue;
                        };
                        let role_level = crate::RoleLevel::from(role.as_str());
                        let expiry_secs = (expiry_hours as u64) * 3600;
                        match crate::scoped_invite::generate_scoped_invite(kp, &local_did, &scope, role_level, expiry_secs) {
                            Ok(invite) => {
                                tx.send(DbResult::ScopedInviteGenerated { invite_link: invite.to_invite_link() }).ok();
                            }
                            Err(e) => {
                                tx.send(DbResult::Error(format!("Generate scoped invite failed: {e}"))).ok();
                            }
                        }
                    }
                    Ok(DbCommand::ResolveLocalRole { scope }) => {
                        match crate::role_manager::resolve_role(&db, &local_did, &scope).await {
                            Ok(level) => { tx.send(DbResult::LocalRoleResolved { scope, role: level.to_string() }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Resolve local role failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Database thread shutting down");
            if tx.send(DbResult::Stopped).is_err() {
                tracing::warn!("Result channel closed — UI may have shut down");
            }
        });
    })
}

async fn reset_database_handler(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    local_did: &str,
) -> anyhow::Result<(String, Vec<String>)> {
    admin::clear_all_tables(db).await?;
    seed_default_data(db, blob_store, local_did).await
}

async fn update_node_transform_handler(
    db: &surrealdb::Surreal<Db>,
    node_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<()> {
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
    let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
    db.query(
        "UPDATE node SET \
            position = <geometry<point>> [$x, $z], \
            elevation = $y, \
            rotation = $rot, \
            scale = $sc \
         WHERE node_id = $node_id",
    )
    .bind(("node_id", node_id.to_string()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("rot", rot))
    .bind(("sc", sc))
    .await
    .map_err(|e| anyhow::anyhow!("UpdateNodeTransform DB query failed: {e}"))?;
    Ok(())
}

async fn update_node_url_handler(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    node_id: &str,
    url: Option<String>,
) -> anyhow::Result<()> {
    // Look up the asset_id for this node, then update the model record.
    db.query(
        "UPDATE model SET external_url = $url \
         WHERE asset_id = (SELECT VALUE asset_id FROM node WHERE node_id = $node_id LIMIT 1)[0]",
    )
    .bind(("node_id", node_id.to_string()))
    .bind(("url", url))
    .await
    .map_err(|e| anyhow::anyhow!("UpdateNodeUrl DB query failed: {e}"))?;
    Ok(())
}

/// Returns `(asset_id_for_name)` map seeded from files in `assets/models/`.
/// Each GLB is written to the blob store and its BLAKE3 content hash is
/// recorded in the `asset` table (Phase B: no more base64 in DB rows).
/// Skips individual files that cannot be read rather than failing the whole seed.
async fn seed_assets(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let models_dir = std::path::Path::new("assets/models");
    let mut id_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    if !models_dir.exists() {
        tracing::warn!("assets/models/ not found — skipping asset seed");
        return Ok(id_map);
    }

    let now = chrono::Utc::now().to_rfc3339();
    for entry in std::fs::read_dir(models_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext != "glb" && ext != "gltf" {
            continue;
        }
        let file_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Could not read {}: {e}", path.display());
                continue;
            }
        };
        let content_type = if ext == "glb" {
            "model/gltf-binary"
        } else {
            "model/gltf+json"
        };
        let size_bytes = bytes.len();
        let hash = blob_store.add_blob(&bytes).map_err(|e| {
            anyhow::anyhow!("blob_store.add_blob failed for {}: {e}", path.display())
        })?;
        let content_hash = hash_to_hex(&hash);
        let asset_id = ulid::Ulid::new().to_string();

        Repo::<Asset>::create_raw(db, serde_json::json!({
            "asset_id": asset_id,
            "name": file_name,
            "content_type": content_type,
            "size_bytes": size_bytes,
            "content_hash": content_hash,
            "created_at": now,
        })).await?;
        tracing::info!("Seeded asset: {file_name} ({size_bytes} bytes → asset_id={asset_id}, hash={content_hash})");
        id_map.insert(file_name, asset_id);
    }
    Ok(id_map)
}

pub async fn seed_default_data(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    local_did: &str,
) -> anyhow::Result<(String, Vec<String>)> {
    // Idempotency: skip if genesis verse already exists.
    let mut check: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse WHERE name = 'Genesis Verse' LIMIT 1")
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;
    if !existing.is_empty() {
        tracing::info!("Seed already present — skipping");
        return Ok((
            "Genesis Petal".to_string(),
            vec![
                "Lobby".to_string(),
                "Workshop".to_string(),
                "Gallery".to_string(),
            ],
        ));
    }

    // Use the node's real DID so the local user is recognized as Owner.
    let seed_keypair = fe_identity::NodeKeypair::generate();
    let local_did = local_did.to_string();
    let node_id = types::NodeId(local_did.clone());
    let now = chrono::Utc::now().to_rfc3339();

    // --- Assets ---
    let asset_map = seed_assets(db, blob_store).await?;
    let beacon_asset_id = asset_map
        .get("animated_box")
        .or_else(|| asset_map.get("beacon"))
        .cloned();
    let panel_asset_id = asset_map
        .get("box")
        .or_else(|| asset_map.get("info_panel"))
        .cloned();
    let duck_asset_id = asset_map.get("duck").cloned();

    // --- Verse ---
    let verse_id = ulid::Ulid::new().to_string();
    Repo::<Verse>::create(db, &Verse {
        verse_id: verse_id.clone(),
        name: "Genesis Verse".to_string(),
        created_by: local_did.clone(),
        created_at: now.clone(),
        namespace_id: None,
        default_access: "viewer".to_string(),
    }).await?;
    tracing::info!("Seeded verse: Genesis Verse ({verse_id})");

    // Local node is the founding member (self-invite).
    // Build invite signature matching VerseInvite canonical payload:
    // "{verse_id}:{invitee_did}:{inviter_did}:{timestamp}"
    let invite_payload = format!("{}:{}:{}:{}", verse_id, local_did, local_did, now);
    let invite_sig_bytes = seed_keypair.sign(invite_payload.as_bytes());
    let invite_sig = hex::encode(invite_sig_bytes.to_bytes());

    let member_id = ulid::Ulid::new().to_string();
    // Note: revoked_at/revoked_by must be None, not null — SurrealDB rejects
    // JSON null for option<string> fields. skip_serializing_if isn't on the
    // macro struct, so we use create_raw to omit them entirely.
    Repo::<VerseMemberRow>::create_raw(db, serde_json::json!({
        "member_id": member_id,
        "verse_id": verse_id,
        "peer_did": local_did,
        "status": "active",
        "invited_by": local_did,
        "invite_sig": invite_sig,
        "invite_timestamp": now,
    })).await?;
    tracing::info!("Seeded verse_member: {local_did}");

    // --- Fractal ---
    let fractal_id = ulid::Ulid::new().to_string();
    Repo::<Fractal>::create(db, &Fractal {
        fractal_id: fractal_id.clone(),
        verse_id: verse_id.clone(),
        owner_did: local_did.clone(),
        name: "Genesis Fractal".to_string(),
        description: Some(format!("The seed fractal for {local_did}")),
        created_at: now.clone(),
    }).await?;
    tracing::info!("Seeded fractal: Genesis Fractal ({fractal_id})");

    // --- Petal (linked to fractal) ---
    // bounds is a GeoJSON Polygon defining the petal floor in XZ space (units = metres).
    // Origin (0,0) is the spawn point. The polygon is 20m × 20m centred at origin.
    let petal_name = "Genesis Petal";
    let room_names = vec!["Lobby", "Workshop", "Gallery"];

    // NOTE: caller_node_id added by Worker B — passing &node_id as caller for seed data
    let petal_id = queries::create_petal(db, petal_name, &node_id, &node_id).await?;
    db.query("UPDATE petal SET fractal_id = $fid, bounds = <geometry<polygon>> { type: 'Polygon', coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]] } WHERE petal_id = $pid")
        .bind(("fid", fractal_id.clone()))
        .bind(("pid", petal_id.0.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("seed UPDATE petal fractal_id failed: {e}"))?;
    tracing::info!("Seeded petal: {petal_name} ({})", petal_id.0);

    // Role must be inserted BEFORE create_room calls, because create_room checks require_write_role.
    Repo::<Role>::create(db, &Role {
        peer_did: node_id.0.clone(),
        scope:    format!("VERSE#_-FRACTAL#_-PETAL#{}", petal_id.0.to_string()),
        role:     "owner".to_string(),
    }).await?;
    tracing::info!("Assigned owner role to {}", node_id.0);

    for room_name in &room_names {
        queries::create_room(db, &node_id, &petal_id, room_name).await?;
        tracing::info!("Seeded room: {room_name}");
    }

    // --- Nodes (GeoJSON Point positions on the XZ plane; elevation = Y height) ---
    // position coordinates are [X, Z] (longitude = X, latitude = Z in engine space).
    #[allow(clippy::type_complexity)]
    let node_defs: &[(&str, Option<&String>, [f64; 2], f64, bool)] = &[
        (
            "Welcome Beacon",
            beacon_asset_id.as_ref(),
            [0.0, 0.0],
            0.0,
            false,
        ),
        ("Info Panel", panel_asset_id.as_ref(), [3.0, 0.0], 1.2, true),
        ("Lucky Duck", duck_asset_id.as_ref(), [-2.0, 2.0], 0.0, true),
    ];
    for (name, asset_id, xz, elevation, interactive) in node_defs {
        let node_record_id = ulid::Ulid::new().to_string();
        let aid: Option<String> = asset_id.map(|s| s.to_string());
        db.query(
            "CREATE node CONTENT {
                node_id: $node_id,
                petal_id: $petal_id,
                display_name: $name,
                asset_id: $asset_id,
                position: <geometry<point>> [$x, $z],
                elevation: $elev,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                interactive: $interactive,
                created_at: $now,
            }",
        )
        .bind(("node_id", node_record_id))
        .bind(("petal_id", petal_id.0.to_string()))
        .bind(("name", name.to_string()))
        .bind(("asset_id", aid))
        .bind(("x", xz[0]))
        .bind(("z", xz[1]))
        .bind(("elev", *elevation))
        .bind(("interactive", *interactive))
        .bind(("now", now.clone()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("seed CREATE node '{name}' failed: {e}"))?;
        tracing::info!("Seeded node: {name} at XZ{xz:?} elev={elevation}");
    }

    Ok((
        petal_name.to_string(),
        room_names.iter().map(|s| s.to_string()).collect(),
    ))
}

// ---------------------------------------------------------------------------
// Phase C: base64 → blob store migration
// ---------------------------------------------------------------------------

/// Migrate legacy asset rows that store file content as base64 in the `data`
/// column to the content-addressed blob store. After migration each row has a
/// `content_hash` and the `data` field is cleared (set to NONE).
///
/// Returns `(migrated_count, total_bytes)`. Errors on individual rows are
/// logged and skipped — the migration never aborts the whole batch.
async fn migrate_base64_assets_to_blob_store(
    db: &surrealdb::Surreal<Db>,
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

        // Decode base64
        let bytes =
            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_str) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!("Skipping asset {asset_id}: base64 decode failed: {e}");
                    continue;
                }
            };

        let byte_len = bytes.len();

        // Compute hash for idempotency check before writing
        let hash = match blob_store.add_blob(&bytes) {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("Skipping asset {asset_id}: blob_store.add_blob failed: {e}");
                continue;
            }
        };
        let hex = hash_to_hex(&hash);

        // Update the DB row: set content_hash, clear data
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

// ---------------------------------------------------------------------------
// CRUD handler helpers
// ---------------------------------------------------------------------------

async fn create_verse_handler(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let verse_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    // Phase E: generate a per-verse namespace (pragmatic approach).
    // A random 32-byte secret is the "namespace secret"; the namespace ID is
    // derived deterministically as blake3(secret). This decouples namespace
    // creation from needing a running iroh-docs engine. The mapping to an
    // actual iroh-docs replica happens when the sync thread opens the replica.
    let ns_secret = generate_namespace_secret();
    let ns_id = derive_namespace_id(&ns_secret);
    let ns_id_hex = hex::encode(ns_id);
    let ns_secret_hex = hex::encode(ns_secret);

    let verse_row = Verse {
        verse_id: verse_id.clone(),
        name: name.to_string(),
        created_by: local_did.to_string(),
        created_at: now.clone(),
        namespace_id: Some(ns_id_hex.clone()),
        default_access: "viewer".to_string(),
    };
    let row_json = serde_json::to_value(&verse_row)?;

    Repo::<Verse>::create(db, &verse_row).await?;

    // Replicate the new verse row.
    replicate_row(
        repl_tx,
        blob_store,
        &verse_id,
        "verse",
        &verse_id,
        serde_json::to_string(&row_json)
            .unwrap_or_default()
            .as_bytes(),
    );

    // Store namespace secret in OS keyring so it survives restarts but stays
    // out of the DB (the ID is public, the secret is not).
    if let Err(e) = store_namespace_secret_in_keyring(&verse_id, &ns_secret_hex) {
        // Non-fatal: log and continue. The verse is created; the secret can
        // be re-derived from a backup flow in a future phase.
        tracing::warn!("Could not store namespace secret in keyring for verse {verse_id}: {e}");
    }

    tracing::info!("Created verse: {name} ({verse_id}) namespace_id={ns_id_hex}");
    Ok(verse_id)
}

/// Generate a random 32-byte namespace secret using a cryptographically
/// secure random number generator (CSPRNG).
fn generate_namespace_secret() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// Derive a deterministic 32-byte namespace ID from a namespace secret.
pub fn derive_namespace_id(secret: &[u8; 32]) -> [u8; 32] {
    *blake3::keyed_hash(b"fractalengine:verse:namespace_id", secret).as_bytes()
}

/// Store a namespace secret in the OS keyring.
fn store_namespace_secret_in_keyring(verse_id: &str, secret_hex: &str) -> anyhow::Result<()> {
    let service = format!("fractalengine:verse:{}:ns_secret", verse_id);
    let entry = keyring::Entry::new(&service, "fractalengine")
        .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;
    entry
        .set_password(secret_hex)
        .map_err(|e| anyhow::anyhow!("keyring set_password failed: {e}"))?;
    Ok(())
}

/// Retrieve a namespace secret from the OS keyring. Returns None if not found.
pub fn get_namespace_secret_from_keyring(verse_id: &str) -> anyhow::Result<Option<String>> {
    let service = format!("fractalengine:verse:{}:ns_secret", verse_id);
    let entry = keyring::Entry::new(&service, "fractalengine")
        .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;
    match entry.get_password() {
        Ok(secret_hex) => {
            // Validate it's a valid 64-char hex string (32 bytes).
            if secret_hex.len() != 64 {
                anyhow::bail!(
                    "Keyring namespace secret has invalid length: {}",
                    secret_hex.len()
                );
            }
            hex::decode(&secret_hex)
                .map_err(|e| anyhow::anyhow!("Keyring namespace secret is not valid hex: {e}"))?;
            Ok(Some(secret_hex))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(anyhow::anyhow!("keyring get_password failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Phase F: Invite handlers
// ---------------------------------------------------------------------------

async fn generate_verse_invite_handler(
    db: &surrealdb::Surreal<Db>,
    verse_id: &str,
    include_write_cap: bool,
    expiry_hours: u32,
    keypair: Option<&fe_identity::NodeKeypair>,
) -> anyhow::Result<String> {
    let keypair =
        keypair.ok_or_else(|| anyhow::anyhow!("NodeKeypair not available for invite signing"))?;

    // Look up verse row for namespace_id and name.
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT namespace_id, name FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("verse not found: {verse_id}"))?;

    let verse_name = row["name"].as_str().unwrap_or("Unknown").to_string();

    // If the verse was created before Phase E, it won't have a namespace_id.
    // Backfill: generate a new namespace secret, store it, and update the verse row.
    let namespace_id = match row["namespace_id"].as_str() {
        Some(ns) if !ns.is_empty() => ns.to_string(),
        _ => {
            tracing::info!("Backfilling namespace_id for pre-Phase-E verse {verse_id}");
            let ns_secret = generate_namespace_secret();
            let ns_id = derive_namespace_id(&ns_secret);
            let ns_id_hex = hex::encode(ns_id);
            let ns_secret_hex = hex::encode(ns_secret);

            db.query("UPDATE verse SET namespace_id = $nsid WHERE verse_id = $vid")
                .bind(("nsid", ns_id_hex.clone()))
                .bind(("vid", verse_id.to_string()))
                .await?;

            if let Err(e) = store_namespace_secret_in_keyring(verse_id, &ns_secret_hex) {
                tracing::warn!("Could not store backfilled namespace secret in keyring: {e}");
            }

            ns_id_hex
        }
    };

    // Retrieve namespace secret from keyring.
    let namespace_secret = if include_write_cap {
        get_namespace_secret_from_keyring(verse_id)?
    } else {
        None
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry = now + (expiry_hours as u64) * 3600;

    // Creator node address: use the keypair's verifying key hex as a node identifier.
    let creator_node_addr = hex::encode(keypair.verifying_key().to_bytes());

    let mut invite = invite::VerseInvite {
        namespace_id,
        namespace_secret,
        creator_node_addr,
        verse_name,
        verse_id: verse_id.to_string(),
        expiry_timestamp: expiry,
        signature: String::new(),
    };
    invite.sign(keypair);

    let invite_string = invite.to_invite_string();
    tracing::info!(
        "Generated invite for verse {verse_id} (write_cap={include_write_cap}, expires in {expiry_hours}h)"
    );
    Ok(invite_string)
}

async fn join_verse_by_invite_handler(
    db: &surrealdb::Surreal<Db>,
    invite_string: &str,
    local_did: &str,
) -> anyhow::Result<(String, String)> {
    // Strip fractalengine://invite/ prefix if present
    let payload = invite_string.trim();
    let payload = payload
        .strip_prefix("fractalengine://invite/")
        .unwrap_or(payload);

    // Try parsing as a scoped invite first (new format), then fall back to verse invite (old format)
    if let Ok(scoped) = scoped_invite::ScopedInvite::from_invite_string(payload) {
        // Validate the scoped invite
        if let Err(e) = scoped_invite::validate_scoped_invite(&scoped) {
            anyhow::bail!("Invalid scoped invite: {e}");
        }
        let parts = crate::scope::parse_scope(&scoped.scope)?;
        let verse_id = parts.verse_id;
        // For scoped invites, we don't have verse name — use scope as name
        return Ok((verse_id, format!("Joined via scoped invite")));
    }

    let invite = invite::VerseInvite::from_invite_string(payload)?;

    // Verify invite signature using the creator's public key (encoded as hex
    // in creator_node_addr — this is the ed25519 verifying key bytes).
    let creator_pk_bytes = hex::decode(&invite.creator_node_addr)
        .map_err(|e| anyhow::anyhow!("Invalid creator_node_addr hex: {e}"))?;
    let creator_pk_array: [u8; 32] = creator_pk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("creator_node_addr is not 32 bytes"))?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&creator_pk_array)
        .map_err(|e| anyhow::anyhow!("Invalid creator public key: {e}"))?;
    if !invite.verify(&verifying_key) {
        anyhow::bail!("Invalid invite signature");
    }

    if invite.is_expired() {
        anyhow::bail!("Invite has expired");
    }

    let verse_id = &invite.verse_id;
    let verse_name = &invite.verse_name;
    let namespace_id = &invite.namespace_id;

    // Check if verse already exists locally.
    let mut check: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;

    if existing.is_empty() {
        // Create the verse row.
        let now = chrono::Utc::now().to_rfc3339();
        Repo::<Verse>::create(db, &Verse {
            verse_id:       verse_id.clone(),
            name:           verse_name.clone(),
            created_by:     invite.creator_node_addr.clone(),
            created_at:     now.clone(),
            namespace_id:   Some(namespace_id.clone()),
            default_access: "viewer".to_string(),
        }).await?;
        tracing::info!("Created verse from invite: {verse_name} ({verse_id})");
    } else {
        tracing::info!("Verse {verse_id} already exists, updating namespace_id");
        db.query("UPDATE verse SET namespace_id = $nsid WHERE verse_id = $vid")
            .bind(("nsid", namespace_id.to_string()))
            .bind(("vid", verse_id.to_string()))
            .await?;
    }

    // Store namespace secret in keyring if present (write capability).
    if let Some(ref secret) = invite.namespace_secret {
        if let Err(e) = store_namespace_secret_in_keyring(verse_id, secret) {
            tracing::warn!("Could not store namespace secret for joined verse {verse_id}: {e}");
        }
    }

    // Create verse_member row for self.
    let now = chrono::Utc::now().to_rfc3339();
    let member_id = ulid::Ulid::new().to_string();
    Repo::<VerseMemberRow>::create_raw(db, serde_json::json!({
        "member_id":        member_id,
        "verse_id":         verse_id,
        "peer_did":         local_did,
        "status":           "active",
        "invited_by":       invite.creator_node_addr,
        "invite_sig":       invite.signature,
        "invite_timestamp": now,
    })).await?;

    tracing::info!(
        "Joined verse {verse_name} ({verse_id}) via invite (write_cap={})",
        invite.namespace_secret.is_some()
    );

    Ok((verse_id.clone(), verse_name.clone()))
}

async fn create_fractal_handler(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    verse_id: &str,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let fractal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let fractal_row = Fractal {
        fractal_id: fractal_id.clone(),
        verse_id: verse_id.to_string(),
        owner_did: local_did.to_string(),
        name: name.to_string(),
        description: Some(String::new()),
        created_at: now.clone(),
    };
    let row_json = serde_json::to_value(&fractal_row)?;

    Repo::<Fractal>::create(db, &fractal_row).await?;

    // Replicate the new fractal row.
    replicate_row(
        repl_tx,
        blob_store,
        verse_id,
        "fractal",
        &fractal_id,
        serde_json::to_string(&row_json)
            .unwrap_or_default()
            .as_bytes(),
    );

    tracing::info!("Created fractal: {name} ({fractal_id}) in verse {verse_id}");
    Ok(fractal_id)
}

async fn create_petal_handler(
    db: &surrealdb::Surreal<Db>,
    fractal_id: &str,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let petal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating petal: {name} ({petal_id}) in fractal {fractal_id}");
    db.query("CREATE petal CONTENT {
            petal_id: $petal_id,
            fractal_id: $fractal_id,
            name: $name,
            node_id: $node_id,
            bounds: <geometry<polygon>> { type: 'Polygon', coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]] },
            created_at: $now,
        }")
        .bind(("petal_id", petal_id.clone()))
        .bind(("fractal_id", fractal_id.to_string()))
        .bind(("name", name.to_string()))
        .bind(("node_id", local_did.to_string()))
        .bind(("now", now))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("CREATE petal '{name}' failed: {e}"))?;
    // Assign owner role so the local node can operate on this petal
    Repo::<Role>::create(db, &Role {
        peer_did: local_did.to_string(),
        scope:    format!("VERSE#_-FRACTAL#_-PETAL#{}", petal_id.clone()),
        role:     "owner".to_string(),
    }).await?;
    Ok(petal_id)
}

async fn create_node_handler(
    db: &surrealdb::Surreal<Db>,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
) -> anyhow::Result<String> {
    let node_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    tracing::info!("Creating node: {name} ({node_id}) in petal {petal_id}");
    db.query(
        "CREATE node CONTENT {
            node_id: $node_id,
            petal_id: $petal_id,
            display_name: $name,
            asset_id: NONE,
            position: <geometry<point>> [$x, $z],
            elevation: $y,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: false,
            created_at: $now,
        }",
    )
    .bind(("node_id", node_id.clone()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("name", name.to_string()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("now", now))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE empty node '{name}' failed: {e}"))?;
    Ok(node_id)
}

async fn import_gltf_handler(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
    petal_id: &str,
    name: &str,
    file_path: &str,
    position: [f32; 3],
) -> anyhow::Result<(String, String, String)> {
    let path = std::path::Path::new(file_path);
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Could not read file {}: {e}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("glb");
    let content_type = if ext == "glb" {
        "model/gltf-binary"
    } else {
        "model/gltf+json"
    };
    let size_bytes = bytes.len();

    // Phase B: write bytes to blob store instead of base64-encoding into the DB row.
    let hash = blob_store
        .add_blob(&bytes)
        .map_err(|e| anyhow::anyhow!("blob_store.add_blob failed: {e}"))?;
    let content_hash = hash_to_hex(&hash);

    let asset_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    db.query(
        "CREATE asset CONTENT {
            asset_id: $asset_id,
            name: $name,
            content_type: $content_type,
            size_bytes: $size_bytes,
            data: NONE,
            content_hash: $content_hash,
            created_at: $now,
        }",
    )
    .bind(("asset_id", asset_id.clone()))
    .bind(("name", name.to_string()))
    .bind(("content_type", content_type.to_string()))
    .bind(("size_bytes", size_bytes as i64))
    .bind(("content_hash", content_hash.clone()))
    .bind(("now", now.clone()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    tracing::info!(
        "Imported asset: {name} ({size_bytes} bytes -> asset_id={asset_id}, hash={content_hash})"
    );

    // Phase B: asset_path uses blob:// scheme so Bevy loads via the BlobAssetReader.
    let asset_path = format!("blob://{}.{}", content_hash, ext);

    let node_id = ulid::Ulid::new().to_string();
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    tracing::info!("Creating node with GLTF: {name} ({node_id}) in petal {petal_id}");
    db.query(
        "CREATE node CONTENT {
            node_id: $node_id,
            petal_id: $petal_id,
            display_name: $name,
            asset_id: $asset_id,
            position: <geometry<point>> [$x, $z],
            elevation: $y,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            interactive: true,
            created_at: $now,
        }",
    )
    .bind(("node_id", node_id.clone()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("name", name.to_string()))
    .bind(("asset_id", asset_id.clone()))
    .bind(("x", x))
    .bind(("y", y))
    .bind(("z", z))
    .bind(("now", now))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE gltf node '{name}' failed: {e}"))?;

    // Verify the node was actually persisted (SurrealDB otherwise swallows
    // errors in the Response even after .check() if the CREATE returned 0 rows).
    let mut verify: surrealdb::IndexedResults = db
        .query("SELECT count() AS c FROM node WHERE node_id = $nid GROUP ALL")
        .bind(("nid", node_id.clone()))
        .await?;
    let verify_rows: Vec<serde_json::Value> = verify.take(0)?;
    let count = verify_rows
        .first()
        .and_then(|r| r["c"].as_i64())
        .unwrap_or(0);
    tracing::debug!("Post-CREATE verify: node {} count={}", node_id, count);
    if count == 0 {
        anyhow::bail!("CREATE gltf node '{name}' returned OK but row not found in DB");
    }

    Ok((node_id, asset_id, asset_path))
}

async fn load_hierarchy_handler(
    db: &surrealdb::Surreal<Db>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Vec<VerseHierarchyData>> {
    // Query all verses
    let mut verse_res: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse ORDER BY created_at ASC")
        .await?;
    let verses_raw: Vec<serde_json::Value> = verse_res.take(0)?;

    let mut verses = Vec::new();
    for v in &verses_raw {
        let verse_id = v["verse_id"].as_str().unwrap_or_default().to_string();
        let verse_name = v["name"].as_str().unwrap_or_default().to_string();
        // Phase E: namespace_id for iroh-docs replica mapping.
        let namespace_id = v["namespace_id"].as_str().map(|s| s.to_string());

        // Fractals for this verse
        let mut frac_res: surrealdb::IndexedResults = db
            .query("SELECT * FROM fractal WHERE verse_id = $vid ORDER BY created_at ASC")
            .bind(("vid", verse_id.clone()))
            .await?;
        let fractals_raw: Vec<serde_json::Value> = frac_res.take(0)?;

        let mut fractals = Vec::new();
        for f in &fractals_raw {
            let fractal_id = f["fractal_id"].as_str().unwrap_or_default().to_string();
            let fractal_name = f["name"].as_str().unwrap_or_default().to_string();

            // Petals for this fractal
            let mut petal_res: surrealdb::IndexedResults = db
                .query("SELECT * FROM petal WHERE fractal_id = $fid ORDER BY created_at ASC")
                .bind(("fid", fractal_id.clone()))
                .await?;
            let petals_raw: Vec<serde_json::Value> = petal_res.take(0)?;

            let mut petals = Vec::new();
            for p in &petals_raw {
                let petal_id = p["petal_id"].as_str().unwrap_or_default().to_string();
                let petal_name = p["name"].as_str().unwrap_or_default().to_string();

                // Nodes for this petal
                let mut node_res: surrealdb::IndexedResults = db
                    .query("SELECT * FROM node WHERE petal_id = $pid ORDER BY created_at ASC")
                    .bind(("pid", petal_id.clone()))
                    .await?;
                let nodes_raw: Vec<serde_json::Value> = node_res.take(0)?;

                // Pre-fetch asset rows for this petal's nodes so we can resolve
                // content_hash → blob:// paths without N+1 queries.
                let asset_ids: Vec<String> = nodes_raw
                    .iter()
                    .filter_map(|n| n["asset_id"].as_str().map(|s| s.to_string()))
                    .collect();
                let mut asset_hash_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                let mut model_url_map: std::collections::HashMap<String, Option<String>> =
                    std::collections::HashMap::new();
                if !asset_ids.is_empty() {
                    let mut asset_res: surrealdb::IndexedResults = db
                        .query("SELECT asset_id, content_hash FROM asset WHERE asset_id IN $aids")
                        .bind(("aids", asset_ids.clone()))
                        .await?;
                    let asset_rows: Vec<serde_json::Value> = asset_res.take(0)?;
                    for row in &asset_rows {
                        if let (Some(aid), Some(ch)) =
                            (row["asset_id"].as_str(), row["content_hash"].as_str())
                        {
                            asset_hash_map.insert(aid.to_string(), ch.to_string());
                        }
                    }
                    // Fetch portal URLs from model records (one query for all nodes).
                    let mut model_res: surrealdb::IndexedResults = db
                        .query("SELECT asset_id, external_url FROM model WHERE asset_id IN $aids")
                        .bind(("aids", asset_ids))
                        .await?;
                    let model_rows: Vec<serde_json::Value> = model_res.take(0)?;
                    for row in &model_rows {
                        if let Some(aid) = row["asset_id"].as_str() {
                            let url = row["external_url"].as_str().map(|s| s.to_string());
                            model_url_map.insert(aid.to_string(), url);
                        }
                    }
                }

                let nodes: Vec<NodeHierarchyData> = nodes_raw
                    .iter()
                    .map(|n| {
                        let has_asset = !n["asset_id"].is_null();
                        let asset_id_str = n["asset_id"].as_str().map(|s| s.to_string());
                        // Extract position from GeoJSON Point coordinates [X, Z] + elevation Y
                        let coords = &n["position"]["coordinates"];
                        let x = coords[0].as_f64().unwrap_or(0.0) as f32;
                        let z = coords[1].as_f64().unwrap_or(0.0) as f32;
                        let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

                        // Phase B: resolve asset_path via content_hash → blob store.
                        // Backward compat: if content_hash is absent, fall back to
                        // the old imported/ directory probe (Phase C migration will
                        // backfill content_hash for all legacy rows).
                        let asset_path = asset_id_str.as_ref().and_then(|aid| {
                            // Try content_hash first (Phase B path).
                            if let Some(ch) = asset_hash_map.get(aid) {
                                if let Ok(hash) = hash_from_hex(ch) {
                                    if blob_store.has_blob(&hash) {
                                        return Some(format!("blob://{}.glb", ch));
                                    }
                                    tracing::warn!(
                                        "Blob missing for asset_id={aid} content_hash={ch}"
                                    );
                                }
                            }
                            // Backward compat: probe imported/ directory.
                            let dir = imported_assets_dir();
                            for ext in ["glb", "gltf"] {
                                let file_name = format!("{}.{}", aid, ext);
                                let disk_path = dir.join(&file_name);
                                let exists = disk_path.exists();
                                tracing::debug!(
                                    "Hierarchy asset probe: {} exists={}",
                                    disk_path.display(),
                                    exists
                                );
                                if exists {
                                    let rel = std::path::Path::new(IMPORTED_ASSETS_SUBDIR)
                                        .join(&file_name)
                                        .to_string_lossy()
                                        .replace('\\', "/");
                                    return Some(rel);
                                }
                            }
                            tracing::warn!(
                                "Hierarchy asset missing for asset_id={aid} (no blob, no imported file)"
                            );
                            None
                        });
                        let webpage_url = asset_id_str
                            .as_ref()
                            .and_then(|aid| model_url_map.get(aid))
                            .and_then(|u| u.clone());
                        NodeHierarchyData {
                            id: n["node_id"].as_str().unwrap_or_default().to_string(),
                            name: n["display_name"].as_str().unwrap_or_default().to_string(),
                            has_asset,
                            position: [x, y, z],
                            asset_path,
                            petal_id: petal_id.clone(),
                            webpage_url,
                        }
                    })
                    .collect();

                petals.push(PetalHierarchyData {
                    id: petal_id,
                    name: petal_name,
                    nodes,
                });
            }

            fractals.push(FractalHierarchyData {
                id: fractal_id,
                name: fractal_name,
                petals,
            });
        }

        verses.push(VerseHierarchyData {
            id: verse_id,
            name: verse_name,
            namespace_id,
            fractals,
        });
    }

    tracing::info!("Loaded hierarchy: {} verses", verses.len());
    Ok(verses)
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
    use crate::repo::Table;
    use fe_runtime::blob_store::mock::MockBlobStore;
    use std::sync::Arc;

    /// Helper: create an in-memory SurrealDB instance with the asset schema applied.
    async fn setup_test_db() -> surrealdb::Surreal<Db> {
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
    async fn seed_legacy_asset(db: &surrealdb::Surreal<Db>, asset_id: &str, raw_bytes: &[u8]) {
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw_bytes);
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
