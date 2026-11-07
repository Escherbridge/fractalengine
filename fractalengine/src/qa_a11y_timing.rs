//! Opt-in real-app accessibility timing; see AGENTS.md §qa-a11y-timing.

use bevy::a11y::AccessibilityRequested;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::window::{PresentMode, PrimaryWindow};
use bevy::winit::{UpdateMode, WinitSettings};
use bevy_egui::{EguiContext, EguiContextSettings, EguiOutput, PrimaryEguiContext};
use fe_identity::file_fallback::{
    create_owner_only_file, qa_storage_root, replace_owner_only_file,
};
use fe_renderer::camera::OrbitCameraController;
use fe_ui::{navigation_manager::NavigationManager, plugin::SpawnedNodeMarker};
use serde_json::{json, Value};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SAMPLE_LIMIT: usize = 10_000;
const MIN_SAMPLES: usize = 30;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Warmup,
    Baseline,
    Ready,
    AwaitOs,
    Transition,
    Active,
    Done,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Warmup => "warmup",
            Self::Baseline => "inactive_baseline",
            Self::Ready => "baseline_ready",
            Self::AwaitOs => "await_os_activation",
            Self::Transition => "activation_transition",
            Self::Active => "active",
            Self::Done => "done",
        }
    }

    fn duration(self) -> Duration {
        Duration::from_secs(match self {
            Self::Warmup => 10,
            Self::Baseline | Self::Active => 20,
            Self::AwaitOs => 45,
            Self::Transition => 5,
            Self::Ready | Self::Done => 0,
        })
    }

    fn measures(self) -> bool {
        matches!(self, Self::Baseline | Self::Active)
    }
}

#[derive(Clone, PartialEq)]
struct Conditions {
    focused: bool,
    physical_size: [u32; 2],
    window_scale: f32,
    ui_scale: Option<f32>,
    pixels_per_point: Option<f32>,
    present: PresentMode,
    focused_update: UpdateMode,
    unfocused_update: UpdateMode,
    scope_hash: u64,
    node_count: usize,
    camera_transform: Option<[f32; 16]>,
    camera_fov: Option<f32>,
}

impl Conditions {
    fn json(&self) -> Value {
        json!({
            "focused": self.focused, "physical_size": self.physical_size,
            "window_scale": self.window_scale, "ui_scale": self.ui_scale,
            "pixels_per_point": self.pixels_per_point, "present_mode": format!("{:?}", self.present),
            "focused_update_mode": format!("{:?}", self.focused_update),
            "unfocused_update_mode": format!("{:?}", self.unfocused_update),
            "scene_scope_hash": self.scope_hash, "scene_node_count": self.node_count,
            "camera_transform": self.camera_transform, "camera_fov_radians": self.camera_fov,
        })
    }
}

struct Sample {
    elapsed_ms: f64,
    wall_ms: f64,
    phase: Phase,
    requested: bool,
    update_nodes: Option<usize>,
}

#[derive(Resource)]
struct Collector {
    root: PathBuf,
    owner_started_unix_ms: u128,
    origin: Instant,
    phase_since: Instant,
    phase: Phase,
    started: bool,
    last_measurement: Option<Instant>,
    discard_next: bool,
    controlled_continuous: bool,
    input_gate: bool,
    reason: &'static str,
    samples: Vec<Sample>,
    conditions: Option<Conditions>,
    boundaries: Vec<Value>,
    missing_measurements: usize,
    duplicate_measurements: usize,
    invalid_measurements: usize,
    excluded_write_frames: usize,
}

/// Install only for explicit timing opt-in and an independently validated QA root.
pub fn install(app: &mut App, input_gate: bool) {
    if std::env::var("FE_QA_A11Y_TIMING").as_deref() != Ok("1") {
        return;
    }
    let root = match qa_storage_root() {
        Ok(Some(root)) => root,
        _ => {
            warn!("accessibility timing requires a validated QA storage root");
            return;
        }
    };
    let Ok(started) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        warn!("accessibility timing requires a valid process-lifetime timestamp");
        return;
    };
    let owner_started_unix_ms = started.as_millis();
    let owner = json!({"process_id": std::process::id(), "started_unix_ms": owner_started_unix_ms});
    if let Err(error) = create_owner_only_file(
        &root.join("a11y-timing-owner.json"),
        owner.to_string().as_bytes(),
    ) {
        warn!("accessibility timing requires fresh exclusive output files: {error}");
        return;
    }
    let controlled_continuous = std::env::var("FE_QA_A11Y_CONTINUOUS").as_deref() == Ok("1");
    if controlled_continuous && !input_gate {
        app.insert_resource(WinitSettings::continuous());
    }
    let now = Instant::now();
    let mut collector = Collector {
        root,
        owner_started_unix_ms,
        origin: now,
        phase_since: now,
        phase: Phase::Warmup,
        started: false,
        last_measurement: None,
        discard_next: false,
        controlled_continuous,
        input_gate,
        reason: "measurement_in_progress",
        samples: Vec::with_capacity(SAMPLE_LIMIT),
        conditions: None,
        boundaries: Vec::with_capacity(8),
        missing_measurements: 0,
        duplicate_measurements: 0,
        invalid_measurements: 0,
        excluded_write_frames: 0,
    };
    if input_gate {
        collector.phase = Phase::Done;
        collector.reason = "input_gate_enabled_not_a_normal_os_comparison";
        collector.publish();
        return;
    }
    app.insert_resource(collector).add_systems(Last, collect);
}

impl Collector {
    fn advance(
        &mut self,
        phase: Phase,
        now: Instant,
        conditions: &Conditions,
        requested: bool,
        update_nodes: Option<usize>,
    ) {
        self.phase = phase;
        self.phase_since = now;
        self.discard_next = true;
        self.boundaries.push(json!({"phase": phase.name(), "elapsed_ms": now.duration_since(self.origin).as_secs_f64() * 1000.0,
            "requested": requested, "accesskit_update_nodes": update_nodes, "conditions": conditions.json()}));
        self.publish();
    }

    fn finish(
        &mut self,
        reason: &'static str,
        now: Instant,
        conditions: &Conditions,
        requested: bool,
        update_nodes: Option<usize>,
    ) {
        self.reason = reason;
        self.advance(Phase::Done, now, conditions, requested, update_nodes);
    }

    fn publish(&mut self) {
        let baseline = summary(&self.samples, Phase::Baseline);
        let active = summary(&self.samples, Phase::Active);
        let complete = self.phase == Phase::Done && self.reason == "paired_measurement_complete";
        let comparison = complete.then(|| {
            let mut value = serde_json::Map::new();
            for metric in ["mean_ms", "median_ms", "p95_ms"] {
                if let (Some(before), Some(after)) =
                    (baseline[metric].as_f64(), active[metric].as_f64())
                {
                    value.insert(
                        metric.to_owned(),
                        json!({"absolute_ms": after - before,
                        "percent": (before > 0.0).then_some((after - before) * 100.0 / before)}),
                    );
                }
            }
            Value::Object(value)
        });
        let report = json!({"schema": 1, "process_id": std::process::id(), "owner_started_unix_ms": self.owner_started_unix_ms,
            "status": if complete { "complete" } else { "incomplete" }, "reason": self.reason,
            "phase": self.phase.name(), "qa_input_gate": self.input_gate,
            "controlled_continuous_both_phases": self.controlled_continuous,
            "metric": "FrameTimeDiagnosticsPlugin::FRAME_TIME wall milliseconds; includes scheduling, vsync, GPU and app work; not CPU cost",
            "sample_limit": SAMPLE_LIMIT, "minimum_samples_per_phase": MIN_SAMPLES,
            "missing_measurements": self.missing_measurements, "duplicate_measurements": self.duplicate_measurements,
            "invalid_measurements": self.invalid_measurements, "excluded_after_write_frames": self.excluded_write_frames,
            "recorded_sample_conditions": self.conditions.as_ref().map(Conditions::json),
            "condition_scope": "cursor hash, node count, camera, focus, dimensions, scales and cadence; other workload changes require external verification",
            "accesskit_count_kind": "nodes in current update, not total retained tree size",
            "boundaries": self.boundaries, "baseline": baseline, "active": active, "comparison": comparison,
            "samples": self.samples.iter().map(|s| json!({"elapsed_ms": s.elapsed_ms, "wall_ms": s.wall_ms,
                "phase": s.phase.name(), "requested": s.requested,
                "accesskit_update_present": s.update_nodes.is_some(), "accesskit_update_nodes": s.update_nodes})).collect::<Vec<_>>()});
        let result = serde_json::to_vec_pretty(&report)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| replace_owner_only_file(&self.root.join("a11y-timing.json"), &bytes));
        if let Err(error) = result {
            self.phase = Phase::Done;
            self.reason = "secure_output_failed";
            warn!("accessibility timing output failed: {error}");
        }
    }
}

fn summary(samples: &[Sample], phase: Phase) -> Value {
    let mut values: Vec<_> = samples
        .iter()
        .filter(|s| s.phase == phase)
        .map(|s| s.wall_ms)
        .collect();
    if values.is_empty() {
        return json!({"count": 0});
    }
    values.sort_by(f64::total_cmp);
    let n = values.len();
    let mean = values.iter().sum::<f64>() / n as f64;
    let median = (values[(n - 1) / 2] + values[n / 2]) / 2.0;
    let p95 = values[(n * 95).div_ceil(100).saturating_sub(1)];
    let stddev = (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    json!({"count": n, "mean_ms": mean, "median_ms": median, "p95_ms": p95,
        "minimum_ms": values[0], "maximum_ms": values[n - 1], "stddev_ms": stddev})
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters
fn collect(
    mut state: ResMut<Collector>,
    diagnostics: Res<DiagnosticsStore>,
    requested: Res<AccessibilityRequested>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut contexts: Query<
        (&mut EguiContext, &EguiContextSettings, &EguiOutput),
        With<PrimaryEguiContext>,
    >,
    settings: Res<WinitSettings>,
    nav: Res<NavigationManager>,
    nodes: Query<(), With<SpawnedNodeMarker>>,
    cameras: Query<(&Transform, &Projection), With<OrbitCameraController>>,
) {
    if state.phase == Phase::Done {
        return;
    }
    let Ok(window) = windows.single() else {
        state.phase = Phase::Done;
        state.reason = "incomplete_no_primary_window";
        state.publish();
        return;
    };
    let now = Instant::now();
    let (ui_scale, pixels_per_point, update_nodes) = contexts
        .single_mut()
        .ok()
        .map(|(mut context, settings, output)| {
            (
                Some(settings.scale_factor),
                Some(context.get_mut().pixels_per_point()),
                output
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .map(|u| u.nodes.len()),
            )
        })
        .unwrap_or((None, None, None));
    let mut hash = DefaultHasher::new();
    (
        &nav.active_verse_id,
        &nav.active_fractal_id,
        &nav.active_petal_id,
    )
        .hash(&mut hash);
    let camera = cameras.single().ok();
    let conditions = Conditions {
        focused: window.focused,
        physical_size: [window.physical_width(), window.physical_height()],
        window_scale: window.resolution.scale_factor(),
        ui_scale,
        pixels_per_point,
        present: window.present_mode,
        focused_update: settings.focused_mode,
        unfocused_update: settings.unfocused_mode,
        scope_hash: hash.finish(),
        node_count: nodes.iter().count(),
        camera_transform: camera.map(|(t, _)| t.to_matrix().to_cols_array()),
        camera_fov: camera.and_then(|(_, p)| match p {
            Projection::Perspective(p) => Some(p.fov),
            _ => None,
        }),
    };
    let requested = requested.get();
    if !state.started {
        state.started = true;
        state.origin = now;
        state.advance(Phase::Warmup, now, &conditions, requested, update_nodes);
        return;
    }
    if matches!(state.phase, Phase::Warmup | Phase::Baseline)
        && (requested || update_nodes.is_some())
    {
        state.finish(
            "baseline_unavailable_accessibility_already_activated",
            now,
            &conditions,
            requested,
            update_nodes,
        );
        return;
    }
    if state
        .conditions
        .as_ref()
        .is_some_and(|before| before != &conditions)
    {
        state.finish(
            "incomplete_observed_workload_conditions_changed",
            now,
            &conditions,
            requested,
            update_nodes,
        );
        return;
    }
    if state.phase == Phase::AwaitOs && requested && update_nodes.is_some_and(|n| n > 0) {
        state.advance(Phase::Transition, now, &conditions, requested, update_nodes);
        return;
    }
    if state.phase == Phase::Active && !requested {
        state.finish(
            "incomplete_accessibility_request_lost",
            now,
            &conditions,
            requested,
            update_nodes,
        );
        return;
    }
    if now.duration_since(state.phase_since) >= state.phase.duration() {
        let next = match state.phase {
            Phase::Warmup => Phase::Baseline,
            Phase::Baseline => {
                if state.samples.len() < MIN_SAMPLES {
                    state.finish(
                        "incomplete_insufficient_baseline_samples",
                        now,
                        &conditions,
                        requested,
                        update_nodes,
                    );
                    return;
                }
                let ready = json!({"process_id": std::process::id(), "owner_started_unix_ms": state.owner_started_unix_ms, "qa_input_gate": false, "baseline_ready": true});
                if create_owner_only_file(
                    &state.root.join("a11y-baseline-ready.json"),
                    ready.to_string().as_bytes(),
                )
                .is_err()
                {
                    state.finish(
                        "incomplete_ready_marker_failed",
                        now,
                        &conditions,
                        requested,
                        update_nodes,
                    );
                    return;
                }
                Phase::Ready
            }
            Phase::Ready => Phase::AwaitOs,
            Phase::AwaitOs => {
                state.finish(
                    "incomplete_os_activation_timed_out",
                    now,
                    &conditions,
                    requested,
                    update_nodes,
                );
                return;
            }
            Phase::Transition => Phase::Active,
            Phase::Active => {
                let count = state
                    .samples
                    .iter()
                    .filter(|s| s.phase == Phase::Active)
                    .count();
                let reason = if count >= MIN_SAMPLES {
                    "paired_measurement_complete"
                } else {
                    "incomplete_insufficient_active_samples"
                };
                state.finish(reason, now, &conditions, requested, update_nodes);
                return;
            }
            Phase::Done => return,
        };
        state.advance(next, now, &conditions, requested, update_nodes);
        return;
    }
    let Some(measurement) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.measurement())
    else {
        if state.phase.measures() {
            state.missing_measurements += 1;
        }
        return;
    };
    if state.last_measurement == Some(measurement.time) {
        state.duplicate_measurements += 1;
        return;
    }
    state.last_measurement = Some(measurement.time);
    if state.discard_next {
        state.discard_next = false;
        state.excluded_write_frames += 1;
        return;
    }
    if !state.phase.measures() {
        return;
    }
    if !measurement.value.is_finite() || measurement.value <= 0.0 {
        state.invalid_measurements += 1;
        return;
    }
    if ui_scale.is_none() || camera.is_none() {
        state.missing_measurements += 1;
        return;
    }
    if state.samples.len() == SAMPLE_LIMIT {
        state.finish(
            "incomplete_sample_limit_reached",
            now,
            &conditions,
            requested,
            update_nodes,
        );
        return;
    }
    if state.conditions.is_none() {
        state.conditions = Some(conditions);
    }
    let sample = Sample {
        elapsed_ms: measurement
            .time
            .saturating_duration_since(state.origin)
            .as_secs_f64()
            * 1000.0,
        wall_ms: measurement.value,
        phase: state.phase,
        requested,
        update_nodes,
    };
    state.samples.push(sample);
}
