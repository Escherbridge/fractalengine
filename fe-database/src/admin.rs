use crate::repo::Db;

/// Host-captured development capability; not serialized or supplied by commands.
#[derive(Debug, Clone)]
pub struct DevelopmentResetPermit {
    enabled: bool,
    runtime_ready: bool,
}

impl DevelopmentResetPermit {
    pub fn capture() -> Self {
        Self { enabled: cfg!(debug_assertions) || std::env::var("FE_DEV_TOOLS").as_deref() == Ok("1"), runtime_ready: false }
    }

    pub fn disabled() -> Self { Self { enabled: false, runtime_ready: false } }

    pub fn is_enabled(&self) -> bool { self.enabled }

    /// Trusted maintenance hosts only: the caller owns an isolated, unopened-to-peers store.
    pub fn capture_for_offline_store() -> Self {
        Self { runtime_ready: true, ..Self::capture() }
    }

    pub fn is_runtime_ready(&self) -> bool { self.runtime_ready }

    pub(crate) fn after_quiescence(&self, proof: &fe_runtime::maintenance::QuiescenceProof) -> anyhow::Result<Self> {
        anyhow::ensure!(proof.consume(), "Terminal reset proof was already consumed");
        Ok(Self { enabled: self.enabled, runtime_ready: true })
    }

}

/// A host-admitted immutable reset intention fenced again at commit.
pub struct DevelopmentResetPlan {
    sql: String,
    verses: Vec<serde_json::Value>,
    table_count: usize,
    auth_generation: String,
    next_auth_generation: String,
}

impl DevelopmentResetPlan {
    pub async fn commit(self, db: &Db) -> anyhow::Result<()> {
        db.query(self.sql)
            .bind(("reset_verses", self.verses))
            .bind(("reset_auth_before", self.auth_generation))
            .bind(("reset_auth_after", self.next_auth_generation))
            .await?
            .check()?;
        tracing::info!(
            table_count = self.table_count,
            "Development local database rows cleared atomically"
        );
        Ok(())
    }
}

/// Atomically clear all known local database rows after global Owner admission.
pub async fn clear_for_development_reset(
    db: &Db,
    local_did: &str,
    permit: &DevelopmentResetPermit,
) -> anyhow::Result<()> {
    prepare_development_reset(db, local_did, permit)
        .await?
        .commit(db)
        .await
}

/// Capture global ownership and the complete allowlisted local table set.
pub async fn prepare_development_reset(
    db: &Db,
    local_did: &str,
    permit: &DevelopmentResetPermit,
) -> anyhow::Result<DevelopmentResetPlan> {
    anyhow::ensure!(
        permit.enabled,
        "Local database reset requires developer tools enabled at startup"
    );
    anyhow::ensure!(permit.runtime_ready, "Local database reset is unavailable while API and synchronization writers are running; close the app for isolated maintenance");
    anyhow::ensure!(
        local_did.starts_with("did:key:") && local_did.len() > 16,
        "Local database reset requires a resolved node identity"
    );
    let auth_generation = crate::api_auth::current_generation(db).await?;
    let next_auth_generation = ulid::Ulid::new().to_string();
    let mut snapshot = db
        .query("SELECT verse_id, created_by FROM verse ORDER BY verse_id LIMIT 1025; INFO FOR DB;")
        .await?
        .check()?;
    let verses: Vec<serde_json::Value> = snapshot.take(0)?;
    let metadata: Option<serde_json::Value> = snapshot.take(1)?;
    let metadata = metadata
        .ok_or_else(|| anyhow::anyhow!("Local database reset cannot read the table inventory"))?;
    anyhow::ensure!(
        verses.len() <= 1024,
        "Local database reset exceeds the 1024-Verse safety bound"
    );
    let mut identities = std::collections::BTreeSet::new();
    for verse in &verses {
        let verse_id = verse["verse_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Local database reset found unknown Verse identity"))?;
        anyhow::ensure!(
            identities.insert(verse_id),
            "Local database reset found ambiguous Verse identity"
        );
        anyhow::ensure!(
            verse["created_by"].as_str() == Some(local_did),
            "Local database reset requires this node to own every stored Verse"
        );
    }
    let tables = metadata["tables"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("Local database reset cannot verify the table inventory"))?;
    anyhow::ensure!(
        tables.len() <= 64,
        "Local database reset table inventory is too large"
    );
    for name in tables.keys() {
        anyhow::ensure!(
            name == crate::api_auth::STATE_TABLE
                || crate::schema::DEVELOPMENT_RESET_TABLE_NAMES.contains(&name.as_str()),
            "Local database reset refuses an unknown table"
        );
    }
    let mut sql = String::from("BEGIN TRANSACTION; IF (SELECT verse_id, created_by FROM verse ORDER BY verse_id) != $reset_verses { THROW 'Local database ownership changed'; }; IF (SELECT VALUE generation FROM api_auth_state WHERE id = api_auth_state:current) != [$reset_auth_before] { THROW 'Credential generation changed'; }; UPDATE api_auth_state:current SET generation = $reset_auth_after;\n");
    let content_tables: Vec<_> = tables
        .keys()
        .filter(|name| name.as_str() != crate::api_auth::STATE_TABLE)
        .collect();
    for name in &content_tables {
        sql.push_str(&format!("DELETE FROM {name};\n"));
    }
    for name in &content_tables {
        sql.push_str(&format!("IF array::len((SELECT VALUE id FROM {name} LIMIT 1)) != 0 {{ THROW 'Local database reset left rows'; }};\n"));
    }
    sql.push_str("IF (SELECT VALUE generation FROM api_auth_state WHERE id = api_auth_state:current) != [$reset_auth_after] { THROW 'Credential reset unconfirmed'; }; COMMIT TRANSACTION;");
    Ok(DevelopmentResetPlan {
        sql,
        verses,
        table_count: content_tables.len(),
        auth_generation,
        next_auth_generation,
    })
}

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
