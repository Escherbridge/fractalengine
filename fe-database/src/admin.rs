use crate::repo::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownTable {
    Petal,
    Room,
    Model,
    Role,
    /// Admitted role statements — the source the `role` cache is rebuilt from.
    RoleGrant,
    /// Held (Deferred) role statements.
    RoleStatementPending,
    OpLog,
    Verse,
    VerseMember,
    Fractal,
    Node,
    Asset,
    DeviceFormat,
    DevicePayload,
}

impl KnownTable {
    pub const ALL: &'static [KnownTable] = &[
        KnownTable::Petal,
        KnownTable::Room,
        KnownTable::Model,
        KnownTable::Role,
        KnownTable::RoleGrant,
        KnownTable::RoleStatementPending,
        KnownTable::OpLog,
        KnownTable::Verse,
        KnownTable::VerseMember,
        KnownTable::Fractal,
        KnownTable::Node,
        KnownTable::Asset,
        KnownTable::DeviceFormat,
        KnownTable::DevicePayload,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            KnownTable::Petal => "petal",
            KnownTable::Room => "room",
            KnownTable::Model => "model",
            KnownTable::Role => "role",
            KnownTable::RoleGrant => "role_grant",
            KnownTable::RoleStatementPending => "role_statement_pending",
            KnownTable::OpLog => "op_log",
            KnownTable::Verse => "verse",
            KnownTable::VerseMember => "verse_member",
            KnownTable::Fractal => "fractal",
            KnownTable::Node => "node",
            KnownTable::Asset => "asset",
            KnownTable::DeviceFormat => "device_format",
            KnownTable::DevicePayload => "device_payload",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "petal" => Some(KnownTable::Petal),
            "room" => Some(KnownTable::Room),
            "model" => Some(KnownTable::Model),
            "role" => Some(KnownTable::Role),
            "role_grant" => Some(KnownTable::RoleGrant),
            "role_statement_pending" => Some(KnownTable::RoleStatementPending),
            "op_log" => Some(KnownTable::OpLog),
            "verse" => Some(KnownTable::Verse),
            "verse_member" => Some(KnownTable::VerseMember),
            "fractal" => Some(KnownTable::Fractal),
            "node" => Some(KnownTable::Node),
            "asset" => Some(KnownTable::Asset),
            "device_format" => Some(KnownTable::DeviceFormat),
            "device_payload" => Some(KnownTable::DevicePayload),
            _ => None,
        }
    }
}

impl std::fmt::Display for KnownTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub async fn clear_all_tables(db: &Db) -> anyhow::Result<()> {
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

/// Clear one table. `role` is a materialized cache, so clearing it (or either statement
/// table) rebuilds the cache from the statements that remain (src/AGENTS.md
/// §role-propagation): to remove roles, clear `role_grant`, not `role`.
pub async fn clear_table(db: &Db, table: KnownTable) -> anyhow::Result<()> {
    db.query(format!("DELETE FROM {table}")).await?;
    tracing::info!("Table '{}' cleared", table);
    if matches!(
        table,
        KnownTable::Role | KnownTable::RoleGrant | KnownTable::RoleStatementPending
    ) {
        crate::role_grants::repartition_all(db).await?;
    }
    Ok(())
}

pub async fn dump_table(db: &Db, table: KnownTable) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut result: surrealdb::IndexedResults = db.query(format!("SELECT * FROM {table}")).await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(rows)
}

pub async fn dump_all(db: &Db) -> anyhow::Result<serde_json::Value> {
    let mut snapshot = serde_json::Map::new();
    for &table in KnownTable::ALL {
        let rows = dump_table(db, table).await?;
        snapshot.insert(table.to_string(), serde_json::Value::Array(rows));
    }
    Ok(serde_json::Value::Object(snapshot))
}

pub async fn dump_to_file(db: &Db, path: &std::path::Path) -> anyhow::Result<()> {
    let snapshot = dump_all(db).await?;
    let json = serde_json::to_string_pretty(&snapshot)?;
    std::fs::write(path, json)?;
    tracing::info!("Database dumped to {}", path.display());
    Ok(())
}

pub async fn load_from_file(db: &Db, path: &std::path::Path) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(path)?;
    let snapshot: serde_json::Value = serde_json::from_str(&json)?;
    let obj = snapshot
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid dump format"))?;

    clear_all_tables(db).await?;

    // A dump with statements carries a derived `role` cache: skip it and rebuild. A legacy
    // dump (no statements) carries raw role rows: re-arm the one-shot migration instead.
    let has_statements = obj.contains_key(KnownTable::RoleGrant.as_str());

    for (table, rows) in obj {
        let known = match KnownTable::from_str(table.as_str()) {
            Some(t) => t,
            None => {
                tracing::warn!("Skipping unknown table '{}' during load", table);
                continue;
            }
        };
        if known == KnownTable::Role && has_statements {
            continue;
        }
        if let Some(arr) = rows.as_array() {
            for row in arr {
                let _: Option<serde_json::Value> = db
                    .create::<Option<serde_json::Value>>(known.as_str())
                    .content(row.clone())
                    .await?;
            }
        }
    }
    if has_statements {
        crate::role_grants::repartition_all(db).await?;
    } else {
        crate::migration::role_grants::clear_marker(db).await?;
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
