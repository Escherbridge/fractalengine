//! Terrain-only verified grant capture; see src/AGENTS.md.

use std::time::Instant;
use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fe_runtime::terrain_grant::{TerrainApiGrant, TerrainCredentialRead, TerrainCredentialSnapshot, CAPTURE_BUDGET};
use crate::server::ApiState;

struct AbandonRead(TerrainCredentialRead);
impl Drop for AbandonRead {
    fn drop(&mut self) { self.0.cancel(); }
}

async fn direct_capture(db: crate::db_reader::ReadDb, request: TerrainCredentialRead)
    -> Option<TerrainCredentialSnapshot>
{
    if !request.active() { return None; }
    let mut response = db.query(fe_database::api_auth::terrain_capture_sql())
        .bind(("credential_jti", request.jti().to_owned())).await.ok()?.check().ok()?;
    let rows: Vec<serde_json::Value> = response.take(fe_database::api_auth::TERRAIN_CAPTURE_SLOT).ok()?;
    if !request.active() { return None; }
    fe_database::api_auth::decode_terrain_snapshot(&rows, request.issuer(), request.jti()).ok()
}

/// Carry the exact admitted scope through durable capture without re-admission.
pub(crate) async fn capture(state: &ApiState, claims: &ApiClaims, petal: &str, admitted: &str)
    -> Result<TerrainApiGrant, &'static str>
{
    let deadline = Instant::now() + CAPTURE_BUDGET;
    tokio::time::timeout_at(deadline.into(), async {
        let issuer = fe_identity::did_key_from_public_key(&state.verifying_key);
        if claims.sub != issuer || crate::auth::require_role(claims, "editor").is_err()
            || crate::auth::require_scope(claims, admitted).is_err()
        { return Err("Terrain credential refused"); }
        let scope = fe_database::parse_scope(admitted).map_err(|_| "Terrain credential refused")?;
        if scope.petal_id.as_deref() != Some(petal)
            || fe_database::build_scope(&scope.verse_id, scope.fractal_id.as_deref(), scope.petal_id.as_deref()) != admitted
        { return Err("Terrain credential refused"); }
        let permit = state.terrain_capture.acquire(deadline).await?;
        let request = TerrainCredentialRead::new(ulid::Ulid::new().to_string(), issuer,
            claims.jti.clone(), deadline, permit)?;
        let _abandon = AbandonRead(request.clone());
        let snapshot = if let Some(db) = &state.db_reader {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let db = db.as_ref().clone();
            let owned = request.clone();
            // The owned task retains the permit until the actual engine read ends.
            let _read = tokio::spawn(async move { let result = direct_capture(db, owned).await; let _ = tx.send(result); });
            rx.await.map_err(|_| "Terrain credential unavailable")?
        } else {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if !request.active() { return Err("Terrain credential unavailable"); }
            state.api_cmd_tx.try_send(ApiCommand::DbRequest {
                cmd: DbCommand::ReadTerrainApiCredential { request: request.clone() }, reply_tx,
            }).map_err(|_| "Terrain credential unavailable")?;
            match reply_rx.await.map_err(|_| "Terrain credential unavailable")? {
                DbResult::TerrainApiCredentialRead(reply)
                    if request.same_request(&reply.request) && reply.request.active() => reply.snapshot,
                _ => return Err("Terrain credential unavailable"),
            }
        }.ok_or("Terrain credential unavailable")?;
        crate::auth::require_terrain_credential_hints(state, claims).await
            .map_err(|_| "Terrain credential refused")?;
        if !request.active() { return Err("Terrain credential unavailable"); }
        TerrainApiGrant::verified(claims, &state.verifying_key, petal.to_owned(),
            admitted.to_owned(), snapshot)
    }).await.map_err(|_| "Terrain credential unavailable")?
}
