//! CRUD handlers for the `field_def` table.
//!
//! `field_def` rows describe the schema of user-defined custom properties for
//! a given entity type within a scope (verse/fractal/petal).  The `value_type`
//! column maps to [`fe_format::PropertyType`] variant names (e.g. `"string"`,
//! `"number"`, `"bool"`, …).

use tracing::instrument;

use fe_query::{DeleteBuilder, Filter, InsertBuilder, QueryBuilder, SortDir};

use crate::query_helpers::exec_query;
use crate::repo::Db;

/// Create a new field definition and return its generated ID.
///
/// Generates a ULID for `field_def_id` and inserts the row into the
/// `field_def` table.
#[instrument(skip(db))]
pub(crate) async fn create_field_def_handler(
    db: &Db,
    scope: &str,
    entity_type: &str,
    key: &str,
    value_type: &str,
    default_val: Option<&serde_json::Value>,
    created_by: &str,
) -> anyhow::Result<String> {
    let field_def_id = ulid::Ulid::new().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    let q = InsertBuilder::insert_into("field_def")
        .values(serde_json::json!({
            "field_def_id": field_def_id,
            "scope":        scope,
            "entity_type":  entity_type,
            "key":          key,
            "value_type":   value_type,
            "default_val":  default_val,
            "created_by":   created_by,
            "created_at":   now,
        }))
        .build();

    exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("CreateFieldDef DB query failed: {e}"))?;

    Ok(field_def_id)
}

/// List all field definitions for a given scope, ordered by `key` ascending.
#[instrument(skip(db))]
pub(crate) async fn list_field_defs_handler(
    db: &Db,
    scope: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("field_def")
        .filter(Filter::eq("scope", scope))
        .order_by("key", SortDir::Asc)
        .build();

    let mut res = exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("ListFieldDefs DB query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("ListFieldDefs take failed: {e}"))?;

    Ok(rows)
}

/// Fetch a single field definition by its ID.
///
/// Returns `None` if no matching row exists.
#[allow(dead_code)] // not yet wired to a DbCommand — planned single-fetch API
#[instrument(skip(db))]
pub(crate) async fn get_field_def_handler(
    db: &Db,
    field_def_id: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("field_def")
        .filter(Filter::eq("field_def_id", field_def_id))
        .limit(1)
        .build();

    let mut res = exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("GetFieldDef DB query failed: {e}"))?;

    let rows: Vec<serde_json::Value> = res
        .take(0)
        .map_err(|e| anyhow::anyhow!("GetFieldDef take failed: {e}"))?;

    Ok(rows.into_iter().next())
}

/// Update the `value_type` and `default_val` of an existing field definition.
#[instrument(skip(db))]
pub(crate) async fn update_field_def_handler(
    db: &Db,
    field_def_id: &str,
    value_type: &str,
    default_val: Option<&serde_json::Value>,
) -> anyhow::Result<()> {
    // Build the SET clause manually via raw query to handle the Option<Value>
    // default_val — UpdateBuilder::set() requires a concrete QueryValue impl
    // and does not accept Option<&Value> directly.
    let default_json = serde_json::to_string(&default_val).unwrap_or_else(|_| "null".to_string());
    let sql = format!(
        "UPDATE field_def SET value_type = $vt, default_val = {} WHERE field_def_id = $id",
        default_json,
    );
    db.query(sql)
        .bind(("vt", value_type.to_string()))
        .bind(("id", field_def_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("UpdateFieldDef DB query failed: {e}"))?;

    Ok(())
}

/// Delete a field definition by its ID.
#[instrument(skip(db))]
pub(crate) async fn delete_field_def_handler(db: &Db, field_def_id: &str) -> anyhow::Result<()> {
    let q = DeleteBuilder::delete_from("field_def")
        .where_clause(Filter::eq("field_def_id", field_def_id))
        .build();

    exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("DeleteFieldDef DB query failed: {e}"))?;

    Ok(())
}
