//! Pure time arithmetic for the fleet driver (no host calls, so it runs and tests natively).

use std::ops::RangeInclusive;

/// Sensor ticks that fall in `(prev_elapsed_ms, elapsed_ms]`; empty when the cadence is zero.
pub fn due_ticks(prev_elapsed_ms: u64, elapsed_ms: u64, cadence_ms: u64) -> RangeInclusive<u64> {
    if cadence_ms == 0 {
        return 1..=0;
    }
    (prev_elapsed_ms / cadence_ms + 1)..=(elapsed_ms / cadence_ms)
}

/// True when a whole `period_ms` boundary was crossed between the two instants.
pub fn crossed(prev_elapsed_ms: u64, elapsed_ms: u64, period_ms: u64) -> bool {
    period_ms > 0 && elapsed_ms / period_ms > prev_elapsed_ms / period_ms
}

/// Route progress `0..=1` for a mover after `elapsed_ms` of fleet time (same maths as `plan_movers`).
pub fn mover_progress(elapsed_ms: u64, speed_mps: f64, start_offset_s: f64, route_length_m: f64, looped: bool) -> f64 {
    let travelled_s = (elapsed_ms as f64 / 1000.0 - start_offset_s).max(0.0);
    fe_sim_model::fleet::route_progress(travelled_s * speed_mps, route_length_m, looped)
}

/// Quaternion `[x, y, z, w]` turning +Z toward the horizontal heading `(tangent_x, tangent_z)`; none when degenerate.
pub fn yaw_quaternion(tangent_x: f64, tangent_z: f64) -> Option<[f64; 4]> {
    if !(tangent_x.is_finite() && tangent_z.is_finite()) || tangent_x.hypot(tangent_z) < 1e-9 {
        return None;
    }
    let half = tangent_x.atan2(tangent_z) / 2.0;
    Some([0.0, half.sin(), 0.0, half.cos()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_progress_and_heading() {
        assert_eq!(due_ticks(0, 1_250, 500).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(due_ticks(1_250, 1_900, 500).count(), 0);
        assert_eq!(due_ticks(1_900, 2_000, 500).collect::<Vec<_>>(), vec![4]);
        assert_eq!(due_ticks(0, 9_000, 0).count(), 0);
        assert!(crossed(400, 600, 500) && !crossed(500, 900, 500));
        // 10 m/s on a 100 m route: halfway after 5 s; a looped mover wraps, a one-way mover stops at the end.
        assert!((mover_progress(5_000, 10.0, 0.0, 100.0, false) - 0.5).abs() < 1e-9);
        assert!((mover_progress(15_000, 10.0, 0.0, 100.0, true) - 0.5).abs() < 1e-9);
        assert_eq!(mover_progress(60_000, 10.0, 0.0, 100.0, false), 1.0);
        assert_eq!(mover_progress(1_000, 10.0, 5.0, 100.0, false), 0.0);
        assert_eq!(yaw_quaternion(0.0, 0.0), None);
        let [_, y, _, w] = yaw_quaternion(1.0, 0.0).unwrap();
        assert!((y - (std::f64::consts::FRAC_PI_4).sin()).abs() < 1e-9 && w > 0.0);
    }
}
