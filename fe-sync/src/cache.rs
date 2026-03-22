use crate::replication::ReplicationConfig;
use fe_network::AssetId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct CacheEntry {
    pub asset_id: AssetId,
    pub size_bytes: u64,
    pub last_accessed: Instant,
}

pub struct AssetCache {
    entries: HashMap<AssetId, CacheEntry>,
    config: ReplicationConfig,
}

impl AssetCache {
    pub fn new(config: ReplicationConfig) -> Self {
        Self {
            entries: HashMap::new(),
            config,
        }
    }

    pub fn touch(&mut self, asset_id: AssetId) -> anyhow::Result<()> {
        if let Some(entry) = self.entries.get_mut(&asset_id) {
            entry.last_accessed = Instant::now();
        }
        Ok(())
    }

    pub fn evict_expired(&mut self) -> anyhow::Result<()> {
        let threshold = Duration::from_secs(self.config.eviction_days * 86400);
        self.entries
            .retain(|_, e| e.last_accessed.elapsed() < threshold);
        Ok(())
    }

    pub fn total_size_bytes(&self) -> u64 {
        self.entries.values().map(|e| e.size_bytes).sum()
    }

    pub fn should_evict(&self) -> bool {
        let max_bytes = (self.config.max_cache_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        self.total_size_bytes() > max_bytes
    }

    pub fn insert(&mut self, entry: CacheEntry) {
        self.entries.insert(entry.asset_id, entry);
    }
}
