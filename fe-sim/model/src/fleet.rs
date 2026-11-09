//! Pure fleet model: sensors, movers and their total-order planners (FleetConfig v2 core).

use serde::{Deserialize, Serialize};

use crate::sensors::{evaluate, SensorModel};

/// Exclusive upper bound on any simulated epoch-ms (HLC packs wall time into 48 bits).
pub const SIM_TIME_LIMIT_MS: u64 = 1 << 48;

/// Planned-samples cap per fleet (sensor readings and mover samples counted separately).
pub const MAX_PLANNED_READINGS: u64 = 100_000;

/// Default mover sampling period: 2 Hz.
pub const DEFAULT_MOVER_TICK_MS: u64 = 500;

/// Model validation failure (plain string so the crate stays dependency-free).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelError(pub String);

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ModelError {}

fn fail<T>(msg: String) -> Result<T, ModelError> {
    Err(ModelError(msg))
}

/// One synthetic sensor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSpec {
    /// Anchor node NAME the sensor reports from.
    pub anchor: String,
    /// Metric name, e.g. `temperature_c` (non-empty).
    pub metric: String,
    /// Unit label, e.g. `C`.
    pub units: String,
    /// Milliseconds between readings (simulated time).
    pub cadence_ms: u64,
    /// The value model.
    pub model: SensorModel,
}

/// One route-following mover (FleetConfig v2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoverSpec {
    /// Node NAME that moves.
    pub node: String,
    /// Route node NAME the mover follows (length is resolved by the driver).
    pub route: String,
    /// Constant speed along the route, metres per second (> 0).
    pub speed_mps: f64,
    /// Wrap to the start at the end of the route instead of stopping there.
    #[serde(rename = "loop", default)]
    pub looped: bool,
    /// Seconds after fleet start before the mover begins (>= 0).
    #[serde(default)]
    pub start_offset_s: f64,
}

fn default_mover_tick_ms() -> u64 {
    DEFAULT_MOVER_TICK_MS
}

/// The pure part of a FleetConfig: what a fleet IS, with no peers or real-clock fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetModel {
    /// Anchor node names the sensors report from.
    #[serde(default)]
    pub anchors: Vec<String>,
    /// Synthetic sensors.
    #[serde(default)]
    pub sensors: Vec<SensorSpec>,
    /// Route-following movers (empty for v1 fleets).
    #[serde(default)]
    pub movers: Vec<MoverSpec>,
    /// Milliseconds between mover samples.
    #[serde(default = "default_mover_tick_ms")]
    pub mover_tick_ms: u64,
    /// Simulated epoch-ms at fleet start.
    #[serde(default)]
    pub start_ms: u64,
    /// Simulated fleet duration.
    pub duration_ms: u64,
}

impl FleetModel {
    /// Structural validation of sensors, movers and the simulated-time window.
    pub fn validate(&self) -> Result<(), ModelError> {
        for sensor in &self.sensors {
            if sensor.metric.trim().is_empty() {
                return fail("fleet config: sensor metric must be non-empty".into());
            }
            if sensor.cadence_ms == 0 {
                return fail(format!(
                    "fleet config: sensor '{}' has cadence_ms 0",
                    sensor.metric
                ));
            }
            if !self.anchors.contains(&sensor.anchor) {
                return fail(format!(
                    "fleet config: sensor '{}' anchors to unknown anchor '{}'",
                    sensor.metric, sensor.anchor
                ));
            }
        }
        for mover in &self.movers {
            if mover.node.trim().is_empty() || mover.route.trim().is_empty() {
                return fail("fleet config: mover node and route must be non-empty".into());
            }
            if !(mover.speed_mps.is_finite() && mover.speed_mps > 0.0) {
                return fail(format!(
                    "fleet config: mover '{}' needs speed_mps > 0",
                    mover.node
                ));
            }
            if !(mover.start_offset_s.is_finite() && mover.start_offset_s >= 0.0) {
                return fail(format!(
                    "fleet config: mover '{}' needs start_offset_s >= 0",
                    mover.node
                ));
            }
        }
        if !self.movers.is_empty() && self.mover_tick_ms == 0 {
            return fail("fleet config: mover_tick_ms must be non-zero".into());
        }
        if self.start_ms >= SIM_TIME_LIMIT_MS {
            return fail(format!(
                "fleet config: start_ms {} is unrepresentable — simulated time must be \
                 below 2^48 ms ({SIM_TIME_LIMIT_MS}; HLC packs wall time into 48 bits)",
                self.start_ms
            ));
        }
        match self.start_ms.checked_add(self.duration_ms) {
            Some(end) if end < SIM_TIME_LIMIT_MS => {}
            _ => {
                return fail(format!(
                    "fleet config: start_ms + duration_ms runs past the simulated time limit \
                     (2^48 ms = {SIM_TIME_LIMIT_MS})"
                ))
            }
        }
        let planned = self.planned_readings();
        if planned > MAX_PLANNED_READINGS {
            return fail(format!(
                "fleet config: {planned} planned readings exceeds the cap of \
                 {MAX_PLANNED_READINGS} (shorten duration_ms or raise cadence_ms)"
            ));
        }
        let samples = self.planned_mover_samples();
        if samples > MAX_PLANNED_READINGS {
            return fail(format!(
                "fleet config: {samples} planned mover samples exceeds the cap of \
                 {MAX_PLANNED_READINGS} (shorten duration_ms or raise mover_tick_ms)"
            ));
        }
        Ok(())
    }

    /// Readings [`plan_readings`] schedules: `duration / cadence` per sensor, saturating.
    pub fn planned_readings(&self) -> u64 {
        self.sensors
            .iter()
            .filter(|s| s.cadence_ms > 0)
            .map(|s| self.duration_ms / s.cadence_ms)
            .fold(0u64, u64::saturating_add)
    }

    /// Samples [`plan_movers`] schedules: `duration / mover_tick_ms` per mover, saturating.
    pub fn planned_mover_samples(&self) -> u64 {
        if self.mover_tick_ms == 0 {
            return 0;
        }
        (self.duration_ms / self.mover_tick_ms).saturating_mul(self.movers.len() as u64)
    }
}

/// One scheduled reading — a pure planner artifact, before the anchor's node id is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledReading {
    /// Simulated epoch-ms when the reading fires.
    pub at_ms: u64,
    /// Index into [`FleetModel::sensors`].
    pub sensor: usize,
    /// 1-based tick index (tick `k` fires at `start_ms + k * cadence_ms`).
    pub tick: u64,
    /// The reading's value (pure `evaluate(model, tick, at_ms)`).
    pub value: f64,
}

/// Precompute the sensor fire schedule in total `(at_ms, sensor)` order (pure; empty over the cap).
pub fn plan_readings(model: &FleetModel) -> Vec<ScheduledReading> {
    let mut plan: Vec<ScheduledReading> = Vec::new();
    if model.planned_readings() > MAX_PLANNED_READINGS {
        return plan;
    }
    for (idx, sensor) in model.sensors.iter().enumerate() {
        // Bounded tick range: never saturating_mul against the duration (u64::MAX never terminated).
        let Some(last_tick) = model.duration_ms.checked_div(sensor.cadence_ms) else {
            continue; // cadence 0 (rejected by validate)
        };
        for tick in 1..=last_tick {
            let at_ms = model.start_ms.saturating_add(tick * sensor.cadence_ms);
            plan.push(ScheduledReading {
                at_ms,
                sensor: idx,
                tick,
                value: evaluate(&sensor.model, tick, at_ms),
            });
        }
    }
    plan.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then(a.sensor.cmp(&b.sensor)));
    plan
}

/// One scheduled mover sample: where along its route a mover sits at a tick.
#[derive(Debug, Clone, PartialEq)]
pub struct MoverSample {
    /// Simulated epoch-ms of the sample.
    pub at_ms: u64,
    /// Index into [`FleetModel::movers`].
    pub mover: usize,
    /// 1-based tick index (tick `k` fires at `start_ms + k * mover_tick_ms`).
    pub tick: u64,
    /// Route progress in `[0, 1]` (feed to the route sampler shared with fe-terrain).
    pub progress: f64,
}

/// Route progress for a distance travelled: wraps when `looped`, else clamps at the end.
pub fn route_progress(distance_m: f64, route_length_m: f64, looped: bool) -> f64 {
    if !(route_length_m.is_finite() && route_length_m > 0.0) || !distance_m.is_finite() {
        return 0.0;
    }
    let ratio = distance_m.max(0.0) / route_length_m;
    if looped {
        ratio.fract()
    } else {
        ratio.min(1.0)
    }
}

/// Precompute the mover schedule in total `(at_ms, mover)` order; `route_length_m` resolves a route
/// name to metres (movers with an unknown route are skipped). Pure; empty over the cap.
pub fn plan_movers(
    model: &FleetModel,
    route_length_m: impl Fn(&str) -> Option<f64>,
) -> Vec<MoverSample> {
    let mut plan: Vec<MoverSample> = Vec::new();
    if model.mover_tick_ms == 0 || model.planned_mover_samples() > MAX_PLANNED_READINGS {
        return plan;
    }
    let last_tick = model.duration_ms / model.mover_tick_ms;
    for (idx, mover) in model.movers.iter().enumerate() {
        let Some(length) = route_length_m(&mover.route) else {
            continue;
        };
        for tick in 1..=last_tick {
            let offset_ms = tick * model.mover_tick_ms;
            let travelled_s = (offset_ms as f64 / 1000.0 - mover.start_offset_s).max(0.0);
            plan.push(MoverSample {
                at_ms: model.start_ms.saturating_add(offset_ms),
                mover: idx,
                tick,
                progress: route_progress(travelled_s * mover.speed_mps, length, mover.looped),
            });
        }
    }
    plan.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then(a.mover.cmp(&b.mover)));
    plan
}
