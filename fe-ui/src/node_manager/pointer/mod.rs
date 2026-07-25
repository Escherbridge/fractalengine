//! Cross-authority pointer bridge (FR-3, `ui_shell_architecture_20260724`):
//! coordinates `NodeManager.selected` (Authority A, viewport) with
//! `PathEditorState` (Authority B, Paths tab) WITHOUT merging them — a viewport
//! track-select queues `UiAction::PathSelectTrack`, and petal entry/change
//! eager-loads the Paths-tab track list. Re-homed verbatim from `viewport_pick`
//! so this module is the single home of the bridge. See
//! `fe-ui/src/node_manager/AGENTS.md` §pointer-manager (+ §track-picking).

use bevy::prelude::*;

use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

/// Keeps `NodeManager.selected` and `PathEditorState.editing_track_id` — the
/// two halves of the selection model — in sync (see `node_manager/AGENTS.md`
/// §track-picking):
/// - selecting a listed track in the viewport opens it for editing
///   (`PathSelectTrack`); skipped when already editing it (buffer clobber);
/// - an explicit deselect (toolbar Deselect / empty click / Esc) also ends the
///   path-edit session;
/// - a Paths-tab selection becomes the viewport/inspector selection via
///   `pending_sidebar_select` (only when the track's entity is spawned, so the
///   sidebar resolver can't deselect-fallback and kill the fresh session);
/// - an active-petal change fully resets the editor (`respawn_on_petal_change`
///   cadence) so Pen clicks can't append to a foreign-petal track.
pub(super) fn open_track_on_select(
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<crate::gis::PathEditorState>,
    nav: Res<NavigationManager>,
    markers: Query<&SpawnedNodeMarker>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut last_selected: Local<Option<String>>,
    mut last_editing: Local<Option<String>>,
    mut last_petal: Local<Option<String>>,
    mut petal_initialized: Local<bool>,
) {
    if advance_petal_tracking(
        &mut petal_initialized,
        &mut last_petal,
        nav.active_petal_id.as_deref(),
        &mut path_state,
        &mut ui_mgr,
    ) {
        *last_editing = None;
    }

    let current = manager.selected.as_ref().map(|s| s.node_id.clone());
    if current != *last_selected {
        let prev = std::mem::replace(&mut *last_selected, current.clone());
        match current {
            Some(node_id) => {
                if track_to_open(&node_id, &path_state) {
                    ui_mgr.push_action(crate::actions::UiAction::PathSelectTrack {
                        track_node_id: node_id,
                    });
                }
            }
            None => {
                if prev.is_some() && path_state.editing_track_id.is_some() {
                    path_state.stop_editing();
                }
            }
        }
    }

    let editing = path_state.editing_track_id.clone();
    if editing != *last_editing {
        *last_editing = editing.clone();
        if let Some(track_id) = editing {
            let already_selected =
                manager.selected.as_ref().map(|s| s.node_id.as_str()) == Some(track_id.as_str());
            if !already_selected
                && spawned_in_petal(&markers, &track_id, nav.active_petal_id.as_deref())
            {
                manager.pending_sidebar_select = Some(track_id);
            }
        }
    }
}

/// FR-2 (`ui_shell_architecture_20260724`): detects the petal-entry/change
/// transition (first-ever frame, or an actual petal switch) and — on either —
/// resets `path_state` (transitions only, matching prior behavior) and
/// eager-loads the Paths-tab track list for the new petal via the same
/// request idiom the Data window's Paths tab uses (`UiAction::PathQueryTracks`
/// → `actions::path::query_tracks`), so `track_to_open`'s gate is fed without
/// requiring that window to ever render (`panels/gis_panel.rs:108-116`'s
/// render-gated load becomes a redundant no-op once `tracks` is populated
/// here). Returns `true` when a transition happened (caller also clears
/// `last_editing` in that case). Deliberately separated from the
/// selection-sync half above so both halves are independently testable.
fn advance_petal_tracking(
    petal_initialized: &mut bool,
    last_petal: &mut Option<String>,
    active_petal: Option<&str>,
    path_state: &mut crate::gis::PathEditorState,
    ui_mgr: &mut crate::actions::UiManager,
) -> bool {
    let active_owned = active_petal.map(str::to_string);
    if !*petal_initialized {
        *last_petal = active_owned;
        *petal_initialized = true;
        request_track_list_refresh(ui_mgr, active_petal, path_state.tracks_pending);
        return true;
    }
    if *last_petal != active_owned {
        *last_petal = active_owned;
        path_state.reset_for_petal_change();
        request_track_list_refresh(ui_mgr, active_petal, path_state.tracks_pending);
        return true;
    }
    false
}

/// Queues `UiAction::PathQueryTracks` for `active_petal`, unless there's no
/// active petal or a track-list request is already in flight (avoids a
/// duplicate `RawQuery` racing the Paths tab's own auto-populate in
/// `gis_panel.rs`, e.g. if it's already open on the transition frame).
fn request_track_list_refresh(
    ui_mgr: &mut crate::actions::UiManager,
    active_petal: Option<&str>,
    tracks_pending: bool,
) {
    if tracks_pending {
        return;
    }
    if let Some(petal_id) = active_petal {
        ui_mgr.push_action(crate::actions::UiAction::PathQueryTracks {
            petal_id: petal_id.to_string(),
        });
    }
}

/// `true` when a spawned entity for `node_id` exists in the active petal.
fn spawned_in_petal(
    markers: &Query<&SpawnedNodeMarker>,
    node_id: &str,
    active_petal: Option<&str>,
) -> bool {
    markers.iter().any(|m| {
        m.node_id == node_id
            && active_petal
                .map(|pid| pid == m.petal_id.as_str())
                .unwrap_or(true)
    })
}

/// `true` when `node_id` names a Paths-tab track that isn't already the one
/// being edited — i.e. selecting it should open it for editing. Pure so the
/// membership/guard logic is unit-testable without a Bevy App.
fn track_to_open(node_id: &str, path_state: &crate::gis::PathEditorState) -> bool {
    if path_state.editing_track_id.as_deref() == Some(node_id) {
        return false; // already editing this track — don't re-open (clobbers buffer)
    }
    path_state.tracks.iter().any(|t| t.node_id == node_id)
}

// ---------------------------------------------------------------------------
// Tests — the cross-authority bridge (Bevy-App-free pure helpers).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;
    use crate::actions::{UiAction, UiManager};
    use crate::gis::{GisResultRow, PathEditorState};

    fn track_row(node_id: &str) -> GisResultRow {
        GisResultRow {
            node_id: node_id.to_string(),
            name: node_id.to_string(),
            position: [0.0, 0.0, 0.0],
            annotation_title: None,
            annotation_color: None,
        }
    }

    #[test]
    fn track_to_open_true_for_listed_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a"), track_row("track-b")];
        assert!(track_to_open("track-a", &state));
        assert!(track_to_open("track-b", &state));
    }

    #[test]
    fn track_to_open_false_for_unlisted_node() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        assert!(!track_to_open("some-plain-node", &state));
    }

    #[test]
    fn track_to_open_false_when_already_editing_that_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        state.editing_track_id = Some("track-a".to_string());
        // Already editing it — re-opening would clobber the point buffer.
        assert!(!track_to_open("track-a", &state));
    }

    #[test]
    fn track_to_open_true_when_editing_a_different_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a"), track_row("track-b")];
        state.editing_track_id = Some("track-a".to_string());
        // Switching to a different listed track should open it.
        assert!(track_to_open("track-b", &state));
    }

    // --- FR-2 eager track-list load (petal change ⇒ track-list request) ---

    fn drained_petal_ids(ui_mgr: &mut UiManager) -> Vec<String> {
        ui_mgr
            .drain_actions()
            .into_iter()
            .filter_map(|a| match a {
                UiAction::PathQueryTracks { petal_id } => Some(petal_id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn request_track_list_refresh_queues_for_active_petal() {
        let mut ui_mgr = UiManager::default();
        request_track_list_refresh(&mut ui_mgr, Some("petal-1"), false);
        assert_eq!(drained_petal_ids(&mut ui_mgr), vec!["petal-1".to_string()]);
    }

    #[test]
    fn request_track_list_refresh_noop_without_active_petal() {
        let mut ui_mgr = UiManager::default();
        request_track_list_refresh(&mut ui_mgr, None, false);
        assert!(drained_petal_ids(&mut ui_mgr).is_empty());
    }

    #[test]
    fn request_track_list_refresh_noop_while_already_pending() {
        let mut ui_mgr = UiManager::default();
        request_track_list_refresh(&mut ui_mgr, Some("petal-1"), true);
        assert!(drained_petal_ids(&mut ui_mgr).is_empty());
    }

    #[test]
    fn advance_petal_tracking_fires_exactly_once_per_transition() {
        let mut ui_mgr = UiManager::default();
        let mut path_state = PathEditorState::default();
        let mut petal_initialized = false;
        let mut last_petal: Option<String> = None;

        // Frame 1: cold start, no active petal yet — no request (nothing to load).
        advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            None,
            &mut path_state,
            &mut ui_mgr,
        );
        // Frame 2: navigate into petal "p1" — one request.
        advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            Some("p1"),
            &mut path_state,
            &mut ui_mgr,
        );
        // Frame 3: same petal — must NOT re-fire (this is the "no spam while
        // the tab stays open" guarantee: the transition-only Local check).
        advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            Some("p1"),
            &mut path_state,
            &mut ui_mgr,
        );
        // Frame 4: switch to petal "p2" — one more request.
        advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            Some("p2"),
            &mut path_state,
            &mut ui_mgr,
        );

        assert_eq!(
            drained_petal_ids(&mut ui_mgr),
            vec!["p1".to_string(), "p2".to_string()]
        );
    }

    #[test]
    fn advance_petal_tracking_resets_session_only_on_real_transition() {
        let mut ui_mgr = UiManager::default();
        let mut path_state = PathEditorState::default();
        path_state.editing_track_id = Some("stale-track".to_string());
        let mut petal_initialized = false;
        let mut last_petal: Option<String> = None;

        // First-ever frame with an already-active petal must not wipe
        // pre-seeded state (matches the pre-existing `petal_initialized`
        // cold-start behavior).
        let changed = advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            Some("p1"),
            &mut path_state,
            &mut ui_mgr,
        );
        assert!(changed);
        assert_eq!(path_state.editing_track_id.as_deref(), Some("stale-track"));

        // A real transition to a different petal resets the edit session.
        let changed = advance_petal_tracking(
            &mut petal_initialized,
            &mut last_petal,
            Some("p2"),
            &mut path_state,
            &mut ui_mgr,
        );
        assert!(changed);
        assert!(path_state.editing_track_id.is_none());
    }
}
