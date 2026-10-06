//! Right-sidebar area manager (FR-6): the single right region, one section at a
//! time (RATIFIED Q-2). `active_section` precedence is portal, then explicit
//! toggle, then selection-default; the Inspector is the never-blank fallback.
//! There is ONE render fn per section variant, and every one of them —
//! Inspector included as of D10 — renders through `section_chrome`, so the
//! section rail is reachable from the resting app (finding #3).
//!
//! ui_semantics_unification_20260808 Phases 3+4 reshaped the variant set:
//! `Tool` became `Options` (D7), the canonical home for the ACTIVE TOOL's
//! mutable settings, dispatched per-tool by `panels::tool_options`; `PathTools`
//! was retired into that dispatcher's Pen arm. Sections also carry a stable
//! machine handle (`slug`/`from_slug`, D11) driven by `UiAction::RevealSection`.
//! See `fe-ui/src/ui_shell/AGENTS.md` §right.

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
    anchor_readout, fresh_path_selection, gimbal_affordance_label, selection_summary,
};
use crate::panels::tool_panel::ToolPanelState;
use crate::panels::{
    inspector, portal_toolbar, proposal_report_panel, terrain_tools_panel, tool_options,
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
/// lives INSIDE `Options`/`TerrainTools`, so there is deliberately NO `Sculpt`
/// variant. `PathTools` was RETIRED (D7): its pen/stamp/shape controls are the
/// Pen arm of `Options`. See `fe-ui/src/ui_shell/AGENTS.md` §right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightSidebarSection {
    Inspector,
    /// The canonical home for the ACTIVE TOOL's mutable settings (D7, was
    /// `Tool`). One dispatcher, one arm per `Tool` — see `panels::tool_options`.
    Options,
    TerrainTools,
    ProposalReport,
    /// Application settings (was `ActiveDialog::Settings`), FR-1.
    Settings,
    /// Map manager (was `ActiveDialog::HexonManager`), FR-2.
    Maps,
}

/// Every section, in rail order. Single source for the rail, the slug table,
/// and the round-trip tests — a new variant that forgets a slug fails to build.
pub const ALL_SECTIONS: [RightSidebarSection; 6] = [
    RightSidebarSection::Inspector,
    RightSidebarSection::Options,
    RightSidebarSection::TerrainTools,
    RightSidebarSection::ProposalReport,
    RightSidebarSection::Settings,
    RightSidebarSection::Maps,
];

impl RightSidebarSection {
    /// D11: the stable machine handle for this surface — the addressable half of
    /// `section_label`'s human half. Stable across renames; consumed by
    /// `UiAction::RevealSection` and (later) fe-api/MCP.
    pub fn slug(&self) -> &'static str {
        match self {
            RightSidebarSection::Inspector => "inspector",
            RightSidebarSection::Options => "options",
            RightSidebarSection::TerrainTools => "terrain",
            RightSidebarSection::ProposalReport => "report",
            RightSidebarSection::Settings => "settings",
            RightSidebarSection::Maps => "maps",
        }
    }

    /// Inverse of [`RightSidebarSection::slug`]; `None` for an unknown handle.
    pub fn from_slug(s: &str) -> Option<Self> {
        ALL_SECTIONS.into_iter().find(|sec| sec.slug() == s)
    }
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

    /// D11 reveal: request `section` unconditionally (idempotent — unlike
    /// [`RightSidebarState::toggle`], a second reveal never closes it). This is
    /// the write `UiAction::RevealSection` performs, so an addressed surface
    /// behaves the same whether it is asked for once or ten times.
    pub fn reveal(&mut self, section: RightSidebarSection) {
        self.requested = Some(section);
    }

    /// Reveal by stable slug (D11). Returns `false` for an unknown handle,
    /// leaving the current request untouched. Pure over `from_slug`.
    pub fn reveal_slug(&mut self, slug: &str) -> bool {
        match RightSidebarSection::from_slug(slug) {
            Some(section) => {
                self.reveal(section);
                true
            }
            None => false,
        }
    }
}

/// Human label for a section (rail tooltip / placeholder header). Pure.
pub fn section_label(section: RightSidebarSection) -> &'static str {
    match section {
        RightSidebarSection::Inspector => "Inspector",
        RightSidebarSection::Options => "Options",
        RightSidebarSection::TerrainTools => "Terrain Tools",
        RightSidebarSection::ProposalReport => "Proposal Report",
        RightSidebarSection::Settings => "Settings",
        RightSidebarSection::Maps => "Maps",
    }
}

/// Decide the active section. Precedence: portal > explicit toggle >
/// selection-default. Returns `None` ONLY when the portal owns the region — a
/// true short-circuit, no section rail underneath. Outside the portal the
/// Inspector is the never-blank fallback. D10: it no longer self-collapses when
/// nothing is selected — the resting app must still show the section rail, and
/// the Inspector body carries its own empty state instead.
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
    // MUTABLE since D8: the Options surface's Pen arm hosts the per-anchor
    // corner editor moved out of the Data window's Paths tab, which live-edits
    // `points` and defers the persist signal (same idiom as the Paths tab).
    path_state: &mut PathEditorState,
    proposal_state: &mut ProposalEditState,
    // Sculpt-tool state. D6-B: its ONE host is the Options section's Brush arm
    // (it used to be edited from TerrainTools too, finding #7).
    sculpt_state: &mut SculptToolState,
    // FR-1/FR-2 (shell_ux_sidebar): full petal-map state (Maps section + the
    // ProposalReport's `world_scale`) and app settings (Settings section).
    petal_map: &mut PetalMapState,
    app_settings: &mut AppSettings,
    // The active tool — what the Options section dispatches on, and the source
    // of the live selection/gimbal readouts. Read-only here: `activate` is the
    // single writer (`plugin.rs`).
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
            state,
            inspector_form,
            node_mgr,
            hierarchy,
            ui_mgr,
            local_role,
            db_tx,
            nav,
            asset_status,
        ),
        Some(RightSidebarSection::Options) => render_options_section(
            ctx,
            state,
            tool,
            node_mgr,
            path_state,
            sculpt_state,
            tool_panel_state,
            ui_mgr,
            hierarchy,
            nav.active_petal_id.as_deref(),
        ),
        Some(RightSidebarSection::TerrainTools) => render_terrain_tools_section(
            ctx,
            state,
            tool_panel_state,
            ui_mgr,
            proposal_state,
            petal_map.world_scale,
        ),
        Some(RightSidebarSection::ProposalReport) => render_proposal_report_section(
            ctx,
            state,
            proposal_state,
            petal_map.world_scale,
            petal_map.terrain_json.as_ref(),
        ),
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
/// D10: renders through `section_chrome` like every other section, so the
/// section rail is present in the RESTING state (the bespoke
/// `SidePanel::right("inspector")` + its `show_animated` self-collapse are
/// gone — they were the reason TerrainTools/ProposalReport had no entry point
/// from a fresh app, finding #3). `right_inspector` now takes a `&mut Ui`.
fn render_inspector_section(
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
) {
    section_chrome(ctx, state, RightSidebarSection::Inspector, |ui| {
        inspector::right_inspector(
            ui,
            inspector_form,
            node_mgr,
            hierarchy,
            ui_mgr,
            local_role,
            db_tx,
            nav,
            asset_status,
        );
    });
}

/// Options section (D7, was `Tool`) — the canonical, always-correct home for
/// the ACTIVE TOOL's mutable settings. Body shape is fixed: selection readout
/// → separator → `panels::tool_options::render`, the one dispatcher with an arm
/// per `Tool`. The old generic "SETTINGS" block is GONE (finding #15: it was
/// unreachable for Brush and duplicated what the arms now own); per-tool calm
/// placeholders live in `tool_inspector::panel_descriptor().options_hints` and
/// are rendered by the dispatcher's placeholder arms.
///
/// `node_mgr` stays a shared ref (display-only). `path_state` is `&mut` because
/// the Pen arm hosts the per-anchor corner editor (D8).
fn render_options_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    tool: &ToolState,
    node_mgr: &NodeManager,
    path_state: &mut PathEditorState,
    sculpt_state: &mut SculptToolState,
    tool_panel_state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    verse_mgr: &VerseManager,
    active_petal_id: Option<&str>,
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

    section_chrome(ctx, state, RightSidebarSection::Options, |ui| {
        render_selection_readout(ui, tool.active_tool, &kind, &path_state.points);
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        tool_options::render(
            ui,
            tool.active_tool,
            &kind,
            tool_panel_state,
            sculpt_state,
            path_state,
            ui_mgr,
            verse_mgr,
            active_petal_id,
        );
    });
}

/// The Options section's fixed header block: what is selected, its per-anchor
/// readout, and the gimbal affordance. Display-only (P5/FR-8 helpers, moved
/// verbatim from the retired `Tool` section body).
fn render_selection_readout(
    ui: &mut egui::Ui,
    active_tool: crate::panels::toolbar::Tool,
    kind: &crate::node_manager::SelectionKind,
    points: &[crate::gis::PathPointRow],
) {
    ui.label(
        egui::RichText::new("SELECTION")
            .small()
            .color(theme::TEXT_SECTION),
    );
    ui.label(
        egui::RichText::new(selection_summary(kind))
            .small()
            .color(theme::TEXT_MUTED),
    );
    // FR-6 per-anchor affordance; the EDITABLE corner card is the Pen arm (D8).
    if let Some(readout) = anchor_readout(kind, points) {
        ui.label(
            egui::RichText::new(readout)
                .small()
                .color(theme::TEXT_MUTED),
        );
    }
    // Gimbal-active affordance exactly when a gimbal is drawn (mirrors
    // `gimbal_interaction.rs`'s draw/interact rule).
    if let Some(affordance) = gimbal_affordance_label(active_tool, kind) {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(affordance)
                .small()
                .color(theme::TEXT_STRONG),
        );
    }
}

/// Terrain-tools section — the 8-mode proposal palette + controls + proposal
/// list. D6-B stripped the sculpt duplicate out of here: `SculptToolState`'s
/// widgets now have exactly ONE host, the Brush arm of `Options` (finding #7).
/// Emits `UiAction::TerrainProposalAdd`/`TerrainProposalDelete`; true terrain is
/// never written here (NFR-1).
fn render_terrain_tools_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    tool_panel_state: &mut ToolPanelState,
    ui_mgr: &mut UiManager,
    proposal_state: &mut ProposalEditState,
    world_scale: f64,
) {
    terrain_tools_panel::ensure_defaults(tool_panel_state);
    section_chrome(ctx, state, RightSidebarSection::TerrainTools, |ui| {
        terrain_tools_panel::render_palette(ui, tool_panel_state);
        ui.add_space(6.0);
        terrain_tools_panel::render_controls_and_emit(ui, tool_panel_state, ui_mgr, world_scale);
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
/// `terrain_json` additionally lets it see Brush-created earthwork regions
/// (finding #5, `ui_semantics_unification_20260808`) — Brush never touches
/// `ProposalEditState`, only the terrain doc.
fn render_proposal_report_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    proposal_state: &ProposalEditState,
    world_scale: f64,
    terrain_json: Option<&serde_json::Value>,
) {
    section_chrome(ctx, state, RightSidebarSection::ProposalReport, |ui| {
        proposal_report_panel::render_report_body(ui, proposal_state, world_scale, terrain_json);
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
    app_settings.render_distance = if app_settings.render_distance.is_finite() {
        app_settings.render_distance.clamp(1.0, 1_000_000.0)
    } else {
        AppSettings::default().render_distance
    };
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
                    .range(1.0..=1_000_000.0)
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
/// body) — the seam EVERY section fn renders through, Inspector included
/// (D10). That "every" is load-bearing: the rail lives in here, so a section
/// that opts out (as the Inspector used to) makes the whole rail invisible in
/// the resting state.
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

/// Rail glyph for a section. Pure + total, so a new variant cannot silently
/// vanish from the rail.
fn section_glyph(section: RightSidebarSection) -> &'static str {
    match section {
        RightSidebarSection::Inspector => "\u{24D8}",
        RightSidebarSection::Options => "\u{1F527}",
        RightSidebarSection::TerrainTools => "\u{26F0}",
        RightSidebarSection::ProposalReport => "\u{1F4C4}",
        RightSidebarSection::Settings => "\u{2699}",
        RightSidebarSection::Maps => "\u{1F4E6}",
    }
}

/// Compact icon rail: one small button per section; click toggles it. Uses the
/// toolbar's single-source `mode_button_fill` to mark the active one. The rail
/// is a TOGGLE affordance (click the active one to fall back to the
/// selection-default), which is why it writes `toggle` rather than the
/// idempotent `reveal` that `UiAction::RevealSection` uses.
fn section_rail(ui: &mut egui::Ui, state: &mut RightSidebarState) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        for section in ALL_SECTIONS {
            let glyph = section_glyph(section);
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

    const ALL: [RightSidebarSection; 6] = ALL_SECTIONS;

    #[test]
    fn portal_short_circuits_to_none_regardless_of_toggle_or_selection() {
        // portal beats explicit toggle AND selection-default.
        assert_eq!(
            active_section(&st(Some(RightSidebarSection::Options)), true, true),
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
        for req in [None, Some(RightSidebarSection::Options)] {
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
        s.toggle(RightSidebarSection::Options);
        assert!(s.is_active(RightSidebarSection::Options));
        // toggling the active one clears back to selection-default.
        s.toggle(RightSidebarSection::Options);
        assert_eq!(s.requested, None);
        // requesting a different section replaces (never stacks two).
        s.toggle(RightSidebarSection::Options);
        s.toggle(RightSidebarSection::TerrainTools);
        assert!(s.is_active(RightSidebarSection::TerrainTools));
        assert!(!s.is_active(RightSidebarSection::Options));
    }

    #[test]
    fn section_label_nonempty_for_all_variants() {
        for sec in ALL {
            assert!(!section_label(sec).is_empty(), "empty label for {sec:?}");
        }
    }

    // ---- D11: the addressable surface handle -------------------------------

    #[test]
    fn slug_round_trips_for_every_section() {
        for sec in ALL {
            assert_eq!(
                RightSidebarSection::from_slug(sec.slug()),
                Some(sec),
                "slug {:?} must round-trip",
                sec.slug()
            );
        }
    }

    #[test]
    fn slugs_are_unique_stable_machine_handles() {
        let mut slugs: Vec<&str> = ALL.iter().map(|s| s.slug()).collect();
        assert!(slugs.iter().all(|s| !s.is_empty()));
        // Machine handles: lowercase ASCII, no spaces — safe in a URL/API path.
        assert!(slugs
            .iter()
            .all(|s| s.chars().all(|c| c.is_ascii_lowercase() || c == '_')));
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), ALL.len(), "duplicate section slug");
    }

    #[test]
    fn from_slug_rejects_unknown_and_retired_handles() {
        assert_eq!(RightSidebarSection::from_slug("nope"), None);
        assert_eq!(RightSidebarSection::from_slug(""), None);
        // `PathTools` is retired (D7) — its old handle must not resolve.
        assert_eq!(RightSidebarSection::from_slug("path_tools"), None);
        // Slugs are exact, not case-insensitive.
        assert_eq!(RightSidebarSection::from_slug("Options"), None);
    }

    #[test]
    fn reveal_is_idempotent_unlike_toggle() {
        let mut s = RightSidebarState::default();
        s.reveal(RightSidebarSection::TerrainTools);
        assert!(s.is_active(RightSidebarSection::TerrainTools));
        // A second reveal of the SAME surface keeps it open (an addressed
        // surface must not close because it was addressed twice).
        s.reveal(RightSidebarSection::TerrainTools);
        assert!(s.is_active(RightSidebarSection::TerrainTools));
    }

    #[test]
    fn reveal_slug_routes_every_known_handle_and_rejects_the_rest() {
        // This is exactly what `UiAction::RevealSection`'s handler does.
        for sec in ALL {
            let mut s = RightSidebarState::default();
            assert!(s.reveal_slug(sec.slug()), "{sec:?} must be addressable");
            assert_eq!(s.requested, Some(sec));
        }
        let mut s = st(Some(RightSidebarSection::Options));
        assert!(!s.reveal_slug("not-a-surface"));
        assert_eq!(
            s.requested,
            Some(RightSidebarSection::Options),
            "an unknown slug must leave the current request untouched"
        );
    }

    #[test]
    fn every_section_has_a_distinct_rail_glyph() {
        // D10/finding #3: the rail is the only resting-state entry point to
        // TerrainTools/ProposalReport, so no section may share or drop a glyph.
        let mut glyphs: Vec<&str> = ALL.iter().map(|s| section_glyph(*s)).collect();
        assert!(glyphs.iter().all(|g| !g.is_empty()));
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), ALL.len(), "duplicate rail glyph");
    }
}
