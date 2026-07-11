//! Mock thermostat device: an in-memory physical simulation used to prove the bridge
//! loop end-to-end. Deterministic (no RNG) so tests are reproducible.

/// A snapshot of what the device reports upstream on a given tick.
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub temperature: f64,
    pub humidity: f64,
    pub timestamp: u64,
}

impl Reading {
    /// Reject data that's structurally unusable before it ever reaches node properties:
    /// non-finite readings (NaN/Infinity) and a non-positive timestamp. Range-clamping
    /// of otherwise-finite values happens downstream in `ingest()` (see the .rhai script).
    pub fn validate(&self) -> Result<(), String> {
        if !self.temperature.is_finite() {
            return Err(format!("non-finite temperature reading: {}", self.temperature));
        }
        if !self.humidity.is_finite() {
            return Err(format!("non-finite humidity reading: {}", self.humidity));
        }
        if self.timestamp == 0 {
            return Err("non-positive timestamp".to_string());
        }
        Ok(())
    }
}

/// Hardware-safe setpoint bounds — the last line of defense before touching the device,
/// independent of whatever the extension script computed.
const MIN_SAFE_SETPOINT_C: f64 = -20.0;
const MAX_SAFE_SETPOINT_C: f64 = 80.0;

/// An in-memory mock thermostat: temperature drifts toward `setpoint` when powered, else
/// toward `ambient`.
#[derive(Debug, Clone)]
pub struct MockThermostat {
    pub temperature: f64,
    pub humidity: f64,
    pub setpoint: f64,
    pub power: bool,
    ambient: f64,
    tick: u64,
}

impl MockThermostat {
    /// Create a device starting at `ambient` temperature, powered off.
    pub fn new(ambient: f64) -> Self {
        Self { temperature: ambient, humidity: 45.0, setpoint: ambient, power: false, ambient, tick: 0 }
    }

    /// Advance device physics by one tick.
    pub fn step(&mut self) {
        self.tick += 1;
        let target = if self.power { self.setpoint } else { self.ambient };
        self.temperature += (target - self.temperature) * 0.3;
        self.humidity = 40.0 + 5.0 * ((self.tick as f64) * 0.5).sin();
    }

    /// Snapshot the current reading, stamped with `timestamp`.
    pub fn reading(&self, timestamp: u64) -> Reading {
        Reading { temperature: self.temperature, humidity: self.humidity, timestamp }
    }

    /// Apply a device command. Never panics: a missing or non-finite field is simply
    /// left unchanged, and the setpoint is clamped into a hardware-safe range.
    pub fn apply_command(&mut self, setpoint: Option<f64>, power: Option<bool>) {
        if let Some(sp) = setpoint {
            if sp.is_finite() {
                self.setpoint = sp.clamp(MIN_SAFE_SETPOINT_C, MAX_SAFE_SETPOINT_C);
            }
        }
        if let Some(p) = power {
            self.power = p;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_rejects_nan_and_infinite() {
        let r = Reading { temperature: f64::NAN, humidity: 50.0, timestamp: 1 };
        assert!(r.validate().is_err());
        let r = Reading { temperature: f64::INFINITY, humidity: 50.0, timestamp: 1 };
        assert!(r.validate().is_err());
    }

    #[test]
    fn reading_rejects_zero_timestamp() {
        let r = Reading { temperature: 20.0, humidity: 50.0, timestamp: 0 };
        assert!(r.validate().is_err());
    }

    #[test]
    fn reading_accepts_sane_values() {
        let r = Reading { temperature: 20.0, humidity: 50.0, timestamp: 1 };
        assert!(r.validate().is_ok());
    }

    #[test]
    fn apply_command_clamps_setpoint_and_ignores_non_finite() {
        let mut dev = MockThermostat::new(20.0);
        dev.apply_command(Some(500.0), Some(true));
        assert_eq!(dev.setpoint, MAX_SAFE_SETPOINT_C);

        dev.apply_command(Some(f64::NAN), None);
        assert_eq!(dev.setpoint, MAX_SAFE_SETPOINT_C, "NaN setpoint must be ignored");
        assert!(dev.power);
    }

    #[test]
    fn step_converges_toward_setpoint_when_powered() {
        let mut dev = MockThermostat::new(20.0);
        dev.setpoint = 30.0;
        dev.power = true;
        for _ in 0..20 {
            dev.step();
        }
        assert!((dev.temperature - 30.0).abs() < 0.1);
    }
}
