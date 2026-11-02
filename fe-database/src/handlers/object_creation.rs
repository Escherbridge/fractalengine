//! Sealed ordinary creation and initial history; see handlers/AGENTS.md.

use fe_runtime::node_history::NodeAccessGrant;
use serde_json::{Map, Value, json};

use super::{entity, field_def, node_history};
use crate::{op_log::commit_operation_atomic_with, repo::Db, types::OpLogEntry};

/// Metadata for bytes already admitted into the blob store.
#[derive(Clone)]
pub(crate) struct ImportedSceneAsset {
    id: String,
    path: String,
    row: Value,
}

impl ImportedSceneAsset {
    pub(crate) fn new(
        name: &str,
        content_type: &str,
        hash: &str,
        size: usize,
    ) -> anyhow::Result<Self> {
        let extension = scene_extension(content_type)?;
        crate::hash_from_hex(hash)?;
        let id = ulid::Ulid::new().to_string();
        Ok(Self {
            path: format!("blob://{hash}.{extension}"),
            row: json!({"asset_id": id, "name": name, "content_type": content_type,
                "size_bytes": i64::try_from(size)?, "content_hash": hash,
                "created_at": chrono::Utc::now().to_rfc3339()}),
            id,
        })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

pub(crate) enum SceneAsset<'a> {
    Existing(Option<&'a str>),
    Imported(ImportedSceneAsset),
}

pub(crate) struct ObjectCreation<'a> {
    pub petal_id: &'a str,
    pub name: &'a str,
    pub asset: SceneAsset<'a>,
    pub position: [f32; 3],
    pub rotation: Vec<f64>,
    pub scale: Vec<f64>,
    pub properties: Map<String, Value>,
    pub actor: &'a str,
    pub grant: &'a NodeAccessGrant,
}

/// Captured authority, schemas and metadata consumed by one atomic commit.
pub(crate) struct PreparedCreation {
    statements: Vec<String>,
    bindings: Vec<(String, Value)>,
    state: Value,
    scope: String,
    actor: String,
    schema_versions: Map<String, Value>,
    id: String,
    materialization: fe_runtime::messages::CommittedNodeMaterialization,
}

fn scene_extension(content_type: &str) -> anyhow::Result<&'static str> {
    match content_type {
        "model/gltf-binary" => Ok("glb"),
        "model/gltf+json" => Ok("gltf"),
        _ => anyhow::bail!("asset is not a scene asset"),
    }
}

pub(crate) async fn prepare(
    db: &Db,
    input: ObjectCreation<'_>,
) -> anyhow::Result<PreparedCreation> {
    let ObjectCreation {
        petal_id,
        name,
        asset,
        position,
        rotation,
        scale,
        properties,
        actor,
        grant,
    } = input;
    let scope = super::preconditions::require_petal_scope(db, petal_id, "CreateObject").await?;
    node_history::authorize_scope(db, grant, actor, &scope, fe_policy::RoleLevel::Editor).await?;
    anyhow::ensure!(
        !name.trim().is_empty()
            && name.len() <= 1024
            && position.iter().all(|v| v.is_finite())
            && matches!(rotation.len(), 3 | 4)
            && scale.len() == 3
            && rotation.iter().chain(&scale).all(|v| v.is_finite()),
        "object requires a non-empty bounded name and finite complete transform"
    );
    let (mut statements, mut bindings) = node_history::scope_fence(&scope, None)?;
    if matches!(grant, NodeAccessGrant::Local) {
        let permission =
            entity::prepare_scope_permission(db, &scope, actor, fe_policy::RoleLevel::Editor)
                .await?;
        let (guards, values) = permission.into_parts();
        statements.extend(guards);
        bindings.extend(values);
    }
    let mut schema_versions = Map::new();
    for (index, (key, value)) in properties.iter().enumerate() {
        let rules = field_def::capture_property_rules(db, &scope, key).await?;
        if let Value::Object(versions) =
            field_def::validate_captured_property_rules(&scope, key, value, &rules)?
        {
            schema_versions.extend(versions);
        }
        let key_binding = format!("creation_schema_key_{index}");
        let rules_binding = format!("creation_schema_{index}");
        statements.push(format!(
            "IF ({}) != ${rules_binding} {{ THROW 'object creation schema changed'; }}",
            field_def::PROPERTY_RULES_SQL
                .replace("$key", &format!("${key_binding}"))
                .replace("$scopes", "$creation_scopes")
        ));
        bindings.extend([(key_binding, json!(key)), (rules_binding, json!(rules))]);
    }
    bindings.push((
        "creation_scopes".into(),
        json!(field_def::property_rule_scopes(&scope)),
    ));
    let imported_asset = matches!(&asset, SceneAsset::Imported(_));
    let (asset_id, asset_path) = match asset {
        SceneAsset::Existing(None) => (None, None),
        SceneAsset::Existing(Some(id)) => {
            let mut response = db.query("SELECT asset_id, content_hash, content_type FROM asset WHERE asset_id=$creation_asset LIMIT 2")
                .bind(("creation_asset", id.to_owned())).await?.check()?;
            let rows: Vec<Value> = response.take(0)?;
            anyhow::ensure!(
                rows.len() == 1,
                "CreateObject matched no asset or an ambiguous asset"
            );
            let hash = rows[0]["content_hash"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("scene asset has no content hash"))?;
            let extension = scene_extension(rows[0]["content_type"].as_str().unwrap_or_default())?;
            let path = format!("blob://{hash}.{extension}");
            statements.push("IF (SELECT asset_id, content_hash, content_type FROM asset WHERE asset_id=$creation_asset) != $creation_asset_before { THROW 'scene asset changed during creation'; }".into());
            bindings.extend([
                ("creation_asset".into(), json!(id)),
                ("creation_asset_before".into(), json!(rows)),
            ]);
            (Some(id.to_owned()), Some(path))
        }
        SceneAsset::Imported(asset) => {
            statements.extend([
                "IF array::len((SELECT VALUE asset_id FROM asset WHERE asset_id=$creation_asset)) != 0 { THROW 'imported asset identity already exists'; }".into(),
                "CREATE asset CONTENT $creation_imported_asset".into(),
            ]);
            bindings.extend([
                ("creation_asset".into(), json!(asset.id)),
                ("creation_imported_asset".into(), asset.row),
            ]);
            (Some(asset.id), Some(asset.path))
        }
    };
    let id = ulid::Ulid::new().to_string();
    let mut state = json!({"node_id":id,"petal_id":petal_id,"display_name":name,
        "position":position,"elevation":position[1] as f64,"rotation":rotation,"scale":scale,
        "properties":properties,"interactive":asset_id.is_some(),"edit_seq":0,
        "created_at":chrono::Utc::now().to_rfc3339()});
    if let Some(asset_id) = asset_id {
        state["asset_id"] = json!(asset_id);
    }
    node_history::check_snapshot_size(&state)?;
    let mut without_position = state.clone();
    without_position.as_object_mut().unwrap().remove("position");
    statements.extend([
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$creation_id)) != 0 OR array::len((SELECT VALUE log_id FROM node_log WHERE node_id=$creation_id)) != 0 { THROW 'object identity already exists'; }".into(),
        "CREATE node CONTENT object::extend($created_state, {position:<geometry<point>> [$created_x,$created_z]})".into(),
        "CREATE node_log CONTENT $created_fact".into(),
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$creation_id) != $creation_expected_node { THROW 'created object readback changed'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$creation_id AND position=<geometry<point>> [$created_x,$created_z])) != 1 { THROW 'created object position changed'; }".into(),
        "IF (SELECT * OMIT id FROM node_log WHERE node_id=$creation_id) != [$created_fact] { THROW 'created object history changed'; }".into(),
    ]);
    if imported_asset {
        statements.push("IF (SELECT asset_id, name, content_type, size_bytes, content_hash, created_at FROM asset WHERE asset_id=$creation_asset) != [$creation_imported_asset] { THROW 'imported asset readback changed'; }".into());
    }
    bindings.extend([
        ("creation_id".into(), json!(id)),
        ("creation_expected_node".into(), json!([without_position])),
        ("created_x".into(), json!(position[0] as f64)),
        ("created_z".into(), json!(position[2] as f64)),
    ]);
    let snapshot: fe_runtime::node_history::NodeRowSnapshot =
        serde_json::from_value(state.clone())?;
    let receipt = fe_runtime::node_history::NodeMutationReceipt {
        node_id: id.clone(),
        history_version: 1,
        state: snapshot,
        persist: fe_runtime::node_history::PersistStatus::Committed,
    };
    let materialization = node_history::materialization(&receipt, asset_path)?;
    Ok(PreparedCreation {
        statements,
        bindings,
        state,
        scope,
        actor: actor.to_owned(),
        schema_versions,
        id,
        materialization,
    })
}

pub(crate) async fn commit(
    db: &Db,
    prepared: PreparedCreation,
) -> anyhow::Result<fe_runtime::messages::CommittedNodeMaterialization> {
    let PreparedCreation {
        statements,
        mut bindings,
        state,
        scope,
        actor,
        schema_versions,
        id,
        materialization,
    } = prepared;
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: crate::types::NodeId(id.clone()),
        op_type: crate::types::OpType::NodeCreated,
        payload: state.clone(),
        sig: "00".repeat(64),
    };
    commit_operation_atomic_with(db, entry, move |hlc| {
        let fact = json!({"log_id":ulid::Ulid::new().to_string(),"node_id":state["node_id"],
            "hlc_timestamp":hlc as i64,"op":"baseline","source_did":actor,"row_version":1,
            "created_at":chrono::Utc::now().to_rfc3339(),"payload":{"journal_version":node_history::JOURNAL_VERSION,
                "scope":scope,"state":state,"after_state":state,"coverage_reason":"object_created","schema_versions":schema_versions}});
        bindings.extend([("created_state".into(), state), ("created_fact".into(), fact)]);
        (statements, bindings)
    }).await?;
    Ok(materialization)
}
