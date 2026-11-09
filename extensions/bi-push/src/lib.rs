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

/// A rendered body waiting for the sink.
struct Pending {
    content_type: &'static str,
    body: Vec<u8>,
}

thread_local! {
    static SUBSCRIPTION: Cell<Option<u64>> = const { Cell::new(None) };
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
    static IN_FLIGHT: Cell<Option<u64>> = const { Cell::new(None) };
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
    PENDING.with(|slot| *slot.borrow_mut() = Some(Pending { content_type: format.content_type(), body }));
}

/// Put a body back unless a newer snapshot already replaced it.
fn requeue(pending: Pending) {
    PENDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(pending);
        }
    });
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

    /// Send the pending body when no request is in flight; a failed push is retried by the next snapshot.
    fn on_tick(_info: TickInfo) {
        if SUBSCRIPTION.with(Cell::get).is_none() {
            if let Err(error) = subscribe() {
                warn(&format!("resubscribe refused: {error:?}"));
            }
        }
        if IN_FLIGHT.with(Cell::get).is_some() {
            return;
        }
        let Some(pending) = PENDING.with(|slot| slot.borrow_mut().take()) else { return };
        let request = OutgoingRequest {
            method: HttpMethod::Post,
            path: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            content_type: Some(pending.content_type.to_string()),
            body: pending.body.clone(),
        };
        match egress::http_request(SINK, &request) {
            Ok(id) => IN_FLIGHT.with(|slot| slot.set(Some(id))),
            Err(error) => {
                warn(&format!("push refused: {error:?}"));
                requeue(pending);
            }
        }
    }

    fn on_egress_result(request: u64, outcome: Result<IncomingResponse, HostError>) {
        if IN_FLIGHT.with(Cell::get) == Some(request) {
            IN_FLIGHT.with(|slot| slot.set(None));
        }
        match outcome {
            Ok(response) if (200..300).contains(&response.status) => {}
            Ok(response) => warn(&format!("sink answered {}; the next snapshot retries", response.status)),
            Err(error) => warn(&format!("push failed: {error:?}; the next snapshot retries")),
        }
    }
}

extension!(BiPush);
