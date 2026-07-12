//! Pure coverage-driven splat hole-filling (no bevy); see
//! `src/splat/AGENTS.md` §interpolation. Closes visible background gaps between
//! baked splats: for any neighbor pair whose coverage discs do not touch, insert
//! a midpoint splat SIZED to bridge that specific gap, recursing until every
//! neighbor pair overlaps (no untouched background) or the tile seam is reached.
//! New splats shrink logarithmically as the local field densifies — big holes
//! get big splats, small remaining gaps get small ones. Runs whenever holes
//! exist (not only past the `max_zoom` ceiling).

use crate::splat::synth::SplatBuffer;

/// Hard safety backstop on fill passes. NOT the primary terminator: the fill
/// normally stops when no neighbor pair leaves an uncovered gap (coverage
/// condition below). This only guards a degenerate input that never converges.
pub const MAX_INTERPOLATION_PASSES: u8 = 8;

/// A synthesized splat's minor radius is `gap * this`, where `gap` is the
/// uncovered distance between the two endpoints' coverage discs. Each endpoint
/// already covers `minor` toward the midpoint; the midpoint sits at the gap
/// center, so a radius of ~half the gap (plus a hair for guaranteed overlap)
/// closes it. `> 0.5` guarantees the new disc overlaps BOTH endpoints (no seam
/// at the fill's own edges) rather than merely kissing them.
const GAP_FILL_RADIUS_FRACTION: f32 = 0.6;
/// Coverage floor: a midpoint whose bridging radius would fall below this
/// (relative to the base spacing) is not worth adding — the residual gap is
/// sub-splat and effectively already touched. This is the logarithmic-shrink
/// terminator: as the field densifies, required fill radii shrink, and once they
/// cross this floor the hole is "closed enough" and the pass stops.
const MIN_FILL_RADIUS_FRACTION: f32 = 0.08;

/// Fraction of local neighbor spacing a synthesized splat's XZ position may
/// jitter (± per axis), mirroring `synth::JITTER_FRACTION` so upscaled points
/// share the native anti-dot-grid pattern (FR-3).
const JITTER_FRACTION: f32 = 0.35;
/// Per-splat radius variation (± fraction of interpolated radius), mirroring
/// `synth::RADIUS_VARIATION_FRACTION`.
const RADIUS_VARIATION_FRACTION: f32 = 0.2;
/// Native splat minor-radius / spacing ratio synth.rs bakes. Used by tests to
/// build a realistically-sized starting field; the fill itself reads each splat's
/// actual baked radius, not this constant.
#[cfg_attr(not(test), allow(dead_code))]
const SPLAT_COVERAGE: f32 = 0.8;
/// A neighbor pair is a real adjacency (candidate hole edge) only when its XZ
/// distance is within this multiple of the current spacing — rejects long
/// spurious links across the tile that would spawn splats in empty gaps.
const EDGE_SPACING_TOLERANCE: f32 = 1.6;
/// Minimum XZ separation between a synthesized splat and any existing one, as a
/// fraction of the base spacing: below this a fill point is effectively
/// coincident with a neighbor (moiré / z-fight). Tied to the base spacing (not
/// the shrinking per-pass spacing) so it neither drifts across passes nor blocks
/// legitimate hole-filling.
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

/// Coverage-driven hole fill (the primary path). Unlike
/// [`augment_splat_buffer_if_needed`], this is NOT gated on the zoom ceiling — it
/// runs whenever the baked field has visible gaps (neighbor coverage discs that
/// don't touch), which happens at normal zoom too because of jitter + irregular
/// spacing. Returns `None` only when there is nothing to fill (already
/// hole-free) or the buffer is too small to define a field.
pub fn augment_splat_buffer_coverage(baked: &SplatBuffer) -> Option<SplatBuffer> {
    if baked.len() < 3 {
        return None;
    }
    let filled = interpolate_density(baked, TileFootprint::from_buffer(baked));
    if filled.len() > baked.len() {
        Some(filled)
    } else {
        None
    }
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

/// Coverage-driven hole fill: each pass finds neighbor pairs whose coverage discs
/// don't touch (a visible background gap) and drops a midpoint splat sized to
/// bridge that specific gap. Passes repeat until no pair leaves an uncovered gap
/// (every spot is touched) or no admissible midpoint remains; the pass count is
/// only a degenerate-input backstop. Fill radii shrink logarithmically on their
/// own — as the field densifies, remaining gaps are smaller, so bridging radii
/// are smaller — until they fall under the coverage floor and the fill halts.
fn interpolate_density(baked: &SplatBuffer, seam: TileFootprint) -> SplatBuffer {
    let mut out = baked.clone();
    let base_spacing = median_neighbor_spacing(&out);
    if !(base_spacing > 0.0) {
        return out;
    }
    // Cluster-rejection floor + fill-radius floor are tied to the ORIGINAL base
    // spacing (not the shrinking per-pass spacing) so they don't drift as the
    // field densifies.
    let min_sep = base_spacing * MIN_MIDPOINT_SEP_FRACTION;
    let min_fill_radius = base_spacing * MIN_FILL_RADIUS_FRACTION;
    for pass in 0..MAX_INTERPOLATION_PASSES {
        let spacing = median_neighbor_spacing(&out);
        if !(spacing > 0.0) {
            break;
        }
        let added = one_pass(&out, seam, spacing, min_sep, min_fill_radius, pass as u32);
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

/// Emit a bridging splat for every near neighbor pair whose coverage discs leave
/// an uncovered gap (visible background). The new splat's radius is proportional
/// to the gap (`GAP_FILL_RADIUS_FRACTION`), so big holes get big splats and, as
/// the field densifies over passes, residual gaps and their fill radii shrink
/// logarithmically. Skips pairs already touching, pairs whose bridge radius falls
/// under the coverage floor, midpoints outside the seam, and degenerate clusters.
#[allow(clippy::too_many_arguments)]
fn one_pass(
    buf: &SplatBuffer,
    seam: TileFootprint,
    spacing: f32,
    min_sep: f32,
    min_fill_radius: f32,
    pass: u32,
) -> SplatBuffer {
    let n = buf.len();
    let mut out = SplatBuffer::default();
    if n < 3 {
        return out;
    }
    // Reject spurious long links across the tile (a pair that far apart isn't a
    // real adjacency, just two distant splats); real holes sit between neighbors.
    let max_edge2 = {
        let t = spacing * EDGE_SPACING_TOLERANCE;
        t * t
    };

    let mut seen = std::collections::HashSet::new();
    for i in 0..n {
        for &j in nearest_neighbors(buf, i, NEIGHBORS_PER_SPLAT).iter() {
            let (lo, hi) = if i < j { (i, j) } else { (j, i) };
            if lo == hi || !seen.insert((lo, hi)) {
                continue;
            }
            let d2 = xz_dist2(buf.positions[lo], buf.positions[hi]);
            if d2 > max_edge2 {
                continue;
            }
            let dist = d2.sqrt();
            // Uncovered gap between the two coverage discs (minor radii). ≤0 means
            // they already touch/overlap → no hole → nothing to fill for this pair.
            let cover = buf.scales[lo][1] + buf.scales[hi][1];
            let gap = dist - cover;
            if gap <= 0.0 {
                continue;
            }
            // Radius that bridges this specific gap; shrinks as gaps shrink.
            let fill_radius = gap * GAP_FILL_RADIUS_FRACTION;
            if fill_radius < min_fill_radius {
                continue;
            }
            out.push_midpoint(buf, lo, hi, fill_radius, min_sep, pass, &seam);
        }
    }
    out
}

impl SplatBuffer {
    /// Append one bridging splat at the midpoint of splats `a` and `b`, sized by
    /// `fill_radius` to close the uncovered gap between them, unless the
    /// (jittered) midpoint would cross the tile seam or land atop a neighbor.
    #[allow(clippy::too_many_arguments)]
    fn push_midpoint(
        &mut self,
        src: &SplatBuffer,
        a: usize,
        b: usize,
        fill_radius: f32,
        min_sep: f32,
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
        // Small deterministic XZ jitter (scaled to the fill radius, not the base
        // spacing) so the fill isn't a regular half-step grid but stays inside the
        // gap it is closing; y stays interpolated (no jitter).
        let ha = a as u32;
        let hb = b as u32;
        pos[0] += hash_signed(ha, hb, 0xA5A5 ^ pass) * JITTER_FRACTION * fill_radius;
        pos[2] += hash_signed(ha, hb, 0x5A5A ^ pass) * JITTER_FRACTION * fill_radius;

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

        // Scale is proximity-driven: the minor radius bridges the actual gap
        // (`fill_radius`), so it grows with big holes and shrinks as the field
        // densifies. Keep the disc roughly round (major ≈ minor) — a hole-filler
        // has no slope to elongate along — with a touch of radius jitter.
        let radius_jitter = 1.0 + hash_signed(ha, hb, 0x1234 ^ pass) * RADIUS_VARIATION_FRACTION;
        let minor = fill_radius * radius_jitter;
        let major = minor;

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
/// grid spacing, used to reject spurious long edges and to anchor the base-spacing
/// cluster/fill floors that terminate the coverage fill.
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

    /// Build a flat W×H grid SplatBuffer with the given ground spacing. Splats are
    /// sized `cover_frac * spacing`. Orthogonal neighbors touch at `cover_frac >=
    /// 0.5`; DIAGONAL neighbors (the largest gap, at `√2·spacing`) only touch at
    /// `cover_frac >= √2/2 ≈ 0.707`. So a truly hole-free grid needs `>= ~0.71` —
    /// the knob that exercises the coverage-driven fill.
    fn grid_buffer_cover(w: usize, h: usize, spacing: f32, cover_frac: f32) -> SplatBuffer {
        let mut buf = SplatBuffer::default();
        for r in 0..h {
            for c in 0..w {
                buf.positions
                    .push([c as f32 * spacing, 0.0, r as f32 * spacing]);
                buf.normals.push([0.0, 1.0, 0.0]);
                buf.colors.push([0.5, 0.5, 0.5, 1.0]);
                buf.scales.push([spacing * cover_frac, spacing * cover_frac]);
            }
        }
        buf
    }

    /// Grid with the native `SPLAT_COVERAGE` sizing (holes between diagonal but not
    /// orthogonal neighbors — the realistic starting field).
    fn grid_buffer(w: usize, h: usize, spacing: f32) -> SplatBuffer {
        grid_buffer_cover(w, h, spacing, SPLAT_COVERAGE)
    }

    // FR-1 boundary conditions (ceiling-gated path still honored).
    #[test]
    fn needs_interpolation_only_above_ceiling() {
        assert!(splat_needs_interpolation(15, 14));
        assert!(splat_needs_interpolation(20, 12));
        assert!(!splat_needs_interpolation(14, 14)); // satisfied by real data
        assert!(!splat_needs_interpolation(13, 14)); // requesting less than actual
        assert!(!splat_needs_interpolation(0, 0));
    }

    // Ceiling-gated path: no-op when the ceiling isn't hit.
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
        assert!(augment_splat_buffer_coverage(&buf).is_none());
    }

    // Coverage fill: a field with real holes (small coverage) densifies; parallel
    // arrays stay in lockstep so it composes with bake_splat_mesh.
    #[test]
    fn coverage_fill_closes_holes() {
        // cover_frac 0.35 → orthogonal neighbors 10 apart, cover 2*3.5=7 < 10 → gap.
        let buf = grid_buffer_cover(5, 5, 10.0, 0.35);
        let before = buf.len();
        let aug = augment_splat_buffer_coverage(&buf).expect("should fill holes");
        assert!(aug.len() > before, "density should increase: {} -> {}", before, aug.len());
        assert_eq!(aug.positions.len(), aug.colors.len());
        assert_eq!(aug.positions.len(), aug.scales.len());
        assert_eq!(aug.positions.len(), aug.normals.len());
    }

    // Coverage fill runs at NORMAL zoom (decoupled from the max_zoom ceiling).
    #[test]
    fn coverage_fill_is_not_ceiling_gated() {
        let buf = grid_buffer_cover(5, 5, 10.0, 0.35);
        // No zoom argument at all — holes alone drive the fill.
        assert!(augment_splat_buffer_coverage(&buf).is_some());
    }

    // Termination: an already-touching field gains nothing — no holes to close.
    // Needs cover_frac ≥ √2/2 so even diagonal neighbors overlap (0.6 still leaves
    // a diagonal gap the fill would legitimately close).
    #[test]
    fn no_fill_when_already_covered() {
        let covered = grid_buffer_cover(6, 6, 10.0, 0.75);
        assert!(
            augment_splat_buffer_coverage(&covered).is_none(),
            "a hole-free field must not gain points"
        );
    }

    // Proximity-driven scale: a synthesized splat is sized to its own gap, and
    // later (smaller-gap) fills are smaller than the first (largest-gap) ones —
    // the logarithmic shrink. Each is positive and bounded by the biggest gap.
    #[test]
    fn fill_radius_scales_with_gap_and_shrinks() {
        let spacing = 10.0f32;
        let buf = grid_buffer_cover(6, 6, spacing, 0.3);
        let native = buf.len();
        let aug = augment_splat_buffer_coverage(&buf).unwrap();
        assert!(aug.len() > native, "expected holes to be filled");
        // The largest possible gap is diagonal: dist √2·spacing − 2·(0.3·spacing).
        let max_gap = std::f32::consts::SQRT_2 * spacing - 2.0 * 0.3 * spacing;
        let ceiling = max_gap * GAP_FILL_RADIUS_FRACTION * (1.0 + RADIUS_VARIATION_FRACTION) + 1e-2;
        let floor = spacing * MIN_FILL_RADIUS_FRACTION * (1.0 - RADIUS_VARIATION_FRACTION) - 1e-2;
        for i in native..aug.len() {
            let m = aug.scales[i][1];
            // Positive, sized to a real gap (≤ the largest possible gap) and never
            // below the coverage floor that terminates the fill.
            assert!(m > floor, "fill radius {m} below coverage floor {floor}");
            assert!(m <= ceiling, "fill radius {m} exceeds largest-gap ceiling {ceiling}");
        }
    }

    // Seam: no synthesized point may cross the tile footprint boundary.
    #[test]
    fn no_synthesized_point_crosses_seam() {
        let spacing = 10.0f32;
        let buf = grid_buffer_cover(6, 6, spacing, 0.3);
        let seam = TileFootprint::from_buffer(&buf);
        let native = buf.len();
        let aug = augment_splat_buffer_coverage(&buf).unwrap();
        for (i, p) in aug.positions.iter().enumerate() {
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

    // Backstop: growth stays bounded even for a very sparse (big-hole) field.
    #[test]
    fn growth_is_bounded() {
        let buf = grid_buffer_cover(6, 6, 10.0, 0.2);
        let base = buf.len();
        let filled = augment_splat_buffer_coverage(&buf).unwrap().len();
        assert!(
            filled <= base * 64,
            "growth {filled} exceeded backstop bound for base {base}"
        );
    }

    // FR-3: no degenerate/overlapping clusters — synthesized splats keep a sane
    // minimum separation (no coincident points → no moiré / z-fight cluster).
    #[test]
    fn no_degenerate_overlapping_clusters() {
        let spacing = 10.0f32;
        let buf = grid_buffer_cover(6, 6, spacing, 0.3);
        let aug = augment_splat_buffer_coverage(&buf).unwrap();
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
        let buf = grid_buffer_cover(5, 5, 10.0, 0.3);
        let a = augment_splat_buffer_coverage(&buf).unwrap();
        let b = augment_splat_buffer_coverage(&buf).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.scales, b.scales);
    }
}
