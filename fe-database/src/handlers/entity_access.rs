//! Local target access reads and checked mutations; see handlers/AGENTS.md.

use std::collections::BTreeSet;

use fe_runtime::entity_access::{EntityAccessAction, EntityAccessSnapshot, EntityAccessTarget};
use serde_json::{json, Value};

use crate::{repo::Db, BlobStoreHandle, ReplicationSender};

const MAX_PEERS: usize = 256;
const MAX_TOKENS: usize = 50;

fn bounded_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

async fn authority(
    db: &Db,
    target: &EntityAccessTarget,
    local_did: &str,
) -> anyhow::Result<super::entity::ParentMutation> {
    let access = super::entity::prepare_parent_mutation(
        db,
        target.entity_type,
        &target.entity_id,
        local_did,
    )
    .await?;
    anyhow::ensure!(
        access.scope() == target.scope,
        "Access target moved; reload its settings"
    );
    Ok(access)
}

pub(crate) async fn load(
    db: &Db,
    target: &EntityAccessTarget,
    local_did: &str,
    peers: &[String],
) -> anyhow::Result<EntityAccessSnapshot> {
    anyhow::ensure!(
        peers.len() <= MAX_PEERS && peers.iter().all(|did| bounded_identity(did)),
        "Known peer inventory exceeds its limit"
    );
    let access = authority(db, target, local_did).await?;
    let local_role = access.role().to_string();
    let parts = crate::parse_scope(&target.scope)?;
    let mut scopes = vec![target.scope.clone()];
    while let Some(parent) = crate::parent_scope(scopes.last().unwrap()) {
        scopes.push(parent);
    }
    let rows = access.read_rows(db,
        "SELECT created_by, default_access, \
         (SELECT peer_did, scope, role FROM role WHERE scope IN $access_scopes ORDER BY scope, peer_did LIMIT 257) AS roles, \
         (SELECT jti, scope, max_role, label, sub, created_at, expires_at, revoked, client FROM api_token \
          WHERE revoked = false AND expires_at > $access_now \
          AND (scope = $access_scope OR string::starts_with(scope, $access_child)) ORDER BY jti LIMIT 51) AS tokens \
         FROM verse WHERE verse_id = $access_verse TIMEOUT 5s",
        vec![("access_scopes".into(), json!(scopes)), ("access_now".into(), json!(chrono::Utc::now().to_rfc3339())),
             ("access_scope".into(), json!(target.scope)), ("access_child".into(), json!(format!("{}-",target.scope))),
             ("access_verse".into(), json!(parts.verse_id))]).await?;
    anyhow::ensure!(
        serde_json::to_vec(&rows)?.len() <= 1024 * 1024,
        "Access metadata exceeds preview limit"
    );
    anyhow::ensure!(rows.len() == 1, "Access target is unavailable");
    let row = &rows[0];
    let owner = row["created_by"]
        .as_str()
        .filter(|v| bounded_identity(v))
        .ok_or_else(|| anyhow::anyhow!("Access owner is unavailable"))?;
    let role_rows = row["roles"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid role snapshot"))?;
    anyhow::ensure!(
        role_rows.len() <= MAX_PEERS,
        "Role inventory exceeds its limit"
    );
    let mut dids: BTreeSet<String> = peers.iter().cloned().collect();
    dids.insert(owner.to_owned());
    dids.insert(local_did.to_owned());
    for role in role_rows {
        let did = role["peer_did"]
            .as_str()
            .filter(|v| bounded_identity(v))
            .ok_or_else(|| anyhow::anyhow!("Invalid stored peer identity"))?;
        dids.insert(did.to_owned());
    }
    anyhow::ensure!(
        dids.len() <= MAX_PEERS,
        "Known peer inventory exceeds its limit"
    );
    let mut roles = Vec::with_capacity(dids.len());
    for did in dids {
        let mut nearest = Vec::new();
        for scope in &scopes {
            let matching: Vec<_> = role_rows
                .iter()
                .filter(|r| {
                    r["peer_did"].as_str() == Some(&did) && r["scope"].as_str() == Some(scope)
                })
                .collect();
            anyhow::ensure!(matching.len() <= 1, "Ambiguous stored peer role");
            if let Some(role) = matching.first() {
                nearest.push(
                    role["role"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("Invalid stored role"))?,
                );
            }
        }
        let role = crate::role_manager::resolve_role_snapshot(
            &did,
            owner,
            row["default_access"].as_str().unwrap_or("viewer"),
            nearest,
        );
        roles.push((did, role.to_string()));
    }
    let records: Vec<crate::api_token_store::ApiTokenRecord> =
        serde_json::from_value(row["tokens"].clone())?;
    let mut tokens = super::api_token::records_to_info(records);
    let tokens_truncated = tokens.len() > MAX_TOKENS;
    tokens.truncate(MAX_TOKENS);
    Ok(EntityAccessSnapshot {
        local_did: local_did.into(),
        local_role,
        roles,
        tokens,
        tokens_truncated,
        notice: None,
        invite_link: None,
        minted: None,
    })
}

pub(crate) async fn execute(
    db: &Db,
    blob_store: &BlobStoreHandle,
    repl_tx: Option<&ReplicationSender>,
    local_did: &str,
    target: &EntityAccessTarget,
    action: &EntityAccessAction,
    keypair: Option<&fe_identity::NodeKeypair>,
    mut on_revoke: impl FnMut(&str),
) -> anyhow::Result<EntityAccessSnapshot> {
    let mut notice = None;
    let mut invite_link = None;
    let mut minted = None;
    let peers = match action {
        EntityAccessAction::Load { peer_dids } => peer_dids.clone(),
        EntityAccessAction::Assign { peer_did, role } => {
            authority(db, target, local_did).await?;
            anyhow::ensure!(bounded_identity(peer_did), "Invalid peer identity");
            super::rbac::assign_role_handler(
                db,
                blob_store,
                repl_tx,
                local_did,
                peer_did,
                &target.scope,
                role,
            )
            .await?;
            notice = Some("Role statement saved. Effective roles were read back.".into());
            vec![peer_did.clone()]
        }
        EntityAccessAction::Revoke { peer_did } => {
            authority(db, target, local_did).await?;
            anyhow::ensure!(bounded_identity(peer_did), "Invalid peer identity");
            let report = super::rbac::revoke_role_handler(
                db,
                blob_store,
                repl_tx,
                local_did,
                peer_did,
                &target.scope,
            )
            .await?;
            notice = Some(if report.still_held() { "Your revoke was saved; inheritance or another grant still provides effective access." } else { "Revoke saved. Effective access was read back." }.into());
            vec![peer_did.clone()]
        }
        EntityAccessAction::GenerateInvite { role, expiry_hours } => {
            anyhow::ensure!(
                matches!(role.as_str(), "viewer" | "editor" | "manager")
                    && *expiry_hours > 0
                    && *expiry_hours <= 720,
                "Invalid invite role or expiry"
            );
            let access = authority(db, target, local_did).await?;
            access
                .read_rows(
                    db,
                    "SELECT verse_id FROM verse WHERE verse_id=$access_verse",
                    vec![(
                        "access_verse".into(),
                        json!(crate::parse_scope(&target.scope)?.verse_id),
                    )],
                )
                .await?;
            let signer = keypair.ok_or_else(|| anyhow::anyhow!("Invite signer unavailable"))?;
            invite_link = Some(
                super::rbac::generate_scoped_invite_handler(
                    signer,
                    local_did,
                    &target.scope,
                    role,
                    *expiry_hours,
                )
                .await?,
            );
            notice = Some("Scoped invite generated for this entry.".into());
            vec![]
        }
        EntityAccessAction::MintToken {
            max_role,
            ttl_hours,
            client_did,
        } => {
            authority(db, target, local_did).await?;
            let signer = keypair.ok_or_else(|| anyhow::anyhow!("Token signer unavailable"))?;
            let receipt = super::api_token::mint_api_token_handler(
                db,
                signer,
                local_did,
                &target.scope,
                max_role,
                *ttl_hours,
                None,
                client_did.as_deref(),
            )
            .await?;
            minted = Some(fe_runtime::entity_access::EntityMintedToken {
                token: receipt.token,
                client: receipt.client,
                expires_at: receipt.expires_at,
            });
            notice = Some("Token metadata saved. Copy the token before leaving API Access.".into());
            vec![]
        }
        EntityAccessAction::RevokeToken { jti, sub, scope } => {
            revoke_token(db, target, local_did, jti, sub, scope).await?;
            on_revoke(jti);
            notice = Some("Token revocation saved.".into());
            vec![]
        }
    };
    let mut snapshot = load(db, target, local_did, &peers).await?;
    snapshot.notice = notice;
    snapshot.invite_link = invite_link;
    snapshot.minted = minted;
    Ok(snapshot)
}

async fn revoke_token(
    db: &Db,
    target: &EntityAccessTarget,
    local_did: &str,
    jti: &str,
    sub: &str,
    scope: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        bounded_identity(jti) && bounded_identity(sub),
        "Invalid token identity"
    );
    anyhow::ensure!(
        scope == target.scope || scope.starts_with(&format!("{}-", target.scope)),
        "Token is outside this settings target"
    );
    crate::parse_scope(scope)?;
    let access = authority(db, target, local_did).await?;
    let exact = "SELECT jti,sub,scope,revoked FROM api_token WHERE jti=$access_jti";
    let mut response = db
        .query(exact)
        .bind(("access_jti", jti.to_owned()))
        .await?
        .check()?;
    let captured: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        captured.len() == 1
            && captured[0]["sub"].as_str() == Some(sub)
            && captured[0]["scope"].as_str() == Some(scope)
            && captured[0]["revoked"].as_bool() == Some(false),
        "Token changed or is unavailable; refresh first"
    );
    access.commit(db,&[
        "IF (SELECT jti,sub,scope,revoked FROM api_token WHERE jti=$access_jti) != $access_token { THROW 'Token changed; refresh first'; }",
        "UPDATE api_token SET revoked=true WHERE jti=$access_jti AND sub=$access_sub AND scope=$access_token_scope;",
        "IF array::len((SELECT jti FROM api_token WHERE jti=$access_jti AND sub=$access_sub AND scope=$access_token_scope AND revoked=true)) != 1 { THROW 'Token revoke was not confirmed'; }"
    ],vec![("access_jti".into(),json!(jti)),("access_sub".into(),json!(sub)),("access_token_scope".into(),json!(scope)),("access_token".into(),json!(captured))]).await
}
