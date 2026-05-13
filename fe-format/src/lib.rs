//! Hexon v1.0.0 format crate for FractalEngine.
//!
//! Provides the `.hexon` ZIP archive format: manifest, entries catalog,
//! licensing, Ed25519 signing, and archive read/write.

pub mod manifest;
pub mod entries;
pub mod license;
pub mod signature;
mod archive;

// Re-exports for convenience
pub use manifest::*;
pub use entries::*;
pub use license::*;
pub use archive::*;
pub use signature::{sign_manifest, verify_manifest, verifying_key_to_did};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Schema types
// ---------------------------------------------------------------------------

/// Supported property value types in the .hexon schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PropertyType {
    String,
    Number,
    Bool,
    Datetime,
    GeometryPoint,
    GeometryPolygon,
    BlobRef,
    /// Hexon URI reference: `hexon://{publisher}/{hexon_id}@{version}#{entry_id}`
    HexonRef,
    /// amp-compatible hex UID string (128-bit).
    Address,
    Array,
    Object,
}

/// A field definition describing a custom property that nodes can carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    pub key: String,
    pub property_type: PropertyType,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// Schema definition embedded in the archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub field_defs: Vec<FieldDef>,
}

// ---------------------------------------------------------------------------
// Node DTO for export (includes custom properties)
// ---------------------------------------------------------------------------

/// A node snapshot for export, extending the runtime NodeDto with properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportNode {
    pub node_id: String,
    pub petal_id: String,
    pub name: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub has_asset: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,
    /// Append-only operation log for this node. Preserved across export/import
    /// for full audit trail and distributed merge support.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub node_log: Vec<serde_json::Value>,
}

impl From<fe_runtime::messages::NodeDto> for ExportNode {
    fn from(n: fe_runtime::messages::NodeDto) -> Self {
        Self {
            node_id: n.node_id,
            petal_id: n.petal_id,
            name: n.name,
            position: n.position,
            rotation: n.rotation,
            scale: n.scale,
            has_asset: n.has_asset,
            asset_path: n.asset_path,
            properties: None,
            node_log: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::HexonArchive;
    use crate::manifest::{HexonManifest, HexonType};

    /// Helper to build a minimal HexonManifest for tests.
    fn test_manifest(hexon_type: HexonType) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "test-hexon".into(),
            hexon_type,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Test Hexon".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            signature: None,
        }
    }

    #[test]
    fn roundtrip_scene_export_import() {
        let nodes = vec![ExportNode {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            name: "Test Node".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            has_asset: false,
            asset_path: None,
            properties: Some(serde_json::json!({"color": "red"})),
            node_log: vec![serde_json::json!({
                "hlc_timestamp": 1000,
                "op": "created",
                "source_did": "did:key:test",
                "payload": {},
                "row_version": 1
            })],
        }];
        let field_defs = vec![FieldDef {
            key: "color".into(),
            property_type: PropertyType::String,
            description: "Node color".into(),
            required: false,
        }];
        let assets = vec![("abc123".to_string(), b"fake-blob".to_vec())];
        let manifest = test_manifest(HexonType::Scene);

        let zip_bytes = HexonArchive::export_scene(
            nodes.clone(),
            field_defs.clone(),
            assets.clone(),
            manifest,
        )
        .expect("export failed");

        let data = HexonArchive::import(&zip_bytes).expect("import failed");

        assert_eq!(data.manifest.schema_version, "1.0.0");
        assert_eq!(data.manifest.publisher_did, "did:key:z6Mktest");
        assert_eq!(data.manifest.hexon_id, "test-hexon");
        assert_eq!(data.nodes.len(), 1);
        assert_eq!(data.nodes[0].node_id, "n1");
        assert_eq!(
            data.nodes[0].properties,
            Some(serde_json::json!({"color": "red"}))
        );
        assert_eq!(data.nodes[0].node_log.len(), 1);
        assert_eq!(data.nodes[0].node_log[0]["op"], "created");
        assert_eq!(data.field_defs.len(), 1);
        assert_eq!(data.field_defs[0].key, "color");
        assert_eq!(data.assets.len(), 1);
        assert_eq!(data.assets[0].0, "abc123");
        assert_eq!(data.assets[0].1, b"fake-blob");
    }

    #[test]
    fn property_type_serde() {
        // Existing types
        let pt = PropertyType::GeometryPoint;
        let json = serde_json::to_string(&pt).unwrap();
        assert_eq!(json, "\"geometry_point\"");
        let deserialized: PropertyType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, pt);

        // New HexonRef variant
        let hr = PropertyType::HexonRef;
        let json = serde_json::to_string(&hr).unwrap();
        assert_eq!(json, "\"hexon_ref\"");
        let deserialized: PropertyType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, hr);

        // New Address variant
        let addr = PropertyType::Address;
        let json = serde_json::to_string(&addr).unwrap();
        assert_eq!(json, "\"address\"");
        let deserialized: PropertyType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, addr);
    }

    #[test]
    fn generic_export_import() {
        let manifest = test_manifest(HexonType::Material);
        let entries = vec![AssetEntry {
            entry_id: "mat-001".into(),
            kind: EntryKind::Material,
            asset_hash: "deadbeef".into(),
            format: "json".into(),
            label: "Red Material".into(),
            tags: vec![],
            description: None,
            is_placeable: false,
            is_private: false,
            auto_scale: false,
            preview_image: None,
            center: None,
            extents: None,
            sub_assets: None,
            address: None,
            metadata: None,
        }];
        let assets = vec![("deadbeef".to_string(), b"{\"albedo\":\"red\"}".to_vec())];

        let zip_bytes = HexonArchive::export(
            manifest,
            entries.clone(),
            assets.clone(),
            Some(License::default()),
            None,
            None,
            None,
        )
        .expect("export failed");

        let data = HexonArchive::import(&zip_bytes).expect("import failed");

        assert_eq!(data.manifest.hexon_id, "test-hexon");
        // No entities/ section for non-scene types
        assert!(data.nodes.is_empty());
        assert!(data.field_defs.is_empty());
        // Entries and assets are present
        assert_eq!(data.entries.len(), 1);
        assert_eq!(data.entries[0].entry_id, "mat-001");
        assert_eq!(data.assets.len(), 1);
        assert_eq!(data.assets[0].0, "deadbeef");
    }

    #[test]
    fn unknown_fields_ignored_on_import() {
        // Export normally, then re-import — verifies serde(deny_unknown_fields) is NOT set
        let manifest = test_manifest(HexonType::Scene);
        let zip_bytes =
            HexonArchive::export_scene(vec![], vec![], vec![], manifest).unwrap();
        let data = HexonArchive::import(&zip_bytes).unwrap();
        assert!(data.nodes.is_empty());
    }

    #[test]
    fn terrain_export_import_roundtrip() {
        let manifest = test_manifest(HexonType::Terrain);
        let terrain_config = serde_json::json!({
            "crs": "EPSG:4326",
            "tile_size": 256,
            "elevation_source": "srtm"
        });
        let gpx_data = b"<gpx><trk><name>Trail 1</name></trk></gpx>".to_vec();
        let geojson_data = b"{\"type\":\"FeatureCollection\",\"features\":[]}".to_vec();

        let zip_bytes = HexonArchive::export(
            manifest,
            vec![],
            vec![],
            Some(License::default()),
            Some(&terrain_config),
            Some(&[("trail1.gpx".to_string(), gpx_data.clone())]),
            Some(&[("overlay1.geojson".to_string(), geojson_data.clone())]),
        )
        .expect("terrain export failed");

        let data = HexonArchive::import(&zip_bytes).expect("terrain import failed");

        // Manifest
        assert!(matches!(data.manifest.hexon_type, HexonType::Terrain));
        assert_eq!(data.manifest.schema_version, "1.0.0");

        // Terrain config
        assert!(data.terrain_config.is_some());
        assert_eq!(data.terrain_config.unwrap()["tile_size"], 256);

        // GPX files
        assert_eq!(data.gpx_files.len(), 1);
        assert_eq!(data.gpx_files[0].0, "trail1.gpx");
        assert_eq!(data.gpx_files[0].1, gpx_data);

        // GeoJSON overlays
        assert_eq!(data.geojson_overlays.len(), 1);
        assert_eq!(data.geojson_overlays[0].0, "overlay1.geojson");
        assert_eq!(data.geojson_overlays[0].1, geojson_data);

        // No entities for terrain type
        assert!(data.nodes.is_empty());
        assert!(data.field_defs.is_empty());
    }
}
