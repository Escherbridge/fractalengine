//! Scale-bar HUD overlay (FR-8) + opt-in adaptive metric world grid (FR-10);
//! see `src/AGENTS.md` §ruler_plugin.

use bevy::prelude::Projection as CameraProjection;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContext, EguiContexts, EguiPrimaryContextPass, PrimaryEguiContext};
use fe_renderer::camera::OrbitCameraController;

use crate::petal_binding::ActivePetalTerrain;
use crate::ruler::{
    format_distance_m, grid_fade_alpha, grid_spacing_for_height, meters_per_pixel, scale_bar_spec,
    ScaleBarSpec,
};
use crate::scale::{sanitize_world_scale, world_to_real_height};

/// Preferred visual width in egui points before nice-number snapping.
const TARGET_BAR_POINTS: f64 = 120.0;
/// Cosmetic inset in egui points, inside the divider-safe scene anchor.
const BAR_MARGIN: f32 = 16.0;
/// End-tick height in egui points.
const TICK_H: f32 = 6.0;
/// Major grid lines aimed for across the visible ground span.
const GRID_TARGET_MAJOR_LINES: f64 = 10.0;
/// Lines drawn each side of the focus, per axis, per tier (≤ 2·64 per tier).
const GRID_HALF_LINES: i32 = 32;
/// Camera heights (real m) over which the whole grid fades out.
const GRID_FADE_START_M: f64 = 50_000.0;
const GRID_FADE_END_M: f64 = 200_000.0;
/// Base line opacities before fades.
const GRID_MAJOR_ALPHA: f32 = 0.28;
const GRID_MINOR_ALPHA: f32 = 0.12;

/// Ruler-layer toggles; mirrored from fe-ui's `AppSettings` by the main
/// binary's `terrain_bridge::sync_ruler_settings`, never a direct dep (C6).
#[derive(Resource, Clone)]
pub struct RulerSettings {
    /// Show the bottom-left scale bar (default on).
    pub show_scale_bar: bool,
    /// Show the adaptive metric world grid (default off — opt-in layer).
    pub show_grid: bool,
}

impl Default for RulerSettings {
    fn default() -> Self {
        Self {
            show_scale_bar: true,
            show_grid: false,
        }
    }
}

/// Same-pass host geometry for the cosmetic HUD; see AGENTS.md §ruler-hud-input.
#[derive(Resource, Default)]
pub struct RulerHudInput {
    frame: Option<RulerHudFrame>,
}

struct RulerHudFrame {
    context: egui::Context,
    pass: u64,
    pixels_per_point: f32,
    placement: RulerHudPlacement,
}

#[derive(Clone, Copy)]
struct RulerHudPlacement {
    scene: egui::Rect,
    excluded: [egui::Rect; 4],
    count: usize,
}

impl RulerHudPlacement {
    fn footprint(self, size: egui::Vec2) -> Option<egui::Rect> {
        let inner = self.scene.shrink(BAR_MARGIN);
        if !size.is_finite() || size.x <= 0.0 || size.y <= 0.0
            || !inner.is_positive() || size.x > inner.width() || size.y > inner.height()
        { return None; }
        let mut bottom = inner.bottom();
        for _ in 0..=self.count {
            let bounds = egui::Rect::from_min_size(egui::pos2(inner.left(), bottom - size.y), size);
            if !inner.contains_rect(bounds) { return None; }
            let blocker = self.excluded[..self.count].iter()
                .filter(|rect| rect.expand(4.0).intersect(bounds).is_positive())
                .min_by(|left, right| left.top().total_cmp(&right.top()));
            match blocker {
                Some(rect) => bottom = rect.top() - 4.0,
                None => return Some(bounds),
            }
        }
        None
    }
}

impl RulerHudInput {
    pub fn clear(&mut self) {
        self.frame = None;
    }

    pub fn publish(&mut self, ctx: &egui::Context, scene: egui::Rect, excluded: &[egui::Rect]) {
        self.clear();
        let pixels_per_point = ctx.pixels_per_point();
        if pixels_per_point.is_finite() && pixels_per_point > 0.0
            && scene.is_finite() && scene.is_positive()
            && ctx.viewport_rect().is_finite() && ctx.viewport_rect().contains_rect(scene)
            && excluded.len() <= 4 && excluded.iter().all(|rect| rect.is_finite() && rect.is_positive())
        {
            let mut placement = RulerHudPlacement { scene, excluded: [egui::Rect::NOTHING; 4], count: excluded.len() };
            placement.excluded[..excluded.len()].copy_from_slice(excluded);
            self.frame = Some(RulerHudFrame {
                context: ctx.clone(), pass: ctx.cumulative_pass_nr(), pixels_per_point, placement,
            });
        }
    }

    pub fn current(&self, ctx: &egui::Context) -> Option<egui::Rect> {
        self.frame.as_ref().filter(|frame| {
            frame.context == *ctx && frame.pass == ctx.cumulative_pass_nr()
                && frame.pixels_per_point == ctx.pixels_per_point()
                && ctx.viewport_rect().is_finite() && ctx.viewport_rect().contains_rect(frame.placement.scene)
        }).map(|frame| frame.placement.scene)
    }

    /// Paint a complete bounded cosmetic footprint and withhold it from scene pointer admission.
    pub fn paint(&self, ctx: &egui::Context, scaled: Option<ScaleBarSpec>) -> Option<egui::Rect> {
        self.current(ctx)?;
        let placement = self.frame.as_ref()?.placement;
        let painted = match scaled {
            Some(spec) => paint_scale_bar(ctx, placement, spec),
            None => draw_unscaled_chip(ctx, placement),
        };
        if let Some(rect) = painted {
            fe_renderer::scene_pointer::ScenePointerChrome::exclude_completed_hud(ctx, rect.intersect(placement.scene));
        }
        painted
    }
}

/// The host publishes final shell geometry before the actual ruler producer.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RulerHudSet {
    Bridge,
    Draw,
}

/// Registers the scale-bar HUD; added by [`crate::terrain_plugin::TerrainPlugin`].
pub struct RulerPlugin;

impl Plugin for RulerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RulerSettings>()
            .init_resource::<RulerHudInput>()
            .configure_sets(EguiPrimaryContextPass, (RulerHudSet::Bridge, RulerHudSet::Draw).chain())
            .add_systems(EguiPrimaryContextPass, draw_scale_bar.in_set(RulerHudSet::Draw))
            .add_systems(Update, draw_world_grid);
    }
}

/// The orbit camera that owns the primary egui context.
type PrimaryOrbitCamera<'w, 's> = Query<'w, 's, (&'static GlobalTransform, &'static CameraProjection, &'static Camera),
    (With<OrbitCameraController>, With<PrimaryEguiContext>, With<EguiContext>)>;

/// Thin egui pass: all bar-length/label math lives in `ruler.rs` (testable).
fn draw_scale_bar(
    mut ctx: EguiContexts,
    settings: Res<RulerSettings>,
    hud: Res<RulerHudInput>,
    active: Res<ActivePetalTerrain>,
    cameras: PrimaryOrbitCamera,
) {
    if !settings.show_scale_bar {
        return;
    }
    let Ok(ectx) = ctx.ctx_mut() else {
        return;
    };
    let Some(scene) = hud.current(ectx) else { return; };
    let Some(config) = active.config.as_ref().filter(|c| c.enabled) else {
        let _ = hud.paint(ectx, None);
        return;
    };
    let Ok((cam_tx, cam_proj, camera)) = cameras.single() else {
        return;
    };
    let CameraProjection::Perspective(perspective) = cam_proj else { return; };
    // Same metric frame as zoom selection: hexon-bounded effective scale.
    let scale = config.effective_world_scale();
    let cam_height_m = world_to_real_height(f64::from(cam_tx.translation().y), scale);
    let Some(spec) = scale_bar_for_camera(camera, cam_height_m, f64::from(perspective.fov),
        ectx.pixels_per_point(), scene) else { return; };
    let _ = hud.paint(ectx, Some(spec));
}

/// Calibrate physical camera pixels, then choose a bounded visual length in points.
pub(crate) fn scale_bar_for_camera(camera: &Camera, cam_height_m: f64, fov_y: f64,
    pixels_per_point: f32, scene: egui::Rect) -> Option<ScaleBarSpec>
{
    if !camera.is_active || !pixels_per_point.is_finite() || pixels_per_point <= 0.0
        || !scene.is_finite() || !scene.is_positive()
    { return None; }
    let viewport_px = camera.physical_viewport_size().filter(|size| size.x > 0 && size.y > 0)?;
    let meters_per_physical_pixel = meters_per_pixel(cam_height_m, fov_y, f64::from(viewport_px.y));
    let pixels_per_point = f64::from(pixels_per_point);
    let room_points = f64::from(scene.width() - 2.0 * BAR_MARGIN - 3.0);
    let target_px = TARGET_BAR_POINTS.min(room_points) * pixels_per_point;
    let mut spec = scale_bar_spec(meters_per_physical_pixel, target_px)?;
    if spec.pixels > room_points * pixels_per_point {
        spec = scale_bar_spec(meters_per_physical_pixel, target_px * 0.5)?;
    }
    (spec.pixels <= room_points * pixels_per_point).then_some(spec)
}

/// Paint the actual numeric HUD only when its complete bounds fit the scene.
fn paint_scale_bar(ectx: &egui::Context, placement: RulerHudPlacement, spec: ScaleBarSpec) -> Option<egui::Rect> {
    let scene = placement.scene;
    let pixels_per_point = ectx.pixels_per_point();
    if !scene.is_finite() || !scene.is_positive()
        || !pixels_per_point.is_finite() || pixels_per_point <= 0.0
    { return None; }
    let bar_points = (spec.pixels / f64::from(pixels_per_point)) as f32;
    if !bar_points.is_finite() || bar_points <= 0.0 { return None; }
    let painter = ectx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ruler_scale_bar"),
    )).with_clip_rect(scene);
    let label = painter.layout_no_wrap(format_distance_m(spec.meters),
        egui::FontId::proportional(12.0), egui::Color32::WHITE);
    let text_bounds = label.rect.union(label.mesh_bounds);
    let size = egui::vec2((bar_points + 3.0).max(text_bounds.width()), text_bounds.height() + TICK_H + 5.0);
    let bounds = placement.footprint(size)?;
    let y = bounds.bottom() - 1.5;
    let x0 = bounds.center().x - bar_points * 0.5;
    let x1 = x0 + bar_points;
    let shadow = egui::Stroke::new(3.0_f32, egui::Color32::from_black_alpha(140));
    let stroke = egui::Stroke::new(1.5_f32, egui::Color32::WHITE);
    // Shadow pass first so the bar stays legible over bright terrain.
    for s in [shadow, stroke] {
        painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], s);
        painter.line_segment([egui::pos2(x0, y - TICK_H), egui::pos2(x0, y)], s);
        painter.line_segment([egui::pos2(x1, y - TICK_H), egui::pos2(x1, y)], s);
    }
    let text_min = egui::pos2(bounds.center().x - text_bounds.width() * 0.5,
        y - TICK_H - 2.0 - text_bounds.height());
    painter.galley(text_min - text_bounds.min.to_vec2(), label, egui::Color32::WHITE);
    Some(bounds)
}

/// Fixed unscaled-state text shares the scale bar's anchor, fit and clip path.
fn draw_unscaled_chip(ectx: &egui::Context, placement: RulerHudPlacement) -> Option<egui::Rect> {
    let scene = placement.scene;
    if !scene.is_finite() || !scene.is_positive() { return None; }
    let painter = ectx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ruler_scale_bar"),
    )).with_clip_rect(scene);
    let dim = egui::Color32::from_white_alpha(110);
    let galley = painter.layout_no_wrap(
        "world units (no map scale)".to_string(),
        egui::FontId::proportional(11.0),
        dim,
    );
    let text_bounds = galley.rect.union(galley.mesh_bounds);
    let size = text_bounds.size() + egui::Vec2::splat(8.0);
    let rect = placement.footprint(size)?;
    let pos = rect.min + egui::Vec2::splat(4.0) - text_bounds.min.to_vec2();
    painter.rect_filled(rect, 3.0, egui::Color32::from_black_alpha(120));
    painter.galley(pos, galley, dim);
    Some(rect)
}

/// Metric grid on the y = 0 plane around the orbit focus: 1/2/5×10ⁿ m spacing
/// from `grid_spacing_for_height`, converted to world units by the map scale.
/// Hidden without an enabled map (no meters to draw). Not terrain-following.
fn draw_world_grid(
    settings: Res<RulerSettings>,
    active: Res<ActivePetalTerrain>,
    cameras: Query<(&GlobalTransform, &OrbitCameraController)>,
    mut gizmos: Gizmos,
) {
    if !settings.show_grid {
        return;
    }
    let Some(config) = active.config.as_ref().filter(|c| c.enabled) else {
        return;
    };
    let Ok((cam_tx, orbit)) = cameras.single() else {
        return;
    };
    let scale = sanitize_world_scale(config.effective_world_scale());
    let cam_height_m = world_to_real_height(f64::from(cam_tx.translation().y), scale);
    let fade = grid_fade_alpha(cam_height_m, GRID_FADE_START_M, GRID_FADE_END_M);
    let Some(spacing) = grid_spacing_for_height(cam_height_m, GRID_TARGET_MAJOR_LINES) else {
        return;
    };
    if fade <= 0.0 {
        return;
    }
    let major_w = (spacing.major_m * scale) as f32;
    let minor_w = (spacing.minor_m * scale) as f32;
    let focus = Vec2::new(orbit.focus.x, orbit.focus.z);
    let major = Color::srgba(1.0, 1.0, 1.0, GRID_MAJOR_ALPHA * fade);
    draw_grid_tier(&mut gizmos, focus, major_w, None, major);
    let minor_alpha = GRID_MINOR_ALPHA * fade * spacing.minor_alpha;
    if minor_alpha > 0.0 {
        let minor = Color::srgba(1.0, 1.0, 1.0, minor_alpha);
        draw_grid_tier(&mut gizmos, focus, minor_w, Some(major_w), minor);
    }
}

/// One tier of grid lines (both axes) snapped to `step` around `focus`,
/// skipping lines that coincide with the `skip_multiple_of` tier.
fn draw_grid_tier(
    gizmos: &mut Gizmos,
    focus: Vec2,
    step: f32,
    skip_multiple_of: Option<f32>,
    color: Color,
) {
    if !(step.is_finite() && step > 0.0) {
        return;
    }
    let half_extent = step * GRID_HALF_LINES as f32;
    let origin = Vec2::new(
        (focus.x / step).round() * step,
        (focus.y / step).round() * step,
    );
    if !(origin.is_finite() && half_extent.is_finite()) {
        return;
    }
    let coincides = |value: f32| {
        skip_multiple_of.is_some_and(|coarse| {
            let nearest = (value / coarse).round() * coarse;
            (value - nearest).abs() < step * 1e-3
        })
    };
    for k in -GRID_HALF_LINES..GRID_HALF_LINES {
        let offset = k as f32 * step;
        let x = origin.x + offset;
        if !coincides(x) {
            gizmos.line(
                Vec3::new(x, 0.0, origin.y - half_extent),
                Vec3::new(x, 0.0, origin.y + half_extent),
                color,
            );
        }
        let z = origin.y + offset;
        if !coincides(z) {
            gizmos.line(
                Vec3::new(origin.x - half_extent, 0.0, z),
                Vec3::new(origin.x + half_extent, 0.0, z),
                color,
            );
        }
    }
}
