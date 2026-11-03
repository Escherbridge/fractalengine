//! Local proof quarantine and current authority fences; see src/AGENTS.md section join-authority.

use fe_policy::{GrantedRole, RoleLevel, RoleStatement};
use serde_json::{json, Value};
use std::collections::BTreeSet;

use crate::{
    handlers::entity::prepare_scope_permission,
    join_delegation::{
        DelegationChain, VerifiedDelegationChain, VerseIdentity, MAX_DELEGATION_BYTES,
    },
    join_invite::RecipientManagerProof,
    repo::Db,
};

const MAX_AUTHORITY_ROWS: usize = 8192;
const MAX_ACTIVE_PROOFS: usize = 32;
const MAX_LIVE_PROOFS: usize = 64;
const PROOF_COLUMNS: &str = "proof_id,verse_id,holder_did,scope,proof_json,state,admission_nonce,invite_signature,expires_at";
const IDENTITY_SELECT: &str = "SELECT verse_id,created_by,created_at,namespace_id FROM verse WHERE verse_id=$delegation_verse LIMIT 2";
const GRANT_SELECT: &str = "SELECT statement_key,verse_id,scope,peer_did,grantor_did,role,revoked,hlc FROM role_grant WHERE verse_id=$delegation_verse ORDER BY statement_key LIMIT 8193";
const HELD_SELECT: &str = "SELECT statement_key,verse_id,scope,peer_did,grantor_did,role,revoked,hlc FROM role_statement_pending WHERE verse_id=$delegation_verse ORDER BY statement_key LIMIT 8193";

fn proof_select() -> String {
    format!("SELECT {PROOF_COLUMNS} FROM join_delegation WHERE proof_id=$delegation_proof LIMIT 2")
}

fn identity_row(identity: &VerseIdentity) -> Value {
    json!({"verse_id":identity.verse_id,"created_by":identity.owner_did,
        "created_at":identity.created_at,"namespace_id":identity.namespace_id})
}

fn canonical_hlc(row: &Value) -> anyhow::Result<u64> {
    let raw = row["hlc"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid delegation grant clock"))?;
    let clock = raw
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("Invalid delegation grant clock"))?;
    anyhow::ensure!(
        clock > 0 && clock.to_string() == raw,
        "Noncanonical delegation grant clock"
    );
    Ok(clock)
}

fn statements(
    rows: &[Value],
    identity: &VerseIdentity,
    keys: &mut BTreeSet<String>,
) -> anyhow::Result<Vec<RoleStatement>> {
    anyhow::ensure!(
        rows.len() <= MAX_AUTHORITY_ROWS,
        "Manager authority snapshot exceeds its bound"
    );
    rows.iter()
        .map(|row| {
            let text = |name: &str| {
                row[name]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Invalid delegation grant row"))
            };
            let scope = text("scope")?;
            crate::join_delegation::validate_scope(scope, identity)?;
            let peer = text("peer_did")?;
            let grantor = text("grantor_did")?;
            let key = format!("{scope}/{peer}/{grantor}");
            anyhow::ensure!(
                text("verse_id")? == identity.verse_id
                    && text("statement_key")? == key
                    && keys.insert(key),
                "Ambiguous delegation grant identity"
            );
            Ok(RoleStatement {
                scope: scope.into(),
                peer_did: peer.into(),
                grantor_did: grantor.into(),
                role: GrantedRole::parse(text("role")?)
                    .ok_or_else(|| anyhow::anyhow!("Invalid delegation grant role"))?,
                revoked: row["revoked"]
                    .as_bool()
                    .ok_or_else(|| anyhow::anyhow!("Invalid delegation grant state"))?,
                hlc: canonical_hlc(row)?,
            })
        })
        .collect()
}

fn checked_proof(
    row: &Value,
    actor: &str,
    identity: &VerseIdentity,
    scope: &str,
    now: u64,
) -> anyhow::Result<VerifiedDelegationChain> {
    let raw = row["proof_json"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Manager proof is unavailable"))?;
    anyhow::ensure!(
        !raw.is_empty() && raw.len() <= MAX_DELEGATION_BYTES,
        "Manager proof exceeds its raw size limit"
    );
    let chain: DelegationChain =
        serde_json::from_str(raw).map_err(|_| anyhow::anyhow!("Invalid stored Manager proof"))?;
    let verified = chain.verify(actor, identity, scope, now)?;
    anyhow::ensure!(
        row["proof_id"].as_str() == Some(verified.proof_id()?.as_str())
            && row["verse_id"].as_str() == Some(identity.verse_id.as_str())
            && row["holder_did"].as_str() == Some(actor)
            && row["scope"].as_str() == Some(verified.scope())
            && row["expires_at"].as_u64() == Some(verified.expires_at())
            && serde_json::to_string(verified.chain())? == raw,
        "Stored Manager proof identity changed"
    );
    Ok(verified)
}

#[derive(Clone)]
pub(crate) struct ManagerAuthority {
    actor: String,
    scope: String,
    identity: VerseIdentity,
    proof: VerifiedDelegationChain,
    roles: fe_policy::EffectiveRoles,
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
}

impl ManagerAuthority {
    pub(crate) fn matches(&self, actor: &str, scope: &str, identity: &VerseIdentity) -> bool {
        self.actor == actor && self.scope == scope && &self.identity == identity
    }
    pub(crate) fn proof(&self) -> &VerifiedDelegationChain {
        &self.proof
    }
    pub(crate) fn into_parts(self) -> (Vec<String>, Vec<(String, Value)>) {
        (self.guards, self.bindings)
    }

    pub(crate) async fn capture(
        db: &Db,
        actor: &str,
        identity: &VerseIdentity,
        scope: &str,
        expected_proof: Option<&str>,
        now: u64,
    ) -> anyhow::Result<Self> {
        let select = if expected_proof.is_some() {
            proof_select()
        } else {
            format!("SELECT {PROOF_COLUMNS} FROM join_delegation WHERE verse_id=$delegation_verse AND holder_did=$delegation_actor AND state='active' AND expires_at > time::unix() ORDER BY proof_id LIMIT 33")
        };
        let mut query = db
            .query(select)
            .bind(("delegation_verse", identity.verse_id.clone()))
            .bind(("delegation_actor", actor.to_owned()));
        if let Some(id) = expected_proof {
            query = query.bind(("delegation_proof", id.to_owned()));
        }
        let rows: Vec<Value> = query.await?.check()?.take(0)?;
        anyhow::ensure!(
            rows.len() <= MAX_ACTIVE_PROOFS,
            "Manager proof selection exceeds its bound; request Owner reauthorization"
        );
        anyhow::ensure!(
            expected_proof.is_none() || rows.len() <= 1,
            "Ambiguous Manager proof identity"
        );
        for row in rows {
            if row["state"].as_str() != Some("active") {
                continue;
            }
            if let Ok(authority) = Self::capture_row(db, actor, identity, scope, &row, now).await {
                return Ok(authority);
            }
        }
        anyhow::bail!("Manager requires current Owner reauthorization with a portable proof")
    }

    async fn capture_row(
        db: &Db,
        actor: &str,
        identity: &VerseIdentity,
        scope: &str,
        row: &Value,
        now: u64,
    ) -> anyhow::Result<Self> {
        let proof = checked_proof(row, actor, identity, scope, now)?;
        let mut current =
            CurrentChainAuthority::capture(db, actor, identity, scope, &proof, now).await?;
        current.guards.push(format!(
            "IF ({}) != $delegation_proof_row {{ THROW 'delegation proof changed'; }}",
            proof_select()
        ));
        current.bindings.extend([
            ("delegation_proof".into(), json!(proof.proof_id()?)),
            ("delegation_proof_row".into(), json!([row])),
        ]);
        Ok(Self {
            actor: actor.into(),
            scope: scope.into(),
            identity: identity.clone(),
            proof,
            roles: current.roles,
            guards: current.guards,
            bindings: current.bindings,
        })
    }
}

#[derive(Debug)]
pub(crate) struct AuthorityNotCurrent;
impl std::fmt::Display for AuthorityNotCurrent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Current Manager proof supporting grants have not converged or changed")
    }
}
impl std::error::Error for AuthorityNotCurrent {}
fn require_current(current: bool) -> anyhow::Result<()> {
    if current {
        Ok(())
    } else {
        Err(AuthorityNotCurrent.into())
    }
}

pub(crate) struct CurrentChainAuthority {
    roles: fe_policy::EffectiveRoles,
    guards: Vec<String>,
    bindings: Vec<(String, Value)>,
}
impl CurrentChainAuthority {
    pub(crate) fn role_at(&self, peer: &str, scope: &str) -> RoleLevel {
        self.roles.role_at(peer, scope)
    }
    pub(crate) fn scoped_parts(self, index: usize) -> (Vec<String>, Vec<(String, Value)>) {
        let prefix = format!("join_chain_{index}_");
        let guards = self
            .guards
            .into_iter()
            .map(|sql| {
                sql.replace("$delegation_", &format!("${prefix}delegation_"))
                    .replace("$scoped_query_", &format!("${prefix}scoped_query_"))
            })
            .collect();
        let bindings = self
            .bindings
            .into_iter()
            .map(|(name, value)| (format!("{prefix}{name}"), value))
            .collect();
        (guards, bindings)
    }
    pub(crate) async fn capture(
        db: &Db,
        actor: &str,
        identity: &VerseIdentity,
        scope: &str,
        proof: &VerifiedDelegationChain,
        now: u64,
    ) -> anyhow::Result<Self> {
        proof.chain().verify(actor, identity, scope, now)?;
        let mut snapshot = db
            .query(format!("{IDENTITY_SELECT};{GRANT_SELECT};{HELD_SELECT}"))
            .bind(("delegation_verse", identity.verse_id.clone()))
            .await?
            .check()?;
        let verses: Vec<Value> = snapshot.take(0)?;
        let grants: Vec<Value> = snapshot.take(1)?;
        let held: Vec<Value> = snapshot.take(2)?;
        anyhow::ensure!(
            verses == vec![identity_row(identity)],
            "Delegation Verse identity changed"
        );
        let mut keys = BTreeSet::new();
        let admitted = statements(&grants, identity, &mut keys)?;
        statements(&held, identity, &mut keys)?;
        let effective = fe_policy::materialize(&identity.owner_did, admitted.iter());
        require_current(
            effective
                .authority(actor, scope)
                .is_at_least(RoleLevel::Manager),
        )?;
        let mut guards=vec![
            format!("IF ({IDENTITY_SELECT}) != $delegation_identity {{ THROW 'delegation Verse changed'; }}"),
            format!("IF ({GRANT_SELECT}) != $delegation_grants {{ THROW 'delegation grants changed'; }}"),
            format!("IF ({HELD_SELECT}) != $delegation_held {{ THROW 'delegation held grants changed'; }}"),
            "IF time::unix() >= $delegation_expiry { THROW 'delegation proof expired'; }".into(),
        ];
        let mut bindings = vec![
            ("delegation_verse".into(), json!(identity.verse_id)),
            ("delegation_identity".into(), json!(verses)),
            ("delegation_grants".into(), json!(grants)),
            ("delegation_held".into(), json!(held)),
            ("delegation_expiry".into(), json!(proof.expires_at())),
        ];
        for (index, support) in proof.supporting_grants().enumerate() {
            let matches: Vec<_> = admitted
                .iter()
                .filter(|statement| {
                    statement.scope == support.scope
                        && statement.peer_did == support.peer_did
                        && statement.grantor_did == support.grantor_did
                })
                .collect();
            require_current(
                matches.len() == 1
                    && matches[0].role == GrantedRole::Manager
                    && !matches[0].revoked
                    && matches[0].hlc == support.hlc
                    && effective.admissible(matches[0])
                    && effective
                        .authority(&support.peer_did, &support.scope)
                        .is_at_least(RoleLevel::Manager),
            )?;
            require_current(
                crate::role_manager::resolve_role(db, &support.peer_did, &support.scope)
                    .await?
                    .is_at_least(RoleLevel::Manager),
            )?;
            let ancestry =
                prepare_scope_permission(db, &support.scope, &support.peer_did, RoleLevel::Manager)
                    .await?;
            let (fences, values) = ancestry.scoped_read_fence(index);
            guards.extend(fences);
            bindings.extend(values);
        }
        require_current(
            crate::role_manager::resolve_role(db, actor, scope)
                .await?
                .is_at_least(RoleLevel::Manager),
        )?;
        let target = prepare_scope_permission(db, scope, actor, RoleLevel::Manager).await?;
        let (fences, values) =
            target.scoped_read_fence(crate::join_delegation::MAX_DELEGATION_DEPTH);
        guards.extend(fences);
        bindings.extend(values);
        Ok(Self {
            roles: effective,
            guards,
            bindings,
        })
    }
}

pub(crate) async fn pending_recipient_authority(
    db: &Db,
    actor: &str,
    branch: &RecipientManagerProof,
    now: u64,
) -> anyhow::Result<CurrentChainAuthority> {
    let proof = branch.chain();
    let id = proof.proof_id()?;
    let mut response = db
        .query(proof_select())
        .bind(("delegation_proof", id.clone()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() == 1
            && matches!(rows[0]["state"].as_str(), Some("quarantined" | "active"))
            && rows[0]["admission_nonce"].as_str() == Some(branch.nonce())
            && rows[0]["invite_signature"].as_str() == Some(branch.signature()),
        "Recipient proof acceptance changed"
    );
    let authority =
        ManagerAuthority::capture_row(db, actor, proof.identity(), proof.scope(), &rows[0], now)
            .await?;
    anyhow::ensure!(
        authority.proof().proof_id()? == id,
        "Recipient proof identity changed"
    );
    let ManagerAuthority {
        roles,
        guards,
        bindings,
        ..
    } = authority;
    Ok(CurrentChainAuthority {
        roles,
        guards,
        bindings,
    })
}

pub(crate) struct QuarantineWrite {
    steps: Vec<String>,
    bindings: Vec<(String, Value)>,
}

impl QuarantineWrite {
    pub(crate) fn prepend_to(
        self,
        statements: &mut Vec<String>,
        bindings: &mut Vec<(String, Value)>,
    ) {
        statements.splice(0..0, self.steps);
        bindings.extend(self.bindings);
    }
}

pub(crate) async fn prepare_quarantine(
    db: &Db,
    actor: &str,
    branch: &RecipientManagerProof,
    now: u64,
) -> anyhow::Result<QuarantineWrite> {
    let proof = branch.chain();
    proof
        .chain()
        .verify(actor, proof.identity(), proof.scope(), now)?;
    anyhow::ensure!(
        proof.holder() == actor && branch.expires_at() == proof.expires_at(),
        "Recipient Manager proof does not match this acceptance"
    );
    let proof_id = proof.proof_id()?;
    let raw = serde_json::to_string(proof.chain())?;
    anyhow::ensure!(
        raw.len() <= MAX_DELEGATION_BYTES,
        "Manager proof exceeds its size limit"
    );
    let row = json!({"proof_id":proof_id,"verse_id":proof.identity().verse_id,"holder_did":actor,
        "scope":proof.scope(),"proof_json":raw,"state":"quarantined","admission_nonce":branch.nonce(),
        "invite_signature":branch.signature(),"expires_at":i64::try_from(branch.expires_at())?});
    let live_select="SELECT proof_id FROM join_delegation WHERE verse_id=$delegation_verse AND holder_did=$delegation_holder AND expires_at > time::unix() ORDER BY proof_id LIMIT 65";
    let mut response = db
        .query(format!(
            "{};{live_select};{IDENTITY_SELECT}",
            proof_select()
        ))
        .bind(("delegation_proof", proof_id.clone()))
        .bind(("delegation_verse", proof.identity().verse_id.clone()))
        .bind(("delegation_holder", actor.to_owned()))
        .await?
        .check()?;
    let before: Vec<Value> = response.take(0)?;
    let live: Vec<Value> = response.take(1)?;
    let verses: Vec<Value> = response.take(2)?;
    anyhow::ensure!(
        verses.is_empty() || verses == vec![identity_row(proof.identity())],
        "Recipient Manager Verse identity conflicts with storage"
    );
    anyhow::ensure!(before.len() <= 1, "Ambiguous recipient Manager proof");
    if let Some(stored) = before.first() {
        let mut expected = row.clone();
        expected["state"] = stored["state"].clone();
        anyhow::ensure!(
            stored == &expected
                && matches!(stored["state"].as_str(), Some("quarantined" | "active")),
            "Recipient proof conflicts with stored acceptance"
        );
    } else {
        anyhow::ensure!(
            live.len() < MAX_LIVE_PROOFS,
            "Local proof quarantine is full; no acceptance was stored"
        );
    }
    let mut steps=vec![
        format!("IF ({IDENTITY_SELECT}) != $delegation_identity_before {{ THROW 'recipient Verse identity changed'; }}"),
        format!("IF ({}) != $delegation_proof_before {{ THROW 'recipient Manager proof changed'; }}",proof_select()),
        format!("IF ({live_select}) != $delegation_live_before {{ THROW 'recipient proof capacity changed'; }}"),
        "IF time::unix() >= $delegation_expiry { THROW 'recipient Manager proof expired'; }".into(),
    ];
    if before.is_empty() {
        steps.push("CREATE join_delegation CONTENT $delegation_new_proof".into());
    }
    Ok(QuarantineWrite {
        steps,
        bindings: vec![
            ("delegation_proof".into(), json!(proof_id)),
            ("delegation_verse".into(), json!(proof.identity().verse_id)),
            ("delegation_holder".into(), json!(actor)),
            ("delegation_proof_before".into(), json!(before)),
            ("delegation_live_before".into(), json!(live)),
            ("delegation_expiry".into(), json!(branch.expires_at())),
            ("delegation_new_proof".into(), row),
            ("delegation_identity_before".into(), json!(verses)),
        ],
    })
}

pub(crate) async fn finalize_recipient_until(
    db: &Db,
    actor: &str,
    branch: &RecipientManagerProof,
    now: u64,
    deadline: u64,
) -> anyhow::Result<()> {
    let proof = branch.chain();
    let id = proof.proof_id()?;
    let mut response = db
        .query(proof_select())
        .bind(("delegation_proof", id.clone()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() == 1
            && matches!(rows[0]["state"].as_str(), Some("quarantined" | "active"))
            && rows[0]["admission_nonce"].as_str() == Some(branch.nonce())
            && rows[0]["invite_signature"].as_str() == Some(branch.signature()),
        "Recipient Manager proof is not pending for this acceptance"
    );
    let authority =
        ManagerAuthority::capture_row(db, actor, proof.identity(), proof.scope(), &rows[0], now)
            .await?;
    anyhow::ensure!(
        authority.proof().proof_id()? == id,
        "Recipient Manager proof changed"
    );
    let member_select="SELECT member_id,verse_id,peer_did,status,invited_by,invite_sig FROM verse_member WHERE member_id=$delegation_nonce LIMIT 2";
    let mut membership = db
        .query(member_select)
        .bind(("delegation_nonce", branch.nonce().to_owned()))
        .await?
        .check()?;
    let members: Vec<Value> = membership.take(0)?;
    let expected = json!({"member_id":branch.nonce(),"verse_id":proof.identity().verse_id,"peer_did":actor,
        "status":"active","invited_by":branch.issuer_did(),"invite_sig":branch.signature()});
    anyhow::ensure!(
        members == vec![expected],
        "Recipient Manager acceptance changed"
    );
    let current = prepare_scope_permission(db, proof.scope(), actor, RoleLevel::Manager).await?;
    let member_guard = format!(
        "IF ({member_select}) != $delegation_members {{ THROW 'recipient acceptance changed'; }}"
    );
    current
        .with_join_deadline(deadline.min(branch.expires_at()))
        .with_join_delegation(authority)
        .commit(
            db,
            &[
                &member_guard,
                "UPDATE join_delegation SET state='active' WHERE proof_id=$delegation_proof",
            ],
            vec![
                ("delegation_nonce".into(), json!(branch.nonce())),
                ("delegation_members".into(), json!(members)),
            ],
        )
        .await
}
