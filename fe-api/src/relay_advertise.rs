//! REST handlers that advertise or withdraw a relay for a verse; see `fe-api/AGENTS.md` §relay-advertise.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::BlobHash;
use fe_runtime::relay_advertisement::{
    validate_advertised_relay_url, AdvertisedRelayUrl, RelayAdvertisementRow,
    RELAY_ADVERT_ROW_VERSION, RELAY_ADVERT_TABLE,
};
use fe_runtime::safe_id::is_safe_path_id;
use serde::Deserialize;

use crate::auth::require_role_and_scope;
use crate::network_ops::NetworkOps;
use crate::server::ApiState;

/// Longest accepted `note` and `relay_id`, in bytes.
const MAX_NOTE_BYTES: usize = 256;
const MAX_RELAY_ID_BYTES: usize = 64;

/// Request body of `POST /api/v1/verses/{verse_id}/relays`.
#[derive(Debug, Deserialize)]
pub struct PostRelayBody {
    pub url: String,
    #[serde(default)]
    pub note: Option<String>,
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "ok": false, "error": message }))).into_response()
}

/// Serialized v1 row for a validated url, authored by the node DID.
fn row_bytes(
    url: &AdvertisedRelayUrl,
    advertised_by: String,
    note: Option<String>,
) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&RelayAdvertisementRow {
        v: RELAY_ADVERT_ROW_VERSION,
        url: url.to_string(),
        advertised_by,
        note,
    })
}

/// Store `bytes` and emit one replication event for the `__relays` record; the error is the HTTP answer.
fn store_and_replicate(
    state: &ApiState,
    verse_id: &str,
    record_id: &str,
    bytes: &[u8],
) -> Result<(), Response> {
    let Some(blob_store) = state.blob_store.as_ref() else {
        return Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "blob store unavailable"));
    };
    let Some(replication_tx) = state.replication_tx.as_ref() else {
        return Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "replication unavailable"));
    };
    let content_hash: BlobHash = match blob_store.add_blob(bytes) {
        Ok(hash) => hash,
        Err(error) => {
            tracing::error!(%error, "relay advertisement blob write failed");
            return Err(error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not store relay row"));
        }
    };
    let event = fe_database::ReplicationEvent {
        verse_id: verse_id.to_string(),
        table: RELAY_ADVERT_TABLE.to_string(),
        record_id: record_id.to_string(),
        content_hash,
        petal_id: None,
    };
    // Never block an async worker on the bounded bridge: a full bridge is a retryable 503.
    match replication_tx.try_send(event) {
        Ok(()) => Ok(()),
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            tracing::warn!(verse_id, "relay advertisement replication bridge full");
            Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "replication busy, retry"))
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            tracing::warn!(verse_id, "relay advertisement replication channel closed");
            Err(error_response(StatusCode::SERVICE_UNAVAILABLE, "replication channel closed"))
        }
    }
}

/// `POST /api/v1/verses/{verse_id}/relays`: advertise a relay to the verse (manager+).
pub async fn post_relay(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    network_ops: Option<Extension<NetworkOps>>,
    Path(verse_id): Path<String>,
    Json(body): Json<PostRelayBody>,
) -> Response {
    if let Err(status) = require_role_and_scope(&claims, "manager", &format!("VERSE#{verse_id}")) {
        return status.into_response();
    }
    if !is_safe_path_id(&verse_id) {
        return error_response(StatusCode::BAD_REQUEST, "invalid verse id");
    }
    let url = match validate_advertised_relay_url(&body.url) {
        Ok(url) => url,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    if body.note.as_deref().is_some_and(|note| note.len() > MAX_NOTE_BYTES) {
        return error_response(StatusCode::BAD_REQUEST, "note too long");
    }
    let Some(signer) = network_ops.and_then(|Extension(ops)| ops.advertisement_signer) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "relay advertisement signer not configured");
    };
    let bytes = match row_bytes(&url, signer.to_did_key(), body.note) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "relay row serialization failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "could not encode relay row");
        }
    };
    let relay_id = ulid::Ulid::new().to_string();
    if let Err(response) = store_and_replicate(&state, &verse_id, &relay_id, &bytes) {
        return response;
    }
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "relay_id": relay_id, "url": url.to_string() })),
    )
        .into_response()
}

/// `DELETE /api/v1/verses/{verse_id}/relays/{relay_id}`: withdraw an advertisement with an empty-data tombstone (manager+).
pub async fn delete_relay(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    network_ops: Option<Extension<NetworkOps>>,
    Path((verse_id, relay_id)): Path<(String, String)>,
) -> Response {
    if let Err(status) = require_role_and_scope(&claims, "manager", &format!("VERSE#{verse_id}")) {
        return status.into_response();
    }
    if !is_safe_path_id(&verse_id) {
        return error_response(StatusCode::BAD_REQUEST, "invalid verse id");
    }
    let well_formed = !relay_id.is_empty()
        && relay_id.len() <= MAX_RELAY_ID_BYTES
        && relay_id.chars().all(|character| character.is_ascii_alphanumeric());
    if !well_formed {
        return error_response(StatusCode::BAD_REQUEST, "invalid relay id");
    }
    if network_ops.and_then(|Extension(ops)| ops.advertisement_signer).is_none() {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "relay advertisement signer not configured");
    }
    // An empty entry is the iroh-docs deletion marker (`row_is_tombstone`).
    if let Err(response) = store_and_replicate(&state, &verse_id, &relay_id, &[]) {
        return response;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::relay_advertisement::{parse_row, RowParse};

    #[test]
    fn relay_advertise_row_is_v1_signed_by_node_did() {
        let url = validate_advertised_relay_url("https://relay.example.org").expect("valid url");
        let bytes = row_bytes(&url, "did:key:z6MkNode".to_string(), Some("eu".to_string())).expect("encode");
        match parse_row(&bytes) {
            RowParse::Row(row) => {
                assert_eq!(row.advertised_by, "did:key:z6MkNode");
                assert_eq!(row.note.as_deref(), Some("eu"));
                assert_eq!(row.v, RELAY_ADVERT_ROW_VERSION);
            }
            other => panic!("expected Row, got {other:?}"),
        }
    }
}
