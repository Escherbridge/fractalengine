//! Signed shareable query URLs (FR-2/FR-6, plan D2 + Tasks 3.1/3.2) — see `fe-api/AGENTS.md` §share.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use fe_identity::api_token::ApiClaims;
use serde::{Deserialize, Serialize};

use crate::auth::require_role;
use crate::limits;
use crate::query_guard;
use crate::server::ApiState;
use crate::types::{ApiResponse, QueryResultDto};

/// Current share-token format version.
const SHARE_TOKEN_VERSION: u8 = 1;

/// Signed claims embedded in a shareable URL token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharePayload {
    /// Format version (currently 1).
    pub v: u8,
    /// The exact SELECT to run on redemption.
    pub sql: String,
    /// Scope ceiling — the issuer's token scope at signing time; redemption
    /// can never see wider than this.
    pub scope: String,
    /// Output format: `json` | `parquet` | `csv`.
    pub fmt: String,
    /// Expiry (UNIX seconds).
    pub exp: u64,
    /// Issuing DID (audit trail).
    pub sub: String,
}

/// Why a share token failed verification (drives 401 vs 410).
#[derive(Debug, PartialEq, Eq)]
pub enum ShareVerifyError {
    Expired,
    Invalid,
}

/// Mint a compact `payload_b64.sig_b64` token signed with the server share keypair.
pub fn mint_share_token(
    signer: &fe_identity::NodeKeypair,
    payload: &SharePayload,
) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(payload)?;
    let sig = signer.sign(&bytes);
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(&bytes),
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    ))
}

/// Verify a share token (ed25519 `verify_strict`) and check expiry.
pub fn verify_share_token(
    verifying_key: &VerifyingKey,
    token: &str,
    now_secs: u64,
) -> Result<SharePayload, ShareVerifyError> {
    let (payload_b64, sig_b64) = token.split_once('.').ok_or(ShareVerifyError::Invalid)?;
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| ShareVerifyError::Invalid)?;
    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| ShareVerifyError::Invalid)?;
    let sig = Signature::from_slice(&sig_bytes).map_err(|_| ShareVerifyError::Invalid)?;
    verifying_key
        .verify_strict(&payload_bytes, &sig)
        .map_err(|_| ShareVerifyError::Invalid)?;
    let payload: SharePayload =
        serde_json::from_slice(&payload_bytes).map_err(|_| ShareVerifyError::Invalid)?;
    if payload.v != SHARE_TOKEN_VERSION {
        return Err(ShareVerifyError::Invalid);
    }
    if payload.exp <= now_secs {
        return Err(ShareVerifyError::Expired);
    }
    Ok(payload)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Structured JSON error with a real HTTP status.
fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "ok": false, "error": msg }))).into_response()
}

// ---------------------------------------------------------------------------
// Issue endpoint (authenticated)
// ---------------------------------------------------------------------------

/// Body for `POST /api/v1/query/share` (fe-ui egress-card contract).
#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    pub sql: String,
    pub format: String,
    pub ttl_secs: Option<u64>,
}

/// POST /api/v1/query/share — mint a short-TTL signed shareable URL.
///
/// The token's scope ceiling is the issuer's scope at signing time; every
/// static query guard runs at mint so a broken link fails fast.
pub async fn issue_share_url(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    Json(req): Json<ShareRequest>,
) -> Response {
    if require_role(&claims, "viewer").is_err() {
        return err(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if !matches!(req.format.as_str(), "json" | "parquet" | "csv") {
        return err(StatusCode::BAD_REQUEST, "format must be one of: json, parquet, csv");
    }
    if let Err(e) = query_guard::validate_select_sql(&req.sql) {
        return err(StatusCode::BAD_REQUEST, &e);
    }
    if req.format != "json" {
        // File exports map onto the node/EntitySnapshot shape and need a petal
        // for CRS resolution (see export.rs / AGENTS.md §share).
        let upper = req.sql.trim().to_uppercase();
        if query_guard::from_table(&upper).as_deref() != Some("NODE") {
            return err(StatusCode::BAD_REQUEST, "export queries must target the node table");
        }
        let petal_scoped = fe_database::parse_scope(&claims.scope)
            .map(|p| p.petal_id.is_some())
            .unwrap_or(false);
        if !petal_scoped {
            return err(
                StatusCode::BAD_REQUEST,
                "parquet/csv share links require a petal-scoped token",
            );
        }
    }
    let ttl = req.ttl_secs.unwrap_or(limits::SHARE_DEFAULT_TTL_SECS);
    if ttl == 0 || ttl > limits::SHARE_MAX_TTL_SECS {
        return err(
            StatusCode::BAD_REQUEST,
            &format!("ttl_secs must be 1..={} (24h max)", limits::SHARE_MAX_TTL_SECS),
        );
    }

    let payload = SharePayload {
        v: SHARE_TOKEN_VERSION,
        sql: req.sql,
        scope: claims.scope.clone(),
        fmt: req.format,
        exp: now_secs() + ttl,
        sub: claims.sub.clone(),
    };
    let token = match mint_share_token(&state.share_signer, &payload) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "share token mint failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "share token mint failed");
        }
    };
    tracing::info!(sub = %payload.sub, scope = %payload.scope, fmt = %payload.fmt, exp = payload.exp, "share URL minted");
    (
        StatusCode::OK,
        Json(ApiResponse::success(serde_json::json!({
            "url": format!("/api/v1/shared/{token}"),
            "token": token,
            "expires_at": payload.exp,
            "scope": payload.scope,
            "format": payload.fmt,
        }))),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Redemption endpoint (public — the signature IS the credential)
// ---------------------------------------------------------------------------

/// Query params accepted on redemption (`query` is fixed by the token).
#[derive(Debug, Default, Deserialize)]
pub struct RedeemParams {
    pub coords: Option<String>,
}

/// GET /api/v1/shared/:token — redeem a signed shareable URL (no session).
///
/// Runs the SAME guard pipeline as /query and the export routes, with the
/// token's scope ceiling substituted for live claims scope.
pub async fn redeem_share_url(
    State(state): State<Arc<ApiState>>,
    Path(token): Path<String>,
    Query(params): Query<RedeemParams>,
) -> Response {
    let payload = match verify_share_token(&state.share_signer.verifying_key(), &token, now_secs()) {
        Ok(p) => p,
        Err(ShareVerifyError::Expired) => {
            tracing::warn!("share redemption rejected: expired token");
            return err(StatusCode::GONE, "share link expired");
        }
        Err(ShareVerifyError::Invalid) => {
            tracing::warn!("share redemption rejected: invalid or tampered token");
            return err(StatusCode::UNAUTHORIZED, "invalid share token");
        }
    };

    // Per-token rate limit key (hash — tokens are long).
    let rate_key = format!("share:{}", token_fingerprint(&token));

    match payload.fmt.as_str() {
        "json" => {
            let guarded = match query_guard::guard_and_prepare_query(
                &state,
                &rate_key,
                limits::SHARE_REDEEM_RATE_PER_SEC,
                "10 redemptions/sec",
                &payload.scope,
                &payload.sql,
            )
            .await
            {
                Ok(g) => g,
                Err(e) => return share_query_err(e),
            };
            let Some(ref db) = state.db_reader else {
                return err(StatusCode::SERVICE_UNAVAILABLE, "shared query not available (no db_reader)");
            };
            let vars = std::collections::HashMap::new();
            match query_guard::run_guarded_query(db, &guarded, &vars, limits::QUERY_ROW_CAP).await {
                Ok(data) => {
                    if let Err(e) = query_guard::enforce_byte_ceiling(
                        &data,
                        limits::QUERY_MAX_RESPONSE_BYTES,
                        limits::QUERY_MAX_RESPONSE_LABEL,
                    ) {
                        return share_query_err(e);
                    }
                    let crs = crate::crs::scope_crs(&state, &payload.scope).await;
                    (
                        StatusCode::OK,
                        Json(ApiResponse::success(QueryResultDto { data, crs: Some(crs) })),
                    )
                        .into_response()
                }
                Err(e) => share_query_err(e),
            }
        }
        "parquet" | "csv" => {
            let petal_id = match fe_database::parse_scope(&payload.scope) {
                Ok(parts) => match parts.petal_id {
                    Some(p) => p,
                    None => {
                        return err(
                            StatusCode::BAD_REQUEST,
                            "shared parquet/csv exports require a petal-scoped token",
                        )
                    }
                },
                Err(_) => return err(StatusCode::UNAUTHORIZED, "invalid share token scope"),
            };
            let coords = match crate::export::parse_coords(params.coords.as_deref()) {
                Ok(c) => c,
                Err(resp) => return resp,
            };
            let out = match crate::export::prepare_export(
                &state,
                &rate_key,
                &petal_id,
                &payload.sql,
                coords,
            )
            .await
            {
                Ok(o) => o,
                Err(resp) => return resp,
            };
            match payload.fmt.as_str() {
                "parquet" => crate::export::parquet_response(&out, &format!("{petal_id}.parquet")),
                _ => crate::export::csv_response(&out, &format!("{petal_id}.csv")),
            }
        }
        other => err(
            StatusCode::BAD_REQUEST,
            &format!("unsupported share format '{other}'"),
        ),
    }
}

/// Map guard error strings onto share-redemption HTTP statuses.
fn share_query_err(e: String) -> Response {
    if e.starts_with("rate limit exceeded") {
        err(StatusCode::TOO_MANY_REQUESTS, &e)
    } else if e.contains("timed out") {
        err(StatusCode::GATEWAY_TIMEOUT, &e)
    } else if e.starts_with("row cap exceeded") || e.starts_with("result size exceeds") {
        err(StatusCode::PAYLOAD_TOO_LARGE, &e)
    } else {
        err(StatusCode::BAD_REQUEST, &e)
    }
}

/// Short stable fingerprint of a token for rate-limiter keying.
fn token_fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fe_identity::NodeKeypair;

    fn payload(exp: u64) -> SharePayload {
        SharePayload {
            v: SHARE_TOKEN_VERSION,
            sql: "SELECT * FROM node".into(),
            scope: "VERSE#v1-FRACTAL#f1-PETAL#p1".into(),
            fmt: "json".into(),
            exp,
            sub: "did:key:z6MkIssuer".into(),
        }
    }

    #[test]
    fn valid_token_verifies_and_preserves_ceiling() {
        let kp = NodeKeypair::generate();
        let token = mint_share_token(&kp, &payload(now_secs() + 3600)).unwrap();
        let back = verify_share_token(&kp.verifying_key(), &token, now_secs()).unwrap();
        // Ceiling = the issuer's scope at signing time, verbatim.
        assert_eq!(back.scope, "VERSE#v1-FRACTAL#f1-PETAL#p1");
        assert_eq!(back.sql, "SELECT * FROM node");
        assert_eq!(back.fmt, "json");
    }

    #[test]
    fn tampered_payload_rejected() {
        let kp = NodeKeypair::generate();
        let token = mint_share_token(&kp, &payload(now_secs() + 3600)).unwrap();
        // Swap in a widened-scope payload while keeping the original signature.
        let sig_part = token.split('.').nth(1).unwrap().to_string();
        let mut widened = payload(now_secs() + 3600);
        widened.scope = "VERSE#v1".into();
        let forged = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&widened).unwrap()),
            sig_part
        );
        assert_eq!(
            verify_share_token(&kp.verifying_key(), &forged, now_secs()).unwrap_err(),
            ShareVerifyError::Invalid
        );
    }

    #[test]
    fn tampered_signature_and_garbage_rejected() {
        let kp = NodeKeypair::generate();
        let token = mint_share_token(&kp, &payload(now_secs() + 3600)).unwrap();
        let mut chars: Vec<char> = token.chars().collect();
        let last = *chars.last().unwrap();
        *chars.last_mut().unwrap() = if last == 'A' { 'B' } else { 'A' };
        let tampered: String = chars.into_iter().collect();
        assert_eq!(
            verify_share_token(&kp.verifying_key(), &tampered, now_secs()).unwrap_err(),
            ShareVerifyError::Invalid
        );
        assert_eq!(
            verify_share_token(&kp.verifying_key(), "not-a-token", now_secs()).unwrap_err(),
            ShareVerifyError::Invalid
        );
    }

    #[test]
    fn expired_token_rejected_as_expired() {
        let kp = NodeKeypair::generate();
        let token = mint_share_token(&kp, &payload(now_secs().saturating_sub(10))).unwrap();
        assert_eq!(
            verify_share_token(&kp.verifying_key(), &token, now_secs()).unwrap_err(),
            ShareVerifyError::Expired
        );
    }

    #[test]
    fn wrong_key_rejected() {
        let kp = NodeKeypair::generate();
        let other = NodeKeypair::generate();
        let token = mint_share_token(&kp, &payload(now_secs() + 3600)).unwrap();
        assert_eq!(
            verify_share_token(&other.verifying_key(), &token, now_secs()).unwrap_err(),
            ShareVerifyError::Invalid
        );
    }
}
