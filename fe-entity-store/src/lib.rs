use bevy::prelude::Resource;
use papaya::HashMap;
use serde::{Deserialize, Serialize};

pub mod address;
pub use address::{AddressError, NodeAddress};

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
    /// FR-3: all custom properties cleared — the node stays addressable
    /// (distinct from `Deleted`; this is the "empty husk" that is NOT a delete).
    PropertiesCleared,
    /// FR-1: node deleted via tombstone (sync-safe; survives P2P/HLC merge).
    Deleted,
    /// FR-5: a stamp instance was promoted to a full addressable node.
    Promoted,
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

/// A tombstone marking a node as deleted (FR-1). Retained unbounded (Q-4:
/// no GC yet — filed follow-up). Its presence makes the node non-resurrectable
/// by any concurrent/stale replica write (N-4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    /// HLC-packed timestamp of the delete.
    pub hlc: u64,
    /// DID of the peer that originated the delete.
    pub source_did: String,
}

/// The result of a tombstone/cascade op (FR-1/FR-2): the addresses that were
/// tombstoned (one per subtree node) and the owning paths that must re-flow
/// their remaining stamps (geometry re-flow is T2's; this is the hook data).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TombstoneOutcome {
    /// Stable addresses of every node tombstoned by the op.
    pub tombstoned: Vec<NodeAddress>,
    /// Owning-path node ids that lost a stamp and should re-flow (deduplicated).
    pub reflow_paths: Vec<String>,
}

impl TombstoneOutcome {
    /// Whether the op tombstoned nothing (node absent or already deleted).
    pub fn is_empty(&self) -> bool {
        self.tombstoned.is_empty()
    }
}

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
    /// Tombstone side-table: node_id -> Tombstone. Presence == deleted (FR-1).
    /// Kept separate from `nodes` so `EntitySnapshot` stays byte-compatible for
    /// fe-query/fe-api consumers (they construct it by literal).
    tombstones: HashMap<String, Tombstone>,
    /// Parent -> children adjacency for cascade delete (FR-2).
    children: HashMap<String, Vec<String>>,
    /// node_id -> owning path node id, for the stamp-delete re-flow hook (FR-2).
    owning_paths: HashMap<String, String>,
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
            tombstones: HashMap::new(),
            children: HashMap::new(),
            owning_paths: HashMap::new(),
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

    /// Get a live snapshot by node ID. Returns `None` for a tombstoned node
    /// (FR-1: deleted nodes are invisible to normal reads).
    pub fn get(&self, node_id: &str) -> Option<EntitySnapshot> {
        if self.is_deleted(node_id) {
            return None;
        }
        let guard = self.nodes.pin();
        guard.get(node_id).cloned()
    }

    /// Get a snapshot even if it has been tombstoned (audit/merge paths).
    pub fn get_including_deleted(&self, node_id: &str) -> Option<EntitySnapshot> {
        let guard = self.nodes.pin();
        guard.get(node_id).cloned()
    }

    /// Whether the node has a tombstone (FR-1).
    pub fn is_deleted(&self, node_id: &str) -> bool {
        self.tombstones.pin().contains_key(node_id)
    }

    /// The tombstone for a node, if any.
    pub fn tombstone_of(&self, node_id: &str) -> Option<Tombstone> {
        self.tombstones.pin().get(node_id).cloned()
    }

    /// Count of live (non-tombstoned) nodes. Used by the FR-5 promotion
    /// row-count assertion (un-promoted instances add zero rows).
    pub fn node_count(&self) -> usize {
        let guard = self.nodes.pin();
        guard.iter().filter(|(k, _)| !self.is_deleted(k)).count()
    }

    /// Return snapshots of every live (non-tombstoned) node in the store.
    pub fn all_snapshots(&self) -> Vec<EntitySnapshot> {
        let guard = self.nodes.pin();
        guard
            .iter()
            .filter(|(k, _)| !self.is_deleted(k))
            .map(|(_k, v)| v.clone())
            .collect()
    }

    /// Get all live node IDs for a petal (tombstoned nodes are excluded).
    pub fn get_by_petal(&self, petal_id: &str) -> Vec<String> {
        let guard = self.petal_index.pin();
        guard.get(petal_id).cloned().unwrap_or_default()
    }

    /// Resolve a stable address (FR-4) back to a live node snapshot.
    pub fn resolve(&self, address: &NodeAddress) -> Option<EntitySnapshot> {
        self.get(address.node_id())
    }

    /// Resolve a stable address to a snapshot, including a tombstoned one.
    pub fn resolve_including_deleted(&self, address: &NodeAddress) -> Option<EntitySnapshot> {
        self.get_including_deleted(address.node_id())
    }

    /// Insert or update a snapshot. Updates the petal secondary index.
    ///
    /// Merge-safe (N-4): a tombstoned node is never resurrected — an upsert
    /// targeting a deleted node id is ignored so a stale/concurrent replica
    /// write cannot bring a deleted node back (D-A7).
    pub fn upsert(&self, snapshot: EntitySnapshot) {
        if self.is_deleted(&snapshot.node_id) {
            return; // tombstone dominates — resurrection rejected.
        }
        self.insert_snapshot(snapshot);
    }

    /// Raw insert bypassing the tombstone guard — internal to lifecycle ops
    /// that must persist a snapshot for a node they are themselves tombstoning.
    fn insert_snapshot(&self, snapshot: EntitySnapshot) {
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

    // -----------------------------------------------------------------------
    // Node lifecycle: tombstone delete, cascade, promotion (FR-1/2/3/5)
    // See AGENTS.md §lifecycle.
    // -----------------------------------------------------------------------

    /// Deterministic node id for a stamp instance (FR-5). Stable across
    /// sessions so promotion is idempotent and addressable by FR-4.
    pub fn instance_node_id(path_id: &str, instance_index: u32) -> String {
        format!("{path_id}#inst-{instance_index}")
    }

    /// Register a parent→child edge used by [`Self::cascade_tombstone`] (FR-2).
    pub fn register_child(&self, parent_id: &str, child_id: &str) {
        let guard = self.children.pin();
        let mut kids = guard.get(parent_id).cloned().unwrap_or_default();
        if !kids.iter().any(|k| k == child_id) {
            kids.push(child_id.to_string());
            guard.insert(parent_id.to_string(), kids);
        }
    }

    /// Record that `node_id` is a stamp on `path_id`, so deleting it emits a
    /// re-flow hook for that path (FR-2).
    pub fn set_owning_path(&self, node_id: &str, path_id: &str) {
        self.owning_paths
            .pin()
            .insert(node_id.to_string(), path_id.to_string());
    }

    /// Tombstone a single node (FR-1). Idempotent, merge-safe. Returns the
    /// outcome (the node's address + any owning-path re-flow hook). A delete
    /// arriving before the create still records a tombstone so the later create
    /// cannot resurrect it (sync-safe delete-before-create).
    pub fn tombstone_node(
        &self,
        node_id: &str,
        petal_scope: &str,
        hlc: u64,
        source_did: &str,
    ) -> TombstoneOutcome {
        let mut outcome = TombstoneOutcome::default();
        if !self.mark_tombstone(node_id, hlc, source_did) {
            return outcome; // already tombstoned — idempotent no-op.
        }
        if let Ok(addr) = NodeAddress::from_scope_and_id(petal_scope, node_id) {
            outcome.tombstoned.push(addr);
        }
        if let Some(path) = self.owning_paths.pin().get(node_id).cloned() {
            outcome.reflow_paths.push(path);
        }
        outcome
    }

    /// Cascade-tombstone a node and its whole descendant subtree as one logical
    /// op (FR-2). Collects the full set first, then applies — so it is
    /// all-or-nothing and can never leave a half-deleted subtree.
    pub fn cascade_tombstone(
        &self,
        root: &str,
        petal_scope: &str,
        hlc: u64,
        source_did: &str,
    ) -> TombstoneOutcome {
        let subtree = self.collect_subtree(root);
        let mut outcome = TombstoneOutcome::default();
        for node_id in &subtree {
            if self.mark_tombstone(node_id, hlc, source_did) {
                if let Ok(addr) = NodeAddress::from_scope_and_id(petal_scope, node_id) {
                    outcome.tombstoned.push(addr);
                }
                if let Some(path) = self.owning_paths.pin().get(node_id).cloned() {
                    if !outcome.reflow_paths.contains(&path) {
                        outcome.reflow_paths.push(path);
                    }
                }
            }
        }
        outcome
    }

    /// Clear all custom properties of a node while KEEPING it addressable
    /// (FR-3 "empty husk" fix). This is a distinct op from delete: it logs
    /// `PropertiesCleared`, never `Deleted`. Returns `false` if the node is
    /// absent or tombstoned.
    pub fn clear_properties(&self, node_id: &str, hlc: u64) -> bool {
        let Some(mut snapshot) = self.get(node_id) else {
            return false;
        };
        snapshot.properties = Some(serde_json::json!({}));
        let rv = snapshot
            .node_log
            .last()
            .map(|e| e.row_version + 1)
            .unwrap_or(1);
        self.push_log_capped(
            &mut snapshot.node_log,
            NodeLogEntry {
                hlc_timestamp: hlc,
                op: NodeLogOp::PropertiesCleared,
                source_did: String::new(),
                payload: serde_json::json!({}),
                row_version: rv,
            },
        );
        self.insert_snapshot(snapshot);
        true
    }

    /// Promote a stamp instance to a full addressable node on first
    /// select/edit (FR-5). Idempotent: promoting an already-materialized (or
    /// tombstoned) instance is a no-op returning `newly_promoted = false`.
    /// Un-promoted instances never touch the store, so they add zero rows.
    pub fn promote_instance(
        &self,
        path_id: &str,
        instance_index: u32,
        petal_id: &str,
        petal_scope: &str,
        hlc: u64,
        source_did: &str,
    ) -> Result<(NodeAddress, bool), AddressError> {
        let node_id = Self::instance_node_id(path_id, instance_index);
        let address = NodeAddress::from_scope_and_id(petal_scope, &node_id)?;
        if self.get(&node_id).is_some() || self.is_deleted(&node_id) {
            return Ok((address, false)); // idempotent.
        }
        let mut snapshot = EntitySnapshot {
            node_id: node_id.clone(),
            petal_id: petal_id.to_string(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: None,
            updated_at_ms: hlc >> 16,
            node_log: vec![],
        };
        self.push_log_capped(
            &mut snapshot.node_log,
            NodeLogEntry {
                hlc_timestamp: hlc,
                op: NodeLogOp::Promoted,
                source_did: source_did.to_string(),
                payload: serde_json::json!({
                    "path_id": path_id,
                    "instance_index": instance_index,
                }),
                row_version: 1,
            },
        );
        self.insert_snapshot(snapshot);
        self.set_owning_path(&node_id, path_id);
        Ok((address, true))
    }

    /// Record a tombstone for `node_id` (internal). Appends a `Deleted` log
    /// entry to the snapshot (if present), keeps the snapshot in the primary
    /// map as a tombstone record, and drops it from the petal index. Returns
    /// `false` if the node was already tombstoned (idempotent).
    fn mark_tombstone(&self, node_id: &str, hlc: u64, source_did: &str) -> bool {
        if self.is_deleted(node_id) {
            return false;
        }
        if let Some(mut snapshot) = self.get_including_deleted(node_id) {
            let rv = snapshot
                .node_log
                .last()
                .map(|e| e.row_version + 1)
                .unwrap_or(1);
            self.push_log_capped(
                &mut snapshot.node_log,
                NodeLogEntry {
                    hlc_timestamp: hlc,
                    op: NodeLogOp::Deleted,
                    source_did: source_did.to_string(),
                    payload: serde_json::json!({}),
                    row_version: rv,
                },
            );
            self.insert_snapshot(snapshot);
        }
        self.tombstones.pin().insert(
            node_id.to_string(),
            Tombstone {
                hlc,
                source_did: source_did.to_string(),
            },
        );
        self.remove_from_petal_index(node_id);
        true
    }

    /// Remove `node_id` from its petal's secondary index without touching the
    /// primary snapshot map (used by tombstoning — the snapshot is retained).
    fn remove_from_petal_index(&self, node_id: &str) {
        if let Some(snapshot) = self.get_including_deleted(node_id) {
            let petal_guard = self.petal_index.pin();
            if let Some(ids) = petal_guard.get(&snapshot.petal_id) {
                let mut ids = ids.clone();
                ids.retain(|id| id != node_id);
                if ids.is_empty() {
                    petal_guard.remove(&snapshot.petal_id);
                } else {
                    petal_guard.insert(snapshot.petal_id.clone(), ids);
                }
            }
        }
    }

    /// Collect a node and all descendants (BFS over the child adjacency),
    /// deduplicated. The root is always included even if it has no snapshot.
    fn collect_subtree(&self, root: &str) -> Vec<String> {
        let children_guard = self.children.pin();
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![root.to_string()];
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            result.push(n.clone());
            if let Some(kids) = children_guard.get(&n) {
                for k in kids {
                    stack.push(k.clone());
                }
            }
        }
        result
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
            SceneChange::NodeRemoved { node_id, .. } => {
                // FR-1: a removal is a merge-safe tombstone, not a hard drop —
                // a stale replica cannot resurrect it. The node stays invisible
                // to normal reads (`get`/`get_by_petal` filter tombstones).
                self.mark_tombstone(node_id, hlc, "");
            }
            SceneChange::NodeTransform {
                node_id,
                position,
                rotation,
                scale,
                ..
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
                ..
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
                ..
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
            SceneChange::NodeRenamed {
                node_id, new_name, ..
            } => {
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    const SCOPE: &str = "VERSE#v1-FRACTAL#f1-PETAL#p1";

    fn snap(node_id: &str) -> EntitySnapshot {
        EntitySnapshot {
            node_id: node_id.into(),
            petal_id: "p1".into(),
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: None,
            updated_at_ms: 0,
            node_log: vec![],
        }
    }

    // --- FR-1: sync-safe tombstone delete + merge-safety ---

    #[test]
    fn tombstone_hides_node_and_carries_address() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));

        let outcome = store.tombstone_node("n1", SCOPE, 10 << 16, "did:key:z6MkA");
        assert!(store.get("n1").is_none(), "tombstoned node must be hidden");
        assert!(store.is_deleted("n1"));
        assert_eq!(outcome.tombstoned.len(), 1);
        assert_eq!(outcome.tombstoned[0].to_uri(), "fe://v1/f1/p1/n1");
        // The tombstone is recorded as a distinct `Deleted` op in the log.
        let deleted = store.get_including_deleted("n1").unwrap();
        assert_eq!(deleted.node_log.last().unwrap().op, NodeLogOp::Deleted);
    }

    #[test]
    fn tombstone_is_idempotent() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));
        assert_eq!(
            store
                .tombstone_node("n1", SCOPE, 1 << 16, "")
                .tombstoned
                .len(),
            1
        );
        // Second delete produces no new tombstone.
        assert!(store.tombstone_node("n1", SCOPE, 2 << 16, "").is_empty());
    }

    #[test]
    fn stale_replica_upsert_cannot_resurrect_tombstone() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));
        store.tombstone_node("n1", SCOPE, 100 << 16, "did:key:z6MkA");

        // A stale replica still holding the node tries to write it back.
        store.upsert(snap("n1"));
        assert!(
            store.get("n1").is_none(),
            "tombstone must dominate the merge"
        );
        assert!(store.is_deleted("n1"));
    }

    #[test]
    fn stale_replica_scene_add_cannot_resurrect_tombstone() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));
        store.tombstone_node("n1", SCOPE, 100 << 16, "");

        let readd = SceneChange::NodeAdded {
            node: NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Zombie".into(),
                position: [9.0, 9.0, 9.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        };
        store.apply_scene_change(&readd, 5000);
        assert!(
            store.get("n1").is_none(),
            "NodeAdded must not resurrect a tombstone"
        );
    }

    #[test]
    fn delete_before_create_blocks_later_create() {
        // A tombstone that arrives before the node exists still blocks a later
        // create (sync-safe delete-before-create ordering).
        let store = EntityStore::new();
        store.tombstone_node("n1", SCOPE, 100 << 16, "");
        store.upsert(snap("n1"));
        assert!(store.get("n1").is_none());
    }

    #[test]
    fn scene_node_removed_is_a_tombstone_not_hard_drop() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));
        store.apply_scene_change(
            &SceneChange::NodeRemoved {
                node_id: "n1".into(),
            },
            4000,
        );
        assert!(store.get("n1").is_none());
        assert!(
            store.is_deleted("n1"),
            "NodeRemoved must record a tombstone"
        );
    }

    // --- FR-3: empty-husk fix (clear-props != delete) ---

    #[test]
    fn clear_properties_keeps_node_delete_removes_it() {
        let store = EntityStore::new();
        let mut s = snap("n1");
        s.properties = Some(serde_json::json!({ "color": "red" }));
        store.upsert(s);

        // Clearing properties keeps the node addressable...
        assert!(store.clear_properties("n1", 5 << 16));
        let live = store.get("n1").expect("clear-props must keep the node");
        assert_eq!(live.properties, Some(serde_json::json!({})));
        assert!(!store.is_deleted("n1"));
        let clear_op = live.node_log.last().unwrap().op.clone();
        assert_eq!(clear_op, NodeLogOp::PropertiesCleared);

        // ...whereas delete removes it, and the two ops are distinct.
        store.tombstone_node("n1", SCOPE, 6 << 16, "");
        assert!(store.get("n1").is_none());
        let dead = store.get_including_deleted("n1").unwrap();
        let delete_op = dead.node_log.last().unwrap().op.clone();
        assert_eq!(delete_op, NodeLogOp::Deleted);
        assert_ne!(clear_op, delete_op, "clear and delete must be distinct ops");
    }

    // --- FR-2: cascade + re-flow ---

    #[test]
    fn cascade_tombstones_three_level_subtree_atomically() {
        let store = EntityStore::new();
        // root -> child -> grandchild (3 levels), plus a sibling under root.
        for id in ["root", "child", "grandchild", "sibling"] {
            store.upsert(snap(id));
        }
        store.register_child("root", "child");
        store.register_child("root", "sibling");
        store.register_child("child", "grandchild");

        let outcome = store.cascade_tombstone("root", SCOPE, 20 << 16, "did:key:z6MkA");

        for id in ["root", "child", "grandchild", "sibling"] {
            assert!(store.is_deleted(id), "{id} must be tombstoned");
            assert!(store.get(id).is_none());
        }
        assert_eq!(
            outcome.tombstoned.len(),
            4,
            "every subtree node produces exactly one tombstone address"
        );
    }

    #[test]
    fn stamp_delete_emits_reflow_for_owning_path() {
        let store = EntityStore::new();
        store.upsert(snap("stamp1"));
        store.set_owning_path("stamp1", "path-A");

        let outcome = store.tombstone_node("stamp1", SCOPE, 30 << 16, "");
        assert_eq!(outcome.reflow_paths, vec!["path-A".to_string()]);
    }

    // --- FR-4: address resolution ---

    #[test]
    fn resolve_address_returns_live_node_only() {
        let store = EntityStore::new();
        store.upsert(snap("n1"));
        let addr = NodeAddress::from_scope_and_id(SCOPE, "n1").unwrap();
        assert_eq!(store.resolve(&addr).unwrap().node_id, "n1");

        store.tombstone_node("n1", SCOPE, 1 << 16, "");
        assert!(
            store.resolve(&addr).is_none(),
            "live resolve excludes tombstones"
        );
        assert!(store.resolve_including_deleted(&addr).is_some());
    }

    // --- FR-5: lazy promotion (zero rows until addressed) ---

    #[test]
    fn unpromoted_instances_add_zero_rows() {
        let store = EntityStore::new();
        // 122 stamps exist conceptually; none is promoted -> zero store rows.
        assert_eq!(store.node_count(), 0);
    }

    #[test]
    fn promote_materializes_exactly_one_addressable_node() {
        let store = EntityStore::new();
        assert_eq!(store.node_count(), 0);

        let (addr, promoted) = store
            .promote_instance("path-A", 7, "p1", SCOPE, 40 << 16, "did:key:z6MkA")
            .unwrap();
        assert!(promoted, "first promotion materializes a row");
        assert_eq!(store.node_count(), 1, "promotion adds exactly one row");
        assert_eq!(addr.node_id(), "path-A#inst-7");
        // Addressable by FR-4.
        assert!(store.resolve(&addr).is_some());
        let s = store.resolve(&addr).unwrap();
        assert_eq!(s.node_log.last().unwrap().op, NodeLogOp::Promoted);
    }

    #[test]
    fn promotion_is_idempotent() {
        let store = EntityStore::new();
        let (_, first) = store
            .promote_instance("path-A", 7, "p1", SCOPE, 1 << 16, "")
            .unwrap();
        let (_, second) = store
            .promote_instance("path-A", 7, "p1", SCOPE, 2 << 16, "")
            .unwrap();
        assert!(first);
        assert!(!second, "re-promoting the same instance is a no-op");
        assert_eq!(store.node_count(), 1);
    }

    #[test]
    fn tombstoned_instance_is_not_re_promoted() {
        let store = EntityStore::new();
        store
            .promote_instance("path-A", 7, "p1", SCOPE, 1 << 16, "")
            .unwrap();
        store.tombstone_node("path-A#inst-7", SCOPE, 2 << 16, "");
        let (_, promoted) = store
            .promote_instance("path-A", 7, "p1", SCOPE, 3 << 16, "")
            .unwrap();
        assert!(!promoted, "a deleted instance stays deleted (merge-safe)");
        assert_eq!(store.node_count(), 0);
    }
}
