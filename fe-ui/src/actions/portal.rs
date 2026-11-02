//! Portal + URL-save action handling. This is the browser-integration seam:
//! keep `compute_open_portal` / `should_auto_close` / `compute_save_url` pure
//! so save/portal semantics stay unit-testable without a Bevy `App`. See
//! `fe-ui/src/AGENTS.md` §actions and §portal.

use bevy::prelude::Entity;

use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;
use crate::portal::PortalState;

#[derive(Default)]
pub(crate) struct PortalFeedback {
    pub loading: bool,
    pub error: Option<String>,
}

impl PortalFeedback {
    pub fn opening() -> Self {
        if fe_webview::backends::embedded_browsing_supported() {
            Self {
                loading: true,
                error: None,
            }
        } else {
            Self { loading: false, error: Some("Embedded browsing is unavailable in this build. Choose Open externally to request your browser.".into()) }
        }
    }
}

pub(crate) fn receive_browser_event(
    ui: &mut super::UiManager,
    nodes: &NodeManager,
    event: &fe_webview::ipc::BrowserEvent,
    now: f64,
) {
    if should_auto_close(&ui.portal, nodes.selected_entity()) || !ui.portal_is_open() {
        return;
    }
    use fe_webview::ipc::BrowserEvent;
    match event {
        BrowserEvent::UrlChanged { url } if fe_webview::security::embedded_preview_allowed(url.as_str()) => {
            if let PortalState::Open {
                current_url,
                cached_hostname,
                ..
            } = &mut ui.portal
            {
                *current_url = url.to_string();
                *cached_hostname = url.host_str().unwrap_or_default().to_owned();
            }
            ui.portal_feedback = PortalFeedback::opening();
        }
        BrowserEvent::PageLoaded { url }
            if ui
                .portal_url()
                .parse::<url::Url>()
                .is_ok_and(|current| &current == url) =>
        {
            ui.portal_feedback.loading = false;
        }
        BrowserEvent::Error { message } => {
            ui.portal_feedback.loading = false;
            ui.portal_feedback.error = Some(message.clone());
        }
        BrowserEvent::ExternalLaunchRequested => {
            ui.show_toast("External browser launch requested", now)
        }
        _ => {}
    }
}

/// One local write owner; obsolete receipts cannot confirm another draft.
pub(crate) struct PendingUrlSave {
    pub request_id: String,
    pub node_id: String,
    pub scope: String,
    pub selection: Option<String>,
    pub started: f64,
    pub submitted: Option<String>,
}

pub(crate) fn node_scope(
    hierarchy: &crate::verse_manager::VerseManager,
    id: &str,
) -> Option<String> {
    for verse in &hierarchy.verses {
        for fractal in &verse.fractals {
            for petal in &fractal.petals {
                if petal.nodes.iter().any(|node| node.id == id) {
                    return Some(fe_database::build_scope(
                        &verse.id,
                        Some(&fractal.id),
                        Some(&petal.id),
                    ));
                }
            }
        }
    }
    None
}

/// Validate the shared Inspector and Node Options draft.
pub(crate) fn validate_url(raw: &str) -> Result<Option<String>, String> {
    if fe_runtime::qa_web::preview_allows(raw) {
        return Ok(Some(raw.to_owned()));
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if raw.len() > 8192 {
        return Err("URL must be at most 8192 bytes".into());
    }
    match raw.parse::<url::Url>() {
        Ok(parsed) if fe_webview::security::is_url_allowed(&parsed) => Ok(Some(raw.to_owned())),
        Ok(_) => Err("URL blocked by security policy (private/loopback hosts and non-http schemes are not allowed)".into()),
        Err(error) => Err(format!("invalid URL: {error}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_url(
    sender: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    ui: &mut super::UiManager,
    hierarchy: &crate::verse_manager::VerseManager,
    nav: &crate::navigation_manager::NavigationManager,
    nodes: &NodeManager,
    role: &crate::plugin::LocalUserRole,
    node_id: String,
    url: Option<String>,
    now: f64,
) {
    if ui.pending_url_save.is_some() {
        ui.show_advisory("Wait for the current URL save confirmation", now);
        return;
    }
    if nodes
        .selected
        .as_ref()
        .map(|selected| selected.node_id.as_str())
        != Some(node_id.as_str())
    {
        ui.show_advisory("Select this node again before saving its URL", now);
        return;
    }
    let Some(scope) =
        node_scope(hierarchy, &node_id).filter(|scope| nav.active_scope().as_ref() == Some(scope))
    else {
        ui.show_advisory("Navigate to this node's Petal before saving its URL", now);
        return;
    };
    if !role.can_edit() {
        ui.show_advisory("Editor access is required to save this Portal URL", now);
        return;
    }
    let url = match validate_url(url.as_deref().unwrap_or_default()) {
        Ok(url) => url,
        Err(reason) => {
            ui.show_error(format!("URL not saved: {reason}"), now);
            return;
        }
    };
    let request_id = ulid::Ulid::new().to_string();
    let submitted = url.clone();
    if let Err(reason) = ui.url_read_model.can_write(hierarchy, &node_id, now) {
        ui.show_advisory(reason, now);
        return;
    }
    if super::send_or_toast(
        sender,
        fe_runtime::messages::DbCommand::UpdateNodeUrl {
            node_id: node_id.clone(),
            url,
            request_id: request_id.clone(),
            expected_scope: scope.clone(),
        },
        ui,
        now,
        "save the URL",
    ) {
        ui.url_read_model
            .track_write(hierarchy, &node_id, request_id.clone(), now);
        ui.pending_url_save = Some(PendingUrlSave {
            request_id,
            node_id,
            scope,
            selection: nodes
                .selected
                .as_ref()
                .map(|selected| selected.node_id.clone()),
            started: now,
            submitted,
        });
        ui.show_toast("Saving URL…", now);
    }
}

pub(crate) fn finish_url_save(
    ui: &mut super::UiManager,
    hierarchy: &mut crate::verse_manager::VerseManager,
    nav: &crate::navigation_manager::NavigationManager,
    nodes: &NodeManager,
    result: &fe_runtime::messages::DbResult,
    now: f64,
    sender: &crossbeam::channel::Sender<fe_runtime::messages::DbCommand>,
    inspector: &InspectorFormState,
) {
    super::url_read_model::committed_write(
        sender, ui, hierarchy, nav, nodes, inspector, result, now,
    );
    let fe_runtime::messages::DbResult::NodeUrlUpdated {
        node_id,
        request_id,
        expected_scope,
        outcome,
        ..
    } = result
    else {
        return;
    };
    let Some(pending) = ui.pending_url_save.as_ref() else {
        return;
    };
    if pending.request_id != *request_id
        || pending.node_id != *node_id
        || pending.scope != *expected_scope
    {
        return;
    }
    let current = now - pending.started <= 30.0
        && nav.active_scope().as_ref() == Some(expected_scope)
        && node_scope(hierarchy, node_id).as_ref() == Some(expected_scope)
        && nodes
            .selected
            .as_ref()
            .map(|selected| selected.node_id.as_str())
            == pending.selection.as_deref();
    let newer_draft =
        inspector.external_url.trim() != pending.submitted.as_deref().unwrap_or_default();
    ui.pending_url_save = None;
    if !current {
        return;
    }
    match outcome {
        Ok(_) => {
            ui.show_toast(
                if newer_draft {
                    "Previous URL saved; your newer draft is still unsaved"
                } else {
                    "URL saved"
                },
                now,
            );
        }
        Err(reason) => ui.show_error(format!("URL not saved: {reason}"), now),
    }
}

/// Result of attempting to open the portal for a raw URL string.
pub(crate) enum OpenPortalOutcome {
    /// URL parsed and a node is selected — apply this portal state and navigate.
    Navigate(PortalState, url::Url),
    /// The caller explains that no node is selected.
    NoSelection,
    /// URL failed to parse; caller should log a warning with this message.
    InvalidUrl(String),
}

/// Pure: decide what `UiAction::OpenPortal` should do given current selection.
pub(crate) fn compute_open_portal(node_mgr: &NodeManager, raw_url: &str) -> OpenPortalOutcome {
    if let Err(reason) = validate_url(raw_url) {
        return OpenPortalOutcome::InvalidUrl(reason);
    }
    match raw_url.trim().parse::<url::Url>() {
        Ok(parsed) => match node_mgr.selected_entity() {
            Some(entity) => {
                let cached_hostname = parsed.host_str().unwrap_or("").to_string();
                let new_state = PortalState::Open {
                    current_url: parsed.to_string(),
                    cached_hostname,
                    opened_for_entity: entity,
                };
                OpenPortalOutcome::Navigate(new_state, parsed)
            }
            None => OpenPortalOutcome::NoSelection,
        },
        Err(e) => OpenPortalOutcome::InvalidUrl(e.to_string()),
    }
}

/// Pure: whether the portal should auto-close this frame because the
/// selection changed or was cleared while the portal was open for a
/// specific entity.
pub(crate) fn should_auto_close(portal: &PortalState, selected: Option<Entity>) -> bool {
    let PortalState::Open {
        opened_for_entity, ..
    } = portal
    else {
        return false;
    };
    let entity_changed = selected != Some(*opened_for_entity);
    selected.is_none() || entity_changed
}

/// Result of validating + computing the persist payload for the captured URL save action.
pub(crate) enum SaveUrlOutcome {
    /// Persist `url` for `node_id`; `None` clears the stored URL.
    Persist {
        node_id: String,
        url: Option<String>,
    },
    /// URL unparseable or rejected by `is_url_allowed` — show `reason` to the user.
    Blocked { reason: String },
    /// Nothing is selected; caller surfaces a "no selection" toast.
    NoSelection,
}

/// Pure: validate + compute what the captured URL save action should persist.
/// Empty/whitespace-only `external_url` maps to `None` (clears the URL);
/// otherwise the trimmed buffer is stored, matching Node Options' save path
/// so both Portal URL editors normalize identically — see AGENTS.md §portal
/// for the FR-1 validation rationale.
pub(crate) fn compute_save_url(
    node_mgr: &NodeManager,
    inspector: &InspectorFormState,
) -> SaveUrlOutcome {
    let Some(node_id) = node_mgr.selected.as_ref().map(|s| s.node_id.clone()) else {
        return SaveUrlOutcome::NoSelection;
    };
    match validate_url(&inspector.external_url) {
        Ok(url) => SaveUrlOutcome::Persist { node_id, url },
        Err(reason) => SaveUrlOutcome::Blocked { reason },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;

    fn entity(n: u32) -> Entity {
        Entity::from_bits(n as u64)
    }

    #[test]
    fn compute_open_portal_invalid_url_reports_error() {
        let mgr = NodeManager::default();
        match compute_open_portal(&mgr, "not a url") {
            OpenPortalOutcome::InvalidUrl(_) => {}
            _ => panic!("expected InvalidUrl"),
        }
    }

    #[test]
    fn compute_open_portal_no_selection_is_silent_noop() {
        let mgr = NodeManager::default();
        match compute_open_portal(&mgr, "https://example.com") {
            OpenPortalOutcome::NoSelection => {}
            _ => panic!("expected NoSelection"),
        }
    }

    #[test]
    fn compute_open_portal_with_selection_navigates() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        match compute_open_portal(&mgr, "https://example.com/path") {
            OpenPortalOutcome::Navigate(
                PortalState::Open {
                    opened_for_entity,
                    cached_hostname,
                    ..
                },
                url,
            ) => {
                assert_eq!(opened_for_entity, entity(1));
                assert_eq!(cached_hostname, "example.com");
                assert_eq!(url.as_str(), "https://example.com/path");
            }
            _ => panic!("expected Navigate"),
        }
    }

    #[test]
    fn should_auto_close_false_when_portal_closed() {
        assert!(!should_auto_close(&PortalState::Closed, Some(entity(1))));
    }

    #[test]
    fn should_auto_close_true_when_selection_cleared() {
        let portal = PortalState::Open {
            current_url: "https://example.com".into(),
            cached_hostname: "example.com".into(),
            opened_for_entity: entity(1),
        };
        assert!(should_auto_close(&portal, None));
    }

    #[test]
    fn should_auto_close_true_when_selection_changed() {
        let portal = PortalState::Open {
            current_url: "https://example.com".into(),
            cached_hostname: "example.com".into(),
            opened_for_entity: entity(1),
        };
        assert!(should_auto_close(&portal, Some(entity(2))));
    }

    #[test]
    fn should_auto_close_false_when_same_entity_still_selected() {
        let portal = PortalState::Open {
            current_url: "https://example.com".into(),
            cached_hostname: "example.com".into(),
            opened_for_entity: entity(1),
        };
        assert!(!should_auto_close(&portal, Some(entity(1))));
        let mut ui = super::super::UiManager::default();
        ui.portal = portal;
        ui.portal_feedback.loading = true;
        let mut nodes = NodeManager::default();
        nodes.select(entity(1), "node-1");
        use fe_webview::ipc::BrowserEvent;
        receive_browser_event(&mut ui, &nodes, &BrowserEvent::LoadComplete, 0.0);
        assert!(
            ui.portal_feedback.loading,
            "an unbound legacy completion cannot confirm this page"
        );
        receive_browser_event(
            &mut ui,
            &nodes,
            &BrowserEvent::PageLoaded {
                url: "https://other.example/".parse().unwrap(),
            },
            0.0,
        );
        assert!(ui.portal_feedback.loading);
        receive_browser_event(
            &mut ui,
            &nodes,
            &BrowserEvent::PageLoaded {
                url: "https://example.com/".parse().unwrap(),
            },
            0.0,
        );
        assert!(!ui.portal_feedback.loading);
        nodes.deselect();
        receive_browser_event(
            &mut ui,
            &nodes,
            &BrowserEvent::UrlChanged {
                url: "https://other.example/".parse().unwrap(),
            },
            1.0,
        );
        assert_eq!(ui.portal_url(), "https://example.com");
    }

    fn selected_with_url(url: &str) -> (NodeManager, InspectorFormState) {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        let mut inspector = InspectorFormState::default();
        inspector.external_url = url.to_string();
        (mgr, inspector)
    }

    #[test]
    fn compute_save_url_no_selection() {
        let mgr = NodeManager::default();
        let inspector = InspectorFormState::default();
        assert!(matches!(
            compute_save_url(&mgr, &inspector),
            SaveUrlOutcome::NoSelection
        ));
    }

    #[test]
    fn compute_save_url_empty_buffer_maps_to_none() {
        let (mgr, inspector) = selected_with_url("   ");
        match compute_save_url(&mgr, &inspector) {
            SaveUrlOutcome::Persist { node_id, url } => {
                assert_eq!(node_id, "node-1");
                assert_eq!(url, None);
            }
            _ => panic!("expected Persist"),
        }
    }

    #[test]
    fn compute_save_url_allowed_url_persists() {
        let (mgr, inspector) = selected_with_url("https://example.com");
        match compute_save_url(&mgr, &inspector) {
            SaveUrlOutcome::Persist { node_id, url } => {
                assert_eq!(node_id, "node-1");
                assert_eq!(url, Some("https://example.com".to_string()));
            }
            _ => panic!("expected Persist"),
        }
    }

    #[test]
    fn compute_save_url_trims_whitespace() {
        let (mgr, inspector) = selected_with_url("  https://example.com  ");
        match compute_save_url(&mgr, &inspector) {
            SaveUrlOutcome::Persist { url, .. } => {
                assert_eq!(url, Some("https://example.com".to_string()));
            }
            _ => panic!("expected Persist"),
        }
    }

    #[test]
    fn compute_save_url_blocks_loopback() {
        let (mgr, inspector) = selected_with_url("http://localhost:8080/");
        assert!(matches!(
            compute_save_url(&mgr, &inspector),
            SaveUrlOutcome::Blocked { .. }
        ));
    }

    #[test]
    fn compute_save_url_blocks_private_range() {
        let (mgr, inspector) = selected_with_url("http://192.168.1.1/admin");
        assert!(matches!(
            compute_save_url(&mgr, &inspector),
            SaveUrlOutcome::Blocked { .. }
        ));
    }

    #[test]
    fn compute_save_url_blocks_unparseable() {
        let (mgr, inspector) = selected_with_url("not a url at all");
        match compute_save_url(&mgr, &inspector) {
            SaveUrlOutcome::Blocked { reason } => assert!(reason.contains("invalid URL")),
            _ => panic!("expected Blocked"),
        }
    }
}
