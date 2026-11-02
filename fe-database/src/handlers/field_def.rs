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
    let row = get_field_def_handler(db, field_def_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("field definition not found"))?;
    if let Some(descriptor) = row.get("enforced_schema").filter(|value| !value.is_null()) {
        let descriptor = parse_descriptor(descriptor)?;
        if row["value_type"].as_str() != Some(value_type) {
            anyhow::bail!("enforced definitions change validation through a new schema revision");
        }
        if let Some(default) = default_val {
            descriptor.validate(default).map_err(anyhow::Error::msg)?;
        }
    }
    db.query(
        "UPDATE field_def SET value_type = $vt, default_val = $default WHERE field_def_id = $id",
    )
    .bind(("vt", value_type.to_string()))
    .bind(("default", default_val.cloned()))
    .bind(("id", field_def_id.to_string()))
    .await
    .map_err(|e| anyhow::anyhow!("UpdateFieldDef DB query failed: {e}"))?
    .check()?;

    Ok(())
}

/// Delete a field definition by its ID.
#[instrument(skip(db))]
pub(crate) async fn delete_field_def_handler(db: &Db, field_def_id: &str) -> anyhow::Result<()> {
    if get_field_def_handler(db, field_def_id)
        .await?
        .is_some_and(|row| {
            row.get("enforced_schema")
                .is_some_and(|value| !value.is_null())
        })
    {
        anyhow::bail!("enforced definitions cannot be deleted; publish a compatible revision");
    }
    let q = DeleteBuilder::delete_from("field_def")
        .where_clause(Filter::eq("field_def_id", field_def_id))
        .build();

    exec_query(db, &q)
        .await
        .map_err(|e| anyhow::anyhow!("DeleteFieldDef DB query failed: {e}"))?;

    Ok(())
}

fn parse_descriptor(
    value: &serde_json::Value,
) -> anyhow::Result<fe_sdk::property_schema::EnforcedPropertySchema> {
    fe_sdk::property_schema::EnforcedPropertySchema::parse(value).map_err(anyhow::Error::msg)
}

/// Check every applicable ancestor rule; descendant metadata cannot weaken it.
pub(crate) async fn validate_property(
    db: &Db,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node scope not found"))?;
    validate_property_in_scope(db, &scope, key, value).await
}

/// Validate a value before creating a node or mutating an existing one.
pub(crate) async fn validate_property_in_scope(
    db: &Db,
    scope: &str,
    key: &str,
    value: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let mut response = db.query("SELECT field_def_id, scope, enforced_schema FROM field_def WHERE entity_type = 'node' AND key = $key AND enforced_schema != NONE")
        .bind(("key", key.to_string())).await?.check()?;
    let rows: Vec<serde_json::Value> = response.take(0)?;
    let mut versions = serde_json::Map::new();
    for row in rows {
        let definition_scope = row["scope"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("definition scope missing"))?;
        if crate::scope_contains(definition_scope, scope) {
            let descriptor = parse_descriptor(&row["enforced_schema"])?;
            descriptor
                .validate(value)
                .map_err(|error| anyhow::anyhow!("property {key:?}: {error}"))?;
            let id = row["field_def_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("definition id missing"))?;
            versions.insert(id.to_string(), descriptor.version.into());
        }
    }
    Ok(versions.into())
}

/// Install a compatible immutable revision after dry-running affected current values.
pub(crate) async fn set_field_schema_handler(
    db: &Db,
    field_def_id: &str,
    value: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let descriptor = parse_descriptor(value)?;
    let row = get_field_def_handler(db, field_def_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("field definition not found"))?;
    if row["entity_type"].as_str() != Some("node") {
        anyhow::bail!("enforced property schemas currently support node definitions only");
    }
    let previous = row
        .get("enforced_schema")
        .filter(|v| !v.is_null())
        .map(parse_descriptor)
        .transpose()?;
    let next = previous.map_or(Ok(1), |old| {
        old.version
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("schema version exhausted"))
    })?;
    if descriptor.version != next {
        anyhow::bail!("schema version must be {next}");
    }
    if let Some(default) = row.get("default_val").filter(|v| !v.is_null()) {
        descriptor
            .validate(default)
            .map_err(|e| anyhow::anyhow!("default: {e}"))?;
    }
    let scope = row["scope"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("definition scope missing"))?;
    let key = row["key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("definition key missing"))?;
    super::preconditions::require_scope_exists(db, scope, "SetFieldSchema").await?;
    let nodes = scoped_nodes(db, scope).await?;
    for node in nodes {
        if let Some(value) = node.get("properties").and_then(|p| p.get(key)) {
            descriptor
                .validate(value)
                .map_err(|e| anyhow::anyhow!("existing node {}: {e}", node["node_id"]))?;
        }
    }
    let mut response = db.query("UPDATE field_def SET enforced_schema = $descriptor, schema_revisions = array::append(schema_revisions ?? [], $descriptor) WHERE field_def_id = $id RETURN AFTER")
        .bind(("descriptor", value.clone())).bind(("id", field_def_id.to_string()))
        .await?.check()?;
    let rows: Vec<serde_json::Value> = response.take(0)?;
    rows.into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("field definition disappeared"))
}

async fn scoped_nodes(db: &Db, scope: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    let parts = crate::parse_scope(scope)?;
    let (sql, id) = if let Some(petal) = parts.petal_id {
        ("SELECT node_id, properties FROM node WHERE petal_id = $scope_id AND tombstone = NONE LIMIT 10001", petal)
    } else if let Some(fractal) = parts.fractal_id {
        ("SELECT node_id, properties FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id = $scope_id) AND tombstone = NONE LIMIT 10001", fractal)
    } else {
        ("SELECT node_id, properties FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id = $scope_id)) AND tombstone = NONE LIMIT 10001", parts.verse_id)
    };
    let mut response = db.query(sql).bind(("scope_id", id)).await?.check()?;
    let rows: Vec<serde_json::Value> = response.take(0)?;
    if rows.len() > 10_000 {
        anyhow::bail!(
            "schema evolution exceeds 10000-node dry-run limit; an explicit migration is required"
        );
    }
    Ok(rows)
}
