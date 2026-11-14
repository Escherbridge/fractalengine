//! Selected terrain volume, material and changed-space object impacts; see panels/AGENTS.md.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::accessibility::ResponseExt;

use fe_renderer::terrain_overlay::{VolumeReportRequest, VolumeReportResult, VolumeUnavailable};
use fe_runtime::messages::DbResult;

use crate::actions::terrain_proposal::{SculptShapeMode, SculptToolState};
use crate::actions::{UiAction, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::panels::proposal_report_panel::earthwork_regions_from_terrain;
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
    Proposal {
        id: String,
        footprint: Vec<[f32; 2]>,
    },
    Region {
        id: String,
        footprint: Vec<[f32; 2]>,
    },
    Drawn {
        footprint: Vec<[f32; 2]>,
    },
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
/// a plain reference so it is testable without a Bevy `App`. A drawn shape
/// uses `terrain_preview::shape_commit_footprint_world` — the WORLD-unit
/// footprint Apply would commit (fe-terrain and node positions are world
/// units; `SculptToolState` is meters). Selecting a volume to report is a
/// SEPARATE concept from the FR-3 preview, even though both read the draft.
pub(crate) fn resolve_volume_selection(
    proposal_state: &ProposalEditState,
    terrain_json: Option<&serde_json::Value>,
    sculpt: &SculptToolState,
    brush_tool_active: bool,
    world_scale: f64,
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
    if brush_tool_active && !matches!(sculpt.shape_mode, SculptShapeMode::Brush) {
        let footprint = crate::terrain_preview::shape_commit_footprint_world(sculpt, world_scale);
        if footprint.len() >= 3 {
            return Some(ResolvedVolumeSelection::Drawn { footprint });
        }
    }
    None
}

const REPORT_TARGET_LIMIT: usize = 128;

fn report_target_kind(
    proposal_state: &ProposalEditState,
    terrain: &serde_json::Value,
    id: &str,
) -> Option<&'static str> {
    let record = terrain
        .get("proposals")?
        .as_array()?
        .iter()
        .find(|record| record.get("id").and_then(serde_json::Value::as_str) == Some(id))?;
    let footprint = record.get("footprint")?.as_array()?;
    if footprint.len() < 3
        || !footprint.iter().all(|point| {
            serde_json::from_value::<[f32; 2]>(point.clone())
                .is_ok_and(|point| point.into_iter().all(f32::is_finite))
        })
    {
        return None;
    }
    if record.get("material").is_some() {
        return Some("Region");
    }
    (proposal_state.hydrated
        && proposal_state
            .proposals
            .iter()
            .any(|record| record.id == id))
    .then_some("Proposal")
}

fn current_report_terrain<'a>(
    petal_id: &str,
    active_petal: Option<&str>,
    map: &'a PetalMapState,
) -> Option<&'a serde_json::Value> {
    if active_petal != Some(petal_id) || map.petal_id.as_deref() != Some(petal_id) || !map.loaded {
        return None;
    }
    map.terrain_json.as_ref().filter(|terrain| {
        terrain.get("enabled").and_then(serde_json::Value::as_bool) != Some(false)
    })
}

/// Apply the read-only report action through the existing selection authority.
pub(crate) fn open_terrain_report(
    petal_id: &str,
    id: &str,
    nav: &NavigationManager,
    map: &PetalMapState,
    proposal_state: &mut ProposalEditState,
    sidebar: &mut crate::ui_shell::right_sidebar::RightSidebarState,
    can_view: bool,
) -> bool {
    let Some(terrain) =
        current_report_terrain(petal_id, nav.active_petal_id.as_deref(), map).filter(|_| can_view)
    else {
        return false;
    };
    if report_target_kind(proposal_state, terrain, id).is_none() {
        return false;
    }
    proposal_state.selected = Some(id.to_owned());
    sidebar.reveal(crate::ui_shell::right_sidebar::RightSidebarSection::ProposalReport);
    true
}

/// Bounded target controls for proposals and Brush-created regions.
pub(crate) fn render_report_target_picker(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    proposal_state: &ProposalEditState,
    map: &PetalMapState,
    active_petal: Option<&str>,
) {
    let Some((petal_id, terrain)) = active_petal
        .and_then(|id| current_report_terrain(id, active_petal, map).map(|terrain| (id, terrain)))
    else {
        ui.label("Load terrain for the active Petal to choose a report target.");
        return;
    };
    let regions = terrain
        .get("proposals")
        .and_then(serde_json::Value::as_array);
    let ids = proposal_state
        .proposals
        .iter()
        .map(|record| record.id.as_str())
        .chain(
            regions
                .into_iter()
                .flatten()
                .filter(|record| record.get("material").is_some())
                .filter_map(|record| record.get("id").and_then(serde_json::Value::as_str)),
        );
    let mut seen = std::collections::HashSet::new();
    let targets: Vec<_> = ids
        .filter_map(|id| report_target_kind(proposal_state, terrain, id).map(|kind| (id, kind)))
        .filter(|(id, _)| seen.insert(*id))
        .take(REPORT_TARGET_LIMIT + 1)
        .collect();
    ui.label(egui::RichText::new("Report target").strong());
    if targets.is_empty() {
        ui.label(
            "No saved proposals or Brush regions in this Petal. Draw a shape to report a draft.",
        );
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("terrain_report_targets")
        .max_height(144.0)
        .show(ui, |ui| {
            for (id, kind) in targets.iter().take(REPORT_TARGET_LIMIT) {
                let label = format!("{kind} {}", short_text(id));
                let selected = proposal_state.selected.as_deref() == Some(*id);
                let button = egui::Button::selectable(selected, label)
                    .truncate()
                    .min_size(egui::vec2(0.0, 24.0));
                if ui
                    .add(button)
                    .named(ui, format_args!("Open terrain report for {kind} {id}"))
                    .on_hover_text(*id)
                    .clicked()
                {
                    ui_mgr.push_action(UiAction::OpenTerrainReport {
                        petal_id: petal_id.to_owned(),
                        id: (*id).to_owned(),
                    });
                }
            }
        });
    if targets.len() > REPORT_TARGET_LIMIT {
        ui.label(
            "First 128 report targets shown; other targets can be opened from their viewport menu.",
        );
    }
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
    scene_revision: u64,
}

impl VolumeReportState {
    pub(crate) fn has_selection(&self) -> bool {
        self.selection_key.is_some()
    }
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
/// changed since last frame — or the active petal's terrain doc was
/// (re)loaded, so the composite under an unchanged selection moved — mint a
/// fresh correlation id and request the true figures from fe-terrain.
/// Registered in `PostUpdate` (plugin.rs) so it observes this frame's
/// `process_ui_actions`/selection writes (both run in `Update`), mirroring
/// `terrain_preview::publish_proposal_preview`.
#[allow(clippy::too_many_arguments)]
pub fn sync_volume_report_request(
    nav: Res<NavigationManager>,
    mut db_results: MessageReader<DbResult>,
    proposal_state: Res<ProposalEditState>,
    petal_map: Res<PetalMapState>,
    sculpt: Res<SculptToolState>,
    tool: Res<ToolState>,
    mut state: ResMut<VolumeReportState>,
    mut requests: MessageWriter<VolumeReportRequest>,
    mut last_petal: Local<Option<String>>,
    scene: Option<Res<fe_renderer::scene_analysis::SceneAnalysisSnapshot>>,
) {
    let active_petal = nav.active_petal_id.clone();
    // A confirmed terrain load for the active petal (any commit/delete/API
    // write) changes what an UNCHANGED selection contains: re-request.
    let terrain_reloaded = db_results.read().any(|result| {
        matches!(result, DbResult::PetalTerrainLoaded { petal_id, .. }
            if active_petal.as_deref() == Some(petal_id.as_str()))
    });
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
    if !petal_map.terrain_json.as_ref().is_some_and(|terrain| {
        terrain.get("enabled").and_then(serde_json::Value::as_bool) != Some(false)
    }) {
        state.clear();
        return;
    }

    let resolved = resolve_volume_selection(
        &proposal_state,
        petal_map.terrain_json.as_ref(),
        &sculpt,
        tool.active_tool == Tool::Brush && tool.terrain.polygon_matches(&nav,&petal_map),
        petal_map.world_scale,
    ).or_else(|| {
        (tool.active_tool==Tool::Brush).then(||tool.terrain.shape_preview(&nav,&petal_map)).flatten()
            .map(|draft|ResolvedVolumeSelection::Drawn{footprint:draft.footprint_world})
    });
    let Some(resolved) = resolved else {
        state.clear();
        return;
    };

    let key = resolved.key();
    let footprint = resolved.footprint().to_vec();
    let scene_revision = scene.as_ref().map_or(0, |scene| scene.revision);
    let unchanged = !terrain_reloaded
        && state.scene_revision == scene_revision
        && state.selection_key.as_deref() == Some(key.as_str())
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
    state.scene_revision = scene_revision;
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
    nav: Res<NavigationManager>,
    mut state: ResMut<VolumeReportState>,
) {
    for result in results.read() {
        if state.correlation_id.as_deref() == Some(result.correlation_id.as_str())
            && state.petal_id.as_deref() == Some(result.petal_id.as_str())
            && nav.active_petal_id.as_deref() == Some(result.petal_id.as_str())
            && (result.unavailable.is_some()
                || result.object_impacts.scene_revision == state.scene_revision)
        {
            state.result = Some(result.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// Display selection (pure) — R-4: never falls back to an area×|delta| guess.
// ---------------------------------------------------------------------------

/// Selection, pending, incomplete coverage, or a measured complete report.
#[derive(Debug)]
pub(crate) enum VolumeDisplay<'a> {
    Unselected,
    Pending {
        label: &'a str,
    },
    Incomplete {
        label: &'a str,
        result: &'a VolumeReportResult,
    },
    Ready {
        label: &'a str,
        result: &'a VolumeReportResult,
    },
    /// fe-terrain answered that this request cannot be measured.
    Unavailable {
        label: &'a str,
        reason: VolumeUnavailable,
    },
}

pub(crate) fn resolve_volume_display(state: &VolumeReportState) -> VolumeDisplay<'_> {
    let Some(label) = state.display_label.as_deref() else {
        return VolumeDisplay::Unselected;
    };
    match &state.result {
        Some(VolumeReportResult {
            unavailable: Some(reason),
            ..
        }) => VolumeDisplay::Unavailable {
            label,
            reason: *reason,
        },
        Some(result) if !result.is_complete() => VolumeDisplay::Incomplete { label, result },
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
        .take(fe_renderer::scene_analysis::SCENE_OBJECT_LIMIT)
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
    if !result.is_complete() {
        return String::new();
    }
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
    out.push_str("Item\tKind\tOperation\tMaterial\tCut\tFill\tOverlap area\n");
    for item in &result.items {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\n",
            tsv_cell(&item.id),
            tsv_cell(&item.kind),
            tsv_cell(&item.operation),
            tsv_cell(item.material.as_deref().unwrap_or("Not specified")),
            item.cut_m3,
            item.fill_m3,
            item.overlap_area_m2
        ));
    }
    out.push('\n');
    out.push_str("Node origin in footprint (up to 128; not intersections)\tKind\n");
    for node in nodes {
        out.push_str(&format!(
            "{}\t{}\n",
            tsv_cell(&node.name),
            tsv_cell(node.kind)
        ));
    }
    out.push_str("\nObject\tNode ID\tMethod\tMaterial JSON\tOverlap estimate\tStatus\n");
    for impact in &result.object_impacts.objects {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            tsv_cell(&impact.object.name),
            tsv_cell(&impact.object.node_id),
            impact_method(impact.object.geometry),
            tsv_cell(&material_label(&impact.object)),
            impact
                .overlap_m3
                .map_or_else(String::new, |volume| format!("{volume:.3}")),
            if impact.overlap_m3.is_some() {
                "sampled overlap"
            } else {
                "unresolved geometry"
            }
        ));
    }
    out.push_str(&format!(
        "Object analysis available\t{}\nScene revision\t{}\nObjects checked\t{}\nObjects omitted\t{}\nNon-geometric objects excluded\t{}\n",
        result.object_impacts.available, result.object_impacts.scene_revision, result.object_impacts.checked_objects,
        result.object_impacts.omitted_objects, result.object_impacts.excluded_non_geometric
    ));
    out
}

fn tsv_cell(value: &str) -> String {
    value.replace(['\t', '\r', '\n'], " ")
}

fn short_text(value: &str) -> String {
    let mut text: String = value.chars().take(256).collect();
    if value.chars().nth(256).is_some() {
        text.push('\u{2026}');
    }
    text
}

fn impact_method(geometry: fe_renderer::scene_analysis::ObjectGeometry) -> &'static str {
    use fe_renderer::scene_analysis::ObjectGeometry;
    match geometry {
        ObjectGeometry::AxisAlignedSolidBox => "Solid box / sampled terrain",
        ObjectGeometry::ConservativeWorldBounds => "Potential intersection / world bounds",
        ObjectGeometry::Unavailable => "Geometry unavailable",
    }
}

fn material_label(object: &fe_renderer::scene_analysis::SceneObjectBounds) -> String {
    if !object.material_loaded {
        "Loading / unavailable; open properties".into()
    } else if object.material_truncated {
        "Material exceeds preview limit; open properties".into()
    } else {
        object
            .material_json
            .clone()
            .unwrap_or_else(|| "Not specified".into())
    }
}

fn render_object_impacts(
    ui: &mut egui::Ui,
    report: &fe_renderer::scene_analysis::ObjectImpactReport,
    unit: &str,
    open_node: &mut Option<String>,
) {
    ui.separator();
    ui.label(egui::RichText::new("Objects in changed terrain space").strong());
    ui.label("Combined surface within the selected footprint");
    if !report.available {
        ui.label("Object analysis is waiting for a current scene snapshot.");
        return;
    }
    let unresolved = report
        .objects
        .iter()
        .filter(|impact| impact.overlap_m3.is_none())
        .count();
    let overlapping = report.objects.len() - unresolved;
    ui.label(format!(
        "{overlapping} overlaps / candidates; {} objects checked; {unresolved} unresolved",
        report.checked_objects
    ));
    ui.label(
        egui::RichText::new(format!(
            "Scene snapshot {} — updates up to four times per second",
            report.scene_revision
        ))
        .small()
        .color(theme::TEXT_DIM),
    );
    ui.label(egui::RichText::new("GLB/world bounds are conservative candidates, not mesh-solid intersections. Volumes sample the changed vertical space; surface-only contact is not measured.").small().color(theme::TEXT_DIM));
    if report.omitted_objects > 0 {
        ui.colored_label(
            theme::TEXT_MUTED,
            format!(
                "Partial scene: {} objects exceed the report limit.",
                report.omitted_objects
            ),
        );
    }
    if report.excluded_non_geometric > 0 {
        ui.label(format!(
            "{} non-geometric data/placard nodes excluded",
            report.excluded_non_geometric
        ));
    }
    for impact in &report.objects {
        ui.push_id(&impact.object.node_id, |ui| {
            render_object_impact_row(ui, impact, unit, open_node);
        });
    }
}

fn render_object_impact_row(
    ui: &mut egui::Ui,
    impact: &fe_renderer::scene_analysis::ObjectImpact,
    unit: &str,
    open_node: &mut Option<String>,
) {
    ui.add(egui::Label::new(egui::RichText::new(&impact.object.name).strong()).truncate())
        .on_hover_text(&impact.object.node_id);
    ui.label(impact_method(impact.object.geometry));
    let material = material_label(&impact.object);
    ui.add(egui::Label::new(format!("Material: {material}")).truncate())
        .on_hover_text(material);
    if let Some(volume) = impact.overlap_m3 {
        ui.label(format!("Overlap estimate: {volume:.3} {unit}\u{b3}"));
    } else {
        ui.label("Intersection unresolved: load usable geometry.");
    }
    let can_select = impact.object.bounds.is_some() && impact.object.selectable;
    let button = egui::Button::new("Select / open properties").min_size(egui::vec2(0.0, 24.0));
    if ui.add_enabled(can_select, button)
        .named(ui, format_args!("Open properties for impacted object {}", impact.object.name))
        .on_disabled_hover_text("Show and load the object before selecting it; its stored material is shown when available.")
        .clicked()
    {
        *open_node = Some(impact.object.node_id.clone());
    }
    ui.separator();
}

// ---------------------------------------------------------------------------
// egui body
// ---------------------------------------------------------------------------

/// Primary measured terrain and object-impact report body.
pub(crate) fn render_volume_report_body(
    ui: &mut egui::Ui,
    ui_mgr: &mut UiManager,
    state: &VolumeReportState,
    hierarchy: &VerseManager,
    active_petal: Option<&str>,
    open_node: &mut Option<String>,
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
        VolumeDisplay::Unavailable { label, reason } => {
            ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
            match reason {
                VolumeUnavailable::NoMap => {
                    ui.label("No map on this petal. Assign one in Maps.");
                    if ui
                        .add(egui::Button::new("Open Maps").min_size(egui::vec2(0.0, 24.0)))
                        .named(ui, format_args!("Open Maps section"))
                        .clicked()
                    {
                        ui_mgr.push_action(crate::panels::tool_options::reveal(
                            crate::ui_shell::right_sidebar::RightSidebarSection::Maps,
                        ));
                    }
                }
                VolumeUnavailable::WrongPetal => {
                    // The terrain side has not bound this petal's map yet; the panel re-requests when it loads.
                    ui.label("Waiting for this petal's map to load.");
                }
            }
            return;
        }
        VolumeDisplay::Incomplete { label, result } => {
            ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
            ui.label(if result.has_finite_measurements() {
                "Volume unavailable: terrain coverage is incomplete."
            } else {
                "Volume unavailable: calculation is not finite."
            });
            ui.label(format!(
                "Sampled {} of {} required terrain points.",
                result.sampled_pairs, result.expected_samples
            ));
            ui.label("The report updates when terrain becomes available.");
            return;
        }
        VolumeDisplay::Ready { label, result } => (label, result),
    };

    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
    ui.add_space(4.0);
    let unit = volume_unit_suffix(result.scaled);
    if let Some(selected) = state
        .selection_key
        .as_deref()
        .and_then(|key| key.split_once(':').map(|(_, id)| id))
    {
        if let Some(item) = result.items.iter().find(|item| item.id == selected) {
            ui.label(egui::RichText::new(format!("Selected {}", item.operation)).strong());
            let material = item.material.as_deref().unwrap_or("Not specified");
            ui.add(egui::Label::new(format!("Material: {}", short_text(material))).truncate())
                .on_hover_text(material.chars().take(4096).collect::<String>());
            ui.label(
                egui::RichText::new(format!(
                    "Fill: {:.2} {unit}\u{b3}  ·  Cut: {:.2} {unit}\u{b3}",
                    item.fill_m3, item.cut_m3
                ))
                .strong(),
            );
            ui.label(format!(
                "Net: {:.2} {unit}\u{b3}",
                item.fill_m3 - item.cut_m3
            ));
            ui.label(
                egui::RichText::new("Selected item alone, clipped to this footprint")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.separator();
        }
    }
    ui.label(egui::RichText::new("Combined terrain change").strong());
    ui.label(format!("Area: {:.2} {unit}\u{b2}", result.area_m2));
    ui.label(format!("Cut: {:.2} {unit}\u{b3}", result.cut_m3));
    ui.label(format!("Fill: {:.2} {unit}\u{b3}", result.fill_m3));
    ui.label(format!("Net: {:.2} {unit}\u{b3}", result.net_m3));
    ui.label(format!(
        "Terrain samples: {}/{}",
        result.sampled_pairs, result.expected_samples
    ));
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
    render_object_impacts(ui, &result.object_impacts, unit, open_node);

    if !result.items.is_empty() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("Items ({})", result.items.len()))
                .strong()
                .color(theme::TEXT_SECTION),
        );
        for item in result.items.iter().take(128) {
            let overlap = if item.overlap_area_m2 > 0.0 {
                format!(", overlap {:.2} {unit}\u{b2}", item.overlap_area_m2)
            } else {
                String::new()
            };
            ui.add(
                egui::Label::new(format!(
                    "{} ({}; {}): cut {:.2} {unit}\u{b3}, fill {:.2} {unit}\u{b3}{overlap}",
                    short_text(&item.id),
                    item.operation,
                    short_text(item.material.as_deref().unwrap_or("Not specified")),
                    item.cut_m3,
                    item.fill_m3
                ))
                .wrap(),
            );
        }
        if result.items.len() > 128 {
            ui.label("First 128 terrain contributions shown; full breakdown is available in TSV.");
        }
    }

    ui.add_space(6.0);
    if ui
        .button("Copy as TSV")
        .named(ui, "Copy volume report as TSV")
        .clicked()
    {
        let nodes = active_petal
            .and_then(|id| hierarchy.find_petal(id))
            .map(|petal| nodes_in_footprint(petal, &state.footprint))
            .unwrap_or_default();
        let tsv = volume_report_to_tsv(result, &nodes);
        ui.ctx().copy_text(tsv);
        let now = ui.ctx().input(|i| i.time);
        ui_mgr.show_toast("Volume report copied as TSV", now);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)] // default-then-set is clearer in test fixtures
    use super::*;

    fn sample_result(correlation_id: &str) -> VolumeReportResult {
        VolumeReportResult {
            correlation_id: correlation_id.to_string(),
            petal_id: "p1".into(),
            expected_samples: 100,
            sampled_pairs: 100,
            area_m2: 100.0,
            cut_m3: 12.5,
            fill_m3: 0.0,
            net_m3: -12.5,
            step_m: 1.0,
            items: Vec::new(),
            has_overlap: false,
            scaled: true,
            object_impacts: Default::default(),
            unavailable: None,
        }
    }

    // --- S6 flow test 1: the R-4 regression, driven through the real seam ---

    #[derive(Resource, Default)]
    struct CapturedRequests(Vec<VolumeReportRequest>);

    fn capture_requests(
        mut reader: MessageReader<VolumeReportRequest>,
        mut sink: ResMut<CapturedRequests>,
    ) {
        sink.0.extend(reader.read().cloned());
    }

    /// The fe-ui half of the Volume report round trip: selection → request,
    /// result → state (fe-terrain's answerer is replaced by the test).
    fn volume_app() -> App {
        let mut app = App::new();
        app.init_resource::<NavigationManager>()
            .init_resource::<ProposalEditState>()
            .init_resource::<PetalMapState>()
            .init_resource::<SculptToolState>()
            .init_resource::<ToolState>()
            .init_resource::<VolumeReportState>()
            .init_resource::<CapturedRequests>()
            .add_message::<DbResult>()
            .add_message::<VolumeReportRequest>()
            .add_message::<VolumeReportResult>()
            .add_systems(
                Update,
                (
                    sync_volume_report_request,
                    capture_requests,
                    drain_volume_report_result,
                )
                    .chain(),
            );
        app
    }

    fn shown_cut(app: &App) -> Option<f64> {
        match resolve_volume_display(app.world().resource::<VolumeReportState>()) {
            VolumeDisplay::Ready { result, .. } => Some(result.cut_m3),
            _ => None,
        }
    }

    #[test]
    fn selected_region_report_shows_true_cut_fill_not_area_times_delta() {
        let terrain = serde_json::json!({"proposals": [{
            "id": "r1", "op": "raise", "material": "earth", "delta": 2.0,
            "footprint": [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
        }, {
            "id": "p2", "op": "raise", "delta": 2.0,
            "footprint": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
        }]});
        let mut app = volume_app();
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .active_petal_id = Some("p1".into());
        app.update(); // petal-entry baseline frame
        let nav = app.world().resource::<NavigationManager>();
        let mut map = PetalMapState {
            petal_id: Some("p1".into()),
            loaded: true,
            terrain_json: Some(terrain.clone()),
            ..Default::default()
        };
        let mut proposals = ProposalEditState::default();
        proposals.replace_all(vec![crate::terrain_proposal_state::ProposalRecord {
            id: "p2".into(),
            op: crate::terrain_proposal_state::ProposalOp::Raise,
            footprint: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            target_height: None,
            delta: Some(2.0),
        }]);
        let records_before = proposals.proposals.clone();
        let mut sidebar = crate::ui_shell::right_sidebar::RightSidebarState::default();
        sidebar.collapsed = true;
        assert!(!open_terrain_report(
            "other",
            "r1",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            true
        ));
        assert!(!open_terrain_report(
            "p1",
            "missing",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            true
        ));
        assert!(!open_terrain_report(
            "p1",
            "r1",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            false
        ));
        map.petal_id = Some("other".into());
        assert!(!open_terrain_report(
            "p1",
            "r1",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            true
        ));
        map.petal_id = Some("p1".into());
        map.loaded = false;
        assert!(!open_terrain_report(
            "p1",
            "r1",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            true
        ));
        map.loaded = true;
        assert!(proposals.selected.is_none() && sidebar.collapsed);
        assert!(open_terrain_report(
            "p1",
            "p2",
            nav,
            &map,
            &mut proposals,
            &mut sidebar,
            true
        ));
        let mut other_nav = NavigationManager::default();
        other_nav.navigate_to_petal("petal-B");
        let mut other_map = map.clone();
        other_map.petal_id = Some("petal-B".into());
        assert!(
            !open_terrain_report(
                "p1",
                "r1",
                &other_nav,
                &other_map,
                &mut proposals,
                &mut sidebar,
                true,
            ),
            "a queued old-Petal action cannot open a same-ID target in the destination"
        );
        assert_eq!(proposals.selected.as_deref(), Some("p2"));
        for _ in 0..2 {
            assert!(open_terrain_report(
                "p1",
                "r1",
                nav,
                &map,
                &mut proposals,
                &mut sidebar,
                true
            ));
            assert_eq!(proposals.selected.as_deref(), Some("r1"));
            assert!(sidebar
                .is_active(crate::ui_shell::right_sidebar::RightSidebarSection::ProposalReport));
            assert!(!sidebar.collapsed);
        }
        assert_eq!(
            proposals.proposals, records_before,
            "report selection never alters terrain records"
        );
        assert_eq!(map.terrain_json, Some(terrain.clone()));
        app.insert_resource(map);
        app.insert_resource(proposals);
        app.update();

        let request = app.world().resource::<CapturedRequests>().0[0].clone();
        assert_eq!(request.petal_id, "p1");
        assert_eq!(
            request.footprint.len(),
            4,
            "the selected region's own footprint"
        );
        assert_eq!(
            shown_cut(&app),
            None,
            "pending until the true figure arrives — no estimate"
        );

        // The integrator's answer is shown verbatim (12.5), never area×|delta|
        // (100 × 2 = 200); a reply for any other correlation is ignored.
        let mut answer = sample_result(&request.correlation_id);
        app.world_mut().write_message(answer.clone());
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5));
        let mut wrong_petal = answer.clone();
        wrong_petal.petal_id = "other".into();
        wrong_petal.cut_m3 = 777.0;
        app.world_mut().write_message(wrong_petal);
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5));
        let mut incomplete = answer.clone();
        incomplete.sampled_pairs = 0;
        app.world_mut().write_message(incomplete.clone());
        app.update();
        assert_eq!(shown_cut(&app), None);
        assert!(matches!(
            resolve_volume_display(app.world().resource::<VolumeReportState>()),
            VolumeDisplay::Incomplete { .. }
        ));
        assert!(volume_report_to_tsv(&incomplete, &[]).is_empty());
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for metric in 0..8 {
                let mut invalid = answer.clone();
                invalid
                    .items
                    .push(fe_renderer::terrain_overlay::VolumeItem {
                        id: "overflow".into(),
                        kind: "raise".into(),
                        operation: "raise".into(),
                        material: Some("soil".into()),
                        cut_m3: 0.0,
                        fill_m3: 0.0,
                        overlap_area_m2: 0.0,
                    });
                match metric {
                    0 => invalid.area_m2 = value,
                    1 => invalid.cut_m3 = value,
                    2 => invalid.fill_m3 = value,
                    3 => invalid.net_m3 = value,
                    4 => invalid.step_m = value as f32,
                    5 => invalid.items[0].cut_m3 = value,
                    6 => invalid.items[0].fill_m3 = value,
                    7 => invalid.items[0].overlap_area_m2 = value,
                    _ => unreachable!(),
                }
                assert!(!invalid.is_complete());
                assert!(volume_report_to_tsv(&invalid, &[]).is_empty());
                app.world_mut().write_message(invalid);
                app.update();
                assert_eq!(
                    shown_cut(&app),
                    None,
                    "nonfinite metric {metric} must not be Ready"
                );
            }
        }
        app.world_mut().write_message(answer.clone());
        app.update();
        assert_eq!(
            shown_cut(&app),
            Some(12.5),
            "same-correlated residency update replaces incomplete result"
        );
        answer.correlation_id = "stale".into();
        answer.cut_m3 = 999.0;
        app.world_mut().write_message(answer);
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5), "stale reply must not surface");

        // A confirmed terrain reload (a commit elsewhere) re-asks for the
        // SAME selection: the old figure is withdrawn, never left stale.
        app.world_mut().write_message(DbResult::PetalTerrainLoaded {
            petal_id: "p1".into(),
            terrain: Some(terrain),
            correlation_id: None,
        });
        app.update();
        let requests = &app.world().resource::<CapturedRequests>().0;
        assert_eq!(
            requests.len(),
            2,
            "terrain reload re-requests the unchanged selection"
        );
        assert_ne!(requests[1].correlation_id, request.correlation_id);
        assert_eq!(
            shown_cut(&app),
            None,
            "previous figure withdrawn until re-answered"
        );
        let latest_id = requests[1].correlation_id.clone();
        app.world_mut()
            .write_message(sample_result(&request.correlation_id));
        app.update();
        assert_eq!(
            shown_cut(&app),
            None,
            "old config reply cannot claim the new request"
        );
        app.world_mut().write_message(sample_result(&latest_id));
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5));
        app.world_mut().resource_mut::<PetalMapState>().terrain_json = None;
        app.world_mut().write_message(sample_result(&latest_id));
        app.update();
        assert!(
            matches!(
                resolve_volume_display(app.world().resource::<VolumeReportState>()),
                VolumeDisplay::Unselected
            ),
            "unbinding withdraws the report and rejects late replies"
        );
        app.insert_resource(fe_renderer::scene_analysis::SceneAnalysisSnapshot {
            petal_id: Some("p1".into()),
            revision: 1,
            ..Default::default()
        });
        app.world_mut().resource_mut::<PetalMapState>().terrain_json =
            Some(serde_json::json!({"proposals":[{
                "id":"r1", "op":"raise", "material":"clay", "delta":2.0,
                "footprint":[[0,0],[10,0],[10,10],[0,10]]
            }]}));
        app.update();
        let first_scene_id = app
            .world()
            .resource::<CapturedRequests>()
            .0
            .last()
            .unwrap()
            .correlation_id
            .clone();
        let mut scene_answer = sample_result(&first_scene_id);
        scene_answer.object_impacts.scene_revision = 1;
        app.world_mut().write_message(scene_answer);
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5));
        app.world_mut()
            .resource_mut::<fe_renderer::scene_analysis::SceneAnalysisSnapshot>()
            .revision = 2;
        app.update();
        let moved_scene_id = app
            .world()
            .resource::<CapturedRequests>()
            .0
            .last()
            .unwrap()
            .correlation_id
            .clone();
        assert_ne!(
            first_scene_id, moved_scene_id,
            "object change invalidates an unchanged footprint"
        );
        assert_eq!(shown_cut(&app), None);
        let mut stale_scene = sample_result(&moved_scene_id);
        stale_scene.object_impacts.scene_revision = 1;
        app.world_mut().write_message(stale_scene);
        app.update();
        assert_eq!(
            shown_cut(&app),
            None,
            "matching request cannot adopt old geometry"
        );
        let mut current_scene = sample_result(&moved_scene_id);
        current_scene.object_impacts.scene_revision = 2;
        app.world_mut().write_message(current_scene);
        app.update();
        assert_eq!(shown_cut(&app), Some(12.5));
    }

    #[test]
    fn no_result_yet_is_pending_not_a_silent_approximation() {
        let state = VolumeReportState {
            display_label: Some("Selected proposal p1".into()),
            correlation_id: Some("c1".into()),
            ..Default::default()
        };
        // No result has arrived for c1 yet.
        match resolve_volume_display(&state) {
            VolumeDisplay::Pending { label } => assert_eq!(label, "Selected proposal p1"),
            other => panic!("expected Pending, got {other:?}"),
        }

        // fe-terrain's explicit "cannot measure" reply replaces Pending, never Ready.
        let mut unavailable = sample_result("c1");
        unavailable.unavailable = Some(VolumeUnavailable::NoMap);
        let state = VolumeReportState {
            result: Some(unavailable),
            ..state
        };
        match resolve_volume_display(&state) {
            VolumeDisplay::Unavailable { label, reason } => {
                assert_eq!(label, "Selected proposal p1");
                assert_eq!(reason, VolumeUnavailable::NoMap);
            }
            other => panic!("expected Unavailable, got {other:?}"),
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
                transform: Default::default(),
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
                transform: Default::default(),
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
        // The copied table is what leaves the app (BI egress): an unscaled
        // result must say world units on every row, never meters.
        let mut result = sample_result("c1");
        result.scaled = false;
        let tsv = volume_report_to_tsv(&result, &[]);
        assert!(tsv.contains("Cut\t12.500\twu\u{b3}"), "{tsv}");
        assert!(tsv.contains("Area\t100.000\twu\u{b2}"), "{tsv}");
        assert!(
            !tsv.contains("\tm"),
            "unscaled rows must not claim meters:\n{tsv}"
        );
        let scaled = volume_report_to_tsv(&sample_result("c1"), &[]);
        assert!(scaled.contains("Cut\t12.500\tm\u{b3}"), "{scaled}");
    }

    #[test]
    fn drawn_shape_selection_is_the_world_footprint_apply_commits() {
        // `SculptToolState` is meters; fe-terrain integrates (and nodes sit)
        // in world units. On a 2 wu/m petal the selection must be 2× larger.
        let sculpt = SculptToolState {
            shape_mode: SculptShapeMode::Rect,
            radius: 3.0,
            ..Default::default()
        };
        let resolved =
            resolve_volume_selection(&ProposalEditState::default(), None, &sculpt, true, 2.0)
                .expect("a Rect draft resolves");
        let expected: Vec<[f32; 2]> = crate::panels::tool_options::shape_apply_footprint(&sculpt)
            .into_iter()
            .map(|[x, z]| [x * 2.0, z * 2.0])
            .collect();
        assert_eq!(resolved.footprint(), expected.as_slice());
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
                operation: "raise".into(),
                material: Some("sand".into()),
                cut_m3: 1.0,
                fill_m3: 2.0,
                overlap_area_m2: 0.0,
            },
            VolumeItem {
                id: "r2".into(),
                kind: "earthwork_region".into(),
                operation: "lower".into(),
                material: Some("clay".into()),
                cut_m3: 3.0,
                fill_m3: 4.0,
                overlap_area_m2: 5.0,
            },
        ];
        let nodes = vec![
            NodeHit {
                id: "n1".into(),
                name: "Rock".into(),
                kind: "asset",
            },
            NodeHit {
                id: "hostile".into(),
                name: "Rock\tA\r\nB".into(),
                kind: "asset",
            },
        ];
        result.object_impacts.available = true;
        result
            .object_impacts
            .objects
            .push(fe_renderer::scene_analysis::ObjectImpact {
                object: fe_renderer::scene_analysis::SceneObjectBounds {
                    node_id: "structure-1".into(),
                    name: "Footing\tA\r\nB".into(),
                    bounds: Some([[-1.0, 0.5, -1.0], [1.0, 1.5, 1.0]]),
                    geometry: fe_renderer::scene_analysis::ObjectGeometry::AxisAlignedSolidBox,
                    selectable: true,
                    material_json: Some("{\"name\":\"concrete\"}".into()),
                    material_loaded: true,
                    material_truncated: false,
                },
                overlap_m3: Some(4.0),
            });

        let tsv = volume_report_to_tsv(&result, &nodes);
        assert!(tsv.contains("Cut\t12.500"), "totals present:\n{tsv}");
        assert!(tsv.contains("Fill\t0.000"), "totals present:\n{tsv}");
        assert!(tsv.contains("raise\tsand") && tsv.contains("lower\tclay"));
        for item in &result.items {
            assert!(
                tsv.contains(&item.id),
                "item {} missing from TSV:\n{tsv}",
                item.id
            );
        }
        assert!(tsv.contains("Rock\tasset"), "node row missing:\n{tsv}");
        assert!(tsv.contains("Rock A  B\tasset") && !tsv.contains("Rock\tA"));
        assert!(
            tsv.contains("Footing A  B\tstructure-1\tSolid box / sampled terrain")
                && tsv.contains("4.000\tsampled overlap")
        );
        exercise_object_impact_layout(&result.object_impacts);
        result.items[0].cut_m3 = -f64::MAX;
        result.items[0].fill_m3 = f64::MAX;
        assert!(
            !result.is_complete() && volume_report_to_tsv(&result, &nodes).is_empty(),
            "derived per-item net must remain finite"
        );
    }

    fn exercise_object_impact_layout(report: &fe_renderer::scene_analysis::ObjectImpactReport) {
        let mut report = report.clone();
        let fixture = report.objects[0].clone();
        report.excluded_non_geometric = 2;
        report.objects = (0..5)
            .map(|index| {
                let mut impact = fixture.clone();
                impact.object.node_id = format!("structure-{index}");
                impact.object.name = format!("QA report overlap box {index}");
                impact
            })
            .collect();

        for (width, height) in [(1024.0, 768.0), (1280.0, 720.0), (1920.0, 1080.0)] {
            let ctx = egui::Context::default();
            let mut time = 1.0;
            let mut frame = |scroll: f32, events: Vec<egui::Event>| {
                time += 0.05;
                crate::ui_shell::layout_policy::begin_frame(&ctx, true);
                let mut selected = None;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, height),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                        egui::SidePanel::right("report_layout_journey")
                            .exact_width(320.0)
                            .show(ctx, |ui| {
                                crate::ui_shell::layout_policy::scroll(
                                    ui,
                                    "Volume report",
                                    [true, true],
                                    egui::ScrollArea::both()
                                        .id_salt("report_layout_flow")
                                        .scroll_offset(egui::vec2(0.0, scroll))
                                        .auto_shrink([false, false]),
                                    |ui| {
                                        ui.add_space(350.0);
                                        let old_id = ui.make_persistent_id("terrain_object_impacts");
                                        let mut old = egui::scroll_area::State::default();
                                        old.offset.y = 40.0;
                                        old.store(ctx, old_id);
                                        render_object_impacts(ui, &report, "m", &mut selected);
                                    },
                                );
                            });
                    },
                );
                (
                    output,
                    crate::ui_shell::layout_policy::observations(&ctx),
                    selected,
                )
            };

            for offset in [0.0, 120.0] {
                frame(offset, vec![]);
                let (output, observations, _) = frame(offset, vec![]);
                assert_eq!(observations.scrolls.len(), 1);
                let texts: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) => Some((
                            text.galley.job.text.as_str(),
                            text.visual_bounding_rect(),
                            shape.clip_rect,
                        )),
                        _ => None,
                    })
                    .collect();
                let painted = |prefix: &str| {
                    let (_, rect, clip) = texts
                        .iter()
                        .find(|(text, _, _)| text.starts_with(prefix))
                        .unwrap_or_else(|| panic!("missing {prefix} at {width}x{height}"));
                    assert!(clip.expand(0.75).contains_rect(*rect), "clipped {prefix}");
                    *rect
                };
                let name = painted("QA report overlap box 0");
                let method = painted("Solid box / sampled terrain");
                let material = painted("Material: {");
                let volume = painted("Overlap estimate: 4.000");
                assert!(name.bottom() <= method.top());
                assert!(method.bottom() <= material.top());
                assert!(material.bottom() <= volume.top());

                let button = &observations.controls[0];
                assert!(button.allowed && button.clickable);
                assert!(button.clip.contains_rect(button.rect));
                let point = button.rect.center();
                frame(offset, vec![egui::Event::PointerMoved(point)]);
                for pressed in [true, false] {
                    let (_, _, selected) = frame(
                        offset,
                        vec![egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                    if !pressed {
                        assert_eq!(selected.as_deref(), Some("structure-0"));
                    }
                }
            }
            let (_, geometry, _) = frame(0.0, vec![]);
            let scroll = &geometry.scrolls[0];
            let bottom_offset = (scroll.content_size.y - scroll.viewport.height()).max(0.0);
            frame(bottom_offset, vec![]);
            let (output, observations, _) = frame(bottom_offset, vec![]);
            assert!(output.shapes.iter().any(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.job.text == "QA report overlap box 4" =>
                {
                    shape.clip_rect.expand(0.75).contains_rect(text.visual_bounding_rect())
                }
                _ => false,
            }));
            assert!(observations.controls[4]
                .clip
                .contains_rect(observations.controls[4].rect));
        }
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

        let sculpt = SculptToolState {
            shape_mode: SculptShapeMode::Circle,
            radius: 5.0,
            ..Default::default()
        };

        let resolved = resolve_volume_selection(&proposal_state, None, &sculpt, true, 1.0)
            .expect("a selected proposal resolves");
        match resolved {
            ResolvedVolumeSelection::Proposal { id: got, .. } => assert_eq!(got, id),
            other => panic!("expected Proposal, got {other:?}"),
        }
    }

    #[test]
    fn sculpt_draft_is_the_fallback_when_nothing_is_selected() {
        let proposal_state = ProposalEditState::default();
        let sculpt = SculptToolState {
            shape_mode: SculptShapeMode::Circle,
            radius: 5.0,
            ..Default::default()
        };

        let resolved = resolve_volume_selection(&proposal_state, None, &sculpt, true, 1.0)
            .expect("a Circle draft resolves when Brush is active");
        assert!(matches!(resolved, ResolvedVolumeSelection::Drawn { .. }));

        // Brush tool not active -> no drawn fallback either.
        assert!(resolve_volume_selection(&proposal_state, None, &sculpt, false, 1.0).is_none());
    }
}
