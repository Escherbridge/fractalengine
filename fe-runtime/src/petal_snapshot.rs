//! Serde-plain petal snapshot rows crossing the DB seam; see fe-database `src/handlers/AGENTS.md` §petal-snapshot.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the op-log petal filter the exporter applied (recorded in the snapshot meta).
pub const OPLOG_PREDICATE_VERSION: u32 = 1;

/// Every row of one petal's state, verbatim (`SELECT *`, record ids included); mirrors `fe_format::PetalSnapshotData`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PetalSnapshotRows {
    /// The petal row itself.
    pub petal: Value,
    /// Node rows of the petal, tombstoned rows included.
    #[serde(default)]
    pub nodes: Vec<Value>,
    /// Journal rows of those nodes.
    #[serde(default)]
    pub node_log: Vec<Value>,
    /// Field definitions scoped at or below the petal.
    #[serde(default)]
    pub field_defs: Vec<Value>,
    /// Room rows of the petal.
    #[serde(default)]
    pub rooms: Vec<Value>,
    /// Model rows placed in the petal.
    #[serde(default)]
    pub models: Vec<Value>,
    /// Asset rows referenced by the petal's nodes and models.
    #[serde(default)]
    pub assets: Vec<Value>,
    /// Role rows scoped exactly at the petal (inherited roles excluded).
    #[serde(default)]
    pub roles: Vec<Value>,
    /// IoT reading rows of the petal.
    #[serde(default)]
    pub iot_readings: Vec<Value>,
    /// Op-log entries matched by predicate [`OPLOG_PREDICATE_VERSION`], lamport ascending.
    #[serde(default)]
    pub oplog: Vec<Value>,
}

impl PetalSnapshotRows {
    /// The snapshot petal row's `petal_id` (empty when the row lacks one).
    pub fn petal_id(&self) -> String {
        self.petal
            .get("petal_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Row count per snapshot table, keyed by the `snapshot/state/<table>.json` stem.
    pub fn table_row_counts(&self) -> BTreeMap<String, usize> {
        let petal_rows = usize::from(!self.petal.is_null());
        [
            ("petal", petal_rows),
            ("nodes", self.nodes.len()),
            ("node_log", self.node_log.len()),
            ("field_defs", self.field_defs.len()),
            ("rooms", self.rooms.len()),
            ("models", self.models.len()),
            ("assets", self.assets.len()),
            ("roles", self.roles.len()),
            ("iot_readings", self.iot_readings.len()),
            ("oplog", self.oplog.len()),
        ]
        .into_iter()
        .map(|(table, count)| (table.to_string(), count))
        .collect()
    }
}
