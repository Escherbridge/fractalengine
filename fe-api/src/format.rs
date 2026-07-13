use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum_extra::extract::Multipart;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use surrealdb::engine::local::Db;

use crate::auth::{require_role, require_scope};

use crate::types::{ApiResponse, is_valid_ulid};

/// GET /api/v1/petals/:petal_id/export — export a petal as a `.hexon` ZIP.
///
/// RBAC: Viewer+ required. Scope resolved from petal's parent chain.
pub async fn export_petal(
    State(state): State<Arc<crate::server::ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return (
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        )
            .into_response();
    }
    if !is_valid_ulid(&petal_id) {
        return (StatusCode::BAD_REQUEST, "invalid petal_id").into_response();
    }

    // Resolve petal scope for enforcement
    let Some(scope) = resolve_petal_scope_for_export(&state, &petal_id).await else {
        return (
            StatusCode::NOT_FOUND,
            "could not resolve petal scope",
        )
            .into_response();
    };
    if require_scope(&claims, &scope).is_err() {
        return (StatusCode::FORBIDDEN, "insufficient scope").into_response();
    }

    // Load nodes — use direct DB query if available
    let nodes: Vec<fe_format::ExportNode> = if let Some(ref db) = state.db_reader {
        load_export_nodes(db, &petal_id).await
    } else {
        // Fallback: channel-based query (no properties/node_log in this path)
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cmd = ApiCommand::DbRequest {
            cmd: DbCommand::LoadNodesByPetal {
                petal_id: petal_id.clone(),
            },
            reply_tx,
        };
        if state.api_cmd_tx.send(cmd).is_err() {
            return (StatusCode::INTERNAL_SERVER_ERROR, "channel closed").into_response();
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10), reply_rx).await {
            Ok(Ok(DbResult::NodesLoaded { nodes, .. })) => nodes
                .into_iter()
                .map(fe_format::ExportNode::from)
                .collect(),
            _ => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "failed to load nodes").into_response();
            }
        }
    };

    // Load field_defs for this petal's scope
    let field_defs: Vec<fe_format::FieldDef> = if let Some(ref db) = state.db_reader {
        load_field_defs(db, &petal_id).await
    } else {
        vec![]
    };

    // Build the hexon manifest
    let manifest = fe_format::HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: petal_id.clone(),
        hexon_type: fe_format::HexonType::Scene,
        publisher_did: claims.sub.clone(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: format!("Petal {petal_id}"),
        description: None,
        tags: vec![],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        source_peer_did: Some(claims.sub.clone()),
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    };

    // Build the archive
    let zip_bytes = match fe_format::HexonArchive::export_scene(
        nodes,
        field_defs,
        vec![], // assets — blob store fetch deferred to fe-hexon registry (Phase 8)
        manifest,
    ) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("export failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "export failed").into_response();
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-hexon+zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{petal_id}.hexon\""),
        )
        .body(Body::from(zip_bytes))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "response build failed").into_response()
        })
}

/// POST /api/v1/petals/:petal_id/import — import a `.hexon` ZIP into a petal.
///
/// RBAC: Editor+ required. Scope resolved from petal's parent chain.
/// Accepts `multipart/form-data` with a single file field named `archive`.
pub async fn import_petal(
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

    // Resolve petal scope for enforcement
    let Some(scope) = resolve_petal_scope_for_export(&state, &petal_id).await else {
        return axum::Json(ApiResponse::<serde_json::Value>::error(
            "could not resolve petal scope",
        ));
    };
    if require_scope(&claims, &scope).is_err() {
        return axum::Json(ApiResponse::<serde_json::Value>::error("insufficient scope"));
    }

    // Read the uploaded file
    let archive_bytes = match read_multipart_archive(&mut multipart).await {
        Some(bytes) => bytes,
        None => {
            return axum::Json(ApiResponse::<serde_json::Value>::error(
                "missing or unreadable 'archive' field in multipart upload",
            ));
        }
    };

    // Parse the archive
    let data = match fe_format::HexonArchive::import(&archive_bytes) {
        Ok(d) => d,
        Err(e) => {
            return axum::Json(ApiResponse::<serde_json::Value>::error(format!(
                "invalid .hexon archive: {e}"
            )));
        }
    };

    // Create nodes in the target petal
    let mut created = 0u32;
    let mut errors = 0u32;
    for node in &data.nodes {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let cmd = ApiCommand::DbRequest {
            cmd: DbCommand::CreateNode {
                petal_id: petal_id.clone(),
                name: node.name.clone(),
                position: node.position,
                correlation_id: None,
            },
            reply_tx,
        };
        if state.api_cmd_tx.send(cmd).is_err() {
            errors += 1;
            continue;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
            Ok(Ok(DbResult::NodeCreated { .. })) => created += 1,
            _ => errors += 1,
        }
    }

    axum::Json(ApiResponse::success(serde_json::json!({
        "schema_version": data.manifest.schema_version,
        "hexon_id": data.manifest.hexon_id,
        "hexon_type": serde_json::to_value(&data.manifest.hexon_type).unwrap_or_default(),
        "source_peer_did": data.manifest.source_peer_did,
        "nodes_imported": created,
        "errors": errors,
        "field_defs_count": data.field_defs.len(),
        "entries_count": data.entries.len(),
        "assets_count": data.assets.len(),
    })))
}

/// Read the first file field from a multipart upload.
async fn read_multipart_archive(multipart: &mut Multipart) -> Option<Vec<u8>> {
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("archive") {
            return field.bytes().await.ok().map(|b| b.to_vec());
        }
    }
    None
}

/// Load full ExportNode data including properties, rotation, scale, and node_log.
async fn load_export_nodes(
    db: &surrealdb::Surreal<Db>,
    petal_id: &str,
) -> Vec<fe_format::ExportNode> {
    // Load nodes with all fields
    let mut res = match db
        .query("SELECT * FROM node WHERE petal_id = $pid ORDER BY created_at ASC")
        .bind(("pid", petal_id.to_string()))
        .await
    {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let nodes_raw: Vec<serde_json::Value> = res.take(0).unwrap_or_default();

    let mut export_nodes = Vec::with_capacity(nodes_raw.len());
    for n in &nodes_raw {
        let node_id = n["node_id"].as_str().unwrap_or("").to_string();
        let coords = &n["position"]["coordinates"];
        let x = coords[0].as_f64().unwrap_or(0.0) as f32;
        let z = coords[1].as_f64().unwrap_or(0.0) as f32;
        let y = n["elevation"].as_f64().unwrap_or(0.0) as f32;

        let rotation = if let Some(arr) = n["rotation"].as_array() {
            [
                arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                arr.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                arr.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            ]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };

        let scale = if let Some(arr) = n["scale"].as_array() {
            [
                arr.first().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                arr.get(1).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                arr.get(2).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
            ]
        } else {
            [1.0, 1.0, 1.0]
        };

        // Load node_log entries for this node
        let node_log: Vec<serde_json::Value> = match db
            .query("SELECT * FROM node_log WHERE node_id = $nid ORDER BY hlc_timestamp ASC")
            .bind(("nid", node_id.clone()))
            .await
        {
            Ok(mut r) => r.take(0).unwrap_or_default(),
            Err(_) => vec![],
        };

        export_nodes.push(fe_format::ExportNode {
            node_id,
            petal_id: n["petal_id"].as_str().unwrap_or("").to_string(),
            name: n["display_name"].as_str().unwrap_or("").to_string(),
            position: [x, y, z],
            rotation,
            scale,
            has_asset: n["asset_id"].as_str().is_some(),
            asset_path: n["asset_path"].as_str().map(|s| s.to_string()),
            properties: n.get("properties").cloned(),
            node_log,
        });
    }

    export_nodes
}

/// Load field_defs scoped to a petal (or global scope).
async fn load_field_defs(
    db: &surrealdb::Surreal<Db>,
    petal_id: &str,
) -> Vec<fe_format::FieldDef> {
    let scope_pattern = format!("%PETAL#{}%", petal_id);
    let mut res = match db
        .query(
            "SELECT * FROM field_def WHERE scope CONTAINS $pid OR scope = 'global' ORDER BY key ASC",
        )
        .bind(("pid", scope_pattern))
        .await
    {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();

    rows.iter()
        .filter_map(|r| {
            let key = r["key"].as_str()?.to_string();
            let vt = r["value_type"].as_str().unwrap_or("string");
            let property_type = serde_json::from_value(serde_json::json!(vt)).unwrap_or(fe_format::PropertyType::String);
            Some(fe_format::FieldDef {
                key,
                property_type,
                description: r["description"].as_str().unwrap_or("").to_string(),
                required: r["required"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// Resolve a petal_id to its full scope string (reuses the rest module's helper).
async fn resolve_petal_scope_for_export(
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
