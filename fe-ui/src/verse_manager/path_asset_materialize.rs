//! Petal-wide path-asset stamp materialization (FR-1/FR-2): a fe-ui-local
//! cache of every track's `path_asset` stamp descriptor + `gpx_points`, fed by
//! `GetNodeProperties` round-trips (petal-load batch + live edits), consumed by
//! the single `materialize_path_assets` system that does ALL stamp spawning.
//! Mirrors `primitive_materialize.rs`. See
//! `fe-ui/src/verse_manager/AGENTS.md` §path-asset-materialization.

use std::collections::HashMap;

use bevy::prelude::*;
use fe_sdk::path_asset::{PathAssetDescriptor, PATH_ASSET_PROPERTY_KEY};

use fe_renderer::instancing::{
    batch_by_asset, InstanceBatch, StampInstanceData, StampSpatialIndex, DEFAULT_CELL_SIZE_M,
};

use super::path_asset_reconcile::{
    points_fingerprint, sample_transforms, sanitize_world_scale, transform_at_arc_length,
};
use super::spawn::{spawn_stamped_entity, PathAssetInstance};
use crate::actions::asset::{StampInteractionState, StampOverride};
use crate::navigation_manager::NavigationManager;

// ---------------------------------------------------------------------------
// Petal-wide descriptor + points cache
// ---------------------------------------------------------------------------

/// One track's cached stamp inputs: which petal it lives in, its stamp
/// descriptor, its path points (petal-local), and a points fingerprint for
/// cheap change detection. See AGENTS.md §path-asset-materialization.
pub(super) struct PathAssetCacheEntry {
    pub(super) petal_id: String,
    pub(super) descriptor: PathAssetDescriptor,
    pub(super) points: Vec<[f32; 3]>,
    pub(super) fingerprint: u64,
}

/// Cache of `track_id → PathAssetCacheEntry`, fed by every `NodePropertiesLoaded`
/// broadcast that carries BOTH a `path_asset` descriptor AND `gpx_points`
/// (independent of the Paths-tab selection) plus the live-edit feeder
/// (`reconcile_path_asset`). The hierarchy payload carries no properties, so —
/// like `PrimitiveDescriptorCache` — petal-wide FR-1 runs on this cache instead
/// of a `NodeEntry` field. See AGENTS.md §path-asset-materialization.
#[derive(Resource, Default)]
pub struct PathAssetCache {
    entries: HashMap<String, PathAssetCacheEntry>,
}

impl PathAssetCache {
    /// Cached entry for a track, if its properties carried a stampable path.
    pub(super) fn get(&self, track_id: &str) -> Option<&PathAssetCacheEntry> {
        self.entries.get(track_id)
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &PathAssetCacheEntry)> {
        self.entries.iter()
    }

    /// Insert/replace a track's cache entry (used by the live-edit feeder).
    pub(super) fn upsert(
        &mut self,
        track_id: &str,
        petal_id: &str,
        descriptor: PathAssetDescriptor,
        points: Vec<[f32; 3]>,
        fingerprint: u64,
    ) {
        self.entries.insert(
            track_id.to_string(),
            PathAssetCacheEntry {
                petal_id: petal_id.to_string(),
                descriptor,
                points,
                fingerprint,
            },
        );
    }

    /// Ingest a `NodePropertiesLoaded` payload: cache the track's descriptor +
    /// points when it carries BOTH a `path_asset` and a non-empty `gpx_points`,
    /// else drop any stale entry. `petal_id` is the node's owning petal (from
    /// `VerseManager`); `None` (unknown node) can't be materialized, so it evicts.
    pub(super) fn note_properties(
        &mut self,
        node_id: &str,
        petal_id: Option<&str>,
        properties: &serde_json::Value,
    ) {
        let (Some(petal_id), Some(descriptor)) = (
            petal_id,
            properties
                .get(PATH_ASSET_PROPERTY_KEY)
                .and_then(|raw| PathAssetDescriptor::from_json(raw).ok()),
        ) else {
            self.invalidate(node_id);
            return;
        };
        // T2 curve-follow: keep the bezier handles and FLATTEN before caching so
        // stamps sit on the same curve the renderer draws (fe-terrain
        // `flatten_route` mirror). The fingerprint is over the FLATTENED
        // polyline, so a handle-only edit retriggers materialization.
        let rows = properties
            .get("gpx_points")
            .map(crate::gis::decode_gpx_points)
            .unwrap_or_default();
        if rows.is_empty() {
            // A `path_asset` with no path to stamp along → nothing to materialize.
            self.invalidate(node_id);
            return;
        }
        let anchors: Vec<crate::node_manager::curve::BezierAnchor> = rows
            .iter()
            .map(|r| (r.position, r.handle_in, r.handle_out))
            .collect();
        let points = crate::node_manager::curve::flatten_anchor_path(
            &anchors,
            crate::node_manager::curve::FLATTEN_SAMPLES_PER_SEGMENT,
        );
        let fingerprint = points_fingerprint(&points);
        self.upsert(node_id, petal_id, descriptor, points, fingerprint);
    }

    /// Drop a track's cached entry (path_asset deleted / node removed).
    pub(super) fn invalidate(&mut self, track_id: &str) {
        self.entries.remove(track_id);
    }

    /// Drop everything (database reset).
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
}

// ---------------------------------------------------------------------------
// Per-track applied-state gate (FR-2)
// ---------------------------------------------------------------------------

/// The `(descriptor, points-fingerprint, world-scale, overrides-fingerprint)`
/// last stamped for a track. See AGENTS.md §path-asset-materialization.
struct AppliedState {
    descriptor: PathAssetDescriptor,
    points_fingerprint: u64,
    world_scale_bits: u32,
    overrides_fingerprint: u64,
}

/// Per-track change gate (FR-2): replaces the old single-slot gate so live
/// editing of one track and petal-load materialization of another never stomp
/// each other's applied state. Cleared wholesale on petal change (see
/// `petal_respawn`) so re-entering a petal always restamps. Keyed on the stamp
/// *output* inputs — descriptor, points, and `world_scale` (metric spacing
/// depends on it) — so any of them changing re-stamps.
#[derive(Resource, Default)]
pub struct PathAssetApplied {
    applied: HashMap<String, AppliedState>,
}

impl PathAssetApplied {
    /// `true` when this track's cached inputs match what was last stamped — the
    /// materializer then skips the despawn/respawn entirely.
    fn matches(
        &self,
        track_id: &str,
        descriptor: &PathAssetDescriptor,
        points_fingerprint: u64,
        world_scale: f32,
        overrides_fingerprint: u64,
    ) -> bool {
        self.applied.get(track_id).is_some_and(|s| {
            &s.descriptor == descriptor
                && s.points_fingerprint == points_fingerprint
                && s.world_scale_bits == world_scale.to_bits()
                && s.overrides_fingerprint == overrides_fingerprint
        })
    }

    fn remember(
        &mut self,
        track_id: &str,
        descriptor: PathAssetDescriptor,
        points_fingerprint: u64,
        world_scale: f32,
        overrides_fingerprint: u64,
    ) {
        self.applied.insert(
            track_id.to_string(),
            AppliedState {
                descriptor,
                points_fingerprint,
                world_scale_bits: world_scale.to_bits(),
                overrides_fingerprint,
            },
        );
    }

    /// Forget a single track (its stamp group was despawned / cache evicted).
    pub(super) fn invalidate(&mut self, track_id: &str) {
        self.applied.remove(track_id);
    }

    /// Forget everything (petal change / database reset) so the next
    /// materializer pass restamps every active-petal track from the cache.
    pub(super) fn clear(&mut self) {
        self.applied.clear();
    }
}

// ---------------------------------------------------------------------------
// Per-track pick index + instanced-draw seam (T2 FR-4 / T4 HitTarget::Stamp)
// ---------------------------------------------------------------------------

/// Stable `SpawnedNodeMarker.node_id` for stamp instance `index` of `track_id`.
/// Single source of the `{track}::stamp::{index}` format; the T4 right-click
/// classifier parses it back via [`parse_stamp_marker_id`].
pub(crate) fn stamp_marker_id(track_id: &str, index: usize) -> String {
    format!("{track_id}::stamp::{index}")
}

/// Inverse of [`stamp_marker_id`]: `(track_id, stamp_index)`, or `None` when
/// `id` is not a stamp marker id.
pub(crate) fn parse_stamp_marker_id(id: &str) -> Option<(&str, usize)> {
    let (track, idx) = id.rsplit_once("::stamp::")?;
    if track.is_empty() {
        return None;
    }
    Some((track, idx.parse().ok()?))
}

/// One materialized track's render/pick data. `index.pick_nearest(x, z, r)`
/// returns the STAMP INDEX within this track (positions are fed in stamp
/// order); `batches` is the CPU seam for a future custom instanced pipeline —
/// today's draw rides Bevy auto-instancing over the shared GLB handles from
/// `spawn_stamped_entity`. See AGENTS.md §path-asset-materialization.
pub struct StampTrackRenderData {
    /// Owning petal (entries are cleared wholesale on petal change).
    pub petal_id: String,
    /// XZ uniform-grid pick index; a hit IS the stamp_index for this track.
    pub index: StampSpatialIndex,
    /// Per-asset instance batches with overrides folded in (one per track today).
    pub batches: Vec<InstanceBatch>,
}

/// `track_node_id → StampTrackRenderData`, rebuilt ONLY when a track (re)stamps
/// (inside the applied-gate — never per frame). T4's right-click classification
/// iterates active-petal tracks, calls `pick_nearest`, and yields
/// `(track_node_id, stamp_index)` for `HitTarget::Stamp`.
#[derive(Resource, Default)]
pub struct StampRenderIndex {
    pub tracks: HashMap<String, StampTrackRenderData>,
}

// ---------------------------------------------------------------------------
// Per-stamp override application (T2 FR-3)
// ---------------------------------------------------------------------------

/// Order-insensitive fingerprint of one track's sparse overrides so the applied
/// gate restamps when any override changes (input list is index-sorted by
/// `StampInteractionState::overrides_for_track`). FNV over presence-tagged bits.
fn overrides_fingerprint(overrides: &[(usize, StampOverride)]) -> u64 {
    const FNV_OFFSET: u64 = 1469598103934665603;
    const FNV_PRIME: u64 = 1099511628211;
    fn mix(hash: &mut u64, v: u64) {
        *hash ^= v;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
    let mut hash = FNV_OFFSET;
    for (index, ov) in overrides {
        mix(&mut hash, *index as u64 ^ 0x9E37_79B9_7F4A_7C15);
        if let Some(s) = ov.scale {
            mix(&mut hash, 1);
            for v in s {
                mix(&mut hash, v.to_bits() as u64);
            }
        }
        if let Some(r) = ov.rotation {
            mix(&mut hash, 2);
            for v in r {
                mix(&mut hash, v.to_bits() as u64);
            }
        }
        if let Some(a) = ov.arc_offset_m {
            mix(&mut hash, 3);
            mix(&mut hash, a.to_bits() as u64);
        }
    }
    mix(&mut hash, overrides.len() as u64);
    hash
}

/// Compose one stamp's final transform: the path-derived default plus the
/// sparse override. Precedence (T2 FR-3): `arc_offset_m` re-samples position +
/// yaw at the absolute arc length (meters, clamped) on the SAME polyline;
/// `rotation` (absolute quaternion xyzw) replaces the tangent-yaw rotation;
/// `scale` sets the transform scale. Position stays path-derived always.
fn stamp_transform(
    points: &[[f32; 3]],
    default_position: [f32; 3],
    default_yaw: f32,
    tangent_align: bool,
    ov: Option<&StampOverride>,
) -> Transform {
    let (mut position, mut yaw) = (default_position, default_yaw);
    if let Some(arc_m) = ov.and_then(|o| o.arc_offset_m) {
        (position, yaw) = transform_at_arc_length(points, arc_m);
    }
    let mut transform = Transform::from_xyz(position[0], position[1], position[2]);
    if tangent_align {
        transform.rotation = Quat::from_rotation_y(yaw);
    }
    if let Some(r) = ov.and_then(|o| o.rotation) {
        let q = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
        // A degenerate (zero-length) quat keeps the tangent default (no NaNs).
        if q.length_squared() > 1e-9 {
            transform.rotation = q.normalize();
        }
    }
    if let Some(s) = ov.and_then(|o| o.scale) {
        transform.scale = Vec3::from_array(s);
    }
    transform
}

// ---------------------------------------------------------------------------
// The single materializer (does ALL stamp spawning)
// ---------------------------------------------------------------------------

/// Petal-wide stamp budget: the per-track `MAX_STAMPS` (4096) still allows
/// tracks × 4096 stamps, so one materializer pass saturates here overall.
const MAX_STAMPS_PER_PETAL: usize = 65_536;

/// Take up to `requested` stamps from the shared petal budget; returns how many
/// may spawn. Pure so the saturation math is unit-testable.
fn take_stamp_budget(remaining: &mut usize, requested: usize) -> usize {
    let granted = requested.min(*remaining);
    *remaining -= granted;
    granted
}

/// A live stamp is orphaned when it belongs to the active petal but its source
/// track no longer has a cache entry there — the `path_asset` was deleted or the
/// track/node itself was removed (FR-4, `PathAssetCache::invalidate` on
/// `NodeDeleted`). The materializer then despawns it. Pure so the delete-cascade
/// contract is unit-testable without a live `App`.
fn stamp_is_orphaned(inst: &PathAssetInstance, active_petal: &str, cache: &PathAssetCache) -> bool {
    inst.petal_id == active_petal
        && cache
            .get(&inst.source_track_id)
            .map(|e| e.petal_id.as_str())
            != Some(active_petal)
}

/// Petal-wide path-asset materializer (FR-1): the ONLY system that spawns
/// stamp instances. For every cached track in the active petal it despawns the
/// old `PathAssetInstance` group and restamps whenever the descriptor, points,
/// `world_scale`, or per-stamp overrides changed (per-track gate). Orphaned
/// groups (track no longer cached — e.g. its `path_asset` was deleted) are torn
/// down. Runs when the cache, petal, `world_scale`, or stamp state changed;
/// chained after `respawn_on_petal_change` + `reconcile_path_asset` so it
/// observes their despawns, gate clears, and cache feeds. See AGENTS.md
/// §path-asset-materialization.
#[allow(clippy::too_many_arguments)]
pub(super) fn materialize_path_assets(
    nav: Res<NavigationManager>,
    cache: Res<PathAssetCache>,
    petal_map: Res<crate::terrain_map::PetalMapState>,
    asset_server: Res<AssetServer>,
    stamp_state: Res<StampInteractionState>,
    mut applied: ResMut<PathAssetApplied>,
    mut render_index: ResMut<StampRenderIndex>,
    mut last_petal: Local<Option<String>>,
    mut commands: Commands,
    existing: Query<(Entity, &PathAssetInstance)>,
    residency: super::spawn::ResidencyBudget,
) {
    let petal_changed = *last_petal != nav.active_petal_id;
    // T2 FR-3: `stamp_state` changes (override gestures) also re-run the pass;
    // the per-track overrides fingerprint keeps untouched tracks gated out.
    if !(cache.is_changed() || petal_map.is_changed() || petal_changed || stamp_state.is_changed())
    {
        return;
    }
    *last_petal = nav.active_petal_id.clone();
    if petal_changed {
        // Pick/instance data is petal-scoped; restamps below rebuild it.
        render_index.tracks.clear();
    }

    let Some(active_petal) = nav.active_petal_id.as_deref() else {
        return;
    };
    // FR-3: metric spacing conversion pulls the active petal's world scale.
    let world_scale = sanitize_world_scale(petal_map.world_scale as f32);

    // Orphan cleanup: tear down active-petal stamp groups whose track no longer
    // has a cache entry in this petal (path_asset deleted / node removed).
    for (entity, inst) in existing.iter() {
        if stamp_is_orphaned(inst, active_petal, &cache) {
            commands.entity(entity).despawn();
            applied.invalidate(&inst.source_track_id);
            render_index.tracks.remove(&inst.source_track_id);
        }
    }

    // Petal-wide stamp budget for this pass (D-74/D-78 residency ledger): the
    // configurable `stamp_ceiling` ranked by `render_distance`, hard-backstopped
    // by MAX_STAMPS_PER_PETAL. The app-wide mesh-budget gate (§mesh-budget)
    // still zeroes it so restamps stop growing a runaway scene. Distance 0 =
    // active petal is the near region; per-frame taper is the ledger's job.
    let mut stamp_budget = if residency.mesh_budget.exceeded {
        0
    } else {
        super::spawn::distance_ranked_allowance(
            0.0,
            residency.settings.render_distance,
            residency.settings.stamp_ceiling.min(MAX_STAMPS_PER_PETAL),
        )
    };

    // (Re)stamp every active-petal cached track whose stamp inputs changed.
    for (track_id, entry) in cache.iter() {
        if entry.petal_id != active_petal {
            continue;
        }
        // T2 FR-3: sparse per-stamp overrides are a stamp input like any other.
        let track_overrides = stamp_state.overrides_for_track(track_id);
        let ov_fingerprint = overrides_fingerprint(&track_overrides);
        if applied.matches(
            track_id,
            &entry.descriptor,
            entry.fingerprint,
            world_scale,
            ov_fingerprint,
        ) {
            continue; // nothing changed — skip the restamp entirely
        }
        let ov_by_index: HashMap<usize, &StampOverride> =
            track_overrides.iter().map(|(i, o)| (*i, o)).collect();

        // Despawn the previous group for this track before rebuild (deferred).
        for (entity, inst) in existing.iter() {
            if inst.source_track_id == *track_id && inst.petal_id == active_petal {
                commands.entity(entity).despawn();
            }
        }

        let mut samples = sample_transforms(&entry.points, &entry.descriptor, world_scale);
        let granted = take_stamp_budget(&mut stamp_budget, samples.len());
        if granted < samples.len() {
            bevy::log::warn!(
                "path-asset stamps saturated: track {} truncated {} → {} (petal cap {})",
                track_id,
                samples.len(),
                granted,
                MAX_STAMPS_PER_PETAL
            );
            samples.truncate(granted);
        }
        let stamped = samples.len();
        // FR-4 seam: per-stamp positions + instance data for the pick index and
        // the instanced-draw batch, built once per restamp (never per frame).
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(stamped);
        let mut instances: Vec<StampInstanceData> = Vec::with_capacity(stamped);
        for (i, (position, yaw)) in samples.into_iter().enumerate() {
            let transform = stamp_transform(
                &entry.points,
                position,
                yaw,
                entry.descriptor.tangent_align,
                ov_by_index.get(&i).copied(),
            );
            positions.push(transform.translation.to_array());
            instances.push(StampInstanceData {
                position: transform.translation.to_array(),
                rotation: transform.rotation.to_array(),
                scale: transform.scale.to_array(),
                stamp_index: i as u32,
            });
            let stamp_id = stamp_marker_id(track_id, i);
            spawn_stamped_entity(
                &mut commands,
                &asset_server,
                &stamp_id,
                track_id,
                active_petal,
                &stamp_id,
                transform,
                &entry.descriptor.asset_path,
            );
        }
        render_index.tracks.insert(
            track_id.clone(),
            StampTrackRenderData {
                petal_id: active_petal.to_string(),
                index: StampSpatialIndex::build(&positions, DEFAULT_CELL_SIZE_M),
                batches: batch_by_asset(
                    instances
                        .iter()
                        .map(|inst| (entry.descriptor.asset_path.as_str(), *inst)),
                ),
            },
        );
        applied.remember(
            track_id,
            entry.descriptor.clone(),
            entry.fingerprint,
            world_scale,
            ov_fingerprint,
        );
        bevy::log::debug!(
            "Materialized {} path-asset stamps of '{}' along track {} (petal {})",
            stamped,
            entry.descriptor.asset_path,
            track_id,
            active_petal
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_sdk::path_asset::SpacingMode;
    use serde_json::json;

    fn track_props() -> serde_json::Value {
        json!({
            "path_asset": {
                "asset_path": "blob://tree.glb",
                "spacing_mode": "fixed_spacing",
                "spacing_value": 5.0,
                "count": 0,
                "tangent_align": true
            },
            "gpx_points": [[0.0, 0.0, 0.0, 0.0], [10.0, 0.0, 0.0, 1.0]]
        })
    }

    fn sample_desc() -> PathAssetDescriptor {
        PathAssetDescriptor {
            asset_path: "blob://x.glb".to_string(),
            spacing_mode: SpacingMode::FixedSpacing,
            spacing_value: 5.0,
            count: 0,
            tangent_align: true,
        }
    }

    // --- stamp marker id round-trip (T4 right-click classification) ---

    #[test]
    fn stamp_marker_id_round_trips_through_parse() {
        let id = stamp_marker_id("track-1", 42);
        assert_eq!(id, "track-1::stamp::42");
        assert_eq!(parse_stamp_marker_id(&id), Some(("track-1", 42)));
        // A track id that itself contains the separator still parses (rsplit).
        let nested = stamp_marker_id("a::stamp::b", 3);
        assert_eq!(parse_stamp_marker_id(&nested), Some(("a::stamp::b", 3)));
    }

    #[test]
    fn parse_stamp_marker_id_rejects_non_stamp_ids() {
        assert_eq!(parse_stamp_marker_id("plain-node"), None);
        assert_eq!(parse_stamp_marker_id("t::stamp::"), None);
        assert_eq!(parse_stamp_marker_id("t::stamp::x"), None);
        assert_eq!(parse_stamp_marker_id("::stamp::3"), None);
    }

    // --- cache feed (FR-1) ---

    #[test]
    fn note_properties_caches_track_with_path_asset_and_points() {
        let mut cache = PathAssetCache::default();
        cache.note_properties("t1", Some("petal-1"), &track_props());
        let entry = cache.get("t1").expect("track cached");
        assert_eq!(entry.petal_id, "petal-1");
        assert_eq!(entry.descriptor.asset_path, "blob://tree.glb");
        assert_eq!(entry.points.len(), 2);
        assert_eq!(entry.points[0], [0.0, 0.0, 0.0]);
        assert_eq!(entry.points[1], [10.0, 0.0, 0.0]);
    }

    #[test]
    fn note_properties_skips_without_path_asset_or_points() {
        let mut cache = PathAssetCache::default();
        // No path_asset key → not a stamp track.
        cache.note_properties(
            "t1",
            Some("p1"),
            &json!({ "gpx_points": [[0.0,0.0,0.0,0.0]] }),
        );
        assert!(cache.get("t1").is_none());
        // path_asset but no gpx_points → nothing to stamp along.
        cache.note_properties(
            "t2",
            Some("p1"),
            &json!({ "path_asset": { "asset_path": "blob://x.glb" } }),
        );
        assert!(cache.get("t2").is_none());
        // path_asset with an EMPTY gpx_points array → nothing to stamp along.
        cache.note_properties(
            "t3",
            Some("p1"),
            &json!({ "path_asset": { "asset_path": "blob://x.glb" }, "gpx_points": [] }),
        );
        assert!(cache.get("t3").is_none());
    }

    #[test]
    fn note_properties_flattens_bezier_handles_and_refingerprints() {
        // T2 curve-follow: the same two anchors WITH a bezier handle must (a)
        // densify the cached polyline onto the curve and (b) change the
        // fingerprint — a handle-only edit MUST retrigger materialization.
        let mut cache = PathAssetCache::default();
        cache.note_properties("t1", Some("p1"), &track_props());
        let plain = cache.get("t1").expect("plain track cached");
        let (plain_len, plain_fp) = (plain.points.len(), plain.fingerprint);
        assert_eq!(plain_len, 2, "all-corner rows stay passthrough");

        // Same positions, but a 12-slot row carrying a non-zero out-handle.
        let handled = json!({
            "path_asset": {
                "asset_path": "blob://tree.glb",
                "spacing_mode": "fixed_spacing",
                "spacing_value": 5.0,
                "count": 0,
                "tangent_align": true
            },
            "gpx_points": [
                [0.0, 0.0, 0.0, 0.0,  0.0, 0.0, 0.0,  3.0, 0.0, 6.0,  1.0, 0.5],
                [10.0, 0.0, 0.0, 1.0]
            ]
        });
        cache.note_properties("t1", Some("p1"), &handled);
        let curved = cache.get("t1").expect("handled track cached");
        assert!(
            curved.points.len() > plain_len,
            "handled segment densifies onto the curve ({} pts)",
            curved.points.len()
        );
        assert_ne!(
            curved.fingerprint, plain_fp,
            "handle-only edit must change the fingerprint (restamp trigger)"
        );
        assert!(
            curved.points.iter().any(|p| p[2] > 0.5),
            "flattened points must bow toward the +Z handle"
        );
    }

    #[test]
    fn note_properties_requires_a_known_petal() {
        // No owning petal → the materializer can't place it, so don't cache.
        let mut cache = PathAssetCache::default();
        cache.note_properties("t1", None, &track_props());
        assert!(cache.get("t1").is_none());
    }

    #[test]
    fn note_properties_evicts_stale_entry_when_path_asset_removed() {
        let mut cache = PathAssetCache::default();
        cache.note_properties("t1", Some("p1"), &track_props());
        assert!(cache.get("t1").is_some());
        // A later load without a path_asset must evict the stale entry so the
        // materializer tears the stamps down.
        cache.note_properties(
            "t1",
            Some("p1"),
            &json!({ "gpx_points": [[0.0,0.0,0.0,0.0]] }),
        );
        assert!(cache.get("t1").is_none());
    }

    #[test]
    fn upsert_then_get_roundtrips_and_invalidate_clears() {
        let mut cache = PathAssetCache::default();
        cache.upsert(
            "t1",
            "p1",
            sample_desc(),
            vec![[0.0; 3], [1.0, 0.0, 0.0]],
            42,
        );
        assert_eq!(cache.get("t1").unwrap().fingerprint, 42);
        cache.invalidate("t1");
        assert!(cache.get("t1").is_none());
        cache.upsert("t2", "p1", sample_desc(), vec![[0.0; 3]], 1);
        cache.clear();
        assert!(cache.get("t2").is_none());
    }

    // --- delete cascade (FR-4 / NFR-5) ---

    #[test]
    fn deleted_track_cascades_to_stamps_and_is_idempotent() {
        let mut cache = PathAssetCache::default();
        cache.upsert(
            "t1",
            "p1",
            sample_desc(),
            vec![[0.0; 3], [1.0, 0.0, 0.0]],
            1,
        );
        let live = PathAssetInstance {
            source_track_id: "t1".to_string(),
            petal_id: "p1".to_string(),
        };
        // Still cached in the active petal → not an orphan; stamps stay.
        assert!(!stamp_is_orphaned(&live, "p1", &cache));
        // FR-4: NodeDeleted invalidates the cache entry → the stamp becomes an
        // orphan the materializer despawns.
        cache.invalidate("t1");
        assert!(stamp_is_orphaned(&live, "p1", &cache));
        // NFR-5: a repeated NodeDeleted for the same id is a no-op — still orphaned,
        // no churn (the entity is already gone from the query, so no double despawn).
        cache.invalidate("t1");
        assert!(stamp_is_orphaned(&live, "p1", &cache));
        // NFR-5: deleting a track that never stamped is a no-op (absent key).
        cache.invalidate("never-stamped");
        // A stamp owned by a different petal is never touched by this pass.
        let other_petal = PathAssetInstance {
            source_track_id: "t1".to_string(),
            petal_id: "p2".to_string(),
        };
        assert!(!stamp_is_orphaned(&other_petal, "p1", &cache));
    }

    // --- petal-wide stamp budget ---

    #[test]
    fn stamp_budget_grants_up_to_remaining() {
        let mut remaining = 10;
        assert_eq!(take_stamp_budget(&mut remaining, 4), 4);
        assert_eq!(remaining, 6);
        assert_eq!(take_stamp_budget(&mut remaining, 6), 6);
        assert_eq!(remaining, 0);
    }

    #[test]
    fn stamp_budget_saturates_and_stays_zero() {
        let mut remaining = 5;
        assert_eq!(take_stamp_budget(&mut remaining, 100), 5);
        assert_eq!(remaining, 0);
        assert_eq!(take_stamp_budget(&mut remaining, 100), 0);
    }

    #[test]
    fn stamp_budget_zero_request_is_noop() {
        let mut remaining = MAX_STAMPS_PER_PETAL;
        assert_eq!(take_stamp_budget(&mut remaining, 0), 0);
        assert_eq!(remaining, MAX_STAMPS_PER_PETAL);
    }

    #[test]
    fn stamp_budget_petal_cap_bounds_many_max_stamp_tracks() {
        // 32 fully-saturated tracks (4096 each) would be 131072 stamps; the
        // petal budget cuts the total at MAX_STAMPS_PER_PETAL.
        let mut remaining = MAX_STAMPS_PER_PETAL;
        let total: usize = (0..32)
            .map(|_| take_stamp_budget(&mut remaining, 4096))
            .sum();
        assert_eq!(total, MAX_STAMPS_PER_PETAL);
    }

    // --- per-track applied gate (FR-2) ---

    #[test]
    fn applied_gate_is_per_track_and_input_sensitive() {
        let d = sample_desc();
        let fp = 12345u64;
        let mut applied = PathAssetApplied::default();
        assert!(
            !applied.matches("t1", &d, fp, 1.0, 0),
            "empty gate never matches"
        );
        applied.remember("t1", d.clone(), fp, 1.0, 0);
        assert!(
            applied.matches("t1", &d, fp, 1.0, 0),
            "identical inputs match"
        );
        // FR-2: a different track is independent — the single-slot gate used to
        // stomp here, dropping one track's stamps when another was applied.
        assert!(
            !applied.matches("t2", &d, fp, 1.0, 0),
            "different track re-stamps"
        );
        // Changed points fingerprint re-stamps.
        assert!(
            !applied.matches("t1", &d, fp ^ 1, 1.0, 0),
            "changed points re-stamp"
        );
        // Changed descriptor re-stamps.
        let mut d2 = d.clone();
        d2.spacing_value = 6.0;
        assert!(
            !applied.matches("t1", &d2, fp, 1.0, 0),
            "changed descriptor re-stamps"
        );
        // FR-3: metric spacing depends on world_scale, so a scale change re-stamps.
        assert!(
            !applied.matches("t1", &d, fp, 0.5, 0),
            "changed world_scale re-stamps"
        );
        // T2 FR-3: a changed per-stamp override set re-stamps.
        assert!(
            !applied.matches("t1", &d, fp, 1.0, 42),
            "changed overrides re-stamp"
        );
    }

    #[test]
    fn applied_gate_invalidate_is_per_track_and_clear_drops_all() {
        let d = sample_desc();
        let mut applied = PathAssetApplied::default();
        applied.remember("t1", d.clone(), 1, 1.0, 0);
        applied.remember("t2", d.clone(), 2, 1.0, 0);
        applied.invalidate("t1");
        assert!(
            !applied.matches("t1", &d, 1, 1.0, 0),
            "invalidated track re-stamps"
        );
        assert!(
            applied.matches("t2", &d, 2, 1.0, 0),
            "sibling survives invalidate"
        );
        // Petal change / reset clears everything so re-entry restamps.
        applied.clear();
        assert!(
            !applied.matches("t2", &d, 2, 1.0, 0),
            "clear drops everything"
        );
    }

    // --- per-stamp override application (T2 FR-3) ---

    /// 3-point straight polyline along +X, total length 20 m.
    fn straight_points() -> Vec<[f32; 3]> {
        vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]]
    }

    #[test]
    fn arc_offset_override_resamples_position_on_the_polyline() {
        let points = straight_points();
        let ov = StampOverride {
            arc_offset_m: Some(7.5),
            ..Default::default()
        };
        let t = stamp_transform(&points, [0.0, 0.0, 0.0], 0.0, true, Some(&ov));
        assert!(
            (t.translation.x - 7.5).abs() < 1e-3,
            "slide to 7.5 m along +X, got {:?}",
            t.translation
        );
        // Yaw is re-derived at the new offset (still +X → FRAC_PI_2).
        let (axis, angle) = t.rotation.to_axis_angle();
        assert!(
            (angle - std::f32::consts::FRAC_PI_2).abs() < 1e-2 && axis.y > 0.9,
            "tangent yaw at the new offset, got axis {axis:?} angle {angle}"
        );
        // Overlong slide clamps to the path end (never off-curve).
        let ov_far = StampOverride {
            arc_offset_m: Some(999.0),
            ..Default::default()
        };
        let t = stamp_transform(&points, [0.0, 0.0, 0.0], 0.0, false, Some(&ov_far));
        assert!((t.translation.x - 20.0).abs() < 1e-3, "clamped to total");
    }

    #[test]
    fn rotation_override_replaces_tangent_yaw() {
        use std::f32::consts::FRAC_1_SQRT_2;
        let points = straight_points();
        let ov = StampOverride {
            rotation: Some([0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2]),
            ..Default::default()
        };
        // Default yaw would be PI (some other heading); the override wins.
        let t = stamp_transform(
            &points,
            [5.0, 0.0, 0.0],
            std::f32::consts::PI,
            true,
            Some(&ov),
        );
        let expected = Quat::from_xyzw(0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2);
        assert!(
            t.rotation.angle_between(expected) < 1e-3,
            "override quat wins over tangent yaw, got {:?}",
            t.rotation
        );
        // A degenerate zero quat keeps the tangent default (no NaN basis).
        let ov_zero = StampOverride {
            rotation: Some([0.0; 4]),
            ..Default::default()
        };
        let t = stamp_transform(&points, [5.0, 0.0, 0.0], 0.5, true, Some(&ov_zero));
        assert!(
            t.rotation.angle_between(Quat::from_rotation_y(0.5)) < 1e-3,
            "zero quat must not replace the default"
        );
    }

    #[test]
    fn scale_override_sets_transform_scale() {
        let points = straight_points();
        let ov = StampOverride {
            scale: Some([2.0, 3.0, 4.0]),
            ..Default::default()
        };
        let t = stamp_transform(&points, [5.0, 0.0, 0.0], 0.0, false, Some(&ov));
        assert_eq!(t.scale, Vec3::new(2.0, 3.0, 4.0));
        // No override → identity scale and the path-derived position.
        let t = stamp_transform(&points, [5.0, 0.0, 0.0], 0.0, false, None);
        assert_eq!(t.scale, Vec3::ONE);
        assert_eq!(t.translation, Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn overrides_fingerprint_tracks_every_field_and_sparseness() {
        let base: Vec<(usize, StampOverride)> = vec![];
        let one_scale = vec![(
            2usize,
            StampOverride {
                scale: Some([2.0, 2.0, 2.0]),
                ..Default::default()
            },
        )];
        let one_scale_other_value = vec![(
            2usize,
            StampOverride {
                scale: Some([3.0, 2.0, 2.0]),
                ..Default::default()
            },
        )];
        let one_arc = vec![(
            2usize,
            StampOverride {
                arc_offset_m: Some(1.5),
                ..Default::default()
            },
        )];
        let fp = overrides_fingerprint;
        assert_ne!(fp(&base), fp(&one_scale), "adding an override re-stamps");
        assert_ne!(
            fp(&one_scale),
            fp(&one_scale_other_value),
            "changing a value re-stamps"
        );
        assert_ne!(
            fp(&one_scale),
            fp(&one_arc),
            "different field with same index differs"
        );
        assert_eq!(fp(&one_scale), fp(&one_scale), "deterministic");
    }
}
