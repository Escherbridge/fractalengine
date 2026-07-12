//! Pure marching-squares-style splat density upscaling (no bevy); see
//! `src/splat/AGENTS.md` §interpolation. Density fallback for when the splat LOD
//! zoom pipeline hits the tileset `max_zoom` ceiling and no higher-res tile
//! exists: synthesize intermediate splats by recursively subdividing the
//! already-baked [`SplatBuffer`] until overlap or the tile seam is reached,
//! rather than holding flat density.

use crate::splat::synth::SplatBuffer;

/// Hard safety backstop on subdivision passes (FR-5). NOT the primary terminator:
/// the fill normally stops on the geometric overlap-or-seam condition below. This
/// only guards against a degenerate input that never reaches overlap.
pub const MAX_INTERPOLATION_PASSES: u8 = 5;

/// Fraction of local neighbor spacing a synthesized splat's XZ position may
/// jitter (± per axis), mirroring `synth::JITTER_FRACTION` so upscaled points
/// share the native anti-dot-grid pattern (FR-3).
const JITTER_FRACTION: f32 = 0.35;
/// Per-splat radius variation (± fraction of interpolated radius), mirroring
/// `synth::RADIUS_VARIATION_FRACTION`.
const RADIUS_VARIATION_FRACTION: f32 = 0.2;
/// Splat minor-radius / spacing ratio synth.rs bakes (`SPLAT_COVERAGE = 0.8`):
/// two splats overlap once their centers are closer than `2 * minor`, i.e. once
/// spacing drops below `2 * SPLAT_COVERAGE`. This is the overlap stop signal —
/// reused from synth's round-3 tuning, not a new invented threshold.
const SPLAT_COVERAGE: f32 = 0.8;
/// A neighbor pair is a marching-squares edge only when its XZ distance is within
/// this multiple of the current spacing — rejects long spurious links across the
/// tile that would spawn splats in empty gaps.
const EDGE_SPACING_TOLERANCE: f32 = 1.6;
/// Minimum XZ separation between any two synthesized midpoints, as a fraction of
/// the overlap radius (`2·SPLAT_COVERAGE`). This is the true degenerate-cluster
/// floor: below this a midpoint is coincident with a neighbor (moiré / z-fight),
/// above it the fill is free to densify all the way to the overlap radius. Tied
/// to the overlap distance (not per-pass or base spacing) so it neither shrinks
/// across passes nor blocks legitimate densification (FR-3).
const MIN_MIDPOINT_SEP_FRACTION: f32 = 0.4;
/// Nearest XZ neighbors considered per splat when reconstructing the implicit grid.
const NEIGHBORS_PER_SPLAT: usize = 8;

/// True when the camera-driven desired zoom exceeds what was actually fetched
/// (the tileset `max_zoom` ceiling) — the hook point where this track engages.
/// Never fires when zoom is already satisfied by real tile data (FR-1).
#[inline]
pub fn splat_needs_interpolation(requested_zoom: u8, actual_zoom: u8) -> bool {
    requested_zoom > actual_zoom
}

/// Axis-aligned XZ tile footprint used as the seam boundary (FR-3/FR-5 term.):
/// synthesized points must stay strictly inside so neighboring tiles don't
/// double-fill the shared edge. Tile-local coords (splats are baked tile-local).
#[derive(Debug, Clone, Copy)]
pub struct TileFootprint {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

impl TileFootprint {
    /// Footprint covering exactly the buffer's current XZ extent (the baked
    /// tile's own splats define its edge). Used when the caller has no explicit
    /// world-space tile rectangle to pass.
    pub fn from_buffer(buf: &SplatBuffer) -> Self {
        let mut fp = TileFootprint {
            min_x: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            min_z: f32::INFINITY,
            max_z: f32::NEG_INFINITY,
        };
        for p in &buf.positions {
            fp.min_x = fp.min_x.min(p[0]);
            fp.max_x = fp.max_x.max(p[0]);
            fp.min_z = fp.min_z.min(p[2]);
            fp.max_z = fp.max_z.max(p[2]);
        }
        fp
    }

    #[inline]
    fn contains_xz(&self, p: [f32; 3]) -> bool {
        p[0] >= self.min_x && p[0] <= self.max_x && p[2] >= self.min_z && p[2] <= self.max_z
    }
}

/// FR-4 integration entry point. Returns an augmented [`SplatBuffer`] when
/// interpolation is needed for this sub-tile, else `None` (caller keeps the
/// baked buffer untouched — zero behavior change when the ceiling isn't hit).
///
/// `requested_zoom` is the sibling track's `splat_desired_zoom`; `actual_zoom`
/// is the zoom the sub-tile was actually fetched/baked at. Seam boundary is the
/// buffer's own footprint; use [`augment_splat_buffer_within`] to pass an
/// explicit tile rectangle.
pub fn augment_splat_buffer_if_needed(
    baked: &SplatBuffer,
    requested_zoom: u8,
    actual_zoom: u8,
) -> Option<SplatBuffer> {
    if !splat_needs_interpolation(requested_zoom, actual_zoom) || baked.len() < 3 {
        return None;
    }
    Some(interpolate_density(baked, TileFootprint::from_buffer(baked)))
}

/// As [`augment_splat_buffer_if_needed`] but with an explicit seam footprint
/// (world-space tile rectangle) so synthesized points never cross into a
/// neighbor tile's territory even if this tile's baked splats stop short of the
/// geometric edge.
pub fn augment_splat_buffer_within(
    baked: &SplatBuffer,
    requested_zoom: u8,
    actual_zoom: u8,
    seam: TileFootprint,
) -> Option<SplatBuffer> {
    if !splat_needs_interpolation(requested_zoom, actual_zoom) || baked.len() < 3 {
        return None;
    }
    Some(interpolate_density(baked, seam))
}

/// Deterministic hash of two integers into `[0, 1)`. Self-contained mirror of
/// `synth::hash01` (that one is private) so this module composes without editing
/// synth.rs — same mix so jitter statistics match native splats.
fn hash01(a: u32, b: u32) -> f32 {
    let mut h = a.wrapping_mul(0x9E3779B1) ^ b.wrapping_mul(0x85EBCA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A2D39);
    h ^= h >> 15;
    (h as f32) / (u32::MAX as f32)
}

/// Deterministic signed hash in `[-1, 1)`.
fn hash_signed(a: u32, b: u32, salt: u32) -> f32 {
    hash01(a ^ salt, b.wrapping_add(salt)) * 2.0 - 1.0
}

/// Squared XZ distance between two splat positions (y ignored: splats drape a
/// roughly-planar surface, so the marching-squares grid lives in XZ).
#[inline]
fn xz_dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    dx * dx + dz * dz
}

/// Recursive logarithmic subdivision: each pass halves the effective point
/// spacing by filling marching-squares cell edges with midpoint splats. Stops
/// when the overlap radius is reached (adding a point would land inside an
/// existing point's overlap footprint — "dense enough" per synth's tuning) or
/// no in-tile edges remain; the pass count is only a degenerate-input backstop.
fn interpolate_density(baked: &SplatBuffer, seam: TileFootprint) -> SplatBuffer {
    let mut out = baked.clone();
    // Absolute cluster-rejection floor tied to the overlap radius: rejects only
    // genuinely coincident midpoints, while still permitting densification down to
    // the overlap radius itself (does not shrink per-pass, nor block the fill).
    let min_sep = 2.0 * SPLAT_COVERAGE * MIN_MIDPOINT_SEP_FRACTION;
    for pass in 0..MAX_INTERPOLATION_PASSES {
        let spacing = median_neighbor_spacing(&out);
        if !(spacing > 0.0) {
            break;
        }
        // Overlap stop: once neighbor spacing has shrunk below the point at which
        // a new midpoint (at ~spacing/2) would sit inside an existing splat's
        // overlap radius, the field is already dense — halt (terminator, not cap).
        if spacing <= 2.0 * SPLAT_COVERAGE {
            break;
        }
        let added = one_pass(&out, seam, spacing, min_sep, pass as u32);
        if added.is_empty() {
            break;
        }
        merge(&mut out, added);
    }
    out
}

fn merge(dst: &mut SplatBuffer, src: SplatBuffer) {
    dst.positions.extend(src.positions);
    dst.colors.extend(src.colors);
    dst.scales.extend(src.scales);
    dst.normals.extend(src.normals);
}

/// Emit midpoint splats for the marching-squares edges of the current point set,
/// skipping any midpoint that (a) falls outside the tile seam footprint or (b)
/// lands within the overlap radius of a source endpoint. `pass` salts jitter.
fn one_pass(buf: &SplatBuffer, seam: TileFootprint, spacing: f32, min_sep: f32, pass: u32) -> SplatBuffer {
    let n = buf.len();
    let mut out = SplatBuffer::default();
    if n < 3 {
        return out;
    }
    let max_edge2 = {
        let t = spacing * EDGE_SPACING_TOLERANCE;
        t * t
    };
    // Below this edge length the midpoint would overlap its endpoints → skip it
    // (per-edge overlap guard, complements the global spacing stop above).
    let min_edge2 = {
        let t = 2.0 * SPLAT_COVERAGE;
        t * t
    };
    // Halve each generation's radius toward the native so doubled density keeps
    // proportional overlap rather than fat blobs.
    let radius_scale = 0.5f32.powi(pass as i32 + 1) + 0.5;

    let mut seen = std::collections::HashSet::new();
    for i in 0..n {
        for &j in nearest_neighbors(buf, i, NEIGHBORS_PER_SPLAT).iter() {
            let (lo, hi) = if i < j { (i, j) } else { (j, i) };
            if lo == hi || !seen.insert((lo, hi)) {
                continue;
            }
            let d2 = xz_dist2(buf.positions[lo], buf.positions[hi]);
            if d2 > max_edge2 || d2 < min_edge2 {
                continue;
            }
            out.push_midpoint(buf, lo, hi, spacing, min_sep, radius_scale, pass, &seam);
        }
    }
    out
}

impl SplatBuffer {
    /// Append one interpolated splat at the midpoint of splats `a` and `b`,
    /// unless the (jittered) midpoint would cross the tile seam.
    #[allow(clippy::too_many_arguments)]
    fn push_midpoint(
        &mut self,
        src: &SplatBuffer,
        a: usize,
        b: usize,
        spacing: f32,
        min_sep: f32,
        radius_scale: f32,
        pass: u32,
        seam: &TileFootprint,
    ) {
        let pa = src.positions[a];
        let pb = src.positions[b];
        let mut pos = [
            (pa[0] + pb[0]) * 0.5,
            (pa[1] + pb[1]) * 0.5,
            (pa[2] + pb[2]) * 0.5,
        ];
        // Deterministic XZ jitter keyed on the edge so the fill is not a regular
        // half-step grid (FR-3 anti-dot-grid); y stays interpolated (no jitter).
        let ha = a as u32;
        let hb = b as u32;
        pos[0] += hash_signed(ha, hb, 0xA5A5 ^ pass) * JITTER_FRACTION * spacing;
        pos[2] += hash_signed(ha, hb, 0x5A5A ^ pass) * JITTER_FRACTION * spacing;

        // Seam guard: never place a synthesized splat past this tile's footprint
        // into a neighbor's territory (prevents double-fill / density seam).
        if !seam.contains_xz(pos) {
            return;
        }

        // Cluster guard: reject a midpoint that lands atop an existing splat (`src`,
        // the accumulated set) or one already emitted this pass (`self`). Adjacent
        // edges + jitter can otherwise collapse midpoints into a degenerate cluster.
        // The floor is absolute (a fraction of the base spacing), NOT the per-pass
        // spacing — the latter shrinks each pass and would let late-pass jittered
        // midpoints drift arbitrarily close.
        let min_sep2 = min_sep * min_sep;
        if src.positions.iter().chain(self.positions.iter()).any(|p| xz_dist2(*p, pos) < min_sep2) {
            return;
        }

        let na = src.normals[a];
        let nb = src.normals[b];
        let mut nrm = [
            (na[0] + nb[0]) * 0.5,
            (na[1] + nb[1]) * 0.5,
            (na[2] + nb[2]) * 0.5,
        ];
        let nl = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        if nl > 1e-6 {
            nrm = [nrm[0] / nl, nrm[1] / nl, nrm[2] / nl];
        } else {
            nrm = [0.0, 1.0, 0.0];
        }

        let ca = src.colors[a];
        let cb = src.colors[b];
        let color = [
            (ca[0] + cb[0]) * 0.5,
            (ca[1] + cb[1]) * 0.5,
            (ca[2] + cb[2]) * 0.5,
            (ca[3] + cb[3]) * 0.5,
        ];

        let sa = src.scales[a];
        let sb = src.scales[b];
        let radius_jitter = 1.0 + hash_signed(ha, hb, 0x1234 ^ pass) * RADIUS_VARIATION_FRACTION;
        let major = (sa[0] + sb[0]) * 0.5 * radius_scale * radius_jitter;
        let minor = (sa[1] + sb[1]) * 0.5 * radius_scale * radius_jitter;

        self.positions.push(pos);
        self.normals.push(nrm);
        self.colors.push(color);
        self.scales.push([major, minor]);
    }
}

/// Indices of the `k` nearest XZ neighbors of splat `i` (excluding `i`).
fn nearest_neighbors(buf: &SplatBuffer, i: usize, k: usize) -> Vec<usize> {
    let n = buf.len();
    let mut d: Vec<(f32, usize)> = Vec::with_capacity(n.saturating_sub(1));
    let pi = buf.positions[i];
    for j in 0..n {
        if j == i {
            continue;
        }
        d.push((xz_dist2(pi, buf.positions[j]), j));
    }
    d.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    d.truncate(k);
    d.into_iter().map(|(_, j)| j).collect()
}

/// Median nearest-neighbor XZ distance across the buffer — the buffer's implicit
/// grid spacing, used to reject spurious long edges, scale jitter, and drive the
/// overlap termination test.
fn median_neighbor_spacing(buf: &SplatBuffer) -> f32 {
    let n = buf.len();
    let mut nearest: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let mut best = f32::INFINITY;
        for j in 0..n {
            if j == i {
                continue;
            }
            let d2 = xz_dist2(buf.positions[i], buf.positions[j]);
            if d2 < best {
                best = d2;
            }
        }
        if best.is_finite() {
            nearest.push(best.sqrt());
        }
    }
    if nearest.is_empty() {
        return 0.0;
    }
    nearest.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    nearest[nearest.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a flat W×H grid SplatBuffer with the given ground spacing.
    fn grid_buffer(w: usize, h: usize, spacing: f32) -> SplatBuffer {
        let mut buf = SplatBuffer::default();
        for r in 0..h {
            for c in 0..w {
                buf.positions
                    .push([c as f32 * spacing, 0.0, r as f32 * spacing]);
                buf.normals.push([0.0, 1.0, 0.0]);
                buf.colors.push([0.5, 0.5, 0.5, 1.0]);
                buf.scales.push([spacing * 0.8, spacing * 0.8]);
            }
        }
        buf
    }

    // FR-1 boundary conditions.
    #[test]
    fn needs_interpolation_only_above_ceiling() {
        assert!(splat_needs_interpolation(15, 14));
        assert!(splat_needs_interpolation(20, 12));
        assert!(!splat_needs_interpolation(14, 14)); // satisfied by real data
        assert!(!splat_needs_interpolation(13, 14)); // requesting less than actual
        assert!(!splat_needs_interpolation(0, 0));
    }

    // FR-4: no-op when the ceiling isn't hit.
    #[test]
    fn augment_returns_none_when_satisfied() {
        let buf = grid_buffer(4, 4, 10.0);
        assert!(augment_splat_buffer_if_needed(&buf, 14, 14).is_none());
        assert!(augment_splat_buffer_if_needed(&buf, 13, 14).is_none());
    }

    #[test]
    fn augment_returns_none_on_tiny_buffer() {
        let buf = grid_buffer(1, 2, 10.0); // 2 splats < 3
        assert!(augment_splat_buffer_if_needed(&buf, 16, 14).is_none());
    }

    // FR-2: point count increases, and parallel arrays stay consistent.
    #[test]
    fn augment_increases_density() {
        let buf = grid_buffer(5, 5, 10.0);
        let before = buf.len();
        let aug = augment_splat_buffer_if_needed(&buf, 15, 14).expect("should augment");
        assert!(aug.len() > before, "density should increase: {} -> {}", before, aug.len());
        // Parallel arrays stay in lockstep so it composes with bake_splat_mesh.
        assert_eq!(aug.positions.len(), aug.colors.len());
        assert_eq!(aug.positions.len(), aug.scales.len());
        assert_eq!(aug.positions.len(), aug.normals.len());
    }

    // FR-5 primary terminator: a field already at/below the overlap spacing gets
    // few/no new points — the overlap condition halts the fill immediately.
    #[test]
    fn overlap_condition_stops_fill_in_dense_region() {
        // Spacing already below 2*SPLAT_COVERAGE (1.6) → overlap reached at entry.
        let dense = grid_buffer(6, 6, 1.0);
        let before = dense.len();
        let aug = augment_splat_buffer_if_needed(&dense, 20, 14).unwrap();
        assert_eq!(aug.len(), before, "dense field must not gain points past overlap");
    }

    // FR-5 logarithmic densify: a coarse field fills over several passes, and the
    // final spacing lands at/below the overlap threshold (not the pass cap).
    #[test]
    fn fill_terminates_at_overlap_not_pass_cap() {
        let buf = grid_buffer(6, 6, 10.0);
        let aug = augment_splat_buffer_if_needed(&buf, 40, 14).unwrap();
        let final_spacing = median_neighbor_spacing(&aug);
        assert!(
            final_spacing <= 2.0 * SPLAT_COVERAGE * EDGE_SPACING_TOLERANCE,
            "fill should densify to near the overlap radius, got spacing {final_spacing}"
        );
    }

    // FR-5 seam: no synthesized point may cross the tile footprint boundary.
    #[test]
    fn no_synthesized_point_crosses_seam() {
        let spacing = 10.0f32;
        let buf = grid_buffer(6, 6, spacing);
        let seam = TileFootprint::from_buffer(&buf);
        let native = buf.len();
        let aug = augment_splat_buffer_within(&buf, 20, 14, seam).unwrap();
        for (i, p) in aug.positions.iter().enumerate().skip(0) {
            // Native points define the seam; only check the synthesized tail.
            if i < native {
                continue;
            }
            assert!(
                seam.contains_xz(*p),
                "synthesized point {p:?} crossed the tile seam {seam:?}"
            );
        }
        assert!(aug.len() > native);
    }

    // FR-5 backstop: extreme zoom stays bounded (overlap OR cap guarantees it).
    #[test]
    fn growth_is_bounded_under_extreme_zoom() {
        let buf = grid_buffer(6, 6, 10.0);
        let base = buf.len();
        let extreme = augment_splat_buffer_if_needed(&buf, 40, 14).unwrap().len();
        assert!(
            extreme <= base * 64,
            "growth {extreme} exceeded backstop bound for base {base}"
        );
    }

    // FR-3: no degenerate/overlapping clusters — synthesized splats keep a sane
    // minimum separation (no coincident points → no moiré / z-fight cluster).
    #[test]
    fn no_degenerate_overlapping_clusters() {
        let spacing = 10.0f32;
        let buf = grid_buffer(6, 6, spacing);
        let aug = augment_splat_buffer_if_needed(&buf, 15, 14).unwrap();
        let mut min_sep = f32::INFINITY;
        for i in 0..aug.len() {
            for j in (i + 1)..aug.len() {
                let d2 = xz_dist2(aug.positions[i], aug.positions[j]);
                if d2 < min_sep {
                    min_sep = d2;
                }
            }
        }
        let min_sep = min_sep.sqrt();
        assert!(
            min_sep > spacing * 0.05,
            "synthesized splats collapsed into a cluster: min sep {min_sep}"
        );
    }

    #[test]
    fn deterministic_across_calls() {
        let buf = grid_buffer(5, 5, 10.0);
        let a = augment_splat_buffer_if_needed(&buf, 16, 14).unwrap();
        let b = augment_splat_buffer_if_needed(&buf, 16, 14).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.scales, b.scales);
    }
}
