use surrealdb::engine::local::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownTable {
    Petal, Room, Model, Role, OpLog,
    Verse, VerseMember, Fractal, Node, Asset,
}

impl KnownTable {
    pub const ALL: &'static [KnownTable] = &[
        KnownTable::Petal, KnownTable::Room, KnownTable::Model,
        KnownTable::Role, KnownTable::OpLog, KnownTable::Verse,
        KnownTable::VerseMember, KnownTable::Fractal, KnownTable::Node,
        KnownTable::Asset,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            KnownTable::Petal       => "petal",
            KnownTable::Room        => "room",
            KnownTable::Model       => "model",
            KnownTable::Role        => "role",
            KnownTable::OpLog       => "op_log",
            KnownTable::Verse       => "verse",
            KnownTable::VerseMember => "verse_member",
            KnownTable::Fractal     => "fractal",
            KnownTable::Node        => "node",
            KnownTable::Asset       => "asset",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "petal"        => Some(KnownTable::Petal),
            "room"         => Some(KnownTable::Room),
            "model"        => Some(KnownTable::Model),
            "role"         => Some(KnownTable::Role),
            "op_log"       => Some(KnownTable::OpLog),
            "verse"        => Some(KnownTable::Verse),
            "verse_member" => Some(KnownTable::VerseMember),
            "fractal"      => Some(KnownTable::Fractal),
            "node"         => Some(KnownTable::Node),
            "asset"        => Some(KnownTable::Asset),
            _              => None,
        }
    }
}

impl std::fmt::Display for KnownTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub async fn clear_all_tables(db: &surrealdb::Surreal<Db>) -> anyhow::Result<()> {
    let stmt = KnownTable::ALL
        .iter()
        .map(|t| format!("REMOVE TABLE IF EXISTS {};", t))
        .collect::<Vec<_>>()
        .join("\n");
    db.query(stmt).await?;
    tracing::info!("All tables cleared");
    crate::rbac::apply_schema(db).await?;
    tracing::info!("Schema re-applied");
    Ok(())
}

pub async fn clear_table(db: &surrealdb::Surreal<Db>, table: KnownTable) -> anyhow::Result<()> {
    db.query(format!("DELETE FROM {table}")).await?;
    tracing::info!("Table '{}' cleared", table);
    Ok(())
}

pub async fn dump_table(
    db: &surrealdb::Surreal<Db>,
    table: KnownTable,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut result: surrealdb::IndexedResults = db.query(format!("SELECT * FROM {table}")).await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(rows)
}

pub async fn dump_all(db: &surrealdb::Surreal<Db>) -> anyhow::Result<serde_json::Value> {
    let mut snapshot = serde_json::Map::new();
    for &table in KnownTable::ALL {
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
        let known = match KnownTable::from_str(table.as_str()) {
            Some(t) => t,
            None => {
                tracing::warn!("Skipping unknown table '{}' during load", table);
                continue;
            }
        };
        if let Some(arr) = rows.as_array() {
            for row in arr {
                let _: Option<serde_json::Value> =
                    db.create::<Option<serde_json::Value>>(known.as_str()).content(row.clone()).await?;
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
