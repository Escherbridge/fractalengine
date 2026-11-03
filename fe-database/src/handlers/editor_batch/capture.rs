//! Exact bounded selection reads and reusable transaction fences.

use crate::{
    handlers::{crud, entity, node_history, preconditions},
    repo::Db,
};
use fe_runtime::{editor_batch::*, node_history::NodeRowSnapshot};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(super) struct StoredNode {
    pub raw: Value,
    pub state: Value,
    pub previous: Option<Value>,
}

pub(super) struct Captured {
    pub view: EditorBatchCapture,
    pub nodes: Vec<StoredNode>,
    pub guards: Vec<String>,
    pub bindings: Vec<(String, Value)>,
}

pub(super) fn validate_ids(ids: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(
        !ids.is_empty() && ids.len() <= MAX_EDITOR_BATCH_TARGETS,
        "select 1..=256 stored objects"
    );
    let mut seen = BTreeSet::new();
    anyhow::ensure!(
        ids.iter()
            .all(|id| !id.is_empty() && id.len() <= 512 && seen.insert(id)),
        "duplicate or invalid batch target"
    );
    Ok(())
}

pub(super) fn bounded(value: &impl serde::Serialize) -> anyhow::Result<()> {
    anyhow::ensure!(
        serde_json::to_vec(value)?.len() <= MAX_EDITOR_BATCH_BYTES,
        "editor batch exceeds 4 MiB"
    );
    Ok(())
}

pub(super) async fn node(db: &Db, id: &str, petal: &str) -> anyhow::Result<StoredNode> {
    let mut query = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$id LIMIT 2")
        .bind(("id", id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = query.take(0)?;
    anyhow::ensure!(
        rows.len() == 1
            && rows[0]["petal_id"] == petal
            && rows[0].get("tombstone").is_none_or(Value::is_null),
        "batch object unavailable in the captured Petal"
    );
    let raw = rows[0].clone();
    Ok(StoredNode {
        state: node_history::normalize_snapshot(raw.clone())?,
        raw,
        previous: node_history::latest(db, id).await?,
    })
}

pub(super) fn version(node: &StoredNode) -> Option<i64> {
    node.previous
        .as_ref()
        .and_then(|row| row["row_version"].as_i64())
}

pub(super) fn fence(
    node: &StoredNode,
    prefix: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) {
    let mut raw = node.raw.clone();
    raw.as_object_mut()
        .expect("stored object")
        .remove("position");
    guards.extend([
        format!("IF (SELECT * OMIT id, position FROM node WHERE node_id=${prefix}_id) != ${prefix}_before {{ THROW 'batch object changed'; }}"),
        format!("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=${prefix}_id AND position=<geometry<point>> [${prefix}_x,${prefix}_z])) != 1 {{ THROW 'batch position changed'; }}"),
        format!("IF (SELECT * OMIT id FROM node_log WHERE node_id=${prefix}_id ORDER BY row_version DESC LIMIT 1) != ${prefix}_history {{ THROW 'batch history changed'; }}"),
    ]);
    bindings.extend([
        (format!("{prefix}_id"), node.state["node_id"].clone()),
        (format!("{prefix}_before"), json!([raw])),
        (format!("{prefix}_x"), node.state["position"][0].clone()),
        (format!("{prefix}_z"), node.state["position"][2].clone()),
        (
            format!("{prefix}_history"),
            json!(node.previous.iter().collect::<Vec<_>>()),
        ),
    ]);
}

pub(super) async fn prepare(
    db: &Db,
    scope: &str,
    petal_id: &str,
    ids: &[String],
    actor: &str,
    floor: fe_policy::RoleLevel,
) -> anyhow::Result<Captured> {
    validate_ids(ids)?;
    anyhow::ensure!(
        crud::resolve_petal_scope_handler(db, petal_id)
            .await?
            .as_deref()
            == Some(scope),
        "batch ancestry unavailable"
    );
    let permission = entity::prepare_scope_permission(db, scope, actor, floor).await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let mut response = db
        .query(format!(
            "SELECT *, {} AS stamp_petal_snapshot FROM petal WHERE petal_id=$petal LIMIT 2",
            preconditions::PETAL_TYPED_SNAPSHOT_SQL
        ))
        .bind(("petal", petal_id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(rows.len() == 1, "ambiguous batch Petal");
    let petal_snapshot = preconditions::petal_typed_snapshot(&rows[0])?.to_owned();
    let groups = match rows[0].get("visibility_groups") {
        None | Some(Value::Null) => vec![],
        Some(value) => serde_json::from_value(value.clone())?,
    };
    crate::handlers::visibility_groups::validate_groups(&groups).map_err(anyhow::Error::msg)?;
    let world_scale = rows[0]["terrain"]["world_scale"]
        .as_f64()
        .map(|v| v as f32)
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    guards.push(format!("IF (SELECT VALUE {} FROM petal WHERE petal_id=$batch_petal) != [$batch_petal_snapshot] {{ THROW 'batch Petal changed'; }}",preconditions::PETAL_TYPED_SNAPSHOT_SQL));
    bindings.extend([
        ("batch_petal".into(), json!(petal_id)),
        ("batch_petal_snapshot".into(), json!(petal_snapshot)),
    ]);
    let mut nodes = Vec::with_capacity(ids.len());
    let mut members = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        let current = node(db, id, petal_id).await?;
        let state: NodeRowSnapshot = serde_json::from_value(current.state.clone())?;
        fence(
            &current,
            &format!("batch_node_{index}"),
            &mut guards,
            &mut bindings,
        );
        let stamp_path =
            if state.properties.get("node_kind").and_then(Value::as_str) == Some("stamp") {
                let path_id = state
                    .properties
                    .get("path_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("stamp owner missing"))?;
                let instance = state
                    .properties
                    .get("instance_index")
                    .and_then(Value::as_u64)
                    .filter(|v| *v < 4096)
                    .ok_or_else(|| anyhow::anyhow!("stamp index invalid"))?;
                anyhow::ensure!(
                    state
                        .properties
                        .get("owning_path_id")
                        .and_then(Value::as_str)
                        == Some(path_id)
                        && id == &format!("{path_id}#inst-{instance}"),
                    "stamp canonical identity changed"
                );
                let path = node(db, path_id, petal_id).await?;
                fence(
                    &path,
                    &format!("batch_path_{index}"),
                    &mut guards,
                    &mut bindings,
                );
                Some(EditorStampPath {
                    history_version: version(&path),
                    state: serde_json::from_value(path.state)?,
                })
            } else {
                None
            };
        members.push(EditorBatchMember {
            node_id: id.clone(),
            history_version: version(&current),
            state,
            stamp_path,
        });
        nodes.push(current);
    }
    let view = EditorBatchCapture {
        scope: scope.into(),
        petal_id: petal_id.into(),
        members,
        groups,
        petal_snapshot,
        world_scale,
    };
    bounded(&view)?;
    Ok(Captured {
        view,
        nodes,
        guards,
        bindings,
    })
}

pub(super) async fn verify(db: &Db, captured: &Captured) -> anyhow::Result<()> {
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        captured.guards.join("; ")
    ));
    for binding in &captured.bindings {
        query = query.bind(binding.clone());
    }
    query.await?.check()?;
    Ok(())
}
