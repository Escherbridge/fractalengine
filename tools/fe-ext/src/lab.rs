//! `run --lab`: the packaged fleet-sim component hosted as the fe-sim reading schedule (R30).
//! Rationale and the fingerprint contract: tools/fe-ext/AGENTS.md §lab.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use fe_plugin::capability::{CapabilityToken, V2Binding};
use fe_plugin::context::PluginContext;
use fe_plugin::tick::{self, Clock};
use fe_plugin::transaction::{StagedCommitSink, StagedFlush};
use fe_plugin::wasm::component::{window_for_token, Binding, ComponentHost, InstanceSetup, KvSlot, MemoryKv};
use fe_plugin_test::mock_host::MockHostEnv;
use fe_sdk::node::NodeSnapshot;
use fe_sdk::plugin_host::{json_property, PendingOp, StagedTimestamp, TransactionReceipt};
use fe_sim::fleet::{FleetConfig, ScheduledReading};
use fe_sim::scenario::ScenarioScript;
use fe_sim::session::{FleetSource, LabClock, ScenarioSession};
use fe_sim_model::fleet::plan_readings;
use serde_json::json;

use crate::project::Project;
use crate::validate;

/// Ticks the hosted package may be stepped for one plan (a runaway duration is refused, not looped).
const MAX_LAB_TICKS: u64 = 200_000;
/// Petal the package is bound to inside the hosting scene.
const LAB_PETAL: &str = "lab";
/// Identity the package binds as while it plans.
const LAB_DID: &str = "did:key:lab";

/// What `fe-ext run --lab` was asked to do.
#[derive(Debug, Clone)]
pub struct LabArgs {
    /// fe-sim scenario script (JSON); its fleet is the one the package plans.
    pub scenario: PathBuf,
    /// The fleet-sim extension project (`extension.json`, built component).
    pub project: PathBuf,
    /// Component override (default: the project's built component).
    pub component: Option<PathBuf>,
    /// Parent for the per-run session directories (default: a pid-named directory under the OS temp dir).
    pub root: Option<PathBuf>,
    /// Sessions to run back to back; every fingerprint must equal the first.
    pub runs: u32,
    /// Write the report JSON here.
    pub out: Option<PathBuf>,
}

/// What a lab run produced.
pub struct LabReport {
    pub fingerprint: String,
    pub readings_planned: usize,
    pub readings_ingested: usize,
    pub runs: u32,
    pub summary: Vec<String>,
}

/// The lab's SimClock as the plugin scheduler's clock: the session origin plus the planner's own steps.
struct LabClockAdapter {
    lab: LabClock,
    stepped_ms: Arc<AtomicU64>,
}

impl Clock for LabClockAdapter {
    fn now_ms(&self) -> u64 {
        self.lab.now_ms().saturating_add(self.stepped_ms.load(Ordering::SeqCst))
    }
}

/// One readings row as the planner's sink recorded it.
struct PlannedRow {
    node_id: String,
    metric: String,
    units: String,
    value: f64,
    at_ms: u64,
}

/// Commits staged windows into the hosting scene atomically; same op rules as the sandbox sink.
struct LabSink {
    host: Arc<Mutex<MockHostEnv>>,
    rows: Arc<Mutex<Vec<PlannedRow>>>,
    created: u64,
    now_ms: Arc<dyn Clock>,
}

impl StagedCommitSink for LabSink {
    fn commit_staged(&mut self, petal_id: &str, flush: &StagedFlush) -> Result<TransactionReceipt, String> {
        let mut host = self.host.lock().map_err(|_| "lab scene lock poisoned".to_string())?;
        // Work on a copy so a refused flush leaves the scene untouched.
        let mut nodes = host.nodes.clone();
        let mut created = self.created;
        let mut created_node_ids = Vec::new();
        for op in &flush.ops {
            match op {
                PendingOp::SetProperty { node_id, key, value } => {
                    let node = nodes
                        .get_mut(node_id)
                        .filter(|node| node.petal_id == petal_id)
                        .ok_or_else(|| "node not found or not permitted".to_string())?;
                    node.properties.insert(key.clone(), json_property(value.clone()));
                }
                PendingOp::CreateNode { petal_id: target, name } => {
                    if target != petal_id {
                        return Err("node creation targets another petal".into());
                    }
                    created += 1;
                    let node_id = format!("lab-{created:06}");
                    nodes.insert(
                        node_id.clone(),
                        NodeSnapshot {
                            node_id: node_id.clone(),
                            petal_id: target.clone(),
                            name: name.clone(),
                            position: [0.0; 3],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0; 3],
                            properties: HashMap::new(),
                        },
                    );
                    created_node_ids.push(node_id);
                }
                PendingOp::DeleteNode { node_id } => {
                    if !nodes.get(node_id).is_some_and(|node| node.petal_id == petal_id) {
                        return Err("node not found or not permitted".into());
                    }
                    nodes.remove(node_id);
                }
                PendingOp::SetTransform { node_id, position, rotation, scale } => {
                    let node = nodes
                        .get_mut(node_id)
                        .filter(|node| node.petal_id == petal_id)
                        .ok_or_else(|| "node not found or not permitted".to_string())?;
                    if let Some(position) = position {
                        node.position = *position;
                    }
                    if let Some(rotation) = rotation {
                        node.rotation = *rotation;
                    }
                    if let Some(scale) = scale {
                        node.scale = *scale;
                    }
                }
            }
        }
        let flushed_at = self.now_ms.now_ms();
        let mut rows = Vec::with_capacity(flush.readings.len());
        for reading in &flush.readings {
            if !nodes.get(&reading.node_id).is_some_and(|node| node.petal_id == petal_id) {
                return Err(format!("reading anchor {} is not a node of the bound petal", reading.node_id));
            }
            let at_ms = match &reading.recorded_at {
                None => flushed_at,
                Some(StagedTimestamp::EpochMs(ms)) => u64::try_from(*ms).map_err(|_| "negative reading timestamp".to_string())?,
                Some(StagedTimestamp::Rfc3339(_)) => return Err("lab plans use epoch-ms reading timestamps".into()),
            };
            rows.push(PlannedRow {
                node_id: reading.node_id.clone(),
                metric: reading.metric.clone(),
                units: reading.units.clone(),
                value: reading.value,
                at_ms,
            });
        }
        host.nodes = nodes;
        self.created = created;
        let reading_count = rows.len();
        self.rows.lock().map_err(|_| "lab rows lock poisoned".to_string())?.extend(rows);
        Ok(TransactionReceipt { op_count: flush.ops.len(), created_node_ids, reading_count })
    }
}

/// A [`FleetSource`] that steps the fleet-sim component under the lab clock and returns what it staged.
pub struct HostedFleetSource {
    manifest_json: Vec<u8>,
    component: Vec<u8>,
}

impl HostedFleetSource {
    pub fn new(manifest_json: Vec<u8>, component: Vec<u8>) -> Self {
        Self { manifest_json, component }
    }
}

impl FleetSource for HostedFleetSource {
    fn plan(&mut self, fleet: &FleetConfig, clock: LabClock) -> Result<Vec<ScheduledReading>> {
        let validated = validate::validate_manifest(&self.manifest_json)?;
        let manifest = &validated.manifest;
        // Movers are not driven in the lab (a plan carries sensors only); dropping them also spares wasm work.
        let mut model = fleet.model();
        model.movers.clear();
        model.validate().map_err(|error| anyhow!("lab fleet: {error}"))?;

        // The hosting scene: one anchor per fleet anchor (id == name) and the fleet node the package drives.
        let mut mock = MockHostEnv::new();
        for anchor in &model.anchors {
            mock.insert_node(anchor.clone(), lab_node(anchor, anchor, HashMap::new()));
        }
        let mut properties = HashMap::new();
        properties.insert("fleet.config".to_string(), json_property(serde_json::to_value(&model)?));
        mock.insert_node("fleet-1", lab_node("fleet-1", "fleet", properties));
        let host = Arc::new(Mutex::new(mock));

        let scopes = V2Binding {
            verse_scope: "VERSE#lab".into(),
            petal_scope: format!("VERSE#lab-FRACTAL#lab-PETAL#{LAB_PETAL}"),
            source_class: None,
        };
        let token = CapabilityToken::mint_v2(&manifest.id, manifest, &scopes).map_err(|error| anyhow!("binding refused: {error}"))?;
        let readings_class = token.grants_v2.as_ref().and_then(|grants| grants.readings_class);
        let budget = token.tick_budget().cloned();
        let cadence_ms = budget.as_ref().map_or(1000, |budget| budget.cadence_ms).max(1);
        let hook_fuel = budget.as_ref().map_or(tick::DEFAULT_TICK_FUEL, |budget| budget.fuel);
        let window = window_for_token(&token);

        // Origin tick claims the fleet, the next tick that reads the claim back starts driving; the last
        // tick lands strictly after the fleet window so every due sensor tick has fired.
        let ticks = model.duration_ms.div_ceil(cadence_ms) + 3;
        anyhow::ensure!(ticks <= MAX_LAB_TICKS, "fleet duration needs {ticks} package ticks, the lab cap is {MAX_LAB_TICKS}");

        let stepped_ms = Arc::new(AtomicU64::new(0));
        let adapter: Arc<dyn Clock> = Arc::new(LabClockAdapter { lab: clock, stepped_ms: stepped_ms.clone() });
        anyhow::ensure!(adapter.now_ms() == model.start_ms, "the lab clock must start at the fleet origin");

        let rows = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Mutex::new(LabSink { host: host.clone(), rows: rows.clone(), created: 0, now_ms: adapter.clone() }));
        let (commands, _commands_receiver) = crossbeam::channel::bounded(1);
        let context = PluginContext::new(manifest.id.clone(), LAB_PETAL.to_string(), token, commands)
            .with_node_reader(host.clone())
            .with_staged_sink(sink);
        let binding = Binding {
            installation_id: "lab-1".into(),
            package_id: manifest.id.clone(),
            content_hash: blake3::hash(&self.manifest_json).to_hex().to_string(),
            verse_id: "lab".into(),
            fractal_id: "lab".into(),
            petal_id: LAB_PETAL.into(),
            identity_did: LAB_DID.into(),
            source_class: readings_class
                .and_then(|class| serde_json::to_value(class).ok())
                .and_then(|value| value.as_str().map(str::to_string)),
        };
        let mut setup = InstanceSetup::new(binding, context.clone(), window.clone(), adapter.clone()).with_geometry_reader(host.clone());
        if let Some(kv) = &manifest.kv {
            setup = setup.with_kv(KvSlot::new(Box::new(MemoryKv::new()), kv.max_bytes));
        }
        let component_host = ComponentHost::new().map_err(|error| anyhow!("{error}"))?;
        let admitted = component_host.compile(&self.component).map_err(|error| anyhow!("{} ({error})", error.code()))?;
        let mut instance = component_host.instantiate(&admitted, setup).map_err(|error| anyhow!("{} ({error})", error.code()))?;

        instance
            .on_install()
            .map_err(|fault| anyhow!("on-install: {fault}"))?
            .map_err(|message| anyhow!("on-install refused: {message}"))?;
        instance
            .on_activate()
            .map_err(|fault| anyhow!("on-activate: {fault}"))?
            .map_err(|message| anyhow!("on-activate refused: {message}"))?;

        let mut last_commit = tick::CommitOutcome::None;
        for tick_number in 1..=ticks {
            stepped_ms.store((tick_number - 1) * cadence_ms, Ordering::SeqCst);
            let now = adapter.now_ms();
            let info = tick::TickInfo {
                tick: tick_number,
                now_ms: now,
                scheduled_at_ms: now,
                delta_ms: if tick_number == 1 { 0 } else { cadence_ms as u32 },
                skipped: 0,
                last_commit: last_commit.clone(),
            };
            instance.refuel(hook_fuel).map_err(|error| anyhow!("refuel: {error}"))?;
            instance.on_tick(&info).map_err(|fault| anyhow!("on-tick {tick_number}: {fault}"))?;
            last_commit = flush_window(&window, &context, tick_number)?;
        }
        instance
            .on_deactivate(tick::DeactivateReason::HostStopping)
            .map_err(|fault| anyhow!("on-deactivate: {fault}"))?;
        flush_window(&window, &context, ticks + 1)?;

        let rows = rows.lock().map_err(|_| anyhow!("lab rows lock poisoned"))?;
        rows_to_plan(&model, &rows)
    }
}

fn lab_node(id: &str, name: &str, properties: HashMap<String, fe_sdk::PropertyValue>) -> NodeSnapshot {
    NodeSnapshot {
        node_id: id.to_string(),
        petal_id: LAB_PETAL.to_string(),
        name: name.to_string(),
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
        properties,
    }
}

/// Take the staged window and commit it as one flush; a refusal ends the plan (the package must run clean).
fn flush_window(window: &tick::SharedWindow, context: &PluginContext, tick_number: u64) -> Result<tick::CommitOutcome> {
    let staged = window.lock().map_err(|_| anyhow!("lab window lock poisoned"))?.take();
    if staged.ops.is_empty() && staged.readings.is_empty() {
        return Ok(tick::CommitOutcome::None);
    }
    let receipt = context.commit_staged(&staged).map_err(|message| anyhow!("tick {tick_number}: commit refused: {message}"))?;
    Ok(tick::CommitOutcome::Committed {
        tick: tick_number,
        ops: staged.ops.len() as u32,
        readings: staged.readings.len() as u32,
        created_node_ids: receipt.created_node_ids,
    })
}

/// Map the package's recorded rows onto `(at_ms, sensor)` plan entries and prove the plan is the whole fleet.
fn rows_to_plan(model: &fe_sim_model::fleet::FleetModel, rows: &[PlannedRow]) -> Result<Vec<ScheduledReading>> {
    let mut plan: BTreeMap<(u64, usize), ScheduledReading> = BTreeMap::new();
    for row in rows {
        let offset = row
            .at_ms
            .checked_sub(model.start_ms)
            .ok_or_else(|| anyhow!("reading for {} at {} predates the fleet origin", row.node_id, row.at_ms))?;
        // Anchor id == name in the hosting scene. Sensors sharing anchor, metric and units take the rows due at
        // one instant in sensor order (the first matching sensor not yet filled there), so a shared anchor never
        // collapses two sensors into one.
        let sensor = model
            .sensors
            .iter()
            .enumerate()
            .position(|(index, spec)| {
                spec.anchor == row.node_id
                    && spec.metric == row.metric
                    && spec.units == row.units
                    && spec.cadence_ms > 0
                    && offset % spec.cadence_ms == 0
                    && !plan.contains_key(&(row.at_ms, index))
            })
            .ok_or_else(|| anyhow!("reading {} {} at {} matches no unfilled fleet sensor", row.node_id, row.metric, row.at_ms))?;
        let cadence = model.sensors[sensor].cadence_ms;
        let tick = offset / cadence;
        if tick == 0 || offset > model.duration_ms {
            bail!("reading {} {} at {} is outside the fleet window", row.node_id, row.metric, row.at_ms);
        }
        plan.insert((row.at_ms, sensor), ScheduledReading { at_ms: row.at_ms, sensor, tick, value: row.value });
    }
    // The package must cover the whole native schedule; a skipped metric or a short run is an error, not a gap.
    let expected = plan_readings(model);
    if let Some(missing) = expected.iter().find(|reading| !plan.contains_key(&(reading.at_ms, reading.sensor))) {
        bail!(
            "the package did not emit sensor {} tick {} at {} ({} of {} planned readings present)",
            missing.sensor,
            missing.tick,
            missing.at_ms,
            plan.len(),
            expected.len()
        );
    }
    anyhow::ensure!(plan.len() == expected.len(), "the package emitted {} readings, the fleet schedules {}", plan.len(), expected.len());
    Ok(plan.into_values().collect())
}

/// Host the fleet-sim package under fe-sim `--runs` times; every run must reproduce the first fingerprint.
pub fn run_lab(args: &LabArgs) -> Result<LabReport> {
    anyhow::ensure!(args.runs >= 1, "--runs must be at least 1");
    let project = Project::open(&args.project)?;
    let manifest_json = project.read_manifest()?;
    let component = project.read_component(args.component.as_deref())?;
    let script_text = std::fs::read_to_string(&args.scenario).with_context(|| format!("reading scenario {}", args.scenario.display()))?;
    let script = ScenarioScript::parse(&script_text)?;
    // A caller-named root keeps every session for inspection; the default one is scratch, removed after each run.
    let keep_sessions = args.root.is_some();
    let root = args
        .root
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("fe-ext-lab-{}", std::process::id())));

    let mut fingerprint: Option<String> = None;
    let mut last = (0usize, 0usize);
    for run in 1..=args.runs {
        let session_dir = fresh_dir(&root, run)?;
        let mut source = HostedFleetSource::new(manifest_json.clone(), component.clone());
        let mut session = ScenarioSession::start_with_source(script.clone(), &session_dir, &mut source)?;
        let planned = session.status().readings_planned;
        session.step(u32::MAX)?;
        let outcome = session.stop()?;
        if !keep_sessions {
            // Best effort: a peer store handle the OS has not released yet only leaves scratch behind.
            let _ = std::fs::remove_dir_all(&session_dir);
        }
        let digest = blake3::hash(&serde_json::to_vec(&outcome.canonical_fingerprint())?).to_hex().to_string();
        match &fingerprint {
            None => fingerprint = Some(digest),
            Some(first) if *first == digest => {}
            Some(first) => bail!("lab run {run} fingerprint {digest} differs from run 1 {first}: the hosted fleet is not deterministic"),
        }
        last = (planned, outcome.ingested);
    }
    let fingerprint = fingerprint.expect("at least one run");
    if !keep_sessions {
        let _ = std::fs::remove_dir(&root);
    }
    let summary = vec![
        format!("scenario {} hosted by {}", script.name, args.project.display()),
        format!("runs {}, readings planned {}, ingested {}", args.runs, last.0, last.1),
        format!("fingerprint {fingerprint}"),
    ];
    let report = LabReport { fingerprint, readings_planned: last.0, readings_ingested: last.1, runs: args.runs, summary };
    if let Some(out) = &args.out {
        let body = json!({
            "fingerprint": report.fingerprint,
            "runs": report.runs,
            "readings_planned": report.readings_planned,
            "readings_ingested": report.readings_ingested,
        });
        std::fs::write(out, serde_json::to_vec_pretty(&body)?).with_context(|| format!("writing {}", out.display()))?;
    }
    Ok(report)
}

/// A session directory that did not exist before this run (fe-sim requires a fresh root per session).
fn fresh_dir(root: &Path, run: u32) -> Result<PathBuf> {
    let dir = root.join(format!("run-{run}"));
    anyhow::ensure!(!dir.exists(), "lab session directory {} already exists; pass a fresh --root", dir.display());
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}
