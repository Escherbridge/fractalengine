//! R5 REST surface: device format descriptors + device payload ingest
//! (CDL-SPEC10 A4) — see `fe-api/AGENTS.md` §device-ingest.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::Multipart;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{
    DbCommand, DbResult, DevicePayloadIngestRejection, DevicePayloadInput, SourceClass,
};
use fe_sdk::device_format::DeviceFormatDescriptor;
use serde::Deserialize;

use crate::auth::require_role;
use crate::limits;
use crate::rest::{covered_scope, db_round_trip, NOT_FOUND_OR_DENIED};
use crate::server::ApiState;
use crate::types::is_valid_ulid;
use crate::upload::{store_payload_files, UploadOwner, MAX_ASSET_BYTES};

/// Structured JSON error with a real HTTP status (iot.rs precedent — this
/// surface uses real statuses throughout, not the house `{ok:false}` 200).
fn err(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": msg })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// POST/GET /api/v1/verses/:verse_id/device-formats
// ---------------------------------------------------------------------------

/// POST /api/v1/verses/:verse_id/device-formats — declare a device payload
/// format. Manager+ at verse scope (R5 assumption 2: schema-defining, like
/// branch controls). Immutable per `(verse_id, format_id)`; a conflicting
/// `mime` is a 422, re-declaring the same mime is an idempotent 201.
pub async fn register_device_format(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
    Json(descriptor): Json<DeviceFormatDescriptor>,
) -> Response {
    if require_role(&claims, "manager").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&verse_id) {
        return err(StatusCode::BAD_REQUEST, "invalid verse_id");
    }
    let target = crate::rest::resolve_verse_scope(&state, &verse_id).await;
    if covered_scope(&claims, target).is_none() {
        return err(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }

    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::RegisterDeviceFormat {
        verse_id: verse_id.clone(),
        descriptor,
        declared_by: claims.sub.clone(),
        correlation_id: Some(correlation_id.clone()),
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::DeviceFormatRegistered {
            format_id,
            verse_id,
            correlation_id: Some(echoed),
        }) if echoed == correlation_id => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "ok": true, "verse_id": verse_id, "format_id": format_id })),
        )
            .into_response(),
        Ok(DbResult::DeviceFormatRejected {
            reason,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id => {
            err(StatusCode::UNPROCESSABLE_ENTITY, &reason.to_string())
        }
        Ok(other) => {
            tracing::error!("RegisterDeviceFormat reply mismatch (not ours): {other:?}");
            err(StatusCode::BAD_GATEWAY, "device format registration failed")
        }
        Err(e) => {
            tracing::warn!("RegisterDeviceFormat round trip failed: {e}");
            err(StatusCode::BAD_GATEWAY, "device format registration failed")
        }
    }
}

/// GET /api/v1/verses/:verse_id/device-formats — Viewer+, verse scope.
pub async fn list_device_formats(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(verse_id): Path<String>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&verse_id) {
        return err(StatusCode::BAD_REQUEST, "invalid verse_id");
    }
    let target = crate::rest::resolve_verse_scope(&state, &verse_id).await;
    if covered_scope(&claims, target).is_none() {
        return err(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }

    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::ListDeviceFormats {
        verse_id,
        correlation_id: Some(correlation_id.clone()),
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::DeviceFormatsListed {
            formats,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id => {
            Json(serde_json::json!({ "ok": true, "formats": formats })).into_response()
        }
        Ok(other) => {
            tracing::error!("ListDeviceFormats reply mismatch (not ours): {other:?}");
            err(StatusCode::BAD_GATEWAY, "device format list failed")
        }
        Err(e) => {
            tracing::warn!("ListDeviceFormats round trip failed: {e}");
            err(StatusCode::BAD_GATEWAY, "device format list failed")
        }
    }
}

// ---------------------------------------------------------------------------
// POST/GET /api/v1/petals/:petal_id/iot/payloads
// ---------------------------------------------------------------------------

/// The declared format's MIME every asset of this payload must carry, or
/// `None` if `format_id` names nothing declared for `verse_id`. A light
/// direct-or-channel read (the `gis::run_select` idiom), not a second
/// validation path: the DB-thread `InsertDevicePayloads` handler re-checks
/// this exact invariant and is the actual authority (fe-api/AGENTS.md
/// §device-ingest) — this lookup only lets the caller assign the right
/// `content_type` to each blob BEFORE storing it, and refuse an unknown
/// format before any byte is written. `pub(crate)` (CDL-SPEC10 A5): MCP's
/// `ingest_device_payload` reuses this exact function rather than a second
/// lookup, so REST and MCP share one core (fe-api/AGENTS.md §device-ingest).
pub(crate) async fn declared_format_mime(
    state: &ApiState,
    verse_id: &str,
    format_id: &str,
) -> Option<String> {
    let rows = crate::gis::run_select(
        state,
        "SELECT mime FROM device_format WHERE verse_id = $vid AND format_id = $fid LIMIT 1",
        vec![
            ("vid".to_string(), serde_json::json!(verse_id)),
            ("fid".to_string(), serde_json::json!(format_id)),
        ],
    )
    .await?;
    rows.first()?
        .get("mime")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// POST /api/v1/petals/:petal_id/iot/payloads — multipart ingest. Editor+,
/// petal scope. Fields: `format_id`, `node_id`, optional `recorded_at` /
/// `payload_id`, and one or more `file` parts (order preserved into
/// `asset_ids`).
///
/// 1. Resolves the declared format's MIME (404s/422s before any byte moves).
/// 2. Stores each file through [`store_payload_files`] (never runs
///    `validate_glb` unless the MIME is `model/gltf-binary`).
/// 3. Sends one `InsertDevicePayloads`, which registers each file's `asset`
///    row (descriptor MIME + this token's owner/anchor) and the payload in the
///    same DB command (QA-0161); validation failures (foreign anchor,
///    MIME mismatch per asset, bad timestamp, …) map to 422 — by this point
///    some asset rows may already exist (plan-accepted: GC reclaims orphans,
///    fe-api/AGENTS.md §asset-ingest).
/// 4. 201 `{payload_id, asset_ids}`.
pub async fn ingest_device_payload(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if require_role(&claims, "editor").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&petal_id) {
        return err(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    let Some(scope) = crate::rest::authorized_petal_scope(&state, &claims, &petal_id).await else {
        return err(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    };

    let mut format_id: Option<String> = None;
    let mut node_id: Option<String> = None;
    let mut recorded_at: Option<String> = None;
    let mut payload_id: Option<String> = None;
    let mut files: Vec<Vec<u8>> = Vec::new();

    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => match field.name() {
                Some("format_id") => format_id = field.text().await.ok(),
                Some("node_id") => node_id = field.text().await.ok(),
                Some("recorded_at") => {
                    recorded_at = field.text().await.ok().filter(|s| !s.is_empty())
                }
                Some("payload_id") => {
                    payload_id = field.text().await.ok().filter(|s| !s.is_empty())
                }
                Some("file") => {
                    if files.len() >= limits::DEVICE_PAYLOAD_MAX_ASSETS {
                        return err(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            &format!(
                                "at most {} files per device payload",
                                limits::DEVICE_PAYLOAD_MAX_ASSETS
                            ),
                        );
                    }
                    match field.bytes().await {
                        Ok(bytes) => files.push(bytes.to_vec()),
                        Err(e) => {
                            return err(
                                StatusCode::BAD_REQUEST,
                                &format!("failed to read upload: {e}"),
                            )
                        }
                    }
                }
                _ => {}
            },
            Ok(None) => break,
            Err(e) => return err(StatusCode::BAD_REQUEST, &format!("multipart error: {e}")),
        }
    }

    let Some(format_id) = format_id else {
        return err(StatusCode::BAD_REQUEST, "missing 'format_id' field");
    };
    let Some(node_id) = node_id else {
        return err(StatusCode::BAD_REQUEST, "missing 'node_id' field");
    };
    if files.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "at least one 'file' field is required",
        );
    }
    // A client retry id is refused before any byte is stored.
    if payload_id.as_deref().is_some_and(|id| !is_valid_ulid(id)) {
        return err(StatusCode::BAD_REQUEST, "invalid payload_id");
    }

    let Some(verse_id) = fe_database::parse_scope(&scope)
        .ok()
        .map(|parts| parts.verse_id)
    else {
        tracing::error!(petal_id, scope, "device payload ingest: unparseable scope");
        return err(StatusCode::BAD_GATEWAY, "could not resolve petal's verse");
    };
    let Some(mime) = declared_format_mime(&state, &verse_id, &format_id).await else {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("no device_format '{format_id}' declared for this verse"),
        );
    };

    let owner = UploadOwner::new(&claims, &petal_id);
    let files = match store_payload_files(
        &state,
        &owner,
        "device-payload-file",
        &mime,
        files,
        MAX_ASSET_BYTES,
    )
    .await
    {
        Ok(files) => files,
        Err(e) => {
            return (
                e.status(),
                Json(serde_json::json!({ "ok": false, "error": e.message() })),
            )
                .into_response()
        }
    };

    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::InsertDevicePayloads {
        petal_id: petal_id.clone(),
        verse_id: Some(verse_id),
        source_did: claims.sub.clone(),
        // REST ingest is a field-device/connector caller by construction
        // (SPEC-10 §7.1 rule 6 forbids inferring the class from `source_did`,
        // never forbids a sound fixed default for a single-class route;
        // `iot.rs` precedent). MCP's `ingest_device_payload` (CDL-SPEC10 A5)
        // is a separate caller class (`Manual`), fixed the same way for the
        // same reason — fe-api/AGENTS.md §device-ingest.
        source_class: SourceClass::Device,
        payload: DevicePayloadInput {
            node_id,
            format_id,
            asset_ids: Vec::new(),
            recorded_at,
            payload_id,
        },
        files,
        correlation_id: Some(correlation_id.clone()),
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::DevicePayloadsInserted {
            payload_id,
            asset_ids,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id => (
            StatusCode::CREATED,
            Json(
                serde_json::json!({ "ok": true, "payload_id": payload_id, "asset_ids": asset_ids }),
            ),
        )
            .into_response(),
        Ok(DbResult::DevicePayloadsRejected {
            reason,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id => {
            err(payload_rejection_status(&reason), &reason.to_string())
        }
        Ok(other) => {
            tracing::error!("InsertDevicePayloads reply mismatch (not ours): {other:?}");
            err(StatusCode::BAD_GATEWAY, "device payload ingest failed")
        }
        Err(e) => {
            tracing::warn!("InsertDevicePayloads round trip failed: {e}");
            err(StatusCode::BAD_GATEWAY, "device payload ingest failed")
        }
    }
}

/// HTTP status for a typed ingest rejection: a taken retry id is a 409 (its
/// body names nothing about the holder), a malformed one a 400, the rest 422.
pub(crate) fn payload_rejection_status(reason: &DevicePayloadIngestRejection) -> StatusCode {
    match reason {
        DevicePayloadIngestRejection::PayloadIdConflict => StatusCode::CONFLICT,
        DevicePayloadIngestRejection::InvalidPayloadId { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    }
}

/// Query params for `GET /api/v1/petals/:petal_id/iot/payloads`.
#[derive(Debug, Deserialize)]
pub struct ListDevicePayloadsQuery {
    pub node_id: Option<String>,
    pub format_id: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
}

/// GET /api/v1/petals/:petal_id/iot/payloads — Viewer+, petal scope.
pub async fn list_device_payloads(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Path(petal_id): Path<String>,
    Query(q): Query<ListDevicePayloadsQuery>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !is_valid_ulid(&petal_id) {
        return err(StatusCode::BAD_REQUEST, "invalid petal_id");
    }
    if crate::rest::authorized_petal_scope(&state, &claims, &petal_id)
        .await
        .is_none()
    {
        return err(StatusCode::NOT_FOUND, NOT_FOUND_OR_DENIED);
    }

    let correlation_id = ulid::Ulid::new().to_string();
    let cmd = DbCommand::ListDevicePayloads {
        petal_id,
        node_id: q.node_id,
        format_id: q.format_id,
        from_ms: q.from,
        to_ms: q.to,
        correlation_id: Some(correlation_id.clone()),
    };
    match db_round_trip(&state, cmd).await {
        Ok(DbResult::DevicePayloadsListed {
            payloads,
            correlation_id: Some(echoed),
            ..
        }) if echoed == correlation_id => {
            Json(serde_json::json!({ "ok": true, "payloads": payloads })).into_response()
        }
        Ok(other) => {
            tracing::error!("ListDevicePayloads reply mismatch (not ours): {other:?}");
            err(StatusCode::BAD_GATEWAY, "device payload list failed")
        }
        Err(e) => {
            tracing::warn!("ListDevicePayloads round trip failed: {e}");
            err(StatusCode::BAD_GATEWAY, "device payload list failed")
        }
    }
}
