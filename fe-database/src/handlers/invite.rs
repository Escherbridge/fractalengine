//! Invite handlers — verse invite generation and join-by-invite.
//!
//! Separated from crud.rs for semantic clarity: invites are an auth/access
//! concern, not a CRUD concern.

use crate::repo::{Db, Repo};
use crate::schema::{Verse, VerseMemberRow};

// ---------------------------------------------------------------------------
// Generate verse invite
// ---------------------------------------------------------------------------

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

    let mut result: surrealdb::IndexedResults = db
        .query("SELECT namespace_id, name FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let row = rows
        .first()
        .ok_or_else(|| anyhow::anyhow!("verse not found: {verse_id}"))?;

    let verse_name = row["name"].as_str().unwrap_or("Unknown").to_string();

    let namespace_id = match row["namespace_id"].as_str() {
        Some(ns) if !ns.is_empty() => ns.to_string(),
        _ => {
            tracing::info!("Backfilling namespace_id for pre-Phase-E verse {verse_id}");
            let ns_secret = generate_namespace_secret();
            let ns_id = crate::derive_namespace_id(&ns_secret);
            let ns_id_hex = hex::encode(ns_id);
            let ns_secret_hex = hex::encode(ns_secret);

            db.query("UPDATE verse SET namespace_id = $nsid WHERE verse_id = $vid")
                .bind(("nsid", ns_id_hex.clone()))
                .bind(("vid", verse_id.to_string()))
                .await?;

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

    let verse_id = &invite.verse_id;
    let verse_name = &invite.verse_name;
    let namespace_id = &invite.namespace_id;

    let mut check: surrealdb::IndexedResults = db
        .query("SELECT * FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let existing: Vec<serde_json::Value> = check.take(0)?;

    if existing.is_empty() {
        let now = chrono::Utc::now().to_rfc3339();
        Repo::<Verse>::create(
            db,
            &Verse {
                verse_id: verse_id.clone(),
                name: verse_name.clone(),
                created_by: invite.creator_node_addr.clone(),
                created_at: now.clone(),
                namespace_id: Some(namespace_id.clone()),
                default_access: "viewer".to_string(),
            },
        )
        .await?;
        tracing::info!("Created verse from invite: {verse_name} ({verse_id})");
    } else {
        tracing::info!("Verse {verse_id} already exists, updating namespace_id");
        db.query("UPDATE verse SET namespace_id = $nsid WHERE verse_id = $vid")
            .bind(("nsid", namespace_id.to_string()))
            .bind(("vid", verse_id.to_string()))
            .await?;
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
