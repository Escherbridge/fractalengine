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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_tints_are_distinct_and_translucent() {
        // A few representative ops must not collide, and all are translucent.
        let ops = ["raise", "lower", "flatten", "cut", "fill", "ramp", "slope", "pad"];
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
        assert_eq!(field.len(), len_before, "sampling must not mutate the field");
    }
}
