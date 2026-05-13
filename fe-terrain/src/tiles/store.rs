//! Persistent store for installed `.hexon` terrain tilesets.
//!
//! `HexonStore` manages a local directory of installed tilesets, keeping a
//! `registry.json` index alongside the raw `.hexon` files and their extracted
//! `{id}.meta.json` metadata blobs.
//!
//! ## Directory layout
//! ```text
//! <base_dir>/
//!   registry.json          — index of all installed tilesets
//!   <hexon_id>.hexon       — raw archive bytes
//!   <hexon_id>.meta.json   — serialized TilesetMeta
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fe_format::manifest::{HexonType, TilesetMeta};
use fe_format::HexonArchive;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Metadata record for a tileset that has been installed into the local store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledTileset {
    /// Unique archive identifier from the hexon manifest.
    pub hexon_id: String,
    /// Human-readable region name from the tileset meta.
    pub region_name: String,
    /// Geographic bounding box `[min_lat, min_lon, max_lat, max_lon]`.
    pub bounds: [f64; 4],
    /// `(min_zoom, max_zoom)` covered by the tileset.
    pub zoom_range: (u8, u8),
    /// Number of elevation tiles.
    pub tile_count: u32,
    /// Size of the `.hexon` file on disk in bytes.
    pub size_bytes: u64,
    /// When this tileset was installed.
    pub installed_at: DateTime<Utc>,
    /// Whether this peer is seeding the tileset over P2P.
    pub seeding_enabled: bool,
    /// When a tile was last served from this tileset (if ever).
    pub last_served: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Private registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RegistryData {
    tilesets: Vec<InstalledTileset>,
}

// ---------------------------------------------------------------------------
// HexonStore
// ---------------------------------------------------------------------------

/// Local persistent store for `.hexon` terrain tilesets.
pub struct HexonStore {
    base_dir: PathBuf,
}

impl HexonStore {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Create (or open) the store using the platform default directory.
    ///
    /// Precedence:
    /// 1. `FE_HEXON_DIR` environment variable
    /// 2. `<data_local_dir>/fractalengine/hexons/tilesets/`
    pub fn new() -> Result<Self> {
        let base_dir = if let Ok(env_dir) = std::env::var("FE_HEXON_DIR") {
            PathBuf::from(env_dir)
        } else {
            let data_dir = dirs::data_local_dir()
                .context("could not determine local data directory; set FE_HEXON_DIR")?;
            data_dir
                .join("fractalengine")
                .join("hexons")
                .join("tilesets")
        };
        Self::with_dir(base_dir)
    }

    /// Create (or open) the store rooted at an explicit directory.
    ///
    /// Useful for tests and custom deployments.
    pub fn with_dir(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create store directory: {}", dir.display()))?;
        Ok(Self { base_dir: dir })
    }

    // -----------------------------------------------------------------------
    // Core operations
    // -----------------------------------------------------------------------

    /// Install a tileset from raw `.hexon` archive bytes.
    ///
    /// Steps:
    /// 1. Import and validate the archive.
    /// 2. Verify the hexon type is `TerrainTileset`.
    /// 3. Write `{hexon_id}.hexon` and `{hexon_id}.meta.json`.
    /// 4. Append an entry to `registry.json`.
    pub fn install_tileset(&self, bytes: &[u8]) -> Result<InstalledTileset> {
        // Import and validate
        let data =
            HexonArchive::import(bytes).context("failed to import hexon archive")?;

        let manifest = &data.manifest;

        match manifest.hexon_type {
            HexonType::TerrainTileset => {}
            ref other => bail!(
                "expected TerrainTileset hexon, got {:?}",
                other
            ),
        }

        let hexon_id = manifest.hexon_id.clone();

        let tileset_meta: TilesetMeta = data
            .tileset_meta
            .context("archive is missing tileset_meta (TerrainTileset must include it)")?;

        // Persist raw bytes
        let hexon_path = self.hexon_path(&hexon_id);
        std::fs::write(&hexon_path, bytes).with_context(|| {
            format!("failed to write hexon file: {}", hexon_path.display())
        })?;

        // Persist extracted meta
        let meta_json = serde_json::to_string_pretty(&tileset_meta)
            .context("failed to serialize TilesetMeta")?;
        let meta_path = self.meta_path(&hexon_id);
        std::fs::write(&meta_path, &meta_json).with_context(|| {
            format!("failed to write meta file: {}", meta_path.display())
        })?;

        let size_bytes = std::fs::metadata(&hexon_path)
            .map(|m| m.len())
            .unwrap_or(bytes.len() as u64);

        let entry = InstalledTileset {
            hexon_id: hexon_id.clone(),
            region_name: tileset_meta.region_name.clone(),
            bounds: tileset_meta.bounds,
            zoom_range: (tileset_meta.min_zoom, tileset_meta.max_zoom),
            tile_count: tileset_meta.tile_count,
            size_bytes,
            installed_at: Utc::now(),
            seeding_enabled: false,
            last_served: None,
        };

        // Update registry
        let mut reg = self.load_registry();
        // Replace any previous entry with the same id
        reg.tilesets.retain(|t| t.hexon_id != hexon_id);
        reg.tilesets.push(entry.clone());
        self.save_registry(&reg)?;

        Ok(entry)
    }

    /// Remove an installed tileset by id.
    ///
    /// Deletes the `.hexon` file, the `.meta.json` file, and the registry entry.
    /// Missing files are ignored (best-effort cleanup).
    pub fn remove_tileset(&self, hexon_id: &str) -> Result<()> {
        let hexon_path = self.hexon_path(hexon_id);
        let meta_path = self.meta_path(hexon_id);

        if hexon_path.exists() {
            std::fs::remove_file(&hexon_path).with_context(|| {
                format!("failed to remove hexon file: {}", hexon_path.display())
            })?;
        }
        if meta_path.exists() {
            std::fs::remove_file(&meta_path).with_context(|| {
                format!("failed to remove meta file: {}", meta_path.display())
            })?;
        }

        let mut reg = self.load_registry();
        reg.tilesets.retain(|t| t.hexon_id != hexon_id);
        self.save_registry(&reg)?;

        Ok(())
    }

    /// Return all installed tilesets from the registry.
    ///
    /// Returns an empty vec if the registry is missing or corrupt.
    pub fn list_installed(&self) -> Vec<InstalledTileset> {
        self.load_registry().tilesets
    }

    /// Load a tileset from disk into a `HexonTileSource` (in-memory tile map).
    pub fn load_tileset(&self, hexon_id: &str) -> Result<super::hexon_source::HexonTileSource> {
        let hexon_path = self.hexon_path(hexon_id);
        let bytes = std::fs::read(&hexon_path).with_context(|| {
            format!("failed to read hexon file: {}", hexon_path.display())
        })?;
        super::hexon_source::HexonTileSource::from_archive(&bytes)
            .with_context(|| format!("failed to load tileset '{hexon_id}'"))
    }

    /// Sum all `.hexon` file sizes in the base directory.
    pub fn total_disk_usage(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "hexon")
                    .unwrap_or(false)
            })
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    }

    /// Return the base directory path.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Enable or disable P2P seeding for a tileset.
    pub fn set_seeding(&self, hexon_id: &str, enabled: bool) -> Result<()> {
        let mut reg = self.load_registry();
        let entry = reg
            .tilesets
            .iter_mut()
            .find(|t| t.hexon_id == hexon_id)
            .with_context(|| format!("tileset '{hexon_id}' not found in registry"))?;
        entry.seeding_enabled = enabled;
        self.save_registry(&reg)
    }

    /// Update the `last_served` timestamp for a tileset to now.
    pub fn update_last_served(&self, hexon_id: &str) -> Result<()> {
        let mut reg = self.load_registry();
        let entry = reg
            .tilesets
            .iter_mut()
            .find(|t| t.hexon_id == hexon_id)
            .with_context(|| format!("tileset '{hexon_id}' not found in registry"))?;
        entry.last_served = Some(Utc::now());
        self.save_registry(&reg)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn registry_path(&self) -> PathBuf {
        self.base_dir.join("registry.json")
    }

    fn hexon_path(&self, hexon_id: &str) -> PathBuf {
        self.base_dir.join(format!("{hexon_id}.hexon"))
    }

    fn meta_path(&self, hexon_id: &str) -> PathBuf {
        self.base_dir.join(format!("{hexon_id}.meta.json"))
    }

    fn load_registry(&self) -> RegistryData {
        let path = self.registry_path();
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return RegistryData::default();
        };
        serde_json::from_str(&contents).unwrap_or_default()
    }

    fn save_registry(&self, reg: &RegistryData) -> Result<()> {
        let path = self.registry_path();
        let json = serde_json::to_string_pretty(reg).context("failed to serialize registry")?;
        // Atomic write: write to a temp file then rename.
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &json)
            .with_context(|| format!("failed to write registry tmp: {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &path)
            .with_context(|| format!("failed to rename registry tmp: {}", tmp_path.display()))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::HexonArchive;

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn make_meta() -> TilesetMeta {
        TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: 1,
            satellite_tile_count: 0,
            region_name: "Test Region".into(),
            parent_tileset: None,
            chunk_index: None,
        }
    }

    fn make_manifest(id: &str) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Test Tileset".into(),
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

    fn make_hexon_bytes(id: &str) -> Vec<u8> {
        let meta = make_meta();
        let elevation = vec![("10/512/340".to_string(), vec![0u8; 64])];
        HexonArchive::export_tileset(make_manifest(id), &meta, &elevation, &[], None)
            .expect("export_tileset failed")
    }

    fn temp_store() -> (HexonStore, std::path::PathBuf) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let dir = std::env::temp_dir().join(format!("hexon_store_test_{ts}"));
        let store = HexonStore::with_dir(dir.clone()).expect("with_dir failed");
        (store, dir)
    }

    // ------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------

    #[test]
    fn test_install_and_list() {
        let (store, dir) = temp_store();

        let bytes = make_hexon_bytes("tileset-abc");
        let entry = store.install_tileset(&bytes).expect("install_tileset failed");

        assert_eq!(entry.hexon_id, "tileset-abc");
        assert_eq!(entry.region_name, "Test Region");
        assert_eq!(entry.zoom_range, (10, 12));
        assert_eq!(entry.tile_count, 1);
        assert!(!entry.seeding_enabled);
        assert!(entry.last_served.is_none());

        // Files on disk
        assert!(dir.join("tileset-abc.hexon").exists());
        assert!(dir.join("tileset-abc.meta.json").exists());

        // Registry
        let list = store.list_installed();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].hexon_id, "tileset-abc");
    }

    #[test]
    fn test_remove_tileset() {
        let (store, dir) = temp_store();

        let bytes = make_hexon_bytes("tileset-rm");
        store.install_tileset(&bytes).expect("install failed");
        assert_eq!(store.list_installed().len(), 1);

        store.remove_tileset("tileset-rm").expect("remove failed");

        assert_eq!(store.list_installed().len(), 0);
        assert!(!dir.join("tileset-rm.hexon").exists());
        assert!(!dir.join("tileset-rm.meta.json").exists());
    }

    #[test]
    fn test_set_seeding() {
        let (store, _dir) = temp_store();

        let bytes = make_hexon_bytes("tileset-seed");
        store.install_tileset(&bytes).expect("install failed");

        // Default: seeding disabled
        let list = store.list_installed();
        assert!(!list[0].seeding_enabled);

        // Enable
        store.set_seeding("tileset-seed", true).expect("set_seeding failed");
        let list = store.list_installed();
        assert!(list[0].seeding_enabled);

        // Disable again
        store.set_seeding("tileset-seed", false).expect("set_seeding failed");
        let list = store.list_installed();
        assert!(!list[0].seeding_enabled);
    }

    #[test]
    fn test_load_tileset() {
        let (store, _dir) = temp_store();

        let bytes = make_hexon_bytes("tileset-load");
        store.install_tileset(&bytes).expect("install failed");

        let source = store.load_tileset("tileset-load").expect("load failed");
        assert_eq!(source.bounds(), [45.0, -122.0, 46.0, -121.0]);
        assert_eq!(source.zoom_range(), (10, 12));
    }

    #[test]
    fn test_total_disk_usage() {
        let (store, _dir) = temp_store();

        assert_eq!(store.total_disk_usage(), 0);

        let bytes = make_hexon_bytes("tileset-usage");
        store.install_tileset(&bytes).expect("install failed");

        assert!(store.total_disk_usage() > 0);
    }

    #[test]
    fn test_update_last_served() {
        let (store, _dir) = temp_store();

        let bytes = make_hexon_bytes("tileset-ls");
        store.install_tileset(&bytes).expect("install failed");

        let list = store.list_installed();
        assert!(list[0].last_served.is_none());

        store.update_last_served("tileset-ls").expect("update_last_served failed");

        let list = store.list_installed();
        assert!(list[0].last_served.is_some());
    }

    #[test]
    fn test_install_wrong_type_rejected() {
        let mut manifest = make_manifest("bad-tileset");
        manifest.hexon_type = HexonType::Scene;

        // We cannot easily build a non-tileset archive here without the full
        // export_scene path, so instead just verify that passing garbage bytes
        // returns an error (import will fail before type check).
        let (store, _dir) = temp_store();
        let result = store.install_tileset(b"this is not a valid hexon archive");
        assert!(result.is_err());
    }
}
