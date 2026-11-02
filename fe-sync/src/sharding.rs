//! Shard model + per-verse fabric state (M2/F6 — A13/A14; RP-5 adds the
//! sync-plane role view). See `fe-sync/src/AGENTS.md` §sharding and
//! §role-view.
//!
//! Shard id = (petal, anchor node, time bucket): a reading maps to exactly
//! one shard by `(anchor, recorded_at_ms)` integer-divided by the verse's
//! bucket width (epoch-aligned). The **shard ledger** rides the verse's own
//! iroh-docs namespace under `__shards/{shard}` keys, peer capacity
//! declarations under `__peers/{did}` — both replicate like any row, so a
//! joining peer converges the whole placement picture from the doc snapshot
//! (`seed_reconciliation` replays it through the same fabric `note_*` seam
//! the live pump uses).
//!
//! The fabric is **sync-plane state, not SurrealDB state**: the DB thread
//! maps `__shards`/`__peers` rows to `NotApplicable` — they never touch the
//! local store. One writer per shard ledger key (the peer that first saw
//! the shard plans and publishes), conflicts resolved by the doc's
//! latest-per-entry semantics.
//!
//! RP-5 §2.5: the fabric also runs the shared `fe_policy::role_grants`
//! materializer over `role` rows (both directions), learns the owner from
//! the first manifest's `created_by` (pinned, never relearned), and learns
//! a `petal -> fractal` map from `petal` rows so a `__shards` key (which
//! carries only the petal id) can be turned into a full scope string. Every
//! gate below calls [`VerseFabric::role_at`] at the row's deepest scope —
//! the one function the DB plane's A3 gate and local grant authority also
//! use (`fe_policy::role_grants::EffectiveRoles::role_at`), with
//! `default_access` folded in as the ⊥ fallback.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use fe_policy::RoleLevel;
use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings};

use crate::placement::{
    plan_shard_hosts, retention_decision, transfer_route, PlacementPlan, Retention, TransferRoute,
};

/// Doc-entry table prefix for shard ledger rows (`__shards/{petal}/{anchor}/{bucket}`).
pub const SHARD_TABLE: &str = "__shards";
/// Doc-entry table prefix for peer capacity declarations (`__peers/{did}`).
pub const PEER_DECL_TABLE: &str = "__peers";
/// Doc-entry table prefix for holder declarations
/// (`__holders/{petal}/{anchor}/{bucket}/{did}`, RP-6 §2.6): an author whose
/// local reading lands in a shard it does not host declares it holds a
/// creator copy, so a requester expects (and briefly waits for) its answer.
pub const HOLDERS_TABLE: &str = "__holders";

/// Resolve a peer's role at `scope` with the §2.2 ⊥ fallback folded in —
/// the one function behind [`VerseFabric::role_at`] and
/// [`AnswererView::verdict`], so the gate and the merge filter can never
/// resolve a role two different ways (`fe_policy::role_grants` underneath).
fn resolve_role(
    effective: &fe_policy::EffectiveRoles,
    default_access: RoleLevel,
    peer_did: &str,
    scope: &str,
) -> RoleLevel {
    let explicit = effective.role_at(peer_did, scope);
    if explicit == RoleLevel::None {
        default_access
    } else {
        explicit
    }
}

/// Split a `__holders` record id (`{petal}/{anchor}/{bucket}/{did}`) into
/// `(shard_key, holder_did)`. `None` when the key is not exactly a shard
/// key plus one trailing DID segment.
pub fn parse_holder_key(record_id: &str) -> Option<(&str, &str)> {
    let (shard_key, did) = record_id.rsplit_once('/')?;
    if did.is_empty()
        || shard_key.split('/').count() != 3
        || shard_key.split('/').any(str::is_empty)
    {
        return None;
    }
    Some((shard_key, did))
}

/// How the requester's role view judges an unexpected answerer (one that is
/// neither a ledger host nor an admitted holder of any targeted shard) —
/// the RP-6 §2.6 merge filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswererVerdict {
    /// Editor+ at the shard's petal scope — its rows are kept.
    Admitted,
    /// Converged view, below Editor — its rows are discarded and the
    /// answerer is reported in `meta.rejected_answerers`.
    Rejected,
    /// The view has not converged (no owner pinned, or the petal's scope is
    /// unknown locally) — its rows are excluded (OF-7c, fail closed) and the
    /// answerer is reported in `meta.unverified_answerers`.
    Unverified,
}

/// An owned snapshot of the requester's role view at one petal's scope,
/// taken at plan time so the (pure) merge can judge answerers without
/// borrowing the sync thread's fabric across the collector's awaits.
#[derive(Debug, Clone, PartialEq)]
pub struct AnswererView {
    petal_scope: Option<String>,
    owner_pinned: bool,
    effective: fe_policy::EffectiveRoles,
    default_access: RoleLevel,
}

impl Default for AnswererView {
    /// A view that has converged on nothing — every answerer is unverified.
    fn default() -> Self {
        Self {
            petal_scope: None,
            owner_pinned: false,
            effective: fe_policy::EffectiveRoles::default(),
            default_access: RoleLevel::None,
        }
    }
}

impl AnswererView {
    /// Judge one unexpected answerer: Editor+ at the petal scope (`role_at`,
    /// the same resolution every fabric gate uses) or not, or not judgeable.
    pub fn verdict(&self, answerer_did: &str) -> AnswererVerdict {
        let Some(scope) = self.petal_scope.as_deref().filter(|_| self.owner_pinned) else {
            return AnswererVerdict::Unverified;
        };
        if resolve_role(&self.effective, self.default_access, answerer_did, scope)
            .is_at_least(RoleLevel::Editor)
        {
            AnswererVerdict::Admitted
        } else {
            AnswererVerdict::Rejected
        }
    }
}

/// Quarantine cap per verse (§2.5) — an accelerator, not the safety net. An
/// evicted row is still in the replica; it comes back through the §2.5
/// re-read on empowerment once its author is granted a sufficient role, or
/// through the next replica open's reconciliation replay once the petal map
/// catches up.
const QUARANTINE_CAP: usize = 1024;

/// One shard's identity: (petal, anchor node, epoch-aligned time bucket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardId {
    pub petal_id: String,
    pub anchor_node_id: String,
    pub bucket: i64,
}

impl ShardId {
    /// The doc-record key: `{petal}/{anchor}/{bucket}` (slashed, so the
    /// `{table}/{record_id}` split keeps the whole shard id in `record_id`).
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.petal_id, self.anchor_node_id, self.bucket)
    }

    /// Epoch-aligned bucket index for a reading's `recorded_at_ms`.
    /// `div_euclid` keeps pre-1970 timestamps floor-aligned.
    pub fn bucket_index(recorded_at_ms: i64, bucket_width_ms: u64) -> i64 {
        recorded_at_ms.div_euclid(bucket_width_ms as i64)
    }

    /// Inclusive-exclusive range covered by one bucket.
    pub fn bucket_range(bucket: i64, bucket_width_ms: u64) -> (i64, i64) {
        let start = bucket.saturating_mul(bucket_width_ms as i64);
        (start, start.saturating_add(bucket_width_ms as i64))
    }

    /// The shard a serialized `iot_reading` row belongs to (`None` when the
    /// row does not carry the fields the model needs).
    pub fn of_reading_row(row: &serde_json::Value, bucket_width_ms: u64) -> Option<Self> {
        let petal_id = row.get("petal_id")?.as_str()?.to_string();
        let anchor_node_id = row.get("node_id")?.as_str()?.to_string();
        let recorded_at_ms = row.get("recorded_at_ms")?.as_i64()?;
        Some(Self {
            petal_id,
            anchor_node_id,
            bucket: Self::bucket_index(recorded_at_ms, bucket_width_ms),
        })
    }

    /// The petal a shard key (`{petal}/{anchor}/{bucket}`) belongs to.
    fn petal_of_key(shard_key: &str) -> &str {
        shard_key.split('/').next().unwrap_or("")
    }
}

/// One peer's hosting declaration — the `__peers/{did}` doc row. `capacity_bytes:
/// None` means unlimited (the default; the local peer declares defaults until
/// the settings surface says otherwise).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerDeclaration {
    /// Declared shard-hosting capacity in bytes (a planning hint, not an
    /// enforcement wall — see `placement::plan_shard_hosts`).
    pub capacity_bytes: Option<u64>,
    /// Fallback / relay seeder: hosts shards the regular peers cannot fit
    /// (D2 #3/#4 — overflow target, never a coordinator).
    pub seeder: bool,
}

impl PeerDeclaration {
    /// Parse a `__peers` doc row payload.
    pub fn from_row(row: &serde_json::Value) -> Self {
        Self {
            capacity_bytes: row
                .get("capacity_bytes")
                .and_then(|v| v.as_u64())
                .filter(|v| *v > 0),
            seeder: row.get("seeder").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

/// One shard ledger row — the `__shards/{key}` doc payload. The host set is
/// the placement plan at first sight; `row_count`/`size_bytes` are
/// best-effort estimates (written once at first sight, tracked locally
/// afterwards — re-publishing per reading would double doc writes for
/// metadata that is not authoritative).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShardLedgerEntry {
    /// The shard key (`{petal}/{anchor}/{bucket}`).
    pub shard: String,
    /// Hosting peers' DIDs (the placement plan).
    pub hosts: Vec<String>,
    /// Mode recorded at placement (the ledger is honest history).
    pub mode: TimeseriesMode,
    /// R requested at placement.
    pub replication_factor: u32,
    /// Bucket width in force at placement (the bucket index depends on it).
    pub bucket_width_ms: u64,
    /// Inclusive bucket start (epoch ms).
    pub range_start_ms: i64,
    /// Exclusive bucket end (epoch ms).
    pub range_end_ms: i64,
    /// Best-effort row count estimate.
    pub row_count: u64,
    /// Best-effort size estimate (bytes).
    pub size_bytes: u64,
}

impl ShardLedgerEntry {
    fn from_row(row: &serde_json::Value) -> Option<Self> {
        Some(Self {
            shard: row.get("shard")?.as_str()?.to_string(),
            hosts: row
                .get("hosts")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            mode: row
                .get("mode")
                .and_then(|v| v.as_str())
                .and_then(TimeseriesMode::parse)
                .unwrap_or(TimeseriesMode::Mirror),
            replication_factor: row
                .get("replication_factor")
                .and_then(|v| v.as_u64())
                .map(|v| v.max(1) as u32)
                .unwrap_or(1),
            bucket_width_ms: row
                .get("bucket_width_ms")
                .and_then(|v| v.as_u64())
                .filter(|v| *v > 0)
                .unwrap_or(fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS),
            range_start_ms: row
                .get("range_start_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            range_end_ms: row
                .get("range_end_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            row_count: row.get("row_count").and_then(|v| v.as_u64()).unwrap_or(0),
            size_bytes: row.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }
}

/// Parse a verse manifest's `default_access` the same way the DB plane's
/// structural check does (§2.2 "never silently map through `RoleLevel::
/// from`"): only `"viewer"`/`"none"` are recognised, anything else
/// (missing, or a forbidden value an admitted DB-plane write would have
/// denied outright) is the safe default.
fn parse_default_access(row: &serde_json::Value) -> RoleLevel {
    match row.get("default_access").and_then(|v| v.as_str()) {
        Some("viewer") => RoleLevel::Viewer,
        _ => RoleLevel::None,
    }
}

/// A row held back because the fabric cannot judge it yet — its author's
/// role hasn't converged, or (for a `__shards` row) its petal isn't known
/// locally. Never a permanent deny: the replica still holds the row, and
/// §2.5's re-read on empowerment or the petal map catching up recovers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuarantineKind {
    Shard,
    Holder,
    /// A `petal` row whose author lacks Editor+ at the petal's scope(s) yet
    /// (OF-9) — the petal->fractal map is the spine of every scope chain.
    Petal,
}

/// Where a `verse`/`petal` row the fabric notes came from (R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowOrigin {
    /// A replicated row signed by another author — fully gated.
    Inbound,
    /// This node's own outbound write, just admitted by the local DB plane.
    LocalWrite,
    /// One of this node's own past entries replayed from the replica —
    /// locally authoritative, but possibly stale.
    OwnReplay,
}

/// An id that cannot smuggle a scope boundary (`#`) into
/// `fe_policy::build_scope` — the DB plane's `is_scope_safe_id`
/// (`fe-database/src/handlers/replicated_row.rs`), mirrored so both planes
/// refuse the same petal rows.
fn is_scope_safe_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('#')
}

/// How many past mappings a petal keeps for move rollback (OF-10b). Past
/// the cap the oldest INBOUND layer is evicted — never the
/// locally-authoritative base, the layer just pushed, or the incumbent it
/// replaced; a move is always applied (OF-13a/c) — `src/AGENTS.md`
/// §role-view "OF-13".
const PETAL_MAPPING_HISTORY_CAP: usize = 4;
/// How many competing claims a contested petal keeps (OF-10a). Past the
/// cap the INCOMING claim is refused, never an existing one evicted
/// (OF-11b); the refusal is remembered in `refused_contest_claims`.
const PETAL_CLAIMS_CAP: usize = 8;

/// One admitted `petal -> fractal` mapping and who authorized it (R6,
/// OF-10a/b) — `src/AGENTS.md` §role-view "Petal -> fractal map".
#[derive(Debug, Clone)]
struct PetalMapping {
    fractal_id: String,
    /// The fractal this layer MOVED the petal from (OF-13): its author must
    /// keep Editor+ there too. `None` = not a move (first mapping, refresh,
    /// settled contest, local write). Stored, never read off the stack
    /// below, so the lapse check is position-independent.
    from_fractal: Option<String>,
    /// The signed author whose Editor+ admitted it; `None` = this node's own
    /// write or own replay of an unmapped petal (locally authoritative, never
    /// re-verdicted or contested).
    author: Option<String>,
    /// The admitted row, re-quarantined if `author` loses its authority.
    row: Vec<u8>,
}

/// Push a new layer onto a petal's rollback stack, capped at
/// [`PETAL_MAPPING_HISTORY_CAP`] by evicting the oldest inbound layer below
/// the incumbent (OF-13a/c — never refuses) — `src/AGENTS.md` §role-view
/// "OF-13".
fn push_mapping_layer(history: &mut Vec<PetalMapping>, layer: PetalMapping) {
    history.push(layer);
    if history.len() <= PETAL_MAPPING_HISTORY_CAP {
        return;
    }
    // `len - 2` excludes the layer just pushed AND the incumbent it replaced.
    let below_incumbent = history.len() - 2;
    if let Some(evict_at) = (0..below_incumbent).find(|&i| history[i].author.is_some()) {
        history.remove(evict_at);
    }
}

#[derive(Debug, Clone)]
struct QuarantinedRow {
    kind: QuarantineKind,
    record_id: String,
    author_id: String,
    data: Vec<u8>,
}

/// Per-verse timeseries fabric state: settings (from the verse manifest),
/// peer declarations, the shard ledger, and (RP-5) the sync-plane role view.
/// Owned by the sync thread's command loop — one entry per open verse,
/// learned from BOTH directions of verse-doc traffic (outbound writes parse
/// the blob before publishing; inbound rows and the reconciliation snapshot
/// go through `note_entry`).
#[derive(Debug)]
pub struct VerseFabric {
    pub settings: VerseTimeseriesSettings,
    /// Every declared peer by DID, the local one included.
    pub peers: BTreeMap<String, PeerDeclaration>,
    /// Shard ledger by shard key.
    pub shards: BTreeMap<String, ShardLedgerEntry>,

    /// The verse owner, pinned once from the first manifest's `created_by`
    /// (§2.5 — same immutability rule as the DB plane: never relearned).
    owner_did: Option<String>,
    /// The verse manifest's `default_access` (§2.2's ⊥ fallback); `None`
    /// until a manifest has been admitted.
    default_access: RoleLevel,
    /// Admitted role statements, one per `(scope, peer_did, grantor_did)` —
    /// the sync-plane analogue of the DB plane's `role_grant` table. A
    /// statement that fails `admissible` is never stored here (§2.5): the
    /// replica already holds it durably.
    role_statements: HashMap<(String, String, String), fe_policy::RoleStatement>,
    /// The materialized role view over `role_statements` (recomputed to a
    /// fixpoint, including the demote cascade, every time the set changes).
    effective: fe_policy::EffectiveRoles,
    /// `petal_id -> fractal_id`, learned from `petal` rows (§2.5) — a
    /// `__shards`/`__holders` key carries only the petal id, but
    /// `fe_policy::build_scope` needs the fractal segment too. Each entry is
    /// the mapping history, last = current; earlier entries are what a move
    /// replaced, restored if the move's author loses authority (OF-10b).
    petal_fractals: HashMap<String, Vec<PetalMapping>>,
    /// Petals whose inbound mappings conflict with no author holding Editor+
    /// at every claimed scope (OF-10a): unmapped — fail closed — until such
    /// an author's row, a local write, or a lapsed claimant settles them.
    contested_petals: HashMap<String, Vec<PetalMapping>>,
    /// Per contested petal, `author -> fractal` of each claim REFUSED at
    /// `PETAL_CLAIMS_CAP` (OF-11b), still owed by settlement's proof. One
    /// entry per author (bounded), lapse-pruned at every settle, cleared
    /// wherever the contest ends (OF-13b) — `src/AGENTS.md` §role-view
    /// "OF-13".
    refused_contest_claims: HashMap<String, BTreeMap<String, String>>,
    /// How many contested-petal claims have been refused at the cap
    /// (diagnostics/tests, OF-11b) — the flood-counter idiom mirroring
    /// `quarantine_evicted`.
    petal_claim_refusals: u64,
    /// DIDs whose `role_at` rose since the last materialization, drained by
    /// the sync thread's command loop (§2.5 re-read on empowerment).
    reread_queue: VecDeque<String>,
    /// Rows gated on a role/petal view that has not converged yet.
    quarantine: VecDeque<QuarantinedRow>,
    quarantine_evicted: u64,
    /// Admitted holder declarations by shard key (RP-6 §2.6): DIDs whose
    /// `__holders/{shard}/{did}` row passed the Editor+ petal-scope gate.
    holders: BTreeMap<String, BTreeSet<String>>,
    /// Shard keys the LOCAL peer has declared it holds — set when the
    /// declaration is published and re-learned from our own snapshot rows
    /// after a restart, so a declaration is published once per shard.
    local_holdings: BTreeSet<String>,
}

impl Default for VerseFabric {
    fn default() -> Self {
        Self {
            settings: VerseTimeseriesSettings::default(),
            peers: BTreeMap::new(),
            shards: BTreeMap::new(),
            owner_did: None,
            default_access: RoleLevel::None,
            role_statements: HashMap::new(),
            effective: fe_policy::EffectiveRoles::default(),
            petal_fractals: HashMap::new(),
            contested_petals: HashMap::new(),
            refused_contest_claims: HashMap::new(),
            petal_claim_refusals: 0,
            reread_queue: VecDeque::new(),
            quarantine: VecDeque::new(),
            quarantine_evicted: 0,
            holders: BTreeMap::new(),
            local_holdings: BTreeSet::new(),
        }
    }
}

impl VerseFabric {
    /// Record the local peer's declaration (replica open / SetShardCapacity).
    pub fn note_local_declaration(&mut self, local_did: &str, decl: PeerDeclaration) {
        self.peers.insert(local_did.to_string(), decl);
    }

    /// Record a remote peer's `__peers` declaration row. Returns whether the
    /// declaration changed (callers may re-plan on membership churn later —
    /// F6 does not re-plan existing shards).
    pub fn note_peer_declaration(&mut self, peer_did: &str, decl: PeerDeclaration) -> bool {
        match self.peers.get(peer_did) {
            Some(old) if *old == decl => false,
            _ => {
                self.peers.insert(peer_did.to_string(), decl);
                true
            }
        }
    }

    /// Parse the `ts_*`/`default_access` settings off an INBOUND (replicated
    /// or snapshot) verse manifest row.
    ///
    /// The owner is pinned (§2.5, never relearned) only from a SELF-SIGNED
    /// manifest — the entry's signed author IS its `created_by` (OF-6d). A
    /// manifest claiming someone else as creator pins nothing and adopts no
    /// settings; instead the claimed creator is queued for the §2.5 re-read,
    /// which replays THEIR own `verse/` entry from the replica (so a later
    /// Manager edit shadowing the owner's entry in the snapshot cannot stall
    /// the pin). Until an owner is pinned no settings are adopted at all.
    /// After that, settings are adopted only from the owner or a verse
    /// Manager+ (RP-5 step 2, `role_at` at the verse scope) — a petal/fractal
    /// Manager's or an Editor's edit is ignored, because verse settings lie
    /// above their subtree. Returns whether the fabric's settings changed.
    pub fn note_verse_row(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        author_id: &str,
    ) -> bool {
        self.note_verse_row_from(verse_id, row, author_id, RowOrigin::Inbound)
    }

    /// [`Self::note_verse_row`] for this node's OWN outbound manifest write:
    /// the local DB plane already admitted it (the creator's manifest, or an
    /// invite-pinned owner — fe-database T10), so its `created_by` pins the
    /// owner — replacing a different pin a replicated manifest set first
    /// (R1) — and, as the first or correcting manifest, its settings are
    /// adopted outright.
    pub fn note_local_verse_row(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        local_did: &str,
    ) -> bool {
        self.note_verse_row_from(verse_id, row, local_did, RowOrigin::LocalWrite)
    }

    /// One of this node's OWN past manifest entries replayed from the
    /// replica (snapshot, re-assert, live echo — R1). Its `created_by` is the
    /// local authority on the owner, so it corrects a false pin like
    /// [`Self::note_local_verse_row`]; but it may be STALE (a newer authorized
    /// edit by someone else can exist), so its settings are adopted outright
    /// only when it just replaced a false owner, and otherwise pass the
    /// ordinary inbound authority gate.
    pub fn note_own_replayed_verse_row(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        local_did: &str,
    ) -> bool {
        self.note_verse_row_from(verse_id, row, local_did, RowOrigin::OwnReplay)
    }

    fn note_verse_row_from(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        author_id: &str,
        origin: RowOrigin,
    ) -> bool {
        let first_manifest = self.owner_did.is_none();
        let mut repinned = false;
        if let Some(created_by) = row.get("created_by").and_then(|v| v.as_str()) {
            if origin != RowOrigin::Inbound {
                repinned = self.repin_owner_from_local(verse_id, created_by);
            } else if created_by == author_id {
                self.learn_owner(verse_id, created_by);
            } else if first_manifest
                && !created_by.is_empty()
                && !self.reread_queue.iter().any(|d| d == created_by)
            {
                tracing::debug!(
                    verse_id,
                    author = author_id,
                    created_by,
                    "verse manifest not signed by its claimed creator — owner not pinned; \
                     re-reading the claimed creator's own manifest entry"
                );
                self.reread_queue.push_back(created_by.to_string());
            }
        }
        // The local DB plane already admitted our own outbound manifest, so
        // with no owner pinned yet it is adopted outright (as before OF-6d) —
        // and likewise when it just corrected a false owner pin (R1), whose
        // settings (e.g. `default_access: viewer`) must not outlive it.
        let adopt_outright = match origin {
            RowOrigin::LocalWrite => first_manifest || repinned,
            RowOrigin::OwnReplay => repinned,
            RowOrigin::Inbound => false,
        };
        if !adopt_outright {
            let Some(owner) = self.owner_did.clone() else {
                tracing::debug!(
                    verse_id,
                    author = author_id,
                    "verse manifest settings ignored — no owner pinned yet"
                );
                return false;
            };
            let verse_scope = fe_policy::build_scope(verse_id, None, None);
            let is_owner = owner == author_id;
            if !is_owner
                && !self
                    .role_at(author_id, &verse_scope)
                    .is_at_least(RoleLevel::Manager)
            {
                tracing::debug!(
                    verse_id,
                    author = author_id,
                    "verse manifest settings ignored — author is not Manager+ at the verse \
                     scope or the owner"
                );
                return false;
            }
        }
        let parsed = VerseTimeseriesSettings::from_verse_row(row);
        let default_access = parse_default_access(row);
        let changed = parsed != self.settings || default_access != self.default_access;
        if changed {
            tracing::info!(
                mode = parsed.mode.as_str(),
                replication_factor = parsed.replication_factor,
                bucket_width_ms = parsed.bucket_width_ms,
                default_access = %default_access,
                "Verse fabric: timeseries settings updated from the verse manifest"
            );
            self.settings = parsed;
            self.default_access = default_access;
        }
        changed
    }

    /// Pin the verse owner once, from the manifest's `created_by` claim —
    /// the same immutability rule as the DB plane (T9): never relearned
    /// once set. Rematerializes (owner learned ⇒ `role_at(owner, *)` jumps
    /// to `Owner`, which the §2.5 re-read picks up as a risen DID).
    fn learn_owner(&mut self, verse_id: &str, created_by: &str) {
        if self.owner_did.is_some() || created_by.is_empty() {
            return;
        }
        self.owner_did = Some(created_by.to_string());
        self.rematerialize(verse_id);
    }

    /// This node's OWN DB-admitted manifest names `created_by` (the creator
    /// itself, or an invite-pinned owner — fe-database T10): that is the
    /// local authority on who owns the verse, so it pins the owner and, when
    /// a replicated self-signed manifest pinned someone else first, REPLACES
    /// that pin (R1). `seed_reconciliation` replays the replica before the
    /// pending local writes flush, and any member holding the namespace
    /// secret can author a self-signed `verse/` entry naming itself, so
    /// first-self-signed-wins alone handed a fresh joiner's sync-plane owner
    /// to whoever wrote first. Rematerializing under the corrected owner
    /// runs the demote cascade over every statement the false owner made.
    /// Returns `true` when it REPLACED a different, earlier pin.
    fn repin_owner_from_local(&mut self, verse_id: &str, created_by: &str) -> bool {
        if created_by.is_empty() || self.owner_did.as_deref() == Some(created_by) {
            return false;
        }
        let replaced = self.owner_did.is_some();
        if let Some(previous) = &self.owner_did {
            tracing::warn!(
                verse_id,
                previous = %previous,
                owner = created_by,
                "sync-plane owner pin corrected by this node's own DB-admitted manifest"
            );
        }
        self.owner_did = Some(created_by.to_string());
        self.rematerialize(verse_id);
        replaced
    }

    /// Learn one `petal -> fractal` mapping from an INBOUND (replicated or
    /// snapshot) `petal` row (§2.5), gated like the DB plane's A3
    /// `confine_petal` (OF-9): the signed author needs Editor+ (the
    /// fe-policy write floor) at the petal's scope under the row's fractal,
    /// AND — when the row MOVES an already-mapped petal — at its current
    /// scope too. The map feeds every `role_at` scope chain, so an
    /// unauthenticated row could otherwise re-parent a victim petal under an
    /// attacker's fractal (their grant there suddenly enclosing it) or strip
    /// legitimate Managers of it. A row that fails is quarantined, never
    /// dropped — re-judged on every role change, and re-read from the
    /// replica on its author's empowerment (`petal/` is in the re-read set).
    /// Conflicting first mappings with no dominant author are contested
    /// (OF-10a) and every inbound mapping is re-verdicted on role change
    /// (OF-10b) — `src/AGENTS.md` §role-view "Petal -> fractal map".
    pub fn note_petal_row(&mut self, verse_id: &str, row: &serde_json::Value, author_id: &str) {
        self.admit_petal_entry(verse_id, row, author_id, RowOrigin::Inbound);
    }

    /// [`Self::note_petal_row`] for this node's OWN outbound petal write:
    /// the local DB plane already admitted it, so it maps outright.
    pub fn note_local_petal_row(&mut self, verse_id: &str, row: &serde_json::Value) {
        self.admit_petal_entry(verse_id, row, "", RowOrigin::LocalWrite);
    }

    /// One of this node's OWN past petal entries replayed from the replica
    /// (R1): it maps a petal not mapped yet outright, but — being possibly
    /// stale — a MOVE of an already-mapped petal passes the ordinary gate
    /// under `local_did`.
    pub fn note_own_replayed_petal_row(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        local_did: &str,
    ) {
        self.admit_petal_entry(verse_id, row, local_did, RowOrigin::OwnReplay);
    }

    fn admit_petal_entry(
        &mut self,
        verse_id: &str,
        row: &serde_json::Value,
        author_id: &str,
        origin: RowOrigin,
    ) {
        let (Some(petal_id), Some(fractal_id)) = (
            row.get("petal_id").and_then(|v| v.as_str()),
            row.get("fractal_id").and_then(|v| v.as_str()),
        ) else {
            return;
        };
        if !is_scope_safe_id(petal_id) || !is_scope_safe_id(fractal_id) {
            tracing::debug!(
                verse_id,
                petal_id,
                fractal_id,
                "petal row with a scope-unsafe id — dropped"
            );
            return;
        }
        let data = serde_json::to_vec(row).unwrap_or_default();
        let current = self
            .petal_fractals
            .get(petal_id)
            .and_then(|history| history.last())
            .cloned();
        let gated = match origin {
            RowOrigin::Inbound => true,
            RowOrigin::LocalWrite => false,
            // OF-11a: a CONTESTED petal has no `current` mapping (it was
            // removed from `petal_fractals` when the contest formed), so
            // `current.is_some()` alone let a stale own replay — which
            // `reassert_own_rows` fires automatically right after every
            // snapshot — take the unconditional locally-authoritative path
            // below and silently settle the contest, bypassing arbitration.
            // A contested own replay must go through the normal claim path
            // (`add_petal_claim` + `settle_contested_petal`) with the local
            // DID as author instead; a LIVE local write (`LocalWrite`,
            // above) is unaffected and keeps its unconditional behavior.
            RowOrigin::OwnReplay => {
                current.is_some() || self.contested_petals.contains_key(petal_id)
            }
        };
        if !gated {
            // Locally authoritative: settles a contest, replaces any history.
            let moved = current
                .as_ref()
                .is_some_and(|cur| cur.fractal_id != fractal_id);
            let was_unmapped = current.is_none();
            self.contested_petals.remove(petal_id);
            self.refused_contest_claims.remove(petal_id);
            self.petal_fractals.insert(
                petal_id.to_string(),
                vec![PetalMapping {
                    fractal_id: fractal_id.to_string(),
                    from_fractal: None,
                    author: None,
                    row: data,
                }],
            );
            if moved || was_unmapped {
                self.reverdict_holders(verse_id);
                self.reevaluate_quarantine(verse_id);
            }
            return;
        }
        let mut claim = PetalMapping {
            fractal_id: fractal_id.to_string(),
            from_fractal: None,
            author: Some(author_id.to_string()),
            row: data,
        };
        let authorized = self.editor_at(verse_id, author_id, fractal_id, petal_id);
        if current
            .as_ref()
            .is_some_and(|cur| cur.fractal_id == fractal_id)
        {
            // Same mapping: an authorized newer row refreshes an inbound
            // provenance, so the mapping outlives its creator's demotion
            // while any current Editor+ still asserts it (OF-10b); the older
            // layer stays below it, restored if this author lapses first.
            let inbound_incumbent = current.as_ref().is_some_and(|cur| cur.author.is_some());
            if authorized && inbound_incumbent {
                if let Some(history) = self.petal_fractals.get_mut(petal_id) {
                    if history.last().is_some_and(|top| top.author == claim.author) {
                        // The author re-asserting its own move still owes
                        // authority at the scope that move replaced.
                        claim.from_fractal = history.pop().and_then(|top| top.from_fractal);
                    }
                    push_mapping_layer(history, claim);
                }
            }
            return;
        }
        if !authorized {
            tracing::debug!(
                verse_id,
                petal_id,
                fractal_id,
                author = author_id,
                "petal row author lacks Editor+ at the petal's scope — mapping quarantined"
            );
            self.quarantine_row(QuarantineKind::Petal, petal_id, author_id, &claim.row);
            return;
        }
        if self.contested_petals.contains_key(petal_id) {
            self.add_petal_claim(petal_id, claim);
            if self.settle_contested_petal(verse_id, petal_id) {
                self.reverdict_holders(verse_id);
                self.reevaluate_quarantine(verse_id);
            }
            return;
        }
        let Some(current) = current else {
            self.petal_fractals
                .insert(petal_id.to_string(), vec![claim]);
            self.reevaluate_quarantine(verse_id);
            return;
        };
        if self.editor_at(verse_id, author_id, &current.fractal_id, petal_id) {
            // A move by an author with authority over BOTH scopes — always
            // applied (OF-13c); the scope it left is recorded on the layer.
            claim.from_fractal = Some(current.fractal_id.clone());
            let history = self.petal_fractals.entry(petal_id.to_string()).or_default();
            push_mapping_layer(history, claim);
            self.reverdict_holders(verse_id);
            self.reevaluate_quarantine(verse_id);
            return;
        }
        // OF-10a: Editor+ at its own target scope only. The incumbent stands
        // when it is local or its author ALSO has authority at the
        // challenger's scope; otherwise neither side dominates — the first
        // writer is no evidence of the petal's real home, so the petal is
        // contested (unmapped, fail-closed) until an author with authority
        // at every claimed scope settles it.
        let incumbent_dominates = match current.author.as_deref() {
            None => true,
            Some(incumbent) => self.editor_at(verse_id, incumbent, fractal_id, petal_id),
        };
        if incumbent_dominates {
            tracing::debug!(
                verse_id,
                petal_id,
                fractal_id,
                author = author_id,
                "petal re-parent without authority at the current scope — quarantined"
            );
            self.quarantine_row(QuarantineKind::Petal, petal_id, author_id, &claim.row);
            return;
        }
        tracing::warn!(
            verse_id,
            petal_id,
            incumbent_fractal = %current.fractal_id,
            challenger_fractal = fractal_id,
            challenger = author_id,
            "conflicting petal mappings with no dominant author — petal contested (unmapped) \
             until an author with authority over every claimed scope settles it"
        );
        self.petal_fractals.remove(petal_id);
        self.contested_petals
            .insert(petal_id.to_string(), vec![current, claim]);
        self.reverdict_holders(verse_id);
        self.reevaluate_quarantine(verse_id);
    }

    /// Editor+ (the fe-policy write floor, R1's petal-row floor) at
    /// `petal_id`'s scope under `fractal_id`.
    fn editor_at(&self, verse_id: &str, did: &str, fractal_id: &str, petal_id: &str) -> bool {
        let scope = fe_policy::build_scope(verse_id, Some(fractal_id), Some(petal_id));
        self.role_at(did, &scope).is_at_least(RoleLevel::Editor)
    }

    /// Add (or refresh, per `(author, fractal)`) one claim on a contested
    /// petal. Past the cap the incoming claim is REFUSED rather than
    /// evicting an existing one (OF-11b): its `author -> fractal` is
    /// remembered in `refused_contest_claims` (latest per author — iroh-docs
    /// keeps one entry per `(key, author)`, so an author's earlier refused
    /// row is already superseded) and the row is quarantined, never dropped,
    /// so a later shrink of the claim set frees room for a retry —
    /// `src/AGENTS.md` §role-view "OF-11"/"OF-13".
    fn add_petal_claim(&mut self, petal_id: &str, claim: PetalMapping) {
        let Some(claims) = self.contested_petals.get_mut(petal_id) else {
            return;
        };
        claims.retain(|c| !(c.author == claim.author && c.fractal_id == claim.fractal_id));
        if claims.len() < PETAL_CLAIMS_CAP {
            claims.push(claim);
            return;
        }
        self.refused_contest_claims
            .entry(petal_id.to_string())
            .or_default()
            .insert(
                claim.author.clone().unwrap_or_default(),
                claim.fractal_id.clone(),
            );
        self.petal_claim_refusals += 1;
        tracing::warn!(
            petal_id,
            fractal_id = %claim.fractal_id,
            author = claim.author.as_deref().unwrap_or_default(),
            cap = PETAL_CLAIMS_CAP,
            total_refused = self.petal_claim_refusals,
            "contested petal at its claim cap — new claim refused, never evicting an existing \
             claim (eviction would let a flood settle the contest without proving authority \
             over the evicted fractal)"
        );
        self.quarantine_row(
            QuarantineKind::Petal,
            petal_id,
            claim.author.as_deref().unwrap_or_default(),
            &claim.row,
        );
    }

    /// Re-judge one contested petal (OF-10a): claims whose author lost
    /// Editor+ at its own scope are quarantined; the petal maps when the
    /// surviving claims agree on one fractal, or when one claimant holds
    /// Editor+ at EVERY claimed scope (the latest such wins). `true` when
    /// the petal left the contested state (mapped, or no claim survived).
    fn settle_contested_petal(&mut self, verse_id: &str, petal_id: &str) -> bool {
        let Some(claims) = self.contested_petals.remove(petal_id) else {
            return false;
        };
        let (valid, lapsed): (Vec<PetalMapping>, Vec<PetalMapping>) =
            claims.into_iter().partition(|c| {
                self.editor_at(
                    verse_id,
                    c.author.as_deref().unwrap_or_default(),
                    &c.fractal_id,
                    petal_id,
                )
            });
        for old in lapsed {
            let author = old.author.unwrap_or_default();
            self.quarantine_row(QuarantineKind::Petal, petal_id, &author, &old.row);
        }
        // OF-11b: a claim refused at the cap never entered `claims`, so the
        // partition above never judged it — fold its fractal back in so a
        // flood cannot settle the contest by keeping a refused fractal out
        // of the proof. OF-13b: the same lapse judgment the partition
        // applies, so a since-revoked refused author stops blocking.
        let mut refused = self
            .refused_contest_claims
            .remove(petal_id)
            .unwrap_or_default();
        refused.retain(|author, fractal| self.editor_at(verse_id, author, fractal, petal_id));
        let fractals: BTreeSet<&str> = valid
            .iter()
            .map(|c| c.fractal_id.as_str())
            .chain(refused.values().map(String::as_str))
            .collect();
        let winner = if fractals.len() <= 1 {
            valid.last()
        } else {
            valid.iter().rev().find(|c| {
                let author = c.author.as_deref().unwrap_or_default();
                fractals
                    .iter()
                    .all(|f| self.editor_at(verse_id, author, f, petal_id))
            })
        };
        match winner.cloned() {
            Some(winner) => {
                tracing::info!(verse_id, petal_id, fractal_id = %winner.fractal_id, "contested petal settled");
                // A settled claim is a fresh single-layer mapping, not a move.
                self.petal_fractals.insert(
                    petal_id.to_string(),
                    vec![PetalMapping {
                        from_fractal: None,
                        ..winner
                    }],
                );
                true
            }
            None if valid.is_empty() => true,
            None => {
                self.contested_petals.insert(petal_id.to_string(), valid);
                if !refused.is_empty() {
                    self.refused_contest_claims
                        .insert(petal_id.to_string(), refused);
                }
                false
            }
        }
    }

    /// The petal's current mapping is inbound-authored and its author no
    /// longer holds Editor+ at its scope or, for a move, at the scope it
    /// left (`from_fractal`, OF-13 — never read off the layer below).
    fn top_mapping_lapsed(&self, verse_id: &str, petal_id: &str) -> bool {
        let Some(top) = self.petal_fractals.get(petal_id).and_then(|h| h.last()) else {
            return false;
        };
        let Some(author) = top.author.as_deref() else {
            return false;
        };
        !(self.editor_at(verse_id, author, &top.fractal_id, petal_id)
            && top
                .from_fractal
                .as_deref()
                .is_none_or(|from| self.editor_at(verse_id, author, from, petal_id)))
    }

    /// OF-10b: re-verdict inbound-authored mappings against the current
    /// role view (an owner re-pin, a revocation, the demote cascade) — see
    /// [`Self::top_mapping_lapsed`]. A lapsed mapping is quarantined and the
    /// layer below restored (itself re-verdicted), or the petal is unmapped.
    /// `fallen` = `Some(dids)` re-checks only petals whose current mapping's
    /// author is in it (author-indexing; `None` = every petal) — sound
    /// because a lapse needs the author's role to fall. Contested petals are
    /// always re-judged (a rise can settle them).
    fn reverdict_petal_mappings(&mut self, verse_id: &str, fallen: Option<&HashSet<String>>) {
        let petal_ids: Vec<String> = self
            .petal_fractals
            .iter()
            .filter(|(_, history)| {
                fallen.is_none_or(|fallen| {
                    history
                        .last()
                        .and_then(|top| top.author.as_ref())
                        .is_some_and(|author| fallen.contains(author))
                })
            })
            .map(|(petal_id, _)| petal_id.clone())
            .collect();
        for petal_id in petal_ids {
            while self.top_mapping_lapsed(verse_id, &petal_id) {
                let Some(history) = self.petal_fractals.get_mut(&petal_id) else {
                    break;
                };
                let Some(dropped) = history.pop() else {
                    break;
                };
                if history.is_empty() {
                    self.petal_fractals.remove(&petal_id);
                }
                let author = dropped.author.unwrap_or_default();
                tracing::info!(
                    verse_id,
                    petal_id = %petal_id,
                    fractal_id = %dropped.fractal_id,
                    author = %author,
                    "petal mapping's author lost Editor+ — mapping withdrawn and quarantined"
                );
                self.quarantine_row(QuarantineKind::Petal, &petal_id, &author, &dropped.row);
            }
        }
        let contested: Vec<String> = self.contested_petals.keys().cloned().collect();
        for petal_id in contested {
            self.settle_contested_petal(verse_id, &petal_id);
        }
    }

    /// Build a petal's scope string from just its id via the learned
    /// petal->fractal map. `None` when the petal hasn't converged locally
    /// yet, or is contested — the caller quarantines (§2.5: "a row waiting
    /// only on the petal map comes back with the next open's replay").
    pub fn petal_scope(&self, verse_id: &str, petal_id: &str) -> Option<String> {
        let mapping = self.petal_fractals.get(petal_id)?.last()?;
        Some(fe_policy::build_scope(
            verse_id,
            Some(&mapping.fractal_id),
            Some(petal_id),
        ))
    }

    /// The verse manifest's `default_access` (§2.2's ⊥ fallback) — exposed
    /// for a caller that needs it directly when it has no scope string to
    /// resolve against yet (e.g. the compute requester gate when the
    /// petal map hasn't converged).
    pub fn default_access(&self) -> RoleLevel {
        self.default_access
    }

    /// `role_at` with the §2.2 ⊥ fallback already folded in — the one
    /// function every gate in this fabric calls, matching the DB plane's A3
    /// gate and local grant authority (same `fe_policy::role_grants`
    /// function underneath). `default_access` is capped at
    /// `{viewer, none}` (§2.2), so it can never satisfy a Manager+ floor —
    /// callers needing that floor use this unconditionally too.
    pub fn role_at(&self, peer_did: &str, scope: &str) -> RoleLevel {
        resolve_role(&self.effective, self.default_access, peer_did, scope)
    }

    /// Snapshot the role view at `petal_id`'s scope for the RP-6 merge
    /// filter (taken at plan time; see [`AnswererView`]).
    pub fn answerer_view(&self, verse_id: &str, petal_id: &str) -> AnswererView {
        AnswererView {
            petal_scope: self.petal_scope(verse_id, petal_id),
            owner_pinned: self.owner_did.is_some(),
            effective: self.effective.clone(),
            default_access: self.default_access,
        }
    }

    /// Admit one `__holders/{petal}/{anchor}/{bucket}/{did}` declaration
    /// (RP-6 §2.6). The trailing DID must be the entry's signed author (a
    /// row declaring ANOTHER peer a holder is dropped, as with `__peers`);
    /// the author needs Editor+ at the shard's petal scope (§2.5 — the same
    /// `role_at` gate as `__shards`). A row that cannot be judged yet is
    /// quarantined, never dropped: the replica holds it, and the §2.5
    /// re-read on empowerment recovers it.
    pub fn note_holder_row(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_id: &str,
        data: &[u8],
    ) {
        if data.is_empty() {
            self.withdraw_holder(verse_id, record_id, author_id);
            return;
        }
        let Some((_, holder_did)) = parse_holder_key(record_id) else {
            tracing::debug!(
                verse_id,
                record_id,
                "malformed holder declaration key — dropped"
            );
            return;
        };
        if holder_did != author_id {
            tracing::warn!(
                verse_id,
                record_id,
                author = author_id,
                "Forged holder declaration dropped — __holders rows must name their own author"
            );
            return;
        }
        self.admit_holder_entry(verse_id, record_id, author_id, data);
    }

    fn admit_holder_entry(
        &mut self,
        verse_id: &str,
        record_id: &str,
        author_id: &str,
        data: &[u8],
    ) {
        let Some((shard_key, holder_did)) = parse_holder_key(record_id) else {
            return;
        };
        let admitted = match self.petal_scope(verse_id, ShardId::petal_of_key(shard_key)) {
            Some(scope) => self
                .role_at(author_id, &scope)
                .is_at_least(RoleLevel::Editor),
            None => false,
        };
        if admitted {
            self.holders
                .entry(shard_key.to_string())
                .or_default()
                .insert(holder_did.to_string());
            return;
        }
        self.quarantine_row(QuarantineKind::Holder, record_id, author_id, data);
    }

    /// A `__holders` tombstone (OF-7a): the holder withdrew its own
    /// declaration — drop it from the admitted set AND from the quarantine
    /// (a held copy must not be re-admitted after the withdrawal). Only the
    /// holder itself may withdraw (the key's DID must be the signed author).
    fn withdraw_holder(&mut self, verse_id: &str, record_id: &str, author_id: &str) {
        let Some((shard_key, holder_did)) = parse_holder_key(record_id) else {
            return;
        };
        if holder_did != author_id {
            tracing::warn!(
                verse_id,
                record_id,
                author = author_id,
                "Forged holder withdrawal dropped — only the holder may tombstone its declaration"
            );
            return;
        }
        if let Some(dids) = self.holders.get_mut(shard_key) {
            dids.remove(holder_did);
            if dids.is_empty() {
                self.holders.remove(shard_key);
            }
        }
        self.quarantine.retain(|q| {
            !(q.kind == QuarantineKind::Holder
                && q.record_id == record_id
                && q.author_id == author_id)
        });
    }

    /// OF-7a: re-verdict every admitted holder against the current role
    /// view — a holder now below Editor at its shard's petal scope (a
    /// revocation, or the demote cascade) stops being an expected answerer.
    /// A later re-promotion restores it through the §2.5 re-read on
    /// empowerment, which replays the DID's `__holders/` entries.
    fn reverdict_holders(&mut self, verse_id: &str) {
        let mut demoted: Vec<(String, String)> = Vec::new();
        for (shard_key, dids) in &self.holders {
            let scope = self.petal_scope(verse_id, ShardId::petal_of_key(shard_key));
            for did in dids {
                let still_editor = scope
                    .as_deref()
                    .is_some_and(|scope| self.role_at(did, scope).is_at_least(RoleLevel::Editor));
                if !still_editor {
                    demoted.push((shard_key.clone(), did.clone()));
                }
            }
        }
        for (shard_key, did) in demoted {
            tracing::info!(verse_id, shard = %shard_key, holder = %did, "holder demoted below Editor — no longer an expected answerer");
            if let Some(dids) = self.holders.get_mut(&shard_key) {
                dids.remove(&did);
                if dids.is_empty() {
                    self.holders.remove(&shard_key);
                }
            }
        }
    }

    /// The admitted holders of one shard (RP-6 §2.6 — expected answerers
    /// beside the ledger hosts).
    pub fn shard_holders(&self, shard_key: &str) -> Option<&BTreeSet<String>> {
        self.holders.get(shard_key)
    }

    /// The shard keys the local peer has declared it holds (published or
    /// re-learned from our own snapshot rows after a restart).
    pub fn declared_holdings(&self) -> &BTreeSet<String> {
        &self.local_holdings
    }

    /// Record that the local peer's `__holders` declaration for this shard
    /// already exists in the replica (our own snapshot row replayed after a
    /// restart), so it is never published twice.
    pub fn note_local_holding(&mut self, shard_key: &str) {
        self.local_holdings.insert(shard_key.to_string());
    }

    /// Undo a local declaration whose publish failed, so the next reading
    /// into that shard declares it again.
    pub fn forget_local_holding(&mut self, shard_key: &str) {
        self.local_holdings.remove(shard_key);
    }

    /// The `__holders` row to publish for a local reading in `shard`, once:
    /// `Some((record_id, payload))` the first time the local peer writes a
    /// reading into a shard it does NOT host, `None` when it hosts the shard
    /// (or the shard is not in the ledger yet) or has already declared it.
    pub fn local_holding_row(
        &mut self,
        shard: &ShardId,
        local_did: &str,
    ) -> Option<(String, Vec<u8>)> {
        let key = shard.key();
        let hosts = self.shard_hosts(&key)?;
        if hosts.iter().any(|h| h == local_did) || self.local_holdings.contains(&key) {
            return None;
        }
        let payload = serde_json::to_vec(&serde_json::json!({
            "shard": key,
            "holder_did": local_did,
        }))
        .ok()?;
        self.local_holdings.insert(key.clone());
        Some((format!("{key}/{local_did}"), payload))
    }

    /// Admit one `role` statement (§2.1 key `role/{scope}/{peer}/{grantor}`
    /// — `record_id` here is `{scope}/{peer}/{grantor}`), from either
    /// direction: an inbound row or our own outbound write. The fabric
    /// retains ONLY admissible statements (§2.5) — a statement that fails
    /// `admissible` is discarded here, never stored as "pending": the
    /// replica already holds it durably, and the re-read on empowerment
    /// (`reread_queue` + `VerseReplicator::entries_by_author`) recovers it
    /// once its grantor's authority rises. A structurally invalid statement
    /// (bad key, grantor mismatch, invalid role, targets the owner,
    /// cross-verse scope) is a permanent deny (§2.2) — discarded and never
    /// retried.
    pub fn note_role_row(&mut self, verse_id: &str, record_id: &str, author_id: &str, data: &[u8]) {
        if data.is_empty() {
            // A `role` tombstone (`Doc::del`) carries no HLC — `NotApplicable`.
            return;
        }
        let key_parts = match fe_policy::StatementKey::parse(record_id) {
            Ok(k) => k,
            Err(reason) => {
                tracing::debug!(verse_id, record_id, %reason, "malformed role statement key — discarded");
                return;
            }
        };
        let payload: fe_policy::RoleStatementPayload = match serde_json::from_slice(data) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    verse_id,
                    record_id,
                    "malformed role statement payload — discarded: {e}"
                );
                return;
            }
        };
        let statement = match fe_policy::validate_statement(
            verse_id,
            self.owner_did.as_deref(),
            &key_parts,
            author_id,
            &payload,
        ) {
            Ok(s) => s,
            Err(reason) => {
                tracing::debug!(
                    verse_id, record_id, author = author_id, %reason,
                    "role statement failed validation — discarded"
                );
                return;
            }
        };
        if !self.effective.admissible(&statement) {
            tracing::debug!(
                verse_id,
                scope = %statement.scope,
                peer = %statement.peer_did,
                grantor = %statement.grantor_did,
                "role statement not yet admissible — held in the replica, awaiting the \
                 grantor's authority"
            );
            return;
        }
        let key = (
            statement.scope.clone(),
            statement.peer_did.clone(),
            statement.grantor_did.clone(),
        );
        let winner = match self.role_statements.remove(&key) {
            Some(existing) => fe_policy::latest_per_grantor([&existing, &statement])
                .into_iter()
                .next()
                .unwrap_or(statement),
            None => statement,
        };
        self.role_statements.insert(key, winner);
        self.rematerialize(verse_id);
    }

    /// Recompute `effective` from `role_statements` to a fixpoint,
    /// including the demote cascade (§2.2 "concurrent revocation
    /// converges"): a statement whose grantor lost authority under the new
    /// materialization is removed and the whole thing recomputed again,
    /// until a pass removes nothing. Risen DIDs are queued for the §2.5
    /// re-read on empowerment; the quarantine is re-evaluated against the
    /// new view either way (a petal-map-only wait isn't affected by a role
    /// change, but re-checking is cheap and always safe).
    fn rematerialize(&mut self, verse_id: &str) {
        let Some(owner) = self.owner_did.clone() else {
            self.effective = fe_policy::EffectiveRoles::default();
            self.reverdict_petal_mappings(verse_id, None);
            self.reverdict_holders(verse_id);
            self.reevaluate_quarantine(verse_id);
            return;
        };
        // Every DID that fell in ANY pass — a superset of the net fall,
        // which is all a mapping lapse can follow from.
        let mut fallen: HashSet<String> = HashSet::new();
        loop {
            let previous = std::mem::take(&mut self.effective);
            self.effective = fe_policy::materialize(&owner, self.role_statements.values());
            let changed = self.effective.diff(&previous);
            fallen.extend(changed.fallen);
            for did in changed.risen {
                if !self.reread_queue.contains(&did) {
                    self.reread_queue.push_back(did);
                }
            }
            let stale: Vec<(String, String, String)> = self
                .role_statements
                .iter()
                .filter(|(_, stmt)| !self.effective.admissible(stmt))
                .map(|(key, _)| key.clone())
                .collect();
            if stale.is_empty() {
                break;
            }
            for key in stale {
                self.role_statements.remove(&key);
            }
        }
        // Mappings before holders: a holder's scope comes from the map.
        self.reverdict_petal_mappings(verse_id, Some(&fallen));
        self.reverdict_holders(verse_id);
        self.reevaluate_quarantine(verse_id);
    }

    /// Drain the DIDs whose `role_at` rose since the last drain — the §2.5
    /// re-read on empowerment seam. The caller (the sync thread's command
    /// loop) replays each one's `role/__shards/__holders/verse` entries
    /// straight from the local replica through the inbound apply path.
    pub fn drain_reread_queue(&mut self) -> Vec<String> {
        self.reread_queue.drain(..).collect()
    }

    /// RP-5 closes `AGENTS.md` §transport-admission-control item 5: a
    /// `__shards` ledger row needs Editor+ at the shard's own petal scope.
    /// A row that cannot be judged yet (the author's role view has not
    /// converged, or the shard's petal is not known locally) is
    /// quarantined, never dropped — the replica holds it, and the §2.5
    /// re-read on empowerment or the petal map catching up recovers it.
    fn admit_shard_entry(&mut self, verse_id: &str, record_id: &str, author_id: &str, data: &[u8]) {
        let petal_id = ShardId::petal_of_key(record_id);
        let admitted = match self.petal_scope(verse_id, petal_id) {
            Some(scope) => self
                .role_at(author_id, &scope)
                .is_at_least(RoleLevel::Editor),
            None => false,
        };
        if admitted {
            if let Ok(row) = serde_json::from_slice::<serde_json::Value>(data) {
                self.note_shard_row(&row);
            }
            return;
        }
        self.quarantine_row(QuarantineKind::Shard, record_id, author_id, data);
    }

    fn quarantine_row(
        &mut self,
        kind: QuarantineKind,
        record_id: &str,
        author_id: &str,
        data: &[u8],
    ) {
        if self.quarantine.len() >= QUARANTINE_CAP {
            self.quarantine.pop_front();
            self.quarantine_evicted += 1;
        }
        self.quarantine.push_back(QuarantinedRow {
            kind,
            record_id: record_id.to_string(),
            author_id: author_id.to_string(),
            data: data.to_vec(),
        });
    }

    /// Re-attempt every quarantined row against the current role/petal
    /// view. A row that still fails is re-quarantined (order may churn —
    /// no ordering guarantee is needed here).
    fn reevaluate_quarantine(&mut self, verse_id: &str) {
        let pending: Vec<QuarantinedRow> = self.quarantine.drain(..).collect();
        for row in pending {
            match row.kind {
                QuarantineKind::Shard => {
                    self.admit_shard_entry(verse_id, &row.record_id, &row.author_id, &row.data);
                }
                QuarantineKind::Holder => {
                    self.admit_holder_entry(verse_id, &row.record_id, &row.author_id, &row.data);
                }
                QuarantineKind::Petal => {
                    if let Ok(petal_row) = serde_json::from_slice::<serde_json::Value>(&row.data) {
                        self.admit_petal_entry(
                            verse_id,
                            &petal_row,
                            &row.author_id,
                            RowOrigin::Inbound,
                        );
                    }
                }
            }
        }
    }

    /// How many rows are currently quarantined (diagnostics/tests).
    pub fn quarantine_len(&self) -> usize {
        self.quarantine.len()
    }

    /// How many quarantined rows have been evicted under the cap
    /// (diagnostics/tests) — §2.5's "accelerator, not a guarantee": an
    /// evicted row still comes back through the re-read.
    pub fn quarantine_evicted(&self) -> u64 {
        self.quarantine_evicted
    }

    /// How many contested-petal claims have been refused at
    /// `PETAL_CLAIMS_CAP` (diagnostics/tests, OF-11b) — a refusal never
    /// evicts an existing claim, so settlement can't be proven over a
    /// shrunk set of fractals.
    pub fn petal_claim_refusals(&self) -> u64 {
        self.petal_claim_refusals
    }

    /// A snapshot of who currently holds Viewer+ AT THE VERSE SCOPE, for
    /// the blob-ALPN request filter (`docs_engine::BlobAccessGate`, RP-5
    /// step 6 / AC-21(e)) — which runs on a background task outside the
    /// sync thread's own state and so cannot borrow the fabric directly.
    /// Tileset roots are bound to a verse, not a petal, so a petal- or
    /// fractal-scoped grant never opens them (OF-6e): `role_at` is resolved
    /// at the verse scope for every statement's peer.
    pub fn access_snapshot(&self, verse_id: &str) -> crate::docs_engine::VerseAccessSnapshot {
        let verse_scope = fe_policy::build_scope(verse_id, None, None);
        let mut viewer_dids = HashSet::new();
        for stmt in self.role_statements.values() {
            if self
                .role_at(&stmt.peer_did, &verse_scope)
                .is_at_least(RoleLevel::Viewer)
            {
                viewer_dids.insert(stmt.peer_did.clone());
            }
        }
        crate::docs_engine::VerseAccessSnapshot {
            owner_did: self.owner_did.clone(),
            open_to_all: self.default_access.is_at_least(RoleLevel::Viewer),
            viewer_dids,
        }
    }

    /// Record an inbound `__shards` ledger row — ungated bookkeeping only
    /// (the admission gate is `admit_shard_entry`; this stays the plain
    /// parse-and-insert step so a matured quarantine replay and a direct
    /// outbound publish share one code path).
    pub fn note_shard_row(&mut self, row: &serde_json::Value) {
        let Some(entry) = ShardLedgerEntry::from_row(row) else {
            tracing::debug!("malformed shard ledger row — ignored");
            return;
        };
        self.shards.insert(entry.shard.clone(), entry);
    }

    /// Bump the local estimates for a reading seen for a known shard.
    pub fn note_reading_seen(&mut self, shard_key: &str, size_bytes: u64) {
        if let Some(entry) = self.shards.get_mut(shard_key) {
            entry.row_count += 1;
            entry.size_bytes = entry.size_bytes.saturating_add(size_bytes);
        }
    }

    /// The current per-host assigned totals (the planner's load input).
    pub fn assigned_bytes(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for entry in self.shards.values() {
            for host in &entry.hosts {
                *out.entry(host.clone()).or_insert(0) += entry.size_bytes;
            }
        }
        out
    }

    /// Ensure the shard exists in the ledger. When it is new, plan its host
    /// set (pure placement over the fabric's current view) and return the
    /// ledger row to publish to the doc; when it is known, just bump the
    /// local estimates and return `None` (the host set never re-plans in
    /// F6 — membership churn re-planning is deferred).
    pub fn ensure_shard(
        &mut self,
        shard: &ShardId,
        first_row_bytes: usize,
        local_did: &str,
    ) -> Option<Vec<u8>> {
        let key = shard.key();
        if let Some(entry) = self.shards.get_mut(&key) {
            entry.row_count += 1;
            entry.size_bytes = entry.size_bytes.saturating_add(first_row_bytes as u64);
            return None;
        }
        // A fabric with no declared peers at all (local declaration raced the
        // first ingest) plans for the local peer alone rather than leaving
        // the shard homeless.
        if self.peers.is_empty() {
            self.note_local_declaration(local_did, PeerDeclaration::default());
        }
        let (start, end) = ShardId::bucket_range(shard.bucket, self.settings.bucket_width_ms);
        let PlacementPlan { hosts, overflowed } = plan_shard_hosts(
            &key,
            first_row_bytes as u64,
            &self.peers,
            &self.assigned_bytes(),
            &self.settings,
        );
        if overflowed {
            tracing::warn!(
                shard = %key,
                hosts = ?hosts,
                "Shard placement exceeded every peer's declared capacity — placed on the least-utilized host (a shard must live somewhere)"
            );
        }
        let entry = ShardLedgerEntry {
            shard: key,
            hosts,
            mode: self.settings.mode,
            replication_factor: self.settings.replication_factor,
            bucket_width_ms: self.settings.bucket_width_ms,
            range_start_ms: start,
            range_end_ms: end,
            row_count: 1,
            size_bytes: first_row_bytes as u64,
        };
        tracing::debug!(
            shard = %entry.shard,
            hosts = ?entry.hosts,
            mode = entry.mode.as_str(),
            "Shard first seen — planned placement, publishing ledger row"
        );
        self.shards.insert(entry.shard.clone(), entry.clone());
        serde_json::to_vec(&entry).ok()
    }

    /// The host set for a shard, when its ledger row has converged locally.
    pub fn shard_hosts(&self, shard_key: &str) -> Option<&[String]> {
        self.shards.get(shard_key).map(|e| e.hosts.as_slice())
    }

    /// The `__peers/{local_did}` doc payload for the local peer's declaration
    /// (published on replica open and on every capacity change).
    pub fn local_peer_row(&self, local_did: &str) -> Option<Vec<u8>> {
        let decl = self.peers.get(local_did)?;
        serde_json::to_vec(decl).ok()
    }

    /// Route the publish of a row for this shard per mode (A13).
    pub fn transfer_route_for(&self, hosts: &[String]) -> TransferRoute {
        transfer_route(&self.settings, hosts)
    }

    /// Whether the local peer retains a reading for this shard (A13,
    /// receive side). Unknown shards retain — see `retention_decision`.
    pub fn retention_for(&self, shard_key: &str, local_did: &str) -> Retention {
        retention_decision(&self.settings, self.shard_hosts(shard_key), local_did)
    }

    /// Feed one inbound (or snapshot) row change into the fabric. Runs
    /// BEFORE the own-author filter: our own snapshot rows are exactly how
    /// the fabric re-learns settings/peers/ledger/role-view after a
    /// restart.
    ///
    /// **Admission control:**
    /// - `__peers` declarations are SELF-declaration only (F23): the row's
    ///   key must name its own author (`author_id`, the doc entry's signed
    ///   author, never the row bytes). A row declaring ANOTHER peer's
    ///   capacity is dropped with a warn.
    /// - `role` statements go through `note_role_row` (§2.5): only
    ///   admissible statements are retained.
    /// - `__shards` ledger rows go through `admit_shard_entry` (RP-5): a
    ///   row whose author is not Editor+ at the shard's petal scope is
    ///   quarantined, not dropped.
    /// - `__holders` declarations go through `note_holder_row` (RP-6): the
    ///   key's DID must be the author, and the author Editor+ at the
    ///   shard's petal scope (quarantined otherwise).
    pub fn note_entry(&mut self, verse_id: &str, change: &crate::replicator::RowChange) {
        match change.table.as_str() {
            "verse" => {
                if !change.data.is_empty() {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                        self.note_verse_row(verse_id, &row, &change.author_id);
                    }
                }
            }
            "role" => {
                self.note_role_row(verse_id, &change.record_id, &change.author_id, &change.data);
            }
            "petal" => {
                if !change.data.is_empty() {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                        self.note_petal_row(verse_id, &row, &change.author_id);
                    }
                }
            }
            PEER_DECL_TABLE => {
                if change.record_id != change.author_id {
                    tracing::warn!(
                        record_id = %change.record_id,
                        author = %change.author_id,
                        "Forged peer declaration dropped — __peers rows must be self-declared \
                         (the key must name the author's own DID)"
                    );
                    return;
                }
                if !change.data.is_empty() {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                        self.note_peer_declaration(
                            &change.record_id,
                            PeerDeclaration::from_row(&row),
                        );
                    }
                }
            }
            SHARD_TABLE if !change.data.is_empty() => {
                self.admit_shard_entry(
                    verse_id,
                    &change.record_id,
                    &change.author_id,
                    &change.data,
                );
            }
            HOLDERS_TABLE => {
                self.note_holder_row(verse_id, &change.record_id, &change.author_id, &change.data);
            }
            _ => {}
        }
    }

    /// Whether a known owner and Petal mapping make distributed planning meaningful.
    pub fn query_catalog_ready(&self, verse_id: &str, petal_id: &str) -> bool {
        self.owner_did.is_some() && self.petal_scope(verse_id, petal_id).is_some()
    }

    /// Serialize the fabric for `SyncCommand::GetShardLedger` (diagnostics,
    /// harness assertions — the fabric is otherwise sync-thread-internal).
    pub fn to_dump_json(&self) -> serde_json::Value {
        serde_json::json!({
            "settings": self.settings,
            "peers": self.peers,
            "shards": self.shards,
            "holders": self.holders,
            "owner_did": self.owner_did,
            "default_access": self.default_access.to_string(),
            "quarantine_len": self.quarantine.len(),
            "quarantine_evicted": self.quarantine_evicted,
        })
    }

    // --- test-only accessors (kept out of the public API surface where a
    // method already exists for the production need) ---

    #[cfg(test)]
    fn owner_did(&self) -> Option<&str> {
        self.owner_did.as_deref()
    }

    #[cfg(test)]
    fn role_statement_count(&self) -> usize {
        self.role_statements.len()
    }

    #[cfg(test)]
    fn refused_contest_claim_count(&self, petal_id: &str) -> usize {
        self.refused_contest_claims
            .get(petal_id)
            .map_or(0, BTreeMap::len)
    }

    /// Test helper (crate-wide — other modules' pre-RP-5 fixtures build a
    /// `VerseFabric` by struct literal rather than through a manifest row
    /// and don't care about role gating): open this verse to anyone
    /// without going through `note_verse_row`.
    #[cfg(test)]
    pub fn set_default_access_for_test(&mut self, access: RoleLevel) {
        self.default_access = access;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::timeseries::{TimeseriesMode, DEFAULT_BUCKET_WIDTH_MS};

    const OWNER: &str = "did:key:owner";

    fn reading_row(petal: &str, anchor: &str, recorded_at_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "reading_id": "r1",
            "node_id": anchor,
            "petal_id": petal,
            "metric": "temperature_c",
            "value": 21.5,
            "recorded_at": "2026-07-15T10:00:00Z",
            "recorded_at_ms": recorded_at_ms,
        })
    }

    /// A `role/{scope}/{peer}/{grantor}` doc row change.
    fn role_change(
        scope: &str,
        peer: &str,
        grantor: &str,
        role: &str,
        revoked: bool,
        hlc: u64,
    ) -> crate::replicator::RowChange {
        let record_id = format!("{scope}/{peer}/{grantor}");
        let payload = serde_json::json!({
            "scope": scope, "peer_did": peer, "grantor_did": grantor,
            "role": role, "revoked": revoked, "hlc": hlc,
        });
        crate::replicator::RowChange {
            table: "role".into(),
            record_id,
            content_hash: [0; 32],
            author_id: grantor.to_string(),
            timestamp: hlc,
            is_tombstone: false,
            data: serde_json::to_vec(&payload).unwrap(),
        }
    }

    fn verse_change(
        verse_id: &str,
        created_by: &str,
        author: &str,
        extra: serde_json::Value,
    ) -> crate::replicator::RowChange {
        let mut row = serde_json::json!({ "verse_id": verse_id, "created_by": created_by });
        if let serde_json::Value::Object(extra_map) = extra {
            if let serde_json::Value::Object(row_map) = &mut row {
                row_map.extend(extra_map);
            }
        }
        crate::replicator::RowChange {
            table: "verse".into(),
            record_id: verse_id.to_string(),
            content_hash: [0; 32],
            author_id: author.to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&row).unwrap(),
        }
    }

    fn petal_change(petal_id: &str, fractal_id: &str) -> crate::replicator::RowChange {
        crate::replicator::RowChange {
            table: "petal".into(),
            record_id: petal_id.to_string(),
            content_hash: [0; 32],
            author_id: OWNER.to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({
                "petal_id": petal_id, "fractal_id": fractal_id,
            }))
            .unwrap(),
        }
    }

    fn shard_change(shard_key: &str, author: &str, hosts: &[&str]) -> crate::replicator::RowChange {
        let payload = serde_json::json!({
            "shard": shard_key, "hosts": hosts,
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": DEFAULT_BUCKET_WIDTH_MS,
            "range_start_ms": 0, "range_end_ms": DEFAULT_BUCKET_WIDTH_MS as i64,
            "row_count": 1, "size_bytes": 10,
        });
        crate::replicator::RowChange {
            table: SHARD_TABLE.into(),
            record_id: shard_key.to_string(),
            content_hash: [0; 32],
            author_id: author.to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&payload).unwrap(),
        }
    }

    fn fabric_with_owner(verse_id: &str) -> VerseFabric {
        let mut fabric = VerseFabric::default();
        fabric.note_entry(
            verse_id,
            &verse_change(verse_id, OWNER, OWNER, serde_json::json!({})),
        );
        assert_eq!(fabric.owner_did(), Some(OWNER));
        fabric
    }

    #[test]
    fn buckets_align_to_epoch_width_boundaries() {
        assert_eq!(ShardId::bucket_index(0, 1_000), 0);
        assert_eq!(ShardId::bucket_index(999, 1_000), 0);
        assert_eq!(ShardId::bucket_index(1_000, 1_000), 1);
        assert_eq!(
            ShardId::bucket_index(-1, 1_000),
            -1,
            "pre-epoch floors down"
        );
        let (start, end) = ShardId::bucket_range(1, 1_000);
        assert_eq!((start, end), (1_000, 2_000));
    }

    #[test]
    fn a_reading_maps_to_exactly_one_shard_by_anchor_and_bucket() {
        let row = reading_row("p1", "a1", 1_752_580_800_000);
        let shard = ShardId::of_reading_row(&row, DEFAULT_BUCKET_WIDTH_MS).expect("shard");
        assert_eq!(shard.key(), "p1/a1/20284");
        // Same day, same shard.
        let same_day = ShardId::of_reading_row(
            &reading_row("p1", "a1", 1_752_600_000_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_eq!(same_day.key(), shard.key());
        // Different anchor or day, different shard.
        let other_anchor = ShardId::of_reading_row(
            &reading_row("p1", "a2", 1_752_580_800_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_ne!(other_anchor.key(), shard.key());
        let other_day = ShardId::of_reading_row(
            &reading_row("p1", "a1", 1_752_667_200_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_ne!(other_day.key(), shard.key());
    }

    #[test]
    fn missing_fields_do_not_shard() {
        assert!(ShardId::of_reading_row(&serde_json::json!({"reading_id": "r"}), 1_000).is_none());
    }

    #[test]
    fn peer_declaration_round_trips_through_the_doc_row() {
        let decl = PeerDeclaration {
            capacity_bytes: Some(5000),
            seeder: true,
        };
        let row = serde_json::to_value(decl).expect("serialize");
        assert_eq!(PeerDeclaration::from_row(&row), decl);
        // Zero/absent capacity reads back as unlimited.
        let none = serde_json::json!({"capacity_bytes": 0, "seeder": false});
        assert_eq!(PeerDeclaration::from_row(&none), PeerDeclaration::default());
    }

    #[test]
    fn verse_manifest_updates_fabric_settings() {
        let mut fabric = fabric_with_owner("v1");
        assert_eq!(fabric.settings.mode, TimeseriesMode::Mirror);
        let changed = fabric.note_verse_row(
            "v1",
            &serde_json::json!({
                "created_by": OWNER, "ts_mode": "balanced", "ts_replication_factor": 2,
                "ts_bucket_width_ms": 3_600_000
            }),
            OWNER,
        );
        assert!(changed);
        assert_eq!(fabric.settings.mode, TimeseriesMode::Balanced);
        assert_eq!(fabric.settings.replication_factor, 2);
        // Same row again: no change reported.
        assert!(!fabric.note_verse_row(
            "v1",
            &serde_json::json!({
                "created_by": OWNER, "ts_mode": "balanced", "ts_replication_factor": 2,
                "ts_bucket_width_ms": 3_600_000
            }),
            OWNER,
        ));
    }

    #[test]
    fn ensure_shard_plans_publishes_once_then_estimates() {
        let mut fabric = VerseFabric::default();
        fabric.note_verse_row(
            "v1",
            &serde_json::json!({
                "created_by": OWNER, "ts_mode": "balanced", "ts_replication_factor": 2,
                "ts_bucket_width_ms": DEFAULT_BUCKET_WIDTH_MS
            }),
            OWNER,
        );
        fabric.note_local_declaration("local", PeerDeclaration::default());
        fabric.note_peer_declaration("peer-b", PeerDeclaration::default());

        let shard = ShardId {
            petal_id: "p1".into(),
            anchor_node_id: "a1".into(),
            bucket: 1,
        };
        let row = fabric
            .ensure_shard(&shard, 200, "local")
            .expect("ledger row");
        let entry: ShardLedgerEntry = serde_json::from_slice(&row).expect("ledger json");
        assert_eq!(entry.hosts.len(), 2, "R=2 over two peers");
        assert_eq!(entry.row_count, 1);
        assert_eq!(entry.size_bytes, 200);

        // Second sight of the same shard: no re-publish, estimates grow.
        assert!(fabric.ensure_shard(&shard, 100, "local").is_none());
        let entry = fabric.shards.get(&shard.key()).expect("entry");
        assert_eq!(entry.row_count, 2);
        assert_eq!(entry.size_bytes, 300);
    }

    #[test]
    fn inbound_shard_and_peer_rows_feed_the_fabric() {
        let mut fabric = VerseFabric::default();
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a1/1", "hosts": ["local", "peer-b"],
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 1000,
            "range_start_ms": 1000, "range_end_ms": 2000, "row_count": 3, "size_bytes": 900
        }));
        assert_eq!(
            fabric.shard_hosts("p1/a1/1"),
            Some(&["local".to_string(), "peer-b".to_string()][..])
        );

        fabric.note_peer_declaration(
            "peer-b",
            PeerDeclaration {
                capacity_bytes: Some(1),
                seeder: true,
            },
        );
        assert_eq!(fabric.peers["peer-b"].capacity_bytes, Some(1));

        // assigned_bytes folds over the ledger.
        let assigned = fabric.assigned_bytes();
        assert_eq!(assigned.get("local"), Some(&900));
        assert_eq!(assigned.get("peer-b"), Some(&900));
    }

    #[test]
    fn retention_follows_mode_and_host_set() {
        let mut fabric = fabric_with_owner("v1");
        // Mirror default: unknown shard retains.
        assert_eq!(fabric.retention_for("p/a/1", "local"), Retention::Retain);
        fabric.note_verse_row(
            "v1",
            &serde_json::json!({"created_by": OWNER, "ts_mode": "sharded"}),
            OWNER,
        );
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p/a/1", "hosts": ["peer-b"], "row_count": 1, "size_bytes": 1
        }));
        assert_eq!(fabric.retention_for("p/a/1", "local"), Retention::Skip);
        assert_eq!(fabric.retention_for("p/a/1", "peer-b"), Retention::Retain);
        // Unknown shard under sharded mode: retain (the safe default).
        assert_eq!(fabric.retention_for("p/a/2", "local"), Retention::Retain);
    }

    #[test]
    fn transfer_route_reflects_the_mode() {
        let mut fabric = fabric_with_owner("v1");
        assert_eq!(
            fabric.transfer_route_for(&["a".to_string()]),
            TransferRoute::Broadcast
        );
        fabric.note_verse_row(
            "v1",
            &serde_json::json!({"created_by": OWNER, "ts_mode": "balanced"}),
            OWNER,
        );
        assert_eq!(
            fabric.transfer_route_for(&["a".to_string(), "b".to_string()]),
            TransferRoute::Targeted(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn ledger_row_serde_round_trips() {
        let entry = ShardLedgerEntry {
            shard: "p/a/1".into(),
            hosts: vec!["did:key:z6MkA".into()],
            mode: TimeseriesMode::Balanced,
            replication_factor: 2,
            bucket_width_ms: 3_600_000,
            range_start_ms: 3_600_000,
            range_end_ms: 7_200_000,
            row_count: 7,
            size_bytes: 1_400,
        };
        let json = serde_json::to_vec(&entry).expect("serialize");
        let back: ShardLedgerEntry = serde_json::from_slice(&json).expect("deserialize");
        assert_eq!(back, entry);
    }

    // ── F23: ledger admission control (__peers is self-declaration only) ──

    fn peer_decl_change(record_id: &str, author_id: &str) -> crate::replicator::RowChange {
        crate::replicator::RowChange {
            table: PEER_DECL_TABLE.into(),
            record_id: record_id.into(),
            content_hash: [0; 32],
            author_id: author_id.into(),
            timestamp: 0,
            is_tombstone: false,
            data: br#"{"capacity_bytes":1,"seeder":false}"#.to_vec(),
        }
    }

    #[test]
    fn forged_peer_declaration_is_dropped() {
        // The M2 scrutiny F6 major shape: a member publishes
        // `__peers/{honest_did}` (starving the honest peer's placement)
        // under its OWN author identity — the record key does not name the
        // author, so the declaration is refused.
        let mut fabric = VerseFabric::default();
        fabric.note_entry(
            "v1",
            &peer_decl_change("did:key:honest", "did:key:attacker"),
        );
        assert!(
            !fabric.peers.contains_key("did:key:honest"),
            "a forged foreign-DID declaration must never enter the fabric"
        );
        assert!(
            !fabric.peers.contains_key("did:key:attacker"),
            "the declaration must not be re-keyed to the attacker either"
        );
    }

    #[test]
    fn honest_self_declaration_converges() {
        let mut fabric = VerseFabric::default();
        fabric.note_entry("v1", &peer_decl_change("did:key:bob", "did:key:bob"));
        let decl = fabric
            .peers
            .get("did:key:bob")
            .expect("self-declaration converges into the fabric");
        assert_eq!(decl.capacity_bytes, Some(1));
        assert!(!decl.seeder);
        // A later honest re-declaration still updates in place.
        fabric.note_entry(
            "v1",
            &crate::replicator::RowChange {
                table: PEER_DECL_TABLE.into(),
                record_id: "did:key:bob".into(),
                content_hash: [0; 32],
                author_id: "did:key:bob".into(),
                timestamp: 1,
                is_tombstone: false,
                data: br#"{"capacity_bytes":900,"seeder":true}"#.to_vec(),
            },
        );
        let decl = fabric.peers.get("did:key:bob").expect("still declared");
        assert_eq!(decl.capacity_bytes, Some(900));
        assert!(decl.seeder);
    }

    // ── RP-5: sync-plane role view ──────────────────────────────────────

    #[test]
    fn note_entry_routed_by_table() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry(
            "v1",
            &verse_change(
                "v1",
                OWNER,
                OWNER,
                serde_json::json!({"ts_mode": "sharded"}),
            ),
        );
        assert_eq!(fabric.settings.mode, TimeseriesMode::Sharded);

        fabric.note_entry(
            "v1",
            &crate::replicator::RowChange {
                table: "iot_reading".into(),
                record_id: "r".into(),
                content_hash: [0; 32],
                author_id: "did:key:peer".into(),
                timestamp: 0,
                is_tombstone: false,
                data: vec![],
            },
        );
        // Readings do not touch the ledger through note_entry (retention and
        // estimate bookkeeping happen on the apply path, where the shard is
        // known).
        assert!(fabric.shards.is_empty());
    }

    #[test]
    fn manifest_settings_are_adopted_only_from_a_verse_manager_or_the_owner() {
        struct Case {
            grantor: &'static str,
            role: &'static str,
            scope: fn(&str) -> String,
            expect_adopted: bool,
        }
        let verse = "v1";
        let cases = [
            Case {
                grantor: "did:key:viewer",
                role: "viewer",
                scope: |v| fe_policy::build_scope(v, None, None),
                expect_adopted: false,
            },
            Case {
                grantor: "did:key:petal-mgr",
                role: "manager",
                scope: |v| fe_policy::build_scope(v, Some("f1"), Some("p1")),
                expect_adopted: false,
            },
            Case {
                grantor: "did:key:verse-mgr",
                role: "manager",
                scope: |v| fe_policy::build_scope(v, None, None),
                expect_adopted: true,
            },
        ];
        for case in cases {
            let mut fabric = fabric_with_owner(verse);
            if (case.scope)(verse) != fe_policy::build_scope(verse, None, None) {
                // A petal-scoped grantor needs the petal->fractal map AND a
                // Manager grant confined to that subtree — but a PETAL
                // Manager has no authority at the VERSE scope (R7), so the
                // grant itself can only be a petal-scope statement from the
                // owner to establish them there first.
                fabric.note_entry("v1", &petal_change("p1", "f1"));
                fabric.note_entry(
                    "v1",
                    &role_change(
                        &(case.scope)(verse),
                        case.grantor,
                        OWNER,
                        case.role,
                        false,
                        1,
                    ),
                );
            } else {
                fabric.note_entry(
                    "v1",
                    &role_change(
                        &(case.scope)(verse),
                        case.grantor,
                        OWNER,
                        case.role,
                        false,
                        1,
                    ),
                );
            }
            let before = fabric.settings.mode;
            fabric.note_entry(
                "v1",
                &verse_change(
                    verse,
                    OWNER,
                    case.grantor,
                    serde_json::json!({"ts_mode": "sharded"}),
                ),
            );
            if case.expect_adopted {
                assert_eq!(
                    fabric.settings.mode,
                    TimeseriesMode::Sharded,
                    "grantor {} should have had its manifest edit adopted",
                    case.grantor
                );
            } else {
                assert_eq!(
                    fabric.settings.mode, before,
                    "grantor {} must not be able to change verse settings",
                    case.grantor
                );
            }
        }
    }

    #[test]
    fn a_petal_scoped_editors_shards_row_is_admitted_for_that_petal_only() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        fabric.note_entry("v1", &petal_change("p2", "f1"));
        let editor = "did:key:editor";
        fabric.note_entry(
            "v1",
            &role_change(
                &fe_policy::build_scope("v1", Some("f1"), Some("p1")),
                editor,
                OWNER,
                "editor",
                false,
                1,
            ),
        );

        // Admitted for p1 (the Editor's own petal).
        fabric.note_entry("v1", &shard_change("p1/a1/1", editor, &["did:key:host"]));
        assert!(fabric.shards.contains_key("p1/a1/1"));

        // Dropped (quarantined) for the sibling petal p2.
        fabric.note_entry("v1", &shard_change("p2/a1/1", editor, &["did:key:host"]));
        assert!(!fabric.shards.contains_key("p2/a1/1"));
        assert_eq!(fabric.quarantine_len(), 1);
    }

    #[test]
    fn a_viewer_authored_shards_row_is_quarantined_and_admitted_once_the_owner_grants_editor() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let viewer = "did:key:viewer";

        // No role yet — the row cannot be judged, so it is quarantined.
        fabric.note_entry("v1", &shard_change("p1/a1/1", viewer, &["did:key:host"]));
        assert!(!fabric.shards.contains_key("p1/a1/1"));
        assert_eq!(fabric.quarantine_len(), 1);

        // The owner grants Editor at the petal scope — rematerialize
        // re-evaluates the quarantine and admits the held row.
        fabric.note_entry(
            "v1",
            &role_change(
                &fe_policy::build_scope("v1", Some("f1"), Some("p1")),
                viewer,
                OWNER,
                "editor",
                false,
                1,
            ),
        );
        assert!(fabric.shards.contains_key("p1/a1/1"));
        assert_eq!(fabric.quarantine_len(), 0);
    }

    #[test]
    fn a_forged_shards_row_from_a_stranger_never_reaches_the_ledger() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let stranger = "did:key:stranger";
        fabric.note_entry("v1", &shard_change("p1/a1/1", stranger, &[stranger]));
        assert!(
            !fabric.shards.contains_key("p1/a1/1"),
            "a stranger's ledger row must never reach the ledger"
        );
        // It stays quarantined (not silently discarded) — it would come
        // back through the re-read if the stranger were ever empowered.
        assert_eq!(fabric.quarantine_len(), 1);
    }

    #[test]
    fn a_quarantine_flood_cannot_lose_a_legit_editors_shards_row() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let editor = "did:key:editor";

        // The legit row arrives before the grant — quarantined first.
        fabric.note_entry("v1", &shard_change("p1/a1/1", editor, &[editor]));
        assert_eq!(fabric.quarantine_len(), 1);

        // Flood past the cap with strangers' rows on OTHER shards — the
        // legit row is evicted (drop-oldest).
        for i in 0..QUARANTINE_CAP + 10 {
            let shard_key = format!("p1/a{i}/1");
            fabric.note_entry(
                "v1",
                &shard_change(&shard_key, "did:key:flooder", &["did:key:flooder"]),
            );
        }
        assert_eq!(fabric.quarantine_len(), QUARANTINE_CAP);
        assert!(fabric.quarantine_evicted() > 0);
        assert!(!fabric.shards.contains_key("p1/a1/1"));

        // The owner grants Editor — this fabric-level re-materialization
        // cannot recover an already-evicted row by itself (it only
        // re-walks what is STILL quarantined); the sync thread's re-read on
        // empowerment (fed from `drain_reread_queue`) is what pulls it back
        // from the replica. Confirm the queue carries the editor's DID so
        // that mechanism has what it needs.
        fabric.note_entry(
            "v1",
            &role_change(
                &fe_policy::build_scope("v1", Some("f1"), Some("p1")),
                editor,
                OWNER,
                "editor",
                false,
                1,
            ),
        );
        assert!(
            fabric.drain_reread_queue().contains(&editor.to_string()),
            "the newly-empowered editor must be queued for the re-read"
        );
    }

    #[test]
    fn a_held_role_statement_is_reread_candidate_once_its_grantor_is_empowered() {
        // R7: M's statement arrives before the owner's grant and is not
        // retained (M has no authority yet). Once the owner grants M
        // Manager, M is queued for the re-read — the fabric itself cannot
        // recover M's EARLIER statement (it was never stored), which is
        // exactly why the sync thread's re-read pulls it back from the
        // replica rather than relying on in-memory state (§2.5).
        let mut fabric = fabric_with_owner("v1");
        let m = "did:key:m";
        let x = "did:key:x";
        let verse_scope = fe_policy::build_scope("v1", None, None);

        fabric.note_entry("v1", &role_change(&verse_scope, x, m, "editor", false, 1));
        assert_eq!(
            fabric.role_statement_count(),
            0,
            "M's statement must not be retained before M has any authority"
        );

        fabric.note_entry(
            "v1",
            &role_change(&verse_scope, m, OWNER, "manager", false, 2),
        );
        assert!(fabric.drain_reread_queue().contains(&m.to_string()));
    }

    #[test]
    fn fabric_and_db_materialize_identical_effective_roles_from_the_same_statements() {
        // Cross-plane parity: the fabric's incrementally-admitted statement
        // set, materialized a second time directly through
        // `fe_policy::materialize`, must equal what the fabric itself
        // computed as it admitted each statement one at a time. A Manager
        // `viewer` statement below the peer's already-higher inherited role
        // is included to pin "Managers only raise" (fe-policy's own
        // guarantee) through the fabric's path too.
        let mut fabric = fabric_with_owner("v1");
        let verse_scope = fe_policy::build_scope("v1", None, None);
        let mgr = "did:key:mgr";
        let editor = "did:key:editor";

        fabric.note_entry(
            "v1",
            &role_change(&verse_scope, mgr, OWNER, "manager", false, 1),
        );
        fabric.note_entry(
            "v1",
            &role_change(&verse_scope, editor, OWNER, "editor", false, 2),
        );
        // mgr tries to demote editor below their owner-granted level — a
        // Manager-sourced statement can only raise, never lower.
        fabric.note_entry(
            "v1",
            &role_change(&verse_scope, editor, mgr, "viewer", false, 3),
        );

        let direct = fe_policy::materialize(OWNER, fabric.role_statements.values());
        assert_eq!(
            fabric.role_at(editor, &verse_scope),
            direct.role_at(editor, &verse_scope)
        );
        assert_eq!(
            direct.role_at(editor, &verse_scope),
            RoleLevel::Editor,
            "a Manager's lower statement must not demote an owner-granted Editor"
        );
        assert_eq!(fabric.effective, direct);
    }

    // ── RP-6: holder declarations ───────────────────────────────────────

    fn holder_change(shard_key: &str, holder: &str, author: &str) -> crate::replicator::RowChange {
        crate::replicator::RowChange {
            table: HOLDERS_TABLE.into(),
            record_id: format!("{shard_key}/{holder}"),
            content_hash: [0; 32],
            author_id: author.to_string(),
            timestamp: 0,
            is_tombstone: false,
            data: serde_json::to_vec(&serde_json::json!({
                "shard": shard_key, "holder_did": holder,
            }))
            .unwrap(),
        }
    }

    #[test]
    fn a_viewer_cannot_declare_itself_a_holder() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let petal_scope = fe_policy::build_scope("v1", Some("f1"), Some("p1"));
        let viewer = "did:key:viewer";
        let editor = "did:key:editor";
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, viewer, OWNER, "viewer", false, 1),
        );
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, editor, OWNER, "editor", false, 2),
        );

        fabric.note_entry("v1", &holder_change("p1/a1/1", viewer, viewer));
        assert!(
            fabric
                .shard_holders("p1/a1/1")
                .is_none_or(|h| !h.contains(viewer)),
            "a Viewer's self-declaration must never make it an expected answerer"
        );
        assert_eq!(fabric.quarantine_len(), 1, "held, not discarded");

        fabric.note_entry("v1", &holder_change("p1/a1/1", editor, editor));
        assert!(fabric
            .shard_holders("p1/a1/1")
            .is_some_and(|h| h.contains(editor)));

        // Declaring SOMEONE ELSE a holder is a forgery, dropped outright.
        fabric.note_entry("v1", &holder_change("p1/a2/1", editor, viewer));
        assert!(fabric.shard_holders("p1/a2/1").is_none());
        assert_eq!(
            fabric.quarantine_len(),
            1,
            "a forged row is not even quarantined"
        );

        // Once the owner raises the Viewer to Editor, the held row matures.
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, viewer, OWNER, "editor", false, 3),
        );
        assert!(fabric
            .shard_holders("p1/a1/1")
            .is_some_and(|h| h.contains(viewer)));
    }

    #[test]
    fn local_holding_row_is_issued_once_and_only_for_an_unhosted_known_shard() {
        let mut fabric = fabric_with_owner("v1");
        let shard = ShardId {
            petal_id: "p1".into(),
            anchor_node_id: "a1".into(),
            bucket: 1,
        };
        assert!(
            fabric.local_holding_row(&shard, "did:key:local").is_none(),
            "unknown shard"
        );
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a1/1", "hosts": ["did:key:host"], "row_count": 1, "size_bytes": 1
        }));
        let (record_id, _payload) = fabric
            .local_holding_row(&shard, "did:key:local")
            .expect("first reading into an unhosted shard declares a holding");
        assert_eq!(record_id, "p1/a1/1/did:key:local");
        assert_eq!(
            parse_holder_key(&record_id),
            Some(("p1/a1/1", "did:key:local"))
        );
        assert!(
            fabric.local_holding_row(&shard, "did:key:local").is_none(),
            "once per shard"
        );
        assert!(
            fabric.local_holding_row(&shard, "did:key:host").is_none(),
            "a host never declares"
        );
    }

    #[test]
    fn answerer_view_rejects_only_when_converged() {
        let mut fabric = fabric_with_owner("v1");
        let petal_scope = fe_policy::build_scope("v1", Some("f1"), Some("p1"));
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, "did:key:editor", OWNER, "editor", false, 1),
        );
        // Petal unknown: nothing to judge against.
        assert_eq!(
            fabric.answerer_view("v1", "p1").verdict("did:key:viewer"),
            AnswererVerdict::Unverified
        );
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let view = fabric.answerer_view("v1", "p1");
        assert_eq!(view.verdict("did:key:viewer"), AnswererVerdict::Rejected);
        assert_eq!(view.verdict("did:key:editor"), AnswererVerdict::Admitted);
        assert_eq!(view.verdict(OWNER), AnswererVerdict::Admitted);
    }

    /// OF-6d: a manifest whose signed author is NOT its `created_by` pins no
    /// owner and adopts no settings (pre-fix, a stranger's first manifest
    /// naming themselves creator-by-proxy took the verse's settings); the
    /// claimed creator is queued for the re-read, and the creator's own
    /// self-signed manifest then pins them.
    #[test]
    fn a_manifest_not_signed_by_its_creator_pins_no_owner_and_adopts_no_settings() {
        let mut fabric = VerseFabric::default();
        let changed = fabric.note_verse_row(
            "v1",
            &serde_json::json!({
                "verse_id": "v1", "created_by": "did:key:attacker",
                "ts_mode": "sharded", "default_access": "viewer",
            }),
            "did:key:relay",
        );
        assert!(
            !changed,
            "an unsigned-by-creator manifest must adopt nothing"
        );
        assert_eq!(fabric.owner_did(), None);
        assert_eq!(fabric.default_access(), RoleLevel::None);
        assert_ne!(fabric.settings.mode, TimeseriesMode::Sharded);
        assert_eq!(
            fabric.drain_reread_queue(),
            vec!["did:key:attacker".to_string()]
        );

        fabric.note_entry(
            "v1",
            &verse_change(
                "v1",
                OWNER,
                OWNER,
                serde_json::json!({"ts_mode": "sharded"}),
            ),
        );
        assert_eq!(fabric.owner_did(), Some(OWNER));
        assert_eq!(fabric.settings.mode, TimeseriesMode::Sharded);

        // Our own locally-admitted outbound manifest (an invite-pinned owner
        // written under the joiner's identity) still pins and adopts.
        let mut joiner = VerseFabric::default();
        assert!(joiner.note_local_verse_row(
            "v1",
            &serde_json::json!({"created_by": OWNER, "ts_mode": "sharded"}),
            "did:key:joiner",
        ));
        assert_eq!(joiner.owner_did(), Some(OWNER));
    }

    /// OF-6e: a PETAL-scoped Viewer never lands in the verse-wide blob
    /// access snapshot (pre-fix, `role_at` at the statement's own scope put
    /// them there, opening every verse-bound tileset); a verse Viewer does.
    #[test]
    fn a_petal_scoped_viewer_is_not_a_verse_wide_blob_viewer() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let petal_scope = fe_policy::build_scope("v1", Some("f1"), Some("p1"));
        let verse_scope = fe_policy::build_scope("v1", None, None);
        fabric.note_entry(
            "v1",
            &role_change(
                &petal_scope,
                "did:key:petal-viewer",
                OWNER,
                "viewer",
                false,
                1,
            ),
        );
        fabric.note_entry(
            "v1",
            &role_change(
                &verse_scope,
                "did:key:verse-viewer",
                OWNER,
                "viewer",
                false,
                2,
            ),
        );
        let snapshot = fabric.access_snapshot("v1");
        assert!(!snapshot.viewer_dids.contains("did:key:petal-viewer"));
        assert!(snapshot.viewer_dids.contains("did:key:verse-viewer"));
    }

    fn petal_change_by(
        petal_id: &str,
        fractal_id: &str,
        author: &str,
    ) -> crate::replicator::RowChange {
        let mut change = petal_change(petal_id, fractal_id);
        change.author_id = author.to_string();
        change
    }

    /// OF-9 (HIGH): the petal->fractal map is gated like the DB plane's
    /// `confine_petal`. A Manager of ANOTHER fractal can neither re-parent
    /// a victim petal under their fractal (which would make their grant
    /// enclose it — here: admit their `__shards` row for it) nor plant a
    /// first mapping without authority there; a move by someone with
    /// authority over BOTH scopes is learned; a held row matures once its
    /// author is empowered.
    #[test]
    fn a_petal_row_without_authority_over_its_scopes_never_bends_the_scope_chain() {
        let mut fabric = fabric_with_owner("v1");
        let attacker = "did:key:attacker";
        let attacker_fractal = fe_policy::build_scope("v1", Some("fa"), None);
        fabric.note_entry(
            "v1",
            &role_change(&attacker_fractal, attacker, OWNER, "manager", false, 1),
        );
        fabric.note_entry("v1", &petal_change("p1", "fv"));
        let victim_scope = fe_policy::build_scope("v1", Some("fv"), Some("p1"));
        assert_eq!(
            fabric.petal_scope("v1", "p1").as_deref(),
            Some(victim_scope.as_str())
        );

        // Re-parent attempt: Editor+ at the target scope, none at the current one.
        fabric.note_entry("v1", &petal_change_by("p1", "fa", attacker));
        assert_eq!(
            fabric.petal_scope("v1", "p1").as_deref(),
            Some(victim_scope.as_str()),
            "a petal row from a Manager of another fractal re-parented the victim petal"
        );
        fabric.note_entry("v1", &shard_change("p1/a1/1", attacker, &[attacker]));
        assert!(
            !fabric.shards.contains_key("p1/a1/1"),
            "the attacker's grant must not enclose p1"
        );

        // A first mapping planted by a stranger is held, not learned.
        fabric.note_entry("v1", &petal_change_by("p9", "fv", "did:key:stranger"));
        assert_eq!(fabric.petal_scope("v1", "p9"), None);
        // A scope-smuggling id is dropped outright.
        fabric.note_entry("v1", &petal_change("p8", "fv-PETAL#p1"));
        assert_eq!(fabric.petal_scope("v1", "p8"), None);

        // The owner (authority over both scopes) moves p1: learned.
        fabric.note_entry("v1", &petal_change("p1", "fw"));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            Some(fe_policy::build_scope("v1", Some("fw"), Some("p1")))
        );

        // The stranger is granted Editor on fv: their held p9 row matures.
        let fv = fe_policy::build_scope("v1", Some("fv"), None);
        fabric.note_entry(
            "v1",
            &role_change(&fv, "did:key:stranger", OWNER, "editor", false, 2),
        );
        assert_eq!(
            fabric.petal_scope("v1", "p9"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p9")))
        );
    }

    /// R1 (owner integrity, the OF-6d residual): a replicated SELF-SIGNED
    /// manifest naming its author creator pins first (snapshot replay runs
    /// before this node's pending writes flush); this node's own DB-admitted
    /// manifest then corrects the pin, drops the false owner's settings, and
    /// the demote cascade drops every statement the false owner made.
    #[test]
    fn this_nodes_own_manifest_corrects_an_owner_pinned_by_a_forged_self_signed_manifest() {
        let mut fabric = VerseFabric::default();
        let attacker = "did:key:attacker";
        let verse_scope = fe_policy::build_scope("v1", None, None);
        fabric.note_entry(
            "v1",
            &verse_change(
                "v1",
                attacker,
                attacker,
                serde_json::json!({ "default_access": "viewer" }),
            ),
        );
        fabric.note_entry(
            "v1",
            &role_change(&verse_scope, "did:key:crony", attacker, "manager", false, 1),
        );
        assert_eq!(fabric.owner_did(), Some(attacker));
        assert!(fabric
            .role_at("did:key:crony", &verse_scope)
            .is_at_least(RoleLevel::Manager));

        fabric.note_local_verse_row(
            "v1",
            &serde_json::json!({ "verse_id": "v1", "created_by": OWNER, "default_access": "none" }),
            "did:key:joiner",
        );
        assert_eq!(fabric.owner_did(), Some(OWNER));
        assert_eq!(
            fabric.default_access(),
            RoleLevel::None,
            "the false owner's open access must not survive"
        );
        assert_eq!(fabric.role_at(attacker, &verse_scope), RoleLevel::None);
        assert_eq!(
            fabric.role_at("did:key:crony", &verse_scope),
            RoleLevel::None
        );
        assert!(!fabric.access_snapshot("v1").open_to_all);
    }

    /// OF-7a: an admitted holder whose Editor role is REVOKED stops being an
    /// expected answerer, and a holder's own `__holders` tombstone withdraws
    /// it (a forged tombstone from someone else does not).
    #[test]
    fn a_revoked_or_withdrawn_holder_is_no_longer_an_expected_answerer() {
        let mut fabric = fabric_with_owner("v1");
        fabric.note_entry("v1", &petal_change("p1", "f1"));
        let petal_scope = fe_policy::build_scope("v1", Some("f1"), Some("p1"));
        let (erin, frank) = ("did:key:erin", "did:key:frank");
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, erin, OWNER, "editor", false, 1),
        );
        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, frank, OWNER, "editor", false, 2),
        );
        fabric.note_entry("v1", &holder_change("p1/a1/1", erin, erin));
        fabric.note_entry("v1", &holder_change("p1/a1/1", frank, frank));
        assert!(fabric
            .shard_holders("p1/a1/1")
            .is_some_and(|h| h.contains(erin) && h.contains(frank)));

        fabric.note_entry(
            "v1",
            &role_change(&petal_scope, erin, OWNER, "editor", true, 3),
        );
        assert!(
            fabric
                .shard_holders("p1/a1/1")
                .is_some_and(|h| !h.contains(erin)),
            "a revoked holder must not stay an expected answerer"
        );

        let mut forged = holder_change("p1/a1/1", frank, erin);
        forged.data.clear();
        forged.is_tombstone = true;
        fabric.note_entry("v1", &forged);
        assert!(
            fabric
                .shard_holders("p1/a1/1")
                .is_some_and(|h| h.contains(frank)),
            "forged withdrawal"
        );

        let mut withdrawal = holder_change("p1/a1/1", frank, frank);
        withdrawal.data.clear();
        withdrawal.is_tombstone = true;
        fabric.note_entry("v1", &withdrawal);
        assert!(
            fabric.shard_holders("p1/a1/1").is_none(),
            "a withdrawn holder must be dropped"
        );
    }

    /// OF-10a: a first-arriving mapping from a Manager of ANOTHER fractal
    /// (the squat) cannot hold a petal against its real home — the genuine
    /// row contests it (unmapped, so the squatter's grant encloses nothing),
    /// and the contest settles to the claim whose author keeps authority.
    #[test]
    fn a_first_arriving_squat_cannot_hold_a_petal_against_its_real_home() {
        let mut fabric = fabric_with_owner("v1");
        let (mallory, alice) = ("did:key:mallory", "did:key:alice");
        let fa = fe_policy::build_scope("v1", Some("fa"), None);
        let fv = fe_policy::build_scope("v1", Some("fv"), None);
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", false, 1));
        fabric.note_entry("v1", &role_change(&fv, alice, OWNER, "editor", false, 2));

        fabric.note_entry("v1", &petal_change_by("p1", "fa", mallory));
        fabric.note_entry("v1", &petal_change_by("p1", "fv", alice));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "the first writer's mapping must not outrank the petal's genuine row"
        );
        fabric.note_entry("v1", &shard_change("p1/a1/1", mallory, &[mallory]));
        assert!(
            !fabric.shards.contains_key("p1/a1/1"),
            "a contested petal must not be enclosed by the squatter's grant"
        );

        // The squatter's grant is revoked: their claim lapses, the genuine one settles.
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", true, 3));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p1")))
        );

        // An author with authority over BOTH claimed scopes settles a contest outright.
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", false, 4));
        fabric.note_entry("v1", &petal_change_by("p2", "fa", mallory));
        fabric.note_entry("v1", &petal_change_by("p2", "fv", alice));
        assert_eq!(fabric.petal_scope("v1", "p2"), None);
        fabric.note_entry("v1", &petal_change("p2", "fv"));
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p2")))
        );
    }

    /// OF-10b: a mapping admitted under an OLD role view does not outlive
    /// it. A false owner's mapping is withdrawn when this node's own
    /// manifest corrects the pin; a move whose author is later demoted rolls
    /// back to the mapping it replaced.
    #[test]
    fn a_petal_mapping_admitted_under_a_stale_role_view_is_withdrawn_on_re_pin_or_demotion() {
        let mut fabric = VerseFabric::default();
        let attacker = "did:key:attacker";
        fabric.note_entry(
            "v1",
            &verse_change("v1", attacker, attacker, serde_json::json!({})),
        );
        fabric.note_entry("v1", &petal_change_by("p1", "fa", attacker));
        assert!(fabric.petal_scope("v1", "p1").is_some());

        fabric.note_local_verse_row(
            "v1",
            &serde_json::json!({ "verse_id": "v1", "created_by": OWNER }),
            "did:key:joiner",
        );
        assert_eq!(fabric.owner_did(), Some(OWNER));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "the false owner's mapping survived the owner re-pin"
        );

        let (alice, carol) = ("did:key:alice", "did:key:carol");
        let fv = fe_policy::build_scope("v1", Some("fv"), None);
        let fw = fe_policy::build_scope("v1", Some("fw"), None);
        fabric.note_entry("v1", &role_change(&fv, alice, OWNER, "editor", false, 1));
        fabric.note_entry("v1", &role_change(&fv, carol, OWNER, "editor", false, 2));
        fabric.note_entry("v1", &role_change(&fw, carol, OWNER, "editor", false, 3));
        fabric.note_entry("v1", &petal_change_by("p2", "fv", alice));
        fabric.note_entry("v1", &petal_change_by("p2", "fw", carol));
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            Some(fe_policy::build_scope("v1", Some("fw"), Some("p2")))
        );

        fabric.note_entry("v1", &role_change(&fw, carol, OWNER, "editor", true, 4));
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p2"))),
            "a demoted mover's re-parent must roll back to the mapping it replaced"
        );
    }

    /// OF-11a: a stale own-replay — this node's historical `petal` entry,
    /// replayed from the doc after a restart (`reassert_own_rows` fires
    /// these automatically right after every snapshot) — must not take the
    /// unconditional locally-authoritative path once the petal is
    /// CONTESTED. A contested petal has no `current` mapping, so the old
    /// `current.is_some()` gate let a stale replay silently settle the
    /// contest. An unauthorized replay is quarantined (the contest stays
    /// unmapped); an authorized one enters the ordinary claim path and
    /// only settles the contest once arbitration actually allows it.
    #[test]
    fn a_stale_own_replay_cannot_settle_a_contested_petal() {
        let mut fabric = fabric_with_owner("v1");
        let (mallory, alice, local) = ("did:key:mallory", "did:key:alice", "did:key:local");
        let fa = fe_policy::build_scope("v1", Some("fa"), None);
        let fv = fe_policy::build_scope("v1", Some("fv"), None);
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", false, 1));
        fabric.note_entry("v1", &role_change(&fv, alice, OWNER, "editor", false, 2));
        fabric.note_entry("v1", &petal_change_by("p1", "fa", mallory));
        fabric.note_entry("v1", &petal_change_by("p1", "fv", alice));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "setup: the petal must be contested before the replay arrives"
        );

        // An unauthorized stale own-replay (this node has no authority at
        // all over the fractal it once claimed) must not silently settle
        // the contest via the unconditional locally-authoritative path.
        fabric.note_own_replayed_petal_row(
            "v1",
            &serde_json::json!({ "petal_id": "p1", "fractal_id": "f_old" }),
            local,
        );
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "a stale own replay with no authority auto-settled a contested petal"
        );

        // An AUTHORIZED own-replay enters the ordinary claim path — one
        // more claim, never the unconditional insert — so it alone cannot
        // settle a contest still held by a second, undefeated fractal.
        fabric.note_entry("v1", &role_change(&fv, local, OWNER, "editor", false, 3));
        fabric.note_own_replayed_petal_row(
            "v1",
            &serde_json::json!({ "petal_id": "p1", "fractal_id": "fv" }),
            local,
        );
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "one more claim on the genuine side alone must not settle a still-contested petal"
        );

        // Mallory's claim lapses: the genuine side — now asserted by BOTH
        // alice and the replayed own claim — settles through arbitration,
        // never through an override.
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", true, 4));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p1"))),
            "the authorized own-replay's claim must settle through arbitration, not override"
        );
    }

    /// OF-11b: claim-cap FLOODING must not let a contest settle without
    /// proving authority over every claimed fractal. Part 1 (p1): once the
    /// claim list is at its cap, the overflow claim is REFUSED — never
    /// evicting an existing (here: the OLDEST, genuine) claim, which would
    /// let the flood's own fractal fast-path to a win once the genuine
    /// claim vanished from the proof. Part 2 (p2): a genuine claim that
    /// itself arrives AFTER the cap is already full is refused outright —
    /// its fractal must still be required even though it never entered
    /// `contested_petals` at all, or the surviving claims legitimately
    /// collapsing to one shared fractal would fast-path to a win that
    /// never had to face the refused claim.
    #[test]
    fn a_claim_flood_cannot_evict_a_contested_fractal_from_the_settlement_proof() {
        let mut fabric = fabric_with_owner("v1");
        let (alice, mallory) = ("did:key:alice", "did:key:mallory");
        let fv = fe_policy::build_scope("v1", Some("fv"), None);
        let fa = fe_policy::build_scope("v1", Some("fa"), None);
        fabric.note_entry("v1", &role_change(&fv, alice, OWNER, "editor", false, 1));
        fabric.note_entry("v1", &role_change(&fa, mallory, OWNER, "manager", false, 2));
        let shills: Vec<String> = (2..=8).map(|i| format!("did:key:shill{i}")).collect();
        for (i, shill) in shills.iter().enumerate() {
            fabric.note_entry(
                "v1",
                &role_change(&fa, shill, OWNER, "manager", false, 10 + i as u64),
            );
        }

        // Part 1 (p1): alice's genuine claim is the OLDEST in the contest;
        // mallory + 6 shills flood the SAME fractal (fa) to exactly fill
        // the cap, then one more (shills[6]) overflows it.
        fabric.note_entry("v1", &petal_change_by("p1", "fv", alice));
        fabric.note_entry("v1", &petal_change_by("p1", "fa", mallory));
        for shill in &shills[..6] {
            fabric.note_entry("v1", &petal_change_by("p1", "fa", shill));
        }
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "setup: the flood fills exactly the cap, still contested"
        );
        fabric.note_entry("v1", &petal_change_by("p1", "fa", &shills[6]));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            None,
            "a flooded-past-cap settlement must not resolve to the flood's fractal while \
             alice's older, genuine claim is excluded from the proof"
        );
        assert!(
            fabric.petal_claim_refusals() >= 1,
            "the overflow claim must be refused, not admitted by evicting alice's claim"
        );

        // Part 2 (p2): shill_x seeds the contest on a distinct fractal
        // (fb); the flood on fa fills the cap; alice's GENUINE claim
        // arrives last and is itself refused. Revoking shill_x then
        // collapses the surviving claims to one shared fractal (fa) —
        // settlement must stay blocked because alice's refused claim is
        // still remembered.
        let shill_x = "did:key:shill_x";
        let fb = fe_policy::build_scope("v1", Some("fb"), None);
        fabric.note_entry(
            "v1",
            &role_change(&fb, shill_x, OWNER, "manager", false, 20),
        );
        fabric.note_entry("v1", &petal_change_by("p2", "fa", mallory));
        fabric.note_entry("v1", &petal_change_by("p2", "fb", shill_x));
        for shill in &shills[..6] {
            fabric.note_entry("v1", &petal_change_by("p2", "fa", shill));
        }
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            None,
            "setup: p2 at its cap, still contested"
        );
        // Every quarantine re-judgment along the way (each role grant
        // above re-ran it) legitimately re-refuses the still-capped p1
        // overflow claim too, so the counter is not a fixed tally — assert
        // it strictly increases, not an exact value.
        let refusals_before_alice = fabric.petal_claim_refusals();
        fabric.note_entry("v1", &petal_change_by("p2", "fv", alice));
        assert!(
            fabric.petal_claim_refusals() > refusals_before_alice,
            "alice's late genuine claim on p2 must also be refused at the cap"
        );

        fabric.note_entry("v1", &role_change(&fb, shill_x, OWNER, "manager", true, 21));
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            None,
            "the surviving claims collapsing to one shared fractal (fa) must not fast-path to \
             a win while alice's refused fv claim is still unanswered"
        );
    }

    /// Grant `did` the given role at each fractal scope of `v1`, one
    /// statement per fractal (OWNER-signed, fresh HLCs from `hlc`).
    fn grant_at_fractals(
        fabric: &mut VerseFabric,
        did: &str,
        role: &str,
        fractals: &[&str],
        hlc: &mut u64,
    ) {
        for fractal in fractals {
            *hlc += 1;
            let scope = fe_policy::build_scope("v1", Some(fractal), None);
            fabric.note_entry("v1", &role_change(&scope, did, OWNER, role, false, *hlc));
        }
    }

    /// Revoke `did`'s grant at one fractal scope of `v1`.
    fn revoke_at_fractal(
        fabric: &mut VerseFabric,
        did: &str,
        role: &str,
        fractal: &str,
        hlc: &mut u64,
    ) {
        *hlc += 1;
        let scope = fe_policy::build_scope("v1", Some(fractal), None);
        fabric.note_entry("v1", &role_change(&scope, did, OWNER, role, true, *hlc));
    }

    fn p1_under(fractal: &str) -> Option<String> {
        Some(fe_policy::build_scope("v1", Some(fractal), Some("p1")))
    }

    /// OF-13a: the cap must never evict the incumbent a move just replaced,
    /// and a layer's lapse check reads the scope it left from the layer
    /// itself, never from whatever sits below it on the stack. History
    /// `[fX local, fY a, fZ b, fX c]` + d's move to fW: the pre-fix scan
    /// evicted c (its fX duplicated by the local base), so d's layer sat on
    /// b's fZ — d (no authority at fZ) was rolled back on the next role
    /// change, and d's demotion restored fZ instead of the fX it replaced.
    #[test]
    fn lapse_rollback_restores_replaced_mapping_not_older_layer() {
        let mut fabric = fabric_with_owner("v1");
        let (a, b, c, d) = ("did:key:a", "did:key:b", "did:key:c", "did:key:d");
        let mut hlc = 0;
        grant_at_fractals(&mut fabric, a, "editor", &["fX", "fY"], &mut hlc);
        grant_at_fractals(&mut fabric, b, "editor", &["fY", "fZ"], &mut hlc);
        grant_at_fractals(&mut fabric, c, "editor", &["fZ", "fX"], &mut hlc);
        grant_at_fractals(&mut fabric, d, "editor", &["fX", "fW", "fQ"], &mut hlc);
        fabric.note_local_petal_row(
            "v1",
            &serde_json::json!({ "petal_id": "p1", "fractal_id": "fX" }),
        );
        for (mover, target) in [(a, "fY"), (b, "fZ"), (c, "fX"), (d, "fW")] {
            fabric.note_entry("v1", &petal_change_by("p1", target, mover));
        }
        assert_eq!(fabric.petal_scope("v1", "p1"), p1_under("fW"));

        // d falls at an unrelated scope: its move (fX -> fW) is still fully
        // authorized and must stand.
        revoke_at_fractal(&mut fabric, d, "editor", "fQ", &mut hlc);
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("fW"),
            "an authorized move was judged against an older layer's scope, not the one it left"
        );

        // d's demotion restores the incumbent it replaced (c's fX).
        revoke_at_fractal(&mut fabric, d, "editor", "fW", &mut hlc);
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("fX"),
            "rollback restored an older layer instead of the mapping the move replaced"
        );

        // c's demotion exposes b's layer, which now sits directly on the
        // local base (a's layer was the one evicted): b moved fY -> fZ and
        // holds both, so it stands — its check never reads the base's fX.
        revoke_at_fractal(&mut fabric, c, "editor", "fX", &mut hlc);
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("fZ"),
            "a layer's lapse check read the scope below it instead of the scope it left"
        );

        // b's demotion falls through to the locally-authoritative base.
        revoke_at_fractal(&mut fabric, b, "editor", "fZ", &mut hlc);
        assert_eq!(fabric.petal_scope("v1", "p1"), p1_under("fX"));
    }

    /// OF-13b: a claim refused at the contest cap stops blocking settlement
    /// once its author is revoked (lapse-pruned at settle, like an admitted
    /// claim), the refused set holds one entry per author, and a local
    /// write that ends a contest clears it. Pre-fix the refused fractals
    /// were never pruned, so p1 stayed unmapped until a local write.
    #[test]
    fn revoked_author_refused_fractal_stops_blocking_settlement() {
        let mut fabric = fabric_with_owner("v1");
        let (alice, mallory, late) = ("did:key:alice", "did:key:mallory", "did:key:late");
        let shills: Vec<String> = (1..=6).map(|i| format!("did:key:shill{i}")).collect();
        let mut hlc = 0;
        grant_at_fractals(&mut fabric, alice, "editor", &["fv"], &mut hlc);
        grant_at_fractals(&mut fabric, mallory, "manager", &["fa"], &mut hlc);
        for shill in &shills {
            grant_at_fractals(&mut fabric, shill, "manager", &["fa"], &mut hlc);
        }
        grant_at_fractals(&mut fabric, late, "manager", &["fb", "fc"], &mut hlc);

        // alice (fv) vs mallory + 6 shills (fa) = PETAL_CLAIMS_CAP claims;
        // `late` arrives past the cap and is refused.
        let fill_contest_to_cap = |fabric: &mut VerseFabric, petal: &str| {
            fabric.note_entry("v1", &petal_change_by(petal, "fv", alice));
            fabric.note_entry("v1", &petal_change_by(petal, "fa", mallory));
            for shill in &shills {
                fabric.note_entry("v1", &petal_change_by(petal, "fa", shill));
            }
            assert_eq!(
                fabric.petal_scope("v1", petal),
                None,
                "setup: {petal} contested"
            );
        };

        // p2: a local write ends the contest and clears its refused claims.
        fill_contest_to_cap(&mut fabric, "p2");
        fabric.note_entry("v1", &petal_change_by("p2", "fb", late));
        assert_eq!(fabric.refused_contest_claim_count("p2"), 1);
        fabric.note_local_petal_row(
            "v1",
            &serde_json::json!({ "petal_id": "p2", "fractal_id": "fv" }),
        );
        assert_eq!(
            fabric.petal_scope("v1", "p2"),
            Some(fe_policy::build_scope("v1", Some("fv"), Some("p2")))
        );
        assert_eq!(
            fabric.refused_contest_claim_count("p2"),
            0,
            "the locally-authoritative branch ended the contest but kept its refused claims"
        );

        // p1: `late` is refused twice (fb, then fc) — one entry per author.
        fill_contest_to_cap(&mut fabric, "p1");
        fabric.note_entry("v1", &petal_change_by("p1", "fb", late));
        fabric.note_entry("v1", &petal_change_by("p1", "fc", late));
        assert_eq!(
            fabric.refused_contest_claim_count("p1"),
            1,
            "an author's superseded refused claim must not grow the set"
        );

        // `late` is revoked everywhere, then every fa claimant: alice's fv
        // claim is the only authorized claim left and must settle.
        revoke_at_fractal(&mut fabric, late, "manager", "fb", &mut hlc);
        revoke_at_fractal(&mut fabric, late, "manager", "fc", &mut hlc);
        revoke_at_fractal(&mut fabric, mallory, "manager", "fa", &mut hlc);
        for shill in &shills {
            revoke_at_fractal(&mut fabric, shill, "manager", "fa", &mut hlc);
        }
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("fv"),
            "a revoked author's refused claim kept blocking settlement"
        );
        assert_eq!(fabric.refused_contest_claim_count("p1"), 0);
    }

    /// OF-13c: a move by an author with authority over both scopes is
    /// ALWAYS applied, even when every history layer was protected under
    /// the R6b rule (the local base, each other layer the sole record of
    /// its fractal) — pre-fix it was refused with only a warn. The cap
    /// evicts the oldest inbound layer instead; rollback still walks back
    /// layer by layer, and the locally-authoritative base survives.
    #[test]
    fn authorized_move_applies_when_all_history_layers_protected() {
        let mut fabric = fabric_with_owner("v1");
        let movers = ["did:key:m1", "did:key:m2", "did:key:m3", "did:key:m4"];
        let fractals = ["f0", "f1", "f2", "f3", "f4"];
        let mut hlc = 0;
        for (i, mover) in movers.iter().enumerate() {
            grant_at_fractals(
                &mut fabric,
                mover,
                "manager",
                &fractals[i..=i + 1],
                &mut hlc,
            );
        }
        fabric.note_local_petal_row(
            "v1",
            &serde_json::json!({ "petal_id": "p1", "fractal_id": "f0" }),
        );
        // Three moves fill the 4-deep stack: [f0 local, f1 m1, f2 m2, f3 m3].
        for (i, mover) in movers[..3].iter().enumerate() {
            fabric.note_entry("v1", &petal_change_by("p1", fractals[i + 1], mover));
        }
        assert_eq!(fabric.petal_scope("v1", "p1"), p1_under("f3"));

        fabric.note_entry("v1", &petal_change_by("p1", "f4", movers[3]));
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("f4"),
            "an authorized move was refused because every history layer was protected"
        );

        // Rollback walks back through the surviving layers: m1's f1 layer
        // was the one evicted, so m2's demotion lands on the local base.
        revoke_at_fractal(&mut fabric, movers[3], "manager", "f4", &mut hlc);
        assert_eq!(fabric.petal_scope("v1", "p1"), p1_under("f3"));
        revoke_at_fractal(&mut fabric, movers[2], "manager", "f3", &mut hlc);
        assert_eq!(fabric.petal_scope("v1", "p1"), p1_under("f2"));
        revoke_at_fractal(&mut fabric, movers[1], "manager", "f2", &mut hlc);
        assert_eq!(
            fabric.petal_scope("v1", "p1"),
            p1_under("f0"),
            "the locally-authoritative base layer must survive the cap"
        );
    }
}
