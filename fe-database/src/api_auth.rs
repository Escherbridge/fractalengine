//! Local credential generations and bounded validity snapshots; see AGENTS.md section api-auth-lifecycle.

use serde_json::Value;

use crate::repo::Db;

pub const STATE_TABLE: &str = "api_auth_state";
pub const GENERATION_SQL: &str =
    "SELECT generation FROM api_auth_state WHERE id = api_auth_state:current TIMEOUT 5s";
pub const CREDENTIAL_SQL: &str = "INFO FOR TABLE api_token; SELECT generation, (SELECT sub, revoked FROM api_token WHERE jti = $credential_jti LIMIT 2) AS tokens FROM api_auth_state WHERE id = api_auth_state:current TIMEOUT 5s";
pub const GENERATION_FENCE: &str = "IF (SELECT VALUE generation FROM api_auth_state WHERE id = api_auth_state:current) != [$mint_generation] { THROW 'Credential generation changed'; }";
const LEGACY: &str = "legacy";

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
