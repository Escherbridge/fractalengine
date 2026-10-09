//! Scripted deterministic scenarios (F8/A19): a [`ScenarioScript`] is one
//! declarative document — a fleet config plus a fault script (churn,
//! partitions, latency, heals at simulated times). [`run_scenario`] plays
//! it against real in-process peers on the virtual transport and returns a
//! canonical [`ScenarioOutcome`] fingerprint.
//!
//! Determinism model (why the same script always yields the same outcome):
//!
//! * Every value is a pure function of (config, tick) — `sensors.rs` — and
//!   the fire schedule itself is a pure function of the config —
//!   `fleet::plan_fleet`. No RNG state anywhere.
//! * The scenario clock only moves when the driver moves it, and the
//!   driver awaits every DB reply before its next step, so each batch's
//!   HLC stamp lands at a fixed simulated millisecond
//!   (`hlc_wall_ms` in the fingerprint).
//! * The hub's delivery order comes from a (due_ms, seq) heap, never from
//!   thread scheduling. Where real threads do interleave (the sync
//!   threads' pumps), the assertions ride only order-independent facts:
//!   the `iot_reading` union CRDT (A12) and convergence sets.
//! * Deliberately excluded from the fingerprint: `reading_id` (a fresh
//!   server-side ULID per ingest), `source_did` (each run generates fresh
//!   node keypairs), and the HLC counter bits (a concurrent pump apply
//!   may share a millisecond). Wall bits are included — they prove the
//!   stamps read the SimClock, not the system clock.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, IotReadingInput};
use fe_sync::messages::SyncCommand;
use serde::{Deserialize, Serialize};

use crate::clock::{install_hlc_source, uninstall_hlc_source, SimClock};
use crate::fleet::{plan_fleet, rfc3339_from_ms, FleetConfig, ScheduledReading};
use crate::net::SimNet;
use crate::peer::SimPeer;
use crate::sensors::evaluate;

/// How long a settle poll may take before the scenario fails (real time —
/// it bounds *waiting*, never what converges).
const SETTLE_BUDGET: Duration = Duration::from_secs(30);
/// Real-time slice between settle polls (lets the sync pumps drain).
const SETTLE_POLL: Duration = Duration::from_millis(10);
/// DB-thread reply budget (matches the harness scenarios).
const DB_REPLY_BUDGET: Duration = Duration::from_secs(30);

/// One scripted fault at a simulated offset (`at_ms` counts from the
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
}

impl ScriptedEvent {
    fn at_ms(&self) -> u64 {
        match self {
            Self::PeerOffline { at_ms, .. }
            | Self::PeerOnline { at_ms, .. }
            | Self::Partition { at_ms, .. }
            | Self::Heal { at_ms }
            | Self::SetLatency { at_ms, .. } => *at_ms,
        }
    }
}

/// A whole scenario: the fleet + the fault script. Declarative JSON (see
/// `tests` for a full example, and the `fe-sim` bin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioScript {
    pub name: String,
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
    /// peer, events must be time-ordered is NOT required (they are sorted
    /// by the driver), but unknown names are a config error.
    pub fn validate(&self) -> Result<()> {
        self.fleet.validate()?;
        for event in &self.events {
            match event {
                ScriptedEvent::PeerOffline { peer, .. }
                | ScriptedEvent::PeerOnline { peer, .. } => {
                    if !self.fleet.peers.contains(peer) {
                        anyhow::bail!("scenario {}: event names unknown peer '{peer}'", self.name);
                    }
                }
                ScriptedEvent::Partition { groups, .. } => {
                    for group in groups {
                        for peer in group {
                            if !self.fleet.peers.contains(peer) {
                                anyhow::bail!(
                                    "scenario {}: partition names unknown peer '{peer}'",
                                    self.name
                                );
                            }
                        }
                    }
                }
                ScriptedEvent::Heal { .. } | ScriptedEvent::SetLatency { .. } => {}
            }
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

/// The canonical outcome of one scenario run — what a determinism
/// assertion compares.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioOutcome {
    pub name: String,
    /// Total readings the ingest host durably wrote.
    pub ingested: usize,
    /// Per-peer canonical readings (sorted), keyed by fleet peer name.
    pub per_peer: BTreeMap<String, Vec<CanonicalReading>>,
    /// Hub deliveries dropped at drain time (link-down losses + channel
    /// backpressure) — diagnostics, not part of the determinism contract.
    pub dropped_deliveries: u64,
    /// `fe_sync::bound_endpoint_count()` before/after the run: a sim
    /// scenario binds no iroh endpoint, so these must be equal (A19's
    /// "no real network" clause).
    pub endpoints_before: u64,
    pub endpoints_after: u64,
}

impl ScenarioOutcome {
    /// The per-peer fingerprint (readings only — the determinism contract).
    pub fn fingerprint(&self) -> &BTreeMap<String, Vec<CanonicalReading>> {
        &self.per_peer
    }
}

/// Restores the real HLC source when the scenario ends (even on error) —
/// the override is process-global, so a leaked install would poison
/// every later test in the process.
struct HlcSourceGuard;

impl Drop for HlcSourceGuard {
    fn drop(&mut self) {
        uninstall_hlc_source();
    }
}

/// One merged driver action in total `(at_ms, event-before-tick)` order.
enum Action<'a> {
    Event(&'a ScriptedEvent),
    /// All readings scheduled at this instant.
    Tick {
        at_ms: u64,
        readings: Vec<&'a ScheduledReading>,
    },
}

impl<'a> Action<'a> {
    fn at_ms(&self) -> u64 {
        match self {
            Self::Event(e) => e.at_ms(),
            Self::Tick { at_ms, .. } => *at_ms,
        }
    }
}

/// Run one scripted scenario under `root_dir` (each peer gets its own
/// subdirectory; use a fresh directory per run — two runs sharing
/// directories would collide on peer names).
pub fn run_scenario(script: &ScenarioScript, root_dir: &Path) -> Result<ScenarioOutcome> {
    script.validate()?;
    // The HLC override is process-global (clock.rs): serialize scenario runs
    // so a concurrent run — or the clock tests — cannot clobber the active
    // source mid-run. Declared first so it outlives the HLC guard (drop
    // order runs the uninstall before the lock releases).
    let _run_lock = crate::clock::SCENARIO_RUN_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    std::fs::create_dir_all(root_dir)?;

    let endpoints_before = fe_sync::bound_endpoint_count();

    // The clock the whole run reads: sensors, the hub, and (via the
    // process-global HLC source) every reading's stamp.
    let clock = SimClock::new(script.fleet.start_ms);
    install_hlc_source(clock.clone());
    let _hlc_guard = HlcSourceGuard;

    let net = SimNet::new(clock.clone());

    // --- Spawn the peers on the virtual transport (no real network). ---
    let mut peers: BTreeMap<String, SimPeer> = BTreeMap::new();
    for name in &script.fleet.peers {
        let peer = SimPeer::spawn(&net, name, root_dir)?;
        peers.insert(name.clone(), peer);
    }
    let host_name = script.fleet.ingest_peer.clone();
    let host = peers
        .get(&host_name)
        .ok_or_else(|| anyhow::anyhow!("ingest peer '{host_name}' missing"))?;
    let host_did = host.did();

    // --- Setup the hierarchy on the ingest host (real DbCommands). ------
    host.peer.send(DbCommand::CreateVerse {
        name: script.fleet.verse_name.clone(),
    });
    let verse_id = match host.peer.wait_for(
        |r| matches!(r, DbResult::VerseCreated { .. }),
        DB_REPLY_BUDGET,
    )? {
        DbResult::VerseCreated { id, .. } => id,
        other => anyhow::bail!("unexpected CreateVerse result: {other:?}"),
    };

    host.peer.send(DbCommand::CreateFractal {
        verse_id: verse_id.clone(),
        name: "Sim Fractal".into(),
    });
    let fractal_id = match host.peer.wait_for(
        |r| matches!(r, DbResult::FractalCreated { .. }),
        DB_REPLY_BUDGET,
    )? {
        DbResult::FractalCreated { id, .. } => id,
        other => anyhow::bail!("unexpected CreateFractal result: {other:?}"),
    };

    host.peer.send(DbCommand::CreatePetal {
        fractal_id,
        name: "Sim Petal".into(),
    });
    let petal_id = match host.peer.wait_for(
        |r| matches!(r, DbResult::PetalCreated { .. }),
        DB_REPLY_BUDGET,
    )? {
        DbResult::PetalCreated { id, .. } => id,
        other => anyhow::bail!("unexpected CreatePetal result: {other:?}"),
    };

    // Anchor nodes — one per fleet anchor, correlated so each reply is
    // unambiguous. anchor name → durable node id (the reading's node_id).
    let mut anchor_node_ids: HashMap<String, String> = HashMap::new();
    for anchor in &script.fleet.anchors {
        host.peer.send(DbCommand::CreateNode {
            petal_id: petal_id.clone(),
            name: anchor.clone(),
            position: [0.0, 0.0, 0.0],
            correlation_id: Some(format!("sim-anchor:{anchor}")),
        });
        let node_id = match host.peer.wait_for(
            |r| {
                matches!(r, DbResult::NodeCreated { ref correlation_id, .. }
                    if correlation_id.as_deref() == Some(format!("sim-anchor:{anchor}").as_str()))
            },
            DB_REPLY_BUDGET,
        )? {
            DbResult::NodeCreated { id, .. } => id,
            other => anyhow::bail!("unexpected CreateNode result: {other:?}"),
        };
        anchor_node_ids.insert(anchor.clone(), node_id);
    }

    // --- Open the replica on every peer (all on the shared hub doc). ----
    let ns_secret_hex = host
        .peer
        .namespace_secret(&verse_id)
        .ok_or_else(|| anyhow::anyhow!("verse namespace secret missing"))?;
    let secret_bytes: [u8; 32] = hex::decode(&ns_secret_hex)?
        .try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("secret must be 32 bytes, got {}", v.len()))?;
    let ns_id_hex = hex::encode(fe_database::derive_namespace_id(&secret_bytes));
    for peer in peers.values() {
        peer.peer
            .sync_cmd_tx
            .send(SyncCommand::OpenVerseReplica {
                verse_id: verse_id.clone(),
                namespace_id: ns_id_hex.clone(),
                namespace_secret: Some(ns_secret_hex.clone()),
                bootstrap_peers: Vec::new(),
            })
            .map_err(|e| anyhow::anyhow!("OpenVerseReplica failed: {e}"))?;
    }
    // Give every sync thread a beat to finish its replica open (subscribe +
    // startup snapshot) before the host publishes the manifest — the hub
    // fans out only to CURRENT subscribers, so a write racing a joiner's
    // subscribe would never be delivered. This is the same 500ms quiescence
    // the real-transport scenarios use; the settle budget backstops any
    // tail latency. It cannot perturb determinism: the clock does not move
    // during it, so no stamp is affected.
    std::thread::sleep(Duration::from_millis(500));

    // --- Publish the verse manifest from the host (A3's admission       ---
    // --- precondition: joiners must resolve the host as Owner).         ---
    let manifest_row = serde_json::json!({
        "verse_id": verse_id,
        "name": script.fleet.verse_name,
        "created_by": host_did,
        "created_at": rfc3339_from_ms(clock.now_ms()),
        "namespace_id": ns_id_hex,
        "default_access": "viewer",
    });
    let manifest_bytes = serde_json::to_vec(&manifest_row)?;
    let manifest_hash = host.peer.blob_store.add_blob(&manifest_bytes)?;
    host.peer
        .sync_cmd_tx
        .send(SyncCommand::WriteRowEntry {
            verse_id: verse_id.clone(),
            table: "verse".into(),
            record_id: verse_id.clone(),
            content_hash: manifest_hash,
        })
        .map_err(|e| anyhow::anyhow!("manifest WriteRowEntry failed: {e}"))?;

    // The manifest must converge BEFORE the fleet starts (the A3 gate on
    // every joiner resolves the host through it). Settling never advances
    // the clock — only steps the hub — so the first tick's simulated
    // timestamp is untouched by however long convergence really takes.
    let joiners: Vec<String> = script
        .fleet
        .peers
        .iter()
        .filter(|p| **p != host_name)
        .cloned()
        .collect();
    settle(
        &clock,
        &net,
        &peers,
        &joiners,
        |peer_rows| peer_rows >= 1,
        "verse manifest convergence",
        |peer| {
            let rows = raw_query(
                &peer.peer,
                &format!("SELECT verse_id FROM verse WHERE verse_id = '{verse_id}'"),
            )?;
            Ok(rows.len())
        },
    )?;

    // --- Merge the fire plan and the fault script into one ordered      ---
    // --- action list (events land before same-instant ticks).           ---
    let plan = plan_fleet(&script.fleet);
    let mut actions: Vec<Action> = Vec::new();
    {
        let mut events: Vec<&ScriptedEvent> = script.events.iter().collect();
        events.sort_by_key(|e| e.at_ms());
        let mut ticks: Vec<(u64, Vec<&ScheduledReading>)> = Vec::new();
        for reading in &plan {
            match ticks.last_mut() {
                Some((at, list)) if *at == reading.at_ms => list.push(reading),
                _ => ticks.push((reading.at_ms, vec![reading])),
            }
        }
        // Merge two sorted streams.
        let mut e = 0usize;
        let mut t = 0usize;
        while e < events.len() || t < ticks.len() {
            let next_event = events.get(e).map(|ev| (ev.at_ms(), 0u8));
            let next_tick = ticks.get(t).map(|(at, _)| (*at, 1u8));
            let take_event = match (next_event, next_tick) {
                (Some((ea, _)), Some((ta, _))) => ea <= ta,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => false,
            };
            if take_event {
                actions.push(Action::Event(events[e]));
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
    }

    // --- Drive the script. The clock only moves between actions, and   ---
    // --- every ingest awaits its reply, so every stamp is a fixed      ---
    // --- simulated millisecond.                                        ---
    let mut ingested: usize = 0;
    for action in &actions {
        let at = action.at_ms();
        if at > clock.now_ms() {
            clock.advance_ms(at - clock.now_ms());
            net.step();
        }
        match action {
            Action::Event(event) => apply_event(&net, &peers, event),
            Action::Tick { readings, .. } => {
                let mut batch: Vec<IotReadingInput> = Vec::with_capacity(readings.len());
                for reading in readings {
                    let sensor = &script.fleet.sensors[reading.sensor];
                    // Values were precomputed by the pure planner; this
                    // re-evaluation is an internal parity check that the
                    // fired value is exactly the model's.
                    debug_assert_eq!(
                        reading.value,
                        evaluate(&sensor.model, reading.tick, reading.at_ms)
                    );
                    batch.push(IotReadingInput {
                        node_id: anchor_node_ids
                            .get(&sensor.anchor)
                            .cloned()
                            .ok_or_else(|| anyhow::anyhow!("anchor '{}' missing", sensor.anchor))?,
                        metric: sensor.metric.clone(),
                        value: reading.value,
                        units: sensor.units.clone(),
                        recorded_at: Some(rfc3339_from_ms(reading.at_ms)),
                    });
                }
                if batch.is_empty() {
                    continue;
                }
                let expected = batch.len();
                let want_petal = petal_id.clone();
                host.peer.send(DbCommand::InsertIotReadings {
                    petal_id: petal_id.clone(),
                    verse_id: Some(verse_id.clone()),
                    source_did: host_did.clone(),
                    readings: batch,
                });
                let written = match host.peer.wait_for(
                    |r| {
                        matches!(r, DbResult::IotReadingsInserted { ref petal_id, .. }
                            if petal_id == &want_petal)
                    },
                    DB_REPLY_BUDGET,
                )? {
                    DbResult::IotReadingsInserted { written, .. } => written,
                    other => anyhow::bail!("unexpected ingest result: {other:?}"),
                };
                if written != expected {
                    anyhow::bail!("ingest wrote {written} of {expected} readings");
                }
                ingested += written;
                // Give the DB→sync bridge + hub a beat, then drain what is
                // due. The end-of-run settle catches anything in flight.
                std::thread::sleep(SETTLE_POLL);
                net.step();
            }
        }
    }

    // --- Settle: every online peer converges to the union of all        ---
    // --- ingested readings (the built-in scripts heal every fault       ---
    // --- before the end, so the target is exact).                        ---
    for name in &joiners {
        settle(
            &clock,
            &net,
            &peers,
            std::slice::from_ref(name),
            |count| count == ingested,
            &format!("reading union convergence on {name}"),
            |peer| {
                let rows = raw_query(&peer.peer, "SELECT * FROM iot_reading")?;
                Ok(rows.len())
            },
        )?;
    }
    // Drain anything still in flight (bounded — quiescence, not progress).
    let settle_deadline = Instant::now() + SETTLE_BUDGET;
    while net.inflight_count() > 0 && Instant::now() < settle_deadline {
        clock.advance_ms(1_000);
        net.step();
        std::thread::sleep(SETTLE_POLL);
    }

    // --- Fingerprint every peer's durable store. -------------------------
    let node_to_anchor: HashMap<String, &String> = anchor_node_ids
        .iter()
        .map(|(k, v)| (v.clone(), k))
        .collect();
    let mut per_peer: BTreeMap<String, Vec<CanonicalReading>> = BTreeMap::new();
    for (name, peer) in &peers {
        let rows = raw_query(&peer.peer, "SELECT * FROM iot_reading")?;
        let mut canonical: Vec<CanonicalReading> = rows
            .iter()
            .map(|row| {
                let node_id = row["node_id"].as_str().unwrap_or_default();
                Ok::<_, anyhow::Error>(CanonicalReading {
                    anchor: node_to_anchor
                        .get(node_id)
                        .map(|s| (*s).clone())
                        .unwrap_or_else(|| node_id.to_string()),
                    metric: row["metric"].as_str().unwrap_or_default().to_string(),
                    units: row["units"].as_str().unwrap_or_default().to_string(),
                    recorded_at_ms: row["recorded_at_ms"].as_i64().unwrap_or_default(),
                    value_bits: row["value"].as_f64().unwrap_or_default().to_bits(),
                    hlc_wall_ms: row["hlc_timestamp"]
                        .as_i64()
                        .map(|h| (h >> 16) as u64)
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<_>>()?;
        canonical.sort();
        per_peer.insert(name.clone(), canonical);
    }

    // --- Clean shutdown: close replicas, drop peers (threads join). ------
    for peer in peers.values() {
        let _ = peer.peer.sync_cmd_tx.send(SyncCommand::CloseVerseReplica {
            verse_id: verse_id.clone(),
        });
    }
    std::thread::sleep(SETTLE_POLL);
    drop(peers);

    let endpoints_after = fe_sync::bound_endpoint_count();
    Ok(ScenarioOutcome {
        name: script.name.clone(),
        ingested,
        per_peer,
        dropped_deliveries: net.dropped_deliveries(),
        endpoints_before,
        endpoints_after,
    })
}

/// Apply one scripted fault to the hub (mapping fleet peer names to DIDs —
/// the hub's membership keys are the DIDs the replicas author as).
fn apply_event(net: &Arc<SimNet>, peers: &BTreeMap<String, SimPeer>, event: &ScriptedEvent) {
    match event {
        ScriptedEvent::PeerOffline { peer, .. } => {
            let did = peers
                .get(peer)
                .map(|p| p.did())
                .unwrap_or_else(|| peer.clone());
            tracing::info!(peer, "scenario: peer offline");
            net.set_peer_online(&did, false);
        }
        ScriptedEvent::PeerOnline { peer, .. } => {
            let did = peers
                .get(peer)
                .map(|p| p.did())
                .unwrap_or_else(|| peer.clone());
            tracing::info!(peer, "scenario: peer back online");
            net.set_peer_online(&did, true);
        }
        ScriptedEvent::Partition { groups, .. } => {
            let did_groups: Vec<Vec<String>> = groups
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|name| {
                            peers
                                .get(name)
                                .map(|p| p.did())
                                .unwrap_or_else(|| name.clone())
                        })
                        .collect()
                })
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
    }
}

/// Poll `peers` until `check` passes for every one — stepping the hub each
/// round and letting the real pumps run between polls. When deliveries are
/// still in flight (scripted latency puts their due-times in the simulated
/// future), the clock is advanced a bounded quantum so they can drain;
/// nothing stamps time after the plan ends, so this moves messages, never
/// content — determinism is untouched. Fails loudly at the budget: a
/// scenario that cannot converge is a bug, not a slow machine.
fn settle<P, C>(
    clock: &Arc<SimClock>,
    net: &Arc<SimNet>,
    peers: &BTreeMap<String, SimPeer>,
    targets: &[String],
    check: C,
    what: &str,
    probe: P,
) -> Result<()>
where
    P: Fn(&SimPeer) -> Result<usize>,
    C: Fn(usize) -> bool,
{
    let deadline = Instant::now() + SETTLE_BUDGET;
    loop {
        net.step();
        let mut all = true;
        for name in targets {
            let peer = peers
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("settle: unknown peer '{name}'"))?;
            let value = probe(peer).unwrap_or(0);
            if !check(value) {
                all = false;
                break;
            }
        }
        if all {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("scenario settle timed out waiting for {what}");
        }
        if net.inflight_count() > 0 {
            clock.advance_ms(1_000);
        }
        std::thread::sleep(SETTLE_POLL);
    }
}

/// Read rows back from a peer's durable store (RawQuery — SELECT-only).
fn raw_query(
    peer: &fractalengine_test_harness::peer::TestPeer,
    sql: &str,
) -> Result<Vec<serde_json::Value>> {
    peer.send(DbCommand::RawQuery {
        sql: sql.to_string(),
        vars: HashMap::new(),
    });
    match peer.wait_for(
        |r| matches!(r, DbResult::QueryResult { .. }),
        DB_REPLY_BUDGET,
    )? {
        DbResult::QueryResult { data } => Ok(data),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A19: the same script, run twice, produces the same canonical
    /// fingerprint on every peer — and binds no iroh endpoint either time.
    #[test]
    fn scripted_scenario_is_deterministic_with_no_real_network() {
        let script = default_script();
        // 3 sensors × 60s cadence over 420s = 7 ticks each = 21 readings.
        let expected_readings = 21;

        let dir_a = tempfile::tempdir().expect("run A tempdir");
        let outcome_a = run_scenario(&script, dir_a.path()).expect("scenario run A");
        let dir_b = tempfile::tempdir().expect("run B tempdir");
        let outcome_b = run_scenario(&script, dir_b.path()).expect("scenario run B");

        // Deterministic content: identical per-peer fingerprints.
        assert_eq!(
            outcome_a.fingerprint(),
            outcome_b.fingerprint(),
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

        // No real network: the endpoint count never moved (A19).
        assert_eq!(
            outcome_a.endpoints_before, outcome_a.endpoints_after,
            "a sim scenario must not bind any iroh endpoint"
        );
        assert_eq!(
            outcome_b.endpoints_before, outcome_b.endpoints_after,
            "run B must not bind any iroh endpoint either"
        );

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

        // Declarative proven: the config above parsed from raw JSON and the
        // run drove itself from it (no imperative sensor code anywhere).
        assert!(script.fleet.sensors.len() == 3);
    }

    #[test]
    fn scenario_validation_rejects_unknown_peer_faults() {
        let mut script = default_script();
        script.events.push(ScriptedEvent::PeerOffline {
            at_ms: 1,
            peer: "nobody".into(),
        });
        assert!(script.validate().is_err());
        assert!(ScenarioScript::parse(&serde_json::to_string(&script).unwrap()).is_err());
    }
}
