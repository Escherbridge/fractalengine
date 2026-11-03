//! Fixed metadata-only publication witness; see AGENTS.md scene-asset-publication-audit.

use crate::repo::Db;
use fe_runtime::scene_asset::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;

const DIGEST: &str = "crypto::blake3(encoding::base64::encode(encoding::cbor::encode($this)))";
const ROW_CAP: usize = 96 * 1024;

/// Query construction uses only private constants, never caller SQL or selectors.
fn witness(fields: &str) -> String {
    format!("IF bytes::len(encoding::cbor::encode($this)) > {ROW_CAP} {{ THROW 'audit row exceeds bound'; }} ELSE {{ RETURN {{ data:{{{fields}}},digest:{DIGEST} }}; }}")
}
fn text<'a>(row: &'a Value, key: &str, max: usize) -> anyhow::Result<&'a str> {
    row[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control))
        .ok_or_else(|| anyhow::anyhow!("invalid audit field"))
}
fn digest(row: &Value) -> anyhow::Result<String> {
    let digest = text(row, "digest", 64)?;
    anyhow::ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid audit digest"
    );
    Ok(digest.into())
}
fn field<T>(
    row: &Value,
    key: &str,
    decode: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<SceneAssetField<T>> {
    match (
        row[format!("{key}_none")].as_bool(),
        row[format!("{key}_null")].as_bool(),
    ) {
        (Some(true), Some(false)) => Ok(SceneAssetField::Absent),
        (Some(false), Some(true)) => Ok(SceneAssetField::Null),
        (Some(false), Some(false)) => Ok(SceneAssetField::Value(decode()?)),
        _ => anyhow::bail!("invalid native field witness"),
    }
}
fn provenance(row: &Value) -> anyhow::Result<SceneAssetAuditProvenance> {
    anyhow::ensure!(
        text(row, "kind", 64)? == "explicit_local_copy_adoption",
        "unsupported provenance"
    );
    let operation_id = text(row, "operation_id", 26)?;
    anyhow::ensure!(
        is_canonical_audit_ulid(operation_id),
        "invalid operation identity"
    );
    let adopter = text(row, "adopter_did", 128)?;
    fe_identity::validate_client_did(adopter)?;
    let adopted_at = text(row, "adopted_at", 64)?;
    chrono::DateTime::parse_from_rfc3339(adopted_at)?;
    Ok(SceneAssetAuditProvenance {
        kind: "explicit_local_copy_adoption".into(),
        operation_id: operation_id.into(),
        adopter_did: adopter.into(),
        adopted_at: adopted_at.into(),
    })
}

pub(crate) struct Prepared {
    output: SceneAssetPublicationAudit,
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
}
impl Prepared {
    pub(crate) async fn finish(self, db: &Db) -> anyhow::Result<SceneAssetPublicationAudit> {
        let script = format!(
            "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
            self.guards.join(";\n")
        );
        anyhow::ensure!(
            script.len() + serde_json::to_vec(&self.bindings)?.len()
                <= SCENE_ASSET_AUDIT_INTERNAL_MAX,
            "audit envelope exceeds bound"
        );
        let mut query = db.query(script);
        for binding in self.bindings {
            query = query.bind(binding);
        }
        tokio::time::timeout(std::time::Duration::from_secs(2), async { query.await })
            .await??
            .check()?;
        anyhow::ensure!(
            serde_json::to_vec(&self.output)?.len() <= SCENE_ASSET_AUDIT_RESPONSE_MAX,
            "audit response exceeds bound"
        );
        Ok(self.output)
    }
}

pub(crate) async fn prepare(
    db: &Db,
    request: &SceneAssetAuditRequest,
    nonce: &str,
    local_did: &str,
) -> anyhow::Result<Prepared> {
    anyhow::ensure!(
        request.validate_ids() && request.is_current() && is_canonical_audit_ulid(nonce),
        "invalid audit request"
    );
    anyhow::ensure!(
        request.caller.role_ceiling == "owner" && request.caller.scope.len() <= 1024,
        "audit caller ceiling refused"
    );
    fe_identity::validate_client_did(&request.caller.did)?;
    fe_identity::validate_client_did(local_did)?;
    let target_query = format!(
        "SELECT VALUE {} FROM node WHERE node_id=$audit_node LIMIT 2 TIMEOUT 2s",
        witness(
            "node_id:node_id,petal_id:petal_id,asset_id:asset_id,live:type::is_none(tombstone)"
        )
    );
    let mut response = db
        .query(target_query)
        .bind(("audit_node", request.node_id.clone()))
        .await?
        .check()?;
    let targets: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        targets.len() == 1 && targets[0]["data"]["live"].as_bool() == Some(true),
        "audit target unavailable"
    );
    let target = &targets[0]["data"];
    let asset_id = text(target, "asset_id", 128)?.to_owned();
    let petal_id = text(target, "petal_id", 128)?.to_owned();
    anyhow::ensure!(
        fe_runtime::safe_id::is_safe_path_id(&asset_id),
        "invalid asset identity"
    );
    let scope = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::crud::resolve_petal_scope_handler(db, &petal_id),
    )
    .await??
    .ok_or_else(|| anyhow::anyhow!("audit ancestry unavailable"))?;
    anyhow::ensure!(
        fe_policy::scope_contains(&request.caller.scope, &scope),
        "audit caller scope refused"
    );
    let mut guards = Vec::new();
    let mut bindings = Vec::new();
    for (index, actor) in [request.caller.did.as_str(), local_did]
        .into_iter()
        .enumerate()
    {
        let permission = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Owner),
        )
        .await??;
        let (g, b) = permission.scoped_read_fence(index);
        guards.extend(g);
        bindings.extend(b);
    }
    let asset_fields="asset_id:asset_id,content_hash:content_hash,content_type:content_type,size_bytes:size_bytes,owner_did:owner_did,owner_none:type::is_none(owner_did),owner_null:type::is_null(owner_did),petal_id:petal_id,petal_none:type::is_none(petal_id),petal_null:type::is_null(petal_id),publication_provenance:publication_provenance,provenance_none:type::is_none(publication_provenance),provenance_null:type::is_null(publication_provenance),provenance_digest:crypto::blake3(encoding::base64::encode(encoding::cbor::encode(publication_provenance)))";
    let references_query=format!("SELECT node_id, {} AS witness FROM node WHERE asset_id=$audit_asset ORDER BY node_id LIMIT 257 TIMEOUT 2s",witness("node_id:node_id,petal_id:petal_id"));
    let models_query = format!(
        "SELECT VALUE {} FROM model WHERE asset_id=$audit_asset LIMIT 2 TIMEOUT 2s",
        witness("petal_id:petal_id")
    );
    let operations_query=format!("SELECT VALUE {} FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.asset_id=$audit_asset LIMIT 2 TIMEOUT 2s",witness("op_type:op_type,node_id:node_id,operation_id:payload.operation_id,asset_id:payload.asset_id,scope:payload.scope,content_hash:payload.content_hash,content_type:payload.content_type,size_bytes:payload.size_bytes,provenance:payload.provenance,provenance_digest:crypto::blake3(encoding::base64::encode(encoding::cbor::encode(payload.provenance)))"));
    let script=format!("SELECT VALUE {} FROM asset WHERE asset_id=$audit_asset LIMIT 2 TIMEOUT 2s; {references_query}; {models_query}; SELECT VALUE id FROM device_payload WHERE asset_ids CONTAINS $audit_asset LIMIT 1 TIMEOUT 2s; {operations_query}",witness(asset_fields));
    let mut response = db
        .query(script)
        .bind(("audit_asset", asset_id.clone()))
        .await?
        .check()?;
    let assets: Vec<Value> = response.take(0)?;
    let references: Vec<Value> = response.take(1)?;
    let models: Vec<Value> = response.take(2)?;
    let payloads: Vec<Value> = response.take(3)?;
    let operations: Vec<Value> = response.take(4)?;
    anyhow::ensure!(
        assets.len() == 1
            && !references.is_empty()
            && references.len() <= 256
            && models.len() <= 1
            && payloads.is_empty()
            && operations.len() <= 1,
        "audit graph unavailable or exceeds bound"
    );
    anyhow::ensure!(
        serde_json::to_vec(&(&assets, &references, &models, &operations))?.len()
            <= SCENE_ASSET_AUDIT_INTERNAL_MAX,
        "audit capture exceeds bound"
    );
    let asset = &assets[0]["data"];
    anyhow::ensure!(
        text(asset, "asset_id", 128)? == asset_id,
        "asset identity changed"
    );
    let content_hash = text(asset, "content_hash", 64)?.to_owned();
    anyhow::ensure!(
        crate::hash_to_hex(&crate::hash_from_hex(&content_hash)?) == content_hash,
        "noncanonical asset hash"
    );
    let content_type = text(asset, "content_type", 64)?.to_owned();
    anyhow::ensure!(
        matches!(
            content_type.as_str(),
            "model/gltf-binary" | "model/gltf+json"
        ),
        "unsupported asset MIME"
    );
    let size_bytes = asset["size_bytes"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_SCENE_ASSET_BYTES)
        .ok_or_else(|| anyhow::anyhow!("invalid asset size"))?;
    let owner = field(asset, "owner", || {
        Ok(text(asset, "owner_did", 128)?.to_owned())
    })?;
    let anchor = field(asset, "petal", || {
        Ok(text(asset, "petal_id", 128)?.to_owned())
    })?;
    let origin = field(asset, "provenance", || {
        provenance(&asset["publication_provenance"])
    })?;
    let mut ids = BTreeSet::new();
    let mut public_refs = Vec::new();
    let mut ref_witnesses = Vec::new();
    for row in &references {
        let data = &row["witness"]["data"];
        let id = text(data, "node_id", 128)?;
        anyhow::ensure!(
            fe_runtime::safe_id::is_safe_path_id(id)
                && ids.insert(id.to_owned())
                && text(data, "petal_id", 128)? == petal_id,
            "ambiguous or foreign asset references"
        );
        let state_digest = digest(&row["witness"])?;
        public_refs.push(SceneAssetAuditReference {
            node_id: id.into(),
            petal_id: petal_id.clone(),
            state_digest: state_digest.clone(),
        });
        ref_witnesses.push(json!({"node_id":id,"digest":state_digest}));
    }
    anyhow::ensure!(ids.contains(&request.node_id), "target left reference set");
    let model_state_digest = models
        .first()
        .map(|row| {
            anyhow::ensure!(
                text(&row["data"], "petal_id", 128)? == petal_id,
                "foreign model reference"
            );
            digest(row)
        })
        .transpose()?;
    let operation = match (&request.operation_id, operations.first()) {
        (None, None) => {
            anyhow::ensure!(
                matches!(owner, SceneAssetField::Absent)
                    && matches!(anchor, SceneAssetField::Absent)
                    && matches!(origin, SceneAssetField::Absent),
                "strict baseline needs native absent fields"
            );
            None
        }
        (Some(expected), Some(row)) => {
            let data = &row["data"];
            let proof = provenance(&data["provenance"])?;
            let op_node = text(data, "node_id", 128)?;
            anyhow::ensure!(
                text(data, "op_type", 64)? == "SceneAssetAdopted"
                    && text(data, "operation_id", 26)? == expected
                    && proof.operation_id == *expected
                    && ids.contains(op_node),
                "adoption operation mismatch"
            );
            anyhow::ensure!(
                data["asset_id"] == asset_id
                    && data["scope"] == scope
                    && data["content_hash"] == content_hash
                    && data["content_type"] == content_type
                    && data["size_bytes"].as_u64() == Some(size_bytes),
                "adoption metadata mismatch"
            );
            anyhow::ensure!(
                matches!(owner, SceneAssetField::Absent)
                    && anchor == SceneAssetField::Value(petal_id.clone())
                    && origin == SceneAssetField::Value(proof.clone())
                    && text(data, "provenance_digest", 64)?
                        == text(asset, "provenance_digest", 64)?,
                "stored adoption provenance mismatch"
            );
            Some(SceneAssetAuditOperation {
                operation_id: expected.clone(),
                op_type: "SceneAssetAdopted".into(),
                node_id: op_node.into(),
                scope: scope.clone(),
                asset_id: asset_id.clone(),
                content_hash: content_hash.clone(),
                content_type: content_type.clone(),
                size_bytes,
                provenance: proof,
                state_digest: digest(row)?,
            })
        }
        _ => anyhow::bail!("requested adoption operation unavailable"),
    };
    // Only server-native digest/string sets cross the final equality boundary.
    guards.extend([
        format!("IF (SELECT VALUE {DIGEST} FROM node WHERE node_id=$audit_node) != $audit_target_digest {{ THROW 'audit target changed'; }}"),
        format!("IF (SELECT VALUE {DIGEST} FROM asset WHERE asset_id=$audit_asset) != $audit_asset_digest {{ THROW 'audit metadata changed'; }}"),
        format!("IF (SELECT node_id,{DIGEST} AS digest FROM node WHERE asset_id=$audit_asset ORDER BY node_id LIMIT 257) != $audit_refs {{ THROW 'audit references changed'; }}"),
        format!("IF (SELECT node_id,{DIGEST} AS digest FROM node WHERE node_id IN $audit_ids ORDER BY node_id LIMIT 257) != $audit_refs {{ THROW 'audit reference identity changed'; }}"),
        format!("IF (SELECT VALUE {DIGEST} FROM model WHERE asset_id=$audit_asset LIMIT 2) != $audit_models {{ THROW 'audit model changed'; }}"),
        "IF array::len((SELECT VALUE id FROM device_payload WHERE asset_ids CONTAINS $audit_asset LIMIT 1)) != 0 { THROW 'audit payload appeared'; }".into(),
        format!("IF (SELECT VALUE {DIGEST} FROM op_log WHERE op_type='SceneAssetAdopted' AND payload.asset_id=$audit_asset LIMIT 2) != $audit_ops {{ THROW 'audit operation set changed'; }}"),
    ]);
    bindings.extend([
        ("audit_node".into(), json!(request.node_id)),
        ("audit_asset".into(), json!(asset_id)),
        ("audit_target_digest".into(), json!([digest(&targets[0])?])),
        ("audit_asset_digest".into(), json!([digest(&assets[0])?])),
        ("audit_refs".into(), json!(ref_witnesses)),
        ("audit_ids".into(), json!(ids)),
        (
            "audit_models".into(),
            json!(models
                .iter()
                .map(digest)
                .collect::<anyhow::Result<Vec<_>>>()?),
        ),
        (
            "audit_ops".into(),
            json!(operations
                .iter()
                .map(digest)
                .collect::<anyhow::Result<Vec<_>>>()?),
        ),
    ]);
    let output = SceneAssetPublicationAudit {
        protocol_version: 1,
        request_nonce: nonce.into(),
        digest_algorithm: "blake3-base64-surreal-cbor-v1".into(),
        node_id: request.node_id.clone(),
        asset_id,
        petal_id,
        scope,
        content_hash,
        content_type,
        size_bytes,
        owner_did: owner,
        asset_petal_id: anchor,
        publication_provenance: origin,
        asset_state_digest: digest(&assets[0])?,
        references: public_refs,
        model_state_digest,
        payload_reference_count: 0,
        adoption_operation: operation,
        bytes_verified_by_this_read: false,
    };
    Ok(Prepared {
        output,
        guards,
        bindings,
    })
}

pub(crate) async fn read(
    db: &Db,
    request: &SceneAssetAuditRequest,
    nonce: &str,
    local_did: &str,
) -> Result<SceneAssetPublicationAudit, SceneAssetAuditFailure> {
    if !request.validate_ids() || !is_canonical_audit_ulid(nonce) {
        return Err(SceneAssetAuditFailure::InvalidRequest);
    }
    if !request.is_current() {
        return Err(SceneAssetAuditFailure::Expired);
    }
    let result = tokio::time::timeout_at(tokio::time::Instant::from_std(request.deadline), async {
        prepare(db, request, nonce, local_did)
            .await?
            .finish(db)
            .await
    })
    .await;
    match result {
        Ok(Ok(output)) if request.is_current() => Ok(output),
        Err(_) | Ok(Ok(_)) => Err(SceneAssetAuditFailure::Expired),
        Ok(Err(_)) => Err(SceneAssetAuditFailure::Unavailable),
    }
}
