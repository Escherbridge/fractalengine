//! Scripted deterministic scenarios (F8/A19, F9/A21): a [`ScenarioScript`]
//! is one declarative document — a fleet config, optional verse timeseries
//! fabric settings, and an event script (faults + distributed queries at
//! simulated times). [`run_scenario`] plays it against real in-process peers
//! on the virtual transport and returns a canonical [`ScenarioOutcome`].
//!
//! Determinism model (why the same script always yields the same outcome):
//!
//! * Every value is a pure function of (config, tick) — `sensors.rs` — and
//!   the fire schedule itself is a pure function of the config —
//!   `fleet::plan_fleet`. No RNG state anywhere; peer identities are keyed
//!   derivations of `(seed, name)` (`peer::identity_seed`).
//! * The scenario clock only moves when the driver moves it, and the
//!   driver awaits every DB reply AND the hub write of every ingested row
//!   before its next step, so each stamp and each delivery due-time is a
//!   fixed simulated millisecond.
//! * The hub's delivery order comes from a (due_ms, seq) heap, never from
//!   thread scheduling. Where real threads do interleave (the sync
//!   threads' pumps), the runner waits on EXACT convergence targets (what
//!   each peer was delivered, filtered by its shard retention) before any
//!   fault or query, so no action races replication.
//! * Deliberately excluded from the fingerprint: `reading_id` (a fresh
//!   server-side ULID per ingest), DB node/petal ids (fresh ULIDs — mapped
//!   to anchor names), and the HLC counter bits. See `AGENTS.md`
//!   §scenario-runner for what each query fingerprint carries.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use fe_runtime::distributed_query::{
    DistributedQueryCall, DistributedQueryOutcome, DistributedQueryRequest, TsQueryKind,
};
use fe_runtime::messages::{DbCommand, DbResult};
use fe_runtime::timeseries::VerseTimeseriesSettings;
use fe_sync::messages::{SyncCommand, SyncEvent};
use fe_sync::{Retention, ShardId, SHARD_TABLE};
use serde::{Deserialize, Serialize};

use crate::clock::{install_hlc_source, uninstall_hlc_source, SimClock};
use crate::fleet::{FleetConfig, ScheduledReading};
use crate::net::SimNet;
use crate::peer::SimPeer;

/// How long a settle poll may take before the scenario fails (real time —
/// it bounds *waiting*, never what converges).
pub(crate) const SETTLE_BUDGET: Duration = Duration::from_secs(30);
/// Real-time slice between settle polls (lets the sync pumps drain).
pub(crate) const SETTLE_POLL: Duration = Duration::from_millis(10);
/// DB-thread reply budget (matches the harness scenarios).
pub(crate) const DB_REPLY_BUDGET: Duration = Duration::from_secs(30);

/// Verse timeseries fabric settings a script publishes on the verse
/// manifest (`ts_mode` / `ts_replication_factor` / `ts_bucket_width_ms`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeseriesSpec {
    /// `mirror` | `sharded` | `balanced`.
    pub mode: String,
    /// R for `balanced` (ignored by `mirror`/`sharded`).
    #[serde(default = "default_replication_factor")]
    pub replication_factor: u32,
    /// Shard bucket width (simulated ms).
    pub bucket_width_ms: u64,
}

fn default_replication_factor() -> u32 {
    1
}

impl TimeseriesSpec {
    /// The sanitized settings (the same validation the DB handler applies).
    pub fn settings(&self) -> Result<VerseTimeseriesSettings> {
        VerseTimeseriesSettings::sanitized(
            &self.mode,
            self.replication_factor,
            self.bucket_width_ms,
        )
        .map_err(|e| anyhow::anyhow!("invalid timeseries settings: {e}"))
    }
}

/// A distributed query a script submits (`SyncCommand::SubmitComputeTask`
/// on the named peer). Window bounds are offsets from the fleet's
/// `start_ms`; the petal is the fleet's petal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimQuery {
    /// avg/min/max/count of `metric` per anchor over `[start_ms, end_ms)`.
    WindowAggregate {
        metric: String,
        start_ms: u64,
        end_ms: u64,
    },
    /// Raw rows of `metric` in `[start_ms, end_ms)`.
    ReadingsInWindow {
        metric: String,
        start_ms: u64,
        end_ms: u64,
    },
    /// Latest reading per (anchor, metric).
    LatestPerAnchor {
        #[serde(default)]
        metric: Option<String>,
    },
    /// Every reading of the petal.
    AllReadings,
}

impl SimQuery {
    /// Whether the merge is the per-shard aggregate (early-settling) shape.
    pub fn is_aggregate(&self) -> bool {
        matches!(self, Self::WindowAggregate { .. })
    }

    fn window(&self) -> Option<(u64, u64)> {
        match self {
            Self::WindowAggregate {
                start_ms, end_ms, ..
            }
            | Self::ReadingsInWindow {
                start_ms, end_ms, ..
            } => Some((*start_ms, *end_ms)),
            Self::LatestPerAnchor { .. } | Self::AllReadings => None,
        }
    }

    /// The wire spec against the run's petal (offsets → absolute ms).
    fn to_kind(&self, petal_id: &str, origin_ms: u64) -> TsQueryKind {
        let abs = |offset: u64| origin_ms.saturating_add(offset) as i64;
        let petal_id = petal_id.to_string();
        match self {
            Self::WindowAggregate {
                metric,
                start_ms,
                end_ms,
            } => TsQueryKind::WindowAggregate {
                metric: metric.clone(),
                start_ms: abs(*start_ms),
                end_ms: abs(*end_ms),
                petal_id,
            },
            Self::ReadingsInWindow {
                metric,
                start_ms,
                end_ms,
            } => TsQueryKind::ReadingsInWindow {
                metric: metric.clone(),
                start_ms: abs(*start_ms),
                end_ms: abs(*end_ms),
                petal_id,
            },
            Self::LatestPerAnchor { metric } => TsQueryKind::LatestPerAnchor {
                petal_id,
                metric: metric.clone(),
            },
            Self::AllReadings => TsQueryKind::AllReadings { petal_id },
        }
    }
}

fn default_query_timeout_ms() -> u64 {
    fe_sync::distributed_query::DEFAULT_QUERY_TIMEOUT_MS
}

/// One scripted event at a simulated offset (`at_ms` counts from the
/// fleet's `start_ms`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScriptedEvent {
    /// Take a peer offline (its writes stop reaching the swarm until it
    /// returns; nothing is delivered to it in the meantime).
    PeerOffline { at_ms: u64, peer: String },
    /// Bring a peer back — the hub converges the doc state to every
    /// linkable subscriber (the rejoin convergence a real swarm performs).
    PeerOnline { at_ms: u64, peer: String },
    /// Partition the network into peer-name groups. Any pair split across
    /// groups of an active partition cannot exchange deliveries.
    Partition {
        at_ms: u64,
        groups: Vec<Vec<String>>,
    },
    /// Heal every active partition (convergence replay across healed links).
    Heal { at_ms: u64 },
    /// Set the per-link one-way delivery latency (simulated ms).
    SetLatency { at_ms: u64, latency_ms: u64 },
    /// Submit a distributed query on `peer` (fans out over the hub's gossip
    /// plane); the merged outcome is recorded under `label`.
    Query {
        at_ms: u64,
        peer: String,
        label: String,
        query: SimQuery,
        /// Per-host answer deadline (real ms — the collector's deadline).
        #[serde(default = "default_query_timeout_ms")]
        timeout_ms: u64,
    },
}

impl ScriptedEvent {
    fn at_ms(&self) -> u64 {
        match self {
            Self::PeerOffline { at_ms, .. }
            | Self::PeerOnline { at_ms, .. }
            | Self::Partition { at_ms, .. }
            | Self::Heal { at_ms }
            | Self::SetLatency { at_ms, .. }
            | Self::Query { at_ms, .. } => *at_ms,
        }
    }
}

/// A whole scenario: the fleet + the event script. Declarative JSON (see
/// `fe-sim/scenarios/` and the `fe-sim` bin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioScript {
    pub name: String,
    /// Identity seed: peer keypairs derive from `(seed, peer name)`.
    #[serde(default)]
    pub seed: u64,
    /// Verse timeseries fabric settings (absent = mirror defaults).
    #[serde(default)]
    pub timeseries: Option<TimeseriesSpec>,
    pub fleet: FleetConfig,
    #[serde(default)]
    pub events: Vec<ScriptedEvent>,
}

impl ScenarioScript {
    /// Parse a scenario from declarative JSON.
    pub fn parse(json: &str) -> Result<Self> {
        let script: Self =
            serde_json::from_str(json).map_err(|e| anyhow::anyhow!("invalid scenario: {e}"))?;
        script.validate()?;
        Ok(script)
    }

    /// Validate cross-references: every scripted peer name must be a fleet
    /// peer, query labels unique with sane windows/deadlines, timeseries
    /// settings valid. Event order is NOT required (the driver sorts).
    pub fn validate(&self) -> Result<()> {
        self.fleet.validate()?;
        if let Some(ts) = &self.timeseries {
            ts.settings()?;
        }
        let mut labels: BTreeSet<&str> = BTreeSet::new();
        for event in &self.events {
            self.validate_event_peers(event)?;
            match event {
                ScriptedEvent::PeerOffline { .. }
                | ScriptedEvent::PeerOnline { .. }
                | ScriptedEvent::Partition { .. } => {}
                ScriptedEvent::Query {
                    label,
                    query,
                    timeout_ms,
                    ..
                } => {
                    if label.trim().is_empty() || label.len() > 64 {
                        anyhow::bail!("scenario {}: query label must be 1..=64 chars", self.name);
                    }
                    if !labels.insert(label.as_str()) {
                        anyhow::bail!("scenario {}: duplicate query label '{label}'", self.name);
                    }
                    if *timeout_ms == 0
                        || *timeout_ms > fe_sync::distributed_query::MAX_QUERY_TIMEOUT_MS
                    {
                        anyhow::bail!(
                            "scenario {}: query '{label}' timeout_ms must be 1..={}",
                            self.name,
                            fe_sync::distributed_query::MAX_QUERY_TIMEOUT_MS
                        );
                    }
                    if let Some((start, end)) = query.window() {
                        if start >= end {
                            anyhow::bail!(
                                "scenario {}: query '{label}' window must have start < end",
                                self.name
                            );
                        }
                    }
                }
                ScriptedEvent::Heal { .. } | ScriptedEvent::SetLatency { .. } => {}
            }
        }
        let max_latency = self
            .events
            .iter()
            .filter_map(|e| match e {
                ScriptedEvent::SetLatency { latency_ms, .. } => Some(*latency_ms),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        self.check_time_limit(max_latency)
    }

    /// The furthest simulated time this script can reach — the fleet end,
    /// every event, every query window, plus `latency_ms` of in-flight drain
    /// — must stay below 2^48 ms (DEC-C13: HLC packs wall time into 48
    /// bits). Shared with interactive `SetLatency` injection.
    pub fn check_time_limit(&self, latency_ms: u64) -> Result<()> {
        let mut horizon = self.fleet.duration_ms;
        for event in &self.events {
            horizon = horizon.max(event.at_ms());
            if let ScriptedEvent::Query { query, .. } = event {
                if let Some((_, end)) = query.window() {
                    horizon = horizon.max(end);
                }
            }
        }
        match self
            .fleet
            .start_ms
            .checked_add(horizon)
            .and_then(|t| t.checked_add(latency_ms))
        {
            Some(t) if t < crate::fleet::SIM_TIME_LIMIT_MS => Ok(()),
            _ => anyhow::bail!(
                "scenario {}: events/latency push simulated time past the 2^48 ms limit",
                self.name
            ),
        }
    }

    /// Every peer name `event` references must be a fleet peer (shared by
    /// script validation and interactive fault injection).
    pub fn validate_event_peers(&self, event: &ScriptedEvent) -> Result<()> {
        let known = |peer: &String| self.fleet.peers.contains(peer);
        match event {
            ScriptedEvent::PeerOffline { peer, .. } | ScriptedEvent::PeerOnline { peer, .. } => {
                if !known(peer) {
                    anyhow::bail!("scenario {}: event names unknown peer '{peer}'", self.name);
                }
            }
            ScriptedEvent::Partition { groups, .. } => {
                for peer in groups.iter().flatten() {
                    if !known(peer) {
                        anyhow::bail!(
                            "scenario {}: partition names unknown peer '{peer}'",
                            self.name
                        );
                    }
                }
            }
            ScriptedEvent::Query { peer, .. } => {
                if !known(peer) {
                    anyhow::bail!("scenario {}: query names unknown peer '{peer}'", self.name);
                }
            }
            ScriptedEvent::Heal { .. } | ScriptedEvent::SetLatency { .. } => {}
        }
        Ok(())
    }
}

/// One reading as the fingerprint holds it: anchor NAME (fleet-local
/// identity, stable across runs — DB node ids are fresh ULIDs), the
/// metric/units, the sensor timestamp, the exact value bits, and the HLC
/// wall bits (the simulated millisecond the ingest stamped).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CanonicalReading {
    pub anchor: String,
    pub metric: String,
    pub units: String,
    pub recorded_at_ms: i64,
    pub value_bits: u64,
    pub hlc_wall_ms: u64,
}

/// One merged query row with run-local ids mapped to fleet names.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum CanonicalQueryRow {
    /// A `WindowAggregate` row.
    Aggregate {
        anchor: String,
        metric: String,
        sample_count: u64,
        avg: f64,
        min: f64,
        max: f64,
    },
    /// A raw / latest reading row.
    Reading {
        anchor: String,
        metric: String,
        recorded_at_ms: i64,
        value: f64,
    },
}

impl CanonicalQueryRow {
    fn sort_key(&self) -> (&str, &str, i64) {
        match self {
            Self::Aggregate { anchor, metric, .. } => (anchor, metric, i64::MIN),
            Self::Reading {
                anchor,
                metric,
                recorded_at_ms,
                ..
            } => (anchor, metric, *recorded_at_ms),
        }
    }
}

/// A merged distributed-query outcome in fleet vocabulary: shards are
/// `{anchor}/{bucket}`, hosts are fleet peer names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CanonicalQueryOutcome {
    pub rows: Vec<CanonicalQueryRow>,
    pub covered_shards: Vec<String>,
    pub missing_shards: Vec<String>,
    pub answered_hosts: Vec<String>,
    pub missing_hosts: Vec<String>,
    pub mode: String,
    pub truncated: bool,
    pub error: Option<String>,
}

/// One scripted query's result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryRecord {
    pub label: String,
    /// The fleet peer that submitted it.
    pub peer: String,
    /// Simulated offset (from `start_ms`) it ran at.
    pub at_ms: u64,
    pub aggregate: bool,
    pub outcome: CanonicalQueryOutcome,
}

/// The determinism-contract slice of a [`QueryRecord`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryFingerprint {
    pub label: String,
    pub rows: Vec<CanonicalQueryRow>,
    pub covered_shards: Vec<String>,
    pub missing_shards: Vec<String>,
    /// `(answered, missing)` hosts — `None` for a fully-covered aggregate,
    /// whose early settle makes the host lists arrival-dependent (§scenario-runner).
    pub hosts: Option<(Vec<String>, Vec<String>)>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl QueryRecord {
    /// The deterministic slice (see [`QueryFingerprint::hosts`]).
    pub fn fingerprint(&self) -> QueryFingerprint {
        let early_settle = self.aggregate
            && self.outcome.missing_shards.is_empty()
            && self.outcome.error.is_none();
        QueryFingerprint {
            label: self.label.clone(),
            rows: self.outcome.rows.clone(),
            covered_shards: self.outcome.covered_shards.clone(),
            missing_shards: self.outcome.missing_shards.clone(),
            hosts: (!early_settle).then(|| {
                (
                    self.outcome.answered_hosts.clone(),
                    self.outcome.missing_hosts.clone(),
                )
            }),
            truncated: self.outcome.truncated,
            error: self.outcome.error.clone(),
        }
    }
}

/// Everything a determinism assertion compares across two runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScenarioFingerprint {
    pub per_peer: BTreeMap<String, Vec<CanonicalReading>>,
    pub placement: BTreeMap<String, Vec<String>>,
    pub queries: Vec<QueryFingerprint>,
}

/// The canonical outcome of one scenario run.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioOutcome {
    pub name: String,
    /// Total readings the ingest host durably wrote.
    pub ingested: usize,
    /// Per-peer canonical readings (sorted), keyed by fleet peer name.
    pub per_peer: BTreeMap<String, Vec<CanonicalReading>>,
    /// Final shard placement from the ingest host's ledger:
    /// `{anchor}/{bucket}` → hosting peer names (sorted).
    pub placement: BTreeMap<String, Vec<String>>,
    /// Scripted query results, in execution order.
    pub queries: Vec<QueryRecord>,
    /// Fleet peer name → DID (seeded, stable across runs).
    pub peer_dids: BTreeMap<String, String>,
    /// Hub deliveries dropped at drain time (link-down losses + channel
    /// backpressure) — diagnostics, not part of the determinism contract.
    pub dropped_deliveries: u64,
    /// Gossip frames the hub delivered (> 0 ⇔ the compute plane was used).
    pub gossip_deliveries: u64,
    /// `fe_sync::bound_endpoint_count()` before/after the run: a sim
    /// scenario binds no iroh endpoint, so these must be equal (A19's
    /// "no real network" clause).
    pub endpoints_before: u64,
    pub endpoints_after: u64,
}

impl ScenarioOutcome {
    /// The per-peer readings fingerprint (F8's determinism contract).
    pub fn fingerprint(&self) -> &BTreeMap<String, Vec<CanonicalReading>> {
        &self.per_peer
    }

    /// The full determinism contract: readings + placement + queries.
    pub fn canonical_fingerprint(&self) -> ScenarioFingerprint {
        ScenarioFingerprint {
            per_peer: self.per_peer.clone(),
            placement: self.placement.clone(),
            queries: self.queries.iter().map(QueryRecord::fingerprint).collect(),
        }
    }

    /// The recorded query with `label`.
    pub fn query(&self, label: &str) -> Option<&QueryRecord> {
        self.queries.iter().find(|q| q.label == label)
    }
}

/// Installs the session's HLC source and, on drop (stop, error, or
/// abandonment), uninstalls it and restores the pre-session HLC state —
/// both are process-global (AGENTS.md §hlc-sim).
pub(crate) struct HlcSourceGuard {
    snapshot: fe_database::op_log::HlcSnapshot,
}

impl HlcSourceGuard {
    /// Snapshot the process HLC, THEN install `clock` as its source.
    pub(crate) fn install(clock: Arc<SimClock>) -> Self {
        let snapshot = fe_database::op_log::snapshot_hlc();
        install_hlc_source(clock);
        Self { snapshot }
    }
}

impl Drop for HlcSourceGuard {
    fn drop(&mut self) {
        // Uninstall first: restore must not observe the sim source.
        uninstall_hlc_source();
        fe_database::op_log::restore_hlc(self.snapshot);
    }
}

/// One merged driver action in total `(at_ms, event-before-tick)` order.
/// Owned (not borrowed from the script/plan) so a long-lived
/// [`crate::session::ScenarioSession`] can hold its action list.
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// A scripted event at its ABSOLUTE simulated time.
    Event { at_ms: u64, event: ScriptedEvent },
    /// All readings scheduled at this instant.
    Tick {
        at_ms: u64,
        readings: Vec<ScheduledReading>,
    },
}

impl Action {
    pub(crate) fn at_ms(&self) -> u64 {
        match self {
            Self::Event { at_ms, .. } | Self::Tick { at_ms, .. } => *at_ms,
        }
    }
}

/// Merge the fire plan and the event script into one ordered action list
/// (events land before same-instant ticks). Event offsets become absolute
/// simulated times here.
pub(crate) fn merge_actions(script: &ScenarioScript, plan: &[ScheduledReading]) -> Vec<Action> {
    let origin = script.fleet.start_ms;
    let mut events: Vec<(u64, &ScriptedEvent)> = script
        .events
        .iter()
        .map(|e| (origin.saturating_add(e.at_ms()), e))
        .collect();
    events.sort_by_key(|(at, _)| *at); // stable: same-instant events keep script order
    let mut ticks: Vec<(u64, Vec<ScheduledReading>)> = Vec::new();
    for reading in plan {
        match ticks.last_mut() {
            Some((at, list)) if *at == reading.at_ms => list.push(reading.clone()),
            _ => ticks.push((reading.at_ms, vec![reading.clone()])),
        }
    }
    let mut actions: Vec<Action> = Vec::with_capacity(events.len() + ticks.len());
    let (mut e, mut t) = (0usize, 0usize);
    while e < events.len() || t < ticks.len() {
        let take_event = match (events.get(e), ticks.get(t)) {
            (Some((ea, _)), Some((ta, _))) => ea <= ta,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if take_event {
            let (at_ms, event) = events[e];
            actions.push(Action::Event {
                at_ms,
                event: event.clone(),
            });
            e += 1;
        } else {
            let (at, list) = &ticks[t];
            actions.push(Action::Tick {
                at_ms: *at,
                readings: list.clone(),
            });
            t += 1;
        }
    }
    actions
}

/// Run one scripted scenario under `root_dir` (each peer gets its own
/// subdirectory; use a fresh directory per run — two runs sharing
/// directories would collide on peer names).
///
/// The one-shot leg of the ONE driver: start a
/// [`ScenarioSession`](crate::session::ScenarioSession), step every action,
/// stop (AGENTS.md §session — the interactive surface cannot drift from it).
pub fn run_scenario(script: &ScenarioScript, root_dir: &Path) -> Result<ScenarioOutcome> {
    let mut session = crate::session::ScenarioSession::start(script.clone(), root_dir)?;
    session.step(u32::MAX)?;
    session.stop()
}

/// The live state of one run the driver's barriers and queries share.
pub(crate) struct Run {
    pub(crate) clock: Arc<SimClock>,
    pub(crate) net: Arc<SimNet>,
    pub(crate) peers: BTreeMap<String, SimPeer>,
    pub(crate) host_name: String,
    pub(crate) verse_id: String,
    pub(crate) petal_id: String,
    /// The fleet's `start_ms` (query window offsets are relative to it).
    pub(crate) origin_ms: u64,
    pub(crate) settings: VerseTimeseriesSettings,
    /// Readings the ingest host durably wrote so far.
    pub(crate) ingested: usize,
    pub(crate) did_to_name: HashMap<String, String>,
    pub(crate) node_to_anchor: HashMap<String, String>,
}

impl Run {
    pub(crate) fn peer(&self, name: &str) -> Result<&SimPeer> {
        self.peers
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown peer '{name}'"))
    }

    pub(crate) fn peer_name(&self, did: &str) -> String {
        self.did_to_name
            .get(did)
            .cloned()
            .unwrap_or_else(|| did.to_string())
    }

    pub(crate) fn anchor_name(&self, node_id: &str) -> String {
        self.node_to_anchor
            .get(node_id)
            .cloned()
            .unwrap_or_else(|| node_id.to_string())
    }

    /// `{petal}/{node}/{bucket}` → `{anchor}/{bucket}` (run-local ids out).
    pub(crate) fn canonical_shard(&self, key: &str) -> String {
        let rest = key
            .strip_prefix(&format!("{}/", self.petal_id))
            .unwrap_or(key);
        match rest.split_once('/') {
            Some((node, bucket)) => format!("{}/{bucket}", self.anchor_name(node)),
            None => rest.to_string(),
        }
    }

    /// Drop every queued sync event AND every unsolicited
    /// `ReplicatedRowApplied` DB echo: both producers block on full
    /// bounded(64) channels and nothing else consumes them in a sim run (a
    /// replica's undrained echoes deadlocked its DB thread against the
    /// driver's next command send — AGENTS.md §scenario-runner).
    ///
    /// Safe to discard DB results here by the single-driver-thread
    /// invariant: every driver DB command is sent and awaited synchronously
    /// on THIS thread (`raw_query`, the tick ingest, setup — `send` then
    /// `wait_for`), so no `wait_for` is outstanding whenever this runs, and
    /// `wait_for` itself drops non-matching results. Anything other than a
    /// `ReplicatedRowApplied` is therefore a stray — logged, never silent.
    pub(crate) fn drain_events(&self) {
        for (name, peer) in &self.peers {
            while peer.peer.sync_evt_rx.try_recv().is_ok() {}
            while let Ok(result) = peer.peer.db_result_rx.try_recv() {
                if !matches!(result, DbResult::ReplicatedRowApplied { .. }) {
                    tracing::warn!(
                        peer = %name,
                        ?result,
                        "scenario drain: stray DB result with no outstanding wait — dropped"
                    );
                }
            }
        }
    }

    /// A peer's fabric dump (`GetShardLedger` → `SyncEvent::ShardLedger`).
    pub(crate) fn ledger(&self, name: &str) -> Result<serde_json::Value> {
        let peer = &self.peer(name)?.peer;
        peer.sync_cmd_tx
            .send(SyncCommand::GetShardLedger {
                verse_id: self.verse_id.clone(),
            })
            .map_err(|e| anyhow::anyhow!("GetShardLedger send failed: {e}"))?;
        let verse_id = self.verse_id.clone();
        match peer.wait_sync_event(
            |e| matches!(e, SyncEvent::ShardLedger { verse_id: v, .. } if *v == verse_id),
            DB_REPLY_BUDGET,
        )? {
            SyncEvent::ShardLedger { ledger_json, .. } => serde_json::from_str(&ledger_json)
                .map_err(|e| anyhow::anyhow!("bad ledger JSON: {e}")),
            other => anyhow::bail!("unexpected sync event: {other:?}"),
        }
    }

    /// The ingest host's shard ledger: shard key → host DIDs. The host
    /// planned every shard, so its ledger is complete even while cut off.
    pub(crate) fn ledger_hosts(&self) -> Result<BTreeMap<String, Vec<String>>> {
        let dump = self.ledger(&self.host_name)?;
        let mut out = BTreeMap::new();
        if let Some(shards) = dump["shards"].as_object() {
            for (key, entry) in shards {
                let hosts = entry["hosts"]
                    .as_array()
                    .map(|hs| {
                        hs.iter()
                            .filter_map(|h| h.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                out.insert(key.clone(), hosts);
            }
        }
        Ok(out)
    }

    /// Poll `probe` (stepping the hub, draining events) until it passes or
    /// the budget runs out — never advances the clock.
    pub(crate) fn settle_probe<P: Fn() -> Result<bool>>(&self, what: &str, probe: P) -> Result<()> {
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            self.net.step();
            self.drain_events();
            if probe().unwrap_or(false) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("scenario settle timed out waiting for {what}");
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// Barrier: every ingested reading has reached the hub doc (the
    /// DB→bridge→sync leg is asynchronous; its write time is a due-time).
    pub(crate) fn settle_writes(&self) -> Result<()> {
        let want = self.ingested;
        self.settle_probe("ingested rows reaching the hub", || {
            Ok(self.net.entry_count("iot_reading") >= want)
        })
    }

    /// How many readings `did`'s store must hold: every row it authored,
    /// plus every row the hub delivered to it that its retention keeps
    /// (the same `retention_decision` its sync thread runs).
    pub(crate) fn expected_readings(
        &self,
        did: &str,
        hosts: &BTreeMap<String, Vec<String>>,
    ) -> usize {
        self.net
            .visible_entries(did, "iot_reading")
            .iter()
            .filter(|entry| {
                if entry.author == did {
                    return true;
                }
                let shard = serde_json::from_slice::<serde_json::Value>(&entry.data)
                    .ok()
                    .and_then(|row| ShardId::of_reading_row(&row, self.settings.bucket_width_ms));
                let Some(shard) = shard else {
                    return true; // unparseable rows retain (sync-thread parity)
                };
                fe_sync::placement::retention_decision(
                    &self.settings,
                    hosts.get(&shard.key()).map(Vec::as_slice),
                    did,
                ) == Retention::Retain
            })
            .count()
    }

    /// Barrier: every peer's store holds exactly its convergence target.
    /// With `drain_network`, in-flight deliveries are drained first by
    /// advancing the clock to their due times. Over-retention (a store
    /// ABOVE target) never settles — it fails loudly.
    pub(crate) fn settle_stores(&self, drain_network: bool, what: &str) -> Result<()> {
        self.settle_writes()?;
        let deadline = Instant::now() + SETTLE_BUDGET;
        loop {
            self.net.step();
            self.drain_events();
            if drain_network && self.net.inflight_count() > 0 {
                if let Some(due) = self.net.next_due_ms() {
                    let now = self.clock.now_ms();
                    if due > now {
                        self.clock.advance_ms(due - now);
                    }
                }
            } else {
                let hosts = self.ledger_hosts()?;
                let mut lagging: Option<String> = None;
                for (name, peer) in &self.peers {
                    let did = peer.did();
                    let expected = self.expected_readings(&did, &hosts);
                    let actual = raw_query(&peer.peer, "SELECT reading_id FROM iot_reading")?.len();
                    // The fabric must also have consumed every ledger row it
                    // was handed: a trailing `__shards` row for a shard this
                    // peer does not host leaves its store count unchanged,
                    // yet a query planned before it lands misses the shard.
                    let shards_visible = self.net.visible_entries(&did, SHARD_TABLE).len();
                    let shards_known = self.ledger(name)?["shards"]
                        .as_object()
                        .map(|s| s.len())
                        .unwrap_or(0);
                    if actual != expected || shards_known != shards_visible {
                        lagging = Some(format!(
                            "peer '{name}' holds {actual}/{expected} readings and knows \
                             {shards_known}/{shards_visible} shards"
                        ));
                        break;
                    }
                }
                match lagging {
                    None => return Ok(()),
                    Some(state) if Instant::now() >= deadline => {
                        anyhow::bail!("scenario settle timed out ({what}): {state}");
                    }
                    Some(_) => {}
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!("scenario settle timed out ({what}): network never drained");
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// Submit one distributed query through the real `SubmitComputeTask`
    /// seam and pump the hub until the merged outcome comes back.
    pub(crate) fn run_query(
        &self,
        peer_name: &str,
        label: &str,
        query: &SimQuery,
        timeout_ms: u64,
    ) -> Result<DistributedQueryOutcome> {
        let peer = &self.peer(peer_name)?.peer;
        let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
        peer.sync_cmd_tx
            .send(SyncCommand::SubmitComputeTask {
                call: DistributedQueryCall {
                    request: DistributedQueryRequest {
                        request_id: format!("sim-query-{label}"),
                        verse_id: self.verse_id.clone(),
                        spec: query.to_kind(&self.petal_id, self.origin_ms),
                        timeout_ms,
                        row_cap: 0,
                    },
                    reply: reply_tx,
                },
            })
            .map_err(|e| anyhow::anyhow!("SubmitComputeTask send failed: {e}"))?;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms) + SETTLE_BUDGET;
        loop {
            match reply_rx.try_recv() {
                Ok(outcome) => return Ok(outcome),
                Err(crossbeam::channel::TryRecvError::Disconnected) => {
                    anyhow::bail!("query '{label}': the collector dropped its reply")
                }
                Err(crossbeam::channel::TryRecvError::Empty) => {}
            }
            // Gossip frames (request, partial responses) ride the hub heap
            // like doc rows: drain what is due, advancing to latency-delayed
            // due times when nothing is due yet.
            self.net.step();
            self.drain_events();
            if let Some(due) = self.net.next_due_ms() {
                let now = self.clock.now_ms();
                if due > now {
                    self.clock.advance_ms(due - now);
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!("query '{label}': no outcome within the deadline");
            }
            std::thread::sleep(SETTLE_POLL);
        }
    }

    /// Map a wire outcome into fleet vocabulary.
    pub(crate) fn canonical_outcome(
        &self,
        outcome: &DistributedQueryOutcome,
        aggregate: bool,
    ) -> CanonicalQueryOutcome {
        let mut rows: Vec<CanonicalQueryRow> = outcome
            .rows
            .iter()
            .map(|row| {
                let anchor = self.anchor_name(row["node_id"].as_str().unwrap_or_default());
                let metric = row["metric"].as_str().unwrap_or_default().to_string();
                if aggregate {
                    CanonicalQueryRow::Aggregate {
                        anchor,
                        metric,
                        sample_count: row["sample_count"].as_u64().unwrap_or(0),
                        avg: row["avg_value"].as_f64().unwrap_or(f64::NAN),
                        min: row["min_value"].as_f64().unwrap_or(f64::NAN),
                        max: row["max_value"].as_f64().unwrap_or(f64::NAN),
                    }
                } else {
                    CanonicalQueryRow::Reading {
                        anchor,
                        metric,
                        recorded_at_ms: row["recorded_at_ms"].as_i64().unwrap_or_default(),
                        value: row["value"].as_f64().unwrap_or(f64::NAN),
                    }
                }
            })
            .collect();
        rows.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        let shards = |keys: &[String]| -> Vec<String> {
            let mut out: Vec<String> = keys.iter().map(|k| self.canonical_shard(k)).collect();
            out.sort();
            out
        };
        let names = |dids: &[String]| -> Vec<String> {
            let mut out: Vec<String> = dids.iter().map(|d| self.peer_name(d)).collect();
            out.sort();
            out
        };
        CanonicalQueryOutcome {
            rows,
            covered_shards: shards(&outcome.meta.covered_shards),
            missing_shards: shards(&outcome.meta.missing_shards),
            answered_hosts: names(&outcome.meta.answered_hosts),
            missing_hosts: names(&outcome.meta.missing_hosts),
            mode: outcome.meta.mode.clone(),
            truncated: outcome.meta.truncated,
            error: outcome.error.clone(),
        }
    }
}

/// Apply one scripted fault to the hub (mapping fleet peer names to DIDs —
/// the hub's membership keys are the DIDs the replicas author as).
pub(crate) fn apply_event(
    net: &Arc<SimNet>,
    peers: &BTreeMap<String, SimPeer>,
    event: &ScriptedEvent,
) {
    let did_of = |name: &String| {
        peers
            .get(name)
            .map(|p| p.did())
            .unwrap_or_else(|| name.clone())
    };
    match event {
        ScriptedEvent::PeerOffline { peer, .. } => {
            tracing::info!(peer, "scenario: peer offline");
            net.set_peer_online(&did_of(peer), false);
        }
        ScriptedEvent::PeerOnline { peer, .. } => {
            tracing::info!(peer, "scenario: peer back online");
            net.set_peer_online(&did_of(peer), true);
        }
        ScriptedEvent::Partition { groups, .. } => {
            let did_groups: Vec<Vec<String>> = groups
                .iter()
                .map(|group| group.iter().map(did_of).collect())
                .collect();
            net.partition(did_groups);
        }
        ScriptedEvent::Heal { .. } => {
            tracing::info!("scenario: heal");
            net.heal();
        }
        ScriptedEvent::SetLatency { latency_ms, .. } => {
            tracing::info!(latency_ms, "scenario: latency");
            net.set_latency_ms(*latency_ms);
        }
        ScriptedEvent::Query { label, .. } => {
            // Queries need the run context; the driver executes them.
            tracing::warn!(label, "query event reached the fault applier — ignored");
        }
    }
}

/// Read rows back from a peer's durable store (RawQuery — SELECT-only).
pub(crate) fn raw_query(
    peer: &fractalengine_test_harness::peer::TestPeer,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    peer.send(DbCommand::RawQuery {
        correlation_id: None,
        sql: sql.to_string(),
        vars: HashMap::new(),
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        DB_REPLY_BUDGET,
    )? {
        DbResult::QueryResult { data, .. } => Ok(data),
        other => anyhow::bail!("unexpected read-back result: {other:?}"),
    }
}

/// A ready-made two-peer script exercising the full fault surface
/// (latency → churn → partition → heal), used by the determinism proof
/// and the `fe-sim` bin's default run. Everything is healed and online
/// before the fleet ends, so the converged target is exact.
pub fn default_script() -> ScenarioScript {
    ScenarioScript::parse(
        r#"{
            "name": "sim-default",
            "fleet": {
                "verse_name": "Sim Fleet Verse",
                "peers": ["alice", "bob"],
                "ingest_peer": "alice",
                "anchors": ["tower-a", "tower-b"],
                "sensors": [
                    { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                      "cadence_ms": 60000,
                      "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                 "period_ms": 3600000 } },
                    { "anchor": "tower-a", "metric": "humidity_pct", "units": "%",
                      "cadence_ms": 60000,
                      "model": { "type": "weather", "baseline": 60, "amplitude": 15,
                                 "diurnal_ms": 86400000, "seed": 42, "jitter": 5 } },
                    { "anchor": "tower-b", "metric": "battery_v", "units": "V",
                      "cadence_ms": 60000,
                      "model": { "type": "random_walk", "start": 4.2, "step": 0.05,
                                 "seed": 7 } }
                ],
                "start_ms": 1750000000000,
                "duration_ms": 420000
            },
            "events": [
                { "kind": "set_latency", "at_ms": 60000, "latency_ms": 500 },
                { "kind": "peer_offline", "at_ms": 120000, "peer": "bob" },
                { "kind": "peer_online", "at_ms": 240000, "peer": "bob" },
                { "kind": "partition", "at_ms": 180000, "groups": [["alice"], ["bob"]] },
                { "kind": "heal", "at_ms": 300000 }
            ]
        }"#,
    )
    .expect("built-in scenario must parse")
}

/// A21 scenario 1 (checked in at `fe-sim/scenarios/sharded_query.json`):
/// three peers, a `sharded` fabric, one distributed aggregate + one raw
/// fan-out from a non-ingest peer.
pub fn sharded_query_script() -> ScenarioScript {
    ScenarioScript::parse(include_str!("../scenarios/sharded_query.json"))
        .expect("checked-in sharded_query.json must parse")
}

/// A21 scenario 2 (checked in at `fe-sim/scenarios/offline_degraded.json`):
/// the ingest host goes offline → a degraded-but-honest query → the host
/// returns → outage readings converge → a fully-covered query.
pub fn offline_degraded_script() -> ScenarioScript {
    ScenarioScript::parse(include_str!("../scenarios/offline_degraded.json"))
        .expect("checked-in offline_degraded.json must parse")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::plan_fleet;
    use crate::sensors::evaluate;

    /// Float tolerance for the MEAN only: merges sum per shard then across
    /// shards, the oracle sums in tick order — summation order, never
    /// transport loss. Counts, min/max, and raw values compare exactly.
    const EPS: f64 = 1e-9;

    /// Run `script` twice in fresh directories (the repeat-run pattern).
    fn run_twice(script: &ScenarioScript) -> (ScenarioOutcome, ScenarioOutcome) {
        let dir_a = tempfile::tempdir().expect("run A tempdir");
        let a = run_scenario(script, dir_a.path()).expect("scenario run A");
        let dir_b = tempfile::tempdir().expect("run B tempdir");
        let b = run_scenario(script, dir_b.path()).expect("scenario run B");
        (a, b)
    }

    /// `{anchor}/{bucket}` of a planned reading — the canonical shard name.
    fn shard_of(script: &ScenarioScript, reading: &ScheduledReading) -> String {
        shard_name(
            script,
            &script.fleet.sensors[reading.sensor].anchor,
            reading.at_ms as i64,
        )
    }

    /// `{anchor}/{bucket}` of a reading at `recorded_at_ms`.
    fn shard_name(script: &ScenarioScript, anchor: &str, recorded_at_ms: i64) -> String {
        let width = script
            .timeseries
            .as_ref()
            .map(|t| t.bucket_width_ms)
            .unwrap_or(fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS);
        format!("{anchor}/{}", ShardId::bucket_index(recorded_at_ms, width))
    }

    /// Per-anchor (sum, count, min, max) over the planned readings `keep`
    /// selects — the A15 oracle ("the same query over the union").
    fn oracle<'a>(
        script: &ScenarioScript,
        readings: impl Iterator<Item = &'a ScheduledReading>,
    ) -> BTreeMap<String, (f64, u64, f64, f64)> {
        let mut acc: BTreeMap<String, (f64, u64, f64, f64)> = BTreeMap::new();
        for r in readings {
            let anchor = script.fleet.sensors[r.sensor].anchor.clone();
            let e = acc
                .entry(anchor)
                .or_insert((0.0, 0, f64::INFINITY, f64::NEG_INFINITY));
            e.0 += r.value;
            e.1 += 1;
            e.2 = e.2.min(r.value);
            e.3 = e.3.max(r.value);
        }
        acc
    }

    fn assert_aggregate_matches(
        record: &QueryRecord,
        expected: &BTreeMap<String, (f64, u64, f64, f64)>,
    ) {
        assert_eq!(
            record.outcome.rows.len(),
            expected.len(),
            "{}: one merged row per anchor: {:?}",
            record.label,
            record.outcome.rows
        );
        for row in &record.outcome.rows {
            let CanonicalQueryRow::Aggregate {
                anchor,
                sample_count,
                avg,
                min,
                max,
                ..
            } = row
            else {
                panic!("{}: non-aggregate row {row:?}", record.label);
            };
            let (sum, count, want_min, want_max) = expected[anchor];
            assert_eq!(*sample_count, count, "{}: {anchor} count", record.label);
            // Only the mean carries summation-order error; min/max select a
            // bit-exact replicated value, so they compare exactly.
            assert!(
                (avg - sum / count as f64).abs() < EPS,
                "{}: {anchor} avg",
                record.label
            );
            assert_eq!(
                min.to_bits(),
                want_min.to_bits(),
                "{}: {anchor} min {min} vs {want_min}",
                record.label
            );
            assert_eq!(
                max.to_bits(),
                want_max.to_bits(),
                "{}: {anchor} max {max} vs {want_max}",
                record.label
            );
        }
    }

    /// (anchor, recorded_at) → value bits of the ingest host's ORIGIN rows.
    fn origin_bits(outcome: &ScenarioOutcome, host: &str) -> BTreeMap<(String, i64), u64> {
        outcome.per_peer[host]
            .iter()
            .map(|r| ((r.anchor.clone(), r.recorded_at_ms), r.value_bits))
            .collect()
    }

    /// A replica's store holds exactly the readings of the shards it hosts,
    /// each bit-identical to the origin row (DEC-C6: bit-exact replication).
    fn assert_store_is_its_hosted_shards(
        script: &ScenarioScript,
        outcome: &ScenarioOutcome,
        peer: &str,
    ) {
        let origin = origin_bits(outcome, &script.fleet.ingest_peer);
        let plan = plan_fleet(&script.fleet);
        let hosted = plan
            .iter()
            .filter(|r| outcome.placement[&shard_of(script, r)] == [peer.to_string()])
            .count();
        let store = &outcome.per_peer[peer];
        assert_eq!(
            store.len(),
            hosted,
            "{peer} retains exactly its hosted shards"
        );
        for reading in store {
            // Membership, not just counts: every held reading belongs to a
            // shard this peer hosts (a swapped hosted/foreign pair would
            // keep the count and still pass the bits check).
            let shard = shard_name(script, &reading.anchor, reading.recorded_at_ms);
            assert!(
                outcome
                    .placement
                    .get(&shard)
                    .is_some_and(|hosts| hosts.iter().any(|h| h == peer)),
                "{peer} holds {reading:?} of shard {shard}, which it does not host"
            );
            assert_eq!(
                origin.get(&(reading.anchor.clone(), reading.recorded_at_ms)),
                Some(&reading.value_bits),
                "{peer}'s replica of {reading:?} must be bit-identical to the origin row"
            );
        }
    }

    /// A19: the same script, run twice, produces the same canonical
    /// fingerprint on every peer — and binds no iroh endpoint either time.
    #[test]
    fn scripted_scenario_is_deterministic_with_no_real_network() {
        let script = default_script();
        // 3 sensors × 60s cadence over 420s = 7 ticks each = 21 readings.
        let expected_readings = 21;
        let (outcome_a, outcome_b) = run_twice(&script);

        // Deterministic content: identical per-peer fingerprints.
        assert_eq!(
            outcome_a.canonical_fingerprint(),
            outcome_b.canonical_fingerprint(),
            "same script must produce the same per-peer readings"
        );

        // The fleet actually ran: every tick of every sensor landed.
        assert_eq!(outcome_a.ingested, expected_readings);
        for (peer, readings) in outcome_a.fingerprint() {
            assert_eq!(
                readings.len(),
                expected_readings,
                "peer '{peer}' must converge to the full union (script heals every fault)"
            );
        }
        // Mirror replicas are bit-identical to the origin — value bits
        // included (DEC-C6: the 1-ULP replication loss is fixed, not masked).
        assert_eq!(
            outcome_a.per_peer["alice"], outcome_a.per_peer["bob"],
            "replicated readings must be bit-identical to the origin rows"
        );

        // No real network: the endpoint count never moved (A19).
        for outcome in [&outcome_a, &outcome_b] {
            assert_eq!(
                outcome.endpoints_before, outcome.endpoints_after,
                "a sim scenario must not bind any iroh endpoint"
            );
        }

        // Cadence + HLC shape on the host: every reading sits on its
        // sensor tick's simulated millisecond (recorded_at_ms) and its HLC
        // wall bits equal that same millisecond — the stamp read the
        // SimClock, not the system clock (A18/A19).
        let host = &outcome_a.fingerprint()["alice"];
        for reading in host {
            let offset = (reading.recorded_at_ms - script.fleet.start_ms as i64) as u64;
            assert_eq!(offset % 60_000, 0, "off-cadence reading: {reading:?}");
            assert_eq!(
                reading.hlc_wall_ms, reading.recorded_at_ms as u64,
                "HLC wall bits must be the simulated ingest millisecond: {reading:?}"
            );
        }
    }

    /// A18: a declarative fleet config drives real `iot_reading` rows —
    /// cadence, values from the pure models, and correct timestamps/HLC.
    #[test]
    fn sim_fleet_writes_real_iot_reading_rows_on_cadence() {
        let json = r#"{
            "name": "sim-a18",
            "fleet": {
                "verse_name": "A18 Fleet",
                "peers": ["solo"],
                "ingest_peer": "solo",
                "anchors": ["tower-a"],
                "sensors": [
                    { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                      "cadence_ms": 60000,
                      "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                 "period_ms": 3600000 } },
                    { "anchor": "tower-a", "metric": "humidity_pct", "units": "%",
                      "cadence_ms": 60000,
                      "model": { "type": "weather", "baseline": 60, "amplitude": 15,
                                 "diurnal_ms": 86400000, "seed": 42, "jitter": 5 } },
                    { "anchor": "tower-a", "metric": "battery_v", "units": "V",
                      "cadence_ms": 60000,
                      "model": { "type": "random_walk", "start": 4.2, "step": 0.05,
                                 "seed": 7 } }
                ],
                "start_ms": 1750000000000,
                "duration_ms": 300000
            }
        }"#;
        let script = ScenarioScript::parse(json).expect("declarative scenario parses");
        let dir = tempfile::tempdir().expect("tempdir");
        let outcome = run_scenario(&script, dir.path()).expect("scenario run");

        // 3 sensors × 5 ticks (60s cadence over 300s) = 15 real rows.
        assert_eq!(outcome.ingested, 15);
        let solo = &outcome.fingerprint()["solo"];
        assert_eq!(solo.len(), 15);

        // Every reading is exactly the pure model's value at its tick, on
        // cadence, stamped by the sim clock.
        let mut seen: HashMap<(String, u64), f64> = HashMap::new();
        for reading in solo {
            let offset = (reading.recorded_at_ms - script.fleet.start_ms as i64) as u64;
            let tick = offset / 60_000;
            assert_eq!(offset, tick * 60_000, "off-cadence reading: {reading:?}");
            assert_eq!(
                reading.hlc_wall_ms, reading.recorded_at_ms as u64,
                "HLC wall bits must be the simulated tick millisecond"
            );
            assert_eq!(reading.anchor, "tower-a");
            let sensor = script
                .fleet
                .sensors
                .iter()
                .find(|s| s.metric == reading.metric)
                .expect("known metric");
            let expected = evaluate(&sensor.model, tick, script.fleet.start_ms + tick * 60_000);
            let actual = f64::from_bits(reading.value_bits);
            assert_eq!(actual, expected, "reading must be the model's value");
            assert_eq!(reading.units, sensor.units);
            seen.insert((reading.metric.clone(), tick), actual);
        }
        // All three models are represented across all five ticks.
        assert_eq!(seen.len(), 15, "15 distinct (metric, tick) readings");
    }

    /// A21 scenario 1: a 3-peer sharded fabric; a window aggregate fanned
    /// out from a non-ingest peer over the VIRTUAL gossip plane equals the
    /// same aggregate over the union of all rows, with full coverage —
    /// deterministic across runs, no real network.
    #[test]
    fn sharded_distributed_query_is_exact_fully_covered_and_deterministic() {
        let script = sharded_query_script();
        let plan = plan_fleet(&script.fleet);
        let (a, b) = run_twice(&script);

        assert_eq!(
            a.canonical_fingerprint(),
            b.canonical_fingerprint(),
            "readings, placement, and query results must repeat exactly"
        );
        for outcome in [&a, &b] {
            assert_eq!(
                outcome.endpoints_before, outcome.endpoints_after,
                "no real network"
            );
            // The hub never self-echoes (iroh-gossip parity), so every
            // counted frame crossed peers: at minimum each of the two
            // queries' requests reached BOTH non-requesting peers.
            assert!(
                outcome.gossip_deliveries >= 4,
                "the fan-out rode the virtual gossip plane cross-peer: {}",
                outcome.gossip_deliveries
            );
        }
        assert_eq!(a.ingested, plan.len());

        // Placement: every shard has ONE host and each peer hosts a
        // distinct, non-empty subset (sharded R=1).
        let all_shards: BTreeSet<String> = plan.iter().map(|r| shard_of(&script, r)).collect();
        assert_eq!(
            a.placement.keys().cloned().collect::<BTreeSet<_>>(),
            all_shards
        );
        let mut per_host: BTreeMap<&str, usize> = BTreeMap::new();
        for hosts in a.placement.values() {
            assert_eq!(
                hosts.len(),
                1,
                "sharded placement is single-host: {hosts:?}"
            );
            *per_host.entry(hosts[0].as_str()).or_default() += 1;
        }
        assert_eq!(per_host.len(), 3, "every peer hosts shards: {per_host:?}");

        // Retention: each replica holds exactly its hosted shards, values
        // bit-identical to the origin. Bob cannot answer the fleet alone.
        for peer in ["bob", "carol"] {
            assert_store_is_its_hosted_shards(&script, &a, peer);
        }
        assert!(
            a.per_peer["bob"].len() < plan.len(),
            "bob holds a strict subset"
        );

        // A15: the fanned-out aggregate == the union oracle; full coverage.
        let agg = a.query("fleet-aggregate").expect("aggregate recorded");
        assert_eq!(agg.outcome.error, None);
        assert!(!agg.outcome.truncated);
        assert_aggregate_matches(agg, &oracle(&script, plan.iter()));
        assert_eq!(
            agg.outcome.covered_shards,
            all_shards.iter().cloned().collect::<Vec<_>>(),
            "every shard covered"
        );
        assert!(agg.outcome.missing_shards.is_empty());
        for must in ["alice", "bob"] {
            // Alice holds the only copy of her shards; bob's is the local partial.
            assert!(
                agg.outcome.answered_hosts.iter().any(|h| h == must),
                "{must} answered"
            );
        }

        // A15 raw union: every reading exactly once, every host answered.
        let raw = a.query("fleet-readings").expect("raw recorded");
        assert_eq!(raw.outcome.error, None);
        assert_eq!(
            raw.outcome.rows.len(),
            plan.len(),
            "the whole fleet, deduped"
        );
        for (row, planned) in raw.outcome.rows.iter().zip(sorted_plan(&script, &plan)) {
            let CanonicalQueryRow::Reading {
                anchor,
                recorded_at_ms,
                value,
                ..
            } = row
            else {
                panic!("non-reading row {row:?}");
            };
            assert_eq!((anchor.as_str(), *recorded_at_ms), (planned.0, planned.1));
            // A raw row is one replicated value (no arithmetic): exact.
            assert_eq!(
                value.to_bits(),
                planned.2.to_bits(),
                "{anchor}@{recorded_at_ms}: {value} vs {}",
                planned.2
            );
        }
        assert_eq!(raw.outcome.answered_hosts, ["alice", "bob", "carol"]);
        assert!(raw.outcome.missing_hosts.is_empty());
        assert!(raw.outcome.missing_shards.is_empty());
    }

    /// Planned readings as (anchor, recorded_at_ms, value), sorted like the
    /// canonical raw rows.
    fn sorted_plan<'a>(
        script: &'a ScenarioScript,
        plan: &[ScheduledReading],
    ) -> Vec<(&'a str, i64, f64)> {
        let mut out: Vec<(&str, i64, f64)> = plan
            .iter()
            .map(|r| {
                (
                    script.fleet.sensors[r.sensor].anchor.as_str(),
                    r.at_ms as i64,
                    r.value,
                )
            })
            .collect();
        out.sort_by(|x, y| (x.0, x.1).cmp(&(y.0, y.1)));
        out
    }

    /// A21 scenario 2: the ingest host goes offline → a query during the
    /// outage is degraded but honest (exactly the host's shards missing,
    /// the host in missing_hosts, the merge over what IS covered) → the
    /// host returns → outage readings converge → full coverage again.
    #[test]
    fn offline_host_degrades_honestly_then_converges() {
        let script = offline_degraded_script();
        let plan = plan_fleet(&script.fleet);
        let (a, b) = run_twice(&script);

        assert_eq!(
            a.canonical_fingerprint(),
            b.canonical_fingerprint(),
            "the outage, the degraded answer, and the convergence must repeat exactly"
        );
        for outcome in [&a, &b] {
            assert_eq!(
                outcome.endpoints_before, outcome.endpoints_after,
                "no real network"
            );
        }
        assert_eq!(
            a.ingested,
            plan.len(),
            "the offline host kept ingesting locally"
        );

        let start = script.fleet.start_ms;
        let (outage_start, outage_end) = (start + 300_000, start + 450_000);
        let host_of = |r: &ScheduledReading| a.placement[&shard_of(&script, r)][0].clone();

        // --- Degraded (A16): exactly alice's pre-outage shards are missing.
        let pre: Vec<&ScheduledReading> = plan.iter().filter(|r| r.at_ms < outage_start).collect();
        let (mut want_missing, mut want_covered) = (BTreeSet::new(), BTreeSet::new());
        for r in &pre {
            let shard = shard_of(&script, r);
            if host_of(r) == "alice" {
                want_missing.insert(shard);
            } else {
                want_covered.insert(shard);
            }
        }
        assert!(
            !want_missing.is_empty() && !want_covered.is_empty(),
            "the seed must place pre-outage shards on both sides of the cut"
        );
        let degraded = a.query("degraded-aggregate").expect("degraded recorded");
        assert_eq!(degraded.outcome.error, None);
        assert_eq!(
            degraded.outcome.missing_shards,
            want_missing.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            degraded.outcome.covered_shards,
            want_covered.iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(degraded.outcome.missing_hosts, ["alice"]);
        assert_eq!(degraded.outcome.answered_hosts, ["bob", "carol"]);
        assert_aggregate_matches(
            degraded,
            &oracle(
                &script,
                pre.iter().copied().filter(|r| host_of(r) != "alice"),
            ),
        );

        // --- Convergence: outage readings reached their hosts (doc plane).
        for r in plan
            .iter()
            .filter(|r| r.at_ms >= outage_start && r.at_ms < outage_end)
        {
            let host = host_of(r);
            let anchor = &script.fleet.sensors[r.sensor].anchor;
            assert!(
                a.per_peer[&host]
                    .iter()
                    .any(|c| &c.anchor == anchor && c.recorded_at_ms == r.at_ms as i64),
                "outage reading {anchor}@{} must converge to its host {host}",
                r.at_ms
            );
        }
        for peer in ["bob", "carol"] {
            assert_store_is_its_hosted_shards(&script, &a, peer);
        }

        // --- Healed: full coverage, exact merge over every reading.
        let healed = a.query("healed-aggregate").expect("healed recorded");
        assert_eq!(healed.outcome.error, None);
        assert!(healed.outcome.missing_shards.is_empty());
        assert_eq!(healed.outcome.covered_shards.len(), a.placement.len());
        assert_aggregate_matches(healed, &oracle(&script, plan.iter()));
        let readings = a.query("healed-readings").expect("healed raw recorded");
        assert_eq!(readings.outcome.rows.len(), plan.len());
        assert_eq!(readings.outcome.answered_hosts, ["alice", "bob", "carol"]);
    }

    /// DEC-C6 tripwire (fast, no peers): a reading row survives the
    /// replication encoding (serde_json bytes → parse) bit-exactly. Fails if
    /// the workspace `serde_json/float_roundtrip` feature is ever dropped —
    /// 62.985254035088204 is a value the best-effort parser rounds 1 ULP off.
    #[test]
    fn replication_json_round_trip_is_bit_exact() {
        let mut values: Vec<f64> = vec![62.985_254_035_088_204];
        for script in [default_script(), sharded_query_script()] {
            values.extend(plan_fleet(&script.fleet).iter().map(|r| r.value));
        }
        for value in values {
            let bytes = serde_json::to_vec(&serde_json::json!({ "value": value })).unwrap();
            let row: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                row["value"].as_f64().map(f64::to_bits),
                Some(value.to_bits()),
                "{value} lost bits across the replication encoding"
            );
        }
    }

    #[test]
    fn scenario_validation_rejects_bad_events() {
        let mut script = default_script();
        script.events.push(ScriptedEvent::PeerOffline {
            at_ms: 1,
            peer: "nobody".into(),
        });
        assert!(script.validate().is_err());
        assert!(ScenarioScript::parse(&serde_json::to_string(&script).unwrap()).is_err());

        let query = |label: &str, peer: &str, end_ms: u64| ScriptedEvent::Query {
            at_ms: 1,
            peer: peer.into(),
            label: label.into(),
            query: SimQuery::WindowAggregate {
                metric: "temperature_c".into(),
                start_ms: 10,
                end_ms,
            },
            timeout_ms: 1_000,
        };
        for (bad, why) in [
            (vec![query("q", "nobody", 20)], "unknown peer"),
            (
                vec![query("q", "bob", 20), query("q", "bob", 20)],
                "duplicate label",
            ),
            (vec![query("q", "bob", 10)], "empty window"),
            (
                vec![ScriptedEvent::SetLatency {
                    at_ms: 1,
                    latency_ms: u64::MAX,
                }],
                "latency pushing sim time past 2^48",
            ),
            (
                vec![ScriptedEvent::Heal {
                    at_ms: crate::fleet::SIM_TIME_LIMIT_MS,
                }],
                "event past 2^48",
            ),
        ] {
            let mut script = default_script();
            script.events = bad;
            assert!(script.validate().is_err(), "{why} must be rejected");
        }

        // A future-dated fleet is rejected at the script level too.
        let mut script = default_script();
        script.fleet.start_ms = crate::fleet::real_now_ms() + 86_400_000;
        let err = script.validate().expect_err("future start_ms");
        assert!(err.to_string().contains("in the future"), "{err}");
    }

    /// The F8 at_ms regression guard: event offsets become ABSOLUTE times
    /// at merge and interleave with ticks by time — an event lands before a
    /// same-instant tick, between earlier/later ticks, and after the last
    /// tick, keeping script order among same-instant events.
    #[test]
    fn merge_actions_places_events_at_their_absolute_times_among_ticks() {
        let mut script = default_script();
        script.fleet.sensors.truncate(1); // one sensor, 60s cadence
        script.fleet.duration_ms = 180_000; // ticks at +60k, +120k, +180k
        script.events = vec![
            ScriptedEvent::Heal { at_ms: 120_000 },
            ScriptedEvent::SetLatency {
                at_ms: 200_000,
                latency_ms: 0,
            },
            ScriptedEvent::PeerOffline {
                at_ms: 60_000,
                peer: "bob".into(),
            },
            ScriptedEvent::SetLatency {
                at_ms: 90_000,
                latency_ms: 5,
            },
            ScriptedEvent::PeerOnline {
                at_ms: 120_000,
                peer: "bob".into(),
            },
        ];
        let origin = script.fleet.start_ms;
        let plan = plan_fleet(&script.fleet);
        let actions = merge_actions(&script, &plan);
        let shape: Vec<(char, u64)> = actions
            .iter()
            .map(|a| match a {
                Action::Event { at_ms, .. } => ('E', at_ms - origin),
                Action::Tick { at_ms, .. } => ('T', at_ms - origin),
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                ('E', 60_000), // offline lands BEFORE the same-instant tick
                ('T', 60_000),
                ('E', 90_000),  // between ticks
                ('E', 120_000), // heal, then online: same instant keeps script order
                ('E', 120_000),
                ('T', 120_000),
                ('T', 180_000),
                ('E', 200_000), // after the last tick
            ]
        );
        let same_instant: Vec<&ScriptedEvent> = actions
            .iter()
            .filter_map(|a| match a {
                Action::Event { at_ms, event } if *at_ms == origin + 120_000 => Some(event),
                _ => None,
            })
            .collect();
        assert!(matches!(same_instant[0], ScriptedEvent::Heal { .. }));
        assert!(matches!(same_instant[1], ScriptedEvent::PeerOnline { .. }));
    }

    /// Fix-1 regression (DEC-C13): a mirror replica retaining 300+ readings
    /// between store barriers (no queries → the only store barrier is the
    /// final one) used to deadlock — the replica's DB thread blocked on its
    /// undrained `ReplicatedRowApplied` echoes while the driver blocked on
    /// the full command channel, and the 30s settle deadline never fired.
    /// Now the driver drains echoes and the harness `try_send`s them.
    ///
    /// Guard: the session runs on its own thread; once it holds the run lock
    /// it gets [`DEADLOCK_BUDGET`]. A regression is a hang that would ALSO
    /// hold the process-global run lock (stalling every later scenario
    /// test), so on timeout the test exits the process (code 101) with a
    /// message naming the drain — a loud CI failure, never a silent hang.
    #[test]
    fn mirror_replica_retaining_hundreds_of_rows_between_barriers_never_deadlocks() {
        const DEADLOCK_BUDGET: Duration = Duration::from_secs(300);
        let script = ScenarioScript::parse(
            r#"{
                "name": "sim-deadlock-guard",
                "fleet": {
                    "verse_name": "Deadlock Guard Verse",
                    "peers": ["alice", "bob"],
                    "ingest_peer": "alice",
                    "anchors": ["tower-a", "tower-b"],
                    "sensors": [
                        { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                          "cadence_ms": 1000,
                          "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                     "period_ms": 3600000 } },
                        { "anchor": "tower-a", "metric": "humidity_pct", "units": "%",
                          "cadence_ms": 1000,
                          "model": { "type": "sine", "baseline": 60, "amplitude": 10,
                                     "period_ms": 600000 } },
                        { "anchor": "tower-b", "metric": "temperature_c", "units": "C",
                          "cadence_ms": 1000,
                          "model": { "type": "sine", "baseline": 12, "amplitude": 5,
                                     "period_ms": 1800000 } },
                        { "anchor": "tower-b", "metric": "pressure_hpa", "units": "hPa",
                          "cadence_ms": 1000,
                          "model": { "type": "sine", "baseline": 1013, "amplitude": 4,
                                     "period_ms": 7200000 } }
                    ],
                    "start_ms": 1750000000000,
                    "duration_ms": 80000
                }
            }"#,
        )
        .expect("deadlock-guard scenario parses");
        let planned = plan_fleet(&script.fleet).len();
        assert!(
            planned >= 300,
            "the guard needs ≥ 300 rows, plans {planned}"
        );

        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = tempfile::tempdir().expect("tempdir");
            let outcome = crate::session::ScenarioSession::start(script, dir.path()).and_then(
                |mut session| {
                    let _ = started_tx.send(()); // the run lock is held from here
                    session.step(u32::MAX)?;
                    session.stop()
                },
            );
            let _ = done_tx.send(outcome);
        });
        // Queueing behind other scenario tests on the run lock is not this
        // test's budget; a failed start drops the sender (falls through).
        let _ = started_rx.recv();
        let outcome = match done_rx.recv_timeout(DEADLOCK_BUDGET) {
            Ok(outcome) => outcome.expect("deadlock-guard scenario run"),
            Err(_) => {
                eprintln!(
                    "FATAL: sim scenario deadlocked ({planned} mirror rows, no barrier) — the \
                     driver must drain ReplicatedRowApplied echoes (Run::drain_events) and the \
                     harness must try_send them (fe-test-harness §peer-model). Exiting: the \
                     hung run holds SCENARIO_RUN_LOCK and would stall every later scenario test."
                );
                std::process::exit(101);
            }
        };
        assert_eq!(outcome.ingested, planned, "every tick landed");
        assert_eq!(
            outcome.per_peer["bob"].len(),
            planned,
            "the mirror replica converged on every row"
        );
        assert_eq!(
            outcome.per_peer["alice"], outcome.per_peer["bob"],
            "replicated rows are bit-identical to the origin"
        );
    }
}
