//! Viewport click → select / deselect nearest spawned node. Claims `NodePick`.
//! Precise ray/AABB pick against glb geometry — see `node_manager/AGENTS.md`
//! §glb-mesh-picking.

use bevy::camera::primitives::Aabb;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::router::{ClickArbiter, ClickPriority};
use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::SpawnedNodeMarker;

pub(super) fn handle_viewport_click(
    node_query: Query<(Entity, &SpawnedNodeMarker)>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    mut arbiter: ResMut<ClickArbiter>,
) {
    // Only act on a fresh left-press that reached the viewport (egui/rect gating
    // already applied by `resolve_pointer_frame`).
    if !arbiter.is_fresh_press() {
        return;
    }
    // Yield if a higher-priority consumer (gimbal / path-point) already claimed
    // this frame's click.
    if !arbiter.claim(ClickPriority::NodePick) {
        return;
    }
    let Some(ray) = arbiter.ray() else { return };

    let active_petal = nav.active_petal_id.as_deref();
    let mut best: Option<(Entity, f32, String)> = None;

    for (entity, marker) in node_query.iter() {
        if active_petal
            .map(|pid| pid != marker.petal_id.as_str())
            .unwrap_or(false)
        {
            continue;
        }
        // Resolve the pickable Aabb: glTF scenes place it on a child mesh, not
        // the root marker entity — mirror `gimbal_center`'s entity-then-children
        // walk (`gimbal.rs`), but slab-test instead of centering. Whatever child
        // we hit, selection resolves to this root `entity` (FR-2).
        let Some(t) = pick_node_aabb(entity, &ray, &g_transform_query, &aabb_query, &children_query)
        else {
            continue;
        };
        if best.as_ref().map_or(true, |b| t < b.1) {
            best = Some((entity, t, marker.node_id.clone()));
        }
    }

    if let Some((entity, _, node_id)) = best {
        manager.select(entity, node_id);
    } else {
        manager.deselect();
    }
}

/// Open a track for editing when it becomes the viewport selection.
///
/// The viewport picker (`handle_viewport_click`) only touches `NodeManager`;
/// the Paths tab's `editing_track_id` is independent. So when the newly
/// selected node is one of the Paths-tab tracks (`PathEditorState.tracks`),
/// dispatch `PathSelectTrack` so clicking a rendered ribbon also opens it for
/// editing. fe-ui can't see fe-terrain's `GpxTrackLine`, so membership in the
/// track list is the crate-visible "is this a track?" test. A `Local`
/// remembers the last selection so this only fires on a *change* (not every
/// frame), and it skips when that track is already being edited so it never
/// clobbers the in-flight point buffer. See `node_manager/AGENTS.md`
/// §track-picking.
pub(super) fn open_track_on_select(
    manager: Res<NodeManager>,
    path_state: Res<crate::gis::PathEditorState>,
    mut ui_mgr: ResMut<crate::actions::UiManager>,
    mut last_selected: Local<Option<String>>,
) {
    let current = manager.selected.as_ref().map(|s| s.node_id.clone());
    if current == *last_selected {
        return; // selection unchanged this frame
    }
    *last_selected = current.clone();

    let Some(node_id) = current else { return };
    if !track_to_open(&node_id, &path_state) {
        return;
    }
    ui_mgr.push_action(crate::actions::UiAction::PathSelectTrack { track_node_id: node_id });
}

/// `true` when `node_id` names a Paths-tab track that isn't already the one
/// being edited — i.e. selecting it should open it for editing. Pure so the
/// membership/guard logic is unit-testable without a Bevy App.
fn track_to_open(node_id: &str, path_state: &crate::gis::PathEditorState) -> bool {
    if path_state.editing_track_id.as_deref() == Some(node_id) {
        return false; // already editing this track — don't re-open (clobbers buffer)
    }
    path_state.tracks.iter().any(|t| t.node_id == node_id)
}

/// Nearest along-ray hit `t` for a node's geometry Aabb, or `None` on a miss.
///
/// Tries the root `entity`'s own `Aabb` first, then scans immediate children
/// (glTF loaders attach the mesh + `Aabb` to a child of the `SceneRoot`), and
/// returns the smallest entry distance across all candidates so overlapping
/// child meshes resolve to the closest surface. `t` is measured along the
/// world-space ray direction and is comparable across entities.
fn pick_node_aabb(
    entity: Entity,
    ray: &Ray3d,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&Aabb>,
    children_query: &Query<&Children>,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut consider = |ent: Entity| {
        if let (Ok(g_tx), Ok(aabb)) = (g_transform_query.get(ent), aabb_query.get(ent)) {
            if let Some(t) = ray_aabb_hit(
                ray.origin,
                *ray.direction,
                g_tx.affine(),
                Vec3::from(aabb.center),
                Vec3::from(aabb.half_extents),
            ) {
                if best.map_or(true, |b| t < b) {
                    best = Some(t);
                }
            }
        }
    };

    consider(entity);
    if let Ok(children) = children_query.get(entity) {
        for child in children.iter() {
            consider(child);
        }
    }
    best
}

/// Ray/AABB slab intersection in the box's local space.
///
/// The `Aabb` is axis-aligned in the entity's local space (`center ±
/// half_extents`); transforming the world ray into that space via the inverse
/// `affine` turns the world-space OBB test into a plain slab test while keeping
/// the parametric `t` identical to the along-world-ray distance (both origin
/// and direction share the same transform). Returns the entry `t` when the ray
/// enters the box in front of its origin, else `None` (miss or box behind).
fn ray_aabb_hit(
    origin: Vec3,
    dir: Vec3,
    affine: Affine3A,
    center: Vec3,
    half_extents: Vec3,
) -> Option<f32> {
    let inv = affine.inverse();
    let local_origin = inv.transform_point3(origin);
    // Direction is a vector, not a point — do NOT normalize: sharing the world
    // ray's magnitude keeps `t` comparable across entities.
    let local_dir = inv.transform_vector3(dir);

    let min = center - half_extents;
    let max = center + half_extents;

    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;

    for axis in 0..3 {
        let o = local_origin[axis];
        let d = local_dir[axis];
        let lo = min[axis];
        let hi = max[axis];
        if d.abs() < 1e-8 {
            // Ray parallel to this slab: miss if the origin is outside it.
            if o < lo || o > hi {
                return None;
            }
        } else {
            let inv_d = 1.0 / d;
            let mut t0 = (lo - o) * inv_d;
            let mut t1 = (hi - o) * inv_d;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            t_enter = t_enter.max(t0);
            t_exit = t_exit.min(t1);
            if t_enter > t_exit {
                return None;
            }
        }
    }

    // Box is entirely behind the ray origin.
    if t_exit < 0.0 {
        return None;
    }
    // Entry distance, clamped to 0 when the origin is already inside the box.
    Some(t_enter.max(0.0))
}

// ---------------------------------------------------------------------------
// Tests — pure ray/AABB math (Bevy-App-free). Mesh-picking correctness is
// validated in-app; only the geometry helper is unit-tested here.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gis::{GisResultRow, PathEditorState};

    /// A unit box centered at the origin in identity local space.
    fn unit_box() -> (Affine3A, Vec3, Vec3) {
        (Affine3A::IDENTITY, Vec3::ZERO, Vec3::splat(0.5))
    }

    fn track_row(node_id: &str) -> GisResultRow {
        GisResultRow {
            node_id: node_id.to_string(),
            name: node_id.to_string(),
            position: [0.0, 0.0, 0.0],
            annotation_title: None,
            annotation_color: None,
        }
    }

    #[test]
    fn track_to_open_true_for_listed_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a"), track_row("track-b")];
        assert!(track_to_open("track-a", &state));
        assert!(track_to_open("track-b", &state));
    }

    #[test]
    fn track_to_open_false_for_unlisted_node() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        assert!(!track_to_open("some-plain-node", &state));
    }

    #[test]
    fn track_to_open_false_when_already_editing_that_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a")];
        state.editing_track_id = Some("track-a".to_string());
        // Already editing it — re-opening would clobber the point buffer.
        assert!(!track_to_open("track-a", &state));
    }

    #[test]
    fn track_to_open_true_when_editing_a_different_track() {
        let mut state = PathEditorState::default();
        state.tracks = vec![track_row("track-a"), track_row("track-b")];
        state.editing_track_id = Some("track-a".to_string());
        // Switching to a different listed track should open it.
        assert!(track_to_open("track-b", &state));
    }

    #[test]
    fn hits_box_straight_on() {
        let (aff, c, he) = unit_box();
        // Ray from z=+5 looking toward -z straight at the box center.
        let t = ray_aabb_hit(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, aff, c, he);
        // Enters the front face at z = 0.5, i.e. 4.5 units along the ray.
        assert!(t.is_some());
        assert!((t.unwrap() - 4.5).abs() < 1e-4, "t = {:?}", t);
    }

    #[test]
    fn misses_box_beside_it() {
        let (aff, c, he) = unit_box();
        // Parallel ray offset well outside the box on x.
        let t = ray_aabb_hit(Vec3::new(5.0, 0.0, 5.0), -Vec3::Z, aff, c, he);
        assert!(t.is_none(), "expected miss, got {:?}", t);
    }

    #[test]
    fn misses_box_behind_origin() {
        let (aff, c, he) = unit_box();
        // Ray points AWAY from the box (+z) so the box is entirely behind.
        let t = ray_aabb_hit(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, aff, c, he);
        assert!(t.is_none(), "expected miss (box behind), got {:?}", t);
    }

    #[test]
    fn origin_inside_box_returns_zero() {
        let (aff, c, he) = unit_box();
        let t = ray_aabb_hit(Vec3::ZERO, Vec3::X, aff, c, he);
        assert_eq!(t, Some(0.0));
    }

    #[test]
    fn respects_translation_offset() {
        // Box translated to x = 10; a ray from the origin along +x should hit
        // its near face at t = 9.5 (box spans x ∈ [9.5, 10.5]).
        let aff = Affine3A::from_translation(Vec3::new(10.0, 0.0, 0.0));
        let t = ray_aabb_hit(Vec3::ZERO, Vec3::X, aff, Vec3::ZERO, Vec3::splat(0.5));
        assert!(t.is_some());
        assert!((t.unwrap() - 9.5).abs() < 1e-4, "t = {:?}", t);
    }

    #[test]
    fn respects_scale() {
        // A 10× scaled unit box spans x ∈ [-5, 5]; ray from x=+20 toward -x
        // enters at x = 5, i.e. 15 units along the ray.
        let aff = Affine3A::from_scale(Vec3::splat(10.0));
        let t = ray_aabb_hit(Vec3::new(20.0, 0.0, 0.0), -Vec3::X, aff, Vec3::ZERO, Vec3::splat(0.5));
        assert!(t.is_some());
        assert!((t.unwrap() - 15.0).abs() < 1e-3, "t = {:?}", t);
    }

    #[test]
    fn picks_nearest_of_two_hits() {
        // Two boxes along the ray: nearer one at z=0, farther at z=-10. The
        // helper reports each box's own entry t; the system keeps the min.
        let near = ray_aabb_hit(
            Vec3::new(0.0, 0.0, 5.0),
            -Vec3::Z,
            Affine3A::IDENTITY,
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
        let far = ray_aabb_hit(
            Vec3::new(0.0, 0.0, 5.0),
            -Vec3::Z,
            Affine3A::from_translation(Vec3::new(0.0, 0.0, -10.0)),
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
        assert!(near.unwrap() < far.unwrap());
    }
}
