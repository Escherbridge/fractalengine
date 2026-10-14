//! `TilesetTransfer`: publish / fetch / resume tilesets as a HashSeq of verified
//! bundles over any `BlobTransport` — see `src/AGENTS.md` §tileset-transfer.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use fe_runtime::blob_store::{hash_from_hex, hash_to_hex};
use fe_runtime::blob_transport::{
    content_hash, decode_hash_seq, hash_seq_root, BlobTransport, Hash32, ProviderRef,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

/// Tag prefix protecting a published tileset's HashSeq root.
pub const TILESET_TAG_PREFIX: &str = "fe/tileset/";
/// Tag prefix protecting an in-flight download's HashSeq root.
pub const DOWNLOAD_TAG_PREFIX: &str = "fe/download/";
/// Download root directory name under the P2P data dir.
pub const DOWNLOADS_DIR: &str = "downloads";
/// Per-download persisted ledger file name.
pub const LEDGER_FILE: &str = "download.json";
/// Per-download index blob (HashSeq child 0) file name.
pub const INDEX_FILE: &str = "index.json";
/// Marker a full [`TilesetTransfer::download`] leaves; only marked downloads resume.
pub const FULL_DOWNLOAD_MARKER: &str = "full-download";
/// Ledger schema version.
pub const LEDGER_SCHEMA: u32 = 1;
/// Concurrent bundle fetches per download.
pub const MAX_CONCURRENT_CHUNK_FETCHES: usize = 4;
/// Longest accepted `tileset_id`.
pub const MAX_TILESET_ID_LEN: usize = fe_runtime::safe_id::MAX_SAFE_ID_LEN;
/// Largest accepted bundle blob (bundles default to 1 MiB).
pub const MAX_CHUNK_BYTES: usize = 64 * 1024 * 1024;
/// Largest accepted index blob.
pub const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;
/// Most children accepted in a tileset HashSeq (index + 65 536 bundles).
pub const MAX_HASH_SEQ_CHILDREN: usize = 1 + (1 << 16);
/// Largest total bundle bytes one download may store (an install caps at 1 GiB decoded).
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Largest accepted `download.json` ledger file.
pub const MAX_LEDGER_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Disk budget for all streamed-bundle caches together (LRU-evicted past it).
pub const MAX_STREAMED_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Per-download file recording when a streamed cache was last used (unix ms).
pub const LAST_USED_FILE: &str = "last-used";

/// Whether `id` is safe as a path component and tag suffix (the shared `safe_id` guard).
pub fn is_safe_tileset_id(id: &str) -> bool {
    fe_runtime::safe_id::is_safe_path_id(id)
}

/// Per-service size caps for peer-supplied blobs (see `src/AGENTS.md` §tileset-security).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferLimits {
    /// Largest accepted bundle blob.
    pub max_chunk_bytes: usize,
    /// Largest accepted index blob.
    pub max_index_bytes: usize,
    /// Largest total bundle bytes one download may store.
    pub max_download_bytes: u64,
    /// Disk budget shared by every streamed-bundle cache (see `src/AGENTS.md` §tileset-security).
    pub max_streamed_cache_bytes: u64,
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            max_chunk_bytes: MAX_CHUNK_BYTES,
            max_index_bytes: MAX_INDEX_BYTES,
            max_download_bytes: MAX_DOWNLOAD_BYTES,
            max_streamed_cache_bytes: MAX_STREAMED_CACHE_BYTES,
        }
    }
}

/// The publish tag name for `tileset_id`.
pub fn tileset_tag(tileset_id: &str) -> String {
    format!("{TILESET_TAG_PREFIX}{tileset_id}")
}

/// The download tag name for `tileset_id`.
pub fn download_tag(tileset_id: &str) -> String {
    format!("{DOWNLOAD_TAG_PREFIX}{tileset_id}")
}

/// Why one bundle could not be stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkError {
    /// The provider's bytes do not hash to the HashSeq child.
    HashMismatch {
        /// The HashSeq child hash.
        expected: Hash32,
        /// BLAKE3 of the bytes received.
        actual: Hash32,
    },
    /// The blob is over the service's bundle cap ([`TransferLimits::max_chunk_bytes`]).
    TooLarge {
        /// Bytes received.
        len: usize,
    },
    /// No provider could deliver the blob.
    Unavailable(String),
    /// Writing the verified bundle failed.
    Io(String),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HashMismatch { expected, actual } => write!(
                f,
                "hash mismatch: expected {}, got {}",
                hash_to_hex(expected),
                hash_to_hex(actual)
            ),
            Self::TooLarge { len } => write!(f, "bundle too large: {len} bytes"),
            Self::Unavailable(reason) => write!(f, "unavailable: {reason}"),
            Self::Io(reason) => write!(f, "io: {reason}"),
        }
    }
}

/// Typed failure of a [`TilesetTransfer`] operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferError {
    /// The id fails [`is_safe_tileset_id`]; nothing was touched.
    UnsafeTilesetId(String),
    /// No download ledger exists for the id.
    UnknownDownload(String),
    /// The request named no provider.
    NoProviders(String),
    /// The chunk sequence number is past the end of the download.
    SeqOutOfRange {
        /// Requested seq.
        seq: u32,
        /// Bundles in the download.
        total: u32,
    },
    /// The HashSeq root could not be resolved to a valid child list.
    BadRoot(String),
    /// The index blob could not be fetched or did not verify.
    BadIndex(String),
    /// One bundle failed.
    Chunk {
        /// Bundle seq.
        seq: u32,
        /// Why.
        error: ChunkError,
    },
    /// The transport's local store failed.
    Transport(String),
    /// Local filesystem failure.
    Io(String),
    /// The download stored more bundle bytes than [`TransferLimits::max_download_bytes`].
    DownloadTooLarge {
        /// Bundle bytes on disk when the cap tripped.
        bytes: u64,
        /// The cap.
        cap: u64,
    },
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeTilesetId(id) => write!(f, "unsafe tileset_id {id:?} (path guard)"),
            Self::UnknownDownload(id) => write!(f, "no download in progress for {id:?}"),
            Self::NoProviders(id) => write!(f, "no provider candidates for {id:?}"),
            Self::SeqOutOfRange { seq, total } => {
                write!(f, "chunk seq {seq} out of range (total {total})")
            }
            Self::BadRoot(reason) => write!(f, "bad HashSeq root: {reason}"),
            Self::BadIndex(reason) => write!(f, "bad index blob: {reason}"),
            Self::Chunk { seq, error } => write!(f, "chunk {seq}: {error}"),
            Self::Transport(reason) => write!(f, "transport: {reason}"),
            Self::Io(reason) => write!(f, "io: {reason}"),
            Self::DownloadTooLarge { bytes, cap } => {
                write!(f, "download stopped at {bytes} bytes (cap {cap})")
            }
        }
    }
}

impl std::error::Error for TransferError {}

/// One published tileset (what an advertisement is built from).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Tileset id.
    pub tileset_id: String,
    /// HashSeq root `[index, bundle_0 … bundle_n]`.
    pub root: Hash32,
    /// Bundle count (children minus the index).
    pub total_chunks: u32,
    /// Sum of bundle sizes.
    pub bytes: u64,
}

/// A download's resolved metadata (the index blob passes through opaque).
#[derive(Debug, Clone)]
pub struct MetaResolved {
    /// Tileset id.
    pub tileset_id: String,
    /// HashSeq root.
    pub root: Hash32,
    /// HashSeq child 0.
    pub index_hash: Hash32,
    /// HashSeq children 1..; `chunk_hashes[seq]` is bundle `seq`.
    pub chunk_hashes: Vec<Hash32>,
    /// Verified index blob bytes.
    pub index_json: Bytes,
    /// Where the verified index blob is on disk.
    pub index_path: PathBuf,
}

/// A verified bundle on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChunk {
    /// Bundle seq.
    pub seq: u32,
    /// `downloads/<id>/<seq>.bundle`.
    pub path: PathBuf,
    /// File size.
    pub byte_len: u64,
}

/// Every bundle of a download is on disk and verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedDownload {
    /// Tileset id.
    pub tileset_id: String,
    /// HashSeq root.
    pub root: Hash32,
    /// Verified index blob path.
    pub index_path: PathBuf,
    /// Bundle paths, indexed by seq.
    pub bundle_paths: Vec<PathBuf>,
}

/// Progress reported while a download runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    /// A bundle verified and landed on disk.
    ChunkStored {
        /// Tileset id.
        tileset_id: String,
        /// The stored bundle.
        chunk: StoredChunk,
    },
    /// A bundle failed on every provider.
    ChunkFailed {
        /// Tileset id.
        tileset_id: String,
        /// Bundle seq.
        seq: u32,
        /// Last provider's failure.
        error: ChunkError,
    },
    /// The download is complete.
    Complete(CompletedDownload),
}

/// Outcome of one [`TilesetTransfer::download`] run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadReport {
    /// Tileset id.
    pub tileset_id: String,
    /// Seqs stored by this run (already-verified seqs are not repeated).
    pub stored: Vec<u32>,
    /// Seqs that failed this run.
    pub failed: Vec<(u32, ChunkError)>,
    /// Set when every bundle is on disk and verified.
    pub complete: Option<CompletedDownload>,
}

/// A persisted download found at start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumedDownload {
    /// Tileset id.
    pub tileset_id: String,
    /// Bundles whose files verify.
    pub received: u32,
    /// Bundles in the download.
    pub total: u32,
    /// Whether the index blob file verifies.
    pub index_present: bool,
    /// Whether a full download was asked for (else it is a streamed-bundle cache).
    pub full_download: bool,
}

/// On-disk `download.json` (hashes hex-encoded).
#[derive(Debug, Serialize, Deserialize)]
struct LedgerFile {
    schema: u32,
    tileset_id: String,
    root: String,
    provider_candidates: Vec<String>,
    chunk_hashes: Vec<String>,
    index_hash: String,
}

/// Parsed download ledger. "Received" is never stored: it is derived from verified files.
#[derive(Debug, Clone)]
struct Ledger {
    tileset_id: String,
    root: Hash32,
    providers: Vec<ProviderRef>,
    chunk_hashes: Vec<Hash32>,
    index_hash: Hash32,
    dir: PathBuf,
}

impl Ledger {
    fn to_file(&self) -> LedgerFile {
        LedgerFile {
            schema: LEDGER_SCHEMA,
            tileset_id: self.tileset_id.clone(),
            root: hash_to_hex(&self.root),
            provider_candidates: self.providers.iter().map(|p| p.0.clone()).collect(),
            chunk_hashes: self.chunk_hashes.iter().map(hash_to_hex).collect(),
            index_hash: hash_to_hex(&self.index_hash),
        }
    }

    fn from_file(file: LedgerFile, dir: PathBuf) -> Result<Self, String> {
        if file.schema != LEDGER_SCHEMA {
            return Err(format!("unsupported ledger schema {}", file.schema));
        }
        if file.chunk_hashes.len() + 1 > MAX_HASH_SEQ_CHILDREN {
            return Err(format!(
                "ledger lists {} bundles (cap {})",
                file.chunk_hashes.len(),
                MAX_HASH_SEQ_CHILDREN - 1
            ));
        }
        let hex = |s: &str| hash_from_hex(s).map_err(|e| e.to_string());
        let chunk_hashes = file
            .chunk_hashes
            .iter()
            .map(|h| hex(h))
            .collect::<Result<Vec<_>, _>>()?;
        let root = hex(&file.root)?;
        let index_hash = hex(&file.index_hash)?;
        let mut children = Vec::with_capacity(chunk_hashes.len() + 1);
        children.push(index_hash);
        children.extend_from_slice(&chunk_hashes);
        if hash_seq_root(&children) != root {
            return Err("ledger children do not hash to its root".to_string());
        }
        Ok(Self {
            root,
            index_hash,
            providers: file
                .provider_candidates
                .into_iter()
                .map(ProviderRef)
                .collect(),
            tileset_id: file.tileset_id,
            chunk_hashes,
            dir,
        })
    }

    fn total(&self) -> u32 {
        self.chunk_hashes.len() as u32
    }

    fn chunk_path(&self, seq: u32) -> PathBuf {
        self.dir.join(format!("{seq}.bundle"))
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join(INDEX_FILE)
    }
}

/// Lock a mutex, recovering the data from a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run blocking filesystem work off the async runtime (NFR-3).
async fn blocking<T, F>(work: F) -> Result<T, TransferError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| TransferError::Io(format!("blocking task: {e}")))?
        .map_err(TransferError::Io)
}

/// Unique suffix for temp files so concurrent writers never collide.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `bytes` to `path` atomically (temp file + rename). Blocking.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp-{}-{n}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("renaming into {}: {e}", path.display())
    })
}

/// The bytes of `path` when it exists, fits `cap` and hashes to `expected`. Blocking.
fn read_verified(path: &Path, expected: &Hash32, cap: usize) -> Option<Vec<u8>> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() <= cap as u64 => {}
        _ => return None,
    }
    std::fs::read(path)
        .ok()
        .filter(|bytes| content_hash(bytes) == *expected)
}

/// Whether `path` exists, fits `cap` and hashes to `expected`. Blocking.
fn file_verifies(path: &Path, expected: &Hash32, cap: usize) -> bool {
    read_verified(path, expected, cap).is_some()
}

/// Per seq, the byte length of its bundle file when it verifies. Blocking.
fn derive_received(ledger: &Ledger, max_chunk_bytes: usize) -> Vec<Option<u64>> {
    (0..ledger.total())
        .map(|seq| {
            read_verified(
                &ledger.chunk_path(seq),
                &ledger.chunk_hashes[seq as usize],
                max_chunk_bytes,
            )
            .map(|bytes| bytes.len() as u64)
        })
        .collect()
}

/// Publishes, fetches, verifies and resumes tilesets over a [`BlobTransport`].
pub struct TilesetTransfer {
    transport: Arc<dyn BlobTransport>,
    download_root: PathBuf,
    limits: TransferLimits,
    catalog: Mutex<BTreeMap<String, CatalogEntry>>,
    ledgers: Mutex<HashMap<String, Arc<Ledger>>>,
    /// Ids with a meta resolution or bundle fetch running (never evicted).
    active: Arc<Mutex<HashMap<String, usize>>>,
}

/// Marks a download id busy for as long as it lives (cache eviction skips it).
struct ActiveGuard {
    active: Arc<Mutex<HashMap<String, usize>>>,
    tileset_id: String,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let mut active = lock(&self.active);
        if let Some(count) = active.get_mut(&self.tileset_id) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.tileset_id);
            }
        }
    }
}

/// One streamed cache found on disk while enforcing the budget.
struct CacheDir {
    tileset_id: String,
    bytes: u64,
    last_used_ms: u64,
}

/// Milliseconds since the unix epoch (the LRU clock).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Total size of the regular files directly under `dir`. Blocking.
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// When `dir` was last used: its `last-used` stamp, else its mtime. Blocking.
fn last_used_ms(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join(LAST_USED_FILE))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .or_else(|| {
            let modified = std::fs::metadata(dir).and_then(|m| m.modified()).ok()?;
            Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
        })
        .unwrap_or(0)
}

/// Every streamed cache (ledger present, no full-download marker) under `root`. Blocking.
fn streamed_caches(root: &Path) -> Vec<CacheDir> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let tileset_id = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            let is_cache = is_safe_tileset_id(&tileset_id)
                && dir.join(LEDGER_FILE).is_file()
                && !dir.join(FULL_DOWNLOAD_MARKER).exists();
            is_cache.then(|| CacheDir {
                bytes: dir_bytes(&dir),
                last_used_ms: last_used_ms(&dir),
                tileset_id,
            })
        })
        .collect()
}

/// The least-recently-used caches to drop so the rest fit `budget`, skipping `keep`.
fn caches_to_evict(
    mut caches: Vec<CacheDir>,
    budget: u64,
    keep: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut total: u64 = caches.iter().map(|c| c.bytes).sum();
    caches.sort_by(|a, b| (a.last_used_ms, &a.tileset_id).cmp(&(b.last_used_ms, &b.tileset_id)));
    let mut evict = Vec::new();
    for cache in caches {
        if total <= budget {
            break;
        }
        if keep(&cache.tileset_id) {
            continue;
        }
        total = total.saturating_sub(cache.bytes);
        evict.push(cache.tileset_id);
    }
    evict
}

impl TilesetTransfer {
    /// Service over `transport`, keeping downloads under `download_root`.
    pub fn new(transport: Arc<dyn BlobTransport>, download_root: impl Into<PathBuf>) -> Self {
        Self {
            transport,
            download_root: download_root.into(),
            limits: TransferLimits::default(),
            catalog: Mutex::new(BTreeMap::new()),
            ledgers: Mutex::new(HashMap::new()),
            active: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Mark `tileset_id` busy until the guard drops.
    fn busy(&self, tileset_id: &str) -> ActiveGuard {
        *lock(&self.active)
            .entry(tileset_id.to_string())
            .or_insert(0) += 1;
        ActiveGuard {
            active: Arc::clone(&self.active),
            tileset_id: tileset_id.to_string(),
        }
    }

    /// Evict least-recently-used streamed caches until all of them fit
    /// [`TransferLimits::max_streamed_cache_bytes`]. A full download (in
    /// progress, or complete and awaiting install) is never a cache, and a
    /// cache with a request running is never evicted. Returns the evicted ids.
    pub async fn trim_streamed_caches(&self) -> Result<Vec<String>, TransferError> {
        let root = self.download_root.clone();
        let caches = blocking(move || Ok(streamed_caches(&root))).await?;
        let active = lock(&self.active).clone();
        let evict = caches_to_evict(caches, self.limits.max_streamed_cache_bytes, &|id| {
            active.contains_key(id)
        });
        for tileset_id in &evict {
            tracing::info!(
                tileset_id,
                "Evicting a least-recently-used streamed tileset cache (disk budget)"
            );
            self.cancel(tileset_id).await?;
        }
        Ok(evict)
    }

    /// Stamp `downloads/<id>/last-used` with now (the streamed-cache LRU clock).
    async fn touch(&self, dir: PathBuf) -> Result<(), TransferError> {
        blocking(move || {
            if !dir.is_dir() {
                return Ok(());
            }
            write_atomic(&dir.join(LAST_USED_FILE), now_ms().to_string().as_bytes())
        })
        .await
    }

    /// The same service under explicit size caps (tests use small ones).
    pub fn with_limits(mut self, limits: TransferLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The size caps this service enforces.
    pub fn limits(&self) -> TransferLimits {
        self.limits
    }

    /// Service keeping downloads under `<p2p_dir>/downloads`.
    pub fn for_p2p_dir(transport: Arc<dyn BlobTransport>, p2p_dir: &Path) -> Self {
        Self::new(transport, p2p_dir.join(DOWNLOADS_DIR))
    }

    /// The download root directory.
    pub fn download_root(&self) -> &Path {
        &self.download_root
    }

    /// `downloads/<id>` for a validated id — the only path join for untrusted ids.
    pub fn download_dir(&self, tileset_id: &str) -> Result<PathBuf, TransferError> {
        if !is_safe_tileset_id(tileset_id) {
            return Err(TransferError::UnsafeTilesetId(tileset_id.to_string()));
        }
        Ok(self.download_root.join(tileset_id))
    }

    // ── publish side ────────────────────────────────────────────────────

    /// Publish `[index, bundle_0 … bundle_n]` as a HashSeq tagged `fe/tileset/<id>`.
    pub async fn publish(
        &self,
        tileset_id: &str,
        index_json: Bytes,
        bundle_paths: &[PathBuf],
    ) -> Result<CatalogEntry, TransferError> {
        self.download_dir(tileset_id)?;
        let transport_err = |e: anyhow::Error| TransferError::Transport(format!("{e:#}"));
        let mut children = Vec::with_capacity(bundle_paths.len() + 1);
        children.push(
            self.transport
                .add(index_json)
                .await
                .map_err(transport_err)?,
        );
        let mut bytes = 0u64;
        for path in bundle_paths {
            let path = path.clone();
            let data = blocking(move || {
                std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))
            })
            .await?;
            bytes += data.len() as u64;
            children.push(
                self.transport
                    .add(Bytes::from(data))
                    .await
                    .map_err(transport_err)?,
            );
        }
        let root = self
            .transport
            .add_hash_seq(&children)
            .await
            .map_err(transport_err)?;
        self.transport
            .set_tag(&tileset_tag(tileset_id), root)
            .await
            .map_err(transport_err)?;
        let entry = CatalogEntry {
            tileset_id: tileset_id.to_string(),
            root,
            total_chunks: bundle_paths.len() as u32,
            bytes,
        };
        lock(&self.catalog).insert(tileset_id.to_string(), entry.clone());
        tracing::info!(
            tileset_id,
            root = %hash_to_hex(&root),
            bundles = entry.total_chunks,
            bytes,
            "Tileset published"
        );
        Ok(entry)
    }

    /// [`Self::publish`] reading the index blob from `index_path`.
    pub async fn publish_files(
        &self,
        tileset_id: &str,
        index_path: &Path,
        bundle_paths: &[PathBuf],
    ) -> Result<CatalogEntry, TransferError> {
        self.download_dir(tileset_id)?;
        let path = index_path.to_path_buf();
        let index = blocking(move || {
            std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))
        })
        .await?;
        self.publish(tileset_id, Bytes::from(index), bundle_paths)
            .await
    }

    /// Stop publishing `tileset_id`; returns whether it was in the catalog.
    pub async fn unpublish(&self, tileset_id: &str) -> Result<bool, TransferError> {
        self.download_dir(tileset_id)?;
        self.transport
            .delete_tag(&tileset_tag(tileset_id))
            .await
            .map_err(|e| TransferError::Transport(format!("{e:#}")))?;
        Ok(lock(&self.catalog).remove(tileset_id).is_some())
    }

    /// Snapshot of the published catalog.
    pub fn catalog(&self) -> Vec<CatalogEntry> {
        lock(&self.catalog).values().cloned().collect()
    }

    /// One catalog entry.
    pub fn catalog_entry(&self, tileset_id: &str) -> Option<CatalogEntry> {
        lock(&self.catalog).get(tileset_id).cloned()
    }

    /// Rebuild the catalog from the persisted `fe/tileset/` tags.
    pub async fn catalog_from_tags(&self) -> Result<Vec<CatalogEntry>, TransferError> {
        let transport_err = |e: anyhow::Error| TransferError::Transport(format!("{e:#}"));
        let tags = self
            .transport
            .list_tags(TILESET_TAG_PREFIX)
            .await
            .map_err(transport_err)?;
        let mut rebuilt = BTreeMap::new();
        for (name, root) in tags {
            let id = &name[TILESET_TAG_PREFIX.len()..];
            if !is_safe_tileset_id(id) {
                tracing::warn!(tag = %name, "Skipping a tileset tag with an unsafe id");
                continue;
            }
            let Some(seq) = self
                .transport
                .read_local(root)
                .await
                .map_err(transport_err)?
            else {
                tracing::warn!(
                    tileset_id = id,
                    "Tileset tag root missing locally — skipped"
                );
                continue;
            };
            let Some(children) = decode_hash_seq(&seq).filter(|c| !c.is_empty()) else {
                tracing::warn!(
                    tileset_id = id,
                    "Tileset tag root is not a HashSeq — skipped"
                );
                continue;
            };
            let mut bytes = 0u64;
            for child in &children[1..] {
                bytes += self
                    .transport
                    .local_size(*child)
                    .await
                    .map_err(transport_err)?
                    .unwrap_or(0);
            }
            rebuilt.insert(
                id.to_string(),
                CatalogEntry {
                    tileset_id: id.to_string(),
                    root,
                    total_chunks: (children.len() - 1) as u32,
                    bytes,
                },
            );
        }
        let entries = rebuilt.values().cloned().collect();
        // Merge, never replace: a `publish` that finished after `list_tags`
        // (startup seeding races this rebuild) must keep its entry.
        let mut catalog = lock(&self.catalog);
        for (id, entry) in rebuilt {
            catalog.entry(id).or_insert(entry);
        }
        Ok(entries)
    }

    // ── download side ───────────────────────────────────────────────────

    /// Resolve `root` from `providers`, persist the ledger, fetch + verify the index.
    ///
    /// A ledger for the same root is reused (providers merged); a different root
    /// discards the old download first.
    pub async fn request_meta(
        &self,
        providers: &[ProviderRef],
        tileset_id: &str,
        root: Hash32,
    ) -> Result<MetaResolved, TransferError> {
        let dir = self.download_dir(tileset_id)?;
        if providers.is_empty() {
            return Err(TransferError::NoProviders(tileset_id.to_string()));
        }
        let _busy = self.busy(tileset_id);
        let existing = match self.ledger(tileset_id).await {
            Ok(ledger) => Some(ledger),
            Err(TransferError::UnknownDownload(_)) => None,
            Err(TransferError::Io(reason)) => {
                tracing::warn!(tileset_id, %reason, "Unreadable download ledger — discarding it");
                self.cancel(tileset_id).await?;
                None
            }
            Err(e) => return Err(e),
        };
        let ledger = match existing {
            Some(old) if old.root == root => {
                let mut merged = (*old).clone();
                for provider in providers {
                    if !merged.providers.contains(provider) {
                        merged.providers.push(provider.clone());
                    }
                }
                merged
            }
            other => {
                if other.is_some() {
                    tracing::info!(
                        tileset_id,
                        "New root for a tileset download — restarting it"
                    );
                    self.cancel(tileset_id).await?;
                }
                let children = self.resolve_root(providers, root).await?;
                Ledger {
                    tileset_id: tileset_id.to_string(),
                    root,
                    providers: providers.to_vec(),
                    index_hash: children[0],
                    chunk_hashes: children[1..].to_vec(),
                    dir,
                }
            }
        };
        self.transport
            .set_tag(&download_tag(tileset_id), root)
            .await
            .map_err(|e| TransferError::Transport(format!("{e:#}")))?;
        let ledger = Arc::new(ledger);
        self.persist_ledger(&ledger).await?;
        lock(&self.ledgers).insert(tileset_id.to_string(), Arc::clone(&ledger));
        let index_json = self.ensure_index(&ledger).await?;
        self.touch(ledger.dir.clone()).await?;
        Ok(MetaResolved {
            tileset_id: ledger.tileset_id.clone(),
            root: ledger.root,
            index_hash: ledger.index_hash,
            chunk_hashes: ledger.chunk_hashes.clone(),
            index_json,
            index_path: ledger.index_path(),
        })
    }

    /// Fetch + verify + store one bundle (no network when its file already verifies).
    pub async fn fetch_chunk(
        &self,
        tileset_id: &str,
        seq: u32,
    ) -> Result<StoredChunk, TransferError> {
        let busy = self.busy(tileset_id);
        let ledger = self.ledger(tileset_id).await?;
        if seq >= ledger.total() {
            return Err(TransferError::SeqOutOfRange {
                seq,
                total: ledger.total(),
            });
        }
        let stored = store_chunk(
            Arc::clone(&self.transport),
            Arc::clone(&ledger),
            seq,
            self.limits.max_chunk_bytes,
        )
        .await
        .map_err(|error| TransferError::Chunk { seq, error })?;
        self.touch(ledger.dir.clone()).await?;
        // Still busy here, so the bundle just stored is never the one evicted.
        if let Err(e) = self.trim_streamed_caches().await {
            tracing::warn!(tileset_id, "Trimming streamed tileset caches failed: {e}");
        }
        drop(busy);
        Ok(stored)
    }

    /// Fetch every missing bundle (bounded concurrency, provider fallback).
    pub async fn download(
        &self,
        tileset_id: &str,
        events: Option<&UnboundedSender<TransferEvent>>,
    ) -> Result<DownloadReport, TransferError> {
        let ledger = self.ledger(tileset_id).await?;
        self.ensure_index(&ledger).await?;
        // S8b: only an explicitly requested full download is resumed at start-up.
        let marker = ledger.dir.join(FULL_DOWNLOAD_MARKER);
        blocking(move || {
            std::fs::write(&marker, b"").map_err(|e| format!("writing {}: {e}", marker.display()))
        })
        .await?;
        let derive_from = Arc::clone(&ledger);
        let max_chunk_bytes = self.limits.max_chunk_bytes;
        let sizes = blocking(move || Ok(derive_received(&derive_from, max_chunk_bytes))).await?;
        let mut stored_bytes: u64 = sizes.iter().flatten().sum();
        let mut received: Vec<bool> = sizes.iter().map(Option::is_some).collect();
        let mut missing = received
            .iter()
            .enumerate()
            .filter(|(_, ok)| !**ok)
            .map(|(seq, _)| seq as u32)
            .collect::<Vec<_>>()
            .into_iter();

        let emit = |event: TransferEvent| {
            if let Some(tx) = events {
                if tx.send(event).is_err() {
                    tracing::warn!(tileset_id, "Tileset transfer event receiver dropped");
                }
            }
        };
        let mut report = DownloadReport {
            tileset_id: tileset_id.to_string(),
            stored: Vec::new(),
            failed: Vec::new(),
            complete: None,
        };
        let cap = self.limits.max_download_bytes;
        let mut over_cap = stored_bytes > cap;
        let mut in_flight = tokio::task::JoinSet::new();
        loop {
            while !over_cap && in_flight.len() < MAX_CONCURRENT_CHUNK_FETCHES {
                let Some(seq) = missing.next() else { break };
                let transport = Arc::clone(&self.transport);
                let ledger = Arc::clone(&ledger);
                in_flight.spawn(async move {
                    (
                        seq,
                        store_chunk(transport, ledger, seq, max_chunk_bytes).await,
                    )
                });
            }
            let Some(joined) = in_flight.join_next().await else {
                break;
            };
            let (seq, outcome) =
                joined.map_err(|e| TransferError::Io(format!("chunk task: {e}")))?;
            match outcome {
                Ok(chunk) => {
                    received[seq as usize] = true;
                    stored_bytes = stored_bytes.saturating_add(chunk.byte_len);
                    over_cap = stored_bytes > cap;
                    report.stored.push(seq);
                    emit(TransferEvent::ChunkStored {
                        tileset_id: tileset_id.to_string(),
                        chunk,
                    });
                }
                Err(error) => {
                    tracing::warn!(tileset_id, seq, %error, "Tileset bundle failed");
                    report.failed.push((seq, error.clone()));
                    emit(TransferEvent::ChunkFailed {
                        tileset_id: tileset_id.to_string(),
                        seq,
                        error,
                    });
                }
            }
        }
        report.stored.sort_unstable();
        report.failed.sort_by_key(|(seq, _)| *seq);
        if over_cap {
            tracing::warn!(
                tileset_id,
                stored_bytes,
                cap,
                "Tileset download over its byte cap — stopped"
            );
            return Err(TransferError::DownloadTooLarge {
                bytes: stored_bytes,
                cap,
            });
        }
        if received.iter().all(|ok| *ok) {
            let complete = CompletedDownload {
                tileset_id: tileset_id.to_string(),
                root: ledger.root,
                index_path: ledger.index_path(),
                bundle_paths: (0..ledger.total()).map(|s| ledger.chunk_path(s)).collect(),
            };
            emit(TransferEvent::Complete(complete.clone()));
            report.complete = Some(complete);
        }
        Ok(report)
    }

    /// [`Self::request_meta`] then [`Self::download`].
    pub async fn start_download(
        &self,
        providers: &[ProviderRef],
        tileset_id: &str,
        root: Hash32,
        events: Option<&UnboundedSender<TransferEvent>>,
    ) -> Result<DownloadReport, TransferError> {
        self.request_meta(providers, tileset_id, root).await?;
        self.download(tileset_id, events).await
    }

    /// Every persisted download under the root, with received counts re-derived from disk.
    pub async fn pending_downloads(&self) -> Result<Vec<ResumedDownload>, TransferError> {
        let root = self.download_root.clone();
        let limits = self.limits;
        let found = blocking(move || {
            let entries = match std::fs::read_dir(&root) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(format!("listing {}: {e}", root.display())),
            };
            let mut found = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !is_safe_tileset_id(&name) || !entry.path().join(LEDGER_FILE).is_file() {
                    continue;
                }
                match read_ledger(&entry.path(), &name) {
                    Ok(ledger) => {
                        let received = derive_received(&ledger, limits.max_chunk_bytes);
                        let index_present = file_verifies(
                            &ledger.index_path(),
                            &ledger.index_hash,
                            limits.max_index_bytes,
                        );
                        let full_download = ledger.dir.join(FULL_DOWNLOAD_MARKER).is_file();
                        found.push((ledger, received, index_present, full_download));
                    }
                    Err(e) => tracing::warn!(tileset_id = %name, "Skipping a bad ledger: {e}"),
                }
            }
            Ok(found)
        })
        .await?;
        let mut out = Vec::with_capacity(found.len());
        let mut ledgers = lock(&self.ledgers);
        for (ledger, received, index_present, full_download) in found {
            out.push(ResumedDownload {
                tileset_id: ledger.tileset_id.clone(),
                received: received.iter().filter(|size| size.is_some()).count() as u32,
                total: ledger.total(),
                index_present,
                full_download,
            });
            ledgers.insert(ledger.tileset_id.clone(), Arc::new(ledger));
        }
        out.sort_by(|a, b| a.tileset_id.cmp(&b.tileset_id));
        Ok(out)
    }

    /// The complete full downloads in `pending` (as just listed by
    /// [`Self::pending_downloads`]), ready for the host to assemble — a
    /// restart between `Complete` and install must not strand them.
    pub fn completed_from(&self, pending: &[ResumedDownload]) -> Vec<CompletedDownload> {
        let ledgers = lock(&self.ledgers);
        pending
            .iter()
            .filter(|p| p.full_download && p.index_present && p.received == p.total)
            .filter_map(|p| ledgers.get(&p.tileset_id))
            .map(|ledger| CompletedDownload {
                tileset_id: ledger.tileset_id.clone(),
                root: ledger.root,
                index_path: ledger.index_path(),
                bundle_paths: (0..ledger.total()).map(|s| ledger.chunk_path(s)).collect(),
            })
            .collect()
    }

    /// Re-verify every persisted full download and fetch only what is missing.
    ///
    /// Already-complete downloads are not re-run; [`Self::pending_downloads`] lists them.
    /// A streamed-bundle cache (no [`FULL_DOWNLOAD_MARKER`]) is never grown into a
    /// full download, and one failing download never blocks the others (S8b).
    pub async fn resume_all(
        &self,
        events: Option<&UnboundedSender<TransferEvent>>,
    ) -> Result<Vec<DownloadReport>, TransferError> {
        let mut reports = Vec::new();
        for pending in self.pending_downloads().await? {
            if pending.received == pending.total && pending.index_present {
                continue;
            }
            if !pending.full_download {
                tracing::debug!(
                    tileset_id = %pending.tileset_id,
                    "Streamed-bundle cache — not resumed as a full download"
                );
                continue;
            }
            tracing::info!(
                tileset_id = %pending.tileset_id,
                received = pending.received,
                total = pending.total,
                "Resuming tileset download"
            );
            match self.download(&pending.tileset_id, events).await {
                Ok(report) => reports.push(report),
                Err(e) => tracing::warn!(
                    tileset_id = %pending.tileset_id,
                    "Resuming tileset download failed: {e}"
                ),
            }
        }
        Ok(reports)
    }

    /// Discard a download: ledger, files and download tag.
    pub async fn cancel(&self, tileset_id: &str) -> Result<(), TransferError> {
        let dir = self.download_dir(tileset_id)?;
        lock(&self.ledgers).remove(tileset_id);
        self.transport
            .delete_tag(&download_tag(tileset_id))
            .await
            .map_err(|e| TransferError::Transport(format!("{e:#}")))?;
        blocking(move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("removing {}: {e}", dir.display())),
        })
        .await
    }

    // ── internals ───────────────────────────────────────────────────────

    /// The ledger for `tileset_id` (memory first, then disk).
    async fn ledger(&self, tileset_id: &str) -> Result<Arc<Ledger>, TransferError> {
        let dir = self.download_dir(tileset_id)?;
        if let Some(ledger) = lock(&self.ledgers).get(tileset_id) {
            return Ok(Arc::clone(ledger));
        }
        let id = tileset_id.to_string();
        let loaded = blocking(move || {
            if !dir.join(LEDGER_FILE).is_file() {
                return Ok(None);
            }
            read_ledger(&dir, &id).map(Some)
        })
        .await?;
        let ledger =
            Arc::new(loaded.ok_or_else(|| TransferError::UnknownDownload(tileset_id.to_string()))?);
        lock(&self.ledgers).insert(tileset_id.to_string(), Arc::clone(&ledger));
        Ok(ledger)
    }

    /// Children of `root` from the first provider whose answer hashes back to it.
    async fn resolve_root(
        &self,
        providers: &[ProviderRef],
        root: Hash32,
    ) -> Result<Vec<Hash32>, TransferError> {
        let mut last = String::from("no provider answered");
        for provider in providers {
            match self
                .transport
                .fetch_hash_seq(provider, root, MAX_HASH_SEQ_CHILDREN)
                .await
            {
                Ok(children) if hash_seq_root(&children) != root => {
                    last = "children do not hash to the root".to_string();
                }
                Ok(children) if children.is_empty() => last = "empty HashSeq".to_string(),
                Ok(children) if children.len() > MAX_HASH_SEQ_CHILDREN => {
                    last = format!("{} children exceeds the cap", children.len());
                }
                Ok(children) => return Ok(children),
                Err(e) => last = format!("{e:#}"),
            }
            tracing::warn!(provider = %provider.as_str(), %last, "Tileset root unresolved");
        }
        Err(TransferError::BadRoot(last))
    }

    /// The verified index blob: from disk when it verifies, else fetched and written.
    async fn ensure_index(&self, ledger: &Arc<Ledger>) -> Result<Bytes, TransferError> {
        let path = ledger.index_path();
        let expected = ledger.index_hash;
        let max_index_bytes = self.limits.max_index_bytes;
        let on_disk =
            blocking(move || Ok(read_verified(&path, &expected, max_index_bytes))).await?;
        if let Some(bytes) = on_disk {
            return Ok(Bytes::from(bytes));
        }
        let mut last = String::from("no provider answered");
        for provider in &ledger.providers {
            match self
                .transport
                .fetch(provider, expected, max_index_bytes as u64)
                .await
            {
                Ok(bytes) if bytes.len() > max_index_bytes => {
                    last = format!("index blob too large: {} bytes", bytes.len());
                }
                Ok(bytes) if content_hash(&bytes) != expected => {
                    last = "index blob hash mismatch".to_string();
                }
                Ok(bytes) => {
                    let path = ledger.index_path();
                    let to_write = bytes.clone();
                    blocking(move || write_atomic(&path, &to_write)).await?;
                    return Ok(bytes);
                }
                Err(e) => last = format!("{e:#}"),
            }
            tracing::warn!(provider = %provider.as_str(), %last, "Tileset index unavailable");
        }
        Err(TransferError::BadIndex(last))
    }

    /// Write `download.json` (creating the download dir).
    async fn persist_ledger(&self, ledger: &Arc<Ledger>) -> Result<(), TransferError> {
        let json = serde_json::to_vec_pretty(&ledger.to_file())
            .map_err(|e| TransferError::Io(format!("encoding ledger: {e}")))?;
        let dir = ledger.dir.clone();
        blocking(move || {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("creating {}: {e}", dir.display()))?;
            write_atomic(&dir.join(LEDGER_FILE), &json)
        })
        .await
    }
}

/// Read and validate `<dir>/download.json` for `tileset_id`. Blocking.
fn read_ledger(dir: &Path, tileset_id: &str) -> Result<Ledger, String> {
    let path = dir.join(LEDGER_FILE);
    let len = std::fs::metadata(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?
        .len();
    if len > MAX_LEDGER_FILE_BYTES {
        return Err(format!(
            "{} is {len} bytes (cap {MAX_LEDGER_FILE_BYTES})",
            path.display()
        ));
    }
    let raw = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let file: LedgerFile =
        serde_json::from_slice(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    if file.tileset_id != tileset_id {
        return Err(format!(
            "ledger names {:?}, directory is {tileset_id:?}",
            file.tileset_id
        ));
    }
    Ledger::from_file(file, dir.to_path_buf())
}

/// Store bundle `seq`: reuse a verifying file, else local blob, else providers in order.
async fn store_chunk(
    transport: Arc<dyn BlobTransport>,
    ledger: Arc<Ledger>,
    seq: u32,
    max_chunk_bytes: usize,
) -> Result<StoredChunk, ChunkError> {
    let expected = ledger.chunk_hashes[seq as usize];
    let path = ledger.chunk_path(seq);
    let check_path = path.clone();
    let existing = tokio::task::spawn_blocking(move || {
        read_verified(&check_path, &expected, max_chunk_bytes).map(|bytes| bytes.len() as u64)
    })
    .await
    .map_err(|e| ChunkError::Io(format!("blocking task: {e}")))?;
    if let Some(byte_len) = existing {
        return Ok(StoredChunk {
            seq,
            path,
            byte_len,
        });
    }

    let mut last = match transport.read_local(expected).await {
        Ok(Some(bytes)) => match verify_chunk(&bytes, expected, max_chunk_bytes) {
            Ok(()) => return write_chunk(path, seq, bytes).await,
            Err(e) => e,
        },
        Ok(None) => ChunkError::Unavailable("no provider candidates".to_string()),
        Err(e) => ChunkError::Unavailable(format!("{e:#}")),
    };
    for provider in &ledger.providers {
        match transport
            .fetch(provider, expected, max_chunk_bytes as u64)
            .await
        {
            Ok(bytes) => match verify_chunk(&bytes, expected, max_chunk_bytes) {
                Ok(()) => return write_chunk(path, seq, bytes).await,
                Err(e) => last = e,
            },
            Err(e) => last = ChunkError::Unavailable(format!("{e:#}")),
        }
        tracing::warn!(provider = %provider.as_str(), seq, error = %last, "Bundle fetch failed");
    }
    Err(last)
}

/// Size cap + BLAKE3 check of one bundle against its HashSeq child.
fn verify_chunk(bytes: &[u8], expected: Hash32, max_chunk_bytes: usize) -> Result<(), ChunkError> {
    if bytes.len() > max_chunk_bytes {
        return Err(ChunkError::TooLarge { len: bytes.len() });
    }
    let actual = content_hash(bytes);
    if actual != expected {
        return Err(ChunkError::HashMismatch { expected, actual });
    }
    Ok(())
}

/// Atomically write a verified bundle.
async fn write_chunk(path: PathBuf, seq: u32, bytes: Bytes) -> Result<StoredChunk, ChunkError> {
    let byte_len = bytes.len() as u64;
    let target = path.clone();
    tokio::task::spawn_blocking(move || write_atomic(&target, &bytes))
        .await
        .map_err(|e| ChunkError::Io(format!("blocking task: {e}")))?
        .map_err(ChunkError::Io)?;
    Ok(StoredChunk {
        seq,
        path,
        byte_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tileset_id_guard_table() {
        for ok in [
            "usgs-z12",
            "a",
            "tileset_1.v2",
            "A-b_c.d",
            "console",
            "nul2",
        ] {
            assert!(is_safe_tileset_id(ok), "{ok:?} should be accepted");
        }
        let long = "x".repeat(MAX_TILESET_ID_LEN + 1);
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "C:x",
            "a b",
            "é",
            "CON",
            "nul.txt",
            "Lpt1",
            long.as_str(),
        ] {
            assert!(!is_safe_tileset_id(bad), "{bad:?} should be refused");
        }
    }
}
