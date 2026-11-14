//! Road builder placement model: modes, settings, chain log and the one event
//! reducer that turns clicks and drags into path commits. Pure; the viewport
//! system maps `RoadCommit` to existing `UiAction`s only. See
//! `fe-ui/src/node_manager/AGENTS.md` §road-chain.
#![allow(dead_code)] // consumed by the road tool systems (CITY-11)

use bevy::prelude::Resource;

use super::curve::bezier_into;

/// Defense-in-depth cap on buffered freeform drag samples.
pub(crate) const MAX_FREEFORM_SAMPLES: usize = 8192;

/// Two points closer than this (world units, XZ) are the same chain anchor.
const ANCHOR_EPSILON: f32 = 1e-4;

/// How the next road segment is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RoadMode {
    /// Click start, click end.
    #[default]
    Straight,
    /// Click start, apex, end: a quadratic Bezier.
    Curved,
    /// Press-drag-release polyline.
    Freeform,
}

impl RoadMode {
    /// Every mode, in picker order (keys 1/2/3).
    pub(crate) const ALL: [RoadMode; 3] = [RoadMode::Straight, RoadMode::Curved, RoadMode::Freeform];

    /// User-facing mode label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RoadMode::Straight => "Straight",
            RoadMode::Curved => "Curved",
            RoadMode::Freeform => "Freeform",
        }
    }
}

/// Road builder toggles and tolerances (independent of `road_snap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RoadSettings {
    pub mode: RoadMode,
    pub angle_snap: bool,
    pub angle_increment_deg: f32,
    pub snap_to_paths: bool,
    pub guidelines: bool,
    /// World-space snap radius.
    pub snap_radius_m: f32,
    /// Samples per curved segment.
    pub curve_samples: usize,
    /// Minimum arc spacing between freeform samples.
    pub freeform_spacing_m: f32,
}

impl Default for RoadSettings {
    fn default() -> Self {
        Self {
            mode: RoadMode::Straight,
            angle_snap: true,
            angle_increment_deg: 45.0,
            snap_to_paths: true,
            guidelines: true,
            snap_radius_m: 3.0,
            curve_samples: 16,
            freeform_spacing_m: 2.0,
        }
    }
}

/// One committed segment: how many points it appended and where it ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SegmentRecord {
    pub count: usize,
    pub end: [f32; 3],
}

/// Per-chain record of appended point counts, for undo-last-segment.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ChainLog {
    /// First point of the track; survives undo of the first segment.
    pub origin: Option<[f32; 3]>,
    pub segments: Vec<SegmentRecord>,
}

impl ChainLog {
    /// No segment has been committed in this chain.
    pub(crate) fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    fn total_points(&self) -> usize {
        self.segments.iter().map(|segment| segment.count).sum()
    }

    fn record(&mut self, points: &[[f32; 3]]) {
        let Some(&end) = points.last() else { return };
        if self.segments.is_empty() {
            self.origin = points.first().copied();
        }
        self.segments.push(SegmentRecord {
            count: points.len(),
            end,
        });
    }

    /// Drop the undone segment from the log (first segment keeps its origin point).
    fn apply_undo(&mut self) {
        match self.segments.len() {
            0 => {}
            1 => {
                if let (Some(origin), Some(first)) = (self.origin, self.segments.first_mut()) {
                    *first = SegmentRecord {
                        count: 1,
                        end: origin,
                    };
                }
            }
            _ => {
                self.segments.pop();
            }
        }
    }
}

/// What Backspace removes: path point indices (high to low) and the anchor to restore.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct UndoPlan {
    pub remove_indices: Vec<usize>,
    pub restored_anchor: Option<[f32; 3]>,
}

/// Plan the removal of the last committed segment (AC-2.2).
pub(crate) fn undo_plan(log: &ChainLog) -> UndoPlan {
    let Some(last) = log.segments.last() else {
        return UndoPlan::default();
    };
    let total = log.total_points();
    if log.segments.len() == 1 {
        // The first segment keeps its origin point so the track is never emptied.
        return match log.origin {
            Some(origin) if last.count > 1 => UndoPlan {
                remove_indices: (1..total).rev().collect(),
                restored_anchor: Some(origin),
            },
            _ => UndoPlan::default(),
        };
    }
    let previous_end = log.segments[log.segments.len() - 2].end;
    UndoPlan {
        remove_indices: (total - last.count..total).rev().collect(),
        restored_anchor: Some(previous_end),
    }
}

/// Resource: the road tool's whole placement state.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct RoadBuilderState {
    pub(crate) settings: RoadSettings,
    /// Chain anchor: the last committed (or first clicked) point.
    pub(crate) anchor: Option<[f32; 3]>,
    /// Clicked but uncommitted control points (curved apex).
    pub(crate) pending: Vec<[f32; 3]>,
    /// Raw freeform drag samples awaiting `DragEnd`.
    pub(crate) freeform_samples: Vec<[f32; 3]>,
    pub(crate) log: ChainLog,
    /// Track node id once the create round-trip answered.
    pub(crate) session_track: Option<String>,
    /// Per-session road counter behind the default "Road N" name.
    pub(crate) track_counter: u32,
    /// Undone point indices still to remove, ascending; issued highest first, one at a time.
    pub(crate) pending_removals: Vec<usize>,
    /// World units per real metre of the active petal map; NaN or 0 = no map.
    pub world_scale: f64,
}

impl RoadBuilderState {
    /// A segment is in flight or a chain is open (gesture claim / Escape owner).
    pub(crate) fn is_active(&self) -> bool {
        !self.pending.is_empty() || !self.freeform_samples.is_empty() || self.anchor.is_some()
    }

    /// Backspace would remove a committed segment (the first segment keeps its origin point).
    pub(crate) fn can_undo_segment(&self) -> bool {
        self.session_track.is_some()
            && match self.log.segments.as_slice() {
                [] => false,
                [only] => only.count > 1,
                _ => true,
            }
    }

    /// Default name of the session's auto-created track.
    pub(crate) fn track_name(&self) -> String {
        format!("Road {}", self.track_counter)
    }

    /// Escape: clear the pending segment, else end the chain. False when idle.
    pub(crate) fn cancel_stage(&mut self) -> bool {
        if !self.is_active() && self.log.is_empty() {
            return false;
        }
        reduce(self, RoadEvent::Escape);
        true
    }

    fn clear_segment(&mut self) {
        self.pending.clear();
        self.freeform_samples.clear();
    }

    fn end_chain(&mut self) {
        self.clear_segment();
        self.anchor = None;
        self.log = ChainLog::default();
        self.session_track = None;
        self.pending_removals.clear();
    }
}

/// Input to the reducer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoadEvent {
    Click([f32; 3]),
    DragSample([f32; 3]),
    DragEnd,
    Escape,
    Backspace,
    SetMode(RoadMode),
    SetSettings(RoadSettings),
    Clear,
}

/// Output of the reducer; the interaction system maps these to `UiAction`s.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RoadCommit {
    /// First commit of a session: create the track with these points.
    CreateTrack { points: Vec<[f32; 3]> },
    /// Append points to the session track (`PathAppendShape` / `PathAppendPoint`).
    Append(Vec<[f32; 3]>),
}

fn is_finite_point(point: [f32; 3]) -> bool {
    point.iter().all(|value| value.is_finite())
}

fn same_anchor(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() <= ANCHOR_EPSILON && (a[2] - b[2]).abs() <= ANCHOR_EPSILON
}

/// Turn a finished segment into a commit; the chain-anchor point is dropped when chaining.
fn commit_points(state: &mut RoadBuilderState, mut points: Vec<[f32; 3]>) -> Vec<RoadCommit> {
    let chaining = !state.log.is_empty();
    if chaining {
        if let (Some(anchor), Some(&first)) = (state.anchor, points.first()) {
            if same_anchor(anchor, first) {
                points.remove(0);
            }
        }
    }
    let minimum = if chaining { 1 } else { 2 };
    let Some(&end) = points.last() else {
        return Vec::new();
    };
    if points.len() < minimum {
        return Vec::new();
    }
    state.log.record(&points);
    state.anchor = Some(end);
    if chaining {
        vec![RoadCommit::Append(points)]
    } else {
        state.track_counter += 1;
        vec![RoadCommit::CreateTrack { points }]
    }
}

fn on_click(state: &mut RoadBuilderState, point: [f32; 3]) -> Vec<RoadCommit> {
    let Some(anchor) = state.anchor else {
        state.anchor = Some(point);
        return Vec::new();
    };
    match state.settings.mode {
        RoadMode::Straight => {
            let points = expand_segment(RoadMode::Straight, &[anchor, point], 0);
            commit_points(state, points)
        }
        RoadMode::Curved => {
            state.pending.push(point);
            if state.pending.len() < 2 {
                return Vec::new();
            }
            let controls = [anchor, state.pending[0], state.pending[1]];
            state.pending.clear();
            let points = expand_segment(RoadMode::Curved, &controls, state.settings.curve_samples);
            commit_points(state, points)
        }
        // Freeform places only by dragging; a bare click just opens the chain.
        RoadMode::Freeform => Vec::new(),
    }
}

fn on_drag_end(state: &mut RoadBuilderState) -> Vec<RoadCommit> {
    let samples = std::mem::take(&mut state.freeform_samples);
    let spacing_world = crate::geometry::meters_to_world(state.settings.freeform_spacing_m, state.world_scale);
    let decimated = decimate(&samples, spacing_world);
    commit_points(state, decimated)
}

fn on_backspace(state: &mut RoadBuilderState) {
    if state.pending.pop().is_some() || state.freeform_samples.pop().is_some() {
        return;
    }
    if state.log.is_empty() {
        // Nothing committed: a lone first click is simply withdrawn.
        state.anchor = None;
        return;
    }
    // The track does not exist until the create echo names it; undo waits for it.
    if state.session_track.is_none() {
        return;
    }
    let plan = undo_plan(&state.log);
    if plan.remove_indices.is_empty() {
        return;
    }
    state.log.apply_undo();
    state.anchor = plan.restored_anchor;
    state.pending_removals.extend(plan.remove_indices);
    state.pending_removals.sort_unstable();
    state.pending_removals.dedup();
}

/// The one reducer: apply `event` to `state`, return the commits to perform.
pub(crate) fn reduce(state: &mut RoadBuilderState, event: RoadEvent) -> Vec<RoadCommit> {
    match event {
        RoadEvent::Click(point) | RoadEvent::DragSample(point) if !is_finite_point(point) => {
            Vec::new()
        }
        // Placement waits until an undone segment has left the track (indices would clash).
        RoadEvent::Click(_) | RoadEvent::DragSample(_) | RoadEvent::DragEnd
            if !state.pending_removals.is_empty() =>
        {
            state.freeform_samples.clear();
            Vec::new()
        }
        RoadEvent::Click(point) => on_click(state, point),
        RoadEvent::DragSample(point) => {
            if state.settings.mode != RoadMode::Freeform {
                return Vec::new();
            }
            if state.freeform_samples.len() >= MAX_FREEFORM_SAMPLES {
                // At the cap the newest sample replaces the tail so the last point stays current.
                if let Some(tail) = state.freeform_samples.last_mut() {
                    *tail = point;
                }
            } else {
                state.freeform_samples.push(point);
            }
            Vec::new()
        }
        RoadEvent::DragEnd => on_drag_end(state),
        RoadEvent::Escape => {
            if !state.pending.is_empty() || !state.freeform_samples.is_empty() {
                state.clear_segment();
            } else {
                state.end_chain();
            }
            Vec::new()
        }
        RoadEvent::Backspace => {
            on_backspace(state);
            Vec::new()
        }
        RoadEvent::SetMode(mode) => {
            if state.settings.mode != mode {
                state.settings.mode = mode;
                state.clear_segment();
            }
            Vec::new()
        }
        RoadEvent::SetSettings(settings) => {
            if state.settings.mode != settings.mode {
                state.clear_segment();
            }
            state.settings = settings;
            Vec::new()
        }
        RoadEvent::Clear => {
            state.end_chain();
            Vec::new()
        }
    }
}

/// Keep first and last samples; between them keep a sample once it is `min_spacing` from the last kept.
pub(crate) fn decimate(samples: &[[f32; 3]], min_spacing: f32) -> Vec<[f32; 3]> {
    let mut kept = Vec::new();
    decimate_into(samples, min_spacing, &mut kept);
    kept
}

/// [`decimate`] appended to `out` (no allocation once `out` has capacity).
pub(crate) fn decimate_into(samples: &[[f32; 3]], min_spacing: f32, out: &mut Vec<[f32; 3]>) {
    let start = out.len();
    let mut finite = samples.iter().copied().filter(|point| is_finite_point(*point));
    let Some(first) = finite.next() else {
        return;
    };
    let spacing = if min_spacing.is_finite() && min_spacing > 0.0 {
        min_spacing
    } else {
        0.0
    };
    out.push(first);
    let mut final_sample = first;
    for point in finite {
        final_sample = point;
        if planar_distance(out[out.len() - 1], point) >= spacing {
            out.push(point);
        }
    }
    // The final sample is always kept: it replaces a too-close tail instead of sitting beside it.
    if out[out.len() - 1] != final_sample {
        if out.len() - start > 1 {
            out.pop();
        }
        out.push(final_sample);
    }
}

/// Expand a segment's control points into path points (start included).
pub(crate) fn expand_segment(
    mode: RoadMode,
    controls: &[[f32; 3]],
    samples: usize,
) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    expand_segment_into(mode, controls, samples, &mut out);
    out
}

/// Appends `expand_segment`'s points to `out`; ghost and commit share this path.
pub(crate) fn expand_segment_into(
    mode: RoadMode,
    controls: &[[f32; 3]],
    samples: usize,
    out: &mut Vec<[f32; 3]>,
) {
    match mode {
        RoadMode::Curved if controls.len() >= 2 => bezier_into(controls, samples, out),
        RoadMode::Straight => out.extend(controls.iter().take(2).copied()),
        _ => out.extend_from_slice(controls),
    }
}

fn planar_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = (b[0] - a[0]) as f64;
    let dz = (b[2] - a[2]) as f64;
    dx.hypot(dz) as f32
}

/// XZ-planar polyline length in world units (pass bezier samples for a curve).
pub(crate) fn pending_length(points: &[[f32; 3]]) -> f64 {
    points
        .windows(2)
        .map(|pair| {
            let dx = (pair[1][0] - pair[0][0]) as f64;
            let dz = (pair[1][2] - pair[0][2]) as f64;
            dx.hypot(dz)
        })
        .filter(|length| length.is_finite())
        .sum()
}

/// Keys the road tool reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoadKey {
    Digit1,
    Digit2,
    Digit3,
    A,
    Backspace,
    Escape,
}

/// Map a key press to a reducer event (Alt-hold suspend is read by the interaction system).
pub(crate) fn shortcut_to_event(key: RoadKey, state: &RoadBuilderState) -> Option<RoadEvent> {
    match key {
        RoadKey::Digit1 => Some(RoadEvent::SetMode(RoadMode::Straight)),
        RoadKey::Digit2 => Some(RoadEvent::SetMode(RoadMode::Curved)),
        RoadKey::Digit3 => Some(RoadEvent::SetMode(RoadMode::Freeform)),
        RoadKey::A => Some(RoadEvent::SetSettings(RoadSettings {
            angle_snap: !state.settings.angle_snap,
            ..state.settings
        })),
        RoadKey::Backspace => Some(RoadEvent::Backspace),
        RoadKey::Escape => Some(RoadEvent::Escape),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, z: f32) -> [f32; 3] {
        [x, 0.0, z]
    }

    fn drive(state: &mut RoadBuilderState, events: Vec<RoadEvent>) -> Vec<Vec<RoadCommit>> {
        events.into_iter().map(|event| reduce(state, event)).collect()
    }

    /// Three-segment chained session: create, append, curved append, undo, escape twice.
    #[test]
    fn road_chain_session_journey() {
        let mut state = RoadBuilderState::default();

        // AC-1.1: first click opens the chain, second creates the track with 2 points.
        let out = drive(
            &mut state,
            vec![RoadEvent::Click(at(0.0, 0.0)), RoadEvent::Click(at(10.0, 0.0))],
        );
        assert!(out[0].is_empty());
        assert_eq!(
            out[1],
            vec![RoadCommit::CreateTrack {
                points: vec![at(0.0, 0.0), at(10.0, 0.0)]
            }]
        );
        assert_eq!(state.track_name(), "Road 1");

        // Backspace before the create echo names the track is held back (no orphan removals).
        assert!(!state.can_undo_segment());
        reduce(&mut state, RoadEvent::Backspace);
        assert!(state.pending_removals.is_empty() && state.log.segments.len() == 1);
        state.session_track = Some("track-1".into());

        // AC-2.1: chained straight appends exactly the new end.
        let out = reduce(&mut state, RoadEvent::Click(at(10.0, 10.0)));
        assert_eq!(out, vec![RoadCommit::Append(vec![at(10.0, 10.0)])]);

        // AC-1.2: curved chained segment appends `samples` points, no duplicate anchor.
        state.settings.curve_samples = 8;
        reduce(&mut state, RoadEvent::SetMode(RoadMode::Curved));
        let out = drive(
            &mut state,
            vec![RoadEvent::Click(at(20.0, 10.0)), RoadEvent::Click(at(20.0, 20.0))],
        );
        assert!(out[0].is_empty());
        let appended = match &out[1][..] {
            [RoadCommit::Append(points)] => points.clone(),
            other => panic!("expected one Append, got {other:?}"),
        };
        assert_eq!(appended.len(), 8);
        assert_eq!(appended.last().copied(), Some(at(20.0, 20.0)));
        assert_ne!(appended.first().copied(), Some(at(10.0, 10.0)));

        // AC-2.2: Backspace queues exactly the curved segment's indices, issued highest first.
        assert!(state.can_undo_segment());
        assert!(reduce(&mut state, RoadEvent::Backspace).is_empty());
        assert_eq!(state.pending_removals, (3..=10).collect::<Vec<usize>>());
        assert_eq!(state.anchor, Some(at(10.0, 10.0)));

        // Placement waits until the queued removals have drained.
        assert!(reduce(&mut state, RoadEvent::Click(at(30.0, 30.0))).is_empty());
        assert!(state.pending.is_empty());
        state.pending_removals.clear();

        // AC-2.3: Escape clears pending first, then ends the chain.
        reduce(&mut state, RoadEvent::Click(at(15.0, 10.0)));
        assert_eq!(state.pending.len(), 1);
        assert!(state.cancel_stage());
        assert!(state.pending.is_empty() && state.anchor.is_some());
        assert!(state.cancel_stage());
        assert!(!state.is_active() && state.log.is_empty() && state.session_track.is_none());
        assert!(!state.cancel_stage());

        // Non-finite points are ignored.
        assert!(reduce(&mut state, RoadEvent::Click([f32::NAN, 0.0, 0.0])).is_empty());
        assert!(!state.is_active());
    }

    /// Freeform stroke decimation, shared helpers and shortcut mapping.
    #[test]
    fn road_chain_helpers_table() {
        // AC-1.3: first and last kept, no two consecutive closer than spacing.
        let samples: Vec<[f32; 3]> = (0..=20).map(|step| at(step as f32 * 0.5, 0.0)).collect();
        let kept = decimate(&samples, 2.0);
        assert_eq!(kept.first(), samples.first());
        assert_eq!(kept.last(), samples.last());
        assert!(kept.windows(2).all(|pair| planar_distance(pair[0], pair[1]) >= 2.0));

        let mut state = RoadBuilderState::default();
        reduce(&mut state, RoadEvent::SetMode(RoadMode::Freeform));
        let mut commits = Vec::new();
        for sample in &samples {
            commits.extend(reduce(&mut state, RoadEvent::DragSample(*sample)));
        }
        commits.extend(reduce(&mut state, RoadEvent::DragEnd));
        assert!(matches!(&commits[..], [RoadCommit::CreateTrack { points }] if points.len() == kept.len()));
        assert_eq!(state.anchor, samples.last().copied());

        // 1:100 petal: 2 m spacing is 0.02 world units; 0.0075 steps keep every 3rd sample clear of f32 ties.
        let fine: Vec<[f32; 3]> = (0..=40).map(|step| at(step as f32 * 0.0075, 0.0)).collect();
        let committed_points = |world_scale: f64| {
            let mut scaled = RoadBuilderState::default();
            scaled.world_scale = world_scale;
            reduce(&mut scaled, RoadEvent::SetMode(RoadMode::Freeform));
            let mut scaled_commits = Vec::new();
            for sample in &fine {
                scaled_commits.extend(reduce(&mut scaled, RoadEvent::DragSample(*sample)));
            }
            scaled_commits.extend(reduce(&mut scaled, RoadEvent::DragEnd));
            match scaled_commits.as_slice() {
                [RoadCommit::CreateTrack { points }] => points.clone(),
                other => panic!("expected one CreateTrack, got {other:?}"),
            }
        };
        let scaled_points = committed_points(0.01);
        assert_eq!(scaled_points.len(), decimate(&fine, 0.02).len());
        assert_eq!(scaled_points.len(), 14);
        assert!(scaled_points.windows(2).all(|pair| planar_distance(pair[0], pair[1]) >= 0.02 - 1e-6));
        assert_eq!(committed_points(f64::NAN).len(), 2);

        assert_eq!(pending_length(&[at(0.0, 0.0), at(3.0, 4.0)]), 5.0);
        assert_eq!(expand_segment(RoadMode::Straight, &[at(0.0, 0.0), at(1.0, 0.0), at(2.0, 0.0)], 4).len(), 2);
        assert_eq!(
            expand_segment(RoadMode::Curved, &[at(0.0, 0.0), at(1.0, 1.0), at(2.0, 0.0)], 4).len(),
            5
        );

        assert_eq!(
            shortcut_to_event(RoadKey::Digit3, &state),
            Some(RoadEvent::SetMode(RoadMode::Freeform))
        );
        match shortcut_to_event(RoadKey::A, &state) {
            Some(RoadEvent::SetSettings(settings)) => assert!(!settings.angle_snap),
            other => panic!("expected SetSettings, got {other:?}"),
        }
    }
}
