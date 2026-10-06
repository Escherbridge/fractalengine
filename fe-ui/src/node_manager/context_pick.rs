//! Right-click → context-menu classification (contextual_controls T4 FR-1).
//! Fills the `ActiveDialog::ContextMenu` target using the SAME pick machinery
//! as the left-click chain (ray/AABB + `TrackPickShape` via `viewport_pick`,
//! stamps via the T2 `StampRenderIndex`). See `node_manager/AGENTS.md`
//! §context-pick.

use bevy::camera::primitives::Aabb;
use bevy::math::Vec3Swizzles;
use bevy::prelude::*;

use super::dispatch::HitTarget;
use super::path_handle_interaction::PathHandleMarker;
use super::path_point_interaction::{PathPointMarker, PICK_RADIUS};
use super::path_segment_interaction::{ray_polyline_hit, TrackPickShape};
use super::viewport_pick::pick_node_aabb;
use crate::actions::UiManager;
use crate::dialogs::{ActiveDialog, ContextTarget};
use crate::gis::PathEditorState;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;
use crate::terrain_proposal_state::{ProposalEditState, ProposalRecord};
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

/// Nearest candidate under `ray` (along-ray + `radius` test), among items
/// paired with their world position — the pure core shared by the path-vertex
/// and path-handle marker picks below. Mirrors
/// `path_point_interaction::pick_marker` / `path_handle_interaction::
/// pick_nearest_handle`, both private to their own module; duplicated here
/// (rather than exposed cross-module) because classification is read-only and
/// must stay decoupled from the interaction systems' internals. Pure —
/// unit-tested directly, no ECS `World` needed. Ties broken by nearest
/// along-ray distance.
fn nearest_along_ray<T: Copy>(
    ray_origin: Vec3,
    ray_dir: Vec3,
    radius: f32,
    candidates: impl Iterator<Item = (T, Vec3)>,
) -> Option<T> {
    let mut best: Option<(T, f32)> = None;
    for (item, pos) in candidates {
        let along = (pos - ray_origin).dot(ray_dir);
        if along < 0.0 {
            continue;
        }
        let closest = ray_origin + ray_dir * along;
        if (pos - closest).length() < radius && best.as_ref().is_none_or(|(_, bt)| along < *bt) {
            best = Some((item, along));
        }
    }
    best.map(|(item, _)| item)
}

/// Index of the polyline segment nearest `point` in the XZ ground plane —
/// classification-only sibling of `path_segment_interaction::nearest_segment`
/// (private to its module, and does the fuller ribbon/fill-triangle test the
/// authoritative left-click select needs). Good enough to label a right-click
/// on the currently-edited track's ribbon; `None` for `< 2` points. Pure.
fn nearest_segment_xz(points: &[Vec3], point: Vec3) -> Option<usize> {
    if points.len() < 2 {
        return None;
    }
    points
        .windows(2)
        .enumerate()
        .map(|(i, w)| {
            let a = w[0].xz();
            let delta = w[1].xz() - a;
            let len_sq = delta.length_squared();
            let t = if len_sq > 0.0 {
                ((point.xz() - a).dot(delta) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (i, point.xz().distance_squared(a + delta * t))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, _)| i)
}

/// Ray-casting point-in-polygon test (PNPOLY) over a closed XZ footprint
/// (world units, `ProposalRecord::footprint`'s convention). A degenerate
/// footprint (`< 3` points) never contains a point — no divide-by-zero risk
/// since the crossing test only evaluates when `zi != zj` (guaranteed by the
/// `(zi > z) != (zj > z)` guard). Pure.
fn point_in_footprint(point: [f32; 2], footprint: &[[f32; 2]]) -> bool {
    if footprint.len() < 3 {
        return false;
    }
    let (x, z) = (point[0], point[1]);
    let n = footprint.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, zi) = (footprint[i][0], footprint[i][1]);
        let (xj, zj) = (footprint[j][0], footprint[j][1]);
        if (zi > z) != (zj > z) && x < (xj - xi) * (z - zi) / (zj - zi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Which ray hit [`classify_context_menu`] labels, once both candidates are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HitChoice {
    /// The ray hit nothing pickable.
    None,
    /// The closest hit over all nodes.
    Nearest,
    /// A hit on the currently-edited track's own ribbon.
    EditedRibbon,
}

/// Pick between the nearest hit overall and the nearest hit on the edited
/// track's ribbon (a subset of the former), by along-ray distance `t`.
///
/// The edited ribbon wins WHENEVER the ray touched it — even from behind a
/// nearer object. That is not a tie-break, it is a priority TIER, and it is
/// what left-click does: `handle_path_segment_interaction` runs ahead of
/// `handle_viewport_click` and claims `ClickPriority::PathSegment` on any
/// ribbon hit of the edited track, never comparing depth against other nodes
/// (finding F10 — the two used to disagree on an occluded edited ribbon).
/// Pure.
fn prefer_edited_ribbon(nearest: Option<f32>, edited: Option<f32>) -> HitChoice {
    match (nearest, edited) {
        (_, Some(_)) => HitChoice::EditedRibbon,
        (Some(_), None) => HitChoice::Nearest,
        (None, None) => HitChoice::None,
    }
}

/// First proposal whose XZ footprint contains `point` — list order wins ties,
/// mirroring `pick_ground_stamp`'s "first in order" tie-break. Pure.
fn pick_ground_proposal(proposals: &[ProposalRecord], point: [f32; 2]) -> Option<String> {
    proposals
        .iter()
        .find(|p| point_in_footprint(point, &p.footprint))
        .map(|p| p.id.clone())
}

/// Writes a resolved [`ContextTarget`] back into the still-open dialog. No-op
/// if the dialog closed or moved on between classification and this write
/// (defensive — classification runs the same frame it opens, but never
/// assume the dialog variant is still `ContextMenu`).
fn write_target(ui_mgr: &mut UiManager, resolved: ContextTarget) {
    if let ActiveDialog::ContextMenu { target, .. } = &mut ui_mgr.active_dialog {
        *target = Some(resolved);
    }
}

/// Classify a freshly-opened context menu (`target: None`). Priority mirrors
/// the left-click `ClickPriority` order (`node_manager/AGENTS.md`
/// §click-priority), tier for tier: path HANDLE beats path VERTEX beats
/// PathSegment (the edited track's ribbon) beats the ray's closest entity hit
/// (Node/Stamp — same ordering as `handle_viewport_click`) beats the
/// ground-stamp fallback beats a terrain PROPOSAL footprint, worst case
/// `Empty`.
///
/// `PathSegment` is a real tier, not a tie-break (finding F10): a hit anywhere
/// along the edited track's ribbon outranks a NEARER hit on any other node,
/// exactly as `handle_path_segment_interaction` claims that ribbon ahead of
/// `handle_viewport_click`'s `NodePick` without comparing depth. Only the
/// entity-backed ribbon is considered here — left-click additionally falls back
/// to `PathEditorState`'s raw points when the ribbon has not rendered yet, a
/// state in which there is nothing to right-click anyway.
///
/// Path vertex/handle/segment hits only ever resolve while their track is the
/// one being edited (`PathEditorState.editing_track_id`) — otherwise a click on
/// that same ribbon is a plain whole-track `Node` hit (finding #4,
/// ui_semantics_unification_20260808 §5 Phase 5).
/// Classification is read-only and never changes either selection authority.
/// Always resolves (worst case `Empty`), so the menu cannot hang
/// unclassified.
#[allow(clippy::too_many_arguments)] // one system, one job: classify a right-click
pub(super) fn classify_context_menu(
    mut ui_mgr: ResMut<UiManager>,
    nav: Res<NavigationManager>,
    path_state: Res<PathEditorState>,
    proposal_state: Res<ProposalEditState>,
    node_query: Query<(
        Entity,
        &SpawnedNodeMarker,
        Option<&TrackPickShape>,
        Option<&PathAssetInstance>,
    )>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    vertex_query: Query<(&GlobalTransform, &PathPointMarker)>,
    handle_query: Query<(&GlobalTransform, &PathHandleMarker)>,
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

    // Path-edit markers only exist while a track is open for editing (the
    // sync systems despawn them on stop-editing), so the queries are
    // naturally empty otherwise; the explicit `editing` check keeps the
    // resolver correct even if this ever ran a frame ahead of that despawn.
    let editing = path_state.editing_track_id.is_some();
    if editing {
        if let Some(ray) = ray {
            if let Some((idx, side)) = nearest_along_ray(
                ray.origin,
                *ray.direction,
                PICK_RADIUS,
                handle_query
                    .iter()
                    .map(|(g, m)| ((m.index, m.side), g.translation())),
            ) {
                write_target(
                    &mut ui_mgr,
                    ContextTarget {
                        hit: HitTarget::PathHandle { idx, side },
                        node_id: None,
                        stamp: None,
                    },
                );
                return;
            }
            if let Some(idx) = nearest_along_ray(
                ray.origin,
                *ray.direction,
                PICK_RADIUS,
                vertex_query.iter().map(|(g, m)| (m.index, g.translation())),
            ) {
                write_target(
                    &mut ui_mgr,
                    ContextTarget {
                        hit: HitTarget::PathVertex { idx },
                        node_id: None,
                        stamp: None,
                    },
                );
                return;
            }
        }
    }

    let mut best: Option<(f32, RayHit)> = None;
    // F10: the edited track's own ribbon is tracked as its own tier, so an
    // occluding node in front of it can't demote a segment pick to a node pick.
    let mut edited_ribbon: Option<(f32, RayHit)> = None;
    if let Some(ray) = ray {
        for (entity, marker, pick_shape, stamp_inst) in node_query.iter() {
            if active_petal
                .map(|pid| pid != marker.petal_id.as_str())
                .unwrap_or(false)
            {
                continue;
            }
            let t = if let Some(shape) = pick_shape {
                ray_polyline_hit(
                    &shape.points,
                    ray.origin,
                    *ray.direction,
                    shape.half_width,
                    &shape.fill_triangles,
                )
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
            let is_edited_ribbon = editing
                && stamp_inst.is_none()
                && path_state.editing_track_id.as_deref() == Some(marker.node_id.as_str());
            if is_edited_ribbon && edited_ribbon.as_ref().is_none_or(|(bt, _)| t < *bt) {
                edited_ribbon = Some((
                    t,
                    RayHit {
                        entity,
                        node_id: marker.node_id.clone(),
                        stamp_track: None,
                    },
                ));
            }
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

    // Apply the PathSegment tier: a touched edited ribbon replaces a nearer
    // generic hit outright, so the redirect below sees it (finding F10). If the
    // segment resolution then fails (a ribbon needs ≥ 2 points), the fall-through
    // still labels the TRACK — never the unrelated object that occluded it.
    if prefer_edited_ribbon(
        best.as_ref().map(|(t, _)| *t),
        edited_ribbon.as_ref().map(|(t, _)| *t),
    ) == HitChoice::EditedRibbon
    {
        best = edited_ribbon;
    }

    // Segment redirect: a ray hit on the CURRENTLY-EDITED track's ribbon is a
    // `PathSegment` pick, not a whole-`Node` pick — matches left-click's
    // `path_segment_interaction::handle_path_segment_interaction`, which
    // claims the same ribbon while editing. `best` already holds the tier
    // winner (above), so this only has to name the segment. `node_id` stays
    // populated (the track id) so the menu's node-scoped verbs
    // (EditPath/AddStamps/etc.) keep working unchanged.
    if let (Some(ray), Some((t, hit))) = (ray, &best) {
        if editing
            && hit.stamp_track.is_none()
            && path_state.editing_track_id.as_deref() == Some(hit.node_id.as_str())
        {
            let hit_point = ray.origin + *ray.direction * *t;
            let points: Vec<Vec3> = path_state
                .points
                .iter()
                .map(|p| Vec3::from(p.position))
                .collect();
            if let Some(idx) = nearest_segment_xz(&points, hit_point) {
                write_target(
                    &mut ui_mgr,
                    ContextTarget {
                        hit: HitTarget::PathSegment { idx },
                        node_id: Some(hit.node_id.clone()),
                        stamp: None,
                    },
                );
                return;
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

    // Lowest-priority ground tier: an earthwork proposal footprint, only
    // considered once nothing else (entity ray hit or stamp ground pick)
    // claimed this click.
    if best.is_none() && ground_stamp.is_none() {
        if let Some(id) = pick_ground_proposal(&proposal_state.proposals, [world[0], world[2]]) {
            write_target(
                &mut ui_mgr,
                ContextTarget {
                    hit: HitTarget::TerrainProposal { id },
                    node_id: None,
                    stamp: None,
                },
            );
            return;
        }
    }

    let resolved = resolve_context_target(best.map(|(_, hit)| hit), ground_stamp);
    write_target(&mut ui_mgr, resolved);
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

    // --- finding #4 (ui_semantics_unification_20260808): the new resolvable
    // targets' pure classification cores ---

    #[test]
    fn nearest_along_ray_picks_the_closest_in_front_candidate() {
        let candidates = vec![
            (1usize, Vec3::new(0.0, 0.0, 5.0)),
            (2usize, Vec3::new(0.0, 0.0, 2.0)),
            (3usize, Vec3::new(0.0, 0.0, 8.0)),
        ];
        let hit = nearest_along_ray(Vec3::ZERO, Vec3::Z, 0.5, candidates.into_iter());
        assert_eq!(hit, Some(2));
    }

    #[test]
    fn nearest_along_ray_ignores_candidates_behind_the_origin() {
        let candidates = vec![(1usize, Vec3::new(0.0, 0.0, -5.0))];
        let hit = nearest_along_ray(Vec3::ZERO, Vec3::Z, 0.5, candidates.into_iter());
        assert!(hit.is_none());
    }

    #[test]
    fn nearest_along_ray_ignores_candidates_outside_radius() {
        let candidates = vec![(1usize, Vec3::new(2.0, 0.0, 5.0))];
        let hit = nearest_along_ray(Vec3::ZERO, Vec3::Z, 0.5, candidates.into_iter());
        assert!(hit.is_none());
    }

    #[test]
    fn nearest_along_ray_supports_compound_payloads() {
        // Mirrors the real handle-pick payload: `(anchor_index, side)`.
        use super::super::dispatch::HandleSide;
        let candidates = vec![
            ((0usize, HandleSide::In), Vec3::new(0.0, 0.0, 3.0)),
            ((0usize, HandleSide::Out), Vec3::new(0.0, 0.0, 1.0)),
        ];
        let hit = nearest_along_ray(Vec3::ZERO, Vec3::Z, 0.5, candidates.into_iter());
        assert_eq!(hit, Some((0, HandleSide::Out)));
    }

    fn zig() -> Vec<Vec3> {
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 10.0),
        ]
    }

    #[test]
    fn nearest_segment_xz_picks_the_closer_leg() {
        assert_eq!(
            nearest_segment_xz(&zig(), Vec3::new(5.0, 0.0, 0.1)),
            Some(0)
        );
        assert_eq!(
            nearest_segment_xz(&zig(), Vec3::new(10.1, 0.0, 5.0)),
            Some(1)
        );
    }

    #[test]
    fn nearest_segment_xz_needs_at_least_two_points() {
        assert_eq!(nearest_segment_xz(&[Vec3::ZERO], Vec3::ZERO), None);
        assert_eq!(nearest_segment_xz(&[], Vec3::ZERO), None);
    }

    fn square() -> Vec<[f32; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn point_in_footprint_true_inside_false_outside() {
        assert!(point_in_footprint([5.0, 5.0], &square()));
        assert!(!point_in_footprint([50.0, 50.0], &square()));
    }

    #[test]
    fn point_in_footprint_degenerate_footprints_never_contain() {
        assert!(!point_in_footprint([0.0, 0.0], &[]));
        assert!(!point_in_footprint([0.0, 0.0], &[[0.0, 0.0]]));
        assert!(!point_in_footprint([0.0, 0.0], &[[0.0, 0.0], [1.0, 1.0]]));
    }

    #[test]
    fn point_in_footprint_degenerate_zero_area_footprint_never_contains() {
        // A collinear (zero-area) "polygon" never contains an interior point.
        let line = vec![[0.0, 0.0], [5.0, 0.0], [10.0, 0.0]];
        assert!(!point_in_footprint([5.0, 0.0], &line));
    }

    fn sample_proposals() -> Vec<crate::terrain_proposal_state::ProposalRecord> {
        use crate::terrain_proposal_state::{ProposalOp, ProposalRecord};
        vec![
            ProposalRecord {
                id: "p1".into(),
                op: ProposalOp::Raise,
                footprint: square(),
                target_height: None,
                delta: Some(1.0),
            },
            ProposalRecord {
                id: "p2".into(),
                op: ProposalOp::Lower,
                footprint: vec![[20.0, 20.0], [30.0, 20.0], [30.0, 30.0], [20.0, 30.0]],
                target_height: None,
                delta: Some(-1.0),
            },
        ]
    }

    #[test]
    fn pick_ground_proposal_finds_containing_footprint() {
        assert_eq!(
            pick_ground_proposal(&sample_proposals(), [5.0, 5.0]),
            Some("p1".to_string())
        );
        assert_eq!(
            pick_ground_proposal(&sample_proposals(), [25.0, 25.0]),
            Some("p2".to_string())
        );
    }

    #[test]
    fn pick_ground_proposal_none_outside_every_footprint() {
        assert_eq!(
            pick_ground_proposal(&sample_proposals(), [100.0, 100.0]),
            None
        );
    }

    #[test]
    fn pick_ground_proposal_empty_set_is_none() {
        assert_eq!(pick_ground_proposal(&[], [0.0, 0.0]), None);
    }

    // --- F10: right-click must agree with left-click's PathSegment tier ---

    #[test]
    fn an_occluded_edited_ribbon_still_classifies_as_a_path_segment() {
        // The ray hits an unrelated node at t=5 and the edited track's ribbon
        // behind it at t=20. Left-click selects the SEGMENT there (PathSegment
        // claims ahead of NodePick, no depth test), so the right-click menu
        // must label a segment too — the disagreement this finding names.
        assert_eq!(
            prefer_edited_ribbon(Some(5.0), Some(20.0)),
            HitChoice::EditedRibbon
        );
        // …and the winning hit point names a real segment index, which is what
        // `classify_context_menu` writes as `HitTarget::PathSegment`.
        let hit_point = Vec3::ZERO + Vec3::X * 20.0;
        let points = vec![
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(30.0, 0.0, 0.0),
        ];
        assert_eq!(nearest_segment_xz(&points, hit_point), Some(1));
    }

    #[test]
    fn an_edited_ribbon_in_front_also_wins_the_tier() {
        // The nearer-ribbon case must resolve identically — the tier does not
        // depend on which hit happened to be closer.
        assert_eq!(
            prefer_edited_ribbon(Some(20.0), Some(5.0)),
            HitChoice::EditedRibbon
        );
    }

    #[test]
    fn the_nearest_hit_wins_when_the_ray_never_touched_the_edited_ribbon() {
        // No edited track open, or the ray missed its ribbon: unchanged
        // nearest-hit behavior for plain node/stamp picks.
        assert_eq!(prefer_edited_ribbon(Some(5.0), None), HitChoice::Nearest);
        assert_eq!(prefer_edited_ribbon(None, None), HitChoice::None);
    }

    #[test]
    fn an_edited_ribbon_hit_resolves_even_with_no_other_hit() {
        // Defensive: `edited` is a subset of `nearest`, so a `None` nearest
        // with a `Some` edited cannot happen — but it must not fall to Empty.
        assert_eq!(
            prefer_edited_ribbon(None, Some(3.0)),
            HitChoice::EditedRibbon,
            "an edited-ribbon hit must never fall through to Empty"
        );
    }
}
