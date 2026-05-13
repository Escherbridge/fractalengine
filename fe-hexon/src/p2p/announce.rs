//! Hexon crate announcement types and local announcement store.

use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::manifest::HexonManifest;
use crate::signature::manifest_hash;

/// A signed announcement that a peer is hosting a particular hexon crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonAnnouncement {
    /// The hexon URI (e.g., "fe-model-v1@1.0.0")
    pub hexon_uri: String,
    /// BLAKE3 hash of the manifest JSON
    pub manifest_hash: String,
    /// Semantic version of the hexon
    pub version: String,
    /// The hexon type as a string (e.g., "model", "terrain")
    pub hexon_type: String,
    /// Tags for discovery
    pub tags: Vec<String>,
    /// DID of the peer hosting this crate
    pub hosting_peer_did: String,
    /// Approximate size in bytes
    pub approx_size_bytes: u64,
    /// ISO 8601 timestamp of when this announcement was made
    pub announced_at: String,
}

/// Inventory of hexon crates a peer is hosting, exchanged during handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateInventory {
    /// DID of the peer providing this inventory
    pub peer_did: String,
    /// List of hexon URIs this peer hosts
    pub hexon_uris: Vec<String>,
}

/// Local store of received announcements, keyed by hexon_uri.
#[derive(Debug, Clone)]
pub struct AnnouncementStore {
    entries: HashMap<String, Vec<HexonAnnouncement>>,
}

impl AnnouncementStore {
    /// Create a new empty announcement store.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Record an announcement, deduplicating by (hexon_uri, hosting_peer_did).
    pub fn record(&mut self, ann: HexonAnnouncement) {
        let bucket = self.entries.entry(ann.hexon_uri.clone()).or_default();
        // Deduplicate: if same peer already announced this URI, replace it
        if let Some(existing) = bucket
            .iter_mut()
            .find(|a| a.hosting_peer_did == ann.hosting_peer_did)
        {
            *existing = ann;
        } else {
            bucket.push(ann);
        }
    }

    /// Get all announcements for a specific hexon URI.
    pub fn get(&self, hexon_uri: &str) -> Vec<&HexonAnnouncement> {
        self.entries
            .get(hexon_uri)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get all announcements across all URIs.
    pub fn all(&self) -> Vec<&HexonAnnouncement> {
        self.entries.values().flatten().collect()
    }

    /// Create an announcement from a manifest and a local peer DID.
    pub fn from_manifest(manifest: &HexonManifest, local_peer_did: &str) -> HexonAnnouncement {
        let manifest_json = serde_json::to_vec(manifest).unwrap_or_default();
        let hash = manifest_hash(&manifest_json);
        let uri = format!("{}@{}", manifest.crate_id, manifest.version);

        HexonAnnouncement {
            hexon_uri: uri,
            manifest_hash: hash,
            version: manifest.version.clone(),
            hexon_type: manifest.hexon_type.to_string(),
            tags: manifest.tags.clone(),
            hosting_peer_did: local_peer_did.to_string(),
            approx_size_bytes: manifest.approx_size_bytes,
            announced_at: Utc::now().to_rfc3339(),
        }
    }
}

impl Default for AnnouncementStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the DHT topic key for a hexon URI.
///
/// This key is used for Kademlia DHT routing when publishing/subscribing
/// to announcements for a specific hexon crate.
pub fn dht_topic_key(hexon_uri: &str) -> String {
    format!("hexon/{}", hexon_uri)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{HexonKind, HexonManifest};

    fn test_manifest() -> HexonManifest {
        HexonManifest {
            schema_version: 1,
            crate_id: "test-model".to_string(),
            publisher_did: "did:key:z123".to_string(),
            publisher_name: "Test".to_string(),
            version: "1.0.0".to_string(),
            build_id: "build1".to_string(),
            name: "Test Model".to_string(),
            description: "A test".to_string(),
            tags: vec!["terrain".to_string(), "outdoor".to_string()],
            hexon_type: HexonKind::Model,
            created_at: "2026-05-10T00:00:00Z".to_string(),
            updated_at: "2026-05-10T00:00:00Z".to_string(),
            approx_size_bytes: 5000,
            min_engine_version: "0.8.0".to_string(),
            homepage_url: None,
            dependencies: vec![],
            signature: "sig".to_string(),
        }
    }

    #[test]
    fn test_record_and_get() {
        let mut store = AnnouncementStore::new();
        let ann = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer1");

        store.record(ann.clone());
        let results = store.get("test-model@1.0.0");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hosting_peer_did, "did:key:peer1");
    }

    #[test]
    fn test_deduplication() {
        let mut store = AnnouncementStore::new();
        let ann1 = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer1");
        let ann2 = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer1");

        store.record(ann1);
        store.record(ann2);
        // Same peer, same URI — should deduplicate
        assert_eq!(store.get("test-model@1.0.0").len(), 1);
    }

    #[test]
    fn test_multiple_peers() {
        let mut store = AnnouncementStore::new();
        let ann1 = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer1");
        let ann2 = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer2");

        store.record(ann1);
        store.record(ann2);
        assert_eq!(store.get("test-model@1.0.0").len(), 2);
    }

    #[test]
    fn test_all() {
        let mut store = AnnouncementStore::new();
        let ann1 = AnnouncementStore::from_manifest(&test_manifest(), "did:key:peer1");

        let mut manifest2 = test_manifest();
        manifest2.crate_id = "other-model".to_string();
        let ann2 = AnnouncementStore::from_manifest(&manifest2, "did:key:peer2");

        store.record(ann1);
        store.record(ann2);
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_dht_topic_key() {
        assert_eq!(dht_topic_key("test-model@1.0.0"), "hexon/test-model@1.0.0");
    }

    #[test]
    fn test_get_missing_uri() {
        let store = AnnouncementStore::new();
        assert!(store.get("nonexistent@0.0.1").is_empty());
    }
}
