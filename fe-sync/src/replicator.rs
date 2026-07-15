//! VerseReplicator trait and implementations (P2P Mycelium Phase E).
//!
//! The `VerseReplicator` trait abstracts iroh-docs replica operations so that
//! the DB-to-sync bridge and subscriber loop can be tested against a mock
//! without a running iroh endpoint.
//!
//! We use `async fn in trait` (RPITIT, stabilised in Rust 1.75) to avoid the
//! `async-trait` proc-macro dependency.

use fe_runtime::blob_store::BlobHash;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// RowChange — a single replicated row event
// ---------------------------------------------------------------------------

/// Describes a single row change received from or published to a replica.
#[derive(Debug, Clone)]
pub struct RowChange {
    /// SurrealDB table name (e.g. "verse", "fractal", "petal", "node", "asset").
    pub table: String,
    /// SurrealDB record identifier (the ULID portion, not the `table:ulid` pair).
    pub record_id: String,
    /// BLAKE3 hash of the serialised row JSON stored in the blob store.
    pub content_hash: BlobHash,
    /// DID or public key identifying the author of the change.
    pub author_id: String,
    /// Lamport-style timestamp for ordering.
    pub timestamp: u64,
    /// If true this entry represents a deletion (tombstone).
    pub is_tombstone: bool,
}

// ---------------------------------------------------------------------------
// VerseReplicator trait
// ---------------------------------------------------------------------------

/// Abstraction over a per-verse iroh-docs replica.
///
/// Each open verse has exactly one `VerseReplicator` instance. The sync thread
/// manages the lifetime via `OpenVerseReplica` / `CloseVerseReplica` commands.
///
/// Implementations must be `Send + Sync` so they can be stored in the sync
/// thread's `HashMap<String, Box<dyn VerseReplicator>>`.
pub trait VerseReplicator: Send + Sync {
    /// Write (or overwrite) a row entry in the replica.
    ///
    /// The entry key is `"{table}/{record_id}"`. The value is the BLAKE3
    /// content hash of the serialised row JSON (the actual bytes live in the
    /// blob store, not in the replica).
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()>;

    /// Subscribe to incoming row changes from peers.
    ///
    /// Returns a receiver that yields `RowChange` events. The receiver is
    /// unbounded-ish (bounded to 1024 in the mock). Dropping the receiver
    /// unsubscribes.
    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>;

    /// Close the replica, flushing any pending state.
    fn close(&self) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// MockVerseReplicator — test double
// ---------------------------------------------------------------------------

/// In-memory mock of `VerseReplicator` for testing.
///
/// Stores entries in a `HashMap<String, Vec<u8>>` keyed by `"{table}/{record_id}"`.
/// Every `write_row` call broadcasts a `RowChange` to all active subscribers.
pub struct MockVerseReplicator {
    entries: Mutex<HashMap<String, Vec<u8>>>,
    subscribers: Mutex<Vec<tokio::sync::mpsc::Sender<RowChange>>>,
    author_id: String,
    closed: Mutex<bool>,
}

impl MockVerseReplicator {
    pub fn new(author_id: impl Into<String>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            subscribers: Mutex::new(Vec::new()),
            author_id: author_id.into(),
            closed: Mutex::new(false),
        }
    }

    /// Test helper: number of entries stored.
    pub fn entry_count(&self) -> usize {
        self.entries.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Test helper: check if a key exists.
    pub fn has_entry(&self, table: &str, record_id: &str) -> bool {
        let key = format!("{table}/{record_id}");
        self.entries
            .lock()
            .map(|m| m.contains_key(&key))
            .unwrap_or(false)
    }
}

impl VerseReplicator for MockVerseReplicator {
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()> {
        if *self.closed.lock().unwrap() {
            anyhow::bail!("MockVerseReplicator is closed");
        }

        let key = format!("{table}/{record_id}");
        let content_hash: BlobHash = *blake3::hash(data).as_bytes();

        self.entries
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
            .insert(key, data.to_vec());

        // Notify subscribers
        let change = RowChange {
            table: table.to_string(),
            record_id: record_id.to_string(),
            content_hash,
            author_id: self.author_id.clone(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            is_tombstone: false,
        };

        let mut subs = self
            .subscribers
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
        subs.retain(|tx| tx.try_send(change.clone()).is_ok());

        Ok(())
    }

    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>> {
        let (tx, rx) = tokio::sync::mpsc::channel(1024);
        self.subscribers
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
            .push(tx);
        Ok(rx)
    }

    fn close(&self) -> anyhow::Result<()> {
        *self.closed.lock().unwrap() = true;
        // Drop all subscriber senders
        self.subscribers
            .lock()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
            .clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IncomingEntryApplicator — subscriber-side framework (E.7-E.9)
// ---------------------------------------------------------------------------

/// Processes incoming `RowChange` events from a `VerseReplicator` subscriber.
///
/// Implements loop prevention (skip own writes) and timestamp tiebreaker
/// (lexicographic author comparison for equal timestamps).
pub struct IncomingEntryApplicator {
    /// The local author ID. Changes with this author are skipped (loop prevention).
    pub self_author_id: String,
}

impl IncomingEntryApplicator {
    pub fn new(self_author_id: impl Into<String>) -> Self {
        Self {
            self_author_id: self_author_id.into(),
        }
    }

    /// Decide whether an incoming `RowChange` should be applied locally.
    ///
    /// Returns `false` if:
    /// - The change was authored by us (loop prevention, E.8)
    /// - The change loses the tiebreaker against an existing entry
    pub fn should_apply(
        &self,
        change: &RowChange,
        local_timestamp: Option<u64>,
        local_author: Option<&str>,
    ) -> bool {
        // E.8: loop prevention — skip our own writes
        if change.author_id == self.self_author_id {
            return false;
        }

        // E.9: tiebreaker for concurrent writes
        if let (Some(lt), Some(la)) = (local_timestamp, local_author) {
            if change.timestamp < lt {
                return false; // remote is older
            }
            if change.timestamp == lt {
                // Equal timestamps: lexicographic comparison of author public key.
                // Higher author wins (deterministic, symmetric).
                return change.author_id.as_bytes() > la.as_bytes();
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// IrohDocsEngineHolder — holds the shared iroh-docs Engine (Phase F.1)
// ---------------------------------------------------------------------------

/// Placeholder holder for the iroh-docs Engine.
///
/// Real iroh-docs 0.35 `Engine<D>` wiring is deferred — the 0.35 `Engine::spawn`
/// requires a full P2P stack (gossip, blob store, downloader, local pool) that is
/// out of scope here. `is_available()` therefore stays `false` and replicators use
/// the in-memory fallback. See `fe-sync/src/AGENTS.md` §iroh-0.35.
#[derive(Default)]
pub struct IrohDocsEngineHolder {
    available: std::sync::atomic::AtomicBool,
}

impl IrohDocsEngineHolder {
    /// Create a new empty holder (real Engine wiring deferred to iroh-0.35 follow-up).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a real iroh-docs Engine is available. Currently always `false`.
    pub fn is_available(&self) -> bool {
        self.available.load(std::sync::atomic::Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// IrohDocsReplicator — backed by iroh-docs Engine (Phase F.1)
// ---------------------------------------------------------------------------

/// Implementation of `VerseReplicator` backed by iroh-docs.
///
/// Real iroh-docs 0.35 wiring is deferred (`Engine<D>` needs the full P2P stack);
/// until then this is backed by the in-memory mock. The `engine_holder` is carried
/// so the real path can be dropped in later without changing call sites.
/// See `fe-sync/src/AGENTS.md` §iroh-0.35.
pub struct IrohDocsReplicator {
    pub namespace_id: String,
    pub namespace_secret: String,
    /// The shared Engine holder (real Engine wiring deferred).
    engine_holder: Arc<IrohDocsEngineHolder>,
    /// In-memory backing store (fallback until iroh-docs is wired).
    inner: MockVerseReplicator,
}

impl IrohDocsReplicator {
    /// Create a new replicator for a given namespace.
    ///
    /// `author_id` is the local peer's DID / public key.
    pub fn new(
        namespace_id: String,
        namespace_secret: String,
        author_id: String,
        engine_holder: Arc<IrohDocsEngineHolder>,
    ) -> Self {
        Self {
            namespace_id,
            namespace_secret,
            engine_holder,
            inner: MockVerseReplicator::new(author_id),
        }
    }

    /// Open the document for this namespace.
    ///
    /// No-op until the real iroh-docs 0.35 Engine is wired.
    pub fn open_document(&self) -> anyhow::Result<()> {
        // TODO(iroh-0.35): open/create the iroh-docs document via Engine<D>.
        Ok(())
    }
}

impl VerseReplicator for IrohDocsReplicator {
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()> {
        // TODO(iroh-0.35): route through the real iroh-docs Doc when available.
        let backend = if self.engine_holder.is_available() {
            "iroh-docs"
        } else {
            "mock fallback"
        };
        tracing::debug!(ns = %self.namespace_id, key = %format!("{table}/{record_id}"), backend, "IrohDocsReplicator::write_row");
        self.inner.write_row(table, record_id, data)
    }

    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>> {
        // TODO(iroh-0.35): subscribe to real iroh-docs Doc events when available.
        tracing::debug!(ns = %self.namespace_id, "IrohDocsReplicator::subscribe (mock fallback)");
        self.inner.subscribe()
    }

    fn close(&self) -> anyhow::Result<()> {
        tracing::debug!(ns = %self.namespace_id, "IrohDocsReplicator::close");
        self.inner.close()
    }
}

// ---------------------------------------------------------------------------
// PetalReplicator trait — petal-level replication (Wave 2)
// ---------------------------------------------------------------------------

/// Abstraction over a per-petal iroh-docs replica.
///
/// Each subscribed petal has exactly one `PetalReplicator` instance. Unlike
/// `VerseReplicator` (verse-scoped), this replicates at petal granularity:
/// one iroh-docs namespace per petal.
///
/// Key encoding within the namespace: `/{table}/{record_id}`
/// (e.g., `/node/{node_id}`)
pub trait PetalReplicator: Send + Sync {
    /// Write (or overwrite) a row entry in the petal replica.
    ///
    /// The entry key is `"/{table}/{record_id}"`. The value is the content
    /// hash of the serialised row JSON.
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()>;

    /// Subscribe to incoming row changes from peers within this petal.
    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>;

    /// Close the replica, flushing any pending state.
    fn close(&self) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// IrohPetalReplicator — petal-level replication backed by iroh-docs
// ---------------------------------------------------------------------------

/// Petal-level replicator using iroh-docs 0.35.
///
/// Each petal gets its own iroh-docs namespace. Key encoding:
/// `/{table}/{record_id}` (e.g., `/node/{node_id}`).
///
/// Currently backed by an in-memory store (same as MockVerseReplicator) with
/// the petal-scoped interface. The iroh-docs wiring will connect in a future
/// phase once the iroh endpoint lifecycle is fully integrated.
pub struct IrohPetalReplicator {
    /// The petal ID this replicator is responsible for.
    pub petal_id: String,
    /// The iroh-docs namespace ID (derived from petal_id).
    pub namespace_id: String,
    inner: MockVerseReplicator,
}

impl IrohPetalReplicator {
    /// Create a new petal replicator.
    ///
    /// `petal_id` — the petal this replica covers.
    /// `namespace_id` — derived namespace ID for the iroh-docs document.
    /// `author_id` — the local peer's DID / public key.
    pub fn new(petal_id: String, namespace_id: String, author_id: String) -> Self {
        Self {
            petal_id,
            namespace_id,
            inner: MockVerseReplicator::new(author_id),
        }
    }

    /// Resolve an HLC conflict: returns `true` if remote should win.
    ///
    /// Rules:
    /// - If remote HLC > local HLC for the same (node_id, key): apply remote
    /// - If remote HLC == local HLC: higher author_id wins (lexicographic)
    /// - If remote HLC < local HLC: discard remote
    pub fn should_apply_remote(
        remote_hlc: u64,
        local_hlc: u64,
        remote_author: &str,
        local_author: &str,
    ) -> bool {
        if remote_hlc > local_hlc {
            return true;
        }
        if remote_hlc == local_hlc {
            return remote_author.as_bytes() > local_author.as_bytes();
        }
        false
    }

    /// Encode a key for the iroh-docs namespace.
    ///
    /// Format: `/{table}/{record_id}`
    pub fn encode_key(table: &str, record_id: &str) -> String {
        format!("/{table}/{record_id}")
    }
}

impl PetalReplicator for IrohPetalReplicator {
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()> {
        tracing::debug!(
            petal = %self.petal_id,
            ns = %self.namespace_id,
            key = %Self::encode_key(table, record_id),
            "IrohPetalReplicator::write_row"
        );
        self.inner.write_row(table, record_id, data)
    }

    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>> {
        self.inner.subscribe()
    }

    fn close(&self) -> anyhow::Result<()> {
        tracing::debug!(petal = %self.petal_id, ns = %self.namespace_id, "IrohPetalReplicator::close");
        self.inner.close()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_replicator_write_and_count() {
        let mock = MockVerseReplicator::new("author-a");
        mock.write_row("verse", "v1", b"{\"name\":\"test\"}")
            .unwrap();
        assert_eq!(mock.entry_count(), 1);
        assert!(mock.has_entry("verse", "v1"));
        assert!(!mock.has_entry("verse", "v2"));
    }

    #[test]
    fn mock_replicator_close_rejects_writes() {
        let mock = MockVerseReplicator::new("author-a");
        mock.close().unwrap();
        assert!(mock.write_row("verse", "v1", b"{}").is_err());
    }

    #[test]
    fn mock_replicator_subscribe_receives_changes() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mock = MockVerseReplicator::new("author-a");
            let mut rx = mock.subscribe().unwrap();
            mock.write_row("fractal", "f1", b"{\"name\":\"frac\"}")
                .unwrap();
            let change = rx.try_recv().unwrap();
            assert_eq!(change.table, "fractal");
            assert_eq!(change.record_id, "f1");
            assert_eq!(change.author_id, "author-a");
            assert!(!change.is_tombstone);
        });
    }

    #[test]
    fn iroh_docs_replicator_stub_works() {
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let repl = IrohDocsReplicator::new(
            "ns-id-hex".to_string(),
            "ns-secret-hex".to_string(),
            "local-author".to_string(),
            engine_holder,
        );
        repl.write_row("verse", "v1", b"{\"name\":\"test\"}")
            .unwrap();
        assert_eq!(repl.inner.entry_count(), 1);
        repl.close().unwrap();
    }

    // --- IncomingEntryApplicator tests (E.7-E.9) ---

    fn make_change(author: &str, ts: u64) -> RowChange {
        RowChange {
            table: "verse".to_string(),
            record_id: "v1".to_string(),
            content_hash: [0u8; 32],
            author_id: author.to_string(),
            timestamp: ts,
            is_tombstone: false,
        }
    }

    #[test]
    fn loop_prevention_skips_own_writes() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-a", 100);
        assert!(!applicator.should_apply(&change, None, None));
    }

    #[test]
    fn applies_remote_writes() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 100);
        assert!(applicator.should_apply(&change, None, None));
    }

    #[test]
    fn newer_remote_wins() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 200);
        assert!(applicator.should_apply(&change, Some(100), Some("author-a")));
    }

    #[test]
    fn older_remote_loses() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 50);
        assert!(!applicator.should_apply(&change, Some(100), Some("author-a")));
    }

    #[test]
    fn equal_timestamp_higher_author_wins() {
        let applicator = IncomingEntryApplicator::new("author-a");
        // "author-b" > "author-a" lexicographically, so remote wins
        let change = make_change("author-b", 100);
        assert!(applicator.should_apply(&change, Some(100), Some("author-a")));

        // "author-a" < "author-c" so if local is "author-c", remote loses
        let applicator2 = IncomingEntryApplicator::new("author-z");
        let change2 = make_change("author-b", 100);
        assert!(!applicator2.should_apply(&change2, Some(100), Some("author-z")));
    }

    // --- IrohPetalReplicator tests ---

    #[test]
    fn petal_replicator_write_and_subscribe() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let repl = IrohPetalReplicator::new(
                "petal-1".to_string(),
                "ns-petal-1".to_string(),
                "local-author".to_string(),
            );
            let mut rx = repl.subscribe().unwrap();
            repl.write_row("node", "n1", b"{\"name\":\"test\"}")
                .unwrap();
            let change = rx.try_recv().unwrap();
            assert_eq!(change.table, "node");
            assert_eq!(change.record_id, "n1");
        });
    }

    #[test]
    fn petal_replicator_close_rejects_writes() {
        let repl = IrohPetalReplicator::new(
            "petal-2".to_string(),
            "ns-petal-2".to_string(),
            "local-author".to_string(),
        );
        repl.close().unwrap();
        assert!(repl.write_row("node", "n1", b"{}").is_err());
    }

    #[test]
    fn petal_key_encoding() {
        assert_eq!(
            IrohPetalReplicator::encode_key("node", "abc123"),
            "/node/abc123"
        );
    }

    #[test]
    fn hlc_conflict_resolution() {
        // Remote is newer — apply
        assert!(IrohPetalReplicator::should_apply_remote(
            200, 100, "author-b", "author-a"
        ));
        // Remote is older — discard
        assert!(!IrohPetalReplicator::should_apply_remote(
            50, 100, "author-b", "author-a"
        ));
        // Equal HLC, higher author wins
        assert!(IrohPetalReplicator::should_apply_remote(
            100, 100, "author-b", "author-a"
        ));
        assert!(!IrohPetalReplicator::should_apply_remote(
            100, 100, "author-a", "author-b"
        ));
    }
}
