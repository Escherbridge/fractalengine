//! Petal snapshot routes: Manager+ export of a full-history `.snapshot.hexon` and its import (see `AGENTS.md` §petal-snapshot).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::Multipart;
use fe_hexon::registry::FsBlobStore;
use fe_identity::api_token::ApiClaims;
use fe_policy::Action;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fe_database::handlers::petal_snapshot::EXPORT_NOT_IMPORTABLE;
use fe_runtime::petal_snapshot::PetalSnapshotRows;
use fe_runtime::scene_hook_gate::{ProvisionFailure, SceneCallerKind};
use fe_runtime::scene_write::{
    ImportJudgment, SceneWrite, SceneWriteDoor, IMPORT_JUDGMENT_BUDGET,
};
use serde_json::Value;

use crate::auth::require_role;
use crate::mcp::scene_gate::SceneDenied;
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

/// Error code when a scene hook in effect at the destination cannot run on this host (unresolved here, or another peer's).
pub const HOOK_UNJUDGEABLE: &str = "hook-unjudgeable";

/// Error code when a bound scene hook gave no usable answer (deadline, fault, decline) or the judgment budget ran out.
pub const HOOK_FAILED: &str = "hook-failed";

/// Error code when the destination's scene hook bindings could not be resolved (503).
pub const HOOKS_UNAVAILABLE: &str = "hooks-unavailable";

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

/// A hook-judgment refusal body: `{ok: false, data: null, error: code, ...detail}`.
fn hook_error(status: StatusCode, code: &str, detail: Value) -> Response {
    let mut body = serde_json::json!({ "ok": false, "data": null, "error": code });
    if let (Some(body), Value::Object(detail)) = (body.as_object_mut(), detail) {
        body.extend(detail);
    }
    (status, Json(body)).into_response()
}

/// Map a refused import judgment to its response (status table in `AGENTS.md` §petal-snapshot); `None` = admitted.
fn judgment_response(judgment: &ImportJudgment, petal_id: &str) -> Option<Response> {
    Some(match judgment {
        ImportJudgment::Admitted(_) => return None,
        ImportJudgment::Refused(refusal) => crate::rest::scene_denied_response(SceneDenied::Hook(refusal.clone())),
        ImportJudgment::Augmented { package_id } => hook_error(
            StatusCode::CONFLICT,
            HOOK_AUGMENT_UNSUPPORTED,
            serde_json::json!({ "package_id": package_id, "reason": "a bound scene hook requires changes an import cannot apply" }),
        ),
        ImportJudgment::Unanswered { package_id, failure } => hook_error(
            StatusCode::CONFLICT,
            HOOK_FAILED,
            serde_json::json!({ "package_id": package_id, "failure": failure.kind(), "reason": "a bound scene hook gave no answer; nothing was imported" }),
        ),
        ImportJudgment::OverBudget { budget_ms } => hook_error(
            StatusCode::CONFLICT,
            HOOK_FAILED,
            serde_json::json!({ "failure": "budget", "budget_ms": budget_ms, "reason": "the scene hooks did not judge every row in time; nothing was imported" }),
        ),
        ImportJudgment::Unresolved(ProvisionFailure::Unavailable(detail)) => {
            tracing::warn!(petal_id, %detail, "petal snapshot import: scene hooks unavailable");
            hook_error(
                StatusCode::SERVICE_UNAVAILABLE,
                HOOKS_UNAVAILABLE,
                serde_json::json!({ "reason": "scene hook bindings could not be resolved; retry later" }),
            )
        }
        ImportJudgment::Unresolved(ProvisionFailure::Unjudgeable { hook, reason }) => hook_error(
            StatusCode::CONFLICT,
            HOOK_UNJUDGEABLE,
            serde_json::json!({ "hook": hook, "reason": reason }),
        ),
        // The DB refuses an existing petal id the same way; answered here so no other petal's hooks see the rows.
        ImportJudgment::Unresolved(ProvisionFailure::Exists) => hook_error(
            StatusCode::CONFLICT,
            "refused",
            serde_json::json!({ "petal_id": petal_id, "reason": "petal already exists" }),
        ),
    })
}

/// A round trip with no usable reply: the error response, and whether the DB thread never got the command.
struct RoundTripFailure {
    response: Response,
    /// True only when the send failed; after a timeout or a dropped reply the command may still commit.
    unsent: bool,
}

impl RoundTripFailure {
    fn sent(response: Response) -> Self {
        Self { response, unsent: false }
    }
}

/// Send one DB command with a fresh correlation id and wait for its echoed reply.
async fn db_round_trip(
    state: &ApiState,
    build: impl FnOnce(Option<String>) -> DbCommand,
    timeout: Duration,
) -> Result<DbResult, RoundTripFailure> {
    let correlation_id = ulid::Ulid::new().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ApiCommand::DbRequest {
        cmd: build(Some(correlation_id.clone())),
        reply_tx,
    };
    if state.api_cmd_tx.send(cmd).is_err() {
        tracing::warn!("petal snapshot: API command channel closed");
        return Err(RoundTripFailure {
            response: envelope_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable"),
            unsent: true,
        });
    }
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(result)) => {
            if reply_correlation(&result).is_some_and(|echoed| echoed != correlation_id) {
                tracing::warn!("petal snapshot: reply carried a foreign correlation id");
                return Err(RoundTripFailure::sent(envelope_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "mismatched database reply",
                )));
            }
            Ok(result)
        }
        Ok(Err(_)) => Err(RoundTripFailure::sent(envelope_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable"))),
        Err(_) => Err(RoundTripFailure::sent(envelope_error(StatusCode::GATEWAY_TIMEOUT, "database timed out"))),
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

/// `?history=N` on the export route: journal facts kept per node (default 8, at most `EXPORT_HISTORY_PER_NODE_MAX`).
#[derive(Debug, Default, serde::Deserialize)]
pub struct ExportSnapshotQuery {
    pub history: Option<usize>,
}

/// GET /api/v1/petals/{petal_id}/snapshot.hexon — full-state petal snapshot (rows, op-log, blobs). Manager+.
pub async fn export_petal_snapshot(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(query): Query<ExportSnapshotQuery>,
) -> Response {
    if !is_valid_ulid(&petal_id) {
        return (StatusCode::BAD_REQUEST, "invalid petal_id").into_response();
    }
    let history_per_node = query.history;
    if history_per_node.is_some_and(|depth| depth > fe_runtime::messages::EXPORT_HISTORY_PER_NODE_MAX) {
        let max = fe_runtime::messages::EXPORT_HISTORY_PER_NODE_MAX;
        return envelope_error(StatusCode::BAD_REQUEST, format!("history must be at most {max}"));
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
        |correlation_id| DbCommand::ExportPetalSnapshot { petal_id: request_petal, correlation_id, history_per_node },
        EXPORT_TIMEOUT,
    )
    .await
    {
        Ok(DbResult::PetalSnapshotExported { rows, .. }) => rows,
        // An archive the import would refuse is the petal's size, not a server fault (AGENTS.md §petal-snapshot).
        Ok(DbResult::RequestFailed { error, .. }) if error.contains(EXPORT_NOT_IMPORTABLE) => {
            let reason = error.find(EXPORT_NOT_IMPORTABLE).map_or(error.as_str(), |at| &error[at..]);
            return envelope_error(StatusCode::PAYLOAD_TOO_LARGE, reason);
        }
        Ok(DbResult::RequestFailed { error, .. }) => {
            tracing::warn!(petal_id = %petal_id, "petal snapshot export failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot export failed");
        }
        Ok(_) => return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "unexpected database reply"),
        Err(failure) => return failure.response,
    };
    // Members plus blobs, before any blob is read: past the upload cap the import would refuse the file.
    match fe_database::handlers::petal_snapshot::importable_size(&rows, Default::default()) {
        Ok(bytes) => {
            if let Some(response) = exceeds_import_cap(bytes) {
                return response;
            }
        }
        Err(reason) => {
            return envelope_error(StatusCode::PAYLOAD_TOO_LARGE, format!("{EXPORT_NOT_IMPORTABLE}: {reason}"))
        }
    }

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
        Ok(Ok(bytes)) => match exceeds_import_cap(bytes.len() as u64) {
            Some(response) => return response,
            None => bytes,
        },
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

/// A 413 when an archive of `bytes` would exceed [`MAX_SNAPSHOT_ARCHIVE_BYTES`], the most the import accepts.
fn exceeds_import_cap(bytes: u64) -> Option<Response> {
    (bytes > MAX_SNAPSHOT_ARCHIVE_BYTES as u64).then(|| {
        envelope_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "{EXPORT_NOT_IMPORTABLE}: the archive would be {bytes} bytes, over the {MAX_SNAPSHOT_ARCHIVE_BYTES} byte import limit"
            ),
        )
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
/// Scene hooks in effect at the destination judge each node as a live create, strictly and all-or-nothing, after
/// authorization and before any blob is written; outcomes and statuses in fe-api/AGENTS.md §petal-snapshot.
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
    let (petal_row, snapshot_nodes, blob_sizes) = match tokio::task::spawn_blocking(move || {
        fe_format::snapshot::import_petal_snapshot(&parse_bytes).map(|imported| {
            // Lengths of the shipped blobs, whose names the reader re-hashed: the only trusted asset sizes.
            let sizes: std::collections::HashMap<String, u64> = imported
                .blobs
                .iter()
                .map(|(hash, body)| (hash.to_ascii_lowercase(), body.len() as u64))
                .collect();
            (imported.data.petal, imported.data.nodes, sizes)
        })
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
    // Authorization first: a caller who may not import never provisions a petal or makes an extension run.
    let Some(fractal_scope) = manager_covers(&claims, resolved) else {
        return denied();
    };
    let Some((verse_id, scope_fractal_id)) = fe_database::parse_scope(&fractal_scope)
        .ok()
        .and_then(|parts| Some((parts.verse_id, parts.fractal_id?)))
        .filter(|(_, scope_fractal_id)| scope_fractal_id == fractal_id)
    else {
        return denied();
    };

    // Scene hooks judge the new petal as it will live: bindings resolved at the destination fractal and verse, each
    // node one strict `scene.create` (+ one `scene.property` per initial key), all-or-nothing, before any blob is
    // written. Hooks never see scale, rotation, node_log, op_log, rooms, models, assets or field definitions.
    let gate_rows = PetalSnapshotRows {
        petal: petal_row.clone(),
        nodes: snapshot_nodes,
        ..Default::default()
    };
    let gate_command = DbCommand::ImportPetalSnapshot { rows: Box::new(gate_rows), correlation_id: None, actor: None };
    let write = match SceneWrite::new(snapshot_petal_id.as_str(), gate_command) {
        Ok(write) => write.by(claims.sub.clone()),
        Err(error) => return envelope_error(StatusCode::BAD_REQUEST, format!("invalid petal snapshot: {error}")),
    };
    let judgment = SceneWriteDoor::installed()
        .judge_import(SceneCallerKind::Rest, write, &verse_id, &scope_fractal_id, IMPORT_JUDGMENT_BUDGET)
        .await;
    if let Some(response) = judgment_response(&judgment, &snapshot_petal_id) {
        return response;
    }
    // The node rows the hooks judged; the restored rows must equal them or nothing is written.
    let ImportJudgment::Admitted(DbCommand::ImportPetalSnapshot { rows: judged, .. }) = judgment else {
        return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed");
    };

    // Blob writes through commit and cleanup run one import at a time (security inbox 18): a refused import can
    // then only remove blobs it introduced, and only those no asset row has come to reference since.
    let _import_section = IMPORT_SECTION.lock().await;

    // Imported assets are attributed to the importer (their uploader), so their verified bytes count against its quota.
    // Checked inside the section: an earlier import by the same caller has committed (or failed) before this read.
    let incoming: u64 = blob_sizes.values().sum();
    if incoming > 0 {
        match importer_quota_allows(&state, &claims.sub, incoming).await {
            Some(true) => {}
            Some(false) => {
                return envelope_error(StatusCode::PAYLOAD_TOO_LARGE, "upload quota exceeded by the snapshot's assets")
            }
            None => return envelope_error(StatusCode::SERVICE_UNAVAILABLE, "upload quota unavailable"),
        }
    }
    let restore_bytes = Arc::clone(&archive);
    let restored = tokio::task::spawn_blocking(move || {
        fe_hexon::snapshot::restore_snapshot_archive(&restore_bytes, &FsBlobStore::open_default())
    })
    .await;
    let (mut rows, newly_stored) = match restored {
        Ok(Ok(restored)) => (restored.rows, restored.newly_stored),
        Ok(Err(error)) => {
            return envelope_error(StatusCode::BAD_REQUEST, format!("invalid petal snapshot: {error}"))
        }
        Err(error) => {
            tracing::warn!("petal snapshot restore task failed: {error}");
            return envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed");
        }
    };
    // The restore re-parsed the same bytes; refuse if it disagrees with what was authorized and judged.
    if rows.petal != petal_row || rows.nodes != judged.nodes {
        discard_unreferenced_new_blobs(&state, newly_stored).await;
        return envelope_error(StatusCode::BAD_REQUEST, "invalid petal snapshot: inconsistent petal or node rows");
    }
    // An asset's size is its verified blob length, never the archive's number.
    for asset in &mut rows.assets {
        let size = asset
            .get("content_hash")
            .and_then(Value::as_str)
            .and_then(|hash| blob_sizes.get(&hash.to_ascii_lowercase()).copied());
        if let (Some(size), Some(object)) = (size, asset.as_object_mut()) {
            object.insert("size_bytes".into(), serde_json::json!(size));
        }
    }
    let roles_not_imported = rows.roles.len();

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
        // `persist: committed` is the local commit only: peers receive the petal after it, and nothing here certifies that.
        Ok(DbResult::PetalSnapshotImported { petal_id, .. }) => Json(ApiResponse::success(serde_json::json!({
            "petal_id": petal_id,
            "persist": "committed",
            "not_imported": { "roles": roles_not_imported },
        })))
        .into_response(),
        // A refusal, a failed restore or an unsent command wrote no rows: roll back the blobs this call wrote.
        // A timeout, dropped or unexpected reply may still commit, so those keep the blobs (an orphan, never a dangling row).
        Ok(DbResult::PetalSnapshotImportRefused { petal_id, reason, .. }) => {
            // A malformed or out-of-petal snapshot is the caller's error (400); an existing petal or a colliding
            // identity is a conflict with local state (409). No rows were written either way.
            discard_unreferenced_new_blobs(&state, newly_stored).await;
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
            discard_unreferenced_new_blobs(&state, newly_stored).await;
            // The importer's node write budget is spent: retryable, nothing written (fe-api/AGENTS.md §write-budget).
            if let Some(throttled) = crate::rest::write_budget_response(&error) {
                return throttled;
            }
            tracing::warn!(petal_id = %snapshot_petal_id, "petal snapshot import failed: {error}");
            envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "snapshot import failed")
        }
        Ok(_) => {
            tracing::warn!(petal_id = %snapshot_petal_id, "petal snapshot import: unexpected reply; its new blobs are kept");
            envelope_error(StatusCode::INTERNAL_SERVER_ERROR, "unexpected database reply")
        }
        Err(failure) => {
            if failure.unsent {
                discard_unreferenced_new_blobs(&state, newly_stored).await;
            } else {
                tracing::warn!(petal_id = %snapshot_petal_id, "petal snapshot import outcome unknown; its new blobs are kept");
            }
            failure.response
        }
    }
}

/// Serializes imports from blob write through commit and cleanup (security inbox 18).
static IMPORT_SECTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Whether `owner` may register `incoming` more asset bytes (the upload quota, summed by `owner_did`); `None` = unreadable.
async fn importer_quota_allows(state: &ApiState, owner: &str, incoming: u64) -> Option<bool> {
    let rows = crate::gis::run_select(
        state,
        "SELECT math::sum(size_bytes) AS used FROM asset WHERE owner_did = $owner GROUP ALL",
        vec![("owner".to_string(), serde_json::json!(owner))],
    )
    .await?;
    let used = rows
        .first()
        .and_then(|row| row.get("used"))
        .and_then(|used| used.as_u64().or_else(|| used.as_f64().map(|value| value as u64)))
        .unwrap_or(0);
    Some(used.saturating_add(incoming) <= state.upload_quota_bytes)
}

/// How long a refused import's new blobs are kept before the sweep: longer than a live upload's store-then-register
/// round trip (`rest::db_round_trip`, 5 s), so an upload that found one of them present has registered its row by then.
const BLOB_SWEEP_GRACE: Duration = Duration::from_secs(60);

/// Schedule the removal of the blobs this import newly wrote: after [`BLOB_SWEEP_GRACE`], under [`IMPORT_SECTION`],
/// only those no `asset` (`content_hash`) or `crate_entry` (`asset_hash`) row references then (§petal-snapshot).
async fn discard_unreferenced_new_blobs(state: &Arc<ApiState>, newly_stored: Vec<String>) {
    if newly_stored.is_empty() {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        tokio::time::sleep(BLOB_SWEEP_GRACE).await;
        // An import in flight holds the section from its first blob write to its commit, so its rows are visible here.
        let _import_section = IMPORT_SECTION.lock().await;
        sweep_unreferenced_blobs(&state, newly_stored).await;
    });
}

/// Remove the given blobs that no asset or crate entry references now; an unreadable check keeps them all.
async fn sweep_unreferenced_blobs(state: &ApiState, newly_stored: Vec<String>) {
    let hashes: Vec<String> = newly_stored.iter().map(|hash| hash.to_ascii_lowercase()).collect();
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sql in [
        "SELECT VALUE string::lowercase(content_hash) FROM asset WHERE content_hash != NONE AND string::lowercase(content_hash) IN $hashes",
        "SELECT VALUE string::lowercase(asset_hash) FROM crate_entry WHERE asset_hash != NONE AND string::lowercase(asset_hash) IN $hashes",
    ] {
        let Some(rows) = crate::gis::run_select(state, sql, vec![("hashes".to_string(), serde_json::json!(hashes))]).await
        else {
            tracing::warn!("petal snapshot blob rollback skipped: the reference check failed; its new blobs are kept");
            return;
        };
        referenced.extend(rows.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    let unreferenced: Vec<String> = hashes.into_iter().filter(|hash| !referenced.contains(hash)).collect();
    discard_new_blobs_off_thread(unreferenced).await;
}

/// Remove the given blobs, off the async thread (blocking file I/O).
async fn discard_new_blobs_off_thread(newly_stored: Vec<String>) {
    if newly_stored.is_empty() {
        return;
    }
    let outcome = tokio::task::spawn_blocking(move || {
        fe_hexon::snapshot::discard_new_blobs(&FsBlobStore::open_default(), &newly_stored);
    })
    .await;
    if let Err(error) = outcome {
        tracing::warn!("petal snapshot blob rollback task failed: {error}");
    }
}
