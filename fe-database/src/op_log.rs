use crate::types::{OpLogEntry, PetalId};

pub async fn write_op_log(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    entry: OpLogEntry,
) -> anyhow::Result<()> {
    db.create::<Vec<serde_json::Value>>("op_log").content(entry).await?;
    Ok(())
}

pub async fn query_petal_at_time(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    petal_id: &PetalId,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<serde_json::Value> {
    let petal_record = format!("petal:{}", petal_id.0);
    let ts = timestamp.to_rfc3339();
    let mut result = db
        .query(format!("SELECT * FROM $record VERSION d'{ts}'"))
        .bind(("record", petal_record))
        .await?;
    let value: Option<serde_json::Value> = result.take(0)?;
    Ok(value.unwrap_or(serde_json::Value::Null))
}
