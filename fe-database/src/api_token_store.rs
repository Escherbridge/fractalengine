//! API token persistence — stores token metadata for revocation and audit.
//!
//! Tokens themselves are JWTs signed by the node keypair. This table stores
//! the `jti` (JWT ID) so tokens can be revoked server-side.

use crate::repo::Db;

/// Maximum number of active (non-revoked, non-expired) tokens a single subject may hold.
pub const MAX_ACTIVE_TOKENS_PER_SUB: usize = 50;

/// Schema definition for the api_token table.
pub async fn apply_api_token_schema(db: &Db) -> anyhow::Result<()> {
    db.query(
        "DEFINE TABLE IF NOT EXISTS api_token SCHEMAFULL;
         DEFINE FIELD IF NOT EXISTS jti ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS scope ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS max_role ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS label ON api_token TYPE option<string>;
         DEFINE FIELD IF NOT EXISTS sub ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS created_at ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS expires_at ON api_token TYPE string;
         DEFINE FIELD IF NOT EXISTS revoked ON api_token TYPE bool DEFAULT false;
         DEFINE INDEX IF NOT EXISTS idx_api_token_jti ON api_token FIELDS jti UNIQUE;",
    )
    .await?;
    Ok(())
}

/// Record stored in the api_token table.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiTokenRecord {
    pub jti: String,
    pub scope: String,
    pub max_role: String,
    pub label: Option<String>,
    pub sub: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked: bool,
}

/// Store a newly minted API token's metadata.
pub async fn store_api_token(db: &Db, record: &ApiTokenRecord) -> anyhow::Result<()> {
    db.query(
        "CREATE api_token SET
            jti = $jti,
            scope = $scope,
            max_role = $max_role,
            label = $label,
            sub = $sub,
            created_at = $created_at,
            expires_at = $expires_at,
            revoked = false",
    )
    .bind(("jti", record.jti.clone()))
    .bind(("scope", record.scope.clone()))
    .bind(("max_role", record.max_role.clone()))
    .bind(("label", record.label.clone()))
    .bind(("sub", record.sub.clone()))
    .bind(("created_at", record.created_at.clone()))
    .bind(("expires_at", record.expires_at.clone()))
    .await?;
    Ok(())
}

/// Revoke a token by setting `revoked = true`.
/// Only revokes if the token belongs to `owner_sub` (ownership check).
///
/// Returns `true` if a matching token was found and updated,
/// `false` if no matching token exists or it belongs to a different subject.
pub async fn revoke_api_token(db: &Db, jti: &str, owner_sub: &str) -> anyhow::Result<bool> {
    let mut result: surrealdb::IndexedResults = db
        .query("UPDATE api_token SET revoked = true WHERE jti = $jti AND sub = $sub RETURN AFTER")
        .bind(("jti", jti.to_string()))
        .bind(("sub", owner_sub.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(!rows.is_empty())
}

/// Check if a token has been revoked.
///
/// Returns `false` if the token is not found (treat unknown tokens as
/// non-revoked; expiry is enforced separately by the JWT verification layer).
pub async fn is_token_revoked(db: &Db, jti: &str) -> anyhow::Result<bool> {
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT revoked FROM api_token WHERE jti = $jti")
        .bind(("jti", jti.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(rows
        .first()
        .and_then(|v| v.get("revoked"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Count active (non-revoked, non-expired) tokens for a subject.
pub async fn count_active_tokens(db: &Db, sub: &str, now_rfc3339: &str) -> anyhow::Result<usize> {
    let mut result: surrealdb::IndexedResults = db
        .query(
            "SELECT count() AS c FROM api_token \
             WHERE sub = $sub AND revoked = false AND expires_at > $now \
             GROUP ALL",
        )
        .bind(("sub", sub.to_string()))
        .bind(("now", now_rfc3339.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let count = rows
        .first()
        .and_then(|v| v.get("c"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(count as usize)
}

/// List active (non-revoked, non-expired) tokens for a subject, with pagination.
pub async fn list_active_tokens(
    db: &Db,
    sub: &str,
    now_rfc3339: &str,
    offset: u32,
    limit: u32,
) -> anyhow::Result<(Vec<ApiTokenRecord>, u64)> {
    // Count total matching
    let mut count_result: surrealdb::IndexedResults = db
        .query(
            "SELECT count() AS c FROM api_token \
             WHERE sub = $sub AND revoked = false AND expires_at > $now \
             GROUP ALL",
        )
        .bind(("sub", sub.to_string()))
        .bind(("now", now_rfc3339.to_string()))
        .await?;
    let count_rows: Vec<serde_json::Value> = count_result.take(0)?;
    let total = count_rows
        .first()
        .and_then(|v| v.get("c"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Fetch page
    let mut result: surrealdb::IndexedResults = db
        .query(
            "SELECT * FROM api_token \
             WHERE sub = $sub AND revoked = false AND expires_at > $now \
             ORDER BY created_at DESC \
             LIMIT $limit START $offset",
        )
        .bind(("sub", sub.to_string()))
        .bind(("now", now_rfc3339.to_string()))
        .bind(("limit", limit as i64))
        .bind(("offset", offset as i64))
        .await?;
    let raw: Vec<serde_json::Value> = result.take(0)?;
    let records: Vec<ApiTokenRecord> = raw
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((records, total))
}

/// List active (non-revoked, non-expired) tokens whose scope falls within the
/// given scope prefix (exact match OR starts with prefix + "-"), with pagination.
pub async fn list_tokens_by_scope(
    db: &Db,
    scope_prefix: &str,
    now_rfc3339: &str,
    offset: u32,
    limit: u32,
) -> anyhow::Result<(Vec<ApiTokenRecord>, u64)> {
    let prefix_dash = format!("{}-", scope_prefix);

    // Count total matching
    let mut count_result: surrealdb::IndexedResults = db
        .query(
            "SELECT count() AS c FROM api_token WHERE revoked = false AND expires_at > $now \
             AND (scope = $prefix OR string::starts_with(scope, $prefix_dash)) \
             GROUP ALL",
        )
        .bind(("prefix", scope_prefix.to_string()))
        .bind(("prefix_dash", prefix_dash.clone()))
        .bind(("now", now_rfc3339.to_string()))
        .await?;
    let count_rows: Vec<serde_json::Value> = count_result.take(0)?;
    let total = count_rows
        .first()
        .and_then(|v| v.get("c"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    // Fetch page
    let mut result: surrealdb::IndexedResults = db
        .query(
            "SELECT * FROM api_token WHERE revoked = false AND expires_at > $now \
             AND (scope = $prefix OR string::starts_with(scope, $prefix_dash)) \
             ORDER BY created_at DESC \
             LIMIT $limit START $offset",
        )
        .bind(("prefix", scope_prefix.to_string()))
        .bind(("prefix_dash", prefix_dash))
        .bind(("now", now_rfc3339.to_string()))
        .bind(("limit", limit as i64))
        .bind(("offset", offset as i64))
        .await?;
    let raw: Vec<serde_json::Value> = result.take(0)?;
    let records: Vec<ApiTokenRecord> = raw
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((records, total))
}

#[cfg(test)]
mod tests {
    // Tests require an in-memory SurrealDB instance.
    // Deferred to integration testing phase.

    #[tokio::test]
    #[ignore = "requires in-memory SurrealDB"]
    async fn store_and_revoke_round_trip() {
        // TODO: set up in-memory DB, apply schema, store token, revoke, verify
    }
}
