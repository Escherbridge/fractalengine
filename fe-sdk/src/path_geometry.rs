//! Shared Bezier route geometry; see src/AGENTS.md section path geometry.

/// Lossless renderer source row; normalization is documented in src/AGENTS.md.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteSourceRow {
    pub position: [f64; 3],
    pub time_seconds: f64,
    pub handle_in: Option<[f64; 3]>,
    pub handle_out: Option<[f64; 3]>,
    pub corner_code: u8,
    pub smoothness: f32,
}

impl RouteSourceRow {
    pub fn normalized(mut self) -> Self {
        for handle in [&mut self.handle_in, &mut self.handle_out] {
            if handle.is_some_and(|values| values.iter().all(|v| v.abs() < f64::EPSILON)) {
                *handle = None;
            }
        }
        if !matches!(self.corner_code, 1 | 2)
            || (self.handle_in.is_none() && self.handle_out.is_none())
        {
            self.corner_code = 0;
        }
        self
    }

    pub fn is_finite(&self) -> bool {
        self.position.iter().all(|v| v.is_finite())
            && self.time_seconds.is_finite()
            && self
                .handle_in
                .iter()
                .chain(&self.handle_out)
                .flatten()
                .all(|v| v.is_finite())
            && self.smoothness.is_finite()
    }
}

/// Decode the renderer's typed source without the editor's f32 coordinate projection.
pub fn decode_route_source(value: &serde_json::Value) -> Vec<RouteSourceRow> {
    fn handle(row: &[serde_json::Value], base: usize) -> Option<[f64; 3]> {
        let values = [
            row.get(base)?.as_f64()?,
            row.get(base + 1)?.as_f64()?,
            row.get(base + 2)?.as_f64()?,
        ];
        Some(values)
    }
    value
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|entry| {
                    let row = entry.as_array()?;
                    let position = [
                        row.first()?.as_f64()?,
                        row.get(1)?.as_f64()?,
                        row.get(2)?.as_f64()?,
                    ];
                    let handle_in = handle(row, 4);
                    let handle_out = handle(row, 7);
                    let corner_code = match row
                        .get(10)
                        .and_then(serde_json::Value::as_f64)
                        .unwrap_or(0.0) as i64
                    {
                        1 => 1,
                        2 => 2,
                        _ => 0,
                    };
                    Some(
                        RouteSourceRow {
                            position,
                            time_seconds: row
                                .get(3)
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or(0.0),
                            handle_in,
                            handle_out,
                            corner_code,
                            smoothness: row
                                .get(11)
                                .and_then(serde_json::Value::as_f64)
                                .unwrap_or(0.0) as f32,
                        }
                        .normalized(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Subdivision count used by stamp consumers.
pub const FLATTEN_SAMPLES_PER_SEGMENT: usize = 16;
/// Position with optional relative incoming and outgoing handle offsets.
pub type BezierAnchor = ([f32; 3], Option<[f32; 3]>, Option<[f32; 3]>);

/// Flatten anchors using the same f32 de Casteljau operations for every consumer.
pub fn flatten_anchor_path(anchors: &[BezierAnchor], samples_per_seg: usize) -> Vec<[f32; 3]> {
    if anchors.len() < 2 {
        return anchors.iter().map(|(point, _, _)| *point).collect();
    }
    let steps = samples_per_seg.max(1);
    let mut out = Vec::with_capacity(anchors.len());
    out.push(anchors[0].0);
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    for pair in anchors.windows(2) {
        let (pa, _, out_h) = pair[0];
        let (pb, in_h, _) = pair[1];
        if out_h.is_none() && in_h.is_none() {
            out.push(pb);
            continue;
        }
        let c1 = add(pa, out_h.unwrap_or([0.0; 3]));
        let c2 = add(pb, in_h.unwrap_or([0.0; 3]));
        for sample in 1..=steps {
            let t = sample as f32 / steps as f32;
            let a = lerp(pa, c1, t);
            let b = lerp(c1, c2, t);
            let c = lerp(c2, pb, t);
            out.push(lerp(lerp(a, b, t), lerp(b, c, t), t));
        }
    }
    out
}

/// Decode the persisted compact/handled point shape, rejecting non-finite coordinates.
pub fn decode_anchor_rows(value: &serde_json::Value) -> Option<Vec<BezierAnchor>> {
    let read_vec = |row: &[serde_json::Value], base: usize| -> Option<[f32; 3]> {
        let mut vector = [0.0; 3];
        for (index, target) in vector.iter_mut().enumerate() {
            *target = row.get(base + index)?.as_f64()? as f32;
            if !target.is_finite() {
                return None;
            }
        }
        Some(vector)
    };
    value
        .as_array()?
        .iter()
        .map(|value| {
            let row = value.as_array()?;
            let handle = |base| {
                read_vec(row, base).filter(|vector| vector.iter().any(|v| v.abs() >= f32::EPSILON))
            };
            Some((read_vec(row, 0)?, handle(4), handle(7)))
        })
        .collect()
}

/// Cumulative world-coordinate distances with the renderer's f32 accumulation order.
pub fn cumulative_distances(points: &[[f32; 3]]) -> (Vec<f32>, f32) {
    let mut distances = Vec::with_capacity(points.len());
    if points.is_empty() {
        return (distances, 0.0);
    }
    let mut total = 0.0;
    distances.push(total);
    for pair in points.windows(2) {
        let dx = pair[1][0] - pair[0][0];
        let dy = pair[1][1] - pair[0][1];
        let dz = pair[1][2] - pair[0][2];
        total += (dx * dx + dy * dy + dz * dz).sqrt();
        distances.push(total);
    }
    (distances, total)
}

/// Arc-length interpolation for shared stamp placement; see src/AGENTS.md.
pub fn position_at_progress(
    points: &[[f32; 3]],
    cumdist: &[f32],
    total: f32,
    progress: f32,
) -> [f32; 3] {
    match points.len() {
        0 => return [0.0, 0.0, 0.0],
        1 => return points[0],
        _ => {}
    }
    let target = progress.clamp(0.0, 1.0) * total;

    // Last cumulative distance <= target; clamp to a valid segment.
    let mut seg = cumdist
        .iter()
        .position(|&d| d > target)
        .unwrap_or(points.len() - 1)
        .saturating_sub(1);
    seg = seg.min(points.len() - 2);

    let seg_start = cumdist[seg];
    let seg_end = cumdist[seg + 1];
    let seg_len = seg_end - seg_start;
    let t = if seg_len > 1e-9 {
        (target - seg_start) / seg_len
    } else {
        0.0
    };

    let a = &points[seg];
    let b = &points[seg + 1];
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// World-coordinate polyline length using shared renderer accumulation.
pub fn polyline_length(points: &[[f32; 3]]) -> f32 {
    cumulative_distances(points).1
}
