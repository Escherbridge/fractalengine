use fe_runtime::messages::{
    DbCommand, DbResult, FractalHierarchyData, NodeHierarchyData, PetalHierarchyData,
    VerseHierarchyData,
};
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
                    Ok(DbCommand::CreateVerse { name }) => {
                        match create_verse_handler(&db, &name).await {
                            Ok(id) => { tx.send(DbResult::VerseCreated { id, name }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Create verse failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::CreateFractal { verse_id, name }) => {
                        match create_fractal_handler(&db, &verse_id, &name).await {
                            Ok(id) => { tx.send(DbResult::FractalCreated { id, verse_id, name }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Create fractal failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::CreatePetal { fractal_id, name }) => {
                        match create_petal_handler(&db, &fractal_id, &name).await {
                            Ok(id) => { tx.send(DbResult::PetalCreated { id, fractal_id, name }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Create petal failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::CreateNode { petal_id, name, position }) => {
                        match create_node_handler(&db, &petal_id, &name, position).await {
                            Ok(id) => { tx.send(DbResult::NodeCreated { id, petal_id, name, has_asset: false }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Create node failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::ImportGltf { petal_id, name, file_path, position }) => {
                        match import_gltf_handler(&db, &petal_id, &name, &file_path, position).await {
                            Ok((node_id, asset_id, asset_path)) => { tx.send(DbResult::GltfImported { node_id, asset_id, petal_id, name, asset_path, position }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("GLTF import failed: {e}"))).ok(); }
                        }
                    }
                    Ok(DbCommand::LoadHierarchy) => {
                        match load_hierarchy_handler(&db).await {
                            Ok(verses) => { tx.send(DbResult::HierarchyLoaded { verses }).ok(); }
                            Err(e) => { tx.send(DbResult::Error(format!("Load hierarchy failed: {e}"))).ok(); }
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
        .query("SELECT * FROM verse WHERE name = 'Genesis Verse' LIMIT 1")
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
    let petal_name = "Genesis Petal";
    let room_names = vec!["Lobby", "Workshop", "Gallery"];

    // NOTE: caller_node_id added by Worker B — passing &node_id as caller for seed data
    let petal_id = queries::create_petal(db, petal_name, &node_id, &node_id).await?;
    db.query("UPDATE petal SET fractal_id = $fid, bounds = <geometry<polygon>> { type: 'Polygon', coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]] } WHERE petal_id = $pid")
        .bind(("fid", fractal_id.clone()))
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
        let aid: Option<String> = asset_id.map(|s| s.to_string());
        db.query("CREATE node CONTENT {
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
            }")
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
// CRUD handler helpers
// ---------------------------------------------------------------------------

async fn create_verse_handler(
    db: &surrealdb::Surreal<Db>,
    name: &str,
) -> anyhow::Result<String> {
    let verse_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let _: Option<serde_json::Value> = db
        .create("verse")
        .content(serde_json::json!({
            "verse_id": verse_id,
            "name": name,
            "created_by": "local-node",
            "created_at": now,
        }))
        .await?;
    tracing::info!("Created verse: {name} ({verse_id})");
    Ok(verse_id)
}

async fn create_fractal_handler(
    db: &surrealdb::Surreal<Db>,
    verse_id: &str,
    name: &str,
) -> anyhow::Result<String> {
    let fractal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let _: Option<serde_json::Value> = db
        .create("fractal")
        .content(serde_json::json!({
            "fractal_id": fractal_id,
            "verse_id": verse_id,
            "owner_did": "local-node",
            "name": name,
            "description": "",
            "created_at": now,
        }))
        .await?;
    tracing::info!("Created fractal: {name} ({fractal_id}) in verse {verse_id}");
    Ok(fractal_id)
}

async fn create_petal_handler(
    db: &surrealdb::Surreal<Db>,
    fractal_id: &str,
    name: &str,
) -> anyhow::Result<String> {
    let petal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating petal: {name} ({petal_id}) in fractal {fractal_id}");
    db.query("CREATE petal CONTENT {
            petal_id: $petal_id,
            fractal_id: $fractal_id,
            name: $name,
            bounds: <geometry<polygon>> { type: 'Polygon', coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]] },
            created_at: $now,
        }")
        .bind(("petal_id", petal_id.clone()))
        .bind(("fractal_id", fractal_id.to_string()))
        .bind(("name", name.to_string()))
        .bind(("now", now))
        .await?;
    // Assign owner role so local-node can operate on this petal
    let _: Option<serde_json::Value> = db
        .create("role")
        .content(serde_json::json!({
            "node_id": "local-node",
            "petal_id": petal_id,
            "role": "owner",
        }))
        .await?;
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
    db.query("CREATE node CONTENT {
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
        }")
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
    petal_id: &str,
    name: &str,
    file_path: &str,
    position: [f32; 3],
) -> anyhow::Result<(String, String, String)> {
    let path = std::path::Path::new(file_path);
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Could not read file {}: {e}", path.display()))?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("glb");
    let content_type = if ext == "glb" { "model/gltf-binary" } else { "model/gltf+json" };
    let size_bytes = bytes.len();
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let asset_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let _: Option<serde_json::Value> = db
        .create("asset")
        .content(serde_json::json!({
            "asset_id": asset_id,
            "name": name,
            "content_type": content_type,
            "size_bytes": size_bytes,
            "data": data,
            "created_at": now,
        }))
        .await?;
    tracing::info!("Imported asset: {name} ({size_bytes} bytes -> asset_id={asset_id})");

    // Copy the file into fractalengine/assets/imported/ so Bevy's asset server can load it.
    // Bevy looks for assets relative to the binary crate's directory, not the workspace root.
    let imported_dir = imported_assets_dir();
    std::fs::create_dir_all(&imported_dir)?;
    let imported_filename = format!("{}.{}", asset_id, ext);
    let imported_path = imported_dir.join(&imported_filename);
    std::fs::write(&imported_path, &bytes)?;
    // Bevy asset_server paths are relative to `fractalengine/assets/`, so use subdir only.
    let asset_path = std::path::Path::new(IMPORTED_ASSETS_SUBDIR)
        .join(&imported_filename)
        .to_string_lossy()
        .replace('\\', "/");
    let absolute_written = imported_path
        .canonicalize()
        .unwrap_or_else(|_| imported_path.clone());
    let cwd_display = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown-cwd>".to_string());
    tracing::info!(
        "Wrote imported model to {} (absolute: {}, cwd: {})",
        imported_path.display(),
        absolute_written.display(),
        cwd_display
    );

    let node_id = ulid::Ulid::new().to_string();
    let x = position[0] as f64;
    let y = position[1] as f64;
    let z = position[2] as f64;
    tracing::info!("Creating node with GLTF: {name} ({node_id}) in petal {petal_id}");
    db.query("CREATE node CONTENT {
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
        }")
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
    let count = verify_rows.first().and_then(|r| r["c"].as_i64()).unwrap_or(0);
    tracing::info!("Post-CREATE verify: node {} count={}", node_id, count);
    if count == 0 {
        anyhow::bail!("CREATE gltf node '{name}' returned OK but row not found in DB");
    }

    Ok((node_id, asset_id, asset_path))
}

async fn load_hierarchy_handler(
    db: &surrealdb::Surreal<Db>,
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
                        // Check if imported file exists on disk — try .glb then .gltf
                        // so we don't need a schema migration to record the original extension.
                        let asset_path = asset_id_str.as_ref().and_then(|aid| {
                            let dir = imported_assets_dir();
                            for ext in ["glb", "gltf"] {
                                let file_name = format!("{}.{}", aid, ext);
                                let disk_path = dir.join(&file_name);
                                let exists = disk_path.exists();
                                tracing::info!(
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
                                "Hierarchy asset missing on disk for asset_id={} (tried .glb and .gltf)",
                                aid
                            );
                            None
                        });
                        NodeHierarchyData {
                            id: n["node_id"].as_str().unwrap_or_default().to_string(),
                            name: n["display_name"].as_str().unwrap_or_default().to_string(),
                            has_asset,
                            position: [x, y, z],
                            asset_path,
                            petal_id: petal_id.clone(),
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
        assert!(
            s.contains("fractalengine/assets/imported"),
            "got: {s}"
        );
    }
}
