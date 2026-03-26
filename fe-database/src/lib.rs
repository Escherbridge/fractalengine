use fe_runtime::messages::{DbCommand, DbResult};
use surrealdb::engine::local::{Db, SurrealKv};

pub mod admin;
pub mod atlas;
pub mod model_url_meta;
pub mod op_log;
pub mod queries;
pub mod rbac;
pub mod schema;
pub mod space_manager;
pub mod types;
pub mod verse;

pub use atlas::*;
pub use model_url_meta::ModelUrlMeta;
pub use types::*;

#[derive(bevy::prelude::Resource, Clone)]
pub struct DbHandle(pub std::sync::Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>);

pub fn spawn_db_thread(
    rx: crossbeam::channel::Receiver<DbCommand>,
    tx: crossbeam::channel::Sender<DbResult>,
) -> std::thread::JoinHandle<()> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_db_thread must not be called from within a Tokio runtime"
    );
    std::thread::spawn(move || {
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
            tracing::info!("SurrealDB ready");
            tx.send(DbResult::Started).ok();
            #[allow(clippy::while_let_loop)]
            loop {
                match rx.recv() {
                    Ok(DbCommand::Ping) => {
                        tx.send(DbResult::Pong).ok();
                    }
                    Ok(DbCommand::Seed) => {
                        match seed_default_data(&db).await {
                            Ok((petal_name, rooms)) => {
                                tx.send(DbResult::Seeded { petal_name, rooms }).ok();
                            }
                            Err(e) => {
                                tracing::error!("Seed failed: {e}");
                                tx.send(DbResult::Error(format!("Seed failed: {e}"))).ok();
                            }
                        }
                    }
                    Ok(DbCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Database thread shutting down");
            tx.send(DbResult::Stopped).ok();
        });
    })
}

/// Returns `(asset_id_for_name)` map seeded from files in `assets/models/`.
/// Each GLB is base64-encoded and stored in the `asset` table.
/// Skips individual files that cannot be read rather than failing the whole seed.
async fn seed_assets(db: &surrealdb::Surreal<Db>) -> anyhow::Result<std::collections::HashMap<String, String>> {
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
        let Some(ext) = path.extension() else { continue };
        if ext != "glb" && ext != "gltf" {
            continue;
        }
        let file_name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Could not read {}: {e}", path.display());
                continue;
            }
        };
        let content_type = if ext == "glb" { "model/gltf-binary" } else { "model/gltf+json" };
        let size_bytes = bytes.len();
        let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        let asset_id = ulid::Ulid::new().to_string();

        let _: Option<serde_json::Value> = db
            .create("asset")
            .content(serde_json::json!({
                "asset_id": asset_id,
                "name": file_name,
                "content_type": content_type,
                "size_bytes": size_bytes,
                "data": data,
                "created_at": now,
            }))
            .await?;
        tracing::info!("Seeded asset: {file_name} ({size_bytes} bytes → asset_id={asset_id})");
        id_map.insert(file_name, asset_id);
    }
    Ok(id_map)
}

async fn seed_default_data(db: &surrealdb::Surreal<Db>) -> anyhow::Result<(String, Vec<String>)> {
    // Idempotency: skip if genesis verse already exists.
    let mut check: surrealdb::IndexedResults = db
        .query("SELECT verse_id FROM verse WHERE name = 'Genesis Verse' LIMIT 1")
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;
    if !existing.is_empty() {
        tracing::info!("Seed already present — skipping");
        return Ok(("Genesis Petal".to_string(), vec!["Lobby".to_string(), "Workshop".to_string(), "Gallery".to_string()]));
    }

    let node_id = types::NodeId("local-node".to_string());
    // Generate ephemeral seed keypair (not persisted — seed data only).
    let seed_keypair = fe_identity::NodeKeypair::generate();
    let local_did = seed_keypair.to_did_key();
    let now = chrono::Utc::now().to_rfc3339();

    // --- Assets ---
    let asset_map = seed_assets(db).await?;
    let beacon_asset_id = asset_map.get("animated_box").or_else(|| asset_map.get("beacon")).cloned();
    let panel_asset_id  = asset_map.get("box").or_else(|| asset_map.get("info_panel")).cloned();
    let duck_asset_id   = asset_map.get("duck").cloned();

    // --- Verse ---
    let verse_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("verse")
        .content(serde_json::json!({
            "verse_id": verse_id,
            "name": "Genesis Verse",
            "created_by": local_did,
            "created_at": now,
        }))
        .await?;
    tracing::info!("Seeded verse: Genesis Verse ({verse_id})");

    // Local node is the founding member (self-invite).
    // Build invite signature matching VerseInvite canonical payload:
    // "{verse_id}:{invitee_did}:{inviter_did}:{timestamp}"
    let invite_payload = format!("{}:{}:{}:{}", verse_id, local_did, local_did, now);
    let invite_sig_bytes = seed_keypair.sign(invite_payload.as_bytes());
    let invite_sig = hex::encode(invite_sig_bytes.to_bytes());

    let member_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("verse_member")
        .content(serde_json::json!({
            "member_id": member_id,
            "verse_id": verse_id,
            "peer_did": local_did,
            "status": "active",
            "invited_by": local_did,
            "invite_sig": invite_sig,
            "invite_timestamp": now,
            // revoked_at and revoked_by omitted — SurrealDB sets option<string> fields to NONE
            // when absent. Sending JSON null causes a type coercion error.
        }))
        .await?;
    tracing::info!("Seeded verse_member: {local_did}");

    // --- Fractal ---
    let fractal_id = ulid::Ulid::new().to_string();
    let _: Option<serde_json::Value> = db
        .create("fractal")
        .content(serde_json::json!({
            "fractal_id": fractal_id,
            "verse_id": verse_id,
            "owner_did": local_did,
            "name": "Genesis Fractal",
            "description": "The seed fractal for local-node",
            "created_at": now,
        }))
        .await?;
    tracing::info!("Seeded fractal: Genesis Fractal ({fractal_id})");

    // --- Petal (linked to fractal) ---
    // bounds is a GeoJSON Polygon defining the petal floor in XZ space (units = metres).
    // Origin (0,0) is the spawn point. The polygon is 20m × 20m centred at origin.
    let petal_bounds = serde_json::json!({
        "type": "Polygon",
        "coordinates": [[
            [-10.0, -10.0],
            [ 10.0, -10.0],
            [ 10.0,  10.0],
            [-10.0,  10.0],
            [-10.0, -10.0]
        ]]
    });

    let petal_name = "Genesis Petal";
    let room_names = vec!["Lobby", "Workshop", "Gallery"];

    // NOTE: caller_node_id added by Worker B — passing &node_id as caller for seed data
    let petal_id = queries::create_petal(db, petal_name, &node_id, &node_id).await?;
    db.query("UPDATE petal SET fractal_id = $fid, bounds = $bounds WHERE petal_id = $pid")
        .bind(("fid", fractal_id.clone()))
        .bind(("bounds", petal_bounds))
        .bind(("pid", petal_id.0.to_string()))
        .await?;
    tracing::info!("Seeded petal: {petal_name} ({})", petal_id.0);

    // Role must be inserted BEFORE create_room calls, because create_room checks require_write_role.
    let _: Option<serde_json::Value> = db
        .create("role")
        .content(serde_json::json!({
            "node_id":  node_id.0,
            "petal_id": petal_id.0.to_string(),
            "role":     "owner",
        }))
        .await?;
    tracing::info!("Assigned owner role to {}", node_id.0);

    for room_name in &room_names {
        queries::create_room(db, &node_id, &petal_id, room_name).await?;
        tracing::info!("Seeded room: {room_name}");
    }

    // --- Nodes (GeoJSON Point positions on the XZ plane; elevation = Y height) ---
    // position coordinates are [X, Z] (longitude = X, latitude = Z in engine space).
    let node_defs: &[(&str, Option<&String>, [f64; 2], f64, bool)] = &[
        ("Welcome Beacon", beacon_asset_id.as_ref(), [0.0, 0.0],  0.0, false),
        ("Info Panel",     panel_asset_id.as_ref(),  [3.0, 0.0],  1.2, true),
        ("Lucky Duck",     duck_asset_id.as_ref(),   [-2.0, 2.0], 0.0, true),
    ];
    for (name, asset_id, xz, elevation, interactive) in node_defs {
        let node_record_id = ulid::Ulid::new().to_string();
        let _: Option<serde_json::Value> = db
            .create("node")
            .content(serde_json::json!({
                "node_id":      node_record_id,
                "petal_id":     petal_id.0.to_string(),
                "display_name": name,
                "asset_id":     asset_id,
                "position":     { "type": "Point", "coordinates": xz },
                "elevation":    elevation,
                "rotation":     [0.0_f64, 0.0, 0.0, 1.0],
                "scale":        [1.0_f64, 1.0, 1.0],
                "interactive":  interactive,
                "created_at":   now,
            }))
            .await?;
        tracing::info!("Seeded node: {name} at XZ{xz:?} elev={elevation}");
    }

    Ok((
        petal_name.to_string(),
        room_names.iter().map(|s| s.to_string()).collect(),
    ))
}
