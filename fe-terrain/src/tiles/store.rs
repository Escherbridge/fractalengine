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

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use fe_format::manifest::{HexonType, TilesetMeta};
use fe_format::{HexonArchive, HexonArchiveIndex, IntegrityStatus};
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
    /// Whether the archive's header verified under a publisher signature
    /// (`fe_format::IntegrityStatus::Sealed`) at install time — `false` for a
    /// legacy unsealed archive (nothing was checked) AND for a tampered one
    /// (which `install_tileset` already refused before this field is ever
    /// written). `#[serde(default)]` so a pre-PAS-P5 `registry.json` entry
    /// deserializes as unverified, never a hard failure (p2p_asset_streaming
    /// S6; see `src/AGENTS.md` §chunked-containers).
    #[serde(default)]
    pub verified: bool,
}

/// Error text `install_tileset` returns when `hexon_id` could escape the
/// store directory (path-traversal guard; campaign plan S1 step 6, critic C1).
pub const UNSAFE_HEXON_ID_ERROR: &str = "unsafe hexon_id (path traversal guard)";

/// Error text `install_tileset` returns when it would replace an installed
/// SEALED tileset with another publisher's archive or an unsealed one (the
/// store-side publisher pin; see `src/AGENTS.md` §chunked-containers "publisher continuity").
pub const PUBLISHER_CONTINUITY_ERROR: &str =
    "installed tileset is sealed by a different publisher (publisher continuity)";

/// Chars allowed in a `hexon_id` used as an on-disk filename component
/// (mirrors the registry's charset, `fe-hexon-registry/src/routes.rs:283`),
/// plus an explicit `.`/`..` refusal the charset alone doesn't catch (both
/// consist only of allowed characters but name the current/parent directory).
///
/// `install_tileset` is the one place every install path goes through — a
/// fresh download, an `assemble_chunks`/`assemble_bundles` P2P install, or a
/// direct call — so it is the single choke point for this guard. S4 makes
/// P2P-assembled installs (where `hexon_id` is the peer-advertised
/// `tileset_id`, untrusted) reachable before PAS-P5's signing lands.
///
/// S8b: delegates to the one shared guard (`fe_format::is_safe_path_id`),
/// which also refuses Windows device stems (`CON`, `NUL.x`) and over-long ids.
pub fn is_safe_hexon_id(id: &str) -> bool {
    fe_format::is_safe_path_id(id)
}

/// Backfill missing `native_scale`/`ground_sample_distance_m` on a loaded
/// [`TilesetMeta`] via [`fe_format::manifest::derive_scale_from_bounds`]
/// (FR-3). Idempotent: already-populated fields are untouched.
pub fn backfill_scale_fields(meta: &mut TilesetMeta) {
    if meta.native_scale.is_some() && meta.ground_sample_distance_m.is_some() {
        return;
    }
    let (gsd, native_scale) =
        fe_format::manifest::derive_scale_from_bounds(meta.bounds, meta.max_zoom, meta.tile_size);
    meta.ground_sample_distance_m.get_or_insert(gsd);
    meta.native_scale.get_or_insert(native_scale);
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
    /// W2b F3: serializes every store mutation — `install_tileset`'s
    /// continuity check → synced write → both renames → registry update, and
    /// each `registry.json` read-modify-write (`remove_tileset`,
    /// `set_seeding`, `update_last_served`). See `src/AGENTS.md`
    /// §chunked-containers "atomic install".
    write_lock: std::sync::Mutex<()>,
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
        Ok(Self {
            base_dir: dir,
            write_lock: std::sync::Mutex::new(()),
        })
    }

    // -----------------------------------------------------------------------
    // Core operations
    // -----------------------------------------------------------------------

    /// Install a tileset from raw `.hexon` archive bytes.
    ///
    /// Steps:
    /// 1. Import and validate the archive (`HexonArchive::import` runs the
    ///    full integrity pass under `VerifyIfSealed` — a sealed archive is
    ///    fully enforced; a tampered one is refused before anything here
    ///    runs; a legacy unsealed one passes through as before).
    /// 2. Verify the hexon type is `TerrainTileset` (monolithic or a signed
    ///    chunked container — both carry `tileset_meta`, p2p_asset_streaming S6).
    /// 3. Refuse an unsafe `hexon_id` before any path join (`is_safe_hexon_id`).
    /// 4. Write `{hexon_id}.hexon` and `{hexon_id}.meta.json` ATOMICALLY
    ///    (temp file in the same directory, then rename over any existing
    ///    file) — see `src/AGENTS.md` §chunked-containers "atomic install".
    /// 5. Append an entry to `registry.json`, recording whether the header
    ///    verified under a publisher signature (`verified`).
    ///
    /// Refuses ([`PUBLISHER_CONTINUITY_ERROR`]) to replace an installed sealed
    /// tileset with another publisher's archive or an unsealed one — every
    /// install path (P2P, registry, local) passes this one pin.
    ///
    /// W2b F3: `write_lock` is held from the continuity check below
    /// through both renames and the registry update, so two installs of the
    /// SAME `hexon_id` racing on different threads (local, P2P, registry)
    /// are fully serialized, never interleaved.
    pub fn install_tileset(&self, bytes: &[u8]) -> Result<InstalledTileset> {
        // Import and validate
        let data = HexonArchive::import(bytes).context("failed to import hexon archive")?;

        let manifest = &data.manifest;

        match manifest.hexon_type {
            HexonType::TerrainTileset => {}
            ref other => bail!("expected TerrainTileset hexon, got {:?}", other),
        }

        let hexon_id = manifest.hexon_id.clone();
        if !is_safe_hexon_id(&hexon_id) {
            bail!("{UNSAFE_HEXON_ID_ERROR}: {hexon_id:?}");
        }

        let _write_guard = self.lock_writes();

        if let Some(installed) = self.installed_publisher(&hexon_id) {
            let incoming = match &data.integrity {
                IntegrityStatus::Sealed { publisher_did } => Some(publisher_did.as_str()),
                IntegrityStatus::Unsealed => None,
            };
            if incoming != Some(installed.as_str()) {
                bail!(
                    "{PUBLISHER_CONTINUITY_ERROR}: {hexon_id:?} is sealed by {installed}, not {}",
                    incoming.unwrap_or("an unsealed archive")
                );
            }
        }

        let tileset_meta: TilesetMeta = data
            .tileset_meta
            .context("archive is missing tileset_meta (TerrainTileset must include it)")?;
        let verified = matches!(data.integrity, IntegrityStatus::Sealed { .. });

        // Persist raw bytes atomically: write + fsync a temp file in the
        // same directory (same filesystem, so the rename is atomic) then
        // rename over any existing `{hexon_id}.hexon`. The `sync_all` (W2b
        // F3) makes the bytes durable BEFORE the rename makes them visible
        // under the final name — a crash between write and rename leaves
        // only an orphaned temp file, never a final-named file with
        // unflushed content. A lazy reader's open `File` handle
        // (`fe_format::HexonArchiveIndex`'s internal `LazyFile`) keeps
        // reading the pre-rename bytes even on Windows (the stdlib opens
        // files with FILE_SHARE_DELETE); its own `is_stale()` check then
        // reports `ArchiveChanged` on its NEXT read, same as a non-atomic
        // rewrite did, so correctness is unchanged — only the WRITE itself
        // stops racing a concurrent reader (critic AC-21b/c).
        let hexon_path = self.hexon_path(&hexon_id);
        let tmp_hexon_path = self.temp_path(&hexon_id, "hexon");
        write_synced_and_rename(&tmp_hexon_path, &hexon_path, bytes)?;

        // Persist extracted meta (same atomic + synced pattern; no lazy
        // reader holds this file open today, but there is no reason to
        // special-case it).
        let meta_json = serde_json::to_string_pretty(&tileset_meta)
            .context("failed to serialize TilesetMeta")?;
        let meta_path = self.meta_path(&hexon_id);
        let tmp_meta_path = self.temp_path(&hexon_id, "meta.json");
        write_synced_and_rename(&tmp_meta_path, &meta_path, meta_json.as_bytes())?;

        // W2b F3: fsync the store directory itself so the renames' directory-
        // entry updates are durable too, not just each file's own fsync
        // (POSIX requires this separately; Windows has no equivalent — NTFS's
        // own metadata journal already covers it, and opening a directory as
        // a `File` is unsupported there anyway). No dedicated test covers
        // this call: a missing directory fsync has no externally observable
        // effect short of an actual power-loss event, which nothing in this
        // test suite can simulate.
        self.fsync_dir();

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
            verified,
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
        if !is_safe_hexon_id(hexon_id) {
            bail!("{UNSAFE_HEXON_ID_ERROR}: {hexon_id:?}");
        }
        let hexon_path = self.hexon_path(hexon_id);
        let meta_path = self.meta_path(hexon_id);
        let _write_guard = self.lock_writes();

        if hexon_path.exists() {
            std::fs::remove_file(&hexon_path).with_context(|| {
                format!("failed to remove hexon file: {}", hexon_path.display())
            })?;
        }
        if meta_path.exists() {
            std::fs::remove_file(&meta_path)
                .with_context(|| format!("failed to remove meta file: {}", meta_path.display()))?;
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

    /// Open an installed tileset as a lazy `HexonTileSource` (header phase only; tiles on demand).
    ///
    /// Backfills missing scale fields (FR-3) via [`backfill_scale_fields`];
    /// idempotent — already-populated fields are left untouched.
    pub fn load_tileset(&self, hexon_id: &str) -> Result<super::hexon_source::HexonTileSource> {
        if !is_safe_hexon_id(hexon_id) {
            bail!("{UNSAFE_HEXON_ID_ERROR}: {hexon_id:?}");
        }
        let hexon_path = self.hexon_path(hexon_id);
        let mut source = super::hexon_source::HexonTileSource::open(&hexon_path)
            .with_context(|| format!("failed to load tileset '{hexon_id}'"))?;
        backfill_scale_fields(&mut source.tileset_meta);
        Ok(source)
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
        let _write_guard = self.lock_writes();
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
        let _write_guard = self.lock_writes();
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

    /// The DID whose signature covers the installed `{hexon_id}.hexon`; `None`
    /// when nothing is installed, the install is unsealed, or it fails its
    /// own header check (a broken install pins nothing and may be replaced).
    pub fn installed_publisher(&self, hexon_id: &str) -> Option<String> {
        if !is_safe_hexon_id(hexon_id) {
            return None;
        }
        let path = self.hexon_path(hexon_id);
        if !path.is_file() {
            return None;
        }
        match HexonArchiveIndex::open(&path).ok()?.integrity_status() {
            IntegrityStatus::Sealed { publisher_did } => Some(publisher_did),
            IntegrityStatus::Unsealed => None,
        }
    }

    /// A same-directory temp path for an atomic write-then-rename of
    /// `{hexon_id}.{suffix}` (`"hexon"` or `"meta.json"`). Unique per call
    /// (pid + nanos) so two concurrent installs of the SAME id never write
    /// the same temp file.
    fn temp_path(&self, hexon_id: &str, suffix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        self.base_dir.join(format!(
            "{hexon_id}.{suffix}.tmp-{}-{nanos}",
            std::process::id()
        ))
    }

    /// Take [`Self::write_lock`] (a poisoned lock is still usable: the
    /// guarded state is the on-disk store, not the `()` inside).
    fn lock_writes(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    /// W2b F3: fsync the store directory so a rename's directory-entry
    /// update is durable, not just the renamed file's own content (POSIX
    /// requires this separately from a file's `sync_all`). A no-op on
    /// Windows, where opening a directory as a `File` for this purpose is
    /// unsupported and NTFS's own metadata journal makes it unnecessary.
    #[cfg(unix)]
    fn fsync_dir(&self) {
        if let Ok(dir) = std::fs::File::open(&self.base_dir) {
            let _ = dir.sync_all();
        }
    }

    #[cfg(not(unix))]
    fn fsync_dir(&self) {}
}

/// Write `bytes` to `tmp_path`, fsync the file (W2b F3 — durable on disk
/// BEFORE the rename makes it visible under `final_path`), then rename over
/// any existing file at `final_path` (same directory as its temp path, so
/// the rename itself is atomic on both POSIX and Windows).
fn write_synced_and_rename(tmp_path: &Path, final_path: &Path, bytes: &[u8]) -> Result<()> {
    {
        let mut file = std::fs::File::create(tmp_path)
            .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to fsync temp file: {}", tmp_path.display()))?;
        // `file` closes here, before the rename — required on Windows,
        // where an open handle can block a rename over the destination.
    }
    std::fs::rename(tmp_path, final_path).with_context(|| {
        format!(
            "failed to install file atomically: {}",
            final_path.display()
        )
    })?;
    Ok(())
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
            tile_count: 1,
            region_name: "Test Region".into(),
            crs: None, // deliberately legacy — differs from Default's Some("EPSG:4326")
            ..Default::default()
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
            integrity: None,
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
        let entry = store
            .install_tileset(&bytes)
            .expect("install_tileset failed");

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

    /// S8b: a peer-declared id naming a Windows device (`CON.hexon` opens the
    /// console, not a file) is refused at install, and `remove_tileset` never
    /// joins a traversing id into a path — a sibling file outside the store survives.
    #[test]
    fn device_and_traversing_ids_never_reach_the_filesystem() {
        let (store, dir) = temp_store();
        for id in ["CON", "nul.part", "lpt1"] {
            let err = store
                .install_tileset(&make_hexon_bytes(id))
                .expect_err("device id refused");
            assert!(err.to_string().contains(UNSAFE_HEXON_ID_ERROR), "{err:#}");
        }
        assert!(store.list_installed().is_empty());

        let outside = dir.with_extension("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.hexon");
        std::fs::write(&victim, b"keep me").unwrap();
        let traversing = format!(
            "../{}/victim",
            outside.file_name().unwrap().to_string_lossy()
        );
        let err = store
            .remove_tileset(&traversing)
            .expect_err("traversing id refused");
        assert!(err.to_string().contains(UNSAFE_HEXON_ID_ERROR), "{err:#}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        let _ = std::fs::remove_dir_all(&outside);
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
        store
            .set_seeding("tileset-seed", true)
            .expect("set_seeding failed");
        let list = store.list_installed();
        assert!(list[0].seeding_enabled);

        // Disable again
        store
            .set_seeding("tileset-seed", false)
            .expect("set_seeding failed");
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

        store
            .update_last_served("tileset-ls")
            .expect("update_last_served failed");

        let list = store.list_installed();
        assert!(list[0].last_served.is_some());
    }

    #[test]
    fn test_load_tileset_backfills_missing_scale_fields() {
        let (store, _dir) = temp_store();
        let bytes = make_hexon_bytes("tileset-backfill");
        store.install_tileset(&bytes).expect("install failed");

        let source = store.load_tileset("tileset-backfill").expect("load failed");
        assert!(source.tileset_meta.native_scale.is_some());
        assert!(source.tileset_meta.ground_sample_distance_m.is_some());
        assert_ne!(source.tileset_meta.native_scale, Some(1.0));
    }

    #[test]
    fn test_backfill_is_idempotent_on_reload() {
        let (store, _dir) = temp_store();
        let bytes = make_hexon_bytes("tileset-idempotent");
        store.install_tileset(&bytes).expect("install failed");

        let first = store
            .load_tileset("tileset-idempotent")
            .expect("load failed");
        let first_scale = first.tileset_meta.native_scale;
        let first_gsd = first.tileset_meta.ground_sample_distance_m;

        let second = store
            .load_tileset("tileset-idempotent")
            .expect("reload failed");
        assert_eq!(second.tileset_meta.native_scale, first_scale);
        assert_eq!(second.tileset_meta.ground_sample_distance_m, first_gsd);
    }

    #[test]
    fn test_backfill_scale_fields_leaves_populated_fields_untouched() {
        let mut meta = make_meta();
        meta.native_scale = Some(0.5);
        meta.ground_sample_distance_m = Some(9.9);
        backfill_scale_fields(&mut meta);
        assert_eq!(meta.native_scale, Some(0.5));
        assert_eq!(meta.ground_sample_distance_m, Some(9.9));
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
