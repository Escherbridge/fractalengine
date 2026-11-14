//! Hexon crate discovery via local announcement store search.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::manifest::{HexonManifest, HexonType};
use crate::p2p::announce::AnnouncementStore;

/// A search result containing manifest metadata and hosting peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HexonSearchResult {
    /// The manifest of the discovered hexon
    pub manifest: HexonManifest,
    /// DIDs of peers hosting this hexon
    pub hosting_peers: Vec<String>,
}

/// A query for searching the local announcement store.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Free-text query (substring match on hexon_uri)
    pub query: Option<String>,
    /// All specified tags must be present on the announcement
    pub tags: Vec<String>,
    /// Filter by hexon type
    pub kind: Option<HexonType>,
}

/// Search the local announcement store and return grouped results.
///
/// Filtering logic:
/// - `kind`: announcement's hexon_type must match (case-insensitive)
/// - `tags`: all specified tags must be present in the announcement
/// - `query`: hexon_uri must contain the query string (case-insensitive)
///
/// Results are grouped by hexon_uri, collecting all hosting peers.
pub fn search_announcements(
    store: &AnnouncementStore,
    query: &SearchQuery,
) -> Vec<HexonSearchResult> {
    let all = store.all();

    // Filter announcements
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|ann| {
            // Kind filter
            // Parsed, so legacy peers' kind spellings ("gpx", "TerrainTileset") still match.
            if let Some(ref kind) = query.kind {
                if ann.hexon_type.parse::<HexonType>().ok().as_ref() != Some(kind) {
                    return false;
                }
            }
            // Tags filter: all specified tags must be present
            if !query.tags.is_empty() {
                for tag in &query.tags {
                    if !ann.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                        return false;
                    }
                }
            }
            // Query filter: substring match on hexon_uri
            if let Some(ref q) = query.query {
                if !ann.hexon_uri.to_lowercase().contains(&q.to_lowercase()) {
                    return false;
                }
            }
            true
        })
        .collect();

    // Group by hexon_uri
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for ann in &filtered {
        grouped
            .entry(ann.hexon_uri.clone())
            .or_default()
            .push(ann.hosting_peer_did.clone());
    }

    // Build results — we reconstruct a minimal manifest from announcement data
    // since we don't store full manifests in the announcement store.
    grouped
        .into_iter()
        .map(|(uri, peers)| {
            // Find the first announcement for this URI to extract metadata
            let ann = filtered.iter().find(|a| a.hexon_uri == uri).unwrap();
            let kind: HexonType = ann.hexon_type.parse().unwrap_or(HexonType::Model);

            // Parse hexon_id and version from URI
            let (hexon_id, version) = fe_format::split_hexon_uri(&uri);
            let hexon_id = hexon_id.to_string();
            let version = version.unwrap_or("0.0.0").to_string();

            let manifest = HexonManifest {
                schema_version: "1.0.0".to_string(),
                hexon_id,
                hexon_type: kind,
                publisher_did: String::new(),
                publisher_name: None,
                version,
                build_id: None,
                name: String::new(),
                description: None,
                tags: ann.tags.clone(),
                created_at: String::new(),
                updated_at: String::new(),
                source_peer_did: None,
                approx_size_bytes: Some(ann.approx_size_bytes),
                min_engine_version: None,
                homepage_url: None,
                dependencies: vec![],
                platforms: vec![],
                address: None,
                integrity: None,
                signature: None,
            };

            HexonSearchResult {
                manifest,
                hosting_peers: peers,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::HexonType;
    use crate::p2p::announce::AnnouncementStore;

    fn make_store() -> AnnouncementStore {
        let mut store = AnnouncementStore::new();

        let manifest1 = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "forest-terrain".to_string(),
            hexon_type: HexonType::Terrain,
            publisher_did: "did:key:zpub1".to_string(),
            publisher_name: Some("Publisher A".to_string()),
            version: "2.1.0".to_string(),
            build_id: Some("b1".to_string()),
            name: "Forest Terrain".to_string(),
            description: Some("A forest terrain pack".to_string()),
            tags: vec!["terrain".to_string(), "forest".to_string()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(10_000),
            min_engine_version: Some("0.8.0".to_string()),
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: Some("sig1".to_string()),
        };

        let manifest2 = HexonManifest {
            schema_version: "1.0.0".to_string(),
            hexon_id: "robot-model".to_string(),
            hexon_type: HexonType::Model,
            publisher_did: "did:key:zpub2".to_string(),
            publisher_name: Some("Publisher B".to_string()),
            version: "1.0.0".to_string(),
            build_id: Some("b2".to_string()),
            name: "Robot Model".to_string(),
            description: Some("A robot model".to_string()),
            tags: vec!["model".to_string(), "sci-fi".to_string()],
            created_at: "2026-02-01T00:00:00Z".to_string(),
            updated_at: "2026-02-01T00:00:00Z".to_string(),
            source_peer_did: None,
            approx_size_bytes: Some(25_000),
            min_engine_version: Some("0.8.0".to_string()),
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            integrity: None,
            signature: Some("sig2".to_string()),
        };

        store.record(AnnouncementStore::from_manifest(
            &manifest1,
            "did:key:peerA",
        ));
        store.record(AnnouncementStore::from_manifest(
            &manifest1,
            "did:key:peerB",
        ));
        store.record(AnnouncementStore::from_manifest(
            &manifest2,
            "did:key:peerA",
        ));

        store
    }

    #[test]
    fn test_search_all_no_filter() {
        let store = make_store();
        let query = SearchQuery::default();
        let results = search_announcements(&store, &query);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_search_by_kind() {
        let store = make_store();
        let query = SearchQuery {
            kind: Some(HexonType::Terrain),
            ..Default::default()
        };
        let results = search_announcements(&store, &query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.hexon_id, "forest-terrain");
        // Two peers host it
        assert_eq!(results[0].hosting_peers.len(), 2);

        // A legacy peer's kind spelling ("gpx") still matches the unified type filter.
        let mut store = store;
        store.record(crate::p2p::announce::HexonAnnouncement {
            hexon_uri: "morning-run@0.1.0".to_string(),
            manifest_hash: "h".to_string(),
            version: "0.1.0".to_string(),
            hexon_type: "gpx".to_string(),
            tags: vec![],
            hosting_peer_did: "did:key:legacy".to_string(),
            approx_size_bytes: 1,
            announced_at: "2026-01-01T00:00:00Z".to_string(),
        });
        let gpx = SearchQuery { kind: Some(HexonType::GpxCollection), ..Default::default() };
        let results = search_announcements(&store, &gpx);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.hexon_type, HexonType::GpxCollection);
    }

    #[test]
    fn test_search_by_tags() {
        let store = make_store();
        let query = SearchQuery {
            tags: vec!["sci-fi".to_string()],
            ..Default::default()
        };
        let results = search_announcements(&store, &query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.hexon_id, "robot-model");
    }

    #[test]
    fn test_search_by_query_string() {
        let store = make_store();
        let query = SearchQuery {
            query: Some("forest".to_string()),
            ..Default::default()
        };
        let results = search_announcements(&store, &query);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].manifest.hexon_id, "forest-terrain");
    }

    #[test]
    fn test_search_combined_filters() {
        let store = make_store();
        let query = SearchQuery {
            kind: Some(HexonType::Terrain),
            tags: vec!["forest".to_string()],
            query: Some("terrain".to_string()),
        };
        let results = search_announcements(&store, &query);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_no_match() {
        let store = make_store();
        let query = SearchQuery {
            query: Some("nonexistent".to_string()),
            ..Default::default()
        };
        let results = search_announcements(&store, &query);
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_multiple_tags_must_all_match() {
        let store = make_store();
        let query = SearchQuery {
            tags: vec!["terrain".to_string(), "sci-fi".to_string()],
            ..Default::default()
        };
        let results = search_announcements(&store, &query);
        // No announcement has both "terrain" and "sci-fi"
        assert!(results.is_empty());
    }
}
