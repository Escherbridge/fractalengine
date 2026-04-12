use crate::op_log::write_op_log;
use crate::repo::Repo;
use crate::schema::{Model, Petal, Room};
use crate::types::{NodeId, OpLogEntry, OpType, PetalId};

pub async fn create_petal(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    name: &str,
    node_id: &NodeId,
    caller_node_id: &NodeId,
) -> anyhow::Result<PetalId> {
    // TODO: verse-level permission check for petal creation
    let _ = caller_node_id; // RBAC skipped for creation: no petal_id exists yet
    let petal_id = PetalId(ulid::Ulid::new());
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: node_id.clone(),
        op_type: OpType::CreatePetal,
        payload: serde_json::json!({ "name": name, "petal_id": petal_id.0.to_string() }),
        sig: "00".repeat(64),
    };
    write_op_log(db, entry).await?;
    let petal = Petal {
        petal_id: petal_id.0.to_string(),
        name: name.to_string(),
        node_id: node_id.0.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        description: None,
        visibility: "private".to_string(),
        tags: vec![],
        fractal_id: None,
        bounds: None,
    };
    Repo::<Petal>::create(db, &petal).await?;
    Ok(petal_id)
}

pub async fn create_room(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    caller_node_id: &NodeId,
    petal_id: &PetalId,
    name: &str,
) -> anyhow::Result<()> {
    crate::rbac::require_write_role(db, &caller_node_id.0, &petal_id.0.to_string()).await?;
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: caller_node_id.clone(),
        op_type: OpType::CreateRoom,
        payload: serde_json::json!({ "petal_id": petal_id.0.to_string(), "name": name }),
        sig: "00".repeat(64),
    };
    write_op_log(db, entry).await?;
    let room = Room {
        petal_id: petal_id.0.to_string(),
        name: name.to_string(),
        description: None,
        bounds: None,
        spawn_point: None,
    };
    Repo::<Room>::create(db, &room).await?;
    Ok(())
}

pub async fn place_model(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    caller_node_id: &NodeId,
    petal_id: &PetalId,
    asset_id: &str,
    transform: serde_json::Value,
) -> anyhow::Result<()> {
    crate::rbac::require_write_role(db, &caller_node_id.0, &petal_id.0.to_string()).await?;
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: caller_node_id.clone(),
        op_type: OpType::PlaceModel,
        payload: serde_json::json!({
            "petal_id": petal_id.0.to_string(),
            "asset_id": asset_id,
            "transform": transform,
        }),
        sig: "00".repeat(64),
    };
    write_op_log(db, entry).await?;
    let model = Model {
        petal_id: petal_id.0.to_string(),
        asset_id: asset_id.to_string(),
        transform,
        display_name: None,
        description: None,
        external_url: None,
        config_url: None,
        tags: vec![],
        metadata: None,
    };
    Repo::<Model>::create(db, &model).await?;
    Ok(())
}
