use crate::schema;
use crate::types::RoleId;

pub async fn apply_schema(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
) -> anyhow::Result<()> {
    db.query(schema::DEFINE_PETAL).await?;
    db.query(schema::DEFINE_ROOM).await?;
    db.query(schema::DEFINE_MODEL).await?;
    db.query(schema::DEFINE_ROLE).await?;
    db.query(schema::DEFINE_OP_LOG).await?;
    Ok(())
}

pub async fn get_role(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    node_id: &str,
    petal_id: &str,
) -> anyhow::Result<RoleId> {
    let node_id = node_id.to_string();
    let petal_id = petal_id.to_string();
    let mut result = db
        .query("SELECT role FROM role WHERE node_id = $node_id AND petal_id = $petal_id")
        .bind(("node_id", node_id))
        .bind(("petal_id", petal_id))
        .await?;
    let role: Option<serde_json::Value> = result.take(0)?;
    Ok(role
        .and_then(|v| v["role"].as_str().map(|s| RoleId(s.to_string())))
        .unwrap_or_else(|| RoleId("public".to_string())))
}
