//! API token persistence — stores token metadata for revocation and audit.
//!
//! Tokens themselves are JWTs signed by the node keypair. This table stores
//! the `jti` (JWT ID) so tokens can be revoked server-side.

use crate::repo::Db;

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
///
/// Returns `true` if a token with the given `jti` was found and updated,
/// `false` if no matching token exists.
pub async fn revoke_api_token(db: &Db, jti: &str) -> anyhow::Result<bool> {
    let mut result: surrealdb::IndexedResults = db
        .query("UPDATE api_token SET revoked = true WHERE jti = $jti RETURN AFTER")
        .bind(("jti", jti.to_string()))
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

/// List all active (non-revoked) tokens for a subject.
///
/// Note: expired tokens are not filtered here — callers that need expiry
/// enforcement should check `expires_at` against the current time.
pub async fn list_active_tokens(db: &Db, sub: &str) -> anyhow::Result<Vec<ApiTokenRecord>> {
    let mut result: surrealdb::IndexedResults = db
        .query("SELECT * FROM api_token WHERE sub = $sub AND revoked = false")
        .bind(("sub", sub.to_string()))
        .await?;
    let raw: Vec<serde_json::Value> = result.take(0)?;
    raw.into_iter()
        .map(|v| serde_json::from_value(v).map_err(Into::into))
        .collect()
}

/// List all active (non-revoked) tokens whose scope falls within the given
/// scope prefix (exact match OR starts with prefix + "-").
///
/// This enables admin views: a verse admin sees all tokens scoped to their
/// verse and its fractals/petals. Uses boundary-safe matching to prevent
/// `VERSE#v1` from matching `VERSE#v10`.
pub async fn list_tokens_by_scope(db: &Db, scope_prefix: &str) -> anyhow::Result<Vec<ApiTokenRecord>> {
    let prefix_dash = format!("{}-", scope_prefix);
    let mut result: surrealdb::IndexedResults = db
        .query(
            "SELECT * FROM api_token WHERE revoked = false \
             AND (scope = $prefix OR string::starts_with(scope, $prefix_dash))",
        )
        .bind(("prefix", scope_prefix.to_string()))
        .bind(("prefix_dash", prefix_dash))
        .await?;
    let raw: Vec<serde_json::Value> = result.take(0)?;
    raw.into_iter()
        .map(|v| serde_json::from_value(v).map_err(Into::into))
        .collect()
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
