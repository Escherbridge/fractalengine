//! Opt-in OpenMetrics registry and its own `/metrics` listener (see `AGENTS.md` §usage-metering).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::{header::CONTENT_TYPE, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use fe_runtime::usage::{UsageEvent, UsageSink};

/// Upper bounds (milliseconds) of the duration histogram buckets; `+Inf` is implicit.
const DURATION_BUCKETS_MS: [u64; 11] = [5, 10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

/// Methods kept as their own label value; anything else collapses to `OTHER` to bound cardinality.
const KNOWN_METHODS: [&str; 9] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT", "TRACE",
];

const OPENMETRICS_CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// Number of metrics listeners bound by this process (AC-1: stays 0 when `FE_API_METRICS_BIND` is unset).
static LISTENER_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Metrics listeners bound so far in this process.
pub fn listener_count() -> usize {
    LISTENER_COUNT.load(Ordering::SeqCst)
}

/// Duration histogram of one route class: per-bucket counts, total count, sum in milliseconds.
#[derive(Default)]
struct DurationHistogram {
    bucket_counts: [u64; DURATION_BUCKETS_MS.len()],
    count: u64,
    sum_ms: u64,
}

#[derive(Default)]
struct RegistryState {
    /// (route_class, method, status_class) -> request count; method and class are bounded static labels.
    requests: BTreeMap<(String, &'static str, &'static str), u64>,
    /// route_class -> duration histogram.
    durations: BTreeMap<String, DurationHistogram>,
}

/// Hand-rolled request counters and duration histograms keyed by route template only (never ids).
#[derive(Default)]
pub struct MetricsRegistry {
    state: Mutex<RegistryState>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// OpenMetrics text exposition of every counter and histogram, ending in `# EOF`.
    pub fn render_openmetrics(&self) -> String {
        let state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut out = String::new();
        out.push_str("# TYPE fe_api_requests counter\n");
        out.push_str("# HELP fe_api_requests API requests by route template, method and status class.\n");
        for ((route_class, method, status_class), count) in &state.requests {
            let _ = writeln!(
                out,
                "fe_api_requests_total{{route_class=\"{}\",method=\"{method}\",status_class=\"{status_class}\"}} {count}",
                escape_label(route_class),
            );
        }
        out.push_str("# TYPE fe_api_request_duration_seconds histogram\n");
        out.push_str("# UNIT fe_api_request_duration_seconds seconds\n");
        out.push_str("# HELP fe_api_request_duration_seconds API request duration by route template.\n");
        for (route_class, histogram) in &state.durations {
            let route = escape_label(route_class);
            let mut cumulative = 0u64;
            for (bound_ms, bucket) in DURATION_BUCKETS_MS.iter().zip(histogram.bucket_counts) {
                cumulative += bucket;
                let _ = writeln!(
                    out,
                    "fe_api_request_duration_seconds_bucket{{route_class=\"{route}\",le=\"{}\"}} {cumulative}",
                    bucket_bound_text(*bound_ms)
                );
            }
            let _ = writeln!(
                out,
                "fe_api_request_duration_seconds_bucket{{route_class=\"{route}\",le=\"+Inf\"}} {}",
                histogram.count
            );
            let _ = writeln!(
                out,
                "fe_api_request_duration_seconds_count{{route_class=\"{route}\"}} {}",
                histogram.count
            );
            let _ = writeln!(
                out,
                "fe_api_request_duration_seconds_sum{{route_class=\"{route}\"}} {}",
                seconds_text(histogram.sum_ms)
            );
        }
        out.push_str("# EOF\n");
        out
    }
}

impl MetricsRegistry {
    /// Counts one request synchronously; the only allocation is the route key on first sight.
    fn observe(&self, event: UsageEvent) {
        let method = KNOWN_METHODS
            .iter()
            .copied()
            .find(|known| *known == event.method)
            .unwrap_or("OTHER");
        let status_class = STATUS_CLASSES[usize::from((event.status / 100).clamp(1, 9)) - 1];
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        *state
            .requests
            .entry((event.route_class.clone(), method, status_class))
            .or_insert(0) += 1;
        let histogram = state.durations.entry(event.route_class).or_default();
        histogram.count += 1;
        histogram.sum_ms = histogram.sum_ms.saturating_add(event.duration_ms);
        // Observations above the last bound land only in the implicit +Inf bucket (count).
        if let Some(index) = DURATION_BUCKETS_MS.iter().position(|bound| event.duration_ms <= *bound) {
            histogram.bucket_counts[index] += 1;
        }
    }
}

/// Desugared `async_trait` impl (fe-api has `async-trait` only as a dev-dependency); counting never awaits.
impl UsageSink for MetricsRegistry {
    fn record<'life0, 'async_trait>(
        &'life0 self,
        event: UsageEvent,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        self.observe(event);
        Box::pin(std::future::ready(()))
    }
}

/// Status-class label values indexed by `status / 100 - 1`.
const STATUS_CLASSES: [&str; 9] = ["1xx", "2xx", "3xx", "4xx", "5xx", "6xx", "7xx", "8xx", "9xx"];

/// A histogram `le` bound in OpenMetrics canonical form (`1` -> `1.0`, `250` ms -> `0.25`).
fn bucket_bound_text(milliseconds: u64) -> String {
    let text = seconds_text(milliseconds);
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

/// Milliseconds as a decimal seconds string (`250` -> `0.25`, `1000` -> `1`).
fn seconds_text(milliseconds: u64) -> String {
    let whole = milliseconds / 1_000;
    let fraction = milliseconds % 1_000;
    if fraction == 0 {
        whole.to_string()
    } else {
        let digits = format!("{fraction:03}");
        format!("{whole}.{}", digits.trim_end_matches('0'))
    }
}

/// Escapes a label value per the OpenMetrics text format.
fn escape_label(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

/// Binds `bind` and serves only `GET /metrics` (unauthenticated, OD-7) until `stop` resolves; returns the bound address.
pub async fn spawn_metrics_listener(
    bind: &str,
    registry: Arc<MetricsRegistry>,
    stop: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let address = listener.local_addr()?;
    LISTENER_COUNT.fetch_add(1, Ordering::SeqCst);
    let router = Router::new().route(
        "/metrics",
        get(move || {
            let registry = registry.clone();
            async move {
                (
                    [(CONTENT_TYPE, HeaderValue::from_static(OPENMETRICS_CONTENT_TYPE))],
                    registry.render_openmetrics(),
                )
                    .into_response()
            }
        }),
    );
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).with_graceful_shutdown(stop).await {
            tracing::warn!(%error, "metrics listener stopped with an error");
        }
    });
    tracing::info!(%address, "metrics listener bound");
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network_ops::NetworkOps;
    use crate::server::{build_router, ApiState};
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use tower::ServiceExt;

    fn test_state() -> Arc<ApiState> {
        let (api_cmd_tx, _) = crossbeam::channel::bounded(1);
        let (transform_broadcast_tx, _) = tokio::sync::broadcast::channel(1);
        let (entity_change_tx, _) = tokio::sync::broadcast::channel(1);
        Arc::new(ApiState {
            api_cmd_tx,
            transform_broadcast_tx,
            entity_change_tx,
            verifying_key: ed25519_dalek::VerifyingKey::from_bytes(&[0u8; 32]).unwrap(),
            revoked_jtis: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            blob_store: None,
            cors_origins: vec![],
            db_reader: None,
            terrain_capture: Default::default(),
            query_rate_limiter: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            entity_store: None,
            tileset_registry: None,
            hexon_registry: None,
            announcement_store: None,
            replication_tx: None,
            distributed_tx: None,
            sim_control_tx: None,
            elevated_query_allowed: false,
            upload_quota_bytes: crate::upload::DEFAULT_UPLOAD_QUOTA_BYTES,
            screenshot: crate::screenshot::ScreenshotSurface::disabled(),
            input: crate::input::InputSurface::disabled(),
            share_signer: Arc::new(fe_identity::NodeKeypair::generate()),
        })
    }

    /// Blocking raw HTTP GET against the metrics listener; returns the full response text.
    fn raw_get(address: SocketAddr, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(address).expect("connect");
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").expect("write");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read");
        response
    }

    #[tokio::test]
    async fn metrics_endpoint_reports_request_count_and_status_for_a_real_request() {
        // AC-1: metrics unset in the config binds nothing.
        let off = NetworkOps::default();
        assert!(off.metrics.is_none(), "metrics registry only exists when a bind is set");
        assert_eq!(listener_count(), 0, "no listener without FE_API_METRICS_BIND");

        // One real request through the real router wrapped in the usage layer.
        let registry = Arc::new(MetricsRegistry::new());
        let sink: Arc<dyn UsageSink> = registry.clone();
        let router = build_router(test_state()).layer(axum::middleware::from_fn_with_state(
            sink,
            crate::usage_middleware::record_usage,
        ));
        let request = axum::http::Request::builder()
            .uri("/api/v1/health")
            .body(axum::body::Body::empty())
            .expect("request");
        let response = router.oneshot(request).await.expect("response");
        assert!(response.status().is_success(), "health answers 2xx");

        // The sink is fed from a detached task: wait for the observable, bounded.
        for _ in 0..200 {
            if registry.render_openmetrics().contains("fe_api_requests_total{") {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let address = spawn_metrics_listener("127.0.0.1:0", registry.clone(), std::future::pending())
            .await
            .expect("bind metrics listener");
        assert_eq!(listener_count(), 1);
        let body = tokio::task::spawn_blocking(move || raw_get(address, "/metrics")).await.expect("join");
        assert!(body.starts_with("HTTP/1.1 200"), "metrics served: {body}");
        assert!(body.contains("application/openmetrics-text"), "content type: {body}");
        assert!(
            body.contains(
                "fe_api_requests_total{route_class=\"/api/v1/health\",method=\"GET\",status_class=\"2xx\"} 1"
            ),
            "template + status class line: {body}"
        );
        assert!(body.contains("# TYPE fe_api_requests counter"));
        assert!(body.trim_end().ends_with("# EOF"));

        // Only /metrics is served.
        let other = tokio::task::spawn_blocking(move || raw_get(address, "/api/v1/health")).await.expect("join");
        assert!(other.starts_with("HTTP/1.1 404"), "other paths are not served: {other}");
    }
}
