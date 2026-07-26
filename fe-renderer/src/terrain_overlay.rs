//! Ghost/tinted material for non-destructive terrain proposal overlays;
//! see `src/AGENTS.md` §terrain-overlay. Reads `TerrainHeightField` READ-ONLY.

use crate::terrain_height::TerrainHeightField;
use bevy::prelude::*;

/// Marker for spawned proposal ghost geometry (analytics overlay, not true terrain).
#[derive(Component, Debug, Clone, Copy)]
pub struct ProposalGhost;

/// Translucency of every proposal ghost so true terrain reads through it.
pub const GHOST_ALPHA: f32 = 0.35;

/// Default ghost tint (translucent cyan) for an unknown/generic proposal op.
pub const PROPOSAL_GHOST_RGBA: [f32; 4] = [0.20, 0.80, 1.00, GHOST_ALPHA];

/// Per-op tint so raise/lower/cut/fill/… read as distinct ghosted geometry
/// (FR-5). `op_snake` is the proposal's snake_case op tag (fe-terrain owns the
/// `TerrainOp` enum; this crate stays below it in the dep graph, so it takes a
/// string). Unknown tags fall back to [`PROPOSAL_GHOST_RGBA`].
pub fn op_tint(op_snake: &str) -> [f32; 4] {
    let a = GHOST_ALPHA;
    match op_snake {
        // Added material: warm/green family.
        "raise" => [0.30, 0.85, 0.40, a],
        "fill" => [0.20, 0.75, 0.55, a],
        "pad" => [0.55, 0.80, 0.30, a],
        // Removed material: red/orange family.
        "lower" => [0.90, 0.55, 0.25, a],
        "cut" => [0.90, 0.30, 0.30, a],
        // Reshaped surface: blue/violet family.
        "flatten" => [0.30, 0.70, 0.95, a],
        "ramp" => [0.55, 0.55, 0.95, a],
        "slope" => [0.75, 0.55, 0.95, a],
        _ => PROPOSAL_GHOST_RGBA,
    }
}

/// A translucent, unlit, double-sided "ghost" material for proposed geometry:
/// blended so true terrain shows through, unlit so the tint reads as intent
/// (not lighting), double-sided so a thin proposal surface is visible from below.
pub fn ghost_material(rgba: [f32; 4]) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3]),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    }
}

/// READ-ONLY base-height sample for grounding a ghost (delegates to the shared
/// field's `height_at`; takes `&TerrainHeightField` so it can never mutate the
/// true heightfield — NFR-1).
pub fn sample_base(field: &TerrainHeightField, x: f32, z: f32) -> Option<f32> {
    field.height_at(x, z)
}

// ---------------------------------------------------------------------------
// Sculpt brush overlay (sculpt_earthwork_regions_20260725 FR-1). The tactile
// area-selection cursor for the sculpt tool folds in here — NO new fe-renderer
// module (T2 owns the one `lib.rs` module line). It is a calm, translucent
// footprint ring drawn on the terrain (ui_ux §1/§7), grounded READ-ONLY on the
// shared height field so it never writes true terrain (NFR-1).
// ---------------------------------------------------------------------------

/// Brush-ring tint (translucent amber) — distinct from every proposal-ghost op
/// tint so the tactile sculpt cursor reads as a tool, not a committed edit.
pub const BRUSH_OVERLAY_RGBA: [f32; 4] = [1.00, 0.75, 0.20, GHOST_ALPHA];

/// Closed XZ ring `[x, z]` of `segments` points for a brush disc of `radius`
/// about `center` (world units). `radius <= 0` or `segments < 3` yields an empty
/// ring (nothing to draw — the caller shows no cursor).
pub fn brush_ring(center: [f32; 2], radius: f32, segments: usize) -> Vec<[f32; 2]> {
    if !(radius.is_finite() && radius > 0.0) || segments < 3 {
        return Vec::new();
    }
    (0..segments)
        .map(|i| {
            let t = (i as f32 / segments as f32) * std::f32::consts::TAU;
            [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
        })
        .collect()
}

/// Brush ring lifted to the READ-ONLY base surface for a floating cursor: each
/// ring vertex is grounded at `field.height_at` (falling back to `fallback_y`
/// where the field has no cover, so the ring still draws over gaps). Takes
/// `&TerrainHeightField` — structurally read-only (NFR-1).
pub fn brush_overlay_positions(
    field: &TerrainHeightField,
    center: [f32; 2],
    radius: f32,
    segments: usize,
    fallback_y: f32,
) -> Vec<[f32; 3]> {
    brush_ring(center, radius, segments)
        .into_iter()
        .map(|[x, z]| [x, field.height_at(x, z).unwrap_or(fallback_y), z])
        .collect()
}

/// Per-region cut/fill volume in real m³ — written by fe-terrain's earthwork
/// bake, consumed by fe-ui persistence (the fe-renderer-as-shared-seam idiom;
/// see `src/AGENTS.md` §terrain-overlay).
#[derive(Message, Debug, Clone, PartialEq)]
pub struct EarthworkVolumeReport {
    pub petal_id: String,
    pub region_id: String,
    pub cut_m3: f64,
    pub fill_m3: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_tints_are_distinct_and_translucent() {
        // A few representative ops must not collide, and all are translucent.
        let ops = [
            "raise", "lower", "flatten", "cut", "fill", "ramp", "slope", "pad",
        ];
        for op in ops {
            let t = op_tint(op);
            assert!(t[3] < 1.0, "{op} tint must be translucent");
        }
        assert_ne!(op_tint("raise"), op_tint("cut"));
        assert_ne!(op_tint("flatten"), op_tint("raise"));
    }

    #[test]
    fn op_tint_unknown_falls_back_to_default() {
        assert_eq!(op_tint("not_an_op"), PROPOSAL_GHOST_RGBA);
        assert_eq!(op_tint(""), PROPOSAL_GHOST_RGBA);
    }

    #[test]
    fn ghost_material_is_blended_and_unlit() {
        let mat = ghost_material(op_tint("raise"));
        assert!(matches!(mat.alpha_mode, AlphaMode::Blend));
        assert!(mat.unlit);
        assert!(mat.cull_mode.is_none());
        assert!(mat.base_color.alpha() < 1.0);
    }

    #[test]
    fn brush_ring_is_on_the_circle_or_empty() {
        let ring = brush_ring([10.0, -4.0], 3.0, 16);
        assert_eq!(ring.len(), 16);
        for [x, z] in &ring {
            let r = ((x - 10.0).powi(2) + (z + 4.0).powi(2)).sqrt();
            assert!((r - 3.0).abs() < 1e-4, "vertex off the ring: r={r}");
        }
        // Degenerate inputs yield no cursor (nothing to draw).
        assert!(brush_ring([0.0, 0.0], 0.0, 16).is_empty());
        assert!(brush_ring([0.0, 0.0], 3.0, 2).is_empty());
        assert!(brush_ring([0.0, 0.0], f32::NAN, 16).is_empty());
    }

    #[test]
    fn brush_overlay_positions_ground_on_field_and_fallback() {
        use crate::terrain_height::HeightTile;
        let mut field = TerrainHeightField::default();
        field.insert(
            (10, 0, 0),
            HeightTile {
                anchor: Vec3::ZERO,
                tile_size: 100.0,
                grid: vec![7.0; 4],
                width: 2,
                height: 2,
            },
        );
        let len_before = field.len();
        // A small ring well inside the flat tile grounds to y = 7.
        let inside = brush_overlay_positions(&field, [50.0, 50.0], 5.0, 8, -1.0);
        assert_eq!(inside.len(), 8);
        assert!(inside.iter().all(|p| (p[1] - 7.0).abs() < 1e-4));
        // A ring off the covered area falls back to `fallback_y`.
        let outside = brush_overlay_positions(&field, [500.0, 500.0], 5.0, 8, -2.0);
        assert!(outside.iter().all(|p| (p[1] + 2.0).abs() < 1e-4));
        assert_eq!(field.len(), len_before, "overlay must not mutate the field");
    }

    #[test]
    fn brush_tint_is_translucent_and_distinct_from_op_tints() {
        let brush_alpha = BRUSH_OVERLAY_RGBA[3];
        assert!(brush_alpha < 1.0, "brush overlay tint must be translucent");
        for op in ["raise", "lower", "flatten", "cut", "fill"] {
            assert_ne!(BRUSH_OVERLAY_RGBA, op_tint(op));
        }
    }

    #[test]
    fn sample_base_is_readonly_passthrough() {
        use crate::terrain_height::HeightTile;
        let mut field = TerrainHeightField::default();
        field.insert(
            (10, 0, 0),
            HeightTile {
                anchor: Vec3::ZERO,
                tile_size: 10.0,
                grid: vec![2.0; 4],
                width: 2,
                height: 2,
            },
        );
        let len_before = field.len();
        assert_eq!(sample_base(&field, 5.0, 5.0), Some(2.0));
        assert!(sample_base(&field, 500.0, 500.0).is_none());
        assert_eq!(
            field.len(),
            len_before,
            "sampling must not mutate the field"
        );
    }
}
