//! Sculpt brush cursor ring (T3 FR-1): immediate-mode `Gizmos` linestrip at the
//! viewport cursor while the TerrainTools section is active — no entity
//! lifecycle, grounded READ-ONLY on the shared height field (NFR-1). See
//! `fe-ui/src/AGENTS.md` §sculpt.

use bevy::prelude::*;
use fe_renderer::terrain_height::TerrainHeightField;
use fe_renderer::terrain_overlay::{brush_overlay_positions, brush_ring, BRUSH_OVERLAY_RGBA};

use crate::actions::terrain_proposal::SculptToolState;
use crate::actions::UiManager;
use crate::node_manager::NodeManager;
use crate::plugin::ViewportCursorWorld;
use crate::ui_shell::right_sidebar::{active_section, RightSidebarSection, RightSidebarState};

/// Ring tessellation (matches the committed brush disc's 24 segments).
const RING_SEGMENTS: usize = 24;

/// Draw the brush ring at the cursor with `SculptToolState.radius`. The
/// activity gate is the right sidebar's own `active_section` (the sculpt
/// controls live in TerrainTools). No cursor / degenerate radius → nothing
/// drawn; a missing height field grounds the ring on the cursor plane
/// (expected pre-terrain — deliberately no per-frame warn).
pub(crate) fn draw_sculpt_brush_ring(
    sidebar: Res<RightSidebarState>,
    ui_mgr: Res<UiManager>,
    node_mgr: Res<NodeManager>,
    cursor: Res<ViewportCursorWorld>,
    sculpt: Res<SculptToolState>,
    height_field: Option<Res<TerrainHeightField>>,
    mut gizmos: Gizmos,
) {
    let selection_present = node_mgr.selected_entity().is_some();
    if active_section(&sidebar, selection_present, ui_mgr.portal_is_open())
        != Some(RightSidebarSection::TerrainTools)
    {
        return;
    }
    let Some([cx, cy, cz]) = cursor.pos else {
        return;
    };
    let positions: Vec<[f32; 3]> = match height_field.as_deref() {
        Some(field) => brush_overlay_positions(field, [cx, cz], sculpt.radius, RING_SEGMENTS, cy),
        None => brush_ring([cx, cz], sculpt.radius, RING_SEGMENTS)
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
