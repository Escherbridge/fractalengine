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

// ---------------------------------------------------------------------------
// stamped_asset_nodes_20260725 (T2 FR-1): arc-length sampling of the DENSE
// flattened route so path-asset stamps sit ON the bezier curve (true tangent),
// not on the anchor chords. Callers flatten anchors via `flatten_route` FIRST,
// then sample. Pure `[f32;3]` meters — no `world_scale` here (N-1). See
// `fe-terrain/src/mesh/AGENTS.md` §curve-stamps.
// ---------------------------------------------------------------------------

/// Hard cap on stamp offsets a single sampler call yields — a tiny spacing over a
/// long path can't allocate unbounded offsets. Mirrors the fe-ui `MAX_STAMPS`.
pub const MAX_STAMP_OFFSETS: usize = 4096;

/// Per-vertex cumulative arc length along a dense polyline (index 0 = 0.0) plus
/// the total length. Empty → `(vec![], 0.0)`; single vertex → `(vec![0.0], 0.0)`.
pub fn arc_length_table(dense: &[[f32; 3]]) -> (Vec<f32>, f32) {
    let mut cum = Vec::with_capacity(dense.len());
    if dense.is_empty() {
        return (cum, 0.0);
    }
    cum.push(0.0);
    let mut total = 0.0;
    for pair in dense.windows(2) {
        total += dist3(&pair[0], &pair[1]);
        cum.push(total);
    }
    (cum, total)
}

/// Position at absolute arc length `s` (clamped to `[0, total]`) along `dense`,
/// linearly interpolated between the two bracketing dense vertices. Because
/// `dense` is the flattened bezier (not the anchor chords), the result lies ON
/// the visible curve. Empty → origin; single vertex → that vertex.
pub fn position_at_arc_length(dense: &[[f32; 3]], cum: &[f32], total: f32, s: f32) -> [f32; 3] {
    match dense.len() {
        0 => return [0.0, 0.0, 0.0],
        1 => return dense[0],
        _ => {}
    }
    let target = s.clamp(0.0, total.max(0.0));
    // First vertex whose cumulative distance exceeds target → its segment start.
    let seg = cum
        .iter()
        .position(|&d| d > target)
        .unwrap_or(dense.len() - 1)
        .saturating_sub(1)
        .min(dense.len() - 2);
    let seg_start = cum[seg];
    let seg_len = cum[seg + 1] - seg_start;
    let t = if seg_len > 1e-9 {
        (target - seg_start) / seg_len
    } else {
        0.0
    };
    lerp3(dense[seg], dense[seg + 1], t)
}

/// Unit tangent of the curve at absolute arc length `s`, sampled from a short
/// look-ahead along `dense` so it is the TRUE curve tangent (bows with the
/// bezier), not the straight anchor-to-anchor chord. Falls back to a
/// look-behind at the path end; `[0,0,0]` for a degenerate path.
pub fn tangent_at_arc_length(dense: &[[f32; 3]], cum: &[f32], total: f32, s: f32) -> [f32; 3] {
    if dense.len() < 2 || total <= 1e-9 {
        return [0.0, 0.0, 0.0];
    }
    let step = (total * 0.01).max(1e-3).min(total);
    let (from, to) = if s + step <= total {
        (
            position_at_arc_length(dense, cum, total, s),
            position_at_arc_length(dense, cum, total, s + step),
        )
    } else {
        (
            position_at_arc_length(dense, cum, total, (s - step).max(0.0)),
            position_at_arc_length(dense, cum, total, s),
        )
    };
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1e-9 {
        [0.0, 0.0, 0.0]
    } else {
        [d[0] / len, d[1] / len, d[2] / len]
    }
}

/// Yaw (radians, about +Y) that aims the model's glTF `-Z` forward down the
/// curve tangent `(dx, _, dz)`. Matches the fe-ui stamp convention
/// (`dx.atan2(dz)`); `0.0` for a null tangent.
pub fn tangent_yaw(tangent: [f32; 3]) -> f32 {
    if tangent[0].abs() < 1e-9 && tangent[2].abs() < 1e-9 {
        return 0.0;
    }
    tangent[0].atan2(tangent[2])
}

/// Absolute arc-length offsets for fixed-METER `spacing` over `total`:
/// `0, spacing, 2·spacing, …` inclusive of the start, capped at
/// [`MAX_STAMP_OFFSETS`]. Non-positive spacing or total → just the endpoints
/// (start + end), never a divide-by-zero.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // NaN spacing/total must take the degenerate branch
pub fn spacing_offsets(total: f32, spacing: f32) -> Vec<f32> {
    if !(spacing > 0.0) || !(total > 0.0) {
        return if total > 0.0 {
            vec![0.0, total]
        } else {
            vec![0.0]
        };
    }
    let n = (total / spacing).floor() as usize; // segment count
    let count = (n + 1).min(MAX_STAMP_OFFSETS);
    (0..count)
        .map(|i| (i as f32 * spacing).min(total))
        .collect()
}

/// Absolute arc-length offsets for a fixed `count` spread evenly over `total`
/// (endpoints inclusive). `0 → none`, `1 → start only`; capped at
/// [`MAX_STAMP_OFFSETS`]. Scale-invariant (no meters), mirrors FixedCount.
pub fn count_offsets(total: f32, count: usize) -> Vec<f32> {
    match count {
        0 => Vec::new(),
        1 => vec![0.0],
        c => {
            let c = c.min(MAX_STAMP_OFFSETS);
            (0..c).map(|i| i as f32 / (c - 1) as f32 * total).collect()
        }
    }
}

/// FR-1 sampler: `(position, yaw)` stamp transforms at each absolute arc-length
/// `offset` along the DENSE flattened route. Every position lies on the curve;
/// `yaw` is the true curve tangent (meaningful only when the caller
/// tangent-aligns). Pair with `flatten_route` + `spacing_offsets`/`count_offsets`.
pub fn sample_stamps_along_curve(dense: &[[f32; 3]], offsets: &[f32]) -> Vec<([f32; 3], f32)> {
    let (cum, total) = arc_length_table(dense);
    offsets
        .iter()
        .map(|&s| {
            let pos = position_at_arc_length(dense, &cum, total, s);
            let yaw = tangent_yaw(tangent_at_arc_length(dense, &cum, total, s));
            (pos, yaw)
        })
        .collect()
}

fn dist3(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
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

    // ---- FR-1: arc-length curve-follow stamp sampler --------------------

    /// A single bezier segment that bows in +z between two on-x endpoints.
    fn bowed_segment() -> Vec<[f32; 3]> {
        let mut a = corner([0.0, 0.0, 0.0]);
        a.handle_out = Some([2.0, 0.0, 4.0]);
        let mut b = corner([8.0, 0.0, 0.0]);
        b.handle_in = Some([-2.0, 0.0, 4.0]);
        flatten_route(&[a, b], SAMPLES_PER_SEGMENT)
    }

    #[test]
    fn arc_length_table_accumulates_and_totals() {
        let dense = vec![[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [3.0, 0.0, 4.0]];
        let (cum, total) = arc_length_table(&dense);
        assert_eq!(cum, vec![0.0, 3.0, 7.0]);
        assert_eq!(total, 7.0);
        // Degenerate inputs never panic.
        assert_eq!(arc_length_table(&[]), (vec![], 0.0));
        assert_eq!(arc_length_table(&[[1.0, 2.0, 3.0]]), (vec![0.0], 0.0));
    }

    #[test]
    fn stamp_sits_on_the_curve_not_the_chord() {
        // The straight anchor chord midpoint has z≈0; the true curve bows to +z.
        // Sampling the DENSE flattened route must land the mid-stamp on the bow,
        // NOT on the chord — this is the FR-1 curve-follow guarantee.
        let dense = bowed_segment();
        let (cum, total) = arc_length_table(&dense);
        let mid = position_at_arc_length(&dense, &cum, total, total * 0.5);
        assert!(
            mid[2] > 0.5,
            "mid stamp should ride the +z bow, got z={}",
            mid[2]
        );
        // Endpoints are exact.
        assert!(close(
            position_at_arc_length(&dense, &cum, total, 0.0),
            [0.0, 0.0, 0.0],
            1e-3
        ));
        assert!(close(
            position_at_arc_length(&dense, &cum, total, total),
            [8.0, 0.0, 0.0],
            1e-3
        ));
    }

    #[test]
    fn tangent_follows_curve_not_chord() {
        // The anchor chord runs along +x (tangent yaw = atan2(1,0) = +PI/2).
        // On the bow's first half the true tangent has a +z component, so its
        // yaw differs from the chord yaw — proving true-curve-tangent alignment.
        let dense = bowed_segment();
        let (cum, total) = arc_length_table(&dense);
        let chord_yaw = std::f32::consts::FRAC_PI_2;
        let early_yaw = tangent_yaw(tangent_at_arc_length(&dense, &cum, total, total * 0.15));
        assert!(
            (early_yaw - chord_yaw).abs() > 0.1,
            "curve tangent must differ from the straight chord: {early_yaw} vs {chord_yaw}"
        );
    }

    #[test]
    fn straight_route_samples_identically_to_chords() {
        // An all-corner (handle-less) route flattens byte-identically to its
        // anchors, so arc-length sampling reduces to plain chord interpolation —
        // legacy straight paths are unchanged (FR-1 acceptance).
        let anchors = [
            corner([0.0, 0.0, 0.0]),
            corner([10.0, 0.0, 0.0]),
            corner([20.0, 0.0, 0.0]),
        ];
        let dense = flatten_route(&anchors, SAMPLES_PER_SEGMENT);
        assert_eq!(
            dense,
            vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]]
        );
        let (cum, total) = arc_length_table(&dense);
        // 50% of length 20 = x=10 exactly (the chord midpoint).
        let mid = position_at_arc_length(&dense, &cum, total, total * 0.5);
        assert!(close(mid, [10.0, 0.0, 0.0], 1e-4), "got {mid:?}");
        // Straight tangent yaw is the chord yaw (+PI/2 for +x heading).
        let yaw = tangent_yaw(tangent_at_arc_length(&dense, &cum, total, 0.0));
        assert!(
            (yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-3,
            "got {yaw}"
        );
    }

    #[test]
    fn spacing_offsets_are_even_and_capped() {
        // total 20, spacing 5 → 0,5,10,15,20 (segments floor(20/5)=4, +1 = 5).
        let offs = spacing_offsets(20.0, 5.0);
        assert_eq!(offs, vec![0.0, 5.0, 10.0, 15.0, 20.0]);
        // Non-positive spacing / total → endpoints only, no divide-by-zero.
        assert_eq!(spacing_offsets(20.0, 0.0), vec![0.0, 20.0]);
        assert_eq!(spacing_offsets(20.0, -3.0), vec![0.0, 20.0]);
        assert_eq!(spacing_offsets(0.0, 5.0), vec![0.0]);
        // A tiny spacing saturates at the cap rather than allocating unbounded.
        assert_eq!(spacing_offsets(20.0, 0.0001).len(), MAX_STAMP_OFFSETS);
    }

    #[test]
    fn count_offsets_spread_evenly() {
        assert_eq!(count_offsets(20.0, 0), Vec::<f32>::new());
        assert_eq!(count_offsets(20.0, 1), vec![0.0]);
        assert_eq!(count_offsets(20.0, 5), vec![0.0, 5.0, 10.0, 15.0, 20.0]);
        assert_eq!(count_offsets(20.0, 1_000_000).len(), MAX_STAMP_OFFSETS);
    }

    #[test]
    fn sample_stamps_places_every_offset_on_the_curve() {
        let dense = bowed_segment();
        let (_, total) = arc_length_table(&dense);
        let offs = count_offsets(total, 5);
        let stamps = sample_stamps_along_curve(&dense, &offs);
        assert_eq!(stamps.len(), 5);
        // First + last land on the exact endpoints; interior rides the bow.
        assert!(close(stamps[0].0, [0.0, 0.0, 0.0], 1e-3));
        assert!(close(stamps[4].0, [8.0, 0.0, 0.0], 1e-3));
        assert!(stamps[2].0[2] > 0.5, "interior stamp rides the bow");
    }
}
