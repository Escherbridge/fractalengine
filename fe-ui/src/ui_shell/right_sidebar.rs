//! Right-sidebar area manager (FR-6): the single right region, one section at a
//! time (RATIFIED Q-2). `active_section` precedence is portal, then explicit
//! toggle, then selection-default; the Inspector is the never-blank fallback.
//! There is ONE render fn per section variant. Phase 4 (FR-9) filled
//! PathTools/TerrainTools/ProposalReport by dissolving the three floating
//! windows into this region (bodies delegate to pure helpers still homed in
//! `panels::{tool_panel, terrain_tools_panel, proposal_report_panel}`); Phase 5
//! (FR-8) fills the `Tool` section with the retired left tool-inspector panel's
//! live readouts (delegates to pure helpers homed in `panels::tool_inspector`).
//! All five sections are filled. See `fe-ui/src/ui_shell/AGENTS.md` §right.

use std::collections::HashMap;

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::terrain_proposal::SculptToolState;
use crate::actions::{UiAction, UiManager};
use crate::asset_ops::AssetDownloadStatus;
use crate::dialogs::ActiveDialog;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::{project_selection, NodeManager};
use crate::panels::tool_inspector::{
    anchor_readout, fresh_path_selection, gimbal_affordance_label, panel_descriptor,
    selection_summary,
};
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::{
    inspector, portal_toolbar, proposal_report_panel, terrain_tools_panel, tool_panel,
};
use crate::plugin::{InspectorFormState, LocalUserRole, ToolState};
use crate::settings::AppSettings;
use crate::terrain_map::dto::{HexonManagerTab, StorageInfoDto};
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalEditState;
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// The mutually-exclusive right-sidebar sections (one at a time — never-double).
/// `Settings` + `Maps` (FR-1/FR-2, D-A10) are ordinary sections here now — the
/// former floating Settings/Map-Manager modals. NOTE (fold rule): T3's sculpt UI
/// lives INSIDE `TerrainTools`, so there is deliberately NO `Sculpt` variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightSidebarSection {
    Inspector,
    Tool,
    PathTools,
    TerrainTools,
    ProposalReport,
    /// Application settings (was `ActiveDialog::Settings`), FR-1.
    Settings,
    /// Map manager (was `ActiveDialog::HexonManager`), FR-2.
    Maps,
}

/// Right-sidebar manager state: the explicitly-requested section (topbar toggle
/// or in-panel rail click). `None` means "no explicit toggle" → selection-default.
#[derive(Resource, Default, Debug, Clone)]
pub struct RightSidebarState {
    pub requested: Option<RightSidebarSection>,
}

impl RightSidebarState {
    /// Toggle a section: request it, or clear it if already active (never-double —
    /// requesting a new section replaces, it does not stack).
    pub fn toggle(&mut self, section: RightSidebarSection) {
        self.requested = if self.requested == Some(section) {
            None
        } else {
            Some(section)
        };
    }

    /// Whether `section` is the explicitly-requested one.
    pub fn is_active(&self, section: RightSidebarSection) -> bool {
        self.requested == Some(section)
    }
}

/// Human label for a section (rail tooltip / placeholder header). Pure.
pub fn section_label(section: RightSidebarSection) -> &'static str {
    match section {
        RightSidebarSection::Inspector => "Inspector",
        RightSidebarSection::Tool => "Tool",
        RightSidebarSection::PathTools => "Path Tools",
        RightSidebarSection::TerrainTools => "Terrain Tools",
        RightSidebarSection::ProposalReport => "Proposal Report",
        RightSidebarSection::Settings => "Settings",
        RightSidebarSection::Maps => "Maps",
    }
}

/// Decide the active section. Precedence: portal > explicit toggle >
/// selection-default. Returns `None` ONLY when the portal owns the region — a
/// true short-circuit, no section rail underneath. Outside the portal the
/// Inspector is the never-blank fallback: it self-collapses (`show_animated`)
/// when nothing is selected, reproducing today's always-call-`right_inspector`
/// behavior exactly.
pub fn active_section(
    state: &RightSidebarState,
    selection_present: bool,
    portal_open: bool,
) -> Option<RightSidebarSection> {
    if portal_open {
        return None; // portal toolbar owns the right region
    }
    if let Some(section) = state.requested {
        return Some(section); // explicit toggle beats the selection-default
    }
    // selection-default: Inspector either way (accepted here for the future
    // "welcome vs inspector" policy split; today both resolve to Inspector).
    let _ = selection_present;
    Some(RightSidebarSection::Inspector)
}

/// Renders the right region. Portal-open swaps the whole region to the portal
/// toolbar (preserved); otherwise dispatches to the active section's render fn.
pub fn render_right_sidebar(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    inspector_form: &mut InspectorFormState,
    node_mgr: &mut crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
    asset_status: &AssetDownloadStatus,
    // Phase 4 (FR-9): threaded so the dissolved-window sections below can
    // reach the state their former floating windows read/wrote.
    tool_panel_state: &mut ToolPanelState,
    path_state: &PathEditorState,
    proposal_state: &mut ProposalEditState,
    // Wave-1 scaffold: sculpt-tool state for the TerrainTools section (T3 fold).
    // Threaded exactly like `proposal_state`; consumed by `terrain_tools_panel`.
    sculpt_state: &mut SculptToolState,
    // FR-1/FR-2 (shell_ux_sidebar): full petal-map state (Maps section + the
    // ProposalReport's `world_scale`) and app settings (Settings section).
    petal_map: &mut PetalMapState,
    app_settings: &mut AppSettings,
    // Phase 5 (FR-8): the active tool, so the Tool section can render the
    // live selection/gimbal/anchor readouts moved from the retired left
    // `tool_inspector_panel`. Read-only here — see `render_tool_section`.
    tool: &ToolState,
) {
    let portal_open = ui_mgr.portal_is_open();
    if portal_open {
        // Portal open swaps the whole right region to the portal toolbar.
        portal_toolbar::right_portal_toolbar(ctx, ui_mgr);
        // Portal owns the region — drop a stale Maps carrier so it can't linger.
        clear_maps_carrier(ui_mgr);
        return;
    }
    let selection_present = node_mgr.selected_entity().is_some();
    let section = active_section(state, selection_present, portal_open);
    // FR-2 Maps lifecycle: the carrier (`ActiveDialog::HexonManager`) exists ONLY
    // while Maps is the active section — clear it whenever Maps is not active.
    if section != Some(RightSidebarSection::Maps) {
        clear_maps_carrier(ui_mgr);
    }
    match section {
        Some(RightSidebarSection::Inspector) => render_inspector_section(
            ctx,
            inspector_form,
            node_mgr,
            hierarchy,
            ui_mgr,
            local_role,
            db_tx,
            nav,
            asset_status,
        ),
        Some(RightSidebarSection::Tool) => {
            render_tool_section(ctx, state, tool, node_mgr, path_state, sculpt_state)
        }
        Some(RightSidebarSection::PathTools) => {
            render_path_tools_section(ctx, state, tool_panel_state, ui_mgr, path_state, hierarchy)
        }
        Some(RightSidebarSection::TerrainTools) => render_terrain_tools_section(
            ctx,
            state,
            tool_panel_state,
            ui_mgr,
            proposal_state,
            sculpt_state,
        ),
        Some(RightSidebarSection::ProposalReport) => {
            render_proposal_report_section(ctx, state, proposal_state, petal_map.world_scale)
        }
        Some(RightSidebarSection::Settings) => render_settings_section(ctx, state, app_settings),
        Some(RightSidebarSection::Maps) => render_maps_section(
            ctx,
            state,
            ui_mgr,
            petal_map,
            nav.active_petal_id.as_deref(),
        ),
        None => {} // unreachable: portal short-circuited above
    }
}

/// Drop the Maps section's data carrier (`ActiveDialog::HexonManager`) if it is
/// set. The carrier is populated by the non-owned sync/bridge writers; the
/// section manager is its sole lifecycle owner (FR-2). Idempotent.
fn clear_maps_carrier(ui_mgr: &mut UiManager) {
    if matches!(ui_mgr.active_dialog, ActiveDialog::HexonManager { .. }) {
        ui_mgr.close_dialog();
    }
}

// ---------------------------------------------------------------------------
// Per-section render fns — ONE per variant. This is the seam: downstream slices
// fill these bodies and MUST NOT collapse them into one another.
// ---------------------------------------------------------------------------

/// Inspector section — hosts the moved node-inspector call (`panels::inspector`).
/// Delegates to `right_inspector`, which owns its own SidePanel + self-collapse.
/// (P4 folds that SidePanel under this manager; this phase just moves the call.)
fn render_inspector_section(
    ctx: &egui::Context,
    inspector_form: &mut InspectorFormState,
    node_mgr: &mut crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
    asset_status: &AssetDownloadStatus,
) {
    inspector::right_inspector(
        ctx,
        inspector_form,
        node_mgr,
        hierarchy,
        ui_mgr,
        local_role,
        db_tx,
        nav,
        asset_status,
    );
}

/// Tool section — P5 (ui_shell Phase 5, FR-8): the active tool's live
/// readouts, moved from the retired left `tool_inspector_panel` (Q-1: no
/// always-open left panel; per-tool title/subtitle/Use guidance now live in
/// the topbar tooltip, `panels::toolbar::tool_tooltip_text`). Read-only for
/// DISPLAY — `node_mgr`/`path_state` are shared refs, never mutated here
/// (two-authority split, `panels/AGENTS.md` §tool-inspector).
fn render_tool_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    tool: &ToolState,
    node_mgr: &NodeManager,
    path_state: &PathEditorState,
    sculpt_state: &mut SculptToolState,
) {
    let kind = project_selection(
        node_mgr
            .selected
            .as_ref()
            .map(|s| (s.entity, s.node_id.as_str())),
        path_state.editing_track_id.as_deref(),
        path_state.selected_point,
        path_state.selected_segment,
    );
    // Guard a stale selected index outliving its points (see helper doc).
    let kind = fresh_path_selection(kind, path_state.points.len());

    section_chrome(ctx, state, RightSidebarSection::Tool, |ui| {
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
        // Gimbal-active affordance exactly when a gimbal is drawn (mirrors
        // `gimbal_interaction.rs`'s draw/interact rule).
        if let Some(affordance) = gimbal_affordance_label(tool.active_tool, &kind) {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(affordance)
                    .small()
                    .color(theme::TEXT_STRONG),
            );
        }
        if tool.active_tool == crate::panels::toolbar::Tool::Brush {
            ui.add_space(8.0);
            terrain_tools_panel::render_brush_controls(ui, sculpt_state);
            return;
        }
        // FR-6: the active tool's per-tool settings. Typed models
        // (SnapSettings/TransformConstraints) are P2 backlog; shown as calm
        // "(soon)" hints so the section is never blank (`ui_ux.md §7`).
        let settings = panel_descriptor(tool.active_tool).settings_zone;
        if !settings.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("SETTINGS")
                    .small()
                    .color(theme::TEXT_SECTION),
            );
            for line in settings {
                ui.label(egui::RichText::new(*line).small().color(theme::TEXT_MUTED));
            }
        }
    });
}

/// Path-tools section — path-asset stamp picker + pen controls, moved
/// verbatim (Phase 4/FR-9) from the retired `tool_panel::render_tool_panel`
/// floating window. `PathAssetApply` targets `PathEditorState.editing_track_id`
/// ONLY — never `NodeManager.selected` (two-authority split, see
/// `panels/AGENTS.md` §tool-panel).
fn render_path_tools_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    tool_panel_state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    path_state: &PathEditorState,
    verse_mgr: &VerseManager,
) {
    section_chrome(ctx, state, RightSidebarSection::PathTools, |ui| {
        tool_panel::render_path_asset_section(ui, tool_panel_state, ui_mgr, path_state, verse_mgr);
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        tool_panel::render_pen_section(ui, tool_panel_state);
    });
}

/// Terrain-tools section — the 8-mode palette + controls + proposal list,
/// moved verbatim (Phase 4/FR-9) from the retired
/// `terrain_tools_panel::terrain_tools_panel` floating window. Emits
/// `UiAction::TerrainProposalAdd`/`TerrainProposalDelete`; true terrain is
/// never written here (NFR-1).
fn render_terrain_tools_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    tool_panel_state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    proposal_state: &mut ProposalEditState,
    sculpt_state: &mut SculptToolState,
) {
    terrain_tools_panel::ensure_defaults(tool_panel_state);
    section_chrome(ctx, state, RightSidebarSection::TerrainTools, |ui| {
        terrain_tools_panel::render_palette(ui, tool_panel_state);
        ui.add_space(6.0);
        terrain_tools_panel::render_controls_and_emit(ui, tool_panel_state, ui_mgr);
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        terrain_tools_panel::render_proposal_list(ui, proposal_state, ui_mgr);
        // Wave-1 seam (T3 sculpt_earthwork_regions): its brush/shape sculpt UI
        // folds in here, reading/writing `sculpt_state` (no new section variant).
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        terrain_tools_panel::render_sculpt_placeholder(ui, sculpt_state);
    });
}

/// Proposal-report section — real-unit extent/area/volume/slope/bearing for
/// the selected terrain proposal, moved verbatim (Phase 4/FR-9) from the
/// retired `proposal_report_panel::proposal_report_panel` floating window.
/// `render_report_body` supplies its own calm empty-state hints in place of
/// the old window's "just don't render" early returns (never-blank).
fn render_proposal_report_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    proposal_state: &ProposalEditState,
    world_scale: f64,
) {
    section_chrome(ctx, state, RightSidebarSection::ProposalReport, |ui| {
        proposal_report_panel::render_report_body(ui, proposal_state, world_scale);
    });
}

/// Settings section (FR-1, D-A10) — the former `ActiveDialog::Settings` floating
/// window, now an ordinary one-at-a-time section. Reads/writes `AppSettings`
/// directly (no `UiAction` round-trip — same as the old window). Widgets kept
/// verbatim from the retired `dialogs/settings.rs`; calm hint for the not-yet-
/// added knobs (ui_ux §7). The old `settings_window`/`ActiveDialog::Settings`
/// are removed — this section is the sole Settings surface.
fn render_settings_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    app_settings: &mut AppSettings,
) {
    section_chrome(ctx, state, RightSidebarSection::Settings, |ui| {
        ui.label(
            egui::RichText::new("Rendering")
                .strong()
                .color(theme::TEXT_SECTION),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Render distance")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::DragValue::new(&mut app_settings.render_distance)
                    .speed(1.0)
                    .range(1.0..=f32::MAX)
                    .suffix(" wu"),
            )
            .on_hover_text(
                "Global default render distance; PetalManifest.render_distance overrides per-petal",
            );
        });

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Mesh budget ceiling")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            // usize has no native egui DragValue support; round-trip via u64.
            let mut ceiling = app_settings.mesh_budget_ceiling as u64;
            if ui
                .add(egui::DragValue::new(&mut ceiling).range(1..=u32::MAX as u64))
                .on_hover_text("MeshInstanceBudget.ceiling — the mesh-instance watchdog gate")
                .changed()
            {
                app_settings.mesh_budget_ceiling = ceiling as usize;
            }
        });

        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "Additional knobs (stamp caps, tile source mode, camera, P2P relay/peer config) land as AppSettings grows further fields.",
            )
            .small()
            .color(theme::TEXT_MUTED)
            .italics(),
        );
    });
}

/// Maps section (FR-2, D-A10) — the former `ActiveDialog::HexonManager` floating
/// Map Manager, now a one-at-a-time section. `ActiveDialog::HexonManager` is
/// retained ONLY as the state carrier (populated by non-owned sync/bridge
/// writers); this manager owns its lifecycle: self-seed + refresh on open, and
/// tear down (carrier + section request) on the manager's Close. It does not
/// fight another exclusive dialog for the slot — it shows a calm hint instead.
fn render_maps_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    ui_mgr: &mut UiManager,
    petal_map: &mut PetalMapState,
    active_petal: Option<&str>,
) {
    if !matches!(ui_mgr.active_dialog, ActiveDialog::HexonManager { .. }) {
        if matches!(ui_mgr.active_dialog, ActiveDialog::None) {
            // First frame Maps is active: seed the carrier + kick a refresh; we
            // fall through to render its (loading) body this same frame.
            ui_mgr.open_dialog(seed_hexon_manager());
            ui_mgr.push_action(UiAction::HexonRefreshList);
        } else {
            // Another exclusive dialog owns the slot — calm hint (ui_ux §7).
            section_chrome(ctx, state, RightSidebarSection::Maps, |ui| {
                ui.label(
                    egui::RichText::new("Close the open dialog to view maps.")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            });
            return;
        }
    }
    let close = crate::dialogs::render_hexon_manager(ctx, ui_mgr, petal_map, active_petal);
    if close {
        clear_maps_carrier(ui_mgr);
        // requested was Maps → toggling it clears back to the selection default.
        state.toggle(RightSidebarSection::Maps);
    }
}

/// A fresh, empty-loading `HexonManager` carrier for the Maps section to seed on
/// open; the non-owned refresh/advertisement writers populate its fields.
fn seed_hexon_manager() -> ActiveDialog {
    ActiveDialog::HexonManager {
        installed_tilesets: Vec::new(),
        available_tilesets: Vec::new(),
        download_progress: HashMap::new(),
        filter_text: String::new(),
        active_tab: HexonManagerTab::Installed,
        storage_info: StorageInfoDto {
            base_dir: String::new(),
            total_bytes: 0,
            count: 0,
        },
        loading: true,
        pending_remove: None,
    }
}

// ---------------------------------------------------------------------------
// Shared chrome: header + rail + separator + scrollable body, used by every
// section fn above. All five sections are filled as of P5 (FR-8).
// ---------------------------------------------------------------------------

/// Shared SidePanel chrome (header + rail + separator + scrollable, padded
/// body) — the seam every section fn (Inspector/Tool/PathTools/TerrainTools/
/// ProposalReport) renders through.
fn section_chrome(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    section: RightSidebarSection,
    body: impl FnOnce(&mut egui::Ui),
) {
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("right_section")
        .resizable(true)
        .default_width(320.0)
        .width_range(260.0..=max_w)
        .frame(
            egui::Frame::NONE
                .fill(theme::BG_PANEL)
                .inner_margin(egui::Margin::same(0))
                .stroke(egui::Stroke::new(2.0_f32, theme::BG_BUTTON)),
        )
        .show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(section_label(section))
                        .strong()
                        .color(theme::TEXT_HEADING),
                );
            });
            ui.add_space(2.0);
            section_rail(ui, state);
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt(format!("right_section_scroll_{section:?}"))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| body(ui));
                });
        });
}

/// Compact icon rail: one small button per section; click toggles it. Uses the
/// toolbar's single-source `mode_button_fill` to mark the active one.
fn section_rail(ui: &mut egui::Ui, state: &mut RightSidebarState) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        for (section, glyph) in [
            (RightSidebarSection::Inspector, "\u{24D8}"),
            (RightSidebarSection::Tool, "\u{1F527}"),
            (RightSidebarSection::PathTools, "\u{223F}"),
            (RightSidebarSection::TerrainTools, "\u{26F0}"),
            (RightSidebarSection::ProposalReport, "\u{1F4C4}"),
            (RightSidebarSection::Settings, "\u{2699}"),
            (RightSidebarSection::Maps, "\u{1F4E6}"),
        ] {
            let active = state.is_active(section);
            if ui
                .add(
                    egui::Button::new(glyph).fill(crate::panels::toolbar::mode_button_fill(active)),
                )
                .on_hover_text(section_label(section))
                .clicked()
            {
                state.toggle(section);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(req: Option<RightSidebarSection>) -> RightSidebarState {
        RightSidebarState { requested: req }
    }

    const ALL: [RightSidebarSection; 7] = [
        RightSidebarSection::Inspector,
        RightSidebarSection::Tool,
        RightSidebarSection::PathTools,
        RightSidebarSection::TerrainTools,
        RightSidebarSection::ProposalReport,
        RightSidebarSection::Settings,
        RightSidebarSection::Maps,
    ];

    #[test]
    fn portal_short_circuits_to_none_regardless_of_toggle_or_selection() {
        // portal beats explicit toggle AND selection-default.
        assert_eq!(
            active_section(&st(Some(RightSidebarSection::Tool)), true, true),
            None
        );
        assert_eq!(active_section(&st(None), true, true), None);
        assert_eq!(active_section(&st(None), false, true), None);
    }

    #[test]
    fn explicit_toggle_beats_selection_default() {
        for sec in ALL {
            assert_eq!(active_section(&st(Some(sec)), true, false), Some(sec));
            assert_eq!(active_section(&st(Some(sec)), false, false), Some(sec));
        }
    }

    #[test]
    fn selection_default_is_inspector_never_blank() {
        // no toggle, not portal -> always Inspector (self-collapses when unselected).
        assert_eq!(
            active_section(&st(None), true, false),
            Some(RightSidebarSection::Inspector)
        );
        assert_eq!(
            active_section(&st(None), false, false),
            Some(RightSidebarSection::Inspector)
        );
    }

    #[test]
    fn never_double_every_non_portal_yields_exactly_one_section() {
        // Option<_> is single by construction; assert every non-portal case is Some.
        for req in [None, Some(RightSidebarSection::Tool)] {
            for &sel in &[true, false] {
                assert!(
                    active_section(&st(req), sel, false).is_some(),
                    "non-portal must yield exactly one section"
                );
            }
        }
    }

    #[test]
    fn toggle_sets_clears_and_replaces() {
        let mut s = RightSidebarState::default();
        assert_eq!(s.requested, None);
        s.toggle(RightSidebarSection::Tool);
        assert!(s.is_active(RightSidebarSection::Tool));
        // toggling the active one clears back to selection-default.
        s.toggle(RightSidebarSection::Tool);
        assert_eq!(s.requested, None);
        // requesting a different section replaces (never stacks two).
        s.toggle(RightSidebarSection::Tool);
        s.toggle(RightSidebarSection::PathTools);
        assert!(s.is_active(RightSidebarSection::PathTools));
        assert!(!s.is_active(RightSidebarSection::Tool));
    }

    #[test]
    fn section_label_nonempty_for_all_variants() {
        for sec in ALL {
            assert!(!section_label(sec).is_empty(), "empty label for {sec:?}");
        }
    }
}
