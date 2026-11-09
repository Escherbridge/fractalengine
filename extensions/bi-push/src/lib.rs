//! bi-push: the guarded `host/query` rows pushed as JSON or CSV to one declared HTTPS sink.
//! Design notes: extensions/bi-push/AGENTS.md.

pub mod payload;

use std::cell::{Cell, RefCell};

use fe_ext_guest::bindings::exports::fractalengine::plugin::bus_hooks::DeliveryKind;
use fe_ext_guest::egress::{self, HttpMethod};
use fe_ext_guest::{bus, extension, kv, log, Binding, BusEvent, Extension, HostError, IncomingResponse, OutgoingRequest, TickInfo};
use payload::{Format, Snapshot};

/// The named query and egress ids declared in extension.json.
const QUERY_ID: &str = "latest";
const SINK: &str = "sink";
/// Mirrors the sink's `max_request_bytes`; a larger body is dropped, not cut.
const MAX_BODY_BYTES: usize = 1_048_576;
/// The kv key a host may seed with the binding's `format` config.
const FORMAT_KEY: &str = "bi-push/format";

/// The newest rendered snapshot; `generation` grows by one per snapshot.
struct Latest {
    generation: u64,
    content_type: &'static str,
    body: Vec<u8>,
}

thread_local! {
    static SUBSCRIPTION: Cell<Option<u64>> = const { Cell::new(None) };
    static LATEST: RefCell<Option<Latest>> = const { RefCell::new(None) };
    /// Newest generation the sink acknowledged with a 2xx.
    static DELIVERED: Cell<u64> = const { Cell::new(0) };
    /// The request in flight and the generation it carries.
    static IN_FLIGHT: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

fn warn(message: &str) {
    log::write(log::Level::Warn, message);
}

/// The configured format; JSON when unset or unknown.
fn configured_format() -> Format {
    kv::get(FORMAT_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<String>(&value).ok())
        .and_then(|name| Format::parse(&name))
        .unwrap_or_default()
}

/// Subscribe to the named query; the host re-runs it only when its tables change.
fn subscribe() -> Result<(), HostError> {
    let params = serde_json::json!({ "query": QUERY_ID }).to_string();
    bus::subscribe("host/query", &params).map(|id| SUBSCRIPTION.with(|slot| slot.set(Some(id))))
}

/// Keep only the newest snapshot: a slow sink never builds a backlog.
fn queue(snapshot: &Snapshot) {
    let format = configured_format();
    let body = payload::render(snapshot, format);
    if body.len() > MAX_BODY_BYTES {
        warn(&format!("snapshot body of {} bytes exceeds the sink limit; dropped", body.len()));
        return;
    }
    LATEST.with(|slot| {
        let generation = slot.borrow().as_ref().map_or(1, |latest| latest.generation + 1);
        *slot.borrow_mut() = Some(Latest { generation, content_type: format.content_type(), body });
    });
}

/// The newest snapshot as a request, unless the sink already acknowledged it.
fn undelivered() -> Option<(u64, OutgoingRequest)> {
    let delivered = DELIVERED.with(Cell::get);
    LATEST.with(|slot| {
        slot.borrow().as_ref().filter(|latest| latest.generation > delivered).map(|latest| {
            let request = OutgoingRequest {
                method: HttpMethod::Post,
                path: String::new(),
                query: Vec::new(),
                headers: Vec::new(),
                content_type: Some(latest.content_type.to_string()),
                body: latest.body.clone(),
            };
            (latest.generation, request)
        })
    })
}

struct BiPush;

impl Extension for BiPush {
    fn on_activate(_binding: Binding) -> Result<(), String> {
        subscribe().map_err(|error| format!("subscribe to host/query refused: {error:?}"))
    }

    fn on_events(events: Vec<BusEvent>) {
        for event in events {
            match event.kind {
                DeliveryKind::Snapshot if event.source == "host/query" => match payload::parse_snapshot(&event.data) {
                    Ok(snapshot) => queue(&snapshot),
                    Err(error) => warn(&format!("snapshot ignored: {error}")),
                },
                DeliveryKind::Ended => SUBSCRIPTION.with(|slot| slot.set(None)),
                // resync-required is followed by a fresh snapshot; host/query has no deltas.
                _ => {}
            }
        }
    }

    /// Every tick (the push interval) sends the newest snapshot the sink has not acknowledged, one request at a time.
    fn on_tick(_info: TickInfo) {
        if SUBSCRIPTION.with(Cell::get).is_none() {
            if let Err(error) = subscribe() {
                warn(&format!("resubscribe refused: {error:?}"));
            }
        }
        if IN_FLIGHT.with(Cell::get).is_some() {
            return;
        }
        let Some((generation, request)) = undelivered() else { return };
        match egress::http_request(SINK, &request) {
            Ok(id) => IN_FLIGHT.with(|slot| slot.set(Some((id, generation)))),
            Err(error) => warn(&format!("push refused: {error:?}; retried next tick")),
        }
    }

    /// A 2xx marks its generation delivered; anything else leaves it for the next tick.
    fn on_egress_result(request: u64, outcome: Result<IncomingResponse, HostError>) {
        let carried = match IN_FLIGHT.with(Cell::get) {
            Some((id, generation)) if id == request => {
                IN_FLIGHT.with(|slot| slot.set(None));
                Some(generation)
            }
            _ => None,
        };
        match outcome {
            Ok(response) if (200..300).contains(&response.status) => {
                if let Some(generation) = carried {
                    DELIVERED.with(|slot| slot.set(slot.get().max(generation)));
                }
            }
            Ok(response) => warn(&format!("sink answered {}; retried next tick", response.status)),
            Err(error) => warn(&format!("push failed: {error:?}; retried next tick")),
        }
    }
}

extension!(BiPush);
