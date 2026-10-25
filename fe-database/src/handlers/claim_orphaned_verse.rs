//! Explicit, user-confirmed recovery for a verse orphaned by an identity
//! change (F6 in-app lockout root cause): `created_by` is a `did:key:`
//! identity that no longer matches this node's local DID, so
//! `role_manager::resolve_role`'s owner check falls through to
//! `default_access` (viewer). The startup `reconcile` rule
//! (`orphaned_creator`) deliberately never touches a `did:key:` creator —
//! OF-4 forbids auto-adopting one, since a replicated verse's owner is also
//! a `did:key:`. This command is the one place that is allowed to, and only
//! because it is explicit (a user clicking "Claim ownership", never
//! automatic) and scoped to a verse this store has never shared with anyone.
//! See fe-database/src/AGENTS.md §reconcile "ClaimOrphanedVerse".

use fe_runtime::messages::DbResult;

use crate::reconcile::{is_owner_identity, verse_never_shared};
use crate::repo::Db;

/// Why an explicit claim was refused, before any write. OF-4 stays dead
/// regardless of which reason fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimRefusalReason {
    /// No verse with this id exists.
    VerseNotFound,
    /// `created_by` already reads the local DID — nothing to claim.
    AlreadyOwnedLocally,
    /// `created_by` is not a `did:key:` identity (the startup `reconcile`
    /// rule already claims this shape; this command only handles the one it
    /// cannot).
    CreatorNotDidKey,
    /// A `verse_member` row shows an invite from someone other than the
    /// joining peer — this verse was shared at some point.
    HasBeenShared,
    /// A `role_grant` or `role_statement_pending` row names a grantor other
    /// than the stale creator — a role was issued in this verse by someone
    /// who is not just the orphaned creator talking to themself.
    HasForeignRoleGrant,
}

impl std::fmt::Display for ClaimRefusalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimRefusalReason::VerseNotFound => write!(f, "verse not found"),
            ClaimRefusalReason::AlreadyOwnedLocally => {
                write!(f, "this node already owns the verse")
            }
            ClaimRefusalReason::CreatorNotDidKey => write!(
                f,
                "the verse's creator is not a did:key identity (not an orphaned-identity case)"
            ),
            ClaimRefusalReason::HasBeenShared => {
                write!(f, "the verse has been shared with another peer")
            }
            ClaimRefusalReason::HasForeignRoleGrant => write!(
                f,
                "a role was granted in this verse by someone other than the stale creator"
            ),
        }
    }
}

/// Outcome of attempting the claim: either it landed, or it was refused
/// before any write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed { old_did: String },
    Refused { reason: ClaimRefusalReason },
}

async fn fetch_created_by(db: &Db, verse_id: &str) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT created_by FROM verse WHERE verse_id = $vid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|r| r.get("created_by"))
        .and_then(|v| v.as_str())
        .map(str::to_string))
}

/// Whether any `role_grant`/`role_statement_pending` row in `verse_id` names
/// a grantor other than `old_did` — a role issued by anyone but the stale
/// creator means this verse's role fabric is not purely local-to-self.
async fn has_foreign_role_grant(db: &Db, verse_id: &str, old_did: &str) -> anyhow::Result<bool> {
    let mut res = db
        .query(
            "SELECT count() AS c FROM role_grant \
             WHERE verse_id = $vid AND grantor_did != $old GROUP ALL;
             SELECT count() AS c FROM role_statement_pending \
             WHERE verse_id = $vid AND grantor_did != $old GROUP ALL;",
        )
        .bind(("vid", verse_id.to_string()))
        .bind(("old", old_did.to_string()))
        .await?;
    let grant_rows: Vec<serde_json::Value> = res.take(0)?;
    let pending_rows: Vec<serde_json::Value> = res.take(1)?;
    Ok(extract_count(&grant_rows) > 0 || extract_count(&pending_rows) > 0)
}

fn extract_count(rows: &[serde_json::Value]) -> usize {
    rows.first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize
}

/// Attempt the claim. Every refusal check runs before any write — the
/// `UPDATE`s below are the exact shape `reconcile::fix_orphaned_creators`
/// uses for the auto-claimed (non-`did:key:`) case.
pub(crate) async fn claim_orphaned_verse_handler(
    db: &Db,
    local_did: &str,
    verse_id: &str,
) -> anyhow::Result<ClaimOutcome> {
    let Some(old_did) = fetch_created_by(db, verse_id).await? else {
        return Ok(ClaimOutcome::Refused {
            reason: ClaimRefusalReason::VerseNotFound,
        });
    };
    if old_did == local_did {
        return Ok(ClaimOutcome::Refused {
            reason: ClaimRefusalReason::AlreadyOwnedLocally,
        });
    }
    if !is_owner_identity(&old_did) {
        return Ok(ClaimOutcome::Refused {
            reason: ClaimRefusalReason::CreatorNotDidKey,
        });
    }
    if !verse_never_shared(db, verse_id).await? {
        return Ok(ClaimOutcome::Refused {
            reason: ClaimRefusalReason::HasBeenShared,
        });
    }
    if has_foreign_role_grant(db, verse_id, &old_did).await? {
        return Ok(ClaimOutcome::Refused {
            reason: ClaimRefusalReason::HasForeignRoleGrant,
        });
    }

    db.query("UPDATE verse SET created_by = $did WHERE verse_id = $vid")
        .bind(("did", local_did.to_string()))
        .bind(("vid", verse_id.to_string()))
        .await?;
    db.query("UPDATE fractal SET owner_did = $did WHERE verse_id = $vid AND owner_did = $old")
        .bind(("did", local_did.to_string()))
        .bind(("vid", verse_id.to_string()))
        .bind(("old", old_did.to_string()))
        .await?;
    db.query("UPDATE verse_member SET peer_did = $did WHERE verse_id = $vid AND peer_did = $old")
        .bind(("did", local_did.to_string()))
        .bind(("vid", verse_id.to_string()))
        .bind(("old", old_did.to_string()))
        .await?;

    tracing::info!(
        verse_id,
        old_did,
        new_did = local_did,
        "explicitly claimed an orphaned verse (ClaimOrphanedVerse)"
    );
    Ok(ClaimOutcome::Claimed { old_did })
}

/// The DB thread's reply to `ClaimOrphanedVerse`: `OrphanedVerseClaimed` on
/// success, `OrphanedVerseClaimRefused` (typed reason, never a hard error)
/// when a guard refuses it, the usual correlated/wildcard failure only for a
/// genuine DB error. See fe-runtime src/AGENTS.md §api-reply-correlation.
pub async fn claim_orphaned_verse_reply(
    db: &Db,
    local_did: &str,
    verse_id: String,
    correlation_id: Option<String>,
) -> DbResult {
    match claim_orphaned_verse_handler(db, local_did, &verse_id).await {
        Ok(ClaimOutcome::Claimed { old_did }) => DbResult::OrphanedVerseClaimed {
            verse_id,
            old_did,
            new_did: local_did.to_string(),
            correlation_id,
        },
        Ok(ClaimOutcome::Refused { reason }) => DbResult::OrphanedVerseClaimRefused {
            verse_id,
            reason: reason.to_string(),
            correlation_id,
        },
        Err(e) => super::correlated_failure(
            &correlation_id,
            format!("ClaimOrphanedVerse failed: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_level::RoleLevel;

    async fn mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    async fn create_verse(db: &Db, verse_id: &str, created_by: &str) {
        db.query(
            "CREATE verse CONTENT { verse_id: $vid, name: 'V', created_by: $cb, \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' }",
        )
        .bind(("vid", verse_id.to_string()))
        .bind(("cb", created_by.to_string()))
        .await
        .expect("create verse")
        .check()
        .expect("create verse");
    }

    /// Reproduces the F6 lockout: a verse created under `did_before`, never
    /// shared. After an identity change, `resolve_role(did_after)` falls to
    /// the verse default (Viewer) while `resolve_role(did_before)` is still
    /// Owner — the data is intact, merely orphaned.
    #[tokio::test]
    async fn orphaned_identity_locks_out_the_new_did_but_data_is_intact() {
        let db = mem_db().await;
        let did_before = fe_identity::NodeKeypair::generate().to_did_key();
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", &did_before).await;
        let scope = crate::scope::build_scope("v1", None, None);

        assert_eq!(
            crate::role_manager::resolve_role(&db, &did_after, &scope)
                .await
                .unwrap(),
            RoleLevel::Viewer,
            "the new identity resolves to the verse default, not Owner"
        );
        assert_eq!(
            crate::role_manager::resolve_role(&db, &did_before, &scope)
                .await
                .unwrap(),
            RoleLevel::Owner,
            "the old identity is still live-resolved as Owner — data is intact"
        );
    }

    /// The never-shared, did:key-orphaned verse claims successfully, and the
    /// new identity resolves Owner immediately (no cache to rematerialize —
    /// `resolve_role` compares `created_by` live).
    #[tokio::test]
    async fn claim_succeeds_on_a_never_shared_orphaned_verse() {
        let db = mem_db().await;
        let did_before = fe_identity::NodeKeypair::generate().to_did_key();
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", &did_before).await;

        let outcome = claim_orphaned_verse_handler(&db, &did_after, "v1")
            .await
            .expect("claim handler ok");
        assert_eq!(
            outcome,
            ClaimOutcome::Claimed {
                old_did: did_before.clone()
            }
        );

        let scope = crate::scope::build_scope("v1", None, None);
        assert_eq!(
            crate::role_manager::resolve_role(&db, &did_after, &scope)
                .await
                .unwrap(),
            RoleLevel::Owner,
            "the claiming identity is Owner immediately after the claim"
        );
    }

    /// OF-4-shaped negative: a `verse_member` row shows an invite from a
    /// THIRD did (i.e. the verse was shared), so the claim is refused even
    /// though the creator is a stale `did:key:`.
    #[tokio::test]
    async fn claim_is_refused_when_a_third_party_was_ever_invited() {
        let db = mem_db().await;
        let did_before = fe_identity::NodeKeypair::generate().to_did_key();
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        let third_party = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", &did_before).await;
        db.query(
            "CREATE verse_member CONTENT { member_id: 'm1', verse_id: 'v1', \
             peer_did: $peer, status: 'active', invited_by: $inviter, invite_sig: 's', \
             invite_timestamp: '2026-10-15T00:00:00Z' }",
        )
        .bind(("peer", third_party.clone()))
        .bind(("inviter", did_before.clone()))
        .await
        .expect("create verse_member")
        .check()
        .expect("create verse_member");

        let outcome = claim_orphaned_verse_handler(&db, &did_after, "v1")
            .await
            .expect("claim handler ok");
        assert_eq!(
            outcome,
            ClaimOutcome::Refused {
                reason: ClaimRefusalReason::HasBeenShared
            }
        );
    }

    /// A verse never claimed because the creator is not a did:key (reuses
    /// `is_owner_identity`, same as the startup reconcile rule) is refused
    /// here too — this command is only for the did:key-orphaned shape.
    #[tokio::test]
    async fn claim_is_refused_when_the_creator_is_not_a_did_key() {
        let db = mem_db().await;
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", "legacy-placeholder").await;

        let outcome = claim_orphaned_verse_handler(&db, &did_after, "v1")
            .await
            .expect("claim handler ok");
        assert_eq!(
            outcome,
            ClaimOutcome::Refused {
                reason: ClaimRefusalReason::CreatorNotDidKey
            }
        );
    }

    /// A `role_grant` row naming a grantor other than the stale creator
    /// means someone else already granted a role in this verse — refused.
    #[tokio::test]
    async fn claim_is_refused_when_a_foreign_role_grant_exists() {
        let db = mem_db().await;
        let did_before = fe_identity::NodeKeypair::generate().to_did_key();
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        let grantor = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", &did_before).await;
        db.query(
            "CREATE role_grant CONTENT { verse_id: 'v1', scope: 'VERSE#v1', \
             peer_did: 'someone', grantor_did: $grantor, role: 'editor', \
             revoked: false, hlc: '1', updated_at: '2026-10-15T00:00:00Z', \
             statement_key: 'k1' }",
        )
        .bind(("grantor", grantor))
        .await
        .expect("create role_grant")
        .check()
        .expect("create role_grant");

        let outcome = claim_orphaned_verse_handler(&db, &did_after, "v1")
            .await
            .expect("claim handler ok");
        assert_eq!(
            outcome,
            ClaimOutcome::Refused {
                reason: ClaimRefusalReason::HasForeignRoleGrant
            }
        );
    }

    /// Migration negative (existing `migrate_legacy_role_rows`, R7): a
    /// legacy `role` row with `role = "owner"` is dropped unconditionally,
    /// and owner resolution — which never reads that row — is unchanged
    /// before and after the claim + migration.
    #[tokio::test]
    async fn legacy_owner_role_row_migrates_away_without_touching_owner_resolution() {
        let db = mem_db().await;
        crate::op_log::init_hlc(0);
        let did_before = fe_identity::NodeKeypair::generate().to_did_key();
        let did_after = fe_identity::NodeKeypair::generate().to_did_key();
        create_verse(&db, "v1", &did_before).await;
        let scope = crate::scope::build_scope("v1", None, None);
        db.query(
            "CREATE role CONTENT { peer_did: $peer, scope: $scope, role: 'owner' }",
        )
        .bind(("peer", did_before.clone()))
        .bind(("scope", scope.clone()))
        .await
        .expect("seed legacy role row")
        .check()
        .expect("seed legacy role row");

        let outcome = claim_orphaned_verse_handler(&db, &did_after, "v1")
            .await
            .expect("claim handler ok");
        assert_eq!(
            outcome,
            ClaimOutcome::Claimed {
                old_did: did_before.clone()
            }
        );
        assert_eq!(
            crate::role_manager::resolve_role(&db, &did_after, &scope)
                .await
                .unwrap(),
            RoleLevel::Owner,
            "owner resolution is already live via created_by before migration runs"
        );

        let report = crate::migration::role_grants::migrate_legacy_role_rows(&db, &did_after)
            .await
            .expect("migration ok");
        assert_eq!(report.dropped_owner_rows, 1, "the legacy owner row is dropped");

        let remaining: Vec<serde_json::Value> = db
            .query("SELECT * FROM role WHERE scope = $scope")
            .bind(("scope", scope.clone()))
            .await
            .expect("query role")
            .take(0)
            .expect("rows");
        assert!(remaining.is_empty(), "legacy role row is gone after migration");
        assert_eq!(
            crate::role_manager::resolve_role(&db, &did_after, &scope)
                .await
                .unwrap(),
            RoleLevel::Owner,
            "owner resolution unchanged by the migration — it never read that row"
        );
    }
}
