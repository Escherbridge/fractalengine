//! Declarative fleet configuration (lab shell) over the pure `fe-sim-model` core (F8/A18, E14a).
//!
//! A fleet is DATA, not code. The lab `FleetConfig` adds the v1 lab-only fields (`peers`,
//! `ingest_peer`, `start_ms` real-clock bound) around the wasm-compatible [`FleetModel`] (sensors,
//! movers, planner), which lives in `fe-sim/model`. See `fe-sim/model/AGENTS.md`.

use serde::{Deserialize, Serialize};

pub use fe_sim_model::fleet::{
    plan_movers, route_progress, MoverSample, MoverSpec, SensorSpec, DEFAULT_MOVER_TICK_MS,
    MAX_PLANNED_READINGS, SIM_TIME_LIMIT_MS,
};
pub use fe_sim_model::fleet::{FleetModel, ScheduledReading};

#[cfg(test)]
use crate::sensors::evaluate;

/// Peers-per-fleet cap (validation; each peer is two threads + a Mem DB).
pub const MAX_FLEET_PEERS: usize = 16;

/// Real wall-clock epoch-ms (the `start_ms ≤ now` validation bound).
pub fn real_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn default_mover_tick_ms() -> u64 {
    DEFAULT_MOVER_TICK_MS
}

/// The declarative fleet config (the `fleet` block of a scenario file); `movers` is the v2 addition.
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
///   "movers": [
///     { "node": "truck-1", "route": "route-a", "speed_mps": 12.5, "loop": true }
///   ],
///   "start_ms": 1750000000000,
///   "duration_ms": 3600000
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FleetConfig {
    /// Verse name for the run's verse (lab-only).
    pub verse_name: String,
    /// Simulated peer names, in spawn order; the first is the verse creator (lab-only).
    pub peers: Vec<String>,
    /// Which peer ingests the fleet's readings (lab-only).
    pub ingest_peer: String,
    /// Anchor node names — one node per entry is created in the fleet's petal on the ingest peer.
    pub anchors: Vec<String>,
    /// The synthetic sensors.
    pub sensors: Vec<SensorSpec>,
    /// Route-following movers (v2; empty for v1 documents).
    #[serde(default)]
    pub movers: Vec<MoverSpec>,
    /// Milliseconds between mover samples (v2).
    #[serde(default = "default_mover_tick_ms")]
    pub mover_tick_ms: u64,
    /// Simulated epoch-ms at fleet start (the scenario clock's origin).
    pub start_ms: u64,
    /// Simulated fleet duration. Sensors fire on their cadence while inside it.
    pub duration_ms: u64,
}

impl FleetConfig {
    /// Parse a fleet config from declarative JSON (A18: the fleet config is declarative).
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let config: Self =
            serde_json::from_str(json).map_err(|e| anyhow::anyhow!("invalid fleet config: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    /// The pure model view of this config (what the planners and the wasm extension consume).
    pub fn model(&self) -> FleetModel {
        FleetModel {
            anchors: self.anchors.clone(),
            sensors: self.sensors.clone(),
            movers: self.movers.clone(),
            mover_tick_ms: self.mover_tick_ms,
            start_ms: self.start_ms,
            duration_ms: self.duration_ms,
        }
    }

    /// Lab checks (peers, ingest peer, anchors) plus the model checks plus "never in the future"
    /// (DEC-C13 — AGENTS.md §hlc-sim), against the real wall clock.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_at(real_now_ms())
    }

    /// [`Self::validate`] against an explicit real "now" (epoch-ms).
    pub fn validate_at(&self, now_ms: u64) -> anyhow::Result<()> {
        if self.peers.is_empty() {
            anyhow::bail!("fleet config: at least one peer is required");
        }
        if self.peers.len() > MAX_FLEET_PEERS {
            anyhow::bail!(
                "fleet config: {} peers exceeds the cap of {MAX_FLEET_PEERS}",
                self.peers.len()
            );
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
        if self.sensors.is_empty() && self.movers.is_empty() {
            anyhow::bail!("fleet config: at least one sensor or mover is required");
        }
        self.model().validate()?;
        if self.start_ms > now_ms {
            anyhow::bail!(
                "fleet config: start_ms {} is in the future (real now is {now_ms}) — sim \
                 time may be in the past, never the future: the session's clock drives the \
                 process-wide, forward-only HLC",
                self.start_ms
            );
        }
        Ok(())
    }

    /// How many readings [`plan_fleet`] schedules (see [`FleetModel::planned_readings`]).
    pub fn planned_readings(&self) -> u64 {
        self.model().planned_readings()
    }
}

/// Precompute the sensor fire schedule in total `(at_ms, sensor)` order (pure; see the model crate).
pub fn plan_fleet(config: &FleetConfig) -> Vec<ScheduledReading> {
    if config.planned_readings() > MAX_PLANNED_READINGS {
        tracing::warn!(
            planned = config.planned_readings(),
            "plan_fleet: fleet exceeds the planned-readings cap — nothing planned (validate first)"
        );
        return Vec::new();
    }
    fe_sim_model::fleet::plan_readings(&config.model())
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

        // Movers (v2): 100 m route at 10 m/s, 500 ms ticks, looped vs clamped, after a 1 s offset.
        let mut cfg = config();
        cfg.mover_tick_ms = 500;
        for (looped, expect_last) in [(false, 1.0), (true, 0.9)] {
            cfg.movers = vec![MoverSpec {
                node: "truck".into(),
                route: "route-a".into(),
                speed_mps: 10.0,
                looped,
                start_offset_s: 1.0,
            }];
            let samples = plan_movers(&cfg.model(), |r| (r == "route-a").then_some(100.0));
            assert_eq!(samples.len(), 600, "300 s / 500 ms ticks");
            assert_eq!(samples[0].progress, 0.0, "still inside the start offset");
            assert!((samples[2].progress - 0.05).abs() < 1e-9, "t=1.5s: 0.5 s * 10 m/s of 100 m");
            // End of run: 299 s * 10 m/s = 2990 m — clamped at 1.0, or wrapped to 0.9 when looping.
            let last = samples.last().unwrap().progress;
            assert!((last - expect_last).abs() < 1e-6, "end of run: {last}");
            assert!(samples.windows(2).all(|w| (w[0].at_ms, w[0].mover) <= (w[1].at_ms, w[1].mover)));
            assert_eq!(samples, plan_movers(&cfg.model(), |_| Some(100.0)), "pure");
        }
        assert!(plan_movers(&cfg.model(), |_| None).is_empty(), "unknown route is skipped");
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

        // v2 mover rows: each broken mover field is rejected.
        for (what, speed, offset, node) in [
            ("zero speed", 0.0, 0.0, "truck"),
            ("negative speed", -1.0, 0.0, "truck"),
            ("nan speed", f64::NAN, 0.0, "truck"),
            ("negative offset", 5.0, -1.0, "truck"),
            ("empty node", 5.0, 0.0, " "),
        ] {
            let mut cfg = config();
            cfg.movers.push(MoverSpec {
                node: node.into(),
                route: "route-a".into(),
                speed_mps: speed,
                looped: false,
                start_offset_s: offset,
            });
            assert!(cfg.validate().is_err(), "{what}");
        }
        let mut cfg = config();
        cfg.movers.push(MoverSpec {
            node: "truck".into(),
            route: "route-a".into(),
            speed_mps: 5.0,
            looped: true,
            start_offset_s: 0.0,
        });
        cfg.validate().expect("a well-formed mover is accepted");
    }

    /// DEC-C13 time bounds (table-driven): sim time may be past, never
    /// future, never ≥ 2^48 (HLC truncation), and the end of the run must
    /// stay representable — each rejection names its reason.
    #[test]
    fn validation_rejects_unsafe_simulated_time() {
        let now = 1_800_000_000_000u64;
        let cases: Vec<(u64, u64, &str)> = vec![
            (now + 1, 60_000, "in the future"),
            (SIM_TIME_LIMIT_MS, 60_000, "unrepresentable"),
            (u64::MAX, 60_000, "unrepresentable"),
            // Past start, but the run end crosses 2^48 / overflows u64 — the
            // duration that would otherwise reach chrono's panic.
            (now, SIM_TIME_LIMIT_MS - now, "time limit"),
            (now, u64::MAX, "time limit"),
        ];
        for (start_ms, duration_ms, why) in cases {
            let mut cfg = config();
            cfg.start_ms = start_ms;
            cfg.duration_ms = duration_ms;
            // A huge cadence keeps the planned-readings cap out of the way.
            for sensor in &mut cfg.sensors {
                sensor.cadence_ms = u64::MAX;
            }
            let err = cfg.validate_at(now).expect_err(&format!(
                "start {start_ms} + {duration_ms} must be rejected"
            ));
            assert!(err.to_string().contains(why), "{why}: {err}");
        }

        // The boundary itself is fine: start == now, past-dated runs.
        let mut cfg = config();
        cfg.start_ms = now;
        cfg.validate_at(now).expect("start == now is allowed");
        config()
            .validate_at(now)
            .expect("a past start is the normal case");
    }

    /// Size caps: peers ≤ 16, planned readings ≤ 100k, cadence ≥ 1.
    #[test]
    fn validation_rejects_oversized_fleets() {
        let mut cfg = config();
        cfg.peers = (0..=MAX_FLEET_PEERS).map(|i| format!("p{i}")).collect();
        cfg.ingest_peer = "p0".into();
        let err = cfg.validate().expect_err("17 peers");
        assert!(err.to_string().contains("peers exceeds"), "{err}");

        let mut cfg = config();
        cfg.sensors[0].cadence_ms = 1;
        cfg.duration_ms = MAX_PLANNED_READINGS + 1;
        let err = cfg.validate().expect_err("over the readings cap");
        assert!(err.to_string().contains("planned readings"), "{err}");

        let mut cfg = config();
        cfg.sensors[0].cadence_ms = 0;
        assert!(cfg.validate().is_err(), "zero cadence");
    }

    /// `duration_ms: u64::MAX` used to spin forever in the planner
    /// (`saturating_mul` pinned the offset ≤ the duration); it now returns
    /// promptly — and plans nothing over the cap.
    #[test]
    fn plan_fleet_terminates_on_a_maximal_duration() {
        let mut cfg = config();
        cfg.duration_ms = u64::MAX;
        assert!(plan_fleet(&cfg).is_empty(), "over the cap: nothing planned");

        // Within the cap, a near-u64::MAX cadence never overflows the tick
        // offset: exactly one tick fits.
        let mut cfg = config();
        cfg.duration_ms = u64::MAX;
        for sensor in &mut cfg.sensors {
            sensor.cadence_ms = u64::MAX / 2 + 1;
        }
        let plan = plan_fleet(&cfg);
        assert_eq!(plan.len(), cfg.sensors.len(), "one tick per sensor");
    }

    #[test]
    fn recorded_at_renders_rfc3339_utc() {
        let raw = rfc3339_from_ms(1_750_000_000_000);
        let parsed = chrono::DateTime::parse_from_rfc3339(&raw).expect("RFC-3339 round trip");
        assert_eq!(parsed.timestamp_millis(), 1_750_000_000_000);
    }
}
