//! Addressed invitation storage phases; see handlers/AGENTS.md section addressed-join.

use fe_runtime::{
    entity_access::EntityAccessTarget,
    join_invite::{InviteSource, IssuedInvitation, JoinAdmission, JoinCredential},
};
use serde_json::{json, Value};

use crate::{
    join_invite::{JoinInvitation, JoinManifest, JoinRole, VerifiedJoinInvitation},
    repo::Db,
    BlobStoreHandle, ReplicationSender,
};

const VERSE_COLUMNS: &str = "verse_id,name,created_by,created_at,namespace_id,default_access,ts_mode,ts_replication_factor,ts_bucket_width_ms";
const MEMBER_COLUMNS: &str = "member_id,verse_id,peer_did,status,invited_by,invite_sig";

fn now() -> anyhow::Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("Invitation clock is unavailable"))?
        .as_secs())
}

fn decode_manifest(value: &Value) -> anyhow::Result<JoinManifest> {
    serde_json::from_value(value.clone())
        .map_err(|_| anyhow::anyhow!("Invitation source metadata is unavailable"))
}

async fn owner_source(
    db: &Db,
    target: &EntityAccessTarget,
    actor: &str,
) -> anyhow::Result<JoinManifest> {
    let authority =
        super::entity::prepare_parent_mutation(db, target.entity_type, &target.entity_id, actor)
            .await?;
    anyhow::ensure!(
        authority.scope() == target.scope && authority.role() == fe_policy::RoleLevel::Owner,
        "Fresh invitations require the current pinned Verse Owner"
    );
    let verse = crate::parse_scope(&target.scope)?.verse_id;
    let rows = authority
        .read_rows(
            db,
            &format!("SELECT {VERSE_COLUMNS} FROM verse WHERE verse_id=$join_verse LIMIT 2"),
            vec![("join_verse".into(), json!(verse))],
        )
        .await?;
    anyhow::ensure!(rows.len() == 1, "Invitation source identity is ambiguous");
    let manifest = decode_manifest(&rows[0])?;
    anyhow::ensure!(
        manifest.created_by == actor,
        "Invitation Owner identity changed"
    );
    JoinManifest::from_verse(&crate::schema::Verse {
        verse_id: manifest.verse_id.clone(),
        name: manifest.name.clone(),
        created_by: manifest.created_by.clone(),
        created_at: manifest.created_at.clone(),
        namespace_id: Some(manifest.namespace_id.clone()),
        default_access: manifest.default_access.clone(),
        ts_mode: manifest.ts_mode.clone(),
        ts_replication_factor: manifest.ts_replication_factor,
        ts_bucket_width_ms: manifest.ts_bucket_width_ms,
        view_hidden: None,
    })
}

pub(crate) async fn prepare(
    db: &Db,
    actor: &str,
    target: &EntityAccessTarget,
    recipient: &str,
    role: &str,
    expiry_hours: u32,
) -> anyhow::Result<InviteSource> {
    crate::join_invite::canonical_did(recipient)?;
    JoinRole::parse(role)?;
    anyhow::ensure!(
        recipient != actor && (1..=720).contains(&expiry_hours),
        "Invalid invitation recipient or expiry"
    );
    let manifest = owner_source(db, target, actor).await?;
    Ok(InviteSource {
        target: target.clone(),
        recipient_did: recipient.into(),
        role: role.into(),
        expiry_hours,
        manifest: serde_json::to_value(manifest)?,
        prepared_at: now()?,
        nonce: ulid::Ulid::new().to_string(),
    })
}

pub(crate) async fn issue(
    db: &Db,
    actor: &str,
    signer: Option<&fe_identity::NodeKeypair>,
    source: &InviteSource,
    ticket: &JoinCredential,
    blob_store: &BlobStoreHandle,
    replication: Option<&ReplicationSender>,
) -> anyhow::Result<IssuedInvitation> {
    let clock = now()?;
    anyhow::ensure!(
        source.prepared_at <= clock && clock - source.prepared_at <= 60,
        "Invitation preparation expired; refresh Access"
    );
    anyhow::ensure!(
        ulid::Ulid::from_string(&source.nonce).is_ok_and(|id| id.to_string() == source.nonce),
        "Invalid invitation preparation identity"
    );
    let signer = signer
        .filter(|signer| signer.to_did_key() == actor)
        .ok_or_else(|| anyhow::anyhow!("Invitation signer is unavailable"))?;
    let captured = decode_manifest(&source.manifest)?;
    let current = owner_source(db, &source.target, actor).await?;
    anyhow::ensure!(
        current == captured,
        "Invitation source changed; refresh Access"
    );
    crate::join_invite::canonical_did(&source.recipient_did)?;
    let role = JoinRole::parse(&source.role)?;
    anyhow::ensure!(
        (1..=720).contains(&source.expiry_hours),
        "Invalid invitation expiry"
    );
    anyhow::ensure!(
        ticket.as_str().len() <= crate::join_invite::MAX_TICKET_BYTES,
        "Invitation ticket is too large"
    );
    JoinInvitation::issue_owner(
        signer,
        source.recipient_did.clone(),
        source.target.scope.clone(),
        role,
        1,
        captured.clone(),
        ticket.as_str().into(),
        clock,
        u64::from(source.expiry_hours) * 3600,
    )?;
    let deadline = source
        .prepared_at
        .checked_add(60)
        .ok_or_else(|| anyhow::anyhow!("Invalid invitation preparation time"))?;
    let grant_hlc = crate::role_manager::assign_role_for_invite(db,actor,&source.recipient_did,&source.target.scope,
        role.as_str(),&captured,deadline,replication,blob_store).await
        .map_err(|_| anyhow::anyhow!("Invitation issuance is unconfirmed; a checked role grant may already be stored. Refresh Access before another change."))?;
    (|| -> anyhow::Result<IssuedInvitation> {
        let invitation = JoinInvitation::issue_owner(
            signer,
            source.recipient_did.clone(),
            source.target.scope.clone(),
            role,
            grant_hlc,
            captured,
            ticket.as_str().into(),
            now()?,
            u64::from(source.expiry_hours) * 3600,
        )?;
        let credential = JoinCredential::new(invitation.to_link()?)?;
        let expires_at =
            JoinInvitation::parse_owner(credential.as_str(), &source.recipient_did, now()?)?
                .expires_at();
        Ok(IssuedInvitation {
            source: source.clone(),
            credential,
            expires_at,
        })
    })()
    .map_err(|_| anyhow::anyhow!("Invitation issuance is unconfirmed; the checked role grant may already be stored. Refresh Access before another change; do not automatically replay issuance."))
}

fn same_identity(row: &Value, manifest: &JoinManifest) -> bool {
    row["verse_id"].as_str() == Some(manifest.verse_id.as_str())
        && row["created_by"].as_str() == Some(manifest.created_by.as_str())
        && row["created_at"].as_str() == Some(manifest.created_at.as_str())
        && row["namespace_id"].as_str() == Some(manifest.namespace_id.as_str())
}

fn admission(invitation: &VerifiedJoinInvitation, name: String) -> JoinAdmission {
    JoinAdmission {
        nonce: invitation.nonce().into(),
        verse_id: invitation.manifest().verse_id.clone(),
        verse_name: name,
        scope: invitation.scope().into(),
        role: invitation.role().as_str().into(),
    }
}

pub(crate) async fn admit(
    db: &Db,
    actor: &str,
    credential: &JoinCredential,
) -> anyhow::Result<JoinAdmission> {
    let invitation = JoinInvitation::parse_owner(credential.as_str(), actor, now()?)?;
    admit_verified(db, actor, &invitation).await
}

#[allow(dead_code)]
pub(crate) async fn admit_v2(
    db: &Db,
    actor: &str,
    credential: &JoinCredential,
) -> anyhow::Result<JoinAdmission> {
    let invitation = JoinInvitation::parse(credential.as_str(), actor, now()?)?;
    anyhow::ensure!(invitation.version() == 2, "Expected an addressed-v2 invitation");
    admit_verified(db, actor, &invitation).await
}

async fn admit_verified(
    db: &Db,
    actor: &str,
    invitation: &VerifiedJoinInvitation,
) -> anyhow::Result<JoinAdmission> {
    let manifest = invitation.manifest();
    let verse_select =
        format!("SELECT {VERSE_COLUMNS} FROM verse WHERE verse_id=$join_verse LIMIT 2");
    let member_select =
        format!("SELECT {MEMBER_COLUMNS} FROM verse_member WHERE member_id=$join_nonce LIMIT 2");
    let mut response = db
        .query(format!("{verse_select};{member_select}"))
        .bind(("join_verse", manifest.verse_id.clone()))
        .bind(("join_nonce", invitation.nonce().to_owned()))
        .await?
        .check()?;
    let verses: Vec<Value> = response.take(0)?;
    let members: Vec<Value> = response.take(1)?;
    anyhow::ensure!(
        verses.len() <= 1 && members.len() <= 1,
        "Ambiguous invitation identity"
    );
    anyhow::ensure!(
        verses
            .first()
            .is_none_or(|row| same_identity(row, manifest)),
        "Invitation conflicts with the stored Verse identity; nothing was rebound"
    );
    let member = json!({"member_id":invitation.nonce(),"verse_id":manifest.verse_id,"peer_did":actor,
        "status":"active","invited_by":invitation.issuer_did(),"invite_sig":invitation.signature()});
    anyhow::ensure!(
        members.first().is_none_or(|row| row == &member),
        "Invitation nonce conflicts with stored membership"
    );
    let mut statements = vec![
        "IF time::unix() >= $join_expires_at { THROW 'invitation expired before admission'; }".into(),
        format!("IF ({verse_select}) != $join_verse_before {{ THROW 'invitation Verse changed'; }}"),
        format!("IF ({member_select}) != $join_member_before {{ THROW 'invitation membership changed'; }}"),
    ];
    if verses.is_empty() {
        statements.push("CREATE verse CONTENT $join_manifest".into());
    }
    if members.is_empty() {
        statements.push("CREATE verse_member CONTENT $join_member".into());
    }
    let mut proof_bindings = Vec::new();
    if let Some(branch) = invitation.recipient_manager_proof(now()?)? {
        crate::join_authority::prepare_quarantine(db, actor, &branch, now()?)
            .await?
            .prepend_to(&mut statements, &mut proof_bindings);
    }
    invitation.revalidate(actor, now()?)?;
    let script = format!(
        "BEGIN TRANSACTION;{};COMMIT TRANSACTION;",
        statements.join(";")
    );
    let mut full_member = member.clone();
    full_member["invite_timestamp"] = json!(chrono::Utc::now().to_rfc3339());
    let mut query = db.query(script)
        .bind(("join_verse", manifest.verse_id.clone()))
        .bind(("join_nonce", invitation.nonce().to_owned()))
        .bind(("join_verse_before", json!(verses)))
        .bind(("join_member_before", json!(members)))
        .bind(("join_manifest", serde_json::to_value(manifest)?))
        .bind(("join_member", full_member))
        .bind(("join_expires_at", invitation.expires_at()));
    for binding in proof_bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    Ok(admission(
        invitation,
        verses
            .first()
            .and_then(|row| row["name"].as_str())
            .unwrap_or(&manifest.name)
            .to_owned(),
    ))
}

#[allow(dead_code)]
pub(crate) async fn finalize_v2(
    db: &Db,
    actor: &str,
    credential: &JoinCredential,
) -> anyhow::Result<()> {
    let invitation = JoinInvitation::parse(credential.as_str(), actor, now()?)?;
    let branch = invitation.recipient_manager_proof(now()?)?.ok_or_else(|| {
        anyhow::anyhow!("Only a verified v2 recipient-Manager branch can finalize a proof")
    })?;
    crate::join_authority::finalize_recipient(db, actor, &branch, now()?).await
}

pub(crate) async fn ready(
    db: &Db,
    actor: &str,
    credential: &JoinCredential,
) -> anyhow::Result<bool> {
    let invitation = JoinInvitation::parse_owner(credential.as_str(), actor, now()?)?;
    let role = crate::role_manager::resolve_role(db, actor, invitation.scope()).await?;
    let required = fe_policy::RoleLevel::from(invitation.role().as_str());
    if !role.is_at_least(required) {
        return Ok(false);
    }
    let parts = crate::parse_scope(invitation.scope())?;
    if let Some(fractal) = &parts.fractal_id {
        let mut result = db.query("SELECT fractal_id FROM fractal WHERE fractal_id=$join_fractal AND verse_id=$join_verse LIMIT 2")
            .bind(("join_fractal",fractal.clone())).bind(("join_verse",parts.verse_id.clone())).await?.check()?;
        let rows: Vec<Value> = result.take(0)?;
        if rows.len() != 1 {
            return Ok(false);
        }
    }
    if let Some(petal) = &parts.petal_id {
        let mut result = db.query("SELECT petal_id FROM petal WHERE petal_id=$join_petal AND fractal_id=$join_fractal LIMIT 2")
            .bind(("join_petal",petal.clone())).bind(("join_fractal",parts.fractal_id.clone())).await?.check()?;
        let rows: Vec<Value> = result.take(0)?;
        if rows.len() != 1 {
            return Ok(false);
        }
    }
    let authority =
        super::entity::prepare_scope_permission(db, invitation.scope(), actor, required).await?;
    let rows = authority.read_rows(db,
        "SELECT verse_id,created_by,created_at,namespace_id, \
         (SELECT member_id FROM verse_member WHERE member_id=$join_nonce AND verse_id=$join_verse AND peer_did=$join_actor AND status='active' AND invited_by=$join_issuer AND invite_sig=$join_signature LIMIT 2) AS memberships, \
         (SELECT role,revoked,hlc FROM role_grant WHERE statement_key=$join_grant_key LIMIT 2) AS grants \
         FROM verse WHERE verse_id=$join_verse LIMIT 2",
        vec![("join_verse".into(),json!(invitation.manifest().verse_id)),("join_nonce".into(),json!(invitation.nonce())),
             ("join_actor".into(),json!(actor)),("join_issuer".into(),json!(invitation.issuer_did())),
             ("join_signature".into(),json!(invitation.signature())),
             ("join_grant_key".into(),json!(format!("{}/{}/{}",invitation.scope(),actor,invitation.issuer_did())))]).await?;
    anyhow::ensure!(
        rows.len() == 1 && same_identity(&rows[0], invitation.manifest()),
        "Joined Verse identity changed"
    );
    let memberships = rows[0]["memberships"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid joined membership read"))?;
    let grants = rows[0]["grants"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid joined role read"))?;
    invitation.revalidate(actor, now()?)?;
    if memberships.len() != 1 || grants.len() != 1 {
        return Ok(false);
    }
    let clock = grants[0]["hlc"]
        .as_str()
        .and_then(|raw| {
            raw.parse::<u64>()
                .ok()
                .filter(|clock| clock.to_string() == raw)
        })
        .ok_or_else(|| anyhow::anyhow!("Invalid stored invitation grant clock"))?;
    let revoked = grants[0]["revoked"]
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("Invalid stored invitation grant state"))?;
    let granted = grants[0]["role"]
        .as_str()
        .and_then(|role| JoinRole::parse(role).ok());
    Ok(!revoked
        && clock >= invitation.grant_hlc()
        && granted
            .is_some_and(|role| fe_policy::RoleLevel::from(role.as_str()).is_at_least(required)))
}
