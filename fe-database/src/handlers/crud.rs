use tracing::instrument;

use crate::handlers::preconditions::require_petal_scope;

use crate::repo::{Db, Repo};
use crate::schema::{Fractal, Verse};
use fe_runtime::messages::{
    DbResult, FractalHierarchyData, NodeHierarchyData, PetalHierarchyData, VerseHierarchyData,
};

use super::invite::{generate_namespace_secret, store_namespace_secret};
use crate::{hash_from_hex, hash_to_hex, replicate_row, BlobStoreHandle, ReplicationSender};

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
) -> anyhow::Result<(String, String)> {
    crate::migration::mapping_inventory::record_bypass_for("CreateVerse");
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
        // M2/F6: explicit defaults on the manifest so peers and the sync
        // thread's fabric learn the settings from the verse row itself.
        ts_mode: fe_runtime::timeseries::TimeseriesMode::default()
            .as_str()
            .to_string(),
        ts_replication_factor: 1,
        ts_bucket_width_ms: fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS as i64,
        view_hidden: None,
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
    // F22: return the namespace id alongside the verse id so the caller can
    // carry it on `DbResult::VerseCreated` — the GUI tree entry then has the
    // id immediately and navigation opens the replica without a reload.
    Ok((verse_id, ns_id_hex))
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
    crate::migration::mapping_inventory::record_bypass_for("CreateFractal");
    let parent = super::entity::prepare_parent_mutation(
        db, fe_runtime::messages::EntityType::Verse, verse_id, local_did,
    ).await?;
    let fractal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let fractal_row = Fractal {
        fractal_id: fractal_id.clone(),
        verse_id: verse_id.to_string(),
        owner_did: local_did.to_string(),
        name: name.to_string(),
        description: Some(String::new()),
        created_at: now.clone(),
        view_hidden: None,
    };
    let row_json = serde_json::to_value(&fractal_row)?;

    parent.commit(db, &["CREATE fractal CONTENT $entity_new_fractal"],
        vec![("entity_new_fractal".into(), crate::repo::strip_json_nulls(row_json.clone()))]).await?;

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
    crate::migration::mapping_inventory::record_bypass_for("CreatePetal");
    let parent = super::entity::prepare_parent_mutation(
        db, fe_runtime::messages::EntityType::Fractal, fractal_id, local_did,
    ).await?;
    let petal_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    tracing::info!("Creating petal: {name} ({petal_id}) in fractal {fractal_id}");
    // Geometry fields need the explicit SurrealQL cast; see AGENTS.md §geometry-inserts.
    parent.commit(db, &[
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
    ], vec![
        ("petal_id".into(), serde_json::json!(petal_id)),
        ("fractal_id".into(), serde_json::json!(fractal_id)),
        ("name".into(), serde_json::json!(name)),
        ("node_id".into(), serde_json::json!(local_did)),
        ("now".into(), serde_json::json!(now)),
    ])
    .await
    .map_err(|e| anyhow::anyhow!("CREATE petal '{name}' failed: {e}"))?;
    // No implicit petal-owner role row (R7, T8): the creator keeps the role they already
    // resolve to from the enclosing scope — src/AGENTS.md §role-propagation.
    Ok(petal_id)
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

#[instrument(skip(db))]
pub(crate) async fn create_node_as(
    db: &Db,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
    actor: &str,
) -> anyhow::Result<String> {
    let (id, _) = super::node_history::create_object(
        db,
        petal_id,
        name,
        None,
        position,
        vec![0.0, 0.0, 0.0, 1.0],
        vec![1.0, 1.0, 1.0],
        serde_json::Map::new(),
        actor,
    )
    .await?;
    Ok(id)
}

/// Delete a node and cascade to its child waypoint rows.
///
/// Returns the node's `petal_id` (needed by the caller to scope the
/// `SceneChange::NodeRemoved` event). Bails if no node row matched — see
/// AGENTS.md §gis for the matched-rows-assertion convention.
#[cfg(test)]
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
    actor: &str,
) -> anyhow::Result<(String, String, String, fe_runtime::messages::CommittedNodeMaterialization)> {
    crate::migration::mapping_inventory::record_bypass_for("ImportGltf");
    let scope = require_petal_scope(db, petal_id, "ImportGltf").await?;
    super::node_history::authorize_scope(db, &fe_runtime::node_history::NodeAccessGrant::Local,
        actor, &scope, fe_policy::RoleLevel::Editor).await?;
    anyhow::ensure!(!name.trim().is_empty() && name.len() <= 1024 && position.iter().all(|v| v.is_finite()),
        "import requires a bounded name and finite position");
    let path = std::path::Path::new(file_path);
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("Could not read file {}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("glb");
    let content_type = if ext == "glb" { "model/gltf-binary" } else { "model/gltf+json" };
    fe_runtime::ktx2_guard::check_asset_bytes(&bytes, Some(ext))
        .map_err(|refusal| anyhow::anyhow!("Import refused for {}: {refusal}", path.display()))?;
    let hash = blob_store.add_blob(&bytes).map_err(|e| anyhow::anyhow!("blob_store.add_blob failed: {e}"))?;
    let imported = super::object_creation::ImportedSceneAsset::new(name, content_type, &hash_to_hex(&hash), bytes.len())?;
    let asset_id = imported.id().to_owned();
    let asset_path = imported.path().to_owned();
    let prepared = super::object_creation::prepare(db, super::object_creation::ObjectCreation {
        petal_id, name, asset: super::object_creation::SceneAsset::Imported(imported), position,
        rotation: vec![0.0,0.0,0.0,1.0], scale: vec![1.0;3], properties: serde_json::Map::new(),
        actor, grant: &fe_runtime::node_history::NodeAccessGrant::Local,
    }).await?;
    let current = super::object_creation::commit(db, prepared).await?;
    let node_id = current.node.node_id.clone();
    tracing::info!(node_id, asset_id, "Imported scene metadata and initial history committed");
    Ok((node_id, asset_id, asset_path, current))
}

// ---------------------------------------------------------------------------
// API asset ingest (F13 — fe-api/AGENTS.md §asset-ingest)
// ---------------------------------------------------------------------------

/// The metadata `DbCommand::CreateAsset` carries (no bytes), including the
/// ownership columns (fe-api/AGENTS.md §assets).
#[derive(Debug, Clone, Default)]
pub struct AssetRegistration {
    pub name: String,
    pub content_type: String,
    pub size_bytes: u64,
    /// 64-hex BLAKE3 hash of a blob already in the shared store.
    pub content_hash: String,
    pub owner_did: Option<String>,
    pub petal_id: Option<String>,
}

/// `CreateAsset`'s reply: `AssetCreated`, or the correlated failure. Shared by
/// the DB dispatch loop and tests that service the API→DB channel.
pub async fn create_asset_reply(
    db: &Db,
    blob_store: &BlobStoreHandle,
    registration: AssetRegistration,
    correlation_id: Option<String>,
) -> DbResult {
    match create_asset_handler(db, blob_store, &registration).await {
        Ok(asset_id) => DbResult::AssetCreated {
            asset_id,
            content_hash: registration.content_hash,
            size_bytes: registration.size_bytes,
            correlation_id,
        },
        Err(e) => super::correlated_failure(&correlation_id, format!("Create asset failed: {e}")),
    }
}

/// Register an asset row for a blob the API thread already stored. No bytes
/// reach this thread; the blob's presence in the shared store is verified.
#[instrument(skip(db, blob_store))]
pub(crate) async fn create_asset_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    registration: &AssetRegistration,
) -> anyhow::Result<String> {
    let AssetRegistration {
        name,
        content_type,
        size_bytes,
        content_hash,
        owner_did,
        petal_id,
    } = registration;
    let hash = hash_from_hex(content_hash)
        .map_err(|e| anyhow::anyhow!("CreateAsset invalid content_hash: {e}"))?;
    if !blob_store.has_blob(&hash) {
        anyhow::bail!("CreateAsset blob {content_hash} is not present in the blob store");
    }
    if name.trim().is_empty() || content_type.trim().is_empty() {
        anyhow::bail!("CreateAsset requires a name and content_type");
    }
    let size_bytes =
        i64::try_from(*size_bytes).map_err(|_| anyhow::anyhow!("CreateAsset size overflow"))?;

    let asset_id = ulid::Ulid::new().to_string();
    db.query(
        "CREATE asset CONTENT {
            asset_id: $asset_id,
            name: $name,
            content_type: $content_type,
            size_bytes: $size_bytes,
            data: NONE,
            content_hash: $content_hash,
            owner_did: $owner_did,
            petal_id: $petal_id,
            created_at: $now,
        }",
    )
    .bind(("asset_id", asset_id.clone()))
    .bind(("name", name.to_string()))
    .bind(("content_type", content_type.to_string()))
    .bind(("size_bytes", size_bytes))
    .bind(("content_hash", content_hash.to_string()))
    .bind(("owner_did", owner_did.clone()))
    .bind(("petal_id", petal_id.clone()))
    .bind(("now", chrono::Utc::now().to_rfc3339()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("CREATE asset failed: {e}"))?;
    tracing::info!("Registered API asset {name} ({size_bytes} bytes, asset_id={asset_id})");
    Ok(asset_id)
}

/// Create a node bound to an existing asset row (log-first, like `CreateNode`).
/// Returns `(node_id, asset_path)`; `rotation` is stored as Euler XYZ radians,
/// exactly as `UpdateNodeTransform` writes it.
#[instrument(skip(db))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_node_with_asset_as(
    db: &Db,
    petal_id: &str,
    name: &str,
    asset_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    actor: &str,
) -> anyhow::Result<(String, String, fe_runtime::messages::CommittedNodeMaterialization)> {
    let current = super::node_history::create_object_materialized_granted(
        db,
        petal_id,
        name,
        Some(asset_id),
        position,
        rotation.iter().map(|&v| v as f64).collect(),
        scale.iter().map(|&v| v as f64).collect(),
        serde_json::Map::new(),
        actor,
        &fe_runtime::node_history::NodeAccessGrant::Local,
    )
    .await?;
    Ok((
        current.node.node_id.clone(),
        current.node.asset_path.clone().ok_or_else(|| anyhow::anyhow!("scene asset path missing"))?,
        current,
    ))
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
    // Blob store is the only asset source (src/AGENTS.md §asset-resolution).
    let resolve_asset_path = |aid: &str| -> Option<String> {
        let Some(ch) = asset_hash_map.get(aid) else {
            tracing::warn!("Hierarchy asset missing for asset_id={aid} (no content_hash)");
            return None;
        };
        match hash_from_hex(ch) {
            Ok(hash) if blob_store.has_blob(&hash) => Some(format!("blob://{ch}.glb")),
            Ok(_) => {
                tracing::warn!("Blob missing for asset_id={aid} content_hash={ch}");
                None
            }
            Err(e) => {
                tracing::warn!("Invalid content_hash for asset_id={aid}: {e}");
                None
            }
        }
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
                                                            let stored_rotation = n["rotation"].as_array().map(|items| items.iter().filter_map(serde_json::Value::as_f64).collect::<Vec<_>>()).unwrap_or_else(|| vec![0.0; 3]);
                                                            let rotation = fe_runtime::node_history::stored_rotation_quaternion(&stored_rotation).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                                                            let scale = std::array::from_fn(|axis| n["scale"][axis].as_f64().filter(|value| value.is_finite() && (*value as f32).is_finite()).unwrap_or(1.0) as f32);
                                                            NodeHierarchyData {
                                                                rotation, scale,
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
                                                view_hidden: p["view_hidden"]
                                                    .as_bool()
                                                    .unwrap_or(false),
                                                // p2p_asset_streaming S6: durable
                                                // residency horizon; `None` = inherit.
                                                render_distance: p["render_distance"]
                                                    .as_f64()
                                                    .map(|v| v as f32),
                                                nodes,
                                            }
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            FractalHierarchyData {
                                id: fractal_id,
                                name: fractal_name,
                                view_hidden: f["view_hidden"].as_bool().unwrap_or(false),
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
                // M2/F6: surface the persisted ts_* columns (missing/invalid
                // values fall back to the mirror defaults — from_verse_row).
                timeseries: fe_runtime::timeseries::VerseTimeseriesSettings::from_verse_row(v),
                view_hidden: v["view_hidden"].as_bool().unwrap_or(false),
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

/// Resolve a `fractal_id` to its scope string (`VERSE#<v>-FRACTAL#<f>`);
/// `None` when the fractal (or its verse link) is absent.
#[instrument(skip(db))]
pub(crate) async fn resolve_fractal_scope_handler(
    db: &Db,
    fractal_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT verse_id FROM fractal WHERE fractal_id = $fid LIMIT 1")
        .bind(("fid", fractal_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let Some(verse_id) = rows.first().and_then(|r| r["verse_id"].as_str()) else {
        return Ok(None);
    };
    Ok(Some(crate::build_scope(verse_id, Some(fractal_id), None)))
}

/// Resolve a `verse_id` to its scope string (`VERSE#<v>`); `None` when no
/// verse row exists (existence is the point — the id alone is not proof).
#[instrument(skip(db))]
pub(crate) async fn resolve_verse_scope_handler(
    db: &Db,
    verse_id: &str,
) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT verse_id FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|r| r["verse_id"].as_str())
        .map(|v| crate::build_scope(v, None, None)))
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
pub(crate) fn tombstone_marker(hlc: u64, source_did: &str) -> serde_json::Value {
    serde_json::json!({
        "hlc": hlc,
        "source_did": source_did,
        "tombstoned_at": chrono::Utc::now().to_rfc3339(),
    })
}

/// Append a `NodeTombstoned` entry and run `update` (which sets `tombstone = $ts`)
/// in ONE transaction; `$ts` is the marker stamped with the op's own lamport
/// clock. See AGENTS.md §lifecycle.
async fn commit_tombstone_atomic(
    db: &Db,
    mut entry: crate::types::OpLogEntry,
    update: &str,
    target: (&str, serde_json::Value),
    source_did: &str,
) -> anyhow::Result<()> {
    let (target_name, target_value) = target;
    let petal = entry.payload["petal_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("delete Petal missing"))?
        .to_owned();
    let scope = require_petal_scope(db, &petal, "TombstoneNode").await?;
    let permission = super::entity::prepare_scope_permission(
        db,
        &scope,
        source_did,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let predicate = match target_name {
        "nid" => "(node_id=$nid OR properties.gpx_track_id=$nid)",
        "ids" => "node_id IN $ids",
        _ => anyhow::bail!("unknown delete target binding"),
    };
    let select=format!("SELECT node_id,petal_id,tombstone FROM node WHERE {predicate} AND tombstone=NONE ORDER BY node_id LIMIT 1025 TIMEOUT 5s");
    let mut response = db
        .query(&select)
        .bind((target_name.to_owned(), target_value.clone()))
        .await?
        .check()?;
    let members: Vec<serde_json::Value> = response.take(0)?;
    anyhow::ensure!(
        !members.is_empty() && members.len() <= 1024,
        "delete requires 1..1024 current members"
    );
    anyhow::ensure!(
        members.iter().all(|row| row["petal_id"] == petal),
        "delete crosses stored Petal ancestry"
    );
    let ids = members
        .iter()
        .map(|row| row["node_id"].clone())
        .collect::<Vec<_>>();
    entry.payload["tombstoned_ids"] = serde_json::json!(ids);
    let member_select=format!("SELECT node_id,petal_id,tombstone FROM node WHERE {predicate} AND tombstone=NONE ORDER BY node_id");
    guards.push(format!(
        "IF ({member_select}) != $delete_members {{ THROW 'delete member set changed'; }}"
    ));
    guards.push(update.to_owned());
    bindings.extend([
        ("delete_members".into(), serde_json::json!(members)),
        (target_name.into(), target_value),
    ]);
    crate::op_log::commit_operation_atomic_with(db, entry, |lamport| {
        bindings.push(("ts".into(), tombstone_marker(lamport, source_did)));
        (guards, bindings)
    })
    .await?;
    Ok(())
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
    commit_tombstone_atomic(
        db,
        entry,
        "UPDATE node SET tombstone = $ts \
         WHERE (node_id = $nid OR properties.gpx_track_id = $nid) AND tombstone = NONE",
        ("nid", serde_json::json!(node_id)),
        source_did,
    )
    .await
    .map_err(|e| e.context("TombstoneNode soft-delete failed"))?;
    let outcome = TombstoneDurableOutcome {
        petal_id: meta.petal_id.clone(),
        reflow_path: meta.reflow_path.clone(),
        deleted_instance_index: meta.instance_index,
        tombstoned_ids: vec![node_id.to_string()],
    };

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
    commit_tombstone_atomic(
        db,
        entry,
        "UPDATE node SET tombstone = $ts WHERE node_id IN $ids",
        ("ids", serde_json::json!(ids)),
        source_did,
    )
    .await
    .map_err(|e| e.context("CascadeTombstoneNode failed"))?;
    let outcome = TombstoneDurableOutcome {
        petal_id: petal_id.clone(),
        reflow_path: None,
        deleted_instance_index: None,
        tombstoned_ids: ids.clone(),
    };

    tracing::info!(
        "Cascade-tombstoned {} node(s) rooted at {node_id} (petal {petal_id})",
        ids.len()
    );
    Ok(outcome)
}

/// The `petal_id` of any node row (tombstoned included) with this id; `None`
/// when absent or when the `node` table does not exist yet.
async fn node_petal_id(db: &Db, node_id: &str) -> anyhow::Result<Option<String>> {
    let lookup = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("node petal lookup failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match lookup {
        Ok(mut res) => res
            .take(0)
            .map_err(|e| anyhow::anyhow!("node petal take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => anyhow::bail!("node petal lookup statement failed: {e}"),
    };
    Ok(rows
        .first()
        .and_then(|row| row.get("petal_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
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
    publisher: &str,
    actor: &str,
    expected_scope: &str,
) -> anyhow::Result<(String, bool)> {
    let scope = require_petal_scope(db, petal_id, "PromoteInstance").await?;
    anyhow::ensure!(scope == expected_scope, "promotion ancestry changed");

    // DEC-C21 M7: the owning path must belong to the AUTHORIZED petal. A
    // foreign path_id would mint `<foreign>#inst-n` here (squatting the
    // victim's deterministic instance id) — or, via the idempotency probe,
    // hand back a node from another petal. An absent path is tolerated (ids
    // are unguessable ULIDs, so it cannot name another petal's live path).
    let node_id = format!("{path_id}#inst-{instance_index}");
    let path_owner = node_petal_id(db, path_id).await?;
    let target_owner = node_petal_id(db, &node_id).await?;
    for (probe_id, owner) in [
        (path_id, path_owner.as_deref()),
        (node_id.as_str(), target_owner.as_deref()),
    ] {
        if owner.is_some_and(|owner| owner != petal_id) {
            anyhow::bail!(
                "PromoteInstance: node {probe_id} belongs to another petal, not {petal_id}"
            );
        }
    }

    let permission = super::entity::prepare_scope_permission(
        db,
        &scope,
        publisher,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let (mut statements, mut bindings) = permission.into_parts();
    let (ancestry, ancestry_bindings) = super::node_history::scope_fence(&scope, None)?;
    statements.extend(ancestry);
    bindings.extend(ancestry_bindings);
    statements.push("IF (SELECT VALUE petal_id FROM node WHERE node_id=$promotion_owner_path) != $promotion_owner_snapshot { THROW 'promotion path owner changed'; }".into());
    bindings.extend([
        ("promotion_owner_path".into(), serde_json::json!(path_id)),
        (
            "promotion_owner_snapshot".into(),
            serde_json::json!(path_owner.iter().collect::<Vec<_>>()),
        ),
    ]);

    // Idempotency probe (tolerate an absent `node` table on a fresh DB).
    let existing: Vec<serde_json::Value> = {
        let lookup = db
            .query("SELECT node_id, petal_id, tombstone, properties FROM node WHERE node_id = $nid LIMIT 2")
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
                ));
            }
        }
    };
    if !existing.is_empty() {
        anyhow::ensure!(existing.len() == 1, "ambiguous promoted stamp identity");
        let target = &existing[0];
        anyhow::ensure!(
            target["petal_id"] == petal_id
                && target
                    .get("tombstone")
                    .is_none_or(serde_json::Value::is_null)
                && target["properties"]["node_kind"] == "stamp"
                && target["properties"]["path_id"] == path_id
                && target["properties"]["owning_path_id"] == path_id
                && target["properties"]["instance_index"].as_u64()
                    == Some(u64::from(instance_index)),
            "stamp node identity collision"
        );
        statements.push("IF (SELECT node_id, petal_id, tombstone, properties FROM node WHERE node_id=$promotion_existing_id) != $promotion_existing_snapshot { THROW 'promoted stamp changed'; }".into());
        bindings.extend([
            ("promotion_existing_id".into(), serde_json::json!(node_id)),
            (
                "promotion_existing_snapshot".into(),
                serde_json::json!(existing),
            ),
        ]);
        let script = format!(
            "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
            statements.join(";\n")
        );
        let mut query = db.query(script);
        for (key, value) in bindings {
            query = query.bind((key, value));
        }
        query.await?.check()?;
        return Ok((node_id, false));
    }

    let properties = serde_json::json!({"node_kind":"stamp","path_id":path_id,"owning_path_id":path_id,"instance_index":instance_index});
    let birth = super::stamp_promotion::prepare_birth(
        db,
        super::stamp_promotion::StampBirthInput {
            node_id: &node_id,
            petal_id,
            path_id,
            index: instance_index,
            scope: &scope,
            actor,
            properties: &properties,
        },
    )
    .await?;
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
    crate::op_log::commit_operation_atomic_with(db, entry, |hlc| {
        let (birth_statements, birth_bindings) = birth.into_parts(hlc);
        statements.extend(birth_statements);
        bindings.extend(birth_bindings);
        (statements, bindings)
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
    publisher: &str,
    actor: &str,
) -> anyhow::Result<()> {
    crate::migration::mapping_inventory::record_bypass_for("RenameNode");
    anyhow::ensure!(
        !new_name.trim().is_empty() && new_name.len() <= 1024,
        "invalid node name"
    );
    let scope = resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node scope unavailable"))?;
    let permission = super::entity::prepare_scope_permission(
        db,
        &scope,
        publisher,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let mut response=db.query("SELECT * OMIT id FROM node WHERE node_id=$rename_node AND tombstone=NONE LIMIT 2 TIMEOUT 5s")
        .bind(("rename_node",node_id.to_owned())).await?.check()?;
    let mut before: Vec<serde_json::Value> = response.take(0)?;
    anyhow::ensure!(before.len() == 1, "RenameNode matched no unique live node");
    let mut row = before.remove(0);
    let point = row.as_object_mut().unwrap().remove("position").unwrap();
    guards.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$rename_node) != [$rename_before] { THROW 'rename source changed'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$rename_node AND position=<geometry<point>> [$rename_x,$rename_z])) != 1 { THROW 'rename source moved'; }".into(),
        "UPDATE node SET display_name=$rename_name WHERE node_id=$rename_node AND tombstone=NONE".into(),
    ]);
    bindings.extend([
        ("rename_node".into(), serde_json::json!(node_id)),
        ("rename_before".into(), row),
        ("rename_x".into(), point["coordinates"][0].clone()),
        ("rename_z".into(), point["coordinates"][1].clone()),
        ("rename_name".into(), serde_json::json!(new_name)),
    ]);
    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: crate::types::NodeId(node_id.into()),
        op_type: crate::types::OpType::NodeRenamed,
        payload: serde_json::json!({"node_id":node_id,"name":new_name}),
        sig: "00".repeat(64),
    };
    super::node_history::commit_mutation(
        db,
        entry,
        guards,
        bindings,
        actor,
        "node_renamed",
        serde_json::json!({"name":new_name}),
        serde_json::json!({}),
        &fe_runtime::node_history::NodeAccessGrant::Scoped(scope),
    )
    .await?;
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
    publisher: &str,
    actor: &str,
) -> anyhow::Result<DuplicateNodeOutcome> {
    crate::migration::mapping_inventory::record_bypass_for("DuplicateNode");
    let lookup = db
        .query("SELECT * OMIT id FROM node WHERE node_id = $nid AND tombstone = NONE LIMIT 2")
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
            ));
        }
    };
    anyhow::ensure!(rows.len() <= 1, "ambiguous duplicate source");
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

    let rotation_values: Vec<f64> = serde_json::from_value(src["rotation"].clone())?;
    let rotation = fe_runtime::node_history::stored_rotation_quaternion(&rotation_values)
        .ok_or_else(|| anyhow::anyhow!("duplicate source rotation unavailable"))?
        .map(f64::from);
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
    let properties = strip_path_identity_properties(&src["properties"]);
    let scope = resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("duplicate source scope unavailable"))?;
    super::entity::prepare_scope_permission(db, &scope, publisher, fe_policy::RoleLevel::Editor)
        .await?;
    let mut prepared = super::object_creation::prepare(
        db,
        super::object_creation::ObjectCreation {
            petal_id: &petal_id,
            name: &name,
            asset: super::object_creation::SceneAsset::Existing(asset_id.as_deref()),
            position: [x as f32, y as f32, z as f32],
            rotation: rotation_values,
            scale: scale.to_vec(),
            properties: properties.as_object().unwrap().clone(),
            actor,
            grant: &fe_runtime::node_history::NodeAccessGrant::Scoped(scope.clone()),
        },
    )
    .await?;
    prepared.preserve_duplicate_fields([x, y, z], src["interactive"].as_bool().unwrap_or(false))?;
    prepared.guard_source(node_id, src)?;
    prepared.guard_publisher(db, &scope, publisher).await?;
    let materialized = super::object_creation::commit(db, prepared).await?;
    let new_id = materialized.node.node_id;

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
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v', created_by:'local' };
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
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v', created_by:'local' };
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

#[cfg(test)]
mod asset_ingest_tests {
    use super::*;
    use crate::{BlobHash, BlobStore};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    /// Presence-only blob store: the DB thread only ever asks `has_blob`.
    #[derive(Default)]
    struct PresenceStore(Mutex<HashSet<BlobHash>>);

    impl BlobStore for PresenceStore {
        fn add_blob(&self, _bytes: &[u8]) -> anyhow::Result<BlobHash> {
            anyhow::bail!("the DB thread must never write blob bytes")
        }
        fn get_blob_path(&self, _hash: &BlobHash) -> Option<std::path::PathBuf> {
            None
        }
        fn has_blob(&self, hash: &BlobHash) -> bool {
            self.0.lock().unwrap().contains(hash)
        }
        fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn setup() -> (Db, BlobStoreHandle, String) {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("asset creation schema");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v', created_by:'local', created_at:'2026-11-02' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', owner_did:'local', name: 'f', created_at:'2026-11-02' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p', node_id:'anchor', created_at:'2026-11-02' };",
        )
        .await
        .expect("seed scope chain");
        crate::op_log::init_hlc(0);
        let store = Arc::new(PresenceStore::default());
        let hash = [7u8; 32];
        store.0.lock().unwrap().insert(hash);
        (db, store, hash_to_hex(&hash))
    }

    #[tokio::test]
    async fn create_asset_requires_the_blob_and_stores_metadata_only() {
        let (db, store, hash) = setup().await;

        let absent = hash_to_hex(&[9u8; 32]);
        let registration = |content_hash: &str| AssetRegistration {
            name: "a.glb".into(),
            content_type: "model/gltf-binary".into(),
            size_bytes: 12,
            content_hash: content_hash.into(),
            owner_did: Some("did:key:z6MkUploader".into()),
            petal_id: Some("petal-1".into()),
        };
        let err = create_asset_handler(&db, &store, &registration(&absent))
            .await
            .expect_err("absent blob must be refused");
        assert!(err.to_string().contains("not present"), "{err}");

        let asset_id = create_asset_handler(&db, &store, &registration(&hash))
            .await
            .expect("present blob registers");
        let mut res = db
            .query(
                "SELECT content_hash, data, size_bytes, owner_did, petal_id FROM asset                  WHERE asset_id = $aid",
            )
            .bind(("aid", asset_id))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        assert_eq!(rows[0]["content_hash"], hash.as_str());
        assert!(rows[0]["data"].is_null(), "no inline bytes: {}", rows[0]);
        assert_eq!(rows[0]["size_bytes"], 12);
        assert_eq!(rows[0]["owner_did"], "did:key:z6MkUploader");
        assert_eq!(rows[0]["petal_id"], "petal-1");
    }

    #[tokio::test]
    async fn place_asset_binds_the_asset_and_rejects_unknown_ids() {
        let (db, store, hash) = setup().await;

        let err = create_node_with_asset_handler(
            &db,
            "petal-1",
            "ghost",
            "no-such-asset",
            [0.0; 3],
            [0.0; 3],
            [1.0; 3],
        )
        .await
        .expect_err("unknown asset must be refused");
        assert!(err.to_string().contains("matched no asset"), "{err}");
        let mut res = db.query("SELECT node_id FROM node").await.unwrap();
        // An absent `node` table is the strongest form of "no row".
        let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
        assert!(rows.is_empty(), "no node row on a refused placement");

        let asset_id = create_asset_handler(
            &db,
            &store,
            &AssetRegistration {
                name: "a.glb".into(),
                content_type: "model/gltf-binary".into(),
                size_bytes: 12,
                content_hash: hash.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (node_id, asset_path, current) = create_node_with_asset_as(
            &db,
            "petal-1",
            "house",
            &asset_id,
            [1.0, 2.0, 3.0],
            [0.5, 0.0, 0.0],
            [2.0; 3],
            "local",
        )
        .await
        .expect("placement succeeds");
        assert_eq!(asset_path, format!("blob://{hash}.glb"));
        assert_eq!(current.history_version,1);
        assert_eq!(current.node.position,[1.0,2.0,3.0]);
        assert_eq!(current.node.scale,[2.0;3]);
        assert_eq!(current.rotation_euler,[0.5,0.0,0.0]);
        assert!(bevy::math::Quat::from_array(current.node.rotation).dot(bevy::math::Quat::from_euler(bevy::math::EulerRot::XYZ,0.5,0.0,0.0)).abs()>1.0-1e-6);
        let mut res = db
            .query("SELECT asset_id, rotation, elevation FROM node WHERE node_id = $nid")
            .bind(("nid", node_id))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        assert_eq!(rows[0]["asset_id"], asset_id.as_str());
        assert_eq!(rows[0]["rotation"], serde_json::json!([0.5, 0.0, 0.0]));
        assert_eq!(rows[0]["elevation"], 2.0);
    }

    /// CDL-SPEC10 Phase A security: an R5 device-payload asset (declared
    /// non-scene MIME) is refused at the DB-thread authority, so no
    /// `blob://<hash>.bin` scene path is ever handed to the AssetServer even
    /// if a caller skips fe-api's `place_asset_core` guard.
    #[tokio::test]
    async fn place_asset_refuses_a_non_scene_asset_and_writes_no_node() {
        let (db, store, hash) = setup().await;
        let asset_id = create_asset_handler(
            &db,
            &store,
            &AssetRegistration {
                name: "frame.html".into(),
                content_type: "text/html".into(),
                size_bytes: 12,
                content_hash: hash,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let err = create_node_with_asset_handler(
            &db, "petal-1", "frame", &asset_id, [0.0; 3], [0.0; 3], [1.0; 3],
        )
        .await
        .expect_err("a non-scene asset must not become a node");
        assert!(err.to_string().contains("not a scene asset"), "{err}");
        let mut res = db.query("SELECT node_id FROM node").await.unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
        assert!(rows.is_empty(), "no node row on a refused placement");
    }

    /// DEC-C21 M7: promotion refuses a path_id owned by another petal (no
    /// squatted `<foreign>#inst-n` row); an own-petal path promotes normally.
    #[tokio::test]
    async fn promote_instance_rejects_a_foreign_petal_path() {
        let (db, _, _) = setup().await;
        db.query(
            "CREATE petal CONTENT { petal_id: 'petal-2', fractal_id: 'fractal-1', name: 'p2' };
             CREATE node CONTENT { node_id: 'path-foreign', petal_id: 'petal-2', display_name: 'f' };
             CREATE node CONTENT { node_id: 'path-own', petal_id: 'petal-1', display_name: 'o' };",
        )
        .await
        .expect("seed paths");

        let err = promote_instance_handler(
            &db,
            "petal-1",
            "path-foreign",
            0,
            "local",
            "local",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        )
        .await
        .expect_err("foreign path must be refused");
        assert!(err.to_string().contains("another petal"), "{err}");
        assert_eq!(
            node_petal_id(&db, "path-foreign#inst-0").await.unwrap(),
            None
        );

        let (node_id, newly) = promote_instance_handler(
            &db,
            "petal-1",
            "path-own",
            0,
            "local",
            "local",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        )
        .await
        .expect("own-petal path promotes");
        assert_eq!(node_id, "path-own#inst-0");
        assert!(newly);
        assert_eq!(
            node_petal_id(&db, &node_id).await.unwrap().as_deref(),
            Some("petal-1")
        );

        let baseline = super::super::node_history::latest(&db, &node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(baseline["row_version"], 1);
        assert_eq!(baseline["source_did"], "local");
        assert_eq!(baseline["payload"]["coverage_reason"], "stamp_promoted");
        assert_eq!(
            baseline["payload"]["after_state"],
            super::super::node_history::snapshot(&db, &node_id)
                .await
                .unwrap()
                .unwrap()
        );
        let (_, newly) = promote_instance_handler(
            &db,
            "petal-1",
            "path-own",
            0,
            "local",
            "local",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        )
        .await
        .unwrap();
        assert!(!newly);
        assert_eq!(
            super::super::node_history::latest(&db, &node_id)
                .await
                .unwrap()
                .unwrap(),
            baseline
        );
        db.query("CREATE node CONTENT {node_id:'path-own#inst-1',petal_id:'petal-1',properties:{node_kind:'stamp',path_id:'path-own',owning_path_id:'path-own',instance_index:1}}").await.unwrap().check().unwrap();
        let (_, newly) = promote_instance_handler(
            &db,
            "petal-1",
            "path-own",
            1,
            "local",
            "local",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        )
        .await
        .unwrap();
        assert!(!newly);
        assert!(
            super::super::node_history::latest(&db, "path-own#inst-1")
                .await
                .unwrap()
                .is_none(),
            "legacy rows gain no invented prior coverage"
        );
        promote_instance_handler(
            &db,"petal-1","path-own",1,"foreign","foreign",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        ).await.expect_err("existing target still requires current publisher authority");
        let before_ops: Vec<serde_json::Value> = db
            .query("SELECT * OMIT id FROM op_log ORDER BY hlc_timestamp")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        for mutation in [
            "UPDATE node SET properties.node_kind='other' WHERE node_id='path-own#inst-1'",
            "UPDATE node SET properties.path_id='other' WHERE node_id='path-own#inst-1'",
            "UPDATE node SET properties.owning_path_id='other' WHERE node_id='path-own#inst-1'",
            "UPDATE node SET properties.instance_index=9 WHERE node_id='path-own#inst-1'",
            "UPDATE node SET tombstone={hlc:1,source_did:'local'} WHERE node_id='path-own#inst-1'",
            "CREATE node:duplicate_existing_stamp CONTENT {node_id:'path-own#inst-1',petal_id:'petal-1',properties:{node_kind:'stamp',path_id:'path-own',owning_path_id:'path-own',instance_index:1}}",
        ] {
            db.query(mutation).await.unwrap().check().unwrap();
            promote_instance_handler(&db,"petal-1","path-own",1,"local","local","VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1").await.expect_err("incompatible existing target must be refused");
            assert!(super::super::node_history::latest(&db,"path-own#inst-1").await.unwrap().is_none());
            let after_ops:Vec<serde_json::Value>=db.query("SELECT * OMIT id FROM op_log ORDER BY hlc_timestamp").await.unwrap().check().unwrap().take(0).unwrap();
            assert_eq!(after_ops,before_ops,"refused no-op writes no intention");
            db.query("DELETE node:duplicate_existing_stamp; UPDATE node SET tombstone=NONE,properties={node_kind:'stamp',path_id:'path-own',owning_path_id:'path-own',instance_index:1} WHERE node_id='path-own#inst-1'").await.unwrap().check().unwrap();
        }
        db.query("DEFINE EVENT fail_legacy_stamp_birth ON TABLE node_log WHEN $after.node_id='path-own#inst-2' THEN { THROW 'induced initial history failure' }").await.unwrap().check().unwrap();
        promote_instance_handler(
            &db,
            "petal-1",
            "path-own",
            2,
            "local",
            "local",
            "VERSE#verse-1-FRACTAL#fractal-1-PETAL#petal-1",
        )
        .await
        .expect_err("initial fact failure rolls back row and intention");
        assert!(node_petal_id(&db, "path-own#inst-2")
            .await
            .unwrap()
            .is_none());
        assert!(super::super::node_history::latest(&db, "path-own#inst-2")
            .await
            .unwrap()
            .is_none());
        let ops: Vec<serde_json::Value> = db
            .query("SELECT * FROM op_log WHERE node_id='path-own#inst-2'")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert!(ops.is_empty());
        db.query("UPDATE node SET tombstone={hlc:1,source_did:'local'} WHERE node_id=$id")
            .bind(("id", node_id.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
        super::super::entity_property::get_entity_properties_handler(&db, &node_id)
            .await
            .expect_err("correlated property fallback must not expose tombstoned bags");
    }

    #[tokio::test]
    async fn fractal_and_verse_scopes_resolve_only_for_existing_rows() {
        let (db, _, _) = setup().await;
        assert_eq!(
            resolve_fractal_scope_handler(&db, "fractal-1")
                .await
                .unwrap(),
            Some("VERSE#verse-1-FRACTAL#fractal-1".to_string())
        );
        assert_eq!(
            resolve_fractal_scope_handler(&db, "nope").await.unwrap(),
            None
        );
        assert_eq!(
            resolve_verse_scope_handler(&db, "verse-1").await.unwrap(),
            Some("VERSE#verse-1".to_string())
        );
        assert_eq!(
            resolve_verse_scope_handler(&db, "nope").await.unwrap(),
            None
        );
    }
}

#[cfg(test)]
mod tombstone_atomicity_tests {
    use super::*;

    /// Schema-applied in-memory DB with a verse→fractal→petal chain and a root
    /// node carrying two `properties.parent_id` children. Returns `(db, root, ids)`.
    async fn seeded_subtree() -> (Db, String, Vec<String>) {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v', created_by: 'did:key:z6MkA', created_at: '2026-01-01' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', owner_did: 'did:key:z6MkA', name: 'f', created_at: '2026-01-01' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p', node_id: 'n', created_at: '2026-01-01' };",
        )
        .await
        .expect("seed scope chain")
        .check()
        .expect("seed scope chain statements");
        crate::op_log::init_hlc(0);
        let root = create_node_as(&db, "petal-1", "root", [0.0, 0.0, 0.0], "did:key:z6MkA")
            .await
            .expect("create root");
        let mut ids = vec![root.clone()];
        for name in ["child-a", "child-b"] {
            let child = create_node_as(&db, "petal-1", name, [1.0, 0.0, 0.0], "did:key:z6MkA")
                .await
                .expect("create child");
            db.query("UPDATE node SET properties.parent_id = $root WHERE node_id = $child")
                .bind(("root", root.clone()))
                .bind(("child", child.clone()))
                .await
                .expect("link child")
                .check()
                .expect("link child statement");
            ids.push(child);
        }
        (db, root, ids)
    }

    async fn tombstoned_op_rows(db: &Db) -> usize {
        let rows: Vec<serde_json::Value> = db
            .query("SELECT lamport_clock FROM op_log WHERE op_type = 'NodeTombstoned'")
            .await
            .expect("read op_log")
            .take(0)
            .expect("op_log rows");
        rows.len()
    }

    async fn live_count(db: &Db, ids: &[String]) -> usize {
        let rows: Vec<serde_json::Value> = db
            .query("SELECT node_id FROM node WHERE node_id IN $ids AND tombstone = NONE")
            .bind(("ids", ids.to_vec()))
            .await
            .expect("read nodes")
            .take(0)
            .expect("node rows");
        rows.len()
    }

    /// A failing row update rolls the cascade's op-log row back with it: never
    /// a logged-but-unapplied cascade. With the fault removed the same call
    /// commits one op and tombstones the whole subtree.
    #[tokio::test]
    async fn cascade_tombstone_rolls_back_its_op_log_row_when_the_update_fails() {
        let (db, root, ids) = seeded_subtree().await;
        db.query(
            "DEFINE EVENT fail_tombstone ON TABLE node \
             WHEN $after.tombstone != NONE THEN { THROW 'induced' }",
        )
        .await
        .expect("define fault")
        .check()
        .expect("define fault statement");

        let error = cascade_tombstone_node_handler(&db, &root, "did:key:z6MkA")
            .await
            .expect_err("the injected fault must fail the cascade");
        assert!(format!("{error:#}").contains("induced"), "{error:#}");
        assert_eq!(
            tombstoned_op_rows(&db).await,
            0,
            "the op-log row rolled back"
        );
        assert_eq!(live_count(&db, &ids).await, 3, "no node was tombstoned");

        db.query("REMOVE EVENT fail_tombstone ON TABLE node")
            .await
            .expect("remove fault")
            .check()
            .expect("remove fault statement");
        let outcome = cascade_tombstone_node_handler(&db, &root, "did:key:z6MkA")
            .await
            .expect("cascade commits once the fault is gone");
        assert_eq!(outcome.tombstoned_ids.len(), 3);
        assert_eq!(tombstoned_op_rows(&db).await, 1);
        assert_eq!(
            live_count(&db, &ids).await,
            0,
            "the whole subtree is tombstoned"
        );
    }

    /// The single-node tombstone shares the seam: a failed update logs nothing.
    #[tokio::test]
    async fn tombstone_rolls_back_its_op_log_row_when_the_update_fails() {
        let (db, root, _) = seeded_subtree().await;
        db.query(
            "DEFINE EVENT fail_tombstone ON TABLE node \
             WHEN $after.tombstone != NONE THEN { THROW 'induced' }",
        )
        .await
        .expect("define fault")
        .check()
        .expect("define fault statement");

        tombstone_node_handler(&db, &root, "did:key:z6MkA")
            .await
            .expect_err("the injected fault must fail the tombstone");
        assert_eq!(
            tombstoned_op_rows(&db).await,
            0,
            "the op-log row rolled back"
        );
        assert_eq!(live_count(&db, std::slice::from_ref(&root)).await, 1);
    }
}

/// Accountability directive AC-13: the KTX2 intake guard
/// (`fe_runtime::ktx2_guard`, RUSTSEC-2026-0344) at the `DbCommand::ImportGltf`
/// dispatch seam (`import_gltf_handler`, see lines ~376-379) must refuse
/// BEFORE any side effect — no blob, no asset row, no node row.
#[cfg(test)]
mod import_gltf_ktx2_guard_tests {
    use super::*;
    use fe_runtime::blob_store::mock::MockBlobStore;

    async fn setup_mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db.query(
            "CREATE verse CONTENT { verse_id: 'verse-1', name: 'v', created_by: 'owner-did', \
             created_at: '2026-10-11T00:00:00Z' };
             CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: 'verse-1', \
             owner_did: 'owner-did', name: 'f', created_at: '2026-10-11T00:00:00Z' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'p', \
             node_id: 'anchor-node', created_at: '2026-10-11T00:00:00Z' };",
        )
        .await
        .expect("seed scope chain")
        .check()
        .expect("seed scope chain");
        db
    }

    /// A minimal valid GLB whose sole image declares `mimeType: "image/ktx2"`
    /// — the exact shape `fe_runtime::ktx2_guard::check_glb` refuses.
    fn glb_with_ktx2_image() -> Vec<u8> {
        let json = r#"{"images":[{"bufferView":0,"mimeType":"image/ktx2"}],"bufferViews":[{"buffer":0,"byteLength":4}]}"#;
        let mut json_bytes = json.as_bytes().to_vec();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        let bin: Vec<u8> = vec![0u8; 4];
        let mut body = Vec::new();
        body.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(b"JSON");
        body.extend_from_slice(&json_bytes);
        body.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        body.extend_from_slice(b"BIN\0");
        body.extend_from_slice(&bin);
        let mut glb = b"glTF".to_vec();
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((12 + body.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&body);
        glb
    }

    #[tokio::test]
    async fn importing_a_ktx2_model_is_refused_and_stores_nothing() {
        let db = setup_mem_db().await;
        let store = std::sync::Arc::new(MockBlobStore::new());
        let blob_store: BlobStoreHandle = store.clone();

        let dir = tempfile::tempdir().expect("temp dir");
        let file_path = dir.path().join("model.glb");
        std::fs::write(&file_path, glb_with_ktx2_image()).expect("write fixture glb");

        let err = import_gltf_handler(
            &db,
            &blob_store,
            "petal-1",
            "bad-model",
            file_path.to_str().expect("utf8 path"),
            [0.0, 0.0, 0.0],
            "owner-did",
        )
        .await
        .expect_err("a KTX2-textured GLB must be refused");
        assert!(
            err.to_string().contains("RUSTSEC-2026-0344")
                || err.to_string().to_lowercase().contains("ktx2"),
            "refusal must name the ktx2 guard, got: {err}"
        );

        assert!(store.is_empty(), "no blob may be written on refusal");

        let assets: Vec<serde_json::Value> = db
            .query("SELECT * FROM asset")
            .await
            .expect("query asset")
            .take(0)
            .unwrap_or_default();
        assert!(assets.is_empty(), "no asset row on refusal");

        let nodes: Vec<serde_json::Value> = db
            .query("SELECT * FROM node")
            .await
            .expect("query node")
            .take(0)
            .unwrap_or_default();
        assert!(nodes.is_empty(), "no node row on refusal");
    }
}

/// Local compatibility entry for seed and test callers.
#[cfg(test)]
pub(crate) async fn create_node_handler(
    db: &Db,
    petal_id: &str,
    name: &str,
    position: [f32; 3],
) -> anyhow::Result<String> {
    create_node_as(db, petal_id, name, position, "local").await
}

/// Local compatibility entry for existing asset-placement callers.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_node_with_asset_handler(
    db: &Db,
    petal_id: &str,
    name: &str,
    asset_id: &str,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
) -> anyhow::Result<(String, String)> {
    create_node_with_asset_as(
        db, petal_id, name, asset_id, position, rotation, scale, "local",
    )
    .await
    .map(|(id, path, _)| (id, path))
}
