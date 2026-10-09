//! Pure sensor models (F8/A18): a reading's value is a **pure function of
//! (model config, tick, simulated time)** — never of hidden mutable state,
//! thread scheduling, or wall time. That purity is what makes a scripted
//! fleet run deterministic: the same config + the same tick always produce
//! the same value, on every machine, in every run.
//!
//! The three models of the A18 assertion:
//! * [`SensorModel::Sine`] — a smooth periodic signal (temperature-like).
//! * [`SensorModel::Weather`] — a diurnal sine plus keyed pseudo-random
//!   jitter (weather-like: a daily cycle that never repeats exactly).
//! * [`SensorModel::RandomWalk`] — a bounded cumulative walk (battery
//!   voltage / soil moisture-like drift).
//!
//! "Randomness" is a keyed hash ([`noise_unit`], splitmix64), so no RNG
//! state exists anywhere: `noise_unit(seed, k)` is a pure function.

use serde::{Deserialize, Serialize};

/// One sensor's model configuration — the declarative `model` field of a
/// fleet sensor (see `fleet::SensorSpec`). Serde-tagged by `type`
/// (`{"type": "sine", ...}`), so a fleet config is plain JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SensorModel {
    /// `baseline + amplitude * sin(2π * (at_ms + phase_ms) / period_ms)` —
    /// a smooth periodic signal.
    Sine {
        baseline: f64,
        amplitude: f64,
        /// Full-wave period in simulated ms (e.g. 3_600_000 = one hour).
        period_ms: u64,
        /// Wave offset in simulated ms.
        #[serde(default)]
        phase_ms: u64,
    },
    /// Weather-like: a diurnal cycle plus keyed per-tick jitter in
    /// `[-jitter, +jitter]` — a daily rhythm that never repeats exactly.
    Weather {
        baseline: f64,
        amplitude: f64,
        /// Daily-cycle length in simulated ms (e.g. 86_400_000 = one day).
        diurnal_ms: u64,
        /// Keyed-noise seed — the ONLY "randomness" input, explicit and
        /// reproducible.
        seed: u64,
        /// Jitter magnitude added to the diurnal value.
        jitter: f64,
    },
    /// Random walk: `start + Σ step * noise_unit(seed, k)` for k in 1..=tick
    /// — a cumulative drift bounded by `± tick * step` around `start`.
    /// Evaluated as a pure fold over the tick index (no carried state).
    RandomWalk {
        start: f64,
        /// Per-tick maximum delta magnitude.
        step: f64,
        /// Keyed-noise seed.
        seed: u64,
    },
}

/// The sensor reading value for `tick` (the tick index since the fleet's
/// start) at simulated time `at_ms` (the reading's sensor timestamp).
///
/// Pure: same inputs, same output, every time.
pub fn evaluate(model: &SensorModel, tick: u64, at_ms: u64) -> f64 {
    match model {
        SensorModel::Sine {
            baseline,
            amplitude,
            period_ms,
            phase_ms,
        } => {
            let t = (at_ms.saturating_add(*phase_ms)) as f64;
            baseline + amplitude * (2.0 * std::f64::consts::PI * t / *period_ms as f64).sin()
        }
        SensorModel::Weather {
            baseline,
            amplitude,
            diurnal_ms,
            seed,
            jitter,
        } => {
            let diurnal = (2.0 * std::f64::consts::PI * at_ms as f64 / *diurnal_ms as f64).sin();
            baseline + amplitude * diurnal + jitter * noise_unit(*seed, tick)
        }
        SensorModel::RandomWalk { start, step, seed } => {
            let mut value = *start;
            for k in 1..=tick {
                value += step * noise_unit(*seed, k);
            }
            value
        }
    }
}

/// Keyed pseudo-random in `[-1.0, 1.0)` — splitmix64 finalizer over
/// `seed ^ mix(index)`, mapped from 53 uniform bits. Pure and reproducible:
/// the "noise source" is a hash function, not an RNG.
pub fn noise_unit(seed: u64, index: u64) -> f64 {
    let h = splitmix64(seed ^ splitmix64(index));
    // Top 53 bits → [0, 1) → [-1, 1).
    ((h >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
}

/// splitmix64 finalizer — the same constants the JDK/Go splittable PRNGs
/// use; adequate for simulation noise (this is NOT a crypto primitive,
/// and does not need to be).
fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_hits_its_known_points() {
        let model = SensorModel::Sine {
            baseline: 15.0,
            amplitude: 8.0,
            period_ms: 3_600_000,
            phase_ms: 0,
        };
        let eps = 1e-9;
        assert!((evaluate(&model, 0, 0) - 15.0).abs() < eps);
        // Quarter period: sin(π/2) = 1 → baseline + amplitude.
        let peak = evaluate(&model, 1, 900_000);
        assert!((peak - 23.0).abs() < 1e-6, "quarter-period value: {peak}");
        // Half period: sin(π) = 0 → baseline.
        let mid = evaluate(&model, 2, 1_800_000);
        assert!((mid - 15.0).abs() < 1e-6, "half-period value: {mid}");
        // Full period wraps to the start value.
        let wrapped = evaluate(&model, 3, 3_600_000);
        assert!((wrapped - evaluate(&model, 0, 0)).abs() < 1e-9);
    }

    #[test]
    fn sine_phase_shifts_the_wave() {
        let plain = SensorModel::Sine {
            baseline: 0.0,
            amplitude: 1.0,
            period_ms: 1_000,
            phase_ms: 0,
        };
        let shifted = SensorModel::Sine {
            baseline: 0.0,
            amplitude: 1.0,
            period_ms: 1_000,
            phase_ms: 250, // quarter-period lead
        };
        let eps = 1e-9;
        // With a quarter-period phase lead, t=0 already sits at the peak.
        assert!((evaluate(&shifted, 0, 0) - 1.0).abs() < eps);
        assert!((evaluate(&plain, 0, 250) - evaluate(&shifted, 0, 0)).abs() < eps);
    }

    #[test]
    fn weather_stays_in_its_bounds_and_is_keyed() {
        let model = SensorModel::Weather {
            baseline: 12.0,
            amplitude: 6.0,
            diurnal_ms: 86_400_000,
            seed: 42,
            jitter: 1.5,
        };
        for tick in 0..200u64 {
            let at_ms = tick * 60_000;
            let v = evaluate(&model, tick, at_ms);
            assert!(
                (5.0..=19.0).contains(&v),
                "tick {tick} value {v} escaped [baseline-amplitude-jitter, +]"
            );
        }
        // Same (seed, tick) → same value; different seed → different series.
        assert_eq!(evaluate(&model, 7, 420_000), evaluate(&model, 7, 420_000));
        let other = match model.clone() {
            SensorModel::Weather {
                baseline,
                amplitude,
                diurnal_ms,
                jitter,
                ..
            } => SensorModel::Weather {
                baseline,
                amplitude,
                diurnal_ms,
                jitter,
                seed: 43,
            },
            _ => unreachable!("constructed as Weather above"),
        };
        let differs = (0..50u64).any(|t| {
            (evaluate(&model, t, t * 60_000) - evaluate(&other, t, t * 60_000)).abs() > 1e-12
        });
        assert!(differs, "different seeds should produce different series");
    }

    #[test]
    fn random_walk_starts_at_start_and_stays_bounded() {
        let model = SensorModel::RandomWalk {
            start: 4.2,
            step: 0.05,
            seed: 7,
        };
        assert_eq!(evaluate(&model, 0, 0), 4.2);
        let v1 = evaluate(&model, 1, 60_000);
        assert!((v1 - 4.2).abs() <= 0.05 + 1e-12, "tick 1 = {v1}");
        for tick in 0..64u64 {
            let v = evaluate(&model, tick, tick * 60_000);
            assert!(
                (v - 4.2).abs() <= 0.05 * tick as f64 + 1e-12,
                "tick {tick} value {v} escaped the ±tick*step envelope"
            );
        }
        // Purity: recomputing a past tick reproduces it exactly.
        assert_eq!(evaluate(&model, 5, 300_000), evaluate(&model, 5, 300_000));
    }

    #[test]
    fn noise_unit_is_uniform_and_pure() {
        assert_eq!(noise_unit(42, 7), noise_unit(42, 7));
        assert_ne!(noise_unit(42, 7), noise_unit(43, 7));
        for k in 0..1000u64 {
            let n = noise_unit(42, k);
            assert!((-1.0..1.0).contains(&n), "noise {n} escaped [-1, 1)");
        }
    }

    #[test]
    fn models_round_trip_through_declarative_json() {
        // A18: the fleet config is declarative — the model tag must parse
        // from plain JSON exactly as a fleet file writes it.
        let cases = [
            (
                r#"{"type":"sine","baseline":15,"amplitude":8,"period_ms":3600000,"phase_ms":0}"#,
                SensorModel::Sine {
                    baseline: 15.0,
                    amplitude: 8.0,
                    period_ms: 3_600_000,
                    phase_ms: 0,
                },
            ),
            (
                r#"{"type":"weather","baseline":12,"amplitude":6,"diurnal_ms":86400000,"seed":42,"jitter":1.5}"#,
                SensorModel::Weather {
                    baseline: 12.0,
                    amplitude: 6.0,
                    diurnal_ms: 86_400_000,
                    seed: 42,
                    jitter: 1.5,
                },
            ),
            (
                r#"{"type":"random_walk","start":4.2,"step":0.05,"seed":7}"#,
                SensorModel::RandomWalk {
                    start: 4.2,
                    step: 0.05,
                    seed: 7,
                },
            ),
        ];
        for (raw, expected) in cases {
            let parsed: SensorModel = serde_json::from_str(raw).expect("parse declarative model");
            assert_eq!(parsed, expected);
            let re = serde_json::to_string(&parsed).expect("serialize model");
            let back: SensorModel = serde_json::from_str(&re).expect("round trip");
            assert_eq!(back, parsed);
        }
    }
}
