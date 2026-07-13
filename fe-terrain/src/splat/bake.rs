//! Bake-time coverage-driven splat hole-filling (no bevy); see
//! `src/splat/AGENTS.md` §bake. Ported from the reverted live-fill
//! (`archive/splat-coverage-experiment-20260712`, `interpolate.rs`) but runs
//! ONCE per tile at hexon/tileset build time instead of per-frame. Neighbor
//! and cluster-proximity lookups use a uniform spatial grid so the whole pass
//! is O(n) instead of the archived O(n^2) all-pairs scan — the specific hot
//! spot that spiked frame time and crashed the app when run live.

use std::collections::HashMap;

use crate::splat::synth::SplatBuffer;

/// Hard safety backstop on fill passes. NOT the primary terminator: the fill
/// normally stops when no neighbor pair leaves an uncovered gap (coverage
/// condition below). This only guards a degenerate input that never converges.
pub const MAX_INTERPOLATION_PASSES: u8 = 8;

/// A synthesized splat's minor radius is `gap * this`, where `gap` is the
/// uncovered distance between the two endpoints' coverage discs. `> 0.5`
/// guarantees the new disc overlaps BOTH endpoints (no seam at the fill's own
/// edges) rather than merely kissing them.
const GAP_FILL_RADIUS_FRACTION: f32 = 0.6;
/// Coverage floor: a midpoint whose bridging radius would fall below this
/// (relative to the base spacing) is not worth adding — logarithmic-shrink
/// terminator, as the field densifies required fill radii shrink under this
/// floor and the pass stops.
const MIN_FILL_RADIUS_FRACTION: f32 = 0.08;

/// Fraction of local neighbor spacing a synthesized splat's XZ position may
/// jitter (± per axis), mirroring `synth::JITTER_FRACTION`.
const JITTER_FRACTION: f32 = 0.35;
/// Per-splat radius variation (± fraction of interpolated radius).
const RADIUS_VARIATION_FRACTION: f32 = 0.2;
/// Native splat minor-radius / spacing ratio synth.rs bakes; used by tests to
/// build a realistically-sized starting field.
#[cfg_attr(not(test), allow(dead_code))]
const SPLAT_COVERAGE: f32 = 0.8;
/// A neighbor pair is a real adjacency (candidate hole edge) only when its XZ
/// distance is within this multiple of the current spacing.
const EDGE_SPACING_TOLERANCE: f32 = 1.6;
/// Minimum XZ separation between a synthesized splat and any existing one, as
/// a fraction of the BASE spacing (not the shrinking per-pass spacing).
const MIN_MIDPOINT_SEP_FRACTION: f32 = 0.4;
/// Nearest XZ neighbors considered per splat when reconstructing the implicit grid.
const NEIGHBORS_PER_SPLAT: usize = 8;

/// Axis-aligned XZ tile footprint used as the seam boundary: synthesized
/// points must stay strictly inside so neighboring tiles don't double-fill
/// the shared edge. Tile-local coords (splats are baked tile-local).
#[derive(Debug, Clone, Copy)]
pub struct TileFootprint {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

impl TileFootprint {
    /// Footprint covering exactly the buffer's current XZ extent.
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

/// Bake-time entry point (FR-1). Runs the coverage-driven hole fill on `baked`
/// once and returns the densified buffer, or `None` if there was nothing to
/// fill (already hole-free) or the buffer is too small to define a field.
/// Called from `TilesetBuilder` at tileset build time — NOT from the runtime
/// render hot path (see FR-4, `render.rs::build_tile_splat_mesh`).
pub fn bake_splat_coverage(baked: &SplatBuffer) -> Option<SplatBuffer> {
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

/// As [`bake_splat_coverage`] but with an explicit seam footprint (world-space
/// tile rectangle) so synthesized points never cross into a neighbor tile's
/// territory even if this tile's baked splats stop short of the geometric edge.
pub fn bake_splat_coverage_within(baked: &SplatBuffer, seam: TileFootprint) -> Option<SplatBuffer> {
    if baked.len() < 3 {
        return None;
    }
    let filled = interpolate_density(baked, seam);
    if filled.len() > baked.len() {
        Some(filled)
    } else {
        None
    }
}

/// Deterministic hash of two integers into `[0, 1)`. Self-contained mirror of
/// `synth::hash01` so this module composes without editing synth.rs.
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
/// roughly-planar surface).
#[inline]
fn xz_dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    dx * dx + dz * dz
}

// ---------------------------------------------------------------------------
// Spatial grid — replaces the archived O(n^2) all-pairs neighbor/cluster scan.
// ---------------------------------------------------------------------------

/// Uniform-bucket spatial index over splat XZ positions. Cell size is chosen
/// from the caller's spacing estimate so each cell holds ~O(1) splats and
/// neighbor/cluster queries only visit the query point's 3x3 cell block —
/// O(1) amortized per query, O(n) total per pass instead of the archived
/// O(n^2) nested loop over every splat pair.
struct SpatialGrid {
    cell_size: f32,
    min_x: f32,
    min_z: f32,
    buckets: HashMap<(i32, i32), Vec<usize>>,
}

impl SpatialGrid {
    fn build(positions: &[[f32; 3]], cell_size: f32) -> Self {
        let cell_size = cell_size.max(1e-6);
        let mut min_x = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        for p in positions {
            min_x = min_x.min(p[0]);
            min_z = min_z.min(p[2]);
        }
        if !min_x.is_finite() {
            min_x = 0.0;
        }
        if !min_z.is_finite() {
            min_z = 0.0;
        }
        let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::with_capacity(positions.len());
        for (i, p) in positions.iter().enumerate() {
            let key = Self::cell_key(p[0], p[2], min_x, min_z, cell_size);
            buckets.entry(key).or_default().push(i);
        }
        Self {
            cell_size,
            min_x,
            min_z,
            buckets,
        }
    }

    #[inline]
    fn cell_key(x: f32, z: f32, min_x: f32, min_z: f32, cell_size: f32) -> (i32, i32) {
        (
            ((x - min_x) / cell_size).floor() as i32,
            ((z - min_z) / cell_size).floor() as i32,
        )
    }

    /// Indices of every splat in the 3x3 cell block around `(x, z)` (excludes
    /// nothing — caller filters). Covers any point within one `cell_size` of
    /// the query, which is sufficient for the cluster-proximity check
    /// (`min_sep` is always well under one spacing). For k-NN, use
    /// `nearby_ring` instead — a single 3x3 block is NOT provably complete.
    fn nearby(&self, x: f32, z: f32) -> Vec<usize> {
        self.nearby_ring(x, z, 1)
    }

    /// Indices of every splat within a `(2*ring+1)^2` cell block around
    /// `(x, z)`. A ring of `r` guarantees every point within `r * cell_size`
    /// of the query is included.
    fn nearby_ring(&self, x: f32, z: f32, ring: i32) -> Vec<usize> {
        let (cx, cz) = Self::cell_key(x, z, self.min_x, self.min_z, self.cell_size);
        let mut out = Vec::new();
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                if let Some(bucket) = self.buckets.get(&(cx + dx, cz + dz)) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        out
    }
}

/// Incremental XZ point index for the CURRENT pass's own emissions; replaces the
/// archived O(m^2) linear self-scan in `push_midpoint`. Cell size is the
/// cluster-rejection separation, so any prior emission within `min_sep` of a
/// query falls in the query's 3x3 cell block — O(1) amortized per check/insert.
struct InsertGrid {
    cell_size: f32,
    origin_x: f32,
    origin_z: f32,
    cells: HashMap<(i32, i32), Vec<[f32; 3]>>,
}

impl InsertGrid {
    /// New grid whose cell size is the min-separation and whose cell origin is
    /// anchored to the same-pass source extent so keys stay stable and small.
    fn new(cell_size: f32, origin_x: f32, origin_z: f32) -> Self {
        Self {
            cell_size: cell_size.max(1e-6),
            origin_x: if origin_x.is_finite() { origin_x } else { 0.0 },
            origin_z: if origin_z.is_finite() { origin_z } else { 0.0 },
            cells: HashMap::new(),
        }
    }

    #[inline]
    fn key(&self, x: f32, z: f32) -> (i32, i32) {
        (
            ((x - self.origin_x) / self.cell_size).floor() as i32,
            ((z - self.origin_z) / self.cell_size).floor() as i32,
        )
    }

    /// True if any already-inserted point is within `sqrt(min_sep2)` of `pos`.
    /// Scans only the 3x3 block, which fully covers the `min_sep` radius because
    /// `cell_size == min_sep`.
    fn any_within(&self, pos: [f32; 3], min_sep2: f32) -> bool {
        let (cx, cz) = self.key(pos[0], pos[2]);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if let Some(bucket) = self.cells.get(&(cx + dx, cz + dz)) {
                    for p in bucket {
                        if xz_dist2(*p, pos) < min_sep2 {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    #[inline]
    fn insert(&mut self, pos: [f32; 3]) {
        let key = self.key(pos[0], pos[2]);
        self.cells.entry(key).or_default().push(pos);
    }
}

/// Coverage-driven hole fill: each pass finds neighbor pairs whose coverage
/// discs don't touch (a visible background gap) and drops a midpoint splat
/// sized to bridge that specific gap. Passes repeat until no pair leaves an
/// uncovered gap or no admissible midpoint remains; the pass count is only a
/// degenerate-input backstop. Fill radii shrink logarithmically on their own.
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

/// Emit a bridging splat for every near neighbor pair whose coverage discs
/// leave an uncovered gap. Neighbor discovery uses the spatial grid (O(n)
/// total) instead of the archived per-splat O(n) scan over every other splat
/// (O(n^2) total).
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
    let max_edge2 = {
        let t = spacing * EDGE_SPACING_TOLERANCE;
        t * t
    };
    // Grid cell sized to the neighbor search radius so nearest_neighbors_grid
    // only needs the 3x3 block.
    let grid = SpatialGrid::build(&buf.positions, spacing * EDGE_SPACING_TOLERANCE);
    // This-pass emission index (min-sep cell size) replaces the O(m^2) self-scan.
    let mut emitted = InsertGrid::new(min_sep.max(1e-6), grid.min_x, grid.min_z);

    let mut seen = std::collections::HashSet::new();
    for i in 0..n {
        for j in nearest_neighbors_grid(buf, &grid, i, NEIGHBORS_PER_SPLAT) {
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
            out.push_midpoint(
                buf,
                &grid,
                &mut emitted,
                lo,
                hi,
                fill_radius,
                min_sep,
                pass,
                &seam,
            );
        }
    }
    out
}

impl SplatBuffer {
    /// Append one bridging splat at the midpoint of splats `a` and `b`, sized by
    /// `fill_radius` to close the uncovered gap between them, unless the
    /// (jittered) midpoint would cross the tile seam or land atop a neighbor.
    /// Cluster rejection uses the pass-start spatial grid (for `src`) and an
    /// incremental `InsertGrid` (for this pass's own emissions), both O(1)
    /// amortized — replacing the archived O(n^2) + O(m^2) linear cluster guards.
    #[allow(clippy::too_many_arguments)]
    fn push_midpoint(
        &mut self,
        src: &SplatBuffer,
        grid: &SpatialGrid,
        emitted: &mut InsertGrid,
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

        // Cluster guard: reject a midpoint that lands atop an existing splat
        // (`src`, the accumulated set) or one already emitted this pass. `src` is
        // checked via the pass-start grid's 3x3 block; this-pass emissions via the
        // incremental `emitted` grid (also 3x3) — both O(1) amortized, no linear
        // scan. Same min_sep2 rejection semantics as the archived guard.
        let min_sep2 = min_sep * min_sep;
        let nearby = grid.nearby(pos[0], pos[2]);
        for idx in nearby {
            if xz_dist2(src.positions[idx], pos) < min_sep2 {
                return;
            }
        }
        if emitted.any_within(pos, min_sep2) {
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
        // densifies. Keep the disc roughly round (major ≈ minor) with a touch
        // of radius jitter.
        let radius_jitter = 1.0 + hash_signed(ha, hb, 0x1234 ^ pass) * RADIUS_VARIATION_FRACTION;
        let minor = fill_radius * radius_jitter;
        let major = minor;

        self.positions.push(pos);
        self.normals.push(nrm);
        self.colors.push(color);
        self.scales.push([major, minor]);
        // Index the accepted emission so later midpoints THIS pass reject against
        // it in O(1) (mirrors the old `self.positions` linear scan, non-linearly).
        emitted.insert(pos);
    }
}

/// Indices of up to `k` nearest XZ neighbors of splat `i` (excluding `i`),
/// drawn from the grid and ranked by distance. Starts at the 3x3 cell block
/// and expands the search ring outward (still O(1) amortized in the common
/// case) until at least `k` candidates are found AND the k-th nearest
/// distance is within `ring * cell_size` — the radius the current ring
/// provably covers. Without this expansion a sparse/edge query point can
/// have its true k-th neighbor sitting just outside a fixed 3x3 block.
/// Ties in distance break by index (ascending) so the result matches a
/// brute-force scan over `0..n` regardless of the grid's bucket iteration
/// order.
fn nearest_neighbors_grid(buf: &SplatBuffer, grid: &SpatialGrid, i: usize, k: usize) -> Vec<usize> {
    let pi = buf.positions[i];
    let n = buf.len();
    // Total (dist2, index) order: distance first, index as tie-break — matches a
    // brute-force stable sort over `0..n` exactly.
    let cmp = |x: &(f32, usize), y: &(f32, usize)| {
        x.0.partial_cmp(&y.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(x.1.cmp(&y.1))
    };
    let mut ring = 1i32;
    loop {
        let mut candidates = grid.nearby_ring(pi[0], pi[2], ring);
        candidates.retain(|&j| j != i);
        let mut d: Vec<(f32, usize)> = candidates
            .into_iter()
            .map(|j| (xz_dist2(pi, buf.positions[j]), j))
            .collect();

        // Partial selection instead of a full sort: we only need the k smallest.
        // `select_nth_unstable_by` puts the k-th element (index k-1) in place with
        // everything smaller before it — O(m) vs the previous O(m log m) full sort.
        let have = d.len();
        if have >= k {
            let (_, kth, _) = d.select_nth_unstable_by(k - 1, cmp);
            let kth_dist2 = kth.0;
            let covered_radius = ring as f32 * grid.cell_size;
            let covered_radius2 = covered_radius * covered_radius;
            if kth_dist2 <= covered_radius2 {
                // k-th nearest is provably within the covered radius: the k smallest
                // are the leading partition; sort only those (with the tie-break) and
                // return. No wider ring can produce a nearer point.
                d.truncate(k);
                d.sort_by(cmp);
                return d.into_iter().map(|(_, j)| j).collect();
            }
        }
        if have >= n.saturating_sub(1) {
            // Swept every remaining point: the ring can't grow usefully. Return the
            // k smallest in order.
            d.sort_by(cmp);
            d.truncate(k);
            return d.into_iter().map(|(_, j)| j).collect();
        }
        ring += 1;
    }
}

/// Median nearest-neighbor XZ distance across the buffer — the buffer's
/// implicit grid spacing, used to reject spurious long edges and to anchor
/// the base-spacing cluster/fill floors that terminate the coverage fill.
/// Uses a coarse spatial grid seeded from the buffer's own bounding box so
/// this stays O(n) rather than the archived O(n^2) all-pairs scan.
fn median_neighbor_spacing(buf: &SplatBuffer) -> f32 {
    let n = buf.len();
    if n < 2 {
        return 0.0;
    }
    // Bootstrap cell size from the bounding box density (n splats over the
    // buffer's XZ area), refined once nearest distances are known isn't
    // possible without spacing itself — so use an area-based estimate.
    let fp = TileFootprint::from_buffer(buf);
    let area = ((fp.max_x - fp.min_x).max(1e-3)) * ((fp.max_z - fp.min_z).max(1e-3));
    let cell = (area / n as f32).sqrt().max(1e-3);
    let grid = SpatialGrid::build(&buf.positions, cell);

    let mut nearest: Vec<f32> = Vec::with_capacity(n);
    for i in 0..n {
        let pi = buf.positions[i];
        let mut best = f32::INFINITY;
        for j in grid.nearby(pi[0], pi[2]) {
            if j == i {
                continue;
            }
            let d2 = xz_dist2(pi, buf.positions[j]);
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
    /// `cover_frac >= √2/2 ≈ 0.707`.
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

    /// Grid with the native `SPLAT_COVERAGE` sizing (holes between diagonal but
    /// not orthogonal neighbors — the realistic starting field).
    fn grid_buffer(w: usize, h: usize, spacing: f32) -> SplatBuffer {
        grid_buffer_cover(w, h, spacing, SPLAT_COVERAGE)
    }

    #[test]
    fn bake_returns_none_on_tiny_buffer() {
        let buf = grid_buffer(1, 2, 10.0); // 2 splats < 3
        assert!(bake_splat_coverage(&buf).is_none());
    }

    // Coverage fill: a field with real holes (small coverage) densifies; parallel
    // arrays stay in lockstep so it composes with bake_splat_mesh.
    #[test]
    fn coverage_fill_closes_holes() {
        // cover_frac 0.35 → orthogonal neighbors 10 apart, cover 2*3.5=7 < 10 → gap.
        let buf = grid_buffer_cover(5, 5, 10.0, 0.35);
        let before = buf.len();
        let aug = bake_splat_coverage(&buf).expect("should fill holes");
        assert!(aug.len() > before, "density should increase: {} -> {}", before, aug.len());
        assert_eq!(aug.positions.len(), aug.colors.len());
        assert_eq!(aug.positions.len(), aug.scales.len());
        assert_eq!(aug.positions.len(), aug.normals.len());
    }

    // Termination: an already-touching field gains nothing — no holes to close.
    #[test]
    fn no_fill_when_already_covered() {
        let covered = grid_buffer_cover(6, 6, 10.0, 0.75);
        assert!(
            bake_splat_coverage(&covered).is_none(),
            "a hole-free field must not gain points"
        );
    }

    // Proximity-driven scale: a synthesized splat is sized to its own gap
    // (logarithmic shrink is implicit — later passes see smaller residual gaps).
    #[test]
    fn fill_radius_scales_with_gap_and_shrinks() {
        let spacing = 10.0f32;
        let buf = grid_buffer_cover(6, 6, spacing, 0.3);
        let native = buf.len();
        let aug = bake_splat_coverage(&buf).unwrap();
        assert!(aug.len() > native, "expected holes to be filled");
        let max_gap = std::f32::consts::SQRT_2 * spacing - 2.0 * 0.3 * spacing;
        let ceiling = max_gap * GAP_FILL_RADIUS_FRACTION * (1.0 + RADIUS_VARIATION_FRACTION) + 1e-2;
        let floor = spacing * MIN_FILL_RADIUS_FRACTION * (1.0 - RADIUS_VARIATION_FRACTION) - 1e-2;
        for i in native..aug.len() {
            let m = aug.scales[i][1];
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
        let aug = bake_splat_coverage(&buf).unwrap();
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
        let filled = bake_splat_coverage(&buf).unwrap().len();
        assert!(
            filled <= base * 64,
            "growth {filled} exceeded backstop bound for base {base}"
        );
    }

    // FR-1: no degenerate/overlapping clusters — synthesized splats keep a sane
    // minimum separation (no coincident points → no moiré / z-fight cluster).
    #[test]
    fn no_degenerate_overlapping_clusters() {
        let spacing = 10.0f32;
        let buf = grid_buffer_cover(6, 6, spacing, 0.3);
        let aug = bake_splat_coverage(&buf).unwrap();
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
        let a = bake_splat_coverage(&buf).unwrap();
        let b = bake_splat_coverage(&buf).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.scales, b.scales);
    }

    // FR-1 perf: the spatial grid produces the SAME topology-independent result
    // as a naive input scaled up — this is a smoke test that the grid handles a
    // few hundred splats without the pathological all-pairs cost (a genuine perf
    // regression would still pass functionally, but this exercises the size
    // that crashed the app live to ensure it at least completes and terminates).
    #[test]
    fn bakes_large_field_without_hanging() {
        let buf = grid_buffer_cover(24, 24, 10.0, 0.3); // 576 splats
        let native = buf.len();
        let aug = bake_splat_coverage(&buf).expect("should fill holes");
        assert!(aug.len() > native);
        assert!(aug.len() <= native * 64);
    }

    #[test]
    fn spatial_grid_finds_same_neighbors_as_brute_force() {
        let buf = grid_buffer(5, 5, 10.0);
        let grid = SpatialGrid::build(&buf.positions, 10.0 * EDGE_SPACING_TOLERANCE);
        for i in 0..buf.len() {
            let grid_neighbors: std::collections::HashSet<usize> =
                nearest_neighbors_grid(&buf, &grid, i, NEIGHBORS_PER_SPLAT)
                    .into_iter()
                    .collect();
            // Brute-force nearest k.
            let pi = buf.positions[i];
            let mut all: Vec<(f32, usize)> = (0..buf.len())
                .filter(|&j| j != i)
                .map(|j| (xz_dist2(pi, buf.positions[j]), j))
                .collect();
            all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            all.truncate(NEIGHBORS_PER_SPLAT);
            let brute: std::collections::HashSet<usize> = all.into_iter().map(|(_, j)| j).collect();
            assert_eq!(
                grid_neighbors, brute,
                "grid neighbor set for splat {i} diverged from brute force"
            );
        }
    }
}
