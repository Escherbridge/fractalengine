//! `run --sandbox`: the real component host over an in-memory scene, a manual clock and fixtures.
//! Rationale and the fingerprint contract: tools/fe-ext/AGENTS.md §sandbox.

mod calls;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use fe_plugin::bus::{EndReason, InstanceSpec};
use fe_plugin::capability::{CapabilityToken, V2Binding};
use fe_plugin::context::PluginContext;
use fe_plugin::tick::{self, Clock, ManualClock};
use fe_plugin::transaction::{StagedCommitSink, StagedFlush};
use fe_plugin::wasm::component::bindings::exports::fractalengine::plugin::ingress_hooks::{IngressFrame, Transport};
use fe_plugin::wasm::component::{window_for_token, Binding, ComponentHost, GridTerrain, InstanceSetup, KvSlot, MemoryKv};
use fe_plugin_test::mock_host::MockHostEnv;
use fe_sdk::manifest_v2::{is_credential_header, ExtensionManifest, ExtensionSourceClass, Ingress, Method, HOST_FUEL_CEILING};

pub use calls::{FixtureCommand, FixtureHook};
use calls::{Calls, When};

/// The newest fixture format this build reads; bump it with every new top-level key (AGENTS.md §sandbox, fixture versions).
pub const FIXTURE_VERSION: u32 = 2;
/// Every top-level key `Fixture` reads; a key outside it means the fixture was written for a newer fe-ext.
const FIXTURE_KEYS: [&str; 12] = [
    "fixture_version",
    "petal_id",
    "start_ms",
    "ticks",
    "cadence_ms",
    "source_class",
    "nodes",
    "fleet",
    "ingress",
    "terrain",
    "commands",
    "hooks",
];
use fe_sdk::node::NodeSnapshot;
use fe_sdk::plugin_host::{json_property, property_json, PendingOp, StagedTimestamp, TransactionReceipt};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::project::Project;
use crate::validate;

/// One seeded node of the in-memory petal.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureNode {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<[f64; 3]>,
    #[serde(default)]
    pub rotation: Option<[f64; 4]>,
    #[serde(default)]
    pub scale: Option<[f64; 3]>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
}

/// One recorded ingress frame, offered to the declared endpoint just before tick `at_tick` (1-based).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureIngress {
    pub at_tick: u32,
    pub endpoint: String,
    /// `http`, `websocket` or `mqtt`; optional, but when given it must match the declared endpoint.
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub connection: Option<u64>,
    #[serde(default)]
    pub caller_did: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub query: Vec<(String, String)>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    /// UTF-8 body text.
    #[serde(default)]
    pub body: Option<String>,
    /// Binary body, standard base64 (wins over `body`).
    #[serde(default)]
    pub body_base64: Option<String>,
}

/// The whole fixture document (`fixtures/*.json`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    /// The format the fixture was written for (absent = 1); see [`FIXTURE_VERSION`].
    #[serde(default = "default_fixture_version")]
    pub fixture_version: u32,
    #[serde(default = "default_petal")]
    pub petal_id: String,
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default = "default_ticks")]
    pub ticks: u32,
    /// Overrides the manifest cadence for the simulated clock only.
    #[serde(default)]
    pub cadence_ms: Option<u64>,
    /// The readings source class pick when the manifest declares a set.
    #[serde(default)]
    pub source_class: Option<String>,
    #[serde(default)]
    pub nodes: Vec<FixtureNode>,
    /// A fe-sim fleet model; validated, then placed on node `fleet-1` as property `fleet.config`.
    #[serde(default)]
    pub fleet: Option<fe_sim_model::fleet::FleetModel>,
    #[serde(default)]
    pub ingress: Vec<FixtureIngress>,
    /// A deterministic height grid served to `terrain: true` manifests (else `terrain` answers `not-available`).
    #[serde(default)]
    pub terrain: Option<FixtureTerrain>,
    /// Bus command calls, each admitted by the real broker before the guest sees it (fixture_version 2).
    #[serde(default)]
    pub commands: Vec<FixtureCommand>,
    /// Hook judgments run through the real scene hook gate (fixture_version 2).
    #[serde(default)]
    pub hooks: Vec<FixtureHook>,
}

/// Parse fixture text; an unknown top-level key or a newer `fixture_version` names the fe-ext it needs.
pub fn parse_fixture(text: &str) -> Result<Fixture> {
    let raw: Value = serde_json::from_str(text).context("the fixture is not JSON")?;
    let Value::Object(members) = &raw else { bail!("a fixture is a JSON object") };
    let this = env!("CARGO_PKG_VERSION");
    if let Some(declared) = members.get("fixture_version") {
        let version = declared.as_u64().filter(|version| *version >= 1).ok_or_else(|| anyhow!("fixture_version must be a whole number >= 1"))?;
        if version > u64::from(FIXTURE_VERSION) {
            bail!("this fixture needs fe-ext >= fixture_version {version}: fe-ext {this} reads fixture_version {FIXTURE_VERSION} and older; update fe-ext");
        }
    }
    if let Some(unknown) = members.keys().find(|key| !FIXTURE_KEYS.contains(&key.as_str())) {
        bail!("this fixture needs a newer fe-ext: `{unknown}` is not a fixture key fe-ext {this} reads (fixture_version {FIXTURE_VERSION} and older); update fe-ext");
    }
    serde_json::from_value(raw).map_err(|error| {
        // An unknown key inside an entry is the same story one level down.
        if error.to_string().contains("unknown field") {
            anyhow!("{error}; this fixture may need a newer fe-ext than {this} (fixture_version {FIXTURE_VERSION})")
        } else {
            anyhow!("{error}")
        }
    })
}

fn default_fixture_version() -> u32 {
    1
}

/// Fixture terrain: a row-major height grid sampled bilinearly (fe-plugin `GridTerrain`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureTerrain {
    /// World `(x, z)` of the first sample.
    pub origin: [f64; 2],
    /// Sample spacing, world units (finite, > 0).
    pub cell: f64,
    /// Samples along +x (2..=257).
    pub columns: u32,
    /// Samples along +z (2..=257).
    pub rows: u32,
    /// `columns * rows` finite world-unit heights.
    pub heights: Vec<f64>,
    /// World units per real metre.
    #[serde(default = "default_world_scale")]
    pub world_scale: f64,
    /// Real elevation (metres) at world height 0.
    #[serde(default)]
    pub origin_ele: f64,
}

impl FixtureTerrain {
    /// Bounds-checked conversion into the host's grid source.
    fn to_grid(&self) -> Result<GridTerrain> {
        const AXIS: std::ops::RangeInclusive<u32> = 2..=257;
        if !AXIS.contains(&self.columns) || !AXIS.contains(&self.rows) {
            bail!("fixture terrain: columns and rows must be 2..=257");
        }
        if !(self.cell.is_finite() && self.cell > 0.0) {
            bail!("fixture terrain: cell must be finite and > 0");
        }
        if self.heights.len() != (self.columns as usize) * (self.rows as usize) {
            bail!("fixture terrain: heights must hold columns * rows values");
        }
        let finite = self.origin.iter().chain(&self.heights).chain([&self.origin_ele]).all(|value| value.is_finite());
        if !finite || !(self.world_scale.is_finite() && self.world_scale > 0.0) {
            bail!("fixture terrain: values must be finite and world_scale > 0");
        }
        Ok(GridTerrain {
            origin: self.origin,
            cell: self.cell,
            columns: self.columns,
            rows: self.rows,
            heights: self.heights.clone(),
            world_scale: self.world_scale,
            origin_ele: self.origin_ele,
        })
    }
}

fn default_world_scale() -> f64 {
    1.0
}

fn default_petal() -> String {
    "p1".into()
}

fn default_ticks() -> u32 {
    10
}

/// What a run produced: the trace, its fingerprint and a few human lines.
pub struct RunReport {
    pub fingerprint: String,
    pub trace: Value,
    pub summary: Vec<String>,
}

/// A reading as the sink recorded it.
struct RecordedReading {
    flushed_at_ms: u64,
    node_id: String,
    metric: String,
    value: f64,
    units: String,
    recorded_at: Value,
}

/// Commits staged windows into the mock scene atomically with deterministic node ids.
struct SandboxSink {
    host: Arc<Mutex<MockHostEnv>>,
    clock: ManualClock,
    readings: Arc<Mutex<Vec<RecordedReading>>>,
    created: u64,
}

impl StagedCommitSink for SandboxSink {
    fn commit_staged(&mut self, petal_id: &str, flush: &StagedFlush) -> Result<TransactionReceipt, String> {
        let mut host = self.host.lock().map_err(|_| "sandbox scene lock poisoned".to_string())?;
        // Work on a copy so a refused flush leaves the scene untouched.
        let mut nodes = host.nodes.clone();
        let mut created_counter = self.created;
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
                    created_counter += 1;
                    let node_id = format!("sbx-{created_counter:06}");
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
        let now = self.clock.now_ms();
        let recorded_count = flush.readings.len();
        let mut recorded = Vec::with_capacity(flush.readings.len());
        for reading in &flush.readings {
            if !nodes.get(&reading.node_id).is_some_and(|node| node.petal_id == petal_id) {
                return Err(format!("reading anchor {} is not a node of the bound petal", reading.node_id));
            }
            recorded.push(RecordedReading {
                flushed_at_ms: now,
                node_id: reading.node_id.clone(),
                metric: reading.metric.clone(),
                value: reading.value,
                units: reading.units.clone(),
                recorded_at: match &reading.recorded_at {
                    None => json!({ "epoch_ms": now }),
                    Some(StagedTimestamp::EpochMs(ms)) => json!({ "epoch_ms": ms }),
                    Some(StagedTimestamp::Rfc3339(text)) => json!({ "rfc3339": text }),
                },
            });
        }
        host.nodes = nodes;
        self.created = created_counter;
        self.readings
            .lock()
            .map_err(|_| "sandbox readings lock poisoned".to_string())?
            .extend(recorded);
        Ok(TransactionReceipt { op_count: flush.ops.len(), created_node_ids, reading_count: recorded_count })
    }
}

/// Read a project's manifest, component and a fixture file, then [`run`].
pub fn run_project(project_dir: &Path, fixture_path: &Path, component_override: Option<&Path>) -> Result<RunReport> {
    let project = Project::open(project_dir)?;
    let manifest = project.read_manifest()?;
    let component = project.read_component(component_override)?;
    let fixture_text = std::fs::read_to_string(fixture_path).with_context(|| format!("reading fixture {}", fixture_path.display()))?;
    let fixture = parse_fixture(&fixture_text).with_context(|| format!("parsing fixture {}", fixture_path.display()))?;
    run(&manifest, &component, &fixture)
}

fn seed_node(fixture_node: &FixtureNode, petal_id: &str) -> NodeSnapshot {
    NodeSnapshot {
        node_id: fixture_node.id.clone(),
        petal_id: petal_id.to_string(),
        name: fixture_node.name.clone().unwrap_or_else(|| fixture_node.id.clone()),
        position: fixture_node.position.unwrap_or([0.0; 3]),
        rotation: fixture_node.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]),
        scale: fixture_node.scale.unwrap_or([1.0; 3]),
        properties: fixture_node
            .properties
            .iter()
            .map(|(key, value)| (key.clone(), json_property(value.clone())))
            .collect(),
    }
}

/// The identity the sandbox binds as, and the caller of http and websocket-accept frames that name none.
const SANDBOX_DID: &str = "did:key:sandbox";

/// What the host's ingress mount does with one fixture frame before any guest code runs.
enum Admission {
    Deliver(Box<IngressFrame>),
    /// Refused or dropped by the host; `status` is the http answer (none for a dropped websocket/mqtt frame).
    Refused { status: Option<u16>, reason: &'static str },
}

fn fixture_body(item: &FixtureIngress) -> Result<Vec<u8>> {
    Ok(match (&item.body_base64, &item.body) {
        (Some(encoded), _) => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|error| anyhow!("body_base64: {error}"))?
        }
        (None, Some(text)) => text.clone().into_bytes(),
        (None, None) => Vec::new(),
    })
}

fn endpoint_id(endpoint: &Ingress) -> &str {
    match endpoint {
        Ingress::HttpServer(declared) => &declared.id,
        Ingress::WebsocketAccept(declared) => &declared.id,
        Ingress::WebsocketDial(declared) => &declared.id,
        Ingress::Mqtt(declared) => &declared.id,
    }
}

/// MQTT topic-filter match (`+` one level, trailing `#` the rest).
fn topic_matches(filter: &str, topic: &str) -> bool {
    let mut levels = topic.split('/');
    for part in filter.split('/') {
        match (part, levels.next()) {
            ("#", _) => return true,
            ("+", Some(_)) => {}
            (expected, Some(level)) if expected == level => {}
            _ => return false,
        }
    }
    levels.next().is_none()
}

/// The declared `forward_headers`, lowercase, minus credential and host-authenticating headers (fe-plugin `net::ingress::forwarded_headers`).
fn forwarded_headers(headers: &[(String, String)], declared: &[String]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.trim().to_ascii_lowercase();
            let wanted = declared.iter().any(|declared| declared.eq_ignore_ascii_case(&name));
            (wanted && !is_credential_header(&name)).then(|| (name, value.clone()))
        })
        .collect()
}

/// Run the host-side ingress checks in production order (fe-plugin `net/ingress.rs`); rates are not modelled.
fn admit_ingress(manifest: &ExtensionManifest, item: &FixtureIngress, received_at_ms: u64) -> Result<Admission> {
    let Some(endpoint) = manifest.ingress.iter().find(|endpoint| endpoint_id(endpoint) == item.endpoint) else {
        return Ok(Admission::Refused { status: Some(404), reason: "not-found" });
    };
    let transport_name = match endpoint {
        Ingress::HttpServer(_) => "http",
        Ingress::WebsocketAccept(_) | Ingress::WebsocketDial(_) => "websocket",
        Ingress::Mqtt(_) => "mqtt",
    };
    if let Some(named) = item.transport.as_deref().filter(|named| *named != transport_name) {
        bail!("fixture frame names transport {named}, but endpoint {} is {transport_name}", item.endpoint);
    }
    let body = fixture_body(item)?;
    let mut frame = IngressFrame {
        endpoint: item.endpoint.clone(),
        transport: Transport::Http,
        connection: None,
        caller_did: None,
        method: None,
        path: None,
        query: Vec::new(),
        headers: Vec::new(),
        topic: None,
        content_type: item.content_type.clone(),
        body,
        received_at_ms,
    };
    match endpoint {
        Ingress::HttpServer(declared) => {
            let sub_path = item.path.clone().unwrap_or_default();
            if !sub_path.is_empty() && !declared.sub_paths.unwrap_or(false) {
                return Ok(Admission::Refused { status: Some(404), reason: "not-found" });
            }
            let method = match &item.method {
                Some(method) => method.to_ascii_uppercase(),
                None => declared.methods.first().and_then(|method| serde_json::to_value(method).ok()).and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default(),
            };
            let allowed = serde_json::from_value::<Method>(Value::String(method.clone())).is_ok_and(|parsed| declared.methods.contains(&parsed));
            if !allowed {
                return Ok(Admission::Refused { status: Some(405), reason: "method-not-allowed" });
            }
            if frame.body.len() as u64 > declared.max_body_bytes {
                return Ok(Admission::Refused { status: Some(413), reason: "payload-too-large" });
            }
            if let Some(accepted) = &declared.content_types {
                let media = item.content_type.as_deref().unwrap_or_default().split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
                if !accepted.iter().any(|kind| kind.eq_ignore_ascii_case(&media)) {
                    return Ok(Admission::Refused { status: Some(415), reason: "unsupported-media-type" });
                }
            }
            frame.caller_did = Some(item.caller_did.clone().unwrap_or_else(|| SANDBOX_DID.to_string()));
            frame.method = Some(method);
            frame.path = Some(sub_path);
            frame.query = item.query.clone();
            frame.headers = forwarded_headers(&item.headers, declared.forward_headers.as_deref().unwrap_or(&[]));
        }
        Ingress::WebsocketAccept(declared) => {
            if frame.body.len() as u64 > declared.max_frame_bytes {
                return Ok(Admission::Refused { status: None, reason: "dropped-oversize" });
            }
            frame.transport = Transport::Websocket;
            frame.connection = Some(item.connection.unwrap_or(1));
            frame.caller_did = Some(item.caller_did.clone().unwrap_or_else(|| SANDBOX_DID.to_string()));
        }
        Ingress::WebsocketDial(declared) => {
            if frame.body.len() as u64 > declared.max_frame_bytes {
                return Ok(Admission::Refused { status: None, reason: "dropped-oversize" });
            }
            frame.transport = Transport::Websocket;
            frame.connection = Some(item.connection.unwrap_or(1));
        }
        Ingress::Mqtt(declared) => {
            let topic = item.topic.clone().unwrap_or_default();
            if !declared.topics.iter().any(|filter| topic_matches(filter, &topic)) {
                bail!("fixture topic {topic:?} matches none of endpoint {}'s topic filters", item.endpoint);
            }
            if frame.body.len() as u64 > declared.max_payload_bytes {
                return Ok(Admission::Refused { status: None, reason: "dropped-oversize" });
            }
            frame.transport = Transport::Mqtt;
            frame.topic = Some(topic);
        }
    }
    Ok(Admission::Deliver(Box::new(frame)))
}

fn node_json(node: &NodeSnapshot) -> Value {
    let properties: BTreeMap<&String, Value> = node.properties.iter().map(|(key, value)| (key, property_json(value))).collect();
    json!({
        "id": node.node_id,
        "name": node.name,
        "position": node.position,
        "rotation": node.rotation,
        "scale": node.scale,
        "properties": properties,
    })
}

/// Take the staged window and commit it as one flush; the outcome feeds the next tick-info.
fn flush_window(
    window: &fe_plugin::tick::SharedWindow,
    context: &PluginContext,
    tick_number: u64,
    commits: &mut Vec<Value>,
) -> tick::CommitOutcome {
    let staged = match window.lock() {
        Ok(mut window) => window.take(),
        Err(_) => return tick::CommitOutcome::Unknown,
    };
    if staged.ops.is_empty() && staged.readings.is_empty() {
        return tick::CommitOutcome::None;
    }
    match context.commit_staged(&staged) {
        Ok(receipt) => {
            commits.push(json!({
                "tick": tick_number,
                "ops": staged.ops.len(),
                "readings": staged.readings.len(),
                "created": receipt.created_node_ids,
            }));
            tick::CommitOutcome::Committed {
                tick: tick_number,
                ops: staged.ops.len() as u32,
                readings: staged.readings.len() as u32,
                created_node_ids: receipt.created_node_ids,
            }
        }
        Err(message) => {
            commits.push(json!({ "tick": tick_number, "refused": message }));
            tick::CommitOutcome::Refused(message)
        }
    }
}

/// The three lifecycle phases of one sandbox run, kept apart so `?` can end a phase early.
struct Recorder {
    events: Vec<Value>,
    faults: Vec<String>,
    commits: Vec<Value>,
    replies: Vec<Value>,
}

/// Run `component` under `extension_json` against `fixture`; deterministic for equal inputs.
pub fn run(extension_json: &[u8], component: &[u8], fixture: &Fixture) -> Result<RunReport> {
    let validated = validate::validate_manifest(extension_json)?;
    let manifest = &validated.manifest;
    if fixture.fixture_version > FIXTURE_VERSION {
        bail!("this fixture needs fe-ext >= fixture_version {}: this fe-ext reads fixture_version {FIXTURE_VERSION} and older", fixture.fixture_version);
    }
    if fixture.ticks == 0 {
        bail!("fixture ticks must be at least 1");
    }
    if let Some(fleet) = &fixture.fleet {
        fleet.validate().map_err(|error| anyhow!("fixture fleet: {error}"))?;
    }

    // Seed the scene.
    let mut mock = MockHostEnv::new();
    for fixture_node in &fixture.nodes {
        mock.insert_node(fixture_node.id.clone(), seed_node(fixture_node, &fixture.petal_id));
    }
    if let Some(fleet) = &fixture.fleet {
        let mut properties = BTreeMap::new();
        properties.insert("fleet.config".to_string(), serde_json::to_value(fleet)?);
        let fleet_node = FixtureNode {
            id: "fleet-1".into(),
            name: Some("fleet".into()),
            position: None,
            rotation: None,
            scale: None,
            properties,
        };
        mock.insert_node("fleet-1", seed_node(&fleet_node, &fixture.petal_id));
    }
    let host = Arc::new(Mutex::new(mock));

    // Token, window and context.
    let source_class: Option<ExtensionSourceClass> = match &fixture.source_class {
        Some(text) => Some(
            serde_json::from_value(Value::String(text.clone()))
                .map_err(|_| anyhow!("fixture source_class must be simulation, device or connector"))?,
        ),
        None => None,
    };
    let petal_scope = format!("VERSE#mock-FRACTAL#mock-PETAL#{}", fixture.petal_id);
    let scopes = V2Binding {
        verse_scope: "VERSE#mock".into(),
        petal_scope: petal_scope.clone(),
        source_class,
    };
    let token = CapabilityToken::mint_v2(&manifest.id, manifest, &scopes).map_err(|error| anyhow!("binding refused: {error}"))?;
    // The class the token writes as (the pick, or the manifest's only class): what production puts in the binding.
    let readings_class = token.grants_v2.as_ref().and_then(|grants| grants.readings_class);
    let tick_budget = token.tick_budget().cloned();
    let cadence_ms = fixture
        .cadence_ms
        .or_else(|| tick_budget.as_ref().map(|budget| budget.cadence_ms))
        .unwrap_or(1000);
    let hook_fuel = tick_budget.as_ref().map_or(tick::DEFAULT_TICK_FUEL, |budget| budget.fuel);
    let window = window_for_token(&token);

    let clock = ManualClock::default();
    clock.set(fixture.start_ms);
    let readings = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(Mutex::new(SandboxSink {
        host: host.clone(),
        clock: clock.clone(),
        readings: readings.clone(),
        created: 0,
    }));
    let (commands, _commands_receiver) = crossbeam::channel::bounded(1);
    let context = PluginContext::new(manifest.id.clone(), fixture.petal_id.clone(), token, commands)
        .with_node_reader(host.clone())
        .with_staged_sink(sink);
    let content_hash = blake3::hash(extension_json).to_hex().to_string();
    // The real broker: the instance joins it before `on-install`, as `TickHost::launch` seats it.
    let bus = calls::sandbox_bus(Arc::new(clock.clone()), SANDBOX_DID);
    let bus_key = bus
        .register_instance(InstanceSpec {
            installation_id: "sandbox-1".into(),
            content_hash: content_hash.clone(),
            capability_digest: validated.capability_digest.clone(),
            verse_id: "mock".into(),
            petal_id: fixture.petal_id.clone(),
            identity_did: SANDBOX_DID.into(),
            manifest: Arc::new(manifest.clone()),
        })
        .map_err(|error| anyhow!("bus registration refused: {error}"))?;
    let mut calls = Calls::plan(
        manifest,
        &fixture.commands,
        &fixture.hooks,
        fixture.ticks,
        &fixture.petal_id,
        &petal_scope,
        bus.clone(),
        bus_key.clone(),
        window.clone(),
        SANDBOX_DID,
    )?;
    let binding = Binding {
        installation_id: "sandbox-1".into(),
        package_id: manifest.id.clone(),
        content_hash,
        verse_id: "mock".into(),
        fractal_id: "mock".into(),
        petal_id: fixture.petal_id.clone(),
        identity_did: SANDBOX_DID.into(),
        source_class: readings_class
            .and_then(|class| serde_json::to_value(class).ok())
            .and_then(|value| value.as_str().map(str::to_string)),
    };
    let mut setup = InstanceSetup::new(binding, context.clone(), window.clone(), Arc::new(clock.clone()) as Arc<dyn Clock>)
        .with_geometry_reader(host.clone())
        .with_broker(calls.broker());
    if let Some(kv) = &manifest.kv {
        setup = setup.with_kv(KvSlot::new(Box::new(MemoryKv::new()), kv.max_bytes));
    }
    // Production only wires a source for `terrain: true`; the grant check stays the host's.
    if let (Some(true), Some(terrain)) = (manifest.terrain, &fixture.terrain) {
        setup = setup.with_terrain(Arc::new(terrain.to_grid()?));
    }

    let component_host = ComponentHost::new().map_err(|error| anyhow!("{error}"))?;
    let admitted = component_host
        .compile(component)
        .map_err(|error| anyhow!("{} ({error})", error.code()))?;
    let mut instance = component_host
        .instantiate(&admitted, setup)
        .map_err(|error| anyhow!("{} ({error})", error.code()))?;

    let mut recorder = Recorder { events: Vec::new(), faults: Vec::new(), commits: Vec::new(), replies: Vec::new() };

    // Lifecycle: install, activate, ticks, deactivate. A refusal or fault ends the run, recorded.
    'run: {
        calls.enter(0, "on-install");
        let installed = instance.on_install();
        calls.note_fuel(&instance);
        match installed {
            Ok(Ok(())) => recorder.events.push(json!({ "hook": "on-install", "ok": true })),
            Ok(Err(message)) => {
                recorder.events.push(json!({ "hook": "on-install", "refused": message }));
                break 'run;
            }
            Err(fault) => {
                recorder.faults.push(format!("on-install: {fault}"));
                break 'run;
            }
        }
        calls.enter(0, "on-activate");
        let activated = instance.on_activate();
        calls.note_fuel(&instance);
        match activated {
            Ok(Ok(())) => recorder.events.push(json!({ "hook": "on-activate", "ok": true })),
            Ok(Err(message)) => {
                recorder.events.push(json!({ "hook": "on-activate", "refused": message }));
                break 'run;
            }
            Err(fault) => {
                recorder.faults.push(format!("on-activate: {fault}"));
                break 'run;
            }
        }
        let mut last_commit = tick::CommitOutcome::None;
        for tick_number in 1..=u64::from(fixture.ticks) {
            let now = fixture.start_ms + (tick_number - 1) * cadence_ms;
            clock.set(now);
            bus.poll();
            for item in fixture.ingress.iter().filter(|item| u64::from(item.at_tick) == tick_number) {
                let frame = match admit_ingress(manifest, item, now)? {
                    Admission::Deliver(frame) => frame,
                    Admission::Refused { status, reason } => {
                        recorder.replies.push(json!({
                            "tick": tick_number,
                            "endpoint": item.endpoint,
                            "host": reason,
                            "reply": status.map(|status| json!({ "status": status })),
                        }));
                        continue;
                    }
                };
                calls.enter(tick_number, &format!("on-ingress {}", item.endpoint));
                let replied = instance.on_ingress(&frame);
                calls.note_fuel(&instance);
                match replied {
                    Ok(reply) => recorder.replies.push(json!({
                        "tick": tick_number,
                        "endpoint": item.endpoint,
                        "reply": reply.map(|reply| json!({
                            "status": reply.status,
                            "content_type": reply.content_type,
                            "body": String::from_utf8_lossy(&reply.body),
                        })),
                    })),
                    Err(fault) => {
                        recorder.faults.push(format!("on-ingress tick {tick_number}: {fault}"));
                        break 'run;
                    }
                }
            }
            let info = tick::TickInfo {
                tick: tick_number,
                now_ms: now,
                scheduled_at_ms: now,
                delta_ms: if tick_number == 1 { 0 } else { cadence_ms as u32 },
                skipped: 0,
                last_commit: last_commit.clone(),
            };
            calls.run_due(When::Before(tick_number as u32), tick_number, &mut instance);
            instance.refuel(hook_fuel).map_err(|error| anyhow!("refuel: {error}"))?;
            calls.enter(tick_number, "on-tick");
            let ticked = instance.on_tick(&info);
            calls.note_fuel(&instance);
            if let Err(fault) = ticked {
                recorder.faults.push(format!("on-tick {tick_number}: {fault}"));
                break 'run;
            }
            last_commit = flush_window(&window, &context, tick_number, &mut recorder.commits);
            calls.run_due(When::After(tick_number as u32), tick_number, &mut instance);
        }
        let final_tick = u64::from(fixture.ticks) + 1;
        calls.enter(final_tick, "on-deactivate");
        let deactivated = instance.on_deactivate(tick::DeactivateReason::HostStopping);
        calls.note_fuel(&instance);
        match deactivated {
            Ok(()) => {
                flush_window(&window, &context, final_tick, &mut recorder.commits);
            }
            Err(fault) => recorder.faults.push(format!("on-deactivate: {fault}")),
        }
    }
    // The seat leaves the bus (OD-E1 order puts it first; nothing is queued by now, so the order is not observable).
    bus.unregister_instance(&bus_key, EndReason::InstanceStopped);
    let (command_calls, hook_calls, stream_events, fuel_calls) = calls.into_trace();

    // Trace and fingerprint: everything observable, in a canonical order.
    let mut final_nodes: Vec<Value> = {
        let host = host.lock().map_err(|_| anyhow!("sandbox scene lock poisoned"))?;
        host.nodes.values().map(node_json).collect()
    };
    final_nodes.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let recorded_readings: Vec<Value> = readings
        .lock()
        .map_err(|_| anyhow!("sandbox readings lock poisoned"))?
        .iter()
        .map(|reading| {
            json!({
                "flushed_at_ms": reading.flushed_at_ms,
                "node_id": reading.node_id,
                "metric": reading.metric,
                "value": reading.value,
                "units": reading.units,
                "recorded_at": reading.recorded_at,
            })
        })
        .collect();
    let trace = json!({
        "extension": { "id": manifest.id, "version": manifest.version },
        "fixture_version": fixture.fixture_version,
        "ticks": fixture.ticks,
        "cadence_ms": cadence_ms,
        "events": recorder.events,
        "ingress_replies": recorder.replies,
        "commands": command_calls,
        "hooks": hook_calls,
        "streams": stream_events,
        "commits": recorder.commits,
        "faults": recorder.faults,
        "fuel": { "tick_limit": hook_fuel.min(HOST_FUEL_CEILING), "calls": fuel_calls },
        "readings": recorded_readings,
        "nodes": final_nodes,
    });
    let fingerprint = blake3::hash(&serde_json::to_vec(&trace)?).to_hex().to_string();
    let count = |key: &str| trace[key].as_array().map_or(0, Vec::len);
    let summary = vec![
        format!("extension {}@{}", manifest.id, manifest.version),
        format!("ticks {} at {cadence_ms} ms, commits {}, readings {}", fixture.ticks, count("commits"), count("readings")),
        format!("commands {}, hooks {}, stream events {}", count("commands"), count("hooks"), count("streams")),
        format!("faults {}", count("faults")),
        format!("fingerprint {fingerprint}"),
    ];
    Ok(RunReport { fingerprint, trace, summary })
}
