//! Handlers for role/invite/access results driving the EntitySettings dialog.
//! See ../AGENTS.md §db-results.

use crate::actions::UiManager;
use crate::dialogs::ActiveDialog;
use crate::navigation_manager::NavigationManager;

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

/// `OrphanedVerseClaimed` (F6): this node now owns `verse_id`. If it's the
/// active verse, flip the cached role badge straight to Owner — no need to
/// round-trip a fresh `LocalRoleResolved` fetch, since `resolve_role`'s
/// owner check is a live `created_by` comparison, never a cache (the claim
/// having landed on the DB side already tells us what the next resolve
/// would say). See `fe-database/src/AGENTS.md` §role-propagation.
pub(super) fn handle_orphaned_verse_claimed(
    verse_id: &str,
    old_did: &str,
    new_did: &str,
    nav: &NavigationManager,
    local_role: &mut crate::plugin::LocalUserRole,
    ui_mgr: &mut UiManager,
    now_secs: f64,
) {
    if nav.active_verse_id.as_deref() == Some(verse_id) {
        local_role.role = Some(fe_database::RoleLevel::Owner);
    }
    bevy::log::info!("Claimed orphaned verse {verse_id}: {old_did} -> {new_did}");
    ui_mgr.show_toast("Verse ownership claimed — you are now Owner.", now_secs);
}

/// `OrphanedVerseClaimRefused`: surface the typed reason as a toast so a
/// refused claim is visible, never a silent no-op.
pub(super) fn handle_orphaned_verse_claim_refused(
    verse_id: &str,
    reason: &str,
    ui_mgr: &mut UiManager,
    now_secs: f64,
) {
    bevy::log::warn!("ClaimOrphanedVerse refused for {verse_id}: {reason}");
    ui_mgr.show_toast(format!("Could not claim verse: {reason}"), now_secs);
}

/// `VerseDefaultAccessSet`: log-only acknowledgement.
pub(super) fn handle_verse_default_access_set(verse_id: &str, default_access: &str) {
    bevy::log::info!(
        "Set default access for verse {} to '{}'",
        verse_id,
        default_access
    );
}

/// `VerseTimeseriesSettingsSet` (M2/F6): fold the authoritative persisted
/// timeseries settings back into the settings surface's per-verse map. The
/// handler sanitizes, so the echoed values are the persisted truth — never an
/// optimistic UI guess (A13).
pub(super) fn handle_verse_timeseries_settings_set(
    verse_id: &str,
    mode: &str,
    replication_factor: u32,
    bucket_width_ms: u64,
    state: &mut crate::timeseries_settings::TimeseriesSettingsState,
) {
    // Re-parse through the canonical parser; an unparseable mode falls back to
    // mirror exactly as the verse-row parse does (defensive — the handler
    // already validated).
    let parsed = fe_runtime::timeseries::VerseTimeseriesSettings::sanitized(
        mode,
        replication_factor,
        bucket_width_ms,
    )
    .unwrap_or_default();
    state.set(verse_id, parsed);
    bevy::log::info!(
        "Set timeseries settings for verse {}: mode={} R={} bucket={}ms",
        verse_id,
        parsed.mode.as_str(),
        parsed.replication_factor,
        parsed.bucket_width_ms
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
