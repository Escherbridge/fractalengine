//! Scale-bar HUD overlay (FR-8); see `src/AGENTS.md` §ruler_plugin.

use bevy::prelude::Projection as CameraProjection;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

use crate::petal_binding::ActivePetalTerrain;
use crate::ruler::{format_distance_m, meters_per_pixel, scale_bar_spec};
use crate::scale::world_to_real_height;

/// Target on-screen bar width (px) before nice-number snapping.
const TARGET_BAR_PX: f64 = 120.0;
/// Distance from the viewport's bottom-left corner (px).
const BAR_MARGIN: f32 = 16.0;
/// End-tick height (px).
const TICK_H: f32 = 6.0;
/// Fallback vertical FOV when the camera is not perspective (Bevy default).
const DEFAULT_FOV_Y: f64 = std::f64::consts::FRAC_PI_4;

/// Ruler-layer HUD toggles; fe-ui flips these via the terrain API surface,
/// never a direct fe-terrain dependency (C6).
#[derive(Resource, Clone)]
pub struct RulerSettings {
    /// Show the bottom-left scale bar (default on).
    pub show_scale_bar: bool,
}

impl Default for RulerSettings {
    fn default() -> Self {
        Self { show_scale_bar: true }
    }
}

/// Registers the scale-bar HUD; added by [`crate::terrain_plugin::TerrainPlugin`].
pub struct RulerPlugin;

impl Plugin for RulerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RulerSettings>()
            .add_systems(EguiPrimaryContextPass, draw_scale_bar);
    }
}

/// Thin egui pass: all bar-length/label math lives in `ruler.rs` (testable).
fn draw_scale_bar(
    mut ctx: EguiContexts,
    settings: Res<RulerSettings>,
    active: Res<ActivePetalTerrain>,
    cameras: Query<(&GlobalTransform, &CameraProjection), With<Camera>>,
) {
    if !settings.show_scale_bar {
        return;
    }
    let Some(config) = active.config.as_ref().filter(|c| c.enabled) else {
        return;
    };
    let Ok((cam_tx, cam_proj)) = cameras.single() else {
        return;
    };
    let Ok(ectx) = ctx.ctx_mut() else {
        return;
    };

    let fov_y = match cam_proj {
        CameraProjection::Perspective(p) => f64::from(p.fov),
        _ => DEFAULT_FOV_Y,
    };
    let screen = ectx.content_rect();
    // Same metric frame as zoom selection: hexon-bounded effective scale.
    let scale = config.effective_world_scale();
    let cam_height_m = world_to_real_height(f64::from(cam_tx.translation().y), scale);
    let mpp = meters_per_pixel(cam_height_m, fov_y, f64::from(screen.height()));
    let Some(spec) = scale_bar_spec(mpp, TARGET_BAR_PX) else {
        return;
    };

    let painter = ectx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ruler_scale_bar"),
    ));
    let y = screen.max.y - BAR_MARGIN;
    let x0 = screen.min.x + BAR_MARGIN;
    let x1 = x0 + spec.pixels as f32;
    let shadow = egui::Stroke::new(3.0, egui::Color32::from_black_alpha(140));
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    // Shadow pass first so the bar stays legible over bright terrain.
    for s in [shadow, stroke] {
        painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], s);
        painter.line_segment([egui::pos2(x0, y - TICK_H), egui::pos2(x0, y)], s);
        painter.line_segment([egui::pos2(x1, y - TICK_H), egui::pos2(x1, y)], s);
    }
    painter.text(
        egui::pos2((x0 + x1) / 2.0, y - TICK_H - 2.0),
        egui::Align2::CENTER_BOTTOM,
        format_distance_m(spec.meters),
        egui::FontId::proportional(12.0),
        egui::Color32::WHITE,
    );
}
