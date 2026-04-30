use fe_runtime::messages::{
    FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
};
use crate::repo::{Db, Repo};
use crate::schema::{Fractal, Role, Verse};

use super::invite::{generate_namespace_secret, store_namespace_secret_in_keyring};
use crate::{
    hash_from_hex, hash_to_hex, imported_assets_dir, replicate_row, BlobStoreHandle,
    ReplicationSender, IMPORTED_ASSETS_SUBDIR,
};

// ---------------------------------------------------------------------------
// Verse
// ---------------------------------------------------------------------------

pub(crate) async fn create_verse_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    name: &str,
    local_did: &str,
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

    if let Err(e) = store_namespace_secret_in_keyring(&verse_id, &ns_secret_hex) {
        tracing::warn!("Could not store namespace secret in keyring for verse {verse_id}: {e}");
    }

    tracing::info!("Created verse: {name} ({verse_id}) namespace_id={ns_id_hex}");
    Ok(verse_id)
}

// ---------------------------------------------------------------------------
// Fractal
// ---------------------------------------------------------------------------

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

pub(crate) async fn create_petal_handler(
    db: &Db,
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

pub(crate) async fn create_node_handler(
    db: &Db,
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

// ---------------------------------------------------------------------------
// Import GLTF
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Load Hierarchy
// ---------------------------------------------------------------------------

pub(crate) async fn load_hierarchy_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<Vec<VerseHierarchyData>> {
    let mut verse_res: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse ORDER BY created_at ASC")
        .await?;
    let verses_raw: Vec<serde_json::Value> = verse_res.take(0)?;

    let mut verses = Vec::new();
    for v in &verses_raw {
        let verse_id = v["verse_id"].as_str().unwrap_or_default().to_string();
        let verse_name = v["name"].as_str().unwrap_or_default().to_string();
        let namespace_id = v["namespace_id"].as_str().map(|s| s.to_string());

        let mut frac_res: surrealdb::IndexedResults = db
            .query("SELECT * FROM fractal WHERE verse_id = $vid ORDER BY created_at ASC")
            .bind(("vid", verse_id.clone()))
            .await?;
        let fractals_raw: Vec<serde_json::Value> = frac_res.take(0)?;

        let mut fractals = Vec::new();
        for f in &fractals_raw {
            let fractal_id = f["fractal_id"].as_str().unwrap_or_default().to_string();
            let fractal_name = f["name"].as_str().unwrap_or_default().to_string();

            let mut petal_res: surrealdb::IndexedResults = db
                .query("SELECT * FROM petal WHERE fractal_id = $fid ORDER BY created_at ASC")
                .bind(("fid", fractal_id.clone()))
                .await?;
            let petals_raw: Vec<serde_json::Value> = petal_res.take(0)?;

            let mut petals = Vec::new();
            for p in &petals_raw {
                let petal_id = p["petal_id"].as_str().unwrap_or_default().to_string();
                let petal_name = p["name"].as_str().unwrap_or_default().to_string();

                let mut node_res: surrealdb::IndexedResults = db
                    .query("SELECT * FROM node WHERE petal_id = $pid ORDER BY created_at ASC")
                    .bind(("pid", petal_id.clone()))
                    .await?;
                let nodes_raw: Vec<serde_json::Value> = node_res.take(0)?;

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
                        let coords = &n["position"]["coordinates"];
                        let x = coords[0].as_f64().unwrap_or(0.0) as f32;
                        let z = coords[1].as_f64().unwrap_or(0.0) as f32;
                        let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

                        let asset_path = asset_id_str.as_ref().and_then(|aid| {
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

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

/// Resolve a `petal_id` to its full hierarchical scope string
/// (`VERSE#<v>-FRACTAL#<f>-PETAL#<p>`).
///
/// Returns `None` when the petal (or its parent fractal/verse chain) cannot be
/// found in the database.
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

