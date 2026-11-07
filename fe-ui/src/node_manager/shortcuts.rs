//! Keyboard shortcuts: tool switching (bindings from `panels::toolbar::TOOL_DEFS`)
//! and THE staged Escape ladder — one place, one ordering, every tool. No tool
//! may read Escape privately. See `node_manager/AGENTS.md` §staged-escape.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use super::{GestureParams, NodeSelection};
use crate::actions::{UiAction, UiManager};
use crate::dialogs::{node_options_prefill, ActiveDialog};
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::panels::toolbar::TOOL_DEFS;
use crate::plugin::{CameraFocusTarget, LocalUserRole, SpawnedNodeMarker, ToolState};
use crate::ui_shell::right_sidebar::RightSidebarState;
use crate::verse_manager::{NodeEntry, VerseManager};
use crate::visibility::{self, NodeAncestry, OverrideState, VisibilityState};
use fe_runtime::{app::DbCommandSender, messages::DbCommand};

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct UndoShortcutContext<'w> {
    undo: ResMut<'w, crate::undo::UndoState>,
    topbar: Option<Res<'w, crate::ui_shell::topbar::TopbarState>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    Activate(Tool),
    DeleteSelection,
    FrameSelection,
    ToggleRightSidebar,
    Undo,
    Redo,
}

impl Shortcut {
    fn requires_edit(self) -> bool {
        match self {
            Self::Activate(tool) => tool.requires_edit(),
            Self::DeleteSelection | Self::Undo | Self::Redo => true,
            _ => false,
        }
    }
}

/// Decode shortcuts once; command/Alt chords never fall through to plain tool keys.
fn shortcut(keyboard: &ButtonInput<KeyCode>, keyboard_owned: bool) -> Option<Shortcut> {
    if keyboard_owned {
        return None;
    }
    let command = keyboard.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if command && shift && !alt && keyboard.just_pressed(KeyCode::KeyB) {
        return Some(Shortcut::ToggleRightSidebar);
    }
    if command && !alt && keyboard.just_pressed(KeyCode::KeyZ) {
        return Some(if shift {
            Shortcut::Redo
        } else {
            Shortcut::Undo
        });
    }
    if command || alt {
        return None;
    }
    if keyboard.just_pressed(KeyCode::Delete) {
        return Some(Shortcut::DeleteSelection);
    }
    if keyboard.just_pressed(KeyCode::KeyF) {
        return Some(Shortcut::FrameSelection);
    }
    TOOL_DEFS
        .iter()
        .find(|def| keyboard.just_pressed(def.key_code))
        .map(|def| Shortcut::Activate(def.tool))
}

/// Advisory UI gate; backend authorization remains authoritative.
fn admit_shortcut(
    command: Shortcut,
    role: &LocalUserRole,
    nav: &NavigationManager,
    ui_mgr: &mut UiManager,
    now: f64,
) -> bool {
    if command.requires_edit() && !role.can_edit() {
        let reason = crate::panels::widgets::role_denial_reason(
            role,
            "Editor",
            crate::notice::active_verse_name(nav),
        );
        ui_mgr.show_advisory(reason, now);
        false
    } else {
        true
    }
}

/// Resolve selection against the spawned entity and the complete current navigation ancestry.
fn selected_node<'a>(
    selection: Option<&NodeSelection>,
    marker: Option<&SpawnedNodeMarker>,
    nav: &NavigationManager,
    hierarchy: &'a VerseManager,
) -> Option<&'a NodeEntry> {
    let selection = selection?;
    let marker = marker?;
    let petal_id = nav.active_petal_id.as_deref()?;
    if marker.node_id != selection.node_id || marker.petal_id != petal_id {
        return None;
    }
    hierarchy
        .verses
        .iter()
        .find(|verse| Some(verse.id.as_str()) == nav.active_verse_id.as_deref())?
        .fractals
        .iter()
        .find(|fractal| Some(fractal.id.as_str()) == nav.active_fractal_id.as_deref())?
        .petals
        .iter()
        .find(|petal| petal.id == petal_id)?
        .nodes
        .iter()
        .find(|node| node.id == selection.node_id)
}

/// Selection shortcuts use the existing confirmation and camera request paths.
fn apply_selection_shortcut(
    command: Shortcut,
    node: &NodeEntry,
    hierarchy: &VerseManager,
    focus: &mut CameraFocusTarget,
    ui_mgr: &mut UiManager,
    db: &crossbeam::channel::Sender<DbCommand>,
) -> Result<(), &'static str> {
    match command {
        Shortcut::FrameSelection => focus.target = Some((node.id.clone(), node.position)),
        Shortcut::DeleteSelection => {
            db.send(DbCommand::CountNodeDescendants {
                node_id: node.id.clone(),
            })
            .map_err(|_| {
                "Could not open delete confirmation: database connection is unavailable"
            })?;
            let mut dialog = node_options_prefill(hierarchy, &node.id);
            if let ActiveDialog::NodeOptions { pending_delete, .. } = &mut dialog {
                *pending_delete = true;
            }
            ui_mgr.open_dialog(dialog);
        }
        _ => {}
    }
    Ok(())
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
    CloseContextMenu,
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
    context_menu_open: bool,
    gesture_active: bool,
    solo_active: bool,
    editing_track: bool,
    has_selection: bool,
    tool_is_select: bool,
) -> EscapeRung {
    if context_menu_open {
        EscapeRung::CloseContextMenu
    } else if gesture_active {
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
    local_role: Res<LocalUserRole>,
    mut focus: ResMut<CameraFocusTarget>,
    spawned: Query<&SpawnedNodeMarker>,
    db: Res<DbCommandSender>,
    clock: Res<crate::notice::NoticeClock>,
    mut undo_context: UndoShortcutContext,
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
    if gestures.node_mgr.pending_selection.is_some()
        || gestures.node_mgr.pending_sidebar_select.is_some()
        || gestures.node_mgr.pending_navigation_select.is_some()
        || gestures
            .node_mgr
            .selected_petal()
            .is_some_and(|petal| nav.active_petal_id.as_deref() != Some(petal))
    {
        gestures.cancel_all();
        tool.terrain.cancel();
        path_state.selected_point = None;
        path_state.selected_segment = None;
    }
    let gesture_active = gestures.any_active() || tool.terrain.is_active();
    if gestures.node_mgr.gesture_active != gesture_active {
        gestures.node_mgr.gesture_active = gesture_active;
    }

    if undo_context
        .topbar
        .as_ref()
        .is_some_and(|topbar| topbar.close_pending)
    {
        return;
    }
    let egui_wants_kb = egui_ctx
        .ctx_mut()
        .map(|ctx| ctx.wants_keyboard_input() || egui::Popup::is_any_open(ctx))
        .unwrap_or(false);
    if egui_wants_kb {
        return;
    }

    let dialog_open = !matches!(
        ui_mgr.active_dialog,
        ActiveDialog::None | ActiveDialog::HexonManager { .. }
    );
    if keyboard.just_pressed(KeyCode::Escape) {
        if !dialog_open || matches!(ui_mgr.active_dialog, ActiveDialog::ContextMenu { .. }) {
            match escape_rung(
                matches!(ui_mgr.active_dialog, ActiveDialog::ContextMenu { .. }),
                gesture_active,
                visibility.solo().is_some(),
                path_state.editing_track_id.is_some(),
                gestures.node_mgr.selected.is_some(),
                tool.active_tool == Tool::Select,
            ) {
                EscapeRung::CloseContextMenu => ui_mgr.close_dialog(),
                EscapeRung::CancelGesture => {
                    gestures.cancel_all();
                    tool.terrain.cancel();
                    gestures.node_mgr.gesture_active = false;
                }
                EscapeRung::ClearIsolate => visibility.clear_solo(),
                EscapeRung::StopEditing => path_state.stop_editing(),
                EscapeRung::Deselect => gestures.node_mgr.deselect(),
                EscapeRung::ResetTool => tool.activate(Tool::Select, &mut right_sidebar),
                EscapeRung::Rest => {}
            }
        }
        return;
    }
    if !dialog_open && tool.terrain.polygon_active() {
        if keyboard.just_pressed(KeyCode::Enter) {tool.terrain.request_close();return;}
        if keyboard.just_pressed(KeyCode::Backspace) {tool.terrain.request_pop();return;}
    }
    if let Some(command) = shortcut(&keyboard, dialog_open) {
        if !admit_shortcut(
            command,
            &local_role,
            &nav,
            &mut ui_mgr,
            clock.elapsed_secs_f64(),
        ) {
            return;
        }
        match command {
            Shortcut::Undo | Shortcut::Redo => {
                if gesture_active {
                    ui_mgr.show_advisory(
                        "Finish or cancel the current gesture before Undo",
                        clock.elapsed_secs_f64(),
                    );
                } else if let Err(error) = crate::editor_batch::replay(
                    &mut ui_mgr.editor_batch,
                    &mut undo_context.undo,
                    command == Shortcut::Redo,
                    &nav,
                    &hierarchy,
                    &visibility,
                    &db.0,
                ) {
                    ui_mgr.show_advisory(error, clock.elapsed_secs_f64());
                }
            }
            Shortcut::Activate(next) => tool.activate(next, &mut right_sidebar),
            Shortcut::ToggleRightSidebar => right_sidebar.toggle_collapsed(),
            Shortcut::FrameSelection | Shortcut::DeleteSelection => {
                if gestures.node_mgr.selection_count() > 1 {
                    if command == Shortcut::FrameSelection {
                        let ids = gestures
                            .node_mgr
                            .members()
                            .map(|member| {
                                spawned
                                    .get(member.entity)
                                    .ok()
                                    .map(|marker| marker.node_id.clone())
                            })
                            .collect::<Option<Vec<_>>>();
                        if let Some(ids) = ids {
                            if let Some(petal) = nav.active_petal_id.clone() {
                                focus.selection = Some((petal, ids));
                            }
                        } else {
                            ui_mgr.show_advisory("One or more selected objects are unavailable; the complete framing request was refused", clock.elapsed_secs_f64());
                        }
                    } else {
                        ui_mgr.show_advisory("Deleting multiple objects is not supported by this confirmation. Select one object before deleting.", clock.elapsed_secs_f64());
                    }
                    return;
                }
                let selection = gestures.node_mgr.selected.as_ref();
                let marker = selection.and_then(|selected| spawned.get(selected.entity).ok());
                let node = selected_node(selection, marker, &nav, &hierarchy)
                    .filter(|node| visibility::pickable(&node.id, &nav, &visibility));
                if let Some(node) = node {
                    if let Err(reason) = apply_selection_shortcut(
                        command,
                        node,
                        &hierarchy,
                        &mut focus,
                        &mut ui_mgr,
                        &db.0,
                    ) {
                        warn!("{reason}");
                        ui_mgr.show_error(reason, clock.elapsed_secs_f64());
                    }
                } else {
                    ui_mgr.show_advisory(
                        "Select a visible node in the current petal first",
                        clock.elapsed_secs_f64(),
                    );
                }
            }
        }
        return;
    }
    if dialog_open {
        return;
    }
    let command_modifier = keyboard.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]);

    if keyboard.just_pressed(KeyCode::KeyH) {
        let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
        let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
        if let Some(key) = visibility_key(shift, alt, command_modifier) {
            if gestures.node_mgr.selection_count() > 1 {
                match key {
                    VisibilityKey::IsolateSelection => {
                        visibility.set_solo(
                            gestures
                                .node_mgr
                                .members()
                                .map(|member| member.node_id.clone())
                                .collect(),
                        );
                        return;
                    }
                    VisibilityKey::HideSelection => {
                        ui_mgr.show_advisory("Hiding the complete selection needs a checked visibility batch; use a Group eye or select one object", clock.elapsed_secs_f64());
                        return;
                    }
                    VisibilityKey::ShowAll => {}
                }
            }

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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_uses_bare_b_without_stealing_sidebar_toggle() {
        for def in &TOOL_DEFS {
            let mut keys = ButtonInput::default();
            keys.press(def.key_code);
            assert_eq!(shortcut(&keys, false), Some(Shortcut::Activate(def.tool)));
            assert_eq!(shortcut(&keys, true), None, "text entry owns keyboard");
            for modifier in [
                KeyCode::ControlLeft,
                KeyCode::ControlRight,
                KeyCode::SuperLeft,
                KeyCode::SuperRight,
                KeyCode::AltLeft,
                KeyCode::AltRight,
            ] {
                let mut modified = keys.clone();
                modified.press(modifier);
                assert_eq!(
                    shortcut(&modified, false),
                    None,
                    "{modifier:?}+{:?}",
                    def.tool
                );
            }
            assert_eq!(
                Shortcut::Activate(def.tool).requires_edit(),
                !matches!(def.tool, Tool::Select | Tool::Measure)
            );
            let viewer = LocalUserRole {
                role: Some(fe_database::RoleLevel::Viewer),
            };
            let editor = LocalUserRole {
                role: Some(fe_database::RoleLevel::Editor),
            };
            let mut ui = UiManager::default();
            let allowed = admit_shortcut(
                Shortcut::Activate(def.tool),
                &viewer,
                &NavigationManager::default(),
                &mut ui,
                0.0,
            );
            assert_eq!(allowed, matches!(def.tool, Tool::Select | Tool::Measure));
            if !allowed {
                assert!(ui
                    .active_notice(0.0)
                    .unwrap()
                    .0
                    .text
                    .contains("Editor role needed"));
            }
            assert!(admit_shortcut(
                Shortcut::Activate(def.tool),
                &editor,
                &NavigationManager::default(),
                &mut ui,
                0.0
            ));
        }
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::ControlLeft);
        keys.press(KeyCode::ShiftLeft);
        keys.press(KeyCode::KeyB);
        assert_eq!(shortcut(&keys, false), Some(Shortcut::ToggleRightSidebar));
        for modifier in [KeyCode::ControlLeft, KeyCode::SuperLeft] {
            let mut undo_keys = ButtonInput::default();
            undo_keys.press(modifier);
            undo_keys.press(KeyCode::KeyZ);
            assert_eq!(shortcut(&undo_keys, false), Some(Shortcut::Undo));
            assert_eq!(shortcut(&undo_keys, true), None);
            undo_keys.press(KeyCode::ShiftLeft);
            assert_eq!(shortcut(&undo_keys, false), Some(Shortcut::Redo));
            undo_keys.press(KeyCode::AltLeft);
            assert_eq!(shortcut(&undo_keys, false), None);
        }
        for replay in [Shortcut::Undo, Shortcut::Redo] {
            let mut ui = UiManager::default();
            assert!(!admit_shortcut(
                replay,
                &LocalUserRole {
                    role: Some(fe_database::RoleLevel::Viewer)
                },
                &NavigationManager::default(),
                &mut ui,
                0.0
            ));
        }
        let mut right = RightSidebarState::default();
        right.toggle_collapsed();
        assert!(right.collapsed);
        right.toggle_collapsed();
        assert!(!right.collapsed);
        assert_eq!(shortcut(&keys, true), None);
    }

    // --- staged Escape ladder: the ordering IS the contract ---

    #[test]
    fn rung0_gesture_wins_over_every_lower_rung() {
        assert_eq!(
            escape_rung(true, true, true, true, true, false),
            EscapeRung::CloseContextMenu
        );
        let mut ui = UiManager::default();
        ui.open_dialog(ActiveDialog::ContextMenu {
            screen_pos: [0.0; 2],
            world_pos: [0.0; 3],
            target: None,
            pending_delete: false,
            descendant_count: None,
        });
        if escape_rung(true, true, false, false, true, false) == EscapeRung::CloseContextMenu {
            ui.close_dialog();
        }
        assert!(!ui.any_dialog_open());

        // A live gesture consumes Escape even when a track is open, a node is
        // selected, AND a non-Select tool is active.
        assert_eq!(
            escape_rung(false, true, false, true, true, false),
            EscapeRung::CancelGesture
        );
        assert_eq!(
            escape_rung(false, true, false, false, false, true),
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
            escape_rung(false, manager.is_dragging(), false, false, true, false),
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
            escape_rung(false, manager.is_dragging(), false, false, true, false),
            EscapeRung::CancelGesture
        );
        // …and the cancel restores the press-time transform instead of
        // dropping the selection (`GestureParams::cancel_all` writes it back).
        let (_, restore) = manager.cancel_axis_drag().expect("drag was live");
        assert_eq!(restore.translation, Vec3::ZERO);
        assert!(manager.is_selected());
        use crate::verse_manager::{FractalEntry, PetalEntry, VerseEntry};
        for (key, expected) in [
            (KeyCode::Delete, Shortcut::DeleteSelection),
            (KeyCode::KeyF, Shortcut::FrameSelection),
        ] {
            let mut keyboard = ButtonInput::default();
            keyboard.press(key);
            assert_eq!(shortcut(&keyboard, false), Some(expected));
            assert_eq!(shortcut(&keyboard, true), None);
            keyboard.press(KeyCode::ControlLeft);
            assert_eq!(shortcut(&keyboard, false), None);
        }
        let node = NodeEntry {
            id: "n1".into(),
            name: "Selected node".into(),
            position: [1.0, 2.0, 3.0],
            ..Default::default()
        };
        let tree = {
            let mut value = VerseManager::default();
            value.verses = vec![VerseEntry {
                id: "v1".into(),
                fractals: vec![FractalEntry {
                    id: "f1".into(),
                    petals: vec![PetalEntry {
                        id: "p1".into(),
                        nodes: vec![node],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }];
            value
        };
        let nav = NavigationManager {
            active_verse_id: Some("v1".into()),
            active_fractal_id: Some("f1".into()),
            active_petal_id: Some("p1".into()),
            ..Default::default()
        };
        let marker = SpawnedNodeMarker {
            node_id: "n1".into(),
            petal_id: "p1".into(),
        };
        let selected =
            selected_node(manager.selected.as_ref(), Some(&marker), &nav, &tree).unwrap();
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut ui = UiManager::default();
        let mut focus = CameraFocusTarget::default();
        apply_selection_shortcut(
            Shortcut::DeleteSelection,
            selected,
            &tree,
            &mut focus,
            &mut ui,
            &tx,
        )
        .unwrap();
        assert!(
            matches!(ui.active_dialog, ActiveDialog::NodeOptions { ref node_id, pending_delete: true, descendant_count: None, .. } if node_id == "n1")
        );
        assert!(
            matches!(rx.try_recv(), Ok(DbCommand::CountNodeDescendants { node_id }) if node_id == "n1")
        );
        assert!(
            rx.try_recv().is_err(),
            "Delete never sends a mutation on first press"
        );
        assert!(ui.drain_actions().is_empty());
        apply_selection_shortcut(
            Shortcut::FrameSelection,
            selected,
            &tree,
            &mut focus,
            &mut ui,
            &tx,
        )
        .unwrap();
        assert_eq!(focus.target, Some(("n1".into(), [1.0, 2.0, 3.0])));
        assert!(!Shortcut::FrameSelection.requires_edit());
        assert!(Shortcut::DeleteSelection.requires_edit());
        let stale_marker = SpawnedNodeMarker {
            petal_id: "old-petal".into(),
            node_id: marker.node_id.clone(),
        };
        assert!(
            selected_node(manager.selected.as_ref(), Some(&stale_marker), &nav, &tree).is_none()
        );
        let foreign_nav = NavigationManager {
            active_verse_id: Some("other".into()),
            ..nav
        };
        assert!(selected_node(
            manager.selected.as_ref(),
            Some(&marker),
            &foreign_nav,
            &tree
        )
        .is_none());
        let viewer = LocalUserRole {
            role: Some(fe_database::RoleLevel::Viewer),
        };
        assert!(!admit_shortcut(
            Shortcut::DeleteSelection,
            &viewer,
            &foreign_nav,
            &mut ui,
            0.0
        ));
        assert!(admit_shortcut(
            Shortcut::FrameSelection,
            &viewer,
            &foreign_nav,
            &mut ui,
            0.0
        ));
        assert!(selected_node(None, Some(&marker), &foreign_nav, &tree).is_none());
        drop(rx);
        ui.close_dialog();
        assert!(apply_selection_shortcut(
            Shortcut::DeleteSelection,
            selected,
            &tree,
            &mut focus,
            &mut ui,
            &tx
        )
        .is_err());
        assert!(!ui.any_dialog_open());
    }

    #[test]
    fn rung1_stop_editing_wins_over_deselect_and_tool_reset() {
        assert_eq!(
            escape_rung(false, false, false, true, true, false),
            EscapeRung::StopEditing
        );
    }

    #[test]
    fn rung2_deselect_wins_over_tool_reset() {
        assert_eq!(
            escape_rung(false, false, false, false, true, false),
            EscapeRung::Deselect
        );
    }

    #[test]
    fn rung3_resets_a_non_select_tool_when_nothing_else_is_pending() {
        // Fixes the sticky-Pen complaint (#12).
        assert_eq!(
            escape_rung(false, false, false, false, false, false),
            EscapeRung::ResetTool
        );
    }

    #[test]
    fn fully_at_rest_escape_does_nothing() {
        assert_eq!(
            escape_rung(false, false, false, false, false, true),
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
            let rung = escape_rung(false, false, false, editing, selected, tool_is_select);
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
                EscapeRung::CloseContextMenu
                | EscapeRung::CancelGesture
                | EscapeRung::ClearIsolate
                | EscapeRung::Rest => {}
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
            escape_rung(false, false, false, true, true, true),
            EscapeRung::StopEditing
        );
        assert_eq!(
            escape_rung(false, false, false, false, true, true),
            EscapeRung::Deselect
        );
    }

    // --- D-17: Esc clears isolate first — right after a live gesture ---

    #[test]
    fn clear_isolate_sits_between_a_live_gesture_and_every_session_rung() {
        // A live drag still owns rung 0 even while isolated (F5).
        assert_eq!(
            escape_rung(false, true, true, true, true, false),
            EscapeRung::CancelGesture
        );
        // Isolate outranks an open track, a selection and a non-Select tool.
        assert_eq!(
            escape_rung(false, false, true, true, true, false),
            EscapeRung::ClearIsolate
        );
        // Walking the ladder from "isolated + track open + Pen": isolate goes
        // first, then the one-press back-out, then the tool reset.
        let (mut solo, mut editing, mut selected, mut tool_is_select) = (true, true, true, false);
        let mut walked = Vec::new();
        for _ in 0..4 {
            let rung = escape_rung(false, false, solo, editing, selected, tool_is_select);
            walked.push(rung);
            match rung {
                EscapeRung::ClearIsolate => solo = false,
                EscapeRung::StopEditing => (editing, selected) = (false, false),
                EscapeRung::Deselect => selected = false,
                EscapeRung::ResetTool => tool_is_select = true,
                EscapeRung::CloseContextMenu | EscapeRung::CancelGesture | EscapeRung::Rest => {}
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
