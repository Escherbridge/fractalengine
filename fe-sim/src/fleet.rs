//! Declarative fleet configuration + the pure tick planner (F8/A18).
//!
//! A fleet is DATA, not code: one JSON document names the peers, the anchor
//! nodes, and one entry per synthetic sensor (anchor + metric + units +
//! cadence + a [`SensorModel`]). The planner is a **pure function of the
//! config**: it precomputes the full fire schedule — every reading's
//! sensor, tick index, and simulated timestamp — in a total
//! `(at_ms, sensor index)` order, so two runs of the same config fire the
//! identical readings in the identical order, by construction.
//!
//! Values are computed at fire time by [`crate::sensors::evaluate`]; the
//! actual ingest (real `iot_reading` rows through the F5 emission seam)
//! is driven by `scenario.rs`.

use serde::{Deserialize, Serialize};

use crate::sensors::{evaluate, SensorModel};

/// One synthetic sensor in the fleet config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorSpec {
    /// Anchor node NAME — the runner creates one anchor node per fleet
    /// `anchors` entry and maps names to the DB's node ids at setup.
    pub anchor: String,
    /// Metric name, e.g. `temperature_c` (non-empty — the ingest seam
    /// validates it).
    pub metric: String,
    /// Unit label, e.g. `C`.
    pub units: String,
    /// Milliseconds between readings (simulated time).
    pub cadence_ms: u64,
    /// The value model — see [`crate::sensors`].
    pub model: SensorModel,
}

/// The declarative fleet config (the `fleet` block of a scenario file).
///
/// ```json
/// {
///   "verse_name": "Sim Fleet",
///   "peers": ["alice", "bob"],
///   "ingest_peer": "alice",
///   "anchors": ["tower-a", "tower-b"],
///   "sensors": [
///     { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
///       "cadence_ms": 60000,
///       "model": { "type": "sine", "baseline": 15, "amplitude": 8,
///                  "period_ms": 3600000 } }
///   ],
///   "start_ms": 1750000000000,
///   "duration_ms": 3600000
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetConfig {
    /// Verse name for the run's verse.
    pub verse_name: String,
    /// Simulated peer names, in spawn order. The first is the verse
    /// creator; every peer opens the verse replica.
    pub peers: Vec<String>,
    /// Which peer ingests the fleet's readings (its DB thread writes the
    /// real `iot_reading` rows through the emission seam).
    pub ingest_peer: String,
    /// Anchor node names — one node per entry is created in the fleet's
    /// petal on the ingest peer.
    pub anchors: Vec<String>,
    /// The synthetic sensors.
    pub sensors: Vec<SensorSpec>,
    /// Simulated epoch-ms at fleet start (the scenario clock's origin).
    pub start_ms: u64,
    /// Simulated fleet duration. Sensors fire on their cadence while
    /// inside it; tick `k` fires at `start_ms + k * cadence_ms`.
    pub duration_ms: u64,
}

impl FleetConfig {
    /// Parse a fleet config from declarative JSON (A18: the fleet config
    /// is declarative).
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let config: Self =
            serde_json::from_str(json).map_err(|e| anyhow::anyhow!("invalid fleet config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Structural validation beyond serde: the ingest peer must be a fleet
    /// peer, every sensor's anchor must exist, cadences must be non-zero,
    /// and at least one peer/anchor must be declared.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.peers.is_empty() {
            anyhow::bail!("fleet config: at least one peer is required");
        }
        if !self.peers.contains(&self.ingest_peer) {
            anyhow::bail!(
                "fleet config: ingest_peer '{}' is not one of the fleet peers",
                self.ingest_peer
            );
        }
        if self.anchors.is_empty() {
            anyhow::bail!("fleet config: at least one anchor is required");
        }
        if self.sensors.is_empty() {
            anyhow::bail!("fleet config: at least one sensor is required");
        }
        for sensor in &self.sensors {
            if sensor.metric.trim().is_empty() {
                anyhow::bail!("fleet config: sensor metric must be non-empty");
            }
            if sensor.cadence_ms == 0 {
                anyhow::bail!("fleet config: sensor '{}' has cadence_ms 0", sensor.metric);
            }
            if !self.anchors.contains(&sensor.anchor) {
                anyhow::bail!(
                    "fleet config: sensor '{}' anchors to unknown anchor '{}'",
                    sensor.metric,
                    sensor.anchor
                );
            }
        }
        Ok(())
    }
}

/// One scheduled reading — a pure planner artifact, before the anchor's DB
/// node id is known (the runner substitutes it at fire time).
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduledReading {
    /// Simulated epoch-ms when the reading fires.
    pub at_ms: u64,
    /// Index into [`FleetConfig::sensors`].
    pub sensor: usize,
    /// 1-based tick index (tick `k` fires at `start_ms + k * cadence_ms`).
    pub tick: u64,
    /// The reading's value (pure `sensors::evaluate(model, tick, at_ms)`).
    pub value: f64,
}

/// Precompute the full fire schedule in total `(at_ms, sensor)` order —
/// a pure function of the config, so the same config always yields the
/// same plan, byte for byte.
pub fn plan_fleet(config: &FleetConfig) -> Vec<ScheduledReading> {
    let mut plan: Vec<ScheduledReading> = Vec::new();
    for (idx, sensor) in config.sensors.iter().enumerate() {
        let mut tick: u64 = 1;
        loop {
            let offset = tick.saturating_mul(sensor.cadence_ms);
            if offset > config.duration_ms {
                break;
            }
            let at_ms = config.start_ms.saturating_add(offset);
            plan.push(ScheduledReading {
                at_ms,
                sensor: idx,
                tick,
                value: evaluate(&sensor.model, tick, at_ms),
            });
            tick += 1;
        }
    }
    plan.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then(a.sensor.cmp(&b.sensor)));
    plan
}

/// The RFC-3339 UTC form of a simulated epoch-ms — the `recorded_at` the
/// fleet stamps on its readings (sensor time, from the SimClock domain).
pub fn rfc3339_from_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .expect("simulated epoch-ms is within chrono's supported range")
        .to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> FleetConfig {
        FleetConfig::parse(
            r#"{
                "verse_name": "Sim Fleet",
                "peers": ["alice", "bob"],
                "ingest_peer": "alice",
                "anchors": ["tower-a", "tower-b"],
                "sensors": [
                    { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                      "cadence_ms": 60000,
                      "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                 "period_ms": 3600000 } },
                    { "anchor": "tower-b", "metric": "battery_v", "units": "V",
                      "cadence_ms": 120000,
                      "model": { "type": "random_walk", "start": 4.2, "step": 0.05,
                                 "seed": 7 } }
                ],
                "start_ms": 1750000000000,
                "duration_ms": 300000
            }"#,
        )
        .expect("valid fleet config")
    }

    #[test]
    fn plan_is_pure_cadenced_and_totally_ordered() {
        let cfg = config();
        let plan = plan_fleet(&cfg);
        // Sensor A: cadence 60s over 300s → ticks 1..=5. Sensor B: 120s →
        // ticks 1..=2 (240s ≤ 300s; 360s would exceed).
        let a_count = plan.iter().filter(|r| r.sensor == 0).count();
        let b_count = plan.iter().filter(|r| r.sensor == 1).count();
        assert_eq!(a_count, 5, "60s cadence over 300s must fire 5 times");
        assert_eq!(b_count, 2, "120s cadence over 300s must fire 2 times");
        assert_eq!(plan.len(), 7);

        // Cadence: every fire time is start + k * cadence for its sensor.
        for reading in &plan {
            let sensor = &cfg.sensors[reading.sensor];
            let offset = reading.at_ms - cfg.start_ms;
            assert_eq!(offset % sensor.cadence_ms, 0, "off-cadence fire");
            assert_eq!(offset / sensor.cadence_ms, reading.tick);
            assert!(offset <= cfg.duration_ms, "fire beyond the duration");
        }

        // Total order: (at_ms, sensor) strictly non-decreasing.
        for pair in plan.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(
                (a.at_ms, a.sensor) <= (b.at_ms, b.sensor),
                "plan not ordered at {a:?} vs {b:?}"
            );
        }

        // Purity: same config → same plan (values included).
        let again = plan_fleet(&cfg);
        assert_eq!(plan, again);
    }

    #[test]
    fn plan_values_are_the_sensor_models() {
        let cfg = config();
        let plan = plan_fleet(&cfg);
        for reading in &plan {
            let sensor = &cfg.sensors[reading.sensor];
            let expected = evaluate(&sensor.model, reading.tick, reading.at_ms);
            assert_eq!(reading.value, expected, "planned value must be the model");
            assert_eq!(
                reading.at_ms,
                cfg.start_ms + reading.tick * sensor.cadence_ms
            );
        }
    }

    #[test]
    fn validation_rejects_broken_configs() {
        let mut cfg = config();
        cfg.ingest_peer = "nobody".into();
        assert!(cfg.validate().is_err(), "unknown ingest peer");

        let mut cfg = config();
        cfg.sensors[0].anchor = "tower-z".into();
        assert!(cfg.validate().is_err(), "unknown anchor");

        let mut cfg = config();
        cfg.sensors[0].cadence_ms = 0;
        assert!(cfg.validate().is_err(), "zero cadence");

        let mut cfg = config();
        cfg.sensors[0].metric = "  ".into();
        assert!(cfg.validate().is_err(), "empty metric");

        let mut cfg = config();
        cfg.anchors.clear();
        assert!(cfg.validate().is_err(), "no anchors");
    }

    #[test]
    fn recorded_at_renders_rfc3339_utc() {
        let raw = rfc3339_from_ms(1_750_000_000_000);
        let parsed = chrono::DateTime::parse_from_rfc3339(&raw).expect("RFC-3339 round trip");
        assert_eq!(parsed.timestamp_millis(), 1_750_000_000_000);
    }
}
