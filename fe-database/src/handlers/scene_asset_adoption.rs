//! Explicit present-tense responsibility for a verified legacy local copy; see AGENTS.md.

use crate::{repo::Db, BlobStoreHandle};
use fe_runtime::scene_asset::{
    SceneAssetAdoptionCandidate, SceneAssetAdoptionReceipt, SceneAssetReferenceWitness,
    MAX_SCENE_ASSET_BYTES,
};
use serde_json::{json, Value};

const REFERENCES: &str="SELECT node_id, encoding::base64::encode(encoding::cbor::encode($this)) AS fingerprint FROM node WHERE asset_id=$adoption_asset ORDER BY node_id LIMIT 257 TIMEOUT 2s";

struct Prepared {
    candidate: SceneAssetAdoptionCandidate,
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
}

async fn prepare(db: &Db, node_id: &str, actor: &str) -> anyhow::Result<Prepared> {
    anyhow::ensure!(
        !node_id.is_empty() && node_id.len() <= 1024,
        "invalid adoption node"
    );
    let mut response=db.query("SELECT node_id,petal_id,asset_id,tombstone FROM node WHERE node_id=$adoption_node LIMIT 2 TIMEOUT 2s")
        .bind(("adoption_node",node_id.to_owned())).await?.check()?;
    let nodes: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        nodes.len() == 1 && nodes[0]["tombstone"].is_null(),
        "adoption needs one live object"
    );
    let asset_id = nodes[0]["asset_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("object has no asset"))?;
    anyhow::ensure!(
        fe_runtime::safe_id::is_safe_path_id(asset_id),
        "invalid legacy asset identity"
    );
    let petal = nodes[0]["petal_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("object has no Petal"))?;
    let scope = super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("adoption ancestry missing"))?;
    let permission =
        super::entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Owner)
            .await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let mut response=db.query("SELECT *, encoding::base64::encode(encoding::cbor::encode($this)) AS fingerprint OMIT id FROM asset WHERE asset_id=$adoption_asset LIMIT 2 TIMEOUT 2s")
        .bind(("adoption_asset",asset_id.to_owned())).await?.check()?;
    let assets: Vec<Value> = response.take(0)?;
    anyhow::ensure!(assets.len() == 1, "adoption metadata missing or ambiguous");
    let asset = &assets[0];
    anyhow::ensure!(
        serde_json::to_vec(asset)?.len() <= 64 * 1024,
        "adoption metadata exceeds 64 KiB"
    );
    anyhow::ensure!(
        asset["owner_did"].is_null()
            && asset["petal_id"].is_null()
            && asset["publication_provenance"].is_null(),
        "only ownerless, unanchored legacy copies can be adopted"
    );
    let hash = crate::hash_from_hex(
        asset["content_hash"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing digest"))?,
    )?;
    anyhow::ensure!(
        asset["content_hash"].as_str() == Some(crate::hash_to_hex(&hash).as_str()),
        "asset digest is not canonical"
    );
    let content_type = asset["content_type"]
        .as_str()
        .filter(|mime| matches!(*mime, "model/gltf-binary" | "model/gltf+json"))
        .ok_or_else(|| anyhow::anyhow!("unsupported scene MIME"))?;
    let size = asset["size_bytes"]
        .as_u64()
        .filter(|size| *size > 0 && *size <= MAX_SCENE_ASSET_BYTES)
        .ok_or_else(|| anyhow::anyhow!("invalid asset size"))?;
    let fingerprint = asset["fingerprint"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing metadata fingerprint"))?;
    let mut response=db.query(format!("{REFERENCES}; SELECT node_id,petal_id FROM node WHERE asset_id=$adoption_asset ORDER BY node_id LIMIT 257 TIMEOUT 2s; SELECT payload_id FROM device_payload WHERE asset_ids CONTAINS $adoption_asset LIMIT 1 TIMEOUT 2s; SELECT petal_id, encoding::base64::encode(encoding::cbor::encode($this)) AS fingerprint FROM model WHERE asset_id=$adoption_asset LIMIT 2 TIMEOUT 2s"))
        .bind(("adoption_asset",asset_id.to_owned())).await?.check()?;
    let reference_rows: Vec<Value> = response.take(0)?;
    let references: Vec<SceneAssetReferenceWitness> = reference_rows
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    let locations: Vec<Value> = response.take(1)?;
    let payloads: Vec<Value> = response.take(2)?;
    let models: Vec<Value> = response.take(3)?;
    anyhow::ensure!(
        models.len() <= 1 && models.iter().all(|row| row["petal_id"] == petal),
        "asset Portal metadata is ambiguous or belongs to another Petal"
    );
    let model_fingerprint = models
        .first()
        .map(|row| {
            row["fingerprint"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("model reference fingerprint missing"))
        })
        .transpose()?;
    anyhow::ensure!(
        !references.is_empty()
            && references.len() <= 256
            && references.len() == locations.len()
            && locations.iter().all(|row| row["petal_id"] == petal)
            && locations.iter().any(|row| row["node_id"] == node_id),
        "asset references are unknown, excessive or span Petals; reimport a fresh asset instead"
    );
    anyhow::ensure!(
        payloads.is_empty(),
        "device payload references cannot be adopted as scene content"
    );
    guards.extend([
        "IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM asset WHERE asset_id=$adoption_asset) != [$adoption_fingerprint] { THROW 'adoption metadata changed'; }".into(),
        format!("IF ({REFERENCES}) != $adoption_references {{ THROW 'adoption reference set changed'; }}"),
        "IF array::len((SELECT payload_id FROM device_payload WHERE asset_ids CONTAINS $adoption_asset LIMIT 1)) != 0 { THROW 'adoption payload reference appeared'; }".into(),
        "IF (SELECT petal_id, encoding::base64::encode(encoding::cbor::encode($this)) AS fingerprint FROM model WHERE asset_id=$adoption_asset LIMIT 2) != $adoption_models { THROW 'adoption model reference changed'; }".into(),
        "IF array::len((SELECT node_id FROM node WHERE node_id=$adoption_node AND asset_id=$adoption_asset AND petal_id=$adoption_petal AND tombstone=NONE)) != 1 { THROW 'adoption target changed'; }".into(),
    ]);
    bindings.extend([
        ("adoption_asset".into(), json!(asset_id)),
        ("adoption_models".into(), json!(models)),
        ("adoption_fingerprint".into(), json!(fingerprint)),
        (
            "adoption_references".into(),
            serde_json::to_value(&references)?,
        ),
        ("adoption_node".into(), json!(node_id)),
        ("adoption_petal".into(), json!(petal)),
    ]);
    anyhow::ensure!(
        serde_json::to_vec(&bindings)?.len() + guards.iter().map(String::len).sum::<usize>() + 4096
            <= 4 * 1024 * 1024,
        "adoption candidate/read transaction exceeds 4 MiB"
    );
    Ok(Prepared {
        candidate: SceneAssetAdoptionCandidate {
            node_id: node_id.into(),
            asset_id: asset_id.into(),
            scope,
            content_hash: hash,
            content_type: content_type.into(),
            size_bytes: size,
            asset_fingerprint: fingerprint.into(),
            reference_fingerprints: references,
            model_fingerprint,
        },
        guards,
        bindings,
    })
}

async fn verify_copy(
    blobs: &BlobStoreHandle,
    candidate: &SceneAssetAdoptionCandidate,
) -> anyhow::Result<()> {
    let path = blobs
        .get_blob_path(&candidate.content_hash)
        .ok_or_else(|| anyhow::anyhow!("local asset bytes are missing"))?;
    let expected = candidate.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "Adoption copy is not a regular file"
        );
        file.take(MAX_SCENE_ASSET_BYTES + 1)
            .read_to_end(&mut bytes)?;
        fe_runtime::scene_asset::verify_scene_asset_bytes(
            &bytes,
            &expected.content_hash,
            &expected.content_type,
            expected.size_bytes,
        )?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

pub(crate) async fn read(
    db: &Db,
    blobs: &BlobStoreHandle,
    node_id: &str,
    actor: &str,
) -> anyhow::Result<SceneAssetAdoptionCandidate> {
    let prepared = prepare(db, node_id, actor).await?;
    verify_copy(blobs, &prepared.candidate).await?;
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        prepared.guards.join(";\n")
    ));
    for binding in prepared.bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    Ok(prepared.candidate)
}

pub(crate) async fn adopt(
    db: &Db,
    blobs: &BlobStoreHandle,
    candidate: &SceneAssetAdoptionCandidate,
    actor: &str,
    request_id: &str,
) -> anyhow::Result<SceneAssetAdoptionReceipt> {
    anyhow::ensure!(
        !request_id.is_empty() && request_id.len() <= 128,
        "invalid adoption request"
    );
    let mut prepared = prepare(db, &candidate.node_id, actor).await?;
    anyhow::ensure!(
        prepared.candidate == *candidate,
        "adoption candidate changed; review it again"
    );
    verify_copy(blobs, candidate).await?;
    let operation_id = ulid::Ulid::new().to_string();
    let adopted_at = chrono::Utc::now().to_rfc3339();
    let provenance = json!({"kind":"explicit_local_copy_adoption","adopter_did":actor,"adopted_at":adopted_at,"operation_id":operation_id});
    prepared.guards.push("UPDATE asset SET petal_id=$adoption_petal, publication_provenance=$adoption_provenance WHERE asset_id=$adoption_asset".into());
    prepared
        .bindings
        .push(("adoption_provenance".into(), provenance.clone()));
    let payload = json!({"operation_id":operation_id,"request_id":request_id,"asset_id":candidate.asset_id,"scope":candidate.scope,"content_hash":crate::hash_to_hex(&candidate.content_hash),"size_bytes":candidate.size_bytes,"content_type":candidate.content_type,"provenance":provenance});
    anyhow::ensure!(
        serde_json::to_vec(&payload)?.len()
            + serde_json::to_vec(&prepared.bindings)?.len()
            + prepared.guards.iter().map(String::len).sum::<usize>()
            + 4096
            <= 4 * 1024 * 1024,
        "adoption transaction exceeds 4 MiB"
    );
    let entry = crate::types::OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: crate::types::NodeId(candidate.node_id.clone()),
        op_type: crate::types::OpType::SceneAssetAdopted,
        payload,
        sig: "00".repeat(64),
    };
    crate::migration::mapping_inventory::record_bypass();
    crate::op_log::commit_operation_atomic(db, entry, &prepared.guards, prepared.bindings).await?;
    Ok(SceneAssetAdoptionReceipt {
        candidate: candidate.clone(),
        operation_id,
        adopter_did: actor.into(),
        adopted_at,
    })
}
