//! Gimbal gizmo rendering — pure visual, no state.
//!
//! All interaction logic and selection state lives in [`crate::node_manager`].
//! This module only knows how to *draw* the coloured axis handles for a given
//! entity, active tool, and optionally highlighted axis.

use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;

use crate::panels::toolbar::Tool;

// ---------------------------------------------------------------------------
// Custom gizmo group — renders on top of all 3D geometry
// ---------------------------------------------------------------------------

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct GimbalGizmoGroup;

/// Call once at Startup to configure the gimbal gizmo group to draw on top.
pub fn configure_gimbal_gizmos(mut config_store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = config_store.config_mut::<GimbalGizmoGroup>();
    cfg.depth_bias = -1.0; // always render in front of meshes
    cfg.line.width = 2.5;
}

// ---------------------------------------------------------------------------
// Public types (used by node_manager for drag state)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GimbalAxis {
    X,
    Y,
    Z,
}

// ---------------------------------------------------------------------------
// Visual constants
// ---------------------------------------------------------------------------

pub const GIMBAL_LEN: f32 = 1.5;
const COLOR_X: Color = Color::srgb(0.9, 0.2, 0.2);
const COLOR_Y: Color = Color::srgb(0.2, 0.9, 0.2);
const COLOR_Z: Color = Color::srgb(0.3, 0.4, 0.9);
const COLOR_HOVER: Color = Color::srgb(1.0, 1.0, 0.2);

// ---------------------------------------------------------------------------
// Public drawing entry point (called by node_manager's draw system)
// ---------------------------------------------------------------------------

/// First value `visit` resolves across `root` and its descendants (iterative
/// DFS), or `None`. Pure over the two lookups so the descendant traversal is
/// unit-testable without a Bevy App.
fn find_in_subtree<T>(
    root: Entity,
    mut visit: impl FnMut(Entity) -> Option<T>,
    children_of: impl Fn(Entity) -> Vec<Entity>,
) -> Option<T> {
    let mut stack = vec![root];
    while let Some(ent) = stack.pop() {
        if let Some(v) = visit(ent) {
            return Some(v);
        }
        stack.extend(children_of(ent));
    }
    None
}

/// Compute the world-space center of the gimbal for an entity.
///
/// If the entity (or ANY descendant) has a Bevy `Aabb`, the gimbal is placed at
/// that AABB center transformed to world space. Otherwise it falls back to the
/// entity's `GlobalTransform` translation. The walk descends the full subtree
/// (not just immediate children): glTF `SceneRoot`s nest the mesh + `Aabb`
/// several levels down, so a one-level scan left the gimbal stuck at the
/// SceneRoot origin for every GLB.
pub fn gimbal_center(
    entity: Entity,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&bevy::camera::primitives::Aabb>,
    children_query: &Query<&Children>,
) -> Option<Vec3> {
    let g_tx = g_transform_query.get(entity).ok()?;
    find_in_subtree(
        entity,
        |ent| {
            let ent_gtx = g_transform_query.get(ent).ok()?;
            let aabb = aabb_query.get(ent).ok()?;
            Some(ent_gtx.transform_point(aabb.center.into()))
        },
        |ent| {
            children_query
                .get(ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        },
    )
    .or(Some(g_tx.translation()))
}

/// World-space `(min, max)` box enclosing every `Aabb` in `entity`'s subtree —
/// the same DFS as [`gimbal_center`], but visiting all nodes rather than
/// stopping at the first; `None` when no node carries an `Aabb`. Agent-vision
/// `focus` framing uses it (fractalengine `src/AGENTS.md` §vision).
pub fn subtree_world_aabb(
    entity: Entity,
    g_transform_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&bevy::camera::primitives::Aabb>,
    children_query: &Query<&Children>,
) -> Option<(Vec3, Vec3)> {
    let mut bounds: Option<(Vec3, Vec3)> = None;
    // `visit` never returns `Some`, so the walk covers the whole subtree.
    find_in_subtree::<()>(
        entity,
        |ent| {
            if let (Ok(ent_gtx), Ok(aabb)) = (g_transform_query.get(ent), aabb_query.get(ent)) {
                let (lo, hi) = world_box(ent_gtx, aabb);
                bounds = Some(match bounds {
                    Some((min, max)) => (min.min(lo), max.max(hi)),
                    None => (lo, hi),
                });
            }
            None
        },
        |ent| {
            children_query
                .get(ent)
                .map(|c| c.iter().collect())
                .unwrap_or_default()
        },
    );
    bounds
}

/// World-space `(min, max)` of a local `Aabb`'s eight corners under `transform`.
fn world_box(transform: &GlobalTransform, aabb: &bevy::camera::primitives::Aabb) -> (Vec3, Vec3) {
    let center = Vec3::from(aabb.center);
    let half = Vec3::from(aabb.half_extents);
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for corner in 0..8u8 {
        let sign = |bit: u8| if corner & bit == 0 { -1.0 } else { 1.0 };
        let local = center + half * Vec3::new(sign(1), sign(2), sign(4));
        let world = transform.transform_point(local);
        min = min.min(world);
        max = max.max(world);
    }
    (min, max)
}

/// Draw the gimbal handles at `center` given the current `tool` and which
/// axis (if any) is highlighted (`highlight` covers both hover and drag).
pub fn draw_gimbal(
    center: Vec3,
    tool: Tool,
    highlight: Option<GimbalAxis>,
    mut gizmos: Gizmos<GimbalGizmoGroup>,
) {
    if matches!(tool, Tool::Select | Tool::Brush | Tool::Measure) {
        return;
    }

    let cx = if highlight == Some(GimbalAxis::X) {
        COLOR_HOVER
    } else {
        COLOR_X
    };
    let cy = if highlight == Some(GimbalAxis::Y) {
        COLOR_HOVER
    } else {
        COLOR_Y
    };
    let cz = if highlight == Some(GimbalAxis::Z) {
        COLOR_HOVER
    } else {
        COLOR_Z
    };

    match tool {
        // Pen is a path-drawing mode, not a transform gizmo — draw nothing.
        Tool::Select | Tool::Pen | Tool::Brush | Tool::Measure => {}
        Tool::Move => {
            draw_move_handle(&mut gizmos, center, Vec3::X, cx);
            draw_move_handle(&mut gizmos, center, Vec3::Y, cy);
            draw_move_handle(&mut gizmos, center, Vec3::Z, cz);
        }
        Tool::Rotate => {
            draw_rotate_ring(&mut gizmos, center, Vec3::X, cx);
            draw_rotate_ring(&mut gizmos, center, Vec3::Y, cy);
            draw_rotate_ring(&mut gizmos, center, Vec3::Z, cz);
        }
        Tool::Scale => {
            draw_scale_handle(&mut gizmos, center, Vec3::X, cx);
            draw_scale_handle(&mut gizmos, center, Vec3::Y, cy);
            draw_scale_handle(&mut gizmos, center, Vec3::Z, cz);
        }
    }
}

/// QA-0006: before this, a selected node's ONLY visible feedback was the
/// Move/Rotate/Scale gimbal — which draws nothing at all in Select tool
/// (`draw_gimbal` returns early for `Tool::Select`), so a plain click-select
/// left the scene looking unchanged. A bright wireframe box, independent of
/// `tool`, gives every selection a visible outline.
pub const COLOR_SELECTION_OUTLINE: Color = Color::srgb(0.1, 0.95, 1.0);

/// Draw the 12-edge wireframe of an axis-aligned world-space box `min..max`
/// (as returned by [`subtree_world_aabb`]) in [`COLOR_SELECTION_OUTLINE`].
pub fn draw_selection_outline(min: Vec3, max: Vec3, gizmos: &mut Gizmos<GimbalGizmoGroup>) {
    let corners = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0), // bottom face
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4), // top face
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7), // verticals joining the two faces
    ];
    for (a, b) in EDGES {
        gizmos.line(corners[a], corners[b], COLOR_SELECTION_OUTLINE);
    }
}

// ---------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------

fn draw_move_handle(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, dir: Vec3, color: Color) {
    let tip = center + dir * GIMBAL_LEN;
    gizmos.line(center, tip, color);
    let perp = if dir.abs().dot(Vec3::Y) < 0.9 {
        dir.cross(Vec3::Y).normalize()
    } else {
        dir.cross(Vec3::X).normalize()
    };
    let back = tip - dir * 0.2;
    gizmos.line(tip, back + perp * 0.12, color);
    gizmos.line(tip, back - perp * 0.12, color);
}

fn draw_rotate_ring(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, axis: Vec3, color: Color) {
    const SEGS: usize = 48;
    let perp1 = if axis.abs().dot(Vec3::Y) < 0.9 {
        axis.cross(Vec3::Y).normalize()
    } else {
        axis.cross(Vec3::X).normalize()
    };
    let perp2 = axis.cross(perp1).normalize();
    let mut prev = center + perp1 * GIMBAL_LEN;
    for i in 1..=SEGS {
        let angle = i as f32 * std::f32::consts::TAU / SEGS as f32;
        let next = center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN;
        gizmos.line(prev, next, color);
        prev = next;
    }
}

fn draw_scale_handle(gizmos: &mut Gizmos<GimbalGizmoGroup>, center: Vec3, dir: Vec3, color: Color) {
    let tip = center + dir * GIMBAL_LEN;
    gizmos.line(center, tip, color);
    let perp1 = if dir.abs().dot(Vec3::Y) < 0.9 {
        dir.cross(Vec3::Y).normalize()
    } else {
        dir.cross(Vec3::X).normalize()
    };
    let perp2 = dir.cross(perp1).normalize();
    let s = 0.1;
    let c1 = tip + (perp1 + perp2) * s;
    let c2 = tip + (perp1 - perp2) * s;
    let c3 = tip + (-perp1 - perp2) * s;
    let c4 = tip + (-perp1 + perp2) * s;
    gizmos.line(c1, c2, color);
    gizmos.line(c2, c3, color);
    gizmos.line(c3, c4, color);
    gizmos.line(c4, c1, color);
}

// ---------------------------------------------------------------------------
// Ring geometry helpers (used by node_manager for hit-testing)
// ---------------------------------------------------------------------------

/// Returns the two perpendicular basis vectors for a rotation ring around `axis`.
pub fn ring_basis(axis: Vec3) -> (Vec3, Vec3) {
    let perp1 = if axis.abs().dot(Vec3::Y) < 0.9 {
        axis.cross(Vec3::Y).normalize()
    } else {
        axis.cross(Vec3::X).normalize()
    };
    let perp2 = axis.cross(perp1).normalize();
    (perp1, perp2)
}

/// Ring sample count used by hover hit-testing (matches `draw_rotate_ring`'s SEGS).
pub const RING_SEGMENTS: usize = 48;

/// Sample points along a rotation ring for screen-space hit testing.
pub fn ring_points(center: Vec3, axis: Vec3, count: usize) -> Vec<Vec3> {
    let (perp1, perp2) = ring_basis(axis);
    (0..count)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::TAU / count as f32;
            center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN
        })
        .collect()
}

/// Allocation-free `ring_points` variant for per-frame hover detection — fills `buf` in place.
pub fn ring_points_buf(center: Vec3, axis: Vec3, buf: &mut [Vec3; RING_SEGMENTS]) {
    let (perp1, perp2) = ring_basis(axis);
    for (i, slot) in buf.iter_mut().enumerate() {
        let angle = i as f32 * std::f32::consts::TAU / RING_SEGMENTS as f32;
        *slot = center + (perp1 * angle.cos() + perp2 * angle.sin()) * GIMBAL_LEN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_points_buf_matches_vec_variant() {
        for axis in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(0.3, 0.8, 0.5).normalize(),
        ] {
            let center = Vec3::new(1.0, -2.0, 3.0);
            let expected = ring_points(center, axis, RING_SEGMENTS);
            let mut buf = [Vec3::ZERO; RING_SEGMENTS];
            ring_points_buf(center, axis, &mut buf);
            assert_eq!(expected.len(), RING_SEGMENTS);
            for (a, b) in expected.iter().zip(buf.iter()) {
                assert!(
                    (*a - *b).length() < 1e-6,
                    "buf variant diverged: {a:?} vs {b:?}"
                );
            }
        }
    }

    #[test]
    fn ring_points_buf_lies_on_ring_of_gimbal_len() {
        let center = Vec3::new(4.0, 5.0, 6.0);
        let mut buf = [Vec3::ZERO; RING_SEGMENTS];
        ring_points_buf(center, Vec3::Y, &mut buf);
        for p in &buf {
            assert!(((*p - center).length() - GIMBAL_LEN).abs() < 1e-5);
            assert!(
                (p.y - center.y).abs() < 1e-5,
                "Y-axis ring must stay in the XZ plane"
            );
        }
    }

    #[test]
    fn find_in_subtree_reaches_a_deep_grandchild() {
        // root → child → grandchild(carries the Aabb). glTF puts the mesh this
        // deep; the immediate-children scan that shipped missed it and the
        // gimbal snapped to the SceneRoot origin instead of the visual center.
        use std::collections::HashMap;
        let ent = Entity::from_bits;
        let children: HashMap<Entity, Vec<Entity>> =
            HashMap::from([(ent(1), vec![ent(2)]), (ent(2), vec![ent(3)])]);
        let found = find_in_subtree(
            ent(1),
            |e| (e == ent(3)).then_some(Vec3::splat(7.0)),
            |e| children.get(&e).cloned().unwrap_or_default(),
        );
        assert_eq!(found, Some(Vec3::splat(7.0)));
    }

    /// A glTF-shaped subtree (root → child → grandchild, each with its own
    /// box and transform) yields ONE world box enclosing every descendant —
    /// not just the first box the DFS meets (that is `gimbal_center`'s job).
    #[test]
    fn subtree_world_aabb_unions_every_descendant_box() {
        use bevy::camera::primitives::Aabb;
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let grandchild = world
            .spawn((
                GlobalTransform::from(Transform::from_xyz(0.0, 10.0, 0.0).with_scale(Vec3::splat(2.0))),
                Aabb::from_min_max(Vec3::splat(-1.0), Vec3::splat(1.0)),
            ))
            .id();
        let child = world
            .spawn((
                GlobalTransform::from(Transform::from_xyz(5.0, 0.0, 0.0)),
                Aabb::from_min_max(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
            ))
            .add_child(grandchild)
            .id();
        let root = world
            .spawn(GlobalTransform::IDENTITY)
            .add_child(child)
            .id();
        let lone = world.spawn(GlobalTransform::IDENTITY).id();

        let (whole, none) = world
            .run_system_once(
                move |transforms: Query<&GlobalTransform>,
                      aabbs: Query<&Aabb>,
                      children: Query<&Children>| {
                    (
                        subtree_world_aabb(root, &transforms, &aabbs, &children),
                        subtree_world_aabb(lone, &transforms, &aabbs, &children),
                    )
                },
            )
            .expect("system runs");
        let (min, max) = whole.expect("the subtree carries boxes");
        // child box: x 4..6, y 0..1, z -1..1; grandchild (scale 2 at y=10): ±2 around (0,10,0).
        assert!((min - Vec3::new(-2.0, 0.0, -2.0)).length() < 1e-5, "{min:?}");
        assert!((max - Vec3::new(6.0, 12.0, 2.0)).length() < 1e-5, "{max:?}");
        assert_eq!(none, None, "no Aabb anywhere in the subtree → None");
    }

    #[test]
    fn find_in_subtree_is_none_when_nothing_matches() {
        let found = find_in_subtree(Entity::from_bits(1), |_| None::<Vec3>, |_| Vec::new());
        assert_eq!(found, None);
    }
}
