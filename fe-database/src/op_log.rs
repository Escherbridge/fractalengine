use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{OpLogEntry, PetalId};

static LAMPORT_CLOCK: AtomicU64 = AtomicU64::new(0);

pub async fn write_op_log(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    mut entry: OpLogEntry,
) -> anyhow::Result<()> {
    entry.lamport_clock = LAMPORT_CLOCK.fetch_add(1, Ordering::SeqCst);
    let val = serde_json::to_value(entry)?;
    let _: Option<serde_json::Value> = db.create("op_log").content(val).await?;
    Ok(())
}

pub async fn query_petal_at_time(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    petal_id: &PetalId,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<serde_json::Value> {
    let petal_record = format!("petal:{}", petal_id.0);
    let ts = timestamp.to_rfc3339();
    let mut result: surrealdb::IndexedResults = db
        .query(format!("SELECT * FROM $record VERSION d'{ts}'"))
        .bind(("record", petal_record))
        .await?;
    let value: Option<serde_json::Value> = result.take(0)?;
    Ok(value.unwrap_or(serde_json::Value::Null))
}
