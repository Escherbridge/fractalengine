use crate::repo::Db;
use crate::RoleLevel;

/// Resolve effective roles for a list of peers at a scope.
pub(crate) async fn resolve_roles_for_peers_handler(
    db: &Db,
    scope: &str,
    peer_dids: &[String],
) -> Vec<(String, String)> {
    let mut roles = Vec::new();
    for did in peer_dids {
        match crate::role_manager::resolve_role(db, did, scope).await {
            Ok(level) => roles.push((did.clone(), level.to_string())),
            Err(e) => {
                tracing::warn!("resolve_role failed for {did}: {e}");
                roles.push((did.clone(), "none".to_string()));
            }
        }
    }
    roles
}

/// Assign a role to a peer (privilege-checked against the actor's own role).
pub(crate) async fn assign_role_handler(
    db: &Db,
    local_did: &str,
    peer_did: &str,
    scope: &str,
    role: &str,
) -> anyhow::Result<()> {
    let role_level = RoleLevel::from(role);
    crate::role_manager::assign_role_checked(db, local_did, peer_did, scope, role_level).await
}

/// Revoke a peer's explicit role at a scope (privilege-checked).
pub(crate) async fn revoke_role_handler(
    db: &Db,
    local_did: &str,
    peer_did: &str,
    scope: &str,
) -> anyhow::Result<()> {
    crate::role_manager::revoke_role_checked(db, local_did, peer_did, scope).await
}

/// Resolve the local user's effective role at a scope.
pub(crate) async fn resolve_local_role_handler(
    db: &Db,
    local_did: &str,
    scope: &str,
) -> anyhow::Result<String> {
    let level = crate::role_manager::resolve_role(db, local_did, scope).await?;
    Ok(level.to_string())
}

/// Generate a scoped invite link with a specific role and expiry.
pub(crate) async fn generate_scoped_invite_handler(
    keypair: &fe_identity::NodeKeypair,
    local_did: &str,
    scope: &str,
    role: &str,
    expiry_hours: u32,
) -> anyhow::Result<String> {
    let role_level = RoleLevel::from(role);
    let expiry_secs = (expiry_hours as u64) * 3600;
    let invite = crate::scoped_invite::generate_scoped_invite(
        keypair, local_did, scope, role_level, expiry_secs,
    )?;
    Ok(invite.to_invite_link())
}
