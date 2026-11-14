//! Pure road-builder snap core: angle snap, guidelines and the snap total order
//! (vertex > guide intersection > single guide > angle > raw). XZ-plane math,
//! Y passthrough, `[f32; 3]` in/out, no ECS types. See
//! `fe-ui/src/node_manager/AGENTS.md` §road-snap.
#![allow(dead_code)] // consumed by the road tool systems (CITY-11)

use super::path_point_interaction::SnapCandidate;

/// Default snap radius in world metres (caller converts via `meters_to_world`).
pub(crate) const DEFAULT_SNAP_RADIUS: f32 = 3.0;

/// Parallel-guide threshold on the 2D cross product of unit directions.
const PARALLEL_EPSILON: f32 = 1e-6;

/// Road snapping toggles and tolerances.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RoadSnapSettings {
    pub angle_snap: bool,
    /// 45.0 (default) or 90.0.
    pub angle_increment_deg: f32,
    pub snap_to_paths: bool,
    pub guidelines: bool,
    /// World-space snap radius.
    pub radius: f32,
}

impl Default for RoadSnapSettings {
    fn default() -> Self {
        Self {
            angle_snap: true,
            angle_increment_deg: 45.0,
            snap_to_paths: true,
            guidelines: true,
            radius: DEFAULT_SNAP_RADIUS,
        }
    }
}

/// Non-finite or non-positive radius falls back to the default.
fn sanitize_radius(radius: f32) -> f32 {
    if radius.is_finite() && radius > 0.0 {
        radius
    } else {
        DEFAULT_SNAP_RADIUS
    }
}

fn finite3(p: [f32; 3]) -> bool {
    p.iter().all(|c| c.is_finite())
}

/// Squared XZ distance.
fn dist2_xz(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    dx * dx + dz * dz
}

/// Unit XZ vector, or `None` for zero / non-finite input.
fn unit2(v: [f32; 2]) -> Option<[f32; 2]> {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len.is_finite() && len > f32::EPSILON && v[0].is_finite() && v[1].is_finite() {
        Some([v[0] / len, v[1] / len])
    } else {
        None
    }
}

/// Wrap degrees into (-180, 180].
fn wrap_degrees(mut degrees: f32) -> f32 {
    while degrees > 180.0 {
        degrees -= 360.0;
    }
    while degrees <= -180.0 {
        degrees += 360.0;
    }
    degrees
}

/// Result of quantising the anchor->cursor direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AngleSnap {
    pub point: [f32; 3],
    /// Quantised angle relative to the reference direction, in (-180, 180].
    pub angle_deg: f32,
    pub locked: bool,
}

/// Quantise anchor->cursor to multiples of `increment_deg` relative to
/// `reference_dir` (or world +X), preserving distance; cursor Y is kept.
pub(crate) fn snap_angle(
    anchor: [f32; 3],
    cursor: [f32; 3],
    reference_dir: Option<[f32; 2]>,
    increment_deg: f32,
    enabled: bool,
) -> AngleSnap {
    let unchanged = AngleSnap {
        point: cursor,
        angle_deg: 0.0,
        locked: false,
    };
    if !enabled
        || !finite3(anchor)
        || !finite3(cursor)
        || !increment_deg.is_finite()
        || increment_deg <= 0.0
    {
        return unchanged;
    }
    let dx = cursor[0] - anchor[0];
    let dz = cursor[2] - anchor[2];
    let distance = (dx * dx + dz * dz).sqrt();
    if !distance.is_finite() || distance <= f32::EPSILON {
        return unchanged;
    }
    let base = reference_dir
        .and_then(unit2)
        .map_or(0.0, |dir| dir[1].atan2(dir[0]));
    let cursor_angle = dz.atan2(dx);
    let increment = increment_deg.to_radians();
    let quantised = ((cursor_angle - base) / increment).round() * increment;
    let absolute = base + quantised;
    AngleSnap {
        point: [
            anchor[0] + distance * absolute.cos(),
            cursor[1],
            anchor[2] + distance * absolute.sin(),
        ],
        angle_deg: wrap_degrees(quantised.to_degrees()),
        locked: true,
    }
}

/// Best candidate within `radius` of the cursor: endpoints outrank interior
/// vertices, ties by distance. Returns the candidate verbatim (D-3).
pub(crate) fn vertex_snap(
    cursor: [f32; 3],
    candidates: &[SnapCandidate],
    radius: f32,
) -> Option<SnapCandidate> {
    if !finite3(cursor) {
        return None;
    }
    let radius = sanitize_radius(radius);
    let limit = radius * radius;
    let mut best: Option<(u8, f32, SnapCandidate)> = None;
    for candidate in candidates {
        if !finite3(candidate.position) {
            continue;
        }
        let d2 = dist2_xz(cursor, candidate.position);
        if !(d2 <= limit) {
            continue;
        }
        let tier = if candidate.endpoint { 0u8 } else { 1u8 };
        let better = match best {
            None => true,
            Some((best_tier, best_d2, _)) => tier < best_tier || (tier == best_tier && d2 < best_d2),
        };
        if better {
            best = Some((tier, d2, *candidate));
        }
    }
    best.map(|(_, _, candidate)| candidate)
}

/// Endpoint position plus its outward XZ tangent, derived by the caller from
/// the track shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GuideSource {
    pub position: [f32; 3],
    pub tangent: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuideKind {
    Extension,
    Perpendicular,
}

/// Infinite guide line through `origin` along unit XZ `dir`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Guide {
    pub origin: [f32; 3],
    pub dir: [f32; 2],
    pub kind: GuideKind,
}

/// Perpendicular XZ distance from `point` to the guide line.
fn distance_to_guide(point: [f32; 3], guide: &Guide) -> f32 {
    let rx = point[0] - guide.origin[0];
    let rz = point[2] - guide.origin[2];
    (guide.dir[0] * rz - guide.dir[1] * rx).abs()
}

/// Extension and perpendicular guides of endpoint candidates (with a known
/// tangent) whose line passes within `radius` of the cursor.
pub(crate) fn guidelines(
    candidates: &[SnapCandidate],
    sources: &[GuideSource],
    cursor: [f32; 3],
    radius: f32,
) -> Vec<Guide> {
    let mut guides = Vec::new();
    if !finite3(cursor) {
        return guides;
    }
    let radius = sanitize_radius(radius);
    for candidate in candidates.iter().filter(|c| c.endpoint && finite3(c.position)) {
        let Some(tangent) = sources
            .iter()
            .find(|s| s.position == candidate.position)
            .and_then(|s| s.tangent)
            .and_then(unit2)
        else {
            continue;
        };
        let extension = Guide {
            origin: candidate.position,
            dir: tangent,
            kind: GuideKind::Extension,
        };
        let perpendicular = Guide {
            origin: candidate.position,
            dir: [-tangent[1], tangent[0]],
            kind: GuideKind::Perpendicular,
        };
        for guide in [extension, perpendicular] {
            if distance_to_guide(cursor, &guide) <= radius {
                guides.push(guide);
            }
        }
    }
    guides
}

/// Closest point on the guide line to the cursor; keeps cursor Y.
pub(crate) fn project_to_guide(cursor: [f32; 3], guide: &Guide) -> [f32; 3] {
    let rx = cursor[0] - guide.origin[0];
    let rz = cursor[2] - guide.origin[2];
    let along = rx * guide.dir[0] + rz * guide.dir[1];
    [
        guide.origin[0] + along * guide.dir[0],
        cursor[1],
        guide.origin[2] + along * guide.dir[1],
    ]
}

/// Crossing of two guide lines (Y from the first origin); `None` if parallel.
pub(crate) fn guide_intersection(a: &Guide, b: &Guide) -> Option<[f32; 3]> {
    let cross = a.dir[0] * b.dir[1] - a.dir[1] * b.dir[0];
    if !cross.is_finite() || cross.abs() < PARALLEL_EPSILON {
        return None;
    }
    let dx = b.origin[0] - a.origin[0];
    let dz = b.origin[2] - a.origin[2];
    let t = (dx * b.dir[1] - dz * b.dir[0]) / cross;
    let point = [
        a.origin[0] + t * a.dir[0],
        a.origin[1],
        a.origin[2] + t * a.dir[1],
    ];
    finite3(point).then_some(point)
}

/// Which rung of the snap order won.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SnapKind {
    Vertex,
    Intersection,
    Guide,
    Angle,
    Raw,
}

/// Everything `resolve_snap` reads for one cursor sample.
pub(crate) struct SnapInput<'a> {
    pub anchor: Option<[f32; 3]>,
    pub cursor: [f32; 3],
    pub reference_dir: Option<[f32; 2]>,
    pub settings: &'a RoadSnapSettings,
    pub candidates: &'a [SnapCandidate],
    pub sources: &'a [GuideSource],
    /// Alt held: no snapping at all.
    pub suspended: bool,
}

/// Snapped point plus the data the overlay draws.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SnapOutcome {
    pub point: [f32; 3],
    pub kind: SnapKind,
    /// Vertex that won (kind `Vertex`).
    pub target_vertex: Option<SnapCandidate>,
    /// Guides that contributed (kinds `Intersection` / `Guide`).
    pub guides: Vec<Guide>,
    /// Quantised angle (kind `Angle`).
    pub angle_deg: Option<f32>,
}

impl SnapOutcome {
    fn raw(point: [f32; 3]) -> Self {
        Self {
            point,
            kind: SnapKind::Raw,
            target_vertex: None,
            guides: Vec::new(),
            angle_deg: None,
        }
    }
}

/// Total order: vertex > guide intersection > single guide > angle > raw.
pub(crate) fn resolve_snap(input: SnapInput<'_>) -> SnapOutcome {
    let cursor = input.cursor;
    if input.suspended || !finite3(cursor) {
        return SnapOutcome::raw(cursor);
    }
    let settings = input.settings;
    let radius = sanitize_radius(settings.radius);

    if settings.snap_to_paths {
        if let Some(target) = vertex_snap(cursor, input.candidates, radius) {
            return SnapOutcome {
                point: target.position,
                kind: SnapKind::Vertex,
                target_vertex: Some(target),
                guides: Vec::new(),
                angle_deg: None,
            };
        }
    }

    if settings.guidelines {
        let guides = guidelines(input.candidates, input.sources, cursor, radius);
        let limit = radius * radius;
        let mut best_crossing: Option<(f32, [f32; 3], usize, usize)> = None;
        for i in 0..guides.len() {
            for j in (i + 1)..guides.len() {
                let Some(mut crossing) = guide_intersection(&guides[i], &guides[j]) else {
                    continue;
                };
                crossing[1] = cursor[1];
                let d2 = dist2_xz(cursor, crossing);
                if d2 <= limit && best_crossing.map_or(true, |(best, ..)| d2 < best) {
                    best_crossing = Some((d2, crossing, i, j));
                }
            }
        }
        if let Some((_, point, i, j)) = best_crossing {
            return SnapOutcome {
                point,
                kind: SnapKind::Intersection,
                target_vertex: None,
                guides: vec![guides[i], guides[j]],
                angle_deg: None,
            };
        }
        let nearest = guides.iter().min_by(|a, b| {
            distance_to_guide(cursor, a)
                .partial_cmp(&distance_to_guide(cursor, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(guide) = nearest {
            return SnapOutcome {
                point: project_to_guide(cursor, guide),
                kind: SnapKind::Guide,
                target_vertex: None,
                guides: vec![*guide],
                angle_deg: None,
            };
        }
    }

    if let (true, Some(anchor)) = (settings.angle_snap, input.anchor) {
        let snapped = snap_angle(
            anchor,
            cursor,
            input.reference_dir,
            settings.angle_increment_deg,
            true,
        );
        if snapped.locked {
            return SnapOutcome {
                point: snapped.point,
                kind: SnapKind::Angle,
                target_vertex: None,
                guides: Vec::new(),
                angle_deg: Some(snapped.angle_deg),
            };
        }
    }

    SnapOutcome::raw(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn candidate(position: [f32; 3], endpoint: bool) -> SnapCandidate {
        SnapCandidate {
            position,
            foreign: true,
            endpoint,
        }
    }

    fn polar(distance: f32, degrees: f32) -> [f32; 3] {
        let radians = degrees.to_radians();
        [distance * radians.cos(), 0.0, distance * radians.sin()]
    }

    #[test]
    fn angle_snap_table_preserves_distance() {
        // (reference_dir, cursor angle deg, expected absolute angle deg)
        let cases: [(Option<[f32; 2]>, f32, f32); 8] = [
            (None, 0.0, 0.0),
            (None, 44.0, 45.0),
            (None, 46.0, 45.0),
            (None, 90.0, 90.0),
            (None, 135.0, 135.0),
            (Some([0.0, 1.0]), 44.0, 45.0),
            (Some([0.0, 1.0]), 90.0, 90.0),
            (Some([0.0, 1.0]), 135.0, 135.0),
        ];
        for (reference, cursor_deg, expected_deg) in cases {
            let snapped = snap_angle([0.0; 3], polar(10.0, cursor_deg), reference, 45.0, true);
            let expected = polar(10.0, expected_deg);
            assert!(snapped.locked, "{cursor_deg}");
            assert!(near(snapped.point[0], expected[0]), "{cursor_deg}");
            assert!(near(snapped.point[2], expected[2]), "{cursor_deg}");
            let length = (snapped.point[0].powi(2) + snapped.point[2].powi(2)).sqrt();
            assert!(near(length, 10.0));
        }
    }

    #[test]
    fn angle_snap_inert_cases_keep_cursor() {
        let cursor = [3.0, 7.0, 1.0];
        assert_eq!(snap_angle([0.0; 3], cursor, None, 45.0, false).point, cursor);
        assert!(!snap_angle(cursor, cursor, None, 45.0, true).locked);
        let bad = [f32::NAN, 0.0, 0.0];
        let outcome = snap_angle([0.0; 3], bad, None, 45.0, true);
        assert!(!outcome.locked);
    }

    #[test]
    fn vertex_snap_radius_rank_and_exact_coordinates() {
        let interior = candidate([1.0, 0.0, 0.0], false);
        let endpoint = candidate([0.0, 5.5, 2.0], true);
        let hit = vertex_snap([0.0; 3], &[interior, endpoint], 3.0).expect("in range");
        assert!(hit.endpoint, "endpoint outranks the closer interior vertex");
        assert_eq!(hit.position[1].to_bits(), 5.5f32.to_bits());
        // Boundary is inclusive; just beyond is rejected.
        let edge = candidate([3.0, 0.0, 0.0], false);
        assert!(vertex_snap([0.0; 3], &[edge], 3.0).is_some());
        assert!(vertex_snap([0.0; 3], &[candidate([3.01, 0.0, 0.0], false)], 3.0).is_none());
        assert!(vertex_snap([f32::NAN, 0.0, 0.0], &[edge], 3.0).is_none());
    }

    #[test]
    fn perpendicular_guides_intersect() {
        let a = Guide {
            origin: [0.0, 0.0, 0.0],
            dir: [1.0, 0.0],
            kind: GuideKind::Extension,
        };
        let b = Guide {
            origin: [10.0, 0.0, 5.0],
            dir: [0.0, 1.0],
            kind: GuideKind::Extension,
        };
        let crossing = guide_intersection(&a, &b).expect("crosses");
        assert!(near(crossing[0], 10.0) && near(crossing[2], 0.0));
        assert!(guide_intersection(&a, &a).is_none());
    }

    #[test]
    fn resolve_snap_total_order_alt_and_non_finite() {
        let end_a = candidate([0.0, 0.0, 0.0], true);
        let end_b = candidate([10.0, 0.0, 5.0], true);
        let interior = candidate([10.5, 2.0, 1.0], false);
        let candidates = [end_a, end_b, interior];
        let sources = [
            GuideSource {
                position: end_a.position,
                tangent: Some([1.0, 0.0]),
            },
            GuideSource {
                position: end_b.position,
                tangent: Some([1.0, 0.0]),
            },
        ];
        let cursor = [10.5, 0.0, 0.5];
        let mut settings = RoadSnapSettings::default();
        let run = |settings: &RoadSnapSettings, sources: &[GuideSource], suspended: bool| {
            resolve_snap(SnapInput {
                anchor: Some([0.0, 0.0, 0.0]),
                cursor,
                reference_dir: None,
                settings,
                candidates: &candidates,
                sources,
                suspended,
            })
        };

        let vertex = run(&settings, &sources, false);
        assert_eq!(vertex.kind, SnapKind::Vertex);
        assert_eq!(vertex.point, interior.position);

        settings.snap_to_paths = false;
        let crossing = run(&settings, &sources, false);
        assert_eq!(crossing.kind, SnapKind::Intersection);
        assert!(near(crossing.point[0], 10.0) && near(crossing.point[2], 0.0));

        let guide = run(&settings, &sources[..1], false);
        assert_eq!(guide.kind, SnapKind::Guide);
        assert!(near(guide.point[2], 0.0) && near(guide.point[0], 10.5));

        settings.guidelines = false;
        let angle = run(&settings, &sources, false);
        assert_eq!(angle.kind, SnapKind::Angle);
        assert!(angle.angle_deg.is_some());

        settings.angle_snap = false;
        assert_eq!(run(&settings, &sources, false).kind, SnapKind::Raw);

        let all_on = RoadSnapSettings::default();
        let suspended = run(&all_on, &sources, true);
        assert_eq!(suspended.kind, SnapKind::Raw);
        assert_eq!(suspended.point, cursor);

        let bad = resolve_snap(SnapInput {
            anchor: None,
            cursor: [f32::INFINITY, 0.0, 0.0],
            reference_dir: None,
            settings: &all_on,
            candidates: &candidates,
            sources: &sources,
            suspended: false,
        });
        assert_eq!(bad.kind, SnapKind::Raw);
    }
}
