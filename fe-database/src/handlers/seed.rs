use crate::repo::{Db, Repo};
use crate::schema::{Asset, Fractal, Role, Verse, VerseMemberRow};
use crate::{hash_to_hex, BlobStoreHandle};
use crate::types::NodeId;

/// Returns `(asset_id_for_name)` map seeded from files in `assets/models/`.
/// Each GLB is written to the blob store and its BLAKE3 content hash is
/// recorded in the `asset` table.
pub(crate) async fn seed_assets(
    db: &Db,
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
    db: &Db,
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

    let seed_keypair = fe_identity::NodeKeypair::generate();
    let local_did = local_did.to_string();
    let node_id = NodeId(local_did.clone());
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

    let invite_payload = format!("{}:{}:{}:{}", verse_id, local_did, local_did, now);
    let invite_sig_bytes = seed_keypair.sign(invite_payload.as_bytes());
    let invite_sig = hex::encode(invite_sig_bytes.to_bytes());

    let member_id = ulid::Ulid::new().to_string();
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

    // --- Petal ---
    let petal_name = "Genesis Petal";
    let room_names = vec!["Lobby", "Workshop", "Gallery"];

    let petal_id = crate::queries::create_petal(db, petal_name, &node_id, &node_id).await?;
    db.query("UPDATE petal SET fractal_id = $fid, bounds = <geometry<polygon>> { type: 'Polygon', coordinates: [[[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0], [-10.0, -10.0]]] } WHERE petal_id = $pid")
        .bind(("fid", fractal_id.clone()))
        .bind(("pid", petal_id.0.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("seed UPDATE petal fractal_id failed: {e}"))?;
    tracing::info!("Seeded petal: {petal_name} ({})", petal_id.0);

    Repo::<Role>::create(db, &Role {
        peer_did: node_id.0.clone(),
        scope:    format!("VERSE#_-FRACTAL#_-PETAL#{}", petal_id.0.to_string()),
        role:     "owner".to_string(),
    }).await?;
    tracing::info!("Assigned owner role to {}", node_id.0);

    for room_name in &room_names {
        crate::queries::create_room(db, &node_id, &petal_id, room_name).await?;
        tracing::info!("Seeded room: {room_name}");
    }

    // --- Nodes ---
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
