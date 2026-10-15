use tracing::instrument;

use crate::repo::Db;
use crate::role_manager::RevokeReport;
use crate::{BlobStoreHandle, ReplicationSender, RoleLevel};

/// Resolve effective roles for a list of peers at a scope.
#[instrument(skip(db))]
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

/// Assign a role as a local statement: stored, materialized and published (src/AGENTS.md
/// §role-propagation; refusals are typed `role_manager::RoleCommandError`s).
#[instrument(skip(db, blob_store, repl_tx))]
pub async fn assign_role_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    local_did: &str,
    peer_did: &str,
    scope: &str,
    role: &str,
) -> anyhow::Result<()> {
    crate::role_manager::assign_role_checked(
        db, local_did, peer_did, scope, role, repl_tx, blob_store,
    )
    .await
}

/// Revoke the local node's own grant (an owner revoke blocks the cell); see
/// `role_manager::revoke_role_checked`.
#[instrument(skip(db, blob_store, repl_tx))]
pub async fn revoke_role_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    local_did: &str,
    peer_did: &str,
    scope: &str,
) -> anyhow::Result<RevokeReport> {
    crate::role_manager::revoke_role_checked(db, local_did, peer_did, scope, repl_tx, blob_store)
        .await
}

/// Resolve the local user's effective role at a scope.
#[instrument(skip(db))]
pub(crate) async fn resolve_local_role_handler(
    db: &Db,
    local_did: &str,
    scope: &str,
) -> anyhow::Result<String> {
    let level = crate::role_manager::resolve_role(db, local_did, scope).await?;
    Ok(level.to_string())
}

/// Generate a scoped invite link with a specific role and expiry.
#[instrument(skip(keypair))]
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
        keypair,
        local_did,
        scope,
        role_level,
        expiry_secs,
    )?;
    Ok(invite.to_invite_link())
}
