//! Viewport right-click context menu (contextual_controls_20260725).
//!
//! The object-aware menu is a pure table: [`menu_for`] maps a viewport
//! [`HitTarget`] (the same classification `node_manager::dispatch` produces) to
//! the ordered [`Verb`] set valid for that object (FR-1). Rendering, labels, and
//! tooltips are thin functions over that table; the verb→`UiAction` wiring is in
//! [`verb_action`]. Rationale + the full verb matrix live in
//! `fe-ui/src/dialogs/AGENTS.md` §context-menu (N-7).

use bevy_egui::egui;

use super::{ActiveDialog, ContextTarget};
use crate::actions::asset::{StampInteractionState, StampRef};
use crate::actions::{UiAction, UiManager};
use crate::gis::{CornerKind, PathEditorState};
use crate::node_manager::HitTarget;
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// One entry in an object's right-click menu. The set is the union of the
/// ratified per-object verb tables (spec Q-1); [`menu_for`] selects the subset
/// valid for each [`HitTarget`]. `CopyApi`/`Report`/`ReportVolume` are the
/// T5-seam verbs (FR-4) — see [`verb_is_seam_gated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    /// Empty ground: create a new empty node at the cursor.
    CreateNode,
    /// Empty ground: place an asset (open the glTF import dialog).
    PlaceAsset,
    /// Node: open the property editor.
    EditProperties,
    /// Node: rename.
    Rename,
    /// Node: duplicate.
    Duplicate,
    /// Node: clear all custom properties WITHOUT deleting (the husk-bug
    /// distinction — clearing properties is NOT delete, FR-2).
    ClearProperties,
    /// Node/stamp/path/region: copy the object's public API/egress string (T5).
    CopyApi,
    /// Node/stamp/path: open the object's report/query view (T5).
    Report,
    /// Delete via the sync-safe tombstone (+ cascade for parents), FR-2.
    Delete,
    /// Stamp: promote an un-promoted instance to a full addressable node (T1 FR-5).
    PromoteToNode,
    /// Stamp: edit the per-node scale/rotation overrides (T2 FR-3).
    ScaleRotate,
    /// Stamp: slide along its owning curve by arc-length (T2 Q-1 ratified).
    SlideAlongPath,
    /// Path: open the path editor.
    EditPath,
    /// Path: add stamps along the path.
    AddStamps,
    /// Path point: set the corner/smooth/symmetric classification.
    SetCornerSmooth,
    /// Path point: delete this point.
    DeletePoint,
    /// Earthwork region: edit its parameters (shape/op/material).
    EditRegionParams,
    /// Earthwork region: report its cut/fill volume in real units (T5).
    ReportVolume,
}

/// FR-1 object-aware menu table: the ordered verb set for a viewport hit. Pure
/// and total over every [`HitTarget`]; unit-tested exhaustively below. No verb
/// appears for an object it cannot act on (spec Q-1).
///
/// `TerrainCell` (bare terrain surface) is treated like empty ground — it offers
/// creation, not object verbs. `GimbalAxis` is a transform widget, not an
/// object, so it yields no menu.
pub(crate) fn menu_for(hit: &HitTarget) -> Vec<Verb> {
    use Verb::*;
    match hit {
        HitTarget::Empty | HitTarget::TerrainCell => vec![CreateNode, PlaceAsset],
        HitTarget::Node(_) => vec![
            EditProperties,
            Rename,
            Duplicate,
            ClearProperties,
            CopyApi,
            Report,
            Delete,
        ],
        HitTarget::Stamp(_) => vec![
            PromoteToNode,
            ScaleRotate,
            SlideAlongPath,
            CopyApi,
            Report,
            Delete,
        ],
        HitTarget::PathSegment { .. } => vec![EditPath, AddStamps, CopyApi, Report, Delete],
        HitTarget::PathVertex { .. } | HitTarget::PathHandle { .. } => {
            vec![SetCornerSmooth, DeletePoint]
        }
        HitTarget::TerrainProposal { .. } => vec![EditRegionParams, ReportVolume, CopyApi, Delete],
        HitTarget::GimbalAxis => vec![],
    }
}

/// The menu label for a verb (calm chrome — short, imperative; ui_ux §1).
pub(crate) fn verb_label(verb: Verb) -> &'static str {
    match verb {
        Verb::CreateNode => "Add Empty Node",
        Verb::PlaceAsset => "Add GLTF Model",
        Verb::EditProperties => "Edit Properties",
        Verb::Rename => "Rename\u{2026}",
        Verb::Duplicate => "Duplicate",
        Verb::ClearProperties => "Clear Properties",
        Verb::CopyApi => "Copy API String",
        Verb::Report => "Report / Query",
        Verb::Delete => "Delete",
        Verb::PromoteToNode => "Promote to Node",
        Verb::ScaleRotate => "Scale / Rotate\u{2026}",
        Verb::SlideAlongPath => "Slide Along Path",
        Verb::EditPath => "Edit Path",
        Verb::AddStamps => "Add Stamps\u{2026}",
        Verb::SetCornerSmooth => "Corner / Smooth",
        Verb::DeletePoint => "Delete Point",
        Verb::EditRegionParams => "Edit Region\u{2026}",
        Verb::ReportVolume => "Report Volume",
    }
}

/// Hover tooltip for a verb (ui_ux §8 — every verb has one).
pub(crate) fn verb_tooltip(verb: Verb) -> &'static str {
    match verb {
        Verb::CreateNode => "Create a new empty node at the cursor position.",
        Verb::PlaceAsset => "Import a glTF/GLB model as a new node here.",
        Verb::EditProperties => "Open this node's custom properties for editing.",
        Verb::Rename => "Change this node's display name.",
        Verb::Duplicate => "Create a copy of this node nearby.",
        Verb::ClearProperties => {
            "Remove this node's custom properties. The node itself stays — this is \
             not a delete."
        }
        Verb::CopyApi => "Copy this object's public read/write API endpoint string.",
        Verb::Report => "Open this object's report / query view.",
        Verb::Delete => {
            "Delete this object. Sync-safe (tombstone) and cascades to its \
             children after a confirm."
        }
        Verb::PromoteToNode => "Materialize this stamp into a full, individually addressable node.",
        Verb::ScaleRotate => "Adjust this stamp's per-node scale and rotation overrides.",
        Verb::SlideAlongPath => "Slide this stamp along its path by arc-length.",
        Verb::EditPath => "Open this path in the path editor.",
        Verb::AddStamps => "Stamp an asset along this path.",
        Verb::SetCornerSmooth => "Set this point's corner / smooth / symmetric handle mode.",
        Verb::DeletePoint => "Remove this point from the path.",
        Verb::EditRegionParams => "Edit this earthwork region's shape, operation, and material.",
        Verb::ReportVolume => "Report this earthwork region's cut/fill volume in real units.",
    }
}

/// Target-aware label: the one case where a verb's SCOPE depends on the object
/// it was invoked from. A `PathSegment` has no per-segment delete — its Delete
/// tombstones the entire track — so under the "Path segment N" header a plain
/// "Delete" reads as if it removed the segment (findings F6/F20). Everything
/// else falls through to [`verb_label`]. Pure.
pub(crate) fn verb_label_for(verb: Verb, hit: &HitTarget) -> &'static str {
    match (verb, hit) {
        (Verb::Delete, HitTarget::PathSegment { .. }) => "Delete Track\u{2026}",
        _ => verb_label(verb),
    }
}

/// Target-aware tooltip, paired with [`verb_label_for`]. Pure.
pub(crate) fn verb_tooltip_for(verb: Verb, hit: &HitTarget) -> &'static str {
    match (verb, hit) {
        (Verb::Delete, HitTarget::PathSegment { .. }) => {
            "Delete the ENTIRE track this segment belongs to \u{2014} every point \
             goes with it. There is no per-segment delete. Sync-safe (tombstone), \
             after a confirm."
        }
        _ => verb_tooltip(verb),
    }
}

/// Confirm copy for a `PathSegment` Delete (findings F6/F20). The generic
/// cascade copy talks about "this node and its children", which under a "Path
/// segment N" header hides that the whole path is going away — so this names
/// the scope outright. `point_count` is `None` when the track is no longer the
/// open one and its size cannot be read. Pure.
pub(crate) fn track_delete_confirm_message(point_count: Option<usize>) -> String {
    match point_count {
        Some(1) => "Delete the WHOLE track, not just this segment? Its single point, \
                    and anything stamped along it, are tombstoned. This cannot be undone."
            .to_string(),
        Some(count) => format!(
            "Delete the WHOLE track, not just this segment? All {count} of its points, \
             and anything stamped along it, are tombstoned. This cannot be undone."
        ),
        None => "Delete the WHOLE track, not just this segment? Every point on it, \
                 and anything stamped along it, are tombstoned. This cannot be undone."
            .to_string(),
    }
}

/// Whether a verb needs the `endpoint_api_surface` (T5) egress seam. Such verbs
/// render disabled-with-hint until the seam yields a string (FR-4, ui_ux §6).
pub(crate) fn verb_is_seam_gated(verb: Verb) -> bool {
    matches!(verb, Verb::CopyApi | Verb::Report | Verb::ReportVolume)
}

/// Map a verb acting on `node_id` to the `UiAction` it queues, when the verb is
/// a node-scoped action this track owns (delete/duplicate/clear/copy-API/report/
/// edit-properties). Verbs that are path/stamp/terrain-domain (owned by T2/T3)
/// or need extra context return `None` — the caller renders them but routes them
/// through the owning surface. `cascade` selects the tombstone-cascade delete.
pub(crate) fn verb_action(verb: Verb, node_id: &str, cascade: bool) -> Option<UiAction> {
    let id = node_id.to_string();
    match verb {
        Verb::Delete => Some(UiAction::DeleteNode {
            node_id: id,
            cascade,
        }),
        Verb::Duplicate => Some(UiAction::DuplicateNode { node_id: id }),
        Verb::ClearProperties => Some(UiAction::ClearNodeProperties { node_id: id }),
        Verb::CopyApi => Some(UiAction::CopyApiString { node_id: id }),
        Verb::Report | Verb::ReportVolume => Some(UiAction::ReportObject { node_id: id }),
        Verb::EditProperties => Some(UiAction::LoadNodeProperties { node_id: id }),
        _ => None,
    }
}

/// Anchor index a `PathVertex`/`PathHandle` hit addresses (both share it —
/// a handle belongs to its anchor's point-list row), `None` for every other
/// `HitTarget`. Pure.
pub(crate) fn anchor_index(hit: &HitTarget) -> Option<usize> {
    match hit {
        HitTarget::PathVertex { idx } | HitTarget::PathHandle { idx, .. } => Some(*idx),
        _ => None,
    }
}

/// Cycles a corner kind Corner → Smooth → Symmetric → Corner — the flat
/// menu's one-click quick-set. The fuller 3-way toggle + collinear-handle
/// re-derive (`corner_toggle_outcome`) lives with the Options-section corner
/// editor (`panels::path_editor_card::render_corner_settings`); this menu
/// only needs the same persist op (`PathSetAnchorCorner`) it ends up calling,
/// not the handle-geometry dance. Pure.
pub(crate) fn next_corner_kind(kind: CornerKind) -> CornerKind {
    match kind {
        CornerKind::Corner => CornerKind::Smooth,
        CornerKind::Smooth => CornerKind::Symmetric,
        CornerKind::Symmetric => CornerKind::Corner,
    }
}

/// Routes `SetCornerSmooth`/`DeletePoint` to the real path-point ops
/// (`actions/path.rs`'s `set_anchor_corner`/`remove_point`, both already
/// wired in `actions::mod::process_ui_actions` — no new `UiAction` needed).
/// `None` when `verb` isn't one of the two, or `hit` isn't an anchor
/// (`PathVertex`/`PathHandle`). Pure.
pub(crate) fn point_verb_action(
    verb: Verb,
    hit: &HitTarget,
    track_node_id: &str,
    current_corner: CornerKind,
) -> Option<UiAction> {
    let index = anchor_index(hit)?;
    match verb {
        Verb::SetCornerSmooth => Some(UiAction::PathSetAnchorCorner {
            track_node_id: track_node_id.to_string(),
            index,
            corner: next_corner_kind(current_corner),
        }),
        Verb::DeletePoint => Some(UiAction::PathRemovePoint {
            track_node_id: track_node_id.to_string(),
            index,
        }),
        _ => None,
    }
}

/// `EditRegionParams`' routing (D16 finding #4): select the proposal for
/// editing + reveal the Terrain section it's edited from. `None` for every
/// other verb/target pairing. Pure.
pub(crate) fn edit_region_actions(verb: Verb, hit: &HitTarget) -> Option<[UiAction; 2]> {
    if verb != Verb::EditRegionParams {
        return None;
    }
    match hit {
        HitTarget::TerrainProposal { id } => Some([
            UiAction::TerrainProposalSelect { id: id.clone() },
            UiAction::RevealSection {
                slug: "terrain".to_string(),
            },
        ]),
        _ => None,
    }
}

/// A `TerrainProposal`'s delete confirm routes to `TerrainProposalDelete`
/// (proposals aren't nodes — `DeleteNode`/cascade doesn't apply). `None` for
/// every other target, which stays on the node-backed delete path. Pure.
pub(crate) fn proposal_delete_action(hit: &HitTarget) -> Option<UiAction> {
    match hit {
        HitTarget::TerrainProposal { id } => {
            Some(UiAction::TerrainProposalDelete { id: id.clone() })
        }
        _ => None,
    }
}

/// Disabled-hint for seam-gated verbs whose T5 egress string is absent (FR-4).
const SEAM_GATED_HINT: &str =
    "No API endpoint for this object yet \u{2014} lights up when the read/write \
     API surface (endpoint_api_surface) provides one.";

/// The one button body every verb row shares: danger tint for the destructive
/// verbs, hover tooltip when enabled, explicit hint when not (N-8 — disabled is
/// never silent). Callers supply the copy so target-aware labels can differ.
fn render_labeled_verb_button(
    ui: &mut egui::Ui,
    verb: Verb,
    label: &str,
    tooltip: &str,
    enabled: bool,
    disabled_hint: &str,
) -> bool {
    let color = if matches!(verb, Verb::Delete | Verb::DeletePoint) {
        egui::Color32::from_rgb(230, 120, 120)
    } else {
        theme::TEXT_BRIGHT
    };
    let button =
        egui::Button::new(egui::RichText::new(label).color(color)).fill(egui::Color32::TRANSPARENT);
    let resp = ui.add_enabled(enabled, button);
    if enabled {
        resp.on_hover_text(tooltip).clicked()
    } else {
        resp.on_disabled_hover_text(disabled_hint);
        false
    }
}

/// Renders one verb button and reports whether it was clicked. Seam-gated verbs
/// whose seam string is absent render disabled-with-an-explanatory-hint (FR-4,
/// never silently absent). All verbs carry a hover tooltip (ui_ux §8).
///
/// Target-free: for callers that already know the object from context (the Node
/// Options dialog). The viewport menu uses [`render_target_verb_button`], which
/// takes the `HitTarget` and can say "Delete Track…" where that is the truth.
pub(crate) fn render_verb_button(ui: &mut egui::Ui, verb: Verb, seam_available: bool) -> bool {
    let enabled = !verb_is_seam_gated(verb) || seam_available;
    render_gated_verb_button(ui, verb, enabled, SEAM_GATED_HINT)
}

/// [`render_verb_button`] with explicit gating + hint — for object-state gates
/// (e.g. a stamp verb waiting on promotion). Disabled is never silent (N-8).
pub(crate) fn render_gated_verb_button(
    ui: &mut egui::Ui,
    verb: Verb,
    enabled: bool,
    disabled_hint: &str,
) -> bool {
    render_labeled_verb_button(
        ui,
        verb,
        verb_label(verb),
        verb_tooltip(verb),
        enabled,
        disabled_hint,
    )
}

/// [`render_verb_button`] labelled through the target-aware table (F6/F20).
fn render_target_verb_button(
    ui: &mut egui::Ui,
    verb: Verb,
    hit: &HitTarget,
    seam_available: bool,
) -> bool {
    let enabled = !verb_is_seam_gated(verb) || seam_available;
    render_target_gated_verb_button(ui, verb, hit, enabled, SEAM_GATED_HINT)
}

/// [`render_gated_verb_button`] labelled through the target-aware table.
fn render_target_gated_verb_button(
    ui: &mut egui::Ui,
    verb: Verb,
    hit: &HitTarget,
    enabled: bool,
    disabled_hint: &str,
) -> bool {
    render_labeled_verb_button(
        ui,
        verb,
        verb_label_for(verb, hit),
        verb_tooltip_for(verb, hit),
        enabled,
        disabled_hint,
    )
}

/// Prefill a Node Options dialog for `node_id` from the loaded hierarchy —
/// the menu's Rename verb opens this (the dialog's Name field is the rename
/// surface, persisted via `DbCommand::RenameNode` on Save).
pub(crate) fn node_options_prefill(hierarchy: &VerseManager, node_id: &str) -> ActiveDialog {
    let (name, url) = hierarchy
        .all_nodes()
        .find(|n| n.id == node_id)
        .map(|n| (n.name.clone(), n.webpage_url.clone().unwrap_or_default()))
        .unwrap_or_default();
    ActiveDialog::NodeOptions {
        node_id: node_id.to_string(),
        node_name_buf: name,
        webpage_url_buf: url,
        pending_delete: false,
        descendant_count: None,
    }
}

/// Side effects a menu pass queues; flushed after the egui borrow window.
#[derive(Default)]
struct MenuOutcome {
    actions: Vec<UiAction>,
    next_dialog: Option<ActiveDialog>,
    toast: Option<&'static str>,
    /// Node id to send `CountNodeDescendants` for (cascade confirm arming).
    count_request: Option<String>,
    close: bool,
}

/// Renders the viewport right-click context menu (T4 FR-1): object-aware via
/// the classified [`ContextTarget`] the `node_manager::context_pick` system
/// fills (a dim placeholder shows for the ≤1 frame before it lands). Delete is
/// a two-step in-menu confirm with the live descendant count (Q-2); stamp
/// verbs key on the `(track, index)` payload + the stamp authority's live
/// promotion state. See `dialogs/AGENTS.md` §context-menu.
#[allow(clippy::too_many_arguments)] // thin egui render fn over one dialog's state
pub fn render_context_menu(
    ctx: &egui::Context,
    ui_mgr: &mut UiManager,
    hierarchy: &VerseManager,
    stamp_state: &StampInteractionState,
    tool_panel: &mut crate::panels::tool_panel::ToolPanelState,
    // D16 (finding #4): read-only — resolves the edited track id + anchor
    // corner kinds for the PathVertex/PathHandle verbs (`SetCornerSmooth`/
    // `DeletePoint`). NOTE for the call site (`panels/mod.rs::gardener_console`,
    // owned by a different slice this wave): `path_state` is already a
    // `gardener_console` param under that same name — pass `&*path_state`.
    path_state: &PathEditorState,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let ActiveDialog::ContextMenu {
        ref screen_pos,
        ref world_pos,
        ref target,
        ref mut pending_delete,
        ref mut descendant_count,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let pos = egui::pos2(screen_pos[0], screen_pos[1]);
    let world = *world_pos;
    let now = ctx.input(|i| i.time);
    let mut outcome = MenuOutcome::default();

    let area_response = egui::Area::new(egui::Id::new("viewport_context_menu"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(theme::BG_CONTEXT_MENU)
                .inner_margin(egui::Margin::same(4))
                .corner_radius(4.0)
                .stroke(egui::Stroke::new(1.0_f32, theme::TEXT_DIM))
                .show(ui, |ui| {
                    ui.set_min_width(170.0);
                    let Some(target) = target else {
                        // Classification pending (≤1 frame) — placeholder, not
                        // a possibly-wrong menu.
                        ui.label(egui::RichText::new("\u{2026}").color(theme::TEXT_DIM));
                        return;
                    };
                    if *pending_delete {
                        render_delete_confirm(
                            ui,
                            target,
                            stamp_state,
                            path_state,
                            *descendant_count,
                            pending_delete,
                            &mut outcome,
                        );
                    } else {
                        render_target_menu(
                            ui,
                            target,
                            world,
                            hierarchy,
                            stamp_state,
                            tool_panel,
                            path_state,
                            pending_delete,
                            descendant_count,
                            &mut outcome,
                        );
                    }
                });
        });

    // Close on click elsewhere — use the actual rendered rect rather than a
    // hardcoded size so all items are accounted for regardless of content.
    if ctx.input(|i| i.pointer.any_pressed()) {
        let ptr = ctx.input(|i| i.pointer.interact_pos());
        if let Some(ptr_pos) = ptr {
            let menu_rect = area_response.response.rect;
            if !menu_rect.contains(ptr_pos) {
                outcome.close = true;
            }
        }
    }

    if let Some(node_id) = outcome.count_request {
        // Authoritative subtree size for the cascade confirm (spine query);
        // the dialog shows the generic copy until the count lands.
        if db_tx
            .send(DbCommand::CountNodeDescendants { node_id })
            .is_err()
        {
            bevy::log::warn!("db_sender channel closed \u{2014} CountNodeDescendants not sent");
        }
    }
    if let Some(msg) = outcome.toast {
        ui_mgr.show_toast(msg, now);
    }
    for action in outcome.actions {
        ui_mgr.push_action(action);
    }
    if let Some(dialog) = outcome.next_dialog {
        ui_mgr.open_dialog(dialog);
    } else if outcome.close {
        ui_mgr.close_dialog();
    }
}

/// Object header + the [`menu_for`] verb list for the classified target.
#[allow(clippy::too_many_arguments)] // thin egui render fn over one dialog's state
fn render_target_menu(
    ui: &mut egui::Ui,
    target: &ContextTarget,
    world: [f32; 3],
    hierarchy: &VerseManager,
    stamp_state: &StampInteractionState,
    tool_panel: &mut crate::panels::tool_panel::ToolPanelState,
    path_state: &PathEditorState,
    pending_delete: &mut bool,
    descendant_count: &mut Option<usize>,
    outcome: &mut MenuOutcome,
) {
    // Stamp payload + live promotion state (the promoted node id backs the
    // node-scoped verbs; it can land WHILE the menu is open — verbs light up).
    let stamp_ref = target.stamp.as_ref().map(|(track, index)| StampRef {
        track_node_id: track.clone(),
        stamp_index: *index,
    });
    let promoted_id = stamp_ref
        .as_ref()
        .and_then(|s| stamp_state.promoted_node_id(s))
        .map(str::to_string);
    // The id node-scoped verbs act on: the node itself, or a stamp's promoted node.
    let node_backed = target.node_id.clone().or_else(|| promoted_id.clone());

    // Calm header: what the menu is about.
    match (&target.hit, &stamp_ref) {
        (HitTarget::Stamp(_), Some(stamp)) => {
            // Live read of the stamp-selection authority (right-click routed
            // through `SelectStamp`, so this marks the menu's own object).
            let selected = stamp_state.selected() == Some(stamp);
            let header = format!(
                "Stamp {}{}",
                stamp.stamp_index,
                if selected { " \u{2014} selected" } else { "" }
            );
            ui.label(egui::RichText::new(header).small().color(theme::TEXT_DIM));
            ui.separator();
        }
        (HitTarget::Node(_), _) => {
            if let Some(id) = &target.node_id {
                let name = hierarchy
                    .all_nodes()
                    .find(|n| n.id == *id)
                    .map(|n| n.name.clone())
                    .unwrap_or_else(|| id.clone());
                ui.label(egui::RichText::new(name).small().color(theme::TEXT_DIM));
                ui.separator();
            }
        }
        // D16 (finding #4): the four newly-resolvable targets get a minimal
        // header too — no lookups needed, the hit payload is self-describing.
        (HitTarget::PathSegment { idx }, _) => {
            ui.label(
                egui::RichText::new(format!("Path segment {idx}"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();
        }
        (HitTarget::PathVertex { idx }, _) => {
            ui.label(
                egui::RichText::new(format!("Path point {idx}"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();
        }
        (HitTarget::PathHandle { idx, side }, _) => {
            ui.label(
                egui::RichText::new(format!("Path handle {idx} ({side:?})"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();
        }
        (HitTarget::TerrainProposal { id }, _) => {
            ui.label(
                egui::RichText::new(format!("Earthwork region {id}"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();
        }
        _ => {}
    }

    for verb in menu_for(&target.hit) {
        match verb {
            // --- empty ground: creation ---
            Verb::CreateNode => {
                if render_target_verb_button(ui, verb, &target.hit, true) {
                    outcome
                        .actions
                        .push(UiAction::CreateNodeAt { position: world });
                    outcome.close = true;
                }
            }
            Verb::PlaceAsset => {
                if render_target_verb_button(ui, verb, &target.hit, true) {
                    outcome.next_dialog = Some(ActiveDialog::GltfImport {
                        file_path_buf: String::new(),
                        name_buf: String::new(),
                        position: world,
                    });
                }
            }
            // --- node-scoped verbs (shared `verb_action` map) ---
            Verb::EditProperties | Verb::Duplicate | Verb::ClearProperties => {
                if let Some(id) = &node_backed {
                    if render_target_verb_button(ui, verb, &target.hit, true) {
                        outcome.actions.extend(verb_action(verb, id, false));
                        outcome.close = true;
                    }
                }
            }
            Verb::Rename => {
                if let Some(id) = &node_backed {
                    if render_target_verb_button(ui, verb, &target.hit, true) {
                        // The Node Options Name field is the rename surface
                        // (Save → DbCommand::RenameNode).
                        outcome.next_dialog = Some(node_options_prefill(hierarchy, id));
                    }
                }
            }
            Verb::CopyApi | Verb::Report | Verb::ReportVolume => {
                let egress = node_backed.as_deref().and_then(|id| match verb {
                    Verb::CopyApi => crate::gis::egress_strings::api_string_for(id),
                    _ => crate::gis::egress_strings::report_for(id),
                });
                let unpromoted_stamp = stamp_ref.is_some() && promoted_id.is_none();
                let clicked = if unpromoted_stamp {
                    render_target_gated_verb_button(
                        ui,
                        verb,
                        &target.hit,
                        false,
                        "Available once this stamp finishes promoting to a node.",
                    )
                } else {
                    render_target_verb_button(ui, verb, &target.hit, egress.is_some())
                };
                if clicked {
                    if let (Some(id), Some(text)) = (&node_backed, &egress) {
                        // Clipboard write is render-side (only egui `ctx` may
                        // touch it); the action surfaces the outcome toast.
                        ui.ctx().copy_text(text.clone());
                        outcome.actions.extend(verb_action(verb, id, false));
                        outcome.close = true;
                    }
                }
            }
            Verb::Delete => {
                // D16: an earthwork region is deletable on its own id (never
                // node-backed — `proposal_delete_action` short-circuits the
                // node/stamp gating below for it).
                let is_proposal = proposal_delete_action(&target.hit).is_some();
                // F6: on a segment this verb tombstones the whole TRACK; the
                // label/tooltip/confirm all say so instead of "this object".
                let is_path_segment = matches!(target.hit, HitTarget::PathSegment { .. });
                let deletable = node_backed.is_some() || is_proposal;
                let clicked = if deletable {
                    render_target_verb_button(ui, verb, &target.hit, true)
                } else {
                    render_target_gated_verb_button(
                        ui,
                        verb,
                        &target.hit,
                        false,
                        "Promoting this stamp to a node \u{2014} try again in a moment.",
                    )
                };
                if clicked {
                    // Two-step confirm (Q-2) + authoritative descendant count
                    // (plain nodes only — a stamp's confirm uses its re-flow
                    // copy, a track's its point count, a proposal's needs no
                    // count at all, so none of the three would display it).
                    *pending_delete = true;
                    *descendant_count = None;
                    if stamp_ref.is_none() && !is_proposal && !is_path_segment {
                        outcome.count_request = node_backed.clone();
                    }
                }
            }
            // --- stamp verbs (T2 payload) ---
            Verb::PromoteToNode => {
                if let Some(stamp) = &stamp_ref {
                    let clicked = render_target_gated_verb_button(
                        ui,
                        verb,
                        &target.hit,
                        promoted_id.is_none(),
                        "Already promoted \u{2014} this stamp is a full addressable node.",
                    );
                    if clicked {
                        outcome.actions.push(UiAction::PromoteStamp {
                            track_node_id: stamp.track_node_id.clone(),
                            stamp_index: stamp.stamp_index,
                        });
                        outcome.close = true;
                    }
                }
            }
            Verb::ScaleRotate | Verb::SlideAlongPath => {
                if let Some(stamp) = &stamp_ref {
                    if render_target_verb_button(ui, verb, &target.hit, true) {
                        // Route to the per-stamp editor (Tools sidebar): open
                        // the owning track for editing and aim the editor at
                        // this stamp's index.
                        outcome.actions.push(UiAction::PathSelectTrack {
                            track_node_id: stamp.track_node_id.clone(),
                        });
                        tool_panel.stamp_edit_index = stamp.stamp_index as u32;
                        outcome.toast = Some("Stamp controls opened in the Tools panel");
                        outcome.close = true;
                    }
                }
            }
            // --- path-object verbs (track-backed) ---
            Verb::EditPath | Verb::AddStamps => {
                if let Some(id) = &node_backed {
                    if render_target_verb_button(ui, verb, &target.hit, true) {
                        outcome.actions.push(UiAction::PathSelectTrack {
                            track_node_id: id.clone(),
                        });
                        outcome.toast = Some("Path opened for editing (Tools panel)");
                        outcome.close = true;
                    }
                }
            }
            // --- path-point verbs (D16 finding #4: anchor-backed, only ever
            // resolved while the anchor's track is open for editing) ---
            Verb::SetCornerSmooth | Verb::DeletePoint => {
                let Some(track_id) = path_state.editing_track_id.clone() else {
                    // Classification only ever yields PathVertex/PathHandle
                    // while editing, but the menu can outlive an Esc that
                    // closed the session between classify and render — never
                    // a silent click.
                    render_target_gated_verb_button(
                        ui,
                        verb,
                        &target.hit,
                        false,
                        "The path is no longer open for editing.",
                    );
                    continue;
                };
                let current_corner = anchor_index(&target.hit)
                    .and_then(|idx| path_state.points.get(idx))
                    .map(|row| row.corner)
                    .unwrap_or_default();
                if render_target_verb_button(ui, verb, &target.hit, true) {
                    if let Some(action) =
                        point_verb_action(verb, &target.hit, &track_id, current_corner)
                    {
                        outcome.actions.push(action);
                    }
                    outcome.close = true;
                }
            }
            // --- earthwork-region verb (D16 finding #4) ---
            Verb::EditRegionParams => {
                if render_target_verb_button(ui, verb, &target.hit, true) {
                    if let Some(actions) = edit_region_actions(verb, &target.hit) {
                        outcome.actions.extend(actions);
                    }
                    outcome.close = true;
                }
            }
        }
    }
}

/// The two-step delete confirm (Q-2): cascade copy with the live descendant
/// count for nodes, re-flow copy for stamps, whole-track copy for a path
/// segment (F6 — the verb has no per-segment scope). Confirm routes the
/// sync-safe tombstone-cascade (T1) — the real remove path, never a raw drop.
#[allow(clippy::too_many_arguments)] // thin egui render fn over one dialog's state
fn render_delete_confirm(
    ui: &mut egui::Ui,
    target: &ContextTarget,
    stamp_state: &StampInteractionState,
    path_state: &PathEditorState,
    descendant_count: Option<usize>,
    pending_delete: &mut bool,
    outcome: &mut MenuOutcome,
) {
    let node_backed = target.node_id.clone().or_else(|| {
        target
            .stamp
            .as_ref()
            .and_then(|(track, index)| {
                stamp_state.promoted_node_id(&StampRef {
                    track_node_id: track.clone(),
                    stamp_index: *index,
                })
            })
            .map(str::to_string)
    });
    let proposal_delete = proposal_delete_action(&target.hit);
    let message = if target.stamp.is_some() {
        "Delete this stamp? Its node is tombstoned and the path re-flows the \
         remaining stamps. This cannot be undone."
            .to_string()
    } else if proposal_delete.is_some() {
        "Delete this earthwork region? Its baked terrain contribution is \
         reverted. This cannot be undone."
            .to_string()
    } else if matches!(target.hit, HitTarget::PathSegment { .. }) {
        // The point count is only readable while this track is the open one —
        // which is the only state a segment can be classified in. A mismatch
        // (session closed between arm and confirm) degrades to countless copy
        // rather than reporting a foreign track's size.
        let point_count = if path_state.editing_track_id.as_deref() == target.node_id.as_deref() {
            Some(path_state.points.len())
        } else {
            None
        };
        track_delete_confirm_message(point_count)
    } else {
        crate::ui_shell::modal::cascade_confirm_message(descendant_count.unwrap_or(0))
    };
    ui.label(egui::RichText::new(message).color(theme::STATUS_OFFLINE));
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE),
                )
                .fill(theme::BG_DANGER),
            )
            .clicked()
        {
            if let Some(action) = proposal_delete {
                outcome.actions.push(action);
            } else if let Some(id) = node_backed {
                outcome.actions.push(UiAction::DeleteNode {
                    node_id: id,
                    cascade: true,
                });
            } else {
                // The backing node vanished between arm and confirm — say so.
                outcome.toast = Some("Nothing to delete \u{2014} the object is gone");
            }
            outcome.close = true;
        }
        if ui
            .add(egui::Button::new("Cancel").fill(theme::BG_BUTTON))
            .clicked()
        {
            *pending_delete = false;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_manager::HandleSide;
    use bevy::prelude::Entity;

    fn entity(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    /// Every `HitTarget` classification the dispatch produces, so the table is
    /// tested exhaustively (FR-1 acceptance).
    fn all_hit_targets() -> Vec<HitTarget> {
        vec![
            HitTarget::Empty,
            HitTarget::TerrainCell,
            HitTarget::Node(entity(1)),
            HitTarget::Stamp(entity(2)),
            HitTarget::PathSegment { idx: 0 },
            HitTarget::PathVertex { idx: 0 },
            HitTarget::PathHandle {
                idx: 0,
                side: HandleSide::In,
            },
            HitTarget::TerrainProposal { id: "r1".into() },
            HitTarget::GimbalAxis,
        ]
    }

    #[test]
    fn empty_ground_and_terrain_cell_offer_creation_only() {
        let expect = vec![Verb::CreateNode, Verb::PlaceAsset];
        assert_eq!(menu_for(&HitTarget::Empty), expect);
        assert_eq!(menu_for(&HitTarget::TerrainCell), expect);
    }

    #[test]
    fn node_menu_is_the_ratified_node_set() {
        assert_eq!(
            menu_for(&HitTarget::Node(entity(1))),
            vec![
                Verb::EditProperties,
                Verb::Rename,
                Verb::Duplicate,
                Verb::ClearProperties,
                Verb::CopyApi,
                Verb::Report,
                Verb::Delete,
            ]
        );
    }

    #[test]
    fn stamp_menu_has_promote_scale_slide_and_delete() {
        let m = menu_for(&HitTarget::Stamp(entity(2)));
        assert_eq!(m[0], Verb::PromoteToNode);
        for v in [
            Verb::ScaleRotate,
            Verb::SlideAlongPath,
            Verb::CopyApi,
            Verb::Report,
            Verb::Delete,
        ] {
            assert!(m.contains(&v), "stamp menu missing {v:?}");
        }
        // A stamp is not a plain node — no clear-properties / rename verbs.
        assert!(!m.contains(&Verb::ClearProperties));
        assert!(!m.contains(&Verb::Rename));
    }

    #[test]
    fn path_segment_is_the_path_object_menu() {
        assert_eq!(
            menu_for(&HitTarget::PathSegment { idx: 3 }),
            vec![
                Verb::EditPath,
                Verb::AddStamps,
                Verb::CopyApi,
                Verb::Report,
                Verb::Delete,
            ]
        );
    }

    #[test]
    fn path_point_and_handle_share_the_point_menu() {
        let expect = vec![Verb::SetCornerSmooth, Verb::DeletePoint];
        assert_eq!(menu_for(&HitTarget::PathVertex { idx: 1 }), expect);
        assert_eq!(
            menu_for(&HitTarget::PathHandle {
                idx: 1,
                side: HandleSide::Out
            }),
            expect
        );
    }

    #[test]
    fn earthwork_region_reports_volume_and_deletes() {
        assert_eq!(
            menu_for(&HitTarget::TerrainProposal { id: "r1".into() }),
            vec![
                Verb::EditRegionParams,
                Verb::ReportVolume,
                Verb::CopyApi,
                Verb::Delete,
            ]
        );
    }

    #[test]
    fn gimbal_axis_has_no_object_menu() {
        assert!(menu_for(&HitTarget::GimbalAxis).is_empty());
    }

    #[test]
    fn every_deletable_object_offers_a_delete_verb() {
        for hit in all_hit_targets() {
            let m = menu_for(&hit);
            let deletable = matches!(
                hit,
                HitTarget::Node(_)
                    | HitTarget::Stamp(_)
                    | HitTarget::PathSegment { .. }
                    | HitTarget::TerrainProposal { .. }
            );
            let point = matches!(
                hit,
                HitTarget::PathVertex { .. } | HitTarget::PathHandle { .. }
            );
            if deletable {
                assert!(m.contains(&Verb::Delete), "{hit:?} should offer Delete");
            } else if point {
                assert!(
                    m.contains(&Verb::DeletePoint),
                    "{hit:?} should offer Delete Point"
                );
            } else {
                assert!(!m.contains(&Verb::Delete), "{hit:?} must not offer Delete");
            }
        }
    }

    #[test]
    fn clear_properties_is_a_node_only_verb_distinct_from_delete() {
        // The husk-bug distinction: only a node can clear-properties, and it is
        // never conflated with Delete in the same slot.
        for hit in all_hit_targets() {
            let m = menu_for(&hit);
            if m.contains(&Verb::ClearProperties) {
                assert!(matches!(hit, HitTarget::Node(_)), "{hit:?}");
                assert!(
                    m.contains(&Verb::Delete),
                    "clear + delete coexist on a node"
                );
            }
        }
    }

    #[test]
    fn labels_and_tooltips_are_present_for_every_verb() {
        // Exhaustive over the union of the tables — a missing arm fails to
        // compile (match is total) and every string is non-empty (calm chrome).
        for hit in all_hit_targets() {
            for verb in menu_for(&hit) {
                assert!(!verb_label(verb).is_empty(), "{verb:?} label");
                assert!(!verb_tooltip(verb).is_empty(), "{verb:?} tooltip");
                // The target-aware overlay must never blank a string either.
                assert!(!verb_label_for(verb, &hit).is_empty(), "{verb:?} label");
                assert!(!verb_tooltip_for(verb, &hit).is_empty(), "{verb:?} tip");
            }
        }
    }

    // --- F6/F20: a segment's Delete removes the WHOLE track — say so ---

    #[test]
    fn path_segment_delete_is_labelled_as_a_whole_track_delete() {
        // Under the "Path segment N" header, a plain "Delete" reads as if it
        // removed the segment. It tombstones the entire track.
        let segment = HitTarget::PathSegment { idx: 3 };
        let label = verb_label_for(Verb::Delete, &segment);
        assert!(
            label.to_lowercase().contains("track"),
            "segment delete label must name the track, got {label:?}"
        );
        assert_ne!(label, verb_label(Verb::Delete));
        let tooltip = verb_tooltip_for(Verb::Delete, &segment).to_lowercase();
        assert!(tooltip.contains("entire"), "tooltip must state the scope");
        assert!(
            tooltip.contains("no per-segment delete"),
            "tooltip must rule out the misread"
        );
    }

    #[test]
    fn the_whole_track_relabel_touches_nothing_else() {
        // Only Delete, only on a segment — every other pairing falls through
        // to the plain tables.
        let segment = HitTarget::PathSegment { idx: 0 };
        for verb in menu_for(&segment) {
            if verb == Verb::Delete {
                continue;
            }
            assert_eq!(verb_label_for(verb, &segment), verb_label(verb), "{verb:?}");
            assert_eq!(
                verb_tooltip_for(verb, &segment),
                verb_tooltip(verb),
                "{verb:?}"
            );
        }
        for hit in all_hit_targets() {
            if matches!(hit, HitTarget::PathSegment { .. }) {
                continue;
            }
            assert_eq!(
                verb_label_for(Verb::Delete, &hit),
                verb_label(Verb::Delete),
                "{hit:?} keeps the plain Delete label"
            );
        }
    }

    #[test]
    fn track_delete_confirm_names_the_scope_and_the_point_count() {
        let m = track_delete_confirm_message(Some(12));
        assert!(m.contains("12"), "the live point count is shown: {m:?}");
        assert!(m.to_lowercase().contains("whole track"));
        assert!(m.to_lowercase().contains("not just this segment"));
        assert!(m.to_lowercase().contains("cannot be undone"));
        // Singular reads correctly rather than "All 1 of its points".
        let one = track_delete_confirm_message(Some(1));
        assert!(one.to_lowercase().contains("single point"), "{one:?}");
        // Unknown count degrades honestly — never "0 points".
        let unknown = track_delete_confirm_message(None);
        assert!(!unknown.contains('0'), "{unknown:?}");
        assert!(unknown.to_lowercase().contains("whole track"));
    }

    #[test]
    fn only_egress_verbs_are_seam_gated() {
        assert!(verb_is_seam_gated(Verb::CopyApi));
        assert!(verb_is_seam_gated(Verb::Report));
        assert!(verb_is_seam_gated(Verb::ReportVolume));
        for v in [
            Verb::CreateNode,
            Verb::Delete,
            Verb::Duplicate,
            Verb::ClearProperties,
            Verb::Rename,
            Verb::PromoteToNode,
        ] {
            assert!(!verb_is_seam_gated(v), "{v:?} must not be seam-gated");
        }
    }

    // --- Rename routing: the Node Options prefill (the rename surface) ---

    fn tree_with_node() -> VerseManager {
        use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
        VerseManager::from_verses(vec![VerseEntry {
            id: "v1".into(),
            name: "V".into(),
            namespace_id: None,
            expanded: true,
            fractals: vec![FractalEntry {
                id: "f1".into(),
                name: "F".into(),
                expanded: true,
                petals: vec![PetalEntry {
                    id: "p1".into(),
                    name: "P".into(),
                    expanded: true,
                    nodes: vec![NodeEntry {
                        id: "n1".into(),
                        name: "Tower".into(),
                        has_asset: false,
                        position: [0.0; 3],
                        webpage_url: Some("https://example.com".into()),
                        asset_path: None,
                    }],
                }],
            }],
        }])
    }

    #[test]
    fn node_options_prefill_carries_current_name_and_url() {
        let dialog = node_options_prefill(&tree_with_node(), "n1");
        match dialog {
            ActiveDialog::NodeOptions {
                node_id,
                node_name_buf,
                webpage_url_buf,
                pending_delete,
                descendant_count,
            } => {
                assert_eq!(node_id, "n1");
                assert_eq!(node_name_buf, "Tower");
                assert_eq!(webpage_url_buf, "https://example.com");
                assert!(!pending_delete);
                assert!(descendant_count.is_none());
            }
            other => panic!("expected NodeOptions, got {other:?}"),
        }
    }

    #[test]
    fn node_options_prefill_unknown_node_defaults_to_empty_buffers() {
        let dialog = node_options_prefill(&tree_with_node(), "missing");
        match dialog {
            ActiveDialog::NodeOptions {
                node_id,
                node_name_buf,
                webpage_url_buf,
                ..
            } => {
                assert_eq!(node_id, "missing");
                assert!(node_name_buf.is_empty());
                assert!(webpage_url_buf.is_empty());
            }
            other => panic!("expected NodeOptions, got {other:?}"),
        }
    }

    #[test]
    fn context_target_payloads_construct_for_every_backed_kind() {
        // Node-backed target: node verbs key on `node_id`.
        let node = ContextTarget {
            hit: HitTarget::Node(entity(1)),
            node_id: Some("n1".into()),
            stamp: None,
        };
        assert_eq!(node.node_id.as_deref(), Some("n1"));
        // Stamp-backed target: stamp verbs key on `(track, index)`.
        let stamp = ContextTarget {
            hit: HitTarget::Stamp(entity(2)),
            node_id: None,
            stamp: Some(("track-1".into(), 4)),
        };
        assert_eq!(stamp.stamp, Some(("track-1".to_string(), 4)));
        assert!(!menu_for(&stamp.hit).is_empty());
    }

    #[test]
    fn verb_action_routes_node_verbs_and_selects_cascade() {
        assert!(matches!(
            verb_action(Verb::Delete, "n1", false),
            Some(UiAction::DeleteNode { cascade: false, .. })
        ));
        assert!(matches!(
            verb_action(Verb::Delete, "n1", true),
            Some(UiAction::DeleteNode { cascade: true, .. })
        ));
        assert!(matches!(
            verb_action(Verb::ClearProperties, "n1", false),
            Some(UiAction::ClearNodeProperties { .. })
        ));
        assert!(matches!(
            verb_action(Verb::CopyApi, "n1", false),
            Some(UiAction::CopyApiString { .. })
        ));
        assert!(matches!(
            verb_action(Verb::Report, "n1", false),
            Some(UiAction::ReportObject { .. })
        ));
        assert!(matches!(
            verb_action(Verb::Duplicate, "n1", false),
            Some(UiAction::DuplicateNode { .. })
        ));
        // Path/stamp-domain verbs are not this track's to route.
        assert!(verb_action(Verb::EditPath, "n1", false).is_none());
        assert!(verb_action(Verb::SlideAlongPath, "n1", false).is_none());
    }

    // --- D16 (finding #4): the 3 previously-stubbed verbs' real routing ---

    #[test]
    fn anchor_index_reads_vertex_and_handle_only() {
        assert_eq!(anchor_index(&HitTarget::PathVertex { idx: 3 }), Some(3));
        assert_eq!(
            anchor_index(&HitTarget::PathHandle {
                idx: 5,
                side: HandleSide::Out
            }),
            Some(5)
        );
        assert_eq!(anchor_index(&HitTarget::Node(entity(1))), None);
        assert_eq!(anchor_index(&HitTarget::Empty), None);
    }

    #[test]
    fn next_corner_kind_cycles_corner_smooth_symmetric() {
        assert_eq!(next_corner_kind(CornerKind::Corner), CornerKind::Smooth);
        assert_eq!(next_corner_kind(CornerKind::Smooth), CornerKind::Symmetric);
        assert_eq!(next_corner_kind(CornerKind::Symmetric), CornerKind::Corner);
    }

    #[test]
    fn point_verb_action_routes_set_corner_smooth_to_the_next_kind() {
        let hit = HitTarget::PathVertex { idx: 2 };
        let action = point_verb_action(Verb::SetCornerSmooth, &hit, "track-1", CornerKind::Corner);
        assert!(matches!(
            action,
            Some(UiAction::PathSetAnchorCorner {
                index: 2,
                corner: CornerKind::Smooth,
                ..
            })
        ));
        if let Some(UiAction::PathSetAnchorCorner { track_node_id, .. }) = action {
            assert_eq!(track_node_id, "track-1");
        }
    }

    #[test]
    fn point_verb_action_routes_delete_point_by_anchor_index() {
        let hit = HitTarget::PathHandle {
            idx: 4,
            side: HandleSide::In,
        };
        let action = point_verb_action(Verb::DeletePoint, &hit, "track-1", CornerKind::Corner);
        assert!(matches!(
            action,
            Some(UiAction::PathRemovePoint { index: 4, .. })
        ));
    }

    #[test]
    fn point_verb_action_none_for_non_anchor_hits_or_other_verbs() {
        assert!(point_verb_action(
            Verb::SetCornerSmooth,
            &HitTarget::Node(entity(1)),
            "track-1",
            CornerKind::Corner
        )
        .is_none());
        assert!(point_verb_action(
            Verb::Delete,
            &HitTarget::PathVertex { idx: 0 },
            "track-1",
            CornerKind::Corner
        )
        .is_none());
    }

    #[test]
    fn edit_region_actions_selects_and_reveals_terrain() {
        let hit = HitTarget::TerrainProposal { id: "r7".into() };
        let actions = edit_region_actions(Verb::EditRegionParams, &hit).expect("region hit");
        assert!(matches!(
            &actions[0],
            UiAction::TerrainProposalSelect { id } if id == "r7"
        ));
        assert!(matches!(
            &actions[1],
            UiAction::RevealSection { slug } if slug == "terrain"
        ));
    }

    #[test]
    fn edit_region_actions_none_for_other_verbs_or_targets() {
        assert!(edit_region_actions(Verb::EditRegionParams, &HitTarget::Node(entity(1))).is_none());
        assert!(edit_region_actions(
            Verb::Delete,
            &HitTarget::TerrainProposal { id: "r1".into() }
        )
        .is_none());
    }

    #[test]
    fn proposal_delete_action_only_for_terrain_proposal() {
        assert!(matches!(
            proposal_delete_action(&HitTarget::TerrainProposal { id: "r1".into() }),
            Some(UiAction::TerrainProposalDelete { id }) if id == "r1"
        ));
        assert!(proposal_delete_action(&HitTarget::Node(entity(1))).is_none());
        assert!(proposal_delete_action(&HitTarget::Stamp(entity(2))).is_none());
    }

    #[test]
    fn every_new_target_kind_has_at_least_one_wired_verb() {
        // Guards against a future stub regression: for each of the four
        // newly-resolvable target kinds, every verb in its `menu_for` set
        // routes to a real action via one of the routing fns above (or the
        // shared node-scoped `verb_action`/seam-gated egress path).
        let track = "track-1";
        for hit in [
            HitTarget::PathVertex { idx: 0 },
            HitTarget::PathHandle {
                idx: 0,
                side: HandleSide::In,
            },
        ] {
            for verb in menu_for(&hit) {
                assert!(
                    point_verb_action(verb, &hit, track, CornerKind::Corner).is_some(),
                    "{hit:?} verb {verb:?} has no routing"
                );
            }
        }
        let region = HitTarget::TerrainProposal { id: "r1".into() };
        for verb in menu_for(&region) {
            let routed = edit_region_actions(verb, &region).is_some()
                || proposal_delete_action(&region).is_some() && verb == Verb::Delete
                || matches!(verb, Verb::CopyApi | Verb::Report | Verb::ReportVolume); // seam-gated, node.rs pattern
            assert!(routed, "TerrainProposal verb {verb:?} has no routing");
        }
        // F20: PathSegment is reachable now (right-click on an edited ribbon),
        // so it joins the net. Delete/CopyApi/Report route through the shared
        // node-scoped map on the TRACK id; EditPath/AddStamps route inline via
        // `UiAction::PathSelectTrack` at the render site (no pure fn to call).
        let segment = HitTarget::PathSegment { idx: 2 };
        for verb in menu_for(&segment) {
            let routed = verb_action(verb, track, true).is_some()
                || matches!(verb, Verb::EditPath | Verb::AddStamps);
            assert!(routed, "PathSegment verb {verb:?} has no routing");
        }
        // …and that Delete is a whole-track cascade, labelled as one.
        assert!(matches!(
            verb_action(Verb::Delete, track, true),
            Some(UiAction::DeleteNode { cascade: true, .. })
        ));
        assert_ne!(
            verb_label_for(Verb::Delete, &segment),
            verb_label(Verb::Delete),
            "a segment's Delete must not read like a node's"
        );
    }
}
