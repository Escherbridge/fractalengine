//! Pure curve + shape math for the pen tool (phase 2): resample placed control
//! points into Catmull-Rom / Bezier curves and generate parametric shape rings.
//! No Bevy/ECS types — everything is `[f32; 3]` in/out so it's fully unit-
//! testable. See `fe-ui/src/node_manager/AGENTS.md` §pen-tool.

/// How the pen tool's placed control points become the final path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PenMode {
    /// Straight segments through the control points, unchanged.
    #[default]
    Polyline,
    /// Cardinal / Catmull-Rom spline passing through every control point.
    CatmullRom,
    /// Cubic Bezier over the control polygon (chained cubic segments).
    Bezier,
}

impl PenMode {
    /// Stable label for radio buttons.
    pub fn label(self) -> &'static str {
        match self {
            PenMode::Polyline => "Polyline",
            PenMode::CatmullRom => "Catmull-Rom",
            PenMode::Bezier => "Bezier",
        }
    }
}

/// Linear interpolation of two `[f32; 3]` points component-wise.
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Cardinal-spline resample passing through the control points. `tension` maps
/// sharp↔smooth: `1.0` ≈ straight/sharp (zero tangents, i.e. polyline through
/// the points), `0.0` ≈ round (classic Catmull-Rom tangents). Values in between
/// scale the tangents linearly. Endpoints are duplicated to give the first/last
/// segment a phantom neighbour, so the curve still passes through `control[0]`
/// and `control[last]`.
///
/// Returns the resampled polyline. For `< 2` control points the input is
/// returned unchanged (nothing to interpolate).
pub fn catmull_rom(
    control: &[[f32; 3]],
    samples_per_segment: usize,
    tension: f32,
) -> Vec<[f32; 3]> {
    if control.len() < 2 {
        return control.to_vec();
    }
    // `c` scales the Catmull-Rom tangent: c=1 → full round tangents (tension 0),
    // c=0 → zero tangents → straight segments (tension 1).
    let c = (1.0 - tension.clamp(0.0, 1.0)) * 0.5;
    let steps = samples_per_segment.max(1);
    let n = control.len();
    let mut out: Vec<[f32; 3]> = Vec::with_capacity((n - 1) * steps + 1);
    out.push(control[0]);

    for i in 0..n - 1 {
        // p1→p2 is the active segment; p0/p3 are the neighbours (duplicated at
        // the ends). Hermite basis with Catmull-Rom tangents scaled by `c`.
        let p0 = control[if i == 0 { 0 } else { i - 1 }];
        let p1 = control[i];
        let p2 = control[i + 1];
        let p3 = control[if i + 2 < n { i + 2 } else { n - 1 }];

        for s in 1..=steps {
            let t = s as f32 / steps as f32;
            out.push(hermite_cardinal(p0, p1, p2, p3, t, c));
        }
    }
    out
}

/// One cardinal-spline sample on the segment `p1→p2` at parameter `t∈[0,1]`,
/// with neighbour points `p0`/`p3` and tangent scale `c`.
fn hermite_cardinal(
    p0: [f32; 3],
    p1: [f32; 3],
    p2: [f32; 3],
    p3: [f32; 3],
    t: f32,
    c: f32,
) -> [f32; 3] {
    let t2 = t * t;
    let t3 = t2 * t;
    // Hermite basis functions.
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        // Cardinal tangents: m1 = c*(p2 - p0), m2 = c*(p3 - p1).
        let m1 = c * (p2[k] - p0[k]);
        let m2 = c * (p3[k] - p1[k]);
        out[k] = h00 * p1[k] + h10 * m1 + h01 * p2[k] + h11 * m2;
    }
    out
}

/// Cubic Bezier over the control polygon. The control points are consumed in
/// groups that share endpoints: `[P0 P1 P2 P3][P3 P4 P5 P6]…` — i.e. each cubic
/// segment reuses the previous segment's last point as its first. A trailing
/// remainder of 2 points forms a quadratic (promoted to cubic) and a remainder
/// of 1 point forms a final straight segment, so no control point is dropped.
///
/// `samples` is the number of samples emitted per cubic segment. For `< 2`
/// control points the input is returned unchanged.
pub fn bezier(control: &[[f32; 3]], samples: usize) -> Vec<[f32; 3]> {
    if control.len() < 2 {
        return control.to_vec();
    }
    let steps = samples.max(1);
    let mut out: Vec<[f32; 3]> = Vec::new();
    out.push(control[0]);

    let mut i = 0usize;
    while i + 3 < control.len() {
        // Full cubic segment P_i..P_{i+3}, reusing P_{i+3} as next start.
        let seg = [control[i], control[i + 1], control[i + 2], control[i + 3]];
        push_cubic(&mut out, seg, steps);
        i += 3;
    }
    // Handle the tail (remaining points after the last full cubic).
    let rem = control.len() - i; // >= 1
    match rem {
        1 => { /* single point already emitted as the last endpoint */ }
        2 => {
            // Straight segment to the last point.
            for s in 1..=steps {
                let t = s as f32 / steps as f32;
                out.push(lerp3(control[i], control[i + 1], t));
            }
        }
        3 => {
            // Quadratic P_i P_{i+1} P_{i+2} promoted to a cubic.
            let p0 = control[i];
            let p1 = control[i + 1];
            let p2 = control[i + 2];
            // Cubic control points equivalent to the quadratic.
            let c1 = lerp3(p0, p1, 2.0 / 3.0);
            let c2 = lerp3(p2, p1, 2.0 / 3.0);
            push_cubic(&mut out, [p0, c1, c2, p2], steps);
        }
        _ => {}
    }
    out
}

/// Append `steps` samples of a single cubic Bezier segment (excluding the
/// segment's start point, which the caller already emitted) via de Casteljau.
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

/// Dispatch a resample of `control` per `mode`. `Polyline` returns the control
/// points unchanged; `CatmullRom`/`Bezier` interpolate them. `tension` only
/// affects `CatmullRom`. `samples_per_segment` is clamped to `>= 1` by the
/// underlying builders.
pub fn resample(
    control: &[[f32; 3]],
    mode: PenMode,
    tension: f32,
    samples_per_segment: usize,
) -> Vec<[f32; 3]> {
    match mode {
        PenMode::Polyline => control.to_vec(),
        PenMode::CatmullRom => catmull_rom(control, samples_per_segment, tension),
        PenMode::Bezier => bezier(control, samples_per_segment),
    }
}

/// Closed ellipse as a ring of `segments` points in the XZ plane, centred on
/// `center` (Y taken from `center`). `segments` is clamped to `>= 3`. The ring
/// is NOT explicitly closed (the caller/renderer closes it) — the last point is
/// the one just before wrapping back to the first.
pub fn ellipse(center: [f32; 3], radius_x: f32, radius_z: f32, segments: usize) -> Vec<[f32; 3]> {
    let n = segments.max(3);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let theta = (i as f32) / (n as f32) * std::f32::consts::TAU;
        out.push([
            center[0] + radius_x * theta.cos(),
            center[1],
            center[2] + radius_z * theta.sin(),
        ]);
    }
    out
}

/// Closed circle (ellipse with equal radii) as a ring of `segments` points.
pub fn circle(center: [f32; 3], radius: f32, segments: usize) -> Vec<[f32; 3]> {
    ellipse(center, radius, radius, segments)
}

/// Axis-aligned rectangle centred on `center` (XZ plane), width `w` (X) by
/// height `h` (Z), as its 4 corners CCW starting from `(-w/2, -h/2)`. Not
/// explicitly closed.
pub fn rectangle(center: [f32; 3], w: f32, h: f32) -> Vec<[f32; 3]> {
    let hw = w * 0.5;
    let hh = h * 0.5;
    vec![
        [center[0] - hw, center[1], center[2] - hh],
        [center[0] + hw, center[1], center[2] - hh],
        [center[0] + hw, center[1], center[2] + hh],
        [center[0] - hw, center[1], center[2] + hh],
    ]
}

/// Collinear symmetric handles for an anchor from its neighbor tangent
/// (pen_curve_tool_20260722): direction = normalize(next − prev), a missing
/// endpoint neighbor duplicating the anchor (mirrors `catmull_rom`'s phantom
/// rule); length = smoothness · (1/3) · min(existing neighbor gaps). Returns
/// `(handle_in, handle_out)` relative meter offsets, `None` when degenerate.
pub fn derive_symmetric_handles(
    prev: Option<[f32; 3]>,
    cur: [f32; 3],
    next: Option<[f32; 3]>,
    smoothness: f32,
) -> Option<([f32; 3], [f32; 3])> {
    // ~1/3 gap: the classic Catmull-Rom / cubic-Bezier tangent convention.
    const K: f32 = 1.0 / 3.0;
    if smoothness <= 0.0 {
        return None;
    }
    let dist = |a: [f32; 3], b: [f32; 3]| {
        ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
    };
    let min_gap = match (prev.map(|p| dist(p, cur)), next.map(|n| dist(cur, n))) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let p = prev.unwrap_or(cur);
    let n = next.unwrap_or(cur);
    let dir = [n[0] - p[0], n[1] - p[1], n[2] - p[2]];
    let dir_len = (dir[0].powi(2) + dir[1].powi(2) + dir[2].powi(2)).sqrt();
    if dir_len < 1e-6 || min_gap < 1e-6 {
        return None;
    }
    let scale = smoothness * K * min_gap / dir_len;
    let out = [dir[0] * scale, dir[1] * scale, dir[2] * scale];
    Some(([-out[0], -out[1], -out[2]], out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Approx-equality for a single point (curve math accumulates float error).
    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps && (a[2] - b[2]).abs() < eps
    }

    // ---- PenMode -------------------------------------------------------

    #[test]
    fn pen_mode_default_is_polyline() {
        assert_eq!(PenMode::default(), PenMode::Polyline);
    }

    // ---- resample dispatch --------------------------------------------

    #[test]
    fn resample_polyline_returns_control_unchanged() {
        let control = [[0.0, 0.0, 0.0], [1.0, 0.0, 2.0], [3.0, 0.0, 1.0]];
        let out = resample(&control, PenMode::Polyline, 0.5, 12);
        assert_eq!(out, control.to_vec());
    }

    // ---- derive_symmetric_handles (pen_curve_tool_20260722) -------------

    #[test]
    fn derive_symmetric_handles_interior_direction_and_length() {
        // prev (0,0,0), cur (2,0,0), next (2,0,2): dir = next−prev = (2,0,2),
        // both gaps 2 → |out| = 1 · (1/3) · 2 along normalize(2,0,2).
        let (hin, hout) = derive_symmetric_handles(
            Some([0.0, 0.0, 0.0]),
            [2.0, 0.0, 0.0],
            Some([2.0, 0.0, 2.0]),
            1.0,
        )
        .unwrap();
        let unit = 1.0 / 2f32.sqrt();
        let expect = [unit * 2.0 / 3.0, 0.0, unit * 2.0 / 3.0];
        for k in 0..3 {
            assert!((hout[k] - expect[k]).abs() < 1e-5, "out[{k}] = {}", hout[k]);
            assert!((hin[k] + expect[k]).abs() < 1e-5, "in[{k}] = {}", hin[k]);
        }
    }

    #[test]
    fn derive_symmetric_handles_first_point_duplicates_missing_prev() {
        // No prev → direction cur→next, length from the next gap (3 · 1/3 = 1).
        let (hin, hout) =
            derive_symmetric_handles(None, [0.0, 0.0, 0.0], Some([3.0, 0.0, 0.0]), 1.0).unwrap();
        assert!((hout[0] - 1.0).abs() < 1e-5, "got {}", hout[0]);
        assert_eq!(hout[1], 0.0);
        assert_eq!(hout[2], 0.0);
        assert!((hin[0] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn derive_symmetric_handles_last_point_duplicates_missing_next() {
        let (_, hout) =
            derive_symmetric_handles(Some([0.0, 0.0, 0.0]), [3.0, 0.0, 0.0], None, 1.0).unwrap();
        assert!(
            (hout[0] - 1.0).abs() < 1e-5,
            "trailing tangent keeps +X, got {}",
            hout[0]
        );
    }

    #[test]
    fn derive_symmetric_handles_smoothness_scales_length() {
        let full = derive_symmetric_handles(Some([0.0; 3]), [3.0, 0.0, 0.0], None, 1.0).unwrap();
        let half = derive_symmetric_handles(Some([0.0; 3]), [3.0, 0.0, 0.0], None, 0.5).unwrap();
        assert!((half.1[0] - full.1[0] * 0.5).abs() < 1e-5);
    }

    #[test]
    fn derive_symmetric_handles_degenerate_is_none() {
        // smoothness 0 = sharp; no neighbors at all; coincident neighbor.
        assert!(derive_symmetric_handles(Some([0.0; 3]), [1.0, 0.0, 0.0], None, 0.0).is_none());
        assert!(derive_symmetric_handles(None, [1.0, 0.0, 0.0], None, 1.0).is_none());
        assert!(
            derive_symmetric_handles(Some([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0], None, 1.0).is_none(),
            "coincident neighbor has no tangent"
        );
    }

    // ---- catmull_rom ---------------------------------------------------

    #[test]
    fn catmull_passes_through_control_points_at_segment_boundaries() {
        let control = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [2.0, 0.0, 0.0],
            [3.0, 0.0, 1.0],
        ];
        let samples = 8;
        let out = catmull_rom(&control, samples, 0.3);
        // Boundary i of the input is at index i*samples in the output.
        for (i, cp) in control.iter().enumerate() {
            let idx = i * samples;
            assert!(
                close(out[idx], *cp, 1e-4),
                "control point {i} not hit at output idx {idx}: got {:?} want {:?}",
                out[idx],
                cp
            );
        }
    }

    #[test]
    fn catmull_output_length_is_segments_times_samples_plus_one() {
        let control = [[0.0, 0.0, 0.0], [1.0, 0.0, 1.0], [2.0, 0.0, 0.0]];
        let samples = 5;
        let out = catmull_rom(&control, samples, 0.5);
        // 2 segments * 5 samples + 1 start point.
        assert_eq!(out.len(), 2 * samples + 1);
    }

    #[test]
    fn catmull_tension_extremes_differ_at_segment_midpoint() {
        // A zig-zag so the round vs sharp tangents visibly disagree mid-segment.
        let control = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 2.0],
            [2.0, 0.0, 0.0],
            [3.0, 0.0, 2.0],
        ];
        let samples = 4; // finer sampling so tension-sensitive points are hit.
        let sharp = catmull_rom(&control, samples, 1.0); // straight-ish
        let round = catmull_rom(&control, samples, 0.0); // fully round
                                                         // The two curves share their control-point samples (tension only scales
                                                         // the *tangents*, so on-control samples coincide) but must diverge at
                                                         // some interior sample. Assert the polylines are not pointwise-equal
                                                         // rather than cherry-picking one index that may be tension-invariant for
                                                         // a symmetric zig-zag.
        assert_eq!(sharp.len(), round.len());
        let differs = sharp
            .iter()
            .zip(round.iter())
            .any(|(a, b)| !close(*a, *b, 1e-3));
        assert!(
            differs,
            "tension extremes should differ somewhere along the curve: sharp {sharp:?} round {round:?}"
        );
    }

    #[test]
    fn catmull_tension_one_is_straight_between_points() {
        // With tension=1 the tangents are zero → each segment is a straight
        // line, so the segment midpoint is the average of its endpoints.
        let control = [[0.0, 0.0, 0.0], [2.0, 0.0, 4.0]];
        let out = catmull_rom(&control, 2, 1.0);
        // index 1 is the midpoint of the single segment.
        assert!(close(out[1], [1.0, 0.0, 2.0], 1e-4), "got {:?}", out[1]);
    }

    #[test]
    fn catmull_preserves_y() {
        let control = [[0.0, 5.0, 0.0], [1.0, 5.0, 1.0], [2.0, 5.0, 0.0]];
        let out = catmull_rom(&control, 4, 0.5);
        for p in &out {
            assert!((p[1] - 5.0).abs() < 1e-4, "y not preserved: {:?}", p);
        }
    }

    // ---- degenerate inputs ---------------------------------------------

    #[test]
    fn catmull_zero_points_returns_empty() {
        let out = catmull_rom(&[], 8, 0.5);
        assert!(out.is_empty());
    }

    #[test]
    fn catmull_one_point_returns_that_point() {
        let out = catmull_rom(&[[1.0, 0.0, 2.0]], 8, 0.5);
        assert_eq!(out, vec![[1.0, 0.0, 2.0]]);
    }

    #[test]
    fn catmull_two_points_does_not_panic() {
        let out = catmull_rom(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 4, 0.5);
        assert_eq!(out.first().copied(), Some([0.0, 0.0, 0.0]));
        assert!(close(*out.last().unwrap(), [1.0, 0.0, 0.0], 1e-4));
    }

    #[test]
    fn catmull_zero_samples_is_clamped_to_one() {
        let out = catmull_rom(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], 0, 0.5);
        // 1 segment * 1 sample + 1 start = 2 points.
        assert_eq!(out.len(), 2);
    }

    // ---- bezier --------------------------------------------------------

    #[test]
    fn bezier_endpoints_equal_first_and_last_control() {
        let control = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 3.0],
            [3.0, 0.0, 3.0],
            [4.0, 0.0, 0.0],
        ];
        let out = bezier(&control, 10);
        assert!(close(out[0], control[0], 1e-4), "start {:?}", out[0]);
        assert!(
            close(*out.last().unwrap(), *control.last().unwrap(), 1e-4),
            "end {:?}",
            out.last().unwrap()
        );
    }

    #[test]
    fn bezier_single_cubic_length() {
        let control = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 3.0],
            [3.0, 0.0, 3.0],
            [4.0, 0.0, 0.0],
        ];
        let samples = 8;
        let out = bezier(&control, samples);
        // 1 cubic segment: samples + 1 start point.
        assert_eq!(out.len(), samples + 1);
    }

    #[test]
    fn bezier_two_points_is_a_straight_segment() {
        let control = [[0.0, 0.0, 0.0], [2.0, 0.0, 2.0]];
        let out = bezier(&control, 2);
        // start + 2 samples along the straight line.
        assert!(close(out[0], [0.0, 0.0, 0.0], 1e-4));
        assert!(close(out[1], [1.0, 0.0, 1.0], 1e-4), "mid {:?}", out[1]);
        assert!(close(*out.last().unwrap(), [2.0, 0.0, 2.0], 1e-4));
    }

    #[test]
    fn bezier_seven_points_two_chained_cubics_endpoints_hit() {
        // 7 points = exactly two chained cubics sharing the middle point.
        let control = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [2.0, 0.0, 1.0],
            [3.0, 0.0, 0.0],
            [4.0, 0.0, -1.0],
            [5.0, 0.0, -1.0],
            [6.0, 0.0, 0.0],
        ];
        let out = bezier(&control, 6);
        assert!(close(out[0], control[0], 1e-4));
        assert!(close(*out.last().unwrap(), *control.last().unwrap(), 1e-4));
        // The shared middle control point (index 3) should be hit at the
        // boundary between the two cubics (6 samples into the output + start).
        assert!(close(out[6], control[3], 1e-4), "shared join {:?}", out[6]);
    }

    #[test]
    fn bezier_zero_points_returns_empty() {
        assert!(bezier(&[], 8).is_empty());
    }

    #[test]
    fn bezier_one_point_returns_that_point() {
        assert_eq!(bezier(&[[1.0, 2.0, 3.0]], 8), vec![[1.0, 2.0, 3.0]]);
    }

    // ---- ellipse / circle / rectangle ----------------------------------

    #[test]
    fn ellipse_produces_segments_points_at_radius() {
        let segments = 16;
        let out = ellipse([2.0, 0.0, -3.0], 5.0, 5.0, segments);
        assert_eq!(out.len(), segments);
        // Every point is `radius` from the center in the XZ plane.
        for p in &out {
            let dx = p[0] - 2.0;
            let dz = p[2] - (-3.0);
            let r = (dx * dx + dz * dz).sqrt();
            assert!((r - 5.0).abs() < 1e-3, "radius off: {r}");
        }
    }

    #[test]
    fn ellipse_first_point_is_on_positive_x_axis() {
        let out = ellipse([0.0, 0.0, 0.0], 4.0, 2.0, 8);
        assert!(close(out[0], [4.0, 0.0, 0.0], 1e-4), "first {:?}", out[0]);
    }

    #[test]
    fn ellipse_respects_distinct_radii() {
        let out = ellipse([0.0, 0.0, 0.0], 4.0, 2.0, 4);
        // 4 segments: +x, +z, -x, -z at rx/rz respectively.
        assert!(close(out[0], [4.0, 0.0, 0.0], 1e-4));
        assert!(close(out[1], [0.0, 0.0, 2.0], 1e-4), "quarter {:?}", out[1]);
    }

    #[test]
    fn ellipse_preserves_center_y() {
        let out = ellipse([0.0, 7.5, 0.0], 3.0, 3.0, 12);
        for p in &out {
            assert!((p[1] - 7.5).abs() < 1e-4);
        }
    }

    #[test]
    fn circle_is_ellipse_with_equal_radii() {
        let c = circle([1.0, 0.0, 1.0], 3.0, 10);
        let e = ellipse([1.0, 0.0, 1.0], 3.0, 3.0, 10);
        assert_eq!(c, e);
    }

    #[test]
    fn ellipse_clamps_segments_to_min_three() {
        let out = ellipse([0.0, 0.0, 0.0], 1.0, 1.0, 1);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn rectangle_has_four_corners_around_center() {
        let out = rectangle([1.0, 0.0, 1.0], 4.0, 2.0);
        assert_eq!(out.len(), 4);
        assert!(close(out[0], [-1.0, 0.0, 0.0], 1e-4));
        assert!(close(out[1], [3.0, 0.0, 0.0], 1e-4));
        assert!(close(out[2], [3.0, 0.0, 2.0], 1e-4));
        assert!(close(out[3], [-1.0, 0.0, 2.0], 1e-4));
    }
}
