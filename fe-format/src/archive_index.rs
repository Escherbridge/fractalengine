//! Lazy offset-indexed `.hexon` reader: header phase once, then single-member reads; see `src/AGENTS.md` §archive-index.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::entries::{AssetEntry, EntryCatalog};
use crate::limits;
use crate::manifest::{HexonManifest, TilesetMeta};

/// Error text when an indexed archive file changed on disk after its header phase.
pub const ARCHIVE_CHANGED_ERROR: &str = "hexon archive changed on disk since it was indexed";

/// Typed form of [`ARCHIVE_CHANGED_ERROR`]; callers `downcast_ref` it to re-open the index.
#[derive(Debug, Clone)]
pub struct ArchiveChanged {
    pub path: PathBuf,
}

impl std::fmt::Display for ArchiveChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{ARCHIVE_CHANGED_ERROR}: {}", self.path.display())
    }
}

impl std::error::Error for ArchiveChanged {}

/// Size + mtime of the archive file, captured at index time to detect replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl FileStamp {
    fn of(path: &Path) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// `Read + Seek` over a path that opens its handle on first use; clones start closed.
struct LazyFile {
    path: Arc<PathBuf>,
    file: Option<File>,
}

impl LazyFile {
    fn new(path: Arc<PathBuf>) -> Self {
        Self { path, file: None }
    }

    fn handle(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            self.file = Some(File::open(self.path.as_ref())?);
        }
        Ok(self.file.as_mut().expect("handle just opened"))
    }
}

impl Clone for LazyFile {
    fn clone(&self) -> Self {
        // A clone never shares or inherits a handle: zip always seeks before reading.
        Self::new(Arc::clone(&self.path))
    }
}

impl Read for LazyFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.handle()?.read(buf)
    }
}

impl Seek for LazyFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.handle()?.seek(pos)
    }
}

/// A `.hexon` archive on disk whose central directory and header JSON were read once;
/// members are read one at a time, on demand, under the `limits` caps.
pub struct HexonArchiveIndex {
    path: Arc<PathBuf>,
    /// Parsed central directory; its reader is closed (cloned per read, opened lazily).
    directory: zip::ZipArchive<LazyFile>,
    manifest: HexonManifest,
    entries: Vec<AssetEntry>,
    tileset_meta: Option<TilesetMeta>,
    stamp: FileStamp,
}

impl std::fmt::Debug for HexonArchiveIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HexonArchiveIndex")
            .field("path", &self.path)
            .field("members", &self.directory.len())
            .field("hexon_id", &self.manifest.hexon_id)
            .finish()
    }
}

impl HexonArchiveIndex {
    /// Header phase: read the central directory plus `manifest.json`, `entries.json`
    /// and (optional) `terrain/tileset_meta.json`; no other member is touched.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = Arc::new(path.as_ref().to_path_buf());
        let stamp = FileStamp::of(&path)
            .with_context(|| format!("failed to stat hexon archive {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(LazyFile::new(Arc::clone(&path)))
            .with_context(|| format!("failed to open ZIP archive {}", path.display()))?;
        limits::check_entry_count(archive.len())?;

        let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
        let json = limits::MAX_JSON_ENTRY_BYTES;
        let manifest: HexonManifest = {
            let buf = read_named(&mut archive, "manifest.json", json, &mut budget)?
                .context("missing manifest.json")?;
            serde_json::from_slice(&buf).context("invalid manifest.json")?
        };
        let entries: Vec<AssetEntry> = {
            let buf = read_named(&mut archive, "entries.json", json, &mut budget)?
                .context("missing entries.json")?;
            let catalog: EntryCatalog =
                serde_json::from_slice(&buf).context("invalid entries.json")?;
            catalog.entries
        };
        let tileset_meta: Option<TilesetMeta> =
            match read_named(&mut archive, "terrain/tileset_meta.json", json, &mut budget)? {
                Some(buf) => Some(
                    serde_json::from_slice(&buf).context("invalid terrain/tileset_meta.json")?,
                ),
                None => None,
            };

        // Keep only a closed clone: the header-phase handle is dropped with `archive`.
        let directory = archive.clone();
        drop(archive);

        Ok(Self {
            path,
            directory,
            manifest,
            entries,
            tileset_meta,
            stamp,
        })
    }

    /// The archive file this index reads from.
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Parsed `manifest.json`.
    pub fn manifest(&self) -> &HexonManifest {
        &self.manifest
    }

    /// Parsed `entries.json` catalog.
    pub fn entries(&self) -> &[AssetEntry] {
        &self.entries
    }

    /// Parsed `terrain/tileset_meta.json`, when the archive is a tileset.
    pub fn tileset_meta(&self) -> Option<&TilesetMeta> {
        self.tileset_meta.as_ref()
    }

    /// Number of members in the central directory.
    pub fn len(&self) -> usize {
        self.directory.len()
    }

    /// True when the archive lists no members.
    pub fn is_empty(&self) -> bool {
        self.directory.is_empty()
    }

    /// Whether `name` is a member (central-directory lookup, no I/O).
    pub fn contains(&self, name: &str) -> bool {
        self.directory.index_for_name(name).is_some()
    }

    /// Every member name, in central-directory order (no I/O).
    pub fn member_names(&self) -> impl Iterator<Item = &str> {
        self.directory.file_names()
    }

    /// True when the file on disk no longer matches the one indexed (size or mtime).
    pub fn is_stale(&self) -> bool {
        !FileStamp::of(&self.path).is_ok_and(|now| now == self.stamp)
    }

    /// Read one member (CRC-checked, inflated under the per-entry cap); `Ok(None)` when absent.
    ///
    /// Fails with [`ArchiveChanged`] when the file was replaced since [`open`](Self::open).
    pub fn read_member(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let cap = if name.ends_with(".json") {
            limits::MAX_JSON_ENTRY_BYTES
        } else {
            limits::MAX_ENTRY_UNCOMPRESSED_BYTES
        };
        self.read_member_capped(name, cap)
    }

    /// [`read_member`](Self::read_member) under an explicit cap (never above the binary cap).
    pub fn read_member_capped(&self, name: &str, cap: u64) -> Result<Option<Vec<u8>>> {
        if !self.contains(name) {
            return Ok(None);
        }
        if self.is_stale() {
            return Err(anyhow::Error::new(ArchiveChanged {
                path: self.path.as_ref().clone(),
            }));
        }
        let cap = cap.min(limits::MAX_ENTRY_UNCOMPRESSED_BYTES);
        let mut budget = cap;
        let mut reader = self.directory.clone();
        read_named(&mut reader, name, cap, &mut budget)
            .with_context(|| format!("reading {name} from {}", self.path.display()))
    }
}

/// Read `name` from `archive` under `cap` and the shared `budget`; `Ok(None)` when absent.
fn read_named<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
    budget: &mut u64,
) -> Result<Option<Vec<u8>>> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("failed to open entry {name}")),
    };
    let declared = file.size();
    limits::read_entry_capped(&mut file, declared, cap, budget).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ElevationEncoding, HexonType};
    use crate::HexonArchive;

    fn manifest(id: &str) -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mkindex".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Index".into(),
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

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "fe_archive_index_{tag}_{}_{nanos}.hexon",
            std::process::id()
        ))
    }

    fn tileset(id: &str, tiles: &[(String, Vec<u8>)]) -> Vec<u8> {
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: tiles.len() as u32,
            region_name: "Index".into(),
            ..Default::default()
        };
        HexonArchive::export_tileset(manifest(id), &meta, tiles, &[], None).unwrap()
    }

    #[test]
    fn header_phase_then_member_reads_match_the_full_import() {
        let tiles = vec![
            ("10/1/2".to_string(), vec![1u8; 300]),
            (
                "11/3/4".to_string(),
                (0..=255u8).cycle().take(5000).collect(),
            ),
        ];
        let bytes = tileset("idx", &tiles);
        let path = temp_path("match");
        std::fs::write(&path, &bytes).unwrap();

        let index = HexonArchiveIndex::open(&path).expect("open");
        let full = HexonArchive::import(&bytes).expect("import");
        assert_eq!(index.manifest().hexon_id, "idx");
        assert_eq!(index.tileset_meta().unwrap().tile_count, 2);
        for (key, data) in &full.elevation_tiles {
            let name = format!("terrain/tiles/{key}.png");
            assert!(index.contains(&name));
            assert_eq!(&index.read_member(&name).unwrap().unwrap(), data, "{name}");
        }
        assert!(index
            .read_member("terrain/tiles/9/9/9.png")
            .unwrap()
            .is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_replaced_file_is_reported_as_changed_not_misread() {
        let path = temp_path("changed");
        std::fs::write(&path, tileset("v1", &[("10/1/2".into(), vec![1u8; 64])])).unwrap();
        let index = HexonArchiveIndex::open(&path).expect("open");

        std::fs::write(&path, tileset("v2", &[("10/1/2".into(), vec![2u8; 4096])])).unwrap();
        let err = index
            .read_member("terrain/tiles/10/1/2.png")
            .expect_err("stale index must refuse");
        assert!(err.downcast_ref::<ArchiveChanged>().is_some(), "{err:#}");
        let _ = std::fs::remove_file(&path);
    }
}
