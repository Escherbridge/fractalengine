//! Portal + URL-save action handling. This is the browser-integration seam:
//! keep `compute_open_portal` / `should_auto_close` / `compute_save_url` pure
//! so save/portal semantics stay unit-testable without a Bevy `App`. See
//! `fe-ui/src/AGENTS.md` §actions and §portal.

use bevy::prelude::Entity;

use crate::node_manager::NodeManager;
use crate::plugin::InspectorFormState;
use crate::portal::PortalState;

/// Result of attempting to open the portal for a raw URL string.
pub(crate) enum OpenPortalOutcome {
    /// URL parsed and a node is selected — apply this portal state and navigate.
    Navigate(PortalState, url::Url),
    /// URL parsed but nothing is selected. Matches original behavior: a silent
    /// no-op (no log, no state change) — see AGENTS.md §portal for why this is
    /// flagged as a fragility rather than fixed here.
    NoSelection,
    /// URL failed to parse; caller should log a warning with this message.
    InvalidUrl(String),
}

/// Pure: decide what `UiAction::OpenPortal` should do given current selection.
pub(crate) fn compute_open_portal(node_mgr: &NodeManager, raw_url: &str) -> OpenPortalOutcome {
    match raw_url.parse::<url::Url>() {
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
    let PortalState::Open { opened_for_entity, .. } = portal else {
        return false;
    };
    let entity_changed = selected != Some(*opened_for_entity);
    selected.is_none() || entity_changed
}

/// Pure: compute `(node_id, url)` to persist for `UiAction::SaveUrl`, or
/// `None` if nothing is selected (matches original silent no-op behavior).
/// Empty/whitespace-only `external_url` maps to `None` (clears the URL);
/// otherwise the buffer is stored as-is (not trimmed) — see AGENTS.md §portal
/// for the whitespace-preservation caveat.
pub(crate) fn compute_save_url(
    node_mgr: &NodeManager,
    inspector: &InspectorFormState,
) -> Option<(String, Option<String>)> {
    let node_id = node_mgr.selected.as_ref().map(|s| s.node_id.clone())?;
    let url = if inspector.external_url.trim().is_empty() {
        None
    } else {
        Some(inspector.external_url.clone())
    };
    Some((node_id, url))
}

#[cfg(test)]
mod tests {
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
            OpenPortalOutcome::Navigate(PortalState::Open { opened_for_entity, cached_hostname, .. }, url) => {
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
    }

    #[test]
    fn compute_save_url_no_selection_is_noop() {
        let mgr = NodeManager::default();
        let inspector = InspectorFormState::default();
        assert!(compute_save_url(&mgr, &inspector).is_none());
    }

    #[test]
    fn compute_save_url_empty_buffer_maps_to_none() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        let mut inspector = InspectorFormState::default();
        inspector.external_url = "   ".to_string();
        let (node_id, url) = compute_save_url(&mgr, &inspector).expect("selection present");
        assert_eq!(node_id, "node-1");
        assert_eq!(url, None);
    }

    #[test]
    fn compute_save_url_populated_buffer_maps_to_some() {
        let mut mgr = NodeManager::default();
        mgr.select(entity(1), "node-1");
        let mut inspector = InspectorFormState::default();
        inspector.external_url = "https://example.com".to_string();
        let (node_id, url) = compute_save_url(&mgr, &inspector).expect("selection present");
        assert_eq!(node_id, "node-1");
        assert_eq!(url, Some("https://example.com".to_string()));
    }
}
