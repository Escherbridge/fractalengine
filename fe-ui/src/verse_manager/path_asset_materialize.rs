//! Petal-wide path-asset stamp materialization (FR-1/FR-2): a fe-ui-local
//! cache of every track's `path_asset` stamp descriptor + `gpx_points`, fed by
//! `GetNodeProperties` round-trips (petal-load batch + live edits), consumed by
//! the single `materialize_path_assets` system that does ALL stamp spawning.
//! Mirrors `primitive_materialize.rs`. See
//! `fe-ui/src/verse_manager/AGENTS.md` §path-asset-materialization.

use std::collections::HashMap;

use bevy::prelude::*;
use fe_sdk::path_asset::{PathAssetDescriptor, PATH_ASSET_PROPERTY_KEY};

use super::path_asset_reconcile::{points_fingerprint, sample_transforms, sanitize_world_scale};
use super::spawn::{spawn_stamped_entity, PathAssetInstance};
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
        let points: Vec<[f32; 3]> = properties
            .get("gpx_points")
            .map(crate::gis::decode_gpx_points)
            .map(|rows| rows.into_iter().map(|r| r.position).collect())
            .unwrap_or_default();
        if points.is_empty() {
            // A `path_asset` with no path to stamp along → nothing to materialize.
            self.invalidate(node_id);
            return;
        }
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

/// The `(descriptor, points-fingerprint, world-scale)` last stamped for a track.
struct AppliedState {
    descriptor: PathAssetDescriptor,
    points_fingerprint: u64,
    world_scale_bits: u32,
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
    ) -> bool {
        self.applied.get(track_id).is_some_and(|s| {
            &s.descriptor == descriptor
                && s.points_fingerprint == points_fingerprint
                && s.world_scale_bits == world_scale.to_bits()
        })
    }

    fn remember(
        &mut self,
        track_id: &str,
        descriptor: PathAssetDescriptor,
        points_fingerprint: u64,
        world_scale: f32,
    ) {
        self.applied.insert(
            track_id.to_string(),
            AppliedState {
                descriptor,
                points_fingerprint,
                world_scale_bits: world_scale.to_bits(),
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

/// Petal-wide path-asset materializer (FR-1): the ONLY system that spawns
/// stamp instances. For every cached track in the active petal it despawns the
/// old `PathAssetInstance` group and restamps whenever the descriptor, points,
/// or `world_scale` changed (per-track gate). Orphaned groups (track no longer
/// cached — e.g. its `path_asset` was deleted) are torn down. Runs when the
/// cache changed, the petal changed, or the petal's `world_scale` changed;
/// chained after `respawn_on_petal_change` + `reconcile_path_asset` so it
/// observes their despawns, gate clears, and cache feeds. See AGENTS.md
/// §path-asset-materialization.
#[allow(clippy::too_many_arguments)]
pub(super) fn materialize_path_assets(
    nav: Res<NavigationManager>,
    cache: Res<PathAssetCache>,
    petal_map: Res<crate::terrain_map::PetalMapState>,
    asset_server: Res<AssetServer>,
    mut applied: ResMut<PathAssetApplied>,
    mut last_petal: Local<Option<String>>,
    mut commands: Commands,
    existing: Query<(Entity, &PathAssetInstance)>,
    mesh_budget: Res<crate::plugin::MeshInstanceBudget>,
) {
    let petal_changed = *last_petal != nav.active_petal_id;
    if !(cache.is_changed() || petal_map.is_changed() || petal_changed) {
        return;
    }
    *last_petal = nav.active_petal_id.clone();

    let Some(active_petal) = nav.active_petal_id.as_deref() else {
        return;
    };
    // FR-3: metric spacing conversion pulls the active petal's world scale.
    let world_scale = sanitize_world_scale(petal_map.world_scale as f32);

    // Orphan cleanup: tear down active-petal stamp groups whose track no longer
    // has a cache entry in this petal (path_asset deleted / node removed).
    for (entity, inst) in existing.iter() {
        if inst.petal_id == active_petal
            && cache
                .get(&inst.source_track_id)
                .map(|e| e.petal_id.as_str())
                != Some(active_petal)
        {
            commands.entity(entity).despawn();
            applied.invalidate(&inst.source_track_id);
        }
    }

    // Petal-wide stamp budget for this pass; the app-wide mesh-budget gate
    // (§mesh-budget) zeroes it so restamps stop growing a runaway scene.
    let mut stamp_budget = if mesh_budget.exceeded {
        0
    } else {
        MAX_STAMPS_PER_PETAL
    };

    // (Re)stamp every active-petal cached track whose stamp inputs changed.
    for (track_id, entry) in cache.iter() {
        if entry.petal_id != active_petal {
            continue;
        }
        if applied.matches(track_id, &entry.descriptor, entry.fingerprint, world_scale) {
            continue; // nothing changed — skip the restamp entirely
        }

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
        for (i, (position, yaw)) in samples.into_iter().enumerate() {
            let mut transform = Transform::from_xyz(position[0], position[1], position[2]);
            if entry.descriptor.tangent_align {
                transform.rotation = Quat::from_rotation_y(yaw);
            }
            let stamp_id = format!("{}::stamp::{}", track_id, i);
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
        applied.remember(
            track_id,
            entry.descriptor.clone(),
            entry.fingerprint,
            world_scale,
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
            !applied.matches("t1", &d, fp, 1.0),
            "empty gate never matches"
        );
        applied.remember("t1", d.clone(), fp, 1.0);
        assert!(applied.matches("t1", &d, fp, 1.0), "identical inputs match");
        // FR-2: a different track is independent — the single-slot gate used to
        // stomp here, dropping one track's stamps when another was applied.
        assert!(
            !applied.matches("t2", &d, fp, 1.0),
            "different track re-stamps"
        );
        // Changed points fingerprint re-stamps.
        assert!(
            !applied.matches("t1", &d, fp ^ 1, 1.0),
            "changed points re-stamp"
        );
        // Changed descriptor re-stamps.
        let mut d2 = d.clone();
        d2.spacing_value = 6.0;
        assert!(
            !applied.matches("t1", &d2, fp, 1.0),
            "changed descriptor re-stamps"
        );
        // FR-3: metric spacing depends on world_scale, so a scale change re-stamps.
        assert!(
            !applied.matches("t1", &d, fp, 0.5),
            "changed world_scale re-stamps"
        );
    }

    #[test]
    fn applied_gate_invalidate_is_per_track_and_clear_drops_all() {
        let d = sample_desc();
        let mut applied = PathAssetApplied::default();
        applied.remember("t1", d.clone(), 1, 1.0);
        applied.remember("t2", d.clone(), 2, 1.0);
        applied.invalidate("t1");
        assert!(
            !applied.matches("t1", &d, 1, 1.0),
            "invalidated track re-stamps"
        );
        assert!(
            applied.matches("t2", &d, 2, 1.0),
            "sibling survives invalidate"
        );
        // Petal change / reset clears everything so re-entry restamps.
        applied.clear();
        assert!(!applied.matches("t2", &d, 2, 1.0), "clear drops everything");
    }
}
