//! Bevy `AssetReader` backed by a `BlobStoreHandle` (P2P Mycelium Phase B).
//!
//! When registered as the `"blob"` asset source, Bevy resolves paths like
//! `blob://{hex_hash}.glb` through this reader. The `read` method extracts the
//! hex hash from the filename stem, looks up the blob on disk via
//! `BlobStore::get_blob_path`, and returns the file bytes -- unless the KTX2
//! intake guard refuses them (src/AGENTS.md §ktx2-guard).
//!
//! Also the extension byte-source seam asked only after a peer miss
//! (bindings.md §6 `blob.source`, `sync.tileset-source`; src/AGENTS.md §blob-fallback).

use bevy::asset::io::{AssetReader, AssetReaderError, PathStream, Reader, VecReader};
use bytes::Bytes;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{mpsc, Arc, OnceLock, RwLock};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use crate::blob_store::{hash_from_hex, hash_to_hex, BlobHash, BlobStoreHandle};

/// Callback invoked when a blob is not found locally.
///
/// The callback receives the missing blob's hash so that the sync layer can
/// attempt a peer fetch.  Set by `fe-sync` at startup — `fe-runtime` knows
/// nothing about the sync crate, avoiding a dependency cycle.
pub type OnMissCallback = Arc<dyn Fn(BlobHash) + Send + Sync>;

/// Largest blob a `blob.source` answer may carry (the hook's 16 MiB body cap).
pub const BLOB_SOURCE_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// A fallback answer; executor-agnostic (no runtime timer inside), `None` when nothing bound serves.
pub type FallbackFuture<'a> = Pin<Box<dyn Future<Output = Option<Bytes>> + Send + 'a>>;

/// One blob every peer missed (`blob.source` input, bindings.md §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobSourceRequest {
    pub hash: BlobHash,
    /// Exact byte count when the asking path knows it (scene metadata); the answer must match it.
    pub expected_size: Option<u64>,
    pub asset_id: Option<String>,
    /// Petal whose bindings pick the source; `None` asks at the host's active petal.
    pub petal_id: Option<String>,
}

/// One tileset HashSeq child every peer missed (`sync.tileset-source` input, bindings.md §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilesetChildRequest {
    pub tileset_id: String,
    pub root: BlobHash,
    pub child_hash: BlobHash,
    /// Exact byte count when known; fe-sync never parses the index, so bundles carry `None`.
    pub size: Option<u64>,
    /// The asking service's cap for this child.
    pub max_bytes: u64,
}

/// An extension-bound byte source asked only after the peers missed; never a transport, relay or peer choice.
pub trait BlobFallback: Send + Sync {
    /// Bytes for one blob; `None` when nothing bound serves it here.
    fn source_blob(&self, request: BlobSourceRequest) -> FallbackFuture<'_>;
    /// Bytes for one tileset child; `None` when nothing bound serves it here.
    fn source_tileset_child(&self, request: TilesetChildRequest) -> FallbackFuture<'_>;
}

/// Why fallback bytes were refused; nothing of them is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackRejection {
    /// Over the asking path's cap.
    TooLarge { len: u64, max: u64 },
    /// The asking path knew the size and the bytes differ.
    SizeMismatch { expected: u64, actual: u64 },
    /// BLAKE3 of the bytes is not the requested hash.
    HashMismatch,
}

impl std::fmt::Display for FallbackRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { len, max } => write!(f, "{len} bytes is over the {max}-byte cap"),
            Self::SizeMismatch { expected, actual } => write!(f, "{actual} bytes where {expected} were expected"),
            Self::HashMismatch => f.write_str("BLAKE3 does not match the requested hash"),
        }
    }
}

/// The content-addressing gate every fallback byte passes: the cap, the exact size when known, then BLAKE3.
pub fn verify_fallback_bytes(
    hash: &BlobHash,
    expected_size: Option<u64>,
    max_bytes: u64,
    bytes: &[u8],
) -> Result<(), FallbackRejection> {
    let len = bytes.len() as u64;
    if len > max_bytes {
        return Err(FallbackRejection::TooLarge { len, max: max_bytes });
    }
    if let Some(expected) = expected_size {
        if expected != len {
            return Err(FallbackRejection::SizeMismatch { expected, actual: len });
        }
    }
    if blake3::hash(bytes).as_bytes() != hash {
        return Err(FallbackRejection::HashMismatch);
    }
    Ok(())
}

/// Late-bound fallback shared by the sync thread and the blob reader: empty (peers only) until the plugin host
/// installs one. Its `source_*` methods return only bytes that passed [`verify_fallback_bytes`].
#[derive(Clone, Default)]
pub struct BlobFallbackSlot(Arc<RwLock<Option<Arc<dyn BlobFallback>>>>);

impl std::fmt::Debug for BlobFallbackSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobFallbackSlot").field("installed", &self.is_installed()).finish()
    }
}

impl BlobFallbackSlot {
    /// Route later misses to `source`.
    pub fn install(&self, source: Arc<dyn BlobFallback>) {
        *self.0.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
    }

    /// Back to peers only (the plugin host stopped).
    pub fn clear(&self) {
        *self.0.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn is_installed(&self) -> bool {
        self.current().is_some()
    }

    fn current(&self) -> Option<Arc<dyn BlobFallback>> {
        self.0.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Verified bytes for one blob the peers missed; refused bytes are logged and dropped.
    pub async fn source_blob(&self, request: BlobSourceRequest) -> Option<Bytes> {
        let source = self.current()?;
        let (hash, expected_size) = (request.hash, request.expected_size);
        let bytes = source.source_blob(request).await?;
        accept("blob.source", &hash, expected_size, BLOB_SOURCE_MAX_BYTES, bytes)
    }

    /// Verified bytes for one tileset child the peers missed; refused bytes are logged and dropped.
    pub async fn source_tileset_child(&self, request: TilesetChildRequest) -> Option<Bytes> {
        let source = self.current()?;
        let (hash, size, max_bytes) = (request.child_hash, request.size, request.max_bytes);
        let bytes = source.source_tileset_child(request).await?;
        accept("sync.tileset-source", &hash, size, max_bytes, bytes)
    }
}

fn accept(hook: &str, hash: &BlobHash, expected_size: Option<u64>, max_bytes: u64, bytes: Bytes) -> Option<Bytes> {
    match verify_fallback_bytes(hash, expected_size, max_bytes, &bytes) {
        Ok(()) => Some(bytes),
        Err(rejection) => {
            tracing::warn!(hook, hash = %hash_to_hex(hash), %rejection, "extension source bytes refused");
            None
        }
    }
}

/// Longest an asset-IO task waits on the bound source, so an unpumped slot cannot park it.
pub const READER_SOURCE_TIMEOUT: Duration = Duration::from_secs(35);

/// `future` or `None` after `limit`; executor-agnostic (the shared deadline thread wakes the task), for Bevy IO tasks
/// that have no tokio timer.
async fn with_deadline(future: impl Future<Output = Option<Bytes>>, limit: Duration) -> Option<Bytes> {
    let mut future = Box::pin(future);
    let deadline = Instant::now() + limit;
    let mut registered: Option<Waker> = None;
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(answer) = future.as_mut().poll(cx) {
            return Poll::Ready(answer);
        }
        if Instant::now() >= deadline {
            return Poll::Ready(None);
        }
        // Register once per distinct waker, so a task moved to another worker is still woken at the deadline.
        if !registered.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) {
            registered = Some(cx.waker().clone());
            wake_at(deadline, cx.waker().clone());
        }
        Poll::Pending
    })
    .await
}

/// Wake `waker` at `deadline` from one shared timer thread (never a thread per read: a burst of misses would
/// otherwise park one sleeping OS thread each for the whole timeout).
fn wake_at(deadline: Instant, waker: Waker) {
    static TIMER: OnceLock<Option<mpsc::Sender<(Instant, Waker)>>> = OnceLock::new();
    let timer = TIMER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("fe-blob-deadline".into())
            .spawn(move || run_deadline_timer(receiver))
            .map_err(|error| tracing::warn!("blob reader deadline timer unavailable: {error}"))
            .ok()
            .map(|_| sender)
    });
    let unsent = match timer {
        Some(sender) => sender.send((deadline, waker)).err().map(|mpsc::SendError((_, waker))| waker),
        None => Some(waker),
    };
    // No shared timer: fall back to one sleeping thread for this read, so the deadline still holds.
    if let Some(waker) = unsent {
        let limit = deadline.saturating_duration_since(Instant::now());
        let spawned = std::thread::Builder::new().name("fe-blob-deadline-1".into()).spawn({
            let waker = waker.clone();
            move || {
                std::thread::sleep(limit);
                waker.wake();
            }
        });
        if spawned.is_err() {
            waker.wake();
        }
    }
}

/// The deadline thread: wakes every registered task whose deadline passed, sleeping until the next one.
fn run_deadline_timer(receiver: mpsc::Receiver<(Instant, Waker)>) {
    let mut pending: Vec<(Instant, Waker)> = Vec::new();
    loop {
        let now = Instant::now();
        pending.retain(|(at, waker)| {
            let due = *at <= now;
            if due {
                waker.wake_by_ref();
            }
            !due
        });
        let received = match pending.iter().map(|(at, _)| *at).min() {
            Some(next) => receiver.recv_timeout(next.saturating_duration_since(Instant::now())),
            None => receiver.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(entry) => pending.push(entry),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// An [`AssetReader`] that reads files from a content-addressed blob store.
///
/// Register with Bevy via:
/// ```ignore
/// app.register_asset_source("blob", AssetSourceBuilder::new(move || {
///     Box::new(BlobAssetReader::new(blob_store.clone()))
/// }));
/// ```
pub struct BlobAssetReader {
    blob_store: BlobStoreHandle,
    on_miss: Option<OnMissCallback>,
    fallback: BlobFallbackSlot,
}

impl BlobAssetReader {
    pub fn new(blob_store: BlobStoreHandle) -> Self {
        Self {
            blob_store,
            on_miss: None,
            fallback: BlobFallbackSlot::default(),
        }
    }

    /// Create a reader with an `on_miss` callback that fires when a blob
    /// lookup returns `NotFound`.
    pub fn with_on_miss(blob_store: BlobStoreHandle, on_miss: OnMissCallback) -> Self {
        Self {
            blob_store,
            on_miss: Some(on_miss),
            fallback: BlobFallbackSlot::default(),
        }
    }

    /// The same reader asking `fallback` on a miss when no peer layer is wired (no `on_miss`, so the miss is final).
    pub fn with_fallback(mut self, fallback: BlobFallbackSlot) -> Self {
        self.fallback = fallback;
        self
    }

    /// Parse the hex hash from a path like `{64-char-hex}.glb`.
    fn resolve_blob_path(&self, path: &Path) -> Result<std::path::PathBuf, AssetReaderError> {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AssetReaderError::NotFound(path.to_path_buf()))?;

        let hash = hash_from_hex(stem).map_err(|_| {
            tracing::warn!(
                "BlobAssetReader: invalid hex hash in path: {}",
                path.display()
            );
            AssetReaderError::NotFound(path.to_path_buf())
        })?;

        self.blob_store.get_blob_path(&hash).ok_or_else(|| {
            tracing::warn!(
                "BlobAssetReader: blob not found for hash {} (path: {})",
                stem,
                path.display()
            );
            AssetReaderError::NotFound(path.to_path_buf())
        })
    }

    /// A local miss: the peer layer fetches when one is wired (`on_miss`); with none, the bound source is asked now.
    async fn after_miss(&self, path: &Path) -> Option<Vec<u8>> {
        let stem = path.file_stem().and_then(|s| s.to_str())?;
        let hash = hash_from_hex(stem).ok()?;
        if let Some(ref cb) = self.on_miss {
            // The sync thread fetches from peers; the asset is available on a
            // subsequent load after the fetch completes.
            tracing::debug!(hash = %stem, "BlobAssetReader: miss — notifying sync layer");
            cb(hash);
            return None;
        }
        let request = BlobSourceRequest { hash, expected_size: None, asset_id: None, petal_id: None };
        let bytes = with_deadline(self.fallback.source_blob(request), READER_SOURCE_TIMEOUT).await?;
        if let Err(error) = self.blob_store.add_blob(&bytes) {
            tracing::warn!(hash = %stem, "BlobAssetReader: verified source bytes not stored: {error:#}");
        }
        Some(bytes.to_vec())
    }
}

impl AssetReader for BlobAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let bytes = match self.resolve_blob_path(path) {
            Ok(blob_path) => std::fs::read(&blob_path).map_err(|e| {
                tracing::error!(
                    "BlobAssetReader: failed to read blob at {}: {e}",
                    blob_path.display()
                );
                AssetReaderError::Io(Arc::new(e))
            })?,
            // We still return NotFound unless a bound source answered with
            // bytes that hash to the requested blob.
            Err(e) => match self.after_miss(path).await {
                Some(bytes) => bytes,
                None => return Err(e),
            },
        };
        // Every blob:// load converges here, whatever put the bytes in
        // the store (REST, MCP, peer sync, hexon install, import, an
        // extension source).
        let extension = path.extension().and_then(|e| e.to_str());
        crate::ktx2_guard::check_asset_bytes(&bytes, extension).map_err(|refusal| {
            tracing::warn!("BlobAssetReader: refused {}: {refusal}", path.display());
            AssetReaderError::Io(Arc::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                refusal,
            )))
        })?;
        Ok(VecReader::new(bytes))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        // Blob store does not support .meta files.
        Err::<VecReader, _>(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        Err(AssetReaderError::NotFound(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_blob_path_returns_err_for_bad_hex() {
        let store: BlobStoreHandle =
            std::sync::Arc::new(crate::blob_store::mock::MockBlobStore::new());
        let reader = BlobAssetReader::new(store);
        let result = reader.resolve_blob_path(Path::new("not-a-hash.glb"));
        assert!(result.is_err());
    }

    /// A real on-disk store under the temp dir, laid out canonically.
    struct DirBlobStore(std::path::PathBuf);

    impl crate::blob_store::BlobStore for DirBlobStore {
        fn add_blob(&self, bytes: &[u8]) -> anyhow::Result<BlobHash> {
            let hash: BlobHash = *blake3::hash(bytes).as_bytes();
            let path = crate::blob_store::canonical_blob_path(&self.0, &hash);
            std::fs::create_dir_all(path.parent().expect("sharded path"))?;
            std::fs::write(path, bytes)?;
            Ok(hash)
        }
        fn get_blob_path(&self, hash: &BlobHash) -> Option<std::path::PathBuf> {
            let path = crate::blob_store::canonical_blob_path(&self.0, hash);
            path.exists().then_some(path)
        }
        fn has_blob(&self, hash: &BlobHash) -> bool {
            self.get_blob_path(hash).is_some()
        }
        fn remove_blob(&self, _hash: &BlobHash) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// A bound source answering fixed bytes per requested hash, honest or forged.
    struct Answers(std::collections::HashMap<BlobHash, Vec<u8>>);

    impl BlobFallback for Answers {
        fn source_blob(&self, request: BlobSourceRequest) -> FallbackFuture<'_> {
            let answer = self.0.get(&request.hash).cloned().map(Bytes::from);
            Box::pin(async move { answer })
        }
        fn source_tileset_child(&self, _request: TilesetChildRequest) -> FallbackFuture<'_> {
            Box::pin(async { None })
        }
    }

    /// The choke point: a blob that arrived by any route is refused at load
    /// time when it carries a KTX2 texture, and served when it does not. With
    /// no peer layer wired, a miss asks the bound source, whose bytes are kept
    /// only when they hash to the requested blob and still pass the guard.
    #[test]
    fn blob_reads_refuse_ktx2_glbs_and_serve_plain_ones() {
        use crate::blob_store::BlobStore as _;
        use crate::ktx2_guard::tests::build_glb;
        let root = std::env::temp_dir().join(format!(
            "fe-ktx2-guard-reader-{}-{}",
            std::process::id(),
            nanos_suffix()
        ));
        let store = Arc::new(DirBlobStore(root.clone()));
        let reader = BlobAssetReader::new(store.clone());

        let plain = build_glb(r#"{"asset":{"version":"2.0"}}"#, None);
        let ktx2 = build_glb(
            r#"{"images":[{"uri":"t.png","mimeType":"image/ktx2"}]}"#,
            None,
        );
        // A JSON glTF stored as a `.glb` blob: bevy's gltf parser sniffs the
        // content, so the `.glb` name must not let it skip the guard.
        let ktx2_json = br#"{"images":[{"uri":"data:image%2Fktx2%3Bbase64,AAAA"}]}"#;
        let plain_path = format!("{}.glb", hash_to_hex(&store.add_blob(&plain).unwrap()));
        let ktx2_path = format!("{}.glb", hash_to_hex(&store.add_blob(&ktx2).unwrap()));
        let json_path = format!("{}.glb", hash_to_hex(&store.add_blob(ktx2_json).unwrap()));

        let read_error = |reader: &BlobAssetReader, path: &str| match bevy::tasks::block_on(reader.read(Path::new(path))) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        };
        let plain_error = read_error(&reader, &plain_path);
        let refusals = [read_error(&reader, &ktx2_path), read_error(&reader, &json_path)];

        // Three blobs nobody stored: an honest answer, a forged one, and an honest KTX2 GLB.
        let sourced = build_glb(r#"{"asset":{"version":"2.0","generator":"source"}}"#, None);
        let forged_target = build_glb(r#"{"asset":{"version":"2.0","generator":"real"}}"#, None);
        let sourced_ktx2 = build_glb(r#"{"images":[{"uri":"s.png","mimeType":"image/ktx2"}]}"#, None);
        let hash_of = |bytes: &[u8]| -> BlobHash { *blake3::hash(bytes).as_bytes() };
        let (sourced_hash, forged_hash, ktx2_hash) = (hash_of(&sourced), hash_of(&forged_target), hash_of(&sourced_ktx2));
        let slot = BlobFallbackSlot::default();
        let unbound = BlobAssetReader::new(store.clone()).with_fallback(slot.clone());
        let missing = format!("{}.glb", hash_to_hex(&sourced_hash));
        let unbound_error = read_error(&unbound, &missing);
        slot.install(Arc::new(Answers(std::collections::HashMap::from([
            (sourced_hash, sourced.clone()),
            (forged_hash, plain.clone()),
            (ktx2_hash, sourced_ktx2.clone()),
        ]))));
        let sourced_error = read_error(&unbound, &missing);
        let forged_error = read_error(&unbound, &format!("{}.glb", hash_to_hex(&forged_hash)));
        let sourced_ktx2_error = read_error(&unbound, &format!("{}.glb", hash_to_hex(&ktx2_hash)));
        // A peer layer is wired: the reader only notifies it, and never asks the source before the peers missed.
        let notified = Arc::new(std::sync::Mutex::new(Vec::<BlobHash>::new()));
        let seen = notified.clone();
        let with_peers =
            BlobAssetReader::with_on_miss(store.clone(), Arc::new(move |hash: BlobHash| seen.lock().unwrap().push(hash)))
                .with_fallback(slot.clone());
        let peer_error = read_error(&with_peers, &format!("{}.glb", hash_to_hex(&forged_hash)));
        let stored = [store.has_blob(&sourced_hash), store.has_blob(&forged_hash)];
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(plain_error, None, "a plain GLB must load");
        for (label, refusal) in ["KTX2 GLB", "KTX2 JSON glTF named .glb"]
            .iter()
            .zip(refusals)
        {
            let refusal = refusal.unwrap_or_else(|| panic!("{label} must be refused"));
            assert!(refusal.contains("RUSTSEC-2026-0344"), "{label}: {refusal}");
        }
        assert!(unbound_error.is_some(), "nothing bound: the miss stays a miss");
        assert_eq!(sourced_error, None, "verified source bytes load");
        assert!(forged_error.is_some(), "bytes hashing to another blob are refused");
        assert!(sourced_ktx2_error.is_some_and(|refusal| refusal.contains("RUSTSEC-2026-0344")), "source bytes meet the guard");
        assert_eq!(stored, [true, false], "only verified source bytes are stored");
        assert!(peer_error.is_some());
        assert_eq!(*notified.lock().unwrap(), vec![forged_hash], "with peers wired the miss goes to them first");
        assert_eq!(
            verify_fallback_bytes(&sourced_hash, Some(sourced.len() as u64 + 1), BLOB_SOURCE_MAX_BYTES, &sourced),
            Err(FallbackRejection::SizeMismatch { expected: sourced.len() as u64 + 1, actual: sourced.len() as u64 })
        );
        assert_eq!(
            verify_fallback_bytes(&sourced_hash, None, 8, &sourced),
            Err(FallbackRejection::TooLarge { len: sourced.len() as u64, max: 8 })
        );
        // A source nobody pumps cannot park the IO task: the shared deadline thread ends both waits.
        let parked = [20, 40].map(|ms| {
            bevy::tasks::block_on(with_deadline(std::future::pending(), Duration::from_millis(ms)))
        });
        assert_eq!(parked, [None, None], "an unanswered source ends at the reader deadline");
    }

    fn nanos_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn resolve_blob_path_returns_err_for_missing_blob() {
        let store: BlobStoreHandle =
            std::sync::Arc::new(crate::blob_store::mock::MockBlobStore::new());
        let reader = BlobAssetReader::new(store);
        let fake_hash = hash_to_hex(&[0u8; 32]);
        let result = reader.resolve_blob_path(Path::new(&format!("{fake_hash}.glb")));
        assert!(result.is_err());
    }
}
