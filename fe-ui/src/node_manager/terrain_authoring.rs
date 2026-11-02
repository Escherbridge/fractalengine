//! Scoped terrain placement state; see AGENTS.md §terrain-authoring.

use crate::actions::terrain_proposal::{petal_map_enabled, SculptShapeMode, SculptToolState};
use crate::actions::UiAction;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::LocalUserRole;
use crate::terrain_map::PetalMapState;
use crate::terrain_proposal_state::ProposalOp;

#[derive(Debug, Clone)]
pub(crate) struct TerrainScope {
    verse: String,
    fractal: String,
    petal: String,
    world_scale: f64,
    tilesets: Vec<String>,
    origin: Option<serde_json::Value>,
}

impl TerrainScope {
    pub(crate) fn capture(
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
    ) -> Option<Self> {
        let petal = nav.active_petal_id.as_deref()?;
        if !role.can_edit()
            || !map.loaded
            || !map.world_scale.is_finite()
            || map.world_scale <= 0.0
            || !petal_map_enabled(map, petal)
        {
            return None;
        }
        Some(Self {
            verse: nav.active_verse_id.clone()?,
            fractal: nav.active_fractal_id.clone()?,
            petal: petal.into(),
            world_scale: map.world_scale,
            tilesets: map.tileset_ids.clone(),
            origin: map
                .terrain_json
                .as_ref()
                .and_then(|v| v.get("origin"))
                .cloned(),
        })
    }

    pub(crate) fn matches(&self, nav: &NavigationManager, map: &PetalMapState) -> bool {
        nav.active_verse_id.as_deref() == Some(self.verse.as_str())
            && nav.active_fractal_id.as_deref() == Some(self.fractal.as_str())
            && nav.active_petal_id.as_deref() == Some(self.petal.as_str())
            && map.loaded
            && petal_map_enabled(map, &self.petal)
            && map.world_scale.to_bits() == self.world_scale.to_bits()
            && map.tileset_ids == self.tilesets
            && map.terrain_json.as_ref().and_then(|v| v.get("origin")) == self.origin.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct TerrainAuthoringCommit {
    scope: TerrainScope,
    action: UiAction,
}

impl TerrainAuthoringCommit {
    pub(crate) fn new(scope: TerrainScope, action: UiAction) -> Self {
        Self { scope, action }
    }

    pub(crate) fn into_action(
        self,
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
    ) -> Option<UiAction> {
        if !role.can_edit()
            || !self.scope.matches(nav, map)
            || !valid_action(&self.action, &self.scope.petal)
        {
            return None;
        }
        Some(self.action)
    }
}

fn finite_points(points: &[[f32; 2]], minimum: usize) -> bool {
    (minimum..=4096).contains(&points.len()) && points.iter().flatten().all(|v| v.is_finite())
}

fn valid_action(action: &UiAction, petal: &str) -> bool {
    let finite = |v: &Option<f32>| v.is_none_or(|v| v.is_finite());
    match action {
        UiAction::TerrainProposalAdd {
            footprint,
            target_height,
            delta,
            ..
        } => finite_points(footprint, 3) && finite(target_height) && finite(delta),
        UiAction::SculptShapeRegion {
            petal_id,
            footprint,
            target_height,
            delta,
            ..
        } => {
            petal_id == petal
                && finite_points(footprint, 3)
                && finite(target_height)
                && finite(delta)
        }
        UiAction::SculptBrushStroke {
            petal_id,
            centers,
            radius,
            strength,
            target_height,
            delta,
            ..
        } => {
            petal_id == petal
                && finite_points(centers, 1)
                && radius.is_finite()
                && *radius > 0.0
                && strength.is_finite()
                && finite(target_height)
                && finite(delta)
        }
        _ => false,
    }
}

pub(crate) struct ProposalPlacementPreview {
    pub op: ProposalOp,
    pub footprint_world: Vec<[f32; 2]>,
    pub target_world: f32,
    pub delta_world: f32,
}

pub(crate) struct ShapePlacementPreview {
    pub op: String,
    pub footprint_world: Vec<[f32; 2]>,
    pub target_m: Option<f32>,
    pub delta_m: Option<f32>,
}

enum PlacementKind {
    Shape {
        mode: SculptShapeMode,
        radius: f32,
        op: String,
        target: Option<f32>,
        delta: Option<f32>,
        material: String,
    },
    Proposal {
        op: ProposalOp,
        radius: f32,
        target: f32,
        delta: f32,
    },
}

struct Placement {
    scope: TerrainScope,
    tool: Tool,
    kind: PlacementKind,
}

#[derive(Default)]
pub struct TerrainAuthoringState {
    placement: Option<Placement>,
    polygon_scope: Option<TerrainScope>,
    polygon_last_click: Option<(f64, [f32; 2])>,
    close_requested: bool,
    pop_requested: bool,
    clear_requested: bool,
    hover_world: Option<[f32; 2]>,
}

impl TerrainAuthoringState {
    pub(crate) fn is_active(&self) -> bool {
        self.placement.is_some() || self.polygon_scope.is_some()
    }
    pub(crate) fn polygon_active(&self) -> bool {
        self.polygon_scope.is_some()
    }
    pub(crate) fn polygon_matches(&self, nav: &NavigationManager, map: &PetalMapState) -> bool {
        self.polygon_scope
            .as_ref()
            .is_some_and(|scope| scope.matches(nav, map))
    }
    pub(crate) fn shape_armed(&self) -> bool {
        self.placement
            .as_ref()
            .is_some_and(|p| matches!(&p.kind, PlacementKind::Shape { .. }))
    }
    pub(crate) fn request_close(&mut self) {
        self.close_requested = true;
    }
    pub(crate) fn request_pop(&mut self) {
        self.pop_requested = true;
    }
    pub(crate) fn cancel(&mut self) {
        self.placement = None;
        self.polygon_scope = None;
        self.polygon_last_click = None;
        self.close_requested = false;
        self.pop_requested = false;
        self.hover_world = None;
        self.clear_requested = true;
    }

    pub(crate) fn arm_shape(
        &mut self,
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
        sculpt: &SculptToolState,
        tool: Tool,
    ) -> bool {
        if !matches!(
            sculpt.shape_mode,
            SculptShapeMode::Circle | SculptShapeMode::Rect
        ) {
            return false;
        }
        let Some(scope) = TerrainScope::capture(nav, map, role) else {
            return false;
        };
        let (target, delta) = crate::panels::tool_options::shape_region_op_fields(sculpt);
        self.cancel();
        self.clear_requested = false;
        self.placement = Some(Placement {
            scope,
            tool,
            kind: PlacementKind::Shape {
                mode: sculpt.shape_mode,
                radius: sculpt.sanitized_radius(),
                op: sculpt.op.to_snake().into(),
                target,
                delta,
                material: crate::panels::tool_options::shape_region_material(sculpt),
            },
        });
        true
    }

    pub(crate) fn arm_proposal(
        &mut self,
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
        op: ProposalOp,
        radius: f32,
        target: f32,
        delta: f32,
        tool: Tool,
    ) -> bool {
        let Some(scope) = TerrainScope::capture(nav, map, role) else {
            return false;
        };
        if !radius.is_finite() || radius <= 0.0 || !target.is_finite() || !delta.is_finite() {
            return false;
        }
        self.cancel();
        self.clear_requested = false;
        self.placement = Some(Placement {
            scope,
            tool,
            kind: PlacementKind::Proposal {
                op,
                radius,
                target,
                delta,
            },
        });
        true
    }

    pub(crate) fn synchronize(
        &mut self,
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
        tool: Tool,
        sculpt: &mut SculptToolState,
    ) {
        if !role.can_edit()
            || self
                .placement
                .as_ref()
                .is_some_and(|p| p.tool != tool || !p.scope.matches(nav, map))
            || self.polygon_scope.as_ref().is_some_and(|s| {
                tool != Tool::Brush
                    || sculpt.shape_mode != SculptShapeMode::Polygon
                    || !s.matches(nav, map)
            })
        {
            self.cancel();
        }
        if self.clear_requested {
            sculpt.region_draft.clear();
            self.clear_requested = false;
        }
        if self.pop_requested {
            sculpt.region_draft.pop();
            self.pop_requested = false;
        }
    }

    pub(crate) fn hover(
        &mut self,
        point: Option<[f32; 2]>,
        sculpt: &mut SculptToolState,
        map: &PetalMapState,
    ) {
        self.hover_world = point;
        if self.shape_armed() {
            if let Some([x, z]) = point {
                sculpt.shape_center_m = [
                    crate::geometry::world_to_meters(x, map.world_scale),
                    crate::geometry::world_to_meters(z, map.world_scale),
                ];
            }
        }
    }

    pub(crate) fn place(
        &mut self,
        point: [f32; 2],
        sculpt: &mut SculptToolState,
        nav: &NavigationManager,
        map: &PetalMapState,
        role: &LocalUserRole,
        tool: Tool,
        now: f64,
    ) -> Option<TerrainAuthoringCommit> {
        if !point.into_iter().all(f32::is_finite) {
            return None;
        }
        if let Some(placement) = self.placement.take() {
            if !role.can_edit() || placement.tool != tool || !placement.scope.matches(nav, map) {
                return None;
            }
            let action = match placement.kind {
                PlacementKind::Proposal {
                    op,
                    radius,
                    target,
                    delta,
                } => UiAction::TerrainProposalAdd {
                    op,
                    footprint: circle_at(point, radius),
                    target_height: Some(target),
                    delta: Some(delta),
                },
                PlacementKind::Shape {
                    mode,
                    radius,
                    op,
                    target,
                    delta,
                    material,
                } => {
                    let center = [
                        crate::geometry::world_to_meters(point[0], map.world_scale),
                        crate::geometry::world_to_meters(point[1], map.world_scale),
                    ];
                    let footprint = shape_at(mode, center, radius);
                    sculpt.shape_center_m = center;
                    UiAction::SculptShapeRegion {
                        petal_id: placement.scope.petal.clone(),
                        footprint,
                        op,
                        target_height: target,
                        delta,
                        material,
                    }
                }
            };
            return Some(TerrainAuthoringCommit::new(placement.scope, action));
        }
        if tool != Tool::Brush || sculpt.shape_mode != SculptShapeMode::Polygon {
            return None;
        }
        if self.polygon_scope.is_none() {
            self.polygon_scope = TerrainScope::capture(nav, map, role);
            sculpt.region_draft.clear();
        }
        self.polygon_scope.as_ref()?;
        let p = [
            crate::geometry::world_to_meters(point[0], map.world_scale),
            crate::geometry::world_to_meters(point[1], map.world_scale),
        ];
        if self.polygon_last_click.is_some_and(|(time, last)| {
            now - time <= 0.35 && (last[0] - p[0]).hypot(last[1] - p[1]) <= 0.2
        }) && sculpt.region_draft.len() >= 3
        {
            self.close_requested = true;
            return self.finish_polygon(sculpt);
        }
        if sculpt.region_draft.len() < 256
            && sculpt.region_draft.last().is_none_or(|last| last != &p)
        {
            sculpt.region_draft.push(p)
        }
        self.polygon_last_click = Some((now, p));
        None
    }

    pub(crate) fn finish_polygon(
        &mut self,
        sculpt: &SculptToolState,
    ) -> Option<TerrainAuthoringCommit> {
        if !self.close_requested {
            return None;
        }
        self.close_requested = false;
        if !finite_points(&sculpt.region_draft, 3)
            || crate::panels::terrain_tools_panel::sculpt_footprint_area(sculpt) <= 0.000001
        {
            return None;
        }
        let scope = self.polygon_scope.take()?;
        self.polygon_last_click = None;
        let (target, delta) = crate::panels::tool_options::shape_region_op_fields(sculpt);
        Some(TerrainAuthoringCommit::new(
            scope.clone(),
            UiAction::SculptShapeRegion {
                petal_id: scope.petal,
                footprint: sculpt.region_draft.clone(),
                op: sculpt.op.to_snake().into(),
                target_height: target,
                delta,
                material: crate::panels::tool_options::shape_region_material(sculpt),
            },
        ))
    }

    pub(crate) fn armed_proposal_op(&self) -> Option<ProposalOp> {
        match &self.placement.as_ref()?.kind {
            PlacementKind::Proposal { op, .. } => Some(*op),
            _ => None,
        }
    }

    pub(crate) fn proposal_preview(
        &self,
        nav: &NavigationManager,
        map: &PetalMapState,
        tool: Tool,
    ) -> Option<ProposalPlacementPreview> {
        let p = self.placement.as_ref()?;
        if p.tool != tool || !p.scope.matches(nav, map) {
            return None;
        }
        let PlacementKind::Proposal {
            op,
            radius,
            target,
            delta,
        } = &p.kind
        else {
            return None;
        };
        Some(ProposalPlacementPreview {
            op: *op,
            footprint_world: circle_at(self.hover_world?, *radius),
            target_world: *target,
            delta_world: *delta,
        })
    }

    pub(crate) fn shape_preview(
        &self,
        nav: &NavigationManager,
        map: &PetalMapState,
    ) -> Option<ShapePlacementPreview> {
        let placement = self.placement.as_ref()?;
        if !placement.scope.matches(nav, map) {
            return None;
        }
        let PlacementKind::Shape {
            mode,
            radius,
            op,
            target,
            delta,
            ..
        } = &placement.kind
        else {
            return None;
        };
        let radius = crate::geometry::meters_to_world(*radius, placement.scope.world_scale);
        Some(ShapePlacementPreview {
            op: op.clone(),
            footprint_world: shape_at(*mode, self.hover_world?, radius),
            target_m: *target,
            delta_m: *delta,
        })
    }
}

pub(crate) fn shape_at(mode: SculptShapeMode, center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    let points = match mode {
        SculptShapeMode::Circle => crate::panels::terrain_tools_panel::circle_footprint(radius),
        SculptShapeMode::Rect => crate::panels::terrain_tools_panel::rect_footprint(radius),
        _ => Vec::new(),
    };
    points
        .into_iter()
        .map(|[x, z]| [x + center[0], z + center[1]])
        .collect()
}

fn circle_at(center: [f32; 2], radius: f32) -> Vec<[f32; 2]> {
    shape_at(SculptShapeMode::Circle, center, radius)
}

pub(super) fn draw_terrain_draft(
    tool: bevy::prelude::Res<crate::plugin::ToolState>,
    sculpt: bevy::prelude::Res<SculptToolState>,
    nav: bevy::prelude::Res<NavigationManager>,
    map: bevy::prelude::Res<PetalMapState>,
    mut gizmos: bevy::prelude::Gizmos<crate::gimbal::GimbalGizmoGroup>,
) {
    use bevy::prelude::*;
    if tool.active_tool != Tool::Brush
        || !tool
            .terrain
            .polygon_scope
            .as_ref()
            .is_some_and(|scope| scope.matches(&nav, &map))
    {
        return;
    }
    let point = |[x, z]: [f32; 2]| {
        Vec3::new(
            crate::geometry::meters_to_world(x, map.world_scale),
            0.0,
            crate::geometry::meters_to_world(z, map.world_scale),
        )
    };
    let color = Color::srgb(0.13, 0.78, 0.37);
    let rotation = Quat::from_rotation_arc(Vec3::Z, Vec3::Y);
    let radius = crate::geometry::meters_to_world(0.12, map.world_scale).max(0.01);
    for p in &sculpt.region_draft {
        gizmos.circle(Isometry3d::new(point(*p), rotation), radius, color);
    }
    for pair in sculpt.region_draft.windows(2) {
        gizmos.line(point(pair[0]), point(pair[1]), color);
    }
    if let (Some(last), Some(hover)) = (sculpt.region_draft.last(), tool.terrain.hover_world) {
        gizmos.line(point(*last), Vec3::new(hover[0], 0.0, hover[1]), color);
    }
}
