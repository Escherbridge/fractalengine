//! pen_curve_tool_20260722 (Phase 2): flatten per-anchor bezier route points into
//! a dense polyline for the ribbon mesh + the pick shape. Pure (`[f32;3]` out, no
//! bevy) so `render_gpx_tracks` (fe-terrain) and `track_pick_shape` (fractalengine)
//! flatten IDENTICALLY — clicks must hit the visible curve. Mirrors the fe-ui
//! de Casteljau (`node_manager/curve.rs`); duplicated here because fe-terrain must
//! not depend on fe-ui. Raw petal-local meters — no `world_scale`. See
//! `fe-terrain/src/mesh/AGENTS.md` §curve.

use crate::iot::animation::TimestampedRoutePoint;

/// Fixed cubic subdivision per handle-carrying segment. Shared by render + pick so
/// the visible ribbon and the clickable polyline are the same geometry. (A later
/// phase may make this adaptive / tool-configurable.)
pub const SAMPLES_PER_SEGMENT: usize = 16;

/// Flatten route anchors into a dense `[f32;3]` polyline. A segment whose BOTH
/// bounding handles are `None` is emitted straight (single endpoint, zero added
/// points) so an all-corner track flattens byte-identically to its legacy polyline;
/// a handle-carrying segment is sampled as the cubic
/// `[P_i, P_i+out_i, P_{i+1}+in_{i+1}, P_{i+1}]`.
pub fn flatten_route(points: &[TimestampedRoutePoint], samples_per_seg: usize) -> Vec<[f32; 3]> {
    if points.len() < 2 {
        return points.iter().map(pos_f32).collect();
    }
    let steps = samples_per_seg.max(1);
    let mut out: Vec<[f32; 3]> = Vec::with_capacity(points.len());
    out.push(pos_f32(&points[0]));
    for i in 0..points.len() - 1 {
        let a = &points[i];
        let b = &points[i + 1];
        let pa = pos_f32(a);
        let pb = pos_f32(b);
        match (a.handle_out, b.handle_in) {
            // Straight segment: passthrough keeps all-corner tracks identical.
            (None, None) => out.push(pb),
            (out_h, in_h) => {
                let c1 = add3(pa, out_h.map(vec_f32).unwrap_or([0.0; 3]));
                let c2 = add3(pb, in_h.map(vec_f32).unwrap_or([0.0; 3]));
                push_cubic(&mut out, [pa, c1, c2, pb], steps);
            }
        }
    }
    out
}

fn pos_f32(p: &TimestampedRoutePoint) -> [f32; 3] {
    [
        p.position[0] as f32,
        p.position[1] as f32,
        p.position[2] as f32,
    ]
}

fn vec_f32(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Append `steps` de Casteljau samples of a cubic (excluding its start point,
/// which the caller already emitted). Mirrors `fe_ui::node_manager::curve::push_cubic`.
fn push_cubic(out: &mut Vec<[f32; 3]>, seg: [[f32; 3]; 4], steps: usize) {
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let a = lerp3(seg[0], seg[1], t);
        let b = lerp3(seg[1], seg[2], t);
        let c = lerp3(seg[2], seg[3], t);
        let d = lerp3(a, b, t);
        let e = lerp3(b, c, t);
        out.push(lerp3(d, e, t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::iot::animation::CornerKind;

    fn corner(pos: [f64; 3]) -> TimestampedRoutePoint {
        TimestampedRoutePoint {
            position: pos,
            ..Default::default()
        }
    }

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps && (a[2] - b[2]).abs() < eps
    }

    #[test]
    fn all_corner_track_flattens_to_identical_polyline() {
        // No handles anywhere -> every segment passes through -> point-for-point
        // the legacy polyline (byte-identical mesh + RDP input).
        let pts = vec![
            corner([0.0, 0.0, 0.0]),
            corner([10.0, 0.0, 0.0]),
            corner([10.0, 0.0, 10.0]),
        ];
        let out = flatten_route(&pts, SAMPLES_PER_SEGMENT);
        assert_eq!(
            out,
            vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0]]
        );
    }

    #[test]
    fn fewer_than_two_points_pass_through() {
        assert!(flatten_route(&[], 8).is_empty());
        assert_eq!(
            flatten_route(&[corner([1.0, 2.0, 3.0])], 8),
            vec![[1.0, 2.0, 3.0]]
        );
    }

    #[test]
    fn symmetric_handle_segment_is_subdivided_and_hits_endpoints() {
        let steps = 8;
        let mut a = corner([0.0, 0.0, 0.0]);
        a.handle_out = Some([1.0, 0.0, 1.0]);
        a.corner = CornerKind::Symmetric;
        let mut b = corner([4.0, 0.0, 0.0]);
        b.handle_in = Some([-1.0, 0.0, 1.0]);
        b.corner = CornerKind::Symmetric;
        let out = flatten_route(&[a, b], steps);
        assert_eq!(out.len(), 1 + steps, "1 curved segment = start + steps");
        assert!(close(out[0], [0.0, 0.0, 0.0], 1e-4));
        assert!(close(*out.last().unwrap(), [4.0, 0.0, 0.0], 1e-4));
        // Handles bow the curve off the straight P0->P1 line (+z here).
        assert!(
            out.iter().any(|p| p[2] > 0.1),
            "handles should bow the curve: {out:?}"
        );
    }

    #[test]
    fn one_sided_handle_still_curves() {
        let steps = 6;
        let mut a = corner([0.0, 0.0, 0.0]);
        a.handle_out = Some([2.0, 0.0, 2.0]);
        let b = corner([4.0, 0.0, 0.0]); // no in-handle
        let out = flatten_route(&[a, b], steps);
        assert_eq!(out.len(), 1 + steps);
        assert!(close(*out.last().unwrap(), [4.0, 0.0, 0.0], 1e-4));
    }

    #[test]
    fn mixed_track_only_subdivides_curved_segments() {
        // seg0 straight (passthrough = 1 pt), seg1 curved (steps pts).
        let steps = 5;
        let mut mid = corner([1.0, 0.0, 0.0]);
        mid.handle_out = Some([0.5, 0.0, 0.5]);
        let mut end = corner([2.0, 0.0, 0.0]);
        end.handle_in = Some([-0.5, 0.0, 0.5]);
        let out = flatten_route(&[corner([0.0, 0.0, 0.0]), mid, end], steps);
        assert_eq!(out.len(), 1 + 1 + steps, "start + passthrough + curved");
    }

    #[test]
    fn positions_pass_through_without_scaling() {
        // Large meter-scale coordinates are preserved exactly (no world_scale).
        let out = flatten_route(
            &[corner([1234.5, 6.0, -789.0]), corner([1235.5, 6.0, -789.0])],
            4,
        );
        assert!(close(out[0], [1234.5, 6.0, -789.0], 1e-2));
        assert!(close(*out.last().unwrap(), [1235.5, 6.0, -789.0], 1e-2));
    }
}
