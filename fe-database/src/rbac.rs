//! DB-layer RBAC entry points — role checks delegate to fe-policy (see src/AGENTS.md §rbac-policy).

use std::sync::{Arc, OnceLock};

use fe_policy::{Action, AuthContext, Decision, PolicyEngine, RoleLevel, RoleLevelPolicy, Scope};
use fe_query::{DeleteBuilder, Filter, InsertBuilder, QueryBuilder};

use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::schema::Role;
use crate::types::RoleId;

pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    crate::schema::apply_all(db).await
}

pub async fn get_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<RoleId> {
    let q = QueryBuilder::new()
        .select(&["role"])
        .from("role")
        .filter(Filter::eq("peer_did", peer_did))
        .filter(Filter::eq("scope", scope))
        .build();
    let mut result = exec_query(db, &q).await?;
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

/// Shared write-path policy engine (standard verb map: Write requires Editor+).
fn write_engine() -> &'static PolicyEngine {
    static ENGINE: OnceLock<PolicyEngine> = OnceLock::new();
    ENGINE.get_or_init(|| PolicyEngine::new().with_policy(Arc::new(RoleLevelPolicy::standard())))
}

/// Pure write decision for a pre-fetched role string — unit-testable without I/O.
pub(crate) fn evaluate_write(peer_did: &str, role: &str, scope: &str) -> Decision {
    let subject = AuthContext::Did {
        did: peer_did.to_string(),
        role: RoleLevel::from(role),
    };
    write_engine().evaluate(&subject, &Action::Write, &Scope::new(scope))
}

/// Check whether `peer_did` may write at `scope` (fe-policy engine decision).
/// Returns Ok(role) on success, Err on permission denied or DB error.
pub async fn require_write_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<RoleId> {
    let role = get_role(db, peer_did, scope).await?;
    match evaluate_write(peer_did, &role.0, scope) {
        Decision::Allow => Ok(role),
        Decision::Deny(reason) => anyhow::bail!("permission denied: {reason}"),
    }
}

/// Get all peer roles for a given scope.
pub async fn get_all_roles_for_scope(
    db: &Db,
    scope: &str,
) -> anyhow::Result<Vec<(String, String)>> {
    let q = QueryBuilder::new()
        .select(&["peer_did", "role"])
        .from("role")
        .filter(Filter::eq("scope", scope))
        .build();
    let mut result = exec_query(db, &q).await?;
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
    // Remove any existing role first
    let del_q = DeleteBuilder::delete_from("role")
        .where_clause(Filter::eq("peer_did", peer_did))
        .where_clause(Filter::eq("scope", scope))
        .build();
    exec_query(db, &del_q).await?;

    // Insert the new role
    let ins_q = InsertBuilder::insert_into("role")
        .values(serde_json::json!({
            "peer_did": peer_did,
            "scope": scope,
            "role": role,
        }))
        .build();
    exec_query(db, &ins_q).await?;
    Ok(())
}

/// Remove a peer's role at a specific scope (they fall back to inherited role).
pub async fn revoke_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<()> {
    let q = DeleteBuilder::delete_from("role")
        .where_clause(Filter::eq("peer_did", peer_did))
        .where_clause(Filter::eq("scope", scope))
        .build();
    exec_query(db, &q).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_allowed_for_editor_and_above() {
        for role in ["owner", "manager", "editor"] {
            assert!(
                evaluate_write("did:key:z6MkTest", role, "petal-1").is_allow(),
                "role '{role}' must be allowed to write"
            );
        }
    }

    #[test]
    fn write_denied_below_editor() {
        for role in ["viewer", "public", "none", "unknown-role", ""] {
            assert!(
                !evaluate_write("did:key:z6MkTest", role, "petal-1").is_allow(),
                "role '{role}' must be denied write"
            );
        }
    }

    #[test]
    fn deny_reason_names_subject_and_scope() {
        let decision = evaluate_write("did:key:z6MkTest", "viewer", "petal-1");
        let reason = decision.reason().unwrap_or_default();
        assert!(reason.contains("did:key:z6MkTest"));
        assert!(reason.contains("petal-1"));
    }
}
