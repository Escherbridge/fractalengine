//! Persisted visibility (`view.state` / `view.groups`, Phase 3): hydration
//! from DB results (the third `NodePropertiesLoaded`-fed cache idiom) and the
//! authed `SetNodeView` write. See `fe-ui/src/AGENTS.md` §visibility.

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{CallerAuth, DbCommand, NodeViewDto};

use super::{OverrideState, VisibilityState};
use crate::navigation_manager::NavigationManager;

use fe_database::handlers::node_view::{VIEW_GROUPS_KEY, VIEW_STATE_KEY};
/// Namespaces the GUI's own `SetNodeView` correlation ids.
const VIEW_WRITE_CORRELATION_PREFIX: &str = "fe-ui:view:";
/// Toast for a denied visibility write (ui_ux §6 — never fake persistence).
pub(crate) const VIEW_NOT_SAVED_DENIED_TOAST: &str =
    "Visibility not saved (you need Editor access)";
/// Toast for any other failed visibility write.
pub(crate) const VIEW_NOT_SAVED_TOAST: &str =
    "Visibility not saved — the change only lasts this session";

/// Wire form of an override (`"auto" | "show" | "hide"`).
pub(crate) fn wire_state(state: OverrideState) -> &'static str {
    match state {
        OverrideState::Auto => "auto",
        OverrideState::Show => "show",
        OverrideState::Hide => "hide",
    }
}

/// Parse a persisted state; unknown strings are ignored (`None`). Pure.
fn parse_state(raw: &str) -> Option<OverrideState> {
    match raw {
        "auto" => Some(OverrideState::Auto),
        "show" => Some(OverrideState::Show),
        "hide" => Some(OverrideState::Hide),
        _ => None,
    }
}

/// Apply a persisted override / membership through the epoch-bumping
/// mutators, only where it differs from the session value.
fn apply_view(
    state: &mut VisibilityState,
    node_id: &str,
    next_state: Option<OverrideState>,
    next_groups: Option<Vec<String>>,
) {
    if let Some(next) = next_state {
        if state.node_override(node_id) != next {
            state.set_node_override(node_id, next);
        }
    }
    if let Some(groups) = next_groups {
        let current = state
            .group_membership
            .get(node_id)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if current != groups.as_slice() {
            state.set_node_group_membership(node_id, groups);
        }
    }
}

fn groups_from_json(value: &serde_json::Value) -> Option<Vec<String>> {
    value.as_array().map(|ids| {
        ids.iter()
            .filter_map(|id| id.as_str().map(str::to_string))
            .collect()
    })
}

/// `NodePropertiesLoaded` hydration: a PRESENT `view.*` key is applied; an
/// absent one leaves the session value alone (it may hold a write the DB
/// refused — kept for the session, see AGENTS §visibility). Never touches solo.
pub(crate) fn hydrate_from_properties(
    state: &mut VisibilityState,
    node_id: &str,
    properties: &serde_json::Value,
) {
    let next_state = properties
        .get(VIEW_STATE_KEY)
        .and_then(|v| v.as_str())
        .and_then(parse_state);
    let next_groups = properties.get(VIEW_GROUPS_KEY).and_then(groups_from_json);
    apply_view(state, node_id, next_state, next_groups);
}

/// `PetalViewStateLoaded` hydration (petal entry / restart): same rule as
/// [`hydrate_from_properties`] for every listed node.
pub(crate) fn apply_petal_view_state(state: &mut VisibilityState, entries: &[NodeViewDto]) {
    for entry in entries {
        apply_view(
            state,
            &entry.node_id,
            entry.state.as_deref().and_then(parse_state),
            entry.groups.clone(),
        );
    }
}

/// `NodeViewSet` echo: the DB's read-back is authoritative for BOTH keys
/// (absent state = Auto, absent groups = none), from this GUI or any API caller.
pub(crate) fn apply_node_view_echo(
    state: &mut VisibilityState,
    node_id: &str,
    persisted_state: Option<&str>,
    persisted_groups: Option<&[String]>,
) {
    apply_view(
        state,
        node_id,
        Some(
            persisted_state
                .and_then(parse_state)
                .unwrap_or(OverrideState::Auto),
        ),
        Some(persisted_groups.map(<[String]>::to_vec).unwrap_or_default()),
    );
}

/// Correlation id for this GUI's `SetNodeView` of `node_id`.
pub(crate) fn view_write_correlation(node_id: &str) -> String {
    format!("{VIEW_WRITE_CORRELATION_PREFIX}{node_id}")
}

/// The node a failed GUI visibility write targeted, if `correlation_id` is one.
pub(crate) fn node_of_view_write(correlation_id: &str) -> Option<&str> {
    correlation_id.strip_prefix(VIEW_WRITE_CORRELATION_PREFIX)
}

/// Toast copy for a failed GUI visibility write, or `None` when the failure
/// belongs to some other request. The session override is kept either way.
pub(crate) fn view_write_failure_toast(correlation_id: &str, error: &str) -> Option<&'static str> {
    node_of_view_write(correlation_id)?;
    Some(if error.contains("denied") {
        VIEW_NOT_SAVED_DENIED_TOAST
    } else {
        VIEW_NOT_SAVED_TOAST
    })
}

/// `UiAction::SetNodeView`: optimistic session write, then the authed
/// `SetNodeView` (`CallerAuth::Local` — the DB resolves the real role, N-5).
pub(crate) fn set_node_view(
    db_sender: &DbCommandSender,
    state: &mut VisibilityState,
    node_id: String,
    next: OverrideState,
) {
    state.set_node_override(&node_id, next);
    let correlation_id = Some(view_write_correlation(&node_id));
    if db_sender
        .0
        .send(DbCommand::SetNodeView {
            node_id,
            state: Some(wire_state(next).to_string()),
            groups: None,
            auth: CallerAuth::Local,
            correlation_id,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetNodeView not persisted");
    }
}

/// Requests the active petal's persisted view state on petal entry/switch
/// (mirrors `terrain_map::load_petal_terrain_on_nav_change`).
pub(crate) fn load_petal_view_state_on_nav_change(
    nav: Res<NavigationManager>,
    db_sender: Res<DbCommandSender>,
    mut last_petal: Local<Option<String>>,
) {
    if *last_petal == nav.active_petal_id {
        return;
    }
    last_petal.clone_from(&nav.active_petal_id);
    let Some(petal_id) = nav.active_petal_id.clone() else {
        return;
    };
    if db_sender
        .0
        .send(DbCommand::LoadPetalViewState { petal_id })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — LoadPetalViewState not dispatched");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn property_hydration_applies_present_keys_bumps_epoch_and_leaves_solo() {
        let mut state = VisibilityState::default();
        state.set_solo(HashSet::from(["n9".to_string()]));
        let before = state.epoch();

        hydrate_from_properties(
            &mut state,
            "n1",
            &serde_json::json!({ "view.state": "hide", "view.groups": ["g1"] }),
        );

        assert_eq!(state.node_override("n1"), OverrideState::Hide);
        assert_eq!(
            state.group_membership.get("n1"),
            Some(&vec!["g1".to_string()])
        );
        assert!(state.epoch() > before, "the apply system must re-run");
        assert!(
            state.solo().is_some_and(|s| s.members.contains("n9")),
            "solo is session-only"
        );

        // Re-hydrating the same values is not a mutation (no spurious re-apply).
        let settled = state.epoch();
        hydrate_from_properties(
            &mut state,
            "n1",
            &serde_json::json!({ "view.state": "hide", "view.groups": ["g1"] }),
        );
        assert_eq!(state.epoch(), settled);
    }

    #[test]
    fn property_hydration_keeps_a_session_override_the_bag_does_not_mention() {
        // A refused write stays for the session; an unrelated property load
        // (no `view.state` key) must not silently revert it.
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        hydrate_from_properties(
            &mut state,
            "n1",
            &serde_json::json!({ "gis.track.name": "A" }),
        );
        assert_eq!(state.node_override("n1"), OverrideState::Hide);
    }

    #[test]
    fn petal_view_state_restores_overrides_after_a_restart() {
        let mut state = VisibilityState::default(); // fresh session
        apply_petal_view_state(
            &mut state,
            &[
                NodeViewDto {
                    node_id: "n1".into(),
                    state: Some("hide".into()),
                    groups: None,
                },
                NodeViewDto {
                    node_id: "n2".into(),
                    state: Some("show".into()),
                    groups: Some(vec!["g1".into()]),
                },
            ],
        );
        assert_eq!(state.node_override("n1"), OverrideState::Hide);
        assert_eq!(state.node_override("n2"), OverrideState::Show);
        assert_eq!(
            state.group_membership.get("n2"),
            Some(&vec!["g1".to_string()])
        );
        assert!(state.solo().is_none());
    }

    #[test]
    fn the_write_echo_is_authoritative_absent_state_means_auto() {
        let mut state = VisibilityState::default();
        state.set_node_override("n1", OverrideState::Hide);
        state.set_node_group_membership("n1", vec!["g1".into()]);
        apply_node_view_echo(&mut state, "n1", None, None);
        assert_eq!(state.node_override("n1"), OverrideState::Auto);
        assert!(!state.group_membership.contains_key("n1"));
    }

    #[test]
    fn set_node_view_writes_the_session_and_sends_the_authed_command() {
        let (tx, rx) = crossbeam::channel::bounded(4);
        let mut state = VisibilityState::default();

        set_node_view(
            &DbCommandSender(tx),
            &mut state,
            "n1".into(),
            OverrideState::Hide,
        );

        assert_eq!(state.node_override("n1"), OverrideState::Hide, "optimistic");
        match rx.try_recv() {
            Ok(DbCommand::SetNodeView {
                node_id,
                state: wire,
                groups,
                auth,
                correlation_id,
            }) => {
                assert_eq!(node_id, "n1");
                assert_eq!(wire.as_deref(), Some("hide"));
                assert_eq!(groups, None, "membership is untouched");
                assert_eq!(auth, CallerAuth::Local, "N-5: the UI asserts no role");
                let correlation_id = correlation_id.expect("GUI writes are correlated");
                assert_eq!(node_of_view_write(&correlation_id), Some("n1"));
            }
            other => panic!("expected SetNodeView, got {other:?}"),
        }
    }

    #[test]
    fn only_this_guis_visibility_failures_raise_the_not_saved_toast() {
        let ours = view_write_correlation("n1");
        assert_eq!(
            view_write_failure_toast(&ours, "SetNodeView denied for did:x: role below editor"),
            Some(VIEW_NOT_SAVED_DENIED_TOAST)
        );
        assert_eq!(
            view_write_failure_toast(&ours, "SetNodeView: no live node n1"),
            Some(VIEW_NOT_SAVED_TOAST)
        );
        assert_eq!(view_write_failure_toast("api-42", "denied"), None);
    }
}
