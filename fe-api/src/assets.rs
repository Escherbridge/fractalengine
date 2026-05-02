//! Asset delivery endpoint -- serves content-addressed blobs by hash.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;

use crate::server::ApiState;

/// GET /api/v1/assets/:content_hash
///
/// Serves a blob from the content-addressed store with immutable caching headers.
/// Returns 404 if the hash is not found in the blob store.
pub async fn get_asset(
    State(state): State<Arc<ApiState>>,
    Path(content_hash): Path<String>,
) -> impl IntoResponse {
    let Some(ref blob_store) = state.blob_store else {
        return (StatusCode::SERVICE_UNAVAILABLE, HeaderMap::new(), Vec::new());
    };

    let hash = match fe_runtime::blob_store::hash_from_hex(&content_hash) {
        Ok(h) => h,
        Err(_) => return (StatusCode::BAD_REQUEST, HeaderMap::new(), Vec::new()),
    };

    let Some(path) = blob_store.get_blob_path(&hash) else {
        return (StatusCode::NOT_FOUND, HeaderMap::new(), Vec::new());
    };

    match std::fs::read(&path) {
        Ok(data) => {
            let mut headers = HeaderMap::new();
            headers.insert(
                "content-type",
                HeaderValue::from_static("application/octet-stream"),
            );
            headers.insert(
                "etag",
                HeaderValue::from_str(&format!("\"blake3:{content_hash}\""))
                    .unwrap_or(HeaderValue::from_static("\"unknown\"")),
            );
            headers.insert(
                "cache-control",
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
            (StatusCode::OK, headers, data)
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, HeaderMap::new(), Vec::new()),
    }
}
