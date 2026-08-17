use tracing::instrument;

use crate::handlers::preconditions::require_petal_scope;

use crate::repo::{Db, Repo};
use crate::schema::{Fractal, Role, Verse};
use fe_runtime::messages::{
    FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
};

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
    let mut scope_result = db
        .query("SELECT verse_id FROM fractal WHERE fractal_id = $fractal_id LIMIT 1")
        .bind(("fractal_id", fractal_id.to_string()))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("CreatePetal parent lookup failed: {error}"))?;
    let parent_rows: Vec<serde_json::Value> = scope_result
        .take(0)
        .map_err(|error| anyhow::anyhow!("CreatePetal parent result read failed: {error}"))?;
    let verse_id = parent_rows
        .first()
        .and_then(|row| row.get("verse_id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!("CreatePetal matched no fractal with fractal_id = {fractal_id}")
        })?;
    if Repo::<Verse>::find_by_id(db, verse_id).await?.is_none() {
        anyhow::bail!("CreatePetal matched no verse with verse_id = {verse_id}");
    }
    let scope = crate::build_scope(verse_id, Some(fractal_id), Some(&petal_id));

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
    Repo::<Role>::create(
        db,
        &Role {
            peer_did: local_did.to_string(),
            scope,
            role: "owner".to_string(),
        },
    )
    .await?;
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
    require_petal_scope(db, petal_id, "CreateNode").await?;

    let node_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating node: {name} ({node_id}) in petal {petal_id}");
    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        node_id: crate::types::NodeId(node_id.clone()),
        op_type: crate::types::OpType::NodeCreated,
        payload: serde_json::json!({
            "node_id": node_id.clone(),
            "petal_id": petal_id,
            "name": name,
            "position": position,
            "interactive": false,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    let materialized_node_id = node_id.clone();
    let materialized_petal_id = petal_id.to_string();
    let materialized_name = name.to_string();
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
    crate::op_log::commit_operation(db, entry, move |_| async move {
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
        .bind(("node_id", materialized_node_id))
        .bind(("petal_id", materialized_petal_id))
        .bind(("name", materialized_name))
        .bind(("x", position[0] as f64))
        .bind(("z", position[2] as f64))
        .bind(("y", position[1] as f64))
        .bind(("now", now))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("CREATE empty node failed: {e}"))?;
        Ok(())
    })
    .await?;

    if let Err(e) = super::node_log::append_node_log(
        db,
        &node_id,
        "created",
        "local",
        &serde_json::json!({ "petal_id": petal_id, "name": name, "position": position }),
    )
    .await
    {
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
    require_petal_scope(db, &petal_id, "DeleteNode").await?;

    // Atomic cascade: delete the parent node row AND its waypoint children
    // (`properties.gpx_track_id == <this node_id>`, see
    // fractalengine/src/AGENTS.md §gpx) in one statement so a crash can't leave
    // a parent with its waypoints gone (inverse orphan). `RETURN BEFORE` on the
    // parent-matching predicate lets the matched-no-node bail still fire.
    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        node_id: crate::types::NodeId(node_id.to_string()),
        op_type: crate::types::OpType::NodeDeleted,
        // Direct waypoint children are selected by the materializer, so the
        // intent stays stable whether or not any exist at commit time.
        payload: serde_json::json!({
            "node_id": node_id,
            "petal_id": petal_id.clone(),
            "cascade_direct_gpx_waypoints": true,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    crate::op_log::commit_operation(db, entry, move |_| async move {
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
        let parent_deleted = deleted
            .iter()
            .any(|row| row.get("node_id").and_then(|v| v.as_str()) == Some(node_id));
        if !parent_deleted {
            anyhow::bail!("DeleteNode matched no node with node_id = {node_id}");
        }
        Ok(())
    })
    .await?;

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
        db,
        &node_id,
        "created",
        "local",
        &serde_json::json!({
            "petal_id": petal_id, "name": name, "position": position,
            "asset_id": asset_id, "asset_path": &asset_path,
        }),
    )
    .await
    {
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
            // FR-1: tombstoned nodes persist as soft-deleted rows but are
            // filtered from the hierarchy so the delete survives reload.
            "SELECT * FROM verse ORDER BY created_at ASC;\
             SELECT * FROM fractal ORDER BY created_at ASC;\
             SELECT * FROM petal ORDER BY created_at ASC;\
             SELECT * FROM node WHERE tombstone = NONE ORDER BY created_at ASC",
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
            if let (Some(aid), Some(ch)) = (row["asset_id"].as_str(), row["content_hash"].as_str())
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
                tracing::warn!("Blob missing for asset_id={aid} content_hash={ch}");
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
        tracing::warn!("Hierarchy asset missing for asset_id={aid} (no blob, no imported file)");
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
                            let fractal_name = f["name"].as_str().unwrap_or_default().to_string();

                            let petals: Vec<PetalHierarchyData> = petals_by_fractal
                                .get(&fractal_id)
                                .map(|ps| {
                                    ps.iter()
                                        .map(|p| {
                                            let petal_id = p["petal_id"]
                                                .as_str()
                                                .unwrap_or_default()
                                                .to_string();
                                            let petal_name =
                                                p["name"].as_str().unwrap_or_default().to_string();

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
                                                            let x =
                                                                coords[0].as_f64().unwrap_or(0.0)
                                                                    as f32;
                                                            let z =
                                                                coords[1].as_f64().unwrap_or(0.0)
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
    // FR-1: exclude tombstoned (soft-deleted) rows from the scene snapshot.
    let mut node_res: surrealdb::IndexedResults = db
        .query(
            "SELECT * FROM node WHERE petal_id = $pid AND tombstone = NONE ORDER BY created_at ASC",
        )
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
            if let (Some(aid), Some(ch)) = (row["asset_id"].as_str(), row["content_hash"].as_str())
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

/// Persisted node transform triple: (position, rotation, scale).
pub(crate) type NodeTransformRow = ([f32; 3], [f32; 3], [f32; 3]);

/// Read a single node's persisted transform (position, rotation, scale).
#[instrument(skip(db))]
pub(crate) async fn get_node_transform_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<NodeTransformRow>> {
    // FR-1: a tombstoned node has no live transform to read.
    let mut res: surrealdb::IndexedResults = db
        .query(
            "SELECT position, elevation, rotation, scale FROM node \
             WHERE node_id = $nid AND tombstone = NONE LIMIT 1",
        )
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

    Ok(Some(crate::build_scope(
        verse_id,
        Some(&fractal_id),
        Some(petal_id),
    )))
}

/// Resolve a node's owning petal for node-scoped event delivery.
#[instrument(skip(db))]
pub(crate) async fn resolve_node_petal_id_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("ResolveNodePetalId statement failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|row| row["petal_id"].as_str())
        .map(str::to_owned))
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
    let Some(petal_id) = resolve_node_petal_id_handler(db, node_id).await? else {
        return Ok(None);
    };
    resolve_petal_scope_handler(db, &petal_id).await
}

// ---------------------------------------------------------------------------
// Node lifecycle: sync-safe tombstone delete + lazy promotion
// (node_lifecycle_addressing_20260725 FR-1/FR-2/FR-5) — see AGENTS.md §lifecycle
// ---------------------------------------------------------------------------

/// Outcome of a durable tombstone / cascade op — the data the dispatch loop
/// needs to emit lifecycle events (FR-6) and re-flow hooks (FR-2).
#[derive(Debug, Clone, Default)]
pub(crate) struct TombstoneDurableOutcome {
    /// Owning petal (scopes the `SceneChange::NodeRemoved` mirror).
    pub petal_id: String,
    /// Owning-path node id if a tombstoned node was a stamp (FR-2 re-flow hook).
    pub reflow_path: Option<String>,
    /// The tombstoned stamp's `properties.instance_index` — threads into
    /// `LifecycleEvent::PathReflow.deleted_index` so T2's re-flow knows which
    /// slot vanished. `None` for non-stamp deletes.
    pub deleted_instance_index: Option<u32>,
    /// Node ids actually tombstoned by this op (empty on an idempotent no-op).
    /// Read by the DB-thread arms to gate lifecycle-event emission (FR-6): a
    /// no-op repeat must not re-emit. Also the basis for a future per-descendant
    /// `PathReflow` on cascade (FR-2 follow-up).
    pub tombstoned_ids: Vec<String>,
}

/// Lifecycle-relevant fields of one node row (see `read_node_lifecycle_meta`).
struct NodeLifecycleMeta {
    petal_id: String,
    reflow_path: Option<String>,
    instance_index: Option<u32>,
    already_tombstoned: bool,
}

/// Read a node's lifecycle meta, tolerating an absent `node` table (fresh DB).
/// `None` = no such node (matched-no-node).
async fn read_node_lifecycle_meta(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<Option<NodeLifecycleMeta>> {
    let lookup = db
        .query("SELECT petal_id, properties, tombstone FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("tombstone lookup query failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match lookup {
        Ok(mut res) => res
            .take(0)
            .map_err(|e| anyhow::anyhow!("tombstone lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("tombstone lookup statement failed: {e}")),
    };
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    let petal_id = row
        .get("petal_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    // A stamp records its owning path under `owning_path_id` (promoted instance)
    // or `gpx_track_id` (waypoint) — either drives the FR-2 re-flow hook.
    let reflow_path = row
        .get("properties")
        .and_then(|p| {
            p.get("owning_path_id")
                .or_else(|| p.get("gpx_track_id"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());
    let instance_index = row
        .get("properties")
        .and_then(|p| p.get("instance_index"))
        .and_then(|v| v.as_u64())
        .map(|i| i as u32);
    let already_tombstoned = row.get("tombstone").map(|t| !t.is_null()).unwrap_or(false);
    Ok(Some(NodeLifecycleMeta {
        petal_id,
        reflow_path,
        instance_index,
        already_tombstoned,
    }))
}

/// Build the durable tombstone marker object stamped onto a soft-deleted row.
fn tombstone_marker(hlc: u64, source_did: &str) -> serde_json::Value {
    serde_json::json!({
        "hlc": hlc,
        "source_did": source_did,
        "tombstoned_at": chrono::Utc::now().to_rfc3339(),
    })
}

/// Sync-safe delete (FR-1): **soft-delete** the node by stamping a durable
/// `tombstone` marker (HLC + source) on the row — never a raw row drop (N-4) —
/// and record a tombstone entry in the op-log. The row persists so the delete
/// survives reload and P2P/HLC merge; a stale replica cannot resurrect it
/// because the merge path honors the marker (see [`crate::merge`]). Reads filter
/// `tombstone = NONE`. Its direct gpx waypoint children are tombstoned in the
/// same atomic statement (inverse-orphan protection). See AGENTS.md §lifecycle.
#[instrument(skip(db))]
pub(crate) async fn tombstone_node_handler(
    db: &Db,
    node_id: &str,
    source_did: &str,
) -> anyhow::Result<TombstoneDurableOutcome> {
    let Some(meta) = read_node_lifecycle_meta(db, node_id).await? else {
        anyhow::bail!("TombstoneNode matched no node with node_id = {node_id}");
    };
    if meta.already_tombstoned {
        // Idempotent no-op — already tombstoned, nothing to re-record (N-4).
        return Ok(TombstoneDurableOutcome {
            petal_id: meta.petal_id,
            reflow_path: meta.reflow_path,
            deleted_instance_index: meta.instance_index,
            tombstoned_ids: Vec::new(),
        });
    }
    require_petal_scope(db, &meta.petal_id, "TombstoneNode").await?;

    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        node_id: crate::types::NodeId(node_id.to_string()),
        op_type: crate::types::OpType::NodeTombstoned,
        payload: serde_json::json!({
            "node_id": node_id,
            "petal_id": meta.petal_id.clone(),
            "source_did": source_did,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    let materialized_petal_id = meta.petal_id.clone();
    let materialized_reflow_path = meta.reflow_path.clone();
    let materialized_instance_index = meta.instance_index;
    let outcome = crate::op_log::commit_operation(db, entry, |lamport| async move {
        let marker = tombstone_marker(lamport, source_did);
        db.query(
            "UPDATE node SET tombstone = $ts \
             WHERE (node_id = $nid OR properties.gpx_track_id = $nid) AND tombstone = NONE",
        )
        .bind(("ts", marker))
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("TombstoneNode soft-delete query failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("TombstoneNode soft-delete statement failed: {e}"))?;

        Ok(TombstoneDurableOutcome {
            petal_id: materialized_petal_id,
            reflow_path: materialized_reflow_path,
            deleted_instance_index: materialized_instance_index,
            tombstoned_ids: vec![node_id.to_string()],
        })
    })
    .await?;

    tracing::info!(
        "Tombstoned node {node_id} (petal {}) — durable soft-delete + op-log",
        meta.petal_id
    );
    Ok(outcome)
}

/// BFS-collect the live (non-tombstoned) subtree rooted at `root` over the
/// durable parent edges, INCLUDING the root itself. Shared by cascade tombstone
/// (FR-2) and `CountNodeDescendants` so the two traversals can never drift.
/// Node children are `properties.parent_id`; gpx waypoints and promoted stamps
/// reference their parent via `gpx_track_id` / `owning_path_id`.
async fn collect_live_subtree(
    db: &Db,
    root: &str,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let mut collected: std::collections::HashSet<String> = std::collections::HashSet::new();
    collected.insert(root.to_string());
    let mut frontier: Vec<String> = vec![root.to_string()];
    // Depth cap: guards against a pathological/self-referential parent edge.
    for _ in 0..10_000 {
        if frontier.is_empty() {
            break;
        }
        let mut child_res = db
            .query(
                "SELECT node_id FROM node \
                 WHERE (properties.parent_id IN $frontier \
                    OR properties.gpx_track_id IN $frontier \
                    OR properties.owning_path_id IN $frontier) \
                   AND tombstone = NONE",
            )
            .bind(("frontier", frontier.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("Cascade child query failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("Cascade child statement failed: {e}"))?;
        let children: Vec<serde_json::Value> = child_res
            .take(0)
            .map_err(|e| anyhow::anyhow!("Cascade child take failed: {e}"))?;

        let mut next: Vec<String> = Vec::new();
        for c in &children {
            let Some(cid) = c.get("node_id").and_then(|v| v.as_str()) else {
                continue;
            };
            if collected.insert(cid.to_string()) {
                next.push(cid.to_string());
            }
        }
        frontier = next;
    }
    Ok(collected)
}

/// Count a node's live descendants, EXCLUDING the root itself (the UI copy is
/// "its N child nodes"). Read-only; shares `collect_live_subtree` with cascade.
#[instrument(skip(db))]
pub(crate) async fn count_node_descendants_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<usize> {
    let mut subtree = collect_live_subtree(db, node_id).await?;
    subtree.remove(node_id);
    Ok(subtree.len())
}

#[instrument(skip(db))]
pub(crate) async fn cascade_tombstone_node_handler(
    db: &Db,
    node_id: &str,
    source_did: &str,
) -> anyhow::Result<TombstoneDurableOutcome> {
    let Some(meta) = read_node_lifecycle_meta(db, node_id).await? else {
        anyhow::bail!("CascadeTombstoneNode matched no node with node_id = {node_id}");
    };
    if meta.already_tombstoned {
        // The root is the cascade's idempotency key. Do not append a second
        // intent, revisit descendants, or signal another scene transition.
        return Ok(TombstoneDurableOutcome {
            petal_id: meta.petal_id,
            reflow_path: None,
            deleted_instance_index: None,
            tombstoned_ids: Vec::new(),
        });
    }
    require_petal_scope(db, &meta.petal_id, "CascadeTombstoneNode").await?;
    let petal_id = meta.petal_id;

    let collected = collect_live_subtree(db, node_id).await?;
    let ids: Vec<String> = collected.into_iter().collect();
    let payload = serde_json::json!({
        "node_id": node_id,
        "petal_id": petal_id.clone(),
        "source_did": source_did,
        "cascade": true,
        "tombstoned_ids": ids.clone(),
    });

    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        node_id: crate::types::NodeId(node_id.to_string()),
        op_type: crate::types::OpType::NodeTombstoned,
        payload,
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    let materialized_petal_id = petal_id.clone();
    let materialized_ids = ids.clone();
    let outcome = crate::op_log::commit_operation(db, entry, |lamport| async move {
        let marker = tombstone_marker(lamport, source_did);
        db.query("UPDATE node SET tombstone = $ts WHERE node_id IN $ids")
            .bind(("ts", marker))
            .bind(("ids", materialized_ids.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("CascadeTombstoneNode query failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("CascadeTombstoneNode statement failed: {e}"))?;
        Ok(TombstoneDurableOutcome {
            petal_id: materialized_petal_id,
            reflow_path: None,
            deleted_instance_index: None,
            tombstoned_ids: materialized_ids,
        })
    })
    .await?;

    tracing::info!(
        "Cascade-tombstoned {} node(s) rooted at {node_id} (petal {petal_id})",
        ids.len()
    );
    Ok(outcome)
}

/// Lazy promotion (FR-5): materialize a full node row for a single stamp
/// instance on first individual select/edit. Idempotent — if the deterministic
/// instance node already exists, this is a no-op. Returns
/// `(node_id, newly_promoted)`.
#[instrument(skip(db))]
pub(crate) async fn promote_instance_handler(
    db: &Db,
    petal_id: &str,
    path_id: &str,
    instance_index: u32,
) -> anyhow::Result<(String, bool)> {
    require_petal_scope(db, petal_id, "PromoteInstance").await?;

    let node_id = format!("{path_id}#inst-{instance_index}");
    let now = chrono::Utc::now().to_rfc3339();

    // Idempotency probe (tolerate an absent `node` table on a fresh DB).
    let existing: Vec<serde_json::Value> = {
        let lookup = db
            .query("SELECT node_id FROM node WHERE node_id = $nid LIMIT 1")
            .bind(("nid", node_id.clone()))
            .await
            .map_err(|e| anyhow::anyhow!("PromoteInstance lookup failed: {e}"))?
            .check();
        match lookup {
            Ok(mut res) => res
                .take(0)
                .map_err(|e| anyhow::anyhow!("PromoteInstance take failed: {e}"))?,
            Err(e) if e.to_string().contains("does not exist") => Vec::new(),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "PromoteInstance lookup statement failed: {e}"
                ))
            }
        }
    };
    if !existing.is_empty() {
        return Ok((node_id, false)); // already promoted — idempotent no-op.
    }

    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        node_id: crate::types::NodeId(node_id.clone()),
        op_type: crate::types::OpType::NodePromoted,
        payload: serde_json::json!({
            "node_id": node_id.clone(),
            "petal_id": petal_id,
            "path_id": path_id,
            "instance_index": instance_index,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    crate::op_log::commit_operation(db, entry, |_| async {
        db.query(
            "CREATE node CONTENT {
                node_id: $node_id,
                petal_id: $petal_id,
                display_name: $name,
                asset_id: NONE,
                position: <geometry<point>> [0.0, 0.0],
                elevation: 0.0,
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                interactive: true,
                created_at: $now,
                properties: { node_kind: 'stamp', path_id: $path_id, \
                              owning_path_id: $path_id, instance_index: $idx },
            }",
        )
        .bind(("node_id", node_id.clone()))
        .bind(("petal_id", petal_id.to_string()))
        .bind(("name", format!("{path_id} #{instance_index}")))
        .bind(("path_id", path_id.to_string()))
        .bind(("idx", instance_index as i64))
        .bind(("now", now.clone()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("PromoteInstance CREATE failed: {e}"))?;
        Ok(())
    })
    .await?;

    tracing::info!("Promoted stamp instance {node_id} (path {path_id}) in petal {petal_id}");
    Ok((node_id, true))
}

/// Rename a node's `display_name` (authorized upstream, Editor+). Mirrors
/// `rename_entity_handler` for the `node` table; a missing or tombstoned node
/// is an error, never a silent success. See AGENTS.md §lifecycle.
#[instrument(skip(db))]
pub(crate) async fn rename_node_handler(
    db: &Db,
    node_id: &str,
    new_name: &str,
) -> anyhow::Result<()> {
    let update = db
        .query("UPDATE node SET display_name = $name WHERE node_id = $nid AND tombstone = NONE")
        .bind(("name", new_name.to_string()))
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("RenameNode query failed: {e}"))?
        .check();
    let updated: Vec<serde_json::Value> = match update {
        Ok(mut res) => res
            .take(0)
            .map_err(|e| anyhow::anyhow!("RenameNode take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("RenameNode statement failed: {e}")),
    };
    if updated.is_empty() {
        anyhow::bail!("RenameNode matched no live node with node_id = {node_id}");
    }
    Ok(())
}

/// Duplicated-node row data the dispatch arm needs to mirror the CreateNode
/// side effects (SceneChange, node_log, lifecycle event, `NodeCreated` result).
#[derive(Debug, Clone)]
pub(crate) struct DuplicateNodeOutcome {
    pub node_id: String,
    pub petal_id: String,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub has_asset: bool,
}

/// Drop the path-binding identity keys from a copied node's properties — a copy
/// is not path-bound, so it must not claim a stamp identity. Rule details in
/// AGENTS.md §lifecycle (duplicate).
fn strip_path_identity_properties(properties: &serde_json::Value) -> serde_json::Value {
    let Some(map) = properties.as_object() else {
        return serde_json::json!({});
    };
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        let strip = match key.as_str() {
            // Path-binding identity + curve-domain override never survive a copy.
            "owning_path_id" | "path_id" | "instance_index" | "stamp.override.arc_m" => true,
            // A copy is not a stamp; other kinds (e.g. earthwork_region) carry over.
            "node_kind" => value.as_str() == Some("stamp"),
            _ => false,
        };
        if !strip {
            out.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Duplicate a node (authorized upstream, Editor+): full row copy with a fresh
/// ulid id, `"{source} (copy)"` name, a +1.0 m x/z offset (raw petal-local
/// meters, N-1), and the path-binding identity keys stripped
/// (`strip_path_identity_properties`). A missing or tombstoned source is an
/// error. See AGENTS.md §lifecycle.
#[instrument(skip(db))]
pub(crate) async fn duplicate_node_handler(
    db: &Db,
    node_id: &str,
) -> anyhow::Result<DuplicateNodeOutcome> {
    let lookup = db
        .query("SELECT * FROM node WHERE node_id = $nid AND tombstone = NONE LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("DuplicateNode lookup query failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match lookup {
        Ok(mut res) => res
            .take(0)
            .map_err(|e| anyhow::anyhow!("DuplicateNode lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "DuplicateNode lookup statement failed: {e}"
            ))
        }
    };
    let Some(src) = rows.first() else {
        anyhow::bail!("DuplicateNode matched no live node with node_id = {node_id}");
    };

    let petal_id = src["petal_id"].as_str().unwrap_or_default().to_string();
    let name = format!(
        "{} (copy)",
        src["display_name"].as_str().unwrap_or_default()
    );
    // +1.0 m on x and z — raw petal-local meters (N-1).
    let coords = &src["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0) + 1.0;
    let z = coords[1].as_f64().unwrap_or(0.0) + 1.0;
    let y = src["elevation"].as_f64().unwrap_or(0.0);

    let rotation: [f64; 4] = {
        let arr = src["rotation"].as_array().cloned().unwrap_or_default();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0),
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0),
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0),
            arr.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0),
        ]
    };
    let scale: [f64; 3] = {
        let arr = src["scale"].as_array().cloned().unwrap_or_default();
        [
            arr.first().and_then(|v| v.as_f64()).unwrap_or(1.0),
            arr.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0),
            arr.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0),
        ]
    };
    let asset_id: Option<String> = src["asset_id"].as_str().map(String::from);
    let has_asset = asset_id.is_some();
    let interactive = src["interactive"].as_bool().unwrap_or(false);
    let properties = strip_path_identity_properties(&src["properties"]);

    let new_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
    db.query(
        "CREATE node CONTENT {
            node_id: $node_id,
            petal_id: $petal_id,
            display_name: $name,
            asset_id: $asset_id,
            position: <geometry<point>> [$x, $z],
            elevation: $y,
            rotation: $rotation,
            scale: $scale,
            interactive: $interactive,
            created_at: $now,
            properties: $props,
        }",
    )
    .bind(("node_id", new_id.clone()))
    .bind(("petal_id", petal_id.clone()))
    .bind(("name", name.clone()))
    .bind(("asset_id", asset_id))
    .bind(("x", x))
    .bind(("z", z))
    .bind(("y", y))
    .bind(("rotation", rotation.to_vec()))
    .bind(("scale", scale.to_vec()))
    .bind(("interactive", interactive))
    .bind(("now", now))
    .bind(("props", properties))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("DuplicateNode CREATE failed: {e}"))?;

    // Preserve the legacy node-log audit trail; duplicate intent is not yet a
    // separately specified legacy op-log operation.
    if let Err(e) = super::node_log::append_node_log(
        db,
        &new_id,
        "created",
        "local",
        &serde_json::json!({
            "petal_id": petal_id,
            "name": name,
            "position": [x, y, z],
            "duplicated_from": node_id,
        }),
    )
    .await
    {
        tracing::warn!("Failed to write node_log for duplicated node {new_id}: {e}");
    }

    tracing::info!("Duplicated node {node_id} -> {new_id} (petal {petal_id})");
    Ok(DuplicateNodeOutcome {
        node_id: new_id,
        petal_id,
        name,
        position: [x as f32, y as f32, z as f32],
        rotation: [
            rotation[0] as f32,
            rotation[1] as f32,
            rotation[2] as f32,
            rotation[3] as f32,
        ],
        scale: [scale[0] as f32, scale[1] as f32, scale[2] as f32],
        has_asset,
    })
}

// ---------------------------------------------------------------------------
// FR-1 tests: DeleteNode cascade
// ---------------------------------------------------------------------------

#[cfg(test)]
mod delete_node_tests {
    use super::*;

    /// In-memory SurrealDB, no DDL — `node` is written schemaless via
    /// `CREATE ... CONTENT {}` (mirrors production; see `create_node_handler`).
    /// Seeds the verse→fractal→petal chain so CreateNode scope preconditions resolve.
    async fn setup_mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', name: 'f' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p' };",
        )
        .await
        .expect("seed scope chain");
        crate::op_log::init_hlc(0);
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
        set_entity_property_helper(&db, &wp_id, "gpx_track_id", serde_json::json!(track_id)).await;

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
        assert!(
            rows2.is_empty(),
            "cascaded waypoint node row should be gone"
        );
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
    async fn set_entity_property_helper(
        db: &Db,
        node_id: &str,
        key: &str,
        value: serde_json::Value,
    ) {
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

// ---------------------------------------------------------------------------
// Cascade batch-update regression guard. The HLC-touching handler is exercised
// end-to-end by `tests/node_lifecycle_test.rs` in a separate process.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cascade_batch_update_tests {
    use super::*;

    /// Seeds the verse→fractal→petal chain so CreateNode scope preconditions resolve.
    async fn setup_mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', name: 'f' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p' };",
        )
        .await
        .expect("seed scope chain");
        // Every write path here reaches `next_hlc_timestamp`, which panics *while holding*
        // the process-global `HLC_STATE` lock when uninitialised — poisoning it for every
        // other test in this binary. The sibling `setup_mem_db` above does the same.
        crate::op_log::init_hlc(0);
        db
    }

    /// Count rows for `node_id` that the tombstone-filtered read path sees.
    async fn visible_count(db: &Db, node_id: &str) -> usize {
        let mut res = db
            .query("SELECT node_id FROM node WHERE node_id = $nid AND tombstone = NONE")
            .bind(("nid", node_id.to_string()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        rows.len()
    }

    #[tokio::test]
    async fn cascade_batch_update_tombstones_every_target() {
        let db = setup_mem_db().await;
        let a = create_node_handler(&db, "petal-1", "a", [0.0, 0.0, 0.0])
            .await
            .unwrap();
        let b = create_node_handler(&db, "petal-1", "b", [0.0, 0.0, 0.0])
            .await
            .unwrap();
        let ids = vec![a.clone(), b.clone()];
        let marker = tombstone_marker(1, "did:key:z6MkA");

        db.query("UPDATE node SET tombstone = $ts WHERE node_id IN $ids")
            .bind(("ts", marker))
            .bind(("ids", ids))
            .await
            .unwrap()
            .check()
            .unwrap();

        assert_eq!(visible_count(&db, &a).await, 0, "a must be tombstoned");
        assert_eq!(visible_count(&db, &b).await, 0, "b must be tombstoned");
    }
}
