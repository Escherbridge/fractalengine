//! Right-click → context-menu classification (contextual_controls T4 FR-1).
//! Fills the `ActiveDialog::ContextMenu` target using the SAME pick machinery
//! as the left-click chain (ray/AABB + `TrackPickShape` via `viewport_pick`,
//! stamps via the T2 `StampRenderIndex`). See `node_manager/AGENTS.md`
//! §context-pick.

use bevy::camera::primitives::Aabb;
use bevy::prelude::*;

use super::dispatch::HitTarget;
use super::path_segment_interaction::{ray_polyline_hit, TrackPickShape};
use super::viewport_pick::pick_node_aabb;
use super::NodeManager;
use crate::actions::{UiAction, UiManager};
use crate::dialogs::{ActiveDialog, ContextTarget};
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;
use crate::verse_manager::{
    parse_stamp_marker_id, stamp_marker_id, PathAssetInstance, StampRenderIndex,
};
use fe_renderer::instancing::DEFAULT_CELL_SIZE_M;

/// Ground-pick radius for stamps (meters): one `StampSpatialIndex` grid cell —
/// the index's own "typical stamp footprint" sizing, keeping picks a 3×3 scan.
const STAMP_PICK_RADIUS_M: f32 = DEFAULT_CELL_SIZE_M;

/// One ray-pick winner over the spawned-node set, pre-digested for
/// [`resolve_context_target`] so the classification core stays pure.
pub(super) struct RayHit {
    pub entity: Entity,
    /// `SpawnedNodeMarker.node_id` (a stamp marker id for stamp instances).
    pub node_id: String,
    /// `PathAssetInstance.source_track_id` when the entity is a stamp instance.
    pub stamp_track: Option<String>,
}

/// Pure classification core: a ray hit beats the ground-stamp fallback; a
/// stamp-instance hit yields `Stamp` with its `(track, index)` payload; an
/// unparseable stamp marker degrades to a plain `Node` hit (defensive — the
/// format is produced by `verse_manager::stamp_marker_id`). No hit = `Empty`.
pub(super) fn resolve_context_target(
    ray_hit: Option<RayHit>,
    ground_stamp: Option<(String, usize, Entity)>,
) -> ContextTarget {
    if let Some(hit) = ray_hit {
        if let Some(track) = hit.stamp_track {
            if let Some((_, index)) = parse_stamp_marker_id(&hit.node_id) {
                return ContextTarget {
                    hit: HitTarget::Stamp(hit.entity),
                    node_id: None,
                    stamp: Some((track, index)),
                };
            }
        }
        return ContextTarget {
            hit: HitTarget::Node(hit.entity),
            node_id: Some(hit.node_id),
            stamp: None,
        };
    }
    if let Some((track, index, entity)) = ground_stamp {
        return ContextTarget {
            hit: HitTarget::Stamp(entity),
            node_id: None,
            stamp: Some((track, index)),
        };
    }
    ContextTarget {
        hit: HitTarget::Empty,
        node_id: None,
        stamp: None,
    }
}

/// Classify a freshly-opened context menu (`target: None`): ray-pick the
/// spawned-node set at the stored click position (identical loop to
/// `viewport_pick::handle_viewport_click`), fall back to the T2 stamp ground
/// index, then write the resolved [`ContextTarget`] back into the dialog.
/// Side effects mirror left-click: a node hit selects the node; a stamp hit
/// routes through the stamp authority's `SelectStamp` (idempotent + lazy
/// promotion — N-3/N-9). Always resolves (worst case `Empty`), so the menu is
/// never stuck unclassified (N-8).
pub(super) fn classify_context_menu(
    mut ui_mgr: ResMut<UiManager>,
    mut node_mgr: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    node_query: Query<(
        Entity,
        &SpawnedNodeMarker,
        Option<&TrackPickShape>,
        Option<&PathAssetInstance>,
    )>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    cameras: Query<(&Camera, &GlobalTransform), With<fe_renderer::camera::OrbitCameraController>>,
    stamp_index: Res<StampRenderIndex>,
) {
    let (screen, world) = match &ui_mgr.active_dialog {
        ActiveDialog::ContextMenu {
            screen_pos,
            world_pos,
            target: None,
            ..
        } => (*screen_pos, *world_pos),
        _ => return,
    };
    let active_petal = nav.active_petal_id.as_deref();

    // Camera ray through the stored click position — same construction as
    // `router::resolve_pointer_frame` (right-click bypasses the left arbiter).
    let ray = cameras.single().ok().and_then(|(camera, cam_tx)| {
        camera
            .viewport_to_world(cam_tx, Vec2::new(screen[0], screen[1]))
            .ok()
    });

    let mut best: Option<(f32, RayHit)> = None;
    if let Some(ray) = ray {
        for (entity, marker, pick_shape, stamp_inst) in node_query.iter() {
            if active_petal
                .map(|pid| pid != marker.petal_id.as_str())
                .unwrap_or(false)
            {
                continue;
            }
            let t = if let Some(shape) = pick_shape {
                ray_polyline_hit(&shape.points, ray.origin, *ray.direction, shape.half_width)
            } else {
                pick_node_aabb(
                    entity,
                    &ray,
                    &g_transform_query,
                    &aabb_query,
                    &children_query,
                )
            };
            let Some(t) = t else { continue };
            if best.as_ref().is_none_or(|(bt, _)| t < *bt) {
                best = Some((
                    t,
                    RayHit {
                        entity,
                        node_id: marker.node_id.clone(),
                        stamp_track: stamp_inst.map(|i| i.source_track_id.clone()),
                    },
                ));
            }
        }
    }

    // Ground fallback for stamps the ray grazed past: the T2 pick index at the
    // ground-projected click. First hit in sorted track order wins (tracks
    // overlapping within one cell are an accepted tie-break).
    let ground_stamp = if best.is_none() {
        pick_ground_stamp(&stamp_index, active_petal, world[0], world[2], &node_query)
    } else {
        None
    };

    let resolved = resolve_context_target(best.map(|(_, hit)| hit), ground_stamp);

    if let (HitTarget::Node(entity), Some(node_id)) = (&resolved.hit, &resolved.node_id) {
        node_mgr.select(*entity, node_id.clone());
    }
    if let Some((track, index)) = &resolved.stamp {
        ui_mgr.push_action(UiAction::SelectStamp {
            track_node_id: track.clone(),
            stamp_index: *index,
        });
    }
    if let ActiveDialog::ContextMenu { target, .. } = &mut ui_mgr.active_dialog {
        *target = Some(resolved);
    }
}

/// Nearest active-petal stamp within [`STAMP_PICK_RADIUS_M`] of ground point
/// `(x, z)`, resolved back to its live entity by marker id
/// (`Entity::PLACEHOLDER` when the instance is mid-respawn — the stamp verbs
/// key on the `(track, index)` payload, never on the entity).
fn pick_ground_stamp(
    stamp_index: &StampRenderIndex,
    active_petal: Option<&str>,
    x: f32,
    z: f32,
    node_query: &Query<(
        Entity,
        &SpawnedNodeMarker,
        Option<&TrackPickShape>,
        Option<&PathAssetInstance>,
    )>,
) -> Option<(String, usize, Entity)> {
    let petal = active_petal?;
    let mut track_ids: Vec<&String> = stamp_index
        .tracks
        .iter()
        .filter(|(_, data)| data.petal_id == petal)
        .map(|(id, _)| id)
        .collect();
    track_ids.sort();
    for track_id in track_ids {
        let Some(data) = stamp_index.tracks.get(track_id) else {
            continue;
        };
        if let Some(index) = data.index.pick_nearest(x, z, STAMP_PICK_RADIUS_M) {
            let marker_id = stamp_marker_id(track_id, index);
            let entity = node_query
                .iter()
                .find(|(_, marker, _, _)| marker.node_id == marker_id)
                .map(|(entity, _, _, _)| entity)
                .unwrap_or(Entity::PLACEHOLDER);
            return Some((track_id.clone(), index, entity));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    #[test]
    fn ray_node_hit_resolves_to_node_with_id() {
        let target = resolve_context_target(
            Some(RayHit {
                entity: entity(1),
                node_id: "n1".into(),
                stamp_track: None,
            }),
            None,
        );
        assert_eq!(target.hit, HitTarget::Node(entity(1)));
        assert_eq!(target.node_id.as_deref(), Some("n1"));
        assert!(target.stamp.is_none());
    }

    #[test]
    fn ray_stamp_hit_resolves_to_stamp_with_track_and_index() {
        let target = resolve_context_target(
            Some(RayHit {
                entity: entity(2),
                node_id: "track-1::stamp::7".into(),
                stamp_track: Some("track-1".into()),
            }),
            None,
        );
        assert_eq!(target.hit, HitTarget::Stamp(entity(2)));
        assert!(target.node_id.is_none(), "stamp id resolves at render time");
        assert_eq!(target.stamp, Some(("track-1".to_string(), 7)));
    }

    #[test]
    fn ray_hit_outranks_ground_stamp_fallback() {
        let target = resolve_context_target(
            Some(RayHit {
                entity: entity(1),
                node_id: "n1".into(),
                stamp_track: None,
            }),
            Some(("track-1".into(), 0, entity(9))),
        );
        assert_eq!(target.hit, HitTarget::Node(entity(1)));
    }

    #[test]
    fn stamp_with_unparseable_marker_degrades_to_node() {
        let target = resolve_context_target(
            Some(RayHit {
                entity: entity(3),
                node_id: "not-a-stamp-id".into(),
                stamp_track: Some("track-1".into()),
            }),
            None,
        );
        assert_eq!(target.hit, HitTarget::Node(entity(3)));
        assert_eq!(target.node_id.as_deref(), Some("not-a-stamp-id"));
    }

    #[test]
    fn ground_stamp_fallback_resolves_when_ray_misses() {
        let target = resolve_context_target(None, Some(("t".into(), 3, entity(5))));
        assert_eq!(target.hit, HitTarget::Stamp(entity(5)));
        assert_eq!(target.stamp, Some(("t".to_string(), 3)));
    }

    #[test]
    fn no_hit_resolves_to_empty_ground() {
        let target = resolve_context_target(None, None);
        assert_eq!(target.hit, HitTarget::Empty);
        assert!(target.node_id.is_none() && target.stamp.is_none());
    }
}
