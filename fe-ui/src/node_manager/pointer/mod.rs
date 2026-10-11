//! Cross-authority pointer bridge (FR-3, `ui_shell_architecture_20260724`):
//! coordinates `NodeManager.selected` (Authority A, viewport) with
//! `PathEditorState` (Authority B, Paths tab) WITHOUT merging them — a viewport
//! track-select queues `UiAction::PathSelectTrack`, and petal entry/change
//! eager-loads the Paths-tab track list. Re-homed verbatim from `viewport_pick`
//! so this module is the single home of the bridge — all FOUR arms live here,
//! and `PathEditorState::stop_editing` itself stays authority-B-only. See
//! `fe-ui/src/node_manager/AGENTS.md` §pointer-manager (+ §track-picking).

use bevy::prelude::*;

use super::path_segment_interaction::TrackPickShape;
use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;
use crate::visibility::{pickable, VisibilityState};

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
/// - closing a session (B→None) drops the viewport selection of that same
///   track — the one-press back-out, arm 4 (see `drop_selection_of_closed_track`);
/// - an active-petal change fully resets the editor (`respawn_on_petal_change`
///   cadence) so Pen clicks can't append to a foreign-petal track;
/// - a track ribbon selected while the track list is still loading cannot open
///   — that no-op is surfaced once per petal load (toast + warn, ui_ux §6);
/// - an effectively-hidden track can neither open nor stay open (D-7/D-12:
///   "cannot open, must close").
#[allow(clippy::too_many_arguments)] // one bridge system; each param is one authority or read
pub(super) fn open_track_on_select(
    mut manager: ResMut<NodeManager>,
    mut path_state: ResMut<crate::gis::PathEditorState>,
    nav: Res<NavigationManager>,
    markers: Query<&SpawnedNodeMarker>,
    pick_shapes: Query<&TrackPickShape>,
    time: Res<Time>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    visibility: Res<VisibilityState>,
    mut last_selected: Local<Option<String>>,
    mut last_editing: Local<Option<String>>,
    mut last_petal: Local<Option<String>>,
    mut petal_initialized: Local<bool>,
    mut warned_this_load: Local<bool>,
) {
    if advance_petal_tracking(
        &mut petal_initialized,
        &mut last_petal,
        nav.active_petal_id.as_deref(),
        &mut path_state,
        &mut ui_mgr,
    ) {
        *last_editing = None;
        *warned_this_load = false;
    }

    // D-7/D-12: a session whose track became hidden closes (arm 4 below then
    // drops the matching viewport selection on the same frame).
    if path_state
        .editing_track_id
        .as_deref()
        .is_some_and(|track| !pickable(track, &nav, &visibility))
    {
        path_state.stop_editing();
    }

    let current = manager.selected.as_ref().map(|s| s.node_id.clone());
    if current != *last_selected {
        let prev = std::mem::replace(&mut *last_selected, current.clone());
        match current {
            Some(node_id) => {
                if track_to_open(&node_id, &path_state, &nav, &visibility) {
                    ui_mgr.push_action(crate::actions::UiAction::PathSelectTrack {
                        track_node_id: node_id,
                    });
                } else {
                    // A hidden ribbon is not a loading race — never warn for it.
                    let is_track_shape = manager
                        .selected
                        .as_ref()
                        .is_some_and(|s| pick_shapes.contains(s.entity))
                        && pickable(&node_id, &nav, &visibility);
                    let listed = path_state.editing_track_id.as_deref() == Some(node_id.as_str())
                        || path_state.tracks.iter().any(|t| t.node_id == node_id);
                    if silent_track_pick_warning(
                        is_track_shape,
                        listed,
                        path_state.tracks_pending,
                        *warned_this_load,
                    ) {
                        *warned_this_load = true;
                        bevy::log::warn!(
                            "track {node_id} selected before the Paths list loaded; not opened for editing"
                        );
                        ui_mgr.show_toast(SILENT_TRACK_PICK_TOAST, time.elapsed_secs_f64());
                    }
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
        let closed = std::mem::replace(&mut *last_editing, editing.clone());
        match editing {
            Some(track_id) => {
                let already_selected = manager.selected.as_ref().map(|s| s.node_id.as_str())
                    == Some(track_id.as_str());
                if !already_selected
                    && spawned_in_petal(&markers, &track_id, nav.active_petal_id.as_deref())
                    && pickable(&track_id, &nav, &visibility)
                {
                    manager.pending_sidebar_select = Some(track_id);
                }
            }
            None => drop_selection_of_closed_track(&mut manager, closed.as_deref()),
        }
    }
}

/// Copy for the silent track-pick notice. It says to re-select because the
/// bridge reacts only to selection CHANGES (`node_manager/AGENTS.md` §track-picking).
pub(super) const SILENT_TRACK_PICK_TOAST: &str =
    "Paths are still loading — press Esc, then click the path again in a moment";

/// Whether a selection that did not open a track should surface the
/// still-loading notice: the entity is a track ribbon (`TrackPickShape`), it is
/// not yet in the Paths list, that list is in flight, and this petal load has
/// not warned yet. Ordinary node clicks never qualify. Pure.
fn silent_track_pick_warning(
    is_track_shape: bool,
    listed: bool,
    tracks_pending: bool,
    already_warned: bool,
) -> bool {
    is_track_shape && !listed && tracks_pending && !already_warned
}

/// Bridge arm 4 (B→None ⇒ A), ratified 2026-08-08 as the one-press back-out:
/// closing a path-edit session also drops the viewport selection when it still
/// points at THAT track, so staged-Escape rung 1 backs fully out of the object
/// instead of leaving rung 2 to fire on the same one. Scoped to the closed
/// track — a selection that has already moved elsewhere is left alone.
fn drop_selection_of_closed_track(manager: &mut NodeManager, closed_track_id: Option<&str>) {
    let Some(track_id) = closed_track_id else {
        return;
    };
    if manager.selected.as_ref().map(|s| s.node_id.as_str()) == Some(track_id) {
        manager.deselect();
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

/// `true` when `node_id` names a visible Paths-tab track that isn't already the
/// one being edited — i.e. selecting it should open it for editing. Pure so the
/// membership/guard logic is unit-testable without a Bevy App.
fn track_to_open(
    node_id: &str,
    path_state: &crate::gis::PathEditorState,
    nav: &NavigationManager,
    visibility: &VisibilityState,
) -> bool {
    if path_state.editing_track_id.as_deref() == Some(node_id) {
        return false; // already editing this track — don't re-open (clobbers buffer)
    }
    path_state.tracks.iter().any(|t| t.node_id == node_id) && pickable(node_id, nav, visibility)
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
        assert!(track_to_open(
            "track-a",
            &state,
            &NavigationManager::default(),
            &VisibilityState::default()
        ));
        assert!(track_to_open(
            "track-b",
            &state,
            &NavigationManager::default(),
            &VisibilityState::default()
        ));
    }

    #[test]
    fn track_to_open_false_for_unlisted_node() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        assert!(!track_to_open(
            "some-plain-node",
            &state,
            &NavigationManager::default(),
            &VisibilityState::default()
        ));
    }

    #[test]
    fn track_to_open_false_when_already_editing_that_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        state.editing_track_id = Some("track-a".to_string());
        // Already editing it — re-opening would clobber the point buffer.
        assert!(!track_to_open(
            "track-a",
            &state,
            &NavigationManager::default(),
            &VisibilityState::default()
        ));
    }

    #[test]
    fn track_to_open_refuses_a_hidden_listed_track() {
        use crate::visibility::OverrideState;
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        let nav = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let mut hidden_override = VisibilityState::default();
        hidden_override.set_node_override("track-a", OverrideState::Hide);
        let mut hidden_petal = VisibilityState::default();
        hidden_petal.toggle_petal_hidden("p1");
        let mut solo_elsewhere = VisibilityState::default();
        solo_elsewhere.set_solo(std::collections::HashSet::from(["n9".to_string()]));
        for (label, vis) in [
            ("own Hide override", hidden_override),
            ("hidden petal", hidden_petal),
            ("isolated away", solo_elsewhere),
        ] {
            assert!(!track_to_open("track-a", &state, &nav, &vis), "{label}");
        }
    }

    #[test]
    fn track_to_open_true_when_editing_a_different_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a"), track_row("track-b")];
        state.editing_track_id = Some("track-a".to_string());
        // Switching to a different listed track should open it.
        assert!(track_to_open(
            "track-b",
            &state,
            &NavigationManager::default(),
            &VisibilityState::default()
        ));
    }

    // --- arm 4: B→None ⇒ A (the ratified one-press back-out) ---

    fn manager_selecting(node_id: &str) -> NodeManager {
        let mut mgr = NodeManager::default();
        mgr.select(Entity::from_bits(7), node_id);
        mgr
    }

    #[test]
    fn closing_a_session_clears_the_viewport_selection_of_that_track() {
        // Esc rung 1 calls `stop_editing()`; this arm makes the SAME press also
        // drop Authority A, so rung 2 doesn't fire on the same object next press.
        let mut mgr = manager_selecting("track-a");
        drop_selection_of_closed_track(&mut mgr, Some("track-a"));
        assert!(!mgr.is_selected());
    }

    #[test]
    fn closing_a_session_leaves_a_selection_that_moved_elsewhere() {
        // The user closed track-a but has since selected some other node —
        // backing out of the session must not steal that selection.
        let mut mgr = manager_selecting("some-other-node");
        drop_selection_of_closed_track(&mut mgr, Some("track-a"));
        assert_eq!(
            mgr.selected.as_ref().map(|s| s.node_id.as_str()),
            Some("some-other-node")
        );
    }

    #[test]
    fn no_closed_track_is_a_noop() {
        let mut mgr = manager_selecting("track-a");
        drop_selection_of_closed_track(&mut mgr, None);
        assert!(mgr.is_selected());
        // …and an empty selection stays empty.
        let mut empty = NodeManager::default();
        drop_selection_of_closed_track(&mut empty, Some("track-a"));
        assert!(!empty.is_selected());
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

    // --- silent track-pick notice (FR-2 follow-up, ui_ux §6), real system ---

    /// The bridge system in a minimal App: petal `p1` entered, track list in
    /// flight. Returns the app plus a track-ribbon entity and a plain entity.
    fn loading_petal_app() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<NodeManager>()
            .init_resource::<PathEditorState>()
            .init_resource::<UiManager>()
            .init_resource::<VisibilityState>()
            .insert_resource(NavigationManager {
                active_petal_id: Some("p1".into()),
                ..Default::default()
            })
            .add_systems(Update, open_track_on_select);
        let ribbon = app
            .world_mut()
            .spawn(TrackPickShape {
                points: vec![Vec3::ZERO, Vec3::X],
                anchors: vec![Vec3::ZERO, Vec3::X],
                half_width: 0.5,
                closed: false,
                fill_triangles: Vec::new(),
                centroid: Vec3::ZERO,
            })
            .id();
        let plain = app.world_mut().spawn_empty().id();
        app.update(); // petal-entry frame
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .tracks_pending = true;
        (app, ribbon, plain)
    }

    /// Select `entity` as `node_id` for one frame, then deselect; returns
    /// whether that click raised the still-loading toast.
    fn click_shows_notice(app: &mut App, entity: Entity, node_id: &str) -> bool {
        app.world_mut()
            .resource_mut::<UiManager>()
            .show_toast("sentinel", 0.0);
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(entity, node_id);
        app.update();
        let shown = app
            .world()
            .resource::<UiManager>()
            .active_toast(0.0)
            .is_some_and(|(msg, _)| msg == SILENT_TRACK_PICK_TOAST);
        app.world_mut().resource_mut::<NodeManager>().deselect();
        app.update();
        shown
    }

    #[test]
    fn silent_track_pick_notice_fires_once_per_load_and_never_on_ordinary_clicks() {
        let (mut app, ribbon, plain) = loading_petal_app();
        assert!(
            !click_shows_notice(&mut app, plain, "plain-node"),
            "an ordinary node click never warns"
        );
        // Once the list has landed, an unlisted ribbon is not a loading race.
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .tracks_pending = false;
        assert!(!click_shows_notice(&mut app, ribbon, "track-new"));
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .tracks_pending = true;
        assert!(
            click_shows_notice(&mut app, ribbon, "track-a"),
            "first pick warns"
        );
        assert!(
            !click_shows_notice(&mut app, ribbon, "track-a"),
            "second pick in the same load is suppressed"
        );

        // A petal switch is a new load: the notice re-arms.
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .active_petal_id = Some("p2".into());
        app.update();
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .tracks_pending = true;
        assert!(click_shows_notice(&mut app, ribbon, "track-a"), "re-armed");

        // A listed track opens (PathSelectTrack) instead of warning.
        let (mut app, ribbon, _) = loading_petal_app();
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .tracks
            .push(track_row("track-a"));
        assert!(!click_shows_notice(&mut app, ribbon, "track-a"));
        assert!(app
            .world_mut()
            .resource_mut::<UiManager>()
            .drain_actions()
            .into_iter()
            .any(|a| matches!(a, UiAction::PathSelectTrack { track_node_id } if track_node_id == "track-a")));
    }

    // --- D-7/D-12: a hidden track cannot stay open (real system) ---

    #[test]
    fn hiding_the_edited_track_closes_its_session_and_drops_its_selection() {
        let (mut app, ribbon, _) = loading_petal_app();
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(ribbon, "track-a");
        app.world_mut()
            .resource_mut::<PathEditorState>()
            .start_editing("track-a".to_string());
        app.update();
        assert_eq!(
            app.world()
                .resource::<PathEditorState>()
                .editing_track_id
                .as_deref(),
            Some("track-a"),
            "visible: the session stays open"
        );

        app.world_mut()
            .resource_mut::<VisibilityState>()
            .set_node_override("track-a", crate::visibility::OverrideState::Hide);
        app.update();

        assert!(app
            .world()
            .resource::<PathEditorState>()
            .editing_track_id
            .is_none());
        assert!(
            !app.world().resource::<NodeManager>().is_selected(),
            "arm 4 drops the closed track's selection on the same frame"
        );
    }
}
