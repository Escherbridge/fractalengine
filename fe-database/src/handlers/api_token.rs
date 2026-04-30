//! API token handlers — mint, revoke, and list API tokens.
//!
//! Extracted from the inline dispatch logic in `lib.rs` for consistency
//! with the other handler modules.

use fe_runtime::messages::ApiTokenInfo;

use crate::api_token_store;
use crate::repo::Db;

/// Mint a new API token and persist its metadata.
///
/// Returns `(jwt_string, jti, created_at, expires_at)` on success.
pub(crate) async fn mint_api_token_handler(
    db: &Db,
    keypair: &fe_identity::NodeKeypair,
    local_did: &str,
    scope: &str,
    max_role: &str,
    ttl_hours: u32,
    label: Option<&str>,
) -> anyhow::Result<(String, String, String, String)> {
    // Validate max_role is a known value (owner is excluded — tokens cannot be minted with owner role)
    if !matches!(max_role, "viewer" | "editor" | "manager") {
        anyhow::bail!("invalid max_role: must be viewer, editor, or manager");
    }
    // Validate scope is well-formed
    if crate::parse_scope(scope).is_err() {
        anyhow::bail!("invalid scope string");
    }
    // Validate TTL
    if ttl_hours > 30 * 24 {
        anyhow::bail!("TTL exceeds 30 day maximum");
    }

    let jti = ulid::Ulid::new().to_string();
    let ttl_secs = (ttl_hours as u64) * 3600;
    let token = fe_identity::api_token::mint_api_token(keypair, scope, max_role, ttl_secs, &jti)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let created_at = chrono::DateTime::from_timestamp(now as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let expires_at = chrono::DateTime::from_timestamp((now + ttl_secs) as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();

    let record = api_token_store::ApiTokenRecord {
        jti: jti.clone(),
        scope: scope.to_string(),
        max_role: max_role.to_string(),
        label: label.map(|s| s.to_string()),
        sub: local_did.to_string(),
        created_at: created_at.clone(),
        expires_at: expires_at.clone(),
        revoked: false,
    };
    api_token_store::store_api_token(db, &record).await?;

    Ok((token, jti, created_at, expires_at))
}

/// Revoke an API token by JTI. Returns `true` if a matching token was found.
pub(crate) async fn revoke_api_token_handler(db: &Db, jti: &str) -> anyhow::Result<bool> {
    api_token_store::revoke_api_token(db, jti).await
}

/// List all active (non-revoked) tokens for the local node.
pub(crate) async fn list_api_tokens_handler(
    db: &Db,
    local_did: &str,
) -> anyhow::Result<Vec<ApiTokenInfo>> {
    let records = api_token_store::list_active_tokens(db, local_did).await?;
    Ok(records_to_info(records))
}

/// List all active (non-revoked) tokens whose scope falls within a given
/// scope prefix. Used by admin dashboards: a verse admin sees all tokens
/// scoped to their verse and its fractals/petals.
pub(crate) async fn list_api_tokens_by_scope_handler(
    db: &Db,
    scope_prefix: &str,
) -> anyhow::Result<Vec<ApiTokenInfo>> {
    let records = api_token_store::list_tokens_by_scope(db, scope_prefix).await?;
    Ok(records_to_info(records))
}

fn records_to_info(records: Vec<api_token_store::ApiTokenRecord>) -> Vec<ApiTokenInfo> {
    records
        .into_iter()
        .map(|r| ApiTokenInfo {
            jti: r.jti,
            scope: r.scope,
            max_role: r.max_role,
            label: r.label,
            created_at: r.created_at,
            expires_at: r.expires_at,
            revoked: r.revoked,
        })
        .collect()
}
