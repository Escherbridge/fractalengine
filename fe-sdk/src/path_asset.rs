//! Path-asset stamp descriptor: the `{asset_path, spacing_mode, spacing_value,
//! count, tangent_align}` shape that rides on a track node's `path_asset`
//! property as `PropertyValue::Json`. Drives repeated model stamping along a
//! GPX path. See `fe-sdk/src/AGENTS.md` §path-asset.

use serde::{Deserialize, Serialize};

/// The property-bag key a track node's path-asset descriptor is stored under.
pub const PATH_ASSET_PROPERTY_KEY: &str = "path_asset";

/// How repeated instances are distributed along a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SpacingMode {
    /// Fixed world-units of arc length between consecutive instances.
    #[default]
    FixedSpacing,
    /// A fixed number of instances distributed evenly across the whole path.
    FixedCount,
}

/// A path-asset stamp descriptor: the model asset to stamp plus the spacing
/// policy and per-instance orientation flag.
///
/// - `asset_path`: the `blob://{hash}.glb` (or asset ref) to stamp, shared by
///   every instance (all resolve to one `Handle<Scene>`).
/// - `spacing_value`: world-units between instances (used in `FixedSpacing`).
/// - `count`: instance count (used in `FixedCount`).
/// - `tangent_align`: rotate each instance to face the local path tangent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathAssetDescriptor {
    pub asset_path: String,
    #[serde(default)]
    pub spacing_mode: SpacingMode,
    #[serde(default)]
    pub spacing_value: f32,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub tangent_align: bool,
}

impl PathAssetDescriptor {
    /// Parse a descriptor from a raw JSON value (the `PropertyValue::Json` payload).
    pub fn from_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }

    /// Serialize this descriptor to a raw JSON value for storage as `PropertyValue::Json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_json_roundtrip_both_modes() {
        let cases = vec![
            PathAssetDescriptor {
                asset_path: "blob://abc123.glb".to_string(),
                spacing_mode: SpacingMode::FixedSpacing,
                spacing_value: 2.5,
                count: 0,
                tangent_align: true,
            },
            PathAssetDescriptor {
                asset_path: "blob://def456.glb".to_string(),
                spacing_mode: SpacingMode::FixedCount,
                spacing_value: 0.0,
                count: 12,
                tangent_align: false,
            },
        ];
        for desc in cases {
            let json = desc.to_json();
            let back = PathAssetDescriptor::from_json(&json).expect("round-trips");
            assert_eq!(desc, back);
        }
    }

    #[test]
    fn descriptor_parses_bare_shape() {
        let json = serde_json::json!({
            "asset_path": "blob://model.glb",
            "spacing_mode": "fixed_count",
            "spacing_value": 3.0,
            "count": 8,
            "tangent_align": true
        });
        let desc = PathAssetDescriptor::from_json(&json).expect("parses");
        assert_eq!(desc.asset_path, "blob://model.glb");
        assert_eq!(desc.spacing_mode, SpacingMode::FixedCount);
        assert_eq!(desc.spacing_value, 3.0);
        assert_eq!(desc.count, 8);
        assert!(desc.tangent_align);
    }

    #[test]
    fn descriptor_defaults_tolerant_of_missing_fields() {
        // Only asset_path is required; everything else defaults.
        let json = serde_json::json!({ "asset_path": "blob://only.glb" });
        let desc = PathAssetDescriptor::from_json(&json).expect("parses");
        assert_eq!(desc.asset_path, "blob://only.glb");
        assert_eq!(desc.spacing_mode, SpacingMode::FixedSpacing);
        assert_eq!(desc.spacing_value, 0.0);
        assert_eq!(desc.count, 0);
        assert!(!desc.tangent_align);
    }

    #[test]
    fn descriptor_rejects_missing_asset_path() {
        let json = serde_json::json!({ "spacing_mode": "fixed_spacing", "spacing_value": 1.0 });
        assert!(PathAssetDescriptor::from_json(&json).is_err());
    }

    #[test]
    fn descriptor_rejects_unknown_spacing_mode() {
        let json =
            serde_json::json!({ "asset_path": "blob://x.glb", "spacing_mode": "logarithmic" });
        assert!(PathAssetDescriptor::from_json(&json).is_err());
    }

    #[test]
    fn spacing_mode_default_is_fixed_spacing() {
        assert_eq!(SpacingMode::default(), SpacingMode::FixedSpacing);
    }
}
