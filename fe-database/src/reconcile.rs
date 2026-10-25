//! Declarative idempotent DB reconciliation — the sole startup data-fixing
//! mechanism (see fe-database/src/AGENTS.md §reconcile).

use crate::repo::Db;

/// Ordered list of rule names. Append-only.
const RULES: &[(&str, &str)] = &[
    (
        "local_node_placeholder",
        "Replace 'local-node' placeholders with real node DID",
    ),
    (
        "verse_default_access",
        "Backfill default_access='viewer' on verses missing the field",
    ),
    (
        "orphaned_creator",
        "Claim locally-created verses whose creator is a non-did:key placeholder",
    ),
];

/// Run all reconciliation rules. Called on every startup after schema apply.
pub async fn reconcile(db: &Db, local_did: &str) -> anyhow::Result<usize> {
    let mut fixed_count = 0;

    for (name, description) in RULES {
        let violations = check_rule(db, name, local_did).await.unwrap_or_else(|e| {
            tracing::error!("Reconcile [{name}]: check failed: {e}");
            0
        });

        if violations == 0 {
            continue;
        }

        tracing::info!("Reconcile [{name}]: {violations} violation(s) — {description}");

        match fix_rule(db, name, local_did).await {
            Ok(()) => {
                fixed_count += 1;
                tracing::info!("Reconcile [{name}]: fixed");
            }
            Err(e) => {
                tracing::error!("Reconcile [{name}]: fix failed: {e}");
            }
        }
    }

    if fixed_count > 0 {
        tracing::info!("Reconciliation complete: {fixed_count} rule(s) applied fixes");
    }

    Ok(fixed_count)
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

async fn check_rule(db: &Db, name: &str, local_did: &str) -> anyhow::Result<usize> {
    match name {
        "local_node_placeholder" => check_local_node_refs(db).await,
        "verse_default_access" => check_missing_default_access(db).await,
        "orphaned_creator" => check_orphaned_creators(db, local_did).await,
        _ => Ok(0),
    }
}

async fn fix_rule(db: &Db, name: &str, local_did: &str) -> anyhow::Result<()> {
    match name {
        "local_node_placeholder" => fix_local_node_refs(db, local_did).await,
        "verse_default_access" => fix_missing_default_access(db).await,
        "orphaned_creator" => fix_orphaned_creators(db, local_did).await,
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Rule: Replace "local-node" placeholders with real DID
// ---------------------------------------------------------------------------

async fn check_local_node_refs(db: &Db) -> anyhow::Result<usize> {
    let c = count_where(db, "verse", "created_by = 'local-node'").await?
        + count_where(db, "fractal", "owner_did = 'local-node'").await?
        + count_where(db, "petal", "node_id = 'local-node'").await?
        + count_where(db, "verse_member", "peer_did = 'local-node'").await?
        + count_where(db, "role", "peer_did = 'local-node'").await?;
    Ok(c)
}

async fn fix_local_node_refs(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();
    db.query("UPDATE verse SET created_by = $did WHERE created_by = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    db.query("UPDATE fractal SET owner_did = $did WHERE owner_did = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    db.query("UPDATE petal SET node_id = $did WHERE node_id = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    db.query("UPDATE verse_member SET peer_did = $did WHERE peer_did = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    db.query("UPDATE role SET peer_did = $did WHERE peer_did = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule: Ensure all verses have default_access
// ---------------------------------------------------------------------------

async fn check_missing_default_access(db: &Db) -> anyhow::Result<usize> {
    count_where(db, "verse", "default_access IS NONE").await
}

async fn fix_missing_default_access(db: &Db) -> anyhow::Result<()> {
    db.query("UPDATE verse SET default_access = 'viewer' WHERE default_access IS NONE")
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule: Claim locally-created verses with orphaned creator DIDs
// ---------------------------------------------------------------------------

/// Whether `creator` is an owner identity `orphaned_creator` never claims. The
/// inbound first-manifest gate admits ONLY such creators (OF-4), so a
/// replicated manifest can never be claimed here (src/AGENTS.md §reconcile).
pub(crate) fn is_owner_identity(creator: &str) -> bool {
    creator.starts_with("did:key:")
}

/// Verses this rule may claim (role propagation RP-4, owner integrity — see
/// src/AGENTS.md §reconcile): the creator is NOT a `did:key:` identity
/// (a legacy placeholder or a pre-RP-4 hex inviter address — a real owner is
/// always a did:key, and a replicated or invite-pinned verse's owner is never
/// claimed), and no invite ever joined this store to the verse (a
/// `verse_member` row whose inviter is someone else).
async fn claimable_verses(db: &Db, local_did: &str) -> anyhow::Result<Vec<(String, String)>> {
    let mut result = db
        .query("SELECT verse_id, created_by FROM verse WHERE created_by != $did")
        .bind(("did", local_did.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    let mut claimable = Vec::new();
    for row in &rows {
        let Some(verse_id) = row.get("verse_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let creator = row.get("created_by").and_then(|v| v.as_str()).unwrap_or("");
        if is_owner_identity(creator) {
            continue;
        }
        let mut joined = db
            .query(
                "SELECT count() AS c FROM verse_member \
                 WHERE verse_id = $vid AND invited_by != peer_did GROUP ALL",
            )
            .bind(("vid", verse_id.to_string()))
            .await?;
        let joined_rows: Vec<serde_json::Value> = joined.take(0)?;
        if extract_count(&joined_rows) > 0 {
            continue;
        }
        claimable.push((verse_id.to_string(), creator.to_string()));
    }
    Ok(claimable)
}

async fn check_orphaned_creators(db: &Db, local_did: &str) -> anyhow::Result<usize> {
    Ok(claimable_verses(db, local_did).await?.len())
}

async fn fix_orphaned_creators(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();

    for (verse_id, old_did) in claimable_verses(db, local_did).await? {
        let verse_id = verse_id.as_str();
        let old_did = old_did.as_str();

        // Only claim if no OTHER peers are members (besides old creator and us).
        let mut member_check = db
            .query(
                "SELECT count() AS c FROM verse_member \
                 WHERE verse_id = $vid AND peer_did != $old AND peer_did != $did \
                 GROUP ALL",
            )
            .bind(("vid", verse_id.to_string()))
            .bind(("old", old_did.to_string()))
            .bind(("did", did.clone()))
            .await?;
        let member_rows: Vec<serde_json::Value> = member_check.take(0)?;
        if extract_count(&member_rows) > 0 {
            continue; // Has real peers — skip
        }

        tracing::info!(
            "Claiming verse {} from {} → {}",
            verse_id,
            old_did,
            local_did
        );

        db.query("UPDATE verse SET created_by = $did WHERE verse_id = $vid")
            .bind(("did", did.clone()))
            .bind(("vid", verse_id.to_string()))
            .await?;
        db.query("UPDATE fractal SET owner_did = $did WHERE verse_id = $vid AND owner_did = $old")
            .bind(("did", did.clone()))
            .bind(("vid", verse_id.to_string()))
            .bind(("old", old_did.to_string()))
            .await?;
        db.query(
            "UPDATE verse_member SET peer_did = $did WHERE verse_id = $vid AND peer_did = $old",
        )
        .bind(("did", did.clone()))
        .bind(("vid", verse_id.to_string()))
        .bind(("old", old_did.to_string()))
        .await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn count_where(db: &Db, table: &str, condition: &str) -> anyhow::Result<usize> {
    let query = format!("SELECT count() AS c FROM {table} WHERE {condition} GROUP ALL");
    let mut result = db.query(&query).await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(extract_count(&rows))
}

fn extract_count(rows: &[serde_json::Value]) -> usize {
    rows.first()
        .and_then(|r| r.get("c"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    async fn owner_of(db: &Db, verse_id: &str) -> String {
        let rows: Vec<serde_json::Value> = db
            .query("SELECT created_by FROM verse WHERE verse_id = $v")
            .bind(("v", verse_id.to_string()))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        rows[0]["created_by"].as_str().unwrap().to_string()
    }

    /// RP-4 owner integrity (the RP-3 restart finding): a verse that arrived
    /// over a replica (a remote did:key owner, no member rows) and a verse
    /// joined by invite are never claimed at startup — only a local
    /// placeholder creator is. Before the fix every replicated verse became
    /// the local node's after a restart.
    #[tokio::test]
    async fn startup_reconcile_never_claims_a_replicated_or_joined_verse() {
        let db = mem_db().await;
        let local = fe_identity::NodeKeypair::generate().to_did_key();
        let remote = fe_identity::NodeKeypair::generate().to_did_key();
        db.query(
            "CREATE verse CONTENT { verse_id: 'replicated', name: 'R', created_by: $remote, \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' };
             CREATE verse CONTENT { verse_id: 'joined-legacy', name: 'J', created_by: 'abcdef0123', \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' };
             CREATE verse_member CONTENT { member_id: 'm1', verse_id: 'joined-legacy', \
             peer_did: $local, status: 'active', invited_by: 'abcdef0123', invite_sig: 's', \
             invite_timestamp: '2026-10-15T00:00:00Z' };
             CREATE verse CONTENT { verse_id: 'placeholder', name: 'P', created_by: 'old-local', \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' };",
        )
        .bind(("remote", remote.clone()))
        .bind(("local", local.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();

        reconcile(&db, &local).await.unwrap();

        assert_eq!(owner_of(&db, "replicated").await, remote);
        assert_eq!(owner_of(&db, "joined-legacy").await, "abcdef0123");
        assert_eq!(
            owner_of(&db, "placeholder").await,
            local,
            "a local placeholder is still claimed"
        );
    }
}
