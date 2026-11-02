//! Invite handlers — verse invite generation and join-by-invite.
//!
//! Separated from crud.rs for semantic clarity: invites are an auth/access
//! concern, not a CRUD concern.

use tracing::instrument;

use fe_query::{Filter, QueryBuilder, UpdateBuilder};

use crate::query_helpers::exec_query;
use crate::repo::Db;

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

    anyhow::ensure!(
        payload.len() <= crate::join_invite::MAX_INVITE_BYTES,
        "Invitation exceeds its size limit"
    );
    if let Ok(scoped) = crate::scoped_invite::ScopedInvite::from_invite_string(payload) {
        crate::scoped_invite::validate_scoped_invite(&scoped)
            .map_err(|_| anyhow::anyhow!("Invalid older scoped invitation"))?;
    } else {
        let invitation = crate::invite::VerseInvite::from_invite_string(payload)
            .map_err(|_| anyhow::anyhow!("Invalid or unsupported invitation"))?;
        let public = hex::decode(&invitation.creator_node_addr)
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .and_then(|bytes| ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok())
            .ok_or_else(|| anyhow::anyhow!("Invalid older invitation issuer"))?;
        anyhow::ensure!(
            invitation.verify(&public) && !invitation.is_expired(),
            "Invalid or expired older invitation"
        );
        crate::join_invite::canonical_did(&invitation.owner_did)?;
    }
    let _ = (db, local_did, secret_store);
    anyhow::bail!("This older invitation has no verified recipient, role and replica bootstrap. Ask the Verse Owner for a new addressed invitation.")
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
            "created_at": CREATED_AT, "namespace_id": hex::encode(crate::derive_namespace_id(&[23u8;32])),
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

    struct AddressedFixture {
        source: Db,
        signer: fe_identity::NodeKeypair,
        owner: String,
        recipient: String,
        credential: fe_runtime::join_invite::JoinCredential,
        grant: crate::ReplicationEvent,
        grant_bytes: Vec<u8>,
    }

    async fn addressed_fixture(role: &str) -> AddressedFixture {
        crate::op_log::init_hlc(0);
        let owner_key = fe_identity::NodeKeypair::generate();
        let owner = owner_key.to_did_key();
        let recipient = fe_identity::NodeKeypair::generate().to_did_key();
        let source = managers_store(&owner).await;
        let target = fe_runtime::entity_access::EntityAccessTarget {
            entity_type: fe_runtime::messages::EntityType::Verse,
            entity_id: VERSE.into(),
            scope: format!("VERSE#{VERSE}"),
        };
        let prepared =
            super::super::addressed_join::prepare(&source, &owner, &target, &recipient, role, 1)
                .await
                .unwrap();
        let memory = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let blobs: crate::BlobStoreHandle = memory.clone();
        let (replication, rx) = crossbeam::channel::bounded(8);
        // This native grammar fixture is separate from actual network convergence.
        let secret = iroh_docs::NamespaceSecret::from_bytes(&[23u8; 32]);
        let capability = if role == "viewer" {
            iroh_docs::Capability::Read(secret.id())
        } else {
            iroh_docs::Capability::Write(secret)
        };
        let peer = iroh::EndpointAddr::new(owner_key.to_iroh_secret().public())
            .with_ip_addr("127.0.0.1:40000".parse().unwrap());
        let native_only_fixture = fe_runtime::join_invite::JoinCredential::new(
            iroh_docs::ticket::DocTicket::new(capability, vec![peer]).to_string(),
        )
        .unwrap();
        let issued = super::super::addressed_join::issue(
            &source,
            &owner,
            Some(&owner_key),
            &prepared,
            &native_only_fixture,
            &blobs,
            Some(&replication),
        )
        .await
        .unwrap();
        let grant = rx
            .try_iter()
            .find(|event| event.table == "role")
            .expect("ordinary issuer role publication");
        let grant_bytes = memory.bytes_for(&grant.content_hash).unwrap();
        AddressedFixture {
            source,
            signer: owner_key,
            owner,
            recipient,
            credential: issued.credential,
            grant,
            grant_bytes,
        }
    }

    #[tokio::test]
    async fn an_invitee_records_the_real_owner_and_admits_the_owners_rows_as_owner() {
        for role in ["viewer", "editor", "manager"] {
            let fixture = addressed_fixture(role).await;
            let invitee = mem_db().await;
            let admitted = super::super::addressed_join::admit(
                &invitee,
                &fixture.recipient,
                &fixture.credential,
            )
            .await
            .unwrap();
            assert_eq!(admitted.scope, format!("VERSE#{VERSE}"));
            let pinned = verse_row(&invitee).await;
            assert_eq!(pinned["created_by"], fixture.owner);
            assert_eq!(pinned["created_at"], CREATED_AT);
            assert!(
                !super::super::addressed_join::ready(
                    &invitee,
                    &fixture.recipient,
                    &fixture.credential
                )
                .await
                .unwrap(),
                "public default Viewer is not issuance grant convergence"
            );
            let applied = apply_replicated_row_handler(
                &invitee,
                VERSE,
                "role",
                &fixture.grant.record_id,
                &fixture.grant_bytes,
                &fixture.owner,
                None,
            )
            .await
            .unwrap();
            assert_eq!(applied, ReplicatedRowOutcome::Applied);
            assert!(super::super::addressed_join::ready(
                &invitee,
                &fixture.recipient,
                &fixture.credential
            )
            .await
            .unwrap());
            assert_eq!(
                crate::role_manager::resolve_role(&invitee, &fixture.recipient, &admitted.scope)
                    .await
                    .unwrap()
                    .to_string(),
                role
            );
            let initial = verse_row(&invitee).await;
            super::super::addressed_join::admit(&invitee, &fixture.recipient, &fixture.credential)
                .await
                .unwrap();
            assert_eq!(
                verse_row(&invitee).await,
                initial,
                "same signed admission is idempotent"
            );
            let members: Vec<serde_json::Value> = invitee
                .query("SELECT invited_by FROM verse_member")
                .await
                .unwrap()
                .take(0)
                .unwrap();
            assert_eq!(members.len(), 1);
            assert_eq!(members[0]["invited_by"], fixture.owner);
            let stranger = fe_identity::NodeKeypair::generate().to_did_key();
            let empty = mem_db().await;
            assert!(
                super::super::addressed_join::admit(&empty, &stranger, &fixture.credential)
                    .await
                    .is_err()
            );
            let absent: Vec<serde_json::Value> = empty
                .query("SELECT * FROM verse")
                .await
                .unwrap()
                .take(0)
                .unwrap();
            assert!(absent.is_empty());
            let source_grants: Vec<serde_json::Value> = fixture
                .source
                .query("SELECT hlc FROM role_grant")
                .await
                .unwrap()
                .take(0)
                .unwrap();
            assert_eq!(
                source_grants.len(),
                1,
                "recipient did not manufacture issuer statements"
            );
            let memory = std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
            let blobs: crate::BlobStoreHandle = memory.clone();
            let (tx, rx) = crossbeam::channel::bounded(8);
            crate::role_manager::revoke_role_checked(
                &fixture.source,
                &fixture.owner,
                &fixture.recipient,
                &admitted.scope,
                Some(&tx),
                &blobs,
            )
            .await
            .unwrap();
            let revoked = rx.try_iter().find(|event| event.table == "role").unwrap();
            apply_replicated_row_handler(
                &invitee,
                VERSE,
                "role",
                &revoked.record_id,
                &memory.bytes_for(&revoked.content_hash).unwrap(),
                &fixture.owner,
                None,
            )
            .await
            .unwrap();
            assert!(
                !super::super::addressed_join::ready(
                    &invitee,
                    &fixture.recipient,
                    &fixture.credential
                )
                .await
                .unwrap(),
                "a signed invitation cannot override current issuer revocation"
            );
        }
    }

    #[tokio::test]
    async fn a_pinned_owner_rejects_a_first_manifest_naming_another_owner() {
        let fixture = addressed_fixture("viewer").await;
        let invitee = mem_db().await;
        super::super::addressed_join::admit(&invitee, &fixture.recipient, &fixture.credential)
            .await
            .unwrap();
        let baseline = verse_row(&invitee).await;
        let impostor = fe_identity::NodeKeypair::generate().to_did_key();
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
        assert_eq!(verse_row(&invitee).await, baseline);
        let foreign_namespace = "ff".repeat(32);
        for (field, value) in [
            ("created_by", impostor.as_str()),
            ("created_at", "2020-01-01T00:00:00Z"),
            ("namespace_id", foreign_namespace.as_str()),
        ] {
            invitee
                .query(format!(
                    "UPDATE verse SET {field}=$changed WHERE verse_id=$verse"
                ))
                .bind(("changed", value.to_owned()))
                .bind(("verse", VERSE.to_owned()))
                .await
                .unwrap()
                .check()
                .unwrap();
            let changed = verse_row(&invitee).await;
            assert!(super::super::addressed_join::admit(
                &invitee,
                &fixture.recipient,
                &fixture.credential
            )
            .await
            .is_err());
            assert_eq!(
                verse_row(&invitee).await,
                changed,
                "identity conflict never rebinds {field}"
            );
            invitee
                .query(format!(
                    "UPDATE verse SET {field}=$original WHERE verse_id=$verse"
                ))
                .bind(("original", baseline[field].clone()))
                .bind(("verse", VERSE.to_owned()))
                .await
                .unwrap()
                .check()
                .unwrap();
        }
        let rollback = mem_db().await;
        rollback
            .query(
                "DEFINE FIELD OVERWRITE invite_timestamp ON verse_member TYPE string ASSERT false",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(super::super::addressed_join::admit(
            &rollback,
            &fixture.recipient,
            &fixture.credential
        )
        .await
        .is_err());
        let absent: Vec<serde_json::Value> = rollback
            .query("SELECT * FROM verse")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(
            absent.is_empty(),
            "membership failure rolls back the initial pinned Verse"
        );

        let target = fe_runtime::entity_access::EntityAccessTarget {
            entity_type: fe_runtime::messages::EntityType::Verse,
            entity_id: VERSE.into(),
            scope: format!("VERSE#{VERSE}"),
        };
        let next_peer = fe_identity::NodeKeypair::generate().to_did_key();
        let prepared = super::super::addressed_join::prepare(
            &fixture.source,
            &fixture.owner,
            &target,
            &next_peer,
            "viewer",
            1,
        )
        .await
        .unwrap();
        let manifest: crate::join_invite::JoinManifest =
            serde_json::from_value(prepared.manifest.clone()).unwrap();
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 60;
        let captured = super::super::entity::prepare_scope_permission(
            &fixture.source,
            &target.scope,
            &fixture.owner,
            fe_policy::RoleLevel::Owner,
        )
        .await
        .unwrap()
        .with_invite_manifest(&manifest, deadline);
        fixture
            .source
            .query("UPDATE verse SET name='Changed after preparation' WHERE verse_id=$verse")
            .bind(("verse", VERSE.to_owned()))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            captured
                .read_rows(&fixture.source, "SELECT verse_id FROM verse", vec![])
                .await
                .is_err(),
            "actual transaction source fence rejects use after capture"
        );
        let counts: Vec<serde_json::Value> = fixture
            .source
            .query("SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        let blobs: crate::BlobStoreHandle =
            std::sync::Arc::new(fe_runtime::blob_store::mock::MockBlobStore::new());
        let verified = crate::join_invite::JoinInvitation::parse_owner(
            fixture.credential.as_str(),
            &fixture.recipient,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap();
        let ticket =
            fe_runtime::join_invite::JoinCredential::new(verified.ticket().to_owned()).unwrap();
        assert!(super::super::addressed_join::issue(
            &fixture.source,
            &fixture.owner,
            Some(&fixture.signer),
            &prepared,
            &ticket,
            &blobs,
            None
        )
        .await
        .is_err());
        let after: Vec<serde_json::Value> = fixture
            .source
            .query("SELECT count() AS n FROM op_log GROUP ALL")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert_eq!(
            after, counts,
            "refused source does not issue another role statement"
        );
        fixture
            .source
            .query("UPDATE verse SET name='Shared' WHERE verse_id=$verse")
            .bind(("verse", VERSE.to_owned()))
            .await
            .unwrap()
            .check()
            .unwrap();
        let expired = super::super::entity::prepare_scope_permission(
            &fixture.source,
            &target.scope,
            &fixture.owner,
            fe_policy::RoleLevel::Owner,
        )
        .await
        .unwrap()
        .with_invite_manifest(&manifest, 1);
        assert!(
            expired
                .read_rows(&fixture.source, "SELECT verse_id FROM verse", vec![])
                .await
                .is_err(),
            "transaction uses current database time for preparation expiry"
        );

        let manager = fe_identity::NodeKeypair::generate();
        let legacy =
            generate_verse_invite_handler(&fixture.source, VERSE, false, 1, Some(&manager), None)
                .await
                .unwrap();
        let empty = mem_db().await;
        assert!(
            join_verse_by_invite_handler(&empty, &legacy, &fixture.recipient, None)
                .await
                .is_err(),
            "bootstrap-incomplete legacy links refuse honestly"
        );
        let absent: Vec<serde_json::Value> = empty
            .query("SELECT * FROM verse")
            .await
            .unwrap()
            .take(0)
            .unwrap();
        assert!(absent.is_empty());
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
