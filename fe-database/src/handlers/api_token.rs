//! API token handlers — mint, revoke, and list API tokens.
//!
//! All authorization checks happen here (server-side), not in the UI.

use fe_runtime::messages::ApiTokenInfo;

use crate::api_token_store;
use crate::repo::Db;
use crate::role_manager::resolve_role;
use crate::RoleLevel;

/// Helper: current time as RFC 3339 string.
fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Mint a new API token and persist its metadata.
///
/// **Authorization:** The caller's role at `scope` must be Manager or Owner,
/// and the requested `max_role` must not exceed the caller's own role.
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

    // --- Authorization: caller must have Manager+ role at the requested scope ---
    let caller_role = resolve_role(db, local_did, scope).await?;
    if !caller_role.can_manage() {
        anyhow::bail!(
            "insufficient permissions: need Manager or Owner at scope '{}', have {:?}",
            scope,
            caller_role
        );
    }
    // Prevent escalation: requested role must not exceed caller's own role
    let requested_level = RoleLevel::from(max_role);
    if requested_level > caller_role {
        anyhow::bail!(
            "cannot mint token with role '{}' — exceeds caller's role '{}'",
            max_role,
            caller_role
        );
    }

    // --- Rate limit: enforce per-subject active token cap ---
    let now = now_rfc3339();
    let active_count = api_token_store::count_active_tokens(db, local_did, &now).await?;
    if active_count >= api_token_store::MAX_ACTIVE_TOKENS_PER_SUB {
        anyhow::bail!(
            "active token limit reached ({}/{}). Revoke unused tokens before creating new ones.",
            active_count,
            api_token_store::MAX_ACTIVE_TOKENS_PER_SUB
        );
    }

    let jti = ulid::Ulid::new().to_string();
    let ttl_secs = (ttl_hours as u64) * 3600;
    let token = fe_identity::api_token::mint_api_token(keypair, scope, max_role, ttl_secs, &jti)?;

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let created_at = chrono::DateTime::from_timestamp(secs as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let expires_at = chrono::DateTime::from_timestamp((secs + ttl_secs) as i64, 0)
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

/// Revoke an API token by JTI. Only the token's owner (`local_did`) can revoke it.
/// Returns `true` if a matching token was found and revoked.
pub(crate) async fn revoke_api_token_handler(
    db: &Db,
    local_did: &str,
    jti: &str,
) -> anyhow::Result<bool> {
    api_token_store::revoke_api_token(db, jti, local_did).await
}

/// List active (non-revoked, non-expired) tokens for the local node, paginated.
pub(crate) async fn list_api_tokens_handler(
    db: &Db,
    local_did: &str,
    offset: u32,
    limit: u32,
) -> anyhow::Result<(Vec<ApiTokenInfo>, u64)> {
    let now = now_rfc3339();
    let (records, total) = api_token_store::list_active_tokens(db, local_did, &now, offset, limit).await?;
    Ok((records_to_info(records), total))
}

/// List active (non-revoked, non-expired) tokens whose scope falls within a
/// given scope prefix, paginated.
pub(crate) async fn list_api_tokens_by_scope_handler(
    db: &Db,
    scope_prefix: &str,
    offset: u32,
    limit: u32,
) -> anyhow::Result<(Vec<ApiTokenInfo>, u64)> {
    let now = now_rfc3339();
    let (records, total) =
        api_token_store::list_tokens_by_scope(db, scope_prefix, &now, offset, limit).await?;
    Ok((records_to_info(records), total))
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
