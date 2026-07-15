//! Handlers for role/invite/access results driving the EntitySettings dialog.
//! See ../AGENTS.md §db-results.

use crate::actions::UiManager;
use crate::dialogs::ActiveDialog;

/// `VerseInviteGenerated`: open the invite dialog with the fresh invite string.
pub(super) fn handle_verse_invite_generated(invite_string: &str, ui_mgr: &mut UiManager) {
    ui_mgr.open_dialog(ActiveDialog::InviteDialog {
        invite_string: invite_string.to_string(),
        include_write_cap: false,
        expiry_hours: 24,
    });
}

/// `PeerRolesResolved`: populate the EntitySettings peer-role list.
pub(super) fn handle_peer_roles_resolved(
    scope: &str,
    roles: &[(String, String)],
    ui_mgr: &mut UiManager,
) {
    if let ActiveDialog::EntitySettings {
        ref mut peer_roles,
        ref mut roles_loading,
        ..
    } = ui_mgr.active_dialog
    {
        *peer_roles = roles
            .iter()
            .map(|(did, role)| crate::dialogs::PeerRoleEntry {
                peer_did: did.clone(),
                display_name: String::new(),
                role: role.clone(),
                is_online: false,
            })
            .collect();
        *roles_loading = false;
    }
    bevy::log::debug!("Resolved {} peer roles at scope {}", roles.len(), scope);
}

/// `RoleAssigned`: reflect the new role in the open EntitySettings dialog.
pub(super) fn handle_role_assigned(
    peer_did: &str,
    scope: &str,
    role: &str,
    ui_mgr: &mut UiManager,
) {
    if let ActiveDialog::EntitySettings {
        ref mut peer_roles, ..
    } = ui_mgr.active_dialog
    {
        if let Some(entry) = peer_roles.iter_mut().find(|p| p.peer_did == peer_did) {
            entry.role = role.to_string();
        }
    }
    bevy::log::info!(
        "Assigned role '{}' to {} at scope {}",
        role,
        peer_did,
        scope
    );
}

/// `RoleRevoked`: mark the peer's role as none in the open EntitySettings dialog.
pub(super) fn handle_role_revoked(peer_did: &str, scope: &str, ui_mgr: &mut UiManager) {
    if let ActiveDialog::EntitySettings {
        ref mut peer_roles, ..
    } = ui_mgr.active_dialog
    {
        if let Some(entry) = peer_roles.iter_mut().find(|p| p.peer_did == peer_did) {
            entry.role = "none".to_string();
        }
    }
    bevy::log::info!("Revoked role for {} at scope {}", peer_did, scope);
}

/// `ScopedInviteGenerated`: surface the invite link in the open EntitySettings dialog.
pub(super) fn handle_scoped_invite_generated(invite_link: &str, ui_mgr: &mut UiManager) {
    if let ActiveDialog::EntitySettings {
        ref mut generated_invite_link,
        ..
    } = ui_mgr.active_dialog
    {
        *generated_invite_link = Some(invite_link.to_string());
    }
    bevy::log::info!("Generated scoped invite link");
}

/// `LocalRoleResolved`: cache the local user's resolved role level.
pub(super) fn handle_local_role_resolved(
    scope: &str,
    role: &str,
    local_role: &mut crate::plugin::LocalUserRole,
) {
    let level = fe_database::RoleLevel::from(role);
    bevy::log::info!("Local role resolved at {}: {} ({:?})", scope, role, level);
    local_role.role = Some(level);
}

/// `VerseDefaultAccessSet`: log-only acknowledgement.
pub(super) fn handle_verse_default_access_set(verse_id: &str, default_access: &str) {
    bevy::log::info!(
        "Set default access for verse {} to '{}'",
        verse_id,
        default_access
    );
}

/// `FractalDescriptionUpdated`: log-only acknowledgement.
pub(super) fn handle_fractal_description_updated(fractal_id: &str, description: &str) {
    bevy::log::info!(
        "Updated description for fractal {}: '{}'",
        fractal_id,
        description
    );
}
