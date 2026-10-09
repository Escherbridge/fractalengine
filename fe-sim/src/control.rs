//! The sim control bridge (F9/A20): services `SimControlCall`s from the
//! API seam on one dedicated thread that owns the (at most one) live
//! [`ScenarioSession`] — see `fe-sim/src/AGENTS.md` §session.

use std::path::PathBuf;

use fe_runtime::sim_control::{
    SimControlCallReceiver, SimControlCommand, SimControlError, SimControlErrorKind,
    SimControlResult, MAX_SIM_STEP,
};

use crate::scenario::{
    default_script, offline_degraded_script, sharded_query_script, ScenarioScript, ScriptedEvent,
};
use crate::session::ScenarioSession;

/// Built-in script names `Start` accepts as a bare string.
pub const BUILTIN_SCRIPTS: [&str; 3] = ["default", "sharded_query", "offline_degraded"];

/// The live session plus its scratch directory (removed after the session
/// drops — field order is drop order).
struct LiveSession {
    session: ScenarioSession,
    _dir: tempfile::TempDir,
}

/// Single-owner session state + verb dispatch (runs on the bridge thread).
pub struct SimControlBridge {
    work_root: PathBuf,
    live: Option<LiveSession>,
}

fn error(kind: SimControlErrorKind, message: impl Into<String>) -> SimControlError {
    SimControlError::new(kind, message)
}

fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

impl SimControlBridge {
    /// Sessions get fresh scratch dirs under `work_root`.
    pub fn new(work_root: PathBuf) -> Self {
        Self {
            work_root,
            live: None,
        }
    }

    /// Whether a session is live.
    pub fn is_running(&self) -> bool {
        self.live.is_some()
    }

    /// Service one verb. A driver failure tears the session down (its run
    /// is no longer a valid deterministic run) and reports `Failed`.
    pub fn handle(&mut self, cmd: SimControlCommand) -> SimControlResult {
        match cmd {
            SimControlCommand::Start { script } => self.start(script),
            SimControlCommand::Stop => self.stop(),
            SimControlCommand::Status => Ok(match &self.live {
                Some(live) => {
                    let mut status = to_json(&live.session.status());
                    status["running"] = true.into();
                    status
                }
                None => serde_json::json!({ "running": false }),
            }),
            SimControlCommand::Step { n } => self.step(n),
            SimControlCommand::InjectFault { event } => self.inject_fault(event),
        }
    }

    fn start(&mut self, script: serde_json::Value) -> SimControlResult {
        // Reject BEFORE parsing or touching SCENARIO_RUN_LOCK: this thread
        // holds the lock through the live session, so a second start would
        // otherwise deadlock the bridge on its own lock.
        if let Some(live) = &self.live {
            return Err(error(
                SimControlErrorKind::Conflict,
                format!(
                    "a sim session is already running ('{}') — stop it first",
                    live.session.script().name
                ),
            ));
        }
        let script = parse_script(script).map_err(|e| error(SimControlErrorKind::BadRequest, e))?;
        std::fs::create_dir_all(&self.work_root)
            .and_then(|()| tempfile::tempdir_in(&self.work_root))
            .map_err(|e| {
                error(
                    SimControlErrorKind::Failed,
                    format!("sim work dir unavailable: {e}"),
                )
            })
            .and_then(|dir| {
                tracing::info!(scenario = %script.name, dir = %dir.path().display(), "sim control: starting session");
                let session = ScenarioSession::start(script, dir.path()).map_err(|e| {
                    error(
                        SimControlErrorKind::Failed,
                        format!("session start failed: {e:#}"),
                    )
                })?;
                let status = session.status();
                self.live = Some(LiveSession { session, _dir: dir });
                Ok(serde_json::json!({ "started": status.name, "status": to_json(&status) }))
            })
    }

    fn stop(&mut self) -> SimControlResult {
        let Some(LiveSession { session, _dir }) = self.live.take() else {
            return Err(not_running());
        };
        let injected = session.injected_faults().to_vec();
        let outcome = session.stop().map_err(|e| {
            error(
                SimControlErrorKind::Failed,
                format!("stop failed (session torn down): {e:#}"),
            )
        })?;
        Ok(serde_json::json!({
            "name": outcome.name,
            "ingested": outcome.ingested,
            "dropped_deliveries": outcome.dropped_deliveries,
            "gossip_deliveries": outcome.gossip_deliveries,
            "endpoints_before": outcome.endpoints_before,
            "endpoints_after": outcome.endpoints_after,
            "peer_dids": outcome.peer_dids,
            "queries": to_json(&outcome.queries),
            "injected_faults": to_json(&injected),
            "fingerprint": to_json(&outcome.canonical_fingerprint()),
        }))
    }

    fn step(&mut self, n: u32) -> SimControlResult {
        if n == 0 || n > MAX_SIM_STEP {
            return Err(error(
                SimControlErrorKind::BadRequest,
                format!("step n must be 1..={MAX_SIM_STEP}"),
            ));
        }
        let live = self.live.as_mut().ok_or_else(not_running)?;
        match live.session.step(n) {
            Ok(report) => Ok(to_json(&report)),
            Err(e) => {
                self.live = None;
                Err(error(
                    SimControlErrorKind::Failed,
                    format!("step failed (session torn down): {e:#}"),
                ))
            }
        }
    }

    fn inject_fault(&mut self, event: serde_json::Value) -> SimControlResult {
        let live = self.live.as_mut().ok_or_else(not_running)?;
        let event = parse_fault(event).map_err(|e| error(SimControlErrorKind::BadRequest, e))?;
        live.session
            .check_fault(&event)
            .map_err(|e| error(SimControlErrorKind::BadRequest, format!("{e:#}")))?;
        match live.session.inject_fault(event) {
            Ok(fault) => Ok(to_json(&fault)),
            Err(e) => {
                self.live = None;
                Err(error(
                    SimControlErrorKind::Failed,
                    format!("fault injection failed (session torn down): {e:#}"),
                ))
            }
        }
    }
}

fn not_running() -> SimControlError {
    error(
        SimControlErrorKind::NotRunning,
        "no sim session is running — start one first",
    )
}

/// `script`: a scenario object, scenario JSON text, or a built-in name.
fn parse_script(script: serde_json::Value) -> Result<ScenarioScript, String> {
    match script {
        serde_json::Value::String(text) if text.trim_start().starts_with('{') => {
            ScenarioScript::parse(&text).map_err(|e| format!("{e:#}"))
        }
        serde_json::Value::String(name) => match name.as_str() {
            "default" => Ok(default_script()),
            "sharded_query" => Ok(sharded_query_script()),
            "offline_degraded" => Ok(offline_degraded_script()),
            other => Err(format!(
                "unknown built-in script '{other}' (expected one of {BUILTIN_SCRIPTS:?})"
            )),
        },
        value @ serde_json::Value::Object(_) => {
            let script: ScenarioScript =
                serde_json::from_value(value).map_err(|e| format!("invalid scenario: {e}"))?;
            script.validate().map_err(|e| format!("{e:#}"))?;
            Ok(script)
        }
        _ => Err("script must be a scenario object, scenario JSON text, or a built-in name".into()),
    }
}

/// A `ScriptedEvent` object; a missing `at_ms` defaults to 0 (ignored —
/// injected faults apply now).
fn parse_fault(event: serde_json::Value) -> Result<ScriptedEvent, String> {
    let serde_json::Value::Object(mut object) = event else {
        return Err("event must be a ScriptedEvent object (e.g. {\"kind\":\"peer_offline\",\"peer\":\"alice\"})".into());
    };
    object.entry("at_ms").or_insert(0.into());
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|e| format!("invalid fault event: {e}"))
}

/// Spawn the bridge thread: services calls serially until every sender is
/// dropped, then tears down any live session.
pub fn spawn_sim_control_bridge(
    rx: SimControlCallReceiver,
    work_root: PathBuf,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("fe-sim-control".into())
        .spawn(move || {
            let mut bridge = SimControlBridge::new(work_root);
            while let Ok(call) = rx.recv() {
                let verb = call.cmd.verb();
                let result = bridge.handle(call.cmd);
                if let Err(e) = &result {
                    tracing::warn!(verb, kind = ?e.kind, "sim control call failed: {e}");
                }
                if call.reply_tx.send(result).is_err() {
                    tracing::warn!(
                        verb,
                        "sim control reply dropped: the caller gave up waiting"
                    );
                }
            }
            if bridge.is_running() {
                tracing::warn!("sim control seam closed with a live session — tearing it down");
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::sim_control::{sim_control_channel, SimControlCall, SimControlCallSender};
    use std::time::Duration;

    /// Generous: a call may queue behind other tests' scenario runs on the
    /// process-global run lock.
    const CALL_BUDGET: Duration = Duration::from_secs(600);

    /// A small one-peer fleet (3 ticks) — cheap to start and stop.
    fn solo_script() -> serde_json::Value {
        serde_json::json!({
            "name": "sim-bridge-solo",
            "fleet": {
                "verse_name": "Bridge Solo",
                "peers": ["solo"],
                "ingest_peer": "solo",
                "anchors": ["tower-a"],
                "sensors": [
                    { "anchor": "tower-a", "metric": "temperature_c", "units": "C",
                      "cadence_ms": 60000,
                      "model": { "type": "sine", "baseline": 15, "amplitude": 8,
                                 "period_ms": 3600000 } }
                ],
                "start_ms": 1750000000000u64,
                "duration_ms": 180000
            }
        })
    }

    fn call(tx: &SimControlCallSender, cmd: SimControlCommand) -> SimControlResult {
        let (reply_tx, reply_rx) = crossbeam::channel::bounded(1);
        tx.send(SimControlCall { cmd, reply_tx })
            .expect("bridge is alive");
        reply_rx
            .recv_timeout(CALL_BUDGET)
            .expect("bridge answered (a leaked run lock would hang here)")
    }

    fn spawn() -> (
        SimControlCallSender,
        tempfile::TempDir,
        std::thread::JoinHandle<()>,
    ) {
        let work = tempfile::tempdir().expect("work root");
        let (tx, rx) = sim_control_channel();
        let handle = spawn_sim_control_bridge(rx, work.path().to_path_buf()).expect("spawn");
        (tx, work, handle)
    }

    /// A second start while a session is live is a clean Conflict (no
    /// deadlock on the run lock the bridge itself holds), and the first
    /// session keeps working through status/step/inject/stop.
    #[test]
    fn concurrent_start_is_rejected_and_the_live_session_keeps_working() {
        let (tx, _work, handle) = spawn();
        assert_eq!(
            call(&tx, SimControlCommand::Status).expect("idle status"),
            serde_json::json!({ "running": false })
        );
        let not_running = call(&tx, SimControlCommand::Step { n: 1 }).unwrap_err();
        assert_eq!(not_running.kind, SimControlErrorKind::NotRunning);

        let started = call(
            &tx,
            SimControlCommand::Start {
                script: solo_script(),
            },
        )
        .expect("first start");
        assert_eq!(started["started"], "sim-bridge-solo");

        let second = call(
            &tx,
            SimControlCommand::Start {
                script: serde_json::json!("default"),
            },
        )
        .unwrap_err();
        assert_eq!(second.kind, SimControlErrorKind::Conflict);
        assert!(second.message.contains("already running"), "{second}");

        // Malformed requests are BadRequest and leave the session intact.
        for (event, why) in [
            (
                serde_json::json!({ "kind": "peer_offline", "peer": "mallory" }),
                "unknown peer",
            ),
            (
                serde_json::json!({ "kind": "query", "peer": "solo", "label": "q",
                                    "query": { "type": "all_readings" } }),
                "queries are not faults",
            ),
            (serde_json::json!("peer_offline"), "not an object"),
        ] {
            let err = call(&tx, SimControlCommand::InjectFault { event }).unwrap_err();
            assert_eq!(err.kind, SimControlErrorKind::BadRequest, "{why}: {err}");
        }
        let zero = call(&tx, SimControlCommand::Step { n: 0 }).unwrap_err();
        assert_eq!(zero.kind, SimControlErrorKind::BadRequest);

        // The first session is still the live one and still drives.
        let status = call(&tx, SimControlCommand::Status).expect("status");
        assert_eq!(status["running"], true);
        assert_eq!(status["name"], "sim-bridge-solo");
        assert_eq!(status["cursor"], 0);
        let step = call(&tx, SimControlCommand::Step { n: 2 }).expect("step");
        assert_eq!(step["executed"], 2);
        let fault = call(
            &tx,
            SimControlCommand::InjectFault {
                event: serde_json::json!({ "kind": "set_latency", "latency_ms": 250 }),
            },
        )
        .expect("inject");
        assert_eq!(fault["at_ms"], 120_000, "applied at the current sim offset");
        assert_eq!(
            call(&tx, SimControlCommand::Status).expect("status")["latency_ms"],
            250
        );

        let stopped = call(&tx, SimControlCommand::Stop).expect("stop");
        assert_eq!(
            stopped["ingested"], 2,
            "stop fingerprints what ran — it never auto-plays"
        );
        assert_eq!(stopped["injected_faults"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            stopped["endpoints_before"], stopped["endpoints_after"],
            "no real network"
        );
        drop(tx);
        handle.join().expect("bridge exits when the seam closes");
    }

    /// Stop releases the process-global run lock: a fresh start on the
    /// same bridge succeeds (a leaked lock would block it forever — the
    /// call budget turns that into a failure).
    #[test]
    fn stop_releases_the_run_lock_so_a_new_start_succeeds() {
        let (tx, _work, handle) = spawn();
        for round in 0..2 {
            call(
                &tx,
                SimControlCommand::Start {
                    script: solo_script(),
                },
            )
            .unwrap_or_else(|e| panic!("start round {round}: {e}"));
            let report = call(&tx, SimControlCommand::Step { n: 100 }).expect("step");
            assert_eq!(report["done"], true);
            let stopped = call(&tx, SimControlCommand::Stop).expect("stop");
            assert_eq!(stopped["ingested"], 3, "round {round}: every tick landed");
            let again = call(&tx, SimControlCommand::Stop).unwrap_err();
            assert_eq!(again.kind, SimControlErrorKind::NotRunning);
        }
        drop(tx);
        handle.join().expect("bridge exits");
    }
}
