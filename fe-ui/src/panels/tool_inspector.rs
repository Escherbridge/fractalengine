//! tool_inspector_ux_20260719 (Phase 1): the active tool as a legible UI MODE.
//!
//! A left `SidePanel` that renders the ACTIVE tool's inspector — its Use zone
//! (what the mode lets you do) and Settings zone (its knobs) — plus a live
//! selection readout and a "gimbal active" affordance so users always know what
//! the current mode enables (FR-1/FR-2/FR-7). The per-tool descriptor, the
//! gimbal affordance, the selection summary, and the active-mode emphasis are
//! all PURE helpers (unit-tested); the egui paint stays thin (validated in-app).
//! Later phases fill the Use/Settings zones with real controls + fold in
//! `tool_panel.rs`. See `fe-ui/src/panels/AGENTS.md` §tool-inspector.

use bevy_egui::egui;

use crate::gis::{PathEditorState, PathPointRow};
use crate::node_manager::{project_selection, NodeManager, SelectionKind};
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::theme;

/// A per-tool inspector descriptor: the mode's title, a one-line "what this mode
/// does", and the labels shown in its Use / Settings zones. Pure data so the
/// tool→panel mapping is unit-testable. Zone entries marked "(soon)" are Phase
/// 2+ placeholders — the scaffold is calm and never blank (`ui_ux.md §7`).
pub(crate) struct ToolPanelDescriptor {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub use_zone: &'static [&'static str],
    pub settings_zone: &'static [&'static str],
}

/// Map the active [`Tool`] to its inspector descriptor. Pure + total.
pub(crate) fn panel_descriptor(tool: Tool) -> ToolPanelDescriptor {
    match tool {
        Tool::Select => ToolPanelDescriptor {
            title: "Select",
            subtitle: "Pick and inspect objects and path elements",
            use_zone: &[
                "Click an object to select it",
                "Click a path point or segment to edit it",
                "Click empty space to deselect",
            ],
            settings_zone: &["Selection filters (soon)", "Highlight style (soon)"],
        },
        Tool::Move => ToolPanelDescriptor {
            title: "Move",
            subtitle: "Translate the selection along a gimbal axis",
            use_zone: &[
                "Drag a gimbal axis to move",
                "Ctrl+drag a path point to change its height",
            ],
            settings_zone: &["Axis lock X / Y / Z (soon)", "Grid snap, m (soon)"],
        },
        Tool::Rotate => ToolPanelDescriptor {
            title: "Rotate",
            subtitle: "Spin the selection about a gimbal ring",
            use_zone: &["Drag a gimbal ring to rotate"],
            settings_zone: &["Angle snap 45° / 90° (soon)", "Pivot (soon)"],
        },
        Tool::Scale => ToolPanelDescriptor {
            title: "Scale",
            subtitle: "Resize the selection along a gimbal axis",
            use_zone: &["Drag a gimbal axis to scale"],
            settings_zone: &["Uniform / per-axis (soon)", "Snap increment (soon)"],
        },
        Tool::Pen => ToolPanelDescriptor {
            title: "Pen",
            subtitle: "Draw a path by clicking the viewport",
            use_zone: &[
                "Click the terrain to add a point",
                "Press-drag to pull out curve handles",
                "Alt mid-drag breaks the handles apart",
                "A gimbal on a selected point stays grabbable",
            ],
            settings_zone: &[
                "New anchor type — Tools window, Pen",
                "Corner settings — Paths tab, selected vertex",
            ],
        },
    }
}

/// One-line "gimbal active" affordance for the current `(tool, selection)`, or
/// `None` when no transform gimbal is shown. Mirrors the ratified draw/grab rule
/// (2026-07-19 "grab it wherever it's shown"): a path vertex/segment gimbal is
/// shown as a Move handle in EVERY tool, so its affordance is always present;
/// an entity / whole-track gimbal is shown only in the transform tools. Pure so
/// it stays in lockstep with `gimbal_interaction.rs`'s draw/interact logic.
pub(crate) fn gimbal_affordance_label(tool: Tool, kind: &SelectionKind) -> Option<&'static str> {
    let transform_tool = matches!(tool, Tool::Move | Tool::Rotate | Tool::Scale);
    match kind {
        SelectionKind::PathVertex { .. } | SelectionKind::PathSegment { .. } => {
            Some("Gimbal active — drag an axis to move this point")
        }
        SelectionKind::Node(_) | SelectionKind::Stamp(_) | SelectionKind::PathTrack { .. }
            if transform_tool =>
        {
            Some(match tool {
                Tool::Rotate => "Gimbal active — drag a ring to rotate",
                Tool::Scale => "Gimbal active — drag an axis to scale",
                _ => "Gimbal active — drag an axis to move",
            })
        }
        _ => None,
    }
}

/// Human-readable summary of the current selection for the inspector readout.
/// Pure. Keeps the two selection authorities distinguishable in text (a node vs.
/// a path element never read the same) — the textual mirror of `ui_ux.md §5`.
pub(crate) fn selection_summary(kind: &SelectionKind) -> String {
    match kind {
        SelectionKind::Empty => "Nothing selected".to_string(),
        SelectionKind::Node(_) => "Object (scene node)".to_string(),
        SelectionKind::PathTrack { track_id } => format!("Path track: {track_id}"),
        SelectionKind::PathVertex { idx, .. } => format!("Path point #{idx}"),
        SelectionKind::PathSegment { idx, .. } => format!("Path segment #{idx}"),
        SelectionKind::Stamp(_) => "Path-asset stamp".to_string(),
        SelectionKind::TerrainProposal { id } => format!("Terrain proposal: {id}"),
    }
}

/// Fill color for a tool-MODE button: the active mode reads via LUMINANCE (a
/// brighter neutral), not a saturated hue (`ui_ux.md §1`). Pure — the toolbar
/// calls it so the emphasis decision lives in one tested place.
pub(crate) fn mode_button_fill(active: bool) -> egui::Color32 {
    if active {
        theme::BG_MODE_ACTIVE
    } else {
        theme::BG_BUTTON
    }
}

/// pen_curve_tool_20260722 (FR-6): read-only per-anchor readout for the
/// inspector — the selected vertex's corner kind + smoothness (unitless 0..1).
/// Pure; `None` unless the selection is an in-range path vertex. The EDITABLE
/// corner settings live in the Paths card / Tools window (NFR-6).
pub(crate) fn anchor_readout(kind: &SelectionKind, points: &[PathPointRow]) -> Option<String> {
    let SelectionKind::PathVertex { idx, .. } = kind else {
        return None;
    };
    let row = points.get(*idx)?;
    Some(format!(
        "Anchor #{idx}: {}, smoothness {:.2}",
        row.corner.label(),
        row.smoothness
    ))
}

/// Downgrade a path selection whose index has outlived the current track's points
/// to `Empty`, so the readout never shows a phantom "Path point #N" /
/// "Gimbal active" for a stale index (cascade-delete can leave `selected_point` /
/// `selected_segment` dangling). Pure. A vertex needs `idx < len`; a segment spans
/// two anchors, so it needs `idx + 1 < len`. Mirrors the drag's own guard
/// (`path_gimbal_target` returns `None` for an out-of-range target).
pub(crate) fn fresh_path_selection(kind: SelectionKind, points_len: usize) -> SelectionKind {
    match kind {
        SelectionKind::PathVertex { idx, .. } if idx >= points_len => SelectionKind::Empty,
        SelectionKind::PathSegment { idx, .. } if idx + 1 >= points_len => SelectionKind::Empty,
        other => other,
    }
}

/// Left `SidePanel` rendering the active tool's inspector. Reads only state
/// already threaded into `gardener_console` (no signature change): the active
/// tool, plus the two selection authorities projected via `project_selection`.
pub(crate) fn tool_inspector_panel(
    ctx: &egui::Context,
    tool: &ToolState,
    node_mgr: &NodeManager,
    path_state: &PathEditorState,
) {
    let desc = panel_descriptor(tool.active_tool);
    let kind = project_selection(
        node_mgr
            .selected
            .as_ref()
            .map(|s| (s.entity, s.node_id.as_str())),
        path_state.editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );
    // Guard against a stale selected index outliving its points (see helper).
    let kind = fresh_path_selection(kind, path_state.points.len());

    egui::SidePanel::left("tool_inspector")
        .resizable(true)
        .default_width(200.0)
        .width_range(160.0..=320.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(8)),
        )
        .show(ctx, |ui| {
            // Mode header — the whole panel is the active MODE.
            ui.label(
                egui::RichText::new(desc.title)
                    .strong()
                    .color(theme::TEXT_HEADING),
            );
            ui.label(
                egui::RichText::new(desc.subtitle)
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();

            zone(ui, "USE", desc.use_zone);
            ui.add_space(6.0);
            zone(ui, "SETTINGS", desc.settings_zone);

            ui.add_space(6.0);
            ui.separator();
            ui.label(
                egui::RichText::new("SELECTION")
                    .small()
                    .color(theme::TEXT_SECTION),
            );
            ui.label(
                egui::RichText::new(selection_summary(&kind))
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            // FR-6 read-only per-anchor affordance (edit in the Paths card).
            if let Some(readout) = anchor_readout(&kind, &path_state.points) {
                ui.label(
                    egui::RichText::new(readout)
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            if let Some(affordance) = gimbal_affordance_label(tool.active_tool, &kind) {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(affordance)
                        .small()
                        .color(theme::TEXT_STRONG),
                );
            }
        });
}

/// Render one labelled zone (USE / SETTINGS) as a section header + bullet lines.
fn zone(ui: &mut egui::Ui, label: &str, items: &[&str]) {
    ui.label(
        egui::RichText::new(label)
            .small()
            .color(theme::TEXT_SECTION),
    );
    for item in items {
        ui.label(
            egui::RichText::new(format!("• {item}"))
                .small()
                .color(theme::TEXT_MUTED),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Entity;

    fn all_tools() -> [Tool; 5] {
        [
            Tool::Select,
            Tool::Move,
            Tool::Rotate,
            Tool::Scale,
            Tool::Pen,
        ]
    }

    #[test]
    fn every_tool_has_a_nonempty_descriptor() {
        for tool in all_tools() {
            let d = panel_descriptor(tool);
            assert!(!d.title.is_empty(), "{tool:?} has empty title");
            assert!(!d.subtitle.is_empty(), "{tool:?} has empty subtitle");
            // Never a blank panel (ui_ux.md §7): both zones carry at least one line.
            assert!(!d.use_zone.is_empty(), "{tool:?} has empty Use zone");
            assert!(
                !d.settings_zone.is_empty(),
                "{tool:?} has empty Settings zone"
            );
        }
    }

    #[test]
    fn descriptor_titles_are_distinct_per_tool() {
        let titles: Vec<&str> = all_tools()
            .iter()
            .map(|t| panel_descriptor(*t).title)
            .collect();
        let mut sorted = titles.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            titles.len(),
            "duplicate mode titles: {titles:?}"
        );
    }

    #[test]
    fn path_element_gimbal_affordance_shows_in_every_tool() {
        // "Grab it wherever it's shown": a selected vertex/segment always offers
        // the gimbal, so the affordance is present in every tool.
        let vertex = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 0,
        };
        let segment = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 0,
        };
        for tool in all_tools() {
            assert!(
                gimbal_affordance_label(tool, &vertex).is_some(),
                "vertex in {tool:?}"
            );
            assert!(
                gimbal_affordance_label(tool, &segment).is_some(),
                "segment in {tool:?}"
            );
        }
    }

    #[test]
    fn entity_gimbal_affordance_only_in_transform_tools() {
        let node = SelectionKind::Node(Entity::from_bits(1));
        assert!(gimbal_affordance_label(Tool::Move, &node).is_some());
        assert!(gimbal_affordance_label(Tool::Rotate, &node).is_some());
        assert!(gimbal_affordance_label(Tool::Scale, &node).is_some());
        // Select / Pen keep the entity gimbal off, so no affordance.
        assert!(gimbal_affordance_label(Tool::Select, &node).is_none());
        assert!(gimbal_affordance_label(Tool::Pen, &node).is_none());
    }

    #[test]
    fn no_affordance_for_empty_selection() {
        for tool in all_tools() {
            assert!(gimbal_affordance_label(tool, &SelectionKind::Empty).is_none());
        }
    }

    #[test]
    fn stamp_and_whole_track_follow_the_entity_gimbal_rule() {
        // Both use the entity gimbal, which is live only in the transform tools.
        let stamp = SelectionKind::Stamp(Entity::from_bits(2));
        let track = SelectionKind::PathTrack {
            track_id: "t".into(),
        };
        for kind in [&stamp, &track] {
            assert!(gimbal_affordance_label(Tool::Move, kind).is_some());
            assert!(gimbal_affordance_label(Tool::Rotate, kind).is_some());
            assert!(gimbal_affordance_label(Tool::Scale, kind).is_some());
            assert!(gimbal_affordance_label(Tool::Select, kind).is_none());
            assert!(gimbal_affordance_label(Tool::Pen, kind).is_none());
        }
    }

    #[test]
    fn terrain_proposal_never_shows_a_gimbal_affordance() {
        let p = SelectionKind::TerrainProposal { id: "p1".into() };
        for tool in all_tools() {
            assert!(gimbal_affordance_label(tool, &p).is_none());
        }
    }

    #[test]
    fn rotate_and_scale_affordances_name_their_gesture() {
        let node = SelectionKind::Node(Entity::from_bits(1));
        assert!(gimbal_affordance_label(Tool::Rotate, &node)
            .unwrap()
            .contains("rotate"));
        assert!(gimbal_affordance_label(Tool::Scale, &node)
            .unwrap()
            .contains("scale"));
    }

    #[test]
    fn selection_summary_distinguishes_authorities() {
        // A node and a path element must never read identically (ui_ux.md §5).
        let node = selection_summary(&SelectionKind::Node(Entity::from_bits(1)));
        let vertex = selection_summary(&SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 3,
        });
        assert_ne!(node, vertex);
        assert!(
            vertex.contains('3'),
            "vertex summary names its index: {vertex}"
        );
        assert_eq!(selection_summary(&SelectionKind::Empty), "Nothing selected");
    }

    #[test]
    fn active_mode_reads_brighter_than_inactive() {
        // Emphasis via luminance, not hue (ui_ux.md §1): the active fill must be
        // strictly brighter than the inactive one.
        let lum =
            |c: egui::Color32| 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
        let active = mode_button_fill(true);
        let inactive = mode_button_fill(false);
        assert!(
            lum(active) > lum(inactive),
            "active {active:?} must be brighter than inactive {inactive:?}"
        );
    }

    #[test]
    fn stale_vertex_index_downgrades_to_empty() {
        let stale = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 5,
        };
        assert!(matches!(
            fresh_path_selection(stale, 3),
            SelectionKind::Empty
        ));
        let fresh = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 2,
        };
        assert!(matches!(
            fresh_path_selection(fresh, 3),
            SelectionKind::PathVertex { idx: 2, .. }
        ));
    }

    #[test]
    fn stale_segment_index_downgrades_to_empty() {
        // A segment spans points[idx]..=points[idx+1], so idx+1 must be in range.
        let stale = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 2, // needs len >= 4; len 3 leaves points[3] missing
        };
        assert!(matches!(
            fresh_path_selection(stale, 3),
            SelectionKind::Empty
        ));
        let fresh = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 1, // points[1]..=points[2] both exist for len 3
        };
        assert!(matches!(
            fresh_path_selection(fresh, 3),
            SelectionKind::PathSegment { idx: 1, .. }
        ));
    }

    #[test]
    fn anchor_readout_names_kind_and_smoothness() {
        // pen_curve_tool_20260722 (FR-6): the read-only per-anchor affordance.
        let points = vec![
            PathPointRow::default(),
            PathPointRow {
                position: [1.0, 0.0, 0.0],
                corner: crate::gis::CornerKind::Smooth,
                smoothness: 0.5,
                ..Default::default()
            },
        ];
        let kind = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 1,
        };
        let readout = anchor_readout(&kind, &points).expect("in-range vertex");
        assert!(readout.contains("#1"), "names its index: {readout}");
        assert!(readout.contains("Smooth"), "names its kind: {readout}");
        assert!(readout.contains("0.50"), "names its smoothness: {readout}");
    }

    #[test]
    fn anchor_readout_none_for_non_vertex_or_stale() {
        let points = vec![PathPointRow::default()];
        // Non-vertex selections have no per-anchor readout.
        assert!(anchor_readout(&SelectionKind::Empty, &points).is_none());
        let seg = SelectionKind::PathSegment {
            track_id: "t".into(),
            idx: 0,
        };
        assert!(anchor_readout(&seg, &points).is_none());
        // An out-of-range index never fabricates a phantom anchor.
        let stale = SelectionKind::PathVertex {
            track_id: "t".into(),
            idx: 7,
        };
        assert!(anchor_readout(&stale, &points).is_none());
    }

    #[test]
    fn non_path_selection_passes_through_fresh_guard() {
        // A node selection is unaffected by the path points count.
        let node = SelectionKind::Node(Entity::from_bits(7));
        assert!(matches!(
            fresh_path_selection(node, 0),
            SelectionKind::Node(_)
        ));
        assert!(matches!(
            fresh_path_selection(SelectionKind::Empty, 0),
            SelectionKind::Empty
        ));
    }
}
