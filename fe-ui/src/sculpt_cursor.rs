//! Sculpt brush cursor ring (T3 FR-1): immediate-mode `Gizmos` linestrip at the
//! viewport cursor while Brush is active — no entity
//! lifecycle, grounded READ-ONLY on the shared height field (NFR-1). Finding
//! #10 (`ui_semantics_unification_20260808` Phase 6): the ring additionally
//! gates on the active petal's map being loaded, matching the commit path's
//! own `petal_map_is_loading` check (`node_manager::brush_interaction`) — see
//! `fe-ui/src/AGENTS.md` §sculpt.

use bevy::prelude::*;
use fe_renderer::terrain_height::TerrainHeightField;
use fe_renderer::terrain_overlay::{brush_overlay_positions, brush_ring, BRUSH_OVERLAY_RGBA};

use crate::actions::terrain_proposal::SculptToolState;
use crate::geometry::meters_to_world;
use crate::navigation_manager::NavigationManager;
use crate::panels::toolbar::Tool;
use crate::plugin::ToolState;
use crate::plugin::ViewportCursorWorld;
use crate::terrain_map::PetalMapState;

/// Ring tessellation (matches the committed brush disc's 24 segments).
const RING_SEGMENTS: usize = 24;

fn cursor_radius_meters_to_world(radius_m: f32, world_scale: f64) -> f32 {
    meters_to_world(radius_m, world_scale)
}

/// Mirrors `node_manager::brush_interaction::petal_map_is_loading`'s
/// predicate (private there, so re-derived here rather than imported) —
/// finding #10: the ring must honor the same load-gate the commit path
/// enforces. `world_scale` resets to `1.0` on every petal switch
/// (`terrain_map::load_petal_terrain_on_nav_change`) and only settles back to
/// its real value once `petal_map.loaded` flips true for the CURRENT active
/// petal, so drawing the ring before then would render it orders of
/// magnitude off. `None` active petal (nothing to sculpt) is also "not
/// ready". Pure.
fn petal_map_ready(petal_map: &PetalMapState, active_petal_id: Option<&str>) -> bool {
    match active_petal_id {
        Some(petal_id) => petal_map.petal_id.as_deref() == Some(petal_id) && petal_map.loaded,
        None => false,
    }
}

/// Draw the brush ring at the cursor with `SculptToolState.radius`. The
/// activity gate is the first-class Brush tool. No cursor / degenerate radius
/// / the active petal's map still loading (finding #10 — honesty over
/// affordance, NFR-4: hidden rather than drawn at a stale/wrong scale) →
/// nothing drawn; a missing height field grounds the ring on the cursor plane
/// (expected pre-terrain — deliberately no per-frame warn).
pub(crate) fn draw_sculpt_brush_ring(
    tool: Res<ToolState>,
    cursor: Res<ViewportCursorWorld>,
    sculpt: Res<SculptToolState>,
    petal_map: Res<PetalMapState>,
    nav: Res<NavigationManager>,
    height_field: Option<Res<TerrainHeightField>>,
    mut gizmos: Gizmos,
) {
    if tool.active_tool != Tool::Brush {
        return;
    }
    let Some([cx, cy, cz]) = cursor.pos else {
        return;
    };
    if !petal_map_ready(&petal_map, nav.active_petal_id.as_deref()) {
        return;
    }
    let radius = cursor_radius_meters_to_world(sculpt.sanitized_radius(), petal_map.world_scale);
    debug_assert!(radius.is_finite() && radius > 0.0);
    let positions: Vec<[f32; 3]> = match height_field.as_deref() {
        Some(field) => brush_overlay_positions(field, [cx, cz], radius, RING_SEGMENTS, cy),
        None => brush_ring([cx, cz], radius, RING_SEGMENTS)
            .into_iter()
            .map(|[x, z]| [x, cy, z])
            .collect(),
    };
    if positions.is_empty() {
        return;
    }
    let mut points: Vec<Vec3> = positions.into_iter().map(Vec3::from).collect();
    points.push(points[0]); // close the loop
    let [r, g, b, a] = BRUSH_OVERLAY_RGBA;
    gizmos.linestrip(points, Color::srgba(r, g, b, a));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_radius_stays_positive_at_tiny_valid_scale() {
        let radius = cursor_radius_meters_to_world(0.1, f64::MIN_POSITIVE);
        assert!(radius.is_finite() && radius > 0.0);
    }

    #[test]
    fn petal_map_ready_requires_the_active_petals_map_to_be_loaded() {
        let mut map = PetalMapState {
            petal_id: Some("p".into()),
            loaded: false,
            ..Default::default()
        };
        assert!(
            !petal_map_ready(&map, Some("p")),
            "still loading — must not be ready"
        );
        map.loaded = true;
        assert!(petal_map_ready(&map, Some("p")));
        assert!(
            !petal_map_ready(&map, Some("other")),
            "stale map from a different petal — must not be ready"
        );
        assert!(!petal_map_ready(&map, None), "no active petal at all");
    }

    #[test]
    fn petal_map_ready_no_active_petal_is_never_ready_even_if_loaded_is_stale_true() {
        let map = PetalMapState {
            petal_id: None,
            loaded: true,
            ..Default::default()
        };
        assert!(!petal_map_ready(&map, None));
    }
}
