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

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;
use fe_database::handlers::node_view::MAX_VIEW_GROUPS;
use fe_database::handlers::visibility_groups::{MAX_GROUP_NAME_CHARS, MAX_VISIBILITY_GROUPS};

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
use crate::panels::volume_report_panel::VolumeReportState;
use crate::panels::{
    inspector, portal_toolbar, proposal_report_panel, terrain_tools_panel, tool_options,
    volume_report_panel,
};
use crate::plugin::{InspectorFormState, LocalUserRole, ToolState};
use crate::settings::AppSettings;
use crate::terrain_map::dto::{HexonManagerTab, StorageInfoDto};
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalEditState;
use crate::theme;
use crate::verse_manager::VerseManager;
use crate::visibility::groups as group_ops;
use crate::visibility::VisibilityState;
use fe_runtime::messages::{CallerAuth, DbCommand};

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
    /// Visibility-Group management (D-10): create/rename/delete/toggle/reorder
    /// the active petal's Groups + selection membership. The left sidebar is
    /// read + toggle only; its "Add selection to group…" reveals this by slug.
    Groups,
}

/// Every section, in rail order. Single source for the rail, the slug table,
/// and the round-trip tests — a new variant that forgets a slug fails to build.
pub const ALL_SECTIONS: [RightSidebarSection; 7] = [
    RightSidebarSection::Inspector,
    RightSidebarSection::Options,
    RightSidebarSection::TerrainTools,
    RightSidebarSection::ProposalReport,
    RightSidebarSection::Settings,
    RightSidebarSection::Maps,
    RightSidebarSection::Groups,
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
            RightSidebarSection::Groups => "groups",
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
    pub collapsed: bool,
    pub requested: Option<RightSidebarSection>,
    /// Settings-section residency readout, refreshed by `update_residency_readout`.
    pub residency: ResidencyReadout,
}

/// What is resident right now (D-74 pointer-set): ledger nodes + stamp groups,
/// and the host-mirrored tile counters. See `fe-ui/src/AGENTS.md` §residency-ledger.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidencyReadout {
    pub nodes: usize,
    pub stamp_groups: usize,
    pub tile_bundles: usize,
    pub tile_bytes: u64,
}

impl ResidencyReadout {
    /// "Resident: N nodes · M stamp groups · K tile bundles (X MiB)".
    pub fn line(&self) -> String {
        format!(
            "Resident: {} nodes · {} stamp groups · {} tile bundles ({:.1} MiB)",
            self.nodes,
            self.stamp_groups,
            self.tile_bundles,
            self.tile_bytes as f64 / (1024.0 * 1024.0)
        )
    }
}

/// Mirror the residency ledger + `ResidencyStats` into the sidebar readout,
/// writing only on change (no per-frame change-detection churn).
pub fn update_residency_readout(
    ledger: bevy::prelude::Res<crate::verse_manager::ResidencyLedger>,
    stats: Option<bevy::prelude::Res<fe_runtime::diag15m::ResidencyStats>>,
    mut state: bevy::prelude::ResMut<RightSidebarState>,
) {
    let stats = stats.as_deref().copied().unwrap_or_default();
    let next = ResidencyReadout {
        nodes: ledger.node_count(),
        stamp_groups: ledger.stamp_group_count(),
        tile_bundles: stats.tile_bundles,
        tile_bytes: stats.tile_bytes,
    };
    if state.residency != next {
        state.residency = next;
    }
}

impl RightSidebarState {
    /// Hide/show the section without changing the request or node selection.
    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    /// Toggle a section: request it, or clear it if already active (never-double —
    /// requesting a new section replaces, it does not stack).
    pub fn toggle(&mut self, section: RightSidebarSection) {
        self.requested = if !self.collapsed && self.requested == Some(section) {
            None
        } else {
            Some(section)
        };
        self.collapsed = false;
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
        self.collapsed = false;
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
/// D-DV-4/OD-4 (terrain_proposal_delta_viz_20261025): `ProposalReport`'s
/// label is renamed to "Volume report" — the section now hosts the true
/// cut/fill figures too (R-4), not just the geometry report. The `slug`
/// stays `"report"` (D11: stable across renames).
pub fn section_label(section: RightSidebarSection) -> &'static str {
    match section {
        RightSidebarSection::Inspector => "Inspector",
        RightSidebarSection::Options => "Options",
        RightSidebarSection::TerrainTools => "Terrain Tools",
        RightSidebarSection::ProposalReport => "Volume report",
        RightSidebarSection::Settings => "Settings",
        RightSidebarSection::Maps => "Maps",
        RightSidebarSection::Groups => "Groups",
    }
}

/// Decide the active section. Precedence: portal/collapse > explicit toggle >
/// selection-default. Returns `None` when collapsed or the portal owns the region — a
/// true short-circuit, no section rail underneath. Outside the portal the
/// Inspector is the never-blank fallback. D10: it no longer self-collapses when
/// nothing is selected — the resting app must still show the section rail, and
/// the Inspector body carries its own empty state instead.
pub fn active_section(
    state: &RightSidebarState,
    selection_present: bool,
    portal_open: bool,
) -> Option<RightSidebarSection> {
    if portal_open || state.collapsed {
        return None; // portal toolbar or explicit collapse owns the region
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
    // M2/F6: per-verse timeseries fabric settings (mode + R slider + bucket
    // width) mirrored from the verse row; the section edits it.
    ts_settings: &mut crate::timeseries_settings::TimeseriesSettingsState,
    // The active tool — what the Options section dispatches on, and the source
    // of the live selection/gimbal readouts. Read-only here: `activate` is the
    // single writer (`plugin.rs`).
    tool: &ToolState,
    // Measure tool anchors (hexon_scale FR-9): the Options Measure arm's
    // Undo/Clear/Close/mode buttons run the same reducer as viewport clicks.
    measure_state: &mut crate::node_manager::measure::MeasureState,
    // VIS-PH5: the Groups section reads the registry + membership; edits go
    // out as `UiAction`s (optimistic + authed in the reducer).
    visibility: &VisibilityState,
    // S4 (p2p_asset_streaming_20260718): active tileset downloads, read-only
    // here — `drain_tileset_events` is the sole writer. Survives the Maps
    // section closing (moved out of `ActiveDialog::HexonManager`).
    downloads: &crate::terrain_map::TilesetDownloads,
    // S6: session-only residency override, read/written by the Settings
    // section's "Session override" field (bundled by the caller, same
    // param-ceiling reason as `downloads`).
    render_override: &mut crate::settings::PetalRenderOverride,
    // terrain_proposal_delta_viz_20261025 S6: the Volume report's live query
    // + latest true-figures answer. Read-only here — `plugin.rs`'s
    // `sync_volume_report_request`/`drain_volume_report_result` (PostUpdate)
    // are its sole writers.
    volume_report: &VolumeReportState,
    modal: &mut super::modal::ModalManagerState,
    settings_session: &mut crate::settings::SettingsSession,
    topbar: &mut super::topbar::TopbarState,
) {
    if !super::layout_policy::can_show_right(ctx) {
        return;
    }
    let portal_open = ui_mgr.portal_is_open();
    if portal_open {
        // Portal open swaps the whole right region to the portal toolbar.
        if modal.disabled_panels.contains("portal_toolbar") {
            egui::SidePanel::right("portal_toolbar_quarantine").show(ctx, |ui| {
                ui.label("Web page controls are unavailable after an error.");
                if ui.button("Retry web page controls").observed(ui).clicked() {
                    modal.clear_quarantine("portal_toolbar");
                }
            });
        } else {
            let _ = super::modal::guarded(modal, "portal_toolbar", || {
                portal_toolbar::right_portal_toolbar(ctx, ui_mgr);
            });
        }
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
    let Some(current_section) = section else { return; };
    let guard_key = section_guard_key(current_section);
    if modal.disabled_panels.contains(guard_key) {
        render_quarantined_section(ctx, state, current_section, modal);
        return;
    }
    let _ = super::modal::guarded(modal, guard_key, || match section {
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
            measure_state,
            crate::node_manager::measure::measure_world_scale(
                petal_map,
                nav.active_petal_id.as_deref(),
            ),
        ),
        Some(RightSidebarSection::TerrainTools) => render_terrain_tools_section(
            ctx,
            state,
            tool_panel_state,
            ui_mgr,
            proposal_state,
            petal_map.world_scale,
        ),
        Some(RightSidebarSection::ProposalReport) => {
            let open_node = render_proposal_report_section(
                ctx,
                state,
                proposal_state,
                petal_map,
                ui_mgr,
                hierarchy,
                nav.active_petal_id.as_deref(),
                volume_report,
                local_role.role.is_some_and(|role| role.can_view()),
            );
            if let Some(node_id) = open_node.filter(|node_id| {
                crate::visibility::pickable(node_id, nav, visibility)
                    && nav
                        .active_petal_id
                        .as_deref()
                        .and_then(|id| hierarchy.find_petal(id))
                        .is_some_and(|petal| petal.nodes.iter().any(|node| &node.id == node_id))
            }) {
                node_mgr.pending_sidebar_select = Some(node_id);
                inspector_form.active_tab = crate::plugin::InspectorTab::Properties;
                state.reveal(RightSidebarSection::Inspector);
            }
        }
        Some(RightSidebarSection::Settings) => render_settings_section(
            ctx,
            state,
            app_settings,
            ts_settings,
            nav,
            db_tx,
            hierarchy,
            local_role,
            render_override,
            settings_session,
            topbar,
        ),
        Some(RightSidebarSection::Maps) => render_maps_section(
            ctx,
            state,
            ui_mgr,
            petal_map,
            nav.active_petal_id.as_deref(),
            nav.active_verse_id.as_deref(),
            downloads,
            app_settings,
            local_role,
        ),
        Some(RightSidebarSection::Groups) => render_groups_section(
            ctx,
            state,
            ui_mgr,
            visibility,
            nav.active_petal_id.as_deref(),
            node_mgr
                .selected
                .as_ref()
                .map(|s| s.node_id.as_str())
                .filter(|id| {
                    nav.active_petal_id
                        .as_deref()
                        .and_then(|petal| hierarchy.find_petal(petal))
                        .is_some_and(|petal| petal.nodes.iter().any(|node| node.id == *id))
                }),
            local_role,
        ),
        None => {}
    });
}

fn render_quarantined_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    section: RightSidebarSection,
    modal: &mut super::modal::ModalManagerState,
) {
    section_chrome(ctx, state, section, |ui| {
        ui.colored_label(theme::NOTICE_TEXT, format!("{} is unavailable after an error.", section_label(section)));
        if ui.button("Retry section").named(ui, format_args!("Retry {} section", section_label(section))).clicked() {
            modal.clear_quarantine(section_guard_key(section));
        }
    });
}

fn section_guard_key(section: RightSidebarSection) -> &'static str {
    match section {
        RightSidebarSection::Inspector => "right.inspector",
        RightSidebarSection::Options => "right.options",
        RightSidebarSection::TerrainTools => "right.terrain_tools",
        RightSidebarSection::ProposalReport => "right.proposal_report",
        RightSidebarSection::Settings => "right.settings",
        RightSidebarSection::Maps => "right.maps",
        RightSidebarSection::Groups => "right.groups",
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
    measure_state: &mut crate::node_manager::measure::MeasureState,
    measure_world_scale: f64,
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
            measure_state,
            measure_world_scale,
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

/// Measured terrain/material/object report with optional footprint geometry.
fn render_proposal_report_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    proposal_state: &ProposalEditState,
    petal_map: &PetalMapState,
    ui_mgr: &mut UiManager,
    hierarchy: &VerseManager,
    active_petal: Option<&str>,
    volume_report: &VolumeReportState,
    can_view: bool,
) -> Option<String> {
    let mut open_node = None;
    section_chrome(ctx, state, RightSidebarSection::ProposalReport, |ui| {
        if !can_view {
            ui.label("Viewer access to this Petal is required to read its report.");
            return;
        }
        volume_report_panel::render_report_target_picker(
            ui,
            ui_mgr,
            proposal_state,
            petal_map,
            active_petal,
        );
        volume_report_panel::render_volume_report_body(
            ui,
            ui_mgr,
            volume_report,
            hierarchy,
            active_petal,
            &mut open_node,
        );
        if proposal_state.selected.as_ref().is_some_and(|selected| {
            proposal_state
                .proposals
                .iter()
                .any(|record| &record.id == selected)
        }) {
            ui.collapsing("Footprint geometry", |ui| {
                proposal_report_panel::render_report_body(
                    ui,
                    proposal_state,
                    petal_map.world_scale,
                );
            });
        }
    });
    open_node
}

/// Settings section (FR-1, D-A10) — the former `ActiveDialog::Settings` floating
/// window, now an ordinary one-at-a-time section. Reads/writes `AppSettings`
/// directly (no `UiAction` round-trip — same as the old window). Widgets kept
/// verbatim from the retired `dialogs/settings.rs`; calm hint for the not-yet-
/// added knobs (ui_ux §7). The old `settings_window`/`ActiveDialog::Settings`
/// are removed — this section is the sole Settings surface.
#[allow(clippy::too_many_arguments)]
fn render_settings_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    app_settings: &mut AppSettings,
    ts_settings: &mut crate::timeseries_settings::TimeseriesSettingsState,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    // S6 (p2p_asset_streaming_20260718): the active petal's durable
    // `render_distance` column + the session-only override, surfaced right
    // under the global knob so the three-way precedence is visible in one place.
    hierarchy: &VerseManager,
    local_role: &LocalUserRole,
    render_override: &mut crate::settings::PetalRenderOverride,
    session: &mut crate::settings::SettingsSession,
    topbar: &mut super::topbar::TopbarState,
) {
    crate::settings::sanitize(app_settings);
    let residency_line = state.residency.line();
    section_chrome(ctx, state, RightSidebarSection::Settings, |ui| {
        if !session.persistence_available {
            ui.label("Settings are available for this session only.");
        } else if let Some(error) = session.error.as_deref() {
            ui.colored_label(theme::NOTICE_TEXT, error);
            if ui.button("Retry saving settings").observed(ui).clicked() {
                session.retry_requested = true;
            }
        } else if session.is_saved(app_settings) {
            if session.saved_at.is_some() { ui.label("Settings saved"); }
        } else {
            ui.label("Settings have unsaved changes");
        }
        if session.needs_restart(app_settings) {
            ui.label("Restart required; close and reopen FractalEngine.");
            if ui.add_enabled(session.is_saved(app_settings), egui::Button::new("Close to restart..."))
                .observed(ui)
                .on_disabled_hover_text("Wait for these settings to be saved before closing.")
                .clicked() {
                topbar.close_pending = true;
            }
        }
        settings_heading(ui, "Rendering");
        settings_row(ui, "Render distance", |ui, label| {
            ui.add(
                egui::DragValue::new(&mut app_settings.render_distance)
                    .speed(1.0)
                    .range(0.0..=crate::settings::RENDER_DISTANCE_MAX)
                    .suffix(" wu"),
            ).labelled(ui, label)
            .on_hover_text(
                "Global residency horizon (0 stops spawning); this petal's shared setting and the session override take precedence",
            );
        });
        ui.label(
            egui::RichText::new(residency_line)
                .small()
                .color(theme::TEXT_DIM),
        )
        .on_hover_text(
            "Live residency: nodes / stamp groups within the render horizon (camera-weighted, ×1.5 exit ring, shrunk under GPU pressure) and streamed tile bundles in RAM",
        );
        render_petal_render_distance_controls(
            ui,
            hierarchy,
            nav,
            local_role,
            render_override,
            db_tx,
        );
        settings_row(ui, "Mesh budget ceiling", |ui, label| {
            usize_drag(
                ui,
                &mut app_settings.mesh_budget_ceiling,
                crate::plugin::MAX_MESH_INSTANCES,
            )
            .labelled(ui, label)
            .on_hover_text("MeshInstanceBudget.ceiling — the mesh-instance watchdog gate");
        });
        settings_row(ui, "Entity cap", |ui, label| {
            usize_drag(
                ui,
                &mut app_settings.entity_cap,
                crate::verse_manager::MAX_PETAL_NODES,
            ).labelled(ui, label)
            .on_hover_text(
                "Soft cap on resident scene nodes per petal (nearest to the camera first); applies live",
            );
        });
        settings_row(ui, "Stamp ceiling", |ui, label| {
            usize_drag(
                ui,
                &mut app_settings.stamp_ceiling,
                crate::verse_manager::MAX_STAMPS_PER_PETAL,
            )
            .labelled(ui, label)
            .on_hover_text("Soft cap on path-asset stamps per petal; applies on the next restamp");
        });

        // hexon_scale FR-8/FR-10 ruler overlays (mirrored into fe-terrain's
        // RulerSettings by the main binary's terrain bridge).
        ui.checkbox(&mut app_settings.show_scale_bar, "Scale bar")
            .observed(ui)
            .on_hover_text("Bottom-left real-distance scale bar");
        ui.checkbox(&mut app_settings.show_world_grid, "Metric grid")
            .observed(ui)
            .on_hover_text("Ground grid at round metric spacing (1 / 2 / 5 × 10ⁿ m); needs a map");

        ui.add_space(8.0);
        settings_heading(ui, "Terrain");
        let mut offline = app_settings.tile_mode == crate::settings::TileSourceMode::Offline;
        if ui
            .checkbox(&mut offline, "Offline tiles (no network)")
            .observed(ui)
            .on_hover_text(
                "Serve terrain only from installed hexons; off = each petal's own tile source mode",
            )
            .changed()
        {
            app_settings.tile_mode = if offline {
                crate::settings::TileSourceMode::Offline
            } else {
                crate::settings::TileSourceMode::Hybrid
            };
        }

        ui.add_space(8.0);
        settings_heading(ui, "Camera");
        settings_row(ui, "Orbit sensitivity", |ui, label| {
            multiplier_drag(ui, &mut app_settings.camera_sensitivity).labelled(ui, label);
        });
        settings_row(ui, "Zoom speed", |ui, label| {
            multiplier_drag(ui, &mut app_settings.camera_zoom_speed).labelled(ui, label);
        });

        ui.add_space(8.0);
        settings_heading(ui, "Network");
        render_relay_setting(ui, app_settings);
        render_registry_setting(ui, app_settings);

        // --- Timeseries fabric (M2/F6 — per active verse) ---
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Timeseries Fabric")
                .strong()
                .color(theme::TEXT_SECTION),
        );
        ui.add_space(2.0);
        render_timeseries_fabric_controls(ui, ts_settings, nav, db_tx);
        render_development_reset(ui, session, nav, db_tx, topbar);
    });
}

fn render_development_reset(
    ui: &mut egui::Ui,
    session: &mut crate::settings::SettingsSession,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    topbar: &mut super::topbar::TopbarState,
) {
    use crate::settings::DatabaseResetState;
    if !session.developer_tools_enabled() { return; }
    ui.add_space(12.0);
    ui.separator();
    settings_heading(ui, "Danger zone");
    ui.label("Reset the local database, including object data, history and API token records. This node must own every stored Verse.");
    ui.label("Identity, local preferences and cached blob files are retained. Shared peers are not erased.");
    match &session.reset {
        DatabaseResetState::Awaiting { .. } => { ui.label("Reset pending. Do not repeat this request."); return; }
        DatabaseResetState::Unconfirmed { .. } => { ui.label("Reset outcome unconfirmed. Waiting for the original reply; no retry has been sent."); return; }
        DatabaseResetState::Finished { cleared, error } => {
            if let Some(error) = error { ui.colored_label(theme::NOTICE_TEXT, error); }
            if *cleared {
                ui.label("Local database cleared. Close and reopen FractalEngine before reconnecting.");
                if ui.button("Close after database reset...").observed(ui).clicked() { topbar.close_pending = true; }
            } else if ui.button("Dismiss reset error").observed(ui).clicked() { session.reset = DatabaseResetState::Ready; }
            return;
        }
        DatabaseResetState::Ready => {}
    }
    let ready = session.reset_runtime_ready();
    if !ready {
        ui.label("Reset is unavailable while API and synchronization writers are running. Close the app for isolated developer maintenance.");
    }
    let scope = super::modal::confirmation_scope(ui.ctx(), "development-reset", nav.active_scope().as_deref().unwrap_or("no-active-verse"));
    if session.reset_confirmation_scope != Some(scope.value()) {
        session.reset_confirmation.clear();
        session.reset_confirmation_revision = session.reset_confirmation_revision.wrapping_add(1);
        session.reset_confirmation_scope = Some(scope.value());
    }
    ui.add_enabled_ui(ready, |ui| {
        ui.label("Type RESET to confirm");
        let response = ui.add(egui::TextEdit::singleline(&mut session.reset_confirmation)).named(ui, "Database reset confirmation");
        if response.changed() { session.reset_confirmation_revision = session.reset_confirmation_revision.wrapping_add(1); }
        let typed = session.reset_confirmation == "RESET";
        let confirmed = ui.add_enabled_ui(typed, |ui| {
            crate::panels::widgets::confirm_button(ui, (scope, session.reset_confirmation_revision, "reset-local-database"), "Reset local database", "Confirm reset local database", "Reset local database")
        }).inner;
        if confirmed && ready && typed {
            let request_id = format!("ui-reset-{}", ulid::Ulid::new());
            if db_tx.send(DbCommand::ResetDevelopmentDatabase { request_id: request_id.clone() }).is_ok() {
                session.reset = DatabaseResetState::Awaiting { request_id, sent_at: ui.input(|input| input.time) };
            } else {
                bevy::log::warn!("DB command channel closed during local reset request");
                session.reset = DatabaseResetState::Finished { cleared: false, error: Some("Reset request was not sent: database connection unavailable".into()) };
            }
            session.reset_confirmation.clear();
        }
    });
}

/// Per-petal render-distance controls (p2p_asset_streaming S6, campaign plan
/// §3 step 6): "This petal" is the durable `petal.render_distance` column
/// (editable by Editor+, read-only for a Viewer — `SetPetalRenderDistance`
/// is the DB thread's own Editor+ gate, this is the UI-side courtesy), and
/// "Session override" is the local-only knob for a weak-machine viewer
/// (`PetalRenderOverride`, never persisted, cleared on petal change). No
/// active petal → nothing to show (mirrors the timeseries fabric's calm hint).
fn render_petal_render_distance_controls(
    ui: &mut egui::Ui,
    hierarchy: &VerseManager,
    nav: &NavigationManager,
    local_role: &LocalUserRole,
    render_override: &mut crate::settings::PetalRenderOverride,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return;
    };
    let Some(petal) = hierarchy.find_petal(petal_id) else {
        return;
    };

    let draft_id = ui.id().with("petal-render-distance-draft");
    let mut draft = super::setting_draft::SettingDraft::load(
        ui,
        draft_id,
        petal_id,
        petal.render_distance.unwrap_or(0.0),
    );
    let editable = local_role.can_edit();
    ui.add_space(4.0);
    ui.push_id(("petal-distance", petal_id), |ui| {
        settings_row(ui, "This petal (shared)", |ui, label| {
            let response = ui
                .add_enabled(
                    editable,
                    egui::DragValue::new(&mut draft.value)
                        .speed(1.0)
                        .range(0.0..=crate::settings::RENDER_DISTANCE_MAX)
                        .clamp_existing_to_range(false)
                        .update_while_editing(false)
                        .suffix(" wu"),
                )
                .labelled(ui, label)
                .on_hover_text("Shared override. 0 inherits global. Saves when editing finishes.")
                .on_disabled_hover_text(
                    "Editor access is required to change this petal's shared distance.",
                );
            draft.observe(ui, "petal-distance", &response);
        });
    });
    if let Some(value) = draft.next_value().filter(|_| editable) {
        if db_tx
            .send(DbCommand::SetPetalRenderDistance {
                petal_id: petal_id.to_owned(),
                render_distance: (value > 0.0).then_some(value),
                auth: CallerAuth::Local,
                correlation_id: None,
            })
            .is_ok()
        {
            draft.sent(ui.input(|input| input.time));
        } else {
            bevy::log::warn!(
                "db_tx channel closed - SetPetalRenderDistance dropped for {petal_id}"
            );
            draft.failed();
        }
    }
    draft.store(ui, draft_id);
    settings_row(ui, "Session override", |ui, label| {
        let mut value = render_override.for_petal(petal_id).unwrap_or(0.0);
        if ui
            .add(
                egui::DragValue::new(&mut value)
                    .speed(1.0)
                    .range(0.0..=crate::settings::RENDER_DISTANCE_MAX)
                    .suffix(" wu"),
            ).labelled(ui, label)
            .on_hover_text(
                "Local-only, never shared; beats both this petal's value and the global one; cleared on petal change",
            )
            .changed()
        {
            let render_distance = (value > 0.0).then_some(value);
            crate::actions::hexon::set_render_override(
                render_override,
                petal_id.to_string(),
                render_distance,
            );
        }
    });
}

/// Bold sub-heading inside the Settings section.
fn settings_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .strong()
            .color(theme::TEXT_SECTION),
    );
    ui.add_space(4.0);
}

/// One `label  widget` row in the Settings section.
fn settings_row(
    ui: &mut egui::Ui,
    label: &str,
    widget: impl FnOnce(&mut egui::Ui, &egui::Response),
) {
    ui.horizontal(|ui| {
        let label = ui.label(egui::RichText::new(label).small().color(theme::TEXT_DIM));
        widget(ui, &label);
    });
}

/// `usize` knob in `1..=max` (egui has no native usize DragValue; round-trips via u64).
fn usize_drag(ui: &mut egui::Ui, value: &mut usize, max: usize) -> egui::Response {
    let mut wide = *value as u64;
    let response = ui.add(egui::DragValue::new(&mut wide).range(1..=max as u64));
    if response.changed() {
        *value = wide as usize;
    }
    response
}

/// Camera multiplier knob (× the controller's default).
fn multiplier_drag(ui: &mut egui::Ui, value: &mut f32) -> egui::Response {
    ui.add(
        egui::DragValue::new(value)
            .speed(0.01)
            .range(crate::settings::CAMERA_MULTIPLIER_RANGE)
            .suffix(" ×"),
    )
}

/// Relay-server spec editor: only a spec `RelayConfig::parse` accepts is stored;
/// the sync thread reads it once at startup.
fn render_relay_setting(ui: &mut egui::Ui, app_settings: &mut AppSettings) {
    let buffer_id = ui.id().with("relay_spec_buffer");
    let mut buffer = ui
        .data_mut(|data| data.get_temp::<String>(buffer_id))
        .unwrap_or_else(|| app_settings.relay.clone());
    settings_row(ui, "Relay", |ui, label| {
        ui.add(egui::TextEdit::singleline(&mut buffer).desired_width(160.0))
            .labelled(ui, label)
            .on_hover_text(
                "default | disabled | comma-separated relay URLs (FE_SYNC_RELAY overrides)",
            );
    });
    match fe_sync::relay_config::RelayConfig::parse(&buffer) {
        Ok(_) => {
            let trimmed = buffer.trim();
            if trimmed != app_settings.relay {
                app_settings.relay = trimmed.to_string();
            }
            ui.label(
                egui::RichText::new("Applies after restart")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
        }
        Err(err) => {
            ui.label(
                egui::RichText::new(format!("Not saved: {err}"))
                    .small()
                    .color(theme::STATUS_OFFLINE),
            );
        }
    }
    ui.data_mut(|data| data.insert_temp(buffer_id, buffer));
}

/// Hosted hexon registry URL (PAS-P5 S7b, A6): a blank field means "not
/// configured" (`None`), which the Hexon Manager's Available tab shows as a
/// disabled "Install from registry" control rather than a faked one (ui_ux
/// §6). `FE_HEXON_REGISTRY_URL` overrides whatever is saved here.
fn render_registry_setting(ui: &mut egui::Ui, app_settings: &mut AppSettings) {
    let buffer_id = ui.id().with("hexon_registry_url_buffer");
    let mut buffer = ui
        .data_mut(|data| data.get_temp::<String>(buffer_id))
        .unwrap_or_else(|| app_settings.hexon_registry_url.clone().unwrap_or_default());
    settings_row(ui, "Hexon registry", |ui, label| {
        ui.add(egui::TextEdit::singleline(&mut buffer).desired_width(220.0))
            .labelled(ui, label)
            .on_hover_text(
                "Hosted fe-hexon-registry base URL, e.g. https://hexons.example.com \
                 (FE_HEXON_REGISTRY_URL overrides); blank = no registry configured",
            );
    });
    let trimmed = buffer.trim();
    let new_value = (!trimmed.is_empty()).then(|| trimmed.to_string());
    if new_value != app_settings.hexon_registry_url {
        app_settings.hexon_registry_url = new_value;
    }
    ui.data_mut(|data| data.insert_temp(buffer_id, buffer));
}

/// The per-verse timeseries-fabric controls (M2/F6 — A13): mode picker, the
/// `balanced` durability slider R (1..N; R clamps to the reachable peers at
/// placement), and the shard bucket width. Writes go out as
/// `DbCommand::SetVerseTimeseriesSettings`; the persisted values come back via
/// `VerseTimeseriesSettingsSet`, so this surface never shows an optimistic
/// guess. No active verse → a calm hint (ui_ux §7), never blank.
fn render_timeseries_fabric_controls(
    ui: &mut egui::Ui,
    ts_settings: &mut crate::timeseries_settings::TimeseriesSettingsState,
    nav: &NavigationManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
) {
    use crate::timeseries_settings as tss;

    let Some(verse_id) = nav.active_verse_id.as_deref() else {
        ui.label(
            egui::RichText::new("Select a verse to configure its timeseries fabric.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        return;
    };

    let draft_id = ui.id().with("timeseries-settings-draft");
    let mut draft = super::setting_draft::SettingDraft::load(
        ui,
        draft_id,
        verse_id,
        ts_settings.settings_for(verse_id),
    );
    ui.push_id(("timeseries-settings", verse_id), |ui| {
        ui.label(egui::RichText::new("Mode").small().color(theme::TEXT_DIM));
        for mode in [
            fe_runtime::timeseries::TimeseriesMode::Mirror,
            fe_runtime::timeseries::TimeseriesMode::Sharded,
            fe_runtime::timeseries::TimeseriesMode::Balanced,
        ] {
            let selected = draft.value.mode == mode;
            if ui
                .radio(selected, tss::mode_label(mode))
                .observed(ui)
                .on_hover_text(mode_tooltip(mode))
                .clicked()
            {
                draft.value.mode = mode;
                draft.finish();
            }
        }
        ui.add_space(4.0);
        settings_row(ui, "Durability (R)", |ui, label| {
            let enabled = tss::replication_slider_enabled(draft.value.mode);
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Slider::new(
                        &mut draft.value.replication_factor,
                        1..=tss::MAX_REPLICATION_FACTOR,
                    )
                    .integer()
                    .clamping(egui::SliderClamping::Edits)
                    .update_while_editing(false)
                    .suffix(" peers/shard"),
                )
                .labelled_by(label.id)
                .named(ui, "Timeseries durability replication factor")
                .on_hover_text(
                    "Saves when editing finishes. Actual replicas depend on reachable peers.",
                )
                .on_disabled_hover_text(
                    "R applies to balanced mode only (sharded = one host, mirror = all).",
                );
            draft.observe(ui, "replication-factor", &response);
        });
        ui.add_space(4.0);
        settings_row(ui, "Bucket width", |ui, label| {
            let response = ui
                .add(
                    egui::DragValue::new(&mut draft.value.bucket_width_ms)
                        .speed(60_000.0)
                        .range(1..=u64::MAX)
                        .clamp_existing_to_range(false)
                        .update_while_editing(false)
                        .suffix(" ms"),
                )
                .labelled_by(label.id)
                .named(ui, "Timeseries bucket width in milliseconds")
                .on_hover_text("Time-bucket width in milliseconds. Saves when editing finishes.");
            draft.observe(ui, "bucket-width", &response);
        });
        ui.horizontal_wrapped(|ui| {
            for (ms, label) in tss::BUCKET_WIDTH_PRESETS {
                let selected = draft.value.bucket_width_ms == ms;
                if ui.selectable_label(selected, label).observed(ui).clicked() {
                    draft.value.bucket_width_ms = ms;
                    draft.finish();
                }
            }
        });
    });
    if let Some(value) = draft.next_value() {
        if db_tx
            .send(DbCommand::SetVerseTimeseriesSettings {
                verse_id: verse_id.to_owned(),
                mode: value.mode.as_str().to_owned(),
                replication_factor: value.replication_factor,
                bucket_width_ms: value.bucket_width_ms,
            })
            .is_ok()
        {
            draft.sent(ui.input(|input| input.time));
        } else {
            bevy::log::warn!(
                "db_tx channel closed - timeseries settings change dropped for verse {verse_id}"
            );
            draft.failed();
        }
    }
    draft.store(ui, draft_id);
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(
            "Shard = (petal, anchor, time bucket). Placement is capacity-aware: the smallest peer never caps total data — overflow spills to fallback/relay seeders.",
        )
        .small()
        .color(theme::TEXT_MUTED)
        .italics(),
    );
}

/// Hover text explaining what a mode does (D2 #1).
fn mode_tooltip(mode: fe_runtime::timeseries::TimeseriesMode) -> &'static str {
    use fe_runtime::timeseries::TimeseriesMode;
    match mode {
        TimeseriesMode::Mirror => {
            "Every peer holds every shard (full replication). Highest durability, highest storage."
        }
        TimeseriesMode::Sharded => {
            "Each peer hosts its declared portion only — one host per shard, capacity-aware."
        }
        TimeseriesMode::Balanced => {
            "Each shard lives on R peers chosen by capacity-aware placement (durability slider below)."
        }
    }
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
    active_verse: Option<&str>,
    downloads: &crate::terrain_map::TilesetDownloads,
    app_settings: &AppSettings,
    // X-02: role-gates Set/Clear/Remove in `dialogs::render_hexon_manager`.
    local_role: &LocalUserRole,
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
    // Resolved fresh every frame (cheap: one env read) so `FE_HEXON_REGISTRY_URL`
    // enables the control even when the persisted setting is blank — same
    // precedence as `resolve_hexon_registry_url`'s doc comment, applied here
    // because (like the relay spec) the Settings UI only ever edits the
    // persisted field, never the env-resolved effective one.
    let env_registry_url = std::env::var(crate::settings::HEXON_REGISTRY_URL_ENV_VAR).ok();
    let registry_url = crate::settings::resolve_hexon_registry_url(
        env_registry_url.as_deref(),
        app_settings.hexon_registry_url.as_deref(),
    );
    let close = crate::dialogs::render_hexon_manager(
        ctx,
        state,
        ui_mgr,
        petal_map,
        active_petal,
        active_verse,
        &downloads.0,
        registry_url.as_deref(),
        local_role,
    );
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
        filter_text: String::new(),
        active_tab: HexonManagerTab::Installed,
        storage_info: StorageInfoDto {
            base_dir: String::new(),
            total_bytes: 0,
            count: 0,
        },
        loading: true,
        pending_remove: None,
        registry_uri_buf: String::new(),
    }
}

// ---------------------------------------------------------------------------
// Shared chrome: header + rail + separator + scrollable body, used by every
// section fn above. All five sections are filled as of P5 (FR-8).
// ---------------------------------------------------------------------------

/// Empty-group copy (spec §4.5 — calm state, never blank).
pub(crate) const EMPTY_GROUP_COPY: &str = "No members — select nodes and use Add to group";

/// Groups section (D-10): the ONE editor for the active petal's Group
/// registry. Every edit is a whole-registry `UiAction::SetGroupRegistry`
/// (pure transforms in `visibility::groups`); membership edits are
/// `UiAction::SetNodeGroups` for the selected node.
fn render_groups_section(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    ui_mgr: &mut UiManager,
    visibility: &VisibilityState,
    active_petal_id: Option<&str>,
    selected_node_id: Option<&str>,
    local_role: &LocalUserRole,
) {
    section_chrome(ctx, state, RightSidebarSection::Groups, |ui| {
        let Some(petal_id) = active_petal_id else {
            ui.label(
                egui::RichText::new("Open a petal to manage its Groups.")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
            return;
        };
        render_groups_controls(
            ui,
            ui_mgr,
            visibility,
            petal_id,
            selected_node_id,
            local_role,
        );
    });
}

fn render_groups_controls(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    visibility: &VisibilityState,
    petal_id: &str,
    selected_node_id: Option<&str>,
    local_role: &LocalUserRole,
) {
    let editable = local_role.can_edit();
    if !editable {
        ui.label("Editor access to this petal is required to change Groups.");
    }
    ui.add_enabled_ui(editable, |ui| {
        let groups = visibility.groups();
        let push_registry = |ui_mgr: &mut UiManager, next: Vec<crate::visibility::Group>| {
            if !editable {
                return;
            }
            ui_mgr.push_action(UiAction::SetGroupRegistry {
                petal_id: petal_id.to_string(),
                groups: next,
            });
        };

        // Create.
        let new_name_id = egui::Id::new(("groups_section_new_name", petal_id));
        let mut new_name: String = ui.data(|d| d.get_temp(new_name_id)).unwrap_or_default();
        ui.horizontal(|ui| {
            let edit = ui
                .add(
                    egui::TextEdit::singleline(&mut new_name)
                        .hint_text("New group name")
                        .char_limit(MAX_GROUP_NAME_CHARS)
                        .desired_width(160.0),
                )
                .named(ui, "New visibility group name");
            let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            // The DB refuses an over-long name or a registry past its cap.
            let can_add = !new_name.trim().is_empty() && groups.len() < MAX_VISIBILITY_GROUPS;
            let clicked = ui
                .add_enabled(can_add, egui::Button::new("Add group"))
                .observed(ui)
                .on_disabled_hover_text(if !editable {
                    "Editor access to this petal is required.".to_owned()
                } else if groups.len() >= MAX_VISIBILITY_GROUPS {
                    format!("Limit: {MAX_VISIBILITY_GROUPS} Groups per petal.")
                } else {
                    "Enter a Group name first.".to_owned()
                })
                .clicked();
            if editable && can_add && (clicked || submitted) {
                push_registry(ui_mgr, group_ops::with_group_added(groups, &new_name));
                new_name.clear();
            }
        });
        ui.data_mut(|d| d.insert_temp(new_name_id, new_name));
        if groups.len() >= MAX_VISIBILITY_GROUPS {
            ui.label(format!(
                "Limit reached: {MAX_VISIBILITY_GROUPS} Groups per petal."
            ));
            ui.label("Remove a Group before adding another.");
        }
        ui.add_space(6.0);

        if groups.is_empty() {
            ui.label(
                egui::RichText::new("No Groups yet — name one above.")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
            return;
        }

        // A stamp instance has no node row to carry `view.groups`.
        let selected_node_id =
            selected_node_id.filter(|id| crate::verse_manager::parse_stamp_marker_id(id).is_none());
        let selected_groups = selected_node_id.map(|id| visibility.node_groups(id));
        for (index, group) in groups.iter().enumerate() {
            ui.push_id((petal_id, &group.id), |ui| {
                ui.separator();
                ui.horizontal(|ui| {
                    let mut visible = group.visible;
                    if ui
                        .checkbox(&mut visible, "")
                        .named(ui, format_args!("Show visibility group {}", group.name))
                        .on_hover_text("Show or hide every member of this Group")
                        .changed()
                    {
                        push_registry(
                            ui_mgr,
                            group_ops::with_group_visible(groups, &group.id, visible),
                        );
                    }
                    let buffer_id =
                        egui::Id::new(("groups_section_rename", petal_id, group.id.as_str()));
                    let mut name: String = ui
                        .data(|d| d.get_temp(buffer_id))
                        .unwrap_or_else(|| group.name.clone());
                    let edit = ui
                        .add(
                            egui::TextEdit::singleline(&mut name)
                                .char_limit(MAX_GROUP_NAME_CHARS)
                                .desired_width(120.0),
                        )
                        .named(ui, format_args!("Rename visibility group {}", group.name));
                    let escaped = (edit.has_focus() || edit.lost_focus())
                        && ui.input(|input| input.key_pressed(egui::Key::Escape));
                    let submitted =
                        edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if escaped {
                        edit.surrender_focus();
                        ui.data_mut(|d| d.remove::<String>(buffer_id));
                    } else if submitted {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() && trimmed != group.name {
                            push_registry(
                                ui_mgr,
                                group_ops::with_group_renamed(groups, &group.id, trimmed),
                            );
                        }
                        ui.data_mut(|d| d.remove::<String>(buffer_id));
                    } else if edit.has_focus() {
                        ui.data_mut(|d| d.insert_temp(buffer_id, name));
                    } else {
                        ui.data_mut(|d| d.remove::<String>(buffer_id));
                    }
                    if ui
                        .add_enabled(index > 0, egui::Button::new("\u{25B2}").small())
                        .named(ui, format_args!("Move visibility group {} up", group.name))
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        push_registry(ui_mgr, group_ops::with_group_moved(groups, index, -1));
                    }
                    if ui
                        .add_enabled(
                            index + 1 < groups.len(),
                            egui::Button::new("\u{25BC}").small(),
                        )
                        .named(
                            ui,
                            format_args!("Move visibility group {} down", group.name),
                        )
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        push_registry(ui_mgr, group_ops::with_group_moved(groups, index, 1));
                    }
                    if crate::panels::widgets::confirm_button(
                        ui,
                        ("delete-group", petal_id, &group.id),
                        "Delete",
                        "Confirm delete",
                        format_args!("Delete visibility group {}", group.name),
                    ) {
                        push_registry(ui_mgr, group_ops::with_group_removed(groups, &group.id));
                    }
                });

                let members = visibility.group_member_count(&group.id);
                ui.label(
                    egui::RichText::new(if members == 0 {
                        EMPTY_GROUP_COPY.to_string()
                    } else {
                        format!("{members} member{}", if members == 1 { "" } else { "s" })
                    })
                    .small()
                    .color(theme::TEXT_MUTED),
                );

                if let (Some(node_id), Some(current)) = (selected_node_id, selected_groups) {
                    let member = current.iter().any(|id| id == &group.id);
                    let label = if member {
                        "Remove from group"
                    } else {
                        "Add selection to group"
                    };
                    let can_join = member || current.len() < MAX_VIEW_GROUPS;
                    if ui
                        .add_enabled(can_join, egui::Button::new(label).small())
                        .named(ui, format_args!("{label}: {}", group.name))
                        .on_disabled_hover_text(if !editable {
                            "Editor access to this petal is required.".to_owned()
                        } else {
                            format!("A node can belong to at most {MAX_VIEW_GROUPS} Groups.")
                        })
                        .clicked()
                    {
                        if let Some(next) = group_ops::membership_with(current, &group.id, !member)
                            .filter(|_| editable)
                        {
                            ui_mgr.push_action(UiAction::SetNodeGroups {
                                node_id: node_id.to_string(),
                                groups: next,
                            });
                        }
                    }
                }
            });
        }
    })
    .response
    .on_disabled_hover_text("Editor access to this petal is required to change Groups.");
}

/// Shared SidePanel chrome (header + rail + separator + scrollable, padded
/// body) — the seam EVERY section fn renders through, Inspector included
/// (D10). That "every" is load-bearing: the rail lives in here, so a section
/// that opts out (as the Inspector used to) makes the whole rail invisible in
/// the resting state.
pub(crate) fn section_chrome(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    section: RightSidebarSection,
    body: impl FnOnce(&mut egui::Ui),
) {
    let max_w = super::layout_policy::right_width(ctx);
    egui::SidePanel::right("right_section")
        .resizable(true)
        .default_width(320.0)
        .width_range(max_w.min(260.0)..=max_w)
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
            crate::ui_shell::layout_policy::scroll(
                ui,
                section_label(section),
                [true, true],
                egui::ScrollArea::both()
                    .id_salt(format!("right_section_scroll_{section:?}"))
                    .auto_shrink([false, false]),
                |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| body(ui));
                },
            );
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
        RightSidebarSection::Groups => "\u{25C9}",
    }
}

/// Compact icon rail: one small button per section; click toggles it. Uses the
/// toolbar's single-source `mode_button_fill` to mark the active one. The rail
/// is a TOGGLE affordance (click the active one to fall back to the
/// selection-default), which is why it writes `toggle` rather than the
/// idempotent `reveal` that `UiAction::RevealSection` uses.
fn section_rail(ui: &mut egui::Ui, state: &mut RightSidebarState) {
    ui.horizontal_wrapped(|ui| {
        ui.add_space(8.0);
        for section in ALL_SECTIONS {
            let glyph = section_glyph(section);
            let active = state.is_active(section);
            if ui
                .add(
                    egui::Button::new(glyph).fill(crate::panels::toolbar::mode_button_fill(active)),
                )
                .named(
                    ui,
                    format_args!("Toggle {} section", section_label(section)),
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
        RightSidebarState {
            requested: req,
            ..Default::default()
        }
    }

    #[test]
    fn sidebar_groups_and_durable_settings_journey() {
        let readout = ResidencyReadout {
            nodes: 12,
            stamp_groups: 3,
            tile_bundles: 4,
            tile_bytes: 3 * 1024 * 1024 / 2,
        };
        assert_eq!(
            readout.line(),
            "Resident: 12 nodes · 3 stamp groups · 4 tile bundles (1.5 MiB)"
        );

        let ctx = egui::Context::default();
        let mut time = 0.0;
        let editor = LocalUserRole {
            role: Some(fe_database::RoleLevel::Editor),
        };
        let viewer = LocalUserRole {
            role: Some(fe_database::RoleLevel::Viewer),
        };
        let unresolved = LocalUserRole::default();
        let mut notices = UiManager::default();
        let mut visibility = VisibilityState::default();
        visibility.replace_groups(vec![crate::visibility::Group {
            id: "g1".into(),
            name: "Original".into(),
            visible: true,
        }]);
        let group_frame = |role: &LocalUserRole,
                           events: Vec<egui::Event>,
                           time: &mut f64,
                           visibility: &VisibilityState,
                           notices: &mut UiManager| {
            *time += 0.05;
            super::super::layout_policy::begin_frame(&ctx, true);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    time: Some(*time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    super::super::layout_policy::begin_ui_pass(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        render_groups_controls(ui, notices, visibility, "p", Some("node"), role);
                    });
                },
            );
            super::super::layout_policy::observations(&ctx)
        };
        let group_click = |role: &LocalUserRole,
                           point: egui::Pos2,
                           time: &mut f64,
                           visibility: &VisibilityState,
                           notices: &mut UiManager| {
            let mut observed = group_frame(
                role,
                vec![egui::Event::PointerMoved(point)],
                time,
                visibility,
                notices,
            );
            for pressed in [true, false] {
                observed = group_frame(
                    role,
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    time,
                    visibility,
                    notices,
                );
            }
            observed
        };
        for role in [&viewer, &unresolved] {
            let observed = group_frame(role, vec![], &mut time, &visibility, &mut notices);
            for control in observed.controls {
                group_click(
                    role,
                    control.rect.center(),
                    &mut time,
                    &visibility,
                    &mut notices,
                );
            }
            assert!(
                notices.drain_actions().is_empty(),
                "every Groups mutation must refuse Viewer/unresolved roles"
            );
            assert_eq!(visibility.groups()[0].name, "Original");
        }
        let key = |key, pressed, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let select_all = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let observed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            observed.controls[3].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        group_frame(
            &editor,
            vec![
                key(egui::Key::A, true, select_all),
                egui::Event::Text("Discard me".into()),
            ],
            &mut time,
            &visibility,
            &mut notices,
        );
        group_frame(
            &editor,
            vec![key(egui::Key::Escape, true, egui::Modifiers::NONE)],
            &mut time,
            &visibility,
            &mut notices,
        );
        assert!(notices.drain_actions().is_empty());
        assert_eq!(visibility.groups()[0].name, "Original");
        let observed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            observed.controls[3].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        group_frame(
            &editor,
            vec![
                key(egui::Key::A, true, select_all),
                egui::Event::Text("Renamed".into()),
            ],
            &mut time,
            &visibility,
            &mut notices,
        );
        assert!(
            notices.drain_actions().is_empty(),
            "typing a rename cannot write"
        );
        group_frame(
            &editor,
            vec![key(egui::Key::Enter, true, egui::Modifiers::NONE)],
            &mut time,
            &visibility,
            &mut notices,
        );
        let actions = notices.drain_actions();
        assert!(
            matches!(&actions[..], [UiAction::SetGroupRegistry { petal_id, groups }] if petal_id == "p" && groups[0].name == "Renamed")
        );
        let observed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            observed.controls[6].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        assert!(notices.drain_actions().is_empty(), "first Delete only arms");
        let armed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            armed.controls[7].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        assert!(notices.drain_actions().is_empty());
        assert_eq!(visibility.groups().len(), 1);
        let observed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            observed.controls[6].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        let armed = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            armed.controls[6].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        let actions = notices.drain_actions();
        assert!(
            matches!(&actions[..], [UiAction::SetGroupRegistry { petal_id, groups }] if petal_id == "p" && groups.is_empty())
        );
        visibility.replace_groups(
            (0..MAX_VISIBILITY_GROUPS)
                .map(|index| crate::visibility::Group {
                    id: format!("g{index}"),
                    name: format!("Group {index}"),
                    visible: true,
                })
                .collect(),
        );
        ctx.data_mut(|data| {
            data.insert_temp(
                egui::Id::new(("groups_section_new_name", "p")),
                "Over cap".to_owned(),
            )
        });
        let full = group_frame(&editor, vec![], &mut time, &visibility, &mut notices);
        group_click(
            &editor,
            full.controls[1].rect.center(),
            &mut time,
            &visibility,
            &mut notices,
        );
        assert!(notices.drain_actions().is_empty());
        assert_eq!(visibility.groups().len(), MAX_VISIBILITY_GROUPS);

        use crate::verse_manager::{FractalEntry, PetalEntry, VerseEntry};
        use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings};
        let hierarchy_for = |distance| {
            VerseManager::from_verses(vec![VerseEntry {
                id: "v".into(),
                name: "Verse".into(),
                fractals: vec![FractalEntry {
                    id: "f".into(),
                    name: "Fractal".into(),
                    petals: vec![PetalEntry {
                        id: "p".into(),
                        name: "Petal".into(),
                        render_distance: Some(distance),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }])
        };
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal_in_fractal("v", "Verse", "f", "Fractal", "p");
        let settings_ctx = egui::Context::default();
        let mut settings_time = 100.0;
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut override_state = crate::settings::PetalRenderOverride::default();
        let mut ts = crate::timeseries_settings::TimeseriesSettingsState::default();
        let mut persisted = VerseTimeseriesSettings {
            mode: TimeseriesMode::Balanced,
            replication_factor: 3,
            bucket_width_ms: 3_600_000,
        };
        ts.set("v", persisted);
        let settings_frame =
            |timeseries: bool,
             events: Vec<egui::Event>,
             time: &mut f64,
             tree: &VerseManager,
             ts: &mut crate::timeseries_settings::TimeseriesSettingsState,
             override_state: &mut crate::settings::PetalRenderOverride| {
                *time += 0.05;
                super::super::layout_policy::begin_frame(&settings_ctx, true);
                let _ = settings_ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000.0, 800.0),
                        )),
                        time: Some(*time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        super::super::layout_policy::begin_ui_pass(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            if timeseries {
                                render_timeseries_fabric_controls(ui, ts, &nav, &tx);
                            } else {
                                render_petal_render_distance_controls(
                                    ui,
                                    tree,
                                    &nav,
                                    &editor,
                                    override_state,
                                    &tx,
                                );
                            }
                        });
                    },
                );
                super::super::layout_policy::observations(&settings_ctx)
            };
        let drag = |timeseries,
                    at: egui::Pos2,
                    time: &mut f64,
                    tree: &VerseManager,
                    ts: &mut crate::timeseries_settings::TimeseriesSettingsState,
                    override_state: &mut crate::settings::PetalRenderOverride| {
            settings_frame(
                timeseries,
                vec![egui::Event::PointerMoved(at)],
                time,
                tree,
                ts,
                override_state,
            );
            settings_frame(
                timeseries,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                time,
                tree,
                ts,
                override_state,
            );
            assert!(rx.is_empty(), "pressing a numeric control must not write");
            for delta in [16.0, 40.0] {
                settings_frame(
                    timeseries,
                    vec![egui::Event::PointerMoved(at + egui::vec2(delta, 0.0))],
                    time,
                    tree,
                    ts,
                    override_state,
                );
                assert!(rx.is_empty(), "durable drag frames must not write");
            }
            settings_frame(
                timeseries,
                vec![egui::Event::PointerButton {
                    pos: at + egui::vec2(40.0, 0.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                time,
                tree,
                ts,
                override_state,
            )
        };
        let tree = hierarchy_for(10.0);
        persisted.replication_factor = 100;
        ts.set("v", persisted);
        settings_frame(
            true,
            vec![],
            &mut settings_time,
            &tree,
            &mut ts,
            &mut override_state,
        );
        assert!(
            rx.is_empty(),
            "opening a control must not rewrite stored out-of-range values"
        );
        assert_eq!(ts.settings_for("v").replication_factor, 100);
        persisted.replication_factor = 3;
        ts.set("v", persisted);
        let observed = settings_frame(
            false,
            vec![],
            &mut settings_time,
            &tree,
            &mut ts,
            &mut override_state,
        );
        drag(
            false,
            observed.controls[0].rect.center(),
            &mut settings_time,
            &tree,
            &mut ts,
            &mut override_state,
        );
        let DbCommand::SetPetalRenderDistance {
            petal_id,
            render_distance: Some(first),
            auth: CallerAuth::Local,
            correlation_id: None,
        } = rx.try_recv().unwrap()
        else {
            panic!("one local durable distance command is required")
        };
        assert_eq!(petal_id, "p");
        assert!(first > 10.0 && rx.is_empty());
        let observed = settings_frame(
            false,
            vec![],
            &mut settings_time,
            &tree,
            &mut ts,
            &mut override_state,
        );
        drag(
            false,
            observed.controls[0].rect.center(),
            &mut settings_time,
            &tree,
            &mut ts,
            &mut override_state,
        );
        assert!(
            rx.is_empty(),
            "a newer finalized draft waits for the first stored echo"
        );
        let readback = hierarchy_for(first);
        settings_frame(
            false,
            vec![],
            &mut settings_time,
            &readback,
            &mut ts,
            &mut override_state,
        );
        let DbCommand::SetPetalRenderDistance {
            render_distance: Some(second),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("the newer draft must follow the matching stored echo")
        };
        assert!(second > first && rx.is_empty());
        settings_frame(
            false,
            vec![],
            &mut settings_time,
            &readback,
            &mut ts,
            &mut override_state,
        );
        assert!(
            rx.is_empty(),
            "a repeated older echo cannot trigger another write"
        );
        let confirmed = hierarchy_for(second);
        settings_frame(
            false,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );

        let observed = settings_frame(
            false,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let at = observed.controls[0].rect.center();
        for event in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            settings_frame(
                false,
                vec![event],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
        }
        let confirmed = hierarchy_for(second + 500.0);
        settings_frame(
            false,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let outside = egui::pos2(800.0, 500.0);
        for pressed in [true, false] {
            settings_frame(
                false,
                vec![egui::Event::PointerButton {
                    pos: outside,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
        }
        settings_frame(
            false,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        assert!(
            rx.is_empty(),
            "merely focusing and leaving a clean field cannot overwrite a newer stored value"
        );

        let observed = settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let clean_bucket = observed.controls[4].rect.center();
        for event in [
            egui::Event::PointerMoved(clean_bucket),
            egui::Event::PointerButton {
                pos: clean_bucket,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: clean_bucket,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            settings_frame(
                true,
                vec![event],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
        }
        assert!(
            rx.is_empty(),
            "clean bucket focus must not create commit eligibility"
        );
        let slider_at = egui::pos2(
            observed.controls[3].rect.left() + 48.0,
            observed.controls[3].rect.center().y,
        );
        drag(
            true,
            slider_at,
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let DbCommand::SetVerseTimeseriesSettings {
            verse_id,
            mode,
            replication_factor,
            bucket_width_ms,
        } = rx.try_recv().unwrap()
        else {
            panic!("R release must write one configuration")
        };
        assert_eq!(verse_id, "v");
        assert_eq!(mode, "balanced");
        assert!(rx.is_empty());
        persisted.replication_factor = replication_factor;
        persisted.bucket_width_ms = bucket_width_ms;
        ts.set("v", persisted);
        settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let arrow = if replication_factor < crate::timeseries_settings::MAX_REPLICATION_FACTOR {
            egui::Key::ArrowRight
        } else {
            egui::Key::ArrowLeft
        };
        settings_frame(
            true,
            vec![key(arrow, true, egui::Modifiers::NONE)],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        assert!(rx.is_empty(), "holding a slider key must not write");
        settings_frame(
            true,
            vec![key(arrow, false, egui::Modifiers::NONE)],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let DbCommand::SetVerseTimeseriesSettings {
            replication_factor, ..
        } = rx.try_recv().unwrap()
        else {
            panic!("slider keyboard release must finalize")
        };
        assert!(rx.is_empty());
        persisted.replication_factor = replication_factor;
        ts.set("v", persisted);
        let observed = settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let bucket_at = observed.controls[4].rect.center();
        for event in [
            egui::Event::PointerMoved(bucket_at),
            egui::Event::PointerButton {
                pos: bucket_at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: bucket_at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            settings_frame(
                true,
                vec![event],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
        }
        settings_frame(
            true,
            vec![
                key(egui::Key::A, true, select_all),
                egui::Event::Text("7200000".into()),
            ],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        assert!(rx.is_empty(), "partial numeric text must not write");
        settings_frame(
            true,
            vec![key(egui::Key::Enter, true, egui::Modifiers::NONE)],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let DbCommand::SetVerseTimeseriesSettings {
            bucket_width_ms, ..
        } = rx.try_recv().unwrap()
        else {
            panic!("numeric Enter must finalize")
        };
        assert_eq!(bucket_width_ms, 7_200_000);
        assert!(rx.is_empty());
        settings_time += 31.0;
        settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        assert!(
            rx.is_empty(),
            "an unconfirmed timeout keeps the draft without automatic retries"
        );
        let timed_out = settings_frame(
            true,
            vec![],
            &mut settings_time,
            &confirmed,
            &mut ts,
            &mut override_state,
        );
        let at = timed_out.controls[4].rect.center();
        for event in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            key(egui::Key::ArrowLeft, false, egui::Modifiers::NONE),
        ] {
            settings_frame(
                true,
                vec![event],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
            assert!(
                rx.is_empty(),
                "focus/key release alone cannot retry a timed-out draft"
            );
        }
        for pressed in [true, false] {
            settings_frame(
                true,
                vec![egui::Event::PointerButton {
                    pos: outside,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                &mut settings_time,
                &confirmed,
                &mut ts,
                &mut override_state,
            );
            assert!(rx.is_empty(), "clean blur cannot retry a timed-out draft");
        }

        let failure_ctx = egui::Context::default();
        let (closed_tx, closed_rx) = crossbeam::channel::unbounded();
        drop(closed_rx);
        let mut attempts = 0;
        let failure_frame = |events: Vec<egui::Event>, time: &mut f64, attempts: &mut usize| {
            *time += 0.05;
            super::super::layout_policy::begin_frame(&failure_ctx, true);
            let _ = failure_ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    )),
                    time: Some(*time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    super::super::layout_policy::begin_ui_pass(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let id = ui.id().with("failed-setting");
                        let mut draft =
                            super::super::setting_draft::SettingDraft::load(ui, id, "p", 1.0_f32);
                        let response = ui
                            .add(egui::DragValue::new(&mut draft.value).update_while_editing(false))
                            .named(ui, "Failed setting fixture");
                        draft.observe(ui, "distance", &response);
                        if let Some(value) = draft.next_value() {
                            *attempts += 1;
                            assert!(closed_tx
                                .send(DbCommand::SetPetalRenderDistance {
                                    petal_id: "p".into(),
                                    render_distance: Some(value),
                                    auth: CallerAuth::Local,
                                    correlation_id: None,
                                })
                                .is_err());
                            draft.failed();
                        }
                        draft.store(ui, id);
                    });
                },
            );
            super::super::layout_policy::observations(&failure_ctx)
        };
        let observed = failure_frame(vec![], &mut settings_time, &mut attempts);
        let at = observed.controls[0].rect.center();
        for event in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(at + egui::vec2(40.0, 0.0)),
            egui::Event::PointerButton {
                pos: at + egui::vec2(40.0, 0.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            failure_frame(vec![event], &mut settings_time, &mut attempts);
        }
        assert_eq!(
            attempts, 1,
            "one finalized edit must try the closed channel once"
        );
        let failed = failure_frame(vec![], &mut settings_time, &mut attempts);
        let at = failed.controls[0].rect.center();
        for event in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            key(egui::Key::ArrowLeft, false, egui::Modifiers::NONE),
            egui::Event::PointerButton {
                pos: outside,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: outside,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            failure_frame(vec![event], &mut settings_time, &mut attempts);
            assert_eq!(
                attempts, 1,
                "clean focus, key release and blur cannot retry a failed draft"
            );
        }

        let retry_ctx = egui::Context::default();
        let mut quarantine = super::super::modal::ModalManagerState::default();
        quarantine.disabled_panels.extend(["right.settings".to_owned(), "right.maps".to_owned()]);
        let mut retry_state = st(Some(RightSidebarSection::Settings));
        let mut retry_time = 0.0;
        let retry_frame = |events: Vec<egui::Event>, time: &mut f64, state: &mut RightSidebarState, modal: &mut super::super::modal::ModalManagerState| {
            *time += 0.05;
            super::super::layout_policy::begin_frame(&retry_ctx, true);
            let _ = retry_ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))), time: Some(*time), events, ..Default::default() }, |ctx| {
                super::super::layout_policy::begin_ui_pass(ctx);
                if modal.disabled_panels.contains("right.settings") { render_quarantined_section(ctx, state, RightSidebarSection::Settings, modal); }
            });
            super::super::layout_policy::observations(&retry_ctx)
        };
        let observed = retry_frame(vec![], &mut retry_time, &mut retry_state, &mut quarantine);
        let retry_point = observed.controls.last().unwrap().hit.center();
        for pressed in [true, false] {
            retry_frame(vec![egui::Event::PointerMoved(retry_point), egui::Event::PointerButton { pos: retry_point, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE }], &mut retry_time, &mut retry_state, &mut quarantine);
        }
        assert!(!quarantine.disabled_panels.contains("right.settings"));
        assert!(quarantine.disabled_panels.contains("right.maps"), "native Retry clears only its own section");

        let reset_ctx = egui::Context::default();
        let mut reset_session = crate::settings::SettingsSession::default();
        reset_session.reset_confirmation = "RESET".into();
        let mut reset_topbar = super::super::topbar::TopbarState::default();
        super::super::layout_policy::begin_frame(&reset_ctx, true);
        let _ = reset_ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0))), ..Default::default() }, |ctx| {
            super::super::layout_policy::begin_ui_pass(ctx);
            egui::CentralPanel::default().show(ctx, |ui| { render_development_reset(ui, &mut reset_session, &nav, &tx, &mut reset_topbar); });
        });
        let observed = super::super::layout_policy::observations(&reset_ctx);
        assert!(!observed.controls.is_empty());
        assert!(observed.controls.iter().all(|control| !control.allowed), "typing RESET cannot bypass runtime quiescence");
        assert!(rx.is_empty());
    }

    const ALL: [RightSidebarSection; 7] = ALL_SECTIONS;

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
        s.reveal(RightSidebarSection::Maps);
        s.toggle_collapsed();
        assert_eq!(active_section(&s, true, false), None);
        assert_eq!(s.requested, Some(RightSidebarSection::Maps));
        s.toggle_collapsed();
        assert_eq!(
            active_section(&s, true, false),
            Some(RightSidebarSection::Maps)
        );
        s.toggle_collapsed();
        s.reveal(RightSidebarSection::TerrainTools);
        assert!(!s.collapsed);
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
