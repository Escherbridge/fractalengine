//! Shared path affine arithmetic for checked writers and native receipt preflight.

use super::EditorAffine;
use bevy::math::{DQuat, DVec3};
use serde_json::{Value, json};

impl EditorAffine {
    /// Transform anchor positions and relative handles, preserving every other slot.
    pub fn transform_path(&self, points: &Value) -> Result<Value, String> {
        if !self
            .pivot
            .iter()
            .chain(&self.translation)
            .chain(&self.scale)
            .all(|v| v.is_finite())
            || !self.rotation.iter().all(|v| v.is_finite())
        {
            return Err("batch affine must be finite".into());
        }
        let rotation = DQuat::from_xyzw(
            self.rotation[0] as f64,
            self.rotation[1] as f64,
            self.rotation[2] as f64,
            self.rotation[3] as f64,
        );
        if (rotation.length_squared() - 1.0).abs() > 0.0001
            || !self.scale.iter().all(|v| *v > 0.0 && *v <= 10000.0)
        {
            return Err("invalid affine rotation or scale".into());
        }
        let source = points
            .as_array()
            .filter(|rows| !rows.is_empty())
            .ok_or("stored route unavailable")?;
        let mut result = source.clone();
        for row in &mut result {
            let row = row
                .as_array_mut()
                .filter(|row| row.len() >= 4)
                .ok_or("invalid stored route row")?;
            for (base, relative) in [(0, false), (4, true), (7, true)] {
                if base != 0 && row.len() < base + 3 {
                    continue;
                }
                let mut vector = [0.0; 3];
                for i in 0..3 {
                    vector[i] = row[base + i]
                        .as_f64()
                        .filter(|v| v.is_finite())
                        .ok_or("route vector must be finite")?;
                }
                let mut vector = DVec3::from_array(vector);
                if !relative {
                    vector -= DVec3::from_array(self.pivot);
                }
                vector = rotation * (vector * DVec3::from_array(self.scale));
                if !relative {
                    vector += DVec3::from_array(self.pivot) + DVec3::from_array(self.translation);
                }
                if !vector.is_finite() || !vector.to_array().iter().all(|v| (*v as f32).is_finite())
                {
                    return Err("route exceeds render precision".into());
                }
                for i in 0..3 {
                    row[base + i] = json!(vector[i]);
                }
            }
        }
        Ok(json!(result))
    }
}
