//! Pure tile→splat synthesis (no bevy); see `src/splat/AGENTS.md` §synthesis.

/// Fraction of the decimated texel spacing a splat's minor radius covers (soft overlap).
const SPLAT_COVERAGE: f32 = 0.7;
/// Cap on along-slope elongation so near-vertical cliffs don't spawn giant quads.
const MAX_ELONGATION: f32 = 4.0;
/// Fallback vertex color when a tile carries no satellite imagery.
const DEFAULT_COLOR: [f32; 4] = [0.42, 0.45, 0.35, 1.0];

/// One synthesized splat per (decimated) elevation texel, in tile-local space.
///
/// Positions/normals are tile-local and align 1:1 with `terrain_mesh`'s vertices,
/// so the caller places a splat chunk at the same SW-corner anchor transform as
/// its mesh chunk. `scales` are `[major, minor]` radii; the major axis follows the
/// downhill slope direction (derived from the normal at bake time).
#[derive(Debug, Clone, Default)]
pub struct SplatBuffer {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub scales: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
}

impl SplatBuffer {
    /// Number of splats in the buffer.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// True when no splats were produced.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// A borrowed satellite RGBA tile for color sampling (north-up, row 0 = north).
pub struct TileSatellite<'a> {
    pub rgba: &'a [u8],
    pub width: usize,
    pub height: usize,
}

/// Synthesize splats from a decoded elevation grid + optional satellite imagery.
///
/// - `elevations`: row-major real-meter heights, `row 0 = north`, `len == width*height`.
/// - `tile_world_size`: **scaled** tile edge in world units (same value fed to `terrain_mesh`).
/// - `world_scale`: world units per real meter (heights become `ele * world_scale`).
/// - `stride`: decimation step (`1` = per-texel, `N` = every Nth texel per axis).
///
/// Position math mirrors `terrain_mesh` exactly: `x = col*cell_x`,
/// `z = (height-1-row)*cell_z` (row flip → north lands at max +z), `y = ele*scale`.
pub fn synthesize_splats(
    elevations: &[f32],
    width: usize,
    height: usize,
    satellite: Option<TileSatellite<'_>>,
    tile_world_size: f64,
    world_scale: f32,
    stride: usize,
) -> SplatBuffer {
    let mut buf = SplatBuffer::default();
    if width < 2 || height < 2 || elevations.len() < width * height {
        return buf;
    }
    let stride = stride.max(1);
    let cell_x = (tile_world_size / (width - 1) as f64) as f32;
    let cell_z = (tile_world_size / (height - 1) as f64) as f32;
    // Decimated splat spacing on the ground (average of the two cell axes).
    let spacing = stride as f32 * 0.5 * (cell_x + cell_z);

    let ele = |r: usize, c: usize| elevations[r * width + c] * world_scale;

    let mut dr = 0usize;
    while dr < height {
        let mut dc = 0usize;
        while dc < width {
            // Central differences (clamped at edges) → world-space surface gradient.
            let (cl, cr) = (dc.saturating_sub(1), (dc + 1).min(width - 1));
            let (rn, rs) = (dr.saturating_sub(1), (dr + 1).min(height - 1));
            let span_x = ((cr - cl).max(1)) as f32 * cell_x;
            let span_z = ((rs - rn).max(1)) as f32 * cell_z;
            // +x = east: slope = d(height)/dx.
            let gx = (ele(dr, cr) - ele(dr, cl)) / span_x;
            // +z = north: north row index is smaller, so height rises toward -row.
            let gz = (ele(rn, dc) - ele(rs, dc)) / span_z;

            let g = (gx * gx + gz * gz).sqrt();
            let inv_len = 1.0 / (1.0 + g * g).sqrt();
            let normal = [-gx * inv_len, inv_len, -gz * inv_len];

            let elong = (1.0 + g * g).sqrt().min(MAX_ELONGATION);
            let minor = SPLAT_COVERAGE * spacing;
            let major = minor * elong;

            buf.positions.push([
                dc as f32 * cell_x,
                ele(dr, dc),
                (height - 1 - dr) as f32 * cell_z,
            ]);
            buf.normals.push(normal);
            buf.scales.push([major, minor]);
            buf.colors
                .push(sample_color(&satellite, dr, dc, width, height));

            dc += stride;
        }
        dr += stride;
    }
    buf
}

/// Nearest-neighbor RGBA sample from the satellite tile for elevation texel `(dr, dc)`.
fn sample_color(
    sat: &Option<TileSatellite<'_>>,
    dr: usize,
    dc: usize,
    w: usize,
    h: usize,
) -> [f32; 4] {
    match sat {
        Some(s) if s.width > 0 && s.height > 0 => {
            let sx = (dc * s.width / w).min(s.width - 1);
            let sy = (dr * s.height / h).min(s.height - 1);
            let i = (sy * s.width + sx) * 4;
            if i + 3 < s.rgba.len() {
                [
                    s.rgba[i] as f32 / 255.0,
                    s.rgba[i + 1] as f32 / 255.0,
                    s.rgba[i + 2] as f32 / 255.0,
                    s.rgba[i + 3] as f32 / 255.0,
                ]
            } else {
                DEFAULT_COLOR
            }
        }
        _ => DEFAULT_COLOR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_on_degenerate_grid() {
        assert!(synthesize_splats(&[], 0, 0, None, 100.0, 1.0, 1).is_empty());
        assert!(synthesize_splats(&[1.0], 1, 1, None, 100.0, 1.0, 1).is_empty());
    }

    #[test]
    fn flat_ground_makes_upward_isotropic_splats() {
        // A perfectly flat grid: every normal points up, every splat is a circle.
        let ele = vec![5.0f32; 16];
        let buf = synthesize_splats(&ele, 4, 4, None, 300.0, 1.0, 1);
        assert_eq!(buf.len(), 16);
        for n in &buf.normals {
            assert!((n[1] - 1.0).abs() < 1e-5, "flat normal not up: {n:?}");
            assert!(n[0].abs() < 1e-5 && n[2].abs() < 1e-5);
        }
        for s in &buf.scales {
            assert!((s[0] - s[1]).abs() < 1e-5, "flat splat not isotropic: {s:?}");
        }
    }

    #[test]
    fn east_ramp_tilts_normal_and_elongates() {
        // Height increases with column (east): gradient points +x, normal tilts -x,
        // and the splat elongates along the slope (major > minor).
        let w = 4;
        let h = 4;
        let mut ele = vec![0.0f32; w * h];
        for r in 0..h {
            for c in 0..w {
                ele[r * w + c] = c as f32 * 50.0; // steep east ramp
            }
        }
        let buf = synthesize_splats(&ele, w, h, None, 100.0, 1.0, 1);
        // Interior sample (row 1, col 1) index in output = row-major of full grid.
        let idx = 1 * w + 1;
        let n = buf.normals[idx];
        assert!(n[0] < -1e-3, "normal should tilt away from +x, got {n:?}");
        assert!(n[1] > 0.0);
        let s = buf.scales[idx];
        assert!(s[0] > s[1], "steep splat should elongate: {s:?}");
    }

    #[test]
    fn position_mirrors_terrain_mesh_formula() {
        // x = col*cell_x, z = (h-1-row)*cell_z, y = ele*scale (row-flip north→+z).
        let w = 3;
        let h = 3;
        let scale = 0.5f32;
        let tile = 200.0f64;
        let mut ele = vec![0.0f32; w * h];
        ele[0 * w + 2] = 80.0; // north-east texel (row 0 = north)
        let buf = synthesize_splats(&ele, w, h, None, tile, scale, 1);
        let cell = (tile / (w - 1) as f64) as f32;
        let ne = buf.positions[0 * w + 2];
        assert!((ne[0] - 2.0 * cell).abs() < 1e-3); // col 2 → east
        assert!((ne[2] - (h - 1) as f32 * cell).abs() < 1e-3); // north row → max +z
        assert!((ne[1] - 80.0 * scale).abs() < 1e-3); // scaled height
    }

    #[test]
    fn stride_decimates_count() {
        // 4x4 grid, stride 2 → cols {0,2}, rows {0,2} → 2*2 = 4 splats.
        let ele = vec![0.0f32; 16];
        let buf = synthesize_splats(&ele, 4, 4, None, 100.0, 1.0, 2);
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn color_sampled_from_satellite() {
        // 2x2 elevation; 2x2 satellite with a distinct red north-west pixel.
        let ele = vec![0.0f32; 4];
        let mut rgba = vec![0u8; 2 * 2 * 4];
        rgba[0] = 200; // NW pixel R
        rgba[3] = 255; // NW pixel A
        let sat = TileSatellite {
            rgba: &rgba,
            width: 2,
            height: 2,
        };
        let buf = synthesize_splats(&ele, 2, 2, Some(sat), 100.0, 1.0, 1);
        // Texel (row 0, col 0) samples the NW satellite pixel.
        let nw = buf.colors[0];
        assert!((nw[0] - 200.0 / 255.0).abs() < 1e-3);
        assert!((nw[3] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn missing_satellite_uses_default_color() {
        let ele = vec![0.0f32; 4];
        let buf = synthesize_splats(&ele, 2, 2, None, 100.0, 1.0, 1);
        assert_eq!(buf.colors[0], DEFAULT_COLOR);
    }
}
