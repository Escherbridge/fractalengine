//! stamped_asset_nodes_20260725 (T2 FR-4): GPU-instanced stamp rendering + a
//! spatial pick index so tens of thousands of path-asset stamps (D-A6, N-9)
//! stay interactive. Un-promoted stamps are drawn as instances of one shared
//! mesh/material (no per-instance ECS entity), and picking resolves an
//! individual stamp through a uniform-grid spatial hash in O(1) amortized time
//! instead of an O(n) scan.
//!
//! This module is deliberately render-backend-agnostic: it builds the CPU-side
//! per-instance transform buffer (`InstanceBatch`) and the pick index
//! (`StampSpatialIndex`) as pure data. Wiring the batch into a concrete draw is
//! the app's job (Bevy's automatic instancing coalesces entities sharing one
//! `Mesh3d` + material handle; a custom instanced pipeline can consume
//! `InstanceBatch::instance_matrices`). All geometry is petal-local METERS
//! (N-1) — `world_scale` never enters here. See `fe-renderer/AGENTS.md`
//! §instancing.

use std::collections::HashMap;

/// Documented scale target (D-A6 / FR-4 acceptance): a scene of at least this
/// many stamps must render + pick within budget. The instance buffer is a flat
/// contiguous `Vec` (one cache-friendly upload) and picking is grid-bucketed, so
/// both stay sub-linear per frame at this ceiling. Benchmarked via
/// `bench_10k_build_and_pick` below (kept as a `#[test]` so CI exercises the
/// path; a criterion harness is a follow-up).
pub const TARGET_INSTANCE_CEILING: usize = 10_000;

/// One stamp's instance transform — position (path-derived, meters), rotation
/// (quaternion xyzw, tangent-align + per-stamp override folded in) and scale
/// (per-stamp override folded in). Promoted stamps carry their overrides here so
/// the instanced draw and the individually-addressed node agree (FR-4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StampInstanceData {
    /// Petal-local position in meters (N-1).
    pub position: [f32; 3],
    /// Rotation quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Non-uniform scale `[sx, sy, sz]`.
    pub scale: [f32; 3],
    /// Stable per-instance index within its stamp group — the value picking
    /// returns and the promotion path addresses (`PromoteInstance.instance_index`).
    pub stamp_index: u32,
}

impl StampInstanceData {
    /// A translate-only instance (identity rotation, unit scale) at `position`.
    pub fn at(position: [f32; 3], stamp_index: u32) -> Self {
        Self {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            stamp_index,
        }
    }

    /// Column-major 4×4 TRS matrix (meters) for GPU upload. Composed manually
    /// (no external math dep) so the wire format is explicit and testable.
    pub fn to_matrix(&self) -> [f32; 16] {
        let [x, y, z, w] = self.rotation;
        // Normalize defensively so a slightly off unit quat can't skew the basis.
        let n = (x * x + y * y + z * z + w * w).sqrt();
        let (x, y, z, w) = if n > 1e-9 {
            (x / n, y / n, z / n, w / n)
        } else {
            (0.0, 0.0, 0.0, 1.0)
        };
        let (sx, sy, sz) = (self.scale[0], self.scale[1], self.scale[2]);
        // Rotation matrix columns from the quaternion.
        let r00 = 1.0 - 2.0 * (y * y + z * z);
        let r01 = 2.0 * (x * y + z * w);
        let r02 = 2.0 * (x * z - y * w);
        let r10 = 2.0 * (x * y - z * w);
        let r11 = 1.0 - 2.0 * (x * x + z * z);
        let r12 = 2.0 * (y * z + x * w);
        let r20 = 2.0 * (x * z + y * w);
        let r21 = 2.0 * (y * z - x * w);
        let r22 = 1.0 - 2.0 * (x * x + y * y);
        // Column-major: each rotation column pre-scaled by the matching axis scale.
        [
            r00 * sx,
            r01 * sx,
            r02 * sx,
            0.0, // col 0 (X basis · sx)
            r10 * sy,
            r11 * sy,
            r12 * sy,
            0.0, // col 1 (Y basis · sy)
            r20 * sz,
            r21 * sz,
            r22 * sz,
            0.0, // col 2 (Z basis · sz)
            self.position[0],
            self.position[1],
            self.position[2],
            1.0, // col 3 (translation)
        ]
    }
}

/// A batch of stamp instances sharing one asset (`asset_path` → one
/// mesh/material handle in the app). Rendered as a single instanced draw. Groups
/// are keyed by asset so distinct models don't fight for one buffer.
#[derive(Debug, Clone, Default)]
pub struct InstanceBatch {
    /// The shared asset every instance in this batch draws (`blob://…glb`).
    pub asset_path: String,
    /// Per-instance transforms, contiguous for a single GPU upload.
    pub instances: Vec<StampInstanceData>,
}

impl InstanceBatch {
    /// A batch for one asset.
    pub fn new(asset_path: impl Into<String>) -> Self {
        Self {
            asset_path: asset_path.into(),
            instances: Vec::new(),
        }
    }

    /// Append an instance.
    pub fn push(&mut self, inst: StampInstanceData) {
        self.instances.push(inst);
    }

    /// Instance count in this batch.
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    /// Whether the batch is empty.
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Flattened column-major matrices for every instance — the raw per-instance
    /// buffer a custom instanced pipeline uploads (`len()` × 16 floats).
    pub fn instance_matrices(&self) -> Vec<[f32; 16]> {
        self.instances.iter().map(|i| i.to_matrix()).collect()
    }
}

/// Group instances by their asset path into per-asset [`InstanceBatch`]es
/// (stable order not guaranteed — one batch per distinct asset). The app turns
/// each batch into one instanced draw.
pub fn batch_by_asset<'a, I>(items: I) -> Vec<InstanceBatch>
where
    I: IntoIterator<Item = (&'a str, StampInstanceData)>,
{
    let mut by_asset: HashMap<&'a str, InstanceBatch> = HashMap::new();
    for (asset, inst) in items {
        by_asset
            .entry(asset)
            .or_insert_with(|| InstanceBatch::new(asset))
            .push(inst);
    }
    by_asset.into_values().collect()
}

// ---------------------------------------------------------------------------
// Spatial pick index (FR-4): uniform grid hash over the XZ ground plane.
// ---------------------------------------------------------------------------

/// Default grid cell size (meters). Sized so a typical stamp footprint lands in
/// ~1 cell; picking scans the query cell plus its 8 neighbours (a constant 3×3
/// window at the default radius), keeping picks O(1) amortized regardless of the
/// total stamp count.
pub const DEFAULT_CELL_SIZE_M: f32 = 4.0;

/// A uniform-grid spatial index over stamp positions on the XZ plane. Built once
/// per instance-set change; a viewport pick (ray → ground point) resolves the
/// nearest stamp within a radius without an O(n) sweep (FR-4 acceptance).
#[derive(Debug, Clone)]
pub struct StampSpatialIndex {
    cell_size: f32,
    /// Cell `(ix, iz)` → indices into the caller's instance slice.
    cells: HashMap<(i32, i32), Vec<usize>>,
    /// Parallel position cache (XZ used for distance; kept per instance index).
    positions: Vec<[f32; 3]>,
}

impl StampSpatialIndex {
    /// Build an index over `positions` (petal-local meters) with `cell_size`
    /// (clamped to a small positive minimum). Index `i` in a query result maps
    /// back to `positions[i]` — i.e. the stamp's instance index.
    pub fn build(positions: &[[f32; 3]], cell_size: f32) -> Self {
        let cell_size = if cell_size.is_finite() && cell_size > 1e-3 {
            cell_size
        } else {
            DEFAULT_CELL_SIZE_M
        };
        let mut cells: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (i, p) in positions.iter().enumerate() {
            cells.entry(cell_of(p, cell_size)).or_default().push(i);
        }
        Self {
            cell_size,
            cells,
            positions: positions.to_vec(),
        }
    }

    /// Number of indexed stamps.
    pub fn len(&self) -> usize {
        self.positions.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// Nearest stamp index to the ground point `(x, z)` within `max_radius`
    /// meters (XZ distance), or `None` if no stamp is that close. Scans only the
    /// grid cells overlapping the radius, so cost is bounded by local density,
    /// not the total count.
    pub fn pick_nearest(&self, x: f32, z: f32, max_radius: f32) -> Option<usize> {
        // Reject a non-positive OR NaN radius (a total check that matches the
        // intent of the old `!(max_radius > 0.0)` without the negated
        // partial-ord comparison clippy forbids).
        if self.positions.is_empty() || max_radius <= 0.0 || max_radius.is_nan() {
            return None;
        }
        let reach = (max_radius / self.cell_size).ceil() as i32;
        let (cx, cz) = cell_of(&[x, 0.0, z], self.cell_size);
        let r2 = max_radius * max_radius;
        let mut best: Option<(usize, f32)> = None;
        for dz in -reach..=reach {
            for dx in -reach..=reach {
                let Some(bucket) = self.cells.get(&(cx + dx, cz + dz)) else {
                    continue;
                };
                for &i in bucket {
                    let p = self.positions[i];
                    let (ddx, ddz) = (p[0] - x, p[2] - z);
                    let d2 = ddx * ddx + ddz * ddz;
                    if d2 <= r2 && best.is_none_or(|(_, bd)| d2 < bd) {
                        best = Some((i, d2));
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }
}

/// Grid cell coordinates of a point at `cell_size` (XZ plane; Y ignored).
fn cell_of(p: &[f32; 3], cell_size: f32) -> (i32, i32) {
    (
        (p[0] / cell_size).floor() as i32,
        (p[2] / cell_size).floor() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_instance_matrix_is_translation_only() {
        let inst = StampInstanceData::at([5.0, 1.0, -3.0], 0);
        let m = inst.to_matrix();
        // Column-major identity rotation/scale, translation in col 3.
        assert_eq!(&m[0..4], &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(&m[4..8], &[0.0, 1.0, 0.0, 0.0]);
        assert_eq!(&m[8..12], &[0.0, 0.0, 1.0, 0.0]);
        assert_eq!(&m[12..16], &[5.0, 1.0, -3.0, 1.0]);
    }

    #[test]
    fn scale_folds_into_matrix_basis() {
        let inst = StampInstanceData {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 3.0, 4.0],
            stamp_index: 1,
        };
        let m = inst.to_matrix();
        assert!((m[0] - 2.0).abs() < 1e-5, "sx on X basis");
        assert!((m[5] - 3.0).abs() < 1e-5, "sy on Y basis");
        assert!((m[10] - 4.0).abs() < 1e-5, "sz on Z basis");
    }

    #[test]
    fn yaw_90_rotation_maps_x_axis_to_minus_z() {
        // 90° about +Y (xyzw = 0, sin45, 0, cos45). The X basis column should
        // rotate toward -Z (right-handed), validating the quaternion→matrix math.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let inst = StampInstanceData {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, s, 0.0, s],
            scale: [1.0, 1.0, 1.0],
            stamp_index: 0,
        };
        let m = inst.to_matrix();
        // X basis column (col 0) ≈ (0, 0, -1).
        assert!(m[0].abs() < 1e-4, "col0.x ~ 0, got {}", m[0]);
        assert!((m[2] + 1.0).abs() < 1e-4, "col0.z ~ -1, got {}", m[2]);
    }

    #[test]
    fn batch_by_asset_groups_and_counts() {
        let items = vec![
            ("blob://tree.glb", StampInstanceData::at([0.0, 0.0, 0.0], 0)),
            ("blob://tree.glb", StampInstanceData::at([1.0, 0.0, 0.0], 1)),
            ("blob://lamp.glb", StampInstanceData::at([2.0, 0.0, 0.0], 0)),
        ];
        let mut batches = batch_by_asset(items);
        batches.sort_by(|a, b| a.asset_path.cmp(&b.asset_path));
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].asset_path, "blob://lamp.glb");
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].asset_path, "blob://tree.glb");
        assert_eq!(batches[1].len(), 2);
        // The flattened matrix buffer has one 16-float entry per instance.
        assert_eq!(batches[1].instance_matrices().len(), 2);
    }

    #[test]
    fn spatial_index_picks_the_nearest_stamp() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [10.2, 0.0, 0.1], // closest to the query below
            [50.0, 0.0, 50.0],
        ];
        let idx = StampSpatialIndex::build(&positions, DEFAULT_CELL_SIZE_M);
        assert_eq!(idx.len(), 4);
        let hit = idx.pick_nearest(10.15, 0.05, 2.0);
        assert_eq!(hit, Some(2), "must resolve the individual nearest stamp");
    }

    #[test]
    fn spatial_index_pick_respects_radius_and_empty() {
        let positions = vec![[0.0, 0.0, 0.0]];
        let idx = StampSpatialIndex::build(&positions, DEFAULT_CELL_SIZE_M);
        // Far outside the radius → no hit.
        assert_eq!(idx.pick_nearest(100.0, 100.0, 5.0), None);
        // Degenerate radius → no hit.
        assert_eq!(idx.pick_nearest(0.0, 0.0, 0.0), None);
        // Empty index → no hit.
        let empty = StampSpatialIndex::build(&[], DEFAULT_CELL_SIZE_M);
        assert!(empty.is_empty());
        assert_eq!(empty.pick_nearest(0.0, 0.0, 10.0), None);
    }

    #[test]
    fn spatial_index_degenerate_cell_size_falls_back() {
        // A zero/NaN cell size must not divide-by-zero or panic on build.
        let positions = vec![[1.0, 0.0, 1.0]];
        let idx = StampSpatialIndex::build(&positions, 0.0);
        assert_eq!(idx.pick_nearest(1.0, 1.0, 1.0), Some(0));
        let idx = StampSpatialIndex::build(&positions, f32::NAN);
        assert_eq!(idx.pick_nearest(1.0, 1.0, 1.0), Some(0));
    }

    #[test]
    fn bench_10k_build_and_pick() {
        // FR-4 budget guard: build + pick over TARGET_INSTANCE_CEILING stamps
        // stays correct (and, by grid-bucketing, sub-linear per pick). This is a
        // correctness+scale smoke test, not a timed criterion bench.
        let n = TARGET_INSTANCE_CEILING;
        let positions: Vec<[f32; 3]> = (0..n)
            .map(|i| {
                let f = i as f32;
                [f % 100.0, 0.0, (f / 100.0).floor()]
            })
            .collect();
        let idx = StampSpatialIndex::build(&positions, DEFAULT_CELL_SIZE_M);
        assert_eq!(idx.len(), n);
        // Query exactly onto the last stamp's cell; it must be resolved.
        let target = positions[n - 1];
        let hit = idx.pick_nearest(target[0], target[2], 1.0);
        assert!(hit.is_some(), "pick must resolve a stamp at 10k scale");
        // Instance buffer builds one matrix per stamp.
        let mut batch = InstanceBatch::new("blob://tree.glb");
        for (i, p) in positions.iter().enumerate() {
            batch.push(StampInstanceData::at(*p, i as u32));
        }
        assert_eq!(batch.instance_matrices().len(), n);
    }
}
