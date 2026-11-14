//! REST handlers that advertise or withdraw a relay for a verse; see `fe-api/AGENTS.md` §relay-advertise.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::blob_store::BlobHash;
use ed25519_dalek::SigningKey;
use fe_identity::NodeKeypair;
use fe_runtime::dialback::{self, DialbackRecord};
use fe_runtime::relay_advertisement::{
    sign_row, AdvertisedRelayUrl, RelayAdvertisementRow, RelayUrlPolicy, MAX_ADVERT_TTL_MS,
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
    /// Required; checked by `prepare_signed_row` so a missing value is a 400, not a 422.
    #[serde(default)]
    pub expires_at_ms: Option<u64>,
    /// Third-party dial-back attestation; required for a peer-run relay.
    #[serde(default)]
    pub dialback: Option<DialbackRecord>,
    /// Marks the relay as run by this peer even when the host is not this node's own relay host.
    #[serde(default)]
    pub peer_run: bool,
}

/// Env var naming the relay this node itself uses (same name fe-sync reads).
const OWN_RELAY_ENV: &str = "FE_SYNC_RELAY";

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "ok": false, "error": message }))).into_response()
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Host of the relay this node itself uses, the only self-relay hostname the API can know.
fn own_relay_host() -> Option<String> {
    let raw = std::env::var(OWN_RELAY_ENV).ok()?;
    url::Url::parse(raw.trim()).ok()?.host_str().map(str::to_ascii_lowercase)
}

/// Why a request body cannot become a signed row; maps to a 400 answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AdvertRequestError(pub String);

/// Validated, signed v1 row bytes for `body`; the per-verse bound is enforced by the sync gate, not here.
pub(crate) fn prepare_signed_row(
    body: &PostRelayBody,
    verse_id: &str,
    signer: &NodeKeypair,
    own_host: Option<&str>,
    now_ms: u64,
) -> Result<(AdvertisedRelayUrl, Vec<u8>), AdvertRequestError> {
    let bad = |message: String| AdvertRequestError(message);
    let url = RelayUrlPolicy::default()
        .check(&body.url)
        .map_err(|error| bad(error.to_string()))?;
    if body.note.as_deref().is_some_and(|note| note.len() > MAX_NOTE_BYTES) {
        return Err(bad("note too long".into()));
    }
    let expires_at_ms = body.expires_at_ms.ok_or_else(|| bad("expires_at_ms is required".into()))?;
    if expires_at_ms <= now_ms {
        return Err(bad("expires_at_ms is in the past".into()));
    }
    if expires_at_ms - now_ms > MAX_ADVERT_TTL_MS {
        return Err(bad("expires_at_ms is too far ahead".into()));
    }
    let advertiser_did = signer.to_did_key();
    let host_is_own = own_host
        .zip(url.url().host_str())
        .is_some_and(|(own, host)| own.eq_ignore_ascii_case(host));
    let dialback = if body.peer_run || host_is_own {
        let record = body
            .dialback
            .as_ref()
            .ok_or_else(|| bad("a peer-run relay needs a dialback record".into()))?;
        dialback::accept(record, &advertiser_did, &url.to_string(), now_ms)
            .map_err(|refusal| bad(format!("dialback refused: {refusal:?}")))?;
        Some(serde_json::to_value(record).map_err(|error| bad(error.to_string()))?)
    } else {
        None
    };
    let mut row = RelayAdvertisementRow {
        v: RELAY_ADVERT_ROW_VERSION,
        url: url.to_string(),
        advertised_by: advertiser_did,
        note: body.note.clone(),
        verse_id: Some(verse_id.to_string()),
        expires_at_ms: Some(expires_at_ms),
        issued_at_ms: Some(now_ms),
        dialback,
        sig: None,
    };
    let signing_key = SigningKey::from_bytes(&signer.seed_bytes());
    sign_row(&mut row, &signing_key);
    let bytes = serde_json::to_vec(&row).map_err(|error| bad(error.to_string()))?;
    Ok((url, bytes))
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
    let Some(signer) = network_ops.and_then(|Extension(ops)| ops.advertisement_signer) else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "relay advertisement signer not configured");
    };
    let own_host = own_relay_host();
    let (url, bytes) =
        match prepare_signed_row(&body, &verse_id, &signer, own_host.as_deref(), unix_now_ms()) {
            Ok(prepared) => prepared,
            Err(AdvertRequestError(message)) => return error_response(StatusCode::BAD_REQUEST, &message),
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
    use fe_runtime::relay_advertisement::{parse_row, verify_row_at, RowParse};

    const NOW: u64 = 1_800_000_000_000;

    fn body(url: &str, expires: Option<u64>) -> PostRelayBody {
        PostRelayBody {
            url: url.to_string(),
            note: Some("eu".to_string()),
            expires_at_ms: expires,
            dialback: None,
            peer_run: false,
        }
    }

    /// One journey: refusals first, then a signed row that verifies, then the self-relay dial-back rule.
    #[test]
    fn relay_advertise_row_is_v1_signed_with_bounded_expiry_and_dialback() {
        let signer = NodeKeypair::generate();
        let prepare = |request: &PostRelayBody, own: Option<&str>| {
            prepare_signed_row(request, "verse1", &signer, own, NOW)
        };
        let good = "https://relay.example.org";
        assert!(prepare(&body(good, None), None).is_err(), "missing expiry");
        assert!(prepare(&body(good, Some(NOW - 1)), None).is_err(), "expired");
        assert!(prepare(&body(good, Some(NOW + MAX_ADVERT_TTL_MS + 1)), None).is_err(), "ttl too long");
        assert!(prepare(&body("http://relay.example.org", Some(NOW + 1000)), None).is_err(), "http");
        assert!(prepare(&body("https://10.0.0.1", Some(NOW + 1000)), None).is_err(), "private literal");

        let (_, bytes) = prepare(&body(good, Some(NOW + 1000)), None).expect("valid row");
        let RowParse::Row(row) = parse_row(&bytes) else { panic!("expected Row") };
        assert_eq!(row.verse_id.as_deref(), Some("verse1"));
        assert_eq!(row.advertised_by, signer.to_did_key());
        verify_row_at(&row, &signer.verifying_key(), NOW, Some("verse1")).expect("signature verifies");

        let mut self_relay = body(good, Some(NOW + 1000));
        assert!(prepare(&self_relay, Some("relay.example.org")).is_err(), "self relay needs dialback");
        let prober = NodeKeypair::generate();
        let prober_key = SigningKey::from_bytes(&prober.seed_bytes());
        self_relay.dialback = Some(DialbackRecord::sign(
            &prober_key,
            &signer.to_did_key(),
            "https://relay.example.org/",
            NOW - 1000,
            true,
        ));
        assert!(prepare(&self_relay, Some("relay.example.org")).is_ok(), "third-party dialback accepted");
    }
}
