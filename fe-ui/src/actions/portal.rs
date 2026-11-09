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
    if should_auto_close(&ui.portal, nodes.selected_entity()) || !ui.portal.is_open() {
        return;
    }
    use fe_webview::ipc::BrowserEvent;
    match event {
        BrowserEvent::UrlChanged { url }
            if fe_webview::security::embedded_preview_allowed(url.as_str()) =>
        {
            // A mounted bundle left for a declared origin (protocol.md §2.6): the page is now an external website,
            // still shown, so it gets the website chrome; the host's `Closed` for the bundle then finds no Bundle.
            // Before the first mounted state (`label` is None) a URL change is the previous page's, so it is ignored.
            let mounted = matches!(&ui.portal, PortalState::Bundle { label: Some(_), .. });
            let left_bundle = mounted.then(|| ui.portal.opened_for_entity()).flatten();
            if let Some(opened_for_entity) = left_bundle {
                ui.portal = PortalState::Open {
                    current_url: url.to_string(),
                    cached_hostname: url.host_str().unwrap_or_default().to_owned(),
                    opened_for_entity,
                };
            } else if let PortalState::Open {
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
        BrowserEvent::BundleState { node_id, label, state } => apply_bundle_state(ui, node_id, label, state),
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
    if is_bundle_origin_url(raw) {
        return Err("bundle origins open only from a repository node".into());
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
    if nodes.primary()
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
            selection: nodes.primary()
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
        && nodes.primary()
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

/// Pure: whether `raw` spells a bundle origin (either spelling) or uses the `fe-portal` scheme.
pub(crate) fn is_bundle_origin_url(raw: &str) -> bool {
    let raw = raw.trim();
    raw.to_ascii_lowercase()
        .starts_with(&format!("{}:", fe_webview::portal_bundle::SCHEME))
        || fe_webview::portal_bundle::origin::parse(raw).is_some()
}

/// Result of attempting to open a repository node's bundle.
pub(crate) enum OpenBundleOutcome {
    /// Apply this state and write `BrowserCommand::OpenBundle { node_id }`.
    Mount(PortalState),
    /// The node is not the single selected node.
    NotSelected,
}

/// Pure: decide what `UiAction::OpenBundle` does; the bundle must be the selected node.
pub(crate) fn compute_open_bundle(node_mgr: &NodeManager, node_id: &str) -> OpenBundleOutcome {
    let (Some(entity), Some(selected)) = (node_mgr.selected_entity(), node_mgr.primary()) else {
        return OpenBundleOutcome::NotSelected;
    };
    if selected.node_id != node_id {
        return OpenBundleOutcome::NotSelected;
    }
    let state = if fe_webview::backends::embedded_browsing_supported() {
        fe_webview::ipc::BundleState::Closed
    } else {
        fe_webview::ipc::BundleState::Unavailable
    };
    OpenBundleOutcome::Mount(PortalState::Bundle {
        node_id: node_id.to_owned(),
        label: None,
        state,
        opened_for_entity: entity,
    })
}

/// `UiAction::OpenBundle`: opens the portal for a repository node; returns the node id to mount.
pub(crate) fn open_bundle(
    ui: &mut super::UiManager,
    nodes: &NodeManager,
    node_id: &str,
    now: f64,
) -> Option<String> {
    match compute_open_bundle(nodes, node_id) {
        OpenBundleOutcome::Mount(state) => {
            ui.portal_feedback = PortalFeedback {
                loading: matches!(state.bundle_state(), Some(fe_webview::ipc::BundleState::Closed)),
                error: None,
            };
            ui.portal = state;
            Some(node_id.to_owned())
        }
        OpenBundleOutcome::NotSelected => {
            ui.show_advisory("Select this repository node before opening it", now);
            None
        }
    }
}

/// Most admission requests one undrained queue holds; a click storm before the binary drains cannot grow it further.
const MAX_QUEUED_ADMISSIONS: usize = 8;

/// `UiAction::PortalAdmitBundle` / `PortalRevokeBundle`: queue the request for the named node (the inspector's
/// selection or the open bundle), never "whatever portal is open"; an identical queued request is not repeated.
pub(crate) fn queue_admission(ui: &mut super::UiManager, request: crate::portal::BundleAdmissionRequest) {
    let queue = &mut ui.bundle_admission_requests;
    if !queue.contains(&request) && queue.len() < MAX_QUEUED_ADMISSIONS {
        queue.push(request);
    }
}

/// Host-decided bundle state -> portal state; events for another node are stale and ignored.
fn apply_bundle_state(
    ui: &mut super::UiManager,
    event_node: &str,
    event_label: &Option<String>,
    event_state: &fe_webview::ipc::BundleState,
) {
    use fe_webview::ipc::BundleState;
    let PortalState::Bundle { node_id, label, state, .. } = &mut ui.portal else {
        return;
    };
    if node_id != event_node {
        return;
    }
    if matches!(event_state, BundleState::Closed) {
        ui.portal = PortalState::Closed;
        ui.portal_feedback = PortalFeedback::default();
        return;
    }
    let admission_moved = *state != *event_state
        && [&*state, event_state]
            .iter()
            .any(|side| matches!(side, BundleState::Admitted { .. } | BundleState::Revoked(_)));
    *label = event_label.clone();
    *state = event_state.clone();
    ui.portal_feedback = PortalFeedback::default();
    if admission_moved {
        // The cached summary's admission is stale; the Repository section re-reads it when it shows again.
        ui.repository_view.invalidate(event_node);
    }
}

/// Pure: whether the portal should auto-close this frame because the
/// selection changed or was cleared while the portal was open for a
/// specific entity.
pub(crate) fn should_auto_close(portal: &PortalState, selected: Option<Entity>) -> bool {
    let Some(opened_for_entity) = portal.opened_for_entity() else {
        return false;
    };
    selected != Some(opened_for_entity)
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
    let Some(node_id) = node_mgr.primary().map(|s| s.node_id.clone()) else {
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

        for (width, height, scale) in [
            (1024.0, 640.0, 1.0),
            (1280.0, 720.0, 1.0),
            (1920.0, 1080.0, 1.0),
            (1280.0, 720.0, 1.25),
            (1024.0, 640.0, 2.0),
        ] {
            use bevy_egui::egui;
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            let mut feedback_ui = super::super::UiManager::default();
            feedback_ui.portal = ui.portal.clone();
            let mut frame = 0;
            let mut render = |feedback_ui: &mut super::super::UiManager| {
                for _ in 0..3 {
                    frame += 1;
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width / scale, height / scale),
                            )),
                            time: Some(frame as f64 * 0.1),
                            ..Default::default()
                        },
                        |ctx| {
                            crate::panels::portal_toolbar::right_portal_toolbar(ctx, feedback_ui);
                        },
                    );
                }
                feedback_ui.portal_content_rect.unwrap()
            };
            let ready = render(&mut feedback_ui);
            assert!(ready.width() > 0.0 && ready.height() > 0.0);
            receive_browser_event(
                &mut feedback_ui,
                &nodes,
                &BrowserEvent::UrlChanged {
                    url: "https://example.com/two".parse().unwrap(),
                },
                1.0,
            );
            assert_eq!(render(&mut feedback_ui), ready,
                "navigation feedback cannot resize the native viewport at {width}x{height}/{scale}x");
            receive_browser_event(
                &mut feedback_ui,
                &nodes,
                &BrowserEvent::PageLoaded {
                    url: "https://example.com/two".parse().unwrap(),
                },
                1.1,
            );
            assert_eq!(render(&mut feedback_ui), ready);
            receive_browser_event(
                &mut feedback_ui,
                &nodes,
                &BrowserEvent::Error {
                    message: "Bounded native browser failure details. ".repeat(40),
                },
                1.2,
            );
            assert_eq!(
                render(&mut feedback_ui),
                ready,
                "long errors stay scrollable inside the same reserved feedback slot"
            );
            feedback_ui.portal_feedback = PortalFeedback::default();
            assert_eq!(render(&mut feedback_ui), ready);
        }
        // Bundle journey: raw bundle URLs are refused, Open mounts, host events drive the chrome.
        for raw in ["fe-portal://a-aaaaaaaaaaaaaaaaaaaaaaaaaa.localhost/x", "https://fe-portal.s-aaaaaaaaaaaaaaaaaaaaaaaaaa.localhost/web/index.html"] {
            assert!(validate_url(raw).is_err(), "{raw}");
        }
        let mut bundle_ui = super::super::UiManager::default();
        assert!(open_bundle(&mut bundle_ui, &NodeManager::default(), "node-1", 0.0).is_none());
        assert_eq!(open_bundle(&mut bundle_ui, &nodes, "node-1", 0.0).as_deref(), Some("node-1"));
        assert!(bundle_ui.portal.is_bundle() && bundle_ui.portal.url().is_empty());
        let blocked = BrowserEvent::BundleState {
            node_id: "node-1".into(),
            label: None,
            state: fe_webview::ipc::BundleState::Blocked(fe_webview::ipc::BlockedReason::TooLarge),
        };
        receive_browser_event(&mut bundle_ui, &nodes, &blocked, 0.0);
        assert!(matches!(bundle_ui.portal.bundle_state(), Some(fe_webview::ipc::BundleState::Blocked(_))));
        let stale = BrowserEvent::BundleState {
            node_id: "node-2".into(),
            label: None,
            state: fe_webview::ipc::BundleState::WaitingForBytes,
        };
        receive_browser_event(&mut bundle_ui, &nodes, &stale, 0.0);
        assert!(matches!(bundle_ui.portal.bundle_state(), Some(fe_webview::ipc::BundleState::Blocked(_))));
        let closed = BrowserEvent::BundleState { node_id: "node-1".into(), label: None, state: fe_webview::ipc::BundleState::Closed };
        receive_browser_event(&mut bundle_ui, &nodes, &closed, 0.0);
        assert!(!bundle_ui.portal.is_open());
        // The inspector's Admit names its node: it queues with no portal open (the inspector case), and only once.
        let admit = crate::portal::BundleAdmissionRequest::Admit { node_id: "node-1".into() };
        for _ in 0..3 {
            queue_admission(&mut bundle_ui, admit.clone());
        }
        assert_eq!(bundle_ui.bundle_admission_requests, [admit]);
        // Leave (protocol.md §2.6): a mounted bundle navigates to a declared origin; the shown page keeps chrome.
        let docs = BrowserEvent::UrlChanged { url: "https://docs.example.org/guide".parse().unwrap() };
        assert_eq!(open_bundle(&mut bundle_ui, &nodes, "node-1", 0.0).as_deref(), Some("node-1"));
        receive_browser_event(&mut bundle_ui, &nodes, &docs, 0.0);
        assert!(bundle_ui.portal.is_bundle(), "a URL change before the first mounted state is the previous page's");
        let mounted = BrowserEvent::BundleState {
            node_id: "node-1".into(),
            label: Some("s-aaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            state: fe_webview::ipc::BundleState::StaticSandbox { can_admit: false },
        };
        receive_browser_event(&mut bundle_ui, &nodes, &mounted, 0.0);
        assert!(bundle_ui.repository_view.take_wanted().is_empty(), "the first mounted state is not an admission change");
        let admitted = BrowserEvent::BundleState {
            node_id: "node-1".into(),
            label: Some("a-aaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            state: fe_webview::ipc::BundleState::Admitted { publisher: "did:key:z6Mkexample0000".into() },
        };
        receive_browser_event(&mut bundle_ui, &nodes, &admitted, 0.0);
        assert_eq!(bundle_ui.repository_view.take_wanted(), ["node-1"], "an admission change re-reads the section summary");
        receive_browser_event(&mut bundle_ui, &nodes, &docs, 0.0);
        receive_browser_event(&mut bundle_ui, &nodes, &closed, 0.0);
        assert_eq!(bundle_ui.portal.url(), "https://docs.example.org/guide", "the left-for page is an open website");

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
