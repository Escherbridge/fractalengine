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
    /// Batches that were applied completely, in commit order.
    pub transactions: Vec<Vec<fe_sdk::plugin_host::PendingOp>>,
    /// Optional full stored ancestry overrides for scope-denial journeys.
    pub petal_scopes: HashMap<String, String>,
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
            transactions: Vec::new(),
            petal_scopes: HashMap::new(),
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

impl fe_sdk::plugin_host::NodeReader for MockHostEnv {
    fn petal_scope(&self, petal_id: &str) -> Result<String, String> {
        Ok(self
            .petal_scopes
            .get(petal_id)
            .cloned()
            .unwrap_or_else(|| format!("VERSE#mock-FRACTAL#mock-PETAL#{petal_id}")))
    }

    fn get_node(&self, petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String> {
        Ok(self
            .nodes
            .get(node_id)
            .filter(|node| node.petal_id == petal_id)
            .cloned())
    }

    fn query_nodes(
        &self,
        petal_id: &str,
        query: &fe_sdk::plugin_host::NodeQuery,
    ) -> Result<Vec<NodeSnapshot>, String> {
        query.validate()?;
        let mut nodes: Vec<_> = self
            .nodes
            .values()
            .filter(|node| node.petal_id == petal_id && query.matches(node))
            .cloned()
            .collect();
        nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        nodes.truncate(query.limit);
        Ok(nodes)
    }
}

impl fe_sdk::plugin_host::TransactionSink for MockHostEnv {
    fn commit(
        &mut self,
        petal_id: &str,
        ops: &[fe_sdk::plugin_host::PendingOp],
    ) -> Result<fe_sdk::plugin_host::TransactionReceipt, String> {
        use fe_sdk::plugin_host::{json_property, PendingOp, TransactionReceipt};
        fe_sdk::plugin_host::validate_transaction(ops)?;
        let mut staged = self.clone();
        let mut created_node_ids = Vec::new();
        for op in ops {
            match op {
                PendingOp::SetProperty {
                    node_id,
                    key,
                    value,
                } => {
                    let node = staged
                        .nodes
                        .get_mut(node_id)
                        .filter(|node| node.petal_id == petal_id)
                        .ok_or("node not found or not permitted")?;
                    node.properties
                        .insert(key.clone(), json_property(value.clone()));
                    staged.properties.insert(
                        (node_id.clone(), key.clone()),
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string()),
                    );
                    staged.scene_changes.push(SceneChange::PropertyChanged {
                        node_id: node_id.clone(),
                        key: key.clone(),
                        value: json_property(value.clone()),
                    });
                }
                PendingOp::CreateNode {
                    petal_id: target,
                    name,
                } => {
                    if target != petal_id {
                        return Err("node creation targets another petal".into());
                    }
                    let node_id = ulid::Ulid::new().to_string();
                    staged.nodes.insert(
                        node_id.clone(),
                        NodeSnapshot {
                            node_id: node_id.clone(),
                            petal_id: target.clone(),
                            name: name.clone(),
                            position: [0.0; 3],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0; 3],
                            properties: HashMap::new(),
                        },
                    );
                    staged.scene_changes.push(SceneChange::NodeAdded {
                        node_id: node_id.clone(),
                        petal_id: target.clone(),
                        name: name.clone(),
                    });
                    created_node_ids.push(node_id);
                }
                PendingOp::DeleteNode { node_id } => {
                    let node = staged
                        .nodes
                        .get(node_id)
                        .filter(|node| node.petal_id == petal_id)
                        .ok_or("node not found or not permitted")?;
                    let _ = node;
                    staged.nodes.remove(node_id);
                    staged.properties.retain(|(id, _), _| id != node_id);
                    staged.scene_changes.push(SceneChange::NodeRemoved {
                        node_id: node_id.clone(),
                    });
                }
            }
        }
        staged.transactions.push(ops.to_vec());
        *self = staged;
        Ok(TransactionReceipt {
            op_count: ops.len(),
            created_node_ids,
        })
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
            host.properties
                .get(&("node_01".into(), "temperature".into())),
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
