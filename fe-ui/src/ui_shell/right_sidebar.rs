//! Right-sidebar area manager (FR-6): the single right region, one section at a
//! time (RATIFIED Q-2). `active_section` precedence is portal > explicit toggle
//! > selection-default; the Inspector is the never-blank fallback. There is ONE
//! render fn per section variant. Phase 4 (FR-9) filled PathTools/TerrainTools/
//! ProposalReport by dissolving the three floating windows into this region
//! (bodies delegate to pure helpers still homed in `panels::{tool_panel,
//! terrain_tools_panel, proposal_report_panel}`); Phase 5 (FR-8) fills the
//! `Tool` section with the retired left tool-inspector panel's live readouts
//! (delegates to pure helpers homed in `panels::tool_inspector`). All five
//! sections are filled. See `fe-ui/src/ui_shell/AGENTS.md` §right.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::UiManager;
use crate::asset_ops::AssetDownloadStatus;
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::{project_selection, NodeManager};
use crate::panels::tool_inspector::{
    anchor_readout, fresh_path_selection, gimbal_affordance_label, selection_summary,
};
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::{
    inspector, portal_toolbar, proposal_report_panel, terrain_tools_panel, tool_panel,
};
use crate::plugin::{InspectorFormState, LocalUserRole, ToolState};
use crate::terrain_proposal_state::ProposalEditState;
use crate::theme;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// The mutually-exclusive right-sidebar sections (one at a time — never-double).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightSidebarSection {
    Inspector,
    Tool,
    PathTools,
    TerrainTools,
    ProposalReport,
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
    world_scale: f64,
    // Phase 5 (FR-8): the active tool, so the Tool section can render the
    // live selection/gimbal/anchor readouts moved from the retired left
    // `tool_inspector_panel`. Read-only here — see `render_tool_section`.
    tool: &ToolState,
) {
    let portal_open = ui_mgr.portal_is_open();
    if portal_open {
        // Portal open swaps the whole right region to the portal toolbar.
        portal_toolbar::right_portal_toolbar(ctx, ui_mgr);
        return;
    }
    let selection_present = node_mgr.selected_entity().is_some();
    match active_section(state, selection_present, portal_open) {
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
            render_tool_section(ctx, state, tool, node_mgr, path_state)
        }
        Some(RightSidebarSection::PathTools) => render_path_tools_section(
            ctx,
            state,
            tool_panel_state,
            ui_mgr,
            path_state,
            hierarchy,
        ),
        Some(RightSidebarSection::TerrainTools) => render_terrain_tools_section(
            ctx,
            state,
            tool_panel_state,
            ui_mgr,
            proposal_state,
        ),
        Some(RightSidebarSection::ProposalReport) => {
            render_proposal_report_section(ctx, state, proposal_state, world_scale)
        }
        None => {} // unreachable: portal short-circuited above
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
        ] {
            let active = state.is_active(section);
            if ui
                .add(
                    egui::Button::new(glyph)
                        .fill(crate::panels::toolbar::mode_button_fill(active)),
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

    const ALL: [RightSidebarSection; 5] = [
        RightSidebarSection::Inspector,
        RightSidebarSection::Tool,
        RightSidebarSection::PathTools,
        RightSidebarSection::TerrainTools,
        RightSidebarSection::ProposalReport,
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
