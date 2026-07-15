//! Tools panel: a floating window (independent of `ActiveDialog`, like
//! `gis_panel`) that hosts the hexon-path-asset stamping controls. The
//! "Stamp along path" button emits `UiAction::PathAssetApply` for the track
//! currently being edited (`PathEditorState.editing_track_id`), building the
//! descriptor from the repetition/pattern controls. See
//! `fe-ui/src/panels/AGENTS.md` §tool-panel.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::PathEditorState;
use crate::node_manager::curve::{self, PenMode};
use crate::theme;
use crate::verse_manager::VerseManager;

/// A row in the path-asset picker: an installed model the user can re-stamp.
/// Sourced from `VerseManager` nodes carrying a resolvable `asset_path`
/// (`blob://{hash}.glb` or a local model path) — see `panels/AGENTS.md`
/// §tool-panel for why this needs no quarantine backend.
#[derive(Clone, PartialEq)]
pub struct AssetPickRow {
    pub name: String,
    pub asset_path: String,
}

/// Collect the installed, re-stampable assets from the loaded hierarchy:
/// every node whose `asset_path` is set, de-duplicated by `asset_path` so a
/// model re-used across nodes appears once. Pure — no ECS/egui — so it is
/// unit-testable without a Bevy `App`.
pub fn installed_assets(verse_mgr: &VerseManager) -> Vec<AssetPickRow> {
    let mut rows: Vec<AssetPickRow> = Vec::new();
    for node in verse_mgr.all_nodes() {
        let Some(path) = node.asset_path.as_ref() else {
            continue;
        };
        if path.trim().is_empty() || rows.iter().any(|r| &r.asset_path == path) {
            continue;
        }
        let name = if node.name.trim().is_empty() {
            path.clone()
        } else {
            node.name.clone()
        };
        rows.push(AssetPickRow {
            name,
            asset_path: path.clone(),
        });
    }
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    rows
}

/// Case-insensitive filter of picker rows by name or asset path. An empty
/// filter matches everything. Pure helper for the picker's filter field.
pub fn filter_assets<'a>(rows: &'a [AssetPickRow], filter: &str) -> Vec<&'a AssetPickRow> {
    let needle = filter.trim().to_lowercase();
    rows.iter()
        .filter(|r| {
            needle.is_empty()
                || r.name.to_lowercase().contains(&needle)
                || r.asset_path.to_lowercase().contains(&needle)
        })
        .collect()
}

/// How repeated instances along a path are spaced. Mirrors
/// `fe_sdk::path_asset::SpacingMode`; kept as a local egui-facing enum so the
/// panel state stays free of the SDK type, converted at emit time.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum SpacingMode {
    #[default]
    FixedSpacing,
    FixedCount,
}

impl SpacingMode {
    /// Map the panel's local spacing mode to the SDK descriptor enum.
    fn to_sdk(self) -> fe_sdk::path_asset::SpacingMode {
        match self {
            SpacingMode::FixedSpacing => fe_sdk::path_asset::SpacingMode::FixedSpacing,
            SpacingMode::FixedCount => fe_sdk::path_asset::SpacingMode::FixedCount,
        }
    }
}

/// Persistent state for the Tools panel (open flag + path-asset stamp
/// controls). `selected_hexon_ref` holds the picked asset's `asset_path`
/// (`blob://{hash}.glb`); the picker (FR-1b) or the collapsible manual-entry
/// fallback populate it. See `panels/AGENTS.md` §tool-panel.
#[derive(Resource, Default)]
pub struct ToolPanelState {
    pub open: bool,
    // --- W4 path-asset stamp controls (owned by the gis_tool_panel track) ---
    pub selected_hexon_ref: Option<String>,
    /// Filter-text buffer for the path-asset picker list.
    pub asset_filter: String,
    pub spacing_mode: SpacingMode,
    pub spacing_value: f32,
    pub count_value: u32,
    pub tangent_align: bool,
    // --- W7b pen-tool phase-2 controls (curves + shapes) — see AGENTS.md ---
    /// How placed control points become the final path (Polyline/Catmull/Bezier).
    pub pen_mode: PenMode,
    /// Sharp↔smooth sensitivity for `CatmullRom` (0 = round, 1 = sharp/straight).
    pub pen_tension: f32,
    /// Samples emitted per curve segment when smoothing.
    pub pen_samples_per_segment: usize,
    /// Radius used by the "Add Circle" / X-radius of "Add Ellipse" shape.
    pub shape_radius: f32,
    /// Z-radius for "Add Ellipse" (X-radius is `shape_radius`).
    pub shape_radius_z: f32,
    /// Segment count for generated shape rings.
    pub shape_segments: usize,
    /// Pen-tool actions queued during the egui pass; drained by
    /// `actions::process_ui_actions` (avoids threading `ui_mgr` through the
    /// `render_tool_panel` signature). See AGENTS.md §tool-panel.
    pub pending_actions: Vec<UiAction>,
}

impl ToolPanelState {
    /// Queues a pen-tool `UiAction` for the drain in `process_ui_actions`.
    pub fn queue_action(&mut self, action: UiAction) {
        self.pending_actions.push(action);
    }

    /// Drains queued pen-tool actions (called by `process_ui_actions`).
    pub fn drain_pending(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.pending_actions)
    }
}

pub fn render_tool_panel(
    ctx: &egui::Context,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
    verse_mgr: &VerseManager,
) {
    if !state.open {
        return;
    }

    let mut still_open = true;
    egui::Window::new("Tools")
        .open(&mut still_open)
        .resizable(true)
        .default_width(320.0)
        .min_width(280.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_DIALOG)
                .inner_margin(egui::Margin::same(12))
                .corner_radius(6.0)
                .stroke(egui::Stroke::new(1.0, theme::TEXT_DIM)),
        )
        .show(ctx, |ui| {
            render_path_asset_section(ui, state, ui_mgr, path_state, verse_mgr);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            render_pen_section(ui, state);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            render_terrain_tools_section(ui);
        });

    if !still_open {
        state.open = false;
    }
}

fn render_path_asset_section(
    ui: &mut egui::Ui,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
    verse_mgr: &VerseManager,
) {
    ui.label(
        egui::RichText::new("Path Asset")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    render_asset_picker(ui, state, verse_mgr);
    ui.add_space(6.0);

    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut state.spacing_mode,
            SpacingMode::FixedSpacing,
            "Fixed spacing",
        );
        ui.selectable_value(
            &mut state.spacing_mode,
            SpacingMode::FixedCount,
            "Fixed count",
        );
    });
    ui.add_space(4.0);

    match state.spacing_mode {
        SpacingMode::FixedSpacing => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Spacing")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(
                    egui::DragValue::new(&mut state.spacing_value)
                        .speed(0.1)
                        .range(0.0..=f32::MAX),
                );
            });
        }
        SpacingMode::FixedCount => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Count").small().color(theme::TEXT_DIM));
                ui.add(egui::DragValue::new(&mut state.count_value).range(0..=u32::MAX));
            });
        }
    }
    ui.add_space(4.0);

    ui.checkbox(&mut state.tangent_align, "Align to path tangent");
    ui.add_space(6.0);

    // The stamp target is the track currently being edited in the Paths tab.
    let target_track = path_state.editing_track_id.clone();
    let asset_ref = state.selected_hexon_ref.clone().unwrap_or_default();
    let can_stamp = target_track.is_some() && !asset_ref.trim().is_empty();

    // FR-4.1: name the target track so multi-track editors know what Stamp hits.
    if let Some(track_id) = target_track.as_deref() {
        let label = track_display_name(path_state, track_id);
        ui.label(
            egui::RichText::new(format!("Stamping onto: {label}"))
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add_space(4.0);
    }

    let resp = ui.add_enabled(can_stamp, egui::Button::new("Stamp along path"));
    if resp.clicked() {
        if let Some(track_node_id) = target_track.clone() {
            let descriptor = build_descriptor(state, asset_ref.clone());
            ui_mgr.push_action(UiAction::PathAssetApply {
                track_node_id,
                descriptor,
            });
        }
    }

    if target_track.is_none() {
        ui.label(
            egui::RichText::new("Select a track in the Paths tab to stamp along.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    } else if asset_ref.trim().is_empty() {
        ui.label(
            egui::RichText::new("Pick an asset above to stamp.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }
}

/// Resolve an edited track's display name from the Paths-tab track list,
/// falling back to the raw node id when no matching row is loaded.
fn track_display_name(path_state: &PathEditorState, track_id: &str) -> String {
    path_state
        .tracks
        .iter()
        .find(|t| t.node_id == track_id)
        .filter(|t| !t.name.trim().is_empty())
        .map(|t| t.name.clone())
        .unwrap_or_else(|| track_id.to_string())
}

/// FR-1(b) picker: a filterable, scrollable list of already-installed
/// re-stampable assets (from `VerseManager`), plus a collapsible manual
/// `blob://` entry fallback for power users. Row click sets
/// `state.selected_hexon_ref`; the emit path (`build_descriptor` + the Stamp
/// button) is untouched. See `panels/AGENTS.md` §tool-panel.
fn render_asset_picker(ui: &mut egui::Ui, state: &mut ToolPanelState, verse_mgr: &VerseManager) {
    // Currently-selected asset readout (clone out first so the closure can
    // mutate `selected_hexon_ref` on Clear without a borrow conflict).
    let selected = state
        .selected_hexon_ref
        .clone()
        .filter(|s| !s.trim().is_empty());
    match selected {
        Some(sel) => {
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Selected")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.label(
                    egui::RichText::new(&sel)
                        .small()
                        .monospace()
                        .color(theme::TEXT_BRIGHT),
                );
                if ui
                    .small_button("\u{2715}")
                    .on_hover_text("Clear selection")
                    .clicked()
                {
                    clear = true;
                }
            });
            if clear {
                state.selected_hexon_ref = None;
            }
        }
        None => {
            ui.label(
                egui::RichText::new("No asset selected")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
        }
    }
    ui.add_space(4.0);

    let rows = installed_assets(verse_mgr);
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(
                "No installed models found. Add a glb node, or paste a path below.",
            )
            .small()
            .color(theme::TEXT_MUTED),
        );
    } else {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Filter").small().color(theme::TEXT_DIM));
            ui.add(
                egui::TextEdit::singleline(&mut state.asset_filter)
                    .desired_width(180.0)
                    .hint_text("name or path"),
            );
        });
        ui.add_space(2.0);

        let filtered = filter_assets(&rows, &state.asset_filter);
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(140.0)
            .show(ui, |ui| {
                if filtered.is_empty() {
                    ui.label(
                        egui::RichText::new("No assets match the filter.")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                }
                for row in filtered {
                    let selected =
                        state.selected_hexon_ref.as_deref() == Some(row.asset_path.as_str());
                    let resp = ui
                        .selectable_label(selected, egui::RichText::new(&row.name).small())
                        .on_hover_text(&row.asset_path);
                    if resp.clicked() {
                        state.selected_hexon_ref = Some(row.asset_path.clone());
                    }
                }
            });
    }
    ui.add_space(4.0);

    // Secondary affordance: manual `blob://` entry for power users / paths not
    // yet surfaced as hierarchy nodes. The list above is the primary UX.
    egui::CollapsingHeader::new(
        egui::RichText::new("Or paste a blob:// path")
            .small()
            .color(theme::TEXT_DIM),
    )
    .default_open(false)
    .show(ui, |ui| {
        let mut asset_buf = state.selected_hexon_ref.clone().unwrap_or_default();
        if ui
            .add(egui::TextEdit::singleline(&mut asset_buf).hint_text("blob://<hash>.glb"))
            .changed()
        {
            state.selected_hexon_ref = if asset_buf.trim().is_empty() {
                None
            } else {
                Some(asset_buf)
            };
        }
    });
}

/// Build the SDK path-asset descriptor from the panel's control state.
fn build_descriptor(
    state: &ToolPanelState,
    asset_path: String,
) -> fe_sdk::path_asset::PathAssetDescriptor {
    fe_sdk::path_asset::PathAssetDescriptor {
        asset_path,
        spacing_mode: state.spacing_mode.to_sdk(),
        spacing_value: state.spacing_value,
        count: state.count_value,
        tangent_align: state.tangent_align,
    }
}

/// Pen-tool phase-2 controls: mode radio, sensitivity (tension) slider, a
/// "Smooth path" button (resample + replace the edited track's points), and
/// shape buttons (ellipse/circle). All buttons queue a `UiAction` into
/// `state.pending_actions`, drained by `process_ui_actions`. See
/// `node_manager/AGENTS.md` §pen-tool.
fn render_pen_section(ui: &mut egui::Ui, state: &mut ToolPanelState) {
    // `ToolPanelState` is `#[derive(Default)]` (shared with W4), so the pen
    // fields start at zero. Lazily seed usable defaults on first paint.
    if state.pen_samples_per_segment == 0 {
        state.pen_samples_per_segment = 12;
    }
    if state.shape_radius <= 0.0 {
        state.shape_radius = 5.0;
    }
    if state.shape_radius_z <= 0.0 {
        state.shape_radius_z = 3.0;
    }
    if state.shape_segments < 3 {
        state.shape_segments = 24;
    }

    ui.label(
        egui::RichText::new("Pen")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("Curve mode")
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut state.pen_mode,
            PenMode::Polyline,
            PenMode::Polyline.label(),
        );
        ui.selectable_value(
            &mut state.pen_mode,
            PenMode::CatmullRom,
            PenMode::CatmullRom.label(),
        );
        ui.selectable_value(
            &mut state.pen_mode,
            PenMode::Bezier,
            PenMode::Bezier.label(),
        );
    });
    ui.add_space(4.0);

    ui.label(
        egui::RichText::new("Sensitivity: sharp ↔ smooth")
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.add(egui::Slider::new(&mut state.pen_tension, 0.0..=1.0).show_value(false));
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Samples/segment")
                .small()
                .color(theme::TEXT_DIM),
        );
        let mut samples = state.pen_samples_per_segment as u32;
        if ui
            .add(egui::DragValue::new(&mut samples).range(1..=128))
            .changed()
        {
            state.pen_samples_per_segment = samples as usize;
        }
    });
    ui.add_space(4.0);

    if ui
        .add_enabled(
            state.pen_mode != PenMode::Polyline,
            egui::Button::new("Smooth path"),
        )
        .on_hover_text("Resample the edited track's points into the chosen curve")
        .clicked()
    {
        state.queue_action(UiAction::PathSmoothCurrent {
            mode: state.pen_mode,
            tension: state.pen_tension,
            samples_per_segment: state.pen_samples_per_segment.max(1),
        });
    }

    ui.add_space(8.0);
    ui.label(egui::RichText::new("Shapes").small().color(theme::TEXT_DIM));
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Radius X")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::DragValue::new(&mut state.shape_radius)
                .speed(0.1)
                .range(0.01..=f32::MAX),
        );
        ui.label(
            egui::RichText::new("Radius Z")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::DragValue::new(&mut state.shape_radius_z)
                .speed(0.1)
                .range(0.01..=f32::MAX),
        );
    });
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Segments")
                .small()
                .color(theme::TEXT_DIM),
        );
        let mut segs = state.shape_segments.max(3) as u32;
        if ui
            .add(egui::DragValue::new(&mut segs).range(3..=256))
            .changed()
        {
            state.shape_segments = segs as usize;
        }
    });
    ui.horizontal(|ui| {
        if ui
            .button("Add Ellipse")
            .on_hover_text("Append an ellipse ring to the edited track")
            .clicked()
        {
            let pts = curve::ellipse(
                [0.0, 0.0, 0.0],
                state.shape_radius,
                state.shape_radius_z,
                state.shape_segments.max(3),
            );
            state.queue_action(UiAction::PathAppendShape { points: pts });
        }
        if ui
            .button("Add Circle")
            .on_hover_text("Append a circle ring to the edited track")
            .clicked()
        {
            let pts = curve::circle(
                [0.0, 0.0, 0.0],
                state.shape_radius,
                state.shape_segments.max(3),
            );
            state.queue_action(UiAction::PathAppendShape { points: pts });
        }
        if ui
            .button("Add Rectangle")
            .on_hover_text("Append a rectangle (Radius X \u{00d7} Radius Z) to the edited track")
            .clicked()
        {
            let pts = curve::rectangle(
                [0.0, 0.0, 0.0],
                state.shape_radius * 2.0,
                state.shape_radius_z * 2.0,
            );
            state.queue_action(UiAction::PathAppendShape { points: pts });
        }
    });
    ui.label(
        egui::RichText::new("Shapes append at the origin; drag points to reposition.")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
}

fn render_terrain_tools_section(ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new("Terrain Tools")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("(placeholder — future terrain tools)")
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};

    /// Build a one-verse hierarchy from `(node_name, asset_path)` pairs.
    fn verse_with_nodes(nodes: &[(&str, Option<&str>)]) -> VerseManager {
        let node_entries = nodes
            .iter()
            .enumerate()
            .map(|(i, (name, path))| NodeEntry {
                id: format!("node-{i}"),
                name: (*name).to_string(),
                has_asset: path.is_some(),
                position: [0.0, 0.0, 0.0],
                webpage_url: None,
                asset_path: path.map(|p| p.to_string()),
            })
            .collect();
        let petal = PetalEntry {
            id: "petal-1".to_string(),
            name: "Petal".to_string(),
            expanded: true,
            nodes: node_entries,
        };
        let fractal = FractalEntry {
            id: "fractal-1".to_string(),
            name: "Fractal".to_string(),
            expanded: true,
            petals: vec![petal],
        };
        let verse = VerseEntry {
            id: "verse-1".to_string(),
            name: "Verse".to_string(),
            namespace_id: None,
            expanded: true,
            fractals: vec![fractal],
        };
        VerseManager {
            verses: vec![verse],
            ..Default::default()
        }
    }

    #[test]
    fn installed_assets_skips_pathless_dedups_and_sorts() {
        let vm = verse_with_nodes(&[
            ("Tree", Some("blob://tree.glb")),
            ("No model", None),
            ("Bench", Some("blob://bench.glb")),
            ("Tree copy", Some("blob://tree.glb")), // dupe path collapses
            ("Blank path", Some("   ")),            // whitespace-only skipped
        ]);
        let rows = installed_assets(&vm);
        // tree + bench only, deduped, sorted case-insensitively by name.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Bench");
        assert_eq!(rows[0].asset_path, "blob://bench.glb");
        assert_eq!(rows[1].name, "Tree");
        assert_eq!(rows[1].asset_path, "blob://tree.glb");
    }

    #[test]
    fn installed_assets_falls_back_to_path_for_unnamed_node() {
        let vm = verse_with_nodes(&[("", Some("blob://anon.glb"))]);
        let rows = installed_assets(&vm);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "blob://anon.glb");
    }

    #[test]
    fn filter_assets_matches_name_or_path_case_insensitively() {
        let rows = vec![
            AssetPickRow {
                name: "Oak Tree".to_string(),
                asset_path: "blob://oak.glb".to_string(),
            },
            AssetPickRow {
                name: "Park Bench".to_string(),
                asset_path: "blob://bench.glb".to_string(),
            },
        ];
        assert_eq!(filter_assets(&rows, "").len(), 2); // empty matches all
        assert_eq!(filter_assets(&rows, "tree").len(), 1); // by name
        assert_eq!(filter_assets(&rows, "BENCH").len(), 1); // by path, case-insensitive
        assert_eq!(filter_assets(&rows, "nope").len(), 0);
    }

    #[test]
    fn build_descriptor_maps_all_controls() {
        let state = ToolPanelState {
            open: true,
            selected_hexon_ref: Some("blob://model.glb".to_string()),
            spacing_mode: SpacingMode::FixedCount,
            spacing_value: 2.5,
            count_value: 7,
            tangent_align: true,
            ..Default::default()
        };
        let desc = build_descriptor(&state, "blob://model.glb".to_string());
        assert_eq!(desc.asset_path, "blob://model.glb");
        assert_eq!(
            desc.spacing_mode,
            fe_sdk::path_asset::SpacingMode::FixedCount
        );
        assert_eq!(desc.spacing_value, 2.5);
        assert_eq!(desc.count, 7);
        assert!(desc.tangent_align);
    }

    #[test]
    fn spacing_mode_to_sdk_roundtrips_both_variants() {
        assert_eq!(
            SpacingMode::FixedSpacing.to_sdk(),
            fe_sdk::path_asset::SpacingMode::FixedSpacing
        );
        assert_eq!(
            SpacingMode::FixedCount.to_sdk(),
            fe_sdk::path_asset::SpacingMode::FixedCount
        );
    }
}
