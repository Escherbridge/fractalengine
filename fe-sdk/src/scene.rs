//! Scene change types for the extension SDK.
//!
//! These types mirror the engine's internal `SceneChange` enum but use SDK
//! types (`PropertyValue`, `f64` transforms) so extension authors never
//! depend on engine internals.

use serde::{Deserialize, Serialize};

use crate::property::PropertyValue;

/// A single change to the scene graph, delivered to extensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneChange {
    /// A new node was created.
    NodeAdded {
        /// The created node's id.
        node_id: String,
        /// The petal the node was created in.
        petal_id: String,
        /// The node's display name.
        name: String,
    },
    /// A node was removed from the scene.
    NodeRemoved {
        /// The removed node's id.
        node_id: String,
    },
    /// A custom property changed on a node.
    PropertyChanged {
        /// The affected node's id.
        node_id: String,
        /// The property key that changed.
        key: String,
        /// The new property value.
        value: PropertyValue,
    },
    /// A node's transform (position/rotation/scale) changed.
    TransformChanged {
        /// The affected node's id.
        node_id: String,
        /// New world position `[x, y, z]`.
        position: [f64; 3],
        /// New rotation quaternion `[x, y, z, w]`.
        rotation: [f64; 4],
        /// New scale `[x, y, z]`.
        scale: [f64; 3],
    },
}

/// A batch of scene changes, grouped by type for efficient processing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneChangeBatch {
    /// Nodes that were added this tick.
    pub added: Vec<SceneChange>,
    /// Nodes that were removed this tick.
    pub removed: Vec<SceneChange>,
    /// Property changes this tick.
    pub property_changes: Vec<SceneChange>,
    /// Transform changes this tick.
    pub transform_changes: Vec<SceneChange>,
}

impl SceneChangeBatch {
    /// Create an empty batch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a scene change into the appropriate category.
    pub fn push(&mut self, change: SceneChange) {
        match &change {
            SceneChange::NodeAdded { .. } => self.added.push(change),
            SceneChange::NodeRemoved { .. } => self.removed.push(change),
            SceneChange::PropertyChanged { .. } => self.property_changes.push(change),
            SceneChange::TransformChanged { .. } => self.transform_changes.push(change),
        }
    }

    /// Total number of changes across all categories.
    pub fn len(&self) -> usize {
        self.added.len()
            + self.removed.len()
            + self.property_changes.len()
            + self.transform_changes.len()
    }

    /// Whether the batch contains no changes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_change_batch_construction() {
        let mut batch = SceneChangeBatch::new();
        assert!(batch.is_empty());

        batch.push(SceneChange::NodeAdded {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            name: "Node 1".into(),
        });
        batch.push(SceneChange::NodeRemoved {
            node_id: "n2".into(),
        });
        batch.push(SceneChange::PropertyChanged {
            node_id: "n1".into(),
            key: "color".into(),
            value: PropertyValue::String("red".into()),
        });
        batch.push(SceneChange::TransformChanged {
            node_id: "n1".into(),
            position: [1.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        });

        assert_eq!(batch.len(), 4);
        assert!(!batch.is_empty());
        assert_eq!(batch.added.len(), 1);
        assert_eq!(batch.removed.len(), 1);
        assert_eq!(batch.property_changes.len(), 1);
        assert_eq!(batch.transform_changes.len(), 1);
    }

    #[test]
    fn scene_change_serde_roundtrip() {
        let change = SceneChange::NodeAdded {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            name: "Test".into(),
        };
        let json = serde_json::to_string(&change).unwrap();
        assert!(json.contains("\"op\":\"node_added\""));
        let roundtripped: SceneChange = serde_json::from_str(&json).unwrap();
        match roundtripped {
            SceneChange::NodeAdded { node_id, .. } => assert_eq!(node_id, "n1"),
            _ => panic!("wrong variant"),
        }
    }
}
