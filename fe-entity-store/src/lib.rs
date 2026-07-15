use bevy::prelude::Resource;
use papaya::HashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Entity snapshot
// ---------------------------------------------------------------------------

/// A point-in-time snapshot of a node's state, stored in the hot cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub node_id: String,
    pub petal_id: String,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub properties: Option<serde_json::Value>,
    pub updated_at_ms: u64,
    /// Last-K window of the node's op log (hot cache only; full history lives
    /// in the durable SurrealDB op_log — see AGENTS.md §node-log-cap).
    #[serde(default)]
    pub node_log: Vec<NodeLogEntry>,
}

/// A log entry recording a single operation on a node (see AGENTS.md §node-log-cap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLogEntry {
    /// HLC-packed timestamp (upper 48 bits = wall_ms, lower 16 = counter).
    pub hlc_timestamp: u64,
    /// The type of operation that was performed.
    pub op: NodeLogOp,
    /// DID of the peer that originated this operation.
    pub source_did: String,
    /// Arbitrary payload specific to the operation type.
    pub payload: serde_json::Value,
    /// Monotonically increasing version within this node's log.
    /// Used as the hidden `_row_version` metadata for "most recent" queries.
    pub row_version: u64,
}

/// Operation types that can appear in a node's log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum NodeLogOp {
    Created,
    TransformUpdate,
    PropertySet,
    PropertyDeleted,
    Renamed,
    AssetAttached,
    AssetDetached,
    HexonInstalled,
    Custom(String),
}

// ---------------------------------------------------------------------------
// Scene change types (mirrored to avoid depending on fe-runtime at the type level)
// ---------------------------------------------------------------------------

/// Describes a single change to the scene graph.
/// This is a local mirror of `fe_runtime::messages::SceneChange` so that
/// fe-entity-store does NOT depend on fe-runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SceneChange {
    NodeAdded {
        node: NodeSnapshot,
    },
    NodeRemoved {
        node_id: String,
    },
    NodeRenamed {
        node_id: String,
        new_name: String,
    },
    NodeTransform {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    TransformFailed {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    PropertyChanged {
        node_id: String,
        key: String,
        value: serde_json::Value,
    },
}

/// Node data as received from scene change events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub node_id: String,
    pub petal_id: String,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub has_asset: bool,
    pub asset_path: Option<String>,
}

// ---------------------------------------------------------------------------
// EntityStore
// ---------------------------------------------------------------------------

/// Lock-free, concurrent in-memory entity cache.
///
/// Backed by `papaya::HashMap` for wait-free reads and low-contention writes.
/// Designed to be populated from `SceneChange` events and queried by the API
/// layer and Bevy systems without DB round-trips.
/// Default last-K cap for the in-memory `node_log` (see AGENTS.md §node-log-cap).
pub const DEFAULT_NODE_LOG_CAP: usize = 1024;

#[derive(Resource)]
pub struct EntityStore {
    /// Primary index: node_id -> EntitySnapshot.
    nodes: HashMap<String, EntitySnapshot>,
    /// Secondary index: petal_id -> Vec<node_id>.
    petal_index: HashMap<String, Vec<String>>,
    /// Max in-memory log entries kept per node (last-K window).
    node_log_cap: usize,
}

impl EntityStore {
    pub fn new() -> Self {
        Self::with_node_log_cap(DEFAULT_NODE_LOG_CAP)
    }

    /// Create a store with a custom last-K cap for per-node hot-cache logs.
    pub fn with_node_log_cap(node_log_cap: usize) -> Self {
        Self {
            nodes: HashMap::new(),
            petal_index: HashMap::new(),
            node_log_cap: node_log_cap.max(1),
        }
    }

    /// The configured last-K cap for in-memory node logs.
    pub fn node_log_cap(&self) -> usize {
        self.node_log_cap
    }

    /// Push a log entry, trimming the front so the window stays within the cap.
    fn push_log_capped(&self, log: &mut Vec<NodeLogEntry>, entry: NodeLogEntry) {
        log.push(entry);
        if log.len() > self.node_log_cap {
            let excess = log.len() - self.node_log_cap;
            log.drain(..excess);
        }
    }

    /// Get a snapshot by node ID.
    pub fn get(&self, node_id: &str) -> Option<EntitySnapshot> {
        let guard = self.nodes.pin();
        guard.get(node_id).cloned()
    }

    /// Return snapshots of every node currently in the store.
    pub fn all_snapshots(&self) -> Vec<EntitySnapshot> {
        let guard = self.nodes.pin();
        guard.iter().map(|(_k, v)| v.clone()).collect()
    }

    /// Get all node IDs for a petal.
    pub fn get_by_petal(&self, petal_id: &str) -> Vec<String> {
        let guard = self.petal_index.pin();
        guard.get(petal_id).cloned().unwrap_or_default()
    }

    /// Insert or update a snapshot. Updates the petal secondary index.
    pub fn upsert(&self, snapshot: EntitySnapshot) {
        let node_id = snapshot.node_id.clone();
        let petal_id = snapshot.petal_id.clone();

        // Update primary index
        let nodes_guard = self.nodes.pin();
        nodes_guard.insert(node_id.clone(), snapshot);

        // Update secondary petal index
        let petal_guard = self.petal_index.pin();
        let mut ids = petal_guard.get(&petal_id).cloned().unwrap_or_default();
        if !ids.contains(&node_id) {
            ids.push(node_id);
            petal_guard.insert(petal_id, ids);
        }
    }

    /// Remove a node from the store. Cleans up the petal secondary index.
    pub fn remove(&self, node_id: &str) {
        let nodes_guard = self.nodes.pin();
        // Get the petal_id before removing so we can clean up the index
        if let Some(snapshot) = nodes_guard.get(node_id) {
            let petal_id = snapshot.petal_id.clone();
            nodes_guard.remove(node_id);

            let petal_guard = self.petal_index.pin();
            if let Some(ids) = petal_guard.get(&petal_id) {
                let mut ids = ids.clone();
                ids.retain(|id| id != node_id);
                if ids.is_empty() {
                    petal_guard.remove(&petal_id);
                } else {
                    petal_guard.insert(petal_id, ids);
                }
            }
        } else {
            nodes_guard.remove(node_id);
        }
    }

    /// Append a log entry to a node (last-K window). Returns the assigned row_version.
    pub fn append_log(
        &self,
        node_id: &str,
        op: NodeLogOp,
        source_did: &str,
        hlc_timestamp: u64,
        payload: serde_json::Value,
    ) -> Option<u64> {
        let mut snapshot = self.get(node_id)?;
        let row_version = snapshot
            .node_log
            .last()
            .map(|e| e.row_version + 1)
            .unwrap_or(1);
        self.push_log_capped(
            &mut snapshot.node_log,
            NodeLogEntry {
                hlc_timestamp,
                op,
                source_did: source_did.to_string(),
                payload,
                row_version,
            },
        );
        self.upsert(snapshot);
        Some(row_version)
    }

    /// Get the node log for a given node, optionally filtered to entries
    /// after a specific row_version (for incremental sync).
    pub fn get_node_log(&self, node_id: &str, after_version: Option<u64>) -> Vec<NodeLogEntry> {
        let Some(snapshot) = self.get(node_id) else {
            return vec![];
        };
        match after_version {
            Some(v) => snapshot
                .node_log
                .into_iter()
                .filter(|e| e.row_version > v)
                .collect(),
            None => snapshot.node_log,
        }
    }

    /// Apply a `SceneChange` event to the store.
    ///
    /// This is the primary ingestion path: wire up a Bevy system or bridge
    /// thread to call this for every scene change event.
    /// Each change is also appended to the node's immutable log.
    pub fn apply_scene_change(&self, change: &SceneChange, timestamp_ms: u64) {
        // Pack timestamp_ms into an HLC-compatible u64 (wall_ms in upper 48 bits)
        let hlc = timestamp_ms << 16;

        match change {
            SceneChange::NodeAdded { node } => {
                let mut snapshot = EntitySnapshot {
                    node_id: node.node_id.clone(),
                    petal_id: node.petal_id.clone(),
                    position: node.position,
                    rotation: [node.rotation[0], node.rotation[1], node.rotation[2]],
                    scale: node.scale,
                    properties: None,
                    updated_at_ms: timestamp_ms,
                    node_log: vec![],
                };
                // Seed the log with the creation event
                self.push_log_capped(
                    &mut snapshot.node_log,
                    NodeLogEntry {
                        hlc_timestamp: hlc,
                        op: NodeLogOp::Created,
                        source_did: String::new(),
                        payload: serde_json::json!({
                            "position": node.position,
                            "name": node.name,
                        }),
                        row_version: 1,
                    },
                );
                self.upsert(snapshot);
            }
            SceneChange::NodeRemoved { node_id } => {
                self.remove(node_id);
            }
            SceneChange::NodeTransform {
                node_id,
                position,
                rotation,
                scale,
            } => {
                if let Some(mut snapshot) = self.get(node_id) {
                    snapshot.position = *position;
                    snapshot.rotation = *rotation;
                    snapshot.scale = *scale;
                    snapshot.updated_at_ms = timestamp_ms;
                    let rv = snapshot
                        .node_log
                        .last()
                        .map(|e| e.row_version + 1)
                        .unwrap_or(1);
                    self.push_log_capped(
                        &mut snapshot.node_log,
                        NodeLogEntry {
                            hlc_timestamp: hlc,
                            op: NodeLogOp::TransformUpdate,
                            source_did: String::new(),
                            payload: serde_json::json!({
                                "position": position,
                                "rotation": rotation,
                                "scale": scale,
                            }),
                            row_version: rv,
                        },
                    );
                    self.upsert(snapshot);
                }
            }
            SceneChange::TransformFailed {
                node_id,
                position,
                rotation,
                scale,
            } => {
                if let Some(mut snapshot) = self.get(node_id) {
                    snapshot.position = *position;
                    snapshot.rotation = *rotation;
                    snapshot.scale = *scale;
                    snapshot.updated_at_ms = timestamp_ms;
                    // Rollbacks are not logged — they restore to last-known-good
                    self.upsert(snapshot);
                }
            }
            SceneChange::PropertyChanged {
                node_id,
                key,
                value,
            } => {
                if let Some(mut snapshot) = self.get(node_id) {
                    let props = snapshot
                        .properties
                        .get_or_insert_with(|| serde_json::json!({}));
                    if let Some(obj) = props.as_object_mut() {
                        obj.insert(key.clone(), value.clone());
                    }
                    snapshot.updated_at_ms = timestamp_ms;
                    let rv = snapshot
                        .node_log
                        .last()
                        .map(|e| e.row_version + 1)
                        .unwrap_or(1);
                    self.push_log_capped(
                        &mut snapshot.node_log,
                        NodeLogEntry {
                            hlc_timestamp: hlc,
                            op: NodeLogOp::PropertySet,
                            source_did: String::new(),
                            payload: serde_json::json!({
                                "key": key,
                                "value": value,
                            }),
                            row_version: rv,
                        },
                    );
                    self.upsert(snapshot);
                }
            }
            SceneChange::NodeRenamed { node_id, new_name } => {
                if let Some(mut snapshot) = self.get(node_id) {
                    let rv = snapshot
                        .node_log
                        .last()
                        .map(|e| e.row_version + 1)
                        .unwrap_or(1);
                    self.push_log_capped(
                        &mut snapshot.node_log,
                        NodeLogEntry {
                            hlc_timestamp: hlc,
                            op: NodeLogOp::Renamed,
                            source_did: String::new(),
                            payload: serde_json::json!({ "new_name": new_name }),
                            row_version: rv,
                        },
                    );
                    snapshot.updated_at_ms = timestamp_ms;
                    self.upsert(snapshot);
                }
            }
        }
    }
}

impl Default for EntityStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_snapshot(node_id: &str, petal_id: &str) -> EntitySnapshot {
        EntitySnapshot {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: None,
            updated_at_ms: 0,
            node_log: vec![],
        }
    }

    #[test]
    fn upsert_and_get() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        let snap = store.get("n1").expect("should exist");
        assert_eq!(snap.node_id, "n1");
        assert_eq!(snap.petal_id, "p1");
    }

    #[test]
    fn get_by_petal() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));
        store.upsert(make_snapshot("n2", "p1"));
        store.upsert(make_snapshot("n3", "p2"));

        let p1_nodes = store.get_by_petal("p1");
        assert_eq!(p1_nodes.len(), 2);
        assert!(p1_nodes.contains(&"n1".to_string()));
        assert!(p1_nodes.contains(&"n2".to_string()));

        let p2_nodes = store.get_by_petal("p2");
        assert_eq!(p2_nodes.len(), 1);
    }

    #[test]
    fn remove_cleans_index() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));
        store.upsert(make_snapshot("n2", "p1"));
        store.remove("n1");

        assert!(store.get("n1").is_none());
        let p1_nodes = store.get_by_petal("p1");
        assert_eq!(p1_nodes.len(), 1);
        assert_eq!(p1_nodes[0], "n2");
    }

    #[test]
    fn apply_node_added() {
        let store = EntityStore::new();
        let change = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&change, 1000);

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.position, [1.0, 2.0, 3.0]);
        assert_eq!(snap.updated_at_ms, 1000);
    }

    #[test]
    fn apply_transform_update() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        let change = SceneChange::NodeTransform {
            node_id: "n1".into(),
            position: [5.0, 6.0, 7.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        store.apply_scene_change(&change, 2000);

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.position, [5.0, 6.0, 7.0]);
        assert_eq!(snap.updated_at_ms, 2000);
    }

    #[test]
    fn apply_property_changed() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        let change = SceneChange::PropertyChanged {
            node_id: "n1".into(),
            key: "color".into(),
            value: serde_json::json!("red"),
        };
        store.apply_scene_change(&change, 3000);

        let snap = store.get("n1").unwrap();
        let props = snap.properties.unwrap();
        assert_eq!(props["color"], "red");
    }

    #[test]
    fn apply_node_removed() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));
        let change = SceneChange::NodeRemoved {
            node_id: "n1".into(),
        };
        store.apply_scene_change(&change, 4000);
        assert!(store.get("n1").is_none());
    }

    #[test]
    fn missing_node_transform_is_noop() {
        let store = EntityStore::new();
        let change = SceneChange::NodeTransform {
            node_id: "missing".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        };
        // Should not panic
        store.apply_scene_change(&change, 5000);
        assert!(store.get("missing").is_none());
    }

    #[test]
    fn node_log_created_on_add() {
        let store = EntityStore::new();
        let change = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&change, 1000);

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 1);
        assert_eq!(snap.node_log[0].op, NodeLogOp::Created);
        assert_eq!(snap.node_log[0].row_version, 1);
    }

    #[test]
    fn node_log_appends_on_transform() {
        let store = EntityStore::new();
        let add = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&add, 1000);

        let transform = SceneChange::NodeTransform {
            node_id: "n1".into(),
            position: [5.0, 6.0, 7.0],
            rotation: [0.0, 90.0, 0.0],
            scale: [2.0, 2.0, 2.0],
        };
        store.apply_scene_change(&transform, 2000);

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 2);
        assert_eq!(snap.node_log[0].op, NodeLogOp::Created);
        assert_eq!(snap.node_log[1].op, NodeLogOp::TransformUpdate);
        assert_eq!(snap.node_log[1].row_version, 2);
    }

    #[test]
    fn node_log_property_and_rename() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        let prop = SceneChange::PropertyChanged {
            node_id: "n1".into(),
            key: "color".into(),
            value: serde_json::json!("blue"),
        };
        store.apply_scene_change(&prop, 1000);

        let rename = SceneChange::NodeRenamed {
            node_id: "n1".into(),
            new_name: "Renamed".into(),
        };
        store.apply_scene_change(&rename, 2000);

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 2);
        assert_eq!(snap.node_log[0].op, NodeLogOp::PropertySet);
        assert_eq!(snap.node_log[1].op, NodeLogOp::Renamed);
    }

    #[test]
    fn append_log_manual() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        let rv = store.append_log(
            "n1",
            NodeLogOp::Custom("my_op".into()),
            "did:key:z6Mktest",
            42 << 16,
            serde_json::json!({"foo": "bar"}),
        );
        assert_eq!(rv, Some(1));

        let log = store.get_node_log("n1", None);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].source_did, "did:key:z6Mktest");
        assert_eq!(log[0].payload["foo"], "bar");
    }

    #[test]
    fn get_node_log_incremental() {
        let store = EntityStore::new();
        store.upsert(make_snapshot("n1", "p1"));

        store.append_log(
            "n1",
            NodeLogOp::PropertySet,
            "",
            1 << 16,
            serde_json::json!({}),
        );
        store.append_log(
            "n1",
            NodeLogOp::TransformUpdate,
            "",
            2 << 16,
            serde_json::json!({}),
        );
        store.append_log("n1", NodeLogOp::Renamed, "", 3 << 16, serde_json::json!({}));

        let all = store.get_node_log("n1", None);
        assert_eq!(all.len(), 3);

        let after_1 = store.get_node_log("n1", Some(1));
        assert_eq!(after_1.len(), 2);
        assert_eq!(after_1[0].row_version, 2);
    }

    #[test]
    fn node_log_op_serde() {
        let op = NodeLogOp::Custom("my_op".into());
        let json = serde_json::to_string(&op).unwrap();
        let deserialized: NodeLogOp = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, op);

        let op2 = NodeLogOp::Created;
        let json2 = serde_json::to_string(&op2).unwrap();
        assert_eq!(json2, "\"created\"");
    }

    #[test]
    fn transform_failed_does_not_log() {
        let store = EntityStore::new();
        let add = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&add, 1000);

        let fail = SceneChange::TransformFailed {
            node_id: "n1".into(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
        };
        store.apply_scene_change(&fail, 2000);

        // Rollbacks should NOT create log entries
        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 1); // Only the Created entry
    }

    #[test]
    fn node_log_capped_to_last_k() {
        let store = EntityStore::with_node_log_cap(4);
        store.upsert(make_snapshot("n1", "p1"));

        for i in 0..10u64 {
            store.append_log(
                "n1",
                NodeLogOp::PropertySet,
                "",
                i << 16,
                serde_json::json!({ "i": i }),
            );
        }

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 4, "log must stay within the cap");
        // Newest entries retained: row_versions 7..=10.
        let versions: Vec<u64> = snap.node_log.iter().map(|e| e.row_version).collect();
        assert_eq!(versions, vec![7, 8, 9, 10]);
        // row_version stays monotonic even after trimming.
        let rv = store.append_log(
            "n1",
            NodeLogOp::Renamed,
            "",
            11 << 16,
            serde_json::json!({}),
        );
        assert_eq!(rv, Some(11));
        assert_eq!(store.get("n1").unwrap().node_log.len(), 4);
    }

    #[test]
    fn node_log_cap_applies_to_scene_changes() {
        let store = EntityStore::with_node_log_cap(3);
        let add = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&add, 1000);

        for i in 0..8u64 {
            let transform = SceneChange::NodeTransform {
                node_id: "n1".into(),
                position: [i as f32, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            };
            store.apply_scene_change(&transform, 2000 + i);
        }

        let snap = store.get("n1").unwrap();
        assert_eq!(snap.node_log.len(), 3);
        // Latest transform (row_version 9 = Created + 8 transforms) is retained.
        assert_eq!(snap.node_log.last().unwrap().row_version, 9);
        assert_eq!(snap.node_log.last().unwrap().op, NodeLogOp::TransformUpdate);
    }

    #[test]
    fn default_cap_is_1024() {
        assert_eq!(EntityStore::new().node_log_cap(), DEFAULT_NODE_LOG_CAP);
        assert_eq!(DEFAULT_NODE_LOG_CAP, 1024);
        // Degenerate cap of 0 is clamped to 1.
        assert_eq!(EntityStore::with_node_log_cap(0).node_log_cap(), 1);
    }
}
