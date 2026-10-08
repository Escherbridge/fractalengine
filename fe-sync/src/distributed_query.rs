//! Distributed timeseries query transport (M2/F7 — A15/A16/A17).
//!
//! `SubmitComputeTask` becomes a real fan-out transport: a structured
//! query spec is planned against the verse's fabric (shard ledger →
//! per-host partials), broadcast over the verse's gossip topic, answered by
//! every peer that holds data (its partial is executed on its DB thread via
//! `DbCommand::ExecuteTsPartial`), and merged commutatively with honest
//! covered/missing shard metadata. See `fe-sync/src/AGENTS.md`
//! §distributed-query for the full contract; the shared vocabulary types
//! live in `fe_runtime::distributed_query` (one definition, all crates).
//!
//! Purity rules (D2, the sim lab depends on them): the planner and the
//! merge are pure functions of their inputs — no I/O, no clocks, no
//! iteration-order dependence. Only the collector/responder tasks (which
//! drive the transport) are async.
//!
//! **Admission control (F23, 2026-10-08):** inbound compute envelopes are
//! gated at `handle_gossip_incoming` — verse-vs-arrival-topic, direct
//! sender identity (`from_did == did_key(from)` on direct deliveries), and
//! declared-fabric-peer membership for requesters. See `AGENTS.md`
//! §distributed-query for the honest boundary, including the relayed
//! delivery residual.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use fe_runtime::distributed_query::{
    clamp_partial_row_cap, DistributedQueryCall, DistributedQueryMeta, DistributedQueryOutcome,
    PartialShard, TsPartialRows, TsQueryKind, PARTIAL_ROW_CAP,
};
use fe_runtime::messages::DbCommand;
use iroh_gossip::net::GossipSender;

use crate::messages::SyncEvent;
use crate::sharding::VerseFabric;

/// Gossip max message size for the whole network (must be identical on every
/// peer — all run this code). Sized for a realistic partial envelope; see
/// `docs_engine.rs` where it is applied.
pub const GOSSIP_MAX_MESSAGE_SIZE: usize = 512 * 1024;

/// Serialization budget for one envelope — headroom below
/// [`GOSSIP_MAX_MESSAGE_SIZE`] so the broadcast never rejects at the wire.
pub const GOSSIP_ENVELOPE_BUDGET: usize = 448 * 1024;

/// Maximum target shards one query may fan out to (planner cap: the request
/// envelope carries the shard list; a wider window is an honest error, never
/// a silently-wrong partial plan).
pub const MAX_TARGET_SHARDS: usize = 256;

/// Concurrent in-flight distributed queries per sync thread (bounded
/// concurrency — a slow fleet must not pile up unbounded collector tasks).
pub const MAX_CONCURRENT_QUERIES: usize = 8;

/// In-flight request registrations cap (backstop behind the semaphore).
pub const PENDING_QUERY_CAP: usize = 64;

/// Default per-host answer deadline.
pub const DEFAULT_QUERY_TIMEOUT_MS: u64 = 3_000;

/// Upper bound on a caller-supplied deadline (the transport never trusts an
/// unbounded value from the wire).
pub const MAX_QUERY_TIMEOUT_MS: u64 = 10_000;

/// Cap on how long a responder waits for its own DB-thread partial.
pub const RESPONDER_EXEC_CAP_MS: u64 = 5_000;

// ---------------------------------------------------------------------------
// Wire envelopes
// ---------------------------------------------------------------------------

/// One gossip message on a verse's compute topic. The request carries the
/// full target shard list (so a responder never needs its own ledger to
/// know what is being asked); the response carries the responder's partial.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ComputeEnvelope {
    Request {
        request_id: String,
        verse_id: String,
        from_did: String,
        spec: TsQueryKind,
        shards: Vec<PartialShard>,
        timeout_ms: u64,
        row_cap: usize,
    },
    Response {
        request_id: String,
        from_did: String,
        partial: TsPartialRows,
    },
}

/// One inbound gossip message forwarded by a verse topic pump.
#[derive(Debug, Clone)]
pub struct GossipIncoming {
    pub verse_id: String,
    pub from: iroh::NodeId,
    /// Whether the message was delivered DIRECTLY from its publisher
    /// (`DeliveryScope::is_direct()`: neighbor broadcast or 0 swarm hops).
    /// Only then does `from` authenticate the ENVELOPE author — a relayed
    /// delivery's `from` is the forwarding neighbor, not the original
    /// broadcaster (iroh-gossip 0.35 `GossipEvent` docs). The F23 identity
    /// check enforces `from_did == did_key(from)` strictly on direct
    /// deliveries; relayed deliveries cannot be wire-verified and fall back
    /// to claim-based admission (see §distributed-query in this crate's
    /// AGENTS.md for the residual).
    pub direct: bool,
    pub content: bytes::Bytes,
}

// ---------------------------------------------------------------------------
// Planner (pure)
// ---------------------------------------------------------------------------

/// The plan for one distributed query: which shards to read, who hosts them,
/// and who is expected to answer.
#[derive(Debug, Clone, PartialEq)]
pub struct DistributedPlan {
    /// Target shards (ledger entries for the petal, window-filtered).
    pub target_shards: Vec<PartialShard>,
    /// Host set per shard key, from the ledger.
    pub shard_hosts: BTreeMap<String, Vec<String>>,
    /// Remote peers expected to answer (host ≥1 target shard), local excluded.
    pub expected_hosts: BTreeSet<String>,
    /// The verse's current fabric mode (reported in the honesty metadata).
    pub mode: String,
    /// The verse's current replication factor.
    pub replication_factor: u32,
    /// The verse's bucket width (row→shard attribution for raw/latest merges).
    pub bucket_width_ms: u64,
}

/// Plan a distributed query against the verse's fabric. Pure.
///
/// Window queries target every ledger shard whose range overlaps
/// `[start_ms, end_ms)` (the ledger's recorded ranges, not recomputed buckets
/// — robust to a bucket-width change since placement); latest-per-anchor
/// targets every shard of the petal.
pub fn plan_distributed_query(
    spec: &TsQueryKind,
    fabric: &VerseFabric,
    local_did: &str,
) -> Result<DistributedPlan, String> {
    let petal_prefix = format!("{}/", spec.petal_id());
    let window = spec.window();
    let mut target_shards = Vec::new();
    let mut shard_hosts = BTreeMap::new();
    for entry in fabric.shards.values() {
        if !entry.shard.starts_with(&petal_prefix) {
            continue;
        }
        if let Some((start, end)) = window {
            if entry.range_start_ms >= end || entry.range_end_ms <= start {
                continue;
            }
        }
        let Some(anchor) = entry
            .shard
            .strip_prefix(&petal_prefix)
            .and_then(|rest| rest.split('/').next())
        else {
            tracing::warn!(shard = %entry.shard, "malformed shard key in ledger — skipped");
            continue;
        };
        if target_shards.len() >= MAX_TARGET_SHARDS {
            return Err(format!(
                "query window spans more than {MAX_TARGET_SHARDS} shards — narrow the window or widen the bucket width"
            ));
        }
        target_shards.push(PartialShard {
            shard: entry.shard.clone(),
            anchor_node_id: anchor.to_string(),
            range_start_ms: entry.range_start_ms,
            range_end_ms: entry.range_end_ms,
        });
        shard_hosts.insert(entry.shard.clone(), entry.hosts.clone());
    }
    let mut expected_hosts: BTreeSet<String> = shard_hosts
        .values()
        .flatten()
        .filter(|h| h.as_str() != local_did)
        .cloned()
        .collect();
    expected_hosts.remove(local_did);
    Ok(DistributedPlan {
        target_shards,
        shard_hosts,
        expected_hosts,
        mode: fabric.settings.mode.as_str().to_string(),
        replication_factor: fabric.settings.replication_factor,
        bucket_width_ms: fabric.settings.bucket_width_ms,
    })
}

// ---------------------------------------------------------------------------
// Merge (pure, commutative)
// ---------------------------------------------------------------------------

/// One host's answer to a distributed query (the local partial counts as a
/// response from the local DID).
#[derive(Debug, Clone, PartialEq)]
pub struct PartialResponse {
    pub host: String,
    pub partial: TsPartialRows,
}

/// Merge partials into the final outcome with honest coverage metadata
/// (A16). Pure and deterministic: winners are chosen by a fixed host order
/// (ledger hosts first, sorted), never by arrival — the sim lab's
/// determinism requirement and the merge's commutativity both depend on it.
///
/// **Failed partials never masquerade as answers (F23):** a response with
/// `partial.failed` (the executing host's DB errored) is excluded from the
/// rows, the coverage attribution, and `answered_hosts` — so a formal host
/// that failed lands in `missing_hosts` and its shards in `missing_shards`
/// (the metadata reads "host failed", never an authoritative covered-empty).
/// A failed host IS still "resolved" for the collector's settlement (it
/// answered; no more data is coming from it), which is why the filter lives
/// here and not at the inbox.
///
/// - `WindowAggregate`: per-shard attribution prevents double-counting when
///   several mirrors of one shard answer (the first NON-EMPTY partial wins
///   the shard; identical data makes any winner correct); across shards the
///   sum/count/min/max monoid adds up exactly. `avg_value` is derived at the
///   merge as `sum/count` — the same value the single-host `math::mean`
///   produces over the union.
/// - `ReadingsInWindow` (and `AllReadings`, the analytics whole-petal view):
///   union of all rows, deduped by `reading_id`, ordered by
///   `(recorded_at_ms, reading_id)` — duplicates from answering mirrors are
///   exact under union semantics.
/// - `LatestPerAnchor`: max `recorded_at_ms` per (anchor, metric), ties by
///   `hlc_timestamp` — idempotent under duplicate answers.
pub fn merge_partials(
    spec: &TsQueryKind,
    plan: &DistributedPlan,
    responses: &[PartialResponse],
) -> DistributedQueryOutcome {
    // Partition out failed partials (clone only on the rare failed path —
    // the pure fns below keep their borrowed slices).
    let usable_owned: Vec<PartialResponse>;
    let usable: &[PartialResponse] = if responses.iter().any(|r| r.partial.failed) {
        usable_owned = responses
            .iter()
            .filter(|r| !r.partial.failed)
            .cloned()
            .collect();
        &usable_owned
    } else {
        responses
    };
    let answered_hosts: BTreeSet<&str> = usable.iter().map(|r| r.host.as_str()).collect();
    let truncated = usable.iter().any(|r| r.partial.truncated);

    let (rows, covered_shards, missing_shards) = match spec {
        TsQueryKind::WindowAggregate { .. } => merge_aggregates(plan, usable),
        TsQueryKind::ReadingsInWindow { petal_id, .. } | TsQueryKind::AllReadings { petal_id } => {
            let (rows, covered, missing) = merge_raw(plan, usable);
            let attributed =
                attribute_rows_to_shards(&rows, petal_id, plan.bucket_width_ms, &plan.shard_hosts);
            let (covered, missing) = fold_attribution(covered, missing, attributed);
            (rows, covered, missing)
        }
        TsQueryKind::LatestPerAnchor { petal_id, .. } => {
            let rows = merge_latest(usable);
            let attributed =
                attribute_rows_to_shards(&rows, petal_id, plan.bucket_width_ms, &plan.shard_hosts);
            let (covered, missing) = covered_by_answered_hosts(plan, &answered_hosts);
            let (covered, missing) = fold_attribution(covered, missing, attributed);
            (rows, covered, missing)
        }
    };

    let missing_hosts: Vec<String> = plan
        .expected_hosts
        .iter()
        .filter(|h| !answered_hosts.contains(h.as_str()))
        .cloned()
        .collect();

    DistributedQueryOutcome {
        rows,
        meta: DistributedQueryMeta {
            covered_shards,
            missing_shards,
            answered_hosts: answered_hosts.iter().map(|h| h.to_string()).collect(),
            missing_hosts,
            mode: plan.mode.clone(),
            replication_factor: plan.replication_factor,
            truncated,
        },
        error: None,
    }
}

/// Per-shard first-non-empty-winner aggregate merge (see `merge_partials`).
fn merge_aggregates(
    plan: &DistributedPlan,
    responses: &[PartialResponse],
) -> (Vec<serde_json::Value>, Vec<String>, Vec<String>) {
    // Responses in a fixed order: by host DID, ledger hosts unchanged (the
    // order the winner scan walks). BTreeMap sort gives determinism.
    let mut ordered: BTreeMap<&str, &PartialResponse> = BTreeMap::new();
    for r in responses {
        ordered.insert(r.host.as_str(), r);
    }
    let mut covered = Vec::new();
    let mut missing = Vec::new();
    // (node_id, metric) → (sum, count, min, max)
    let mut acc: BTreeMap<(String, String), (f64, u64, f64, f64)> = BTreeMap::new();
    for shard in &plan.target_shards {
        // Winner scan in fixed host-DID order (deterministic, commutative).
        // A FORMAL ledger host's non-empty answer is authoritative for the
        // shard — the host holds the complete shard by construction, so an
        // over-retainer's stale subset (rows kept from before a mode switch)
        // must never displace it. Over-retained rows contribute only when no
        // formal host answered with data. Coverage honesty (A16): only a
        // FORMAL host's answer is authoritative for the shard (empty
        // included — "I host this and it contributes nothing"); a non-host's
        // empty answer proves nothing about the shard's true contents, so it
        // can never mark a shard covered, while its over-retained rows still
        // contribute data (and coverage, because the data IS in the result).
        let formal_hosts = plan.shard_hosts.get(&shard.shard);
        let is_formal = |host: &str| {
            formal_hosts
                .map(|hs| hs.iter().any(|h| h == host))
                .unwrap_or(false)
        };
        let mut authoritative = false;
        let mut formal_rows: Option<&fe_runtime::distributed_query::ShardRows> = None;
        let mut any_rows: Option<&fe_runtime::distributed_query::ShardRows> = None;
        for (host, resp) in &ordered {
            let Some(sr) = resp
                .partial
                .per_shard
                .iter()
                .find(|s| s.shard == shard.shard)
            else {
                continue;
            };
            if is_formal(host) {
                authoritative = true;
                if !sr.rows.is_empty() && formal_rows.is_none() {
                    formal_rows = Some(sr);
                }
            }
            if !sr.rows.is_empty() && any_rows.is_none() {
                any_rows = Some(sr);
            }
        }
        if let Some(sr) = formal_rows.or(any_rows) {
            for row in &sr.rows {
                let node_id = row["node_id"].as_str().unwrap_or_default().to_string();
                let metric = row["metric"].as_str().unwrap_or_default().to_string();
                let sum = row["sum_value"].as_f64().unwrap_or(0.0);
                let count = row["sample_count"].as_u64().unwrap_or(0);
                let min = row["min_value"].as_f64();
                let max = row["max_value"].as_f64();
                let entry = acc.entry((node_id, metric)).or_insert((
                    0.0,
                    0,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                ));
                entry.0 += sum;
                entry.1 += count;
                if let Some(m) = min {
                    entry.2 = entry.2.min(m);
                }
                if let Some(m) = max {
                    entry.3 = entry.3.max(m);
                }
            }
            tracing::debug!(shard = %shard.shard, "aggregate shard winner");
        }
        if authoritative || any_rows.is_some() {
            covered.push(shard.shard.clone());
        } else {
            missing.push(shard.shard.clone());
        }
    }
    let mut rows = Vec::with_capacity(acc.len());
    for ((node_id, metric), (sum, count, min, max)) in acc {
        if count == 0 {
            continue;
        }
        rows.push(serde_json::json!({
            "node_id": node_id,
            "metric": metric,
            "avg_value": sum / count as f64,
            "min_value": min,
            "max_value": max,
            "sample_count": count,
        }));
    }
    (rows, covered, missing)
}

/// Raw union + dedupe by `reading_id`, ordered by `(recorded_at_ms, reading_id)`.
fn merge_raw(
    plan: &DistributedPlan,
    responses: &[PartialResponse],
) -> (Vec<serde_json::Value>, Vec<String>, Vec<String>) {
    let mut seen: HashMap<String, serde_json::Value> = HashMap::new();
    // Deterministic dedupe: fold in host-DID order so which duplicate wins
    // never depends on arrival.
    let mut ordered: BTreeMap<&str, &PartialResponse> = BTreeMap::new();
    for r in responses {
        ordered.insert(r.host.as_str(), r);
    }
    for (_, resp) in ordered {
        for row in &resp.partial.rows {
            if let Some(id) = row["reading_id"].as_str() {
                seen.entry(id.to_string()).or_insert_with(|| row.clone());
            }
        }
    }
    let mut rows: Vec<serde_json::Value> = seen.into_values().collect();
    rows.sort_by(|a, b| {
        let ka = (
            a["recorded_at_ms"].as_i64().unwrap_or(0),
            a["reading_id"].as_str().unwrap_or_default(),
        );
        let kb = (
            b["recorded_at_ms"].as_i64().unwrap_or(0),
            b["reading_id"].as_str().unwrap_or_default(),
        );
        ka.cmp(&kb)
    });
    let answered: BTreeSet<&str> = responses.iter().map(|r| r.host.as_str()).collect();
    let (covered, missing) = covered_by_answered_hosts(plan, &answered);
    (rows, covered, missing)
}

/// Latest per (anchor, metric): max `recorded_at_ms`, ties by `hlc_timestamp`.
fn merge_latest(responses: &[PartialResponse]) -> Vec<serde_json::Value> {
    let mut best: BTreeMap<(String, String), serde_json::Value> = BTreeMap::new();
    let mut ordered: BTreeMap<&str, &PartialResponse> = BTreeMap::new();
    for r in responses {
        ordered.insert(r.host.as_str(), r);
    }
    for (_, resp) in ordered {
        for row in &resp.partial.rows {
            let key = (
                row["node_id"].as_str().unwrap_or_default().to_string(),
                row["metric"].as_str().unwrap_or_default().to_string(),
            );
            let rank = |v: &serde_json::Value| {
                (
                    v["recorded_at_ms"].as_i64().unwrap_or(0),
                    v["hlc_timestamp"].as_i64().unwrap_or(0),
                )
            };
            match best.get(&key) {
                Some(current) if rank(current) >= rank(row) => {}
                _ => {
                    best.insert(key, row.clone());
                }
            }
        }
    }
    best.into_values().collect()
}

/// Shards covered by hosts that answered (raw/latest: any host that formally
/// hosts the shard and sent any response covers it).
fn covered_by_answered_hosts(
    plan: &DistributedPlan,
    answered: &BTreeSet<&str>,
) -> (Vec<String>, Vec<String>) {
    let mut covered = Vec::new();
    let mut missing = Vec::new();
    for shard in &plan.target_shards {
        let hosts = plan
            .shard_hosts
            .get(&shard.shard)
            .cloned()
            .unwrap_or_default();
        if hosts.iter().any(|h| answered.contains(h.as_str())) {
            covered.push(shard.shard.clone());
        } else {
            missing.push(shard.shard.clone());
        }
    }
    (covered, missing)
}

/// Fold row→shard attribution into coverage (a shard whose rows are present
/// in the merged result is covered even if no formal host answered — the
/// data IS in the result; hiding it would be dishonest metadata).
fn fold_attribution(
    covered: Vec<String>,
    missing: Vec<String>,
    attributed: BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let covered_set: BTreeSet<String> = covered.into_iter().collect();
    let missing_set: BTreeSet<String> = missing.into_iter().collect();
    let covered: BTreeSet<String> = covered_set.union(&attributed).cloned().collect();
    let missing: Vec<String> = missing_set.difference(&covered).cloned().collect();
    (covered.into_iter().collect(), missing)
}

/// Attribute merged rows back to shard keys (`{petal}/{anchor}/{bucket}`) —
/// the coverage metadata for raw/latest merges.
fn attribute_rows_to_shards(
    rows: &[serde_json::Value],
    petal_id: &str,
    bucket_width_ms: u64,
    shard_hosts: &BTreeMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let width = bucket_width_ms.max(1) as i64;
    for row in rows {
        let Some(node) = row["node_id"].as_str() else {
            continue;
        };
        let Some(ms) = row["recorded_at_ms"].as_i64() else {
            continue;
        };
        let key = format!("{}/{}/{}", petal_id, node, ms.div_euclid(width));
        if shard_hosts.contains_key(&key) {
            out.insert(key);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Envelope serialization + budget
// ---------------------------------------------------------------------------

/// Serialize an envelope, trimming rows until it fits the gossip budget.
/// Trimming flags the partial `truncated` so the merge reports it (A16
/// honesty: a partial that did not fit is never silently rounded off).
/// No-progress guard (F23, M2 scrutiny F7 minor): when a single row alone
/// exceeds the budget the halving loop would re-serialize an identical
/// envelope forever — bail with an error instead (the responder drops the
/// answer with a warn, which the requester honestly sees as a silent host).
pub fn encode_envelope(envelope: &mut ComputeEnvelope) -> Result<bytes::Bytes, String> {
    let mut prev_len = usize::MAX;
    loop {
        let bytes = serde_json::to_vec(envelope).map_err(|e| e.to_string())?;
        if bytes.len() <= GOSSIP_ENVELOPE_BUDGET {
            return Ok(bytes.into());
        }
        if bytes.len() >= prev_len {
            return Err(
                "compute envelope cannot shrink below the gossip budget — a single row exceeds \
                 it; dropping the answer"
                    .into(),
            );
        }
        prev_len = bytes.len();
        match envelope {
            ComputeEnvelope::Response { partial, .. } => {
                let mut total = partial.rows.len();
                for sr in &mut partial.per_shard {
                    total += sr.rows.len();
                }
                if total == 0 {
                    return Err("compute response exceeds the gossip envelope budget".into());
                }
                partial.truncated = true;
                // Halve the heaviest collections first.
                if !partial.rows.is_empty() {
                    let keep = partial.rows.len() / 2;
                    partial.rows.truncate(keep.max(1));
                    continue;
                }
                let heaviest = partial
                    .per_shard
                    .iter_mut()
                    .max_by_key(|s| s.rows.len())
                    .expect("non-empty total implies a heaviest shard");
                let keep = heaviest.rows.len() / 2;
                heaviest.rows.truncate(keep.max(1));
            }
            ComputeEnvelope::Request { .. } => {
                return Err("compute request exceeds the gossip envelope budget".into());
            }
        }
    }
}

/// Decode an envelope from gossip bytes.
pub fn decode_envelope(bytes: &[u8]) -> Option<ComputeEnvelope> {
    serde_json::from_slice(bytes).ok()
}

// ---------------------------------------------------------------------------
// Transport runtime (correlation registry, collector, responder)
// ---------------------------------------------------------------------------

/// One response routed to a collector by request id.
#[derive(Debug)]
struct RoutedResponse {
    host: String,
    partial: TsPartialRows,
}

/// Request/response correlation: request id → the collector's inbox.
/// Shared between the select loop (routing inbound responses) and the
/// collector tasks (registration); short critical sections, std Mutex.
#[derive(Default)]
pub struct PendingQueries {
    map: std::sync::Mutex<HashMap<String, tokio::sync::mpsc::Sender<RoutedResponse>>>,
}

impl PendingQueries {
    fn register(&self, request_id: &str, tx: tokio::sync::mpsc::Sender<RoutedResponse>) -> bool {
        let mut map = self.map.lock().expect("pending queries lock");
        // F23 (M2 scrutiny F7 doc/code fix): a duplicate id REFUSES rather
        // than clobbering a live collector — the clobbered collector's
        // inbox would be replaced and its later deregister would evict the
        // replacement's registration. Unreachable with ULID request ids,
        // but the doc claim ("a duplicate id never clobbers a live
        // collector") and the code now agree.
        if map.contains_key(request_id) {
            return false;
        }
        if map.len() >= PENDING_QUERY_CAP {
            return false;
        }
        map.insert(request_id.to_string(), tx);
        true
    }

    fn deregister(&self, request_id: &str) {
        self.map
            .lock()
            .expect("pending queries lock")
            .remove(request_id);
    }

    /// Route one inbound response to its collector (drop with a debug log if
    /// the collector is gone — a late answer after a deadline).
    fn route(&self, request_id: &str, resp: RoutedResponse) {
        let tx = self
            .map
            .lock()
            .expect("pending queries lock")
            .get(request_id)
            .cloned();
        match tx {
            Some(tx) => {
                if let Err(e) = tx.try_send(resp) {
                    tracing::debug!("dropping compute response for a slow collector: {e:?}");
                }
            }
            None => tracing::debug!(
                request_id,
                "late compute response — collector already finished"
            ),
        }
    }
}

/// Shared transport state the sync thread hands to collector/responder tasks.
#[derive(Clone)]
pub struct DistributedTransport {
    pub local_did: String,
    pub db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
    pub pending: Arc<PendingQueries>,
    pub concurrency: Arc<tokio::sync::Semaphore>,
    pub evt_tx: crate::messages::SyncEventSender,
}

impl DistributedTransport {
    pub fn new(
        local_did: String,
        db_cmd_tx: Option<crossbeam::channel::Sender<DbCommand>>,
        evt_tx: crate::messages::SyncEventSender,
    ) -> Self {
        Self {
            local_did,
            db_cmd_tx,
            pending: Arc::new(PendingQueries::default()),
            concurrency: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_QUERIES)),
            evt_tx,
        }
    }

    /// Reply one outcome to the caller (warn on send failure — §warn-on-send).
    fn reply(
        &self,
        reply: &crossbeam::channel::Sender<DistributedQueryOutcome>,
        outcome: DistributedQueryOutcome,
    ) {
        if let Err(e) = reply.send(outcome) {
            tracing::warn!("distributed query reply send failed (requester gone): {e:?}");
        }
    }
}

/// A planned query handed to the collector task (all inputs owned — the
/// select loop never lends its state across an await).
struct CollectTask {
    request: fe_runtime::distributed_query::DistributedQueryRequest,
    reply: crossbeam::channel::Sender<DistributedQueryOutcome>,
    plan: DistributedPlan,
    gossip_sender: Option<GossipSender>,
    inbox: tokio::sync::mpsc::Receiver<RoutedResponse>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Effective partial row cap: the caller's cap clamped into range, or the
/// default when the caller passed zero.
fn effective_row_cap(cap: usize) -> usize {
    if cap == 0 {
        PARTIAL_ROW_CAP
    } else {
        clamp_partial_row_cap(cap)
    }
}

/// Entry point for [`crate::messages::SyncCommand::SubmitComputeTask`]:
/// validate, plan, register, then spawn the collector. Runs synchronously
/// in the select loop (no awaits) — everything slow lives in the task.
pub fn submit_distributed_query(
    transport: &DistributedTransport,
    fabrics: &HashMap<String, VerseFabric>,
    gossip_senders: &HashMap<String, GossipSender>,
    call: DistributedQueryCall,
) {
    let request = &call.request;
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        transport.reply(&call.reply, error_outcome("invalid request id"));
        return;
    }
    if request.timeout_ms == 0 {
        transport.reply(&call.reply, error_outcome("timeout_ms must be > 0"));
        return;
    }
    let timeout_ms = request.timeout_ms.min(MAX_QUERY_TIMEOUT_MS);
    let Some(fabric) = fabrics.get(&request.verse_id) else {
        transport.reply(
            &call.reply,
            error_outcome("no open fabric for this verse — open the replica first"),
        );
        return;
    };
    let plan = match plan_distributed_query(&request.spec, fabric, &transport.local_did) {
        Ok(p) => p,
        Err(reason) => {
            transport.reply(&call.reply, error_outcome(&reason));
            return;
        }
    };
    // Bounded concurrency: fail fast when the fleet is saturated.
    let permit = match transport.concurrency.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            transport.reply(
                &call.reply,
                error_outcome("distributed query queue full — too many concurrent fan-outs"),
            );
            return;
        }
    };
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::channel::<RoutedResponse>(64);
    if !transport.pending.register(&request.request_id, inbox_tx) {
        transport.reply(
            &call.reply,
            error_outcome(
                "distributed query queue full — too many pending requests or a duplicate \
                 request id is already live",
            ),
        );
        return;
    }
    let gossip_sender = gossip_senders
        .get(&crate::sync_thread::derive_gossip_topic(&request.verse_id))
        .cloned();
    let request = fe_runtime::distributed_query::DistributedQueryRequest {
        timeout_ms,
        ..request.clone()
    };
    tokio::spawn(collect_distributed_query(
        transport.clone(),
        CollectTask {
            request,
            reply: call.reply,
            plan,
            gossip_sender,
            inbox: inbox_rx,
            _permit: permit,
        },
    ));
}

/// An empty-outcome error reply (the metadata shape stays present and honest).
fn error_outcome(reason: &str) -> DistributedQueryOutcome {
    DistributedQueryOutcome {
        rows: Vec::new(),
        meta: DistributedQueryMeta {
            covered_shards: Vec::new(),
            missing_shards: Vec::new(),
            answered_hosts: Vec::new(),
            missing_hosts: Vec::new(),
            mode: "unknown".into(),
            replication_factor: 0,
            truncated: false,
        },
        error: Some(reason.to_string()),
    }
}

/// The collector task: broadcast the request, run the local partial, gather
/// answers until the query is settled (every shard won — aggregates — or
/// every expected host answered with the local partial resolved) or the
/// deadline hits, then merge (pure) and reply with honest metadata.
async fn collect_distributed_query(transport: DistributedTransport, task: CollectTask) {
    let CollectTask {
        request,
        reply,
        plan,
        gossip_sender,
        mut inbox,
        _permit,
    } = task;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(request.timeout_ms);
    let is_aggregate = matches!(request.spec, TsQueryKind::WindowAggregate { .. });
    let row_cap = effective_row_cap(request.row_cap);

    // 1. Local partial: the DB thread executes it; a side task awaits the
    //    crossbeam reply (spawn_blocking) and routes it into our inbox like
    //    any remote response — one inbox, one routing table, no special
    //    cases. `local_resolved` flips when it lands (or when none could).
    let local_spawned = transport
        .db_cmd_tx
        .as_ref()
        .map(|db| {
            let (tx, rx) = crossbeam::channel::bounded::<TsPartialRows>(1);
            let cmd = DbCommand::ExecuteTsPartial {
                spec: request.spec.clone(),
                shards: if is_aggregate {
                    plan.target_shards.clone()
                } else {
                    Vec::new()
                },
                row_cap,
                reply: tx,
            };
            if let Err(e) = db.try_send(cmd) {
                tracing::warn!(
                    request_id = %request.request_id,
                    "local partial send failed: {e:?}"
                );
                return false;
            }
            let pending = transport.pending.clone();
            let host = transport.local_did.clone();
            let request_id = request.request_id.clone();
            let wait = Duration::from_millis(request.timeout_ms.min(MAX_QUERY_TIMEOUT_MS));
            tokio::spawn(async move {
                let partial = tokio::task::spawn_blocking(move || rx.recv_timeout(wait).ok()).await;
                match partial {
                    Ok(Some(partial)) => {
                        pending.route(&request_id, RoutedResponse { host, partial })
                    }
                    _ => tracing::warn!(request_id, "local partial did not complete in time"),
                }
            });
            true
        })
        .unwrap_or(false);
    let mut local_resolved = !local_spawned;

    // 2. Fan out over the verse's gossip topic (None → local-only query;
    //    offline mode still serves the local partial — P2P-first but
    //    offline-friendly, D2). Aggregate requests carry the shard list for
    //    per-shard attribution; raw/latest need none (dedupe/max merges).
    if let Some(sender) = &gossip_sender {
        let mut env = ComputeEnvelope::Request {
            request_id: request.request_id.clone(),
            verse_id: request.verse_id.clone(),
            from_did: transport.local_did.clone(),
            spec: request.spec.clone(),
            shards: if is_aggregate {
                plan.target_shards.clone()
            } else {
                Vec::new()
            },
            timeout_ms: request.timeout_ms,
            row_cap,
        };
        match encode_envelope(&mut env) {
            Ok(bytes) => {
                if let Err(e) = sender.broadcast(bytes).await {
                    tracing::warn!(
                        request_id = %request.request_id,
                        "compute request broadcast failed: {e}"
                    );
                }
            }
            Err(reason) => {
                tracing::warn!(
                    request_id = %request.request_id,
                    reason,
                    "compute request envelope rejected"
                );
            }
        }
    } else {
        tracing::debug!(
            verse_id = %request.verse_id,
            "no gossip topic for the verse — running the distributed query local-only"
        );
    }

    // 3. Gather until settled or deadline.
    let mut responses: Vec<PartialResponse> = Vec::new();
    loop {
        let answered: BTreeSet<&str> = responses.iter().map(|r| r.host.as_str()).collect();
        let all_expected_answered = plan
            .expected_hosts
            .iter()
            .all(|h| answered.contains(h.as_str()));
        let settled = if is_aggregate {
            // Every shard won by a non-empty partial, or fully answered out
            // (no host can still contribute rows to it).
            plan.target_shards.iter().all(|shard| {
                responses.iter().any(|r| {
                    r.partial
                        .per_shard
                        .iter()
                        .any(|s| s.shard == shard.shard && !s.rows.is_empty())
                })
            }) || (all_expected_answered && local_resolved)
        } else {
            all_expected_answered && local_resolved
        };
        if settled {
            break;
        }
        match tokio::time::timeout_at(deadline, inbox.recv()).await {
            Ok(Some(routed)) => {
                if routed.host == transport.local_did {
                    local_resolved = true;
                }
                responses.push(PartialResponse {
                    host: routed.host,
                    partial: routed.partial,
                });
            }
            Ok(None) => break, // registration lost (shutdown) — merge what we have
            Err(_) => break,   // per-host deadline
        }
    }

    // 4. Merge (pure), reply, deregister, and emit the diagnostics event.
    let outcome = merge_partials(&request.spec, &plan, &responses);
    transport.reply(&reply, outcome.clone());
    transport.pending.deregister(&request.request_id);
    let evt = SyncEvent::ComputeResultReady {
        task_id: request.request_id.clone(),
        row_count: outcome.rows.len(),
        result_hash: crate::compute::hash_rows(&outcome.rows),
    };
    if let Err(e) = transport.evt_tx.send(evt) {
        tracing::warn!("sync event send failed: {e}");
    }
}

/// The disposition of one inbound gossip compute message: what the
/// admission gate (F23 hardening) decided. Returned so every drop shape is
/// unit-testable without a gossip stack; production callers ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GossipDisposition {
    /// A response was routed to its collector.
    Routed,
    /// A request passed admission — the responder task was spawned.
    Answered,
    /// Not a compute envelope (tileset/foreign traffic) — ignored at debug.
    Ignored,
    /// Refused at the gate; the reason is the honest log line.
    Dropped(&'static str),
}

/// Admission-gate drop reasons — named so the logs and the tests share one
/// definition.
pub mod drop_reason {
    /// Our own request echoed back — never respond to self.
    pub const SELF_ECHO: &str = "self echo";
    /// Structural validation failed (id/timeout/shard-list shape).
    pub const MALFORMED: &str = "malformed request";
    /// The envelope's verse claim does not match the verse whose topic the
    /// message arrived on (the only authenticated verse binding).
    pub const VERSE_MISMATCH: &str = "envelope verse does not match the arrival topic";
    /// On a direct delivery the claimed `from_did` is not the authenticated
    /// sender's did:key — forged attribution.
    pub const FORGED_SENDER: &str = "claimed from_did does not match the authenticated sender";
    /// The requesting peer is not declared in this verse's fabric
    /// (`__peers/{did}`) — deny-by-default for strangers on a topic id that
    /// is derivable from the public verse ULID (no capability).
    pub const UNDECLARED_REQUESTER: &str = "requester is not a declared fabric peer";
    /// Aggregate shard list does not sit inside the spec's petal.
    pub const SHARD_LIST_MISMATCH: &str = "shard list does not match its petal";
    /// Raw/latest request for a petal this verse's ledger knows no shard
    /// for — legitimately not answering.
    pub const PETAL_UNKNOWN: &str = "petal unknown here";
    /// The verse topic's sender is gone (replica closed mid-request).
    pub const TOPIC_CLOSED: &str = "topic closed under us";
}

/// Handle one inbound gossip message: route responses, answer requests.
/// Synchronous (the responder's DB round trip lives in a spawned task).
///
/// **Admission control (F23, the sync-plane hardening pass):**
/// 1. A request's envelope `verse_id` must equal the verse of the topic the
///    message ARRIVED on — the arrival topic is the authenticated binding;
///    the envelope field is a claim. A mismatch is dropped.
/// 2. On a DIRECT delivery, the claimed `from_did` must equal
///    `did_key(incoming.from)` — the F20 endpoint-identity == fe-DID
///    alignment makes the authenticated sender's DID derivable, so
///    forged-attribution envelopes (a covered-empty masquerading as a
///    formal host's answer, a request impersonating a declared peer) are
///    dropped. RELAYED deliveries cannot be wire-verified (iroh-gossip's
///    `from` is the forwarding neighbor, not the original broadcaster), so
///    they are admitted claim-based — the residual is documented in
///    `AGENTS.md` §distributed-query.
/// 3. The requesting peer must be a DECLARED fabric peer of the arrival
///    verse (`__peers/{did}`) before we execute anything — membership
///    requires a doc round trip, so a stranger on the derivable gossip
///    topic cannot read. Claim-based for relayed requests; honest about
///    convergence (a responder that has not converged the requester's
///    declaration yet refuses until it does).
pub fn handle_gossip_incoming(
    transport: &DistributedTransport,
    fabrics: &HashMap<String, VerseFabric>,
    gossip_senders: &HashMap<String, GossipSender>,
    incoming: GossipIncoming,
) -> GossipDisposition {
    let Some(envelope) = decode_envelope(&incoming.content) else {
        // Foreign traffic on the verse topic (tileset announcements) — not
        // consumed by anyone today; ignore at debug, never fatal.
        tracing::debug!(verse_id = %incoming.verse_id, "ignoring non-compute gossip message");
        return GossipDisposition::Ignored;
    };
    match envelope {
        ComputeEnvelope::Response {
            request_id,
            from_did,
            partial,
        } => {
            if incoming.direct && from_did != crate::replicator::peer_did_key(&incoming.from) {
                tracing::warn!(
                    request_id = %request_id,
                    claimed = %from_did,
                    "forged-attribution compute response dropped — claimed from_did does not \
                     match the authenticated direct sender"
                );
                return GossipDisposition::Dropped(drop_reason::FORGED_SENDER);
            }
            transport.pending.route(
                &request_id,
                RoutedResponse {
                    host: from_did,
                    partial,
                },
            );
            GossipDisposition::Routed
        }
        ComputeEnvelope::Request {
            request_id,
            verse_id,
            from_did,
            spec,
            shards,
            timeout_ms,
            row_cap,
        } => {
            if from_did == transport.local_did {
                return GossipDisposition::Dropped(drop_reason::SELF_ECHO);
            }
            if request_id.is_empty()
                || request_id.len() > 128
                || timeout_ms == 0
                || timeout_ms > MAX_QUERY_TIMEOUT_MS
            {
                tracing::warn!(request_id, "ignoring malformed compute request");
                return GossipDisposition::Dropped(drop_reason::MALFORMED);
            }
            if verse_id != incoming.verse_id {
                tracing::warn!(
                    request_id,
                    claimed_verse = %verse_id,
                    topic_verse = %incoming.verse_id,
                    "compute request verse does not match its arrival topic — dropped"
                );
                return GossipDisposition::Dropped(drop_reason::VERSE_MISMATCH);
            }
            if incoming.direct && from_did != crate::replicator::peer_did_key(&incoming.from) {
                tracing::warn!(
                    request_id = %request_id,
                    claimed = %from_did,
                    "forged-attribution compute request dropped — claimed from_did does not \
                     match the authenticated direct sender"
                );
                return GossipDisposition::Dropped(drop_reason::FORGED_SENDER);
            }
            let declared = fabrics
                .get(&incoming.verse_id)
                .map(|f| f.peers.contains_key(&from_did))
                .unwrap_or(false);
            if !declared {
                tracing::warn!(
                    request_id,
                    requester = %from_did,
                    "compute request from a peer not declared in this verse's fabric — dropped"
                );
                return GossipDisposition::Dropped(drop_reason::UNDECLARED_REQUESTER);
            }
            // Petal containment (unchanged, the honest narrow boundary): the
            // petal prefix bounds the read surface — aggregate shard lists
            // must sit entirely inside the spec's petal, raw/latest requests
            // must name a petal this verse's ledger has at least one shard
            // for. A compute request can never read outside the petal it
            // was sent for.
            let petal_prefix = format!("{}/", spec.petal_id());
            let is_aggregate = matches!(spec, TsQueryKind::WindowAggregate { .. });
            if is_aggregate {
                if shards.is_empty() || shards.len() > MAX_TARGET_SHARDS {
                    tracing::warn!(
                        request_id,
                        "aggregate compute request with bad shard list — ignored"
                    );
                    return GossipDisposition::Dropped(drop_reason::MALFORMED);
                }
                if !shards.iter().all(|s| s.shard.starts_with(&petal_prefix)) {
                    tracing::warn!(
                        request_id,
                        "compute request shards do not match its petal — ignored"
                    );
                    return GossipDisposition::Dropped(drop_reason::SHARD_LIST_MISMATCH);
                }
            } else {
                // Raw/latest: no shard list rides the request; answer only
                // when this verse's ledger knows the petal (no rows could be
                // hosted for an unknown petal anyway).
                let petal_known = fabrics
                    .get(&incoming.verse_id)
                    .map(|f| f.shards.keys().any(|k| k.starts_with(&petal_prefix)))
                    .unwrap_or(false);
                if !petal_known {
                    tracing::debug!(
                        request_id,
                        "compute request petal unknown here — not answering"
                    );
                    return GossipDisposition::Dropped(drop_reason::PETAL_UNKNOWN);
                }
            }
            let Some(sender) =
                gossip_senders.get(&crate::sync_thread::derive_gossip_topic(&incoming.verse_id))
            else {
                return GossipDisposition::Dropped(drop_reason::TOPIC_CLOSED);
            };
            tokio::spawn(respond_to_compute_request(
                transport.clone(),
                sender.clone(),
                request_id,
                spec,
                shards,
                timeout_ms.min(MAX_QUERY_TIMEOUT_MS),
                effective_row_cap(row_cap),
            ));
            GossipDisposition::Answered
        }
    }
}

/// The responder task: execute the requested partial on the local DB thread
/// and broadcast the answer. Absence of an answer is the requester's signal
/// that this host had nothing (or was too slow) — never an error path here.
async fn respond_to_compute_request(
    transport: DistributedTransport,
    sender: GossipSender,
    request_id: String,
    spec: TsQueryKind,
    shards: Vec<PartialShard>,
    timeout_ms: u64,
    row_cap: usize,
) {
    let Some(db) = transport.db_cmd_tx.clone() else {
        return; // a peer with no DB thread cannot answer
    };
    let (tx, rx) = crossbeam::channel::bounded::<TsPartialRows>(1);
    let cmd = DbCommand::ExecuteTsPartial {
        spec,
        shards,
        row_cap,
        reply: tx,
    };
    if let Err(e) = db.try_send(cmd) {
        tracing::warn!(request_id, "compute partial send failed (DB busy): {e:?}");
        return;
    }
    let wait = Duration::from_millis(timeout_ms.min(RESPONDER_EXEC_CAP_MS));
    let partial = match tokio::task::spawn_blocking(move || rx.recv_timeout(wait)).await {
        Ok(Ok(p)) => p,
        Ok(Err(_)) => {
            tracing::debug!(request_id, "local partial timed out — not answering");
            return;
        }
        Err(e) => {
            tracing::warn!(request_id, "local partial task failed: {e:?}");
            return;
        }
    };
    let mut envelope = ComputeEnvelope::Response {
        request_id: request_id.clone(),
        from_did: transport.local_did.clone(),
        partial,
    };
    match encode_envelope(&mut envelope) {
        Ok(bytes) => {
            if let Err(e) = sender.broadcast(bytes).await {
                tracing::warn!(request_id, "compute response broadcast failed: {e}");
            }
        }
        Err(reason) => {
            tracing::warn!(
                request_id,
                reason,
                "compute response rejected by envelope budget"
            );
        }
    }
}

/// Bridge the API-side seam channel into sync commands (spawned by the
/// binaries, mirroring the replication bridge): every call becomes a
/// `SubmitComputeTask`.
pub fn bridge_distributed_queries(
    rx: crossbeam::channel::Receiver<DistributedQueryCall>,
    sync_cmd_tx: crate::messages::SyncCommandSender,
) {
    std::thread::spawn(move || {
        for call in rx {
            if sync_cmd_tx
                .send(crate::messages::SyncCommand::SubmitComputeTask { call })
                .is_err()
            {
                break; // sync thread gone — shutdown, silent (§backpressure)
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sharding::{PeerDeclaration, ShardId, ShardLedgerEntry};
    use fe_runtime::distributed_query::ShardRows;
    use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings};

    fn fabric_with_shards(
        settings: VerseTimeseriesSettings,
        shards: &[(&str, i64, i64, &[&str])],
    ) -> VerseFabric {
        let mut fabric = VerseFabric {
            settings,
            ..Default::default()
        };
        for (key, start, end, hosts) in shards {
            fabric.shards.insert(
                key.to_string(),
                ShardLedgerEntry {
                    shard: key.to_string(),
                    hosts: hosts.iter().map(|h| h.to_string()).collect(),
                    mode: TimeseriesMode::Mirror,
                    replication_factor: 1,
                    bucket_width_ms: 60_000,
                    range_start_ms: *start,
                    range_end_ms: *end,
                    row_count: 1,
                    size_bytes: 100,
                },
            );
        }
        fabric
    }

    fn agg_spec(start: i64, end: i64) -> TsQueryKind {
        TsQueryKind::WindowAggregate {
            metric: "temperature_c".into(),
            start_ms: start,
            end_ms: end,
            petal_id: "p1".into(),
        }
    }

    fn reading_row(id: &str, node: &str, value: f64, ms: i64) -> serde_json::Value {
        serde_json::json!({
            "reading_id": id,
            "node_id": node,
            "petal_id": "p1",
            "metric": "temperature_c",
            "value": value,
            "recorded_at_ms": ms,
            "hlc_timestamp": ms,
        })
    }

    fn agg_row(node: &str, sum: f64, count: u64, min: f64, max: f64) -> serde_json::Value {
        serde_json::json!({
            "node_id": node,
            "metric": "temperature_c",
            "sum_value": sum,
            "min_value": min,
            "max_value": max,
            "sample_count": count,
        })
    }

    fn shard_rows(shard: &str, rows: Vec<serde_json::Value>) -> ShardRows {
        ShardRows {
            shard: shard.into(),
            rows,
        }
    }

    // ── Planner ────────────────────────────────────────────────────────────

    #[test]
    fn plan_targets_only_window_overlapping_shards_of_the_petal() {
        let settings = VerseTimeseriesSettings {
            mode: TimeseriesMode::Sharded,
            replication_factor: 1,
            bucket_width_ms: 60_000,
        };
        let fabric = fabric_with_shards(
            settings,
            &[
                ("p1/a/0", 0, 60_000, &["did:a"]),
                ("p1/a/1", 60_000, 120_000, &["did:b"]),
                ("p1/a/9", 540_000, 600_000, &["did:a"]),
                ("p2/a/0", 0, 60_000, &["did:a"]),
            ],
        );
        let plan = plan_distributed_query(&agg_spec(0, 120_000), &fabric, "did:local").unwrap();
        let keys: Vec<&str> = plan
            .target_shards
            .iter()
            .map(|s| s.shard.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["p1/a/0", "p1/a/1"],
            "window-overlap + petal filter"
        );
        assert_eq!(
            plan.expected_hosts,
            BTreeSet::from(["did:a".into(), "did:b".into()])
        );
        assert_eq!(plan.mode, "sharded");
    }

    #[test]
    fn plan_latest_targets_all_petal_shards() {
        let settings = VerseTimeseriesSettings {
            mode: TimeseriesMode::Balanced,
            replication_factor: 2,
            bucket_width_ms: 60_000,
        };
        let fabric = fabric_with_shards(
            settings,
            &[
                ("p1/a/0", 0, 60_000, &["did:a", "did:b"]),
                ("p1/b/3", 180_000, 240_000, &["did:b"]),
            ],
        );
        let spec = TsQueryKind::LatestPerAnchor {
            petal_id: "p1".into(),
            metric: None,
        };
        let plan = plan_distributed_query(&spec, &fabric, "did:a").unwrap();
        assert_eq!(plan.target_shards.len(), 2);
        assert_eq!(plan.expected_hosts, BTreeSet::from(["did:b".into()]));
        assert_eq!(plan.replication_factor, 2);
    }

    #[test]
    fn plan_caps_target_shards_honestly() {
        let settings = VerseTimeseriesSettings::default();
        let shards: Vec<(&str, i64, i64, &[&str])> = (0..(MAX_TARGET_SHARDS + 1))
            .map(|i| {
                let start = i as i64 * 60_000;
                (
                    Box::leak(format!("p1/a/{i}").into_boxed_str()) as &str,
                    start,
                    start + 60_000,
                    &["did:a"][..],
                )
            })
            .collect();
        let fabric = fabric_with_shards(settings, &shards);
        let err = plan_distributed_query(&agg_spec(0, i64::MAX), &fabric, "did:local").unwrap_err();
        assert!(err.contains("shards"), "honest over-cap error: {err}");
    }

    #[test]
    fn plan_empty_ledger_is_an_empty_plan() {
        let fabric = VerseFabric::default();
        let plan = plan_distributed_query(&agg_spec(0, 1_000), &fabric, "did:local").unwrap();
        assert!(plan.target_shards.is_empty());
        assert!(plan.expected_hosts.is_empty());
    }

    // ── Aggregate merge: exact vs the union ground truth ───────────────────

    /// Ground truth: aggregate over the UNION of rows (what the single-host
    /// `math::mean` would produce if every row were in one store).
    fn union_aggregate(
        rows: &[serde_json::Value],
        metric: &str,
    ) -> BTreeMap<(String, String), (f64, u64, f64, f64)> {
        let mut acc: BTreeMap<(String, String), (f64, u64, f64, f64)> = BTreeMap::new();
        for r in rows {
            if r["metric"].as_str() != Some(metric) {
                continue;
            }
            let key = (
                r["node_id"].as_str().unwrap_or_default().to_string(),
                r["metric"].as_str().unwrap_or_default().to_string(),
            );
            let v = r["value"].as_f64().unwrap_or(0.0);
            let e = acc
                .entry(key)
                .or_insert((0.0, 0, f64::INFINITY, f64::NEG_INFINITY));
            e.0 += v;
            e.1 += 1;
            e.2 = e.2.min(v);
            e.3 = e.3.max(v);
        }
        acc
    }

    fn merged_aggregate_map(
        outcome: &DistributedQueryOutcome,
    ) -> BTreeMap<(String, String), (f64, u64, f64, f64)> {
        outcome
            .rows
            .iter()
            .map(|r| {
                (
                    (
                        r["node_id"].as_str().unwrap_or_default().to_string(),
                        r["metric"].as_str().unwrap_or_default().to_string(),
                    ),
                    (
                        r["avg_value"].as_f64().unwrap_or(0.0),
                        r["sample_count"].as_u64().unwrap_or(0),
                        r["min_value"].as_f64().unwrap_or(0.0),
                        r["max_value"].as_f64().unwrap_or(0.0),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn aggregate_merge_across_three_hosts_equals_union_ground_truth() {
        // A15: the merged window aggregate across peers must equal the same
        // query over the UNION of all rows — computed here from raw rows,
        // not hand-made fixtures.
        let rows_a = [
            reading_row("r1", "a", 10.0, 0),
            reading_row("r2", "a", 20.0, 10),
            reading_row("r3", "b", 5.0, 20),
        ];
        let rows_b = vec![reading_row("r4", "a", 30.0, 61_000)];
        let rows_c = vec![
            reading_row("r5", "b", 7.5, 62_000),
            reading_row("r6", "b", 2.5, 63_000),
        ];
        let union: Vec<_> = rows_a
            .iter()
            .chain(&rows_b)
            .chain(&rows_c)
            .cloned()
            .collect();
        let truth = union_aggregate(&union, "temperature_c");

        let settings = VerseTimeseriesSettings::default();
        let fabric = fabric_with_shards(
            settings,
            &[
                ("p1/a/0", 0, 60_000, &["did:a"]),
                ("p1/b/0", 0, 60_000, &["did:a"]),
                ("p1/a/1", 60_000, 120_000, &["did:b"]),
                ("p1/b/1", 60_000, 120_000, &["did:c"]),
            ],
        );
        let plan = plan_distributed_query(&agg_spec(0, 120_000), &fabric, "did:a").unwrap();

        let mk = |per_shard: Vec<(&str, Vec<serde_json::Value>)>| PartialResponse {
            host: String::new(),
            partial: TsPartialRows {
                per_shard: per_shard
                    .into_iter()
                    .map(|(s, rows)| fe_runtime::distributed_query::ShardRows {
                        shard: s.into(),
                        rows,
                    })
                    .collect(),
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        // Host A answers its shards (sum/count per shard, exactly what the
        // partial SQL produces).
        let a = mk(vec![
            ("p1/a/0", vec![agg_row("a", 30.0, 2, 10.0, 20.0)]),
            ("p1/b/0", vec![agg_row("b", 5.0, 1, 5.0, 5.0)]),
        ]);
        let b = mk(vec![("p1/a/1", vec![agg_row("a", 30.0, 1, 30.0, 30.0)])]);
        let c = mk(vec![("p1/b/1", vec![agg_row("b", 10.0, 2, 2.5, 7.5)])]);

        let responses = [
            PartialResponse {
                host: "did:a".into(),
                ..a
            },
            PartialResponse {
                host: "did:b".into(),
                ..b
            },
            PartialResponse {
                host: "did:c".into(),
                ..c
            },
        ];
        let outcome = merge_partials(&agg_spec(0, 120_000), &plan, &responses);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.meta.covered_shards.len(), 4);
        assert!(outcome.meta.missing_shards.is_empty());
        assert_eq!(outcome.meta.missing_hosts, Vec::<String>::new());

        let merged = merged_aggregate_map(&outcome);
        let truth_rows: BTreeMap<_, _> = truth
            .iter()
            .map(|(k, (sum, count, min, max))| {
                (k.clone(), (sum / *count as f64, *count, *min, *max))
            })
            .collect();
        assert_eq!(
            merged, truth_rows,
            "merged result must equal the union ground truth"
        );
    }

    #[test]
    fn aggregate_merge_is_commutative_and_duplicate_mirror_safe() {
        // Same shard answered by two mirrors (R=2): first non-empty wins,
        // never double-counted. Order of responses must not matter.
        let settings = VerseTimeseriesSettings {
            mode: TimeseriesMode::Balanced,
            replication_factor: 2,
            bucket_width_ms: 60_000,
        };
        let fabric = fabric_with_shards(settings, &[("p1/a/0", 0, 60_000, &["did:a", "did:b"])]);
        let plan = plan_distributed_query(&agg_spec(0, 60_000), &fabric, "did:local").unwrap();
        let partial = |host: &str, sum: f64, count: u64| PartialResponse {
            host: host.into(),
            partial: TsPartialRows {
                per_shard: vec![fe_runtime::distributed_query::ShardRows {
                    shard: "p1/a/0".into(),
                    rows: vec![agg_row("a", sum, count, sum, sum)],
                }],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        let r1 = [partial("did:a", 30.0, 2), partial("did:b", 30.0, 2)];
        let r2 = [partial("did:b", 30.0, 2), partial("did:a", 30.0, 2)];
        let o1 = merge_partials(&agg_spec(0, 60_000), &plan, &r1);
        let o2 = merge_partials(&agg_spec(0, 60_000), &plan, &r2);
        assert_eq!(o1, o2, "commutative under response order");
        assert_eq!(o1.rows.len(), 1);
        assert_eq!(
            o1.rows[0]["sample_count"].as_u64(),
            Some(2),
            "mirror duplicates never double-count"
        );
        assert_eq!(o1.rows[0]["avg_value"].as_f64(), Some(15.0));
    }

    #[test]
    fn aggregate_formal_host_answer_never_loses_to_over_retained_subset() {
        // A mode switch can leave an over-retainer holding a STALE SUBSET of
        // a shard the ledger moved to a new host. The formal host holds the
        // complete shard, so its answer must win regardless of DID order —
        // an over-retainer's partial view would silently under-count.
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Sharded,
                replication_factor: 1,
                bucket_width_ms: 60_000,
            },
            &[("p1/a/0", 0, 60_000, &["did:host"])],
        );
        let plan = plan_distributed_query(&agg_spec(0, 60_000), &fabric, "did:host").unwrap();
        let host_full = PartialResponse {
            host: "did:host".into(),
            partial: TsPartialRows {
                per_shard: vec![shard_rows(
                    "p1/a/0",
                    vec![agg_row("a", 30.0, 2, 10.0, 20.0)],
                )],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        // "did:over" < "did:host" in DID order — the naive first-non-empty
        // scan would let the stale subset win.
        let over_retainer_stale = PartialResponse {
            host: "did:over".into(),
            partial: TsPartialRows {
                per_shard: vec![shard_rows(
                    "p1/a/0",
                    vec![agg_row("a", 10.0, 1, 10.0, 10.0)],
                )],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        for responses in [
            vec![over_retainer_stale.clone(), host_full.clone()],
            vec![host_full, over_retainer_stale],
        ] {
            let outcome = merge_partials(&agg_spec(0, 60_000), &plan, &responses);
            assert_eq!(outcome.rows.len(), 1);
            assert_eq!(
                outcome.rows[0]["sample_count"].as_u64(),
                Some(2),
                "the formal host's complete answer must win the shard"
            );
            assert_eq!(outcome.rows[0]["avg_value"].as_f64(), Some(15.0));
            assert_eq!(outcome.meta.covered_shards, vec!["p1/a/0".to_string()]);
            assert!(outcome.meta.missing_shards.is_empty());
        }
    }

    #[test]
    fn aggregate_offline_host_at_r1_is_missing_but_covered_at_r2() {
        // A16: at R=1 an offline host's shard is invisible when no local copy
        // exists (missing); a surviving mirror (R=2) covers it. And when the
        // local peer DOES hold an over-retained copy that answers with rows,
        // honesty wins — the data is in the result, so the shard is covered
        // even though the ledger never listed the local peer as a host.
        let mk_fabric = |hosts: &[&str]| {
            fabric_with_shards(
                VerseTimeseriesSettings {
                    mode: TimeseriesMode::Sharded,
                    replication_factor: 1,
                    bucket_width_ms: 60_000,
                },
                &[("p1/a/1", 60_000, 120_000, hosts)],
            )
        };
        let r1_plan =
            plan_distributed_query(&agg_spec(0, 120_000), &mk_fabric(&["did:offline"]), "did:a")
                .unwrap();
        let r2_fabric = fabric_with_shards(
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Balanced,
                replication_factor: 2,
                bucket_width_ms: 60_000,
            },
            &[("p1/a/1", 60_000, 120_000, &["did:offline", "did:a"])],
        );
        let r2_plan = plan_distributed_query(&agg_spec(0, 120_000), &r2_fabric, "did:a").unwrap();

        // Case 1: the local peer holds nothing (empty partial) — the offline
        // host's shard is genuinely invisible at R=1.
        let local_empty = PartialResponse {
            host: "did:a".into(),
            partial: TsPartialRows {
                per_shard: vec![fe_runtime::distributed_query::ShardRows {
                    shard: "p1/a/1".into(),
                    rows: Vec::new(),
                }],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        let o1 = merge_partials(
            &agg_spec(0, 120_000),
            &r1_plan,
            std::slice::from_ref(&local_empty),
        );
        assert_eq!(
            o1.meta.covered_shards,
            Vec::<String>::new(),
            "an empty answer covers nothing"
        );
        assert_eq!(o1.meta.missing_shards, vec!["p1/a/1".to_string()]);
        assert_eq!(o1.meta.missing_hosts, vec!["did:offline".to_string()]);
        assert!(
            o1.rows.is_empty(),
            "the offline host's data is invisible at R=1"
        );

        // Case 2: the local peer answers with rows it over-retained before
        // the mode switch — the data IS in the result, so coverage is
        // honest about it (hiding contributed data would be a lie).
        let local_data = PartialResponse {
            host: "did:a".into(),
            partial: TsPartialRows {
                per_shard: vec![fe_runtime::distributed_query::ShardRows {
                    shard: "p1/a/1".into(),
                    rows: vec![agg_row("a", 30.0, 2, 10.0, 20.0)],
                }],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        let o_local = merge_partials(
            &agg_spec(0, 120_000),
            &r1_plan,
            std::slice::from_ref(&local_data),
        );
        assert_eq!(o_local.meta.covered_shards, vec!["p1/a/1".to_string()]);
        assert_eq!(o_local.rows.len(), 1, "the local copy's rows are served");
        assert_eq!(
            o_local.meta.missing_hosts,
            vec!["did:offline".to_string()],
            "the offline formal host is still reported missing"
        );

        // Case 3: R=2 with the local peer a formal mirror — covered, served.
        let o2 = merge_partials(
            &agg_spec(0, 120_000),
            &r2_plan,
            std::slice::from_ref(&local_data),
        );
        assert_eq!(o2.meta.missing_shards, Vec::<String>::new());
        assert_eq!(o2.meta.covered_shards, vec!["p1/a/1".to_string()]);
        assert_eq!(
            o2.rows.len(),
            1,
            "the surviving mirror serves the shard at R=2"
        );
    }

    #[test]
    fn aggregate_answered_but_empty_shard_is_covered() {
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings::default(),
            &[("p1/a/0", 0, 60_000, &["did:b"])],
        );
        let plan = plan_distributed_query(&agg_spec(0, 60_000), &fabric, "did:a").unwrap();
        let answered_empty = PartialResponse {
            host: "did:b".into(),
            partial: TsPartialRows {
                per_shard: vec![fe_runtime::distributed_query::ShardRows {
                    shard: "p1/a/0".into(),
                    rows: Vec::new(),
                }],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        let outcome = merge_partials(&agg_spec(0, 60_000), &plan, &[answered_empty]);
        assert_eq!(outcome.meta.covered_shards, vec!["p1/a/0".to_string()]);
        assert!(outcome.meta.missing_shards.is_empty());
        assert!(
            outcome.rows.is_empty(),
            "covered with zero rows is honest, not missing"
        );
        assert!(outcome.meta.missing_hosts.is_empty());
    }

    // ── Raw merge ──────────────────────────────────────────────────────────

    #[test]
    fn raw_merge_dedupes_by_reading_id_against_union() {
        let rows = vec![
            reading_row("r1", "a", 1.0, 0),
            reading_row("r2", "a", 2.0, 10),
            reading_row("r3", "b", 3.0, 20),
        ];
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings::default(),
            &[
                ("p1/a/0", 0, 60_000, &["did:a"]),
                ("p1/b/0", 0, 60_000, &["did:b"]),
            ],
        );
        let spec = TsQueryKind::ReadingsInWindow {
            metric: "temperature_c".into(),
            start_ms: 0,
            end_ms: 60_000,
            petal_id: "p1".into(),
        };
        let plan = plan_distributed_query(&spec, &fabric, "did:a").unwrap();

        // Both hosts return the SAME mirror rows — the union must dedupe.
        let resp = |host: &str, rows: &Vec<serde_json::Value>| PartialResponse {
            host: host.into(),
            partial: TsPartialRows {
                per_shard: Vec::new(),
                rows: rows.clone(),
                truncated: false,
                failed: false,
            },
        };
        let outcome = merge_partials(&spec, &plan, &[resp("did:a", &rows), resp("did:b", &rows)]);
        let ids: Vec<&str> = outcome
            .rows
            .iter()
            .map(|r| r["reading_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["r1", "r2", "r3"],
            "dedupe by reading_id, ordered by time"
        );
        assert_eq!(outcome.meta.covered_shards.len(), 2);
    }

    #[test]
    fn all_readings_merge_dedupes_the_whole_petal_view() {
        // The analytics surface's whole-petal view rides the same union
        // dedupe: every mirror's rows fold to one set per reading_id.
        let a_rows = vec![
            reading_row("r1", "a", 1.0, 0),
            reading_row("r2", "a", 2.0, 10),
        ];
        let b_rows = vec![
            reading_row("r2", "a", 2.0, 10),
            reading_row("r3", "b", 3.0, 20),
        ];
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings::default(),
            &[
                ("p1/a/0", 0, 60_000, &["did:a"]),
                ("p1/b/0", 0, 60_000, &["did:b"]),
            ],
        );
        let spec = TsQueryKind::AllReadings {
            petal_id: "p1".into(),
        };
        let plan = plan_distributed_query(&spec, &fabric, "did:a").unwrap();
        let resp = |host: &str, rows: Vec<serde_json::Value>| PartialResponse {
            host: host.into(),
            partial: TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated: false,
                failed: false,
            },
        };
        let outcome = merge_partials(
            &spec,
            &plan,
            &[resp("did:a", a_rows), resp("did:b", b_rows)],
        );
        let ids: Vec<&str> = outcome
            .rows
            .iter()
            .map(|r| r["reading_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["r1", "r2", "r3"], "whole-petal union, deduped");
        assert_eq!(outcome.meta.covered_shards.len(), 2);
        assert!(outcome.meta.missing_shards.is_empty());
        assert!(outcome.error.is_none());
    }

    #[test]
    fn latest_merge_keeps_max_timestamp() {
        let spec = TsQueryKind::LatestPerAnchor {
            petal_id: "p1".into(),
            metric: None,
        };
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings::default(),
            &[("p1/a/0", 0, 60_000, &["did:a"])],
        );
        let plan = plan_distributed_query(&spec, &fabric, "did:a").unwrap();
        let older = reading_row("r1", "a", 1.0, 0);
        let newer = reading_row("r2", "a", 2.0, 30);
        let resp = |host: &str, rows: Vec<serde_json::Value>| PartialResponse {
            host: host.into(),
            partial: TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated: false,
                failed: false,
            },
        };
        let outcome = merge_partials(
            &spec,
            &plan,
            &[
                resp("did:a", vec![newer.clone(), older]),
                resp("did:b", vec![newer]),
            ],
        );
        assert_eq!(outcome.rows.len(), 1);
        assert_eq!(outcome.rows[0]["reading_id"], "r2");
    }

    // ── Envelope budget ───────────────────────────────────────────────────

    #[test]
    fn envelope_over_budget_trims_and_flags_truncated() {
        let rows: Vec<serde_json::Value> = (0..20_000)
            .map(|i| {
                serde_json::json!({
                    "reading_id": format!("reading-{i}"),
                    "node_id": "a",
                    "petal_id": "p1",
                    "metric": "temperature_c",
                    "value": 1.5,
                    "recorded_at_ms": i,
                    "hlc_timestamp": i,
                    "recorded_at": "2026-07-15T10:00:00Z",
                    "units": "C",
                    "source_did": "did:key:z6MkLongIdentityStringForPadding0000000000000000000",
                })
            })
            .collect();
        let mut envelope = ComputeEnvelope::Response {
            request_id: "req".into(),
            from_did: "did:a".into(),
            partial: TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated: false,
                failed: false,
            },
        };
        let bytes = encode_envelope(&mut envelope).expect("fits after trimming");
        assert!(bytes.len() <= GOSSIP_ENVELOPE_BUDGET);
        assert!(envelope_partial(&envelope).truncated, "trimming is flagged");
        assert!(!envelope_partial(&envelope).rows.is_empty());
    }

    fn envelope_partial(e: &ComputeEnvelope) -> &TsPartialRows {
        match e {
            ComputeEnvelope::Response { partial, .. } => partial,
            _ => panic!("expected response"),
        }
    }

    #[test]
    fn envelope_roundtrip() {
        let envelope = ComputeEnvelope::Request {
            request_id: "req-1".into(),
            verse_id: "v".into(),
            from_did: "did:a".into(),
            spec: agg_spec(0, 60),
            shards: vec![PartialShard {
                shard: "p1/a/0".into(),
                anchor_node_id: "a".into(),
                range_start_ms: 0,
                range_end_ms: 60_000,
            }],
            timeout_ms: 3_000,
            row_cap: 2_048,
        };
        let mut e = envelope.clone();
        let bytes = encode_envelope(&mut e).unwrap();
        let back = decode_envelope(&bytes).unwrap();
        assert_eq!(back, envelope);
    }

    // ── Shard helper sanity ────────────────────────────────────────────────

    #[test]
    fn shard_id_bucket_math_matches_ledger_ranges() {
        let shard = ShardId {
            petal_id: "p1".into(),
            anchor_node_id: "a".into(),
            bucket: ShardId::bucket_index(90_000, 60_000),
        };
        assert_eq!(shard.key(), "p1/a/1");
        let (start, end) = ShardId::bucket_range(shard.bucket, 60_000);
        assert_eq!((start, end), (60_000, 120_000));
    }

    #[test]
    fn peer_declaration_default_is_unlimited_non_seeder() {
        assert_eq!(
            PeerDeclaration::default(),
            PeerDeclaration {
                capacity_bytes: None,
                seeder: false,
            }
        );
    }

    // ── F23: envelope budget no-progress guard ─────────────────────────────

    /// One row whose serialized size alone exceeds the gossip budget — the
    /// halving loop's no-op case (len == 1 → keep.max(1) truncates nothing).
    /// Pre-F23 this looped forever; the guard must bail with an error.
    #[test]
    fn encode_envelope_bails_rather_than_looping_when_one_row_exceeds_the_budget() {
        let huge_row = serde_json::json!({
            "reading_id": "huge",
            "node_id": "a",
            "petal_id": "p1",
            "metric": "m",
            "value": 0.5,
            "payload": "x".repeat(GOSSIP_ENVELOPE_BUDGET),
        });
        // Shape 1: the huge row sits in `rows`.
        let mut rows_env = ComputeEnvelope::Response {
            request_id: "req".into(),
            from_did: "did:a".into(),
            partial: TsPartialRows {
                per_shard: Vec::new(),
                rows: vec![huge_row.clone()],
                truncated: false,
                failed: false,
            },
        };
        assert!(
            encode_envelope(&mut rows_env).is_err_and(|e| e.contains("cannot shrink")),
            "a single over-budget row must error, never loop"
        );
        // Shape 2: the huge row is the only row of the heaviest shard.
        let mut shard_env = ComputeEnvelope::Response {
            request_id: "req".into(),
            from_did: "did:a".into(),
            partial: TsPartialRows {
                per_shard: vec![shard_rows("p1/a/0", vec![huge_row])],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        assert!(
            encode_envelope(&mut shard_env).is_err_and(|e| e.contains("cannot shrink")),
            "the per-shard shape must error too, never loop"
        );
    }

    // ── F23: duplicate request ids never clobber a live collector ──────────

    #[test]
    fn pending_register_refuses_a_duplicate_id_and_respects_the_cap() {
        let pending = PendingQueries::default();
        let (tx_a, _rx_a) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
        let (tx_b, _rx_b) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
        assert!(pending.register("req-1", tx_a.clone()));
        // A duplicate id REFUSES — the live collector's inbox is never
        // replaced (pre-F23 register overwrote via map.insert).
        assert!(!pending.register("req-1", tx_b), "duplicate refused");
        // Deregister frees the id for reuse.
        pending.deregister("req-1");
        let (tx_c, _rx_c) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
        assert!(
            pending.register("req-1", tx_c),
            "id reusable after deregister"
        );
        // The cap still refuses NEW ids when full ("req-1" already holds a
        // slot, so the fill loop stops one short of the cap).
        for i in 0..(PENDING_QUERY_CAP - 1) {
            let (tx, _rx) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
            assert!(pending.register(&format!("cap-{i}"), tx));
        }
        let (tx_over, _rx_over) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
        assert!(!pending.register("over-cap", tx_over), "cap enforced");
    }

    // ── F23: a failed local partial never masquerades as covered-empty ──────

    #[test]
    fn a_failed_formal_host_partial_is_reported_missing_not_covered() {
        // The M2 scrutiny F7 minor shape: a formal host whose partial
        // execution FAILED (DB error) used to answer empty and the merge
        // counted it as an authoritative covered-empty. With the failed
        // flag, the shard is honestly MISSING and the host lands in
        // missing_hosts — A16 reads "host failed", never "covered".
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings::default(),
            &[("p1/a/0", 0, 60_000, &["did:failed-host"])],
        );
        let plan = plan_distributed_query(&agg_spec(0, 60_000), &fabric, "did:local").unwrap();
        let failed = PartialResponse {
            host: "did:failed-host".into(),
            partial: TsPartialRows::failed(),
        };
        let outcome = merge_partials(&agg_spec(0, 60_000), &plan, &[failed]);
        assert!(
            outcome.meta.covered_shards.is_empty(),
            "a failed answer covers nothing"
        );
        assert_eq!(outcome.meta.missing_shards, vec!["p1/a/0".to_string()]);
        assert_eq!(
            outcome.meta.missing_hosts,
            vec!["did:failed-host".to_string()]
        );
        assert!(
            !outcome
                .meta
                .answered_hosts
                .contains(&"did:failed-host".to_string()),
            "a failed host is not reported as having answered"
        );
        assert!(outcome.rows.is_empty());
    }

    #[test]
    fn a_failed_formal_host_loses_to_a_surviving_mirror_at_r2() {
        // At R=2 the surviving mirror's usable answer covers the shard even
        // when the other formal host failed — the honest mixed case.
        let fabric = fabric_with_shards(
            VerseTimeseriesSettings {
                mode: TimeseriesMode::Balanced,
                replication_factor: 2,
                bucket_width_ms: 60_000,
            },
            &[("p1/a/0", 0, 60_000, &["did:failed-host", "did:mirror"])],
        );
        let plan = plan_distributed_query(&agg_spec(0, 60_000), &fabric, "did:local").unwrap();
        let failed = PartialResponse {
            host: "did:failed-host".into(),
            partial: TsPartialRows::failed(),
        };
        let mirror = PartialResponse {
            host: "did:mirror".into(),
            partial: TsPartialRows {
                per_shard: vec![shard_rows(
                    "p1/a/0",
                    vec![agg_row("a", 30.0, 2, 10.0, 20.0)],
                )],
                rows: Vec::new(),
                truncated: false,
                failed: false,
            },
        };
        let outcome = merge_partials(&agg_spec(0, 60_000), &plan, &[failed, mirror]);
        assert_eq!(outcome.meta.covered_shards, vec!["p1/a/0".to_string()]);
        assert!(outcome.meta.missing_shards.is_empty());
        assert_eq!(outcome.meta.answered_hosts, vec!["did:mirror".to_string()]);
        // The failed host honestly did NOT answer — it is missing, while its
        // covered shard is served by the surviving mirror.
        assert_eq!(
            outcome.meta.missing_hosts,
            vec!["did:failed-host".to_string()]
        );
        assert_eq!(outcome.rows.len(), 1);
        assert_eq!(outcome.rows[0]["avg_value"].as_f64(), Some(15.0));
    }

    // ── F23: transport admission control ───────────────────────────────────

    /// A deterministic test node identity — its did:key IS the app DID of
    /// the peer that "sent" the envelope (the F20 alignment).
    fn test_node_id(seed: u8) -> iroh::NodeId {
        iroh::SecretKey::from_bytes(&[seed; 32]).public()
    }

    fn request_envelope(
        request_id: &str,
        verse_id: &str,
        from_did: &str,
        spec: TsQueryKind,
        shards: Vec<PartialShard>,
    ) -> ComputeEnvelope {
        ComputeEnvelope::Request {
            request_id: request_id.into(),
            verse_id: verse_id.into(),
            from_did: from_did.into(),
            spec,
            shards,
            timeout_ms: 3_000,
            row_cap: 0,
        }
    }

    fn response_envelope(
        request_id: &str,
        from_did: &str,
        partial: TsPartialRows,
    ) -> ComputeEnvelope {
        ComputeEnvelope::Response {
            request_id: request_id.into(),
            from_did: from_did.into(),
            partial,
        }
    }

    fn gossip_incoming(
        verse_id: &str,
        from: iroh::NodeId,
        direct: bool,
        envelope: &ComputeEnvelope,
    ) -> GossipIncoming {
        GossipIncoming {
            verse_id: verse_id.into(),
            from,
            direct,
            content: bytes::Bytes::from(serde_json::to_vec(envelope).expect("envelope json")),
        }
    }

    /// A transport whose local peer is `did:local`, plus the verse fabric
    /// with `p1/a/0` sharded to the requester and the requester DECLARED
    /// (the honest requester shape). Returns the requester's own did:key —
    /// the identity its endpoint authenticates as.
    fn admission_setup() -> (
        DistributedTransport,
        HashMap<String, VerseFabric>,
        HashMap<String, GossipSender>,
        String,
    ) {
        let (evt_tx, _evt_rx) = crossbeam::channel::bounded(8);
        let transport = DistributedTransport::new("did:local".into(), None, evt_tx);
        let requester_did = crate::replicator::peer_did_key(&requester_node());
        let mut fabric = VerseFabric::default();
        fabric.note_verse_row(&serde_json::json!({"ts_mode": "sharded"}));
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a/0", "hosts": [requester_did.clone()],
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 60_000,
            "range_start_ms": 0, "range_end_ms": 60_000, "row_count": 1, "size_bytes": 10
        }));
        fabric.note_peer_declaration(&requester_did, PeerDeclaration::default());
        let mut fabrics = HashMap::new();
        fabrics.insert("v-1".to_string(), fabric);
        (transport, fabrics, HashMap::new(), requester_did)
    }

    /// The node whose did:key the fixture fabric declares as a peer.
    fn requester_node() -> iroh::NodeId {
        test_node_id(7)
    }

    fn agg_shards() -> Vec<PartialShard> {
        vec![PartialShard {
            shard: "p1/a/0".into(),
            anchor_node_id: "a".into(),
            range_start_ms: 0,
            range_end_ms: 60_000,
        }]
    }

    #[test]
    fn non_compute_gossip_is_ignored() {
        let (transport, fabrics, senders, _requester_did) = admission_setup();
        let incoming = GossipIncoming {
            verse_id: "v-1".into(),
            from: test_node_id(7),
            direct: true,
            content: bytes::Bytes::from_static(b"{\"tileset\":\"ad\"}"),
        };
        assert_eq!(
            handle_gossip_incoming(&transport, &fabrics, &senders, incoming),
            GossipDisposition::Ignored
        );
    }

    #[test]
    fn request_verse_mismatch_is_dropped() {
        // The envelope claims verse v-OTHER but arrived on v-1's topic —
        // the arrival topic is the only authenticated verse binding.
        let (transport, fabrics, senders, requester_did) = admission_setup();
        let from = test_node_id(7);
        let env = request_envelope(
            "req-1",
            "v-OTHER",
            &requester_did,
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", from, true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::VERSE_MISMATCH)
        );
    }

    #[test]
    fn direct_request_with_forged_from_did_is_dropped() {
        // Direct delivery: the authenticated sender's did:key is the
        // declared requester's, but the envelope claims somebody else —
        // forged attribution (the request-impersonation shape).
        let (transport, fabrics, senders, requester_did) = admission_setup();
        let from = requester_node();
        assert_eq!(requester_did, crate::replicator::peer_did_key(&from));
        let env = request_envelope(
            "req-1",
            "v-1",
            "did:key:z6MkSomebodyElse",
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_ne!(requester_did, "did:key:z6MkSomebodyElse");
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", from, true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::FORGED_SENDER)
        );
    }

    #[test]
    fn request_from_an_undeclared_peer_is_dropped() {
        // The identity claim is honest (direct, from_did == did_key(from))
        // but the requester never declared itself in the fabric —
        // deny-by-default for strangers on the derivable topic.
        let (transport, fabrics, senders, _declared_requester) = admission_setup();
        let from = test_node_id(9);
        let requester_did = crate::replicator::peer_did_key(&from);
        let env = request_envelope(
            "req-1",
            "v-1",
            &requester_did,
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", from, true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::UNDECLARED_REQUESTER)
        );
    }

    #[test]
    fn honest_direct_request_from_a_declared_peer_is_admitted() {
        // Every gate passes: verse matches the arrival topic, from_did is
        // the authenticated sender's did:key, the requester is declared,
        // and the aggregate shard list sits inside the spec's petal.
        // Reaching the (empty) gossip-sender lookup — TOPIC_CLOSED —
        // proves admission passed; only the un-fakeable GossipSender
        // stands between here and the responder spawn.
        let (transport, fabrics, senders, requester_did) = admission_setup();
        let from = requester_node();
        let env = request_envelope(
            "req-1",
            "v-1",
            &requester_did,
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", from, true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::TOPIC_CLOSED)
        );
    }

    #[test]
    fn relayed_request_falls_back_to_claim_admission() {
        // A relayed delivery's `from` is the FORWARDING neighbor, not the
        // author (iroh-gossip GossipEvent docs) — the identity gate is
        // skipped and the claim walks the membership + petal gates. This
        // is the documented residual: relays cannot be wire-verified.
        let (transport, fabrics, senders, requester_did) = admission_setup();
        let relayed_via = test_node_id(21); // NOT the author
        let env = request_envelope(
            "req-1",
            "v-1",
            &requester_did, // the author's claim; relayed, so not checked against `from`
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", relayed_via, false, &env)
            ),
            GossipDisposition::Dropped(drop_reason::TOPIC_CLOSED),
            "a relayed honest-claim request from a declared peer passes the gates"
        );
    }

    #[test]
    fn self_echoed_request_is_dropped() {
        let (transport, fabrics, senders, _requester_did) = admission_setup();
        let env = request_envelope(
            "req-1",
            "v-1",
            "did:local",
            agg_spec(0, 60_000),
            agg_shards(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", test_node_id(7), true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::SELF_ECHO)
        );
    }

    #[test]
    fn aggregate_shard_list_outside_the_petal_is_dropped() {
        let (transport, fabrics, senders, requester_did) = admission_setup();
        let from = requester_node();
        let env = request_envelope(
            "req-1",
            "v-1",
            &requester_did,
            agg_spec(0, 60_000), // spec's petal is p1
            vec![PartialShard {
                shard: "p2/a/0".into(), // ...but the shard list names p2
                anchor_node_id: "a".into(),
                range_start_ms: 0,
                range_end_ms: 60_000,
            }],
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", from, true, &env)
            ),
            GossipDisposition::Dropped(drop_reason::SHARD_LIST_MISMATCH)
        );
    }

    #[test]
    fn forged_direct_response_is_dropped_an_honest_one_routes() {
        // The forge-another-host's-partial hole: a direct response claims
        // to be `did:key:z6MkFormalHost` but the authenticated sender is
        // somebody else — dropped, never routed into the collector. The
        // honest claim (from_did == did_key(from)) routes by request id.
        let (transport, fabrics, senders, _requester_did) = admission_setup();
        let (inbox_tx, mut inbox_rx) = tokio::sync::mpsc::channel::<RoutedResponse>(8);
        assert!(transport.pending.register("req-live", inbox_tx));

        let attacker = test_node_id(31);
        let forged =
            response_envelope("req-live", "did:key:z6MkFormalHost", TsPartialRows::empty());
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", attacker, true, &forged)
            ),
            GossipDisposition::Dropped(drop_reason::FORGED_SENDER)
        );
        assert!(
            inbox_rx.try_recv().is_err(),
            "the forged partial never reaches the collector"
        );

        let honest_host = test_node_id(7);
        let honest_did = crate::replicator::peer_did_key(&honest_host);
        let honest = response_envelope(
            "req-live",
            &honest_did,
            TsPartialRows {
                per_shard: Vec::new(),
                rows: vec![serde_json::json!({"reading_id": "r1"})],
                truncated: false,
                failed: false,
            },
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", honest_host, true, &honest)
            ),
            GossipDisposition::Routed
        );
        let routed = inbox_rx
            .try_recv()
            .expect("the honest partial routes to the live collector");
        assert_eq!(routed.host, honest_did);
        assert_eq!(routed.partial.rows.len(), 1);
    }

    #[test]
    fn response_for_an_unknown_request_id_is_routed_and_dropped_by_the_registry() {
        // The routing table keys by request id: a foreign id routes to
        // nothing (debug log inside route, disposition stays Routed —
        // the envelope itself was well-formed and honestly attributed).
        let (transport, fabrics, senders, _requester_did) = admission_setup();
        let host = test_node_id(7);
        let env = response_envelope(
            "req-unknown",
            &crate::replicator::peer_did_key(&host),
            TsPartialRows::empty(),
        );
        assert_eq!(
            handle_gossip_incoming(
                &transport,
                &fabrics,
                &senders,
                gossip_incoming("v-1", host, true, &env)
            ),
            GossipDisposition::Routed
        );
    }
}
