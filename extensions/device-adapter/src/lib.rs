//! device-adapter: HTTP, websocket (accepted or dialled) and MQTT frames become readings and transforms; actuation goes out over MQTT.
//! Design notes: extensions/device-adapter/AGENTS.md.

pub mod mapping;
pub mod plan;

use std::cell::RefCell;
use std::collections::BTreeMap;

use fe_ext_guest::egress::{self, MqttQos};
use fe_ext_guest::readings_batch::ReadingBatcher;
use fe_ext_guest::{
    extension, log, scene, txn, window, Caller, CommandError, Extension, HostError, IncomingResponse, IngressFrame, IngressReply, TickInfo,
};
use plan::DeviceNaming;
use serde_json::{json, Value};

/// The MQTT site prefix; it must match the `site-broker` and `actuate` topic filters in extension.json.
const SITE: &str = "site-a";
/// Readings waiting for window room before new frames are answered 429.
const MAX_PENDING_READINGS: usize = 5_000;

/// What a tick still has to stage for one node: the newest value of each kind wins.
#[derive(Default)]
struct NodeUpdate {
    position: Option<[f64; 3]>,
    heading_deg: Option<f64>,
    last_seen_ms: Option<u64>,
}

thread_local! {
    static READINGS: RefCell<ReadingBatcher> = RefCell::new(ReadingBatcher::new());
    static UPDATES: RefCell<BTreeMap<String, NodeUpdate>> = const { RefCell::new(BTreeMap::new()) };
}

fn warn(message: &str) {
    log::write(log::Level::Warn, message);
}

fn reply(status: u16, body: Value) -> Option<IngressReply> {
    Some(IngressReply { status, content_type: Some("application/json".into()), body: body.to_string().into_bytes() })
}

/// The node whose `device.id` property equals `device_id`, if any.
fn node_for_device(device_id: &str) -> Result<Option<String>, HostError> {
    let query = json!({ "property": { "key": "device.id", "value": device_id }, "limit": 1 });
    Ok(scene::query_nodes(&query.to_string())?.into_iter().next().map(|node| node.node_id))
}

/// The device id the endpoint's plan says the transport names: the forwarded header (http) or the topic (mqtt).
fn transport_hint(frame: &IngressFrame, naming: DeviceNaming) -> Option<String> {
    match naming {
        DeviceNaming::Header => frame.headers.iter().find(|(name, _)| name == "x-device-id").map(|(_, value)| value.clone()),
        DeviceNaming::Topic => frame.topic.as_deref().and_then(mapping::device_id_from_topic).map(str::to_string),
        DeviceNaming::Body => None,
    }
}

struct DeviceAdapter;

impl Extension for DeviceAdapter {
    /// Map one frame, queue its readings and remember the newest transform; the reply is the transport's acknowledgement.
    fn on_ingress(frame: IngressFrame) -> Option<IngressReply> {
        let Some(plan) = plan::plan_for(&frame.endpoint) else {
            warn(&format!("{}: frame from an undeclared endpoint dropped", frame.endpoint));
            return None;
        };
        // Only a caller on http or an accepted socket is answered; dialled-feed and mqtt failures are logged and dropped.
        let answer = plan.answers_errors;
        let refuse = |status: u16, error: String| {
            if answer {
                return reply(status, json!({ "error": error }));
            }
            warn(&format!("{}: frame dropped: {error}", frame.endpoint));
            None
        };

        let hint = transport_hint(&frame, plan.naming);
        let parsed = match mapping::parse_frame(&frame.body, hint.as_deref()) {
            Ok(parsed) => parsed,
            Err(error) => return refuse(400, error),
        };
        let node_id = match node_for_device(&parsed.device_id) {
            Ok(Some(node_id)) => node_id,
            Ok(None) => return refuse(404, format!("no node carries device.id {}", parsed.device_id)),
            Err(error) => return refuse(503, format!("scene query refused: {error:?}")),
        };
        if READINGS.with(|batcher| batcher.borrow().pending()) + parsed.readings.len() > MAX_PENDING_READINGS {
            return refuse(429, "reading queue is full; retry later".into());
        }

        READINGS.with(|batcher| {
            let mut batcher = batcher.borrow_mut();
            for reading in &parsed.readings {
                batcher.push_at(&node_id, &reading.metric, reading.value, &reading.units, reading.at_ms);
            }
        });
        UPDATES.with(|updates| {
            let mut updates = updates.borrow_mut();
            let update = updates.entry(node_id).or_default();
            if parsed.position.is_some() {
                update.position = parsed.position;
            }
            if parsed.heading_deg.is_some() {
                update.heading_deg = parsed.heading_deg;
            }
            update.last_seen_ms = update.last_seen_ms.max(Some(frame.received_at_ms));
        });

        // http acknowledges with the count; websocket (accepted or dialled) and mqtt stay quiet on success.
        if plan.acknowledges {
            reply(202, json!({ "accepted": parsed.readings.len() }))
        } else {
            None
        }
    }

    /// Stage what the window has room for: transforms and `device.last_seen` first, then queued readings.
    fn on_tick(info: TickInfo) {
        if let txn::CommitOutcome::Refused(error) = &info.last_commit {
            warn(&format!("previous window refused whole: {error:?}"));
        }
        UPDATES.with(|updates| {
            let mut updates = updates.borrow_mut();
            let node_ids: Vec<String> = updates.keys().cloned().collect();
            for node_id in node_ids {
                // Each node needs up to two ops (pose, last_seen); a node that does not fit waits for the next tick.
                if window::room().ops < 2 {
                    break;
                }
                let Some(update) = updates.remove(&node_id) else { continue };
                let staged = match (update.position, update.heading_deg) {
                    (Some(position), Some(heading)) => window::set_pose(&node_id, position, mapping::heading_quaternion(heading)),
                    (Some(position), None) => window::set_position(&node_id, position),
                    _ => Ok(()),
                };
                if let Err(error) = staged {
                    warn(&format!("transform for {node_id} refused: {error:?}"));
                }
                if let Some(seen) = update.last_seen_ms {
                    if let Err(error) = window::set_property(&node_id, "device.last_seen", &json!(seen)) {
                        warn(&format!("device.last_seen for {node_id} refused: {error:?}"));
                    }
                }
            }
        });
        if let Err(error) = READINGS.with(|batcher| batcher.borrow_mut().flush()) {
            warn(&format!("readings refused, requeued: {error:?}"));
        }
    }

    /// `actuate` and `reset`: publish one command frame to the device anchored at `node_id`.
    fn on_command(command: String, args: String, _caller: Caller) -> Result<String, CommandError> {
        let args: Value = serde_json::from_str(&args).map_err(|error| CommandError::InvalidInput(error.to_string()))?;
        let node_id = args.get("node_id").and_then(Value::as_str).ok_or_else(|| CommandError::InvalidInput("node_id is required".into()))?;
        let action = match command.as_str() {
            "reset" => "reset",
            "actuate" => args.get("action").and_then(Value::as_str).ok_or_else(|| CommandError::InvalidInput("action is required".into()))?,
            other => return Err(CommandError::Refused(format!("unknown command {other}"))),
        };
        let node = scene::get_node(node_id)
            .map_err(|error| CommandError::Failed(format!("{error:?}")))?
            .ok_or_else(|| CommandError::Refused("node_id is not a node of this petal".into()))?;
        let properties: Value = serde_json::from_str(&node.properties).unwrap_or(Value::Null);
        let device_id = properties
            .get("device.id")
            .and_then(Value::as_str)
            .filter(|id| mapping::valid_device_id(id))
            .ok_or_else(|| CommandError::Refused("node carries no device.id".into()))?;

        let topic = format!("{SITE}/{device_id}/command");
        let payload = json!({ "action": action }).to_string();
        egress::mqtt_publish("actuate", &topic, payload.as_bytes(), MqttQos::AtLeastOnce, false)
            .map_err(|error| CommandError::Failed(format!("{error:?}")))?;
        Ok(json!({ "queued": true, "topic": topic }).to_string())
    }

    /// Outbound HTTP is not used here; log anything that arrives so a manifest change cannot fail silently.
    fn on_egress_result(request: u64, outcome: Result<IncomingResponse, HostError>) {
        if let Err(error) = outcome {
            warn(&format!("egress request {request} failed: {error:?}"));
        }
    }
}

extension!(DeviceAdapter);
