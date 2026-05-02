use crate::repo::Db;
use crate::schema::Role;
use crate::types::RoleId;

pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    crate::schema::apply_all(db).await
}

pub async fn get_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<RoleId> {
    let peer_did = peer_did.to_string();
    let scope = scope.to_string();
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT role FROM role WHERE peer_did = $peer_did AND scope = $scope")
        .bind(("peer_did", peer_did))
        .bind(("scope", scope))
        .await?;
    // Deserialize as typed Role rows; compound-key lookup requires raw SurrealQL
    // because Repo::find_where only supports a single equality predicate.
    let raw: Vec<serde_json::Value> = result.take(0)?;
    let roles: Vec<Role> = raw
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()?;
    Ok(roles
        .into_iter()
        .next()
        .map(|r| RoleId(r.role))
        .unwrap_or_else(|| RoleId("public".to_string())))
}

/// Roles that are permitted to perform write/mutation operations on a petal.
const WRITE_ROLES: &[&str] = &["owner", "manager", "editor"];

/// Check whether `peer_did` has one of the required roles at `scope`.
/// Returns Ok(role) on success, Err on permission denied or DB error.
pub async fn require_write_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<RoleId> {
    let role = get_role(db, peer_did, scope).await?;
    if WRITE_ROLES.contains(&role.0.as_str()) {
        Ok(role)
    } else {
        anyhow::bail!(
            "permission denied: peer '{peer_did}' has role '{}' at scope '{scope}' \
             — write requires one of {WRITE_ROLES:?}",
            role.0
        )
    }
}

/// Get all peer roles for a given scope.
pub async fn get_all_roles_for_scope(db: &Db, scope: &str) -> anyhow::Result<Vec<(String, String)>> {
    let scope = scope.to_string();
    let mut result = db
        .query("SELECT peer_did, role FROM role WHERE scope = $scope")
        .bind(("scope", scope))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let mut roles = Vec::new();
    for row in rows {
        if let (Some(peer_did), Some(role)) = (
            row.get("peer_did").and_then(|v| v.as_str()),
            row.get("role").and_then(|v| v.as_str()),
        ) {
            roles.push((peer_did.to_string(), role.to_string()));
        }
    }
    Ok(roles)
}

/// Assign a role to a peer at a specific scope.
pub async fn assign_role(db: &Db, peer_did: &str, scope: &str, role: &str) -> anyhow::Result<()> {
    let peer_did = peer_did.to_string();
    let scope = scope.to_string();
    let role = role.to_string();
    db.query("DELETE FROM role WHERE peer_did = $peer_did AND scope = $scope")
        .bind(("peer_did", peer_did.clone()))
        .bind(("scope", scope.clone()))
        .await?;
    db.query("CREATE role SET peer_did = $peer_did, scope = $scope, role = $role")
        .bind(("peer_did", peer_did))
        .bind(("scope", scope))
        .bind(("role", role))
        .await?;
    Ok(())
}

/// Remove a peer's role at a specific scope (they fall back to inherited role).
pub async fn revoke_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<()> {
    let peer_did = peer_did.to_string();
    let scope = scope.to_string();
    db.query("DELETE FROM role WHERE peer_did = $peer_did AND scope = $scope")
        .bind(("peer_did", peer_did))
        .bind(("scope", scope))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn write_roles_contains_owner_manager_and_editor() {
        assert!(WRITE_ROLES.contains(&"owner"));
        assert!(WRITE_ROLES.contains(&"manager"));
        assert!(WRITE_ROLES.contains(&"editor"));
        assert!(!WRITE_ROLES.contains(&"public"));
    }
}
