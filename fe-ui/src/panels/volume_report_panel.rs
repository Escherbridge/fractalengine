//! terrain_proposal_delta_viz_20261025 S6 (FR-6/7/8, R-4): the "Volume
//! report" block of the renamed Proposal/Volume report section
//! (`ui_shell::right_sidebar`). Resolves the current selection (a committed
//! palette proposal, a Brush-created earthwork region, or — when neither is
//! explicitly selected — the live sculpt-shape draft, reusing the same
//! `circle_footprint`/`rect_footprint`/`region_draft` sources
//! `terrain_preview.rs` uses for FR-3's preview), round-trips it through
//! fe-renderer's `VolumeReportRequest`/`VolumeReportResult` seam to
//! fe-terrain's real composite integrator, and renders TRUE area/cut/fill/net
//! + per-item breakdown + contained-node list + Copy-as-TSV. fe-ui never
//! depends on fe-terrain (NFR-5) — every cross-crate type here is
//! fe-renderer's seam, same idiom as `terrain_preview.rs`. See
//! `panels/AGENTS.md` §volume-report.

use bevy::prelude::*;
use bevy_egui::egui;

use fe_renderer::terrain_overlay::{VolumeReportRequest, VolumeReportResult};

use crate::actions::terrain_proposal::{SculptShapeMode, SculptToolState};
use crate::actions::UiManager;
use crate::navigation_manager::NavigationManager;
use crate::panels::proposal_report_panel::earthwork_regions_from_terrain;
use crate::panels::terrain_tools_panel::{circle_footprint, rect_footprint};
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalEditState;
use crate::theme;
use crate::verse_manager::{NodeEntry, PetalEntry, VerseManager};

// ---------------------------------------------------------------------------
// Selection resolution (pure)
// ---------------------------------------------------------------------------

/// What the Volume report is currently scoped to. Priority (most-deliberate
/// wins): an explicit `ProposalEditState.selected` id — resolved against the
/// palette mirror first, then Brush-created regions parsed from the terrain
/// doc (same discriminator `proposal_report_panel::earthwork_regions_from_
/// terrain` uses) — falling back to the live sculpt-shape draft only when
/// nothing is explicitly selected (FR-6's "drawn selection footprint").
#[derive(Debug, Clone)]
pub(crate) enum ResolvedVolumeSelection {
    Proposal { id: String, footprint: Vec<[f32; 2]> },
    Region { id: String, footprint: Vec<[f32; 2]> },
    Drawn { footprint: Vec<[f32; 2]> },
}

impl ResolvedVolumeSelection {
    /// Stable identity for change detection — distinct from the human label
    /// so a rename never looks like "nothing changed".
    fn key(&self) -> String {
        match self {
            Self::Proposal { id, .. } => format!("proposal:{id}"),
            Self::Region { id, .. } => format!("region:{id}"),
            Self::Drawn { .. } => "drawn".to_string(),
        }
    }

    fn footprint(&self) -> &[[f32; 2]] {
        match self {
            Self::Proposal { footprint, .. }
            | Self::Region { footprint, .. }
            | Self::Drawn { footprint } => footprint,
        }
    }

    fn display_label(&self) -> String {
        match self {
            Self::Proposal { id, .. } => format!("Selected proposal {id}"),
            Self::Region { id, .. } => format!("Selected region {id}"),
            Self::Drawn { .. } => "Drawn footprint".to_string(),
        }
    }
}

/// Resolve the current volume-report selection. Pure — takes every input as
/// a plain reference so it is testable without a Bevy `App`. Mirrors
/// `terrain_preview::sculpt_shape_draft`'s shape-footprint sourcing exactly
/// (same cap: Polygon needs >=3 drafted points), but this is a SEPARATE
/// concept from the FR-3 preview — selecting a volume to report is not the
/// same action as arming an edit, even though both can read the same draft.
pub(crate) fn resolve_volume_selection(
    proposal_state: &ProposalEditState,
    terrain_json: Option<&serde_json::Value>,
    sculpt: &SculptToolState,
    brush_tool_active: bool,
) -> Option<ResolvedVolumeSelection> {
    if let Some(selected) = proposal_state.selected.as_deref() {
        if let Some(record) = proposal_state.proposals.iter().find(|r| r.id == selected) {
            return Some(ResolvedVolumeSelection::Proposal {
                id: record.id.clone(),
                footprint: record.footprint.clone(),
            });
        }
        let regions = earthwork_regions_from_terrain(terrain_json);
        if let Some(region) = regions.into_iter().find(|r| r.id == selected) {
            return Some(ResolvedVolumeSelection::Region {
                id: region.id,
                footprint: region.footprint,
            });
        }
    }
    if brush_tool_active {
        let footprint = match sculpt.shape_mode {
            SculptShapeMode::Brush => None,
            SculptShapeMode::Circle => Some(circle_footprint(sculpt.radius)),
            SculptShapeMode::Rect => Some(rect_footprint(sculpt.radius)),
            SculptShapeMode::Polygon => {
                (sculpt.region_draft.len() >= 3).then(|| sculpt.region_draft.clone())
            }
        };
        if let Some(footprint) = footprint.filter(|f| f.len() >= 3) {
            return Some(ResolvedVolumeSelection::Drawn { footprint });
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Resource + request/response systems
// ---------------------------------------------------------------------------

/// The Volume report's live query + latest true-figures answer. `result` is
/// `None` until a matching `VolumeReportResult` arrives — the render side
/// must never mistake a stale/previous-selection result for the current one
/// (same "absence is the signal" idiom as `terrain_preview::ProposalPreview`).
#[derive(Resource, Default, Debug, Clone)]
pub struct VolumeReportState {
    display_label: Option<String>,
    selection_key: Option<String>,
    pub(crate) footprint: Vec<[f32; 2]>,
    petal_id: Option<String>,
    correlation_id: Option<String>,
    result: Option<VolumeReportResult>,
    next_id: u64,
}

impl VolumeReportState {
    fn clear(&mut self) {
        self.display_label = None;
        self.selection_key = None;
        self.footprint.clear();
        self.petal_id = None;
        self.correlation_id = None;
        self.result = None;
    }
}

/// FR-6: resolve the current selection and, when it (or the active petal)
/// changed since last frame, mint a fresh correlation id and request the true
/// figures from fe-terrain. Registered in `PostUpdate` (plugin.rs) so it
/// observes this frame's `process_ui_actions`/selection writes (both run in
/// `Update`), mirroring `terrain_preview::publish_proposal_preview`.
pub fn sync_volume_report_request(
    nav: Res<NavigationManager>,
    proposal_state: Res<ProposalEditState>,
    petal_map: Res<PetalMapState>,
    sculpt: Res<SculptToolState>,
    tool: Res<ToolState>,
    mut state: ResMut<VolumeReportState>,
    mut requests: MessageWriter<VolumeReportRequest>,
    mut last_petal: Local<Option<String>>,
) {
    let active_petal = nav.active_petal_id.clone();
    // Petal switch: the outgoing petal's selection/result must never bleed
    // into the next one (mirrors `terrain_preview`'s own rule).
    if *last_petal != active_petal {
        *last_petal = active_petal.clone();
        state.clear();
        return;
    }
    let Some(petal_id) = active_petal else {
        state.clear();
        return;
    };

    let resolved = resolve_volume_selection(
        &proposal_state,
        petal_map.terrain_json.as_ref(),
        &sculpt,
        tool.active_tool == Tool::Brush,
    );
    let Some(resolved) = resolved else {
        state.clear();
        return;
    };

    let key = resolved.key();
    let footprint = resolved.footprint().to_vec();
    let unchanged = state.selection_key.as_deref() == Some(key.as_str())
        && state.footprint == footprint
        && state.petal_id.as_deref() == Some(petal_id.as_str());
    if unchanged {
        return;
    }

    state.next_id += 1;
    let correlation_id = format!("volume-report:{}", state.next_id);
    state.display_label = Some(resolved.display_label());
    state.selection_key = Some(key);
    state.footprint = footprint.clone();
    state.petal_id = Some(petal_id.clone());
    state.correlation_id = Some(correlation_id.clone());
    state.result = None;
    requests.write(VolumeReportRequest {
        correlation_id,
        petal_id,
        footprint,
        elevation_band: None,
    });
}

/// Mirrors the matching `VolumeReportResult` into [`VolumeReportState`],
/// dropping any reply whose correlation id doesn't match the in-flight
/// request — a stale or wrong-selection answer must never surface (mirrors
/// `terrain_preview::drain_preview_reports`' cross-petal drop).
pub fn drain_volume_report_result(
    mut results: MessageReader<VolumeReportResult>,
    mut state: ResMut<VolumeReportState>,
) {
    for result in results.read() {
        if state.correlation_id.as_deref() == Some(result.correlation_id.as_str()) {
            state.result = Some(result.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Display selection (pure) — R-4: never falls back to an area×|delta| guess.
// ---------------------------------------------------------------------------

/// What the panel shows for the current state: nothing selected, a request
/// in flight (no true figure yet), or the true figure. There is no fourth
/// "approximate" variant — that is the R-4 regression this type guards
/// against (see `selected_region_report_shows_true_cut_fill_not_area_times_delta`).
#[derive(Debug)]
pub(crate) enum VolumeDisplay<'a> {
    Unselected,
    Pending { label: &'a str },
    Ready {
        label: &'a str,
        result: &'a VolumeReportResult,
    },
}

pub(crate) fn resolve_volume_display(state: &VolumeReportState) -> VolumeDisplay<'_> {
    let Some(label) = state.display_label.as_deref() else {
        return VolumeDisplay::Unselected;
    };
    match &state.result {
        Some(result) => VolumeDisplay::Ready { label, result },
        None => VolumeDisplay::Pending { label },
    }
}

/// "m"/"wu" per NFR-4 — never mislabels world units as meters.
pub(crate) fn volume_unit_suffix(scaled: bool) -> &'static str {
    if scaled {
        "m"
    } else {
        "wu"
    }
}

// ---------------------------------------------------------------------------
// FR-8: nodes inside the selection footprint (pure, in-memory tree only —
// AGENTS anti-pattern #3, no DB round-trip).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NodeHit {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: &'static str,
}

/// Best-effort node kind from the fields `NodeEntry` exposes today (no
/// generic `node_kind` property bag is surfaced to fe-ui's in-memory tree —
/// see `fe-ui/src/AGENTS.md`). "node" is the honest fallback for a stamp,
/// path point, or anything else `NodeEntry` can't distinguish; never a guess
/// dressed up as certain (FR-8 "kind when known").
fn node_kind(node: &NodeEntry) -> &'static str {
    if node.asset_path.is_some() {
        "asset"
    } else if node.webpage_url.is_some() {
        "portal"
    } else if node.name.starts_with("Earthwork ") {
        // Matches `actions::terrain_proposal::earthwork_display_name`'s
        // "Earthwork {op} {region_id}" convention.
        "earthwork"
    } else {
        "node"
    }
}

/// Even-odd ray-casting point-in-polygon. Self-contained — fe-ui must not
/// depend on fe-terrain's own `point_in_polygon` (NFR-5).
fn point_in_polygon(point: [f32; 2], polygon: &[[f32; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (xi, zi) = (polygon[i][0], polygon[i][1]);
        let (xj, zj) = (polygon[j][0], polygon[j][1]);
        if (zi > point[1]) != (zj > point[1]) {
            let t = (point[1] - zi) / (zj - zi);
            let x_at = xi + t * (xj - xi);
            if point[0] < x_at {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// FR-8: every node in `petal` whose XZ position lies inside `footprint`.
pub(crate) fn nodes_in_footprint(petal: &PetalEntry, footprint: &[[f32; 2]]) -> Vec<NodeHit> {
    petal
        .nodes
        .iter()
        .filter(|n| point_in_polygon([n.position[0], n.position[2]], footprint))
        .map(|n| NodeHit {
            id: n.id.clone(),
            name: n.name.clone(),
            kind: node_kind(n),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// FR-9: Copy as TSV
// ---------------------------------------------------------------------------

/// Totals + per-item rows + contained-node rows as tab-separated text (FR-9,
/// BI-egress spirit). Pure.
pub(crate) fn volume_report_to_tsv(result: &VolumeReportResult, nodes: &[NodeHit]) -> String {
    let unit = volume_unit_suffix(result.scaled);
    let mut out = String::new();
    out.push_str("Volume report\n");
    out.push_str(&format!("Area\t{:.3}\t{unit}\u{b2}\n", result.area_m2));
    out.push_str(&format!("Cut\t{:.3}\t{unit}\u{b3}\n", result.cut_m3));
    out.push_str(&format!("Fill\t{:.3}\t{unit}\u{b3}\n", result.fill_m3));
    out.push_str(&format!("Net\t{:.3}\t{unit}\u{b3}\n", result.net_m3));
    out.push_str(&format!("Step\t{:.3}\t{unit}\n", result.step_m));
    out.push_str(&format!("Overlap flagged\t{}\n", result.has_overlap));
    out.push('\n');
    out.push_str("Item\tKind\tCut\tFill\tOverlap area\n");
    for item in &result.items {
        out.push_str(&format!(
            "{}\t{}\t{:.3}\t{:.3}\t{:.3}\n",
            item.id, item.kind, item.cut_m3, item.fill_m3, item.overlap_area_m2
        ));
    }
    out.push('\n');
    out.push_str("Node\tKind\n");
    for node in nodes {
        out.push_str(&format!("{}\t{}\n", node.name, node.kind));
    }
    out
}

// ---------------------------------------------------------------------------
// egui body
// ---------------------------------------------------------------------------

/// The "Volume report" block (D-DV-4/OD-4), hosted below
/// `proposal_report_panel::render_report_body` in the same renamed section
/// (`ui_shell::right_sidebar::render_proposal_report_section`). No
/// window/chrome here — the caller supplies `section_chrome`.
pub(crate) fn render_volume_report_body(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    state: &VolumeReportState,
    hierarchy: &VerseManager,
    active_petal: Option<&str>,
) {
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Volume report")
            .strong()
            .color(theme::TEXT_SECTION),
    );

    let (label, result) = match resolve_volume_display(state) {
        VolumeDisplay::Unselected => {
            ui.label(
                egui::RichText::new(
                    "Select a proposal, region, or draw a shape to see its volume report here.",
                )
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
            );
            return;
        }
        VolumeDisplay::Pending { label } => {
            ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
            ui.label(
                egui::RichText::new("Computing true cut/fill\u{2026}")
                    .small()
                    .color(theme::TEXT_MUTED)
                    .italics(),
            );
            return;
        }
        VolumeDisplay::Ready { label, result } => (label, result),
    };

    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
    ui.add_space(4.0);
    let unit = volume_unit_suffix(result.scaled);
    ui.label(format!("Area: {:.2} {unit}\u{b2}", result.area_m2));
    ui.label(format!("Cut: {:.2} {unit}\u{b3}", result.cut_m3));
    ui.label(format!("Fill: {:.2} {unit}\u{b3}", result.fill_m3));
    ui.label(format!("Net: {:.2} {unit}\u{b3}", result.net_m3));
    ui.label(
        egui::RichText::new(format!(
            "Lattice step: {:.2} {unit} (integration tolerance)",
            result.step_m
        ))
        .small()
        .color(theme::TEXT_DIM),
    );
    if !result.scaled {
        ui.add_space(4.0);
        ui.colored_label(theme::TEXT_MUTED, "no map scale — showing world units");
    }
    if result.has_overlap {
        ui.add_space(4.0);
        ui.colored_label(
            theme::TEXT_MUTED,
            "per-item figures overlap — totals use the combined surface",
        );
    }

    let nodes = active_petal
        .and_then(|id| hierarchy.find_petal(id))
        .map(|petal| nodes_in_footprint(petal, &state.footprint))
        .unwrap_or_default();

    if !result.items.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("Items ({})", result.items.len()))
                .strong()
                .color(theme::TEXT_SECTION),
        );
        for item in &result.items {
            let overlap = if item.overlap_area_m2 > 0.0 {
                format!(", overlap {:.2} {unit}\u{b2}", item.overlap_area_m2)
            } else {
                String::new()
            };
            ui.label(format!(
                "{} ({}): cut {:.2} {unit}\u{b3}, fill {:.2} {unit}\u{b3}{overlap}",
                item.id, item.kind, item.cut_m3, item.fill_m3
            ));
        }
    }

    if !nodes.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("Nodes inside ({})", nodes.len()))
                .strong()
                .color(theme::TEXT_SECTION),
        );
        for node in &nodes {
            ui.label(format!("{} \u{2014} {}", node.name, node.kind));
        }
    }

    ui.add_space(6.0);
    if ui.button("Copy as TSV").clicked() {
        let tsv = volume_report_to_tsv(result, &nodes);
        ui.ctx().copy_text(tsv);
        let now = ui.ctx().input(|i| i.time);
        ui_mgr.show_toast("Volume report copied as TSV", now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result(correlation_id: &str) -> VolumeReportResult {
        VolumeReportResult {
            correlation_id: correlation_id.to_string(),
            area_m2: 100.0,
            cut_m3: 12.5,
            fill_m3: 0.0,
            net_m3: -12.5,
            step_m: 1.0,
            items: Vec::new(),
            has_overlap: false,
            scaled: true,
        }
    }

    // --- S6 flow test 1: the R-4 regression ---

    #[test]
    fn selected_region_report_shows_true_cut_fill_not_area_times_delta() {
        let mut state = VolumeReportState::default();
        state.display_label = Some("Selected region r1".into());
        state.correlation_id = Some("c1".into());
        // The true composite cut/fill over sloped terrain (12.5/0.0) differs
        // from what the old area×|delta| approximation would have shown for
        // this footprint+delta (area 100 × |delta| 2.0 = 200.0) — the
        // regression this guards is a silent revert to that approximation.
        state.result = Some(sample_result("c1"));

        match resolve_volume_display(&state) {
            VolumeDisplay::Ready { result, .. } => {
                assert_eq!(result.cut_m3, 12.5);
                assert_eq!(result.fill_m3, 0.0);
                assert_ne!(
                    result.cut_m3, 200.0,
                    "must show the true figure, never the area×|delta| approximation"
                );
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn no_result_yet_is_pending_not_a_silent_approximation() {
        let mut state = VolumeReportState::default();
        state.display_label = Some("Selected proposal p1".into());
        state.correlation_id = Some("c1".into());
        // No result has arrived for c1 yet.
        match resolve_volume_display(&state) {
            VolumeDisplay::Pending { label } => assert_eq!(label, "Selected proposal p1"),
            other => panic!("expected Pending, got {other:?}"),
        }
    }

    #[test]
    fn nothing_selected_is_unselected() {
        assert!(matches!(
            resolve_volume_display(&VolumeReportState::default()),
            VolumeDisplay::Unselected
        ));
    }

    // --- S6 flow test 2: nodes_in_footprint, table-driven ---

    #[test]
    fn nodes_inside_selection_listed_with_kind() {
        let footprint = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let cases: [(&str, Option<&str>, Option<&str>, &str); 4] = [
            ("Rock", Some("rock.glb"), None, "asset"),
            ("Portal", None, Some("https://example"), "portal"),
            ("Earthwork raise r1", None, None, "earthwork"),
            ("Stray marker", None, None, "node"),
        ];
        let nodes: Vec<NodeEntry> = cases
            .iter()
            .enumerate()
            .map(|(i, (name, asset, url, _))| NodeEntry {
                id: format!("n{i}"),
                name: name.to_string(),
                position: [5.0, 0.0, 5.0],
                asset_path: asset.map(str::to_string),
                webpage_url: url.map(str::to_string),
                ..Default::default()
            })
            .collect();
        let petal = PetalEntry {
            id: "p1".into(),
            nodes,
            ..Default::default()
        };

        let hits = nodes_in_footprint(&petal, &footprint);
        assert_eq!(hits.len(), cases.len());
        for (i, (_, _, _, expected_kind)) in cases.iter().enumerate() {
            let hit = hits
                .iter()
                .find(|h| h.id == format!("n{i}"))
                .unwrap_or_else(|| panic!("case {i} missing from hits"));
            assert_eq!(hit.kind, *expected_kind, "case {i}");
        }

        // A node outside the footprint is excluded, not just mis-kinded.
        let far = PetalEntry {
            id: "p1".into(),
            nodes: vec![NodeEntry {
                id: "far".into(),
                name: "Far".into(),
                position: [500.0, 0.0, 500.0],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(nodes_in_footprint(&far, &footprint).is_empty());
    }

    // --- S6 flow test 3: unscaled petal shows world units ---

    #[test]
    fn unscaled_petal_report_shows_world_units_chip() {
        assert_eq!(volume_unit_suffix(true), "m");
        assert_eq!(volume_unit_suffix(false), "wu");
    }

    // --- S6 flow test 4: Copy as TSV ---

    #[test]
    fn copy_as_tsv_contains_totals_and_every_item_row() {
        use fe_renderer::terrain_overlay::VolumeItem;

        let mut result = sample_result("c1");
        result.items = vec![
            VolumeItem {
                id: "p1".into(),
                kind: "raise".into(),
                cut_m3: 1.0,
                fill_m3: 2.0,
                overlap_area_m2: 0.0,
            },
            VolumeItem {
                id: "r2".into(),
                kind: "earthwork_region".into(),
                cut_m3: 3.0,
                fill_m3: 4.0,
                overlap_area_m2: 5.0,
            },
        ];
        let nodes = vec![NodeHit {
            id: "n1".into(),
            name: "Rock".into(),
            kind: "asset",
        }];

        let tsv = volume_report_to_tsv(&result, &nodes);
        assert!(tsv.contains("Cut\t12.500"), "totals present:\n{tsv}");
        assert!(tsv.contains("Fill\t0.000"), "totals present:\n{tsv}");
        for item in &result.items {
            assert!(
                tsv.contains(&item.id),
                "item {} missing from TSV:\n{tsv}",
                item.id
            );
        }
        assert!(tsv.contains("Rock\tasset"), "node row missing:\n{tsv}");
    }

    // --- resolve_volume_selection: proposal beats a stale sculpt draft ---

    #[test]
    fn explicit_proposal_selection_wins_over_a_live_sculpt_draft() {
        let mut proposal_state = ProposalEditState::default();
        let id = proposal_state.push_new(
            crate::terrain_proposal_state::ProposalOp::Raise,
            vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            None,
            Some(2.0),
        );
        proposal_state.selected = Some(id.clone());

        let mut sculpt = SculptToolState::default();
        sculpt.shape_mode = SculptShapeMode::Circle;
        sculpt.radius = 5.0;

        let resolved = resolve_volume_selection(&proposal_state, None, &sculpt, true)
            .expect("a selected proposal resolves");
        match resolved {
            ResolvedVolumeSelection::Proposal { id: got, .. } => assert_eq!(got, id),
            other => panic!("expected Proposal, got {other:?}"),
        }
    }

    #[test]
    fn sculpt_draft_is_the_fallback_when_nothing_is_selected() {
        let proposal_state = ProposalEditState::default();
        let mut sculpt = SculptToolState::default();
        sculpt.shape_mode = SculptShapeMode::Circle;
        sculpt.radius = 5.0;

        let resolved = resolve_volume_selection(&proposal_state, None, &sculpt, true)
            .expect("a Circle draft resolves when Brush is active");
        assert!(matches!(resolved, ResolvedVolumeSelection::Drawn { .. }));

        // Brush tool not active -> no drawn fallback either.
        assert!(resolve_volume_selection(&proposal_state, None, &sculpt, false).is_none());
    }
}
