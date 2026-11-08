//! Current scene-asset authority and bounded legacy repair; see handlers/AGENTS.md.

use crate::repo::Db;
use fe_runtime::scene_asset::{
    SceneAssetTransferPermit, SceneAssetTransferPurpose, SceneAssetTransferRequest,
    MAX_SCENE_ASSET_BYTES,
};
use serde_json::{json, Value};

/// Resolve an exact current asset/node reference under actual DB-thread identity.
pub(crate) async fn inspect(
    db: &Db,
    request: &SceneAssetTransferRequest,
    local_did: &str,
) -> anyhow::Result<SceneAssetTransferPermit> {
    anyhow::ensure!(
        request.deadline > std::time::Instant::now(),
        "asset admission expired"
    );
    anyhow::ensure!(
        !request.verse_id.is_empty() && request.verse_id.len() <= 1024,
        "invalid asset Verse"
    );
    anyhow::ensure!(
        request
            .node_id
            .as_ref()
            .is_none_or(|id| !id.is_empty() && id.len() <= 1024),
        "invalid asset node"
    );
    if let SceneAssetTransferPurpose::Serve { requester_did } = &request.purpose {
        anyhow::ensure!(
            !requester_did.is_empty() && requester_did.len() <= 512,
            "invalid authenticated requester"
        );
    }
    let (exact_asset, exact_petal) = match &request.purpose {
        SceneAssetTransferPurpose::Publish { asset_id, petal_id } => {
            anyhow::ensure!(
                !asset_id.is_empty()
                    && asset_id.len() <= 1024
                    && !petal_id.is_empty()
                    && petal_id.len() <= 1024,
                "invalid publication identity"
            );
            (Some(asset_id.clone()), Some(petal_id.clone()))
        }
        _ => (None, None),
    };
    let hash = crate::hash_to_hex(&request.hash);
    let mut response = db.query("SELECT *, encoding::base64::encode(encoding::cbor::encode($this)) AS transfer_fingerprint OMIT id FROM asset WHERE content_hash=$transfer_hash AND ($transfer_exact_asset=NONE OR asset_id=$transfer_exact_asset) AND ($transfer_exact_petal=NONE OR petal_id=$transfer_exact_petal) ORDER BY asset_id LIMIT 17 TIMEOUT 2s")
        .bind(("transfer_hash", hash.clone())).bind(("transfer_exact_asset", exact_asset)).bind(("transfer_exact_petal", exact_petal)).await?.check()?;
    let assets: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        assets.len() <= 16,
        "asset hash has more than 16 metadata rows"
    );
    for asset in assets {
        anyhow::ensure!(
            serde_json::to_vec(&asset)?.len() <= 64 * 1024,
            "transfer metadata exceeds 64 KiB"
        );
        let Some(asset_id) = asset["asset_id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 1024)
        else {
            continue;
        };
        let Some(petal_id) = asset["petal_id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(content_type) = asset["content_type"]
            .as_str()
            .filter(|mime| fe_runtime::scene_asset::SCENE_ASSET_MIMES.contains(mime))
        else {
            continue;
        };
        let Some(size_bytes) = asset["size_bytes"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= MAX_SCENE_ASSET_BYTES)
        else {
            continue;
        };
        let Some(fingerprint) = asset["transfer_fingerprint"].as_str() else {
            continue;
        };
        let Some(scope) = super::crud::resolve_petal_scope_handler(db, petal_id).await? else {
            continue;
        };
        if crate::parse_scope(&scope)?.verse_id != request.verse_id {
            continue;
        }
        // QA-0137: Serve re-shares verified bytes of a current row; only Publish
        // originates metadata, so only it needs Editor and upload/adoption proof.
        let publishing = matches!(request.purpose, SceneAssetTransferPurpose::Publish { .. });
        let floor = if publishing {
            fe_policy::RoleLevel::Editor
        } else {
            fe_policy::RoleLevel::Viewer
        };
        let Ok(permission) =
            super::entity::prepare_scope_permission(db, &scope, local_did, floor).await
        else {
            continue;
        };
        let (mut guards, mut bindings) = permission.scoped_read_fence(0);
        if publishing && !publication_proof(db, &asset, &scope, &mut guards, &mut bindings).await? {
            continue;
        }

        if let SceneAssetTransferPurpose::Serve { requester_did } = &request.purpose {
            let Ok(permission) = super::entity::prepare_scope_permission(
                db,
                &scope,
                requester_did,
                fe_policy::RoleLevel::Viewer,
            )
            .await
            else {
                continue;
            };
            let (more, bound) = permission.scoped_read_fence(1);
            guards.extend(more);
            bindings.extend(bound);
        }
        // QA-0158: the anchored metadata row is the Verse content; a live node reference
        // is required only when the request names one (handlers/AGENTS.md §scene-asset-transfer).
        if let Some(node_id) = &request.node_id {
            let mut response = db.query("SELECT VALUE node_id FROM node WHERE node_id=$transfer_node AND asset_id=$transfer_asset AND petal_id=$transfer_petal AND tombstone=NONE LIMIT 2 TIMEOUT 2s")
                .bind(("transfer_asset",asset_id.to_owned())).bind(("transfer_petal",petal_id.to_owned())).bind(("transfer_node",node_id.clone())).await?.check()?;
            let nodes: Vec<Value> = response.take(0)?;
            if nodes.len() != 1 {
                continue;
            }
            guards.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$transfer_node AND petal_id=$transfer_petal AND asset_id=$transfer_asset AND tombstone=NONE)) != 1 { THROW 'scene asset live reference changed'; }".into());
            bindings.push(("transfer_node".into(), json!(node_id)));
        } else if !publishing && !locally_proven(&asset) {
            match unplaced_transfer_fence(db, &scope, asset_id, petal_id).await? {
                Some((more, bound)) => {
                    guards.extend(more);
                    bindings.extend(bound);
                }
                None => continue,
            }
        }
        guards.push("IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM asset WHERE asset_id=$transfer_asset) != [$transfer_fingerprint] { THROW 'scene asset metadata changed'; }".into());
        bindings.extend([
            ("transfer_asset".into(), json!(asset_id)),
            ("transfer_petal".into(), json!(petal_id)),
            ("transfer_fingerprint".into(), json!(fingerprint)),
        ]);
        let script = format!(
            "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
            guards.join(";\n")
        );
        anyhow::ensure!(
            script.len() + serde_json::to_vec(&bindings)?.len() <= 4 * 1024 * 1024,
            "asset admission exceeds 4 MiB"
        );
        let mut query = db.query(script);
        for binding in bindings {
            query = query.bind(binding);
        }
        query.await?.check()?;
        anyhow::ensure!(
            request.deadline > std::time::Instant::now(),
            "asset admission expired"
        );
        return Ok(SceneAssetTransferPermit {
            verse_id: request.verse_id.clone(),
            petal_id: petal_id.into(),
            node_id: request.node_id.clone(),
            asset_id: asset_id.into(),
            content_hash: request.hash,
            content_type: content_type.into(),
            size_bytes,
            asset_fingerprint: fingerprint.into(),
        });
    }
    anyhow::bail!("no current authorized scene asset reference")
}

/// A local upload or adoption: inbound rows never carry these fields (stripped on apply).
fn locally_proven(asset: &Value) -> bool {
    asset["owner_did"].as_str().is_some_and(|owner| !owner.is_empty())
        || !asset["publication_provenance"].is_null()
}

/// Fences for an unplaced inbound row: a live node reference in its Petal, else its replica
/// author's current Editor role; `None` admits neither (handlers/AGENTS.md "unplaced rows").
async fn unplaced_transfer_fence(
    db: &Db,
    scope: &str,
    asset_id: &str,
    petal_id: &str,
) -> anyhow::Result<Option<(Vec<String>, Vec<(String, Value)>)>> {
    const LIVE: &str = "SELECT VALUE node_id FROM node WHERE asset_id=$transfer_asset AND petal_id=$transfer_petal AND tombstone=NONE LIMIT 1";
    const AUTHOR: &str = "SELECT VALUE author FROM asset_inbound_author WHERE asset_id=$transfer_asset LIMIT 2";
    let mut response = db
        .query(format!("{LIVE} TIMEOUT 2s; {AUTHOR} TIMEOUT 2s"))
        .bind(("transfer_asset", asset_id.to_owned()))
        .bind(("transfer_petal", petal_id.to_owned()))
        .await?
        .check()?;
    let live: Vec<Value> = response.take(0)?;
    let authors: Vec<String> = response.take(1)?;
    if !live.is_empty() {
        return Ok(Some((vec![format!("IF array::len(({LIVE})) != 1 {{ THROW 'scene asset live reference changed'; }}")], Vec::new())));
    }
    let [author] = authors.as_slice() else { return Ok(None) };
    let Ok(permission) =
        super::entity::prepare_scope_permission(db, scope, author, fe_policy::RoleLevel::Editor).await
    else {
        return Ok(None);
    };
    let (mut guards, mut bindings) = permission.scoped_read_fence(2);
    guards.push(format!("IF ({AUTHOR}) != [$transfer_author] {{ THROW 'scene asset author changed'; }}"));
    bindings.push(("transfer_author".into(), json!(author)));
    Ok(Some((guards, bindings)))
}

/// Local upload/adoption provenance is distinct from role authority.
pub(crate) async fn publication_proof(
    db: &Db,
    asset: &Value,
    scope: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<bool> {
    if asset["owner_did"]
        .as_str()
        .is_some_and(|owner| !owner.is_empty() && owner.len() <= 512)
    {
        return Ok(true);
    }
    let provenance = &asset["publication_provenance"];
    if provenance["kind"] != "explicit_local_copy_adoption" {
        return Ok(false);
    }
    let Some(operation) = provenance["operation_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 128)
    else {
        return Ok(false);
    };
    let mut response=db.query("SELECT * OMIT id FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.operation_id=$asset_adoption_operation LIMIT 2 TIMEOUT 2s")
        .bind(("asset_adoption_operation",operation.to_owned())).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    if rows.len() != 1 {
        return Ok(false);
    }
    let payload = &rows[0]["payload"];
    if payload["asset_id"] != asset["asset_id"]
        || payload["scope"] != scope
        || payload["content_hash"] != asset["content_hash"]
        || payload["size_bytes"] != asset["size_bytes"]
        || payload["content_type"] != asset["content_type"]
        || payload["provenance"] != *provenance
    {
        return Ok(false);
    }
    guards.push("IF (SELECT * OMIT id FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.operation_id=$asset_adoption_operation) != $asset_adoption_proof { THROW 'asset adoption proof changed'; }".into());
    bindings.extend([
        ("asset_adoption_operation".into(), json!(operation)),
        ("asset_adoption_proof".into(), json!(rows)),
    ]);
    Ok(true)
}
