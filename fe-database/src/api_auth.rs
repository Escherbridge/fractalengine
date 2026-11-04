//! Local credential generations and bounded validity snapshots; see AGENTS.md section api-auth-lifecycle.

use serde_json::Value;

use crate::repo::Db;

pub const STATE_TABLE: &str = "api_auth_state";
pub const GENERATION_SQL: &str =
    "SELECT generation FROM api_auth_state WHERE id = api_auth_state:current TIMEOUT 5s";
pub const CREDENTIAL_SQL: &str = "INFO FOR TABLE api_token; SELECT generation, (SELECT sub, revoked FROM api_token WHERE jti = $credential_jti LIMIT 2) AS tokens FROM api_auth_state WHERE id = api_auth_state:current TIMEOUT 5s";
pub const GENERATION_FENCE: &str = "IF (SELECT VALUE generation FROM api_auth_state WHERE id = api_auth_state:current) != [$mint_generation] { THROW 'Credential generation changed'; }";
const LEGACY: &str = "legacy";

pub const TERRAIN_TABLE_GUARDS: [&str; 3] = [
    "LET $terrain_credential_inventory = INFO FOR DB",
    "IF (object::keys($terrain_credential_inventory.tables) CONTAINS 'api_token') = false { THROW 'Terrain credential metadata unavailable'; }",
    "IF (object::keys($terrain_credential_inventory.tables) CONTAINS 'api_auth_state') = false { THROW 'Terrain credential metadata unavailable'; }",
];
pub const TERRAIN_CAPTURE_SLOT: usize = 4;

/// Shared trusted read script for the direct facade and actual DB worker.
pub fn terrain_capture_sql() -> String {
    format!("BEGIN TRANSACTION; {}; SELECT generation, (SELECT sub, revoked FROM api_token WHERE jti=$credential_jti LIMIT 2) AS tokens FROM api_auth_state WHERE id=api_auth_state:current LIMIT 2 TIMEOUT 5s; COMMIT TRANSACTION;", TERRAIN_TABLE_GUARDS.join("; "))
}

/// Validate only the fixed bounded credential projection.
pub fn decode_terrain_snapshot(rows: &[Value], issuer: &str, jti: &str)
    -> anyhow::Result<fe_runtime::terrain_grant::TerrainCredentialSnapshot>
{
    anyhow::ensure!(serde_json::to_vec(rows)?.len() <= fe_runtime::terrain_grant::CAPTURE_MAX_BYTES,
        "Terrain credential metadata unavailable");
    let generation = generation_from_rows(rows)?;
    anyhow::ensure!(accepts_jti(generation, jti), "Terrain credential refused");
    let tokens = rows[0]["tokens"].as_array()
        .ok_or_else(|| anyhow::anyhow!("Terrain credential metadata unavailable"))?;
    anyhow::ensure!(tokens.len() <= 1, "Terrain credential metadata unavailable");
    for token in tokens {
        anyhow::ensure!(token.as_object().is_some_and(|object| object.len() == 2)
            && token["sub"].as_str() == Some(issuer) && token["revoked"].as_bool() == Some(false),
            "Terrain credential refused");
    }
    fe_runtime::terrain_grant::TerrainCredentialSnapshot::new(generation.into(), tokens.clone())
        .map_err(anyhow::Error::msg)
}

/// Read current durable authority without a raw-query transport escape.
pub async fn terrain_snapshot(db: &Db, issuer: &str, jti: &str)
    -> anyhow::Result<fe_runtime::terrain_grant::TerrainCredentialSnapshot>
{
    let mut response = db.query(terrain_capture_sql()).bind(("credential_jti", jti.to_owned()))
        .await?.check()?;
    let rows: Vec<Value> = response.take(TERRAIN_CAPTURE_SLOT)?;
    decode_terrain_snapshot(&rows, issuer, jti)
}

/// Retain read ownership until actual engine completion; expiry is not cancellation.
pub async fn terrain_credential_reply(db: &Db, request: fe_runtime::terrain_grant::TerrainCredentialRead)
    -> fe_runtime::messages::DbResult
{
    let snapshot = if request.active() {
        terrain_snapshot(db, request.issuer(), request.jti()).await.ok()
            .filter(|_| request.active())
    } else { None };
    fe_runtime::messages::DbResult::TerrainApiCredentialRead(
        fe_runtime::terrain_grant::TerrainCredentialReply { request, snapshot })
}

/// Fences the exact admitted credential inside its containing terrain transaction.
pub fn terrain_credential_fence(grant: &fe_runtime::terrain_grant::TerrainApiGrant,
    snapshot: &fe_runtime::terrain_grant::TerrainCredentialSnapshot)
    -> anyhow::Result<(Vec<String>, Vec<(String, Value)>)>
{
    anyhow::ensure!(snapshot.generation() == grant.generation(), "Terrain credential generation changed");
    anyhow::ensure!(snapshot.tokens() == grant.tokens(), "Terrain credential changed since admission");
    let expiry = i64::try_from(grant.expiry())?;
    let mut guards: Vec<String> = TERRAIN_TABLE_GUARDS.iter().map(|sql| (*sql).into()).collect();
    guards.extend([
        "IF (SELECT VALUE generation FROM api_auth_state WHERE id=api_auth_state:current) != [$terrain_credential_generation] { THROW 'Terrain credential generation changed'; }".into(),
        "IF (SELECT sub, revoked FROM api_token WHERE jti=$terrain_credential_jti LIMIT 2) != $terrain_credential_tokens { THROW 'Terrain credential changed'; }".into(),
        "IF time::unix() >= $terrain_credential_expiry { THROW 'Terrain credential expired'; }".into(),
    ]);
    Ok((guards, vec![
        ("terrain_credential_generation".into(), Value::from(grant.generation())),
        ("terrain_credential_jti".into(), Value::from(grant.jti())),
        ("terrain_credential_tokens".into(), serde_json::json!(snapshot.tokens())),
        ("terrain_credential_expiry".into(), Value::from(expiry)),
    ]))
}

pub async fn apply_schema(db: &Db) -> anyhow::Result<()> {
    let mut inventory = db.query("INFO FOR DB").await?.check()?;
    let metadata: Option<Value> = inventory.take(0)?;
    let tables = metadata
        .as_ref()
        .and_then(|value| value["tables"].as_object())
        .ok_or_else(|| anyhow::anyhow!("Credential schema inventory is unavailable"))?;
    let existing = tables.contains_key(STATE_TABLE);
    db.query("DEFINE TABLE IF NOT EXISTS api_auth_state SCHEMAFULL; DEFINE FIELD IF NOT EXISTS generation ON api_auth_state TYPE string;").await?.check()?;
    if !existing {
        db.query("CREATE api_auth_state:current SET generation = 'legacy'")
            .await?
            .check()?;
    }
    current_generation(db).await?;
    Ok(())
}

pub fn validate_generation(value: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        value == LEGACY || canonical_ulid(value),
        "Invalid credential generation"
    );
    Ok(())
}

fn canonical_ulid(value: &str) -> bool {
    value.len() == 26 && ulid::Ulid::from_string(value).is_ok_and(|id| id.to_string() == value)
}

pub fn generation_from_rows(rows: &[Value]) -> anyhow::Result<&str> {
    anyhow::ensure!(rows.len() == 1, "Credential generation is unavailable");
    let generation = rows[0]["generation"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Credential generation is invalid"))?;
    validate_generation(generation)?;
    Ok(generation)
}

pub async fn current_generation(db: &Db) -> anyhow::Result<String> {
    let mut response = db.query(GENERATION_SQL).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    Ok(generation_from_rows(&rows)?.to_owned())
}

pub fn new_jti(generation: &str) -> anyhow::Result<String> {
    validate_generation(generation)?;
    Ok(format!("fe1.{generation}.{}", ulid::Ulid::new()))
}

pub fn accepts_jti(generation: &str, jti: &str) -> bool {
    if validate_generation(generation).is_err()
        || jti.is_empty()
        || jti.len() > 128
        || jti.chars().any(char::is_control)
    {
        return false;
    }
    let Some(rest) = jti.strip_prefix("fe1.") else {
        return generation == LEGACY;
    };
    let Some((issued_generation, nonce)) = rest.split_once('.') else {
        return false;
    };
    validate_generation(issued_generation).is_ok()
        && canonical_ulid(nonce)
        && issued_generation == generation
}
