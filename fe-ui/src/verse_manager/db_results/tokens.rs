//! Handlers for API-token results (mint/revoke/list) plus the token-list
//! refresh helpers. See ../AGENTS.md §db-results.

use fe_runtime::app::{DbCommandSender, RevocationBroadcastSender};
use fe_runtime::messages::{ApiTokenInfo, DbCommand};

use crate::actions::UiManager;
use crate::dialogs::{ActiveDialog, ApiTokenEntry};
use crate::plugin::InspectorFormState;

/// `ApiTokenMinted`: surface the minted token (and its `client` claim, W6-7b)
/// and refresh the token list.
pub(super) fn handle_api_token_minted(
    token: &str,
    jti: &str,
    scope: &str,
    max_role: &str,
    client: &str,
    ui_mgr: &mut UiManager,
    inspector: &mut InspectorFormState,
    db_sender: &DbCommandSender,
) {
    if let ActiveDialog::EntitySettings {
        ref mut generated_api_token,
        ref mut generated_api_token_client,
        ..
    } = ui_mgr.active_dialog
    {
        *generated_api_token = Some(token.to_string());
        *generated_api_token_client = Some(client.to_string());
    }
    inspector.generated_api_token = Some(token.to_string());
    inspector.generated_api_token_client = Some(client.to_string());
    bevy::log::info!(
        "API token minted: jti={} scope={} role={} client={}",
        jti,
        scope,
        max_role,
        client
    );
    // Refresh the scoped token list at current page
    refresh_inspector_tokens(db_sender, inspector);
}

/// `ApiTokenRevoked`: broadcast the revocation to the API thread and refresh the list.
pub(super) fn handle_api_token_revoked(
    jti: &str,
    revocation_tx: Option<&RevocationBroadcastSender>,
    inspector: &InspectorFormState,
    db_sender: &DbCommandSender,
) {
    bevy::log::info!("API token revoked: jti={}", jti);
    if let Some(tx) = revocation_tx {
        if tx.0.send(jti.to_string()).is_err() {
            bevy::log::error!("revocation_tx channel closed — API thread may have crashed");
        }
    }
    // Refresh the scoped token list at current page
    refresh_inspector_tokens(db_sender, inspector);
}

/// `ApiTokensListed`: populate dialog + inspector token lists.
pub(super) fn handle_api_tokens_listed(
    tokens: &[ApiTokenInfo],
    total: u64,
    ui_mgr: &mut UiManager,
    inspector: &mut InspectorFormState,
) {
    let entries = tokens_to_entries(tokens);
    if let ActiveDialog::EntitySettings {
        ref mut api_tokens,
        ref mut api_tokens_loading,
        ..
    } = ui_mgr.active_dialog
    {
        *api_tokens = entries.clone();
        *api_tokens_loading = false;
    }
    inspector.api_tokens = entries;
    inspector.api_tokens_total = total;
    inspector.api_tokens_loading = false;
}

/// `ScopedApiTokensListed`: populate the admin scoped-token views.
pub(super) fn handle_scoped_api_tokens_listed(
    tokens: &[ApiTokenInfo],
    total: u64,
    ui_mgr: &mut UiManager,
    inspector: &mut InspectorFormState,
) {
    let entries = tokens_to_entries(tokens);
    if let ActiveDialog::EntitySettings {
        ref mut scoped_api_tokens,
        ref mut scoped_tokens_loading,
        ..
    } = ui_mgr.active_dialog
    {
        *scoped_api_tokens = entries.clone();
        *scoped_tokens_loading = false;
    }
    inspector.api_tokens = entries;
    inspector.api_tokens_total = total;
    inspector.api_tokens_loading = false;
}

/// Convert ApiTokenInfo list to UI-displayable ApiTokenEntry list.
pub(super) fn tokens_to_entries(tokens: &[ApiTokenInfo]) -> Vec<ApiTokenEntry> {
    tokens
        .iter()
        .map(|t| ApiTokenEntry {
            jti: t.jti.clone(),
            scope: t.scope.clone(),
            max_role: t.max_role.clone(),
            label: t.label.clone(),
            created_at: t.created_at.clone(),
            expires_at: t.expires_at.clone(),
            revoked: t.revoked,
            sub: t.sub.clone(),
            client: t.client.clone(),
        })
        .collect()
}

/// Send a scoped, paginated token list refresh using the inspector's current scope and page.
fn refresh_inspector_tokens(db_sender: &DbCommandSender, inspector: &InspectorFormState) {
    let offset = inspector.api_tokens_page * crate::plugin::API_TOKEN_PAGE_SIZE;
    let limit = crate::plugin::API_TOKEN_PAGE_SIZE;
    let scope = &inspector.api_token_scope_buf;
    if scope.is_empty() {
        if db_sender
            .0
            .send(DbCommand::ListApiTokens { offset, limit })
            .is_err()
        {
            bevy::log::error!(
                "db_sender channel closed during token list refresh — DB thread may have crashed"
            );
        }
    } else {
        if db_sender
            .0
            .send(DbCommand::ListApiTokensByScope {
                scope_prefix: scope.clone(),
                offset,
                limit,
            })
            .is_err()
        {
            bevy::log::error!("db_sender channel closed during scoped token list refresh — DB thread may have crashed");
        }
    }
}

/// Build `DbCommand::MintApiToken` from a mint surface's form buffers
/// (W6-7b). A blank (post-trim) `client_buf` mints a fresh client (`None`);
/// anything else is sent trimmed. This is the exact, tested shape both GUI
/// mint surfaces (`panels::inspector`, `dialogs::entity_settings`) build
/// inline at their "Generate API Token" send sites — keep those in sync with
/// this function if the shape ever changes. The DB door re-validates
/// `client_did` regardless of what the GUI sends (`client` is never trusted
/// client-side).
pub(crate) fn mint_command(
    scope: String,
    max_role: String,
    ttl_hours: u32,
    label: Option<String>,
    client_buf: &str,
) -> DbCommand {
    let trimmed = client_buf.trim();
    let client_did = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    DbCommand::MintApiToken {
        scope,
        max_role,
        ttl_hours,
        label,
        client_did,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::{ApiTokenEntry, EntitySettingsType, SettingsTab};

    fn sender() -> (DbCommandSender, crossbeam::channel::Receiver<DbCommand>) {
        let (tx, rx) = crossbeam::channel::bounded(8);
        (DbCommandSender(tx), rx)
    }

    fn entity_settings_dialog() -> ActiveDialog {
        ActiveDialog::EntitySettings {
            entity_type: EntitySettingsType::Verse,
            entity_id: "v1".to_string(),
            entity_name: "Verse".to_string(),
            parent_verse_id: "v1".to_string(),
            parent_fractal_id: None,
            active_tab: SettingsTab::ApiAccess,
            name_buf: "Verse".to_string(),
            default_access_buf: Some("viewer".to_string()),
            description_buf: None,
            peer_roles: vec![],
            roles_loading: false,
            invite_role_buf: "viewer".to_string(),
            invite_expiry_buf: 24,
            generated_invite_link: None,
            pending_delete: false,
            pending_claim_ownership: false,
            api_tokens: vec![],
            api_tokens_loading: false,
            api_token_scope_buf: "VERSE#v1".to_string(),
            api_token_role_buf: "viewer".to_string(),
            api_token_expiry_buf: 24,
            api_token_client_buf: String::new(),
            generated_api_token: None,
            generated_api_token_client: None,
            scoped_api_tokens: vec![],
            scoped_tokens_loading: false,
        }
    }

    #[test]
    fn a_blank_client_field_sends_none_and_a_filled_one_sends_it_trimmed() {
        let blank = mint_command(
            "VERSE#v1".to_string(),
            "viewer".to_string(),
            24,
            None,
            "   ",
        );
        match blank {
            DbCommand::MintApiToken { client_did, .. } => assert_eq!(client_did, None),
            other => panic!("expected MintApiToken, got {other:?}"),
        }

        let filled = mint_command(
            "VERSE#v1".to_string(),
            "editor".to_string(),
            720,
            Some("node:n1".to_string()),
            "  did:key:zExample  ",
        );
        match filled {
            DbCommand::MintApiToken {
                client_did, label, ..
            } => {
                assert_eq!(client_did, Some("did:key:zExample".to_string()));
                assert_eq!(label, Some("node:n1".to_string()));
            }
            other => panic!("expected MintApiToken, got {other:?}"),
        }
    }

    #[test]
    fn handle_api_token_minted_surfaces_the_client_for_both_mint_surfaces() {
        let mut ui_mgr = UiManager::default();
        ui_mgr.open_dialog(entity_settings_dialog());
        let mut inspector = InspectorFormState::default();
        let (db_sender, _rx) = sender();

        handle_api_token_minted(
            "jwt.token.value",
            "jti-1",
            "VERSE#v1",
            "viewer",
            "did:key:zClient1",
            &mut ui_mgr,
            &mut inspector,
            &db_sender,
        );

        assert_eq!(
            inspector.generated_api_token,
            Some("jwt.token.value".to_string())
        );
        assert_eq!(
            inspector.generated_api_token_client,
            Some("did:key:zClient1".to_string())
        );
        if let ActiveDialog::EntitySettings {
            generated_api_token,
            generated_api_token_client,
            ..
        } = &ui_mgr.active_dialog
        {
            assert_eq!(generated_api_token, &Some("jwt.token.value".to_string()));
            assert_eq!(
                generated_api_token_client,
                &Some("did:key:zClient1".to_string())
            );
        } else {
            panic!("expected EntitySettings dialog to stay open");
        }
    }

    #[test]
    fn tokens_to_entries_carries_the_client_claim() {
        let infos = vec![
            ApiTokenInfo {
                jti: "jti-a".into(),
                scope: "VERSE#v1".into(),
                max_role: "viewer".into(),
                label: None,
                created_at: "2026-10-25T00:00:00Z".into(),
                expires_at: "2026-11-25T00:00:00Z".into(),
                revoked: false,
                sub: "did:key:zNode".into(),
                client: Some("did:key:zClientA".into()),
            },
            ApiTokenInfo {
                jti: "jti-b".into(),
                scope: "VERSE#v1".into(),
                max_role: "viewer".into(),
                label: None,
                created_at: "2026-10-25T00:00:00Z".into(),
                expires_at: "2026-11-25T00:00:00Z".into(),
                revoked: false,
                sub: "did:key:zNode".into(),
                client: None,
            },
        ];
        let entries: Vec<ApiTokenEntry> = tokens_to_entries(&infos);
        assert_eq!(entries[0].client, Some("did:key:zClientA".to_string()));
        assert_eq!(entries[1].client, None);
    }
}
