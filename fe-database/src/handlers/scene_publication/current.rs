//! Current native projections after actual replica commits; see AGENTS.md.

use super::{clock_of, read_clock, CLOCK_FENCE};
use crate::{repo::Db, BlobStoreHandle};
use fe_runtime::{
    messages::{DbResult, ReplicatedRowOutcome},
    scene_replication::{
        ReplicaAssetUnavailable, ReplicaNodeAsset, ReplicaNodeProvenance, ReplicatedNodeCurrent,
    },
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Read current row, provenance and available asset under recipient Viewer authority.
pub(crate) async fn read_current(
    db: &Db,
    blobs: &BlobStoreHandle,
    verse: &str,
    id: &str,
    actor: &str,
) -> anyhow::Result<ReplicatedNodeCurrent> {
    let mut response=db.query("SELECT * OMIT id FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s; SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id",id.to_owned())).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    let fingerprints: Vec<Value> = response.take(1)?;
    anyhow::ensure!(
        rows.len() == 1 && fingerprints.len() == 1,
        "replica current row missing or ambiguous"
    );
    let raw = rows[0].clone();
    anyhow::ensure!(
        serde_json::to_vec(&raw)?.len() <= super::wire::MAX_ROW_BYTES,
        "replica current row exceeds 96 KiB"
    );
    let petal = raw["petal_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("replica current Petal missing"))?;
    let scope = super::super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("replica current ancestry unavailable"))?;
    anyhow::ensure!(
        crate::parse_scope(&scope)?.verse_id == verse,
        "replica current row belongs to another Verse"
    );
    let permission = super::super::entity::prepare_scope_permission(
        db,
        &scope,
        actor,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let mut without_position = raw.clone();
    without_position.as_object_mut().unwrap().remove("position");
    guards.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$scene_id) != [$scene_current_row] { THROW 'replica current row changed'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$scene_id AND petal_id=$scene_current_petal AND position=<geometry<point>> [$scene_current_x,$scene_current_z])) != 1 { THROW 'replica current point or ancestry changed'; }".into(),
        "IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id) != $scene_current_fingerprints { THROW 'replica current row changed'; }".into(),
        CLOCK_FENCE.into(),
    ]);
    let clocks = read_clock(db, id).await?;
    let provenance = match clocks.first() {
        Some(clock) if clock["row_fingerprint"] == fingerprints[0] => {
            let clock = clock_of(&clocks)?
                .ok_or_else(|| anyhow::anyhow!("replica current original clock unavailable"))?;
            ReplicaNodeProvenance::OriginalSceneCommit {
                hlc: clock.hlc,
                publisher: clock.publisher,
            }
        }
        _ => ReplicaNodeProvenance::LegacyReplicaApply,
    };
    bindings.extend([
        ("scene_id".into(), json!(id)),
        ("scene_current_petal".into(), json!(petal)),
        ("scene_current_row".into(), without_position),
        (
            "scene_current_x".into(),
            raw["position"]["coordinates"][0].clone(),
        ),
        (
            "scene_current_z".into(),
            raw["position"]["coordinates"][1].clone(),
        ),
        ("scene_current_fingerprints".into(), json!(fingerprints)),
        ("scene_clock_before".into(), json!(clocks)),
    ]);
    let mut asset = asset_state(db, blobs, &raw, actor, &mut guards, &mut bindings).await?;
    let state = serde_json::from_value(super::super::node_history::normalize_snapshot(raw)?)?;
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        guards.join(";\n")
    ));
    for binding in bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    if let ReplicaNodeAsset::LocalBlob {
        asset_id,
        content_hash,
        ..
    } = &asset
    {
        if !blobs.has_blob(&crate::hash_from_hex(content_hash)?) {
            asset = ReplicaNodeAsset::Unavailable {
                asset_id: asset_id.clone(),
                reason: ReplicaAssetUnavailable::MissingBlob,
            };
        }
    }
    Ok(ReplicatedNodeCurrent {
        scope,
        state,
        provenance,
        asset,
    })
}

async fn asset_state(
    db: &Db,
    blobs: &BlobStoreHandle,
    row: &Value,
    actor: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<ReplicaNodeAsset> {
    let Some(id) = row["asset_id"].as_str() else {
        return Ok(ReplicaNodeAsset::NoAsset);
    };
    let unavailable = |reason| ReplicaNodeAsset::Unavailable {
        asset_id: id.into(),
        reason,
    };
    let mut response=db.query("SELECT asset_id,petal_id,owner_did,content_hash,content_type FROM asset WHERE asset_id=$scene_current_asset LIMIT 2 TIMEOUT 5s")
        .bind(("scene_current_asset",id.to_owned())).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    guards.push("IF (SELECT asset_id,petal_id,owner_did,content_hash,content_type FROM asset WHERE asset_id=$scene_current_asset) != $scene_current_asset_rows { THROW 'replica current asset metadata changed'; }".into());
    bindings.extend([
        ("scene_current_asset".into(), json!(id)),
        ("scene_current_asset_rows".into(), json!(rows)),
    ]);
    if rows.is_empty() {
        return Ok(unavailable(ReplicaAssetUnavailable::MissingMetadata));
    }
    if rows.len() != 1 {
        return Ok(unavailable(ReplicaAssetUnavailable::AmbiguousMetadata));
    }
    let metadata = &rows[0];
    match metadata["petal_id"].as_str() {
        Some(petal) if row["petal_id"].as_str() != Some(petal) => {
            return Ok(unavailable(ReplicaAssetUnavailable::ForeignAnchor))
        }
        None if metadata["owner_did"].as_str() != Some(actor) => {
            return Ok(unavailable(ReplicaAssetUnavailable::UnanchoredMetadata))
        }
        _ => {}
    }
    let (Some(hash), Some(content_type)) = (
        metadata["content_hash"].as_str(),
        metadata["content_type"].as_str(),
    ) else {
        return Ok(unavailable(ReplicaAssetUnavailable::InvalidMetadata));
    };
    let Ok(decoded) = crate::hash_from_hex(hash) else {
        return Ok(unavailable(ReplicaAssetUnavailable::InvalidMetadata));
    };
    let extension = match content_type {
        "model/gltf-binary" => "glb",
        "model/gltf+json" => "gltf",
        _ => return Ok(unavailable(ReplicaAssetUnavailable::UnsupportedSceneAsset)),
    };
    if !blobs.has_blob(&decoded) {
        return Ok(unavailable(ReplicaAssetUnavailable::MissingBlob));
    }
    Ok(ReplicaNodeAsset::LocalBlob {
        asset_id: id.into(),
        content_hash: hash.into(),
        content_type: content_type.into(),
        asset_path: format!("blob://{hash}.{extension}"),
    })
}

/// Echo actual settled outcomes and coalesce current node/asset refreshes in DB result order.
pub(crate) async fn echo_applied(
    db: &Db,
    blobs: &BlobStoreHandle,
    actor: &str,
    tx: &crossbeam::channel::Sender<DbResult>,
    reports: Vec<super::super::inbound_admissions::RowReport>,
) {
    let mut nodes = BTreeSet::new();
    let mut assets = BTreeSet::new();
    for report in reports {
        if matches!(
            report.outcome,
            ReplicatedRowOutcome::Applied | ReplicatedRowOutcome::AppliedTombstone
        ) {
            if report.table == "node" {
                nodes.insert((report.verse_id.clone(), report.record_id.clone()));
            } else if report.table == "asset" {
                assets.insert((report.verse_id.clone(), report.record_id.clone()));
            }
        }
        super::super::send_result(
            tx,
            DbResult::ReplicatedRowApplied {
                verse_id: report.verse_id,
                table: report.table,
                record_id: report.record_id,
                outcome: report.outcome,
            },
        );
    }
    for (verse, asset) in assets {
        let result=async {
            let mut response=db.query("SELECT node_id FROM node WHERE asset_id=$scene_refresh_asset AND petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$scene_refresh_verse)) LIMIT 1025 TIMEOUT 5s")
                .bind(("scene_refresh_asset",asset.clone())).bind(("scene_refresh_verse",verse.clone())).await?.check()?;
            let rows:Vec<Value>=response.take(0)?;anyhow::ensure!(rows.len()<=1024,"asset replica projection exceeds 1024 references; no reference prefix refreshed");
            let mut references=Vec::new();
            for row in rows {references.push((verse.clone(),row["node_id"].as_str().ok_or_else(||anyhow::anyhow!("invalid asset reference"))?.to_owned()));}
            Ok::<_,anyhow::Error>(references)
        }.await;
        match result {
            Ok(references) => nodes.extend(references),
            Err(error) => tracing::warn!(
                asset_id = asset,
                "replica asset projection deferred: {error:#}"
            ),
        }
    }
    if nodes.len() > 1024 {
        tracing::warn!(
            nodes = nodes.len(),
            "replica current projection exceeds 1024 nodes; no refresh prefix emitted"
        );
        return;
    }
    for (verse, id) in nodes {
        match read_current(db, blobs, &verse, &id, actor).await {
            Ok(current) => {
                super::super::send_result(tx, DbResult::ReplicatedNodeCurrent { current })
            }
            Err(error) => tracing::warn!(
                node_id = id,
                "replica current projection deferred: {error:#}"
            ),
        }
    }
}
