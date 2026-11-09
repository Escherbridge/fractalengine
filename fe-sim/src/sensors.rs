//! Pure sensor models — re-exported from `fe-sim-model` (E14a); tests stay here (batch filters).

pub use fe_sim_model::sensors::*;

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
