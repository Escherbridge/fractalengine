use surrealdb::engine::local::Db;

pub async fn clear_all_tables(db: &surrealdb::Surreal<Db>) -> anyhow::Result<()> {
    db.query(
        "REMOVE TABLE IF EXISTS petal;
         REMOVE TABLE IF EXISTS room;
         REMOVE TABLE IF EXISTS model;
         REMOVE TABLE IF EXISTS role;
         REMOVE TABLE IF EXISTS op_log;",
    )
    .await?;
    tracing::info!("All tables cleared");
    crate::rbac::apply_schema(db).await?;
    tracing::info!("Schema re-applied");
    Ok(())
}

pub async fn clear_table(db: &surrealdb::Surreal<Db>, table: &str) -> anyhow::Result<()> {
    let table = table.to_string();
    db.query(format!("DELETE FROM {table}")).await?;
    tracing::info!("Table '{table}' cleared");
    Ok(())
}

pub async fn dump_table(
    db: &surrealdb::Surreal<Db>,
    table: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let table = table.to_string();
    let mut result = db.query(format!("SELECT * FROM {table}")).await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(rows)
}

pub async fn dump_all(db: &surrealdb::Surreal<Db>) -> anyhow::Result<serde_json::Value> {
    let tables = ["petal", "room", "model", "role", "op_log"];
    let mut snapshot = serde_json::Map::new();
    for table in &tables {
        let rows = dump_table(db, table).await?;
        snapshot.insert(table.to_string(), serde_json::Value::Array(rows));
    }
    Ok(serde_json::Value::Object(snapshot))
}

pub async fn dump_to_file(
    db: &surrealdb::Surreal<Db>,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let snapshot = dump_all(db).await?;
    let json = serde_json::to_string_pretty(&snapshot)?;
    std::fs::write(path, json)?;
    tracing::info!("Database dumped to {}", path.display());
    Ok(())
}

pub async fn load_from_file(
    db: &surrealdb::Surreal<Db>,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(path)?;
    let snapshot: serde_json::Value = serde_json::from_str(&json)?;
    let obj = snapshot
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid dump format"))?;

    clear_all_tables(db).await?;

    for (table, rows) in obj {
        if let Some(arr) = rows.as_array() {
            for row in arr {
                let _: Option<serde_json::Value> =
                    db.create(table.as_str()).content(row.clone()).await?;
            }
        }
    }
    tracing::info!("Database loaded from {}", path.display());
    Ok(())
}

pub fn clear_local_storage(db_path: &std::path::Path) -> anyhow::Result<()> {
    if db_path.exists() {
        std::fs::remove_dir_all(db_path)?;
        tracing::info!("Local storage cleared at {}", db_path.display());
    }
    Ok(())
}
