//! Parquet/CSV export endpoints (FR-2, plan Tasks 2.3/2.4/5.2) — see `fe-api/AGENTS.md` §export.

// `Response` is axum's own error carrier for handlers, so `Result<T, Response>` is the idiomatic
// shape here and boxing it would add indirection at every call site for no benefit. clippy 1.98
// began flagging it as `result_large_err`; allowed at module scope rather than distorting four
// signatures to satisfy a lint that does not apply to axum's conventions.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_entity_store::EntitySnapshot;
use fe_identity::api_token::ApiClaims;
use fe_query::columnar::geoparquet::{write_nodes_parquet_bytes, GeoParquetMeta};
use serde::Deserialize;

use crate::auth::{require_role, require_scope};
use crate::crs::{resolve_petal_crs, CRS_EPSG_4326};
use crate::limits;
use crate::query_guard;
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
    } else if e.contains("timed out") {
        err(StatusCode::GATEWAY_TIMEOUT, &e)
    } else if e.starts_with("rate limit exceeded") {
        err(StatusCode::TOO_MANY_REQUESTS, &e)
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

/// Result of the shared export pipeline: scoped snapshots + the CRS label to stamp.
pub(crate) struct ExportOutput {
    pub snapshots: Vec<EntitySnapshot>,
    pub crs_label: String,
    pub coords: Coords,
}

/// Guarded export pipeline shared with share-URL redemption: same static
/// validation as `/query`, node-table-only, forced petal pre-filter, 5s
/// timeout, export row cap, CRS resolution + optional lat/lon conversion.
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
    if let Err(e) = query_guard::validate_select_sql(sql) {
        return Err(err(StatusCode::BAD_REQUEST, &e));
    }
    // Exports map rows onto the node/EntitySnapshot shape — other tables are /query territory.
    let upper = sql.trim().to_uppercase();
    if query_guard::from_table(&upper).as_deref() != Some("NODE") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "export queries must target the node table",
        ));
    }

    // FR-6: pre-filter to the authorized petal regardless of the query text.
    let filter = format!("petal_id = '{}'", petal_id.replace('\'', ""));
    let guarded = query_guard::GuardedQuery {
        sql: query_guard::inject_scope_filter(sql.trim(), &filter),
    };

    let Some(ref db) = state.db_reader else {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "export endpoint not available (no db_reader)",
        ));
    };
    let vars = std::collections::HashMap::new();
    let rows = query_guard::run_guarded_query(db, &guarded, &vars, limits::EXPORT_ROW_CAP)
        .await
        .map_err(query_err_response)?;

    // FR-5: resolve the petal CRS; latlon requires a configured terrain origin.
    let crs = resolve_petal_crs(state, petal_id).await;
    let (snapshots, crs_label) = match coords {
        Coords::Local => (
            rows.iter().map(|r| row_to_snapshot(r, None)).collect(),
            crs.label,
        ),
        Coords::LatLon => {
            let Some(proj) = crs.projection else {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "coords=latlon requires a petal terrain origin (lat/lon); none configured",
                ));
            };
            (
                rows.iter()
                    .map(|r| row_to_snapshot(r, Some(&proj)))
                    .collect(),
                CRS_EPSG_4326.to_string(),
            )
        }
    };

    Ok(ExportOutput {
        snapshots,
        crs_label,
        coords,
    })
}

/// GET /api/v1/petals/:petal_id/export.parquet?query=...&coords=local|latlon
pub async fn export_parquet(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(params): Query<ExportParams>,
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
    parquet_response(&out, &format!("{petal_id}.parquet"))
}

/// GET /api/v1/petals/:petal_id/export.csv?query=...&coords=local|latlon
pub async fn export_csv(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(params): Query<ExportParams>,
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
    csv_response(&out, &format!("{petal_id}.csv"))
}

/// Serialize an export to a parquet HTTP response (DuckDB-httpfs-friendly headers).
pub(crate) fn parquet_response(out: &ExportOutput, filename: &str) -> Response {
    let meta = GeoParquetMeta {
        crs: out.crs_label.clone(),
        ..Default::default()
    };
    let bytes = match write_nodes_parquet_bytes(&out.snapshots, &meta) {
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
    )
}

/// Serialize an export to a CSV HTTP response.
pub(crate) fn csv_response(out: &ExportOutput, filename: &str) -> Response {
    let csv = snapshots_to_csv(&out.snapshots, out.coords, &out.crs_label);
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
    )
}

/// Assemble the download response with content-type/CRS/range headers.
fn body_response(bytes: Vec<u8>, content_type: &str, crs_label: &str, filename: &str) -> Response {
    let mut resp = (StatusCode::OK, bytes).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = header::HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    // Advertise range support so DuckDB httpfs can consume the URL (plan D1).
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    if let Ok(v) = header::HeaderValue::from_str(crs_label) {
        headers.insert("x-fe-crs", v);
    }
    if let Ok(v) = header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    resp
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
}
