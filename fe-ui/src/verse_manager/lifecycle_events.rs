//! In-app consumer of DB lifecycle events (T2 integration): `PathReflow` →
//! stamp identity remap + cache invalidation + property re-fetch so the
//! materializer restamps with the new count. Other variants ride `DbResult`
//! (`NodePromoted` etc.). See AGENTS.md §path-asset-materialization.

use bevy::prelude::*;
use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::{DbCommand, LifecycleEvent};

use super::path_asset_materialize::{PathAssetApplied, PathAssetCache};
use crate::actions::asset::StampInteractionState;

/// Drain `Messages<LifecycleEvent>` (pumped by fe-runtime from the DB thread's
/// crossbeam seam). `PathReflow` shifts surviving stamp overrides/promotions
/// down one index (T2 FR-5) and invalidates the track's stamp caches; the
/// follow-up `GetNodeProperties` re-feeds the cache (`NodePropertiesLoaded` →
/// `note_properties`) so the restamp uses the reflowed truth.
pub(super) fn apply_lifecycle_events(
    mut reader: MessageReader<LifecycleEvent>,
    mut stamp_state: ResMut<StampInteractionState>,
    mut cache: ResMut<PathAssetCache>,
    mut applied: ResMut<PathAssetApplied>,
    db_sender: Res<DbCommandSender>,
) {
    for event in reader.read() {
        if let LifecycleEvent::PathReflow {
            path_id,
            deleted_index,
        } = event
        {
            apply_path_reflow(
                path_id,
                *deleted_index,
                &mut stamp_state,
                &mut cache,
                &mut applied,
            );
            // Re-feed the invalidated cache with the reflowed points/descriptor
            // (same refresh idiom as the `path_asset` property-set handler).
            if db_sender
                .0
                .send(DbCommand::GetNodeProperties {
                    node_id: path_id.clone(),
                })
                .is_err()
            {
                bevy::log::warn!("db_sender channel closed — reflow re-fetch not sent");
            }
        }
    }
}

/// Pure part of the reflow handling (unit-tested; the index remap itself is
/// pinned by `actions::asset` tests): shift stamp identities for the deleted
/// index, then drop the track's cache + applied entries (the `NodeDeleted`
/// invalidation idiom) so the materializer restamps with the new count.
fn apply_path_reflow(
    path_id: &str,
    deleted_index: Option<u32>,
    stamp_state: &mut StampInteractionState,
    cache: &mut PathAssetCache,
    applied: &mut PathAssetApplied,
) {
    if let Some(index) = deleted_index {
        stamp_state.reflow_after_delete(path_id, index as usize);
    }
    cache.invalidate(path_id);
    applied.invalidate(path_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::asset::StampRef;
    use fe_sdk::path_asset::{PathAssetDescriptor, SpacingMode};

    fn desc() -> PathAssetDescriptor {
        PathAssetDescriptor {
            asset_path: "blob://x.glb".to_string(),
            spacing_mode: SpacingMode::FixedCount,
            spacing_value: 0.0,
            count: 4,
            tangent_align: false,
        }
    }

    #[test]
    fn path_reflow_shifts_overrides_and_invalidates_track_caches() {
        let mut stamps = StampInteractionState::default();
        stamps.set_arc_offset(
            &StampRef {
                track_node_id: "path-1".into(),
                stamp_index: 3,
            },
            5.0,
        );
        let mut cache = PathAssetCache::default();
        cache.upsert("path-1", "p1", desc(), vec![[0.0; 3], [10.0, 0.0, 0.0]], 7);
        let mut applied = PathAssetApplied::default();

        apply_path_reflow("path-1", Some(1), &mut stamps, &mut cache, &mut applied);

        // Identity remap: index 3 slid down to 2 with its override intact.
        assert_eq!(
            stamps
                .override_for(&StampRef {
                    track_node_id: "path-1".into(),
                    stamp_index: 2,
                })
                .and_then(|o| o.arc_offset_m),
            Some(5.0)
        );
        // Cache eviction forces a restamp once the re-fetch re-feeds it.
        assert!(cache.get("path-1").is_none(), "stale entry must be evicted");
    }

    #[test]
    fn path_reflow_without_index_still_invalidates() {
        // A reflow for a non-stamp delete carries no index — identities keep,
        // but the track still restamps against the reloaded truth.
        let mut stamps = StampInteractionState::default();
        stamps.set_arc_offset(
            &StampRef {
                track_node_id: "path-1".into(),
                stamp_index: 3,
            },
            5.0,
        );
        let mut cache = PathAssetCache::default();
        cache.upsert("path-1", "p1", desc(), vec![[0.0; 3]], 1);
        let mut applied = PathAssetApplied::default();
        apply_path_reflow("path-1", None, &mut stamps, &mut cache, &mut applied);
        assert!(cache.get("path-1").is_none());
        assert_eq!(
            stamps
                .override_for(&StampRef {
                    track_node_id: "path-1".into(),
                    stamp_index: 3,
                })
                .and_then(|o| o.arc_offset_m),
            Some(5.0),
            "no index → identities untouched"
        );
    }

    #[test]
    fn path_reflow_on_unknown_track_is_a_noop() {
        let mut stamps = StampInteractionState::default();
        let mut cache = PathAssetCache::default();
        let mut applied = PathAssetApplied::default();
        // Never-stamped track: nothing to drop, nothing to shift, no panic.
        apply_path_reflow("ghost", Some(0), &mut stamps, &mut cache, &mut applied);
        assert!(cache.get("ghost").is_none());
    }
}
