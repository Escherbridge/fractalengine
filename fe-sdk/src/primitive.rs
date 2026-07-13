//! BIM primitive descriptors (FR-1/FR-2, C5): the `{kind, dims, texture_ref}`
//! shape that rides on a node's `primitive` property as `PropertyValue::Json`.
//! See `fe-sdk/src/AGENTS.md` §primitive for the render-branch contract.

use serde::{Deserialize, Serialize};

/// The property-bag key a node's primitive descriptor is stored under.
pub const PRIMITIVE_PROPERTY_KEY: &str = "primitive";

/// Which Bevy shape a primitive materializes as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveKind {
    Cube,
    Plane,
    Cylinder,
    Sphere,
    /// FR-5: a path-driven vertical wall — shape comes from a `source_path`
    /// track's `gpx_points` polyline, not from `dims`. See `fe-sdk/src/AGENTS.md`
    /// §primitive-wall.
    Wall,
}

/// A first-party BIM primitive descriptor: shape kind, dimensions, and an
/// optional texture reference resolved through the [`crate::TextureRegistry`].
///
/// `dims` semantics per kind (all in world units):
/// - `Cube`: `[width, height, depth]`
/// - `Plane`: `[width, depth]`
/// - `Cylinder`: `[radius, height]`
/// - `Sphere`: `[radius]`
/// - `Wall`: `[height]` — the polyline shape comes from `source_path`, not `dims`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrimitiveDescriptor {
    pub kind: PrimitiveKind,
    pub dims: Vec<f32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub texture_ref: Option<String>,
    /// FR-5: for [`PrimitiveKind::Wall`], the `gpx_points`-carrying track
    /// node_id whose polyline drives the wall's shape. `None` for all other
    /// kinds. Serde-default so pre-Wall descriptors still parse.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_path: Option<String>,
}

impl PrimitiveDescriptor {
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
    fn descriptor_json_roundtrip_all_kinds() {
        let cases = vec![
            PrimitiveDescriptor {
                kind: PrimitiveKind::Cube,
                dims: vec![1.0, 2.0, 3.0],
                texture_ref: None,
                source_path: None,
            },
            PrimitiveDescriptor {
                kind: PrimitiveKind::Plane,
                dims: vec![5.0, 5.0],
                texture_ref: Some("tex-albedo-abc123".to_string()),
                source_path: None,
            },
            PrimitiveDescriptor {
                kind: PrimitiveKind::Cylinder,
                dims: vec![0.5, 2.0],
                texture_ref: None,
                source_path: None,
            },
            PrimitiveDescriptor {
                kind: PrimitiveKind::Sphere,
                dims: vec![1.5],
                texture_ref: None,
                source_path: None,
            },
            PrimitiveDescriptor {
                kind: PrimitiveKind::Wall,
                dims: vec![3.0],
                texture_ref: Some("brick-albedo".to_string()),
                source_path: Some("track-node-xyz".to_string()),
            },
        ];
        for desc in cases {
            let json = desc.to_json();
            let back = PrimitiveDescriptor::from_json(&json).expect("round-trips");
            assert_eq!(desc, back);
        }
    }

    #[test]
    fn descriptor_parses_bare_kind_dims_texture_ref_shape() {
        let json = serde_json::json!({
            "kind": "cube",
            "dims": [1.0, 1.0, 1.0],
            "texture_ref": "brick-01"
        });
        let desc = PrimitiveDescriptor::from_json(&json).expect("parses");
        assert_eq!(desc.kind, PrimitiveKind::Cube);
        assert_eq!(desc.dims, vec![1.0, 1.0, 1.0]);
        assert_eq!(desc.texture_ref.as_deref(), Some("brick-01"));
    }

    #[test]
    fn descriptor_texture_ref_defaults_to_none_when_absent() {
        let json = serde_json::json!({"kind": "sphere", "dims": [2.0]});
        let desc = PrimitiveDescriptor::from_json(&json).expect("parses");
        assert_eq!(desc.texture_ref, None);
    }

    #[test]
    fn descriptor_rejects_unknown_kind() {
        let json = serde_json::json!({"kind": "torus", "dims": [1.0]});
        assert!(PrimitiveDescriptor::from_json(&json).is_err());
    }

    #[test]
    fn wall_descriptor_carries_source_path() {
        let json = serde_json::json!({
            "kind": "wall",
            "dims": [3.0],
            "source_path": "track-abc"
        });
        let desc = PrimitiveDescriptor::from_json(&json).expect("wall parses");
        assert_eq!(desc.kind, PrimitiveKind::Wall);
        assert_eq!(desc.dims, vec![3.0]);
        assert_eq!(desc.source_path.as_deref(), Some("track-abc"));
        assert_eq!(desc.texture_ref, None);
    }

    #[test]
    fn source_path_defaults_to_none_when_absent() {
        // Pre-Wall descriptors (no `source_path` key) must still parse — the
        // serde default is what keeps existing JSON forward-compatible (C5).
        let json = serde_json::json!({"kind": "cube", "dims": [1.0, 1.0, 1.0]});
        let desc = PrimitiveDescriptor::from_json(&json).expect("parses");
        assert_eq!(desc.source_path, None);
    }

    #[test]
    fn source_path_omitted_from_serialized_when_none() {
        // `skip_serializing_if` keeps non-wall descriptors byte-identical to
        // their pre-Wall serialized form (no spurious `source_path: null`).
        let desc = PrimitiveDescriptor {
            kind: PrimitiveKind::Sphere,
            dims: vec![1.0],
            texture_ref: None,
            source_path: None,
        };
        let json = desc.to_json();
        assert!(json.get("source_path").is_none(), "source_path must be omitted when None: {json}");
    }
}
