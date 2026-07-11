//! Mock host environment — an in-memory substitute for the real engine.
//!
//! [`MockHostEnv`] holds node snapshots, properties, scene changes, and a
//! [`SpyRecorder`] so tests can inject state *before* running a plugin script
//! and inspect mutations *after*.

use std::collections::HashMap;

use fe_sdk::node::NodeSnapshot;
use fe_sdk::scene::SceneChange;

use crate::mock_storage::MockStorage;
use crate::spy::SpyRecorder;

/// An in-memory mock of the FractalEngine host environment.
///
/// Plugin test scripts interact with this instead of a real Bevy/SurrealDB
/// backend. All reads come from pre-seeded data and all writes are captured
/// for later assertion.
#[derive(Debug, Clone)]
pub struct MockHostEnv {
    /// Node snapshots keyed by node ID.
    pub nodes: HashMap<String, NodeSnapshot>,
    /// Properties keyed by `(node_id, key)` with string values.
    pub properties: HashMap<(String, String), String>,
    /// Call recorder for spy-based assertions.
    pub spy: SpyRecorder,
    /// Scene changes accumulated during script execution.
    pub scene_changes: Vec<SceneChange>,
    /// In-memory backend implementing the `fe-sdk` storage + query traits.
    pub storage: MockStorage,
}

impl MockHostEnv {
    /// Create a new empty mock host environment.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            properties: HashMap::new(),
            spy: SpyRecorder::new(),
            scene_changes: Vec::new(),
            storage: MockStorage::new(),
        }
    }

    /// Insert a node snapshot into the mock.
    pub fn insert_node(&mut self, id: impl Into<String>, snapshot: NodeSnapshot) {
        self.nodes.insert(id.into(), snapshot);
    }

    /// Insert a property value for a node.
    pub fn insert_property(
        &mut self,
        node_id: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.properties
            .insert((node_id.into(), key.into()), value.into());
    }

    /// Get a node snapshot by ID.
    pub fn get_node(&self, id: &str) -> Option<&NodeSnapshot> {
        self.nodes.get(id)
    }

    /// Get a reference to the spy recorder.
    pub fn spy(&self) -> &SpyRecorder {
        &self.spy
    }

    /// Get accumulated scene changes.
    pub fn scene_changes(&self) -> &[SceneChange] {
        &self.scene_changes
    }
}

impl Default for MockHostEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use fe_sdk::node::NodeSnapshot;
    use fe_sdk::property::PropertyValue;

    use super::*;

    fn make_snapshot(id: &str, name: &str) -> NodeSnapshot {
        NodeSnapshot {
            node_id: id.into(),
            petal_id: "petal_01".into(),
            name: name.into(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            properties: HashMap::new(),
        }
    }

    #[test]
    fn mock_host_insert_and_get_node() {
        let mut host = MockHostEnv::new();
        let snap = make_snapshot("node_01", "Test Node");
        host.insert_node("node_01", snap);

        let retrieved = host.get_node("node_01").unwrap();
        assert_eq!(retrieved.node_id, "node_01");
        assert_eq!(retrieved.name, "Test Node");
        assert!(host.get_node("nonexistent").is_none());
    }

    #[test]
    fn mock_host_insert_and_get_property() {
        let mut host = MockHostEnv::new();
        host.insert_property("node_01", "temperature", "25.5");

        assert_eq!(
            host.properties.get(&("node_01".into(), "temperature".into())),
            Some(&"25.5".to_string())
        );
    }

    #[test]
    fn mock_host_scene_changes() {
        let mut host = MockHostEnv::new();
        assert!(host.scene_changes().is_empty());

        host.scene_changes.push(SceneChange::PropertyChanged {
            node_id: "n1".into(),
            key: "color".into(),
            value: PropertyValue::String("blue".into()),
        });
        assert_eq!(host.scene_changes().len(), 1);
    }

    #[test]
    fn mock_host_spy_is_accessible() {
        let host = MockHostEnv::new();
        assert!(!host.spy().was_called("get_node"));
    }
}
