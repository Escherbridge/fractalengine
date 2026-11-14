//! Per-request usage emission layer (see `AGENTS.md` §usage-metering).

use std::sync::{Arc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{MatchedPath, Request, State};
use axum::http::{header::CONTENT_LENGTH, HeaderMap};
use axum::middleware::Next;
use axum::response::Response;
use fe_runtime::usage::{UsageEvent, UsageSink};

/// Route class reported when no route template matched (never the raw path).
const UNMATCHED_ROUTE_CLASS: &str = "<unmatched>";

/// Request extension the auth layer fills with `(sub, scope)` after this layer already ran.
#[derive(Clone, Default)]
pub struct UsageCallerSlot(pub Arc<OnceLock<(String, String)>>);

/// Parses a Content-Length header; absent or malformed counts as 0.
fn content_length(headers: &HeaderMap) -> u64 {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Emits one `UsageEvent` per request, off the response path, labelled by route template only.
pub async fn record_usage(
    State(sink): State<Arc<dyn UsageSink>>,
    mut req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let route_class = req
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| UNMATCHED_ROUTE_CLASS.to_owned());
    let method = req.method().as_str().to_owned();
    let bytes_in = content_length(req.headers());
    let slot = UsageCallerSlot::default();
    req.extensions_mut().insert(slot.clone());

    let response = next.run(req).await;

    let (caller_did, scope) = match slot.0.get() {
        Some((subject, scope)) => (Some(subject.clone()), Some(scope.clone())),
        None => (None, None),
    };
    let event = UsageEvent {
        caller_did,
        scope,
        route_class,
        method,
        status: response.status().as_u16(),
        bytes_in,
        bytes_out: content_length(response.headers()),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0),
    };
    // Detached so a slow sink never delays the response.
    tokio::spawn(async move { sink.record(event).await });
    response
}
