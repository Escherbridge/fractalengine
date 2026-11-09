//! fleet-sim: drives `fleet.config` nodes (FleetConfig v2). Design notes: extensions/fleet-sim/AGENTS.md.

pub mod plan;

use std::cell::RefCell;
use std::collections::BTreeMap;

use fe_ext_guest::readings_batch::ReadingBatcher;
use fe_ext_guest::{
    bus, context, extension, kv, log, routes, scene, window, Caller, CommandError, DeactivateReason, Extension, NodeSnapshot, TickInfo,
};
use fe_sim_model::fleet::FleetModel;
use fe_sim_model::sensors::evaluate;
use serde_json::{json, Value};

/// Metrics the manifest declares; anything else would make the host refuse the whole readings batch.
const ALLOWED_METRICS: [&str; 2] = ["altitude_m", "temperature_c"];
/// One page of the bound petal, the most the host returns per query.
const ALL_NODES: &str = r#"{"limit":200}"#;
/// Mover rows in one `positions` event (the stream caps an event at 4 KiB).
const MAX_PUBLISHED_MOVERS: usize = 24;

thread_local! {
    static READINGS: RefCell<ReadingBatcher> = RefCell::new(ReadingBatcher::new());
}

/// Lifecycle of one fleet node, read from its `fleet.status` property.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Running,
    Stopped,
    Finished,
}

/// A `fleet.config` node as the driver sees it.
struct Fleet {
    node_id: String,
    model: FleetModel,
    status: Option<Status>,
    driver: Option<String>,
}

/// Per-fleet clock kept in `kv` so a restart resumes instead of re-emitting.
struct Clock {
    origin_ms: u64,
    last_elapsed_ms: u64,
}

fn state_key(node_id: &str) -> String {
    format!("fleet/{node_id}")
}

fn load_clock(node_id: &str) -> Option<Clock> {
    let text = kv::get(&state_key(node_id)).ok().flatten()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(Clock { origin_ms: value["origin_ms"].as_u64()?, last_elapsed_ms: value["last_elapsed_ms"].as_u64()? })
}

fn save_clock(node_id: &str, clock: &Clock) {
    let text = json!({ "origin_ms": clock.origin_ms, "last_elapsed_ms": clock.last_elapsed_ms }).to_string();
    let _ = kv::set(&state_key(node_id), &text);
}

fn warn(message: &str) {
    log::write(log::Level::Warn, message);
}

/// Parse a node's `fleet.config`; the property may be an object or a JSON string of one.
fn parse_fleet(node: &NodeSnapshot) -> Option<Fleet> {
    let properties: Value = serde_json::from_str(&node.properties).ok()?;
    let config = properties.get("fleet.config")?;
    let model: FleetModel = match config {
        Value::String(text) => serde_json::from_str(text).ok()?,
        other => serde_json::from_value(other.clone()).ok()?,
    };
    if let Err(error) = model.validate() {
        warn(&format!("fleet {} ignored: {error}", node.node_id));
        return None;
    }
    let status = match properties.get("fleet.status").and_then(Value::as_str) {
        Some("running") => Some(Status::Running),
        Some("stopped") => Some(Status::Stopped),
        Some("finished") => Some(Status::Finished),
        _ => None,
    };
    let driver = properties.get("fleet.driver").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string);
    Some(Fleet { node_id: node.node_id.clone(), model, status, driver })
}

fn set_text(node_id: &str, key: &str, text: &str) {
    if let Err(error) = window::set_property(node_id, key, &json!(text)) {
        warn(&format!("could not stage {key} on {node_id}: {error:?}"));
    }
}

/// Advance one fleet to this tick: claim it, or (once the claim reads back as ours) emit due sensor readings,
/// move the movers and publish positions.
fn drive(fleet: &Fleet, ids_by_name: &BTreeMap<&str, &str>, me: &str, info: &TickInfo) {
    if matches!(fleet.status, Some(Status::Stopped | Status::Finished)) {
        return;
    }
    match fleet.driver.as_deref() {
        Some(owner) if owner != me => {
            // Passive: a clock kept from a lost claim must not replay a burst if this instance takes over later.
            let _ = kv::delete(&state_key(&fleet.node_id));
            return;
        }
        Some(_) => {}
        None => {
            // R30: the claim travels without this fleet's data; driving starts on the tick that reads it back.
            set_text(&fleet.node_id, "fleet.driver", me);
            if load_clock(&fleet.node_id).is_none() {
                save_clock(&fleet.node_id, &Clock { origin_ms: info.now_ms, last_elapsed_ms: 0 });
            }
            return;
        }
    }
    if fleet.status.is_none() {
        set_text(&fleet.node_id, "fleet.status", "running");
    }

    let model = &fleet.model;
    let mut clock = load_clock(&fleet.node_id).unwrap_or(Clock { origin_ms: info.now_ms, last_elapsed_ms: 0 });
    let elapsed_ms = info.now_ms.saturating_sub(clock.origin_ms).min(model.duration_ms);

    READINGS.with(|batcher| {
        let mut batcher = batcher.borrow_mut();
        for sensor in &model.sensors {
            if !ALLOWED_METRICS.contains(&sensor.metric.as_str()) {
                warn(&format!("sensor metric {} is not declared in the manifest; skipped", sensor.metric));
                continue;
            }
            let Some(anchor) = ids_by_name.get(sensor.anchor.as_str()) else { continue };
            for tick in plan::due_ticks(clock.last_elapsed_ms, elapsed_ms, sensor.cadence_ms) {
                let model_time_ms = model.start_ms.saturating_add(tick * sensor.cadence_ms);
                let value = evaluate(&sensor.model, tick, model_time_ms);
                let sensor_time_ms = clock.origin_ms.saturating_add(tick * sensor.cadence_ms);
                batcher.push_at(anchor, &sensor.metric, value, &sensor.units, i64::try_from(sensor_time_ms).ok());
            }
        }
    });

    if plan::crossed(clock.last_elapsed_ms, elapsed_ms, model.mover_tick_ms) {
        let mut published = Vec::new();
        for mover in &model.movers {
            let (Some(node), Some(route)) = (ids_by_name.get(mover.node.as_str()), ids_by_name.get(mover.route.as_str())) else { continue };
            if window::room().ops == 0 {
                warn("write window is full; mover updates deferred to the next tick");
                break;
            }
            let Ok(length) = routes::info(route).map(|route_info| route_info.length_m) else { continue };
            let progress = plan::mover_progress(elapsed_ms, mover.speed_mps, mover.start_offset_s, length, mover.looped);
            let Ok(sample) = routes::sample(route, progress) else { continue };
            let position = [sample.position.x, sample.position.y, sample.position.z];
            let staged = match plan::yaw_quaternion(sample.tangent.x, sample.tangent.z) {
                Some(rotation) => window::set_pose(node, position, rotation),
                None => window::set_position(node, position),
            };
            if staged.is_ok() && published.len() < MAX_PUBLISHED_MOVERS {
                published.push(json!({ "node": mover.node, "progress": progress, "position": position }));
            }
        }
        let event = json!({ "fleet": fleet.node_id, "count": published.len(), "movers": published });
        // Not-available on a headless host or without a subscriber is expected.
        let _ = bus::publish("positions", &event.to_string());
    }

    clock.last_elapsed_ms = elapsed_ms;
    save_clock(&fleet.node_id, &clock);
    if elapsed_ms >= model.duration_ms {
        set_text(&fleet.node_id, "fleet.status", "finished");
    }
}

struct FleetSim;

impl Extension for FleetSim {
    /// Drive every `fleet.config` node this instance owns, then stage whatever readings fit the window.
    fn on_tick(info: TickInfo) {
        let nodes = match scene::query_nodes(ALL_NODES) {
            Ok(nodes) => nodes,
            Err(error) => return warn(&format!("scene query refused: {error:?}")),
        };
        let ids_by_name: BTreeMap<&str, &str> = nodes.iter().map(|node| (node.name.as_str(), node.node_id.as_str())).collect();
        let me = context::current_binding().installation_id;
        for fleet in nodes.iter().filter_map(parse_fleet) {
            drive(&fleet, &ids_by_name, &me, &info);
        }
        if let Err(error) = READINGS.with(|batcher| batcher.borrow_mut().flush()) {
            warn(&format!("readings refused, requeued: {error:?}"));
        }
    }

    /// `start` and `stop`, for every fleet in the petal or the one named by `node_id`.
    fn on_command(command: String, args: String, _caller: Caller) -> Result<String, CommandError> {
        let (status, counter) = match command.as_str() {
            "start" => ("running", "started"),
            "stop" => ("stopped", "stopped"),
            other => return Err(CommandError::Refused(format!("unknown command {other}"))),
        };
        let args: Value = serde_json::from_str(&args).map_err(|error| CommandError::InvalidInput(error.to_string()))?;
        let target = args.get("node_id").and_then(Value::as_str);
        let nodes = scene::query_nodes(ALL_NODES).map_err(|error| CommandError::Failed(format!("{error:?}")))?;
        let fleets: Vec<Fleet> = nodes.iter().filter_map(parse_fleet).filter(|fleet| target.is_none_or(|id| id == fleet.node_id)).collect();
        if target.is_some() && fleets.is_empty() {
            return Err(CommandError::Refused("node_id is not a fleet.config node".into()));
        }
        for fleet in &fleets {
            window::set_property(&fleet.node_id, "fleet.status", &json!(status)).map_err(|error| CommandError::Failed(format!("{error:?}")))?;
            if status == "running" {
                // A fresh start replays the fleet from its origin.
                let _ = kv::delete(&state_key(&fleet.node_id));
            }
        }
        Ok(json!({ counter: fleets.len() }).to_string())
    }

    /// Release the driver claim so another installation can take the fleet over.
    fn on_deactivate(reason: DeactivateReason) {
        if matches!(reason, DeactivateReason::Disabled | DeactivateReason::Unbound | DeactivateReason::Updating | DeactivateReason::HostStopping) {
            release_claims();
        }
    }

    fn on_uninstall(_binding: fe_ext_guest::Binding) {
        release_claims();
    }
}

/// Stage an empty `fleet.driver` on every fleet this instance drives.
fn release_claims() {
    let Ok(nodes) = scene::query_nodes(ALL_NODES) else { return };
    let me = context::current_binding().installation_id;
    for fleet in nodes.iter().filter_map(parse_fleet).filter(|fleet| fleet.driver.as_deref() == Some(me.as_str())) {
        set_text(&fleet.node_id, "fleet.driver", "");
    }
}

extension!(FleetSim);
