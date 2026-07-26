//! Viewport right-click context menu (contextual_controls_20260725).
//!
//! The object-aware menu is a pure table: [`menu_for`] maps a viewport
//! [`HitTarget`] (the same classification `node_manager::dispatch` produces) to
//! the ordered [`Verb`] set valid for that object (FR-1). Rendering, labels, and
//! tooltips are thin functions over that table; the verb→`UiAction` wiring is in
//! [`verb_action`]. Rationale + the full verb matrix live in
//! `fe-ui/src/dialogs/AGENTS.md` §context-menu (N-7).

use bevy_egui::egui;

use super::ActiveDialog;
use crate::actions::{UiAction, UiManager};
use crate::node_manager::HitTarget;
use crate::theme;

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

/// Renders one verb button and reports whether it was clicked. Seam-gated verbs
/// whose seam string is absent render disabled-with-an-explanatory-hint (FR-4,
/// never silently absent). All verbs carry a hover tooltip (ui_ux §8).
pub(crate) fn render_verb_button(ui: &mut egui::Ui, verb: Verb, seam_available: bool) -> bool {
    let gated = verb_is_seam_gated(verb);
    let enabled = !gated || seam_available;
    let color = if matches!(verb, Verb::Delete | Verb::DeletePoint) {
        egui::Color32::from_rgb(230, 120, 120)
    } else {
        theme::TEXT_BRIGHT
    };
    let button = egui::Button::new(egui::RichText::new(verb_label(verb)).color(color))
        .fill(egui::Color32::TRANSPARENT);
    let resp = ui.add_enabled(enabled, button);
    if enabled {
        resp.on_hover_text(verb_tooltip(verb)).clicked()
    } else {
        resp.on_disabled_hover_text(
            "No API endpoint for this object yet \u{2014} lights up when the read/write \
             API surface (endpoint_api_surface) provides one.",
        );
        false
    }
}

/// Renders the viewport right-click context menu.
///
/// The `ActiveDialog::ContextMenu` variant carries only the cursor world
/// position, so this surface renders the empty-ground menu (create / place),
/// driven by [`menu_for`]. Object menus (node/stamp/path/region) render through
/// the same [`menu_for`]/[`render_verb_button`] machinery the moment the
/// `ContextMenu` variant carries a `HitTarget` — see `dialogs/AGENTS.md`
/// §context-menu for the one-line follow-up that lights them up. Node verbs are
/// live now via the Node Options surface (`node_options.rs`).
pub fn render_context_menu(ctx: &egui::Context, ui_mgr: &mut UiManager) {
    let ActiveDialog::ContextMenu {
        screen_pos,
        world_pos,
    } = ui_mgr.active_dialog
    else {
        return;
    };

    let pos = egui::pos2(screen_pos[0], screen_pos[1]);
    let world = world_pos;

    let mut next_dialog: Option<ActiveDialog> = None;
    let mut create_node_at: Option<[f32; 3]> = None;
    let mut close = false;

    // Empty-ground menu, built from the object-aware table (FR-1).
    let verbs = menu_for(&HitTarget::Empty);

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
                    ui.set_min_width(160.0);
                    for verb in verbs {
                        // Empty-ground verbs are never seam-gated.
                        if render_verb_button(ui, verb, true) {
                            match verb {
                                Verb::PlaceAsset => {
                                    next_dialog = Some(ActiveDialog::GltfImport {
                                        file_path_buf: String::new(),
                                        name_buf: String::new(),
                                        position: world,
                                    });
                                }
                                Verb::CreateNode => {
                                    create_node_at = Some(world);
                                    close = true;
                                }
                                _ => {}
                            }
                        }
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
                close = true;
            }
        }
    }

    if let Some(position) = create_node_at {
        ui_mgr.push_action(UiAction::CreateNodeAt { position });
    }
    if let Some(dialog) = next_dialog {
        ui_mgr.open_dialog(dialog);
    } else if close {
        ui_mgr.close_dialog();
    }
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
            }
        }
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
}
