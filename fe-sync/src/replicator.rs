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
//! `IrohDocsReplicator` rides the real iroh-docs 0.101 `Doc` API
//! (`set_bytes` / `subscribe` / `del` / `close`) whenever the P2P stack is
//! online, and degrades to the in-memory [`MockVerseReplicator`] only when
//! it is not (offline bind failure, stack spawn failure). On an ONLINE
//! stack, a document-open failure leaves a loudly non-replicating replica
//! (writes warn and fail, `snapshot`/`subscribe` error — F20 finding 4),
//! never a usable in-memory success path. See `fe-sync/src/AGENTS.md`
//! §iroh-1.x.
//!
//! **One authenticated author identity (F20 finding 2):** the docs author
//! is the endpoint identity (imported + made default at
//! `DocsStack::spawn`), so `entry.author()` maps to the same `did:key` the
//! A3 gate resolves. Live delivery and snapshot replay attribute rows from
//! `entry.author()` identically; `InsertRemote.from` is only the
//! forwarding neighbor.

use fe_runtime::blob_store::BlobHash;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use bytes::Bytes;
use futures_lite::{Stream, StreamExt};
use iroh_docs::api::{Doc, DocsApi};
use iroh_docs::engine::LiveEvent;
use iroh_docs::{AuthorId, Capability, NamespaceId, NamespaceSecret};
use tokio::sync::mpsc;

/// Boxed async return for the dyn-compatible trait methods.
pub type ReplicatorFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// A live iroh-docs document handle as handed out by the in-process docs API.
pub type DocHandle = Doc;

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
    /// Current authenticated native synchronization peers for this exact open replica.
    fn scene_provider_roster(&self) -> Option<SceneProviderRoster> { None }

    fn joined_peer_ready(&self, namespace_id: &str, issuer_did: &str) -> bool {
        let _ = (namespace_id, issuer_did);
        false
    }
    fn export_join_ticket(
        &self,
        namespace_id: &str,
        role: fe_database::join_invite::JoinRole,
        issuer_did: &str,
    ) -> ReplicatorFuture<'_, anyhow::Result<crate::join_ticket::NativeJoinTicket>> {
        let _ = (namespace_id, role, issuer_did);
        Box::pin(async { anyhow::bail!("This replica has no native document ticket") })
    }
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

    /// Whether this node's own current entry for `table/record_id` already holds exactly
    /// `content_hash` (a BLAKE3 of the row bytes), so republishing it changes nothing
    /// (QA-0164, AGENTS.md §unchanged-rewrite). Replicas without a local store say no.
    fn holds_own_entry(
        &self,
        table: &str,
        record_id: &str,
        content_hash: &BlobHash,
    ) -> ReplicatorFuture<'_, bool> {
        let _ = (table, record_id, content_hash);
        Box::pin(async { false })
    }

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

    /// Entries authored by `author_id` whose table matches one of
    /// `prefixes` (each a bare table name, e.g. `"role"` or `"role/"` —
    /// a trailing slash is stripped). This is the §2.5 "re-read on
    /// empowerment" seam: when a grantor's `role_at` rises, the sync
    /// thread replays exactly this — their `role`/`__shards`/`__holders`/
    /// `verse` entries — straight from the replica through the same
    /// inbound apply path a live row takes, recovering whatever this
    /// author's statements the fabric held back while they lacked
    /// authority (the fabric never stores a held statement itself; the
    /// replica already does).
    ///
    /// The default filters [`VerseReplicator::snapshot`] — correct but
    /// O(whole replica) per call. [`IrohDocsReplicator`] overrides it with
    /// a direct `Query::author(..).key_prefix(..)` per prefix; every other
    /// implementation (the mock, `fe-sim`'s virtual transport) inherits
    /// this default unchanged.
    fn entries_by_author(
        &self,
        author_id: &str,
        prefixes: &[&str],
    ) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        let author_id = author_id.to_string();
        let tables: Vec<String> = prefixes
            .iter()
            .map(|p| p.trim_end_matches('/').to_string())
            .collect();
        Box::pin(async move {
            let all = self.snapshot().await?;
            Ok(all
                .into_iter()
                .filter(|c| c.author_id == author_id && tables.iter().any(|t| t == &c.table))
                .collect())
        })
    }

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
    /// RP-5 §2.5 step 3: `role/` keys need per-(key, author) retention so a
    /// unit test can exercise the "snapshot reads every author" guarantee
    /// without a real iroh loopback (`entries` is single-author-per-key,
    /// matching `single_latest_per_key` — exactly the gap role rows must
    /// NOT have). Seeded by `write_row` (for this instance's own author)
    /// and by `seed_role_entry` (to simulate another author's write
    /// landing in the shared doc); read by `snapshot`.
    role_entries: Mutex<HashMap<String, HashMap<String, Vec<u8>>>>,
    subscribers: Mutex<Vec<mpsc::Sender<RowChange>>>,
    author_id: String,
    closed: Mutex<bool>,
}

impl MockVerseReplicator {
    pub fn new(author_id: impl Into<String>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            role_entries: Mutex::new(HashMap::new()),
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

    /// Test-only: seed another author's `role/{record_id}` entry directly
    /// into this replica's store, simulating a peer's write landing in the
    /// shared doc without standing up a real loopback transport (RP-5
    /// §2.5 step 3). Scoped to `role` because that is the one table whose
    /// multi-author semantics need exercising here.
    pub fn seed_role_entry(&self, record_id: &str, author_id: &str, data: &[u8]) {
        self.role_entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(record_id.to_string())
            .or_default()
            .insert(author_id.to_string(), data.to_vec());
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
            if table == "role" {
                self.role_entries
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?
                    .entry(record_id.clone())
                    .or_default()
                    .insert(self.author_id.clone(), data.clone());
            }

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
            let role_entries = self
                .role_entries
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
                if table == "role" {
                    // RP-5 §2.5 step 3: `role` rows come exclusively from
                    // `role_entries` below (ALL authors), never from the
                    // single-author `entries` map — a single-winner view
                    // here would reproduce the exact `single_latest_per_key`
                    // shadowing bug this step closes.
                    continue;
                }
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
            for (record_id, by_author) in role_entries {
                for (author_id, data) in by_author {
                    out.push(RowChange {
                        table: "role".to_string(),
                        record_id: record_id.clone(),
                        content_hash: *blake3::hash(&data).as_bytes(),
                        author_id,
                        timestamp: now,
                        is_tombstone: row_is_tombstone(&data),
                        data,
                    });
                }
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

/// Holder for the real iroh-docs P2P stack
/// ([`crate::docs_engine::DocsStack`]: Blobs + Gossip + Docs + Router).
///
/// The sync thread constructs it `online` when the endpoint bound and the
/// full stack spawned, and leaves it empty in offline mode (bind failure or
/// stack spawn failure) — `is_available()` then reports `false` and
/// replicators degrade to the in-memory mock without crashing.
/// See `fe-sync/src/AGENTS.md` §iroh-1.x.
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

    /// Clone of the in-process docs API, when online.
    ///
    /// The handle the replicator layer uses for `import_namespace` /
    /// `set_bytes` / `subscribe` / `start_sync` / `close` — the real
    /// Doc-backed path rides this seam.
    pub fn docs_client(&self) -> Option<DocsApi> {
        self.stack.as_ref().map(|s| s.docs_client())
    }
}

// ---------------------------------------------------------------------------
// IrohDocsReplicator — backed by the real iroh-docs Doc client (A2)
// ---------------------------------------------------------------------------

/// Implementation of `VerseReplicator` backed by iroh-docs 0.101.
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
    join_readiness: RwLock<Arc<JoinReadiness>>,
}

#[derive(Default)]
struct JoinReadiness {
    alive: AtomicBool,
    invalidated: AtomicBool,
    successful_peers: Mutex<HashSet<iroh::EndpointId>>,
}

/// Bounded live provider identities owned by one native subscription generation.
#[derive(Clone)]
pub struct SceneProviderRoster(Arc<JoinReadiness>);
impl SceneProviderRoster {
    /// Every live sync-confirmed peer, sorted (at most 256, `confirm_peer`). A
    /// verse with more than 16 collaborators keeps its roster (QA-0136): the
    /// scene-asset layer samples a 16-provider window from it per fetch.
    pub(crate) fn peers(&self) -> anyhow::Result<Vec<iroh::EndpointId>> {
        anyhow::ensure!(
            self.0.alive.load(Ordering::Acquire) && !self.0.invalidated.load(Ordering::Acquire),
            "Native provider subscription is no longer live"
        );
        let peers = self
            .0
            .successful_peers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut peers: Vec<_> = peers.iter().copied().collect();
        peers.sort();
        anyhow::ensure!(
            self.0.alive.load(Ordering::Acquire) && !self.0.invalidated.load(Ordering::Acquire),
            "Native provider subscription closed during capture"
        );
        Ok(peers)
    }
}
#[cfg(test)]
impl SceneProviderRoster {
    /// A live roster of `peers`, for journeys that need more than a loopback pair.
    pub(crate) fn live_for_test(peers: impl IntoIterator<Item = iroh::EndpointId>) -> Self {
        let readiness = JoinReadiness::default();
        readiness.alive.store(true, Ordering::Release);
        readiness
            .successful_peers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(peers);
        Self(Arc::new(readiness))
    }
}

struct JoinReadinessGuard(Arc<JoinReadiness>);

impl Drop for JoinReadinessGuard {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
    }
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
            join_readiness: RwLock::new(Arc::new(JoinReadiness::default())),
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
    /// namespace id is opened read-only (`DocsApi::open` — a previously
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
    /// now returns the Ed25519 id), which `DocsApi::open` resolves directly.
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
            .author_default()
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
    pub async fn start_sync(&self, peers: Vec<iroh::EndpointAddr>) -> anyhow::Result<()> {
        let Some(doc) = self.live_doc() else {
            return Ok(());
        };
        doc.start_sync(peers)
            .await
            .map_err(|e| anyhow::anyhow!("start_sync: {e}"))
    }

    /// The newest entry for `key` across every author is this author's, holds `hash`, and its
    /// blob is complete locally (QA-0164, AGENTS.md §unchanged-rewrite). Local store reads only.
    async fn own_entry_holds(&self, key: &str, hash: iroh_blobs::Hash) -> bool {
        let (Some(doc), Some(author), Some(stack)) =
            (self.live_doc(), self.live_author(), self.engine_holder.stack())
        else {
            return false;
        };
        // Another author's newer entry (or tombstone) wins the key, so our identical bytes must be
        // re-set to win it back: a static row merges in arrival order and a snapshot keeps the newest.
        let latest = iroh_docs::store::Query::single_latest_per_key()
            .key_exact(key.as_bytes())
            .include_empty()
            .build();
        let same_entry = doc
            .get_one(latest)
            .await
            .ok()
            .flatten()
            .is_some_and(|entry| entry.author() == author && entry.content_hash() == hash);
        same_entry
            && matches!(
                stack.blob_store().blobs().status(hash).await,
                Ok(iroh_blobs::api::blobs::BlobStatus::Complete { .. })
            )
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
            // QA-0164: identical bytes are not a new version; see AGENTS.md §unchanged-rewrite.
            if self.own_entry_holds(&key, iroh_blobs::Hash::new(data)).await {
                tracing::debug!(ns = %doc.id().fmt_short(), key = %key, "unchanged row not rewritten");
                return Ok(());
            }
            doc.set_bytes(author, key.as_bytes().to_vec(), data.to_vec())
                .await
                .map_err(|e| anyhow::anyhow!("set_bytes {key}: {e}"))?;
            tracing::debug!(ns = %doc.id().fmt_short(), key = %key, "row written");
        }
        Ok(())
    }
}

impl VerseReplicator for IrohDocsReplicator {
    fn scene_provider_roster(&self) -> Option<SceneProviderRoster> {
        if self.open_error().is_some() || !self.is_doc_backed() { return None; }
        Some(SceneProviderRoster(self.join_readiness.read().unwrap_or_else(|error| error.into_inner()).clone()))
    }

    fn joined_peer_ready(&self, namespace_id: &str, issuer_did: &str) -> bool {
        if self.open_error().is_some() || !self.live_doc().is_some_and(|doc| hex::encode(doc.id().as_bytes()) == namespace_id) {
            return false;
        }
        let Ok(issuer) = fe_identity::public_key_from_did_key(issuer_did) else { return false; };
        let readiness = self.join_readiness.read().unwrap_or_else(|error| error.into_inner());
        let ready = readiness.alive.load(Ordering::Acquire) && readiness.successful_peers.lock().unwrap_or_else(|error| error.into_inner())
            .iter().any(|peer| peer.as_bytes() == &issuer.to_bytes());
        ready
    }
    fn export_join_ticket(
        &self,
        namespace_id: &str,
        role: fe_database::join_invite::JoinRole,
        issuer_did: &str,
    ) -> ReplicatorFuture<'_, anyhow::Result<crate::join_ticket::NativeJoinTicket>> {
        let namespace_id = namespace_id.to_owned();
        let issuer_did = issuer_did.to_owned();
        Box::pin(async move {
            anyhow::ensure!(self.open_error().is_none(), "Replica is unavailable");
            let doc = self.live_doc().ok_or_else(|| anyhow::anyhow!("A live native replica is required"))?;
            let mode = if role.requires_write() { iroh_docs::api::protocol::ShareMode::Write }
                else { iroh_docs::api::protocol::ShareMode::Read };
            let ticket = doc.share(mode, iroh_docs::api::protocol::AddrInfoOptions::RelayAndAddresses)
                .await.map_err(|_| anyhow::anyhow!("Document ticket export failed"))?;
            crate::join_ticket::NativeJoinTicket::parse(&ticket.to_string(), &namespace_id, role, &issuer_did)
        })
    }
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

    fn holds_own_entry(
        &self,
        table: &str,
        record_id: &str,
        content_hash: &BlobHash,
    ) -> ReplicatorFuture<'_, bool> {
        let key = format!("{table}/{record_id}");
        let hash = iroh_blobs::Hash::from_bytes(*content_hash);
        Box::pin(async move { self.open_error().is_none() && self.own_entry_holds(&key, hash).await })
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
            if let Some(old) = self.pump.lock().unwrap_or_else(|error| error.into_inner()).take() { old.abort(); }
            self.join_readiness.read().unwrap_or_else(|error| error.into_inner()).invalidated.store(true, Ordering::Release);
            self.join_readiness.read().unwrap_or_else(|error| error.into_inner()).alive.store(false, Ordering::Release);
            let readiness = Arc::new(JoinReadiness::default());
            *self.join_readiness.write().unwrap_or_else(|error| error.into_inner()) = readiness.clone();
            let handle = tokio::spawn(pump_live_events(
                Box::pin(stream),
                stack.blob_store().clone(),
                tx,
                readiness,
                Some(doc.clone()),
                Some(Arc::new(crate::blob_transport::IrohBlobTransport::from_stack(stack))),
                Some(Arc::new({
                    let gate = stack.blob_access_gate().clone();
                    let verse_id = self.verse_id.clone();
                    move |author: &str| gate.names_member(&verse_id, author)
                })),
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
            let blobs = stack.blob_store().clone();

            // Latest entry per key (a key may have been written by several
            // authors); `include_empty` keeps `Doc::del` tombstones in the set.
            let latest_query = iroh_docs::store::Query::single_latest_per_key()
                .include_empty()
                .build();
            let mut out = collect_query_rows(&doc, &blobs, latest_query).await;
            // RP-5 §2.5 step 3: `role` statements must be read from ALL
            // authors — `single_latest_per_key` picks the single physically
            // newest entry for a KEY across every author who wrote it, so a
            // later forged entry on the owner's own key (grantor mismatch,
            // denied by `validate_statement`) would shadow the owner's
            // genuine entry from this pass entirely, hiding it from every
            // fresh peer whose only source is this snapshot. Drop the
            // single-winner `role` rows this pass picked up and replace them
            // with every author's entry under the `role/` prefix.
            out.retain(|c| c.table != "role" && c.table != "node");
            let role_query = iroh_docs::store::Query::key_prefix("role/")
                .include_empty()
                .build();
            out.extend(collect_query_rows(&doc, &blobs, role_query).await);
            // QA-0100: a node's transform is an `edit_seq` register merged by the
            // DB, so a (re)joining peer must see EVERY author's entry — the newest
            // one can be the register's tie loser. Oldest first, so the blind-merged
            // non-transform fields still end on the newest entry.
            let node_query = iroh_docs::store::Query::key_prefix("node/")
                .include_empty()
                .build();
            let mut nodes = collect_query_rows(&doc, &blobs, node_query).await;
            nodes.sort_by_key(|change| change.timestamp);
            out.extend(nodes);
            Ok(out)
        })
    }

    fn entries_by_author(
        &self,
        author_id: &str,
        prefixes: &[&str],
    ) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        let author_id = author_id.to_string();
        let tables: Vec<String> = prefixes
            .iter()
            .map(|p| p.trim_end_matches('/').to_string())
            .collect();
        Box::pin(async move {
            if let Some(reason) = self.open_error() {
                anyhow::bail!("replica is not replicating: document open failed: {reason}");
            }
            let Some(doc) = self.live_doc() else {
                return VerseReplicator::entries_by_author(
                    &self.inner,
                    &author_id,
                    &tables.iter().map(String::as_str).collect::<Vec<_>>(),
                )
                .await;
            };
            let Some(stack) = self.engine_holder.stack() else {
                anyhow::bail!("document open but stack unavailable");
            };
            let blobs = stack.blob_store().clone();
            let author = match author_id_from_did_key(&author_id) {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!(author = %author_id, "entries_by_author: not a parseable did:key — nothing to re-read: {e}");
                    return Ok(Vec::new());
                }
            };
            let mut out = Vec::new();
            for table in &tables {
                let query = iroh_docs::store::Query::author(author)
                    .key_prefix(format!("{table}/"))
                    .include_empty()
                    .build();
                out.extend(collect_query_rows(&doc, &blobs, query).await);
            }
            Ok(out)
        })
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            // Stop the inbound pump first so no further rows are emitted.
            self.join_readiness.read().unwrap_or_else(|error| error.into_inner()).invalidated.store(true, Ordering::Release);
            self.join_readiness.read().unwrap_or_else(|error| error.into_inner()).alive.store(false, Ordering::Release);
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
async fn find_doc_by_verse_manifest(client: &DocsApi, verse_id: &str) -> Option<DocHandle> {
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
            Ok(None) => {}
            Err(e) => {
                // An unreadable doc must not abort the scan of the rest.
                tracing::warn!(
                    verse_id,
                    ns = %doc_id.fmt_short(),
                    "manifest probe failed during doc scan: {e}"
                );
            }
        }
        // iroh-docs 0.101's `Doc` has no close-on-drop (0.35's rpc handle
        // did): release the probe's open-handle refcount explicitly.
        if let Err(e) = doc.close().await {
            tracing::debug!(ns = %doc_id.fmt_short(), "closing probed doc failed: {e}");
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

impl PendingContent {
    /// The (author, table, record) slot iroh-docs keeps one latest entry for.
    fn slot(&self) -> (String, String, String) {
        (self.author_id.clone(), self.table.clone(), self.record_id.clone())
    }
}

/// First content re-fetch after a pending entry arrives (QA-0145).
const CONTENT_RETRY_FIRST: std::time::Duration = std::time::Duration::from_secs(2);
/// Longest wait between two fast content re-fetches.
const CONTENT_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);
/// Re-fetch interval once a hash has spent its fast attempts: slow, never stopped (QA-0155).
const CONTENT_RETRY_SLOW: std::time::Duration = std::time::Duration::from_secs(300);
/// Content re-fetches in flight per replica.
const MAX_CONTENT_FETCHES: usize = 4;
/// Re-fetches a replica keeps armed at once.
const MAX_CONTENT_RETRIES: usize = 4096;
/// Re-fetch slots one admitted author may hold (QA-0150).
const MAX_CONTENT_RETRIES_PER_AUTHOR: usize = 512;
/// Fast attempts (about 3.5 min) before a hash drops to the slow retry and becomes evictable.
const MAX_CONTENT_ATTEMPTS: u32 = 10;
/// Providers one re-fetch remembers.
const MAX_CONTENT_PROVIDERS: usize = 16;
/// Unarmed hashes one arming sweep examines (round-robin), so a flood of
/// unnamed authors costs bounded work per sweep and never displaces a hash (QA-0157).
const ARM_SWEEP_BUDGET: usize = 4096;
/// How often waiting hashes are offered for arming again.
const ARM_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// Largest row payload a content re-fetch accepts.
const MAX_ROW_CONTENT_BYTES: u64 = 16 * 1024 * 1024;

/// Whether an entry author is a named member of the replica's verse: only such an
/// author arms a re-fetch, so minted author DIDs never take a slot (QA-0155).
type AuthorGate = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Re-fetch state of one pending content hash; see `src/AGENTS.md` §content-refetch.
struct ContentRetry {
    attempt: u32,
    due: tokio::time::Instant,
    providers: Vec<iroh::EndpointId>,
    in_flight: bool,
    /// Admitted authors with a row waiting on this hash; each holds one of its slots here.
    authors: std::collections::BTreeSet<String>,
    armed: tokio::time::Instant,
}

impl ContentRetry {
    fn new() -> Self {
        let now = tokio::time::Instant::now();
        Self {
            attempt: 0,
            due: now + CONTENT_RETRY_FIRST,
            providers: Vec::new(),
            in_flight: false,
            authors: Default::default(),
            armed: now,
        }
    }

    /// Remember `providers`; one not tried before restarts the fast attempts (QA-0155).
    fn add_providers(&mut self, providers: &[iroh::EndpointId]) {
        let mut fresh = false;
        for provider in providers {
            if !self.providers.contains(provider) && self.providers.len() < MAX_CONTENT_PROVIDERS {
                self.providers.push(*provider);
                fresh = true;
            }
        }
        if fresh && self.attempt > 0 {
            self.attempt = 0;
            self.due = self.due.min(tokio::time::Instant::now() + CONTENT_RETRY_FIRST);
        }
    }

    /// The fast attempts ran out: the hash retries slowly and may be evicted.
    fn spent(&self) -> bool {
        self.attempt >= MAX_CONTENT_ATTEMPTS
    }

    /// Exponential backoff (4, 8, 16, 30, 30 ... s), then every 5 min once spent.
    fn failed(&mut self, hash: &iroh_blobs::Hash, error: &anyhow::Error) {
        self.attempt = self.attempt.saturating_add(1);
        self.in_flight = false;
        let delay = if self.spent() {
            CONTENT_RETRY_SLOW
        } else {
            CONTENT_RETRY_FIRST
                .saturating_mul(1u32 << self.attempt.min(5))
                .min(CONTENT_RETRY_MAX)
        };
        self.due = tokio::time::Instant::now() + delay;
        if self.attempt == MAX_CONTENT_ATTEMPTS {
            tracing::warn!(hash = %hash, attempts = self.attempt, "replica entry content unavailable from its providers; retrying every 5 min until a new provider or a rewrite: {error:#}");
        } else if self.attempt == 3 {
            tracing::warn!(hash = %hash, attempt = self.attempt, "replica entry content still unavailable from its providers; retrying: {error:#}");
        } else {
            tracing::debug!(hash = %hash, attempt = self.attempt, "replica entry content re-fetch failed: {error:#}");
        }
    }
}

/// Entries waiting for content, keyed by hash, plus their re-fetch schedule.
#[derive(Default)]
struct PendingRows {
    by_hash: HashMap<iroh_blobs::Hash, Vec<PendingContent>>,
    /// The waiting hash and timestamp of each slot's newest pending entry.
    by_slot: HashMap<(String, String, String), (iroh_blobs::Hash, u64)>,
    retries: HashMap<iroh_blobs::Hash, ContentRetry>,
    /// Re-fetch slots held per admitted author.
    per_author: HashMap<String, usize>,
    /// Waiting hashes with no armed re-fetch (no admitted author yet, or no room), with their
    /// providers. Keyed only by hashes in `by_hash`, so it is bounded by the waiting rows
    /// themselves and never refuses one (QA-0157).
    unarmed: HashMap<iroh_blobs::Hash, Vec<iroh::EndpointId>>,
    /// Sweep order of `unarmed`; a hash that left it is skipped lazily.
    unarmed_order: std::collections::VecDeque<iroh_blobs::Hash>,
    /// Re-fetch slots freed for newer ones (logged at 1, 2, 4, 8 ...).
    evictions: u64,
}

impl PendingRows {
    /// Drop the slot's older waiting entry: iroh-docs replaced it, so its content must never land after this one.
    fn supersede(&mut self, newer: &PendingContent) {
        let slot = newer.slot();
        let Some(&(hash, timestamp)) = self.by_slot.get(&slot) else { return };
        if timestamp >= newer.timestamp {
            return;
        }
        self.by_slot.remove(&slot);
        let Some(waiting) = self.by_hash.get_mut(&hash) else { return };
        waiting.retain(|older| older.slot() != slot);
        if waiting.is_empty() {
            self.by_hash.remove(&hash);
            self.remove_retry(&hash);
            self.unarmed.remove(&hash);
        } else if !waiting.iter().any(|other| other.author_id == slot.0) {
            self.release(&hash, &slot.0);
        }
    }

    /// Park `meta` until its content lands. With `refetch`, an `admitted` author arms a
    /// re-fetch from `providers`; any other waits unarmed for a later arming (QA-0155).
    fn wait(
        &mut self,
        hash: iroh_blobs::Hash,
        meta: PendingContent,
        providers: &[iroh::EndpointId],
        refetch: bool,
        admitted: bool,
    ) {
        self.by_slot.insert(meta.slot(), (hash, meta.timestamp));
        let author = meta.author_id.clone();
        self.by_hash.entry(hash).or_default().push(meta);
        if refetch && !(admitted && self.arm(hash, &author, providers, true)) {
            self.keep_unarmed(hash, providers);
        }
    }

    /// Arm (or join) `hash`'s re-fetch for the admitted `author`; `false` when there is no room.
    /// Room comes only from `author`'s own slots or from spent entries: another principal's
    /// unspent re-fetch is never evicted for a new hash (QA-0155).
    fn arm(&mut self, hash: iroh_blobs::Hash, author: &str, providers: &[iroh::EndpointId], may_evict: bool) -> bool {
        let armed = self.retries.contains_key(&hash);
        let holds = self.retries.get(&hash).is_some_and(|retry| retry.authors.contains(author));
        let mut charge = !holds;
        if charge
            && self.per_author.get(author).copied().unwrap_or(0) >= MAX_CONTENT_RETRIES_PER_AUTHOR
            && !(may_evict && self.evict_own(author, &hash))
        {
            if !armed {
                return false;
            }
            charge = false; // armed for other authors already: it runs without this author's slot
        }
        if !armed && self.retries.len() >= MAX_CONTENT_RETRIES && !(may_evict && self.evict_spent()) {
            return false;
        }
        let retry = self.retries.entry(hash).or_insert_with(ContentRetry::new);
        if charge && retry.authors.insert(author.to_string()) {
            *self.per_author.entry(author.to_string()).or_default() += 1;
        }
        if let Some(known) = self.unarmed.remove(&hash) {
            retry.add_providers(&known);
        }
        retry.add_providers(providers);
        true
    }

    /// Remember a waiting hash for a later arming (an armed or no-longer-waiting hash needs nothing).
    fn keep_unarmed(&mut self, hash: iroh_blobs::Hash, providers: &[iroh::EndpointId]) {
        if self.retries.contains_key(&hash) || !self.by_hash.contains_key(&hash) {
            return;
        }
        let known = match self.unarmed.entry(hash) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.unarmed_order.push_back(hash);
                entry.insert(Vec::new())
            }
        };
        for provider in providers {
            if !known.contains(provider) && known.len() < MAX_CONTENT_PROVIDERS {
                known.push(*provider);
            }
        }
    }

    /// Offer the next `ARM_SWEEP_BUDGET` unarmed hashes (round-robin) to their admitted
    /// waiting authors, without evicting: an author admitted after its row arrived, or room
    /// freed since (QA-0155). Each author is judged once per sweep (QA-0157).
    fn arm_waiting(&mut self, admitted: &dyn Fn(&str) -> bool) {
        if self.unarmed_order.len() > 2 * self.unarmed.len() + ARM_SWEEP_BUDGET {
            let (unarmed, mut seen) = (&self.unarmed, std::collections::HashSet::new());
            self.unarmed_order.retain(|hash| unarmed.contains_key(hash) && seen.insert(*hash));
        }
        let mut verdicts: HashMap<String, bool> = HashMap::new();
        for _ in 0..self.unarmed_order.len().min(ARM_SWEEP_BUDGET) {
            let Some(hash) = self.unarmed_order.pop_front() else { break };
            if !self.unarmed.contains_key(&hash) {
                continue;
            }
            let Some(waiting) = self.by_hash.get(&hash) else {
                self.unarmed.remove(&hash);
                continue;
            };
            let authors: std::collections::BTreeSet<String> = waiting
                .iter()
                .map(|meta| meta.author_id.clone())
                .filter(|author| *verdicts.entry(author.clone()).or_insert_with(|| admitted(author)))
                .collect();
            for author in authors {
                self.arm(hash, &author, &[], false);
            }
            if self.unarmed.contains_key(&hash) {
                self.unarmed_order.push_back(hash);
            }
        }
    }

    /// Free one of `author`'s own slots (never the one on `keep`): its spent, then
    /// most-failed, then oldest idle re-fetch; `false` when it holds none.
    fn evict_own(&mut self, author: &str, keep: &iroh_blobs::Hash) -> bool {
        let victim = self
            .retries
            .iter()
            .filter(|(hash, retry)| *hash != keep && !retry.in_flight && retry.authors.contains(author))
            .max_by_key(|(_, retry)| (retry.spent(), retry.attempt, std::cmp::Reverse(retry.armed)))
            .map(|(hash, _)| *hash);
        let Some(victim) = victim else { return false };
        self.release(&victim, author);
        self.note_eviction(&victim, "own cap");
        true
    }

    /// Drop the most-failed, then oldest, idle spent re-fetch; `false` when none is spent.
    fn evict_spent(&mut self) -> bool {
        let victim = self
            .retries
            .iter()
            .filter(|(_, retry)| retry.spent() && !retry.in_flight)
            .max_by_key(|(_, retry)| (retry.attempt, std::cmp::Reverse(retry.armed)))
            .map(|(hash, _)| *hash);
        let Some(victim) = victim else { return false };
        if let Some(retry) = self.remove_retry(&victim) {
            self.park(victim, retry);
        }
        self.note_eviction(&victim, "table full");
        true
    }

    fn note_eviction(&mut self, hash: &iroh_blobs::Hash, reason: &str) {
        self.evictions += 1;
        if self.evictions.is_power_of_two() {
            tracing::warn!(hash = %hash, reason, evictions = self.evictions, "content re-fetch slot freed for a newer one (the row still lands on ContentReady, a rewrite or a later arming)");
        }
    }

    /// `author` stops holding a slot in `hash`'s re-fetch; one nobody holds is parked unarmed.
    fn release(&mut self, hash: &iroh_blobs::Hash, author: &str) {
        let Some(retry) = self.retries.get_mut(hash) else { return };
        if !retry.authors.remove(author) {
            return;
        }
        let orphaned = retry.authors.is_empty();
        self.uncharge(author);
        if orphaned {
            if let Some(retry) = self.retries.remove(hash) {
                self.park(*hash, retry);
            }
        }
    }

    /// Keep a dropped re-fetch's providers so its waiting rows can be armed again later.
    fn park(&mut self, hash: iroh_blobs::Hash, retry: ContentRetry) {
        if self.by_hash.contains_key(&hash) {
            self.keep_unarmed(hash, &retry.providers);
        }
    }

    fn uncharge(&mut self, author: &str) {
        if let Some(count) = self.per_author.get_mut(author) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.per_author.remove(author);
            }
        }
    }

    /// Forget `hash`'s re-fetch schedule and release every author's slot in it.
    fn remove_retry(&mut self, hash: &iroh_blobs::Hash) -> Option<ContentRetry> {
        let retry = self.retries.remove(hash)?;
        for author in &retry.authors {
            self.uncharge(author);
        }
        Some(retry)
    }

    /// Record a failed re-fetch (it keeps retrying; see `ContentRetry::failed`).
    fn fetch_failed(&mut self, hash: &iroh_blobs::Hash, error: &anyhow::Error) {
        if let Some(retry) = self.retries.get_mut(hash) {
            retry.failed(hash, error);
        }
    }

    /// Take every entry waiting on `hash` (its content is now local).
    fn take(&mut self, hash: &iroh_blobs::Hash) -> Vec<PendingContent> {
        self.remove_retry(hash);
        self.unarmed.remove(hash);
        let waiting = self.by_hash.remove(hash).unwrap_or_default();
        for meta in &waiting {
            if self.by_slot.get(&meta.slot()).is_some_and(|(slot_hash, _)| slot_hash == hash) {
                self.by_slot.remove(&meta.slot());
            }
        }
        waiting
    }

    /// The earliest due re-fetch not already running, while a fetch slot is free.
    fn next_due(&self, running: usize) -> Option<tokio::time::Instant> {
        if running >= MAX_CONTENT_FETCHES {
            return None;
        }
        self.retries.values().filter(|retry| !retry.in_flight).map(|retry| retry.due).min()
    }
}

/// Wait before syncing a local write that had no gossip neighbor (bursts coalesce).
const ISOLATED_SYNC_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
/// Shortest gap between two such syncs of one replica.
const ISOLATED_SYNC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// Peers a replica remembers as direct-sync targets.
const MAX_KNOWN_SYNC_PEERS: usize = 256;

/// Direct sync for local writes made while no gossip neighbor is live; see `src/AGENTS.md` §isolated-write-sync.
#[derive(Default)]
struct IsolatedWrites {
    neighbors: HashSet<iroh::EndpointId>,
    known: HashSet<iroh::EndpointId>,
    due: Option<tokio::time::Instant>,
    last: Option<tokio::time::Instant>,
}

impl IsolatedWrites {
    fn note_peer(&mut self, peer: iroh::EndpointId) {
        if self.known.len() < MAX_KNOWN_SYNC_PEERS {
            self.known.insert(peer);
        }
    }

    /// A local write: its gossip broadcast reached no one when no neighbor is live.
    fn local_write(&mut self) {
        if !self.neighbors.is_empty() || self.due.is_some() {
            return;
        }
        let earliest = tokio::time::Instant::now() + ISOLATED_SYNC_DELAY;
        let throttled = self.last.map_or(earliest, |last| last + ISOLATED_SYNC_INTERVAL);
        self.due = Some(earliest.max(throttled));
    }

    /// The peers to sync with now (none once a neighbor came up meanwhile).
    fn take_due(&mut self) -> Vec<iroh::EndpointId> {
        self.due = None;
        self.last = Some(tokio::time::Instant::now());
        if self.neighbors.is_empty() {
            self.known.iter().copied().collect()
        } else {
            Vec::new()
        }
    }
}

/// Fetch `hash` from the first provider that serves it (BAO-verified, size-capped).
async fn fetch_row_content(
    fetcher: &crate::blob_transport::IrohBlobTransport,
    hash: iroh_blobs::Hash,
    providers: &[iroh::EndpointId],
) -> anyhow::Result<Bytes> {
    use fe_runtime::blob_transport::BlobTransport as _;
    let mut last_error = anyhow::anyhow!("no provider known for the entry content");
    for provider in providers {
        let provider_ref =
            crate::blob_transport::IrohBlobTransport::provider_ref(&iroh::EndpointAddr::new(*provider))?;
        match fetcher.fetch(&provider_ref, *hash.as_bytes(), MAX_ROW_CONTENT_BYTES).await {
            Ok(data) => return Ok(data),
            Err(error) => last_error = error.context(format!("provider {}", provider.fmt_short())),
        }
    }
    Err(last_error)
}

/// Emit the rows waiting on one content blob; `false` once the receiver is gone.
async fn emit_waiting_rows(out: &mpsc::Sender<RowChange>, waiting: Vec<PendingContent>, data: &[u8]) -> bool {
    for meta in waiting {
        let change = RowChange {
            table: meta.table,
            record_id: meta.record_id,
            content_hash: meta.content_hash,
            author_id: meta.author_id,
            timestamp: meta.timestamp,
            is_tombstone: row_is_tombstone(data),
            data: data.to_vec(),
        };
        if out.send(change).await.is_err() {
            return false;
        }
    }
    true
}

/// Map an iroh peer public key to the app's `did:key` identity form.
///
/// The endpoint identity's `EndpointId` did:key IS the app DID (fe-identity's
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

/// Inverse of [`author_did_key`]: parse an app-level `did:key` string back
/// to the iroh-docs `AuthorId` it was derived from — the §2.5 re-read seam
/// (`entries_by_author`) needs this to query the replica by author.
fn author_id_from_did_key(did: &str) -> anyhow::Result<AuthorId> {
    let verifying_key = fe_identity::did_key::public_key_from_did_key(did)?;
    Ok(AuthorId::from(verifying_key.to_bytes()))
}

/// Shared `doc.get_many(query)` → `Vec<RowChange>` loop, used by both
/// `snapshot` passes (single-latest-per-key, then the §2.5 all-authors
/// `role/` pass) and by `entries_by_author` (once per table prefix). A
/// `get_many` failure or a malformed key is a loud warn, never a
/// propagated error — the same best-effort stance the single pass this
/// replaces already took per entry.
async fn collect_query_rows(
    doc: &Doc,
    blobs: &iroh_blobs::api::Store,
    query: iroh_docs::store::Query,
) -> Vec<RowChange> {
    let stream = match doc.get_many(query).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("replica query failed: {e}");
            return Vec::new();
        }
    };
    let mut stream = std::pin::pin!(stream);
    let mut out = Vec::new();
    while let Some(entry) = stream.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!("replica query entry failed: {e}");
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
        match read_blob_bytes(blobs, &entry.content_hash()).await {
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
                // Content not local yet — the live pump emits it when the
                // download completes, so nothing is lost.
                tracing::debug!(
                    key = %key,
                    "replica query: content not local — deferred to live pump: {e}"
                );
            }
        }
    }
    out
}

/// Read one blob's full content bytes from the iroh-blobs store.
///
/// Completeness is checked first so an absent or partial blob is a clean
/// error (the callers defer it to the live pump), never a partial read.
async fn read_blob_bytes(
    store: &iroh_blobs::api::Store,
    hash: &iroh_blobs::Hash,
) -> anyhow::Result<Bytes> {
    use iroh_blobs::api::blobs::BlobStatus;

    match store
        .status(*hash)
        .await
        .map_err(|e| anyhow::anyhow!("blob store lookup: {e}"))?
    {
        BlobStatus::Complete { .. } => {}
        BlobStatus::NotFound => anyhow::bail!("blob {} not in the local store", hash),
        BlobStatus::Partial { .. } => anyhow::bail!("blob {} is incomplete", hash),
    }
    store
        .get_bytes(*hash)
        .await
        .map_err(|e| anyhow::anyhow!("reading blob bytes: {e}"))
}

/// Fallback re-dial for a peer still unconfirmed after a failed sync: at most
/// once per peer per `RESYNC_COOLDOWN`, and only if no other evidence (a later
/// successful sync or delivered content) confirmed it during the delay. A
/// re-dial re-joins gossip and makes the remote dial back, so an unthrottled
/// retry storms (QA-0101; fe-sync `src/AGENTS.md` §join-readiness).
fn schedule_resync(
    doc: &DocHandle,
    readiness: &Arc<JoinReadiness>,
    peer: iroh::EndpointId,
    last_attempt: &mut HashMap<iroh::EndpointId, std::time::Instant>,
) {
    const RESYNC_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(60);
    if last_attempt.get(&peer).is_some_and(|at| at.elapsed() < RESYNC_COOLDOWN)
        || (last_attempt.len() >= 256 && !last_attempt.contains_key(&peer))
    {
        return;
    }
    last_attempt.insert(peer, std::time::Instant::now());
    // Jitter from the clock's sub-second nanos so two dialers drift apart.
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_nanos()) % 1000)
        .unwrap_or(0);
    let delay = std::time::Duration::from_millis(1000 + jitter);
    let doc = doc.clone();
    let readiness = readiness.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let confirmed = readiness
            .successful_peers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&peer);
        if confirmed || !readiness.alive.load(Ordering::Acquire) {
            return;
        }
        if let Err(error) = doc.start_sync(vec![iroh::EndpointAddr::new(peer)]).await {
            tracing::debug!(peer = %peer_did_key(&peer), "replica re-sync request failed: {error}");
        }
    });
}

/// Record `peer` as a live, authenticated sync partner (bounded).
fn confirm_peer(readiness: &JoinReadiness, peer: iroh::EndpointId) {
    let mut peers = readiness.successful_peers.lock().unwrap_or_else(|error| error.into_inner());
    if peers.len() < 256 {
        peers.insert(peer);
    }
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
/// - A failed iroh-docs download is terminal, so `fetcher` re-fetches pending
///   content from the forwarder and the author with backoff (QA-0145,
///   `src/AGENTS.md` §content-refetch); a newer entry in the same slot drops the older one.
///   Only an author `author_gate` names arms one (`None`: every author, QA-0155).
async fn pump_live_events(
    mut stream: std::pin::Pin<Box<dyn Stream<Item = anyhow::Result<LiveEvent>> + Send>>,
    blobs: iroh_blobs::api::Store,
    out: mpsc::Sender<RowChange>,
    readiness: Arc<JoinReadiness>,
    resync: Option<DocHandle>,
    fetcher: Option<Arc<crate::blob_transport::IrohBlobTransport>>,
    author_gate: Option<AuthorGate>,
) {
    readiness.alive.store(true, Ordering::Release);
    // QA-0101: when each peer was last re-dialed (fallback cooldown).
    let mut resync_attempts: HashMap<iroh::EndpointId, std::time::Instant> = HashMap::new();
    let _readiness_guard = JoinReadinessGuard(readiness.clone());
    let mut pending = PendingRows::default();
    let mut fetches: tokio::task::JoinSet<(iroh_blobs::Hash, anyhow::Result<Bytes>)> =
        tokio::task::JoinSet::new();
    let mut isolated = IsolatedWrites::default();
    let admitted = |author: &str| author_gate.as_ref().is_none_or(|gate| gate(author));
    let mut last_arm_sweep = tokio::time::Instant::now();

    loop {
        let next_due = pending.next_due(fetches.len());
        let isolated_due = isolated.due;
        let arm_sweep = (fetcher.is_some() && !pending.unarmed.is_empty()).then_some(last_arm_sweep + ARM_SWEEP_INTERVAL);
        let event = tokio::select! {
            event = stream.next() => match event {
                Some(event) => event,
                None => break,
            },
            // QA-0155: a waiting row whose author was admitted later (or that found no room) arms now.
            _ = tokio::time::sleep_until(arm_sweep.unwrap_or_else(tokio::time::Instant::now)), if arm_sweep.is_some() => {
                last_arm_sweep = tokio::time::Instant::now();
                pending.arm_waiting(&admitted);
                continue;
            }
            _ = tokio::time::sleep_until(isolated_due.unwrap_or_else(tokio::time::Instant::now)), if isolated_due.is_some() => {
                let peers = isolated.take_due();
                if let (Some(doc), false) = (&resync, peers.is_empty()) {
                    tracing::info!(peers = peers.len(), "local write made while no gossip neighbor is live; syncing it to known peers directly");
                    let doc = doc.clone();
                    tokio::spawn(async move {
                        if let Err(error) = doc.start_sync(peers.into_iter().map(iroh::EndpointAddr::new).collect()).await {
                            tracing::debug!("isolated-write sync request failed: {error}");
                        }
                    });
                }
                continue;
            }
            Some(joined) = fetches.join_next(), if !fetches.is_empty() => {
                let Ok((hash, fetched)) = joined else { continue };
                match fetched {
                    Ok(data) => {
                        let waiting = pending.take(&hash);
                        if !waiting.is_empty() {
                            tracing::info!(hash = %hash, rows = waiting.len(), "replica entry content re-fetched after its docs download failed");
                            if !emit_waiting_rows(&out, waiting, &data).await {
                                return;
                            }
                        }
                    }
                    Err(error) => pending.fetch_failed(&hash, &error),
                }
                continue;
            }
            _ = tokio::time::sleep_until(next_due.unwrap_or_else(tokio::time::Instant::now)), if next_due.is_some() => {
                let Some(fetcher) = &fetcher else { continue };
                let now = tokio::time::Instant::now();
                for (hash, retry) in pending.retries.iter_mut() {
                    if fetches.len() >= MAX_CONTENT_FETCHES {
                        break;
                    }
                    if retry.in_flight || retry.due > now {
                        continue;
                    }
                    retry.in_flight = true;
                    let (fetcher, hash, providers) = (fetcher.clone(), *hash, retry.providers.clone());
                    fetches.spawn(async move { (hash, fetch_row_content(&fetcher, hash, &providers).await) });
                }
                continue;
            }
        };
        let event = match event {
            Ok(event) => event,
            Err(e) => {
                readiness.successful_peers.lock().unwrap_or_else(|error| error.into_inner()).clear();
                tracing::warn!("replica event stream error: {e}");
                continue;
            }
        };
        match event {
            LiveEvent::InsertLocal { entry } => {
                // Loop prevention at the source: our own write echoing back.
                tracing::debug!(key = ?String::from_utf8_lossy(entry.key()), "local insert skipped");
                isolated.local_write();
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
                // never from `from` — iroh-docs' `InsertRemote.from`
                // is the FORWARDING neighbor, not the author. With the docs
                // author imported from the endpoint identity at stack
                // spawn, `entry.author()` maps to the same did:key the A3
                // gate resolves, so >=3-member docs judge the author, not
                // the relaying node, and the snapshot path (which always
                // read `entry.author()`) agrees with live delivery. `from`
                // is retained only as tracing context.
                confirm_peer(&readiness, from);
                isolated.note_peer(from);
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
                pending.supersede(&meta);
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
                        // The forwarder and the author both may hold the content; never ask ourselves.
                        let author = iroh::EndpointId::from_bytes(entry.author().as_bytes()).ok();
                        let own = fetcher.as_ref().map(|fetcher| fetcher.local_id());
                        let providers: Vec<iroh::EndpointId> = [Some(from), author]
                            .into_iter()
                            .flatten()
                            .filter(|provider| Some(*provider) != own)
                            .collect();
                        let author_admitted = admitted(&meta.author_id);
                        pending.wait(entry.content_hash(), meta, &providers, fetcher.is_some(), author_admitted);
                    }
                }
            }
            LiveEvent::ContentReady { hash } => {
                let waiting = pending.take(&hash);
                if waiting.is_empty() {
                    continue;
                }
                let data = match read_blob_bytes(&blobs, &hash).await {
                    Ok(data) => data,
                    Err(e) => {
                        tracing::warn!(hash = %hash, "content-ready read failed: {e}");
                        continue;
                    }
                };
                if !emit_waiting_rows(&out, waiting, &data).await {
                    return;
                }
            }
            LiveEvent::NeighborUp(peer) => {
                isolated.neighbors.insert(peer);
                isolated.note_peer(peer);
                tracing::debug!(peer = %peer_did_key(&peer), "replica swarm neighbor up");
            }
            LiveEvent::NeighborDown(peer) => {
                isolated.neighbors.remove(&peer);
                readiness.successful_peers.lock().unwrap_or_else(|error| error.into_inner()).remove(&peer);
                tracing::debug!(peer = %peer_did_key(&peer), "replica swarm neighbor down");
            }
            LiveEvent::SyncFinished(event) => match &event.result {
                Ok(_) => {
                    isolated.note_peer(event.peer);
                    confirm_peer(&readiness, event.peer);
                }
                Err(error) => {
                    isolated.note_peer(event.peer);
                    // A close failure is the simultaneous-dial artifact: the other
                    // leg carried the sync, so it is not evidence against the peer.
                    if !error.contains("Failed to close") {
                        readiness.successful_peers.lock().unwrap_or_else(|error| error.into_inner()).remove(&event.peer);
                    }
                    if let Some(doc) = &resync {
                        schedule_resync(doc, &readiness, event.peer, &mut resync_attempts);
                    }
                }
            },
            LiveEvent::PendingContentReady => {
                tracing::debug!("replica sync progress event");
            }
        }
    }
    if !pending.by_hash.is_empty() {
        tracing::warn!(
            count = pending.by_hash.len(),
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

/// Petal-level replicator using iroh-docs.
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
        let addr_a = ep_a.addr();
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
        let alice_endpoint_did = peer_did_key(&ep_a.id());
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

        // QA-0164: an identical rewrite is not a new version, so Bob's next event is the real change.
        alice.write_row("verse", "verse-a2", &payload).await.expect("identical rewrite");
        let renamed = br#"{"verse_id":"verse-a2","name":"Renamed Verse"}"#.to_vec();
        alice.write_row("verse", "verse-a2", &renamed).await.expect("alice renames");
        let next = tokio::time::timeout(std::time::Duration::from_secs(20), bob_rx.recv())
            .await
            .expect("the rename arrives")
            .expect("pump stays alive");
        assert_eq!(next.data, renamed, "the identical rewrite published nothing");
        // ...unless another author's newer entry holds the key: then the same bytes must win it back.
        let bobs = br#"{"verse_id":"verse-a2","name":"Bob's Name"}"#.to_vec();
        bob.write_row("verse", "verse-a2", &bobs).await.expect("bob renames");
        let alice_doc = alice.live_doc().expect("alice's doc is live");
        let newest = || iroh_docs::store::Query::single_latest_per_key().key_exact(b"verse/verse-a2").build();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !alice_doc.get_one(newest()).await.ok().flatten()
                .is_some_and(|entry| entry.content_hash() == iroh_blobs::Hash::new(&bobs))
            {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.expect("bob's rename reaches alice");
        alice.write_row("verse", "verse-a2", &renamed).await.expect("alice restores her name");
        let restored = tokio::time::timeout(std::time::Duration::from_secs(20), bob_rx.recv())
            .await
            .expect("alice's restore arrives")
            .expect("pump stays alive");
        assert_eq!(restored.data, renamed, "a rewrite another author overtook is a new version");

        // QA-0145: Bob's iroh-docs download of a row's content fails (here Alice
        // refuses it; in the field a restarted author is dialed on its dead old
        // path) and iroh-docs never retries — the row must still arrive once
        // Alice serves it.
        let late = br#"{"verse_id":"verse-a2","name":"Served Late"}"#.to_vec();
        let late_hash = iroh_blobs::Hash::new(&late);
        stack_a.blob_access_gate().record_tileset_root("gate-verse", *late_hash.as_bytes());
        alice.write_row("verse", "verse-late", &late).await.expect("alice writes the refused row");
        let bob_doc = bob.live_doc().expect("bob's doc is live");
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while bob_doc.get_one(iroh_docs::store::Query::key_exact(b"verse/verse-late")).await.ok().flatten().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.expect("the entry reaches bob");
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !matches!(stack_b.blob_store().blobs().status(late_hash).await, Ok(iroh_blobs::api::blobs::BlobStatus::Complete { .. })),
            "bob's docs download of the refused content failed"
        );
        // QA-0155: Alice is not yet a named member of Bob's verse, so her row waits unarmed;
        // once Bob's role view names her, the arming sweep schedules the re-fetch.
        stack_b.blob_access_gate().set_verse_access(
            "verse-a2",
            crate::docs_engine::VerseAccessSnapshot { owner_did: Some(alice_endpoint_did.clone()), open_to_all: false, viewer_dids: HashSet::new() },
        );
        stack_a.blob_access_gate().set_verse_access(
            "gate-verse",
            crate::docs_engine::VerseAccessSnapshot { owner_did: None, open_to_all: true, viewer_dids: HashSet::new() },
        );
        let late_change = tokio::time::timeout(std::time::Duration::from_secs(20), bob_rx.recv())
            .await
            .expect("the row arrives once its author serves the content")
            .expect("pump stays alive");
        assert_eq!((late_change.record_id.as_str(), late_change.data.as_slice()), ("verse-late", late.as_slice()));

        let native_ns = hex::encode(fe_database::derive_namespace_id(&secret));
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !bob.joined_peer_ready(&native_ns, &alice_endpoint_did) {
                tokio::task::yield_now().await;
            }
        }).await.expect("actual issuer sync completion precedes the error fixture");
        let readiness = Arc::new(JoinReadiness::default());
        readiness.successful_peers.lock().unwrap().insert(ep_a.id());
        let errors = futures_lite::stream::iter([Err(anyhow::anyhow!("controlled stream error"))])
            .chain(futures_lite::stream::pending());
        let (error_tx, _error_rx) = mpsc::channel(1);
        let error_pump = tokio::spawn(pump_live_events(
            Box::pin(errors), stack_b.blob_store().clone(), error_tx, readiness.clone(), None, None, None,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if readiness.alive.load(Ordering::Acquire) && readiness.successful_peers.lock().unwrap().is_empty() { break; }
                tokio::task::yield_now().await;
            }
        }).await.expect("a continuing errored stream invalidates earlier synchronization proof");
        assert!(readiness.alive.load(Ordering::Acquire), "error handling continues with no stale readiness");
        error_pump.abort();
        assert!(error_pump.await.unwrap_err().is_cancelled());

        // QA-0145: a local write made while no gossip neighbor is live (a
        // restarted node whose rejoin went unanswered) reached no one, so it is
        // synced directly to the known peers: coalesced, throttled, and skipped
        // once a neighbor is back.
        let mut isolated = IsolatedWrites::default();
        isolated.neighbors.insert(ep_a.id());
        isolated.local_write();
        assert!(isolated.due.is_none(), "a live neighbor carries the broadcast");
        isolated.neighbors.clear();
        isolated.note_peer(ep_a.id());
        let before = tokio::time::Instant::now();
        isolated.local_write();
        let first_due = isolated.due.expect("an isolated write schedules a direct sync");
        assert!(first_due >= before + ISOLATED_SYNC_DELAY, "a write burst coalesces into one sync");
        isolated.local_write();
        assert_eq!(isolated.due, Some(first_due), "later writes join the scheduled sync");
        assert_eq!(isolated.take_due(), vec![ep_a.id()]);
        isolated.local_write();
        assert!(isolated.due.expect("rescheduled") >= isolated.last.expect("synced") + ISOLATED_SYNC_INTERVAL, "syncs are throttled");
        isolated.neighbors.insert(ep_a.id());
        assert!(isolated.take_due().is_empty(), "a neighbor that came back makes the sync moot");

        // QA-0150 / QA-0155: only named verse members arm re-fetches; a member fills
        // only its own share; a full table evicts nothing unspent for a new hash; a hash
        // past its fast attempts keeps a slow retry, makes room, and a new provider revives it.
        let row = |author: &str, n: usize| {
            let content = format!("{author}-{n}");
            let hash = iroh_blobs::Hash::new(content.as_bytes());
            let meta = PendingContent {
                table: "node".into(),
                record_id: content,
                content_hash: *hash.as_bytes(),
                author_id: author.into(),
                timestamp: 1,
            };
            (hash, meta)
        };
        let (provider, second_provider) = ([ep_a.id()], [ep_b.id()]);
        let unfetchable = anyhow::anyhow!("provider refused");
        let mut pending = PendingRows::default();
        let (honest, honest_row) = row("did:honest", 0);
        pending.wait(honest, honest_row, &provider, true, true);
        for sybil in 0..100 {
            for n in 0..64 {
                let (hash, meta) = row(&format!("did:sybil-{sybil}"), n);
                pending.wait(hash, meta, &provider, true, false);
            }
        }
        assert_eq!(pending.retries.len(), 1, "100 minted authors never take a re-fetch slot");
        assert!(pending.retries.contains_key(&honest), "Sybil authors cannot evict an honest pending hash");
        let granted = row("did:sybil-0", 0).0;
        pending.arm_waiting(&|author: &str| author == "did:sybil-0");
        assert!(pending.retries.contains_key(&granted), "an author named after its rows arrived arms on the sweep");
        for n in 0..MAX_CONTENT_RETRIES_PER_AUTHOR + 64 {
            let (hash, meta) = row("did:flooder", n);
            pending.wait(hash, meta, &provider, true, true);
        }
        assert_eq!(pending.per_author["did:flooder"], MAX_CONTENT_RETRIES_PER_AUTHOR, "one member holds at most its own share");
        assert!(pending.retries.contains_key(&honest), "a member at its cap evicts only its own re-fetches");
        for author in 1..MAX_CONTENT_RETRIES / MAX_CONTENT_RETRIES_PER_AUTHOR {
            for n in 0..MAX_CONTENT_RETRIES_PER_AUTHOR {
                let (hash, meta) = row(&format!("did:author-{author}"), n);
                pending.wait(hash, meta, &provider, true, true);
            }
        }
        assert_eq!(pending.retries.len(), MAX_CONTENT_RETRIES, "the table is full");
        let (legit, legit_row) = row("did:legit", 0);
        pending.wait(legit, legit_row, &provider, true, true);
        assert!(!pending.retries.contains_key(&legit) && pending.unarmed.contains_key(&legit), "no unspent re-fetch is evicted for a new hash; it waits for room");
        let stale = *pending.retries.iter().find(|(_, retry)| retry.authors.contains("did:flooder")).expect("flooder entry").0;
        for _ in 0..MAX_CONTENT_ATTEMPTS {
            pending.fetch_failed(&stale, &unfetchable);
        }
        assert!(pending.retries[&stale].spent(), "fast attempts are bounded");
        assert!(pending.retries[&stale].due > tokio::time::Instant::now() + CONTENT_RETRY_MAX, "a spent hash keeps a slow retry");
        pending.arm_waiting(&|_: &str| true);
        assert!(!pending.retries.contains_key(&legit), "a sweep never evicts");
        let rewritten = PendingContent { timestamp: 2, ..row("did:legit", 0).1 };
        pending.supersede(&rewritten);
        pending.wait(legit, rewritten, &provider, true, true);
        assert!(pending.retries.contains_key(&legit) && !pending.retries.contains_key(&stale), "only a spent re-fetch makes room");
        assert!(pending.unarmed.contains_key(&stale), "the evicted hash still waits for a later arming");
        for _ in 0..MAX_CONTENT_ATTEMPTS {
            pending.fetch_failed(&legit, &unfetchable);
        }
        let mirror = PendingContent { record_id: "mirror".into(), ..row("did:legit", 0).1 };
        pending.wait(legit, mirror, &second_provider, true, true);
        assert_eq!(pending.retries[&legit].attempt, 0, "a second provider revives a spent hash's fast attempts");
        assert!(pending.retries[&legit].due <= tokio::time::Instant::now() + CONTENT_RETRY_FIRST);
        assert_eq!(pending.take(&legit).len(), 2, "every row waiting on the content lands with it");
        assert!(!pending.per_author.contains_key("did:legit"), "taking the content releases its author's slot");

        // QA-0157: Sybil authors fill no table an honest author's hash can be refused from.
        // An editor whose role statement lands after its row waits unarmed behind more
        // unnamed hashes than any former cap, and the bounded round-robin sweep still arms it.
        let mut flooded = PendingRows::default();
        for sybil in 0..3 * ARM_SWEEP_BUDGET + 17 {
            let (hash, meta) = row(&format!("did:mint-{sybil}"), 0);
            flooded.wait(hash, meta, &provider, true, false);
        }
        let (late, late_row) = row("did:late-editor", 0);
        flooded.wait(late, late_row, &provider, true, false);
        assert!(flooded.unarmed.contains_key(&late), "a hash whose author is not yet named is parked, never refused for room");
        let named = |author: &str| author == "did:late-editor";
        let sweeps = (1..=flooded.unarmed.len().div_ceil(ARM_SWEEP_BUDGET))
            .find(|_| {
                flooded.arm_waiting(&named);
                flooded.retries.contains_key(&late)
            })
            .expect("the late-named author's content is re-fetched within one pass over the waiting hashes");
        assert_eq!(sweeps, 4, "each sweep examines a bounded slice, round-robin");
        assert_eq!(flooded.retries.len(), 1, "minted authors still arm nothing");

        // Close must leave the swarm cleanly on both sides.
        alice.close().await.unwrap();
        bob.close().await.unwrap();
    }

    /// RP-5 §2.5 step 3: `single_latest_per_key` picks the single
    /// PHYSICALLY newest entry at a key across every author who wrote it —
    /// so an attacker who writes a forged, later-timestamped entry to the
    /// SAME key as the owner's genuine role statement (grantor mismatch,
    /// permanently denied once read) would shadow the owner's real entry
    /// from that query entirely. `snapshot()`'s all-authors `role/` pass
    /// must still surface the owner's genuine entry alongside the forged
    /// one. Two real endpoints, two real authors, over loopback.
    #[tokio::test]
    async fn snapshot_replay_sees_the_owners_role_statement_despite_a_later_forged_entry_on_the_same_key(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let (Some(ep_owner), Some(ep_attacker)) = (
            bind_loopback_endpoint(51).await,
            bind_loopback_endpoint(52).await,
        ) else {
            return; // sandboxed environment without UDP — tolerated
        };

        let stack_owner = Arc::new(
            DocsStack::spawn(ep_owner.clone(), tmp.path().join("owner"))
                .await
                .expect("owner stack spawns"),
        );
        let stack_attacker = Arc::new(
            DocsStack::spawn(ep_attacker.clone(), tmp.path().join("attacker"))
                .await
                .expect("attacker stack spawns"),
        );
        let holder_owner = Arc::new(IrohDocsEngineHolder::online(stack_owner.clone()));
        let holder_attacker = Arc::new(IrohDocsEngineHolder::online(stack_attacker.clone()));

        let secret = [11u8; 32];
        let ns_id_hex = hex::encode(secret);
        let owner = IrohDocsReplicator::new(
            "verse-r5".to_string(),
            ns_id_hex.clone(),
            hex::encode(secret),
            "did:key:owner-app".to_string(),
            holder_owner,
        );
        let attacker = IrohDocsReplicator::new(
            "verse-r5".to_string(),
            ns_id_hex,
            hex::encode(secret),
            "did:key:attacker-app".to_string(),
            holder_attacker,
        );
        owner.open_document().await.expect("owner opens doc");
        attacker.open_document().await.expect("attacker opens doc");

        // F20: the AUTHENTICATED author is the endpoint identity, never the
        // app-level did string passed to `new` — the owner's genuine grant
        // and the attacker's forgery must both be keyed on this.
        let owner_did = peer_did_key(&ep_owner.id());
        let attacker_did = peer_did_key(&ep_attacker.id());
        let record_id = format!("VERSE#verse-r5/did:key:peer/{owner_did}");

        let mut owner_rx = owner.subscribe().await.expect("owner subscribes");
        let mut attacker_rx = attacker.subscribe().await.expect("attacker subscribes");
        attacker
            .start_sync(vec![ep_owner.addr()])
            .await
            .expect("attacker syncs with owner");
        owner
            .start_sync(Vec::new())
            .await
            .expect("owner's doc serves entries");

        // The owner's genuine role statement.
        let genuine = br#"{"scope":"VERSE#verse-r5","peer_did":"did:key:peer","grantor_did":"PLACEHOLDER","role":"editor","revoked":false,"hlc":1}"#;
        let genuine = String::from_utf8_lossy(genuine).replace("PLACEHOLDER", &owner_did);
        owner
            .write_row("role", &record_id, genuine.as_bytes())
            .await
            .expect("owner writes the genuine statement");

        // Wait for the genuine entry to actually reach the attacker side —
        // this is what makes the attacker's write strictly LATER (real
        // wall-clock time), the exact precondition under which
        // `single_latest_per_key` would pick the forgery over the original.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), attacker_rx.recv())
            .await
            .expect("the genuine entry reaches the attacker side");

        // The attacker writes a forged entry to the SAME key, claiming the
        // owner as grantor — the key itself is just a string; nothing
        // about iroh-docs stops another author writing to it.
        let forged = br#"{"scope":"VERSE#verse-r5","peer_did":"did:key:peer","grantor_did":"PLACEHOLDER","role":"manager","revoked":false,"hlc":99}"#;
        let forged = String::from_utf8_lossy(forged).replace("PLACEHOLDER", &owner_did);
        attacker
            .write_row("role", &record_id, forged.as_bytes())
            .await
            .expect("attacker writes the forged statement");

        // Wait for the forgery to reach the owner's own store too.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(20), owner_rx.recv())
            .await
            .expect("the forged entry reaches the owner side");

        let snap = owner.snapshot().await.expect("owner snapshots the doc");
        let role_rows: Vec<&RowChange> = snap.iter().filter(|c| c.table == "role").collect();
        let owner_row = role_rows
            .iter()
            .find(|c| c.author_id == owner_did && c.record_id == record_id)
            .expect(
                "the owner's genuine entry must be visible despite a later forged entry on the \
                 same key — this is exactly what the all-authors `role/` pass exists for",
            );
        assert_eq!(owner_row.data, genuine.as_bytes());
        let attacker_row = role_rows
            .iter()
            .find(|c| c.author_id == attacker_did && c.record_id == record_id)
            .expect(
                "the forged entry is also visible (and separately denied by validate_statement)",
            );
        assert_eq!(attacker_row.data, forged.as_bytes());

        owner.close().await.unwrap();
        attacker.close().await.unwrap();
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
