//! Petal snapshot routes: Manager+ export of a full-history `.snapshot.hexon` and its import (see `AGENTS.md` §petal-snapshot).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::Multipart;
use fe_hexon::registry::FsBlobStore;
use fe_identity::api_token::ApiClaims;
use fe_policy::Action;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fe_runtime::petal_snapshot::PetalSnapshotRows;
use fe_runtime::scene_hook_gate::SceneCallerKind;
use serde_json::Value;

use crate::auth::require_role;
use crate::mcp::scene_gate::{admit, Door};
use crate::rest::NOT_FOUND_OR_DENIED;
use crate::server::ApiState;
use crate::types::{is_valid_ulid, ApiResponse};

/// Minimum token role for both snapshot routes.
const SNAPSHOT_MINIMUM_ROLE: &str = "manager";
/// Budget for the DB thread to collect a petal's rows.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);
/// Budget for the one-transaction restore of a petal.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest uploaded snapshot archive accepted, in bytes.
pub const MAX_SNAPSHOT_ARCHIVE_BYTES: usize = crate::upload::MAX_ASSET_BYTES;
/// Per-route body limit for `POST /api/v1/petals/import-snapshot` (archive + form overhead).
pub const SNAPSHOT_ROUTE_BODY_LIMIT: usize = MAX_SNAPSHOT_ARCHIVE_BYTES + 1024 * 1024;
/// Multipart field carrying the archive.
const ARCHIVE_FIELD: &str = "archive";
/// Error code when a bound scene hook admits an import only with augments, which an import cannot apply.
pub const HOOK_AUGMENT_UNSUPPORTED: &str = "hook-augment-unsupported";

/// Prefix of the DB refusal reason for a malformed or out-of-petal snapshot (`ImportError::Invalid`).
const INVALID_SNAPSHOT_REASON: &str = "invalid petal snapshot";

/// The one existence-safe denial: missing, out of scope, or under-privileged look the same.
pub(crate) fn denied() -> Response {
    (StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED).into_response()
}

/// A JSON error envelope with a real status.
fn envelope_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<Value>::error(message))).into_response()
}

/// Manager+ on `scope`: token role floor, token scope coverage, then the registry policy for `Manage`.
pub(crate) fn manager_covers(claims: &ApiClaims, scope: Option<String>) -> Option<String> {
    if require_role(claims, SNAPSHOT_MINIMUM_ROLE).is_err() {
        return None;
    }
    let scope = crate::rest::covered_scope(claims, scope)?;
    let subject = crate::auth::api_token_context(claims);
    fe_hexon::authz::authorize(&subject, &Action::Manage, &scope).ok()?;
    Some(scope)
}

/// Send one DB command with a fresh correlation id and wait for its echoed reply.
async fn db_round_trip(
    state: &ApiState,
    build: impl FnOnce(Option<String>) -> DbCommand,
    timeout: Duration,
) -> Result<DbResult, Response> {
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: build(Some(correlation_id.clone())),
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        tracing::warn!("petal snapshot: API command channel closed");
        return Err(envelope_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable"));
    }
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(result)) => {
            if reply_correlation(&result).is_some_and(|echoed| echoed != correlation_id) {
                tracing::warn!("petal snapshot: reply carried a foreign correlation id");
                return Err(envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "mismatched database reply"));
            }
            Ok(result)
        }
        Ok(Err(_)) => Err(envelope_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable")),
        Err(_) => Err(envelope_error(StatusCode::GATEWAY_TIMEOUT, "database timed out")),
    }
}

/// Correlation id echoed by the snapshot replies (and a correlated failure).
fn reply_correlation(result: &DbResult) -> Option<&str> {
    match result {
        DbResult::PetalSnapshotExported { correlation_id, .. }
        | DbResult::PetalSnapshotImported { correlation_id, .. }
        | DbResult::PetalSnapshotImportRefused { correlation_id, .. } => correlation_id.as_deref(),
        DbResult::RequestFailed { correlation_id, .. } => Some(correlation_id.as_str()),
        _ => None,
    }
}

/// GET /api/v1/petals/{petal_id}/snapshot.hexon — full-state petal snapshot (rows, op-log, blobs). Manager+.
pub async fn export_petal_snapshot(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
) -> Response {
    if !is_valid_ulid(&petal_id) {
        return (StatusCode::BAD_REQUEST, "invalid petal_id").into_response();
    }
    if require_role(&claims, SNAPSHOT_MINIMUM_ROLE).is_err() {
        return denied();
    }
    let resolved = crate::rest::resolve_petal_scope(&state, &petal_id).await;
    let Some(scope) = manager_covers(&claims, resolved) else {
        return denied();
    };

    let request_petal = petal_id.clone();
    let rows = match db_round_trip(
        &state,
        |correlation_id| DbCommand::ExportPetalSnapshot { petal_id: request_petal, correlation_id },
        EXPORT_TIMEOUT,
    )
    .await
    {
        Ok(DbResult::PetalSnapshotExported { rows, .. }) => rows,
        Ok(DbResult::RequestFailed { error, .. }) => {
            tracing::warn!(petal_id = %petal_id, "petal snapshot export failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot export failed");
        }
        Ok(_) => return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "unexpected database reply"),
        Err(response) => return response,
    };

    let source_node_id = state.share_signer.to_did_key();
    let built = tokio::task::spawn_blocking(move || {
        fe_hexon::snapshot::build_snapshot_archive(
            &rows,
            &FsBlobStore::open_default(),
            None,
            &source_node_id,
            &scope,
        )
    })
    .await;
    let archive = match built {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            tracing::warn!(petal_id = %petal_id, "petal snapshot archive build failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot export failed");
        }
        Err(error) => {
            tracing::warn!(petal_id = %petal_id, "petal snapshot archive task failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot export failed");
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-hexon+zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{petal_id}.snapshot.hexon\""),
        )
        .body(Body::from(archive))
        .unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "response build failed").into_response()
        })
}

/// Why the uploaded archive could not be read.
enum UploadError {
    Missing,
    TooLarge,
    Unreadable,
}

/// Read the `archive` field, refusing it past [`MAX_SNAPSHOT_ARCHIVE_BYTES`] without buffering the rest.
async fn read_archive_field(multipart: &mut Multipart) -> Result<Vec<u8>, UploadError> {
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => return Err(UploadError::Missing),
            Err(_) => return Err(UploadError::Unreadable),
        };
        if field.name() != Some(ARCHIVE_FIELD) {
            continue;
        }
        let mut bytes = Vec::new();
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > MAX_SNAPSHOT_ARCHIVE_BYTES {
                        return Err(UploadError::TooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => return Ok(bytes),
                Err(_) => return Err(UploadError::Unreadable),
            }
        }
    }
}

/// POST /api/v1/petals/import-snapshot — restore a snapshot under its original fractal. Manager+ there.
///
/// Multipart with one file field `archive`. Authorization runs on a parse-only pass, before any blob is written.
///
/// Scene hooks judge each node's name, position and initial properties only (not scale, rotation, node_log, op_log or
/// rooms); a hook admitting with augments refuses the import (409 `hook-augment-unsupported`), see fe-api/AGENTS.md §petal-snapshot.
pub async fn import_petal_snapshot(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    mut multipart: Multipart,
) -> Response {
    if require_role(&claims, SNAPSHOT_MINIMUM_ROLE).is_err() {
        return denied();
    }
    let archive = match read_archive_field(&mut multipart).await {
        Ok(bytes) => bytes,
        Err(UploadError::TooLarge) => {
            return envelope_error(StatusCode::PAYLOAD_TOO_LARGE, "snapshot archive too large")
        }
        Err(UploadError::Missing) => {
            return envelope_error(StatusCode::BAD_REQUEST, "missing 'archive' field in multipart upload")
        }
        Err(UploadError::Unreadable) => {
            return envelope_error(StatusCode::BAD_REQUEST, "unreadable multipart upload")
        }
    };
    let archive = Arc::new(archive);

    // Parse-only pass: read the petal row without touching the blob store.
    let parse_bytes = Arc::clone(&archive);
    let (petal_row, snapshot_nodes) = match tokio::task::spawn_blocking(move || {
        fe_format::snapshot::import_petal_snapshot(&parse_bytes)
            .map(|imported| (imported.data.petal, imported.data.nodes))
    })
    .await
    {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(error)) => {
            return envelope_error(StatusCode::BAD_REQUEST, format!("invalid petal snapshot: {error:#}"))
        }
        Err(error) => {
            tracing::warn!("petal snapshot parse task failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed");
        }
    };
    let snapshot_petal_id = PetalSnapshotRows { petal: petal_row.clone(), ..Default::default() }.petal_id();
    if !is_valid_ulid(&snapshot_petal_id) {
        return envelope_error(StatusCode::BAD_REQUEST, "invalid petal snapshot: petal_id");
    }

    // Import never creates a fractal: the snapshot's parent must exist here and grant the caller Manager+.
    let Some(fractal_id) = petal_row
        .get("fractal_id")
        .and_then(Value::as_str)
        .filter(|fractal_id| !fractal_id.is_empty())
    else {
        return denied();
    };
    let resolved = crate::rest::resolve_fractal_scope(&state, fractal_id).await;
    if manager_covers(&claims, resolved).is_none() {
        return denied();
    }

    // Scene gate: bound extensions judge the import (one create per node) before any blob is written.
    // Hooks see per node only: display name, position (x, elevation, z) and initial property keys/values; never
    // scale, rotation, node_log, op_log or rooms. Augments are refused (409 hook-augment-unsupported).
    // A call count over MAX_HOOK_CALLS_PER_WRITE is refused by the door (413), never skipped.
    let gate_rows = PetalSnapshotRows {
        petal: petal_row.clone(),
        nodes: snapshot_nodes,
        ..Default::default()
    };
    let gate_command = DbCommand::ImportPetalSnapshot { rows: Box::new(gate_rows), correlation_id: None, actor: None };
    let door = Door { claims: &claims, kind: SceneCallerKind::Rest };
    // Node ids are only known to the DB, so augments cannot be staged: refuse rather than drop them silently.
    let admission = match admit(door, &snapshot_petal_id, gate_command).await {
        Ok((_, admission)) => admission,
        Err(denied) => return crate::rest::scene_denied_response(denied),
    };
    if !admission.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "ok": false, "data": null, "error": HOOK_AUGMENT_UNSUPPORTED,
                "reason": "a bound scene hook requires changes an import cannot apply",
            })),
        )
            .into_response();
    }
    // Empty: consume the admission explicitly (nothing to stage).
    admission.stage(None);

    let restore_bytes = Arc::clone(&archive);
    let restored = tokio::task::spawn_blocking(move || {
        fe_hexon::snapshot::restore_snapshot_archive(&restore_bytes, &FsBlobStore::open_default())
    })
    .await;
    let rows = match restored {
        Ok(Ok(rows)) => rows,
        Ok(Err(error)) => {
            return envelope_error(StatusCode::BAD_REQUEST, format!("invalid petal snapshot: {error}"))
        }
        Err(error) => {
            tracing::warn!("petal snapshot restore task failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed");
        }
    };
    // The restore re-parsed the same bytes; refuse if it disagrees with what was authorized.
    if rows.petal != petal_row {
        return envelope_error(StatusCode::BAD_REQUEST, "invalid petal snapshot: inconsistent petal row");
    }

    match db_round_trip(
        &state,
        |correlation_id| DbCommand::ImportPetalSnapshot {
            rows: Box::new(rows),
            correlation_id,
            actor: Some(claims.sub.clone()),
        },
        IMPORT_TIMEOUT,
    )
    .await
    {
        Ok(DbResult::PetalSnapshotImported { petal_id, .. }) => {
            Json(ApiResponse::success(serde_json::json!({ "petal_id": petal_id }))).into_response()
        }
        // A malformed or out-of-petal snapshot is the caller's error (400); an existing petal or a colliding
        // identity is a conflict with local state (409). Nothing was written either way.
        Ok(DbResult::PetalSnapshotImportRefused { petal_id, reason, .. }) => {
            let status = if reason.starts_with(INVALID_SNAPSHOT_REASON) {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::CONFLICT
            };
            (
                status,
                Json(serde_json::json!({
                    "ok": false, "data": null, "error": "refused", "petal_id": petal_id, "reason": reason,
                })),
            )
                .into_response()
        }
        Ok(DbResult::RequestFailed { error, .. }) => {
            tracing::warn!(petal_id = %snapshot_petal_id, "petal snapshot import failed: {error}");
            envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed")
        }
        Ok(_) => envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "unexpected database reply"),
        Err(response) => response,
    }
}
