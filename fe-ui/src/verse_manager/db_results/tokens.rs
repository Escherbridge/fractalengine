//! Handlers for API-token results (mint/revoke/list) plus the token-list
//! refresh helpers. See ../AGENTS.md §db-results.

use fe_runtime::app::{DbCommandSender, RevocationBroadcastSender};
use fe_runtime::messages::{ApiTokenInfo, DbCommand};

use crate::actions::UiManager;
use crate::dialogs::{ActiveDialog, ApiTokenEntry};
use crate::plugin::InspectorFormState;

/// `ApiTokenMinted`: surface the minted token and refresh the token list.
pub(super) fn handle_api_token_minted(
    token: &str,
    jti: &str,
    scope: &str,
    max_role: &str,
    ui_mgr: &mut UiManager,
    inspector: &mut InspectorFormState,
    db_sender: &DbCommandSender,
) {
    if let ActiveDialog::EntitySettings {
        ref mut generated_api_token,
        ..
    } = ui_mgr.active_dialog
    {
        *generated_api_token = Some(token.to_string());
    }
    inspector.generated_api_token = Some(token.to_string());
    bevy::log::info!(
        "API token minted: jti={} scope={} role={}",
        jti,
        scope,
        max_role
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
