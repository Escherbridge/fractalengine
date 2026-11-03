//! D-74 residency ledger — the node + stamp-group half of the per-frame
//! pointer-set (fe-terrain's `TileBundleDemand` is the tile half): camera-forward
//! weighting (B2), enter/exit hysteresis, the GPU-pressure loop (B3), and the
//! `apply_node_residency` reconciler. See `fe-ui/src/AGENTS.md` §residency-ledger.

use std::collections::HashSet;

use bevy::prelude::*;
use fe_renderer::camera::OrbitCameraController;
use fe_runtime::diag15m::RenderPressure;

use super::path_asset_materialize::PathAssetCache;
use super::petal_respawn::NodeSpawner;
use super::spawn::{ResidencyBudget, ResidentNode, MAX_PETAL_NODES, RESIDENCY_TAPER_FACTOR};
use super::VerseManager;
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::plugin::SpawnedNodeMarker;

/// Camera-forward weight `w` in `d × (1 − w·max(0, cosθ))` (D-74 B2).
pub(crate) const FORWARD_WEIGHT: f32 = 0.5;

/// Newly-admitted nodes `apply_node_residency` spawns per frame; the rest
/// carry over as a backlog (no single-frame spawn spike).
pub(crate) const MAX_NODE_SPAWNS_PER_FRAME: usize = 256;

const GIB: u64 = 1 << 30;
/// Below this measured instance-buffer demand the horizon is untouched.
pub(crate) const PRESSURE_RAMP_START_BYTES: u64 = GIB;
/// End of the linear ramp, where the scale bottoms out at [`MIN_PRESSURE_SCALE`].
pub(crate) const PRESSURE_RAMP_END_BYTES: u64 = GIB + GIB / 2;
/// At or above this, admission halts (scale `0`) — 0.25 GiB under the 2 GiB limit.
pub(crate) const PRESSURE_HALT_BYTES: u64 = GIB + 3 * (GIB / 4);
/// Floor of the pressure ramp; also the exit-ring scale once admission halts.
pub(crate) const MIN_PRESSURE_SCALE: f32 = 0.25;
/// Per-update cap on how fast a pressure scale may recover (drops are instant).
pub(crate) const PRESSURE_RECOVERY_PER_UPDATE: f32 = 0.02;

/// Horizon multiplier for a measured GPU instance-buffer demand (D-74 B3):
/// `1.0` below 1 GiB, linear to `0.25` at 1.5 GiB, held to 1.75 GiB, then `0`.
pub fn radius_scale_for_pressure(bytes: u64) -> f32 {
    if bytes >= PRESSURE_HALT_BYTES {
        return 0.0;
    }
    if bytes <= PRESSURE_RAMP_START_BYTES {
        return 1.0;
    }
    if bytes >= PRESSURE_RAMP_END_BYTES {
        return MIN_PRESSURE_SCALE;
    }
    let t = (bytes - PRESSURE_RAMP_START_BYTES) as f64
        / (PRESSURE_RAMP_END_BYTES - PRESSURE_RAMP_START_BYTES) as f64;
    (1.0 - t * (1.0 - MIN_PRESSURE_SCALE as f64)) as f32
}

/// Next smoothed pressure scale: falls to `target` at once, climbs at most
/// [`PRESSURE_RECOVERY_PER_UPDATE`] per update (no evict/readmit oscillation).
pub(crate) fn step_pressure_scale(current: f32, target: f32) -> f32 {
    let target = if target.is_finite() {
        target.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let current = if current.is_finite() {
        current.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if target <= current {
        target
    } else {
        (current + PRESSURE_RECOVERY_PER_UPDATE).min(target)
    }
}

/// Camera-forward weighted distance (B2); `cos_to_forward` is clamped so only
/// what is in front is pulled closer.
pub(crate) fn weighted_distance(raw: f32, cos_to_forward: f32) -> f32 {
    raw * (1.0 - FORWARD_WEIGHT * cos_to_forward.clamp(0.0, 1.0))
}

/// Camera position + unit forward used to rank residency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CameraPose {
    pub position: Vec3,
    pub forward: Vec3,
}

impl CameraPose {
    fn is_finite(&self) -> bool {
        self.position.is_finite() && self.forward.is_finite()
    }
}

/// One pass's enter/exit radii (already scaled by GPU pressure).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Horizon {
    /// Enter at weighted distance ≤ this.
    pub enter: f32,
    /// Exit at weighted distance > this (weighted ≤ raw, so rotation never evicts inside `enter`).
    pub exit: f32,
    /// `false` while the mesh budget is exceeded or pressure halted admission.
    pub admit_new: bool,
}

impl Horizon {
    /// `None` (admit nothing) for a non-finite / non-positive `render_distance`.
    pub fn new(render_distance: f32, pressure_scale: f32, budget_exceeded: bool) -> Option<Self> {
        if !render_distance.is_finite() || render_distance <= 0.0 {
            return None;
        }
        let scale = if pressure_scale.is_finite() {
            pressure_scale.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(Self {
            enter: render_distance * scale,
            exit: render_distance * RESIDENCY_TAPER_FACTOR * scale.max(MIN_PRESSURE_SCALE),
            admit_new: scale > 0.0 && !budget_exceeded,
        })
    }
}

/// One ranked candidate: `tier` 0 = always-admit, 1 = inside the raw enter
/// radius, 2 = everything else; then weighted distance, then input order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    tier: u8,
    key: f32,
    index: usize,
}

/// Fills `out` with the input indices admitted this pass, nearest-first, at
/// most `cap`. Enter at weighted ≤ `enter`; stay while `previously_admitted`
/// and weighted ≤ `exit`. `camera: None` = every distance `0`;
/// a non-finite camera or `horizon: None` admits nothing. Pure; see AGENTS.md
/// §residency-ledger.
pub(crate) fn select_admitted<'n>(
    camera: Option<CameraPose>,
    nodes: impl Iterator<Item = (&'n str, [f32; 3])>,
    previously_admitted: &HashSet<String>,
    always_admit: Option<&str>,
    horizon: Option<Horizon>,
    cap: usize,
    scratch: &mut Vec<Candidate>,
    out: &mut Vec<usize>,
) {
    scratch.clear();
    out.clear();
    let Some(horizon) = horizon else {
        return;
    };
    if camera.is_some_and(|pose| !pose.is_finite()) || cap == 0 {
        return;
    }
    let forward = camera.map(|pose| pose.forward.normalize_or_zero());
    for (index, (id, position)) in nodes.enumerate() {
        let position = Vec3::from_array(position);
        if always_admit == Some(id) {
            scratch.push(Candidate {
                tier: 0,
                key: 0.0,
                index,
            });
            continue;
        }
        if !position.is_finite() {
            continue;
        }
        let (raw, weighted) = match (camera, forward) {
            (Some(pose), Some(forward)) => {
                let offset = position - pose.position;
                let raw = offset.length();
                let cos = if raw > f32::EPSILON {
                    offset.dot(forward) / raw
                } else {
                    1.0
                };
                (raw, weighted_distance(raw, cos))
            }
            _ => (0.0, 0.0),
        };
        let entering = horizon.admit_new && weighted <= horizon.enter;
        // Weighted ≤ raw, so this keeps everything inside raw `exit` AND gives a
        // forward node the same 1.5× band (no enter/exit flap on camera jitter).
        let retained = previously_admitted.contains(id) && weighted <= horizon.exit;
        if !(entering || retained) {
            continue;
        }
        scratch.push(Candidate {
            tier: if raw <= horizon.enter { 1 } else { 2 },
            key: weighted,
            index,
        });
    }
    scratch.sort_unstable_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then(a.key.total_cmp(&b.key))
            .then(a.index.cmp(&b.index))
    });
    out.extend(scratch.iter().take(cap).map(|c| c.index));
}

/// Stamp-group residency (raw distance to the track's nearest point; no
/// forward weighting): enter ≤ `enter`, stay while ≤ `exit`.
pub(crate) fn stamp_group_admitted(
    previously: bool,
    nearest: f32,
    horizon: Option<Horizon>,
) -> bool {
    let Some(horizon) = horizon else {
        return false;
    };
    if !nearest.is_finite() || nearest < 0.0 {
        return false;
    }
    (horizon.admit_new && nearest <= horizon.enter)
        || (previously && (nearest <= horizon.exit || nearest <= horizon.enter))
}

/// Raw distance from `from` to the polyline `points` (segment projection);
/// `INFINITY` for an empty or all-non-finite polyline.
pub(crate) fn distance_to_polyline(from: Vec3, points: &[[f32; 3]]) -> f32 {
    let mut best = f32::INFINITY;
    let mut previous: Option<Vec3> = None;
    for point in points {
        let point = Vec3::from_array(*point);
        if !point.is_finite() {
            previous = None;
            continue;
        }
        let distance = match previous {
            Some(start) => {
                let segment = point - start;
                let length_squared = segment.length_squared();
                let t = if length_squared > f32::EPSILON {
                    ((from - start).dot(segment) / length_squared).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                from.distance(start + segment * t)
            }
            None => from.distance(point),
        };
        best = best.min(distance);
        previous = Some(point);
    }
    best
}

/// The D-74 pointer-set for scene nodes and path-asset stamp groups of the
/// active petal. Written only by `update_residency_ledger`; read by every
/// spawner. See `fe-ui/src/AGENTS.md` §residency-ledger.
#[derive(Resource)]
pub struct ResidencyLedger {
    admitted_nodes: HashSet<String>,
    admitted_stamp_groups: HashSet<String>,
    petal_id: Option<String>,
    node_epoch: u64,
    stamp_epoch: u64,
    camera: Option<Vec3>,
    pressure_scale: f32,
    /// This frame's admitted active-petal node indices, nearest-first.
    ranked: Vec<usize>,
    scratch_candidates: Vec<Candidate>,
    scratch_stamp_groups: HashSet<String>,
}

impl Default for ResidencyLedger {
    fn default() -> Self {
        Self {
            admitted_nodes: HashSet::new(),
            admitted_stamp_groups: HashSet::new(),
            petal_id: None,
            node_epoch: 0,
            stamp_epoch: 0,
            camera: None,
            pressure_scale: 1.0,
            ranked: Vec::new(),
            scratch_candidates: Vec::new(),
            scratch_stamp_groups: HashSet::new(),
        }
    }
}

impl ResidencyLedger {
    /// Whether node `node_id` of the active petal is resident.
    pub fn admits_node(&self, node_id: &str) -> bool {
        self.admitted_nodes.contains(node_id)
    }

    /// Whether track `track_id`'s stamp group is resident.
    pub fn admits_stamp_group(&self, track_id: &str) -> bool {
        self.admitted_stamp_groups.contains(track_id)
    }

    /// Resident node count (readout).
    pub fn node_count(&self) -> usize {
        self.admitted_nodes.len()
    }

    /// Resident stamp-group count (readout).
    pub fn stamp_group_count(&self) -> usize {
        self.admitted_stamp_groups.len()
    }

    /// Bumped whenever the admitted node set changes.
    pub fn node_epoch(&self) -> u64 {
        self.node_epoch
    }

    /// Bumped whenever the admitted stamp-group set changes.
    pub fn stamp_epoch(&self) -> u64 {
        self.stamp_epoch
    }

    /// Camera position of the last ledger pass (`None` = no camera).
    pub fn camera_position(&self) -> Option<Vec3> {
        self.camera
    }

    /// Current smoothed GPU-pressure horizon scale.
    pub fn pressure_scale(&self) -> f32 {
        self.pressure_scale
    }

    /// Admitted active-petal node indices (into `PetalEntry::nodes`),
    /// nearest-first, as of this frame's pass.
    pub(crate) fn ranked_indices(&self) -> &[usize] {
        &self.ranked
    }

    /// Recompute both halves for `petal_nodes` / `stamp_tracks`; returns whether
    /// either admitted set changed (epochs bumped).
    #[allow(clippy::too_many_arguments)]
    fn update<'n, 't>(
        &mut self,
        petal_id: Option<&str>,
        petal_nodes: &'n [super::NodeEntry],
        stamp_tracks: impl Iterator<Item = (&'t String, &'t [[f32; 3]])> + Clone,
        camera: Option<CameraPose>,
        always_admit: Option<&str>,
        render_distance: f32,
        budget_exceeded: bool,
        cap: usize,
    ) -> bool {
        if self.petal_id.as_deref() != petal_id {
            self.petal_id = petal_id.map(str::to_string);
            self.admitted_nodes.clear();
            self.admitted_stamp_groups.clear();
            self.node_epoch += 1;
            self.stamp_epoch += 1;
        }
        self.camera = camera.map(|pose| pose.position);
        let horizon = Horizon::new(render_distance, self.pressure_scale, budget_exceeded);
        select_admitted(
            camera,
            petal_nodes.iter().map(|n| (n.id.as_str(), n.position)),
            &self.admitted_nodes,
            always_admit,
            horizon,
            cap,
            &mut self.scratch_candidates,
            &mut self.ranked,
        );
        let mut changed = false;
        let nodes_changed = self.ranked.len() != self.admitted_nodes.len()
            || self
                .ranked
                .iter()
                .any(|&i| !self.admitted_nodes.contains(&petal_nodes[i].id));
        if nodes_changed {
            self.admitted_nodes.clear();
            for &i in &self.ranked {
                self.admitted_nodes.insert(petal_nodes[i].id.clone());
            }
            self.node_epoch += 1;
            changed = true;
        }

        let camera_position = camera.map(|pose| pose.position);
        let nearest = |points: &[[f32; 3]]| match camera_position {
            Some(position) => distance_to_polyline(position, points),
            None if points.is_empty() => f32::INFINITY,
            None => 0.0,
        };
        let mut admitted_count = 0usize;
        let mut groups_changed = false;
        for (track_id, points) in stamp_tracks.clone() {
            let was = self.admitted_stamp_groups.contains(track_id);
            let now = stamp_group_admitted(was, nearest(points), horizon);
            admitted_count += usize::from(now);
            if now != was {
                groups_changed = true;
                break;
            }
        }
        if groups_changed || admitted_count != self.admitted_stamp_groups.len() {
            self.scratch_stamp_groups.clear();
            for (track_id, points) in stamp_tracks {
                let was = self.admitted_stamp_groups.contains(track_id);
                if stamp_group_admitted(was, nearest(points), horizon) {
                    self.scratch_stamp_groups.insert(track_id.clone());
                }
            }
            if self.scratch_stamp_groups != self.admitted_stamp_groups {
                std::mem::swap(
                    &mut self.scratch_stamp_groups,
                    &mut self.admitted_stamp_groups,
                );
                self.stamp_epoch += 1;
                changed = true;
            }
        }
        changed
    }
}

/// Per-frame ledger pass (D-74 "first ahead of every spawner"): ranks the
/// active petal's nodes and stamp groups against the orbit camera. A
/// non-finite camera freezes the ledger for the frame. Chained after
/// `apply_db_results`, before `respawn_on_petal_change`.
#[allow(clippy::too_many_arguments)]
pub(super) fn update_residency_ledger(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    path_cache: Res<PathAssetCache>,
    node_mgr: Option<Res<NodeManager>>,
    cameras: Query<&GlobalTransform, With<OrbitCameraController>>,
    pressure: Option<Res<RenderPressure>>,
    residency: ResidencyBudget,
    mut ledger: ResMut<ResidencyLedger>,
) {
    let camera = cameras.single().ok().map(|transform| CameraPose {
        position: transform.translation(),
        forward: transform.forward().as_vec3(),
    });
    if camera.is_some_and(|pose| !pose.is_finite()) {
        return;
    }
    let target = pressure
        .as_deref()
        .map_or(1.0, |p| radius_scale_for_pressure(p.bytes()));
    let petal_id = nav.active_petal_id.as_deref();
    let petal = petal_id.and_then(|id| verse_mgr.find_petal(id));
    let nodes: &[super::NodeEntry] = petal.map_or(&[], |p| p.nodes.as_slice());
    let render_distance = match (petal_id, petal) {
        (Some(id), Some(p)) => residency.render_distance_for(id, p.render_distance),
        (Some(id), None) => residency.render_distance_for(id, None),
        _ => 0.0,
    };
    let selected = node_mgr
        .as_deref()
        .and_then(|m| m.selected.as_ref())
        .map(|s| s.node_id.as_str());
    let cap = residency.settings.entity_cap.min(MAX_PETAL_NODES);
    let budget_exceeded = residency.mesh_budget.exceeded;
    let stamp_tracks = path_cache
        .iter()
        .filter(move |(_, entry)| Some(entry.petal_id.as_str()) == petal_id)
        .map(|(track_id, entry)| (track_id, entry.points.as_slice()));

    let changed = {
        let ledger = ledger.bypass_change_detection();
        ledger.pressure_scale = step_pressure_scale(ledger.pressure_scale, target);
        ledger.update(
            petal_id,
            nodes,
            stamp_tracks,
            camera,
            selected,
            render_distance,
            budget_exceeded,
            cap,
        )
    };
    if changed {
        ledger.set_changed();
    }
}

/// Reconciles live tree-node entities (`ResidentNode`) of the active petal
/// with the ledger: despawns evicted ones, spawns newly-admitted ones
/// nearest-first (≤ [`MAX_NODE_SPAWNS_PER_FRAME`], backlog carried). Runs on an
/// epoch change, a backlog, or a `ResidentNode` spawned elsewhere this frame.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_node_residency(
    nav: Res<NavigationManager>,
    verse_mgr: Res<VerseManager>,
    ledger: Res<ResidencyLedger>,
    live: Query<(Entity, &SpawnedNodeMarker), With<ResidentNode>>,
    added: Query<(), Added<ResidentNode>>,
    mut last_epoch: Local<Option<u64>>,
    mut backlog: Local<bool>,
    mut commands: Commands,
    mut spawner: NodeSpawner,
) {
    let epoch = ledger.node_epoch();
    if *last_epoch == Some(epoch) && !*backlog && added.is_empty() && !spawner.projection_changed()
    {
        return;
    }
    *last_epoch = Some(epoch);
    *backlog = false;
    let Some(petal_id) = nav.active_petal_id.as_deref() else {
        return;
    };

    // Kept ids first: despawns are deferred, so the query must not be read
    // for "is it live?" after they are issued (root AGENTS.md "Commands").
    let kept_node_ids: HashSet<&str> = live
        .iter()
        .filter(|(_, m)| m.petal_id == petal_id && ledger.admits_node(&m.node_id))
        .map(|(_, m)| m.node_id.as_str())
        .collect();
    for (entity, marker) in live.iter() {
        if marker.petal_id == petal_id && !ledger.admits_node(&marker.node_id) {
            commands.entity(entity).try_despawn();
        }
    }

    let Some(petal) = verse_mgr.find_petal(petal_id) else {
        return;
    };
    let mut spawned = 0usize;
    for &index in ledger.ranked_indices() {
        let Some(node) = petal.nodes.get(index) else {
            continue;
        };
        if !ledger.admits_node(&node.id) || kept_node_ids.contains(node.id.as_str()) {
            continue;
        }
        if spawned == MAX_NODE_SPAWNS_PER_FRAME {
            *backlog = true;
            break;
        }
        if spawner.spawn(&mut commands, node, petal_id) {
            spawned += 1;
        } else if spawner.classification_pending(node) || !spawner.is_stamp_projection(node) {
            *backlog = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use fe_sdk::path_asset::{PathAssetDescriptor, SpacingMode};

    use super::*;
    use crate::verse_manager::spawn::PathAssetInstance;
    use crate::verse_manager::{
        FractalEntry, NodeEntry, PetalEntry, PrimitiveDescriptorCache, PrimitiveMaterialAssets,
        TextureRegistryRes, VerseEntry,
    };

    const R: f32 = 10.0;

    fn pose(position: [f32; 3], forward: [f32; 3]) -> Option<CameraPose> {
        Some(CameraPose {
            position: Vec3::from_array(position),
            forward: Vec3::from_array(forward),
        })
    }

    fn select(
        camera: Option<CameraPose>,
        nodes: &[(&str, [f32; 3])],
        prev: &[&str],
        horizon: Option<Horizon>,
        cap: usize,
    ) -> Vec<String> {
        let prev: HashSet<String> = prev.iter().map(|s| s.to_string()).collect();
        let (mut scratch, mut out) = (Vec::new(), Vec::new());
        select_admitted(
            camera,
            nodes.iter().copied(),
            &prev,
            None,
            horizon,
            cap,
            &mut scratch,
            &mut out,
        );
        out.iter().map(|&i| nodes[i].0.to_string()).collect()
    }

    fn full() -> Option<Horizon> {
        Horizon::new(R, 1.0, false)
    }

    // --- pure ledger math (table-driven) ---

    #[test]
    fn admission_is_ranked_nearest_first() {
        let nodes = [
            ("far", [9.0, 0.0, 0.0]),
            ("near", [1.0, 0.0, 0.0]),
            ("mid", [5.0, 0.0, 0.0]),
        ];
        // Forward ⟂ the line, so weighted == raw.
        let got = select(pose([0.0; 3], [0.0, -1.0, 0.0]), &nodes, &[], full(), 10);
        assert_eq!(got, ["near", "mid", "far"]);
    }

    #[test]
    fn forward_weighting_pulls_in_what_is_ahead_only() {
        // Raw 18 > r: ahead → weighted 9 (enters); behind → 18 (does not).
        let nodes = [("ahead", [0.0, 0.0, -18.0]), ("behind", [0.0, 0.0, 18.0])];
        let looking_forward = pose([0.0; 3], [0.0, 0.0, -1.0]);
        assert_eq!(select(looking_forward, &nodes, &[], full(), 10), ["ahead"]);
        let looking_back = pose([0.0; 3], [0.0, 0.0, 1.0]);
        assert_eq!(select(looking_back, &nodes, &[], full(), 10), ["behind"]);
        let table = [
            (10.0, 1.0, 5.0),
            (10.0, 0.0, 10.0),
            (10.0, -1.0, 10.0),
            (10.0, 0.5, 7.5),
        ];
        for (raw, cos, expected) in table {
            assert_eq!(weighted_distance(raw, cos), expected, "raw {raw} cos {cos}");
        }
    }

    #[test]
    fn enter_exit_band_keeps_only_previously_admitted() {
        let camera = pose([0.0; 3], [0.0, -1.0, 0.0]);
        // (distance, previously admitted, admitted now)
        let table = [
            (9.0, false, true),
            (10.0, false, true),
            (12.0, false, false),
            (12.0, true, true),
            (15.0, true, true),
            (15.5, true, false),
            (40.0, true, false),
        ];
        for (distance, previously, expected) in table {
            let nodes = [("n", [distance, 0.0, 0.0])];
            let prev: &[&str] = if previously { &["n"] } else { &[] };
            assert_eq!(
                select(camera, &nodes, prev, full(), 10) == ["n"],
                expected,
                "d {distance} prev {previously}"
            );
        }
    }

    /// A node straight ahead enters at raw 2r (weighted r); it must keep the
    /// same 1.5× band before it leaves, or camera jitter at 2r flaps it.
    #[test]
    fn a_node_straight_ahead_keeps_a_hysteresis_band() {
        let camera = pose([0.0; 3], [1.0, 0.0, 0.0]);
        // (raw distance ahead, previously admitted, admitted now)
        let table = [
            (20.0, false, true),
            (21.0, false, false),
            (21.0, true, true),
            (29.0, true, true),
            (31.0, true, false),
        ];
        for (distance, previously, expected) in table {
            let nodes = [("n", [distance, 0.0, 0.0])];
            let prev: &[&str] = if previously { &["n"] } else { &[] };
            assert_eq!(
                select(camera, &nodes, prev, full(), 10) == ["n"],
                expected,
                "ahead d {distance} prev {previously}"
            );
        }
    }

    #[test]
    fn cap_saturation_keeps_the_nearest_and_inside_r_first() {
        let nodes: Vec<(String, [f32; 3])> = (0..20)
            .map(|i| (format!("n{i:02}"), [i as f32, 0.0, 0.0]))
            .collect();
        let nodes: Vec<(&str, [f32; 3])> = nodes.iter().map(|(id, p)| (id.as_str(), *p)).collect();
        let camera = pose([0.0; 3], [0.0, -1.0, 0.0]);
        assert_eq!(
            select(camera, &nodes, &[], full(), 3),
            ["n00", "n01", "n02"]
        );
        assert!(select(camera, &nodes, &[], full(), 0).is_empty());
        // A band (retained) node never outranks one inside r, even when the
        // inside node is weighted farther.
        let looking_at_band = pose([0.0; 3], [1.0, 0.0, 0.0]);
        let mixed = [("inside", [0.0, 0.0, 9.0]), ("band", [14.0, 0.0, 0.0])];
        assert_eq!(
            select(looking_at_band, &mixed, &["band"], full(), 1),
            ["inside"]
        );
    }

    #[test]
    fn invalid_inputs_admit_nothing() {
        let nodes = [("n", [1.0, 0.0, 0.0])];
        let camera = pose([0.0; 3], [0.0, -1.0, 0.0]);
        let nan_camera = pose([f32::NAN, 0.0, 0.0], [0.0, -1.0, 0.0]);
        assert!(select(nan_camera, &nodes, &["n"], full(), 10).is_empty());
        for r in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert!(select(camera, &nodes, &["n"], Horizon::new(r, 1.0, false), 10).is_empty());
        }
        let nan_node = [("n", [f32::NAN, 0.0, 0.0])];
        assert!(select(camera, &nan_node, &["n"], full(), 10).is_empty());
        assert!(!stamp_group_admitted(true, f32::NAN, full()));
        assert!(!stamp_group_admitted(true, -1.0, full()));
        assert!(!stamp_group_admitted(true, 1.0, None));
    }

    #[test]
    fn no_camera_admits_by_cap_in_tree_order() {
        let nodes = [("a", [500.0, 0.0, 0.0]), ("b", [-900.0, 0.0, 0.0])];
        assert_eq!(select(None, &nodes, &[], full(), 10), ["a", "b"]);
        assert_eq!(select(None, &nodes, &[], full(), 1), ["a"]);
    }

    #[test]
    fn budget_or_halted_pressure_freezes_admission_but_keeps_the_ring() {
        let camera = pose([0.0; 3], [0.0, -1.0, 0.0]);
        let nodes = [("old", [12.0, 0.0, 0.0]), ("new", [2.0, 0.0, 0.0])];
        let exceeded = Horizon::new(R, 1.0, true);
        assert_eq!(select(camera, &nodes, &["old"], exceeded, 10), ["old"]);
        let halted = Horizon::new(R, 0.0, false).unwrap();
        assert!(!halted.admit_new);
        assert_eq!(halted.exit, R * RESIDENCY_TAPER_FACTOR * MIN_PRESSURE_SCALE);
    }

    #[test]
    fn pressure_scale_table() {
        let table = [
            (0, 1.0),
            (GIB / 2, 1.0),
            (GIB, 1.0),
            (GIB + GIB / 4, 0.625),
            (GIB + GIB / 2, 0.25),
            (GIB + GIB / 2 + GIB / 8, 0.25),
            (PRESSURE_HALT_BYTES - 1, 0.25),
            (PRESSURE_HALT_BYTES, 0.0),
            (2 * GIB, 0.0),
            (u64::MAX, 0.0),
        ];
        for (bytes, expected) in table {
            let got = radius_scale_for_pressure(bytes);
            assert!(
                (got - expected).abs() < 1e-6,
                "{bytes} B → {got}, want {expected}"
            );
        }
        // Drops are immediate; recovery is rate-limited; NaN target halts.
        assert_eq!(step_pressure_scale(1.0, 0.25), 0.25);
        assert_eq!(step_pressure_scale(0.25, 0.0), 0.0);
        assert!((step_pressure_scale(0.0, 1.0) - PRESSURE_RECOVERY_PER_UPDATE).abs() < 1e-6);
        assert_eq!(step_pressure_scale(0.99, 1.0), 1.0);
        assert_eq!(step_pressure_scale(1.0, f32::NAN), 0.0);
    }

    #[test]
    fn polyline_distance_projects_onto_segments() {
        let line = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
        assert_eq!(distance_to_polyline(Vec3::new(5.0, 3.0, 0.0), &line), 3.0);
        assert_eq!(distance_to_polyline(Vec3::new(-4.0, 0.0, 0.0), &line), 4.0);
        assert_eq!(
            distance_to_polyline(Vec3::new(1.0, 0.0, 0.0), &[[4.0, 0.0, 0.0]]),
            3.0
        );
        assert_eq!(distance_to_polyline(Vec3::ZERO, &[]), f32::INFINITY);
    }

    // --- system flows (real chain over a Bevy App) ---

    /// Holds the DB command receiver so spawner sends never hit a closed channel.
    #[derive(Resource)]
    struct DbCommandSink(
        #[allow(dead_code)] crossbeam::channel::Receiver<fe_runtime::messages::DbCommand>,
    );

    fn node(id: String, position: [f32; 3]) -> NodeEntry {
        NodeEntry {
            transform: Default::default(),
            name: id.clone(),
            id,
            has_asset: false,
            position,
            webpage_url: None,
            asset_path: None,
        }
    }

    fn tree(nodes: Vec<NodeEntry>) -> VerseManager {
        VerseManager::from_verses(vec![VerseEntry {
            id: "v".into(),
            name: "V".into(),
            namespace_id: None,
            expanded: true,
            fractals: vec![FractalEntry {
                id: "f".into(),
                name: "F".into(),
                expanded: true,
                petals: vec![PetalEntry {
                    id: "p".into(),
                    name: "P".into(),
                    expanded: true,
                    render_distance: None,
                    nodes,
                }],
            }],
        }])
    }

    /// The residency-relevant slice of `VerseManagerPlugin`'s chain, in order.
    fn residency_app(nodes: Vec<NodeEntry>) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .init_asset::<bevy::scene::Scene>();
        let (db_tx, db_rx) = crossbeam::channel::unbounded();
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal("p");
        let settings = crate::settings::AppSettings {
            render_distance: R,
            ..Default::default()
        };
        let mut classified = PrimitiveDescriptorCache::default();
        for node in &nodes {
            classified.note_properties(&node.id, &serde_json::json!({}));
        }
        app.insert_resource(nav)
            .insert_resource(tree(nodes))
            .insert_resource(settings)
            .insert_resource(fe_runtime::app::DbCommandSender(db_tx))
            .insert_resource(DbCommandSink(db_rx))
            .init_resource::<crate::plugin::MeshInstanceBudget>()
            .init_resource::<crate::settings::PetalRenderOverride>()
            .init_resource::<NodeManager>()
            .init_resource::<RenderPressure>()
            .init_resource::<ResidencyLedger>()
            .init_resource::<PathAssetCache>()
            .init_resource::<super::super::PathAssetApplied>()
            .init_resource::<super::super::StampRenderIndex>()
            .insert_resource(classified)
            .init_resource::<TextureRegistryRes>()
            .init_resource::<PrimitiveMaterialAssets>()
            .init_resource::<crate::terrain_map::PetalMapState>()
            .init_resource::<crate::actions::asset::StampInteractionState>()
            .init_resource::<crate::gis::PathEditorState>()
            .add_systems(
                Update,
                (
                    super::super::petal_respawn::remove_standalone_stamp_projection,
                    update_residency_ledger,
                    super::super::petal_respawn::respawn_on_petal_change,
                    apply_node_residency,
                    super::super::primitive_materialize::materialize_cached_primitives,
                    super::super::path_asset_reconcile::reconcile_path_asset,
                    super::super::path_asset_materialize::materialize_path_assets,
                )
                    .chain(),
            );
        app.world_mut().spawn((
            OrbitCameraController::default(),
            Transform::default(),
            GlobalTransform::default(),
        ));
        app
    }

    /// 200 asset-less nodes one unit apart on the +X axis: `n000`..`n199`.
    fn line_app() -> App {
        residency_app(
            (0..200)
                .map(|i| node(format!("n{i:03}"), [i as f32, 0.0, 0.0]))
                .collect(),
        )
    }

    /// Move the orbit camera and run one frame.
    fn step_camera(app: &mut App, position: Vec3, forward: Vec3) {
        let up = if forward.normalize().y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let transform = Transform::from_translation(position).looking_to(forward, up);
        let mut cameras = app
            .world_mut()
            .query_filtered::<(&mut Transform, &mut GlobalTransform), With<OrbitCameraController>>(
            );
        for (mut local, mut global) in cameras.iter_mut(app.world_mut()) {
            *local = transform;
            *global = GlobalTransform::from(transform);
        }
        app.update();
    }

    /// Camera on the line looking straight down: every node is ⟂ forward, so
    /// weighted == raw and the expected set is pure hysteresis.
    fn step_line(app: &mut App, x: f32) {
        step_camera(app, Vec3::new(x, 0.0, 0.0), Vec3::NEG_Y);
    }

    fn live_nodes(app: &mut App) -> BTreeMap<String, Entity> {
        let mut query = app
            .world_mut()
            .query_filtered::<(Entity, &SpawnedNodeMarker), With<ResidentNode>>();
        query
            .iter(app.world())
            .map(|(entity, marker)| (marker.node_id.clone(), entity))
            .collect()
    }

    fn live_ids(app: &mut App) -> BTreeSet<String> {
        live_nodes(app).into_keys().collect()
    }

    /// Spec model: {within r} ∪ {previously admitted within 1.5 r}.
    fn expected_line(previous: &BTreeSet<String>, x: f32) -> BTreeSet<String> {
        (0..200)
            .filter(|&i| {
                let id = format!("n{i:03}");
                let distance = (i as f32 - x).abs();
                distance <= R || (previous.contains(&id) && distance <= R * 1.5)
            })
            .map(|i| format!("n{i:03}"))
            .collect()
    }

    fn ids(range: std::ops::RangeInclusive<usize>) -> BTreeSet<String> {
        range.map(|i| format!("n{i:03}")).collect()
    }

    #[test]
    fn moving_camera_streams_nodes_in_and_out_with_hysteresis() {
        let mut app = line_app();
        let mut expected = BTreeSet::new();
        // Fly the whole line: the live set tracks the spec model at every step.
        for step in 0..=39 {
            let x = step as f32 * 5.0;
            step_line(&mut app, x);
            expected = expected_line(&expected, x);
            assert_eq!(live_ids(&mut app), expected, "camera at x={x}");
        }
        assert!(
            live_ids(&mut app).len() < 40,
            "far nodes were despawned, not accumulated"
        );

        // Oscillate ±0.2 r around x=100.5 (band boundaries fall between nodes):
        // after one warm-up sweep, zero spawns or despawns.
        for x in [100.5, 98.5, 102.5] {
            step_line(&mut app, x);
            expected = expected_line(&expected, x);
        }
        let settled = live_nodes(&mut app);
        assert_eq!(settled.keys().cloned().collect::<BTreeSet<_>>(), expected);
        for _ in 0..10 {
            for x in [98.5, 100.5, 102.5, 100.5] {
                step_line(&mut app, x);
                assert_eq!(
                    live_nodes(&mut app),
                    settled,
                    "oscillation at x={x} must not churn entities"
                );
            }
        }

        // Flying back re-admits what was evicted.
        step_line(&mut app, 0.0);
        assert_eq!(live_ids(&mut app), ids(0..=10));
    }

    #[test]
    fn the_selected_node_is_never_evicted() {
        let mut app = line_app();
        step_line(&mut app, 0.0);
        let selected = live_nodes(&mut app)["n005"];
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(selected, "n005");
        step_line(&mut app, 150.0);
        let live = live_nodes(&mut app);
        assert_eq!(live.get("n005"), Some(&selected), "edited node stays put");
        assert!(!live.contains_key("n004"));
        app.world_mut().resource_mut::<NodeManager>().deselect();
        step_line(&mut app, 150.0);
        assert!(!live_ids(&mut app).contains("n005"));
    }

    #[test]
    fn rotating_in_place_never_evicts_inside_r() {
        // A ring inside r (8) and a ring beyond 1.5 r (18), 12 nodes each.
        let mut nodes = Vec::new();
        for k in 0..12 {
            let angle = k as f32 * std::f32::consts::TAU / 12.0;
            let dir = Vec3::new(angle.cos(), 0.0, angle.sin());
            nodes.push(node(format!("inner{k:02}"), (dir * 8.0).to_array()));
            nodes.push(node(format!("outer{k:02}"), (dir * 18.0).to_array()));
        }
        let mut app = residency_app(nodes);
        step_camera(&mut app, Vec3::ZERO, Vec3::X);
        let inner_entities = |live: &BTreeMap<String, Entity>| -> BTreeMap<String, Entity> {
            live.iter()
                .filter(|(id, _)| id.starts_with("inner"))
                .map(|(id, e)| (id.clone(), *e))
                .collect()
        };
        let first = inner_entities(&live_nodes(&mut app));
        assert_eq!(first.len(), 12, "the whole inner ring is resident");
        for k in 0..12 {
            let angle = k as f32 * std::f32::consts::TAU / 12.0;
            let forward = Vec3::new(angle.cos(), 0.0, angle.sin());
            step_camera(&mut app, Vec3::ZERO, forward);
            let live = live_nodes(&mut app);
            assert_eq!(inner_entities(&live), first, "rotation evicted inside r");
            assert!(
                live.contains_key(&format!("outer{k:02}")),
                "the far node straight ahead is pulled in by forward weighting"
            );
            let behind = format!("outer{:02}", (k + 6) % 12);
            assert!(!live.contains_key(&behind), "the far node behind stays out");
        }
    }

    #[test]
    fn mesh_budget_exceeded_freezes_admission_without_evicting() {
        let mut app = line_app();
        step_line(&mut app, 50.0);
        let before = live_nodes(&mut app);
        assert_eq!(
            before.keys().cloned().collect::<BTreeSet<_>>(),
            ids(40..=60)
        );

        app.world_mut()
            .resource_mut::<crate::plugin::MeshInstanceBudget>()
            .exceeded = true;
        step_line(&mut app, 58.0);
        let frozen = live_nodes(&mut app);
        assert_eq!(
            frozen.keys().cloned().collect::<BTreeSet<_>>(),
            ids(43..=60),
            "nothing new (61..=68) admitted; only beyond 1.5 r left"
        );
        for (id, entity) in &frozen {
            assert_eq!(before.get(id), Some(entity), "{id} kept its entity");
        }

        app.world_mut()
            .resource_mut::<crate::plugin::MeshInstanceBudget>()
            .exceeded = false;
        step_line(&mut app, 58.0);
        assert_eq!(live_ids(&mut app), ids(43..=68), "admission resumes");
    }

    #[test]
    fn gpu_pressure_shrinks_the_admitted_set() {
        let mut app = line_app();
        step_line(&mut app, 50.0);
        let baseline = live_ids(&mut app);
        assert_eq!(baseline, ids(40..=60));

        // 1.6 GiB → scale 0.25: enter 2.5, exit 3.75.
        app.world()
            .resource::<RenderPressure>()
            .store(GIB + GIB / 2 + GIB / 10);
        step_line(&mut app, 50.0);
        let shrunk = live_ids(&mut app);
        assert!(shrunk.len() < baseline.len());
        assert_eq!(shrunk, ids(47..=53));

        // 1.8 GiB → halted: nothing new even as the camera moves.
        app.world()
            .resource::<RenderPressure>()
            .store(GIB + 4 * (GIB / 5));
        step_line(&mut app, 52.0);
        let halted = live_ids(&mut app);
        assert!(halted.is_subset(&shrunk), "no new admissions under halt");
        assert_eq!(halted, ids(49..=53));
    }

    #[test]
    fn a_far_stamp_group_is_evicted_and_restamped_on_return() {
        let mut app = residency_app(vec![node("t1".into(), [100.0, 0.0, 0.0])]);
        let route = serde_json::json!({
            "path_asset": {"asset_path":"tree.glb", "spacing_mode":"fixed_spacing", "spacing_value":5.0, "count":0, "tangent_align":false},
            "gpx_points": [[100.0,0.0,0.0,0.0],[110.0,0.0,0.0,0.0]]
        });
        app.world_mut()
            .resource_mut::<PathAssetCache>()
            .note_properties("t1", Some("p"), &route);
        let stamps = |app: &mut App| -> BTreeSet<Entity> {
            let mut query = app
                .world_mut()
                .query_filtered::<Entity, With<PathAssetInstance>>();
            query.iter(app.world()).collect()
        };

        step_line(&mut app, 0.0);
        assert!(stamps(&mut app).is_empty(), "far group not stamped");

        step_line(&mut app, 104.0);
        let resident = stamps(&mut app);
        assert!(!resident.is_empty(), "group stamped on entry");

        step_line(&mut app, 112.0);
        assert_eq!(stamps(&mut app), resident, "no restamp while resident");

        step_line(&mut app, 0.0);
        assert!(stamps(&mut app).is_empty(), "evicted beyond 1.5 r");
        assert!(
            !app.world()
                .resource::<super::super::StampRenderIndex>()
                .tracks
                .contains_key("t1"),
            "pick data dropped with the group"
        );

        step_line(&mut app, 104.0);
        let returned = stamps(&mut app);
        assert_eq!(returned.len(), resident.len(), "restamped on return");
        assert!(returned.is_disjoint(&resident), "fresh entities");

        let mut markers = app.world_mut().query::<(Entity, &SpawnedNodeMarker)>();
        let selected = markers
            .iter(app.world())
            .find_map(|(entity, marker)| (marker.node_id == "t1::stamp::1").then_some(entity))
            .unwrap();
        app.world_mut()
            .resource_mut::<NodeManager>()
            .select(selected, "t1::stamp::1");
        let mut promoted = serde_json::json!({
            "node_kind":"stamp", "path_id":"t1", "owning_path_id":"t1", "instance_index":1
        });
        crate::actions::asset::hydrate_promoted_stamp(
            "t1#inst-1",
            &promoted,
            &mut app
                .world_mut()
                .resource_mut::<crate::actions::asset::StampInteractionState>(),
        );
        app.world_mut()
            .resource_mut::<NodeManager>()
            .bind_promoted_stamp(selected, "t1#inst-1");
        app.update();
        assert_eq!(
            stamps(&mut app),
            returned,
            "empty promotion metadata cannot replace live stamps"
        );
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(selected)
        );

        promoted["stamp.override.scale"] = serde_json::json!([2.0, 3.0, 4.0]);
        promoted["stamp.override.rotation"] = serde_json::json!([0.0, 1.0, 0.0, 0.0]);
        promoted["stamp.override.arc_m"] = serde_json::json!(7.5);
        crate::actions::asset::hydrate_promoted_stamp(
            "t1#inst-1",
            &promoted,
            &mut app
                .world_mut()
                .resource_mut::<crate::actions::asset::StampInteractionState>(),
        );
        app.world_mut()
            .resource_mut::<PrimitiveDescriptorCache>()
            .note_properties("t1#inst-1", &promoted);
        app.world_mut()
            .resource_mut::<VerseManager>()
            .upsert_node("p", node("t1#inst-1".into(), [0.0; 3]));
        app.update();
        assert_eq!(
            stamps(&mut app),
            returned,
            "confirmed overrides update the same rendered group"
        );
        let actual = app.world().get::<Transform>(selected).unwrap();
        assert_eq!(actual.translation, Vec3::new(107.5, 0.0, 0.0));
        assert_eq!(actual.scale, Vec3::new(2.0, 3.0, 4.0));
        assert!(actual
            .rotation
            .abs_diff_eq(Quat::from_rotation_y(std::f32::consts::PI), 0.0001));
        assert_eq!(
            app.world().resource::<NodeManager>().selected_entity(),
            Some(selected)
        );
        assert!(
            !live_nodes(&mut app).contains_key("t1#inst-1"),
            "typed stamp metadata never spawns an ordinary fallback"
        );
        let index = &app
            .world()
            .resource::<super::super::StampRenderIndex>()
            .tracks["t1"];
        assert_eq!(
            index.pick_stamp(107.5, 0.0, 0.01),
            Some(1),
            "pick projection follows the confirmed slide"
        );

        {
            let mut hierarchy = app.world_mut().resource_mut::<VerseManager>();
            hierarchy.find_verse_mut("v").unwrap().fractals[0]
                .petals
                .push(PetalEntry {
                    id: "other".into(),
                    ..Default::default()
                });
        }
        {
            let mut path = app
                .world_mut()
                .resource_mut::<crate::gis::PathEditorState>();
            path.start_editing("t1".into());
            path.points = crate::gis::decode_gpx_points(&route["gpx_points"]);
            path.edited_track_path_asset =
                Some(PathAssetDescriptor::from_json(&route["path_asset"]).unwrap());
        }
        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("other");
        app.update();
        app.update();
        assert!(
            stamps(&mut app).is_empty(),
            "an old editor cannot materialize its stamps into the new Petal"
        );
        assert_eq!(
            app.world()
                .resource::<PathAssetCache>()
                .get("t1")
                .unwrap()
                .petal_id,
            "p"
        );
        assert!(app
            .world()
            .resource::<super::super::StampRenderIndex>()
            .tracks
            .is_empty());

        app.world_mut()
            .resource_mut::<NavigationManager>()
            .navigate_to_petal("p");
        step_line(&mut app, 104.0);
        assert_eq!(
            stamps(&mut app).len(),
            returned.len(),
            "re-entry restores only the owning Petal's stamps"
        );
        assert!(!live_nodes(&mut app).contains_key("t1#inst-1"));
        let current = stamps(&mut app);
        let mut reflow = route.clone();
        reflow["path_asset"]["spacing_value"] = serde_json::json!(10.0);
        app.world_mut()
            .resource_mut::<crate::gis::PathEditorState>()
            .stop_editing();
        app.world_mut()
            .resource_mut::<PathAssetCache>()
            .note_properties("t1", Some("p"), &reflow);
        app.update();
        assert!(
            stamps(&mut app).is_disjoint(&current),
            "a genuine descriptor reflow replaces the old identities"
        );
    }
}
