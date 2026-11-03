//! Dormant current-authority v2 adapters; see handlers/AGENTS.md section addressed-v2.

use fe_policy::RoleLevel;
use fe_runtime::{
    entity_access::EntityAccessTarget,
    join_invite::{InviteAuthority, InviteSource, IssuedInvitation, JoinCredential},
};
use serde_json::{json, Value};

use crate::{
    join_authority::{AuthorityNotCurrent, CurrentChainAuthority, ManagerAuthority},
    join_delegation::{VerseIdentity, MAX_DELEGATION_DEPTH},
    join_invite::{JoinInvitation, JoinManifest, JoinRole},
    repo::Db,
    BlobStoreHandle, ReplicationSender,
};

use super::{
    addressed_join::{decode_manifest, now, same_identity, VERSE_COLUMNS},
    entity::{prepare_parent_mutation, prepare_scope_permission},
};

struct SourceCapture {
    manifest: JoinManifest,
    identity: InviteAuthority,
    manager: Option<ManagerAuthority>,
}

fn unconfirmed_issue(_: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("Invitation issuance is unconfirmed; the checked grant may already be stored. Refresh Access before another change; do not automatically replay issuance.")
}

async fn capture_source(
    db: &Db,
    actor: &str,
    target: &EntityAccessTarget,
    expected: Option<&InviteAuthority>,
    clock: u64,
) -> anyhow::Result<SourceCapture> {
    if let Some(expected) = expected {
        anyhow::ensure!(
            expected.issuer_did() == actor,
            "Invitation issuer identity changed"
        );
        if let Some(id) = expected.proof_id() {
            anyhow::ensure!(
                id.len() == 64
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "Invalid invitation proof identity"
            );
        }
    }
    let initial = prepare_parent_mutation(db, target.entity_type, &target.entity_id, actor).await?;
    anyhow::ensure!(
        initial.scope() == target.scope,
        "Invitation target ancestry changed"
    );
    let verse_id = crate::parse_scope(&target.scope)?.verse_id;
    let select =
        format!("SELECT {VERSE_COLUMNS} FROM verse WHERE verse_id=$join_source_verse LIMIT 2");
    let rows = initial
        .read_rows(
            db,
            &select,
            vec![("join_source_verse".into(), json!(verse_id))],
        )
        .await?;
    anyhow::ensure!(rows.len() == 1, "Ambiguous invitation Verse identity");
    let manifest = decode_manifest(&rows[0])?;
    manifest.validate()?;
    let manager = if manifest.created_by == actor {
        None
    } else {
        Some(
            ManagerAuthority::capture(
                db,
                actor,
                &VerseIdentity::from_manifest(&manifest),
                &target.scope,
                expected.and_then(InviteAuthority::proof_id),
                clock,
            )
            .await?,
        )
    };
    let identity = match &manager {
        Some(authority) => InviteAuthority::DelegatedManager {
            issuer_did: actor.into(),
            proof_id: authority.proof().proof_id()?,
        },
        None => InviteAuthority::Owner {
            issuer_did: actor.into(),
        },
    };
    anyhow::ensure!(
        expected.is_none_or(|expected| expected == &identity),
        "Invitation authority proof changed; refresh Access"
    );
    let mut current =
        prepare_parent_mutation(db, target.entity_type, &target.entity_id, actor).await?;
    anyhow::ensure!(
        current.scope() == target.scope,
        "Invitation target ancestry changed during preparation"
    );
    current = current.with_invite_manifest(
        &manifest,
        clock
            .checked_add(60)
            .ok_or_else(|| anyhow::anyhow!("Invitation preparation clock overflow"))?,
    );
    if let Some(manager) = &manager {
        current = current.with_join_delegation(manager.clone());
    }
    let confirmed = current
        .read_rows(
            db,
            &select,
            vec![("join_source_verse".into(), json!(verse_id))],
        )
        .await?;
    anyhow::ensure!(
        confirmed == rows,
        "Invitation source changed while preparing"
    );
    Ok(SourceCapture {
        manifest,
        identity,
        manager,
    })
}

fn validate_selection(
    actor: &str,
    recipient: &str,
    role: &str,
    hours: u32,
) -> anyhow::Result<JoinRole> {
    crate::join_invite::canonical_did(recipient)?;
    anyhow::ensure!(
        recipient != actor && (1..=720).contains(&hours),
        "Invalid invitation recipient or expiry"
    );
    JoinRole::parse(role)
}

fn validate_delegated_limits(
    source: &SourceCapture,
    role: JoinRole,
    clock: u64,
    hours: u32,
) -> anyhow::Result<()> {
    if let Some(manager) = &source.manager {
        let expires = clock
            .checked_add(u64::from(hours) * 3600)
            .ok_or_else(|| anyhow::anyhow!("Invitation expiry overflow"))?;
        anyhow::ensure!(
            expires <= manager.proof().expires_at(),
            "Choose a shorter invitation lifetime or obtain Owner reauthorization"
        );
        anyhow::ensure!(
            role != JoinRole::Manager || manager.proof().depth() < MAX_DELEGATION_DEPTH,
            "Manager delegation depth is exhausted; obtain Owner reauthorization"
        );
    }
    Ok(())
}

pub(crate) async fn prepare(
    db: &Db,
    actor: &str,
    target: &EntityAccessTarget,
    recipient: &str,
    role: &str,
    expiry_hours: u32,
) -> anyhow::Result<InviteSource> {
    let role = validate_selection(actor, recipient, role, expiry_hours)?;
    let clock = now()?;
    let captured = capture_source(db, actor, target, None, clock).await?;
    anyhow::ensure!(
        recipient != captured.manifest.created_by,
        "The Verse Owner cannot be an invitation recipient"
    );
    validate_delegated_limits(&captured, role, clock, expiry_hours)?;
    Ok(InviteSource {
        authority: captured.identity,
        target: target.clone(),
        recipient_did: recipient.into(),
        role: role.as_str().into(),
        expiry_hours,
        manifest: serde_json::to_value(captured.manifest)?,
        prepared_at: clock,
        nonce: ulid::Ulid::new().to_string(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn issue(
    db: &Db,
    actor: &str,
    signer: Option<&fe_identity::NodeKeypair>,
    source: &InviteSource,
    ticket: &JoinCredential,
    blobs: &BlobStoreHandle,
    replication: Option<&ReplicationSender>,
) -> anyhow::Result<IssuedInvitation> {
    let clock = now()?;
    anyhow::ensure!(
        source.prepared_at <= clock && clock - source.prepared_at <= 60,
        "Invitation preparation expired; refresh Access"
    );
    anyhow::ensure!(
        ulid::Ulid::from_string(&source.nonce).is_ok_and(|nonce| nonce.to_string() == source.nonce),
        "Invalid invitation preparation identity"
    );
    let signer = signer
        .filter(|signer| signer.to_did_key() == actor)
        .ok_or_else(|| anyhow::anyhow!("Invitation signer unavailable"))?;
    let role = validate_selection(
        actor,
        &source.recipient_did,
        &source.role,
        source.expiry_hours,
    )?;
    let manifest = decode_manifest(&source.manifest)?;
    let captured =
        capture_source(db, actor, &source.target, Some(&source.authority), clock).await?;
    anyhow::ensure!(
        captured.manifest == manifest,
        "Invitation source changed; refresh Access"
    );
    validate_delegated_limits(&captured, role, clock, source.expiry_hours)?;
    let parent = captured
        .manager
        .as_ref()
        .map(|manager| manager.proof().clone());
    let lifetime = u64::from(source.expiry_hours) * 3600;
    JoinInvitation::issue_addressed(
        signer,
        source.recipient_did.clone(),
        source.target.scope.clone(),
        role,
        1,
        manifest.clone(),
        ticket.as_str().into(),
        clock,
        lifetime,
        parent.as_ref(),
    )?;
    let deadline = source
        .prepared_at
        .checked_add(60)
        .ok_or_else(|| anyhow::anyhow!("Invalid invitation preparation time"))?;
    let grant = match captured.manager {
        Some(authority) => {
            crate::role_manager::assign_role_for_delegated_invite(
                db,
                actor,
                &source.recipient_did,
                &source.target.scope,
                role.as_str(),
                &manifest,
                deadline,
                authority,
                replication,
                blobs,
            )
            .await
        }
        None => {
            crate::role_manager::assign_role_for_invite(
                db,
                actor,
                &source.recipient_did,
                &source.target.scope,
                role.as_str(),
                &manifest,
                deadline,
                replication,
                blobs,
            )
            .await
        }
    }
    .map_err(unconfirmed_issue)?;
    (|| -> anyhow::Result<IssuedInvitation> {
        let invitation = JoinInvitation::issue_addressed(
            signer,
            source.recipient_did.clone(),
            source.target.scope.clone(),
            role,
            grant,
            manifest,
            ticket.as_str().into(),
            now()?,
            lifetime,
            parent.as_ref(),
        )?;
        let credential = JoinCredential::new(invitation.to_link()?)?;
        let verified = JoinInvitation::parse(credential.as_str(), &source.recipient_did, now()?)?;
        anyhow::ensure!(
            verified.issuer_did() == actor
                && verified.scope() == source.target.scope
                && verified.grant_hlc() == grant,
            "Invitation finalization changed its checked grant"
        );
        Ok(IssuedInvitation {
            source: source.clone(),
            credential,
            expires_at: verified.expires_at(),
        })
    })()
    .map_err(unconfirmed_issue)
}

async fn target_loaded(db: &Db, scope: &str) -> anyhow::Result<bool> {
    let parts = crate::parse_scope(scope)?;
    if let Some(fractal) = &parts.fractal_id {
        let mut response=db.query("SELECT fractal_id FROM fractal WHERE fractal_id=$ready_fractal AND verse_id=$ready_verse LIMIT 2")
            .bind(("ready_fractal",fractal.clone())).bind(("ready_verse",parts.verse_id)).await?.check()?;
        let rows: Vec<Value> = response.take(0)?;
        if rows.len() != 1 {
            return Ok(false);
        }
    }
    if let Some(petal) = &parts.petal_id {
        let mut response=db.query("SELECT petal_id FROM petal WHERE petal_id=$ready_petal AND fractal_id=$ready_fractal LIMIT 2")
            .bind(("ready_petal",petal.clone())).bind(("ready_fractal",parts.fractal_id)).await?.check()?;
        let rows: Vec<Value> = response.take(0)?;
        if rows.len() != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) async fn ready(
    db: &Db,
    actor: &str,
    credential: &JoinCredential,
) -> anyhow::Result<bool> {
    let clock = now()?;
    let invitation = JoinInvitation::parse(credential.as_str(), actor, clock)?;
    anyhow::ensure!(
        invitation.version() == 2,
        "Expected an addressed-v2 invitation"
    );
    let required = RoleLevel::from(invitation.role().as_str());
    if !crate::role_manager::resolve_role(db, actor, invitation.scope())
        .await?
        .is_at_least(required)
        || !target_loaded(db, invitation.scope()).await?
    {
        return Ok(false);
    }
    let identity = VerseIdentity::from_manifest(invitation.manifest());
    let mut current = prepare_scope_permission(db, invitation.scope(), actor, required)
        .await?
        .with_join_deadline(invitation.expires_at());
    if let Some(chain) = invitation.issuer_delegation(clock)? {
        let authority = match CurrentChainAuthority::capture(
            db,
            invitation.issuer_did(),
            &identity,
            invitation.scope(),
            &chain,
            clock,
        )
        .await
        {
            Ok(authority) => authority,
            Err(error) if error.is::<AuthorityNotCurrent>() => return Ok(false),
            Err(error) => return Err(error),
        };
        if !authority
            .role_at(actor, invitation.scope())
            .is_at_least(required)
        {
            return Ok(false);
        }
        current = current.with_join_chain(authority, 0);
    }
    if let Some(branch) = invitation.recipient_manager_proof(clock)? {
        let authority =
            match crate::join_authority::pending_recipient_authority(db, actor, &branch, clock)
                .await
            {
                Ok(authority) => authority,
                Err(error) if error.is::<AuthorityNotCurrent>() => return Ok(false),
                Err(error) => return Err(error),
            };
        current = current.with_join_chain(authority, 1);
    }
    let key = format!(
        "{}/{}/{}",
        invitation.scope(),
        actor,
        invitation.issuer_did()
    );
    let rows=current.read_rows(db,
        "SELECT verse_id,created_by,created_at,namespace_id, \
         (SELECT member_id FROM verse_member WHERE member_id=$ready_nonce AND verse_id=$ready_verse AND peer_did=$ready_actor AND status='active' AND invited_by=$ready_issuer AND invite_sig=$ready_signature LIMIT 2) AS memberships, \
         (SELECT statement_key,verse_id,scope,peer_did,grantor_did,role,revoked,hlc FROM role_grant WHERE statement_key=$ready_key LIMIT 2) AS grants \
         FROM verse WHERE verse_id=$ready_verse LIMIT 2",
        vec![("ready_verse".into(),json!(identity.verse_id)),("ready_nonce".into(),json!(invitation.nonce())),
            ("ready_actor".into(),json!(actor)),("ready_issuer".into(),json!(invitation.issuer_did())),
            ("ready_signature".into(),json!(invitation.signature())),("ready_key".into(),json!(key))]).await?;
    anyhow::ensure!(
        rows.len() == 1 && same_identity(&rows[0], invitation.manifest()),
        "Joined Verse identity changed"
    );
    invitation.revalidate(actor, now()?)?;
    let members = rows[0]["memberships"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid readiness membership read"))?;
    let grants = rows[0]["grants"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid readiness grant read"))?;
    if members.len() != 1 || grants.len() != 1 {
        return Ok(false);
    }
    let grant = &grants[0];
    let raw = grant["hlc"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid readiness grant clock"))?;
    let hlc = raw
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid readiness grant clock"))?;
    anyhow::ensure!(
        hlc > 0 && hlc.to_string() == raw,
        "Noncanonical readiness grant clock"
    );
    let role = grant["role"]
        .as_str()
        .and_then(|role| JoinRole::parse(role).ok());
    Ok(grant["statement_key"].as_str() == Some(key.as_str())
        && grant["verse_id"].as_str() == Some(identity.verse_id.as_str())
        && grant["scope"].as_str() == Some(invitation.scope())
        && grant["peer_did"].as_str() == Some(actor)
        && grant["grantor_did"].as_str() == Some(invitation.issuer_did())
        && grant["revoked"].as_bool() == Some(false)
        && hlc >= invitation.grant_hlc()
        && role.is_some_and(|role| RoleLevel::from(role.as_str()).is_at_least(required)))
}
