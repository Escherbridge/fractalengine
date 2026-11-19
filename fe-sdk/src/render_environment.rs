//! Petal lighting and atmosphere override descriptor stored under
//! [`RENDER_ENVIRONMENT_PROPERTY_KEY`].
//!
//! Extensions attached to a petal or environment node can publish time-of-day,
//! weather, or operational lighting telemetry without calling renderer internals.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical node property key storing a [`RenderEnvironmentDescriptor`] JSON payload.
pub const RENDER_ENVIRONMENT_PROPERTY_KEY: &str = "render.environment";

/// Declarative scene lighting and atmosphere descriptor reconciled by the host renderer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RenderEnvironmentDescriptor {
    /// Ambient light RGB color `[r, g, b]`, clamped to `0.0..=1.0`.
    pub ambient_color: Option<[f32; 3]>,
    /// Ambient light illuminance/brightness in lux-scaled units (`>= 0.0`).
    pub ambient_brightness: Option<f32>,
    /// Directional sun/key light normalized direction `[x, y, z]` (pointing from origin toward light or world ray).
    pub sun_direction: Option<[f32; 3]>,
    /// Directional sun/key light RGB color `[r, g, b]`, clamped to `0.0..=1.0`.
    pub sun_color: Option<[f32; 3]>,
    /// Directional sun/key illuminance in lux (`>= 0.0`).
    pub sun_illuminance: Option<f32>,
    /// Viewport clear / sky RGB color `[r, g, b]`, clamped to `0.0..=1.0`.
    pub clear_color: Option<[f32; 3]>,
    /// Atmospheric fog color `[r, g, b]`, clamped to `0.0..=1.0`.
    pub fog_color: Option<[f32; 3]>,
    /// Exponential/linear fog density scalar clamped to `0.0..=1.0`.
    pub fog_density: Option<f32>,
}

fn sanitize_rgb(rgb: &mut [f32; 3]) -> bool {
    if !rgb.iter().all(|v| v.is_finite()) {
        return false;
    }
    for ch in rgb.iter_mut() {
        *ch = ch.clamp(0.0, 1.0);
    }
    true
}

impl RenderEnvironmentDescriptor {
    /// Parses a descriptor from JSON, returning `None` on null or malformed values.
    pub fn from_json(value: &Value) -> Option<Self> {
        if value.is_null() {
            return None;
        }
        let mut parsed: Self = serde_json::from_value(value.clone()).ok()?;
        if let Some(ref mut c) = parsed.ambient_color {
            if !sanitize_rgb(c) {
                return None;
            }
        }
        if let Some(ref mut b) = parsed.ambient_brightness {
            if !b.is_finite() {
                return None;
            }
            *b = b.max(0.0);
        }
        if let Some(ref mut dir) = parsed.sun_direction {
            if !dir.iter().all(|v| v.is_finite()) {
                return None;
            }
            let len_sq = dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2];
            if len_sq <= 1e-8 {
                return None;
            }
            let inv = len_sq.sqrt().recip();
            dir[0] *= inv;
            dir[1] *= inv;
            dir[2] *= inv;
        }
        if let Some(ref mut c) = parsed.sun_color {
            if !sanitize_rgb(c) {
                return None;
            }
        }
        if let Some(ref mut lux) = parsed.sun_illuminance {
            if !lux.is_finite() {
                return None;
            }
            *lux = lux.max(0.0);
        }
        if let Some(ref mut c) = parsed.clear_color {
            if !sanitize_rgb(c) {
                return None;
            }
        }
        if let Some(ref mut c) = parsed.fog_color {
            if !sanitize_rgb(c) {
                return None;
            }
        }
        if let Some(ref mut d) = parsed.fog_density {
            if !d.is_finite() {
                return None;
            }
            *d = d.clamp(0.0, 1.0);
        }
        Some(parsed)
    }

    /// Serializes this descriptor to a [`serde_json::Value`].
    pub fn to_json(&self) -> Result<Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_environment_and_normalizes_sun_direction() {
        let parsed = RenderEnvironmentDescriptor::from_json(&json!({
            "ambient_color": [0.2, 0.3, 0.4],
            "ambient_brightness": 320.0,
            "sun_direction": [3.0, -4.0, 0.0],
            "sun_color": [1.0, 0.92, 0.8],
            "sun_illuminance": 15000.0,
            "clear_color": [0.1, 0.15, 0.22],
            "fog_color": [0.6, 0.65, 0.7],
            "fog_density": 0.05
        }))
        .expect("parse env descriptor");

        let dir = parsed.sun_direction.expect("sun direction");
        assert!((dir[0] - 0.6).abs() < 1e-5);
        assert!((dir[1] + 0.8).abs() < 1e-5);
        assert!(dir[2].abs() < 1e-5);

        let encoded = parsed.to_json().expect("serialize env");
        let again = RenderEnvironmentDescriptor::from_json(&encoded).expect("re-parse env");
        assert_eq!(again, parsed);
    }

    #[test]
    fn rejects_zero_sun_direction_and_null() {
        assert!(RenderEnvironmentDescriptor::from_json(&Value::Null).is_none());
        assert!(RenderEnvironmentDescriptor::from_json(&json!({
            "sun_direction": [0.0, 0.0, 0.0]
        }))
        .is_none());
    }
}
