//! Node material and shader override descriptor stored under [`RENDER_STYLE_PROPERTY_KEY`].
//!
//! Host renderers reconcile valid [`RenderStyleDescriptor`] payloads onto the
//! entity's `StandardMaterial` (and optional emissive pulse driver) without
//! requiring an extension to hold Bevy asset handles directly.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical node property key storing a [`RenderStyleDescriptor`] JSON payload.
pub const RENDER_STYLE_PROPERTY_KEY: &str = "render.style";

/// Alpha blending strategy mapped by the renderer onto material transparency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RenderAlphaMode {
    /// Fully opaque surface.
    #[default]
    Opaque,
    /// Alpha-blended translucency.
    Blend,
    /// Binary alpha-test cutout (`0.5` cutoff).
    Mask,
}

/// Declarative PBR material and shader override descriptor for a scene node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RenderStyleDescriptor {
    /// RGBA linear/sRGB-normalized tint color `[r, g, b, a]`, clamped to `0.0..=1.0`.
    pub base_color: Option<[f32; 4]>,
    /// Linear HDR emissive RGB color `[r, g, b]`, clamped to `>= 0.0`.
    pub emissive: Option<[f32; 3]>,
    /// Metallic factor clamped to `0.0..=1.0`.
    pub metallic: Option<f32>,
    /// Perceptual roughness factor clamped to `0.0..=1.0`.
    pub perceptual_roughness: Option<f32>,
    /// Whether lighting calculations are bypassed (self-lit telemetry overlays).
    pub unlit: Option<bool>,
    /// Transparency mode override.
    pub alpha_mode: Option<RenderAlphaMode>,
    /// Optional sinusoidal emissive pulse frequency in Hz (`0.0` disables pulsing).
    pub pulse_hz: Option<f32>,
}

impl RenderStyleDescriptor {
    /// Parses a descriptor from JSON, returning `None` if the payload is null or invalid.
    ///
    /// Numeric fields are sanitized so non-finite values fail validation and
    /// in-range PBR factors are clamped to safe bounds.
    pub fn from_json(value: &Value) -> Option<Self> {
        if value.is_null() {
            return None;
        }
        let mut parsed: Self = serde_json::from_value(value.clone()).ok()?;
        if let Some(ref mut c) = parsed.base_color {
            if !c.iter().all(|v| v.is_finite()) {
                return None;
            }
            for ch in c.iter_mut() {
                *ch = ch.clamp(0.0, 1.0);
            }
        }
        if let Some(ref mut e) = parsed.emissive {
            if !e.iter().all(|v| v.is_finite()) {
                return None;
            }
            for ch in e.iter_mut() {
                *ch = ch.max(0.0);
            }
        }
        if let Some(ref mut m) = parsed.metallic {
            if !m.is_finite() {
                return None;
            }
            *m = m.clamp(0.0, 1.0);
        }
        if let Some(ref mut r) = parsed.perceptual_roughness {
            if !r.is_finite() {
                return None;
            }
            *r = r.clamp(0.0, 1.0);
        }
        if let Some(ref mut hz) = parsed.pulse_hz {
            if !hz.is_finite() {
                return None;
            }
            *hz = hz.clamp(0.0, 30.0);
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
    fn round_trips_full_descriptor() {
        let desc = RenderStyleDescriptor {
            base_color: Some([0.9, 0.25, 0.1, 0.85]),
            emissive: Some([2.5, 0.4, 0.0]),
            metallic: Some(0.8),
            perceptual_roughness: Some(0.2),
            unlit: Some(false),
            alpha_mode: Some(RenderAlphaMode::Blend),
            pulse_hz: Some(2.0),
        };
        let encoded = desc.to_json().expect("serialize render style");
        let decoded = RenderStyleDescriptor::from_json(&encoded).expect("deserialize render style");
        assert_eq!(decoded, desc);
    }

    #[test]
    fn clamps_factors_and_rejects_null_or_wrong_shape() {
        assert!(RenderStyleDescriptor::from_json(&Value::Null).is_none());
        assert!(RenderStyleDescriptor::from_json(&json!("not-an-object")).is_none());

        let clamped = RenderStyleDescriptor::from_json(&json!({
            "base_color": [1.5, -0.2, 0.5, 2.0],
            "emissive": [-1.0, 4.0, 0.0],
            "metallic": 1.4,
            "perceptual_roughness": -0.5,
            "pulse_hz": 120.0,
            "alpha_mode": "mask"
        }))
        .expect("parse clamped descriptor");
        assert_eq!(clamped.base_color, Some([1.0, 0.0, 0.5, 1.0]));
        assert_eq!(clamped.emissive, Some([0.0, 4.0, 0.0]));
        assert_eq!(clamped.metallic, Some(1.0));
        assert_eq!(clamped.perceptual_roughness, Some(0.0));
        assert_eq!(clamped.pulse_hz, Some(30.0));
        assert_eq!(clamped.alpha_mode, Some(RenderAlphaMode::Mask));
    }
}
