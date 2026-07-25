//! Right-sidebar area manager (FR-6): the single right region, one section at a
//! time (RATIFIED Q-2). `active_section` precedence is portal > explicit toggle
//! > selection-default; the Inspector is the never-blank fallback. There is ONE
//! render fn per section variant — the seam downstream slices fill (P4:
//! PathTools/TerrainTools/ProposalReport; P5: Tool). See
//! `fe-ui/src/ui_shell/AGENTS.md` §right.

use bevy::prelude::Resource;
use bevy_egui::egui;

use crate::actions::UiManager;
use crate::asset_ops::AssetDownloadStatus;
use crate::navigation_manager::NavigationManager;
use crate::panels::{inspector, portal_toolbar};
use crate::plugin::{InspectorFormState, LocalUserRole};
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
        Some(RightSidebarSection::Tool) => render_tool_section(ctx, state),
        Some(RightSidebarSection::PathTools) => render_path_tools_section(ctx, state),
        Some(RightSidebarSection::TerrainTools) => render_terrain_tools_section(ctx, state),
        Some(RightSidebarSection::ProposalReport) => render_proposal_report_section(ctx, state),
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

/// Tool section — P5 (ui_shell Phase 5) fills this. Calm placeholder this phase.
fn render_tool_section(ctx: &egui::Context, state: &mut RightSidebarState) {
    section_placeholder(
        ctx,
        state,
        RightSidebarSection::Tool,
        "Tool settings are in the floating Tools panel this phase.",
    );
}

/// Path-tools section — P4 fills this. Calm placeholder this phase.
fn render_path_tools_section(ctx: &egui::Context, state: &mut RightSidebarState) {
    section_placeholder(
        ctx,
        state,
        RightSidebarSection::PathTools,
        "Path tools arrive in this panel in a later phase.",
    );
}

/// Terrain-tools section — P4 fills this. Calm placeholder this phase.
fn render_terrain_tools_section(ctx: &egui::Context, state: &mut RightSidebarState) {
    section_placeholder(
        ctx,
        state,
        RightSidebarSection::TerrainTools,
        "Terrain tools are in the floating Terrain palette this phase.",
    );
}

/// Proposal-report section — P4 fills this. Calm placeholder this phase.
fn render_proposal_report_section(ctx: &egui::Context, state: &mut RightSidebarState) {
    section_placeholder(
        ctx,
        state,
        RightSidebarSection::ProposalReport,
        "Select a proposal to see its metrics here (coming soon).",
    );
}

// ---------------------------------------------------------------------------
// Shared placeholder chrome (rail + hint). Downstream fills the section fns
// above; this helper is only the transitional calm placeholder body.
// ---------------------------------------------------------------------------

/// A calm one-section placeholder: the section rail + a single hint line (never
/// blank — `ui_ux.md §7`). Shared by the four not-yet-filled sections.
fn section_placeholder(
    ctx: &egui::Context,
    state: &mut RightSidebarState,
    section: RightSidebarSection,
    hint: &str,
) {
    let max_w = ctx.viewport_rect().width() * 0.8;
    egui::SidePanel::right("right_section")
        .resizable(true)
        .default_width(260.0)
        .width_range(200.0..=max_w)
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
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(hint).color(theme::TEXT_DIM).small());
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
