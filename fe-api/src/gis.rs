//! Petal-scoped GIS read endpoints — geo-positioned nodes + GPX tracks.
//!
//! Rationale, the annotation-key contract, and the local-SQL / in-Rust-filter
//! tradeoff: see `fe-api/AGENTS.md` §gis.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fe_terrain::projection::Projection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::{require_role, require_scope};
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

// ---------------------------------------------------------------------------
// Reserved annotation property keys (shared contract with gis_query_ui track)
// ---------------------------------------------------------------------------

// Single source of truth: the reserved `gis.annotation.*` keys live in
// `fe_query::gis` and are re-exported here so the endpoint and the query layer
// can never drift. See `fe-api/AGENTS.md` §gis.
pub use fe_query::gis::{ANNOTATION_BODY_KEY, ANNOTATION_COLOR_KEY, ANNOTATION_TITLE_KEY};

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// A geo-positioned node with its optional annotation.
#[derive(Debug, Clone, Serialize)]
pub struct GisNodeDto {
    pub node_id: String,
    pub display_name: String,
    /// Petal-local coordinates `[x, z]` in meters (XZ plane).
    pub position: [f64; 2],
    /// Y-axis height in meters (the node's `elevation` column).
    pub elevation: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation: Option<GisAnnotationDto>,
}

/// The `gis.annotation.*` reserved-key bundle for a node.
#[derive(Debug, Clone, Serialize)]
pub struct GisAnnotationDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A GPX track node (`gpx_type == "track"`) with its cached stats.
///
/// Mirrors the property shape written by `fe_terrain::gpx` conversion.
#[derive(Debug, Clone, Serialize)]
pub struct GisTrackDto {
    pub node_id: String,
    pub display_name: String,
    pub position: [f64; 2],
    pub elevation: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_distance_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation_gain_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation_loss_m: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_speed_kmh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speed_kmh: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounding_box: Option<Value>,
}

/// Query string for the `/gis/nodes` endpoint. At most one spatial filter may
/// be supplied (`bbox`, `bbox_ll`, or the `radius`/`cx`/`cz` triple).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct GisNodesQuery {
    /// Local-meter AABB: `minx,minz,maxx,maxz`.
    pub bbox: Option<String>,
    /// Lat/lon AABB: `minLat,minLon,maxLat,maxLon` (needs a petal terrain origin).
    pub bbox_ll: Option<String>,
    /// Radius in local meters (requires `cx` + `cz`).
    pub radius: Option<f64>,
    /// Circle center X in local meters.
    pub cx: Option<f64>,
    /// Circle center Z in local meters.
    pub cz: Option<f64>,
}

// ---------------------------------------------------------------------------
// Spatial filter primitives (pure — unit-tested from tests/gis_test.rs)
// ---------------------------------------------------------------------------

/// Axis-aligned bounding box in petal-local meters (XZ plane).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    pub minx: f64,
    pub minz: f64,
    pub maxx: f64,
    pub maxz: f64,
}

impl Bbox {
    /// Inclusive containment test for a local `[x, z]` point.
    pub fn contains(&self, x: f64, z: f64) -> bool {
        x >= self.minx && x <= self.maxx && z >= self.minz && z <= self.maxz
    }
}

/// Parse a `minx,minz,maxx,maxz` local bbox string, normalizing min/max order.
pub fn parse_bbox(s: &str) -> Result<Bbox, String> {
    let (a, b, c, d) = parse_four(s)?;
    Ok(Bbox {
        minx: a.min(c),
        minz: b.min(d),
        maxx: a.max(c),
        maxz: b.max(d),
    })
}

/// Euclidean radius test in local meters: `(x-cx)^2 + (z-cz)^2 <= r^2`.
pub fn within_radius(x: f64, z: f64, cx: f64, cz: f64, r: f64) -> bool {
    let dx = x - cx;
    let dz = z - cz;
    dx * dx + dz * dz <= r * r
}

/// Convert a `minLat,minLon,maxLat,maxLon` string to a local [`Bbox`] using the
/// petal's terrain-origin projection (equirectangular, via `fe_terrain`).
pub fn bbox_ll_to_local(s: &str, proj: &Projection) -> Result<Bbox, String> {
    let (min_lat, min_lon, max_lat, max_lon) = parse_four(s)?;
    let a = proj
        .wgs84_to_local(min_lat, min_lon, proj.origin_ele)
        .map_err(|e| e.to_string())?;
    let b = proj
        .wgs84_to_local(max_lat, max_lon, proj.origin_ele)
        .map_err(|e| e.to_string())?;
    // Projection returns [x=easting, y=up, z=northing]; keep the XZ plane.
    let (ax, az) = (a[0], a[2]);
    let (bx, bz) = (b[0], b[2]);
    Ok(Bbox {
        minx: ax.min(bx),
        minz: az.min(bz),
        maxx: ax.max(bx),
        maxz: az.max(bz),
    })
}

fn parse_four(s: &str) -> Result<(f64, f64, f64, f64), String> {
    let parts: Vec<&str> = s.split(',').map(|p| p.trim()).collect();
    if parts.len() != 4 {
        return Err("expected 4 comma-separated numbers".to_string());
    }
    let mut v = [0.0f64; 4];
    for (i, p) in parts.iter().enumerate() {
        v[i] = p
            .parse::<f64>()
            .map_err(|_| format!("invalid number: {p:?}"))?;
    }
    Ok((v[0], v[1], v[2], v[3]))
}

/// A resolved spatial predicate applied in-process after loading petal nodes.
enum SpatialFilter {
    Bbox(Bbox),
    Radius { cx: f64, cz: f64, r: f64 },
}

impl SpatialFilter {
    fn contains(&self, x: f64, z: f64) -> bool {
        match self {
            SpatialFilter::Bbox(b) => b.contains(x, z),
            SpatialFilter::Radius { cx, cz, r } => within_radius(x, z, *cx, *cz, *r),
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/petals/:petal_id/gis/nodes
///
/// Geo-positioned nodes with their `gis.annotation.*` properties. Optional
/// `bbox` / `bbox_ll` / `radius` spatial filters (local meters, or lat/lon
/// converted API-side via the petal terrain origin).
///
/// RBAC: Viewer+ on the petal scope. Deny-by-default.
pub async fn list_gis_nodes(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(q): Query<GisNodesQuery>,
) -> Response {
    if let Err(status) = authorize_petal_read(&state, &claims, &petal_id).await {
        return status;
    }

    let filter = match build_spatial_filter(&state, &petal_id, &q).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };

    // `created_at` is projected only because SurrealDB 3 rejects ORDER BY on
    // a field absent from an explicit projection list.
    let sql = "SELECT node_id, display_name, position, elevation, properties, created_at \
               FROM node WHERE petal_id = $pid ORDER BY created_at ASC";
    let Some(rows) = run_select(&state, sql, vec![("pid".to_string(), json!(petal_id))]).await
    else {
        return err(StatusCode::BAD_GATEWAY, "node query failed");
    };

    let mut nodes = Vec::with_capacity(rows.len());
    for row in &rows {
        let (x, z, elevation) = row_position(row);
        if let Some(ref f) = filter {
            if !f.contains(x, z) {
                continue;
            }
        }
        let properties = row.get("properties").cloned().unwrap_or(Value::Null);
        nodes.push(GisNodeDto {
            node_id: str_field(row, "node_id"),
            display_name: str_field(row, "display_name"),
            position: [x, z],
            elevation,
            annotation: extract_annotation(&properties),
        });
    }

    ok(json!({ "petal_id": petal_id, "nodes": nodes }))
}

/// GET /api/v1/petals/:petal_id/gis/tracks
///
/// GPX track nodes (`gpx_type == "track"`) bound to the petal, with the cached
/// stats written by GPX import. Mirrors the `gpx.rs` property shape.
///
/// RBAC: Viewer+ on the petal scope. Deny-by-default.
pub async fn list_gis_tracks(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if let Err(status) = authorize_petal_read(&state, &claims, &petal_id).await {
        return status;
    }

    let sql = "SELECT node_id, display_name, position, elevation, properties, created_at \
               FROM node WHERE petal_id = $pid AND properties.gpx_type = 'track' \
               ORDER BY created_at ASC";
    let Some(rows) = run_select(&state, sql, vec![("pid".to_string(), json!(petal_id))]).await
    else {
        return err(StatusCode::BAD_GATEWAY, "track query failed");
    };

    let tracks: Vec<GisTrackDto> = rows.iter().map(row_to_track).collect();
    ok(json!({ "petal_id": petal_id, "tracks": tracks }))
}

// ---------------------------------------------------------------------------
// Auth (deny-by-default, mirroring assets.rs precedent: real status codes)
// ---------------------------------------------------------------------------

/// Viewer+ role + petal-scope check. `Err(Response)` is the ready-to-return
/// rejection (403 role/scope, 400 bad ULID, 404 unknown petal).
async fn authorize_petal_read(
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
    let Some(scope) = resolve_petal_scope(state, petal_id).await else {
        return Err(err(StatusCode::NOT_FOUND, "petal not found"));
    };
    if require_scope(claims, &scope).is_err() {
        return Err(err(StatusCode::FORBIDDEN, "insufficient scope"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spatial filter assembly
// ---------------------------------------------------------------------------

/// Resolve at most one spatial filter from the query string. `Err(Response)`
/// carries a 400 for malformed/ambiguous input (deny-by-default on bad params).
async fn build_spatial_filter(
    state: &ApiState,
    petal_id: &str,
    q: &GisNodesQuery,
) -> Result<Option<SpatialFilter>, Response> {
    let has_bbox = q.bbox.is_some();
    let has_bbox_ll = q.bbox_ll.is_some();
    let has_radius = q.radius.is_some() || q.cx.is_some() || q.cz.is_some();

    let count = [has_bbox, has_bbox_ll, has_radius]
        .iter()
        .filter(|b| **b)
        .count();
    if count == 0 {
        return Ok(None);
    }
    if count > 1 {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "specify only one of: bbox, bbox_ll, or radius",
        ));
    }

    if let Some(ref s) = q.bbox {
        let bbox = parse_bbox(s).map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
        return Ok(Some(SpatialFilter::Bbox(bbox)));
    }

    if let Some(ref s) = q.bbox_ll {
        let Some(proj) = load_petal_terrain_origin(state, petal_id).await else {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "bbox_ll requires a petal terrain origin (lat/lon); none configured",
            ));
        };
        let bbox = bbox_ll_to_local(s, &proj).map_err(|e| err(StatusCode::BAD_REQUEST, &e))?;
        return Ok(Some(SpatialFilter::Bbox(bbox)));
    }

    // Radius branch: all three of radius/cx/cz are required.
    let (Some(r), Some(cx), Some(cz)) = (q.radius, q.cx, q.cz) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "radius filter requires radius, cx, and cz",
        ));
    };
    if !(r.is_finite() && r >= 0.0) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "radius must be finite and non-negative",
        ));
    }
    Ok(Some(SpatialFilter::Radius { cx, cz, r }))
}

/// Load the petal's terrain-origin projection from its `terrain` JSON, if any.
async fn load_petal_terrain_origin(state: &ApiState, petal_id: &str) -> Option<Projection> {
    let sql = "SELECT terrain FROM petal WHERE petal_id = $pid LIMIT 1";
    let rows = run_select(state, sql, vec![("pid".to_string(), json!(petal_id))]).await?;
    let terrain = rows.first()?.get("terrain")?;
    if terrain.is_null() {
        return None;
    }
    let origin = terrain.get("origin")?;
    let lat = origin.get("origin_lat")?.as_f64()?;
    let lon = origin.get("origin_lon")?.as_f64()?;
    let ele = origin
        .get("origin_ele")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(Projection::new(lat, lon, ele))
}

// ---------------------------------------------------------------------------
// Row parsing / annotation extraction
// ---------------------------------------------------------------------------

/// Extract `[x, z]` and `elevation` from a node row (GeoJSON Point coordinates).
fn row_position(row: &Value) -> (f64, f64, f64) {
    let coords = &row["position"]["coordinates"];
    let x = coords[0].as_f64().unwrap_or(0.0);
    let z = coords[1].as_f64().unwrap_or(0.0);
    let elevation = row["elevation"].as_f64().unwrap_or(0.0);
    (x, z, elevation)
}

fn str_field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Pull the `gis.annotation.*` bundle out of a node's `properties` object.
/// Returns `None` when none of the three reserved keys are present.
fn extract_annotation(properties: &Value) -> Option<GisAnnotationDto> {
    if !properties.is_object() {
        return None;
    }
    let title = annotation_str(properties, ANNOTATION_TITLE_KEY);
    let body = annotation_str(properties, ANNOTATION_BODY_KEY);
    let color = annotation_str(properties, ANNOTATION_COLOR_KEY);
    if title.is_none() && body.is_none() && color.is_none() {
        None
    } else {
        Some(GisAnnotationDto { title, body, color })
    }
}

/// Read a reserved dotted key, tolerating both flat (`{"a.b.c": v}`, how
/// `SetNodeProperty` stores it) and nested (`{"a":{"b":{"c":v}}}`) shapes.
fn annotation_str(properties: &Value, dotted: &str) -> Option<String> {
    if let Some(v) = properties.get(dotted).and_then(Value::as_str) {
        return Some(v.to_string());
    }
    let mut cur = properties;
    for seg in dotted.split('.') {
        cur = cur.get(seg)?;
    }
    cur.as_str().map(str::to_string)
}

fn row_to_track(row: &Value) -> GisTrackDto {
    let (x, z, elevation) = row_position(row);
    let p = row.get("properties").cloned().unwrap_or(Value::Null);
    let num = |k: &str| p.get(k).and_then(Value::as_f64);
    GisTrackDto {
        node_id: str_field(row, "node_id"),
        display_name: str_field(row, "display_name"),
        position: [x, z],
        elevation,
        total_distance_m: num("total_distance_m"),
        elevation_gain_m: num("elevation_gain_m"),
        elevation_loss_m: num("elevation_loss_m"),
        duration_s: num("duration_s"),
        avg_speed_kmh: num("avg_speed_kmh"),
        max_speed_kmh: num("max_speed_kmh"),
        bounding_box: p.get("bounding_box").cloned(),
    }
}

// ---------------------------------------------------------------------------
// Data access (direct db_reader, falling back to the RawQuery gateway channel)
// ---------------------------------------------------------------------------

/// Run a read-only SELECT, preferring the direct `db_reader` and falling back
/// to the `DbCommand::RawQuery` gateway channel. Returns the first statement's
/// rows, or `None` on any transport/query error.
pub(crate) async fn run_select(
    state: &ApiState,
    sql: &str,
    vars: Vec<(String, Value)>,
) -> Option<Vec<Value>> {
    if let Some(ref db) = state.db_reader {
        let mut q = db.query(sql);
        for (k, v) in &vars {
            q = q.bind((k.clone(), v.clone()));
        }
        let mut res =
            match tokio::time::timeout(Duration::from_secs(5), async move { q.await }).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, sql, "gis direct read failed");
                    return None;
                }
                Err(_) => {
                    tracing::warn!(sql, "gis direct read timed out");
                    return None;
                }
            };
        return match res.take::<Vec<Value>>(0) {
            Ok(rows) => Some(rows),
            Err(e) => {
                tracing::warn!(error = %e, sql, "gis direct read take failed");
                None
            }
        };
    }

    // Fallback: route through the gateway channel as a RawQuery.
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let vars_map: std::collections::HashMap<String, Value> = vars.into_iter().collect();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::RawQuery {
                sql: sql.to_string(),
                vars: vars_map,
            },
            reply_tx,
        })
        .ok()?;
    match tokio::time::timeout(Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::QueryResult { data })) => Some(data),
        _ => None,
    }
}

/// Resolve a petal's full RBAC scope (direct query, else gateway channel).
async fn resolve_petal_scope(state: &ApiState, petal_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return crate::rest::direct_resolve_petal_scope(db, petal_id).await;
    }
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::ResolvePetalScope {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .ok()?;
    match tokio::time::timeout(Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Response helpers (ApiResponse envelope + real HTTP status codes)
// ---------------------------------------------------------------------------

fn ok(payload: Value) -> Response {
    (StatusCode::OK, Json(ApiResponse::success(payload))).into_response()
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(ApiResponse::<Value>::error(msg.to_string()))).into_response()
}
