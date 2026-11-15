//! Archive read guard: bounded permits + parsed-index cache — see `AGENTS.md` §hardening.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fe_format::HexonArchiveIndex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Default concurrent archive reads (`FE_REGISTRY_ARCHIVE_CONCURRENCY`).
pub const DEFAULT_ARCHIVE_CONCURRENCY: usize = 4;
/// How long a request waits for a permit before answering 503.
const PERMIT_WAIT: Duration = Duration::from_millis(250);
/// Parsed indexes kept at most.
const CACHE_MAX_ENTRIES: usize = 8;
/// Header bytes (manifest + header members) the cached indexes may hold in total.
const CACHE_MAX_BYTES: usize = 64 << 20;

/// How long a buffered response may keep its permit while the client reads it (`AGENTS.md` §hardening).
#[derive(Clone, Copy, Debug)]
pub struct DrainPolicy {
    /// Time a client gets before the minimum rate applies.
    pub grace: Duration,
    /// Bytes per second the client must take once the grace period is over.
    pub min_bytes_per_sec: u64,
}

impl Default for DrainPolicy {
    fn default() -> Self {
        Self { grace: Duration::from_secs(30), min_bytes_per_sec: 64 << 10 }
    }
}

impl DrainPolicy {
    /// True when a body `taken` bytes in after `elapsed` is behind the minimum-rate schedule.
    pub fn is_stalled(&self, taken: u64, elapsed: Duration) -> bool {
        let Some(overdue) = elapsed.checked_sub(self.grace) else {
            return false;
        };
        let owed = (overdue.as_secs_f64() * self.min_bytes_per_sec as f64).ceil().max(1.0);
        (taken as f64) < owed
    }

    /// How often the watchdog checks a held response.
    pub(crate) fn tick(&self) -> Duration {
        (self.grace / 4).clamp(Duration::from_millis(50), Duration::from_secs(5))
    }
}

/// One archive permit and the drain policy its response body runs under.
pub struct ArchivePermit {
    _permit: OwnedSemaphorePermit,
    pub(crate) drain: DrainPolicy,
}

type CacheKey = (PathBuf, u64, Option<SystemTime>);

struct Slot {
    index: Arc<HexonArchiveIndex>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Cache {
    slots: HashMap<CacheKey, Slot>,
    bytes: usize,
    tick: u64,
}

/// Permit pool bounding concurrent archive reads, plus a bounded LRU of parsed archive indexes.
pub struct ArchiveGuard {
    permits: Arc<Semaphore>,
    cache: Mutex<Cache>,
    drain: DrainPolicy,
}

impl ArchiveGuard {
    pub fn new(concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            cache: Mutex::new(Cache::default()),
            drain: DrainPolicy::default(),
        }
    }

    /// Replace the drain policy held responses run under.
    pub fn with_drain(mut self, drain: DrainPolicy) -> Self {
        self.drain = drain;
        self
    }

    /// The drain policy held responses run under.
    pub fn drain(&self) -> DrainPolicy {
        self.drain
    }

    /// The permit pool itself (tests hold every permit through this to force the 503 path).
    pub fn permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }

    /// One permit, or `None` when none frees up within a short wait.
    pub async fn acquire(&self) -> Option<ArchivePermit> {
        tokio::time::timeout(PERMIT_WAIT, Arc::clone(&self.permits).acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
            .map(|permit| ArchivePermit { _permit: permit, drain: self.drain })
    }

    /// Parsed index for `path`, cached per (path, length, mtime); blocking — call inside `spawn_blocking`.
    pub fn open(&self, path: &Path) -> anyhow::Result<Arc<HexonArchiveIndex>> {
        let meta = std::fs::metadata(path)?;
        let key: CacheKey = (path.to_path_buf(), meta.len(), meta.modified().ok());
        if let Some(index) = self.lookup(&key) {
            return Ok(index);
        }
        let index = Arc::new(HexonArchiveIndex::open(path)?);
        let doc = index.header_doc();
        let bytes = doc.manifest.len()
            + doc.header_members.values().map(String::len).sum::<usize>();
        self.insert(key, Arc::clone(&index), bytes);
        Ok(index)
    }

    fn lookup(&self, key: &CacheKey) -> Option<Arc<HexonArchiveIndex>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.tick += 1;
        let tick = cache.tick;
        cache.slots.get_mut(key).map(|slot| {
            slot.used = tick;
            Arc::clone(&slot.index)
        })
    }

    fn insert(&self, key: CacheKey, index: Arc<HexonArchiveIndex>, bytes: usize) {
        if bytes > CACHE_MAX_BYTES {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // A changed file under the same path supersedes its old slot.
        let superseded: Vec<CacheKey> = cache
            .slots
            .keys()
            .filter(|k| k.0 == key.0)
            .cloned()
            .collect();
        for old in superseded {
            if let Some(slot) = cache.slots.remove(&old) {
                cache.bytes -= slot.bytes;
            }
        }
        while cache.slots.len() >= CACHE_MAX_ENTRIES || cache.bytes + bytes > CACHE_MAX_BYTES {
            let Some(oldest) = cache
                .slots
                .iter()
                .min_by_key(|(_, slot)| slot.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(slot) = cache.slots.remove(&oldest) {
                cache.bytes -= slot.bytes;
            }
        }
        cache.tick += 1;
        let used = cache.tick;
        cache.bytes += bytes;
        cache.slots.insert(key, Slot { index, bytes, used });
    }
}
