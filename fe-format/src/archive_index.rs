//! Lazy offset-indexed `.hexon` reader: header phase once, then single-member reads; see `src/AGENTS.md` §archive-index.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::entries::{AssetEntry, EntryCatalog};
use crate::integrity::{
    IntegrityPolicy, IntegrityStatus, SealCheck, CHUNK_INDEX_PATH, ENTRIES_PATH,
    HEADER_MEMBER_PATHS,
};
use crate::limits;
use crate::manifest::{HexonManifest, TilesetBundleIndex, TilesetMeta};
use crate::partial::{HeaderDoc, Layout, HEADER_DOC_VERSION};

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
///
/// Header-phase verification (PAS-P5 S2) runs once at [`open`](Self::open), under
/// a caller-chosen [`IntegrityPolicy`] (default [`IntegrityPolicy::VerifyIfSealed`]).
/// A sealed archive's subsequent [`read_member`](Self::read_member) calls are
/// then checked against the signed coverage; a legacy unsealed archive behaves
/// exactly as before (nothing is checked, per [`IntegrityPolicy::VerifyIfSealed`]).
pub struct HexonArchiveIndex {
    path: Arc<PathBuf>,
    /// Parsed central directory; its reader is closed (cloned per read, opened lazily).
    directory: zip::ZipArchive<LazyFile>,
    manifest: HexonManifest,
    entries: Vec<AssetEntry>,
    tileset_meta: Option<TilesetMeta>,
    stamp: FileStamp,
    /// Raw `manifest.json` bytes, for [`header_doc`](Self::header_doc) — digests
    /// are over exact stored bytes, so the parsed `manifest` is never re-serialized.
    manifest_raw: Vec<u8>,
    /// Raw bytes of every `HEADER_MEMBER_PATHS` member actually present.
    header_raw: BTreeMap<String, Vec<u8>>,
    /// Monolithic, or a container with its signed chunk count.
    layout: Layout,
    /// The header-phase verification outcome: what [`read_member`](Self::read_member)
    /// checks subsequent reads against.
    coverage: SealCheck,
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
    /// [`open_with_policy`](Self::open_with_policy) under [`IntegrityPolicy::VerifyIfSealed`]
    /// (the safe default: a sealed archive is fully enforced, a legacy unsealed
    /// one passes through exactly as before — see fe-format/src/AGENTS.md §integrity).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_policy(path, IntegrityPolicy::VerifyIfSealed)
    }

    /// Header phase: read the central directory plus every `HEADER_MEMBER_PATHS`
    /// member present (manifest, entries, license, schema, tileset_meta, terrain
    /// config, members.json, chunks/index.json), then verify under `policy`.
    /// `RequireSealed` fails `open` itself for an unsealed or tampered archive —
    /// nothing is read lazily before this check passes.
    pub fn open_with_policy(path: impl AsRef<Path>, policy: IntegrityPolicy) -> Result<Self> {
        let path = Arc::new(path.as_ref().to_path_buf());
        let stamp = FileStamp::of(&path)
            .with_context(|| format!("failed to stat hexon archive {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(LazyFile::new(Arc::clone(&path)))
            .with_context(|| format!("failed to open ZIP archive {}", path.display()))?;
        limits::check_entry_count(archive.len())?;

        let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
        let json = limits::MAX_JSON_ENTRY_BYTES;
        let manifest_raw = read_named(&mut archive, "manifest.json", json, &mut budget)?
            .context("missing manifest.json")?;
        let manifest: HexonManifest =
            serde_json::from_slice(&manifest_raw).context("invalid manifest.json")?;

        let mut header_raw: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for member_path in HEADER_MEMBER_PATHS {
            if let Some(buf) = read_named(&mut archive, member_path, json, &mut budget)? {
                header_raw.insert((*member_path).to_string(), buf);
            }
        }

        let entries: Vec<AssetEntry> = {
            let buf = header_raw
                .get(ENTRIES_PATH)
                .context("missing entries.json")?;
            let catalog: EntryCatalog =
                serde_json::from_slice(buf).context("invalid entries.json")?;
            catalog.entries
        };
        let tileset_meta: Option<TilesetMeta> = match header_raw.get("terrain/tileset_meta.json") {
            Some(buf) => {
                Some(serde_json::from_slice(buf).context("invalid terrain/tileset_meta.json")?)
            }
            None => None,
        };
        let coverage = crate::integrity::verify_sealed_header(&manifest_raw, &header_raw, &policy)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("header verification failed for {}", path.display()))?;
        // Sealed: every header member present must be committed (an injected,
        // uncommitted one is Uncovered — the coverage invariant this reader
        // shares with `verify_archive`), and the layout comes from the signed index.
        let layout = match &coverage {
            SealCheck::Sealed(sealed) => {
                for (member_path, bytes) in &header_raw {
                    sealed
                        .check_member(member_path, bytes)
                        .map_err(anyhow::Error::from)
                        .with_context(|| {
                            format!("header verification failed for {}", path.display())
                        })?;
                }
                sealed
                    .chunk_index()
                    .map_or(Layout::Monolithic, |index| Layout::Container {
                        chunk_count: index.bundles.len() as u32,
                    })
            }
            SealCheck::Unsealed => match header_raw.get(CHUNK_INDEX_PATH) {
                Some(buf) => {
                    let index: TilesetBundleIndex =
                        serde_json::from_slice(buf).context("invalid chunks/index.json")?;
                    Layout::Container {
                        chunk_count: index.bundles.len() as u32,
                    }
                }
                None => Layout::Monolithic,
            },
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
            manifest_raw,
            header_raw,
            layout,
            coverage,
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

    /// Whether this archive is a signed container, and if so how many chunks.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// What header-phase verification established (`Sealed{publisher_did}` or
    /// `Unsealed`) — the same outcome [`HexonArchive::import`] would report.
    pub fn integrity_status(&self) -> IntegrityStatus {
        match &self.coverage {
            SealCheck::Unsealed => IntegrityStatus::Unsealed,
            SealCheck::Sealed(coverage) => IntegrityStatus::Sealed {
                publisher_did: coverage.publisher_did().to_string(),
            },
        }
    }

    /// The header document both transports carry (PAS-P5 §3.2): raw manifest
    /// and header-member text, captured once at [`open`](Self::open). Uses a
    /// lossy UTF-8 conversion for API simplicity; every writer in this crate
    /// emits strict UTF-8 JSON, so this is a no-op in practice, and a hostile
    /// non-UTF-8 byte would only ever make a downstream hash check fail safe,
    /// never succeed on the wrong bytes.
    pub fn header_doc(&self) -> HeaderDoc {
        let manifest = String::from_utf8_lossy(&self.manifest_raw).into_owned();
        let header_members = self
            .header_raw
            .iter()
            .map(|(path, bytes)| (path.clone(), String::from_utf8_lossy(bytes).into_owned()))
            .collect();
        HeaderDoc {
            v: HEADER_DOC_VERSION,
            uri: format!("{}@{}", self.manifest.hexon_id, self.manifest.version),
            manifest,
            header_members,
            archive_size: self.stamp.len,
            layout: self.layout,
        }
    }

    /// Read container chunk `seq` (`chunks/{seq}.hexon`), verified against the
    /// signed chunk index when sealed; `Ok(None)` when absent (monolithic
    /// archive, or `seq` out of range).
    pub fn chunk(&self, seq: u32) -> Result<Option<Vec<u8>>> {
        self.read_member(&crate::integrity::chunk_member_path(seq))
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
    ///
    /// When the archive is sealed, the bytes are checked against the signed
    /// coverage (the committed digest, or the `asset_hash` self-certification
    /// for an `assets/{h}` member) before being returned — the ONE coverage
    /// rule every lazy reader applies, shared with `HexonArchive::import` and
    /// `verify_archive` via `SealedCoverage::check_member` (no second guard set).
    pub fn read_member_capped(&self, name: &str, cap: u64) -> Result<Option<Vec<u8>>> {
        if !self.contains(name) {
            return Ok(None);
        }
        if self.is_stale() {
            return Err(anyhow::Error::new(ArchiveChanged {
                path: self.path.as_ref().clone(),
            }));
        }
        // Header members were already read (and, if sealed, verified) at
        // `open`; return the cached bytes rather than decompressing again.
        if let Some(bytes) = self.header_raw.get(name) {
            return Ok(Some(bytes.clone()));
        }
        let cap = cap.min(limits::MAX_ENTRY_UNCOMPRESSED_BYTES);
        let mut budget = cap;
        let mut reader = self.directory.clone();
        let data = read_named(&mut reader, name, cap, &mut budget)
            .with_context(|| format!("reading {name} from {}", self.path.display()))?;
        if let (Some(bytes), SealCheck::Sealed(coverage)) = (&data, &self.coverage) {
            coverage
                .check_member(name, bytes)
                .map_err(anyhow::Error::from)
                .with_context(|| format!("verifying {name} from {}", self.path.display()))?;
        }
        Ok(data)
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
    use crate::integrity::{seal, IntegrityError};
    use crate::manifest::{ElevationEncoding, HexonType};
    use crate::{ChunkInput, HexonArchive};
    use ed25519_dalek::SigningKey;

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
            integrity: None,
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

    /// S2 acceptance: the header phase (`open` + `header_doc`) never reads tile
    /// bytes — true by construction (the doc only ever carries
    /// `HEADER_MEMBER_PATHS` text), checked here against a real multi-MiB tileset.
    #[test]
    fn header_phase_reads_no_tile_bytes() {
        let tiles: Vec<(String, Vec<u8>)> = (0..64)
            .map(|i| (format!("10/{i}/0"), vec![7u8; 64 * 1024]))
            .collect();
        let bytes = tileset("header-phase", &tiles);
        assert!(
            bytes.len() > 4 * 1024 * 1024,
            "fixture must be at least 4 MiB: {} bytes",
            bytes.len()
        );
        let path = temp_path("header_phase");
        std::fs::write(&path, &bytes).unwrap();

        let index = HexonArchiveIndex::open(&path).expect("open");
        let doc = index.header_doc();
        let doc_bytes =
            doc.manifest.len() + doc.header_members.values().map(String::len).sum::<usize>();
        assert!(
            doc_bytes < 64 * 1024,
            "header doc leaked tile bytes: {doc_bytes} bytes"
        );
        for member_path in doc.header_members.keys() {
            assert!(
                !member_path.starts_with("terrain/tiles/"),
                "{member_path} is a tile member, not a header member"
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// S2 acceptance: a sealed archive's lazy member reads are verified
    /// per-member — one tampered tile is refused, an untampered sibling
    /// still reads.
    #[test]
    fn lazy_member_read_refuses_only_the_tampered_member() {
        let key = SigningKey::from_bytes(&[5u8; 32]);
        let good = vec![1u8; 300];
        let bad = vec![2u8; 300];
        let tiles = vec![
            ("10/0/0".to_string(), good.clone()),
            ("10/0/1".to_string(), bad.clone()),
        ];
        let bytes = tileset("tamper", &tiles);
        let mut sealed = seal(&bytes, &key).expect("seal");

        // Flip one byte inside the second tile's (Stored, uncompressed) bytes.
        let pos = sealed
            .windows(bad.len())
            .position(|w| w == bad.as_slice())
            .expect("tile bytes must appear verbatim (Stored compression)");
        sealed[pos] ^= 0xFF;

        let path = temp_path("tamper");
        std::fs::write(&path, &sealed).unwrap();
        let index = HexonArchiveIndex::open(&path).expect("open");
        assert_eq!(
            index.integrity_status(),
            IntegrityStatus::Sealed {
                publisher_did: crate::verifying_key_to_did(&key.verifying_key())
            }
        );

        assert_eq!(
            index
                .read_member("terrain/tiles/10/0/0.png")
                .unwrap()
                .unwrap(),
            good,
            "the untampered tile must still read"
        );
        let err = index
            .read_member("terrain/tiles/10/0/1.png")
            .expect_err("tampered tile must be refused");
        assert!(
            matches!(
                err.downcast_ref::<IntegrityError>(),
                Some(IntegrityError::DigestMismatch { .. })
            ),
            "{err:#}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// S2 acceptance: a chunked container round-trips every chunk byte-for-byte
    /// and each chunk's signed hash is enforced on read.
    #[test]
    fn chunked_container_roundtrips_and_each_chunk_matches_its_signed_hash() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let mut chunks = Vec::new();
        for seq in 0..3u32 {
            let chunk_tiles = vec![(format!("10/{seq}/0"), vec![seq as u8; 128])];
            let chunk_bytes = tileset(&format!("chunk-{seq}"), &chunk_tiles);
            let sealed_chunk = seal(&chunk_bytes, &key).expect("seal chunk");
            chunks.push(ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [seq as u64, seq as u64],
                archive_bytes: sealed_chunk,
            });
        }

        let container_meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 3,
            region_name: "Container".into(),
            ..Default::default()
        };
        let unsealed =
            HexonArchive::export_chunked(manifest("container"), &container_meta, &chunks)
                .expect("export_chunked");
        let sealed = seal(&unsealed, &key).expect("seal container");

        let path = temp_path("container");
        std::fs::write(&path, &sealed).unwrap();
        let index = HexonArchiveIndex::open(&path).expect("open");
        assert_eq!(index.layout(), Layout::Container { chunk_count: 3 });
        let doc = index.header_doc();
        assert_eq!(doc.layout, Layout::Container { chunk_count: 3 });

        for chunk in &chunks {
            let read = index
                .chunk(chunk.seq)
                .expect("chunk read must not error")
                .expect("chunk must be present");
            assert_eq!(
                &read, &chunk.archive_bytes,
                "chunk {} must roundtrip byte-for-byte",
                chunk.seq
            );
        }
        assert!(
            index.chunk(99).unwrap().is_none(),
            "an out-of-range seq is just absent"
        );

        // Tamper with chunk 1's distinctive tile bytes (all 0x01) directly in
        // the outer container; the signed index still commits to the
        // original hash, so only chunk 1's read is refused.
        let marker = vec![1u8; 32];
        let pos = sealed
            .windows(marker.len())
            .position(|w| w == marker.as_slice())
            .expect("chunk 1 tile bytes present");
        let mut tampered = sealed.clone();
        tampered[pos] ^= 0xFF;
        let tampered_path = temp_path("container_tampered");
        std::fs::write(&tampered_path, &tampered).unwrap();
        let tampered_index = HexonArchiveIndex::open(&tampered_path).expect("open tampered");
        let err = tampered_index
            .chunk(1)
            .expect_err("tampered chunk must be refused");
        assert!(
            matches!(
                err.downcast_ref::<IntegrityError>(),
                Some(IntegrityError::DigestMismatch { .. })
            ),
            "{err:#}"
        );
        assert!(
            tampered_index.chunk(0).unwrap().is_some(),
            "chunk 0 is unaffected"
        );
        assert!(
            tampered_index.chunk(2).unwrap().is_some(),
            "chunk 2 is unaffected"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&tampered_path);
    }

    /// S8: a header member injected into a sealed archive without being
    /// committed by its signed root (here a forged `chunks/index.json` that
    /// would turn a monolithic archive into a "container") is refused at
    /// `open`, exactly as `verify_archive` refuses it — the lazy reader never
    /// serves or parses an uncommitted header member.
    #[test]
    fn an_uncommitted_header_member_in_a_sealed_archive_is_refused_at_open() {
        use std::io::Write;
        let key = SigningKey::from_bytes(&[6u8; 32]);
        let sealed = seal(
            &tileset("inject", &[("10/0/0".into(), vec![3u8; 64])]),
            &key,
        )
        .expect("seal");

        let forged_index = serde_json::to_vec(&TilesetBundleIndex {
            schema: 1,
            tileset_id: "inject".into(),
            tileset_meta: TilesetMeta::default(),
            bundles: vec![],
        })
        .unwrap();
        let mut source = zip::ZipArchive::new(std::io::Cursor::new(&sealed)).unwrap();
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..source.len() {
            out.raw_copy_file(source.by_index_raw(i).unwrap()).unwrap();
        }
        out.start_file(CHUNK_INDEX_PATH, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&forged_index).unwrap();
        let injected = out.finish().unwrap().into_inner();

        let path = temp_path("inject");
        std::fs::write(&path, &injected).unwrap();
        let err = HexonArchiveIndex::open(&path).expect_err("uncommitted header member");
        assert_eq!(
            err.downcast_ref::<IntegrityError>(),
            Some(&IntegrityError::Uncovered {
                path: CHUNK_INDEX_PATH.into()
            }),
            "{err:#}"
        );
        assert!(
            crate::integrity::verify_archive(&injected, &IntegrityPolicy::VerifyIfSealed).is_err()
        );

        std::fs::write(&path, &sealed).unwrap();
        let honest = HexonArchiveIndex::open(&path).expect("the untouched archive opens");
        assert_eq!(honest.layout(), Layout::Monolithic);
        let _ = std::fs::remove_file(&path);
    }
}
