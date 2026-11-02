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

// ---------------------------------------------------------------------------
// P1+P2 seam types: cut/fill heatmap + preview + volume reporting.
// Shared between fe-terrain and fe-ui via fe-renderer to avoid fe-ui→fe-terrain
// dependency (OD-10, NFR-5).
// ---------------------------------------------------------------------------

/// Cut/fill heatmap color (red=cut, blue=fill) graded by depth within a neutral
/// transparent band (OD-1, OD-2). Takes delta height in real meters, ramp max
/// depth (full saturation point), and neutral band half-width. Returns RGBA
/// with red dominant for cut (dh < 0), blue for fill (dh > 0), transparent
/// inside ±neutral_m, and saturated beyond ±ramp_max_m.
pub fn cut_fill_rgba(dh_m: f32, ramp_max_m: f32, neutral_m: f32) -> [f32; 4] {
    // Degenerate inputs never yield out-of-range channels: a non-finite dh is
    // undrawable (transparent); a neutral band at/above the ramp saturates.
    let ramp_max = if ramp_max_m.is_finite() { ramp_max_m.max(0.001) } else { 0.001 };
    let neutral = if neutral_m.is_finite() { neutral_m.max(0.0) } else { 0.0 };
    if !dh_m.is_finite() || dh_m.abs() <= neutral {
        return [0.0, 0.0, 0.0, 0.0];
    }

    // Grade from the neutral band edge to the ramp max, clamped to [0, 1].
    let span = ramp_max - neutral;
    let intensity = if span > 0.0 {
        ((dh_m.abs() - neutral) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };

    if dh_m < 0.0 {
        // Cut: red family — green and blue drop with intensity.
        [1.0, 1.0 - intensity, 1.0 - intensity, 0.8]
    } else {
        // Fill: blue family — red and green drop with intensity.
        [1.0 - intensity, 1.0 - intensity, 1.0, 0.8]
    }
}

/// Unlit, blended heatmap material for cut/fill overlay drape. Vertex colors
/// (set by the lattice builder) control the per-vertex heatmap tone.
pub fn heatmap_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        cull_mode: None,
        ..default()
    }
}

/// Preview state for the armed terrain tool before commit (P1 live preview).
/// The preview system reads this Resource and renders the cut/fill heatmap +
/// ghost surface without writing the DB or terrain field (NFR-5, FR-3).
/// Units: `footprint` is WORLD units; `target_m`/`delta_m` are real meters.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct ProposalPreview {
    /// Petal scope (authoritative for visibility; wrong-petal previews are dropped).
    pub petal_id: String,
    /// Palette operation name (e.g., "raise", "cut", "smooth").
    pub op: String,
    /// Footprint of the tool draft (XZ boundary).
    pub footprint: Vec<[f32; 2]>,
    /// Target height (for palette ops) or region mean (for sculpt smooth).
    pub target_m: f32,
    /// Delta height magnitude (depth of cut/fill).
    pub delta_m: f32,
    /// Tool strength / blend factor (0.0..=1.0).
    pub strength: f32,
    /// Identifies the source: palette op vs shape vs brush stroke (for coherence checks).
    pub source: PreviewSource,
}

/// Discriminator for preview source (palette op, sculpt shape, brush stroke).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewSource {
    /// Armed palette op at a location (radius, delta from toolbars).
    PaletteOp,
    /// Sculpt shape draft (circle, rect, polygon, elevation).
    SculptShape,
    /// In-progress brush stroke (corridor footprint from samples).
    BrushStroke,
    /// The selected COMMITTED palette proposal: readout only — its committed
    /// ghost/drape already render, so the preview spawns no duplicate mesh.
    SelectedProposal,
}

/// Live preview report: cut/fill/net volumes from the armed tool before commit.
/// Sent by the render preview system (fe-terrain/src/terrain_plugin.rs) after
/// computing the heatmap lattice, consumed by fe-ui to show the live readout.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct ProposalPreviewReport {
    pub petal_id: String,
    pub cut_m3: f64,
    pub fill_m3: f64,
    pub net_m3: f64,
    /// `false` = no usable map scale: the figures are world units³ (NFR-4).
    pub scaled: bool,
}

/// Request for a volume report on a selected footprint (P2 user selection).
/// Sent by fe-ui panels (selection, region, drawn footprint), answered by
/// fe-terrain's report system with a [`VolumeReportResult`].
#[derive(Message, Debug, Clone)]
pub struct VolumeReportRequest {
    /// Correlation ID to match async request/reply (UUID/ULID).
    pub correlation_id: String,
    /// Petal scope for the report.
    pub petal_id: String,
    /// Footprint of the selection (XZ boundary).
    pub footprint: Vec<[f32; 2]>,
    /// Reserved for P4: optional vertical extent (min, max elevation in meters).
    /// Ignored today (2.5D footprint prism used).
    pub elevation_band: Option<[f32; 2]>,
}

/// Selection volume report result: totals, per-item breakdown, and scaled units.
/// Sent by fe-terrain's answer system, consumed by fe-ui panels to render the
/// Volume report sidebar section (P2, FR-7/8).
#[derive(Message, Debug, Clone)]
pub struct VolumeReportResult {
    /// Correlation ID matching the originating [`VolumeReportRequest`].
    pub correlation_id: String,
    /// Scope of the sampled surface.
    pub petal_id: String,
    /// Required cell centers on the selected integration lattice.
    pub expected_samples: u64,
    /// Cell centers with finite base and proposed samples.
    pub sampled_pairs: u64,
    /// Total area of the footprint in m² (or world units if unscaled, NFR-4).
    pub area_m2: f64,
    /// Composite cut volume in m³ (or world units cubed if unscaled).
    pub cut_m3: f64,
    /// Composite fill volume in m³ (or world units cubed if unscaled).
    pub fill_m3: f64,
    /// Net volume (fill - cut) in m³.
    pub net_m3: f64,
    /// Lattice step used in the integration (reported for tolerance/honesty, NFR-4).
    pub step_m: f32,
    /// Per-item breakdown: each proposal/region overlapping the selection.
    pub items: Vec<VolumeItem>,
    /// True if items' standalone figures do not sum to totals (overlap detected).
    pub has_overlap: bool,
    /// Scaled indicator: true if world_scale is finite and > 0.
    pub scaled: bool,
}

impl VolumeReportResult {
    /// Whether every exposed measurement is finite.
    pub fn has_finite_measurements(&self) -> bool {
        [self.area_m2, self.cut_m3, self.fill_m3, self.net_m3]
            .into_iter()
            .all(f64::is_finite)
            && self.step_m.is_finite()
            && self.items.iter().all(|item| {
                [item.cut_m3, item.fill_m3, item.overlap_area_m2]
                    .into_iter()
                    .all(f64::is_finite)
            })
    }

    /// Whether every selected cell is measured and every result is finite.
    pub fn is_complete(&self) -> bool {
        self.expected_samples > 0
            && self.sampled_pairs == self.expected_samples
            && self.has_finite_measurements()
    }
}

/// One proposal or region in a volume report breakdown.
#[derive(Debug, Clone)]
pub struct VolumeItem {
    /// Identifier (proposal or region node ID).
    pub id: String,
    /// Operator/type name (e.g., "raise", "earthwork_region").
    pub kind: String,
    /// Standalone cut m³ clipped to the selection footprint.
    pub cut_m3: f64,
    /// Standalone fill m³ clipped to the selection footprint.
    pub fill_m3: f64,
    /// Overlap area with other items in m² (nonzero if composite ≠ sum).
    pub overlap_area_m2: f64,
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

    // -----------------------------------------------------------------------
    // P1 heatmap tests: cut/fill ramp (OD-1) and material properties (FR-2).
    // -----------------------------------------------------------------------

    #[test]
    fn cut_fill_ramp_table() {
        // Test the full color ramp: transparent neutral band, red cut grading,
        // blue fill grading, and saturation at depth bounds (OD-1, OD-2).
        // Mutation check: inverting the sign check would make this test go red.
        let ramp_max = 5.0;
        let neutral = 0.5;

        // Inside the neutral band: fully transparent.
        let [r, g, b, a] = cut_fill_rgba(0.0, ramp_max, neutral);
        assert_eq!(a, 0.0, "dh=0 must be transparent");
        assert_eq!([r, g, b], [0.0, 0.0, 0.0], "neutral band is black");

        let [_, _, _, a] = cut_fill_rgba(0.3, ramp_max, neutral);
        assert_eq!(a, 0.0, "dh<neutral must be transparent");

        let [_, _, _, a] = cut_fill_rgba(-0.4, ramp_max, neutral);
        assert_eq!(a, 0.0, "dh<neutral must be transparent");

        // Just outside neutral: faint color.
        let [r_out, g_out, b_out, a_out] = cut_fill_rgba(0.6, ramp_max, neutral);
        assert!(a_out > 0.0, "dh>neutral must be opaque");
        assert!(
            b_out > r_out && b_out > g_out,
            "fill (dh>0) must have blue dominant"
        );
        assert!(
            b_out >= 0.9,
            "fill blue channel must be high; got {b_out}"
        );

        let [r_out, g_out, b_out, a_out] = cut_fill_rgba(-0.6, ramp_max, neutral);
        assert!(a_out > 0.0, "dh<neutral must be opaque");
        assert!(
            r_out > b_out && r_out > g_out,
            "cut (dh<0) must have red dominant"
        );
        assert!(r_out >= 0.9, "cut red channel must be high; got {r_out}");

        // Half the ramp depth: medium intensity.
        let half_depth = (ramp_max + neutral) / 2.0;
        let [r_half, _, b_half, a_half] = cut_fill_rgba(half_depth, ramp_max, neutral);
        assert!(a_half > 0.0, "halfway fill must be opaque");
        assert!(b_half > r_half, "fill blue > red");
        let [r_cut, _, b_cut, a_cut] = cut_fill_rgba(-half_depth, ramp_max, neutral);
        assert!(a_cut > 0.0, "halfway cut must be opaque");
        assert!(r_cut > b_cut, "cut red > blue");

        // At the ramp max: full intensity (saturated).
        let [r_max, _, b_max, a_max] = cut_fill_rgba(ramp_max, ramp_max, neutral);
        assert!(a_max > 0.0, "ramp max must be opaque");
        assert_eq!(b_max, 1.0, "fill saturation: blue = 1.0");
        assert!(r_max < 1.0, "fill: red < 1.0");

        let [r_min, _, b_min, a_min] = cut_fill_rgba(-ramp_max, ramp_max, neutral);
        assert!(a_min > 0.0, "ramp max must be opaque");
        assert_eq!(r_min, 1.0, "cut saturation: red = 1.0");
        assert!(b_min < 1.0, "cut: blue < 1.0");

        // Beyond the ramp max: clamped to saturation (not brighter).
        let [_, _, b_beyond, a_beyond] =
            cut_fill_rgba(2.0 * ramp_max, ramp_max, neutral);
        assert!(a_beyond > 0.0, "beyond ramp must be opaque");
        assert_eq!(b_beyond, 1.0, "beyond saturates at blue = 1.0");
        assert_eq!(
            a_beyond, a_max,
            "alpha does not change beyond saturation"
        );

        // Negative neutral band (clamped to 0) should not panic.
        let [_, _, _, a_neg] = cut_fill_rgba(1.0, 5.0, -0.5);
        assert!(a_neg > 0.0, "negative neutral band is clamped; 1.0 must be opaque");

        // Negative ramp max (clamped to 0.001) should not panic.
        let [_, _, _, a_tiny] = cut_fill_rgba(0.1, -5.0, 0.05);
        assert!(a_tiny > 0.0, "negative ramp is clamped; 0.1 must be opaque");

        // Degenerate inputs never produce out-of-range channels (neutral band
        // wider than the ramp used to give 1 - (negative) > 1), and a
        // non-finite dh is undrawable rather than NaN-coloured.
        for (dh, ramp, band) in [(0.1, -5.0, 0.05), (3.0, 1.0, 2.0), (-3.0, 1.0, 2.0), (1.0, f32::NAN, 0.1)] {
            let rgba = cut_fill_rgba(dh, ramp, band);
            assert!(
                rgba.iter().all(|c| (0.0..=1.0).contains(c)),
                "cut_fill_rgba({dh}, {ramp}, {band}) = {rgba:?} left [0, 1]"
            );
        }
        assert_eq!(cut_fill_rgba(f32::NAN, 5.0, 0.5)[3], 0.0, "NaN dh is transparent");
    }

    #[test]
    fn heatmap_material_is_blended_unlit_vertex_colored() {
        // Test that the heatmap material is configured for vertex colors:
        // blended (translucent) and unlit (tint reads as intent, not lighting).
        // Mutation check: removing `alpha_mode: AlphaMode::Blend` would fail;
        // removing `unlit: true` would fail; removing `cull_mode: None` would fail.
        let mat = heatmap_material();
        assert!(
            matches!(mat.alpha_mode, AlphaMode::Blend),
            "heatmap must be blended"
        );
        assert!(mat.unlit, "heatmap must be unlit");
        assert!(
            mat.cull_mode.is_none(),
            "heatmap must not cull (double-sided for thin overlays)"
        );
        assert_eq!(
            mat.base_color,
            Color::WHITE,
            "heatmap starts white; vertex colors will tint per-vertex"
        );
    }
}
