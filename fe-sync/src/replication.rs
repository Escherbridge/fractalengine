use fe_database::PetalId;

/// How a table's rows replicate (M2/F5): `Row` (mutable, tombstone-aware) vs
/// `Timeseries` (append-only union CRDT keyed by `reading_id`).
///
/// The canonical definition lives in `fe_database::replication_mode` because
/// the inbound apply handler — the DB thread's enforcement point — dispatches
/// on it, and fe-sync depends on fe-database (never the reverse). Re-exported
/// here so the replication side reads and writes the same type; the table
/// mapping (`for_table`) is pinned by the tests below.
pub use fe_database::replication_mode::ReplicationMode;

pub struct ReplicationConfig {
    pub max_cache_gb: f32,
    pub eviction_days: u64,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            max_cache_gb: 2.0,
            eviction_days: 7,
        }
    }
}

pub struct PetalReplica {
    pub petal_id: PetalId,
    pub last_seen: std::time::Instant,
    pub local_db_namespace: String,
}

pub trait ReplicationStore: Send + Sync {
    fn sync(&self, peer_id: &str, petal_id: &PetalId) -> anyhow::Result<Vec<u8>>;
}

pub struct IrohDocsStore;

impl ReplicationStore for IrohDocsStore {
    fn sync(&self, peer_id: &str, petal_id: &PetalId) -> anyhow::Result<Vec<u8>> {
        let petal_id_str = petal_id.0.to_string();
        tracing::info!(
            "IrohDocsStore::sync petal={} peer={}",
            petal_id_str,
            peer_id
        );
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::ReplicationMode;

    /// The table → mode mapping is the contract `iot_reading` union apply and
    /// the row-merge path both key on; a silent change here would flip a
    /// table's convergence semantics.
    #[test]
    fn for_table_maps_iot_reading_to_timeseries() {
        assert_eq!(
            ReplicationMode::for_table("iot_reading"),
            ReplicationMode::Timeseries
        );
    }

    #[test]
    fn for_table_maps_hierarchy_and_scene_tables_to_row() {
        for table in ["verse", "fractal", "petal", "node"] {
            assert_eq!(
                ReplicationMode::for_table(table),
                ReplicationMode::Row,
                "{table} must replicate as a row table"
            );
        }
    }

    #[test]
    fn for_table_defaults_unknown_tables_to_row() {
        for table in ["asset", "op_log", "__shards", ""] {
            assert_eq!(ReplicationMode::for_table(table), ReplicationMode::Row);
        }
    }
}
