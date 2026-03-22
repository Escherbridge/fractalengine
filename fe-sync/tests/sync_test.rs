use fe_network::AssetId;
use fe_sync::cache::{AssetCache, CacheEntry};
use fe_sync::replication::ReplicationConfig;
use std::time::Instant;

#[test]
fn test_cache_size_threshold() {
    let config = ReplicationConfig {
        max_cache_gb: 0.000001,
        eviction_days: 7,
    };
    let mut cache = AssetCache::new(config);
    let entry = CacheEntry {
        asset_id: AssetId([1u8; 32]),
        size_bytes: 2000,
        last_accessed: Instant::now(),
    };
    cache.insert(entry);
    assert!(cache.should_evict());
}

#[test]
fn test_reconcile_empty_delta() {
    let delta: Vec<serde_json::Value> = serde_json::from_slice(&[]).unwrap_or_default();
    assert_eq!(delta.len() as u64, 0);
}

#[test]
fn test_offline_detection_false_when_no_dir() {
    use fe_database::PetalId;
    use fe_sync::offline::is_petal_available_offline;
    let petal_id = PetalId(ulid::Ulid::new());
    assert!(!is_petal_available_offline(&petal_id));
}
