//! Path-tools + pen controls: pure rendering helpers called by
//! `ui_shell::right_sidebar::render_path_tools_section` (ui_shell_architecture
//! Phase 4 folded the former floating "Tools" window into the right-sidebar
//! PathTools section). The "Stamp along path" button emits
//! `UiAction::PathAssetApply` for the track currently being edited
//! (`PathEditorState.editing_track_id`), building the descriptor from the
//! repetition/pattern controls. See `fe-ui/src/panels/AGENTS.md` §tool-panel.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::{UiAction, UiManager};
use crate::gis::{CornerKind, PathEditorState};
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
    rows.sort_by_key(|a| a.name.to_lowercase());
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

/// FR-5 (terrain_editor_overhaul): the Cities-Skylines-style terrain edit
/// modes, kept as a panel-local enum (same idiom as `SpacingMode`) so this
/// file has no hard dependency on `terrain_proposal_state::ProposalOp`'s
/// exact derive set — converted at emit time via `to_proposal_op`. See
/// `panels/AGENTS.md` §terrain-tools and `panels/terrain_tools_panel.rs`.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum TerrainToolMode {
    #[default]
    Raise,
    Lower,
    Flatten,
    Ramp,
    Slope,
    Pad,
    Cut,
    Fill,
}

impl TerrainToolMode {
    pub fn label(self) -> &'static str {
        match self {
            TerrainToolMode::Raise => "Raise",
            TerrainToolMode::Lower => "Lower",
            TerrainToolMode::Flatten => "Flatten",
            TerrainToolMode::Ramp => "Ramp",
            TerrainToolMode::Slope => "Slope",
            TerrainToolMode::Pad => "Pad",
            TerrainToolMode::Cut => "Cut",
            TerrainToolMode::Fill => "Fill",
        }
    }

    pub const ALL: [TerrainToolMode; 8] = [
        TerrainToolMode::Raise,
        TerrainToolMode::Lower,
        TerrainToolMode::Flatten,
        TerrainToolMode::Ramp,
        TerrainToolMode::Slope,
        TerrainToolMode::Pad,
        TerrainToolMode::Cut,
        TerrainToolMode::Fill,
    ];

    /// Map to the cross-worker `ProposalOp` (owned by `terrain_proposal_state`,
    /// w4b). TODO(ultrapilot): confirm variant names line up 1:1 once that
    /// module lands; adjust this match if they diverge.
    pub fn to_proposal_op(self) -> crate::terrain_proposal_state::ProposalOp {
        use crate::terrain_proposal_state::ProposalOp;
        match self {
            TerrainToolMode::Raise => ProposalOp::Raise,
            TerrainToolMode::Lower => ProposalOp::Lower,
            TerrainToolMode::Flatten => ProposalOp::Flatten,
            TerrainToolMode::Ramp => ProposalOp::Ramp,
            TerrainToolMode::Slope => ProposalOp::Slope,
            TerrainToolMode::Pad => ProposalOp::Pad,
            TerrainToolMode::Cut => ProposalOp::Cut,
            TerrainToolMode::Fill => ProposalOp::Fill,
        }
    }
}

/// Persistent state for the path-tools section (path-asset stamp + pen +
/// terrain-tool controls). `selected_hexon_ref` holds the picked asset's
/// `asset_path` (`blob://{hash}.glb`); the picker (FR-1b) or the collapsible
/// manual-entry fallback populate it. See `panels/AGENTS.md` §tool-panel.
#[derive(Resource, Default)]
pub struct ToolPanelState {
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
    /// pen_curve_tool_20260722 (FR-4): anchor kind a plain (below-threshold)
    /// Pen click places — Corner = legacy sharp append; Smooth/Symmetric
    /// auto-derive collinear handles. Edited here (NFR-6: the tool inspector
    /// stays read-only), consumed by the Pen release decision.
    pub pen_new_anchor_kind: CornerKind,
    /// Radius used by the "Add Circle" / X-radius of "Add Ellipse" shape.
    pub shape_radius: f32,
    /// Z-radius for "Add Ellipse" (X-radius is `shape_radius`).
    pub shape_radius_z: f32,
    /// Segment count for generated shape rings.
    pub shape_segments: usize,
    /// Pen-tool actions queued during the egui pass; drained by
    /// `actions::process_ui_actions` (avoids threading `ui_mgr` through the
    /// `render_pen_section` signature). See AGENTS.md §tool-panel.
    pub pending_actions: Vec<UiAction>,
    // --- FR-5 terrain tool palette (terrain_editor_overhaul_20260718) ---
    // Reachability is now the right-sidebar `TerrainTools` section rail entry
    // (ui_shell_architecture Phase 4 retired the standalone-window pointer).
    pub terrain_tool_mode: TerrainToolMode,
    pub terrain_footprint_radius: f32,
    pub terrain_target_height: f32,
    pub terrain_delta: f32,
    // --- stamped_asset_nodes_20260725 (T2): per-stamp editor buffers ---
    /// Which instance index of the edited track's stamp group the stamp editor
    /// targets (select/promote/scale/rotate/slide). Position stays path-locked.
    pub stamp_edit_index: u32,
    /// Uniform per-stamp scale override to apply (FR-3). 1.0 = inherit base.
    pub stamp_scale: f32,
    /// Per-stamp yaw override in DEGREES about +Y (FR-3), converted to a
    /// quaternion at emit time.
    pub stamp_yaw_deg: f32,
    /// Arc-length "slide along path" offset in real METERS (FR-3, Q-1). N-1:
    /// meters only — no `world_scale` in the buffer.
    pub stamp_arc_m: f32,
}

const MAX_DISTANCE_INPUT: f32 = 1_000_000.0;
const MAX_SCALE_INPUT: f32 = 10_000.0;
const MAX_ANGLE_INPUT_DEG: f32 = 36_000.0;

fn sanitize_f32(value: &mut f32, default: f32, min: f32, max: f32) {
    *value = if !value.is_finite() || *value < min {
        default
    } else {
        (*value).min(max)
    };
}

impl ToolPanelState {
    /// Repair numeric UI buffers before egui constructs widgets from them.
    pub(crate) fn sanitize_numeric_state(&mut self) {
        sanitize_f32(&mut self.spacing_value, 1.0, 0.01, MAX_DISTANCE_INPUT);
        sanitize_f32(&mut self.pen_tension, 0.5, 0.0, 1.0);
        sanitize_f32(&mut self.shape_radius, 5.0, 0.01, MAX_DISTANCE_INPUT);
        sanitize_f32(&mut self.shape_radius_z, 3.0, 0.01, MAX_DISTANCE_INPUT);
        sanitize_f32(
            &mut self.terrain_footprint_radius,
            5.0,
            0.1,
            MAX_DISTANCE_INPUT,
        );
        sanitize_f32(
            &mut self.terrain_target_height,
            0.0,
            -MAX_DISTANCE_INPUT,
            MAX_DISTANCE_INPUT,
        );
        sanitize_f32(
            &mut self.terrain_delta,
            1.0,
            -MAX_DISTANCE_INPUT,
            MAX_DISTANCE_INPUT,
        );
        sanitize_f32(&mut self.stamp_scale, 1.0, 0.01, MAX_SCALE_INPUT);
        sanitize_f32(
            &mut self.stamp_yaw_deg,
            0.0,
            -MAX_ANGLE_INPUT_DEG,
            MAX_ANGLE_INPUT_DEG,
        );
        sanitize_f32(&mut self.stamp_arc_m, 0.0, 0.0, MAX_DISTANCE_INPUT);
        debug_assert!([
            self.spacing_value,
            self.pen_tension,
            self.shape_radius,
            self.shape_radius_z,
            self.terrain_footprint_radius,
            self.terrain_target_height,
            self.terrain_delta,
            self.stamp_scale,
            self.stamp_yaw_deg,
            self.stamp_arc_m,
        ]
        .into_iter()
        .all(f32::is_finite));
    }

    /// Queues a pen-tool `UiAction` for the drain in `process_ui_actions`.
    pub fn queue_action(&mut self, action: UiAction) {
        self.pending_actions.push(action);
    }

    /// Drains queued pen-tool actions (called by `process_ui_actions`).
    pub fn drain_pending(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.pending_actions)
    }
}

/// Path-asset stamp controls. Called directly by
/// `ui_shell::right_sidebar::render_path_tools_section` (the former
/// `render_tool_panel` floating-window shell was retired in Phase 4 — this
/// body moved verbatim, only the `egui::Window` wrapper was dropped).
pub(crate) fn render_path_asset_section(
    ui: &mut egui::Ui,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
    verse_mgr: &VerseManager,
) {
    state.sanitize_numeric_state();
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
                // FR-3: spacing is real meters (converted to world units via the
                // petal `world_scale` at sampling time).
                ui.label(
                    egui::RichText::new("Spacing (m)")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(
                    egui::DragValue::new(&mut state.spacing_value)
                        .speed(0.1)
                        .range(0.01..=MAX_DISTANCE_INPUT)
                        .suffix(" m"),
                )
                .on_hover_text("Distance between stamped instances, in real-world meters");
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
            egui::RichText::new("Select a path in the Paths tab to stamp along.")
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

    // stamped_asset_nodes_20260725 (T2): per-stamp editor — select an individual
    // stamp (promotes on first select, FR-2), then scale/rotate/slide it (FR-3).
    if let Some(track_id) = target_track {
        ui.add_space(8.0);
        ui.separator();
        render_stamp_editor(ui, state, ui_mgr, track_id);
    }
}

/// Convert a yaw angle (radians about +Y) to a rotation quaternion `[x,y,z,w]`
/// in the glTF/Bevy convention. Pure so the emit math is unit-testable.
pub(crate) fn yaw_to_quat(yaw_rad: f32) -> [f32; 4] {
    let half = yaw_rad * 0.5;
    [0.0, half.sin(), 0.0, half.cos()]
}

/// FR-2/FR-3 per-stamp editor (T2). Emits `SelectStamp`/`PromoteStamp` and the
/// scale/rotate/slide override actions for `track_id`'s stamp at
/// `state.stamp_edit_index`. Free translate is intentionally absent — position
/// stays path-derived; "Slide" is the only reposition (Q-1). Values are real
/// units (scale factor, degrees, meters) — N-1: no `world_scale` here.
fn render_stamp_editor(
    ui: &mut egui::Ui,
    state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    track_id: String,
) {
    state.sanitize_numeric_state();

    ui.label(
        egui::RichText::new("Selected Stamp")
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Index").small().color(theme::TEXT_DIM));
        ui.add(egui::DragValue::new(&mut state.stamp_edit_index).range(0..=u32::MAX))
            .on_hover_text("Which stamp along the path to edit (0-based)");
        if ui
            .button("Select")
            .on_hover_text("Select this stamp as an individual node (promotes on first select)")
            .clicked()
        {
            ui_mgr.push_action(UiAction::SelectStamp {
                track_node_id: track_id.clone(),
                stamp_index: state.stamp_edit_index as usize,
            });
        }
        if ui
            .button("Promote")
            .on_hover_text("Materialize this stamp as an addressable node (T1 promotion)")
            .clicked()
        {
            ui_mgr.push_action(UiAction::PromoteStamp {
                track_node_id: track_id.clone(),
                stamp_index: state.stamp_edit_index as usize,
            });
        }
    });
    ui.add_space(4.0);

    // Scale override (uniform) — position stays path-derived (no translate handle).
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Scale").small().color(theme::TEXT_DIM));
        ui.add(
            egui::DragValue::new(&mut state.stamp_scale)
                .speed(0.05)
                .range(0.01..=MAX_SCALE_INPUT),
        );
        if ui.button("Apply").clicked() {
            let s = state.stamp_scale;
            ui_mgr.push_action(UiAction::SetStampScale {
                track_node_id: track_id.clone(),
                stamp_index: state.stamp_edit_index as usize,
                scale: [s, s, s],
            });
        }
    });

    // Rotation override — yaw about +Y, entered in degrees.
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Rotate (°)")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::DragValue::new(&mut state.stamp_yaw_deg)
                .speed(1.0)
                .range(-MAX_ANGLE_INPUT_DEG..=MAX_ANGLE_INPUT_DEG),
        );
        if ui.button("Apply").clicked() {
            let quat = yaw_to_quat(state.stamp_yaw_deg.to_radians());
            ui_mgr.push_action(UiAction::SetStampRotation {
                track_node_id: track_id.clone(),
                stamp_index: state.stamp_edit_index as usize,
                rotation: quat,
            });
        }
    });

    // Slide along path — 1-D arc-length reposition in real meters (Q-1).
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Slide (m)")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::DragValue::new(&mut state.stamp_arc_m)
                .speed(0.1)
                .range(0.0..=MAX_DISTANCE_INPUT)
                .suffix(" m"),
        )
        .on_hover_text(
            "Reposition the stamp along its curve by arc length (free translate stays off)",
        );
        if ui.button("Apply").clicked() {
            ui_mgr.push_action(UiAction::SlideStampAlongPath {
                track_node_id: track_id.clone(),
                stamp_index: state.stamp_edit_index as usize,
                arc_length: state.stamp_arc_m,
            });
        }
    });
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
/// `state.pending_actions`, drained by `process_ui_actions`. Called directly
/// by `ui_shell::right_sidebar::render_path_tools_section` (see
/// `render_path_asset_section`'s doc for the Phase-4 window retirement). See
/// `node_manager/AGENTS.md` §pen-tool.
pub(crate) fn render_pen_section(ui: &mut egui::Ui, state: &mut ToolPanelState) {
    state.sanitize_numeric_state();
    // `ToolPanelState` is `#[derive(Default)]` (shared with W4), so the pen
    // fields start at zero. Lazily seed usable defaults on first paint.
    if state.pen_samples_per_segment == 0 {
        state.pen_samples_per_segment = 12;
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

    // pen_curve_tool_20260722 (FR-4/FR-6): the tool-level default anchor kind
    // a plain Pen click places, read by the release decision.
    ui.label(
        egui::RichText::new("New anchor")
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.horizontal(|ui| {
        ui.selectable_value(
            &mut state.pen_new_anchor_kind,
            CornerKind::Corner,
            CornerKind::Corner.label(),
        );
        ui.selectable_value(
            &mut state.pen_new_anchor_kind,
            CornerKind::Smooth,
            CornerKind::Smooth.label(),
        );
        ui.selectable_value(
            &mut state.pen_new_anchor_kind,
            CornerKind::Symmetric,
            CornerKind::Symmetric.label(),
        );
    })
    .response
    .on_hover_text("Anchor kind a plain Pen click places (press-drag always pulls out handles)");
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
        .on_hover_text("Resample the edited path's points into the chosen curve")
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
                .range(0.01..=MAX_DISTANCE_INPUT),
        );
        ui.label(
            egui::RichText::new("Radius Z")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::DragValue::new(&mut state.shape_radius_z)
                .speed(0.1)
                .range(0.01..=MAX_DISTANCE_INPUT),
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
            .on_hover_text("Append an ellipse ring to the edited path")
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
            .on_hover_text("Append a circle ring to the edited path")
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
            .on_hover_text("Append a rectangle (Radius X \u{00d7} Radius Z) to the edited path")
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
    fn yaw_to_quat_identity_and_ninety() {
        // 0° → identity quaternion.
        let q = yaw_to_quat(0.0);
        assert!((q[0]).abs() < 1e-6 && (q[1]).abs() < 1e-6 && (q[2]).abs() < 1e-6);
        assert!((q[3] - 1.0).abs() < 1e-6);
        // 90° about +Y → (0, sin45, 0, cos45).
        let q = yaw_to_quat(std::f32::consts::FRAC_PI_2);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!((q[1] - s).abs() < 1e-5, "y = sin45, got {}", q[1]);
        assert!((q[3] - s).abs() < 1e-5, "w = cos45, got {}", q[3]);
        assert!(q[0].abs() < 1e-6 && q[2].abs() < 1e-6);
    }

    #[test]
    fn yaw_to_quat_is_unit_length() {
        for deg in [0.0f32, 30.0, 45.0, 180.0, 270.0, -90.0] {
            let q = yaw_to_quat(deg.to_radians());
            let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((n - 1.0).abs() < 1e-5, "quat not unit for {deg}°: {n}");
        }
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

    #[test]
    fn terrain_tool_mode_labels_are_all_distinct_and_nonempty() {
        let mut labels: Vec<&str> = TerrainToolMode::ALL.iter().map(|m| m.label()).collect();
        assert!(labels.iter().all(|l| !l.is_empty()));
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), TerrainToolMode::ALL.len());
    }

    // TODO(ultrapilot): depends on `crate::terrain_proposal_state::ProposalOp`
    // (w4b) — will compile once that module lands. Locks the FR-5 op mapping.
    #[test]
    fn terrain_tool_mode_maps_1to1_onto_proposal_op() {
        use crate::terrain_proposal_state::ProposalOp;
        assert_eq!(TerrainToolMode::Raise.to_proposal_op(), ProposalOp::Raise);
        assert_eq!(TerrainToolMode::Lower.to_proposal_op(), ProposalOp::Lower);
        assert_eq!(
            TerrainToolMode::Flatten.to_proposal_op(),
            ProposalOp::Flatten
        );
        assert_eq!(TerrainToolMode::Ramp.to_proposal_op(), ProposalOp::Ramp);
        assert_eq!(TerrainToolMode::Slope.to_proposal_op(), ProposalOp::Slope);
        assert_eq!(TerrainToolMode::Pad.to_proposal_op(), ProposalOp::Pad);
        assert_eq!(TerrainToolMode::Cut.to_proposal_op(), ProposalOp::Cut);
        assert_eq!(TerrainToolMode::Fill.to_proposal_op(), ProposalOp::Fill);
    }

    #[test]
    fn numeric_state_recovers_from_nan_and_infinity() {
        let mut state = ToolPanelState {
            spacing_value: f32::NAN,
            pen_tension: f32::INFINITY,
            shape_radius: f32::NEG_INFINITY,
            shape_radius_z: f32::NAN,
            terrain_footprint_radius: f32::INFINITY,
            terrain_target_height: f32::NAN,
            terrain_delta: f32::NEG_INFINITY,
            stamp_scale: f32::NAN,
            stamp_yaw_deg: f32::INFINITY,
            stamp_arc_m: f32::NEG_INFINITY,
            ..Default::default()
        };
        state.sanitize_numeric_state();
        assert_eq!(state.shape_radius_z, 3.0);
        assert_eq!(state.stamp_scale, 1.0);
        assert!([
            state.spacing_value,
            state.pen_tension,
            state.shape_radius,
            state.shape_radius_z,
            state.terrain_footprint_radius,
            state.terrain_target_height,
            state.terrain_delta,
            state.stamp_scale,
            state.stamp_yaw_deg,
            state.stamp_arc_m,
        ]
        .into_iter()
        .all(f32::is_finite));
    }
}
