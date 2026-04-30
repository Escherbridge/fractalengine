//! # Reconciliation — the sole startup data-fixing mechanism
//!
//! The former `migrations.rs` module was removed because tracked-once semantics
//! don't work in P2P (peers can't coordinate "which migration ran").
//! Reconciliation rules are idempotent invariants — safe to re-run forever.
//!
//! Declarative DB reconciliation — runs every startup, converges to correct state.
//!
//! Unlike ordered migrations that run once, reconciliation rules are **invariants**:
//! they describe what the DB *should* look like, check for violations, and fix them.
//! Every rule is idempotent and safe to run repeatedly. No tracking table needed.
//!
//! This pattern works for P2P because:
//! - Peers on different client versions all converge to the same correct state
//! - New rules are additive — old clients ignore fields they don't know about
//! - No coordination needed between peers about "which migration ran"

use crate::repo::Db;

/// Ordered list of rule names. Append-only.
const RULES: &[(&str, &str)] = &[
    ("local_node_placeholder", "Replace 'local-node' placeholders with real node DID"),
    ("verse_default_access",   "Backfill default_access='viewer' on verses missing the field"),
    ("orphaned_creator",       "Claim locally-created verses with no other peers"),
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
        "verse_default_access"   => check_missing_default_access(db).await,
        "orphaned_creator"       => check_orphaned_creators(db, local_did).await,
        _ => Ok(0),
    }
}

async fn fix_rule(db: &Db, name: &str, local_did: &str) -> anyhow::Result<()> {
    match name {
        "local_node_placeholder" => fix_local_node_refs(db, local_did).await,
        "verse_default_access"   => fix_missing_default_access(db).await,
        "orphaned_creator"       => fix_orphaned_creators(db, local_did).await,
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
        .bind(("did", did.clone())).await?;
    db.query("UPDATE fractal SET owner_did = $did WHERE owner_did = 'local-node'")
        .bind(("did", did.clone())).await?;
    db.query("UPDATE petal SET node_id = $did WHERE node_id = 'local-node'")
        .bind(("did", did.clone())).await?;
    db.query("UPDATE verse_member SET peer_did = $did WHERE peer_did = 'local-node'")
        .bind(("did", did.clone())).await?;
    db.query("UPDATE role SET peer_did = $did WHERE peer_did = 'local-node'")
        .bind(("did", did.clone())).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule: Ensure all verses have default_access
// ---------------------------------------------------------------------------

async fn check_missing_default_access(db: &Db) -> anyhow::Result<usize> {
    count_where(db, "verse", "default_access IS NONE").await
}

async fn fix_missing_default_access(db: &Db) -> anyhow::Result<()> {
    db.query("UPDATE verse SET default_access = 'viewer' WHERE default_access IS NONE").await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rule: Claim locally-created verses with orphaned creator DIDs
// ---------------------------------------------------------------------------

async fn check_orphaned_creators(db: &Db, local_did: &str) -> anyhow::Result<usize> {
    let mut result = db
        .query("SELECT count() AS c FROM verse WHERE created_by != $did GROUP ALL")
        .bind(("did", local_did.to_string()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;
    Ok(extract_count(&rows))
}

async fn fix_orphaned_creators(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();

    // Find verses not owned by us
    let mut result = db
        .query("SELECT verse_id, created_by FROM verse WHERE created_by != $did")
        .bind(("did", did.clone()))
        .await?;
    let rows: Vec<serde_json::Value> = result.take(0)?;

    for row in &rows {
        let Some(verse_id) = row.get("verse_id").and_then(|v| v.as_str()) else {
            continue;
        };
        let old_did = row.get("created_by").and_then(|v| v.as_str()).unwrap_or("");

        // Only claim if no OTHER peers are members (besides old creator and us).
        let mut member_check = db
            .query(
                "SELECT count() AS c FROM verse_member \
                 WHERE verse_id = $vid AND peer_did != $old AND peer_did != $did \
                 GROUP ALL"
            )
            .bind(("vid", verse_id.to_string()))
            .bind(("old", old_did.to_string()))
            .bind(("did", did.clone()))
            .await?;
        let member_rows: Vec<serde_json::Value> = member_check.take(0)?;
        if extract_count(&member_rows) > 0 {
            continue; // Has real peers — skip
        }

        tracing::info!("Claiming verse {} from {} → {}", verse_id, old_did, local_did);

        db.query("UPDATE verse SET created_by = $did WHERE verse_id = $vid")
            .bind(("did", did.clone()))
            .bind(("vid", verse_id.to_string()))
            .await?;
        db.query("UPDATE fractal SET owner_did = $did WHERE verse_id = $vid AND owner_did = $old")
            .bind(("did", did.clone()))
            .bind(("vid", verse_id.to_string()))
            .bind(("old", old_did.to_string()))
            .await?;
        db.query("UPDATE verse_member SET peer_did = $did WHERE verse_id = $vid AND peer_did = $old")
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
