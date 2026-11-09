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
