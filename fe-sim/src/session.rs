//! `ScenarioSession` (F9/A20): the long-lived scenario driver behind the
//! sim control surface — and, via [`crate::scenario::run_scenario`], the
//! one-shot runner too (ONE driver, no drift). See `fe-sim/src/AGENTS.md`
//! §session for the lifecycle, lock discipline, and event pumping.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::MutexGuard;
use std::time::Duration;

use anyhow::Result;
use fe_runtime::messages::{DbCommand, DbResult, IotReadingInput};
use fe_runtime::timeseries::VerseTimeseriesSettings;
use fe_sync::messages::SyncCommand;
use serde::Serialize;

use crate::clock::{SimClock, SCENARIO_RUN_LOCK};
use crate::fleet::{plan_fleet, rfc3339_from_ms};
use crate::net::SimNet;
use crate::peer::SimPeer;
use crate::scenario::{
    apply_event, merge_actions, raw_query, Action, CanonicalReading, HlcSourceGuard, QueryRecord,
    Run, ScenarioOutcome, ScenarioScript, ScriptedEvent, DB_REPLY_BUDGET, SETTLE_POLL,
};
use crate::sensors::evaluate;

/// A fault applied interactively ([`ScenarioSession::inject_fault`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InjectedFault {
    /// Simulated offset (from `start_ms`) it was applied at.
    pub at_ms: u64,
    /// Merged actions already executed when it landed.
    pub cursor: usize,
    pub event: ScriptedEvent,
}

/// One peer in a [`SessionStatus`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PeerStatus {
    pub name: String,
    pub did: String,
    pub online: bool,
}

/// An honest snapshot of a live session.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionStatus {
    pub name: String,
    pub ingest_peer: String,
    pub peers: Vec<PeerStatus>,
    /// Merged actions executed / total.
    pub cursor: usize,
    pub total_actions: usize,
    pub done: bool,
    /// Simulated offset of the next action (`None` when done).
    pub next_action_at_ms: Option<u64>,
    /// Simulated offset of the clock now.
    pub clock_offset_ms: u64,
    pub readings_ingested: usize,
    pub readings_planned: usize,
    pub queries_recorded: usize,
    pub faults_injected: Vec<InjectedFault>,
    pub latency_ms: u64,
    pub active_partitions: usize,
    pub inflight_deliveries: usize,
    pub dropped_deliveries: u64,
    pub gossip_deliveries: u64,
    /// iroh endpoints bound since start (a sim session binds none — A19).
    pub endpoints_bound: u64,
}

/// What one [`ScenarioSession::step`] call did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepReport {
    pub executed: u32,
    pub cursor: usize,
    pub total_actions: usize,
    pub done: bool,
    pub clock_offset_ms: u64,
    /// Scripted queries completed during this call.
    pub queries: Vec<QueryRecord>,
}

/// A live scenario: peers + hub + clock + the merged action list and its
/// cursor. Holds [`SCENARIO_RUN_LOCK`] and the HLC-source install from
/// [`Self::start`] until [`Self::stop`] or drop.
///
/// `!Send` by construction (it owns a `MutexGuard`): a session lives and
/// dies on the thread that started it (the control bridge's own thread).
pub struct ScenarioSession {
    script: ScenarioScript,
    /// `None` only after [`Self::stop`] took it for teardown.
    run: Option<Run>,
    actions: Vec<Action>,
    cursor: usize,
    readings_planned: usize,
    queries: Vec<QueryRecord>,
    injected: Vec<InjectedFault>,
    anchor_node_ids: HashMap<String, String>,
    host_did: String,
    endpoints_before: u64,
    // Field order IS drop order: peers (in `run`) → HLC uninstall → lock release.
    _hlc_guard: HlcSourceGuard,
    _run_lock: MutexGuard<'static, ()>,
}

impl ScenarioSession {
    /// Setup: spawn peers, build the hierarchy on the ingest host, open the
    /// replica everywhere, publish the manifest, and wait for the fabric to
    /// know every peer. Blocks on [`SCENARIO_RUN_LOCK`] (held for the
    /// session's lifetime). `root_dir` must be fresh per session.
    pub fn start(script: ScenarioScript, root_dir: &Path) -> Result<Self> {
        script.validate()?;
        // The HLC override is process-global (clock.rs): serialize sessions
        // so a concurrent run cannot clobber the active source. Taken first
        // so every error path below releases it LAST.
        let run_lock = SCENARIO_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::create_dir_all(root_dir)?;

        let endpoints_before = fe_sync::bound_endpoint_count();
        let settings = match &script.timeseries {
            Some(ts) => ts.settings()?,
            None => VerseTimeseriesSettings::default(),
        };

        // The clock the whole session reads: sensors, the hub, and (via the
        // process-global HLC source) every reading's stamp.
        let clock = SimClock::new(script.fleet.start_ms);
        // Snapshots the process HLC before installing (restored on drop —
        // DEC-C13; AGENTS.md §hlc-sim).
        let hlc_guard = HlcSourceGuard::install(clock.clone());

        let net = SimNet::new(clock.clone());

        // --- Spawn the peers on the virtual transport (no real network). ---
        let mut peers: BTreeMap<String, SimPeer> = BTreeMap::new();
        for name in &script.fleet.peers {
            let peer = SimPeer::spawn(&net, name, root_dir, script.seed)?;
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
        // Manifest quiescence (AGENTS.md §scenario-runner): the hub fans out
        // only to CURRENT subscribers, so give every replica open a beat
        // before the manifest write. The clock does not move during it.
        std::thread::sleep(Duration::from_millis(500));

        // --- Publish the verse manifest from the host (A3's admission       ---
        // --- precondition; the ts_* columns are how every fabric learns the ---
        // --- placement mode).                                               ---
        let mut manifest_row = serde_json::json!({
            "verse_id": verse_id,
            "name": script.fleet.verse_name,
            "created_by": host_did,
            "created_at": rfc3339_from_ms(clock.now_ms()),
            "namespace_id": ns_id_hex,
            "default_access": "viewer",
        });
        if script.timeseries.is_some() {
            manifest_row["ts_mode"] = settings.mode.as_str().into();
            manifest_row["ts_replication_factor"] = settings.replication_factor.into();
            manifest_row["ts_bucket_width_ms"] = settings.bucket_width_ms.into();
        }
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

        let did_to_name: HashMap<String, String> = peers
            .iter()
            .map(|(name, peer)| (peer.did(), name.clone()))
            .collect();
        let node_to_anchor: HashMap<String, String> = anchor_node_ids
            .iter()
            .map(|(anchor, node)| (node.clone(), anchor.clone()))
            .collect();
        let run = Run {
            clock,
            net,
            peers,
            host_name: host_name.clone(),
            verse_id: verse_id.clone(),
            petal_id,
            origin_ms: script.fleet.start_ms,
            settings,
            ingested: 0,
            did_to_name,
            node_to_anchor,
        };

        // The manifest must converge BEFORE the fleet starts, and every
        // fabric must know every peer's declaration + the mode before the
        // first shard is planned. Settling never advances the clock.
        for name in script.fleet.peers.iter().filter(|p| **p != host_name) {
            run.settle_probe(&format!("verse manifest on {name}"), || {
                let rows = raw_query(
                    &run.peer(name)?.peer,
                    &format!("SELECT verse_id FROM verse WHERE verse_id = '{verse_id}'"),
                )?;
                Ok(!rows.is_empty())
            })?;
        }
        for name in &script.fleet.peers {
            run.settle_probe(&format!("fabric membership on {name}"), || {
                let dump = run.ledger(name)?;
                let peers_known = dump["peers"].as_object().map(|p| p.len()).unwrap_or(0);
                Ok(peers_known == script.fleet.peers.len()
                    && dump["settings"]["mode"].as_str() == Some(run.settings.mode.as_str()))
            })?;
        }

        let plan = plan_fleet(&script.fleet);
        let actions = merge_actions(&script, &plan);
        Ok(Self {
            readings_planned: plan.len(),
            script,
            run: Some(run),
            actions,
            cursor: 0,
            queries: Vec::new(),
            injected: Vec::new(),
            anchor_node_ids,
            host_did,
            endpoints_before,
            _hlc_guard: hlc_guard,
            _run_lock: run_lock,
        })
    }

    /// The script this session plays.
    pub fn script(&self) -> &ScenarioScript {
        &self.script
    }

    /// Faults applied interactively so far (in application order).
    pub fn injected_faults(&self) -> &[InjectedFault] {
        &self.injected
    }

    /// Whether every merged action has executed.
    pub fn is_done(&self) -> bool {
        self.cursor >= self.actions.len()
    }

    fn run(&self) -> &Run {
        self.run
            .as_ref()
            .expect("session run state is present until stop() consumes the session")
    }

    fn clock_offset_ms(&self) -> u64 {
        let run = self.run();
        run.clock.now_ms().saturating_sub(run.origin_ms)
    }

    /// Execute the next `n` merged actions with the one-shot runner's exact
    /// barriers. An error leaves the session mid-action — drop it.
    pub fn step(&mut self, n: u32) -> Result<StepReport> {
        let first_query = self.queries.len();
        let mut executed = 0u32;
        while executed < n && self.cursor < self.actions.len() {
            self.execute(self.cursor)?;
            self.cursor += 1;
            executed += 1;
        }
        Ok(StepReport {
            executed,
            cursor: self.cursor,
            total_actions: self.actions.len(),
            done: self.is_done(),
            clock_offset_ms: self.clock_offset_ms(),
            queries: self.queries[first_query..].to_vec(),
        })
    }

    /// One merged action: advance the clock to it, then apply/query/ingest.
    fn execute(&mut self, index: usize) -> Result<()> {
        let Self {
            script,
            run,
            actions,
            queries,
            anchor_node_ids,
            host_did,
            ..
        } = self;
        let run = run
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("session already stopped"))?;
        let action = &actions[index];
        let at = action.at_ms();
        if at > run.clock.now_ms() {
            run.clock.advance_ms(at - run.clock.now_ms());
            run.net.step();
        }
        match action {
            Action::Event {
                event:
                    ScriptedEvent::Query {
                        at_ms,
                        peer,
                        label,
                        query,
                        timeout_ms,
                    },
                ..
            } => {
                // A query reads stores: settle every peer to its exact
                // convergence target first (draining the network — this may
                // advance the clock to in-flight due times).
                run.settle_stores(true, &format!("pre-query '{label}'"))?;
                let outcome = run.run_query(peer, label, query, *timeout_ms)?;
                queries.push(QueryRecord {
                    label: label.clone(),
                    peer: peer.clone(),
                    at_ms: *at_ms,
                    aggregate: query.is_aggregate(),
                    outcome: run.canonical_outcome(&outcome, query.is_aggregate()),
                });
            }
            Action::Event { event, .. } => {
                // Every ingested row is already in the hub (the tick
                // barrier), so the fault cuts a deterministic in-flight set.
                run.settle_writes()?;
                apply_event(&run.net, &run.peers, event);
            }
            Action::Tick { at_ms, readings } => {
                if *at_ms < run.clock.now_ms() {
                    anyhow::bail!(
                        "simulated time overran the tick at {at_ms} (clock {}): a query's \
                         network drain advanced past it — move the query or lower the latency",
                        run.clock.now_ms()
                    );
                }
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
                    return Ok(());
                }
                let expected = batch.len();
                let want_petal = run.petal_id.clone();
                // Correlated per merged action (the `sim-anchor:` idiom).
                let want_correlation = format!("sim-ingest:{index}");
                let host = run.peer(&run.host_name)?;
                host.peer.send(DbCommand::InsertIotReadings {
                    petal_id: run.petal_id.clone(),
                    verse_id: Some(run.verse_id.clone()),
                    source_did: host_did.clone(),
                    readings: batch,
                    correlation_id: Some(want_correlation.clone()),
                });
                let written = match host.peer.wait_for(
                    |r| {
                        matches!(r, DbResult::IotReadingsInserted { ref petal_id, ref correlation_id, .. }
                            if petal_id == &want_petal
                                && correlation_id.as_deref() == Some(want_correlation.as_str()))
                    },
                    DB_REPLY_BUDGET,
                )? {
                    DbResult::IotReadingsInserted { written, .. } => written,
                    other => anyhow::bail!("unexpected ingest result: {other:?}"),
                };
                if written != expected {
                    anyhow::bail!("ingest wrote {written} of {expected} readings");
                }
                run.ingested += written;
                // The rows reach the hub through the DB→sync bridge; wait for
                // that write (its due-time is this tick's millisecond), then
                // drain what is due.
                run.settle_writes()?;
                run.net.step();
            }
        }
        Ok(())
    }

    /// Reject a fault this session cannot apply (queries are scripted
    /// actions, not faults; peer names must be fleet peers). Pure check.
    pub fn check_fault(&self, event: &ScriptedEvent) -> Result<()> {
        if let ScriptedEvent::Query { .. } = event {
            anyhow::bail!("query events are scripted actions, not faults — script them instead");
        }
        if let ScriptedEvent::SetLatency { latency_ms, .. } = event {
            self.script.check_time_limit(*latency_ms)?;
        }
        self.script.validate_event_peers(event)
    }

    /// Apply `event` NOW (its `at_ms` is ignored) behind the same write
    /// barrier a scripted fault gets; recorded with the actual sim offset.
    pub fn inject_fault(&mut self, event: ScriptedEvent) -> Result<InjectedFault> {
        self.check_fault(&event)?;
        let run = self.run();
        run.settle_writes()?;
        apply_event(&run.net, &run.peers, &event);
        let fault = InjectedFault {
            at_ms: self.clock_offset_ms(),
            cursor: self.cursor,
            event,
        };
        tracing::info!(?fault, "sim session: fault injected");
        self.injected.push(fault.clone());
        Ok(fault)
    }

    /// Snapshot the session. Drains queued sync events (so idle sync
    /// threads never stay blocked on a full event channel) but never steps
    /// the hub or the clock — a status call cannot perturb the run.
    pub fn status(&self) -> SessionStatus {
        let run = self.run();
        run.drain_events();
        SessionStatus {
            name: self.script.name.clone(),
            ingest_peer: run.host_name.clone(),
            peers: run
                .peers
                .iter()
                .map(|(name, peer)| {
                    let did = peer.did();
                    PeerStatus {
                        name: name.clone(),
                        online: run.net.is_peer_online(&did),
                        did,
                    }
                })
                .collect(),
            cursor: self.cursor,
            total_actions: self.actions.len(),
            done: self.is_done(),
            next_action_at_ms: self
                .actions
                .get(self.cursor)
                .map(|a| a.at_ms().saturating_sub(run.origin_ms)),
            clock_offset_ms: self.clock_offset_ms(),
            readings_ingested: run.ingested,
            readings_planned: self.readings_planned,
            queries_recorded: self.queries.len(),
            faults_injected: self.injected.clone(),
            latency_ms: run.net.latency_ms(),
            active_partitions: run.net.active_partitions(),
            inflight_deliveries: run.net.inflight_count(),
            dropped_deliveries: run.net.dropped_deliveries(),
            gossip_deliveries: run.net.gossip_deliveries(),
            endpoints_bound: fe_sync::bound_endpoint_count().saturating_sub(self.endpoints_before),
        }
    }

    /// Settle every peer to its exact target, fingerprint the stores and
    /// the final placement, tear down, and release the run lock.
    pub fn stop(mut self) -> Result<ScenarioOutcome> {
        let run = self.run();
        run.settle_stores(true, "final convergence")?;

        // --- Fingerprint every peer's durable store + the final placement. --
        let mut per_peer: BTreeMap<String, Vec<CanonicalReading>> = BTreeMap::new();
        for (name, peer) in &run.peers {
            let rows = raw_query(&peer.peer, "SELECT * FROM iot_reading")?;
            let mut canonical: Vec<CanonicalReading> = rows
                .iter()
                .map(|row| CanonicalReading {
                    anchor: run.anchor_name(row["node_id"].as_str().unwrap_or_default()),
                    metric: row["metric"].as_str().unwrap_or_default().to_string(),
                    units: row["units"].as_str().unwrap_or_default().to_string(),
                    recorded_at_ms: row["recorded_at_ms"].as_i64().unwrap_or_default(),
                    value_bits: row["value"].as_f64().unwrap_or_default().to_bits(),
                    hlc_wall_ms: row["hlc_timestamp"]
                        .as_i64()
                        .map(|h| (h >> 16) as u64)
                        .unwrap_or_default(),
                })
                .collect();
            canonical.sort();
            per_peer.insert(name.clone(), canonical);
        }
        let mut placement: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (shard, hosts) in run.ledger_hosts()? {
            let mut names: Vec<String> = hosts.iter().map(|d| run.peer_name(d)).collect();
            names.sort();
            placement.insert(run.canonical_shard(&shard), names);
        }
        let peer_dids: BTreeMap<String, String> = run
            .peers
            .iter()
            .map(|(name, peer)| (name.clone(), peer.did()))
            .collect();

        let run = self
            .run
            .take()
            .ok_or_else(|| anyhow::anyhow!("session already stopped"))?;
        let ingested = run.ingested;
        let net = run.net.clone();
        shutdown(run);

        let endpoints_after = fe_sync::bound_endpoint_count();
        Ok(ScenarioOutcome {
            name: self.script.name.clone(),
            ingested,
            per_peer,
            placement,
            queries: std::mem::take(&mut self.queries),
            peer_dids,
            dropped_deliveries: net.dropped_deliveries(),
            gossip_deliveries: net.gossip_deliveries(),
            endpoints_before: self.endpoints_before,
            endpoints_after,
        })
        // `self` drops here: HLC uninstall, then the run lock releases.
    }
}

impl Drop for ScenarioSession {
    /// Abandoned (error / never stopped): still close replicas and join
    /// the peers before the HLC guard and the run lock release.
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            shutdown(run);
        }
    }
}

/// Clean shutdown: drain, close every replica, give the sync threads a
/// beat, drain again, then drop the peers (their threads join while
/// draining — `TestPeer::shutdown_inner`). Never a blocking send: an
/// abandoned session (Drop) may hold sync threads parked on full event
/// channels whose command channels are full too.
fn shutdown(run: Run) {
    run.drain_events();
    for peer in run.peers.values() {
        if let Err(e) = peer
            .peer
            .sync_cmd_tx
            .try_send(SyncCommand::CloseVerseReplica {
                verse_id: run.verse_id.clone(),
            })
        {
            // Teardown proceeds regardless (the peer's Shutdown follows).
            tracing::warn!("scenario shutdown: CloseVerseReplica not sent: {e}");
        }
    }
    std::thread::sleep(SETTLE_POLL);
    run.drain_events();
    drop(run);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{default_script, offline_degraded_script, run_scenario};

    /// The anti-drift proof: driving the default script through the
    /// interactive surface — uneven step chunks with a status snapshot
    /// between every call — yields the IDENTICAL canonical fingerprint as
    /// the one-shot runner.
    #[test]
    fn stepped_session_matches_one_shot_run_exactly() {
        let script = default_script();
        let dir_a = tempfile::tempdir().expect("one-shot tempdir");
        let one_shot = run_scenario(&script, dir_a.path()).expect("one-shot run");

        let dir_b = tempfile::tempdir().expect("session tempdir");
        let mut session = ScenarioSession::start(script.clone(), dir_b.path()).expect("start");
        let total = session.status().total_actions;
        assert!(total > 3, "the default script has several merged actions");
        let mut chunk = 1u32;
        let mut calls = 0;
        while !session.is_done() {
            let before = session.status();
            let report = session.step(chunk).expect("step");
            assert_eq!(
                report.cursor,
                before.cursor + report.executed as usize,
                "a step executes exactly the actions it reports"
            );
            assert!(report.executed >= 1 && report.executed <= chunk);
            chunk = chunk % 3 + 1;
            calls += 1;
        }
        assert!(calls > 1, "the run was actually driven in several calls");
        let status = session.status();
        assert_eq!((status.cursor, status.done), (total, true));
        assert_eq!(status.endpoints_bound, 0, "a sim session binds no endpoint");
        // Stepping past the end is an honest no-op.
        assert_eq!(session.step(5).expect("step past end").executed, 0);

        let interactive = session.stop().expect("stop");
        assert_eq!(
            interactive.canonical_fingerprint(),
            one_shot.canonical_fingerprint(),
            "start/step*/stop must reproduce run_scenario exactly"
        );
        assert_eq!(interactive.ingested, 21);
    }

    /// DEC-C13: a past-dated session resets the process-global HLC to sim
    /// time (`init_hlc(0)` per peer spawn); teardown must hand it back no
    /// lower than it found it. The pre-session state runs AHEAD of the wall
    /// clock (a production DB that persisted stamps under clock skew), so
    /// real time alone would NOT restore monotonicity — only the snapshot
    /// does.
    #[test]
    fn past_time_session_restores_the_process_hlc() {
        let before = {
            let _lock = SCENARIO_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let ahead_wall = crate::fleet::real_now_ms() + 120_000;
            fe_database::op_log::init_hlc((ahead_wall << 16) | 3);
            fe_database::op_log::next_hlc_timestamp().0
        };
        // (Any session that runs in between also restores ≥ its snapshot,
        // so the chain stays monotonic.)
        let script = ScenarioScript::parse(
            r#"{
                "name": "sim-hlc-restore",
                "fleet": {
                    "verse_name": "HLC Restore",
                    "peers": ["solo"],
                    "ingest_peer": "solo",
                    "anchors": ["tower-a"],
                    "sensors": [
                        { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                          "cadence_ms": 60000,
                          "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                     "period_ms": 3600000 } }
                    ],
                    "start_ms": 1600000000000,
                    "duration_ms": 120000
                }
            }"#,
        )
        .expect("past-dated scenario parses");
        let dir = tempfile::tempdir().expect("tempdir");
        let outcome = run_scenario(&script, dir.path()).expect("past-time run");
        assert!(
            outcome.per_peer["solo"]
                .iter()
                .all(|r| r.hlc_wall_ms < 1_600_000_200_000),
            "the session really stamped (past) sim time"
        );

        let _lock = SCENARIO_RUN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (after, _) = fe_database::op_log::next_hlc_timestamp();
        assert!(
            after > before,
            "post-session production stamp {after} must exceed the pre-session stamp {before}"
        );
    }

    /// Advance until the next action sits at simulated offset `at_ms`, then
    /// execute it (a no-op `set_latency` marker that moves the clock there).
    fn step_through_marker(session: &mut ScenarioSession, at_ms: u64) {
        while session.status().next_action_at_ms != Some(at_ms) {
            assert!(!session.is_done(), "marker at {at_ms} never reached");
            session.step(1).expect("step to marker");
        }
        session.step(1).expect("execute marker");
        assert_eq!(session.status().clock_offset_ms, at_ms);
    }

    /// Interactive faults change what the fleet observes — and are exactly
    /// a scripted fault at the same instant: the A21 outage scenario with its
    /// two churn events REPLACED by no-op markers, driven step-wise with the
    /// outage injected live, reproduces the scripted run's fingerprint
    /// (degraded query, convergence, healed query) bit for bit.
    #[test]
    fn injected_outage_degrades_then_heals_like_the_scripted_one() {
        let scripted = offline_degraded_script();
        let dir_a = tempfile::tempdir().expect("scripted tempdir");
        let expected = run_scenario(&scripted, dir_a.path()).expect("scripted run");

        let mut live_script = scripted.clone();
        live_script.name = "sim-offline-degraded-live".into();
        for event in &mut live_script.events {
            match event {
                ScriptedEvent::PeerOffline { at_ms, .. }
                | ScriptedEvent::PeerOnline { at_ms, .. } => {
                    *event = ScriptedEvent::SetLatency {
                        at_ms: *at_ms,
                        latency_ms: 0,
                    };
                }
                _ => {}
            }
        }
        let dir_b = tempfile::tempdir().expect("session tempdir");
        let mut session = ScenarioSession::start(live_script, dir_b.path()).expect("start");

        step_through_marker(&mut session, 300_000);
        let alice = ScriptedEvent::PeerOffline {
            at_ms: 0,
            peer: "alice".into(),
        };
        let fault = session.inject_fault(alice).expect("inject offline");
        assert_eq!(fault.at_ms, 300_000, "applied at the CURRENT sim instant");
        let status = session.status();
        let alice_status = status.peers.iter().find(|p| p.name == "alice").unwrap();
        assert!(!alice_status.online, "status reflects the injected outage");
        assert_eq!(status.faults_injected.len(), 1);

        // The next action is the degraded query: alice's shards are missing.
        let report = session.step(1).expect("degraded query");
        let degraded = &report.queries[0];
        assert_eq!(degraded.label, "degraded-aggregate");
        assert!(!degraded.outcome.missing_shards.is_empty());
        assert_eq!(degraded.outcome.missing_hosts, ["alice"]);

        step_through_marker(&mut session, 450_000);
        session
            .inject_fault(ScriptedEvent::PeerOnline {
                at_ms: 0,
                peer: "alice".into(),
            })
            .expect("inject online");
        let report = session.step(u32::MAX).expect("run to end");
        assert!(report.done);
        let healed = report
            .queries
            .iter()
            .find(|q| q.label == "healed-aggregate")
            .expect("healed query ran");
        assert!(healed.outcome.missing_shards.is_empty(), "recovered");

        assert_eq!(session.injected_faults().len(), 2);
        let live = session.stop().expect("stop");
        assert_eq!(
            live.canonical_fingerprint(),
            expected.canonical_fingerprint(),
            "a live-injected outage must equal the scripted outage at the same instants"
        );
    }
}
