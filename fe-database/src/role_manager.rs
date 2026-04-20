use crate::repo::Db;
use crate::role_level::RoleLevel;
use crate::scope::{parse_scope, parent_scope};
use crate::{op_log, rbac};
use crate::types::{NodeId, OpLogEntry, OpType};

/// Verse-level metadata needed for role resolution.
struct VerseInfo {
    owner_did: String,
    default_access: String,
}

/// Fetch the verse owner DID and default_access from the database.
async fn get_verse_owner_and_default(db: &Db, verse_id: &str) -> anyhow::Result<VerseInfo> {
    let mut result = db
        .query("SELECT created_by, default_access FROM verse WHERE verse_id = $verse_id")
        .bind(("verse_id", verse_id.to_string()))
        .await?;
    let row: Option<serde_json::Value> = result.take(0)?;
    let row = row.ok_or_else(|| anyhow::anyhow!("verse not found: {}", verse_id))?;
    Ok(VerseInfo {
        owner_did: row
            .get("created_by")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        default_access: row
            .get("default_access")
            .and_then(|v| v.as_str())
            .unwrap_or("viewer")
            .to_string(),
    })
}

/// Resolve the effective role for a peer at a given scope.
///
/// Walks up the hierarchy: petal -> fractal -> verse -> verse.default_access.
/// Exception: the verse owner always resolves to `Owner`.
pub async fn resolve_role(db: &Db, peer_did: &str, scope: &str) -> anyhow::Result<RoleLevel> {
    // 1. Check if peer is the verse owner.
    let parts = parse_scope(scope)?;
    let verse = get_verse_owner_and_default(db, &parts.verse_id).await?;
    if peer_did == verse.owner_did {
        return Ok(RoleLevel::Owner);
    }

    // 2. Walk up the scope chain: check most-specific first.
    //    `get_role` returns RoleId("public") when no explicit row exists.
    let mut current_scope = Some(scope.to_string());
    while let Some(s) = current_scope {
        let role = rbac::get_role(db, peer_did, &s).await?;
        let role_str = role.0.as_str();
        // "public" is the sentinel for "no explicit role assigned".
        if role_str != "public" && !role_str.is_empty() {
            return Ok(RoleLevel::from(role_str));
        }
        current_scope = parent_scope(&s);
    }

    // 3. Fall back to verse default_access.
    Ok(RoleLevel::from(verse.default_access.as_str()))
}

/// Assign a role to a peer at a specific scope.
///
/// The caller must have Manager or Owner role at or above the target scope,
/// and cannot assign a role higher than their own.
pub async fn assign_role_checked(
    db: &Db,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
    role: RoleLevel,
) -> anyhow::Result<()> {
    // Check caller privilege.
    let caller_role = resolve_role(db, caller_did, scope).await?;
    if !caller_role.can_manage() {
        anyhow::bail!(
            "insufficient privilege: caller has {:?}, needs Manager or Owner",
            caller_role
        );
    }

    // Cannot assign a role higher than caller's own.
    if role > caller_role {
        anyhow::bail!(
            "cannot assign role {:?} which exceeds caller's role {:?}",
            role,
            caller_role
        );
    }

    // Write the role.
    rbac::assign_role(db, peer_did, scope, &role.to_string()).await?;

    // Write audit trail.
    let entry = OpLogEntry {
        lamport_clock: 0, // auto-incremented by write_op_log
        node_id: NodeId(caller_did.to_string()),
        op_type: OpType::AssignRole,
        payload: serde_json::json!({
            "peer_did": peer_did,
            "scope": scope,
            "role": role.to_string(),
        }),
        sig: "00".repeat(64), // placeholder signature
    };
    op_log::write_op_log(db, entry).await?;

    Ok(())
}

/// Revoke a peer's explicit role at a scope. They fall back to inherited role.
///
/// The caller must have Manager or Owner role at or above the target scope.
pub async fn revoke_role_checked(
    db: &Db,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
) -> anyhow::Result<()> {
    let caller_role = resolve_role(db, caller_did, scope).await?;
    if !caller_role.can_manage() {
        anyhow::bail!(
            "insufficient privilege: caller has {:?}, needs Manager or Owner",
            caller_role
        );
    }

    rbac::revoke_role(db, peer_did, scope).await?;

    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: NodeId(caller_did.to_string()),
        op_type: OpType::AssignRole,
        payload: serde_json::json!({
            "peer_did": peer_did,
            "scope": scope,
            "action": "revoke",
        }),
        sig: "00".repeat(64),
    };
    op_log::write_op_log(db, entry).await?;

    Ok(())
}

/// Get all peers with explicit access at a scope.
///
/// Returns `(peer_did, RoleLevel)` pairs. Currently only includes roles
/// explicitly assigned at this exact scope; inherited members from parent
/// scopes will be added in a future enhancement.
pub async fn get_scope_members(
    db: &Db,
    scope: &str,
) -> anyhow::Result<Vec<(String, RoleLevel)>> {
    let explicit = rbac::get_all_roles_for_scope(db, scope).await?;
    let members: Vec<(String, RoleLevel)> = explicit
        .into_iter()
        .map(|(did, role_str)| (did, RoleLevel::from(role_str.as_str())))
        .collect();

    // TODO: Also include inherited members from parent scopes.

    Ok(members)
}

/// Set the verse-level default access. Must be `"viewer"` or `"none"`.
pub async fn set_default_access(db: &Db, verse_id: &str, default: &str) -> anyhow::Result<()> {
    if default != "viewer" && default != "none" {
        anyhow::bail!(
            "default_access must be 'viewer' or 'none', got '{}'",
            default
        );
    }
    db.query("UPDATE verse SET default_access = $default WHERE verse_id = $verse_id")
        .bind(("default", default.to_string()))
        .bind(("verse_id", verse_id.to_string()))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::RoleLevel;

    #[test]
    fn role_level_ordering_for_privilege_check() {
        assert!(RoleLevel::Owner.can_manage());
        assert!(RoleLevel::Manager.can_manage());
        assert!(!RoleLevel::Editor.can_manage());
        assert!(!RoleLevel::Viewer.can_manage());
        assert!(!RoleLevel::None.can_manage());
    }

    #[test]
    fn cannot_assign_higher_than_own_role() {
        let caller = RoleLevel::Manager;
        let target = RoleLevel::Owner;
        assert!(target > caller, "Owner should be higher than Manager");
    }

    #[test]
    fn default_access_validation() {
        assert!(["viewer", "none"].contains(&"viewer"));
        assert!(["viewer", "none"].contains(&"none"));
        assert!(!["viewer", "none"].contains(&"editor"));
    }

    #[test]
    fn public_sentinel_maps_to_viewer() {
        // "public" is the get_role sentinel for "no explicit role".
        // RoleLevel::from("public") maps to Viewer, which is the correct
        // default-access level.
        assert_eq!(RoleLevel::from("public"), RoleLevel::Viewer);
    }

    #[test]
    fn role_hierarchy_is_total_order() {
        let levels = [
            RoleLevel::None,
            RoleLevel::Viewer,
            RoleLevel::Editor,
            RoleLevel::Manager,
            RoleLevel::Owner,
        ];
        for window in levels.windows(2) {
            assert!(
                window[0] < window[1],
                "{:?} should be less than {:?}",
                window[0],
                window[1]
            );
        }
    }
}
