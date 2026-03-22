use fe_database::PetalId;

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
