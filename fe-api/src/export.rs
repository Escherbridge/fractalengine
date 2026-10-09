//! Parquet/CSV export endpoints (FR-2, plan Tasks 2.3/2.4/5.2) — see `fe-api/AGENTS.md` §export.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_entity_store::{EntitySnapshot, ReadingSnapshot};
use fe_identity::api_token::ApiClaims;
use fe_query::columnar::geoparquet::{
    write_nodes_parquet_bytes, write_readings_parquet_bytes, GeoParquetMeta,
};
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::crs::{resolve_petal_crs, CRS_EPSG_4326};
use crate::limits;
use crate::query_guard::{self, GuardMode};
use crate::server::ApiState;
use crate::types::is_valid_ulid;

/// Query-string parameters shared by both export routes.
#[derive(Debug, Default, Deserialize)]
pub struct ExportParams {
    /// SurrealQL SELECT against the node table; defaults to the whole petal.
    pub query: Option<String>,
    /// `local` (default, petal-local meters) or `latlon` (WGS84 via the petal origin).
    pub coords: Option<String>,
}

/// Output coordinate frame for an export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coords {
    Local,
    LatLon,
}

/// Structured JSON error with a real HTTP status (assets/gis precedent).
fn err(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

/// Map guard/execution error strings onto HTTP statuses.
fn query_err_response(e: String) -> Response {
    if e.starts_with("row cap exceeded") {
        err(StatusCode::PAYLOAD_TOO_LARGE, &e)
    } else if query_guard::is_timeout_error(&e) {
        err(StatusCode::GATEWAY_TIMEOUT, &e)
    } else if e.starts_with("rate limit exceeded") {
        err(StatusCode::TOO_MANY_REQUESTS, &e)
    } else if e.starts_with("egress busy") {
        // DEC-C19 N3: the channel-fallback semaphore failed closed — an
        // honest, bounded 503, not a retry-blind 502.
        err(StatusCode::SERVICE_UNAVAILABLE, &e)
    } else {
        err(StatusCode::BAD_GATEWAY, &e)
    }
}

/// Parse the `coords` param (`local` default).
#[allow(clippy::result_large_err)] // Err is the axum error Response itself
pub(crate) fn parse_coords(raw: Option<&str>) -> Result<Coords, Response> {
    match raw {
        None | Some("local") => Ok(Coords::Local),
        Some("latlon") => Ok(Coords::LatLon),
        Some(other) => Err(err(
            StatusCode::BAD_REQUEST,
            &format!("invalid coords '{other}' (expected 'local' or 'latlon')"),
        )),
    }
}

/// Viewer role + valid ULID + petal-scope coverage; deny-by-default.
async fn authorize_petal_export(
    state: &ApiState,
    claims: &ApiClaims,
    petal_id: &str,
) -> Result<(), Response> {
    if require_role(claims, "viewer").is_err() {
        return Err(err(StatusCode::FORBIDDEN, "insufficient permissions"));
    }
    if !is_valid_ulid(petal_id) {
        return Err(err(StatusCode::BAD_REQUEST, "invalid petal_id"));
    }
    let Some(scope) = crate::rest::resolve_petal_scope(state, petal_id).await else {
        return Err(err(StatusCode::NOT_FOUND, "unknown petal"));
    };
    if require_scope(claims, &scope).is_err() {
        tracing::warn!(petal_id, sub = %claims.sub, "export denied: token scope does not cover petal");
        return Err(err(StatusCode::FORBIDDEN, "insufficient scope"));
    }
    Ok(())
}

/// Which row-shaping path an export/share query needs (A23). Shared between
/// `prepare_export`'s runtime dispatch and `issue_share_url`'s mint-time
/// check (share.rs) so the two surfaces agree on the whitelist — see
/// `fe-api/AGENTS.md` §export.
pub(crate) enum ExportShape {
    Node,
    IotReading,
}

/// Classify the (single, `GuardMode::Export`-validated) FROM table as NODE or
/// IOT_READING (400 otherwise) — strictly which row mapper the export uses.
pub(crate) fn classify_export_table(tables: &[String]) -> Result<ExportShape, Response> {
    match tables.first().map(String::as_str) {
        Some("NODE") => Ok(ExportShape::Node),
        Some("IOT_READING") => Ok(ExportShape::IotReading),
        _ => Err(err(
            StatusCode::BAD_REQUEST,
            "export queries must target the node or iot_reading table",
        )),
    }
}

/// The shaped rows for one export, keyed by which table they came from.
pub(crate) enum ExportRows {
    Nodes(Vec<EntitySnapshot>),
    Readings(Vec<ReadingSnapshot>),
}

/// Result of the shared export pipeline: shaped rows + the CRS label to stamp.
pub(crate) struct ExportOutput {
    pub rows: ExportRows,
    pub crs_label: String,
    pub coords: Coords,
}

/// Guarded export pipeline shared with share-URL redemption: Export-dialect
/// validation (no comments / nesting, projection forced to `*`), node or
/// iot_reading only, source-substituted petal filter, DB-side TIMEOUT, export
/// row cap, row-level petal re-check, CRS resolution + optional lat/lon.
pub(crate) async fn prepare_export(
    state: &ApiState,
    rate_key: &str,
    petal_id: &str,
    sql: &str,
    coords: Coords,
) -> Result<ExportOutput, Response> {
    if let Err(e) = query_guard::check_rate_limit(
        state,
        rate_key,
        limits::EXPORT_RATE_PER_SEC,
        "10 export requests/sec",
    )
    .await
    {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, &e));
    }
    // FR-6 / M4 fix B1: the petal filter is substituted INTO the FROM source,
    // so no query text can widen it (AGENTS.md §query-guard).
    let prepared =
        match query_guard::prepare_scoped_sql(sql, &[petal_id.to_string()], GuardMode::Export) {
            Ok(p) => p,
            Err(e) => return Err(err(StatusCode::BAD_REQUEST, &e)),
        };
    // Exports map rows onto the node/EntitySnapshot or reading/ReadingSnapshot
    // shape — every other table is /query territory.
    let shape = classify_export_table(&prepared.tables)?;
    let guarded = query_guard::GuardedQuery { sql: prepared.sql };

    let vars = std::collections::HashMap::new();
    let rows =
        query_guard::run_guarded_query_via_state(state, &guarded, &vars, limits::EXPORT_ROW_CAP)
            .await
            .map_err(query_err_response)?;

    // Egress-point re-check, independent of the SQL rewrite: a row leaves
    // only if its own `petal_id` IS the authorized petal (`*` projection
    // guarantees the real column is present).
    let before = rows.len();
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .filter(|r| r.get("petal_id").and_then(serde_json::Value::as_str) == Some(petal_id))
        .collect();
    let dropped_foreign_rows = before - rows.len();
    if dropped_foreign_rows > 0 {
        tracing::warn!(
            petal_id,
            dropped_foreign_rows,
            "export post-filter dropped rows outside the authorized petal"
        );
    }

    // FR-5: resolve the petal CRS; latlon requires a configured terrain origin.
    let crs = resolve_petal_crs(state, petal_id).await;
    let (proj, crs_label) = match coords {
        Coords::Local => (None, crs.label),
        Coords::LatLon => {
            let Some(proj) = crs.projection else {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "coords=latlon requires a petal terrain origin (lat/lon); none configured",
                ));
            };
            (Some(proj), CRS_EPSG_4326.to_string())
        }
    };

    let export_rows = match shape {
        ExportShape::Node => ExportRows::Nodes(
            rows.iter()
                .map(|r| row_to_snapshot(r, proj.as_ref()))
                .collect(),
        ),
        ExportShape::IotReading => {
            // Deterministic batched join (never per-row): collect the
            // distinct anchor node_ids this page of readings touches, then
            // resolve their positions in ONE second guarded query — see
            // `fetch_anchor_positions` + AGENTS.md §export.
            let mut seen = std::collections::HashSet::new();
            let node_ids: Vec<&str> = rows
                .iter()
                .filter_map(|r| r["node_id"].as_str())
                .filter(|id| seen.insert(*id))
                .collect();
            let anchors = fetch_anchor_positions(state, petal_id, &node_ids).await?;
            ExportRows::Readings(
                rows.iter()
                    .map(|r| {
                        let anchor = r["node_id"].as_str().and_then(|id| anchors.get(id));
                        reading_row_to_snapshot(r, anchor, proj.as_ref())
                    })
                    .collect(),
            )
        }
    };

    Ok(ExportOutput {
        rows: export_rows,
        crs_label,
        coords,
    })
}

/// Resolve anchor-node positions for a batch of `node_id`s, scoped to
/// `petal_id`, in a single guarded query (fixed shape, server-built — not
/// user SQL, so it is bound via `$pid`/`$ids` rather than re-run through
/// `validate_select_sql`). Missing anchors (hard-deleted nodes) and rows
/// without a readable position are absent from the map, so
/// `reading_row_to_snapshot` emits a null geometry — never a fabricated 0.0.
/// Tombstoned anchors are NOT filtered: they still export their last position.
async fn fetch_anchor_positions(
    state: &ApiState,
    petal_id: &str,
    node_ids: &[&str],
) -> Result<std::collections::HashMap<String, (f64, f64, f64)>, Response> {
    let mut map = std::collections::HashMap::new();
    if node_ids.is_empty() {
        return Ok(map);
    }
    let guarded = query_guard::GuardedQuery {
        sql: "SELECT node_id, position, elevation FROM node WHERE petal_id = $pid AND node_id IN $ids"
            .to_string(),
    };
    let mut vars = std::collections::HashMap::new();
    vars.insert("pid".to_string(), serde_json::json!(petal_id));
    vars.insert("ids".to_string(), serde_json::json!(node_ids));
    let rows =
        query_guard::run_guarded_query_via_state(state, &guarded, &vars, limits::EXPORT_ROW_CAP)
            .await
            .map_err(query_err_response)?;
    for row in rows {
        let Some(node_id) = row["node_id"].as_str() else {
            continue;
        };
        let coords = &row["position"]["coordinates"];
        let (Some(x), Some(z)) = (coords[0].as_f64(), coords[1].as_f64()) else {
            continue;
        };
        // `elevation` is `TYPE float DEFAULT 0.0` in the schema: absent = 0.0.
        let y = row["elevation"].as_f64().unwrap_or(0.0);
        map.insert(node_id.to_string(), (x, y, z));
    }
    Ok(map)
}

/// GET /api/v1/petals/:petal_id/export.parquet?query=...&coords=local|latlon
pub async fn export_parquet(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(params): Query<ExportParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = authorize_petal_export(&state, &claims, &petal_id).await {
        return resp;
    }
    let coords = match parse_coords(params.coords.as_deref()) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let sql = params.query.unwrap_or_else(|| {
        format!("SELECT * FROM node WHERE petal_id = '{petal_id}' AND tombstone = NONE")
    });
    let rate_key = format!("export:{}", claims.sub);
    let out = match prepare_export(&state, &rate_key, &petal_id, &sql, coords).await {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    parquet_response(&out, &format!("{petal_id}.parquet"), &headers)
}

/// GET /api/v1/petals/:petal_id/export.csv?query=...&coords=local|latlon
pub async fn export_csv(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(params): Query<ExportParams>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = authorize_petal_export(&state, &claims, &petal_id).await {
        return resp;
    }
    let coords = match parse_coords(params.coords.as_deref()) {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let sql = params.query.unwrap_or_else(|| {
        format!("SELECT * FROM node WHERE petal_id = '{petal_id}' AND tombstone = NONE")
    });
    let rate_key = format!("export:{}", claims.sub);
    let out = match prepare_export(&state, &rate_key, &petal_id, &sql, coords).await {
        Ok(o) => o,
        Err(resp) => return resp,
    };
    csv_response(&out, &format!("{petal_id}.csv"), &headers)
}

/// The conditional-range request headers `body_response` honors (DEC-C10/C18).
struct RangeRequest<'a> {
    range: Option<&'a str>,
    if_range: Option<&'a str>,
}

impl<'a> RangeRequest<'a> {
    fn from_headers(headers: &'a HeaderMap) -> Self {
        let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
        Self {
            range: get(header::RANGE),
            if_range: get(header::IF_RANGE),
        }
    }
}

/// Serialize an export to a parquet HTTP response (DuckDB-httpfs-friendly headers).
pub(crate) fn parquet_response(
    out: &ExportOutput,
    filename: &str,
    headers: &HeaderMap,
) -> Response {
    let meta = GeoParquetMeta {
        crs: out.crs_label.clone(),
        // GeoParquet: an ABSENT `crs` key means OGC:CRS84 (lon/lat) — true
        // only for latlon exports; petal-local frames keep `crs: null`.
        lonlat_crs84: out.coords == Coords::LatLon,
        ..Default::default()
    };
    let written = match &out.rows {
        ExportRows::Nodes(snapshots) => write_nodes_parquet_bytes(snapshots, &meta),
        ExportRows::Readings(rows) => write_readings_parquet_bytes(rows, &meta),
    };
    let bytes = match written {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "parquet export serialization failed");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "parquet serialization failed",
            );
        }
    };
    if bytes.len() > limits::EXPORT_MAX_BYTES {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "export exceeds size ceiling ({})",
                limits::EXPORT_MAX_BYTES_LABEL
            ),
        );
    }
    body_response(
        bytes,
        "application/vnd.apache.parquet",
        &out.crs_label,
        filename,
        RangeRequest::from_headers(headers),
    )
}

/// Serialize an export to a CSV HTTP response.
pub(crate) fn csv_response(out: &ExportOutput, filename: &str, headers: &HeaderMap) -> Response {
    let csv = match &out.rows {
        ExportRows::Nodes(snapshots) => snapshots_to_csv(snapshots, out.coords, &out.crs_label),
        ExportRows::Readings(rows) => readings_to_csv(rows, out.coords, &out.crs_label),
    };
    if csv.len() > limits::EXPORT_MAX_BYTES {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            &format!(
                "export exceeds size ceiling ({})",
                limits::EXPORT_MAX_BYTES_LABEL
            ),
        );
    }
    body_response(
        csv.into_bytes(),
        "text/csv; charset=utf-8",
        &out.crs_label,
        filename,
        RangeRequest::from_headers(headers),
    )
}

/// Strong entity tag over the exact body bytes (quoted blake3 hex).
fn strong_etag(bytes: &[u8]) -> String {
    format!("\"{}\"", blake3::hash(bytes).to_hex())
}

/// Live query output is never cacheable (DEC-C18 — `immutable` stays assets-only).
const EXPORT_CACHE_CONTROL: &str = "private, no-store";

/// Assemble the download response with content-type/CRS/range/validator headers.
///
/// DEC-C10: one `Range: bytes=a-b` is honored as a slice of the buffered body
/// (`parse_single_byte_range`); multi-range/malformed falls back to 200; a
/// well-formed range at/past EOF answers 416. DEC-C18: every body carries a
/// strong `ETag` and `Cache-Control: private, no-store`; `If-Range` that does
/// not match the current ETag (or is a date — we send no Last-Modified)
/// downgrades the ranged GET to a full 200 so a reader can never stitch
/// ranges from two different result sets.
fn body_response(
    bytes: Vec<u8>,
    content_type: &str,
    crs_label: &str,
    filename: &str,
    request: RangeRequest<'_>,
) -> Response {
    let total = bytes.len();
    let etag = strong_etag(&bytes);
    let range = match request.if_range {
        Some(validator) if validator.trim() != etag => None,
        _ => request.range,
    };
    // F10/DEC-C10 evidence: direct server-side record of whether a client
    // (DuckDB httpfs, in the verification script) issued a Range request at
    // all, independent of any client-side logging capability.
    tracing::debug!(range = ?range, if_range = ?request.if_range, total, "export body_response: range decision");
    let (status, body, content_range) = match range.and_then(|h| parse_single_byte_range(h, total))
    {
        Some(ByteRange::Satisfiable { start, end }) => (
            StatusCode::PARTIAL_CONTENT,
            bytes[start..=end].to_vec(),
            Some(format!("bytes {start}-{end}/{total}")),
        ),
        Some(ByteRange::Unsatisfiable) => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            Vec::new(),
            Some(format!("bytes */{total}")),
        ),
        None => (StatusCode::OK, bytes, None),
    };

    let mut resp = (status, body).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = header::HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(EXPORT_CACHE_CONTROL),
    );
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        if let Some(cr) = content_range.and_then(|cr| header::HeaderValue::from_str(&cr).ok()) {
            headers.insert(header::CONTENT_RANGE, cr);
        }
        return resp;
    }
    if let Ok(v) = header::HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    // Advertise range support so DuckDB httpfs can consume the URL (plan D1)
    // — now backed by real single-range handling above (DEC-C10).
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    if let Some(cr) = content_range {
        if let Ok(v) = header::HeaderValue::from_str(&cr) {
            headers.insert(header::CONTENT_RANGE, v);
        }
    }
    if let Ok(v) = header::HeaderValue::from_str(crs_label) {
        headers.insert("x-fe-crs", v);
    }
    if let Ok(v) = header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    resp
}

/// Outcome of matching a `Range` header against a known body length.
#[derive(Debug)]
enum ByteRange {
    Satisfiable { start: usize, end: usize },
    Unsatisfiable,
}

/// Parse a single-range `Range: bytes=<start>-<end>` header (RFC 7233 §2.1),
/// including the open-ended (`bytes=N-`) and suffix (`bytes=-N`) forms.
/// Returns `None` for anything this server declines to honor as a partial
/// response (missing/malformed header, non-`bytes` unit, or a comma-separated
/// multi-range request) — the caller then serves the full 200 body, which is
/// always a valid response to a declined Range. Returns
/// `Some(Unsatisfiable)` only when the range is well-formed but selects no
/// byte (starts at/past EOF, or the empty suffix `bytes=-0`) — 416, not 200.
fn parse_single_byte_range(header_value: &str, total: usize) -> Option<ByteRange> {
    let spec = header_value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    if total == 0 {
        return Some(ByteRange::Unsatisfiable);
    }
    let last = total - 1;
    if start_s.is_empty() {
        // Suffix range: `bytes=-N` = the last N bytes; `-0` selects nothing
        // (RFC 7233 §2.1: unsatisfiable).
        let suffix_len: usize = end_s.parse().ok()?;
        if suffix_len == 0 {
            return Some(ByteRange::Unsatisfiable);
        }
        let start = last.saturating_sub(suffix_len.saturating_sub(1));
        return Some(ByteRange::Satisfiable { start, end: last });
    }
    let start: usize = start_s.parse().ok()?;
    if start > last {
        return Some(ByteRange::Unsatisfiable);
    }
    let end = if end_s.is_empty() {
        last
    } else {
        end_s.parse::<usize>().ok()?.min(last)
    };
    if end < start {
        return None;
    }
    Some(ByteRange::Satisfiable { start, end })
}

// ---------------------------------------------------------------------------
// Row mapping + CSV serialization
// ---------------------------------------------------------------------------

/// Map a node-table row onto an `EntitySnapshot`; `proj` converts to WGS84
/// (position becomes `[lon, lat, ele]` — GeoParquet EPSG:4326 axis order).
pub(crate) fn row_to_snapshot(
    row: &serde_json::Value,
    proj: Option<&fe_terrain::projection::Projection>,
) -> EntitySnapshot {
    let coords = &row["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0);
    let z = coords[1].as_f64().unwrap_or(0.0);
    let y = row["elevation"].as_f64().unwrap_or(0.0);

    let position = match proj {
        Some(p) => {
            let (lat, lon, ele) = p.local_to_wgs84(x, y, z);
            [lon as f32, lat as f32, ele as f32]
        }
        None => [x as f32, y as f32, z as f32],
    };

    let properties = match row.get("properties") {
        Some(v) if !v.is_null() => Some(v.clone()),
        _ => None,
    };
    let updated_at_ms = row
        .get("updated_at")
        .or_else(|| row.get("created_at"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .unwrap_or(0);

    EntitySnapshot {
        node_id: row["node_id"].as_str().unwrap_or_default().to_string(),
        petal_id: row["petal_id"].as_str().unwrap_or_default().to_string(),
        position,
        rotation: crate::rest::parse_f32_array3(&row["rotation"], 0.0),
        scale: crate::rest::parse_f32_array3(&row["scale"], 1.0),
        properties,
        updated_at_ms,
        node_log: Vec::new(),
    }
}

/// Map a reading row + its (possibly absent) resolved anchor position onto a
/// [`ReadingSnapshot`]. `anchor` / `proj` mirror `row_to_snapshot`'s
/// local-vs-latlon handling exactly, applied to the anchor instead of a
/// node's own geometry (readings carry none).
pub(crate) fn reading_row_to_snapshot(
    row: &serde_json::Value,
    anchor: Option<&(f64, f64, f64)>,
    proj: Option<&fe_terrain::projection::Projection>,
) -> ReadingSnapshot {
    let anchor_position = anchor.map(|&(x, y, z)| match proj {
        Some(p) => {
            let (lat, lon, ele) = p.local_to_wgs84(x, y, z);
            [lon as f32, lat as f32, ele as f32]
        }
        None => [x as f32, y as f32, z as f32],
    });
    ReadingSnapshot {
        reading_id: row["reading_id"].as_str().unwrap_or_default().to_string(),
        node_id: row["node_id"].as_str().unwrap_or_default().to_string(),
        petal_id: row["petal_id"].as_str().unwrap_or_default().to_string(),
        metric: row["metric"].as_str().unwrap_or_default().to_string(),
        value: row["value"].as_f64().unwrap_or(0.0),
        units: row["units"].as_str().unwrap_or_default().to_string(),
        recorded_at: row["recorded_at"].as_str().unwrap_or_default().to_string(),
        recorded_at_ms: row["recorded_at_ms"].as_i64().unwrap_or(0),
        anchor_position,
    }
}

/// RFC-4180 CSV with a leading `# crs=` comment line; properties as one JSON
/// string column (choice documented in `fe-api/AGENTS.md` §export).
pub(crate) fn snapshots_to_csv(
    snapshots: &[EntitySnapshot],
    coords: Coords,
    crs_label: &str,
) -> String {
    let position_headers = match coords {
        Coords::Local => "x_m,y_m,z_m",
        // row_to_snapshot stores [lon, lat, ele] in latlon mode.
        Coords::LatLon => "lon,lat,ele_m",
    };
    let mut out = String::new();
    out.push_str(&format!("# crs={crs_label}\r\n"));
    out.push_str(&format!(
        "node_id,petal_id,{position_headers},rotation_x,rotation_y,rotation_z,scale_x,scale_y,scale_z,properties,updated_at_ms\r\n"
    ));
    for s in snapshots {
        let props = s
            .properties
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_default();
        let fields = [
            csv_escape(&s.node_id),
            csv_escape(&s.petal_id),
            s.position[0].to_string(),
            s.position[1].to_string(),
            s.position[2].to_string(),
            s.rotation[0].to_string(),
            s.rotation[1].to_string(),
            s.rotation[2].to_string(),
            s.scale[0].to_string(),
            s.scale[1].to_string(),
            s.scale[2].to_string(),
            csv_escape(&props),
            s.updated_at_ms.to_string(),
        ];
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// RFC-4180 CSV for readings exports (flat rows + anchor position columns,
/// same local/latlon header split as `snapshots_to_csv`). `value` renders at
/// full `f64` precision — see `fe-api/AGENTS.md` §export.
pub(crate) fn readings_to_csv(rows: &[ReadingSnapshot], coords: Coords, crs_label: &str) -> String {
    let position_headers = match coords {
        Coords::Local => "anchor_x_m,anchor_y_m,anchor_z_m",
        Coords::LatLon => "anchor_lon,anchor_lat,anchor_ele_m",
    };
    let mut out = String::new();
    out.push_str(&format!("# crs={crs_label}\r\n"));
    out.push_str(&format!(
        "reading_id,node_id,petal_id,metric,value,units,recorded_at,recorded_at_ms,{position_headers}\r\n"
    ));
    for r in rows {
        let (px, py, pz) = match r.anchor_position {
            Some(p) => (p[0].to_string(), p[1].to_string(), p[2].to_string()),
            None => (String::new(), String::new(), String::new()),
        };
        let fields = [
            csv_escape(&r.reading_id),
            csv_escape(&r.node_id),
            csv_escape(&r.petal_id),
            csv_escape(&r.metric),
            r.value.to_string(),
            csv_escape(&r.units),
            csv_escape(&r.recorded_at),
            r.recorded_at_ms.to_string(),
            px,
            py,
            pz,
        ];
        out.push_str(&fields.join(","));
        out.push_str("\r\n");
    }
    out
}

/// RFC-4180 field quoting: wrap + double internal quotes when needed.
fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escape_rfc4180() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_has_crs_comment_and_headers() {
        let snap = EntitySnapshot {
            node_id: "n1".into(),
            petal_id: "p1".into(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: Some(serde_json::json!({"k": "v,with,commas"})),
            updated_at_ms: 42,
            node_log: vec![],
        };
        let csv = snapshots_to_csv(&[snap], Coords::Local, "PETAL-LOCAL:meters;origin=unset");
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some("# crs=PETAL-LOCAL:meters;origin=unset"));
        let header = lines.next().unwrap();
        assert!(
            header.starts_with("node_id,petal_id,x_m,y_m,z_m,"),
            "{header}"
        );
        let row = lines.next().unwrap();
        assert!(row.starts_with("n1,p1,1,2,3,"), "{row}");
        assert!(
            row.contains("\"{\"\"k\"\":\"\"v,with,commas\"\"}\""),
            "{row}"
        );
    }

    #[test]
    fn csv_latlon_headers_lon_lat() {
        let csv = snapshots_to_csv(&[], Coords::LatLon, "EPSG:4326");
        assert!(csv.contains("node_id,petal_id,lon,lat,ele_m,"), "{csv}");
        assert!(csv.starts_with("# crs=EPSG:4326"));
    }

    // ── DEC-C10: Range support (F10) ───────────────────────────────────────

    #[test]
    fn range_parses_satisfiable_start_end() {
        match parse_single_byte_range("bytes=2-5", 10) {
            Some(ByteRange::Satisfiable { start, end }) => {
                assert_eq!((start, end), (2, 5));
            }
            other => panic!("expected satisfiable 2-5, got {other:?}"),
        }
    }

    #[test]
    fn range_open_ended_clamps_to_last_byte() {
        // bytes=7- on a 10-byte body means "byte 7 through the end" (9).
        match parse_single_byte_range("bytes=7-", 10) {
            Some(ByteRange::Satisfiable { start, end }) => assert_eq!((start, end), (7, 9)),
            other => panic!("expected satisfiable 7-9, got {other:?}"),
        }
    }

    #[test]
    fn range_suffix_form_counts_from_end() {
        // bytes=-3 on a 10-byte body means "the last 3 bytes" (7-9).
        match parse_single_byte_range("bytes=-3", 10) {
            Some(ByteRange::Satisfiable { start, end }) => assert_eq!((start, end), (7, 9)),
            other => panic!("expected satisfiable 7-9, got {other:?}"),
        }
    }

    #[test]
    fn range_past_eof_is_unsatisfiable() {
        assert!(matches!(
            parse_single_byte_range("bytes=100-200", 10),
            Some(ByteRange::Unsatisfiable)
        ));
    }

    #[test]
    fn range_multi_range_is_declined_not_partially_honored() {
        // A comma-separated multi-range request falls back to a full 200 —
        // we never claim to serve a byte range we didn't compute.
        assert!(parse_single_byte_range("bytes=0-1,5-6", 10).is_none());
    }

    #[test]
    fn range_malformed_is_declined() {
        assert!(parse_single_byte_range("not-a-range", 10).is_none());
        assert!(parse_single_byte_range("bytes=abc-def", 10).is_none());
    }

    #[tokio::test]
    async fn body_response_honors_single_range_with_206() {
        let body = b"0123456789".to_vec();
        let resp = body_response(
            body,
            "text/csv",
            "PETAL-LOCAL:meters;origin=unset",
            "f.csv",
            rr(Some("bytes=2-5"), None),
        );
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            resp.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 2-5/10"
        );
        assert_eq!(resp.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
        let bytes = axum::body::to_bytes(resp.into_body(), 100).await.unwrap();
        assert_eq!(&bytes[..], b"2345");
    }

    #[tokio::test]
    async fn body_response_range_past_eof_is_416() {
        let body = b"0123456789".to_vec();
        let resp = body_response(
            body,
            "text/csv",
            "PETAL-LOCAL:meters;origin=unset",
            "f.csv",
            rr(Some("bytes=50-60"), None),
        );
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            resp.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes */10"
        );
    }

    #[tokio::test]
    async fn body_response_no_range_serves_full_200() {
        let body = b"0123456789".to_vec();
        let resp = body_response(
            body,
            "text/csv",
            "PETAL-LOCAL:meters;origin=unset",
            "f.csv",
            rr(None, None),
        );
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get(header::CONTENT_RANGE).is_none());
        let bytes = axum::body::to_bytes(resp.into_body(), 100).await.unwrap();
        assert_eq!(&bytes[..], b"0123456789");
    }

    #[tokio::test]
    async fn body_response_multi_range_falls_back_to_full_200() {
        let body = b"0123456789".to_vec();
        let resp = body_response(
            body,
            "text/csv",
            "PETAL-LOCAL:meters;origin=unset",
            "f.csv",
            rr(Some("bytes=0-1,5-6"), None),
        );
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 100).await.unwrap();
        assert_eq!(&bytes[..], b"0123456789");
    }

    fn rr<'a>(range: Option<&'a str>, if_range: Option<&'a str>) -> RangeRequest<'a> {
        RangeRequest { range, if_range }
    }

    fn respond(body: &[u8], range: Option<&str>, if_range: Option<&str>) -> Response {
        body_response(
            body.to_vec(),
            "text/csv",
            "PETAL-LOCAL:meters;origin=unset",
            "f.csv",
            rr(range, if_range),
        )
    }

    #[test]
    fn empty_suffix_range_is_unsatisfiable() {
        assert!(matches!(
            parse_single_byte_range("bytes=-0", 10),
            Some(ByteRange::Unsatisfiable)
        ));
        assert_eq!(
            respond(b"0123456789", Some("bytes=-0"), None).status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
    }

    /// DEC-C18: a strong ETag over the exact body, stable for identical bytes
    /// and different for different bytes, on 200 AND 206, with no-store.
    #[test]
    fn etag_is_strong_stable_and_body_derived() {
        let full = respond(b"0123456789", None, None);
        let ranged = respond(b"0123456789", Some("bytes=2-5"), None);
        let other = respond(b"0123456780", None, None);
        let etag = |r: &Response| r.headers()[header::ETAG].to_str().unwrap().to_string();
        assert!(etag(&full).starts_with('"') && etag(&full).ends_with('"'));
        assert_eq!(
            etag(&full),
            etag(&ranged),
            "206 carries the full body's ETag"
        );
        assert_ne!(etag(&full), etag(&other));
        for r in [&full, &ranged] {
            assert_eq!(r.headers()[header::CACHE_CONTROL], "private, no-store");
        }
    }

    /// If-Range: matching ETag → the range is served (206); a stale ETag or
    /// a date → the full body (200), never a slice of a different result.
    #[tokio::test]
    async fn if_range_match_serves_range_mismatch_serves_full_body() {
        let current = strong_etag(b"0123456789");
        let hit = respond(b"0123456789", Some("bytes=2-5"), Some(&current));
        assert_eq!(hit.status(), StatusCode::PARTIAL_CONTENT);

        for stale in [
            strong_etag(b"older body"),
            "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
        ] {
            let miss = respond(b"0123456789", Some("bytes=2-5"), Some(&stale));
            assert_eq!(miss.status(), StatusCode::OK, "If-Range {stale}");
            assert!(miss.headers().get(header::CONTENT_RANGE).is_none());
            let bytes = axum::body::to_bytes(miss.into_body(), 100).await.unwrap();
            assert_eq!(&bytes[..], b"0123456789");
        }
    }
}
