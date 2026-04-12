use crate::repo::Db;
use crate::schema::Role;
use crate::types::RoleId;

pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    crate::schema::apply_all(db).await
}

pub async fn get_role(db: &Db, node_id: &str, petal_id: &str) -> anyhow::Result<RoleId> {
    let node_id = node_id.to_string();
    let petal_id = petal_id.to_string();
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT role FROM role WHERE node_id = $node_id AND petal_id = $petal_id")
        .bind(("node_id", node_id))
        .bind(("petal_id", petal_id))
        .await?;
    // Deserialize as typed Role rows; compound-key lookup requires raw SurrealQL
    // because Repo::find_where only supports a single equality predicate.
    let raw: Vec<serde_json::Value> = result.take(0)?;
    let roles: Vec<Role> = raw
        .into_iter()
        .map(|v| serde_json::from_value(v))
        .collect::<Result<_, _>>()?;
    Ok(roles
        .into_iter()
        .next()
        .map(|r| RoleId(r.role))
        .unwrap_or_else(|| RoleId("public".to_string())))
}

/// Roles that are permitted to perform write/mutation operations on a petal.
const WRITE_ROLES: &[&str] = &["owner", "editor"];

/// Check whether `node_id` has one of the required roles in `petal_id`.
/// Returns Ok(role) on success, Err on permission denied or DB error.
pub async fn require_write_role(db: &Db, node_id: &str, petal_id: &str) -> anyhow::Result<RoleId> {
    let role = get_role(db, node_id, petal_id).await?;
    if WRITE_ROLES.contains(&role.0.as_str()) {
        Ok(role)
    } else {
        anyhow::bail!(
            "permission denied: node '{node_id}' has role '{}' in petal '{petal_id}' \
             — write requires one of {WRITE_ROLES:?}",
            role.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_roles_contains_owner_and_editor() {
        assert!(WRITE_ROLES.contains(&"owner"));
        assert!(WRITE_ROLES.contains(&"editor"));
        assert!(!WRITE_ROLES.contains(&"public"));
    }
}
