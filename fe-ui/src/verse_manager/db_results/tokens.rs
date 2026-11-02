//! Handlers for API-token results (mint/revoke/list) plus the token-list
//! refresh helpers. See ../AGENTS.md §db-results.

use fe_runtime::app::{DbCommandSender, RevocationBroadcastSender};
use fe_runtime::messages::{ApiTokenInfo, DbCommand, DbResult};

use crate::actions::UiManager;
use crate::dialogs::ApiTokenEntry;
use crate::plugin::InspectorFormState;

/// Route only an exact Inspector mint; legacy secrets have no UI owner.
pub(super) fn handle_api_token_minted(
    result: &DbResult,
    selected: Option<&str>,
    current_scope: &str,
    can_manage: bool,
    ui_mgr: &mut UiManager,
    inspector: &mut InspectorFormState,
    db_sender: &DbCommandSender,
) -> bool {
    let DbResult::ApiTokenMinted {
        token,
        scope,
        max_role,
        client,
        correlation_id,
        ..
    } = result
    else {
        return false;
    };
    if let Some(id) = correlation_id {
        if !crate::panels::inspector::InspectorTokenRequest::owns(id) {
            return false;
        }
        crate::panels::inspector::synchronize_token_target(
            inspector,
            selected,
            current_scope,
            can_manage,
        );
        if inspector.active_tab() == crate::plugin::InspectorTab::ApiAccess
            && inspector.api_token_request.accept(
                id,
                scope,
                max_role,
                client,
                std::time::Instant::now(),
            )
        {
            inspector.generated_api_token = Some(token.clone());
            inspector.generated_api_token_client = Some(client.clone());
            refresh_inspector_tokens(db_sender, inspector);
        }
        return true;
    }
    // EntitySettings uses EntityAccessFinished; never retain an unowned credential.
    false
}

/// Consume Inspector mint failures by nonce without echoing backend error text.
pub(super) fn handle_inspector_token_failure(
    result: &DbResult,
    selected: Option<&str>,
    current_scope: &str,
    can_manage: bool,
    inspector: &mut InspectorFormState,
) -> bool {
    let DbResult::RequestFailed { correlation_id, .. } = result else {
        return false;
    };
    if !crate::panels::inspector::InspectorTokenRequest::owns(correlation_id) {
        return false;
    }
    crate::panels::inspector::synchronize_token_target(
        inspector,
        selected,
        current_scope,
        can_manage,
    );
    inspector
        .api_token_request
        .fail(correlation_id, std::time::Instant::now());
    true
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
    correlation_id: Option<String>,
) -> DbCommand {
    let trimmed = client_buf.trim();
    let client_did = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
    DbCommand::MintApiToken {
        correlation_id,
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
        let mut dialog = crate::dialogs::entity_details::dialog(
            EntitySettingsType::Verse,
            "v1",
            "Verse",
            "v1",
            None,
        );
        if let ActiveDialog::EntitySettings {
            active_tab,
            ..
        } = &mut dialog
        {
            *active_tab = SettingsTab::ApiAccess;
        }
        dialog
    }

    #[test]
    fn a_blank_client_field_sends_none_and_a_filled_one_sends_it_trimmed() {
        let blank = mint_command(
            "VERSE#v1".to_string(),
            "viewer".to_string(),
            24,
            None,
            "   ",
            None,
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
            None,
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
    fn uncorrelated_mint_retains_no_secret_and_exact_inspector_nonce_stays_isolated() {
        let mut ui_mgr = UiManager::default();
        ui_mgr.open_dialog(entity_settings_dialog());
        let mut inspector = InspectorFormState::default();
        let (db_sender, rx) = sender();

        let result = DbResult::ApiTokenMinted {
            token: "private-fixture".into(),
            jti: "jti-1".into(),
            scope: "VERSE#v1".into(),
            max_role: "viewer".into(),
            client: "did:key:zClient1".into(),
            expires_at: "2026-11-02T12:00:00Z".into(),
            label: None,
            correlation_id: None,
        };
        handle_api_token_minted(
            &result,
            None,
            "",
            true,
            &mut ui_mgr,
            &mut inspector,
            &db_sender,
        );
        assert!(inspector.generated_api_token.is_none());
        assert!(inspector.generated_api_token_client.is_none());
        if let ActiveDialog::EntitySettings {access,..}=&ui_mgr.active_dialog {
            assert!(access.snapshot.is_none(),"Uncorrelated mint cannot populate target Access state");
        } else {panic!("EntitySettings remains open");}
        while rx.try_recv().is_ok() {}
        inspector.set_active_tab(crate::plugin::InspectorTab::ApiAccess);
        crate::panels::inspector::synchronize_token_target(
            &mut inspector,
            Some("n1"),
            "VERSE#v1",
            true,
        );
        crate::panels::inspector::submit_inspector_token(
            &mut inspector,
            "n1",
            true,
            &db_sender.0,
            std::time::Instant::now(),
        );
        let DbCommand::MintApiToken {
            correlation_id: Some(nonce),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("Inspector mint must carry a nonce")
        };
        let mut correlated = result.clone();
        if let DbResult::ApiTokenMinted {
            correlation_id,
            token,
            ..
        } = &mut correlated
        {
            *correlation_id = Some(format!("{nonce}-unknown"));
            *token = "inspector-fixture".into();
        }
        assert!(handle_api_token_minted(
            &correlated,
            Some("n1"),
            "VERSE#v1",
            true,
            &mut ui_mgr,
            &mut inspector,
            &db_sender
        ));
        assert!(inspector.generated_api_token.is_none());
        if let DbResult::ApiTokenMinted { correlation_id, .. } = &mut correlated {
            *correlation_id = Some(nonce);
        }
        assert!(handle_api_token_minted(
            &correlated,
            Some("n1"),
            "VERSE#v1",
            true,
            &mut ui_mgr,
            &mut inspector,
            &db_sender
        ));
        assert_eq!(
            inspector.generated_api_token.as_deref(),
            Some("inspector-fixture")
        );
        assert_eq!(
            inspector.generated_api_token_client.as_deref(),
            Some("did:key:zClient1")
        );
        if let ActiveDialog::EntitySettings {access,..}=&ui_mgr.active_dialog {assert!(access.snapshot.is_none());}
        while rx.try_recv().is_ok() {}
        crate::panels::inspector::submit_inspector_token(
            &mut inspector,
            "n1",
            true,
            &db_sender.0,
            std::time::Instant::now(),
        );
        let DbCommand::MintApiToken {
            correlation_id: Some(report_nonce),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("report transition fixture needs a pending mint")
        };
        if let DbResult::ApiTokenMinted { correlation_id, .. } = &mut correlated {
            *correlation_id = Some(report_nonce);
        }
        let mut nodes = crate::node_manager::NodeManager::default();
        nodes.select(bevy::prelude::Entity::from_bits(1), "n1");
        let mut sidebar = crate::ui_shell::right_sidebar::RightSidebarState::default();
        crate::ui_shell::right_sidebar::open_report_node_properties(
            "n1".into(),
            &mut nodes,
            &mut inspector,
            &mut sidebar,
        );
        for tab in [
            crate::plugin::InspectorTab::Properties,
            crate::plugin::InspectorTab::ApiAccess,
        ] {
            inspector.set_active_tab(tab);
            assert!(handle_api_token_minted(
                &correlated,
                Some("n1"),
                "VERSE#v1",
                true,
                &mut ui_mgr,
                &mut inspector,
                &db_sender,
            ));
            assert!(
                inspector.generated_api_token.is_none()
                    && inspector.generated_api_token_client.is_none()
            );
            assert!(
                rx.try_recv().is_err(),
                "late report-transition reply must not refresh as accepted"
            );
        }
        assert!(handle_api_token_minted(
            &correlated,
            Some("n2"),
            "VERSE#v1",
            true,
            &mut ui_mgr,
            &mut inspector,
            &db_sender
        ));
        assert!(
            inspector.generated_api_token.is_none()
                && inspector.generated_api_token_client.is_none()
        );
        assert!(handle_api_token_minted(
            &correlated,
            Some("n1"),
            "VERSE#other",
            true,
            &mut ui_mgr,
            &mut inspector,
            &db_sender
        ));
        assert!(inspector.generated_api_token.is_none());
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
