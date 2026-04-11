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
use std::sync::Mutex;

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
// IrohDocsReplicator — stub (E.4)
// ---------------------------------------------------------------------------

/// Stub implementation of `VerseReplicator` backed by an in-memory HashMap.
///
/// In a future phase this will connect to an `iroh_docs::Engine` replica. For
/// now it behaves identically to `MockVerseReplicator` but is typed separately
/// so production code can depend on it without importing test utilities.
pub struct IrohDocsReplicator {
    pub namespace_id: String,
    pub namespace_secret: String,
    inner: MockVerseReplicator,
}

impl IrohDocsReplicator {
    /// Create a new stub replicator for a given namespace.
    ///
    /// `author_id` is the local peer's DID / public key.
    pub fn new(namespace_id: String, namespace_secret: String, author_id: String) -> Self {
        Self {
            namespace_id,
            namespace_secret,
            inner: MockVerseReplicator::new(author_id),
        }
    }
}

impl VerseReplicator for IrohDocsReplicator {
    fn write_row(&self, table: &str, record_id: &str, data: &[u8]) -> anyhow::Result<()> {
        // TODO(Phase F): Use iroh_docs::Engine to write an entry with
        //   key = "{table}/{record_id}", value = blake3(data)
        //   and write the actual bytes to the blob store via iroh-blobs.
        tracing::debug!(
            ns = %self.namespace_id,
            key = %format!("{table}/{record_id}"),
            "IrohDocsReplicator::write_row (stub)"
        );
        self.inner.write_row(table, record_id, data)
    }

    fn subscribe(&self) -> anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>> {
        // TODO(Phase F): Subscribe to the iroh-docs replica event stream.
        self.inner.subscribe()
    }

    fn close(&self) -> anyhow::Result<()> {
        // TODO(Phase F): Close the iroh-docs replica handle.
        tracing::debug!(ns = %self.namespace_id, "IrohDocsReplicator::close (stub)");
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
        let repl = IrohDocsReplicator::new(
            "ns-id-hex".to_string(),
            "ns-secret-hex".to_string(),
            "local-author".to_string(),
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
}
