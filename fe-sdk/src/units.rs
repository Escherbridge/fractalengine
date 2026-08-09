//! Exact nano-base-unit scalars for canonical transforms (canonical-log SPEC-1 §4).
//!
//! The `i64` is authoritative in every one of these types. A renderer may derive an
//! explicitly approximate `f64` view, but that view is render-tier only: `f64` cannot
//! preserve every integer beyond roughly ±9.2e15 nanometres (about ±9,200 km), and that
//! precision limit is never permission to alter canonical data.

use serde::{Deserialize, Serialize};

/// Position in nanometres; the integer is authoritative, any `f64` view is render-tier only.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Nanometers(
    /// Signed nanometre count.
    pub i64,
);

/// Rotation in nanodegrees; the integer is authoritative, any `f64` view is render-tier only.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Nanodegrees(
    /// Signed nanodegree count.
    pub i64,
);

/// Scale in integer parts per billion; the integer is authoritative, any `f64` view is render-tier only.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct ScalePartsPerBillion(
    /// Signed parts-per-billion count, where 1_000_000_000 is unit scale.
    pub i64,
);

impl From<i64> for Nanometers {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<i64> for Nanodegrees {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<i64> for ScalePartsPerBillion {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

/// A three-axis nano-base-unit vector in X, Y, Z order; the integers are authoritative.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct NanoVec3 {
    /// X axis, in nano base units.
    pub x: i64,
    /// Y axis, in nano base units.
    pub y: i64,
    /// Z axis, in nano base units.
    pub z: i64,
}

impl NanoVec3 {
    /// Builds a vector from its X, Y, and Z nano base units.
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// Returns the axes in canonical X, Y, Z order.
    pub const fn as_array(&self) -> [i64; 3] {
        [self.x, self.y, self.z]
    }
}

impl From<[i64; 3]> for NanoVec3 {
    fn from(axes: [i64; 3]) -> Self {
        Self::new(axes[0], axes[1], axes[2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_newtypes_carry_the_full_i64_range() {
        assert_eq!(Nanometers::from(i64::MIN).0, i64::MIN);
        assert_eq!(Nanodegrees::from(i64::MAX).0, i64::MAX);
        assert_eq!(ScalePartsPerBillion::from(1_000_000_000).0, 1_000_000_000);
    }

    #[test]
    fn nano_vec3_preserves_axis_order() {
        let vector = NanoVec3::new(i64::MIN, 0, i64::MAX);
        assert_eq!(vector.as_array(), [i64::MIN, 0, i64::MAX]);
        assert_eq!(NanoVec3::from([i64::MIN, 0, i64::MAX]), vector);
    }

    #[test]
    fn nano_vec3_round_trips_through_json() {
        let vector = NanoVec3::new(-1, 1, i64::MAX);
        let encoded = serde_json::to_string(&vector).expect("serialize");
        let decoded: NanoVec3 = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, vector);
    }
}
