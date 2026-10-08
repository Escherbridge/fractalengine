//! VerseReplicator trait and implementations (P2P Mycelium Phase E).
//!
//! The `VerseReplicator` trait abstracts iroh-docs replica operations so that
//! the DB-to-sync bridge and subscriber loop can be tested against a mock
//! without a running iroh endpoint.
//!
//! The trait is async. To keep it dyn-compatible (`Box<dyn VerseReplicator>`
//! in the sync thread) without the `async-trait` proc-macro dependency, the
//! methods return hand-boxed futures ([`ReplicatorFuture`]) — the same
//! desugaring `async-trait` performs, minus the dependency.
//!
//! `IrohDocsReplicator` rides the real iroh-docs 0.35 `Doc` client
//! (`set_bytes` / `subscribe` / `del` / `close`) whenever the P2P stack is
//! online, and degrades to the in-memory [`MockVerseReplicator`] only when
//! it is not (offline bind failure, stack spawn failure). On an ONLINE
//! stack, a document-open failure leaves a loudly non-replicating replica
//! (writes warn and fail, `snapshot`/`subscribe` error — F20 finding 4),
//! never a usable in-memory success path. See `fe-sync/src/AGENTS.md`
//! §iroh-0.35.
//!
//! **One authenticated author identity (F20 finding 2):** the docs author
//! is the endpoint identity (imported + made default at
//! `DocsStack::spawn`), so `entry.author()` maps to the same `did:key` the
//! A3 gate resolves. Live delivery and snapshot replay attribute rows from
//! `entry.author()` identically; `InsertRemote.from` is only the
//! forwarding neighbor.

use fe_runtime::blob_store::BlobHash;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use bytes::Bytes;
use futures_lite::{Stream, StreamExt};
use iroh_blobs::store::fs::Store as FsBlobStore;
use iroh_docs::engine::LiveEvent;
use iroh_docs::rpc::client::docs::Doc;
use iroh_docs::{AuthorId, Capability, NamespaceId, NamespaceSecret};
use tokio::sync::mpsc;

/// Boxed async return for the dyn-compatible trait methods.
pub type ReplicatorFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// The in-process docs client connector — the transport `MemClient` rides.
type DocsFlumeConnector = quic_rpc::transport::flume::FlumeConnector<
    iroh_docs::rpc::proto::Response,
    iroh_docs::rpc::proto::Request,
>;

/// A live iroh-docs document handle as handed out by the in-process client.
pub type DocHandle = Doc<DocsFlumeConnector>;

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
    /// The entry's timestamp (iroh-docs entries carry microsecond wall time).
    pub timestamp: u64,
    /// If true this entry represents a deletion (tombstone).
    pub is_tombstone: bool,
    /// The row's payload bytes so the inbound apply path never has to re-read
    /// a blob store (A4: the DB thread applies what the wire delivered).
    pub data: Vec<u8>,
}

/// Whether a serialized row JSON represents a tombstoned (soft-deleted) node —
/// i.e. it carries a non-null `tombstone` field (FR-1), or the payload is
/// empty (the iroh-docs `del` marker — an empty entry is a deletion). Used to
/// set [`RowChange::is_tombstone`] so the merge path honors deletes and never
/// resurrects a tombstoned node (N-4).
pub fn row_is_tombstone(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    serde_json::from_slice::<serde_json::Value>(data)
        .ok()
        .and_then(|v| v.get("tombstone").map(|t| !t.is_null()))
        .unwrap_or(false)
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
    /// The entry key is `"{table}/{record_id}"`. The value is the row payload
    /// bytes (`Doc::set_bytes`); an empty `data` writes a deletion marker
    /// (`Doc::del`).
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>>;

    /// Subscribe to incoming row changes from peers.
    ///
    /// Returns a receiver that yields `RowChange` events. Dropping the
    /// receiver unsubscribes.
    fn subscribe(&self) -> ReplicatorFuture<'_, anyhow::Result<mpsc::Receiver<RowChange>>>;

    /// Enumerate the replica's **current** entries (latest per key) as
    /// [`RowChange`]s — the startup reconciliation pass (F4).
    ///
    /// A row that was denied by the inbound role gate while a role or the
    /// verse manifest had not yet converged stays in the doc store but never
    /// re-fires as an `InsertRemote` event, so it would be lost forever. On
    /// every replica open the sync thread replays this snapshot through the
    /// **same** inbound apply path, giving those rows a second chance to
    /// converge. Own-author rows are filtered downstream like any other
    /// inbound row.
    ///
    /// Entries whose content has not finished downloading are omitted (they
    /// arrive through the live pump when the content lands).
    fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>>;

    /// Close the replica, flushing any pending state.
    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>>;
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
    subscribers: Mutex<Vec<mpsc::Sender<RowChange>>>,
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
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        let table = table.to_string();
        let record_id = record_id.to_string();
        let data = data.to_vec();
        Box::pin(async move {
            if *self.closed.lock().unwrap() {
                anyhow::bail!("MockVerseReplicator is closed");
            }

            let key = format!("{table}/{record_id}");
            let content_hash: BlobHash = *blake3::hash(&data).as_bytes();

            self.entries
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                .insert(key, data.clone());

            // Notify subscribers. `is_tombstone` is derived from the row content so
            // a soft-deleted node propagates as a tombstone the merge path honors
            // (N-4) — previously hardcoded `false`, which silently dropped deletes.
            let change = RowChange {
                table,
                record_id,
                content_hash,
                author_id: self.author_id.clone(),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as u64,
                is_tombstone: row_is_tombstone(&data),
                data,
            };

            let mut subs = self
                .subscribers
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;
            subs.retain(|tx| tx.try_send(change.clone()).is_ok());

            Ok(())
        })
    }

    fn subscribe(&self) -> ReplicatorFuture<'_, anyhow::Result<mpsc::Receiver<RowChange>>> {
        Box::pin(async move {
            let (tx, rx) = mpsc::channel(1024);
            self.subscribers
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                .push(tx);
            Ok(rx)
        })
    }

    fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        Box::pin(async move {
            let entries = self
                .entries
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                .clone();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64;
            let mut out = Vec::with_capacity(entries.len());
            for (key, data) in entries {
                let Some((table, record_id)) = key.split_once('/') else {
                    continue;
                };
                out.push(RowChange {
                    table: table.to_string(),
                    record_id: record_id.to_string(),
                    content_hash: *blake3::hash(&data).as_bytes(),
                    author_id: self.author_id.clone(),
                    timestamp: now,
                    is_tombstone: row_is_tombstone(&data),
                    data,
                });
            }
            Ok(out)
        })
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            *self.closed.lock().unwrap() = true;
            // Drop all subscriber senders
            self.subscribers
                .lock()
                .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                .clear();
            Ok(())
        })
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
    /// - The change would resurrect a locally-tombstoned node (N-4)
    /// - The change loses the tiebreaker against an existing entry
    ///
    /// `local_is_tombstoned` is whether the local row for this record is already
    /// soft-deleted (FR-1). A tombstone is never LWW: a delete dominates a
    /// concurrent live write and can never be resurrected by one (D-A7/N-4).
    pub fn should_apply(
        &self,
        change: &RowChange,
        local_timestamp: Option<u64>,
        local_author: Option<&str>,
        local_is_tombstoned: bool,
    ) -> bool {
        // E.8: loop prevention — skip our own writes
        if change.author_id == self.self_author_id {
            return false;
        }

        // N-4 tombstone dominance (independent of timestamps / LWW):
        if local_is_tombstoned && !change.is_tombstone {
            // Never resurrect a tombstoned node with a stale live write.
            return false;
        }
        if change.is_tombstone && !local_is_tombstoned {
            // A delete always wins over a concurrent live local row.
            return true;
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
// IrohDocsEngineHolder — holds the real iroh-docs stack (Phase F.1, A1)
// ---------------------------------------------------------------------------

/// Holder for the real iroh-docs 0.35 P2P stack
/// ([`crate::docs_engine::DocsStack`]: Blobs + Gossip + Docs + Router).
///
/// The sync thread constructs it `online` when the endpoint bound and the
/// full stack spawned, and leaves it empty in offline mode (bind failure or
/// stack spawn failure) — `is_available()` then reports `false` and
/// replicators degrade to the in-memory mock without crashing.
/// See `fe-sync/src/AGENTS.md` §iroh-0.35.
#[derive(Default)]
pub struct IrohDocsEngineHolder {
    stack: Option<Arc<crate::docs_engine::DocsStack>>,
}

impl IrohDocsEngineHolder {
    /// Create an empty holder (offline — no stack, replicators use the mock).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a holder around a successfully spawned stack (online).
    pub fn online(stack: Arc<crate::docs_engine::DocsStack>) -> Self {
        Self { stack: Some(stack) }
    }

    /// Whether the real iroh-docs stack is available.
    pub fn is_available(&self) -> bool {
        self.stack.is_some()
    }

    /// The live stack, when online.
    pub fn stack(&self) -> Option<&crate::docs_engine::DocsStack> {
        self.stack.as_deref()
    }

    /// Clone of the docs RPC client, when online.
    ///
    /// The handle the replicator layer uses for `import_namespace` /
    /// `set_bytes` / `subscribe` / `start_sync` / `close` — the real
    /// Doc-backed path rides this seam.
    pub fn docs_client(&self) -> Option<iroh_docs::rpc::client::docs::MemClient> {
        self.stack.as_ref().map(|s| s.docs_client())
    }
}

// ---------------------------------------------------------------------------
// IrohDocsReplicator — backed by the real iroh-docs Doc client (A2)
// ---------------------------------------------------------------------------

/// Implementation of `VerseReplicator` backed by iroh-docs 0.35.
///
/// Online (the stack spawned and the namespace capability usable) every
/// operation rides the real `Doc` handle: `write_row` → `set_bytes` (or `del`
/// for empty payloads), `subscribe` → a `LiveEvent` pump that maps
/// `InsertRemote`/`ContentReady` into `RowChange`s carrying the payload bytes
/// read from the blobs store, `close` → `Doc::close`.
///
/// Offline or when the namespace capability cannot be imported, operations
/// fall back to the in-memory mock — degradation is total, never a crash.
pub struct IrohDocsReplicator {
    /// The verse this replica serves (used by the secretless-open fallback
    /// to find the right persisted doc when the stored namespace id misses).
    pub verse_id: String,
    pub namespace_id: String,
    pub namespace_secret: String,
    /// The shared stack holder (online or offline/mock).
    engine_holder: Arc<IrohDocsEngineHolder>,
    /// In-memory backing store — the offline fallback.
    inner: MockVerseReplicator,
    /// Live Doc handle when the namespace opened on the real stack.
    doc: RwLock<Option<DocHandle>>,
    /// The node's default author for `set_bytes` writes.
    author: RwLock<Option<AuthorId>>,
    /// Abort handle of the live-event pump spawned by the last `subscribe`.
    pump: Mutex<Option<tokio::task::AbortHandle>>,
    /// Why `open_document` failed **on an online stack** (F20 finding 4).
    ///
    /// Set by the sync thread's open handler via [`Self::mark_open_failed`]:
    /// a failure while the stack is ONLINE must leave a loud,
    /// NON-replicating replica (writes fail with a warn, `snapshot` and
    /// `subscribe` error) — never a usable in-memory mock success path.
    /// `None` offline (the offline mock fallback is sanctioned) and on
    /// successful opens.
    open_failed: RwLock<Option<String>>,
}

impl IrohDocsReplicator {
    /// Create a new replicator for a given verse namespace.
    ///
    /// `author_id` is the local peer's DID / public key (the offline mock's
    /// author). The document itself is opened by [`Self::open_document`].
    pub fn new(
        verse_id: String,
        namespace_id: String,
        namespace_secret: String,
        author_id: String,
        engine_holder: Arc<IrohDocsEngineHolder>,
    ) -> Self {
        Self {
            verse_id,
            namespace_id,
            namespace_secret,
            engine_holder,
            inner: MockVerseReplicator::new(author_id),
            doc: RwLock::new(None),
            author: RwLock::new(None),
            pump: Mutex::new(None),
            open_failed: RwLock::new(None),
        }
    }

    /// Whether the real Doc-backed path is active (stack online + doc open).
    pub fn is_doc_backed(&self) -> bool {
        self.doc.read().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// Record a failed document open on an ONLINE stack (F20 finding 4).
    ///
    /// Marks the replica loudly non-replicating: every later write warns and
    /// fails, `subscribe`/`snapshot` error. The sync thread calls this only
    /// when `IrohDocsEngineHolder::is_available()` — offline stacks keep the
    /// sanctioned in-memory mock fallback.
    pub fn mark_open_failed(&self, reason: String) {
        *self.open_failed.write().unwrap_or_else(|e| e.into_inner()) = Some(reason);
    }

    /// The recorded online-open failure, if any (see [`Self::mark_open_failed`]).
    pub fn open_error(&self) -> Option<String> {
        self.open_failed
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn live_doc(&self) -> Option<DocHandle> {
        self.doc.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn live_author(&self) -> Option<AuthorId> {
        *self.author.read().unwrap_or_else(|e| e.into_inner())
    }

    /// Open the document for this namespace on the real stack.
    ///
    /// A non-empty `namespace_secret` is validated **eagerly** — an
    /// unparseable secret surfaces as an error even when the stack is
    /// offline, so a bad config is never silently deferred past startup.
    /// With a valid secret the verse namespace is imported as a **write
    /// capability** (`Capability::Write(NamespaceSecret)`); without one the
    /// namespace id is opened read-only (`Client::open` — a previously
    /// imported or ticket-joined replica).
    ///
    /// **Secretless reopen fallback (F20/M1 finding 3):** production
    /// `verse.namespace_id` values used to be a keyed-BLAKE3 digest that can
    /// never equal the iroh namespace id (the Ed25519 public key the secret
    /// derives to), so a stored-id miss must not end the open. On a miss the
    /// client scans the node's **known** docs for the one carrying this
    /// verse's manifest row (`verse/{verse_id}`) and reopens that doc
    /// read-only — the compatibility path for BLAKE3 values in live DBs.
    /// New verses store the aligned id (`fe_database::derive_namespace_id`
    /// now returns the Ed25519 id), which `Client::open` resolves directly.
    ///
    /// Offline stacks return `Ok(())` with no doc (the sanctioned mock
    /// fallback). An error return while online is recorded by the caller as
    /// a loud non-replicating replica state — never a usable mock.
    pub async fn open_document(&self) -> anyhow::Result<()> {
        if !self.namespace_secret.is_empty() {
            // Surface config errors eagerly, stack or no stack.
            parse_secret_hex(&self.namespace_secret)?;
        }
        let Some(client) = self.engine_holder.docs_client() else {
            tracing::debug!(
                ns = %self.namespace_id,
                "P2P stack offline — replica stays on the in-memory mock"
            );
            return Ok(());
        };

        let author = client
            .authors()
            .default()
            .await
            .map_err(|e| anyhow::anyhow!("resolving default author: {e}"))?;

        let doc = if !self.namespace_secret.is_empty() {
            let secret_bytes = parse_secret_hex(&self.namespace_secret)?;
            let secret = NamespaceSecret::from_bytes(&secret_bytes);
            let doc = client
                .import_namespace(Capability::Write(secret))
                .await
                .map_err(|e| anyhow::anyhow!("importing namespace capability: {e}"))?;
            tracing::info!(
                ns = %self.namespace_id,
                iroh_ns = %doc.id().fmt_short(),
                mode = "write-capability",
                "Opened verse replica document on the real iroh-docs stack"
            );
            doc
        } else {
            let ns = parse_namespace_id(&self.namespace_id)?;
            match client.open(ns).await {
                Ok(Some(doc)) => {
                    tracing::info!(
                        ns = %self.namespace_id,
                        mode = "read-only",
                        "Opened verse replica document on the real iroh-docs stack"
                    );
                    doc
                }
                Ok(None) | Err(_) => {
                    // Stored-id miss (legacy BLAKE3 id, or a doc imported
                    // under a different id form): scan known docs for the
                    // one carrying this verse's manifest row.
                    match find_doc_by_verse_manifest(&client, &self.verse_id).await {
                        Some(doc) => {
                            tracing::info!(
                                stored_ns = %self.namespace_id,
                                iroh_ns = %doc.id().fmt_short(),
                                mode = "read-only (stored-id miss → manifest scan)",
                                "Reopened the verse's persisted document by its manifest"
                            );
                            doc
                        }
                        None => anyhow::bail!(
                            "namespace {ns} is not a known local replica and no \
                             known doc carries the manifest for verse {}",
                            self.verse_id
                        ),
                    }
                }
            }
        };

        *self.doc.write().unwrap_or_else(|e| e.into_inner()) = Some(doc);
        *self.author.write().unwrap_or_else(|e| e.into_inner()) = Some(author);
        Ok(())
    }

    /// Join the live sync swarm for this replica, dialing `peers`.
    ///
    /// No-op on the mock fallback (no document to sync).
    pub async fn start_sync(&self, peers: Vec<iroh::NodeAddr>) -> anyhow::Result<()> {
        let Some(doc) = self.live_doc() else {
            return Ok(());
        };
        doc.start_sync(peers)
            .await
            .map_err(|e| anyhow::anyhow!("start_sync: {e}"))
    }

    async fn write_row_inner(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> anyhow::Result<()> {
        // F20 finding 4: a replica whose document open FAILED on an online
        // stack is loudly non-replicating — a write here must warn and fail,
        // never "succeed" against the in-memory mock (which would publish
        // nothing while looking healthy).
        if let Some(reason) = self.open_error() {
            tracing::warn!(
                verse_id = %self.verse_id,
                key = %format!("{table}/{record_id}"),
                reason = %reason,
                "WriteRowEntry on a NON-replicating replica (open failed) — not written"
            );
            anyhow::bail!("replica is not replicating: document open failed: {reason}");
        }
        let Some(doc) = self.live_doc() else {
            tracing::debug!(
                ns = %self.namespace_id,
                key = %format!("{table}/{record_id}"),
                "write_row on mock fallback (doc unavailable)"
            );
            return VerseReplicator::write_row(&self.inner, table, record_id, data).await;
        };
        let Some(author) = self.live_author() else {
            anyhow::bail!("document open but no author resolved");
        };

        let key = format!("{table}/{record_id}");
        if data.is_empty() {
            // Empty payload = deletion marker (tombstone semantics, `Doc::del`
            // inserts an empty entry — detected by `row_is_tombstone`).
            let removed = doc
                .del(author, key.as_bytes().to_vec())
                .await
                .map_err(|e| anyhow::anyhow!("del {key}: {e}"))?;
            tracing::debug!(ns = %doc.id().fmt_short(), key = %key, removed, "tombstone written");
        } else {
            doc.set_bytes(author, key.as_bytes().to_vec(), data.to_vec())
                .await
                .map_err(|e| anyhow::anyhow!("set_bytes {key}: {e}"))?;
            tracing::debug!(ns = %doc.id().fmt_short(), key = %key, "row written");
        }
        Ok(())
    }
}

impl VerseReplicator for IrohDocsReplicator {
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        // Own the params inside the boxed future so the returned future
        // borrows only `self` — the trait's `ReplicatorFuture<'_>` ties to
        // `&self` and the param lifetimes may be shorter.
        let table = table.to_string();
        let record_id = record_id.to_string();
        let data = data.to_vec();
        Box::pin(async move { self.write_row_inner(&table, &record_id, &data).await })
    }

    fn subscribe(&self) -> ReplicatorFuture<'_, anyhow::Result<mpsc::Receiver<RowChange>>> {
        Box::pin(async move {
            // F20 finding 4: no live pump for a failed online open — there is
            // no document to receive events from, and a mock pump would
            // fabricate an inbound stream that applies nothing real.
            if let Some(reason) = self.open_error() {
                anyhow::bail!("replica is not replicating: document open failed: {reason}");
            }
            let Some(doc) = self.live_doc() else {
                tracing::debug!(ns = %self.namespace_id, "subscribe on mock fallback");
                return VerseReplicator::subscribe(&self.inner).await;
            };
            let Some(stack) = self.engine_holder.stack() else {
                anyhow::bail!("document open but stack unavailable");
            };

            let stream = doc
                .subscribe()
                .await
                .map_err(|e| anyhow::anyhow!("doc subscribe: {e}"))?;
            let (tx, rx) = mpsc::channel::<RowChange>(1024);
            let handle = tokio::spawn(pump_live_events(
                Box::pin(stream),
                stack.blobs().store().clone(),
                tx,
            ));
            *self.pump.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle.abort_handle());
            Ok(rx)
        })
    }

    fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        Box::pin(async move {
            // F20 finding 4: a failed online open has no document to
            // snapshot — error loudly rather than replaying mock rows.
            if let Some(reason) = self.open_error() {
                anyhow::bail!("replica is not replicating: document open failed: {reason}");
            }
            let Some(doc) = self.live_doc() else {
                tracing::debug!(ns = %self.namespace_id, "snapshot on mock fallback");
                return VerseReplicator::snapshot(&self.inner).await;
            };
            let Some(stack) = self.engine_holder.stack() else {
                anyhow::bail!("document open but stack unavailable");
            };
            let blobs = stack.blobs().store().clone();

            // Latest entry per key (a key may have been written by several
            // authors); `include_empty` keeps `Doc::del` tombstones in the set.
            let query = iroh_docs::store::Query::single_latest_per_key()
                .include_empty()
                .build();
            let stream = doc
                .get_many(query)
                .await
                .map_err(|e| anyhow::anyhow!("get_many for reconciliation: {e}"))?;
            let mut stream = std::pin::pin!(stream);

            let mut out = Vec::new();
            while let Some(entry) = stream.next().await {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        tracing::warn!(ns = %self.namespace_id, "reconciliation query entry failed: {e}");
                        continue;
                    }
                };
                let key = String::from_utf8_lossy(entry.key()).into_owned();
                let Some((table, record_id)) = key.split_once('/') else {
                    tracing::warn!(key = %key, "replica entry key not in table/id form — skipped");
                    continue;
                };
                let meta = (
                    table.to_string(),
                    record_id.to_string(),
                    *entry.content_hash().as_bytes(),
                    author_did_key(&entry.author()),
                    entry.timestamp(),
                );
                if entry.record().is_empty() {
                    // Deletion marker: no content to read.
                    out.push(RowChange {
                        table: meta.0,
                        record_id: meta.1,
                        content_hash: meta.2,
                        author_id: meta.3,
                        timestamp: meta.4,
                        is_tombstone: true,
                        data: Vec::new(),
                    });
                    continue;
                }
                match read_blob_bytes(&blobs, &entry.content_hash()).await {
                    Ok(data) => {
                        let data = data.to_vec();
                        out.push(RowChange {
                            table: meta.0,
                            record_id: meta.1,
                            content_hash: meta.2,
                            author_id: meta.3,
                            timestamp: meta.4,
                            is_tombstone: row_is_tombstone(&data),
                            data,
                        });
                    }
                    Err(e) => {
                        // Content not local yet — the live pump emits it when
                        // the download completes, so nothing is lost.
                        tracing::debug!(
                            key = %key,
                            "reconciliation snapshot: content not local — deferred to live pump: {e}"
                        );
                    }
                }
            }
            Ok(out)
        })
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            // Stop the inbound pump first so no further rows are emitted.
            if let Some(handle) = self.pump.lock().unwrap_or_else(|e| e.into_inner()).take() {
                handle.abort();
            }
            // Take the doc out of the write lock BEFORE awaiting — an
            // `RwLockWriteGuard` held across an await makes the future
            // non-Send.
            let doc = self.doc.write().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(doc) = doc {
                if let Err(e) = doc.close().await {
                    tracing::warn!(ns = %self.namespace_id, "Doc close failed: {e}");
                }
                *self.author.write().unwrap_or_else(|e| e.into_inner()) = None;
            }
            VerseReplicator::close(&self.inner).await
        })
    }
}

/// Parse a hex-encoded 32-byte namespace secret.
fn parse_secret_hex(secret: &str) -> anyhow::Result<[u8; 32]> {
    hex::decode(secret.trim())
        .map_err(|e| anyhow::anyhow!("namespace secret is not valid hex: {e}"))
        .and_then(|bytes| {
            bytes.try_into().map_err(|v: Vec<u8>| {
                anyhow::anyhow!("namespace secret must be 32 bytes, got {}", v.len())
            })
        })
}

/// Parse a hex-encoded iroh-docs `NamespaceId`.
fn parse_namespace_id(ns_hex: &str) -> anyhow::Result<NamespaceId> {
    use std::str::FromStr;
    NamespaceId::from_str(ns_hex.trim()).map_err(|e| anyhow::anyhow!("invalid namespace id: {e}"))
}

/// Find the known doc that carries a verse's manifest row (F20/M1 finding 3).
///
/// The secretless reopen compatibility path: legacy `verse.namespace_id`
/// values are keyed-BLAKE3 digests that never equal the iroh namespace id
/// the doc actually registered under, so a stored-id miss resolves the doc
/// by content instead — the one whose `verse/{verse_id}` key exists. Scan
/// cost is one local `get_one` per known doc (all reads against the local
/// redb store, no network). `None` when no known doc claims the verse.
async fn find_doc_by_verse_manifest(
    client: &iroh_docs::rpc::client::docs::MemClient,
    verse_id: &str,
) -> Option<DocHandle> {
    let manifest_key = format!("verse/{verse_id}");
    let mut stream = client.list().await.ok()?;
    while let Some(item) = stream.next().await {
        let (doc_id, _capability) = match item {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(verse_id, "doc list entry failed during manifest scan: {e}");
                continue;
            }
        };
        let Ok(doc) = client.open(doc_id).await else {
            continue;
        };
        let Some(doc) = doc else { continue };
        let query = iroh_docs::store::Query::key_exact(manifest_key.as_bytes()).build();
        match doc.get_one(query).await {
            Ok(Some(_)) => return Some(doc),
            Ok(None) => continue,
            Err(e) => {
                // An unreadable doc must not abort the scan of the rest.
                tracing::warn!(
                    verse_id,
                    ns = %doc_id.fmt_short(),
                    "manifest probe failed during doc scan: {e}"
                );
                continue;
            }
        }
    }
    None
}

/// An entry whose content has not finished downloading yet.
#[derive(Debug, Clone)]
struct PendingContent {
    table: String,
    record_id: String,
    content_hash: BlobHash,
    author_id: String,
    timestamp: u64,
}

/// Map an iroh peer public key to the app's `did:key` identity form.
///
/// The endpoint identity's `NodeId` did:key IS the app DID (fe-identity's
/// keypair and the iroh secret share the ed25519 seed), which is what makes
/// author attribution and the A3 role gate agree across peers.
/// `pub(crate)` since F23: the distributed-query transport uses the same
/// mapping to verify a gossip envelope's claimed `from_did` against the
/// authenticated direct sender.
pub(crate) fn peer_did_key(peer: &iroh::PublicKey) -> String {
    fe_identity::did_key::did_key_from_public_key_bytes(peer.as_bytes())
        .unwrap_or_else(|| peer.to_string())
}

/// Map an iroh-docs entry `AuthorId` to the app's `did:key` identity form.
///
/// Snapshot entries and live entries both attribute authorship from
/// `entry.author()` (the key that signed the entry — the endpoint identity,
/// since `DocsStack::spawn` imports it as the docs author); both are
/// ed25519 keys over the same identity seed, so the same did:key mapping
/// applies. Never confuse the author with `InsertRemote.from`, which names
/// only the forwarding neighbor.
fn author_did_key(author: &AuthorId) -> String {
    fe_identity::did_key::did_key_from_public_key_bytes(author.as_bytes())
        .unwrap_or_else(|| author.to_string())
}

/// Read one blob's full content bytes from the iroh-blobs fs store.
async fn read_blob_bytes(store: &FsBlobStore, hash: &iroh_blobs::Hash) -> anyhow::Result<Bytes> {
    use iroh_blobs::store::Map;
    use iroh_io::AsyncSliceReaderExt;

    let entry = store
        .get(hash)
        .await
        .map_err(|e| anyhow::anyhow!("blob store lookup: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("blob {} not in the local store", hash))?;
    if !entry.is_complete() {
        anyhow::bail!("blob {} is incomplete", hash);
    }
    // The fs store's `data_reader` is a synchronous inherent method.
    let mut reader = entry.data_reader();
    reader
        .read_to_end()
        .await
        .map_err(|e| anyhow::anyhow!("reading blob bytes: {e}"))
}

/// The per-replica inbound event pump: maps the iroh-docs `LiveEvent` stream
/// to `RowChange`s carrying the payload bytes.
///
/// - `InsertLocal` is skipped — our own writes must never loop back (E.8).
/// - `InsertRemote` entries whose content is already local are emitted
///   immediately; entries whose content is still downloading are stashed until
///   `ContentReady` fires for their hash.
/// - Empty entries (`Doc::del` markers) carry no content; they are emitted
///   immediately as tombstones.
async fn pump_live_events(
    mut stream: std::pin::Pin<Box<dyn Stream<Item = anyhow::Result<LiveEvent>> + Send>>,
    blobs: FsBlobStore,
    out: mpsc::Sender<RowChange>,
) {
    // Entries whose content hash has not finished downloading yet.
    let mut pending: HashMap<iroh_blobs::Hash, Vec<PendingContent>> = HashMap::new();

    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                tracing::warn!("replica event stream error: {e}");
                continue;
            }
        };
        match event {
            LiveEvent::InsertLocal { entry } => {
                // Loop prevention at the source: our own write echoing back.
                tracing::debug!(key = ?String::from_utf8_lossy(entry.key()), "local insert skipped");
            }
            LiveEvent::InsertRemote {
                entry,
                content_status,
                from,
            } => {
                let key = String::from_utf8_lossy(entry.key()).into_owned();
                let Some((table, record_id)) = key.split_once('/') else {
                    tracing::warn!(key = %key, "replica entry key not in table/id form — skipped");
                    continue;
                };
                // F20/M1 finding 2: attribute authorship from the ENTRY
                // author (`entry.author()` — the key that signed the entry),
                // never from `from` — iroh-docs 0.35's `InsertRemote.from`
                // is the FORWARDING neighbor, not the author. With the docs
                // author imported from the endpoint identity at stack
                // spawn, `entry.author()` maps to the same did:key the A3
                // gate resolves, so >=3-member docs judge the author, not
                // the relaying node, and the snapshot path (which always
                // read `entry.author()`) agrees with live delivery. `from`
                // is retained only as tracing context.
                let content_hash: BlobHash = *entry.content_hash().as_bytes();
                let meta = PendingContent {
                    table: table.to_string(),
                    record_id: record_id.to_string(),
                    content_hash,
                    author_id: author_did_key(&entry.author()),
                    timestamp: entry.timestamp(),
                };
                tracing::debug!(
                    key = %key,
                    forwarded_by = %from.fmt_short(),
                    author = %meta.author_id,
                    "InsertRemote"
                );
                if entry.record().is_empty() {
                    // Deletion marker: no content to download.
                    let change = RowChange {
                        table: meta.table,
                        record_id: meta.record_id,
                        content_hash: meta.content_hash,
                        author_id: meta.author_id,
                        timestamp: meta.timestamp,
                        is_tombstone: true,
                        data: Vec::new(),
                    };
                    if out.send(change).await.is_err() {
                        return; // receiver dropped — replica closed
                    }
                    continue;
                }
                match content_status {
                    iroh_docs::ContentStatus::Complete => {
                        let data = match read_blob_bytes(&blobs, &entry.content_hash()).await {
                            Ok(data) => data,
                            Err(e) => {
                                tracing::warn!(
                                    key = %key,
                                    "reading replica entry content failed: {e}"
                                );
                                continue;
                            }
                        };
                        let data = data.to_vec();
                        let change = RowChange {
                            table: meta.table,
                            record_id: meta.record_id,
                            content_hash: meta.content_hash,
                            author_id: meta.author_id,
                            timestamp: meta.timestamp,
                            is_tombstone: row_is_tombstone(&data),
                            data,
                        };
                        if out.send(change).await.is_err() {
                            return;
                        }
                    }
                    status => {
                        tracing::debug!(
                            key = %key,
                            ?status,
                            "replica entry content pending download"
                        );
                        pending.entry(entry.content_hash()).or_default().push(meta);
                    }
                }
            }
            LiveEvent::ContentReady { hash } => {
                let Some(waiting) = pending.remove(&hash) else {
                    continue;
                };
                let data = match read_blob_bytes(&blobs, &hash).await {
                    Ok(data) => data,
                    Err(e) => {
                        tracing::warn!(hash = %hash, "content-ready read failed: {e}");
                        continue;
                    }
                };
                let data = data.to_vec();
                for meta in waiting {
                    let change = RowChange {
                        table: meta.table,
                        record_id: meta.record_id,
                        content_hash: meta.content_hash,
                        author_id: meta.author_id,
                        timestamp: meta.timestamp,
                        is_tombstone: row_is_tombstone(&data),
                        data: data.clone(),
                    };
                    if out.send(change).await.is_err() {
                        return;
                    }
                }
            }
            LiveEvent::NeighborUp(peer) => {
                tracing::debug!(peer = %peer_did_key(&peer), "replica swarm neighbor up");
            }
            LiveEvent::NeighborDown(peer) => {
                tracing::debug!(peer = %peer_did_key(&peer), "replica swarm neighbor down");
            }
            LiveEvent::SyncFinished(_) | LiveEvent::PendingContentReady => {
                tracing::debug!("replica sync progress event");
            }
        }
    }
    if !pending.is_empty() {
        tracing::warn!(
            count = pending.len(),
            "replica event stream ended with content still pending"
        );
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
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>>;

    /// Subscribe to incoming row changes from peers within this petal.
    fn subscribe(&self) -> ReplicatorFuture<'_, anyhow::Result<mpsc::Receiver<RowChange>>>;

    /// Close the replica, flushing any pending state.
    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>>;
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
/// the petal-scoped interface. Petal-granularity verse namespaces are a
/// follow-up — petal rows replicate through their verse's namespace today.
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
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        tracing::debug!(
            petal = %self.petal_id,
            ns = %self.namespace_id,
            key = %Self::encode_key(table, record_id),
            "IrohPetalReplicator::write_row"
        );
        Box::pin(VerseReplicator::write_row(
            &self.inner,
            table,
            record_id,
            data,
        ))
    }

    fn subscribe(&self) -> ReplicatorFuture<'_, anyhow::Result<mpsc::Receiver<RowChange>>> {
        Box::pin(VerseReplicator::subscribe(&self.inner))
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        tracing::debug!(petal = %self.petal_id, ns = %self.namespace_id, "IrohPetalReplicator::close");
        Box::pin(VerseReplicator::close(&self.inner))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs_engine::DocsStack;

    #[tokio::test]
    async fn mock_replicator_write_and_count() {
        let mock = MockVerseReplicator::new("author-a");
        mock.write_row("verse", "v1", br#"{"name":"test"}"#)
            .await
            .unwrap();
        assert_eq!(mock.entry_count(), 1);
        assert!(mock.has_entry("verse", "v1"));
        assert!(!mock.has_entry("verse", "v2"));
    }

    #[tokio::test]
    async fn mock_replicator_close_rejects_writes() {
        let mock = MockVerseReplicator::new("author-a");
        mock.close().await.unwrap();
        assert!(mock.write_row("verse", "v1", b"{}").await.is_err());
    }

    #[tokio::test]
    async fn mock_replicator_subscribe_receives_changes() {
        let mock = MockVerseReplicator::new("author-a");
        let mut rx = mock.subscribe().await.unwrap();
        mock.write_row("fractal", "f1", br#"{"name":"frac"}"#)
            .await
            .unwrap();
        let change = rx.try_recv().unwrap();
        assert_eq!(change.table, "fractal");
        assert_eq!(change.record_id, "f1");
        assert_eq!(change.author_id, "author-a");
        assert!(!change.is_tombstone);
    }

    #[tokio::test]
    async fn row_change_carries_payload_bytes() {
        // A2/A4: the inbound apply path rides the payload bytes on the event —
        // the DB thread never re-reads a blob store for an inbound row.
        let mock = MockVerseReplicator::new("author-a");
        let mut rx = mock.subscribe().await.unwrap();
        let payload = br#"{"verse_id":"v1","name":"payload"}"#;
        mock.write_row("verse", "v1", payload).await.unwrap();
        let change = rx.try_recv().unwrap();
        assert_eq!(change.data, payload.to_vec());
        assert_eq!(
            change.content_hash,
            *blake3::hash(payload).as_bytes(),
            "content hash must still cover the payload bytes"
        );
    }

    #[tokio::test]
    async fn offline_holder_degrades_doc_replicator_to_mock() {
        // Mock fallback is the *offline* behavior: with an empty holder every
        // operation must still work through the in-memory store.
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let repl = IrohDocsReplicator::new(
            "verse-offline".to_string(),
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32]),
            "local-author".to_string(),
            engine_holder,
        );
        repl.open_document().await.unwrap();
        assert!(!repl.is_doc_backed());
        repl.write_row("verse", "v1", br#"{"name":"test"}"#)
            .await
            .unwrap();
        assert_eq!(repl.inner.entry_count(), 1);
        assert!(!repl.is_doc_backed());
        repl.close().await.unwrap();
    }

    #[tokio::test]
    async fn invalid_secret_degrades_to_mock_without_crashing() {
        // A garbage namespace secret (not 32-byte hex) must degrade loudly to
        // the mock path — never a panic, never a crash. Offline (empty
        // holder) the mock fallback is sanctioned; ON an online stack the
        // sync thread's open handler marks the replica non-replicating
        // instead (see sync_thread tests).
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let repl = IrohDocsReplicator::new(
            "verse-bad-secret".to_string(),
            "some-ns".to_string(),
            "test-secret".to_string(),
            "local-author".to_string(),
            engine_holder,
        );
        assert!(
            repl.open_document().await.is_err(),
            "an unparseable secret must surface as an open error"
        );
        repl.write_row("verse", "v1", b"{}").await.unwrap();
        assert!(!repl.is_doc_backed());
    }

    /// F20 finding 4: a failed online open makes the replica loudly
    /// non-replicating — writes warn and fail, `subscribe`/`snapshot`
    /// error. There is no path back to the in-memory mock success state.
    #[tokio::test]
    async fn marked_failed_replica_is_loudly_non_replicating() {
        let engine_holder = Arc::new(IrohDocsEngineHolder::new());
        let repl = IrohDocsReplicator::new(
            "verse-failed".to_string(),
            hex::encode([1u8; 32]),
            hex::encode([2u8; 32]),
            "local-author".to_string(),
            engine_holder,
        );
        repl.mark_open_failed("namespace import failed (test)".to_string());

        assert_eq!(
            repl.open_error().as_deref(),
            Some("namespace import failed (test)")
        );
        assert!(
            repl.write_row("verse", "v1", b"{}").await.is_err(),
            "writes on a failed online open must fail loudly, never publish to the mock"
        );
        assert!(repl.subscribe().await.is_err());
        assert!(repl.snapshot().await.is_err());
        // The replica must still be closeable (re-open churn depends on it).
        repl.close().await.unwrap();
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
            data: Vec::new(),
        }
    }

    #[test]
    fn loop_prevention_skips_own_writes() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-a", 100);
        assert!(!applicator.should_apply(&change, None, None, false));
    }

    #[test]
    fn applies_remote_writes() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 100);
        assert!(applicator.should_apply(&change, None, None, false));
    }

    #[test]
    fn newer_remote_wins() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 200);
        assert!(applicator.should_apply(&change, Some(100), Some("author-a"), false));
    }

    #[test]
    fn older_remote_loses() {
        let applicator = IncomingEntryApplicator::new("author-a");
        let change = make_change("author-b", 50);
        assert!(!applicator.should_apply(&change, Some(100), Some("author-a"), false));
    }

    #[test]
    fn equal_timestamp_higher_author_wins() {
        let applicator = IncomingEntryApplicator::new("author-a");
        // "author-b" > "author-a" lexicographically, so remote wins
        let change = make_change("author-b", 100);
        assert!(applicator.should_apply(&change, Some(100), Some("author-a"), false));

        // "author-a" < "author-c" so if local is "author-c", remote loses
        let applicator2 = IncomingEntryApplicator::new("author-z");
        let change2 = make_change("author-b", 100);
        assert!(!applicator2.should_apply(&change2, Some(100), Some("author-z"), false));
    }

    // --- N-4 tombstone dominance (FR-1 non-resurrection) ---

    #[test]
    fn row_is_tombstone_detects_soft_delete() {
        assert!(row_is_tombstone(
            br#"{"node_id":"n1","tombstone":{"hlc":42,"source_did":"did:key:z"}}"#
        ));
        assert!(!row_is_tombstone(br#"{"node_id":"n1"}"#));
        assert!(!row_is_tombstone(br#"{"node_id":"n1","tombstone":null}"#));
    }

    #[test]
    fn row_is_tombstone_detects_empty_entry_del_marker() {
        // iroh-docs `del` writes an empty entry (no content) — the empty
        // payload IS the deletion marker (F2 empty-entry tombstone detection).
        assert!(row_is_tombstone(b""));
    }

    #[tokio::test]
    async fn tombstone_row_propagates_is_tombstone_flag() {
        let mock = MockVerseReplicator::new("author-a");
        let mut rx = mock.subscribe().await.unwrap();
        mock.write_row("node", "n1", br#"{"node_id":"n1","tombstone":{"hlc":1}}"#)
            .await
            .unwrap();
        let change = rx.try_recv().unwrap();
        assert!(
            change.is_tombstone,
            "soft-deleted row must propagate as tombstone"
        );
        assert!(
            !change.data.is_empty(),
            "a soft-delete row still carries its payload"
        );

        // An empty payload (the del marker) is a tombstone with no bytes.
        mock.write_row("node", "n2", b"").await.unwrap();
        let change = rx.try_recv().unwrap();
        assert!(change.is_tombstone);
        assert!(change.data.is_empty());
    }

    #[test]
    fn tombstone_never_resurrected_and_delete_wins() {
        let applicator = IncomingEntryApplicator::new("author-a");
        // A stale LIVE remote write must NOT resurrect a locally tombstoned node.
        let live = make_change("author-b", 999);
        assert!(!applicator.should_apply(&live, Some(1), Some("author-a"), true));

        // An incoming DELETE wins over a concurrent live local row.
        let mut del = make_change("author-b", 1);
        del.is_tombstone = true;
        assert!(applicator.should_apply(&del, Some(999), Some("author-a"), false));
    }

    // --- IrohPetalReplicator tests ---

    #[tokio::test]
    async fn petal_replicator_write_and_subscribe() {
        let repl = IrohPetalReplicator::new(
            "petal-1".to_string(),
            "ns-petal-1".to_string(),
            "local-author".to_string(),
        );
        let mut rx = repl.subscribe().await.unwrap();
        repl.write_row("node", "n1", br#"{"name":"test"}"#)
            .await
            .unwrap();
        let change = rx.try_recv().unwrap();
        assert_eq!(change.table, "node");
        assert_eq!(change.record_id, "n1");
    }

    #[tokio::test]
    async fn petal_replicator_close_rejects_writes() {
        let repl = IrohPetalReplicator::new(
            "petal-2".to_string(),
            "ns-petal-2".to_string(),
            "local-author".to_string(),
        );
        repl.close().await.unwrap();
        assert!(repl.write_row("node", "n1", b"{}").await.is_err());
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

    // --- Real doc-backed replicator over loopback (A2 transport half) ---

    /// Bind a hermetic loopback endpoint (relay disabled — loopback-only rule),
    /// reusing the same `SyncEndpoint` seam the sync thread binds with.
    async fn bind_loopback_endpoint(seed: u8) -> Option<iroh::Endpoint> {
        let secret = iroh::SecretKey::from_bytes(&[seed; 32]);
        match crate::endpoint::SyncEndpoint::new(
            secret,
            &crate::relay_config::RelayConfig::Disabled,
        )
        .await
        {
            Ok(ep) => Some(ep.inner().clone()),
            Err(e) => {
                tracing::warn!("endpoint bind failed (sandboxed env?) — skipping: {e}");
                None
            }
        }
    }

    #[tokio::test]
    async fn doc_replicator_replicates_row_over_real_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let (Some(ep_a), Some(ep_b)) = (
            bind_loopback_endpoint(11).await,
            bind_loopback_endpoint(22).await,
        ) else {
            return; // sandboxed environment without UDP — tolerated, see F1 tests
        };

        let stack_a = Arc::new(
            DocsStack::spawn(ep_a.clone(), tmp.path().join("peer-a"))
                .await
                .expect("stack A spawns"),
        );
        let stack_b = Arc::new(
            DocsStack::spawn(ep_b.clone(), tmp.path().join("peer-b"))
                .await
                .expect("stack B spawns"),
        );
        let holder_a = Arc::new(IrohDocsEngineHolder::online(stack_a.clone()));
        let holder_b = Arc::new(IrohDocsEngineHolder::online(stack_b.clone()));

        // The verse namespace: one 32-byte secret shared by both sides; the
        // app-level namespace_id is derived the same way on both (the
        // Ed25519 id the secret registers the doc under).
        let secret = [7u8; 32];
        let ns_id_hex = hex::encode(secret);

        let alice = IrohDocsReplicator::new(
            "verse-a2".to_string(),
            ns_id_hex.clone(),
            hex::encode(secret),
            "did:key:alice".to_string(),
            holder_a,
        );
        let bob = IrohDocsReplicator::new(
            "verse-a2".to_string(),
            ns_id_hex,
            hex::encode(secret),
            "did:key:bob".to_string(),
            holder_b,
        );
        alice.open_document().await.expect("alice opens doc");
        bob.open_document().await.expect("bob opens doc");
        assert!(alice.is_doc_backed() && bob.is_doc_backed());

        // Subscribe Bob before the write, then start BOTH docs syncing.
        // Alice's side runs with no outbound peers — but her sync task must
        // be running to serve the entry to the peer that dials her (the same
        // passive-side rule `handle_open_verse_replica` follows).
        let mut bob_rx = bob.subscribe().await.expect("bob subscribes");
        let addr_a = ep_a.node_addr().await.expect("alice node addr");
        bob.start_sync(vec![addr_a])
            .await
            .expect("bob syncs with alice");
        alice
            .start_sync(Vec::new())
            .await
            .expect("alice's doc serves entries");

        // Alice writes a row through the real Doc::set_bytes path.
        let payload = br#"{"verse_id":"verse-a2","name":"Real Transport Verse"}"#.to_vec();
        alice
            .write_row("verse", "verse-a2", &payload)
            .await
            .expect("alice set_bytes");

        // Bob's pump must deliver the change with the payload bytes attached.
        let change = tokio::time::timeout(std::time::Duration::from_secs(20), bob_rx.recv())
            .await
            .expect("replicated row arrives over the real transport")
            .expect("pump stays alive");
        assert_eq!(change.table, "verse");
        assert_eq!(change.record_id, "verse-a2");
        assert_eq!(change.data, payload, "payload bytes must ride the event");
        assert!(!change.is_tombstone);

        // F20 finding 2: the live pump attributes authorship from the ENTRY
        // author (the endpoint identity `DocsStack::spawn` imported as the
        // docs author) — the same did:key the A3 gate resolves. It is NOT
        // the app-level "did:key:alice" string, and NOT the forwarding
        // neighbor's raw key form.
        let alice_endpoint_did = peer_did_key(&ep_a.node_id());
        assert_eq!(
            change.author_id, alice_endpoint_did,
            "live author must be the entry author (endpoint identity did:key)"
        );

        // The snapshot path must attribute the SAME row to the SAME author —
        // live delivery and restart replay judge one identity, which is what
        // makes the reconciliation second chance work.
        let snap = bob.snapshot().await.expect("bob snapshots the doc");
        let snap_row = snap
            .iter()
            .find(|c| c.record_id == "verse-a2")
            .expect("snapshot contains the replicated row");
        assert_eq!(
            snap_row.author_id, alice_endpoint_did,
            "snapshot author must equal live author for the same entry"
        );

        // Close must leave the swarm cleanly on both sides.
        alice.close().await.unwrap();
        bob.close().await.unwrap();
    }

    /// F20/M1 finding 3: the app-level `derive_namespace_id` must produce
    /// the ACTUAL iroh namespace id (the Ed25519 public key the secret's
    /// signing key derives to) — the cross-crate alignment that makes a
    /// secretless read-only reopen addressable by the stored id.
    #[test]
    fn derive_namespace_id_matches_iroh_namespace_id() {
        let secret = [7u8; 32];
        let derived_hex = hex::encode(fe_database::derive_namespace_id(&secret));
        let iroh_ns = NamespaceSecret::from_bytes(&secret).id();
        assert_eq!(
            derived_hex,
            iroh_ns.to_string(),
            "fe-database's derivation must equal iroh-docs' namespace id"
        );
        // And it parses back to the same id.
        use std::str::FromStr;
        assert_eq!(
            NamespaceId::from_str(&derived_hex).unwrap(),
            iroh_ns,
            "the derived hex must round-trip through iroh's own parser"
        );
    }

    /// F20/M1 finding 3: a doc imported with its secret reopens read-only
    /// WITHOUT the secret on the same persisted store, addressed by the
    /// aligned (Ed25519) namespace id the verse row now carries.
    #[tokio::test]
    async fn secretless_reopen_finds_persisted_doc_by_aligned_id() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(ep) = bind_loopback_endpoint(44).await else {
            return; // sandboxed environment without UDP — tolerated
        };
        let stack = Arc::new(
            DocsStack::spawn(ep, tmp.path().join("p2p"))
                .await
                .expect("stack"),
        );
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));

        let secret = [9u8; 32];
        // The production form: verse.namespace_id = derive_namespace_id.
        let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret));

        // First open: with the secret (write capability import).
        let writer = IrohDocsReplicator::new(
            "verse-reopen".to_string(),
            ns_id_hex.clone(),
            hex::encode(secret),
            "did:key:writer".to_string(),
            holder.clone(),
        );
        writer.open_document().await.expect("writer opens doc");
        writer
            .write_row(
                "verse",
                "verse-reopen",
                br#"{"verse_id":"verse-reopen","name":"Reopen Verse"}"#,
            )
            .await
            .expect("manifest written");
        writer
            .write_row("node", "node-1", br#"{"node_id":"node-1","name":"n"}"#)
            .await
            .expect("row written");
        writer.close().await.unwrap();

        // Second open: NO secret — the persisted doc must be found by the
        // stored id and opened read-only.
        let reader = IrohDocsReplicator::new(
            "verse-reopen".to_string(),
            ns_id_hex,
            String::new(),
            "did:key:reader".to_string(),
            holder,
        );
        reader.open_document().await.expect("secretless reopen");
        assert!(reader.is_doc_backed(), "read-only doc handle is live");
        let snap = reader.snapshot().await.expect("snapshot of reopened doc");
        assert_eq!(snap.len(), 2, "manifest + node row both present");
        reader.close().await.unwrap();
    }

    /// F20/M1 finding 3 compatibility: a LEGACY `namespace_id` (the pre-fix
    /// keyed-BLAKE3 digest, still present in live DBs) never equals the iroh
    /// namespace id — the secretless open must then fall back to scanning
    /// known docs for the one carrying this verse's manifest row, and find
    /// the same persisted doc.
    #[tokio::test]
    async fn secretless_reopen_falls_back_to_manifest_scan_on_legacy_id() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(ep) = bind_loopback_endpoint(55).await else {
            return; // sandboxed environment without UDP — tolerated
        };
        let stack = Arc::new(
            DocsStack::spawn(ep, tmp.path().join("p2p"))
                .await
                .expect("stack"),
        );
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));

        let secret = [11u8; 32];
        // Import first (the id the doc registers under is the Ed25519 key).
        let writer = IrohDocsReplicator::new(
            "verse-legacy".to_string(),
            hex::encode(fe_database::derive_namespace_id(&secret)),
            hex::encode(secret),
            "did:key:writer".to_string(),
            holder.clone(),
        );
        writer.open_document().await.expect("writer opens doc");
        writer
            .write_row(
                "verse",
                "verse-legacy",
                br#"{"verse_id":"verse-legacy","name":"Legacy Verse"}"#,
            )
            .await
            .expect("manifest written");
        writer.close().await.unwrap();

        // Legacy stored id: the pre-F20 keyed-BLAKE3 digest form.
        let legacy_ns_hex = hex::encode(
            blake3::keyed_hash(b"fractalengine:verse:namespace_id", &secret).as_bytes(),
        );
        let reader = IrohDocsReplicator::new(
            "verse-legacy".to_string(),
            legacy_ns_hex,
            String::new(),
            "did:key:reader".to_string(),
            holder,
        );
        reader
            .open_document()
            .await
            .expect("legacy stored id resolves through the manifest scan");
        assert!(
            reader.is_doc_backed(),
            "the persisted doc reopened read-only"
        );
        let snap = reader.snapshot().await.expect("snapshot of reopened doc");
        assert!(
            snap.iter().any(|c| c.record_id == "verse-legacy"),
            "the manifest row is readable after the fallback reopen"
        );
        reader.close().await.unwrap();
    }

    /// F20/M1 finding 3 negative: with no persisted doc claiming the verse
    /// and an id the store does not know, the secretless open fails loudly
    /// (an error the online open handler turns into a non-replicating
    /// replica — never a silent mock).
    #[tokio::test]
    async fn secretless_open_with_no_matching_doc_fails_loudly() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(ep) = bind_loopback_endpoint(66).await else {
            return; // sandboxed environment without UDP — tolerated
        };
        let stack = Arc::new(
            DocsStack::spawn(ep, tmp.path().join("p2p"))
                .await
                .expect("stack"),
        );
        let holder = Arc::new(IrohDocsEngineHolder::online(stack));

        let reader = IrohDocsReplicator::new(
            "verse-unknown".to_string(),
            "0".repeat(64),
            String::new(),
            "did:key:reader".to_string(),
            holder,
        );
        assert!(
            reader.open_document().await.is_err(),
            "an unknown namespace with no claiming doc must be a loud open error"
        );
    }

    #[tokio::test]
    async fn peer_did_key_maps_iroh_node_id_to_did_key() {
        let secret = iroh::SecretKey::from_bytes(&[33u8; 32]);
        let node_id = secret.public();
        let did = peer_did_key(&node_id);
        assert!(did.starts_with("did:key:z6Mk"), "got: {did}");
        // Round-trips through fe-identity's parser.
        let recovered = fe_identity::did_key::public_key_from_did_key(&did).unwrap();
        assert_eq!(recovered.to_bytes(), *node_id.as_bytes());
    }
}
