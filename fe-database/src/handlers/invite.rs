//! Invite handlers — verse invite generation and join-by-invite.
//!
//! Separated from crud.rs for semantic clarity: invites are an auth/access
//! concern, not a CRUD concern.

use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::query_helpers::exec_query;
use crate::repo::{Db, Repo};
use crate::schema::{Verse, VerseMemberRow};

// ---------------------------------------------------------------------------
// Generate verse invite
// ---------------------------------------------------------------------------

#[instrument(skip(db, keypair, secret_store))]
pub(crate) async fn generate_verse_invite_handler(
    db: &Db,
    verse_id: &str,
    include_write_cap: bool,
    expiry_hours: u32,
    keypair: Option<&fe_identity::NodeKeypair>,
    secret_store: Option<&std::sync::Arc<dyn fe_identity::SecretStore>>,
) -> anyhow::Result<String> {
    let keypair =
        keypair.ok_or_else(|| anyhow::anyhow!("NodeKeypair not available for invite signing"))?;

    let sel_q = QueryBuilder::new()
        .select(&["namespace_id", "name", "created_by", "created_at"])
        .from("verse")
        .filter(Filter::eq("verse_id", verse_id))
        .limit(1)
        .build();
    let mut result = exec_query(db, &sel_q).await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("verse not found: {verse_id}"))?;

    let verse_name = row["name"].as_str().unwrap_or("Unknown").to_string();
    // RP-4 step 4 (T10): the invite pins the verse OWNER — never the inviter —
    // inside the signed bytes, so a non-owner Manager's invite still names the
    // real owner (src/AGENTS.md §handlers "replicated_row").
    let owner_did = row["created_by"].as_str().unwrap_or_default().to_string();
    if fe_identity::did_key::public_key_from_did_key(&owner_did).is_err() {
        anyhow::bail!("verse {verse_id} owner '{owner_did}' is not a did:key — cannot pin it");
    }
    let verse_created_at = row["created_at"].as_str().unwrap_or_default().to_string();

    let namespace_id = match row["namespace_id"].as_str() {
        Some(ns) if !ns.is_empty() => ns.to_string(),
        _ => {
            tracing::info!("Backfilling namespace_id for pre-Phase-E verse {verse_id}");
            let ns_secret = generate_namespace_secret();
            let ns_id = crate::derive_namespace_id(&ns_secret);
            let ns_id_hex = hex::encode(ns_id);
            let ns_secret_hex = hex::encode(ns_secret);

            let upd_q = UpdateBuilder::update("verse")
                .set("namespace_id", ns_id_hex.as_str())
                .where_clause(Filter::eq("verse_id", verse_id))
                .build();
            exec_query(db, &upd_q).await?;

            if let Some(ss) = secret_store {
                if let Err(e) = store_namespace_secret(ss.as_ref(), verse_id, &ns_secret_hex) {
                    tracing::warn!("Could not store backfilled namespace secret: {e}");
                }
            }

            ns_id_hex
        }
    };

    let namespace_secret = if include_write_cap {
        match secret_store {
            Some(ss) => crate::get_namespace_secret(ss.as_ref(), verse_id)?,
            None => None,
        }
    } else {
        None
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry = now + (expiry_hours as u64) * 3600;

    let creator_node_addr = hex::encode(keypair.verifying_key().to_bytes());

    let mut invite = crate::invite::VerseInvite {
        namespace_id,
        namespace_secret,
        creator_node_addr,
        owner_did,
        verse_created_at,
        verse_name,
        verse_id: verse_id.to_string(),
        expiry_timestamp: expiry,
        signature: String::new(),
    };
    invite.sign(keypair);

    let invite_string = invite.to_invite_string();
    tracing::info!(
        "Generated invite for verse {verse_id} (write_cap={include_write_cap}, expires in {expiry_hours}h)"
    );
    Ok(invite_string)
}

// ---------------------------------------------------------------------------
// Join verse by invite
// ---------------------------------------------------------------------------

#[instrument(skip(db, invite_string, secret_store))]
pub(crate) async fn join_verse_by_invite_handler(
    db: &Db,
    invite_string: &str,
    local_did: &str,
    secret_store: Option<&std::sync::Arc<dyn fe_identity::SecretStore>>,
) -> anyhow::Result<(String, String)> {
    let payload = invite_string.trim();
    let payload = payload
        .strip_prefix("fractalengine://invite/")
        .unwrap_or(payload);

    // Try parsing as a scoped invite first (new format)
    if let Ok(scoped) = crate::scoped_invite::ScopedInvite::from_invite_string(payload) {
        if let Err(e) = crate::scoped_invite::validate_scoped_invite(&scoped) {
            anyhow::bail!("Invalid scoped invite: {e}");
        }
        let parts = crate::scope::parse_scope(&scoped.scope)?;
        let verse_id = parts.verse_id;
        return Ok((verse_id, "Joined via scoped invite".to_string()));
    }

    let invite = crate::invite::VerseInvite::from_invite_string(payload)?;

    let creator_pk_bytes = hex::decode(&invite.creator_node_addr)
        .map_err(|e| anyhow::anyhow!("Invalid creator_node_addr hex: {e}"))?;
    let creator_pk_array: [u8; 32] = creator_pk_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("creator_node_addr is not 32 bytes"))?;
    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&creator_pk_array)
        .map_err(|e| anyhow::anyhow!("Invalid creator public key: {e}"))?;
    if !invite.verify(&verifying_key) {
        anyhow::bail!("Invalid invite signature");
    }

    if invite.is_expired() {
        anyhow::bail!("Invite has expired");
    }
    // The signed owner pin must be a real did:key: it becomes the owner every
    // role check and the immutable-identity gate resolve against.
    if fe_identity::did_key::public_key_from_did_key(&invite.owner_did).is_err() {
        anyhow::bail!("Invite owner pin is not a did:key");
    }

    let verse_id = &invite.verse_id;
    let verse_name = &invite.verse_name;
    let namespace_id = &invite.namespace_id;

    let check_q = QueryBuilder::new()
        .select(&["*"])
        .from("verse")
        .filter(Filter::eq("verse_id", verse_id.as_str()))
        .limit(1)
        .build();
    let mut check = exec_query(db, &check_q).await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;

    if existing.is_empty() {
        // The owner pin (T10): the owner's replicated manifest must match these
        // identity fields exactly, so they come from the signed invite.
        Repo::<Verse>::create(
            db,
            &Verse {
                verse_id: verse_id.clone(),
                name: verse_name.clone(),
                created_by: invite.owner_did.clone(),
                created_at: invite.verse_created_at.clone(),
                namespace_id: Some(namespace_id.clone()),
                default_access: "viewer".to_string(),
                ts_mode: "mirror".to_string(),
                ts_replication_factor: 1,
                ts_bucket_width_ms: fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS as i64,
                view_hidden: None,
            },
        )
        .await?;
        tracing::info!("Created verse from invite: {verse_name} ({verse_id})");
    } else {
        let pinned = existing[0]["created_by"].as_str().unwrap_or_default();
        if pinned != invite.owner_did {
            tracing::warn!(
                verse_id = %verse_id,
                local_owner = pinned,
                invite_owner = %invite.owner_did,
                "joined verse already exists locally under another owner — the local identity is kept (immutable)"
            );
        }
        tracing::info!("Verse {verse_id} already exists, updating namespace_id");
        let upd_ns_q = UpdateBuilder::update("verse")
            .set("namespace_id", namespace_id.as_str())
            .where_clause(Filter::eq("verse_id", verse_id.as_str()))
            .build();
        exec_query(db, &upd_ns_q).await?;
    }

    if let Some(ref secret) = invite.namespace_secret {
        if let Some(ss) = secret_store {
            if let Err(e) = store_namespace_secret(ss.as_ref(), verse_id, secret) {
                tracing::warn!("Could not store namespace secret for joined verse {verse_id}: {e}");
            }
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let member_id = ulid::Ulid::new().to_string();
    Repo::<VerseMemberRow>::create_raw(
        db,
        serde_json::json!({
            "member_id":        member_id,
            "verse_id":         verse_id,
            "peer_did":         local_did,
            "status":           "active",
            "invited_by":       invite.creator_node_addr,
            "invite_sig":       invite.signature,
            "invite_timestamp": now,
        }),
    )
    .await?;

    tracing::info!(
        "Joined verse {verse_name} ({verse_id}) via invite (write_cap={})",
        invite.namespace_secret.is_some()
    );

    Ok((verse_id.clone(), verse_name.clone()))
}

// ---------------------------------------------------------------------------
// Helpers (private)
// ---------------------------------------------------------------------------

pub(crate) fn generate_namespace_secret() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

/// Store a namespace secret for a verse in the given secret store.
pub(crate) fn store_namespace_secret(
    store: &dyn fe_identity::SecretStore,
    verse_id: &str,
    secret_hex: &str,
) -> anyhow::Result<()> {
    let service = format!("fractalengine:verse:{}:ns_secret", verse_id);
    store
        .set(&service, "fractalengine", secret_hex)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::replicated_row::apply_replicated_row_handler;
    use fe_runtime::messages::ReplicatedRowOutcome;

    const VERSE: &str = "verse-invite";
    const CREATED_AT: &str = "2026-10-15T00:00:00Z";

    async fn mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    fn manifest(owner: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "verse_id": VERSE, "name": "Shared", "created_by": owner,
            "created_at": CREATED_AT, "namespace_id": "ab".repeat(32),
            "default_access": "viewer", "ts_mode": "balanced",
            "ts_replication_factor": 2, "ts_bucket_width_ms": 3_600_000,
        }))
        .unwrap()
    }

    /// The Manager's store: the owner's manifest replicated in (trust on first
    /// use), so the Manager is NOT the owner of the verse it invites into.
    async fn managers_store(owner: &str) -> Db {
        let db = mem_db().await;
        let outcome =
            apply_replicated_row_handler(&db, VERSE, "verse", VERSE, &manifest(owner), owner, None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        db
    }

    async fn verse_row(db: &Db) -> serde_json::Value {
        let mut res = db
            .query("SELECT * FROM verse WHERE verse_id = $v")
            .bind(("v", VERSE.to_string()))
            .await
            .unwrap();
        let rows: Vec<serde_json::Value> = res.take(0).unwrap();
        rows.into_iter().next().expect("verse row")
    }

    /// T10: an invite generated by a NON-owner Manager pins the real owner (a
    /// did:key, not the inviter's hex address); the invitee records it, and the
    /// owner's later manifest and petal rows are applied as the Owner's.
    #[tokio::test]
    async fn an_invitee_records_the_real_owner_and_admits_the_owners_rows_as_owner() {
        let owner = fe_identity::NodeKeypair::generate().to_did_key();
        let manager = fe_identity::NodeKeypair::generate();
        let invitee_did = fe_identity::NodeKeypair::generate().to_did_key();

        let invite = generate_verse_invite_handler(
            &managers_store(&owner).await,
            VERSE,
            false,
            1,
            Some(&manager),
            None,
        )
        .await
        .expect("a Manager can invite");

        let invitee = mem_db().await;
        join_verse_by_invite_handler(&invitee, &invite, &invitee_did, None)
            .await
            .expect("join");
        let pinned = verse_row(&invitee).await;
        assert_eq!(
            pinned["created_by"],
            owner.as_str(),
            "the owner, not the inviter"
        );
        assert_eq!(pinned["created_at"], CREATED_AT);
        let member: Vec<serde_json::Value> = invitee
            .query("SELECT invited_by FROM verse_member")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            member[0]["invited_by"],
            hex::encode(manager.verifying_key().to_bytes()),
            "invited_by keeps the inviter"
        );

        // The owner's manifest matches the pinned identity and replaces the placeholders.
        let outcome = apply_replicated_row_handler(
            &invitee,
            VERSE,
            "verse",
            VERSE,
            &manifest(&owner),
            &owner,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(verse_row(&invitee).await["ts_mode"], "balanced");

        let fractal = serde_json::to_vec(&serde_json::json!({
            "fractal_id": "fractal-o", "verse_id": VERSE, "owner_did": owner,
            "name": "F", "created_at": CREATED_AT,
        }))
        .unwrap();
        let petal = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-o", "fractal_id": "fractal-o", "name": "P",
            "node_id": owner, "created_at": CREATED_AT,
        }))
        .unwrap();
        for (table, id, bytes) in [
            ("fractal", "fractal-o", fractal),
            ("petal", "petal-o", petal),
        ] {
            let outcome =
                apply_replicated_row_handler(&invitee, VERSE, table, id, &bytes, &owner, None)
                    .await
                    .unwrap();
            assert_eq!(
                outcome,
                ReplicatedRowOutcome::Applied,
                "the owner's {table}"
            );
        }
        let verse_scope = crate::build_scope(VERSE, None, None);
        assert_eq!(
            crate::role_manager::resolve_role(&invitee, &owner, &verse_scope)
                .await
                .unwrap(),
            fe_policy::RoleLevel::Owner
        );
    }

    /// RP-4 steps 1+4: once an invite pinned the owner, a manifest naming
    /// another owner is never applied — the pin IS the local manifest, whose
    /// identity is immutable — and the pinned owner keeps the verse.
    #[tokio::test]
    async fn a_pinned_owner_rejects_a_first_manifest_naming_another_owner() {
        let owner = fe_identity::NodeKeypair::generate().to_did_key();
        let impostor = fe_identity::NodeKeypair::generate().to_did_key();
        let inviter = fe_identity::NodeKeypair::generate();
        let invite = generate_verse_invite_handler(
            &managers_store(&owner).await,
            VERSE,
            false,
            1,
            Some(&inviter),
            None,
        )
        .await
        .unwrap();
        let invitee = mem_db().await;
        join_verse_by_invite_handler(&invitee, &invite, "did:key:z6MkInvitee", None)
            .await
            .unwrap();

        let outcome = apply_replicated_row_handler(
            &invitee,
            VERSE,
            "verse",
            VERSE,
            &manifest(&impostor),
            &impostor,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        assert_eq!(verse_row(&invitee).await["created_by"], owner.as_str());
    }

    /// An invite whose pinned owner is not a did:key is refused before any write.
    #[tokio::test]
    async fn an_invite_pinning_a_non_did_owner_is_refused_before_any_write() {
        let inviter = fe_identity::NodeKeypair::generate();
        let mut invite = crate::invite::VerseInvite {
            namespace_id: "ab".repeat(32),
            namespace_secret: None,
            creator_node_addr: hex::encode(inviter.verifying_key().to_bytes()),
            owner_did: "not-a-did".to_string(),
            verse_created_at: CREATED_AT.to_string(),
            verse_name: "V".to_string(),
            verse_id: VERSE.to_string(),
            expiry_timestamp: u64::MAX,
            signature: String::new(),
        };
        invite.sign(&inviter);
        let invitee = mem_db().await;
        let joined = join_verse_by_invite_handler(
            &invitee,
            &invite.to_invite_string(),
            "did:key:z6MkX",
            None,
        )
        .await;
        assert!(joined.is_err());
        let rows: Vec<serde_json::Value> = invitee
            .query("SELECT * FROM verse")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(rows.is_empty());
    }
}
