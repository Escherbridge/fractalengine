use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum_extra::extract::Multipart;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fe_terrain::gpx::{compute_stats, parse_gpx_bytes, gpx_to_scene_commands, scene_nodes_to_gpx};
use fe_terrain::projection::Projection;

use crate::auth::{require_role, require_scope};
use crate::types::{ApiResponse, is_valid_ulid};

/// POST /api/v1/petals/:petal_id/import/gpx
///
/// Multipart GPX upload. Parses GPX, converts to nodes, creates them in the petal.
/// Returns track_count, waypoint_count, total_points, bounding_box.
///
/// RBAC: Editor+ required.
pub async fn import_gpx(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    if require_role(&claims, "editor").is_err() {
        return axum::Json(ApiResponse::<serde_json::Value>::error("insufficient permissions"));
    }
    if !is_valid_ulid(&petal_id) {
        return axum::Json(ApiResponse::<serde_json::Value>::error("invalid petal_id"));
    }

    // Resolve scope
    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return axum::Json(ApiResponse::<serde_json::Value>::error(
            "could not resolve petal scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return axum::Json(ApiResponse::<serde_json::Value>::error("insufficient scope"));
    }

    // Read the uploaded GPX file
    let gpx_bytes = match read_multipart_field(&mut multipart, "file").await {
        Some(bytes) => bytes,
        None => {
            return axum::Json(ApiResponse::<serde_json::Value>::error(
                "missing or unreadable 'file' field in multipart upload",
            ));
        }
    };

    // Parse GPX
    let data = match parse_gpx_bytes(&gpx_bytes) {
        Ok(d) => d,
        Err(e) => {
            return axum::Json(ApiResponse::<serde_json::Value>::error(format!(
                "invalid GPX: {e}"
            )));
        }
    };

    let stats = compute_stats(&data);
    let track_count = data.tracks.len();
    let waypoint_count = data.waypoints.len();
    let total_points: usize = data
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .map(|s| s.points.len())
        .sum();

    // Use bounding box center as projection origin
    let origin_lat = (stats.bounding_box.min_lat + stats.bounding_box.max_lat) / 2.0;
    let origin_lon = (stats.bounding_box.min_lon + stats.bounding_box.max_lon) / 2.0;
    let origin_ele = stats.bounding_box.min_ele.unwrap_or(0.0);
    let projection = Projection::new(origin_lat, origin_lon, origin_ele);

    let commands = gpx_to_scene_commands(&data, &petal_id, &projection);

    // Create nodes
    let mut created = 0u32;
    let mut errors = 0u32;
    for cmd in &commands {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let db_cmd = ApiCommand::DbRequest {
            cmd: DbCommand::CreateNode {
                petal_id: petal_id.clone(),
                name: cmd.name.clone(),
                position: cmd.position,
                correlation_id: None,
            },
            reply_tx,
        };
        if state.api_cmd_tx.send(db_cmd).is_err() {
            errors += 1;
            continue;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
            Ok(Ok(DbResult::NodeCreated { .. })) => created += 1,
            _ => errors += 1,
        }
    }

    axum::Json(ApiResponse::success(serde_json::json!({
        "track_count": track_count,
        "waypoint_count": waypoint_count,
        "total_points": total_points,
        "nodes_created": created,
        "errors": errors,
        "bounding_box": {
            "min_lat": stats.bounding_box.min_lat,
            "max_lat": stats.bounding_box.max_lat,
            "min_lon": stats.bounding_box.min_lon,
            "max_lon": stats.bounding_box.max_lon,
        },
        "projection": {
            "origin_lat": projection.origin_lat,
            "origin_lon": projection.origin_lon,
            "origin_ele": projection.origin_ele,
        }
    })))
}

/// GET /api/v1/petals/:petal_id/export/gpx
///
/// Export nodes with gpx_type properties back to GPX XML.
/// Content-Type: application/gpx+xml
///
/// RBAC: Viewer+ required.
pub async fn export_gpx(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return (StatusCode::FORBIDDEN, "insufficient permissions").into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return (StatusCode::BAD_REQUEST, "invalid petal_id").into_response();
    }

    let Some(scope) = resolve_petal_scope(&state, &petal_id).await else {
        return (StatusCode::NOT_FOUND, "could not resolve petal scope").into_response();
    };
    if require_scope(&claims, &scope).is_err() {
        return (StatusCode::FORBIDDEN, "insufficient scope").into_response();
    }

    // Load GPX nodes from DB
    let export_nodes = if let Some(ref db) = state.db_reader {
        load_gpx_export_nodes(db, &petal_id).await
    } else {
        // Fallback: no direct DB reader, return empty
        vec![]
    };

    // Determine projection from the first track's bounding box or use default
    let projection = determine_projection(&export_nodes);

    let gpx_xml = scene_nodes_to_gpx(&export_nodes, &projection);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gpx+xml")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{petal_id}.gpx\""),
        )
        .body(Body::from(gpx_xml))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "response build failed").into_response())
}

/// Load nodes with gpx_type property and reconstruct the hierarchy for GPX export.
async fn load_gpx_export_nodes(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    petal_id: &str,
) -> Vec<fe_terrain::ExportNode> {
    // Load all nodes with gpx_type in their properties
    let mut res = match db
        .query("SELECT * FROM node WHERE petal_id = $pid AND properties.gpx_type != NONE ORDER BY created_at ASC")
        .bind(("pid", petal_id.to_string()))
        .await
    {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();

    // Build flat list, then reconstruct tree
    let mut all_nodes: Vec<(String, Option<String>, fe_terrain::ExportNode)> = Vec::new();
    for n in &rows {
        let node_id = n["node_id"].as_str().unwrap_or("").to_string();
        let parent_id = n["parent_node_id"].as_str().map(|s| s.to_string());

        let coords = &n["position"]["coordinates"];
        let x = coords[0].as_f64().unwrap_or(0.0) as f32;
        let z = coords[1].as_f64().unwrap_or(0.0) as f32;
        let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

        all_nodes.push((
            node_id.clone(),
            parent_id,
            fe_terrain::ExportNode {
                node_id,
                name: n["display_name"].as_str().unwrap_or("").to_string(),
                position: [x, y, z],
                properties: n.get("properties").cloned(),
                children: vec![],
            },
        ));
    }

    // Build tree: attach children to parents
    // Collect children first
    let mut children_map: std::collections::HashMap<String, Vec<fe_terrain::ExportNode>> =
        std::collections::HashMap::new();
    let mut roots: Vec<fe_terrain::ExportNode> = Vec::new();

    // Two-pass: first identify all children, then build tree
    for (_node_id, parent_id, node) in &all_nodes {
        if let Some(pid) = parent_id {
            children_map
                .entry(pid.clone())
                .or_default()
                .push(node.clone());
        }
    }

    // Recursively attach children
    fn attach_children(
        node: &mut fe_terrain::ExportNode,
        children_map: &std::collections::HashMap<String, Vec<fe_terrain::ExportNode>>,
    ) {
        if let Some(children) = children_map.get(&node.node_id) {
            node.children = children.clone();
            for child in &mut node.children {
                attach_children(child, children_map);
            }
        }
    }

    for (_node_id, parent_id, node) in all_nodes {
        if parent_id.is_none() {
            let mut root = node;
            attach_children(&mut root, &children_map);
            roots.push(root);
        }
    }

    roots
}

/// Determine a projection origin from existing GPX nodes.
fn determine_projection(nodes: &[fe_terrain::ExportNode]) -> Projection {
    // Find any node with lat/lon in properties
    for node in nodes {
        if let Some(props) = &node.properties {
            if let (Some(lat), Some(lon)) = (props["lat"].as_f64(), props["lon"].as_f64()) {
                let ele = props["ele"].as_f64().unwrap_or(0.0);
                return Projection::new(lat, lon, ele);
            }
        }
        // Check children recursively
        if !node.children.is_empty() {
            let child_proj = determine_projection(&node.children);
            if child_proj.origin_lat != 0.0 || child_proj.origin_lon != 0.0 {
                return child_proj;
            }
        }
    }
    Projection::new(0.0, 0.0, 0.0)
}

async fn read_multipart_field(multipart: &mut Multipart, field_name: &str) -> Option<Vec<u8>> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some(field_name) {
            return field.bytes().await.ok().map(|b| b.to_vec());
        }
    }
    None
}

async fn resolve_petal_scope(
    state: &crate::server::ApiState,
    petal_id: &str,
) -> Option<String> {
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
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}
