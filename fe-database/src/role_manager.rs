use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use fe_policy::GrantedRole;

use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::role_level::RoleLevel;
use crate::scope::{parent_scope, parse_scope};
use crate::types::{NodeId, OpLogEntry, OpType};
use crate::{rbac, role_grants, BlobStoreHandle, ReplicationSender};

/// Verse-level metadata needed for role resolution.
struct VerseInfo {
    owner_did: String,
    default_access: String,
}

/// Fetch the verse owner DID and default_access from the database.
async fn get_verse_owner_and_default(db: &Db, verse_id: &str) -> anyhow::Result<VerseInfo> {
    let q = QueryBuilder::new()
        .select(&["created_by", "default_access"])
        .from("verse")
        .filter(Filter::eq("verse_id", verse_id))
        .build();
    let mut result = exec_query(db, &q).await?;
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

/// Resolve Owner, nearest explicit role, then default from captured inputs.
pub(crate) fn resolve_role_snapshot<'a>(
    peer_did: &str,
    owner_did: &str,
    default_access: &str,
    roles: impl IntoIterator<Item = &'a str>,
) -> RoleLevel {
    if peer_did == owner_did {
        return RoleLevel::Owner;
    }
    roles.into_iter()
        .find(|role| *role != "public" && !role.is_empty())
        .map(RoleLevel::from)
        .unwrap_or_else(|| RoleLevel::from(default_access))
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
    let mut roles = Vec::new();
    while let Some(s) = current_scope {
        let role = rbac::get_role(db, peer_did, &s).await?;
        let explicit = role.0 != "public" && !role.0.is_empty();
        roles.push(role.0);
        // "public" is the sentinel for "no explicit role assigned".
        if explicit {
            break;
        }
        current_scope = parent_scope(&s);
    }

    // 3. Fall back to verse default_access.
    Ok(resolve_role_snapshot(peer_did, &verse.owner_did, &verse.default_access,
        roles.iter().map(String::as_str)))
}

/// A local role command refused before anything was written (plan §2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleCommandError {
    /// Not one of `manager`/`editor`/`viewer`/`none`.
    InvalidRole(String),
    /// Nobody can grant Owner (T3).
    OwnerNotAssignable,
    /// The owner is never a grant or revoke target (T5).
    TargetIsOwner,
    /// Managers only raise; a Manager retracts their own grant with a revoke (D-RP-2).
    ManagerCannotAssignNone,
    /// The caller lacks Manager+ at the target scope (subtree authority, T19).
    InsufficientAuthority { caller_role: RoleLevel },
}

impl std::fmt::Display for RoleCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleCommandError::InvalidRole(role) => {
                write!(f, "invalid role '{role}': expected manager, editor, viewer or none")
            }
            RoleCommandError::OwnerNotAssignable => write!(f, "the Owner role cannot be assigned"),
            RoleCommandError::TargetIsOwner => {
                write!(f, "the verse owner cannot be a role-command target")
            }
            RoleCommandError::ManagerCannotAssignNone => write!(
                f,
                "a Manager cannot assign 'none' (Managers only raise; revoke your own grant instead)"
            ),
            RoleCommandError::InsufficientAuthority { caller_role } => write!(
                f,
                "insufficient privilege: caller has {caller_role:?}, needs Manager or Owner"
            ),
        }
    }
}

impl std::error::Error for RoleCommandError {}

/// What a revoke left in place: the target's resolved role before and after (plan §2.3 —
/// a Manager's revoke retracts only their own grant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevokeReport {
    pub role_before: RoleLevel,
    pub role_after: RoleLevel,
}

impl RevokeReport {
    /// True when the target still holds the role it held (another grantor or inheritance).
    pub fn still_held(&self) -> bool {
        self.role_after >= self.role_before
    }
}

/// Shared §2.3 preconditions: the scope chain exists, the target is not the owner and the
/// caller holds Manager+ at the scope. Returns `(verse_id, owner_did)`.
async fn check_role_command(
    db: &Db,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
    operation: &str,
) -> anyhow::Result<(String, String)> {
    crate::handlers::preconditions::require_scope_exists(db, scope, operation).await?;
    let verse_id = parse_scope(scope)?.verse_id;
    let owner = role_grants::verse_owner(db, &verse_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{operation}: verse {verse_id} has no owner"))?;
    if peer_did == owner {
        return Err(RoleCommandError::TargetIsOwner.into());
    }
    let caller_role = resolve_role(db, caller_did, scope).await?;
    if !rbac::evaluate_manage(caller_did, caller_role, scope).is_allow() {
        return Err(RoleCommandError::InsufficientAuthority { caller_role }.into());
    }
    Ok((verse_id, owner))
}

/// Issue, store, materialize and publish one local statement (op log first, §log-first-commit).
#[allow(clippy::too_many_arguments)]
async fn issue_local_statement(
    db: &Db,
    verse_id: &str,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
    role: GrantedRole,
    revoked: bool,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<()> {
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: NodeId(caller_did.to_string()),
        op_type: OpType::AssignRole,
        payload: serde_json::json!({
            "peer_did": peer_did,
            "scope": scope,
            "role": role.as_str(),
            "grantor_did": caller_did,
            "revoked": revoked,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    let (_, owner) = check_role_command(db,caller_did,peer_did,scope,"Role statement").await?;
    if role == GrantedRole::None && !revoked && caller_did != owner {
        return Err(RoleCommandError::ManagerCannotAssignNone.into());
    }
    let authority=crate::handlers::entity::prepare_scope_permission(db,scope,caller_did,fe_policy::RoleLevel::Manager).await?;
    let prior = role_grants::load_effective_roles(db, verse_id).await?;
    let stmt=role_grants::commit_local_statement(db,entry,verse_id,caller_did,peer_did,scope,role,revoked,authority,&owner).await?;
    // A crash before this line is repaired by the startup repartition and re-emit (AC-42).
    role_grants::repartition(db, verse_id, prior.as_ref()).await?;
    role_grants::publish_statement(repl_tx, blob_store, verse_id, &stmt);
    Ok(())
}

/// Assign a role as a local statement (plan §2.3): the caller needs Manager+ at the target
/// scope (subtree authority over the materialized cache), `owner` is never assignable, the
/// owner is never a target, and only the owner may assign `none` (it pins the cell).
pub async fn assign_role_checked(
    db: &Db,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
    role: &str,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<()> {
    // Cheap, I/O-free refusals first; every refusal precedes the op-log append.
    let granted = match role {
        "owner" => return Err(RoleCommandError::OwnerNotAssignable.into()),
        other => GrantedRole::parse(other)
            .ok_or_else(|| RoleCommandError::InvalidRole(other.to_string()))?,
    };
    let (verse_id, owner) =
        check_role_command(db, caller_did, peer_did, scope, "AssignRole").await?;
    if granted == GrantedRole::None && caller_did != owner {
        return Err(RoleCommandError::ManagerCannotAssignNone.into());
    }
    issue_local_statement(
        db, &verse_id, caller_did, peer_did, scope, granted, false, repl_tx, blob_store,
    )
    .await
}

/// Revoke as a local statement: the caller's own `revoked` statement for the key, which
/// retracts only the caller's own grant (an owner revoke blocks the cell). The report says
/// whether the target still holds its role through another grantor or by inheritance.
pub async fn revoke_role_checked(
    db: &Db,
    caller_did: &str,
    peer_did: &str,
    scope: &str,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<RevokeReport> {
    let (verse_id, _owner) =
        check_role_command(db, caller_did, peer_did, scope, "RevokeRole").await?;
    let role_before = resolve_role(db, peer_did, scope).await?;
    issue_local_statement(
        db,
        &verse_id,
        caller_did,
        peer_did,
        scope,
        GrantedRole::None,
        true,
        repl_tx,
        blob_store,
    )
    .await?;
    let role_after = resolve_role(db, peer_did, scope).await?;
    Ok(RevokeReport {
        role_before,
        role_after,
    })
}

/// Set the verse-level default access (`"viewer"` or `"none"`); the caller needs Manager+
/// at the VERSE scope — a petal or fractal Manager is refused (governance lies above them).
pub async fn set_default_access(
    db: &Db,
    caller_did: &str,
    verse_id: &str,
    default: &str,
) -> anyhow::Result<()> {
    if default != "viewer" && default != "none" {
        anyhow::bail!(
            "default_access must be 'viewer' or 'none', got '{}'",
            default
        );
    }
    rbac::require_manage_role(db, caller_did, &crate::build_scope(verse_id, None, None)).await?;
    let q = UpdateBuilder::update("verse")
        .set("default_access", default)
        .where_clause(Filter::eq("verse_id", verse_id))
        .build();
    exec_query(db, &q).await?;
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

    #[tokio::test]
    async fn resolve_role_uses_verse_default_access() {
        // Set up in-memory SurrealDB
        let db: surrealdb::Surreal<surrealdb::engine::local::Db> =
            surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
                .await
                .expect("SurrealDB Mem init");
        db.use_ns("test").use_db("test").await.expect("ns/db");

        // Apply schema
        crate::rbac::apply_schema(&db).await.expect("schema");

        // Create a verse with default_access = "none"
        let verse_id = "test-verse-1";
        let owner_did = "did:key:zOwnerXYZ";
        let now = chrono::Utc::now().to_rfc3339();
        let _: Option<serde_json::Value> = db
            .create("verse")
            .content(serde_json::json!({
                "verse_id": verse_id,
                "name": "Test Verse",
                "created_by": owner_did,
                "created_at": now,
                "default_access": "none",
            }))
            .await
            .expect("create verse");

        let scope = crate::scope::build_scope(verse_id, None, None);

        // A random peer with no explicit role should get None
        let random_peer = "did:key:zRandomPeer123";
        let role = super::resolve_role(&db, random_peer, &scope)
            .await
            .expect("resolve_role");
        assert_eq!(
            role,
            RoleLevel::None,
            "peer with no role on default_access=none verse should get None"
        );

        // The owner should still get Owner regardless of default_access
        let owner_role = super::resolve_role(&db, owner_did, &scope)
            .await
            .expect("resolve_role for owner");
        assert_eq!(
            owner_role,
            RoleLevel::Owner,
            "verse owner should always resolve to Owner"
        );

        // Now change default_access to "viewer"
        super::set_default_access(&db, owner_did, verse_id, "viewer")
            .await
            .expect("set_default_access");

        // The random peer should now get Viewer
        let role2 = super::resolve_role(&db, random_peer, &scope)
            .await
            .expect("resolve_role after change");
        assert_eq!(
            role2,
            RoleLevel::Viewer,
            "peer with no role on default_access=viewer verse should get Viewer"
        );

        let captured=crate::handlers::entity::prepare_scope_permission(&db,&scope,owner_did,fe_policy::RoleLevel::Manager).await.unwrap();
        let before:Vec<serde_json::Value>=db.query("SELECT * FROM op_log").await.unwrap().take(0).unwrap();
        db.query("UPDATE verse SET created_by='did:key:new-owner' WHERE verse_id=$v").bind(("v",verse_id.to_owned())).await.unwrap().check().unwrap();
        let intention=crate::types::OpLogEntry {lamport_clock:0,node_id:crate::types::NodeId(owner_did.into()),op_type:crate::types::OpType::AssignRole,payload:serde_json::json!({"scope":scope,"peer_did":random_peer,"grantor_did":owner_did,"role":"editor","revoked":false}),sig:"00".repeat(64),hlc_timestamp:String::new()};
        assert!(crate::role_grants::commit_local_statement(&db,intention,verse_id,owner_did,random_peer,&scope,fe_policy::GrantedRole::Editor,false,captured,owner_did).await.is_err(),"Owner change between preparation and use rolls back statement and operation");
        let after:Vec<serde_json::Value>=db.query("SELECT * FROM op_log").await.unwrap().take(0).unwrap();assert_eq!(before,after);
        let grants:Vec<serde_json::Value>=db.query("SELECT * FROM role_grant").await.unwrap().take(0).unwrap();assert!(grants.is_empty());

    }
}
