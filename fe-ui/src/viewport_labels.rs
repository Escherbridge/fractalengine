//! FR-3 (data_icons_20260713): 3D floating labels drawn as an egui screen-space
//! overlay. For each point of the currently-edited track, projects the world
//! position to screen and paints a translucent `glyph + index` label near it —
//! no in-world text meshes (`bevy_text`/`Text2d` are not enabled). Gated to the
//! editing session so it doesn't clutter the whole scene. See
//! `fe-ui/src/AGENTS.md` §data-icons.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::gis::PathEditorState;
use crate::panels::path_editor_card::type_glyph;
use crate::plugin::ViewportRect;
use crate::theme;

/// Small upward screen-space offset (px) so a label floats just above its point
/// marker rather than overlapping it.
const LABEL_OFFSET_Y: f32 = -14.0;

/// Paint a `glyph + index` label above each edited-track point. Runs in the
/// egui pass so it layers over the 3-D viewport like the toast/role-chip HUD.
/// Only active while a track is being edited (`editing_track_id.is_some()`).
pub fn draw_viewport_point_labels(
    mut ctx: EguiContexts,
    path_state: Res<PathEditorState>,
    viewport_rect: Res<ViewportRect>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
) {
    if path_state.editing_track_id.is_none() || path_state.points.is_empty() {
        return;
    }
    let Ok((camera, cam_tx)) = cameras.single() else { return };
    let Ok(ectx) = ctx.ctx_mut() else { return };

    // A trackpoint carries a GPX timestamp; a Pen-authored point does not — use
    // the point glyph either way (the trackpoint discriminant covers both).
    let glyph = type_glyph("trackpoint");
    let painter = ectx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("viewport_point_labels"),
    ));

    for (i, point) in path_state.points.iter().enumerate() {
        let world = Vec3::new(point.position[0], point.position[1], point.position[2]);
        let Ok(screen) = camera.world_to_viewport(cam_tx, world) else { continue };
        let pos = egui::pos2(screen.x, screen.y + LABEL_OFFSET_Y);
        // Reject labels that project outside the 3-D viewport panel (e.g. behind
        // a side panel) — mirrors the gimbal's `ViewportRect` gating.
        if !viewport_rect.0.contains(pos) {
            continue;
        }
        painter.text(
            pos,
            egui::Align2::CENTER_BOTTOM,
            format!("{glyph} {i}"),
            egui::FontId::proportional(12.0),
            theme::TEXT_VIEWPORT_HINT,
        );
    }
}
