//! Viewport click → select / deselect nearest spawned node. Claims `NodePick`.
//! Precise ray/AABB pick against glb geometry — see `node_manager/AGENTS.md`
//! §glb-mesh-picking.

use bevy::camera::primitives::Aabb;
use bevy::math::Affine3A;
use bevy::prelude::*;

use super::dispatch::{resolve_operation, HitTarget, Operation};
use super::path_segment_interaction::{ray_polyline_hit, TrackPickShape};
use super::router::{ClickArbiter, ClickPriority};
use super::selection::SelectionKind;
use super::NodeManager;
use crate::navigation_manager::NavigationManager;
use crate::plugin::{SpawnedNodeMarker, ToolState};
use crate::visibility::{effective_visibility, NodeAncestry, VisibilityState};

/// Whether a node may win a viewport pick: an EFFECTIVELY hidden node (own
/// override, hidden ancestor scope, group, or a solo lens it is not in) is not
/// pickable — ratified D-13/D-14, `hierarchy_visibility_groups_20260808`. Pure
/// wrapper so the picker's own rule is named and testable, rather than an
/// inline call to someone else's resolver.
///
/// NOTE: this is the minimal D-13/D-14 slice. The remaining pick surfaces —
/// `context_pick` (right-click), `pointer`, `sidebar_sync`, and the path
/// segment/vertex/handle filters — stay Phase-2 work of that track.
fn node_is_pickable(node_id: &str, ancestry: NodeAncestry, visibility: &VisibilityState) -> bool {
    effective_visibility(node_id, ancestry, visibility)
}

pub(super) fn handle_viewport_click(
    node_query: Query<(Entity, &SpawnedNodeMarker, Option<&TrackPickShape>)>,
    g_transform_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    mut manager: ResMut<NodeManager>,
    nav: Res<NavigationManager>,
    tool: Res<ToolState>,
    visibility: Res<VisibilityState>,
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
    // Built exactly as `visibility::sync_node_visibility` builds it — same
    // resource, same active-scope fallbacks, so the picker and the renderer
    // can never disagree about what "hidden" means.
    let ancestry = NodeAncestry {
        verse_id: nav.active_verse_id.as_deref().unwrap_or(""),
        fractal_id: nav.active_fractal_id.as_deref().unwrap_or(""),
        petal_id: nav.active_petal_id.as_deref().unwrap_or(""),
    };
    let mut best: Option<(Entity, f32, String)> = None;

    for (entity, marker, pick_shape) in node_query.iter() {
        if active_petal
            .map(|pid| pid != marker.petal_id.as_str())
            .unwrap_or(false)
        {
            continue;
        }
        if !node_is_pickable(&marker.node_id, ancestry, &visibility) {
            continue;
        }
        // FR-1: a rendered track ribbon carries `TrackPickShape` (its actual
        // polyline). Narrow-phase ray-vs-segment against it instead of the giant
        // flat AABB, so a km-scale track no longer swallows clicks for nearby
        // objects. `t` is the closest-approach distance along the ray, directly
        // comparable to the AABB entry `t` below (nearer objects still win).
        let t = if let Some(shape) = pick_shape {
            ray_polyline_hit(
                &shape.points,
                ray.origin,
                *ray.direction,
                shape.half_width,
                &shape.fill_triangles,
            )
        } else {
            // Resolve the pickable Aabb: glTF scenes place it on a child mesh,
            // not the root marker entity — mirror `gimbal_center`'s
            // entity-then-children walk (`gimbal.rs`), but slab-test instead of
            // centering. Whatever child we hit, selection resolves to this root
            // `entity` (FR-2).
            pick_node_aabb(
                entity,
                &ray,
                &g_transform_query,
                &aabb_query,
                &children_query,
            )
        };
        let Some(t) = t else { continue };
        if best.as_ref().is_none_or(|b| t < b.1) {
            best = Some((entity, t, marker.node_id.clone()));
        }
    }

    // Route the select/deselect decision through the FR-2 dispatch table
    // (no-bypass): a node hit resolves to `SelectNode`, an empty hit to
    // `Deselect`, in every tool but Pen — and Pen never reaches here (its
    // higher-priority `PathPlace`/`PathMarker` claim wins the frame first). A
    // `Node`/`Empty` hit resolves selection-independently, so the projected
    // kind is immaterial and `Empty` stands in.
    let hit = match best.as_ref() {
        Some((entity, _, _)) => HitTarget::Node(*entity),
        None => HitTarget::Empty,
    };
    match resolve_operation(tool.active_tool, &SelectionKind::Empty, hit) {
        Operation::SelectNode(_) => {
            if let Some((entity, _, node_id)) = best {
                manager.select(entity, node_id);
            }
        }
        Operation::Deselect => manager.deselect(),
        // The only other verbs a `Node`/`Empty` hit can produce here are Pen's
        // `PlacePathPoint` (unreachable — `PathPlace` claims the frame first)
        // and Brush's `None` (unreachable — `Brush` claims 2nd in the chain).
        // The wildcard keeps the match total over the whole `Operation` set.
        _ => {}
    }
}

/// Nearest per-entity hit `t` across `root` and ALL its descendants (iterative
/// DFS). Pure over the two lookups so the *depth* traversal — the crux of the
/// glTF-scene fix below — is unit-testable without a Bevy App. `hit_of` yields a
/// candidate along-ray distance for an entity's own geometry (if any);
/// `children_of` yields its direct children.
fn nearest_in_subtree(
    root: Entity,
    hit_of: impl Fn(Entity) -> Option<f32>,
    children_of: impl Fn(Entity) -> Vec<Entity>,
) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut stack = vec![root];
    while let Some(ent) = stack.pop() {
        if let Some(t) = hit_of(ent) {
            if best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        }
        stack.extend(children_of(ent));
    }
    best
}

/// Nearest along-ray hit `t` for a node's geometry Aabb, or `None` on a miss.
///
/// Walks the root `entity` AND every descendant (not just immediate children):
/// glTF `SceneRoot`s nest the mesh + `Aabb` several levels down, so a one-level
/// scan finds nothing and silently breaks GLB selection (the shipped
/// regression). Returns the smallest entry distance across all candidates so
/// overlapping child meshes resolve to the closest surface. `t` is measured
/// along the world-space ray direction and is comparable across entities.
/// `pub(super)`: shared with the right-click classifier (`context_pick`).
pub(super) fn pick_node_aabb(
    entity: Entity,
    ray: &Ray3d,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&Aabb>,
    children_query: &Query<&Children>,
) -> Option<f32> {
    nearest_in_subtree(
        entity,
        |ent| {
            let g_tx = g_transform_query.get(ent).ok()?;
            let aabb = aabb_query.get(ent).ok()?;
            ray_aabb_hit(
                ray.origin,
                *ray.direction,
                g_tx.affine(),
                Vec3::from(aabb.center),
                Vec3::from(aabb.half_extents),
            )
        },
        |ent| {
            children_query
                .get(ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        },
    )
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

    /// A unit box centered at the origin in identity local space.
    fn unit_box() -> (Affine3A, Vec3, Vec3) {
        (Affine3A::IDENTITY, Vec3::ZERO, Vec3::splat(0.5))
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
        let t = ray_aabb_hit(
            Vec3::new(20.0, 0.0, 0.0),
            -Vec3::X,
            aff,
            Vec3::ZERO,
            Vec3::splat(0.5),
        );
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

    // --- descendant traversal (the GLB-selection depth fix) ---

    use std::collections::HashMap;

    fn ent(n: u64) -> Entity {
        Entity::from_bits(n)
    }

    #[test]
    fn subtree_finds_hit_on_a_deep_grandchild() {
        // root(no geometry) → child(no geometry) → grandchild(hit @ t=5). glTF
        // SceneRoots put the Aabb this deep; the old immediate-children scan
        // returned None here and deselected every GLB. The DFS must find it.
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2)]), (ent(2), vec![ent(3)])]);
        let hits: HashMap<Entity, f32> = HashMap::from([(ent(3), 5.0)]);
        let t = nearest_in_subtree(
            ent(1),
            |e| hits.get(&e).copied(),
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, Some(5.0));
    }

    #[test]
    fn subtree_keeps_the_closest_hit_across_depths() {
        // Immediate child hits at t=9, its own child (grandchild) at t=3 — the
        // nearer surface, even though it's deeper, must win.
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2)]), (ent(2), vec![ent(3)])]);
        let hits: HashMap<Entity, f32> = HashMap::from([(ent(2), 9.0), (ent(3), 3.0)]);
        let t = nearest_in_subtree(
            ent(1),
            |e| hits.get(&e).copied(),
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, Some(3.0));
    }

    // --- D-13/D-14: hidden nodes are not pickable ---

    #[test]
    fn a_hidden_node_is_not_pickable_and_its_visible_sibling_still_is() {
        use crate::visibility::OverrideState;

        let ancestry = NodeAncestry {
            verse_id: "v1",
            fractal_id: "f1",
            petal_id: "p1",
        };
        let mut visibility = VisibilityState::default();
        assert!(node_is_pickable("n1", ancestry, &visibility));
        visibility
            .node_overrides
            .insert("n1".into(), OverrideState::Hide);
        assert!(!node_is_pickable("n1", ancestry, &visibility));
        assert!(
            node_is_pickable("n2", ancestry, &visibility),
            "only the overridden node drops out of the pick set"
        );
    }

    #[test]
    fn a_node_under_a_hidden_petal_is_not_pickable() {
        // Ancestor-scope hiding reaches the picker too, not just the renderer.
        let ancestry = NodeAncestry {
            verse_id: "v1",
            fractal_id: "f1",
            petal_id: "p1",
        };
        let mut visibility = VisibilityState::default();
        visibility.hidden_petals.insert("p1".into());
        assert!(!node_is_pickable("n1", ancestry, &visibility));
    }

    #[test]
    fn subtree_is_none_when_no_entity_has_geometry() {
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2), ent(3)])]);
        let t = nearest_in_subtree(
            ent(1),
            |_| None,
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(t, None);
    }
}
