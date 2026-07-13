use tracing::instrument;

use fe_runtime::messages::{
    FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
};
use crate::repo::{Db, Repo};
use crate::schema::{Fractal, Role, Verse};

use super::invite::{generate_namespace_secret, store_namespace_secret};
use crate::{
    hash_from_hex, hash_to_hex, imported_assets_dir, replicate_row, BlobStoreHandle,
    ReplicationSender, IMPORTED_ASSETS_SUBDIR,
};

// ---------------------------------------------------------------------------
// Verse
// ---------------------------------------------------------------------------

#[instrument(skip(db, blob_store, repl_tx, secret_store))]
pub(crate) async fn create_verse_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    name: &str,
    local_did: &str,
    secret_store: Option<&std::sync::Arc<dyn fe_identity::SecretStore>>,
) -> anyhow::Result<String> {
    let verse_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let ns_secret = generate_namespace_secret();
    let ns_id = crate::derive_namespace_id(&ns_secret);
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

    if let Some(ss) = secret_store {
        if let Err(e) = store_namespace_secret(ss.as_ref(), &verse_id, &ns_secret_hex) {
            tracing::warn!("Could not store namespace secret for verse {verse_id}: {e}");
        }
    }

    tracing::info!("Created verse: {name} ({verse_id}) namespace_id={ns_id_hex}");
    Ok(verse_id)
}

// ---------------------------------------------------------------------------
// Fractal
// ---------------------------------------------------------------------------

#[instrument(skip(db, blob_store, repl_tx))]
pub(crate) async fn create_fractal_handler(
    db: &Db,
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

// ---------------------------------------------------------------------------
// Petal
// ---------------------------------------------------------------------------

#[instrument(skip(db))]
pub(crate) async fn create_petal_handler(
    db: &Db,
    fractal_id: &str,
    name: &str,
    local_did: &str,
) -> anyhow::Result<String> {
    let petal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating petal: {name} ({petal_id}) in fractal {fractal_id}");
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
    db.query(
        "CREATE petal CONTENT {
            petal_id: $petal_id,
            fractal_id: $fractal_id,
            name: $name,
            node_id: $node_id,
            bounds: <geometry<polygon>> {
                type: 'Polygon',
                coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]]
            },
            created_at: $now,
        }",
    )
    .bind(("petal_id", petal_id.clone()))
    .bind(("fractal_id", fractal_id.to_string()))
    .bind(("name", name.to_string()))
    .bind(("node_id", local_did.to_string()))
    .bind(("now", now.clone()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE petal '{name}' failed: {e}"))?;
    Repo::<Role>::create(db, &Role {
        peer_did: local_did.to_string(),
        scope:    format!("VERSE#_-FRACTAL#_-PETAL#{}", petal_id.clone()),
        role:     "owner".to_string(),
    }).await?;
    Ok(petal_id)
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[instrument(skip(db))]
pub(crate) async fn create_node_handler(
    db: &Db,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
) -> anyhow::Result<String> {
    let node_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating node: {name} ({node_id}) in petal {petal_id}");
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
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
    .bind(("x", position[0] as f64))
    .bind(("z", position[2] as f64))
    .bind(("y", position[1] as f64))
    .bind(("now", now.clone()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE empty node '{name}' failed: {e}"))?;

    if let Err(e) = super::node_log::append_node_log(
        db, &node_id, "created", "local",
        &serde_json::json!({ "petal_id": petal_id, "name": name, "position": position }),
    ).await {
        tracing::warn!("Failed to write node_log for created node {node_id}: {e}");
    }

    Ok(node_id)
}

/// Delete a node and cascade to its child waypoint rows.
///
/// Returns the node's `petal_id` (needed by the caller to scope the
/// `SceneChange::NodeRemoved` event). Bails if no node row matched — see
/// AGENTS.md §gis for the matched-rows-assertion convention.
#[instrument(skip(db))]
pub(crate) async fn delete_node_handler(db: &Db, node_id: &str) -> anyhow::Result<String> {
    // An absent `node` table means no nodes exist at all — treat it as
    // "matched no node" rather than a hard error, so the missing-node contract
    // holds on a fresh/empty DB too (see AGENTS.md §gis).
    let petal_id = {
        let lookup = db
            .query("SELECT petal_id FROM node WHERE node_id = $node_id")
            .bind(("node_id", node_id.to_string()))
            .await
            .map_err(|e| anyhow::anyhow!("DeleteNode lookup query failed: {e}"))?
            .check();
        let rows: Vec<serde_json::Value> = match lookup {
            Ok(mut res) => res
                .take(0)
                .map_err(|e| anyhow::anyhow!("DeleteNode lookup take failed: {e}"))?,
            Err(e) if e.to_string().contains("does not exist") => Vec::new(),
            Err(e) => return Err(anyhow::anyhow!("DeleteNode lookup statement failed: {e}")),
        };
        rows.first()
            .and_then(|r| r.get("petal_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("DeleteNode matched no node with node_id = {node_id}"))?
    };

    // Atomic cascade: delete the parent node row AND its waypoint children
    // (`properties.gpx_track_id == <this node_id>`, see
    // fractalengine/src/AGENTS.md §gpx) in one statement so a crash can't leave
    // a parent with its waypoints gone (inverse orphan). `RETURN BEFORE` on the
    // parent-matching predicate lets the matched-no-node bail still fire.
    let mut node_res = db
        .query(
            "DELETE node WHERE node_id = $node_id OR properties.gpx_track_id = $node_id \
             RETURN BEFORE",
        )
        .bind(("node_id", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("DeleteNode cascade query failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("DeleteNode cascade statement failed: {e}"))?;
    let deleted: Vec<serde_json::Value> = node_res
        .take(0)
        .map_err(|e| anyhow::anyhow!("DeleteNode take failed: {e}"))?;
    // The parent node existed (the petal_id lookup above matched it), so if the
    // combined delete returned nothing the row vanished between lookup and
    // delete — preserve the matched-no-node contract.
    let parent_deleted = deleted.iter().any(|row| {
        row.get("node_id").and_then(|v| v.as_str()) == Some(node_id)
    });
    if !parent_deleted {
        anyhow::bail!("DeleteNode matched no node with node_id = {node_id}");
    }

    tracing::info!("Deleted node {node_id} (petal {petal_id}), cascaded waypoints");
    Ok(petal_id)
}

// ---------------------------------------------------------------------------
// Import GLTF
// ---------------------------------------------------------------------------

#[instrument(skip(db, blob_store))]
pub(crate) async fn import_gltf_handler(
    db: &Db,
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

    let asset_path = format!("blob://{}.{}", content_hash, ext);

    let node_id = ulid::Ulid::new().to_string();
    tracing::info!("Creating node with GLTF: {name} ({node_id}) in petal {petal_id}");
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
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
    .bind(("x", position[0] as f64))
    .bind(("z", position[2] as f64))
    .bind(("y", position[1] as f64))
    .bind(("now", now.clone()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE gltf node '{name}' failed: {e}"))?;

    if let Err(e) = super::node_log::append_node_log(
        db, &node_id, "created", "local",
        &serde_json::json!({
            "petal_id": petal_id, "name": name, "position": position,
            "asset_id": asset_id, "asset_path": &asset_path,
        }),
    ).await {
        tracing::warn!("Failed to write node_log for imported gltf node {node_id}: {e}");
    }

    Ok((node_id, asset_id, asset_path))
}

// ---------------------------------------------------------------------------
// Load Hierarchy
// ---------------------------------------------------------------------------

#[instrument(skip(db, blob_store))]
pub(crate) async fn load_hierarchy_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Vec<VerseHierarchyData>> {
    use std::collections::HashMap;

    // -----------------------------------------------------------------------
    // Phase 1: Batch-fetch all four entity tables in 4 queries (was N+1)
    // -----------------------------------------------------------------------
    let mut all_res: surrealdb::IndexedResults = db
        .query(
            "SELECT * FROM verse ORDER BY created_at ASC;\
             SELECT * FROM fractal ORDER BY created_at ASC;\
             SELECT * FROM petal ORDER BY created_at ASC;\
             SELECT * FROM node ORDER BY created_at ASC",
        )
        .await?;

    let verses_raw: Vec<serde_json::Value> = all_res.take(0)?;
    let fractals_raw: Vec<serde_json::Value> = all_res.take(1)?;
    let petals_raw: Vec<serde_json::Value> = all_res.take(2)?;
    let nodes_raw: Vec<serde_json::Value> = all_res.take(3)?;

    // -----------------------------------------------------------------------
    // Phase 2: Collect all asset_ids, batch-fetch asset hashes + model URLs
    // -----------------------------------------------------------------------
    let all_asset_ids: Vec<String> = nodes_raw
        .iter()
        .filter_map(|n| n["asset_id"].as_str().map(|s| s.to_string()))
        .collect();

    let mut asset_hash_map: HashMap<String, String> = HashMap::new();
    let mut model_url_map: HashMap<String, Option<String>> = HashMap::new();

    if !all_asset_ids.is_empty() {
        let mut asset_res: surrealdb::IndexedResults = db
            .query("SELECT asset_id, content_hash FROM asset WHERE asset_id IN $aids")
            .bind(("aids", all_asset_ids.clone()))
            .await?;
        let asset_rows: Vec<serde_json::Value> = asset_res.take(0)?;
        for row in &asset_rows {
            if let (Some(aid), Some(ch)) =
                (row["asset_id"].as_str(), row["content_hash"].as_str())
            {
                asset_hash_map.insert(aid.to_string(), ch.to_string());
            }
        }

        let mut model_res: surrealdb::IndexedResults = db
            .query("SELECT asset_id, external_url FROM model WHERE asset_id IN $aids")
            .bind(("aids", all_asset_ids))
            .await?;
        let model_rows: Vec<serde_json::Value> = model_res.take(0)?;
        for row in &model_rows {
            if let Some(aid) = row["asset_id"].as_str() {
                let url = row["external_url"].as_str().map(|s| s.to_string());
                model_url_map.insert(aid.to_string(), url);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Phase 3: Build parent→children lookup maps
    // -----------------------------------------------------------------------

    // fractal.verse_id → Vec<fractal row>
    let mut fractals_by_verse: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for f in &fractals_raw {
        let vid = f["verse_id"].as_str().unwrap_or_default().to_string();
        fractals_by_verse.entry(vid).or_default().push(f);
    }

    // petal.fractal_id → Vec<petal row>
    let mut petals_by_fractal: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for p in &petals_raw {
        let fid = p["fractal_id"].as_str().unwrap_or_default().to_string();
        petals_by_fractal.entry(fid).or_default().push(p);
    }

    // node.petal_id → Vec<node row>
    let mut nodes_by_petal: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for n in &nodes_raw {
        let pid = n["petal_id"].as_str().unwrap_or_default().to_string();
        nodes_by_petal.entry(pid).or_default().push(n);
    }

    // -----------------------------------------------------------------------
    // Phase 4: Assemble the hierarchy tree in-memory
    // -----------------------------------------------------------------------
    let resolve_asset_path = |aid: &str| -> Option<String> {
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
    };

    let verses: Vec<VerseHierarchyData> = verses_raw
        .iter()
        .map(|v| {
            let verse_id = v["verse_id"].as_str().unwrap_or_default().to_string();
            let verse_name = v["name"].as_str().unwrap_or_default().to_string();
            let namespace_id = v["namespace_id"].as_str().map(|s| s.to_string());

            let fractals: Vec<FractalHierarchyData> = fractals_by_verse
                .get(&verse_id)
                .map(|fs| {
                    fs.iter()
                        .map(|f| {
                            let fractal_id =
                                f["fractal_id"].as_str().unwrap_or_default().to_string();
                            let fractal_name =
                                f["name"].as_str().unwrap_or_default().to_string();

                            let petals: Vec<PetalHierarchyData> = petals_by_fractal
                                .get(&fractal_id)
                                .map(|ps| {
                                    ps.iter()
                                        .map(|p| {
                                            let petal_id = p["petal_id"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string();
                                            let petal_name = p["name"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string();

                                            let nodes: Vec<NodeHierarchyData> = nodes_by_petal
                                                .get(&petal_id)
                                                .map(|ns| {
                                                    ns.iter()
                                                        .map(|n| {
                                                            let has_asset =
                                                                !n["asset_id"].is_null();
                                                            let asset_id_str = n["asset_id"]
                                                                .as_str()
                                                                .map(|s| s.to_string());
                                                            let coords =
                                                                &n["position"]["coordinates"];
                                                            let x = coords[0]
                                                                .as_f64()
                                                                .unwrap_or(0.0)
                                                                as f32;
                                                            let z = coords[1]
                                                                .as_f64()
                                                                .unwrap_or(0.0)
                                                                as f32;
                                                            let y = n["elevation"]
                                                                .as_f64()
                                                                .unwrap_or(0.0)
                                                                as f32;

                                                            let asset_path =
                                                                asset_id_str.as_ref().and_then(
                                                                    |aid| resolve_asset_path(aid),
                                                                );
                                                            let webpage_url = asset_id_str
                                                                .as_ref()
                                                                .and_then(|aid| {
                                                                    model_url_map.get(aid)
                                                                })
                                                                .and_then(|u| u.clone());
                                                            NodeHierarchyData {
                                                                id: n["node_id"]
                                                                    .as_str()
                                                                    .unwrap_or_default()
                                                                    .to_string(),
                                                                name: n["display_name"]
                                                                    .as_str()
                                                                    .unwrap_or_default()
                                                                    .to_string(),
                                                                has_asset,
                                                                position: [x, y, z],
                                                                asset_path,
                                                                petal_id: petal_id.clone(),
                                                                webpage_url,
                                                            }
                                                        })
                                                        .collect()
                                                })
                                                .unwrap_or_default();

                                            PetalHierarchyData {
                                                id: petal_id,
                                                name: petal_name,
                                                nodes,
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            FractalHierarchyData {
                                id: fractal_id,
                                name: fractal_name,
                                petals,
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            VerseHierarchyData {
                id: verse_id,
                name: verse_name,
                namespace_id,
                fractals,
            }
        })
        .collect();

    tracing::info!("Loaded hierarchy: {} verses", verses.len());
    Ok(verses)
}

// ---------------------------------------------------------------------------
// Load nodes by petal (scene snapshot)
// ---------------------------------------------------------------------------

/// Load all nodes belonging to a petal as `NodeDto` values for scene streaming.
#[instrument(skip(db, blob_store))]
pub(crate) async fn load_nodes_by_petal_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    petal_id: &str,
) -> anyhow::Result<Vec<fe_runtime::messages::NodeDto>> {
    let mut node_res: surrealdb::IndexedResults = db
        .query("SELECT * FROM node WHERE petal_id = $pid ORDER BY created_at ASC")
        .bind(("pid", petal_id.to_string()))
        .await?;
    let nodes_raw: Vec<serde_json::Value> = node_res.take(0)?;

    // Batch-fetch asset content hashes for blob:// paths
    let asset_ids: Vec<String> = nodes_raw
        .iter()
        .filter_map(|n| n["asset_id"].as_str().map(|s| s.to_string()))
        .collect();
    let mut asset_hash_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if !asset_ids.is_empty() {
        let mut asset_res: surrealdb::IndexedResults = db
            .query("SELECT asset_id, content_hash FROM asset WHERE asset_id IN $aids")
            .bind(("aids", asset_ids))
            .await?;
        let asset_rows: Vec<serde_json::Value> = asset_res.take(0)?;
        for row in &asset_rows {
            if let (Some(aid), Some(ch)) =
                (row["asset_id"].as_str(), row["content_hash"].as_str())
            {
                asset_hash_map.insert(aid.to_string(), ch.to_string());
            }
        }
    }

    let nodes = nodes_raw
        .iter()
        .map(|n| {
            let has_asset = !n["asset_id"].is_null();
            let asset_id_str = n["asset_id"].as_str().map(|s| s.to_string());
            let coords = &n["position"]["coordinates"];
            let x = coords[0].as_f64().unwrap_or(0.0) as f32;
            let z = coords[1].as_f64().unwrap_or(0.0) as f32;
            let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

            let rotation_raw = &n["rotation"];
            let rotation = if rotation_raw.is_array() {
                let arr = rotation_raw.as_array().unwrap();
                [
                    arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    arr.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                ]
            } else {
                [0.0, 0.0, 0.0, 1.0]
            };

            let scale_raw = &n["scale"];
            let scale = if scale_raw.is_array() {
                let arr = scale_raw.as_array().unwrap();
                [
                    arr.first().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                    arr.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                    arr.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                ]
            } else {
                [1.0, 1.0, 1.0]
            };

            let asset_path = asset_id_str.as_ref().and_then(|aid| {
                if let Some(ch) = asset_hash_map.get(aid) {
                    if let Ok(hash) = hash_from_hex(ch) {
                        if blob_store.has_blob(&hash) {
                            return Some(format!("blob://{}.glb", ch));
                        }
                    }
                }
                None
            });

            fe_runtime::messages::NodeDto {
                node_id: n["node_id"].as_str().unwrap_or_default().to_string(),
                petal_id: petal_id.to_string(),
                name: n["display_name"].as_str().unwrap_or_default().to_string(),
                position: [x, y, z],
                rotation,
                scale,
                has_asset,
                asset_path,
            }
        })
        .collect();

    Ok(nodes)
}

// ---------------------------------------------------------------------------
// Get node transform
// ---------------------------------------------------------------------------

/// Read a single node's persisted transform (position, rotation, scale).
#[instrument(skip(db))]
pub(crate) async fn get_node_transform_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<([f32; 3], [f32; 3], [f32; 3])>> {
    let mut res: surrealdb::IndexedResults = db
        .query("SELECT position, elevation, rotation, scale FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let coords = &row["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0) as f32;
    let z = coords[1].as_f64().unwrap_or(0.0) as f32;
    let y = row["elevation"].as_f64().unwrap_or(0.0) as f32;

    let rotation_raw = &row["rotation"];
    let rotation = if rotation_raw.is_array() {
        let arr = rotation_raw.as_array().unwrap();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
        ]
    } else {
        [0.0, 0.0, 0.0]
    };

    let scale_raw = &row["scale"];
    let scale = if scale_raw.is_array() {
        let arr = scale_raw.as_array().unwrap();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
        ]
    } else {
        [1.0, 1.0, 1.0]
    };

    Ok(Some(([x, y, z], rotation, scale)))
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

/// Resolve a `petal_id` to its full hierarchical scope string
/// (`VERSE#<v>-FRACTAL#<f>-PETAL#<p>`).
///
/// Returns `None` when the petal (or its parent fractal/verse chain) cannot be
/// found in the database.
#[instrument(skip(db))]
pub(crate) async fn resolve_petal_scope_handler(
    db: &Db,
    petal_id: &str,
) -> anyhow::Result<Option<String>> {
    // Step 1: petal → fractal_id
    let mut res = db
        .query("SELECT fractal_id FROM petal WHERE petal_id = $pid LIMIT 1")
        .bind(("pid", petal_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(fractal_id) = rows.first().and_then(|r| r["fractal_id"].as_str()) else {
        return Ok(None);
    };
    let fractal_id = fractal_id.to_string();

    // Step 2: fractal → verse_id
    let mut res2 = db
        .query("SELECT verse_id FROM fractal WHERE fractal_id = $fid LIMIT 1")
        .bind(("fid", fractal_id.clone()))
        .await?;
    let rows2: Vec<serde_json::Value> = res2.take(0)?;
    let Some(verse_id) = rows2.first().and_then(|r| r["verse_id"].as_str()) else {
        return Ok(None);
    };

    Ok(Some(crate::build_scope(verse_id, Some(&fractal_id), Some(petal_id))))
}

/// Resolve a `node_id` to its full hierarchical scope string by first
/// resolving the node's owning petal and then delegating to
/// [`resolve_petal_scope_handler`].
///
/// Returns `None` when the node (or any ancestor in the chain) is not found.
#[instrument(skip(db))]
pub(crate) async fn resolve_node_scope_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<String>> {
    // Step 1: node → petal_id
    let mut res = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(petal_id) = rows.first().and_then(|r| r["petal_id"].as_str()) else {
        return Ok(None);
    };
    resolve_petal_scope_handler(db, petal_id).await
}

// ---------------------------------------------------------------------------
// FR-1 tests: DeleteNode cascade
// ---------------------------------------------------------------------------

#[cfg(test)]
mod delete_node_tests {
    use super::*;

    /// In-memory SurrealDB, no DDL — `node` is written schemaless via
    /// `CREATE ... CONTENT {}` (mirrors production; see `create_node_handler`).
    async fn setup_mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        db
    }

    #[tokio::test]
    async fn delete_node_removes_row_and_cascades_waypoints() {
        let db = setup_mem_db().await;
        let track_id = create_node_handler(&db, "petal-1", "track", [0.0, 0.0, 0.0])
            .await
            .expect("create track node");
        let wp_id = create_node_handler(&db, "petal-1", "wp", [1.0, 0.0, 1.0])
            .await
            .expect("create waypoint node");

        // Tag the waypoint as a child of the track, mirroring gpx_bridge's
        // property shape (properties.gpx_track_id == track node_id).
        set_entity_property_helper(&db, &wp_id, "gpx_track_id", serde_json::json!(track_id))
            .await;

        let petal_id = delete_node_handler(&db, &track_id)
            .await
            .expect("delete_node_handler should succeed");
        assert_eq!(petal_id, "petal-1");

        let mut res = db
            .query("SELECT node_id FROM node WHERE node_id = $nid")
            .bind(("nid", track_id.clone()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        assert!(rows.is_empty(), "track node row should be gone");

        let mut res2 = db
            .query("SELECT node_id FROM node WHERE node_id = $nid")
            .bind(("nid", wp_id.clone()))
            .await
            .unwrap();
        let rows2: Vec<serde_json::Value> = res2.take(0).unwrap();
        assert!(rows2.is_empty(), "cascaded waypoint node row should be gone");
    }

    #[tokio::test]
    async fn delete_node_bails_on_missing_node() {
        let db = setup_mem_db().await;
        let err = delete_node_handler(&db, "does-not-exist")
            .await
            .expect_err("deleting a non-existent node must error, not silently no-op");
        assert!(
            err.to_string().contains("matched no node"),
            "unexpected error message: {err}"
        );
    }

    /// Minimal property-set helper mirroring `set_entity_property_handler`'s
    /// shape without pulling in the fe-query builder (keeps this test local).
    async fn set_entity_property_helper(db: &Db, node_id: &str, key: &str, value: serde_json::Value) {
        db.query("UPDATE node SET properties[$key] = $val WHERE node_id = $nid")
            .bind(("key", key.to_string()))
            .bind(("val", value))
            .bind(("nid", node_id.to_string()))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
}

