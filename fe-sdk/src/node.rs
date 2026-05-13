//! Node snapshot type for the extension SDK.
//!
//! [`NodeSnapshot`] is a read-only view of a node's state, decoupled from
//! the engine's internal ECS representation. Extension authors receive these
//! from scene queries and change notifications.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::property::PropertyValue;

/// A read-only snapshot of a node's current state.
///
/// All fields are `pub` for ergonomic access by extension code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    /// The node's unique identifier (ULID).
    pub node_id: String,
    /// The petal that owns this node.
    pub petal_id: String,
    /// Human-readable name.
    pub name: String,
    /// World-space position `[x, y, z]`.
    pub position: [f64; 3],
    /// Rotation as a quaternion `[x, y, z, w]`.
    pub rotation: [f64; 4],
    /// Scale factors `[x, y, z]`.
    pub scale: [f64; 3],
    /// Custom properties attached to this node.
    pub properties: HashMap<String, PropertyValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_snapshot_serde_roundtrip() {
        let mut props = HashMap::new();
        props.insert("color".into(), PropertyValue::String("#ff0000".into()));
        props.insert("mass".into(), PropertyValue::Number(1.5));

        let snapshot = NodeSnapshot {
            node_id: "node_01".into(),
            petal_id: "petal_01".into(),
            name: "Test Node".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            properties: props,
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let roundtripped: NodeSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(roundtripped.node_id, "node_01");
        assert_eq!(roundtripped.petal_id, "petal_01");
        assert_eq!(roundtripped.name, "Test Node");
        assert_eq!(roundtripped.position, [1.0, 2.0, 3.0]);
        assert_eq!(roundtripped.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(roundtripped.scale, [1.0, 1.0, 1.0]);
        assert_eq!(
            roundtripped.properties.get("color"),
            Some(&PropertyValue::String("#ff0000".into()))
        );
        assert_eq!(
            roundtripped.properties.get("mass"),
            Some(&PropertyValue::Number(1.5))
        );
    }
}
