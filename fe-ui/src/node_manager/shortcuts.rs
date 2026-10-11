//! Keyboard shortcuts: tool switching (bindings from `panels::toolbar::TOOL_DEFS`)
//! and THE staged Escape ladder — one place, one ordering, every tool. No tool
//! may read Escape privately. See `node_manager/AGENTS.md` §staged-escape.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

use super::GestureParams;
use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::panels::toolbar::TOOL_DEFS;
use crate::plugin::ToolState;
use crate::ui_shell::right_sidebar::RightSidebarState;
use crate::verse_manager::VerseManager;
use crate::visibility::{self, NodeAncestry, OverrideState, VisibilityState};

/// Bare `B` activates Brush; `Ctrl/Cmd+B` stays the left-sidebar toggle.
fn shortcut_allowed(tool: Tool, command_modifier: bool) -> bool {
    tool != Tool::Brush || !command_modifier
}

/// The H-key visibility verbs (hierarchy_visibility_groups D-17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityKey {
    /// `H` — hide the selection (persisted override).
    HideSelection,
    /// `Shift+H` — show everything in the active branch.
    ShowAll,
    /// `Alt+H` — isolate the selection (session-only solo).
    IsolateSelection,
}

/// Map an `H` press's modifiers to its verb; `Ctrl/Cmd+H` and `Shift+Alt+H`
/// are unbound. Pure.
fn visibility_key(shift: bool, alt: bool, command_modifier: bool) -> Option<VisibilityKey> {
    match (command_modifier, shift, alt) {
        (true, _, _) | (false, true, true) => None,
        (false, false, false) => Some(VisibilityKey::HideSelection),
        (false, true, false) => Some(VisibilityKey::ShowAll),
        (false, false, true) => Some(VisibilityKey::IsolateSelection),
    }
}

/// Apply one H-key verb. Hide persists through `UiAction::SetNodeView` (the
/// sync system force-deselects once it lands); Show all persists the Hide
/// overrides it clears; Isolate is session-only.
fn apply_visibility_key(
    key: VisibilityKey,
    selected_node_id: Option<&str>,
    ancestry: NodeAncestry,
    petal_node_ids: &[&str],
    vis: &mut VisibilityState,
    ui_mgr: &mut UiManager,
) {
    match (key, selected_node_id) {
        // A stamp instance has no node row: its hide is session-only (Shift+H
        // restores it with its track).
        (VisibilityKey::HideSelection, Some(node_id))
            if crate::verse_manager::parse_stamp_marker_id(node_id).is_some() =>
        {
            vis.set_node_override(node_id, OverrideState::Hide);
        }
        (VisibilityKey::HideSelection, Some(node_id)) => {
            ui_mgr.push_action(UiAction::SetNodeView {
                node_id: node_id.to_string(),
                state: OverrideState::Hide,
            })
        }
        (VisibilityKey::IsolateSelection, Some(node_id)) => {
            vis.set_solo(std::collections::HashSet::from([node_id.to_string()]));
        }
        (VisibilityKey::ShowAll, _) => {
            visibility::show_all_and_persist(vis, ancestry, petal_node_ids.iter().copied(), ui_mgr);
        }
        (VisibilityKey::HideSelection | VisibilityKey::IsolateSelection, None) => {}
    }
}

/// Which rung an Escape press fires. Exactly one rung fires per press — the
/// highest applicable one consumes the key (D2/D3/D4-override).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeRung {
    /// 0 — cancel ONLY the in-flight viewport gesture.
    CancelGesture,
    /// 0.5 — clear an active isolate/solo lens (D-17). Below a live gesture
    /// (F5: a drag must still be rung 0), above every session/selection rung.
    ClearIsolate,
    /// 1 — close the path-edit session. The pointer bridge clears the matching
    /// viewport selection in the same frame, so this is a one-press back-out.
    StopEditing,
    /// 2 — clear the viewport selection.
    Deselect,
    /// 3 — return the active tool to Select.
    ResetTool,
    /// Fully at rest: nothing left to back out of.
    Rest,
}

/// Resolve the staged-Escape rung. Pure — the ORDERING is the semantic, so it
/// is unit-tested without a Bevy App.
fn escape_rung(
    gesture_active: bool,
    solo_active: bool,
    editing_track: bool,
    has_selection: bool,
    tool_is_select: bool,
) -> EscapeRung {
    if gesture_active {
        EscapeRung::CancelGesture
    } else if solo_active {
        EscapeRung::ClearIsolate
    } else if editing_track {
        EscapeRung::StopEditing
    } else if has_selection {
        EscapeRung::Deselect
    } else if !tool_is_select {
        EscapeRung::ResetTool
    } else {
        EscapeRung::Rest
    }
}

/// `NodeManager` is reached through [`GestureParams`], never as a second param:
/// the entity gimbal's `AxisDrag` is one of the six gestures the bundle
/// aggregates, and two `ResMut<NodeManager>` on one system is a Bevy
/// param-aliasing panic (finding F5).
#[allow(clippy::too_many_arguments)] // one keyboard system: tools, H-keys, Escape ladder
pub(super) fn handle_tool_shortcuts(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tool: ResMut<ToolState>,
    mut path_state: ResMut<PathEditorState>,
    mut right_sidebar: ResMut<RightSidebarState>,
    mut gestures: GestureParams,
    mut egui_ctx: EguiContexts,
    mut visibility: ResMut<VisibilityState>,
    nav: Res<NavigationManager>,
    hierarchy: Res<VerseManager>,
    mut ui_mgr: ResMut<UiManager>,
) {
    // Mirror the gesture aggregate onto `NodeManager` first — even on the egui
    // early-return below — so the topbar's `InputContext` stash (and thus the
    // viewport's right-click rule) sees a value written this frame.
    //
    // It is a value from this frame, but computed BEFORE any gesture system has
    // run, so it lags them in BOTH directions (F16 — the older comment claimed
    // only the safe one):
    //   * stale-FALSE on the frame a gesture STARTS — a right-click landing in
    //     that same frame still opens the object menu, and Escape that frame
    //     resolves at a lower rung. This is the unsafe direction.
    //   * stale-TRUE on the frame a gesture RELEASES — one extra frame of menu
    //     suppression, harmless.
    // Only the rung-0 cancel below is exact: it writes `false` in this pass.
    let gesture_active = gestures.any_active();
    if gestures.node_mgr.gesture_active != gesture_active {
        gestures.node_mgr.gesture_active = gesture_active;
    }

    let egui_wants_kb = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.wants_keyboard_input())
        .unwrap_or(false);
    if egui_wants_kb {
        return;
    }

    let command_modifier = keyboard.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    for def in &TOOL_DEFS {
        if keyboard.just_pressed(def.key_code) && shortcut_allowed(def.tool, command_modifier) {
            tool.activate(def.tool, &mut right_sidebar);
            return;
        }
    }

    if keyboard.just_pressed(KeyCode::KeyH) {
        let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        if let Some(key) = visibility_key(shift, alt, command_modifier) {
            let petal_node_ids: Vec<&str> = nav
                .active_petal_id
                .as_deref()
                .and_then(|pid| hierarchy.find_petal(pid))
                .map(|petal| petal.nodes.iter().map(|n| n.id.as_str()).collect())
                .unwrap_or_default();
            let selected = gestures
                .node_mgr
                .selected
                .as_ref()
                .map(|s| s.node_id.as_str());
            apply_visibility_key(
                key,
                selected,
                visibility::active_ancestry(&nav),
                &petal_node_ids,
                &mut visibility,
                &mut ui_mgr,
            );
            return;
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        match escape_rung(
            gesture_active,
            visibility.solo().is_some(),
            path_state.editing_track_id.is_some(),
            gestures.node_mgr.selected.is_some(),
            tool.active_tool == Tool::Select,
        ) {
            // This system runs FIRST in the chain, so the gesture systems see
            // the cleared resource on this very frame (no one-frame replay).
            EscapeRung::CancelGesture => {
                gestures.cancel_all();
                gestures.node_mgr.gesture_active = false;
            }
            EscapeRung::ClearIsolate => visibility.clear_solo(),
            EscapeRung::StopEditing => path_state.stop_editing(),
            EscapeRung::Deselect => gestures.node_mgr.deselect(),
            EscapeRung::ResetTool => tool.activate(Tool::Select, &mut right_sidebar),
            EscapeRung::Rest => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_uses_bare_b_without_stealing_sidebar_toggle() {
        assert!(shortcut_allowed(Tool::Brush, false));
        assert!(!shortcut_allowed(Tool::Brush, true));
        assert!(shortcut_allowed(Tool::Pen, true));
    }

    // --- staged Escape ladder: the ordering IS the contract ---

    #[test]
    fn rung0_gesture_wins_over_every_lower_rung() {
        // A live gesture consumes Escape even when a track is open, a node is
        // selected, AND a non-Select tool is active.
        assert_eq!(
            escape_rung(true, false, true, true, false),
            EscapeRung::CancelGesture
        );
        assert_eq!(
            escape_rung(true, false, false, false, true),
            EscapeRung::CancelGesture
        );
    }

    #[test]
    fn a_live_entity_gimbal_drag_pins_escape_at_rung_zero() {
        // F5: the entity gimbal's `AxisDrag` reaches `escape_rung` through
        // `GestureParams::any_active` → `NodeManager::is_dragging`. Before it
        // joined the aggregate, Escape mid-transform-drag fell to rung 2 and
        // deselected — abandoning the live-applied Transform mid-drag.
        use crate::node_manager::{AxisDrag, NodeManager};

        let mut manager = NodeManager::default();
        manager.select(Entity::from_bits(1), "n1");
        // Selected, not dragging, non-Select tool → plain deselect.
        assert_eq!(
            escape_rung(manager.is_dragging(), false, false, true, false),
            EscapeRung::Deselect
        );
        if let Some(ref mut sel) = manager.selected {
            sel.drag = Some(AxisDrag {
                axis: crate::gimbal::GimbalAxis::Y,
                start_cursor: Vec2::ZERO,
                axis_screen_dir: Vec2::X,
                start_pos: Vec3::ZERO,
                start_rot: Quat::IDENTITY,
                start_scale: Vec3::ONE,
            });
        }
        assert_eq!(
            escape_rung(manager.is_dragging(), false, false, true, false),
            EscapeRung::CancelGesture
        );
        // …and the cancel restores the press-time transform instead of
        // dropping the selection (`GestureParams::cancel_all` writes it back).
        let (_, restore) = manager.cancel_axis_drag().expect("drag was live");
        assert_eq!(restore.translation, Vec3::ZERO);
        assert!(manager.is_selected());
    }

    #[test]
    fn rung1_stop_editing_wins_over_deselect_and_tool_reset() {
        assert_eq!(
            escape_rung(false, false, true, true, false),
            EscapeRung::StopEditing
        );
    }

    #[test]
    fn rung2_deselect_wins_over_tool_reset() {
        assert_eq!(
            escape_rung(false, false, false, true, false),
            EscapeRung::Deselect
        );
    }

    #[test]
    fn rung3_resets_a_non_select_tool_when_nothing_else_is_pending() {
        // Fixes the sticky-Pen complaint (#12).
        assert_eq!(
            escape_rung(false, false, false, false, false),
            EscapeRung::ResetTool
        );
    }

    #[test]
    fn fully_at_rest_escape_does_nothing() {
        assert_eq!(
            escape_rung(false, false, false, false, true),
            EscapeRung::Rest
        );
    }

    #[test]
    fn ladder_is_the_same_for_brush_as_for_every_other_tool() {
        // Regression guard for the deleted Brush short-circuit (#6): the rung
        // resolution takes no `Tool` at all, so Brush cannot swallow Escape.
        // Pressing Escape three times from "Brush + track open + selection"
        // walks the ladder down to the rest state.
        let mut editing = true;
        let mut selected = true;
        let mut tool_is_select = false;
        let mut walked = Vec::new();
        for _ in 0..4 {
            let rung = escape_rung(false, false, editing, selected, tool_is_select);
            walked.push(rung);
            match rung {
                // The bridge clears the selection alongside the session (D4
                // override), so ONE press backs out of both authorities.
                EscapeRung::StopEditing => {
                    editing = false;
                    selected = false;
                }
                EscapeRung::Deselect => selected = false,
                EscapeRung::ResetTool => tool_is_select = true,
                EscapeRung::CancelGesture | EscapeRung::ClearIsolate | EscapeRung::Rest => {}
            }
        }
        assert_eq!(
            walked,
            vec![
                EscapeRung::StopEditing,
                EscapeRung::ResetTool,
                EscapeRung::Rest,
                EscapeRung::Rest,
            ]
        );
    }

    #[test]
    fn without_the_one_press_back_out_the_ladder_would_need_two_presses() {
        // Documents WHAT D4-override buys: if `stop_editing` left the track
        // selected, rung 2 would fire on the next press for the same object.
        assert_eq!(
            escape_rung(false, false, true, true, true),
            EscapeRung::StopEditing
        );
        assert_eq!(
            escape_rung(false, false, false, true, true),
            EscapeRung::Deselect
        );
    }

    // --- D-17: Esc clears isolate first — right after a live gesture ---

    #[test]
    fn clear_isolate_sits_between_a_live_gesture_and_every_session_rung() {
        // A live drag still owns rung 0 even while isolated (F5).
        assert_eq!(
            escape_rung(true, true, true, true, false),
            EscapeRung::CancelGesture
        );
        // Isolate outranks an open track, a selection and a non-Select tool.
        assert_eq!(
            escape_rung(false, true, true, true, false),
            EscapeRung::ClearIsolate
        );
        // Walking the ladder from "isolated + track open + Pen": isolate goes
        // first, then the one-press back-out, then the tool reset.
        let (mut solo, mut editing, mut selected, mut tool_is_select) = (true, true, true, false);
        let mut walked = Vec::new();
        for _ in 0..4 {
            let rung = escape_rung(false, solo, editing, selected, tool_is_select);
            walked.push(rung);
            match rung {
                EscapeRung::ClearIsolate => solo = false,
                EscapeRung::StopEditing => (editing, selected) = (false, false),
                EscapeRung::Deselect => selected = false,
                EscapeRung::ResetTool => tool_is_select = true,
                EscapeRung::CancelGesture | EscapeRung::Rest => {}
            }
        }
        assert_eq!(
            walked,
            vec![
                EscapeRung::ClearIsolate,
                EscapeRung::StopEditing,
                EscapeRung::ResetTool,
                EscapeRung::Rest,
            ]
        );
    }

    // --- D-17: H / Shift+H / Alt+H ---

    #[test]
    fn h_key_modifiers_map_to_exactly_one_verb() {
        let table = [
            ((false, false, false), Some(VisibilityKey::HideSelection)),
            ((true, false, false), Some(VisibilityKey::ShowAll)),
            ((false, true, false), Some(VisibilityKey::IsolateSelection)),
            ((true, true, false), None),
            ((false, false, true), None), // Ctrl/Cmd+H stays unbound
        ];
        for ((shift, alt, command), expected) in table {
            assert_eq!(
                visibility_key(shift, alt, command),
                expected,
                "{shift} {alt} {command}"
            );
        }
    }

    const ANCESTRY: NodeAncestry<'static> = NodeAncestry {
        verse_id: "v1",
        fractal_id: "f1",
        petal_id: "p1",
    };

    fn queued_view_writes(ui_mgr: &mut UiManager) -> Vec<(String, OverrideState)> {
        ui_mgr
            .drain_actions()
            .into_iter()
            .filter_map(|a| match a {
                UiAction::SetNodeView { node_id, state } => Some((node_id, state)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn h_hides_the_selection_through_the_persisted_write_and_is_inert_without_one() {
        let (mut vis, mut ui_mgr) = (VisibilityState::default(), UiManager::default());
        let nodes = ["n1", "n2"];
        apply_visibility_key(
            VisibilityKey::HideSelection,
            Some("n1"),
            ANCESTRY,
            &nodes,
            &mut vis,
            &mut ui_mgr,
        );
        assert_eq!(
            queued_view_writes(&mut ui_mgr),
            vec![("n1".to_string(), OverrideState::Hide)]
        );
        apply_visibility_key(
            VisibilityKey::HideSelection,
            None,
            ANCESTRY,
            &nodes,
            &mut vis,
            &mut ui_mgr,
        );
        apply_visibility_key(
            VisibilityKey::IsolateSelection,
            None,
            ANCESTRY,
            &nodes,
            &mut vis,
            &mut ui_mgr,
        );
        assert!(queued_view_writes(&mut ui_mgr).is_empty());
        assert!(vis.solo().is_none());

        // A stamp instance hides for the session only — no row to persist to.
        apply_visibility_key(
            VisibilityKey::HideSelection,
            Some("n1::stamp::2"),
            ANCESTRY,
            &nodes,
            &mut vis,
            &mut ui_mgr,
        );
        assert!(queued_view_writes(&mut ui_mgr).is_empty());
        assert_eq!(vis.node_override("n1::stamp::2"), OverrideState::Hide);
    }

    #[test]
    fn alt_h_isolates_the_selection_for_the_session_only() {
        let (mut vis, mut ui_mgr) = (VisibilityState::default(), UiManager::default());
        apply_visibility_key(
            VisibilityKey::IsolateSelection,
            Some("n2"),
            ANCESTRY,
            &["n1", "n2"],
            &mut vis,
            &mut ui_mgr,
        );
        let solo = vis.solo().expect("isolate is active");
        assert_eq!(
            solo.members,
            std::collections::HashSet::from(["n2".to_string()])
        );
        assert!(
            queued_view_writes(&mut ui_mgr).is_empty(),
            "solo is never persisted"
        );
    }

    #[test]
    fn shift_h_shows_everything_and_persists_only_the_hides_it_cleared() {
        let (mut vis, mut ui_mgr) = (VisibilityState::default(), UiManager::default());
        vis.set_node_override("n1", OverrideState::Hide);
        vis.toggle_petal_hidden("p1");
        vis.set_solo(std::collections::HashSet::from(["n2".to_string()]));

        apply_visibility_key(
            VisibilityKey::ShowAll,
            None,
            ANCESTRY,
            &["n1", "n2"],
            &mut vis,
            &mut ui_mgr,
        );

        assert_eq!(
            queued_view_writes(&mut ui_mgr),
            vec![("n1".to_string(), OverrideState::Auto)]
        );
        assert!(vis.solo().is_none());
        assert!(visibility::effective_visibility("n2", ANCESTRY, &vis));
    }
}
