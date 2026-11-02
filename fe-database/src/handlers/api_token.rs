//! API token handlers — mint, revoke, and list API tokens.
//!
//! All authorization checks happen here (server-side), not in the UI.

use tracing::instrument;

use fe_runtime::messages::ApiTokenInfo;

use crate::api_token_store;
use crate::repo::Db;
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

/// A token minted by [`mint_api_token_handler`].
#[derive(Debug, Clone)]
pub(crate) struct MintedApiToken {
    pub token: String,
    pub jti: String,
    pub expires_at: String,
    /// The token's `client` claim — the requested DID, or a fresh one.
    pub client: String,
}

/// Mint a new API token and persist its metadata.
///
/// **Authorization:** The caller's role at `scope` must be Manager or Owner,
/// and the requested `max_role` must not exceed the caller's own role. The
/// `client` claim is resolved during input validation and is never consulted
/// for authorization (fe-identity/src/AGENTS.md §api-token "client claim").
#[allow(clippy::too_many_arguments)]
#[instrument(skip(db, keypair))]
pub(crate) async fn mint_api_token_handler(
    db: &Db,
    keypair: &fe_identity::NodeKeypair,
    local_did: &str,
    scope: &str,
    max_role: &str,
    ttl_hours: u32,
    label: Option<&str>,
    client_did: Option<&str>,
) -> anyhow::Result<MintedApiToken> {
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
    // Validate (or generate) the client before any read or write.
    let client = fe_identity::resolve_client_did(client_did)?;

    // --- Authorization: caller must have Manager+ role at the requested scope ---
    let authority = super::entity::prepare_scope_permission(
        db,
        scope,
        local_did,
        fe_policy::RoleLevel::Manager,
    )
    .await?;
    let caller_role = authority.role();
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

    let generation = crate::api_auth::current_generation(db).await?;
    let jti = crate::api_auth::new_jti(&generation)?;
    let ttl_secs = (ttl_hours as u64) * 3600;
    let token =
        fe_identity::mint_client_api_token(keypair, scope, max_role, ttl_secs, &jti, &client)?;

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
        created_at,
        expires_at: expires_at.clone(),
        revoked: false,
        client: Some(client.clone()),
    };
    let mut value = serde_json::to_value(&record)?;
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    authority.commit(db,&[
        crate::api_auth::GENERATION_FENCE,
        "IF array::len((SELECT jti FROM api_token WHERE sub=$mint_actor AND revoked=false AND expires_at>$mint_now)) >= 50 { THROW 'active token limit reached'; }",
        "CREATE api_token CONTENT $mint_record;",
        "IF array::len((SELECT jti FROM api_token WHERE jti=$mint_jti AND sub=$mint_actor AND scope=$mint_scope AND revoked=false)) != 1 { THROW 'token persistence unconfirmed'; }"
    ],vec![("mint_generation".into(),serde_json::json!(generation)),("mint_actor".into(),serde_json::json!(local_did)),("mint_now".into(),serde_json::json!(now)),("mint_record".into(),value),("mint_jti".into(),serde_json::json!(jti)),("mint_scope".into(),serde_json::json!(scope))]).await?;

    Ok(MintedApiToken {
        token,
        jti,
        expires_at,
        client,
    })
}

/// Revoke an API token by JTI. Only the token's owner (`local_did`) can revoke it.
/// Returns `true` if a matching token was found and revoked.
#[instrument(skip(db))]
pub(crate) async fn revoke_api_token_handler(
    db: &Db,
    local_did: &str,
    jti: &str,
) -> anyhow::Result<bool> {
    api_token_store::revoke_api_token(db, jti, local_did).await
}

/// Revoke only a token owned by the actual DB-thread identity.
pub(crate) async fn revoke_local_api_token_handler(
    db: &Db,
    local_did: &str,
    target_sub: &str,
    jti: &str,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        target_sub == local_did,
        "Token subject is not the local revocation authority"
    );
    revoke_api_token_handler(db, local_did, jti).await
}

/// List active (non-revoked, non-expired) tokens for the local node, paginated.
#[instrument(skip(db))]
pub(crate) async fn list_api_tokens_handler(
    db: &Db,
    local_did: &str,
    offset: u32,
    limit: u32,
) -> anyhow::Result<(Vec<ApiTokenInfo>, u64)> {
    let now = now_rfc3339();
    let (records, total) =
        api_token_store::list_active_tokens(db, local_did, &now, offset, limit).await?;
    Ok((records_to_info(records), total))
}

/// List active (non-revoked, non-expired) tokens whose scope falls within a
/// given scope prefix, paginated.
#[instrument(skip(db))]
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

pub(crate) fn records_to_info(records: Vec<api_token_store::ApiTokenRecord>) -> Vec<ApiTokenInfo> {
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
            sub: r.sub,
            client: r.client,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::replicated_row::apply_replicated_row_handler;

    const VERSE: &str = "verse-token";
    const SCOPE: &str = "VERSE#verse-token";

    /// A store whose verse is owned by `owner` (so `owner` mints as Owner).
    async fn owned_store(owner: &str) -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        api_token_store::apply_api_token_schema(&db)
            .await
            .expect("token schema");
        let manifest = serde_json::to_vec(&serde_json::json!({
            "verse_id": VERSE, "name": "Tokens", "created_by": owner,
            "created_at": "2026-10-25T00:00:00Z", "namespace_id": "cd".repeat(32),
            "default_access": "viewer", "ts_mode": "balanced",
            "ts_replication_factor": 2, "ts_bucket_width_ms": 3_600_000,
        }))
        .unwrap();
        apply_replicated_row_handler(&db, VERSE, "verse", VERSE, &manifest, owner, None)
            .await
            .expect("verse manifest");
        db
    }

    async fn token_rows(db: &Db) -> Vec<serde_json::Value> {
        let mut res = db.query("SELECT * FROM api_token").await.unwrap();
        res.take(0).unwrap()
    }

    /// TOKEN-CLIENT-CLAIM at the production mint door: a named client lands
    /// in the JWT, the bookkeeping row and the token list; two blank mints get
    /// two distinct, valid fresh clients; `sub` stays the node.
    #[tokio::test]
    async fn minting_records_the_requested_client_and_a_blank_request_gets_a_fresh_one() {
        let node = fe_identity::NodeKeypair::generate();
        let node_did = node.to_did_key();
        let db = owned_store(&node_did).await;
        let named = fe_identity::new_client_did();

        let minted = mint_api_token_handler(
            &db,
            &node,
            &node_did,
            SCOPE,
            "editor",
            1,
            Some("ci"),
            Some(&named),
        )
        .await
        .expect("named mint");
        assert_eq!(minted.client, named);
        let claims =
            fe_identity::verify_api_token(&minted.token, &node.verifying_key()).expect("verify");
        assert_eq!(claims.client.as_deref(), Some(named.as_str()));
        assert_eq!(claims.sub, node_did, "the client never replaces sub");

        let blank_a = mint_api_token_handler(&db, &node, &node_did, SCOPE, "viewer", 1, None, None)
            .await
            .expect("blank mint a");
        let blank_b = mint_api_token_handler(&db, &node, &node_did, SCOPE, "viewer", 1, None, None)
            .await
            .expect("blank mint b");
        for blank in [&blank_a, &blank_b] {
            assert!(fe_identity::validate_client_did(&blank.client).is_ok());
            assert_ne!(blank.client, node_did);
            assert_ne!(blank.client, named);
            let c = fe_identity::verify_api_token(&blank.token, &node.verifying_key()).unwrap();
            assert_eq!(c.client.as_deref(), Some(blank.client.as_str()));
        }
        assert_ne!(
            blank_a.client, blank_b.client,
            "each blank mint is a new client"
        );

        let (listed, total) = list_api_tokens_handler(&db, &node_did, 0, 10)
            .await
            .unwrap();
        assert_eq!(total, 3);
        let client_of = |jti: &str| {
            listed
                .iter()
                .find(|t| t.jti == jti)
                .expect("listed")
                .client
                .clone()
        };
        assert_eq!(client_of(&minted.jti), Some(named.clone()));
        assert_eq!(client_of(&blank_a.jti), Some(blank_a.client.clone()));
        assert_eq!(client_of(&blank_b.jti), Some(blank_b.client.clone()));
        let (scoped, _) = list_api_tokens_by_scope_handler(&db, SCOPE, 0, 10)
            .await
            .unwrap();
        assert!(scoped
            .iter()
            .any(|t| t.jti == minted.jti && t.client.as_deref() == Some(named.as_str())));
    }

    /// TC-5: a malformed client is refused at the mint door and leaves no
    /// bookkeeping row behind; a legacy row (no client column) still lists.
    #[tokio::test]
    async fn a_malformed_client_did_is_refused_at_mint_and_nothing_is_stored() {
        let node = fe_identity::NodeKeypair::generate();
        let node_did = node.to_did_key();
        let db = owned_store(&node_did).await;
        let x25519 = fe_identity::DeviceKeypair::generate().to_did_key();
        let with_control = format!("{}\n", fe_identity::new_client_did());
        for bad in ["not-a-did", x25519.as_str(), "", with_control.as_str()] {
            let refused =
                mint_api_token_handler(&db, &node, &node_did, SCOPE, "viewer", 1, None, Some(bad))
                    .await;
            assert!(refused.is_err(), "client {bad:?} must be refused");
        }
        assert!(token_rows(&db).await.is_empty(), "nothing is stored");

        // A row stored before the claim existed still lists, as client None.
        api_token_store::store_api_token(
            &db,
            &api_token_store::ApiTokenRecord {
                jti: "legacy".into(),
                scope: SCOPE.into(),
                max_role: "viewer".into(),
                label: None,
                sub: node_did.clone(),
                created_at: "2026-10-25T00:00:00+00:00".into(),
                expires_at: "2999-01-01T00:00:00+00:00".into(),
                revoked: false,
                client: None,
            },
        )
        .await
        .expect("a legacy row stores without a client value");
        let (listed, _) = list_api_tokens_handler(&db, &node_did, 0, 10)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].client, None);
    }
}
