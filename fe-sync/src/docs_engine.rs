//! Real iroh 1.x P2P stack (iroh-docs 0.101 / iroh-blobs 0.103 /
//! iroh-gossip 0.101): Blobs (fs) + Gossip + Docs (persistent redb) + a
//! Router accepting all three ALPNs (A1, P2P Mycelium Phase F).
//!
//! [`DocsStack`] owns every long-lived protocol actor for one online sync
//! thread so that spawning and shutting them down is a single operation.
//! It is built over an **already-bound** endpoint; when the endpoint fails
//! to bind (or the stack cannot spawn) the sync thread runs without a
//! stack and the replicator layer degrades to the in-memory mock — never
//! a crash. See `fe-sync/src/AGENTS.md` §iroh-1.x and §store-migration.
//!
//! Persistent layout under the data dir ([`p2p_data_dir`], env
//! [`P2P_DIR_ENV_VAR`], default `data/p2p`):
//!
//! ```text
//! <dir>/blobs/             — iroh-blobs fs store (blobs.db + data/ outboards)
//! <dir>/docs.redb          — iroh-docs replica store
//! <dir>/default-author     — hex-encoded default author key (the endpoint identity)
//! <dir>/.fe-store-format   — store format marker (see p2p_store_migration.rs)
//! ```

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context as _, Result};

use iroh::protocol::Router;
use iroh_blobs::provider::events::{
    AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage, RequestMode,
};
use iroh_blobs::store::fs::FsStore;
use crate::scene_asset_gate::ServeDecision;
use iroh_blobs::BlobsProtocol;
use iroh_docs::protocol::Docs;
use iroh_gossip::Gossip;

/// One verse's admission snapshot for the blob-ALPN request filter: the
/// owner, whether the verse is open to anyone (`default_access` at or
/// above Viewer), and every DID an admitted role statement resolves to
/// Viewer+ AT THE VERSE SCOPE (`VerseFabric::access_snapshot`, OF-6e — a
/// petal-scoped grant never opens verse-bound tileset content;
/// `src/AGENTS.md` §role-view).
#[derive(Clone, Default)]
pub struct VerseAccessSnapshot {
    pub owner_did: Option<String>,
    pub open_to_all: bool,
    pub viewer_dids: HashSet<String>,
}

impl VerseAccessSnapshot {
    fn admits(&self, did: &str) -> bool {
        self.owner_did.as_deref() == Some(did) || self.open_to_all || self.viewer_dids.contains(did)
    }
}

/// File (under the P2P data dir) persisting the gate's tracked tileset
/// content across restarts (OF-6b).
pub const BLOB_GATE_FILE: &str = "blob-access-gate.json";

/// Persisted gate-state format. Format 1 (W5s) let gossip ads grow a root's
/// `learned` set after the fetch, so a format-1 `learned` set may carry
/// attacker narrowing; it is dropped on load (fail-closed: the root denies
/// on the learned path until this node's next own fetch re-learns it).
/// Format 2 (R1) marked an unvouched fetch with [`LEGACY_UNVOUCHED_SENTINEL`]
/// inside `learned`; format 3 (R6, OF-10c) migrates it to `never_seed`.
/// See `src/AGENTS.md` §tileset-security "binding model".
const GATE_STATE_FORMAT: u32 = 3;

/// The format-2 in-data marker for an unvouched fetch context — only ever
/// read back during the format-3 migration, never written.
const LEGACY_UNVOUCHED_SENTINEL: &str = "\u{0}unvouched-provider";

/// What this node learned about one tileset HashSeq it serves, published,
/// or fetched — and therefore how a request for it is admitted. Binding
/// model (R1, OF-8): every field is set only by an AUTHORITATIVE local
/// event (own publish, own fetch decision, own HashSeq resolution) and
/// gossip never mutates it — `src/AGENTS.md` §tileset-security.
#[derive(Clone, Default, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct RootBinding {
    /// This node publishes it (`fe/tileset/` tag): a Viewer+ of ANY open
    /// verse is admitted — the publish path advertises to every joined verse.
    published: bool,
    /// The fetch context of this node's FIRST own fetch decision for the
    /// root: every verse the root (or its tileset id) was advertised on at
    /// that moment. A Viewer+ of EVERY one of them is admitted (a closed
    /// verse has no snapshot and denies). Fixed once set — neither a later
    /// fetch nor any inbound ad changes it (OF-8b).
    learned: std::collections::BTreeSet<String>,
    /// The first fetch went through a provider that never vouched for the
    /// root (R1 provenance rule): fetched for local use, never re-served on
    /// the learned path. Fixed with `learned`, at the first fetch decision.
    never_seed: bool,
    /// The HashSeq's children (index + bundles), once known (OF-6b).
    /// Content-addressed, so set once.
    children: Vec<String>,
}

impl RootBinding {
    /// The publish path: this node publishes the root and `did` is a
    /// Viewer+ of some open verse.
    fn publish_admits(&self, did: &str, access: &HashMap<String, VerseAccessSnapshot>) -> bool {
        self.published && access.values().any(|s| s.admits(did))
    }

    /// The learned (secondary-seeder) path: `did` is a Viewer+ of every
    /// verse of the fixed fetch context.
    fn learned_admits(&self, did: &str, access: &HashMap<String, VerseAccessSnapshot>) -> bool {
        !self.never_seed
            && !self.learned.is_empty()
            && self
                .learned
                .iter()
                .all(|v| access.get(v).is_some_and(|s| s.admits(did)))
    }

    /// The first fetch decision has fixed this root's learned binding.
    fn is_fetch_bound(&self) -> bool {
        self.never_seed || !self.learned.is_empty()
    }
}

/// Every tracked tileset root (hex) and the reverse child -> roots index.
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct TrackedContent {
    /// [`GATE_STATE_FORMAT`] of the persisted file (absent = format 1).
    format: u32,
    roots: HashMap<String, RootBinding>,
    #[serde(skip)]
    child_roots: HashMap<[u8; 32], HashSet<[u8; 32]>>,
}

impl TrackedContent {
    fn binding_mut(&mut self, root: &[u8; 32]) -> &mut RootBinding {
        self.roots.entry(hex::encode(root)).or_default()
    }

    /// Attach `children` to `root` once (a root's HashSeq is content-
    /// addressed, so its child list can never legitimately change);
    /// `true` when anything changed. A child already claimed by ANOTHER
    /// root gains `root` as a second claimant — which can only NARROW its
    /// admission (see [`BlobAccessGate::admits_hash`]), never widen it.
    fn set_children(&mut self, root: &[u8; 32], children: &[[u8; 32]]) -> bool {
        let binding = self.binding_mut(root);
        if !binding.children.is_empty() || children.is_empty() {
            return false;
        }
        binding.children = children.iter().map(hex::encode).collect();
        for child in children {
            self.child_roots.entry(*child).or_default().insert(*root);
        }
        true
    }

    /// Bring a just-loaded state to [`GATE_STATE_FORMAT`]; `true` when
    /// anything was dropped.
    fn upgrade(&mut self) -> bool {
        let mut dropped = false;
        if self.format < 2 {
            for binding in self.roots.values_mut() {
                dropped |= !binding.learned.is_empty();
                binding.learned.clear();
            }
        }
        if self.format < 3 {
            for binding in self.roots.values_mut() {
                if binding.learned.remove(LEGACY_UNVOUCHED_SENTINEL) {
                    binding.never_seed = true;
                }
            }
        }
        self.format = GATE_STATE_FORMAT;
        dropped
    }

    /// Rebuild the child index after a load (it is never persisted itself).
    fn reindex(&mut self) {
        self.child_roots.clear();
        for (root_hex, binding) in &self.roots {
            let Ok(root) = fe_runtime::blob_store::hash_from_hex(root_hex) else {
                continue;
            };
            for child_hex in &binding.children {
                if let Ok(child) = fe_runtime::blob_store::hash_from_hex(child_hex) {
                    self.child_roots.entry(child).or_default().insert(root);
                }
            }
        }
    }
}

/// What a [`crate::tileset_transfer::TilesetTransfer`] learned about a
/// tileset HashSeq — fed into [`BlobAccessGate::track_content`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TilesetContent {
    /// This node published `root` (`fe/tileset/` tag, at publish or rebuilt
    /// from tags at start-up).
    Published {
        root: [u8; 32],
        children: Vec<[u8; 32]>,
    },
    /// This node stopped publishing `root` (it stays tracked).
    Unpublished { root: [u8; 32] },
    /// This node resolved `root`'s children for a download or streamed cache
    /// (BEFORE any child is fetched into the store).
    Resolved {
        root: [u8; 32],
        children: Vec<[u8; 32]>,
    },
}

/// RP-5 §transport-admission-control item 5 / `AGENTS.md` §tileset-security
/// "Risk 2" (critic tick 4, AC-21(e)), hardened by W5s (OF-6a/b) and
/// redesigned by R1 (OF-8): shared state for the blobs-ALPN request filter.
/// Tracks every tileset HashSeq this node publishes or fetches — root AND
/// children — with how each is admitted ([`RootBinding`]). A tracked hash
/// is never forgotten (a closed verse's roots deny, they do not fall back
/// to "unknown"), and the whole map is persisted to [`BLOB_GATE_FILE`] so a
/// restart never re-opens it. A raw-blob request for a hash this gate does
/// not track carries no tileset context and is let through unconditionally
/// — row-entry content and `two_peer_blob_exchange` fetches are exactly
/// that case; a HashSeq (child-range) request is admitted only for a
/// tracked root whose children are known. See `src/AGENTS.md`
/// §tileset-security.
#[derive(Clone, Default)]
pub struct BlobAccessGate(Arc<GateState>);

#[derive(Default)]
struct GateState {
    verse_access: RwLock<HashMap<String, VerseAccessSnapshot>>,
    scene_assets: crate::scene_asset_gate::SceneAssetGate,
    tracked: RwLock<TrackedContent>,
    /// `connection_id -> did:key`, recorded at `ClientConnected` and
    /// dropped at `ConnectionClosed` — a request event carries no identity
    /// of its own, only the `connection_id` it arrived on.
    connections: Mutex<HashMap<u64, String>>,
    /// Where [`TrackedContent`] is persisted (`None`: in-memory only).
    persist_path: Option<PathBuf>,
    /// Serializes persistence writes (each writes the then-current state).
    persist_lock: Mutex<()>,
}

impl BlobAccessGate {
    /// A gate persisted at `path`, seeded from it when it exists. Blocking
    /// (one small file read) — call from the blocking pool.
    pub fn load(path: PathBuf) -> Self {
        let mut tracked = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<TrackedContent>(&bytes).unwrap_or_else(|e| {
                tracing::warn!(
                    path = %path.display(),
                    "blob access gate state unreadable — starting empty (the start-up tag scan \
                     re-tracks published and downloaded tilesets): {e}"
                );
                TrackedContent::default()
            }),
            Err(_) => TrackedContent::default(),
        };
        if tracked.upgrade() {
            tracing::warn!(
                path = %path.display(),
                "blob access gate state predates the fixed fetch-context binding — learned \
                 verse sets dropped (fail-closed until this node re-fetches each root)"
            );
        }
        tracked.reindex();
        Self(Arc::new(GateState {
            tracked: RwLock::new(tracked),
            scene_assets: crate::scene_asset_gate::SceneAssetGate::load(path.with_file_name("scene-asset-classification.json")),
            persist_path: Some(path),
            ..GateState::default()
        }))
    }

    pub(crate) fn scene_assets(&self)->crate::scene_asset_gate::SceneAssetGate {self.0.scene_assets.clone()}

    /// The scene-asset gate's answer; a slow admission is `Unavailable`, never a denial (QA-0143).
    async fn current_scene_decision(&self, connection_id: u64, hashes: &[[u8; 32]]) -> ServeDecision {
        let Some(did) = self.did_for_connection(connection_id) else {
            return ServeDecision::Deny;
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), self.0.scene_assets.decide(&did, hashes))
            .await
            .unwrap_or(ServeDecision::Unavailable)
    }

    /// Replace one verse's access snapshot (called after the fabric's role
    /// view settles).
    /// Whether `did` is a NAMED member of `verse_id` (its owner or a DID the role
    /// view admits), never merely anyone an open verse admits — the re-fetch
    /// arming gate, which minted author DIDs must not pass (QA-0155).
    pub(crate) fn names_member(&self, verse_id: &str, did: &str) -> bool {
        self.0
            .verse_access
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(verse_id)
            .is_some_and(|snapshot| snapshot.owner_did.as_deref() == Some(did) || snapshot.viewer_dids.contains(did))
    }

    pub fn set_verse_access(&self, verse_id: &str, snapshot: VerseAccessSnapshot) {
        self.0
            .verse_access
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(verse_id.to_string(), snapshot);
    }

    /// [`Self::bind_fetched_root`] with a single-verse, vouched fetch context.
    pub fn record_tileset_root(&self, verse_id: &str, root: [u8; 32]) {
        self.bind_fetched_root(root, std::iter::once(verse_id), true);
    }

    /// This node decided to fetch `root` in the context of `verses` (every
    /// verse it was advertised on — OF-6a's "all mapped verses" rule): track
    /// it BEFORE the fetch puts any of it in the store, and fix its learned
    /// binding to that context if it has none yet. The FIRST authoritative
    /// learn wins (OF-8): a later fetch context — and any inbound ad, which
    /// has no entry point here at all — never changes it, so neither can
    /// re-bind a root another verse already owns, nor narrow it after the
    /// fact (the pre-R1 gossip narrowing + persisted overflow was an
    /// attacker-triggerable durable refuse-everyone state). `vouched: false`
    /// (some provider never vouched for the root) binds it `never_seed`
    /// (OF-10c: a flag, not a sentinel verse id inside `learned`).
    pub fn bind_fetched_root<'a>(
        &self,
        root: [u8; 32],
        verses: impl IntoIterator<Item = &'a str>,
        vouched: bool,
    ) {
        let context: std::collections::BTreeSet<String> =
            verses.into_iter().map(str::to_string).collect();
        let changed = {
            let mut tracked = self.0.tracked.write().unwrap_or_else(|e| e.into_inner());
            let key = hex::encode(root);
            let newly_tracked = !tracked.roots.contains_key(&key);
            let binding = tracked.roots.entry(key).or_default();
            if !binding.is_fetch_bound() && (!context.is_empty() || !vouched) {
                binding.learned = context;
                binding.never_seed = !vouched;
                true
            } else {
                if !context.is_empty() && binding.learned != context {
                    tracing::debug!(
                        root = %hex::encode(root),
                        "tileset root already bound to its first fetch context — later context ignored"
                    );
                }
                newly_tracked
            }
        };
        if changed {
            self.persist();
        }
    }

    /// Record what the tileset transfer service learned (publish, unpublish,
    /// child resolution) — the child index that closes direct child fetches.
    pub fn track_content(&self, content: TilesetContent) {
        let changed = {
            let mut tracked = self.0.tracked.write().unwrap_or_else(|e| e.into_inner());
            match &content {
                TilesetContent::Published { root, children } => {
                    let binding = tracked.binding_mut(root);
                    let flipped = !binding.published;
                    binding.published = true;
                    tracked.set_children(root, children) | flipped
                }
                TilesetContent::Unpublished { root } => {
                    let binding = tracked.binding_mut(root);
                    std::mem::replace(&mut binding.published, false)
                }
                TilesetContent::Resolved { root, children } => tracked.set_children(root, children),
            }
        };
        if changed {
            self.persist();
        }
    }

    /// A verse closed: drop its access snapshot. Its roots stay tracked, so
    /// a root learned on it now DENIES (no snapshot) instead of falling
    /// back to the "untracked → admit" path (OF-6b).
    pub fn forget_verse(&self, verse_id: &str) {
        self.0
            .verse_access
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(verse_id);
    }

    /// Admission decision for one requested hash. `true` when the hash is
    /// neither a tracked tileset root nor a child of one (unaffected).
    /// Otherwise its CLAIMANTS are its own binding (when it is a root) plus
    /// every root listing it as a child, and it is admitted when either
    /// - some claimant is a root THIS node publishes and admits `did` on
    ///   the publish path (an own publish is the most authoritative learn —
    ///   this node holds the bytes and chose to distribute them), or
    /// - EVERY claimant admits `did` on its learned path.
    ///
    /// The AND is the OF-8a fix: a fetched HashSeq's child list is
    /// attacker-shaped content, so a root listing a victim tileset's
    /// children can only add a claimant that must ALSO admit — it narrows,
    /// it never lends its own (attacker-controlled) membership to the
    /// victim's bytes, whatever order the two roots were learned in.
    pub(crate) fn admits_hash(&self, did: &str, hash: &[u8; 32]) -> bool {
        let tracked = self.0.tracked.read().unwrap_or_else(|e| e.into_inner());
        let own = tracked.roots.get(&hex::encode(hash));
        let parents = tracked.child_roots.get(hash);
        if own.is_none() && parents.is_none() {
            return true;
        }
        let access = self
            .0
            .verse_access
            .read()
            .unwrap_or_else(|e| e.into_inner());
        // A parent link with no binding (never expected) is a claimant that
        // admits no one — fail closed.
        let claimants: Vec<Option<&RootBinding>> = own
            .map(Some)
            .into_iter()
            .chain(
                parents
                    .into_iter()
                    .flatten()
                    .map(|p| tracked.roots.get(&hex::encode(p))),
            )
            .collect();
        claimants
            .iter()
            .flatten()
            .any(|binding| binding.publish_admits(did, &access))
            || claimants
                .iter()
                .all(|binding| binding.is_some_and(|b| b.learned_admits(did, &access)))
    }

    /// The hashes one `GetRequest` for `hash` would stream: `hash` alone for
    /// a raw-blob request; for a request whose ranges reach into a HashSeq's
    /// children, the root plus every child — known only for a tracked root
    /// whose children were recorded. `None` (refuse) for a child-range
    /// request on any other hash: iroh-blobs would stream whatever children
    /// that blob lists, and an untracked HashSeq (say, a replicated row's
    /// content blob listing a protected tileset's bundle hashes) would
    /// otherwise launder tracked bytes past the per-hash gate. No client in
    /// this workspace sends child-range requests (`IrohBlobTransport` and
    /// iroh-docs fetch raw blobs).
    fn get_request_hashes(&self, hash: [u8; 32], reaches_children: bool) -> Option<Vec<[u8; 32]>> {
        if !reaches_children {
            return Some(vec![hash]);
        }
        let tracked = self.0.tracked.read().unwrap_or_else(|e| e.into_inner());
        let binding = tracked.roots.get(&hex::encode(hash))?;
        if binding.children.is_empty() {
            return None;
        }
        let mut hashes = Vec::with_capacity(binding.children.len() + 1);
        hashes.push(hash);
        for child in &binding.children {
            hashes.push(fe_runtime::blob_store::hash_from_hex(child).ok()?);
        }
        Some(hashes)
    }

    /// Write the current tracked state to [`BLOB_GATE_FILE`] — on the
    /// blocking pool when inside a runtime (§sync-thread-blocking-io). Each
    /// write takes the lock and serializes the THEN-current state, so the
    /// last write always holds the newest state regardless of task order.
    fn persist(&self) {
        if self.0.persist_path.is_none() {
            return;
        }
        let state = Arc::clone(&self.0);
        let write = move || {
            let _guard = state.persist_lock.lock().unwrap_or_else(|e| e.into_inner());
            let Some(path) = state.persist_path.as_ref() else {
                return;
            };
            let bytes = {
                let tracked = state.tracked.read().unwrap_or_else(|e| e.into_inner());
                match serde_json::to_vec(&*tracked) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::warn!("serializing blob access gate state failed: {e}");
                        return;
                    }
                }
            };
            let tmp = path.with_extension("json.tmp");
            let result = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path));
            if let Err(e) = result {
                tracing::warn!(path = %path.display(), "persisting blob access gate state failed: {e}");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(write);
            }
            Err(_) => write(),
        }
    }

    fn note_connected(&self, connection_id: u64, did: Option<String>) {
        if let Some(did) = did {
            self.0
                .connections
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(connection_id, did);
        }
    }

    fn note_closed(&self, connection_id: u64) {
        self.0
            .connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&connection_id);
    }

    fn did_for_connection(&self, connection_id: u64) -> Option<String> {
        self.0
            .connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&connection_id)
            .cloned()
    }

    /// Which gate refuses the requester on `connection_id` any of `hashes`, and
    /// whether it denied or could not decide (`None`: both admit) — named in
    /// the refusal log (QA-0137, QA-0143).
    async fn refusing_gate(&self, connection_id: u64, hashes: &[[u8; 32]]) -> Option<Refusal> {
        if !self.admits_connection(connection_id, hashes.iter()) {
            return Some(Refusal::denied("tileset"));
        }
        match self.current_scene_decision(connection_id, hashes).await {
            ServeDecision::Admit => None,
            ServeDecision::Deny => Some(Refusal::denied("scene-asset")),
            ServeDecision::Unavailable => Some(Refusal { gate: "scene-asset", busy: true }),
        }
    }

    /// Whether the requester on `connection_id` may read every hash in `hashes`.
    fn admits_connection<'a>(
        &self,
        connection_id: u64,
        mut hashes: impl Iterator<Item = &'a [u8; 32]>,
    ) -> bool {
        let Some(did) = self.did_for_connection(connection_id) else {
            return false;
        };
        hashes.all(|hash| self.admits_hash(&did, hash))
    }
}

/// Why a blob-ALPN request was not served: which gate, and whether it could
/// not decide in time (`busy`) rather than deny.
#[derive(Clone, Copy, Debug)]
struct Refusal {
    gate: &'static str,
    busy: bool,
}

impl Refusal {
    fn denied(gate: &'static str) -> Self {
        Self { gate, busy: false }
    }
}

/// Requests whose admission is decided at once; more wait in the event channel.
const CONCURRENT_DECISIONS: usize = 8;

/// Answer one intercepted request and drain its update channel so the
/// responder's progress sends never block on a reader we never consult.
/// A busy admission aborts `RateLimited` (wire code 2: retry later), a
/// denial `Permission` (QA-0143).
macro_rules! answer_intercepted {
    ($msg:expr, $refused_by:expr, $connection_id:expr, $kind:literal) => {{
        let msg = $msg;
        let refused_by: Option<Refusal> = $refused_by;
        let decision = match refused_by {
            None => Ok(()),
            Some(Refusal { gate, busy: true }) => {
                tracing::info!(
                    connection_id = $connection_id,
                    kind = $kind,
                    gate,
                    "blob-ALPN request deferred — its admission could not be decided in time (not a denial; the requester retries)"
                );
                Err(AbortReason::RateLimited)
            }
            Some(Refusal { gate, busy: false }) => {
                tracing::warn!(
                    connection_id = $connection_id,
                    kind = $kind,
                    gate,
                    "blob-ALPN request refused — requester holds no role in the content's verse (AC-21(e))"
                );
                Err(AbortReason::Permission)
            }
        };
        let _ = msg.tx.send(decision).await;
        let mut updates = msg.rx;
        tokio::spawn(async move { while let Ok(Some(_)) = updates.recv().await {} });
    }};
}

/// Build the blobs-ALPN `EventSender` implementing the request filter:
/// intercepts `GetRequest` (every hash it would stream must be admitted —
/// a child-range request only for a tracked root, R1), `GetManyRequest`
/// (every requested hash must be admitted; each is a raw blob) and
/// `ObserveRequest` (a bitfield is an existence oracle) and consults
/// [`BlobAccessGate`]; `push` stays `Disabled` (the `EventMask::DEFAULT`
/// value, pinned explicitly here — OF-6c). A raw-blob request without
/// tracked tileset context passes through untouched.
/// `ClientConnected`/`ConnectionClosed` are notify-only — just enough to map
/// a request's `connection_id` to the dialing `EndpointId`'s did:key.
fn spawn_blob_access_filter(gate: BlobAccessGate) -> EventSender {
    let mask = EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::Intercept,
        get_many: RequestMode::Intercept,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
        ..EventMask::DEFAULT
    };
    let (events, mut rx) = EventSender::channel(64, mask);
    // QA-0143: one slow admission must not hold every other peer's request.
    let deciding = Arc::new(tokio::sync::Semaphore::new(CONCURRENT_DECISIONS));
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                ProviderMessage::ClientConnectedNotify(notify) => {
                    let did = notify
                        .endpoint_id
                        .map(|id| crate::replicator::peer_did_key(&id));
                    gate.note_connected(notify.connection_id, did);
                }
                ProviderMessage::ConnectionClosed(msg) => {
                    gate.note_closed(msg.inner.connection_id);
                }
                ProviderMessage::GetRequestReceived(msg) => {
                    let Ok(permit) = deciding.clone().acquire_owned().await else { break };
                    let gate = gate.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let hash: [u8; 32] = msg.inner.request.hash.into();
                        let ranges = &msg.inner.request.ranges;
                        let reaches_children = !ranges.is_empty() && !ranges.is_blob();
                        let connection_id = msg.inner.connection_id;
                        let refused_by = match gate.get_request_hashes(hash, reaches_children) {
                            Some(hashes) => gate.refusing_gate(connection_id, &hashes).await,
                            None => Some(Refusal::denied("child-range")),
                        };
                        answer_intercepted!(msg, refused_by, connection_id, "get");
                    });
                }
                ProviderMessage::GetManyRequestReceived(msg) => {
                    let Ok(permit) = deciding.clone().acquire_owned().await else { break };
                    let gate = gate.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let hashes: Vec<[u8; 32]> = msg
                            .inner
                            .request
                            .hashes
                            .iter()
                            .map(|h| (*h).into())
                            .collect();
                        let connection_id = msg.inner.connection_id;
                        let refused_by = gate.refusing_gate(connection_id, &hashes).await;
                        answer_intercepted!(msg, refused_by, connection_id, "get_many");
                    });
                }
                ProviderMessage::ObserveRequestReceived(msg) => {
                    let Ok(permit) = deciding.clone().acquire_owned().await else { break };
                    let gate = gate.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let hash: [u8; 32] = msg.inner.request.hash.into();
                        let connection_id = msg.inner.connection_id;
                        let refused_by = gate.refusing_gate(connection_id, &[hash]).await;
                        answer_intercepted!(msg, refused_by, connection_id, "observe");
                    });
                }
                _ => {}
            }
        }
    });
    events
}

/// Env var selecting the persistent P2P data dir.
pub const P2P_DIR_ENV_VAR: &str = "FE_P2P_DIR";

/// Default persistent P2P data dir, relative to the process working dir.
pub const DEFAULT_P2P_DIR: &str = "data/p2p";

/// Resolve the P2P data dir from [`P2P_DIR_ENV_VAR`] (default `data/p2p`).
///
/// Blank/whitespace values fall back to the default; the value is used
/// verbatim otherwise.
pub fn p2p_data_dir() -> PathBuf {
    resolve_p2p_dir(std::env::var(P2P_DIR_ENV_VAR).ok().as_deref())
}

/// Pure core of [`p2p_data_dir`], unit-testable without touching process env.
pub fn resolve_p2p_dir(env_value: Option<&str>) -> PathBuf {
    match env_value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(DEFAULT_P2P_DIR),
    }
}

/// The full P2P protocol stack owned by an online sync thread.
///
/// The [`Router`] accepts all three ALPNs so inbound peer connections are
/// routable: blobs (`/iroh-bytes/4`), gossip (`/iroh-gossip/1`), docs
/// (`/iroh-sync/1`). Before the router existed inbound protocol connections
/// were silently unroutable (see AGENTS.md §iroh-1.x).
pub struct DocsStack {
    endpoint: iroh::Endpoint,
    blob_store: FsStore,
    blobs: BlobsProtocol,
    gossip: Gossip,
    docs: Docs,
    router: Router,
    data_dir: PathBuf,
    /// RP-5 step 6 (AC-21(e)): the blob-ALPN request filter's shared state.
    /// The sync thread updates it via [`DocsStack::blob_access_gate`] as
    /// verses open/close and their role views settle.
    blob_access_gate: BlobAccessGate,
}

impl DocsStack {
    /// Spawn the full stack over an already-bound endpoint.
    ///
    /// `data_dir` is created if missing; every persistent store lives under
    /// it. A legacy iroh 0.35 store is migrated first (backup copy + redb
    /// file-format upgrade — AGENTS.md §store-migration). Any failure returns
    /// an error — the caller degrades to offline/mock mode rather than
    /// panicking. Directory creation and the migration run on the blocking
    /// pool (see AGENTS.md §sync-thread-blocking-io).
    ///
    /// **One authenticated author identity (F20/M1 finding 2):** the
    /// endpoint's identity key is imported as the docs author and made the
    /// node-wide default, so every write the replicator layer publishes is
    /// authored by the endpoint identity and `entry.author()` maps to the
    /// same `did:key` the A3 inbound gate resolves (endpoint `EndpointId`
    /// did:key == the app DID — fe-identity's keypair and the iroh secret
    /// share the ed25519 seed). Left un-imported, the docs default author
    /// would be an unrelated per-`FE_P2P_DIR` key that maps to no role
    /// anywhere, and reconciliation replays would re-attribute rows to that
    /// role-less identity (denied outside the bootstrap window).
    pub async fn spawn(endpoint: iroh::Endpoint, data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        // redb and the docs author storage need the parent dir to exist;
        // the blobs fs store creates its own subtree. A 0.35-written store is
        // upgraded BEFORE any 1.x store opens it (redb 4 refuses v2 files).
        let dir = data_dir.clone();
        let migration = tokio::task::spawn_blocking(move || -> Result<_> {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating P2P data dir {}", dir.display()))?;
            crate::p2p_store_migration::migrate_legacy_p2p_store(&dir)
                .context("migrating the legacy iroh 0.35 P2P store")
        })
        .await
        .context("joining P2P data dir preparation task")??;
        if let crate::p2p_store_migration::MigrationOutcome::Migrated {
            backup_dir,
            upgraded,
        } = &migration
        {
            tracing::warn!(
                p2p_dir = %data_dir.display(),
                backup = %backup_dir.display(),
                upgraded = upgraded.len(),
                "Legacy iroh 0.35 P2P store migrated (the backup copy is the rollback)"
            );
        }

        let blob_store = FsStore::load(data_dir.join("blobs"))
            .await
            .map_err(|e| anyhow::anyhow!("opening persistent iroh-blobs store: {e}"))?;
        // RP-5 step 6 / AC-21(e): the blob-ALPN request filter (closes
        // `AGENTS.md` §tileset-security "Risk 2"). Built fresh per stack —
        // the sync thread populates it as verses open and their role views
        // settle (`DocsStack::blob_access_gate`). Seeded from its persisted
        // state BEFORE the router accepts a single request (OF-6b).
        let gate_path = data_dir.join(BLOB_GATE_FILE);
        let blob_access_gate = tokio::task::spawn_blocking(move || BlobAccessGate::load(gate_path))
            .await
            .context("joining blob access gate load task")?;
        let blob_events = spawn_blob_access_filter(blob_access_gate.clone());
        let blobs = BlobsProtocol::new(&blob_store, Some(blob_events));
        let gossip = Gossip::builder()
            // F7's distributed-query transport rides gossip envelopes (the
            // request carries a shard list; responses carry per-shard
            // aggregate partials or capped raw rows). iroh-gossip's default
            // 4096-byte cap is sized for control messages — raise it so a
            // realistic partial fits without a chunking protocol. The value
            // must be identical network-wide (all peers run this code); the
            // sync thread still bounds payloads itself via
            // `distributed_query::GOSSIP_ENVELOPE_BUDGET` + row caps.
            .max_message_size(crate::distributed_query::GOSSIP_MAX_MESSAGE_SIZE)
            .spawn(endpoint.clone());
        let docs = Docs::persistent(data_dir.clone())
            .spawn(
                endpoint.clone(),
                iroh_blobs::api::Store::from(blob_store.clone()),
                gossip.clone(),
            )
            .await
            .context("spawning persistent iroh-docs engine")?;

        // Import the endpoint identity as the docs author + make it the
        // node-wide default (persisted to <data_dir>/default-author), so
        // writes, live events, and snapshots all carry one authenticated
        // author identity — see the method doc comment.
        let identity_author = iroh_docs::Author::from_bytes(&endpoint.secret_key().to_bytes());
        let identity_author_id = identity_author.id();
        docs.author_import(identity_author)
            .await
            .context("importing the endpoint identity as the docs author")?;
        docs.author_set_default(identity_author_id)
            .await
            .context("setting the endpoint identity as the default docs author")?;
        tracing::info!(
            author = %identity_author_id,
            node_id = %endpoint.id().fmt_short(),
            "Docs author imported from the endpoint identity"
        );

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs.clone())
            .accept(iroh_gossip::ALPN, gossip.clone())
            .accept(iroh_docs::ALPN, docs.clone())
            .spawn();
        tracing::info!(
            node_id = %endpoint.id().fmt_short(),
            p2p_dir = %data_dir.display(),
            blobs_alpn = %String::from_utf8_lossy(iroh_blobs::ALPN),
            gossip_alpn = %String::from_utf8_lossy(iroh_gossip::ALPN),
            docs_alpn = %String::from_utf8_lossy(iroh_docs::ALPN),
            "P2P stack online: blobs + gossip + docs routed"
        );
        Ok(Self {
            endpoint,
            blob_store,
            blobs,
            gossip,
            docs,
            router,
            data_dir,
            blob_access_gate,
        })
    }

    /// The blob-ALPN request filter's shared state (RP-5 step 6 /
    /// AC-21(e)) — the sync thread calls `set_verse_access`/
    /// `record_tileset_root`/`forget_verse` on this as verses open, close,
    /// and their role views settle.
    pub fn blob_access_gate(&self) -> &BlobAccessGate {
        &self.blob_access_gate
    }

    /// The bound endpoint the stack rides on.
    pub fn endpoint(&self) -> &iroh::Endpoint {
        &self.endpoint
    }

    /// This node's public key identifier.
    pub fn node_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// The persistent blobs protocol (routed for inbound blob requests).
    pub fn blobs(&self) -> &BlobsProtocol {
        &self.blobs
    }

    /// The persistent blob store (fs store under `<data_dir>/blobs`).
    pub fn blob_store(&self) -> &iroh_blobs::api::Store {
        &self.blob_store
    }

    /// The gossip instance routed for inbound swarm connections.
    pub fn gossip(&self) -> &Gossip {
        &self.gossip
    }

    /// The docs engine (persistent replica store at `<data_dir>/docs.redb`).
    pub fn docs(&self) -> &Docs {
        &self.docs
    }

    /// Clone of the in-process docs API (author/doc/entry ops).
    ///
    /// This is the handle the replicator layer uses for `import_namespace`,
    /// `set_bytes`, `subscribe`, `start_sync` and `close`.
    pub fn docs_client(&self) -> iroh_docs::api::DocsApi {
        self.docs.api().clone()
    }

    /// The accept-loop router for all three ALPNs.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// The persistent data dir backing the stack's stores.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Shut the protocol handlers down (docs engine flush, blob store
    /// shutdown — the router calls each handler's `shutdown`).
    ///
    /// The endpoint close stays with the caller (`SyncEndpoint::shutdown`)
    /// so the existing sync-thread shutdown order is preserved.
    pub async fn shutdown(&self) {
        if let Err(e) = self.router.shutdown().await {
            tracing::warn!("P2P router shutdown error: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_config::RelayConfig;

    /// Bind a hermetic loopback endpoint (RelayMode::Disabled — no relay
    /// contact, satisfies the loopback-only P2P test rule).
    ///
    /// Returns `None` in sandboxed environments where UDP binding is
    /// unavailable (same precedent as `endpoint.rs` tests).
    async fn bind_test_endpoint(seed: u8) -> Option<iroh::Endpoint> {
        let secret = iroh::SecretKey::from_bytes(&[seed; 32]);
        match crate::endpoint::SyncEndpoint::new(secret, &RelayConfig::Disabled).await {
            Ok(ep) => Some(ep.inner().clone()),
            Err(e) => {
                tracing::warn!("endpoint bind failed (sandboxed env?) — skipping: {e}");
                None
            }
        }
    }

    #[test]
    fn resolve_p2p_dir_defaults_when_env_missing_or_blank() {
        assert_eq!(resolve_p2p_dir(None), PathBuf::from(DEFAULT_P2P_DIR));
        assert_eq!(resolve_p2p_dir(Some("")), PathBuf::from(DEFAULT_P2P_DIR));
        assert_eq!(resolve_p2p_dir(Some("   ")), PathBuf::from(DEFAULT_P2P_DIR));
    }

    #[test]
    fn resolve_p2p_dir_uses_env_override_verbatim() {
        assert_eq!(
            resolve_p2p_dir(Some("/tmp/p2p-x")),
            PathBuf::from("/tmp/p2p-x")
        );
        assert_eq!(
            resolve_p2p_dir(Some("  D:\\fe\\p2p  ")),
            PathBuf::from("D:\\fe\\p2p")
        );
    }

    #[test]
    fn p2p_dir_env_var_name_is_stable() {
        assert_eq!(P2P_DIR_ENV_VAR, "FE_P2P_DIR");
    }

    #[tokio::test]
    async fn docs_stack_spawns_all_persistent_stores() {
        let Some(endpoint) = bind_test_endpoint(21).await else {
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        let stack = DocsStack::spawn(endpoint, dir.path())
            .await
            .expect("stack spawn");

        // Persistent stores exist under the data dir (READ-BACK, not just Ok).
        assert!(dir.path().join("blobs").is_dir(), "blobs fs store dir");
        assert!(dir.path().join("docs.redb").is_file(), "docs redb store");
        assert!(
            dir.path().join("default-author").is_file(),
            "persistent default author"
        );
        assert_eq!(stack.data_dir(), dir.path());

        // The docs engine answers through the client (author storage is
        // loaded at engine spawn — a default author proves the engine lives),
        // and it is the endpoint identity (F20 alignment).
        let author = stack
            .docs_client()
            .author_default()
            .await
            .expect("default author from persistent storage");
        assert_ne!(author, iroh_docs::AuthorId::default());
        assert_eq!(author.as_bytes(), stack.node_id().as_bytes());
        // A fresh dir is stamped with the 1.x store-format marker.
        assert!(
            dir.path()
                .join(crate::p2p_store_migration::STORE_FORMAT_MARKER)
                .is_file(),
            "store format marker"
        );

        stack.shutdown().await;
    }

    #[tokio::test]
    async fn docs_stack_routes_all_three_alpns() {
        let Some(endpoint) = bind_test_endpoint(22).await else {
            return;
        };
        let dir = tempfile::TempDir::new().unwrap();
        let stack = DocsStack::spawn(endpoint, dir.path())
            .await
            .expect("stack spawn");

        // A second bare endpoint connects to the stack's node for each
        // routed ALPN; an unknown ALPN must be refused by the handshake.
        let client = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint bind");
        let addr = stack.endpoint().addr();

        for alpn in [iroh_blobs::ALPN, iroh_gossip::ALPN, iroh_docs::ALPN] {
            let alpn_name = String::from_utf8_lossy(alpn).to_string();
            match client.connect(addr.clone(), alpn).await {
                Ok(_conn) => tracing::debug!(alpn_name, "routed"),
                Err(e) => panic!("ALPN {alpn_name} should route through the stack: {e}"),
            }
        }
        // Negative control: the router refuses an ALPN it does not accept.
        assert!(
            client
                .connect(addr.clone(), b"/not-a-protocol/1")
                .await
                .is_err(),
            "unknown ALPN must be refused"
        );

        stack.shutdown().await;
    }

    #[tokio::test]
    async fn docs_stack_spawn_failure_is_an_error_not_a_panic() {
        // Offline degrade vector: a data dir path that cannot be created
        // (its parent is a regular file) makes spawn fail cleanly — the
        // sync thread's caller degrades to mock on this error.
        let parent = tempfile::TempDir::new().unwrap();
        let blocker = parent.path().join("blocker");
        std::fs::write(&blocker, b"file, not a dir").unwrap();
        let bad_dir = blocker.join("p2p");

        let Some(endpoint) = bind_test_endpoint(23).await else {
            return;
        };
        let result = DocsStack::spawn(endpoint, &bad_dir).await;
        assert!(result.is_err(), "spawn under a file path must fail");
    }

    /// RP-5 step 6 / AC-21(e): the blob-ALPN `GetRequest` filter refuses a
    /// requester with no role in any open verse and serves the same
    /// requester once it holds Viewer+ in the verse the root was learned
    /// on — the end-to-end proof for `BlobAccessGate`/
    /// `spawn_blob_access_filter`. Two real stacks over loopback.
    #[tokio::test]
    async fn a_verse_stranger_is_refused_the_blob_alpn_and_an_open_verse_viewer_is_served() {
        use fe_runtime::blob_transport::BlobTransport;

        let (Some(provider_ep), Some(requester_ep)) =
            (bind_test_endpoint(61).await, bind_test_endpoint(62).await)
        else {
            return; // sandboxed environment without UDP — tolerated
        };

        let provider_dir = tempfile::TempDir::new().unwrap();
        let requester_dir = tempfile::TempDir::new().unwrap();
        let provider_stack = Arc::new(
            DocsStack::spawn(provider_ep.clone(), provider_dir.path())
                .await
                .expect("provider stack spawns"),
        );
        let requester_stack = Arc::new(
            DocsStack::spawn(requester_ep.clone(), requester_dir.path())
                .await
                .expect("requester stack spawns"),
        );

        let provider_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&provider_stack);
        let hash = provider_transport
            .add(bytes::Bytes::from_static(b"tileset root bytes"))
            .await
            .expect("provider stores the root blob");
        provider_stack
            .blob_access_gate()
            .record_tileset_root("verse-1", hash);

        let requester_did = crate::replicator::peer_did_key(&requester_ep.id());
        let provider_addr =
            crate::blob_transport::IrohBlobTransport::provider_ref(&provider_ep.addr())
                .expect("provider address resolves");
        let requester_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&requester_stack);

        // No verse access registered at all for this hash's verse — refused.
        let refused = requester_transport.fetch(&provider_addr, hash, 1024).await;
        assert!(
            refused.is_err(),
            "a requester with no role in any open verse must be refused"
        );

        // The SAME requester, now a declared Viewer of the root's verse, is served.
        provider_stack.blob_access_gate().set_verse_access(
            "verse-1",
            VerseAccessSnapshot {
                owner_did: None,
                open_to_all: false,
                viewer_dids: [requester_did].into_iter().collect(),
            },
        );
        let served = requester_transport.fetch(&provider_addr, hash, 1024).await;
        assert!(
            served.is_ok(),
            "a verse Viewer must be served the tileset root: {:?}",
            served.err()
        );
        assert_eq!(served.unwrap().as_ref(), b"tileset root bytes");

        provider_stack.shutdown().await;
        requester_stack.shutdown().await;
    }

    /// A hash never recorded as a tileset root (ordinary row-entry content,
    /// or a `two_peer_blob_exchange` hash) carries no tileset context and
    /// is unaffected by the filter — the risk note's explicit requirement.
    #[tokio::test]
    async fn a_hash_with_no_tileset_context_is_unaffected_by_the_filter() {
        use fe_runtime::blob_transport::BlobTransport;

        let (Some(provider_ep), Some(requester_ep)) =
            (bind_test_endpoint(63).await, bind_test_endpoint(64).await)
        else {
            return;
        };
        let provider_dir = tempfile::TempDir::new().unwrap();
        let requester_dir = tempfile::TempDir::new().unwrap();
        let provider_stack = Arc::new(
            DocsStack::spawn(provider_ep.clone(), provider_dir.path())
                .await
                .expect("provider stack spawns"),
        );
        let requester_stack = Arc::new(
            DocsStack::spawn(requester_ep.clone(), requester_dir.path())
                .await
                .expect("requester stack spawns"),
        );

        let provider_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&provider_stack);
        let hash = provider_transport
            .add(bytes::Bytes::from_static(b"an ordinary row-entry blob"))
            .await
            .expect("provider stores the blob");
        // Deliberately NOT recorded as a tileset root — no `verse_access`
        // registered anywhere either.

        let provider_addr =
            crate::blob_transport::IrohBlobTransport::provider_ref(&provider_ep.addr())
                .expect("provider address resolves");
        let requester_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&requester_stack);
        let result = requester_transport.fetch(&provider_addr, hash, 1024).await;
        assert!(
            result.is_ok(),
            "a hash with no tracked tileset context must pass through: {:?}",
            result.err()
        );

        // Exercise the real native transfer/jobs with a controlled typed admission seam.
        // Actual DB authority/metadata fences are covered by the existing DB journey.
        use crate::{event_outbox::EventOutbox, messages::SyncEvent, scene_assets::SceneAssetJobs};
        use fe_runtime::blob_store::BlobStoreHandle;
        use fe_runtime::messages::DbCommand;
        use fe_runtime::scene_asset::{SceneAssetTransferPermit, SceneAssetTransferPurpose};
        let provider_files: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(provider_dir.path().join("scene-files")).unwrap());
        let requester_files: BlobStoreHandle =
            Arc::new(crate::FsBlobStore::new(requester_dir.path().join("scene-files")).unwrap());
        let json = br#"{"asset":{"version":"2.0"}}"#;
        let padded = (json.len() + 3) & !3;
        let total = 20 + padded;
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(padded as u32).to_le_bytes());
        glb.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        glb.extend_from_slice(json);
        glb.resize(total, b' ');
        let scene_hash = provider_files.add_blob(&glb).unwrap();
        let asset_id = ulid::Ulid::new().to_string();
        let row=serde_json::to_vec(&serde_json::json!({"_fe_scene_asset":1,"asset_id":asset_id,"petal_id":"scene-p","content_hash":fe_runtime::blob_store::hash_to_hex(&scene_hash),"content_type":"model/gltf-binary","size_bytes":glb.len()})).unwrap();
        let row_hash = provider_files.add_blob(&row).unwrap();
        assert!(crate::scene_assets::is_scene_row(&provider_files, row_hash)
            .await
            .unwrap());
        let mut ordinary: serde_json::Value = serde_json::from_slice(&row).unwrap();
        ordinary
            .as_object_mut()
            .unwrap()
            .remove(fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER);
        let ordinary_hash = provider_files
            .add_blob(&serde_json::to_vec(&ordinary).unwrap())
            .unwrap();
        assert!(
            !crate::scene_assets::is_scene_row(&provider_files, ordinary_hash)
                .await
                .unwrap(),
            "GLB MIME alone never reroutes ordinary/device metadata"
        );
        let second_asset_id = ulid::Ulid::new().to_string();
        let mut second_row: serde_json::Value = serde_json::from_slice(&row).unwrap();
        second_row["asset_id"] = serde_json::json!(second_asset_id);
        second_row["petal_id"] = serde_json::json!("scene-p2");
        let second_row_hash = provider_files
            .add_blob(&serde_json::to_vec(&second_row).unwrap())
            .unwrap();
        use crate::replicator::{IrohDocsEngineHolder, IrohDocsReplicator, VerseReplicator};
        let secret = [161u8; 32];
        let namespace = hex::encode(fe_database::derive_namespace_id(&secret));
        // Original Owner opens with no providers; the joining Editor dials the Owner.
        let owner_replica = IrohDocsReplicator::new(
            "scene-v".into(),
            namespace.clone(),
            hex::encode(secret),
            "owner".into(),
            Arc::new(IrohDocsEngineHolder::online(requester_stack.clone())),
        );
        let editor_replica = IrohDocsReplicator::new(
            "scene-v".into(),
            namespace.clone(),
            hex::encode(secret),
            "editor".into(),
            Arc::new(IrohDocsEngineHolder::online(provider_stack.clone())),
        );
        owner_replica.open_document().await.unwrap();
        editor_replica.open_document().await.unwrap();
        let _owner_rows = owner_replica.subscribe().await.unwrap();
        let _editor_rows = editor_replica.subscribe().await.unwrap();
        owner_replica.start_sync(Vec::new()).await.unwrap();
        editor_replica
            .start_sync(vec![requester_ep.addr()])
            .await
            .unwrap();
        let owner_roster = owner_replica.scene_provider_roster().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !owner_roster
                .peers()
                .is_ok_and(|peers| peers.contains(&provider_ep.id()))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Owner learns the joined authenticated Editor from its actual native subscription");
        let permit = SceneAssetTransferPermit {
            verse_id: "scene-v".into(),
            petal_id: "scene-p".into(),
            node_id: None,
            asset_id: asset_id.clone(),
            content_hash: scene_hash,
            content_type: "model/gltf-binary".into(),
            size_bytes: glb.len() as u64,
            asset_fingerprint: "controlled-admission-fixture".into(),
        };
        // Portal-bundle MIME row: a stored-ZIP sealed bundle replicates like any scene asset (inert bytes).
        let bundle_bytes = {
            let members: [(&str, &[u8]); 2] = [
                ("manifest.json", br#"{"hexon_type":"plugin","integrity":{"root":"fixture"}}"#),
                ("plugin/extension.json", b"{}"),
            ];
            let (mut zip, mut directory) = (Vec::<u8>::new(), Vec::<u8>::new());
            for (name, data) in members {
                let offset = zip.len() as u32;
                let (name_len, data_len) = (name.len() as u16, data.len() as u32);
                zip.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
                zip.extend_from_slice(&20u16.to_le_bytes());
                zip.extend_from_slice(&[0; 8]); // flags, method 0 (stored), time, date
                zip.extend_from_slice(&0u32.to_le_bytes()); // crc is not checked by the structural gate
                zip.extend_from_slice(&data_len.to_le_bytes());
                zip.extend_from_slice(&data_len.to_le_bytes());
                zip.extend_from_slice(&name_len.to_le_bytes());
                zip.extend_from_slice(&0u16.to_le_bytes());
                zip.extend_from_slice(name.as_bytes());
                zip.extend_from_slice(data);
                directory.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
                directory.extend_from_slice(&20u16.to_le_bytes());
                directory.extend_from_slice(&20u16.to_le_bytes());
                directory.extend_from_slice(&[0; 8]);
                directory.extend_from_slice(&0u32.to_le_bytes());
                directory.extend_from_slice(&data_len.to_le_bytes());
                directory.extend_from_slice(&data_len.to_le_bytes());
                directory.extend_from_slice(&name_len.to_le_bytes());
                directory.extend_from_slice(&[0; 12]); // extra, comment, disk, internal attrs
                directory.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                directory.extend_from_slice(&offset.to_le_bytes());
                directory.extend_from_slice(name.as_bytes());
            }
            let directory_offset = zip.len() as u32;
            zip.extend_from_slice(&directory);
            zip.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0]);
            zip.extend_from_slice(&(members.len() as u16).to_le_bytes());
            zip.extend_from_slice(&(members.len() as u16).to_le_bytes());
            zip.extend_from_slice(&(directory.len() as u32).to_le_bytes());
            zip.extend_from_slice(&directory_offset.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip
        };
        let bundle_hash = provider_files.add_blob(&bundle_bytes).unwrap();
        let bundle_asset_id = ulid::Ulid::new().to_string();
        fe_runtime::scene_asset::verify_scene_asset_bytes(
            &bundle_bytes,
            &bundle_hash,
            fe_runtime::scene_asset::PORTAL_BUNDLE_MIME,
            bundle_bytes.len() as u64,
        )
        .expect("the fixture bundle passes the structural gate");
        let bundle_permit = SceneAssetTransferPermit {
            asset_id: bundle_asset_id.clone(),
            content_hash: bundle_hash,
            content_type: fe_runtime::scene_asset::PORTAL_BUNDLE_MIME.into(),
            size_bytes: bundle_bytes.len() as u64,
            asset_fingerprint: "portal-bundle-fixture".into(),
            ..permit.clone()
        };
        let bundle_row_hash = provider_files
            .add_blob(&serde_json::to_vec(&serde_json::json!({"_fe_scene_asset":1,"asset_id":bundle_asset_id,"petal_id":"scene-p","content_hash":fe_runtime::blob_store::hash_to_hex(&bundle_hash),"content_type":fe_runtime::scene_asset::PORTAL_BUNDLE_MIME,"size_bytes":bundle_bytes.len()})).unwrap())
            .unwrap();
        assert!(
            crate::scene_assets::is_scene_row(&provider_files, bundle_row_hash).await.unwrap(),
            "a portal-bundle MIME row arrives as a scene row"
        );
        let allowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let admission_stop = fe_runtime::service_lifecycle::StopSignal::default();
        let requester_did = crate::replicator::peer_did_key(&requester_ep.id());
        let (provider_db, provider_rx) = crossbeam::channel::bounded::<DbCommand>(16);
        let (requester_db, requester_rx) = crossbeam::channel::bounded::<DbCommand>(16);
        let spawn_admissions = |rx: crossbeam::channel::Receiver<DbCommand>, source: bool| {
            let permit = permit.clone();
            let bundle_permit = bundle_permit.clone();
            let second_asset_id = second_asset_id.clone();
            let allowed = allowed.clone();
            let stalled = stalled.clone();
            let stop = admission_stop.clone();
            let requester_did = requester_did.clone();
            tokio::spawn(async move {
                let mut held_replies = Vec::new();
                while !stop.is_requested() {
                    while let Ok(command) = rx.try_recv() {
                        if let DbCommand::InspectSceneAssetTransfer { request, reply } = command {
                            // A DB stuck behind a write burst never answers in time.
                            if stalled.load(std::sync::atomic::Ordering::Acquire)
                                && matches!(request.purpose, SceneAssetTransferPurpose::Serve { .. })
                            {
                                held_replies.push(reply);
                                continue;
                            }
                            assert_eq!(request.verse_id, "scene-v");
                            let mut selected = if request.hash == bundle_permit.content_hash {
                                bundle_permit.clone()
                            } else {
                                assert_eq!(request.hash, permit.content_hash);
                                permit.clone()
                            };
                            if let SceneAssetTransferPurpose::Publish { asset_id, petal_id } = &request.purpose {
                                if asset_id == &second_asset_id && petal_id == "scene-p2" {
                                    selected.asset_id=second_asset_id.clone(); selected.petal_id="scene-p2".into();
                                    selected.asset_fingerprint="second-exact-fixture".into();
                                }
                            }
                            let valid = match &request.purpose {
                                SceneAssetTransferPurpose::Publish { asset_id, petal_id } => source && asset_id == &selected.asset_id && petal_id == &selected.petal_id,
                                SceneAssetTransferPurpose::Fetch => !source,
                                SceneAssetTransferPurpose::Serve {
                                    requester_did: actual,
                                } => {
                                    source
                                        && actual == &requester_did
                                        && allowed.load(std::sync::atomic::Ordering::Acquire)
                                }
                            };
                            let _ = reply.try_send(if valid {
                                Ok(selected)
                            } else {
                                Err("controlled current admission refusal".into())
                            });
                        } else {
                            panic!("unexpected private admission command");
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
        };
        let source_admissions = spawn_admissions(provider_rx, true);
        let receiver_admissions = spawn_admissions(requester_rx, false);
        let (source_events, _) = crossbeam::channel::bounded(32);
        let (events_tx, events_rx) = crossbeam::channel::bounded(32);
        let receiver_outbox = EventOutbox::new(events_tx);
        let (source_publications, mut publication_rx) = tokio::sync::mpsc::channel(8);
        let (receiver_publications, _) = tokio::sync::mpsc::channel(8);
        let mut source_jobs = SceneAssetJobs::new(
            provider_files.clone(),
            Some(Arc::new(crate::IrohBlobTransport::from_stack(
                &provider_stack,
            ))),
            provider_stack.blob_access_gate().scene_assets(),
            Some(provider_db),
            EventOutbox::new(source_events),
            source_publications,
        );
        let mut receiver_jobs = SceneAssetJobs::new(
            requester_files.clone(),
            Some(Arc::new(crate::IrohBlobTransport::from_stack(
                &requester_stack,
            ))),
            requester_stack.blob_access_gate().scene_assets(),
            Some(requester_db),
            receiver_outbox.clone(),
            receiver_publications,
        );
        source_jobs.opened_with_roster("scene-v", &[], editor_replica.scene_provider_roster());
        receiver_jobs.opened_with_roster("scene-v", &[], Some(owner_roster.clone()));
        source_jobs.publish_row("scene-v", row_hash);
        let publication =
            tokio::time::timeout(std::time::Duration::from_secs(20), publication_rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert!(source_jobs.publication_is_current(&publication));
        assert_eq!(publication.record_id, asset_id);
        assert_eq!(publication.row_hash, row_hash);
        source_jobs.publish_row("scene-v", second_row_hash);
        let second_publication=tokio::time::timeout(std::time::Duration::from_secs(20), publication_rx.recv()).await.unwrap().unwrap();
        assert_eq!(second_publication.record_id,second_asset_id,"same bytes do not collapse metadata identities");
        assert_eq!(second_publication.row_hash,second_row_hash);
        assert!(
            provider_dir
                .path()
                .join("scene-asset-classification.json")
                .is_file(),
            "classification precedes metadata emission"
        );
        let root = fe_runtime::blob_transport::hash_seq_root(&[scene_hash]);
        let reloaded = crate::scene_asset_gate::SceneAssetGate::load(
            provider_dir.path().join("scene-asset-classification.json"),
        );
        assert!(
            !reloaded.admits(&requester_did, &[scene_hash]).await,
            "restart retains raw scene classification without an authorizer"
        );
        assert!(
            !reloaded.admits(&requester_did, &[root]).await,
            "retention root cannot fall through the raw-blob policy"
        );
        assert!(
            requester_transport
                .verify_provider_size(&provider_addr, scene_hash, glb.len() as u64)
                .await
                .is_err(),
            "unknown requester cannot read scene bytes while ordinary row blobs stay unaffected"
        );
        allowed.store(true, std::sync::atomic::Ordering::Release);
        let mut wrong = scene_hash;
        wrong[0] ^= 1;
        assert!(fe_runtime::scene_asset::verify_scene_asset_bytes(
            &glb,
            &wrong,
            "model/gltf-binary",
            glb.len() as u64
        )
        .is_err());
        assert!(
            requester_transport
                .verify_provider_size(&provider_addr, scene_hash, glb.len() as u64 - 1)
                .await
                .is_err(),
            "provider size cap refuses before blob fetch"
        );
        assert!(requester_transport
            .read_local(scene_hash)
            .await
            .unwrap()
            .is_none());

        receiver_jobs.fetch_owned("scene-v", scene_hash, Some("scene-owner-a".into()));
        receiver_jobs.fetch_owned("scene-v", scene_hash, Some("scene-owner-b".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut settled = false;
        let mut ready = false;
        while !settled || !ready {
            assert!(
                std::time::Instant::now() < deadline,
                "owned native scene download timed out"
            );
            match events_rx.try_recv() {
                Ok(SyncEvent::SceneAssetFetchSettled {
                    hash,
                    request_ids,
                    result,
                    ..
                }) => {
                    assert_eq!(hash, scene_hash);
                    assert!(result.is_ok(), "{result:?}");
                    assert_eq!(request_ids, vec!["scene-owner-a", "scene-owner-b"]);
                    settled = true;
                    assert!(
                        requester_files.has_blob(&scene_hash),
                        "owned success follows actual shared-store persistence"
                    );
                }
                Ok(SyncEvent::BlobReady { hash }) => {
                    assert_eq!(hash, scene_hash);
                    ready = true;
                }
                Ok(other) => panic!("unexpected scene success event: {other:?}"),
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        assert_eq!(
            std::fs::read(requester_files.get_blob_path(&scene_hash).unwrap()).unwrap(),
            glb
        );
        let canonical = requester_files.get_blob_path(&scene_hash).unwrap();
        std::fs::write(&canonical, b"corrupt pre-existing cache file").unwrap();
        crate::sync_thread::handle_fetch_blob(&mut receiver_jobs, &scene_hash, "scene-v");
        assert!(events_rx.try_recv().is_err(), "compatibility request cannot synchronously announce corrupt cache readiness");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "corrupt cache writeback did not settle"
            );
            match events_rx.try_recv() {
                Ok(SyncEvent::SceneAssetFetchSettled {
                    request_ids,
                    result,
                    ..
                }) => {
                    panic!("compatibility fetch has no owned request IDs: {request_ids:?} {result:?}");
                }
                Ok(SyncEvent::BlobReady { hash }) => {
                    assert_eq!(hash, scene_hash);
                    assert_eq!(std::fs::read(&canonical).unwrap(), glb);
                    break;
                }
                Ok(SyncEvent::SceneAssetFetchFailed { reason, .. }) => panic!("compatibility repair failed: {reason}"),
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        while let Ok(event) = events_rx.try_recv() {
            if let SyncEvent::BlobReady { hash } = event {
                assert_eq!(hash, scene_hash);
            }
        }
        assert_eq!(std::fs::read(&canonical).unwrap(), glb);
        allowed.store(false, std::sync::atomic::Ordering::Release);
        receiver_jobs.fetch_owned("scene-v", scene_hash, Some("scene-revoked".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "revoked native transfer did not settle"
            );
            match events_rx.try_recv() {
                Ok(SyncEvent::SceneAssetFetchSettled {
                    request_ids,
                    result,
                    ..
                }) => {
                    assert_eq!(request_ids, vec!["scene-revoked"]);
                    assert!(result.is_err());
                    break;
                }
                Ok(SyncEvent::BlobReady { .. }) => {
                    panic!("a cached native copy cannot bypass fresh provider admission")
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        while let Ok(event) = events_rx.try_recv() {
            assert!(!matches!(event, SyncEvent::BlobReady { .. }));
        }
        // QA-0118: a refused fetch is re-armed, never given up: once the provider
        // admits again, a pump retries it with no new row and no petal entry.
        requester_files.remove_blob(&scene_hash).unwrap();
        allowed.store(true, std::sync::atomic::Ordering::Release);
        receiver_jobs.rearm("scene-v");
        assert!(receiver_jobs.pump_prefetches(), "the unavailable blob is kept for retry");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            assert!(std::time::Instant::now() < deadline, "re-armed fetch did not settle");
            match events_rx.try_recv() {
                Ok(SyncEvent::BlobReady { hash }) => {
                    assert_eq!(hash, scene_hash);
                    break;
                }
                Ok(SyncEvent::SceneAssetFetchFailed { reason, .. }) => panic!("re-armed fetch failed: {reason}"),
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        assert_eq!(std::fs::read(&canonical).unwrap(), glb);
        assert!(!receiver_jobs.pump_prefetches(), "a delivered blob leaves the re-arm set");
        // The portal-bundle row's blob is fetched through the same admitted path and lands byte-identical.
        receiver_jobs.fetch_owned("scene-v", bundle_hash, Some("portal-bundle".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            assert!(std::time::Instant::now() < deadline, "portal-bundle fetch did not settle");
            match events_rx.try_recv() {
                Ok(SyncEvent::BlobReady { hash }) if hash == bundle_hash => break,
                Ok(SyncEvent::SceneAssetFetchFailed { reason, .. }) => panic!("portal-bundle fetch failed: {reason}"),
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        assert_eq!(std::fs::read(requester_files.get_blob_path(&bundle_hash).unwrap()).unwrap(), bundle_bytes);
        receiver_jobs.fetch_owned("scene-v", scene_hash, Some("scene-old-generation".into()));
        receiver_jobs.close("scene-v");
        owner_replica.close().await.unwrap();
        assert!(
            owner_roster.peers().is_err(),
            "closed subscription cannot retain a usable provider roster"
        );
        owner_replica.open_document().await.unwrap();
        let _owner_rows_reopened = owner_replica.subscribe().await.unwrap();
        owner_replica.start_sync(Vec::new()).await.unwrap();
        editor_replica
            .start_sync(vec![requester_ep.addr()])
            .await
            .unwrap();
        let reopened_roster = owner_replica.scene_provider_roster().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !reopened_roster
                .peers()
                .is_ok_and(|peers| peers.contains(&provider_ep.id()))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("reopened retained namespace relearns connected Editor without explicit bootstrap");
        assert!(
            owner_roster.peers().is_err(),
            "a fresh subscription never resurrects the old roster"
        );
        // QA-0143: an admission the DB cannot answer in time is "busy" (wire code 2,
        // retry later), never a role denial (code 1) that sends the requester into back-off.
        // (Placed after the reopen: that relearn is sensitive to a delay before it.)
        allowed.store(false, std::sync::atomic::Ordering::Release);
        let provider_gate = provider_stack.blob_access_gate().scene_assets();
        assert_eq!(
            provider_gate.decide(&requester_did, &[scene_hash]).await,
            crate::scene_asset_gate::ServeDecision::Deny
        );
        let denied = requester_transport
            .verify_provider_size(&provider_addr, scene_hash, glb.len() as u64)
            .await
            .expect_err("a requester without a role is refused");
        assert!(format!("{denied:#}").contains("error 1"), "a denial is a permission abort: {denied:#}");
        stalled.store(true, std::sync::atomic::Ordering::Release);
        assert_eq!(
            provider_gate.decide(&requester_did, &[scene_hash]).await,
            crate::scene_asset_gate::ServeDecision::Unavailable
        );
        let busy = requester_transport
            .verify_provider_size(&provider_addr, scene_hash, glb.len() as u64)
            .await
            .expect_err("an undecided admission serves nothing");
        assert!(format!("{busy:#}").contains("error 2"), "a busy admission is a rate-limit abort: {busy:#}");
        stalled.store(false, std::sync::atomic::Ordering::Release);
        allowed.store(true, std::sync::atomic::Ordering::Release);
        receiver_jobs.opened_with_roster("scene-v", &[], Some(reopened_roster));
        receiver_jobs.fetch_owned("scene-v", scene_hash, Some("scene-new-generation".into()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut old = false;
        let mut new = false;
        while !old || !new {
            assert!(
                std::time::Instant::now() < deadline,
                "generation owners did not settle"
            );
            match events_rx.try_recv() {
                Ok(SyncEvent::SceneAssetFetchSettled {
                    request_ids,
                    result,
                    ..
                }) if request_ids == vec!["scene-old-generation"] => {
                    assert!(result.is_err());
                    old = true;
                }
                Ok(SyncEvent::SceneAssetFetchSettled {
                    request_ids,
                    result,
                    ..
                }) => {
                    assert_eq!(request_ids, vec!["scene-new-generation"]);
                    assert!(result.is_ok());
                    new = true;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
        }
        while events_rx.try_recv().is_ok() {}
        for owner in 0..69 {
            receiver_jobs.opened("scene-v", &[provider_ep.addr()]);
            receiver_jobs.fetch_owned(
                "scene-v",
                scene_hash,
                Some(format!("bounded-owner-{owner}")),
            );
        }
        assert_eq!(
            receiver_jobs.pending_job_count(),
            68,
            "four active slots plus 64 waiting jobs, with no unbounded prefix"
        );
        receiver_jobs.close("scene-v");
        receiver_jobs.shutdown().await;
        let mut bounded_owners = std::collections::BTreeSet::new();
        loop {
            receiver_outbox.flush();
            match events_rx.try_recv() {
                Ok(SyncEvent::SceneAssetFetchSettled {
                    request_ids,
                    result,
                    ..
                }) => {
                    assert!(result.is_err());
                    bounded_owners.extend(request_ids);
                }
                Ok(SyncEvent::BlobReady { .. }) => {
                    panic!("closed queued generations cannot announce readiness")
                }
                Ok(_) => {}
                Err(_) => {
                    if receiver_outbox.backlog_len() == 0 {
                        break;
                    }
                }
            }
        }
        assert_eq!(
            bounded_owners.len(),
            69,
            "queue refusal and every cancelled owner settle without loop self-drain deadlock"
        );
        owner_replica.close().await.unwrap();
        editor_replica.close().await.unwrap();
        source_jobs.shutdown().await;
        admission_stop.request();
        source_admissions.await.unwrap();
        receiver_admissions.await.unwrap();
        std::fs::remove_file(provider_dir.path().join("scene-asset-classification.json")).unwrap();
        let lost = crate::scene_asset_gate::SceneAssetGate::load(
            provider_dir.path().join("scene-asset-classification.json"),
        );
        assert!(!lost.admits(&requester_did, &[scene_hash]).await);
        assert!(
            !lost.admits(&requester_did, &[hash]).await,
            "lost activated registry cannot silently reopen unknown native bytes"
        );

        provider_stack.shutdown().await;
        requester_stack.shutdown().await;
    }

    fn viewers(dids: &[&str]) -> VerseAccessSnapshot {
        VerseAccessSnapshot {
            owner_did: None,
            open_to_all: false,
            viewer_dids: dids.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// OF-6a + OF-8b (R1 binding model): a root's binding is fixed at this
    /// node's FIRST own fetch decision. A later fetch of the same root in an
    /// attacker's verse context neither grants the attacker access (no
    /// remap) nor locks the original verse's viewers out (no narrowing —
    /// pre-R1, every accepted ad narrowed the set and 64 of them persisted a
    /// refuse-everyone overflow). Gossip has no entry point into the gate
    /// at all any more.
    #[test]
    fn a_root_bound_to_its_first_fetch_context_is_never_rebound_or_narrowed() {
        let gate = BlobAccessGate::default();
        let root = [7u8; 32];
        gate.record_tileset_root("victim-verse", root);
        gate.set_verse_access("victim-verse", viewers(&["did:key:victim-viewer"]));
        gate.set_verse_access("attacker-verse", viewers(&["did:key:attacker"]));
        assert!(gate.admits_hash("did:key:victim-viewer", &root));
        assert!(!gate.admits_hash("did:key:attacker", &root));

        for i in 0..100 {
            let verse = format!("attacker-verse-{i}");
            gate.bind_fetched_root(root, ["attacker-verse", verse.as_str()], true);
        }
        assert!(
            !gate.admits_hash("did:key:attacker", &root),
            "a later fetch context must not remap access"
        );
        assert!(
            gate.admits_hash("did:key:victim-viewer", &root),
            "a later fetch context must not narrow the first one (durable DoS)"
        );
    }

    /// OF-8b reversibility: a format-1 state file (W5s — whose learned sets
    /// may carry gossip narrowing, here persisted as overflow) loads with
    /// its learned sets DROPPED: the root stays tracked and denies, and this
    /// node's next own fetch re-learns a clean binding.
    #[test]
    fn a_pre_r1_state_file_drops_its_gossip_narrowed_bindings_and_relearns_on_the_next_fetch() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(BLOB_GATE_FILE);
        let (root, bundle) = ([11u8; 32], [12u8; 32]);
        let mut poisoned: std::collections::BTreeSet<String> =
            (0..64).map(|i| format!("attacker-{i}")).collect();
        poisoned.insert("v".to_string());
        let format_one = serde_json::json!({
            "roots": {
                hex::encode(root): {
                    "published": false,
                    "learned": poisoned,
                    "learned_overflow": true,
                    "children": [hex::encode(bundle)],
                }
            }
        });
        std::fs::write(&path, serde_json::to_vec(&format_one).unwrap()).unwrap();

        let gate = BlobAccessGate::load(path.clone());
        gate.set_verse_access("v", viewers(&["did:key:viewer"]));
        assert!(
            !gate.admits_hash("did:key:stranger", &bundle),
            "still tracked: denies"
        );
        assert!(
            !gate.admits_hash("did:key:viewer", &bundle),
            "no binding until re-fetched"
        );

        gate.record_tileset_root("v", root);
        assert!(
            gate.admits_hash("did:key:viewer", &bundle),
            "the re-fetch re-learns a clean binding"
        );
        assert!(gate.admits_hash("did:key:viewer", &root));

        let restarted = BlobAccessGate::load(path);
        restarted.set_verse_access("v", viewers(&["did:key:viewer"]));
        assert!(
            restarted.admits_hash("did:key:viewer", &bundle),
            "a format-3 binding survives the restart"
        );
    }

    /// OF-10c: an unvouched fetch is an explicit `never_seed` flag, not a
    /// sentinel verse id inside `learned`. A format-2 file's sentinel
    /// migrates to the flag on load — so even a snapshot registered under
    /// the old sentinel id (the collision a refactor could introduce) opens
    /// nothing — and a later vouched fetch never re-binds it.
    #[test]
    fn an_unvouched_root_stays_unseeded_even_if_a_verse_snapshot_collides_with_the_old_sentinel() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(BLOB_GATE_FILE);
        let (root, bundle) = ([21u8; 32], [22u8; 32]);
        let format_two = serde_json::json!({
            "format": 2,
            "roots": {
                hex::encode(root): {
                    "published": false,
                    "learned": ["v", LEGACY_UNVOUCHED_SENTINEL],
                    "children": [hex::encode(bundle)],
                }
            }
        });
        std::fs::write(&path, serde_json::to_vec(&format_two).unwrap()).unwrap();

        let open_to_all = VerseAccessSnapshot {
            owner_did: None,
            open_to_all: true,
            viewer_dids: HashSet::new(),
        };
        let gate = BlobAccessGate::load(path.clone());
        gate.set_verse_access("v", viewers(&["did:key:viewer"]));
        gate.set_verse_access(LEGACY_UNVOUCHED_SENTINEL, open_to_all.clone());
        gate.record_tileset_root("v", root);
        assert!(
            !gate.admits_hash("did:key:viewer", &root)
                && !gate.admits_hash("did:key:viewer", &bundle),
            "a root fetched through an unvouched provider was secondary-seeded"
        );

        let fresh = [23u8; 32];
        gate.bind_fetched_root(fresh, ["v"], false);
        gate.record_tileset_root("v", fresh);
        assert!(!gate.admits_hash("did:key:viewer", &fresh));

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["format"], GATE_STATE_FORMAT);
        assert!(
            !persisted.to_string().contains("unvouched-provider"),
            "the sentinel must not be written back"
        );
        let restarted = BlobAccessGate::load(path);
        restarted.set_verse_access("v", viewers(&["did:key:viewer"]));
        restarted.set_verse_access(LEGACY_UNVOUCHED_SENTINEL, open_to_all);
        assert!(!restarted.admits_hash("did:key:viewer", &root));
        assert!(!restarted.admits_hash("did:key:viewer", &fresh));
    }

    /// OF-8a (HIGH): a fetched HashSeq whose child list names a VICTIM
    /// tileset's bundle hashes never lends its own verse's viewers access to
    /// those bundles — in either learn order (pre-R1, `.any(..)` over the
    /// parent roots admitted the attacker's viewers through the attacker's
    /// root). An own PUBLISHED root listing the same bytes still serves them.
    #[test]
    fn a_fetched_hash_seq_listing_a_victims_children_never_lends_them_its_viewers() {
        let (victim_root, attacker_root, published_root) = ([21u8; 32], [22u8; 32], [23u8; 32]);
        let shared_bundle = [24u8; 32];
        for victim_first in [true, false] {
            let gate = BlobAccessGate::default();
            gate.set_verse_access("victim-verse", viewers(&["did:key:victim-viewer"]));
            gate.set_verse_access("attacker-verse", viewers(&["did:key:attacker"]));
            let learn_victim = || {
                gate.record_tileset_root("victim-verse", victim_root);
                gate.track_content(TilesetContent::Resolved {
                    root: victim_root,
                    children: vec![shared_bundle],
                });
            };
            let learn_attacker = || {
                gate.record_tileset_root("attacker-verse", attacker_root);
                gate.track_content(TilesetContent::Resolved {
                    root: attacker_root,
                    children: vec![shared_bundle],
                });
            };
            if victim_first {
                learn_victim();
                learn_attacker();
            } else {
                learn_attacker();
                learn_victim();
            }
            assert!(
                !gate.admits_hash("did:key:attacker", &shared_bundle),
                "the attacker's root lent its viewers the victim's bundle (victim_first={victim_first})"
            );
            assert!(!gate.admits_hash("did:key:stranger", &shared_bundle));

            gate.track_content(TilesetContent::Published {
                root: published_root,
                children: vec![shared_bundle],
            });
            assert!(
                gate.admits_hash("did:key:victim-viewer", &shared_bundle),
                "an own publish of the same bytes serves them to any open-verse viewer"
            );
        }
    }

    /// R1 (HIGH, child-range laundering): a `GetRequest` whose ranges reach
    /// into a HashSeq's children streams every child the root blob lists.
    /// An UNTRACKED HashSeq this node happens to store (here: a raw blob
    /// listing a protected bundle's hash, as a replicated row's content blob
    /// could) must not carry a stranger past the per-hash gate; the same
    /// blob fetched raw stays unaffected (untracked), and a viewer's
    /// child-range request on the TRACKED root is served. Real loopback.
    #[tokio::test]
    async fn a_child_range_request_through_an_untracked_hash_seq_cannot_launder_a_tracked_bundle() {
        use fe_runtime::blob_transport::BlobTransport;

        let (Some(provider_ep), Some(requester_ep)) =
            (bind_test_endpoint(67).await, bind_test_endpoint(68).await)
        else {
            return;
        };
        let provider_dir = tempfile::TempDir::new().unwrap();
        let requester_dir = tempfile::TempDir::new().unwrap();
        let provider_stack = Arc::new(
            DocsStack::spawn(provider_ep.clone(), provider_dir.path())
                .await
                .expect("provider stack spawns"),
        );
        let requester_stack = Arc::new(
            DocsStack::spawn(requester_ep.clone(), requester_dir.path())
                .await
                .expect("requester stack spawns"),
        );
        let provider_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&provider_stack);
        let bundle = provider_transport
            .add(bytes::Bytes::from_static(b"protected bundle"))
            .await
            .expect("provider stores the bundle");
        let root = provider_transport
            .add_hash_seq(&[bundle])
            .await
            .expect("tracked root");
        provider_stack
            .blob_access_gate()
            .record_tileset_root("verse-1", root);
        provider_stack
            .blob_access_gate()
            .track_content(TilesetContent::Resolved {
                root,
                children: vec![bundle],
            });
        // The laundering vehicle: byte-identical HashSeq content, but a
        // DIFFERENT hash (one extra child), never tracked as a tileset root.
        let decoy = provider_transport
            .add(bytes::Bytes::from_static(b"decoy"))
            .await
            .expect("decoy");
        let untracked_seq = provider_transport
            .add_hash_seq(&[bundle, decoy])
            .await
            .expect("untracked hash seq");

        let get = |hash: [u8; 32], children: bool| {
            let request = if children {
                iroh_blobs::protocol::GetRequest::all(iroh_blobs::Hash::from(hash))
            } else {
                iroh_blobs::protocol::GetRequest::blob(iroh_blobs::Hash::from(hash))
            };
            let requester_ep = requester_ep.clone();
            let provider_addr = provider_ep.addr();
            let store = requester_stack.blob_store().clone();
            async move {
                let conn = requester_ep
                    .connect(provider_addr, iroh_blobs::ALPN)
                    .await
                    .expect("blob ALPN connects");
                store.remote().execute_get(conn, request).complete().await
            }
        };

        assert!(
            get(untracked_seq, true).await.is_err(),
            "a child-range request through an untracked HashSeq must be refused"
        );
        let requester_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&requester_stack);
        assert!(
            requester_transport
                .read_local(bundle)
                .await
                .expect("local read")
                .is_none(),
            "the protected bundle must not land on the stranger"
        );
        assert!(
            get(untracked_seq, false).await.is_ok(),
            "a raw request for an untracked blob stays unaffected"
        );
        assert!(
            get(root, true).await.is_err(),
            "a stranger is refused the tracked root's children"
        );

        let requester_did = crate::replicator::peer_did_key(&requester_ep.id());
        provider_stack
            .blob_access_gate()
            .set_verse_access("verse-1", viewers(&[requester_did.as_str()]));
        assert!(
            get(root, true).await.is_ok(),
            "a verse viewer's child-range request on the tracked root is served"
        );

        provider_stack.shutdown().await;
        requester_stack.shutdown().await;
    }

    /// OF-6b: a HashSeq CHILD is gated like its root, a closed verse's root
    /// denies instead of falling back to "untracked → admit", and the
    /// tracked state survives a restart (reload from the persisted file).
    #[test]
    fn a_tileset_child_stays_gated_across_verse_close_and_restart() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(BLOB_GATE_FILE);
        let (root, index, bundle) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let gate = BlobAccessGate::load(path.clone());
        gate.record_tileset_root("v", root);
        gate.track_content(TilesetContent::Resolved {
            root,
            children: vec![index, bundle],
        });
        gate.set_verse_access("v", viewers(&["did:key:viewer"]));
        assert!(gate.admits_hash("did:key:viewer", &bundle));
        assert!(
            !gate.admits_hash("did:key:stranger", &bundle),
            "direct child fetch bypassed the gate"
        );
        assert!(!gate.admits_hash("did:key:stranger", &index));

        gate.forget_verse("v");
        assert!(
            !gate.admits_hash("did:key:viewer", &bundle),
            "a closed verse's content must deny"
        );

        // Outside a runtime `persist` writes synchronously, so the file is current.
        let restarted = BlobAccessGate::load(path);
        assert!(
            !restarted.admits_hash("did:key:stranger", &bundle),
            "restart re-opened the child"
        );
        assert!(
            !restarted.admits_hash("did:key:stranger", &root),
            "restart re-opened the root"
        );
        restarted.set_verse_access("v", viewers(&["did:key:viewer"]));
        assert!(restarted.admits_hash("did:key:viewer", &bundle));
    }

    /// A tileset this node PUBLISHES is served to a Viewer+ of any open
    /// verse (the publish path advertises to every joined verse), and stops
    /// being served once unpublished — it stays tracked, so it denies.
    #[test]
    fn a_published_tileset_serves_any_open_verse_viewer_until_unpublished() {
        let gate = BlobAccessGate::default();
        let (root, bundle) = ([4u8; 32], [5u8; 32]);
        gate.track_content(TilesetContent::Published {
            root,
            children: vec![bundle],
        });
        gate.set_verse_access("a", viewers(&["did:key:a"]));
        gate.set_verse_access("b", viewers(&["did:key:b"]));
        assert!(gate.admits_hash("did:key:a", &bundle));
        assert!(gate.admits_hash("did:key:b", &bundle));
        assert!(!gate.admits_hash("did:key:stranger", &bundle));
        gate.track_content(TilesetContent::Unpublished { root });
        assert!(!gate.admits_hash("did:key:a", &bundle));
    }

    /// OF-6c: a `GetManyRequest` naming a tracked tileset child is refused
    /// over the real blob ALPN (pre-fix the mask only intercepted `get`, so
    /// `get_many` streamed protected blobs to anyone who knew the hash).
    #[tokio::test]
    async fn a_get_many_request_for_a_tracked_child_is_refused_to_a_stranger() {
        use fe_runtime::blob_transport::BlobTransport;

        let (Some(provider_ep), Some(requester_ep)) =
            (bind_test_endpoint(65).await, bind_test_endpoint(66).await)
        else {
            return;
        };
        let provider_dir = tempfile::TempDir::new().unwrap();
        let requester_dir = tempfile::TempDir::new().unwrap();
        let provider_stack = Arc::new(
            DocsStack::spawn(provider_ep.clone(), provider_dir.path())
                .await
                .expect("provider stack spawns"),
        );
        let requester_stack = DocsStack::spawn(requester_ep.clone(), requester_dir.path())
            .await
            .expect("requester stack spawns");
        let provider_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&provider_stack);
        let child = provider_transport
            .add(bytes::Bytes::from_static(b"protected bundle"))
            .await
            .expect("provider stores the bundle");
        let root = provider_transport
            .add_hash_seq(&[child])
            .await
            .expect("hash seq");
        provider_stack
            .blob_access_gate()
            .record_tileset_root("verse-1", root);
        provider_stack
            .blob_access_gate()
            .track_content(TilesetContent::Resolved {
                root,
                children: vec![child],
            });

        let conn = requester_ep
            .connect(provider_ep.addr(), iroh_blobs::ALPN)
            .await
            .expect("blob ALPN connects");
        let refused = requester_stack
            .blob_store()
            .remote()
            .execute_get_many(
                conn,
                iroh_blobs::protocol::GetManyRequest::from_iter([child]),
            )
            .complete()
            .await;
        assert!(refused.is_err(), "get_many must be gated like get");
        let requester_stack = Arc::new(requester_stack);
        let requester_transport =
            crate::blob_transport::IrohBlobTransport::from_stack(&requester_stack);
        assert!(
            requester_transport
                .read_local(child)
                .await
                .expect("local read")
                .is_none(),
            "no protected blob may land on the stranger"
        );

        provider_stack.shutdown().await;
        requester_stack.shutdown().await;
    }

    #[test]
    fn alpn_constants_match_iroh_1x_wire_protocols() {
        assert_eq!(iroh_blobs::ALPN, b"/iroh-bytes/4");
        // iroh-gossip 0.101 bumped the gossip ALPN (0.35 was /iroh-gossip/0).
        assert_eq!(iroh_gossip::ALPN, b"/iroh-gossip/1");
        assert_eq!(iroh_docs::ALPN, b"/iroh-sync/1");
    }
}
