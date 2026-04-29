//! Lightweight data migration system for fractalengine.
//!
//! Each migration is an idempotent function identified by a unique string ID.
//! On startup, `run_pending_migrations` checks a `_migration` table in
//! SurrealDB and applies any migrations that haven't run yet.
//!
//! Migrations never delete user data — they fix ownership records, backfill
//! defaults, and evolve the schema non-destructively.

use crate::repo::Db;

/// A single data migration.
struct Migration {
    /// Unique identifier. Once applied, never re-run. Use format "NNN_snake_name".
    id: &'static str,
    /// Human-readable description for logging.
    description: &'static str,
}

/// Ordered list of all migrations. Append-only — never reorder or remove.
const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "001_fix_local_node_to_real_did",
        description: "Replace 'local-node' ownership strings with the node's real DID",
    },
    Migration {
        id: "002_ensure_verse_default_access",
        description: "Backfill default_access='viewer' on verses missing the field",
    },
    Migration {
        id: "003_claim_unowned_verses",
        description: "Set created_by to local DID on verses with unknown/mismatched creator",
    },
    Migration {
        id: "004_claim_orphaned_seed_did",
        description: "Reassign verses created by ephemeral seed keypairs to the real node DID",
    },
];

/// Run all pending migrations. Safe to call on every startup.
///
/// Creates the `_migration` table if it doesn't exist, then applies each
/// migration whose ID is not already recorded.
pub async fn run_pending_migrations(db: &Db, local_did: &str) -> anyhow::Result<usize> {
    // Ensure migration tracking table exists.
    db.query(
        "DEFINE TABLE IF NOT EXISTS _migration SCHEMAFULL;
         DEFINE FIELD IF NOT EXISTS migration_id ON TABLE _migration TYPE string;
         DEFINE FIELD IF NOT EXISTS applied_at   ON TABLE _migration TYPE string;
         DEFINE FIELD IF NOT EXISTS description  ON TABLE _migration TYPE string;"
    )
    .await?;

    let mut applied_count = 0;

    for migration in MIGRATIONS {
        // Check if already applied.
        let mut result = db
            .query("SELECT * FROM _migration WHERE migration_id = $id LIMIT 1")
            .bind(("id", migration.id.to_string()))
            .await?;
        let rows: Vec<serde_json::Value> = result.take(0)?;

        if !rows.is_empty() {
            continue;
        }

        tracing::info!(
            "Applying migration {}: {}",
            migration.id,
            migration.description
        );

        match apply_migration(db, migration.id, local_did).await {
            Ok(()) => {
                // Record successful application.
                let now = chrono::Utc::now().to_rfc3339();
                db.query(
                    "CREATE _migration SET migration_id = $id, description = $desc, applied_at = $now"
                )
                .bind(("id", migration.id.to_string()))
                .bind(("desc", migration.description.to_string()))
                .bind(("now", now))
                .await?;

                applied_count += 1;
                tracing::info!("Migration {} applied successfully", migration.id);
            }
            Err(e) => {
                tracing::error!("Migration {} failed: {e}", migration.id);
                // Non-fatal — log and continue so the app still starts.
            }
        }
    }

    if applied_count > 0 {
        tracing::info!("Applied {applied_count} migration(s)");
    }

    Ok(applied_count)
}

/// Dispatch a migration by ID. Each match arm is idempotent.
async fn apply_migration(db: &Db, id: &str, local_did: &str) -> anyhow::Result<()> {
    match id {
        "001_fix_local_node_to_real_did" => migrate_001_fix_local_node(db, local_did).await,
        "002_ensure_verse_default_access" => migrate_002_default_access(db).await,
        "003_claim_unowned_verses" => migrate_003_claim_unowned(db, local_did).await,
        "004_claim_orphaned_seed_did" => migrate_004_claim_orphaned_seed(db, local_did).await,
        other => {
            tracing::warn!("Unknown migration ID: {other}");
            Ok(())
        }
    }
}

/// Migration 001: Replace all "local-node" strings with the real node DID.
///
/// Fixes ownership on verses, fractals, petals, verse_members, and roles
/// that were created before the DID was wired through.
async fn migrate_001_fix_local_node(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();

    // verse.created_by
    let mut r = db
        .query("UPDATE verse SET created_by = $did WHERE created_by = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    // fractal.owner_did
    let mut r = db
        .query("UPDATE fractal SET owner_did = $did WHERE owner_did = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    // petal.node_id (creator field)
    let mut r = db
        .query("UPDATE petal SET node_id = $did WHERE node_id = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    // verse_member.peer_did (fix "local-node" and missing values)
    let mut r = db
        .query("UPDATE verse_member SET peer_did = $did WHERE peer_did = 'local-node' OR peer_did IS NONE")
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    // role.peer_did
    let mut r = db
        .query("UPDATE role SET peer_did = $did WHERE peer_did = 'local-node'")
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    tracing::info!("Migrated 'local-node' references to {}", local_did);
    Ok(())
}

/// Migration 002: Ensure all verse rows have a default_access field.
///
/// Verses created before the default_access column was added will be
/// missing it. Backfill with "viewer" (public by default).
async fn migrate_002_default_access(db: &Db) -> anyhow::Result<()> {
    db.query(
        "UPDATE verse SET default_access = 'viewer' WHERE default_access IS NONE"
    )
    .await?;

    Ok(())
}

/// Migration 003: Claim verses whose created_by doesn't match any known DID.
///
/// On a single-user local node, all verses should be owned by the local DID.
/// This migration catches verses created with ephemeral seed keypairs or
/// other temporary identities and reassigns them to the real node DID.
async fn migrate_003_claim_unowned(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();

    // Only claim verses where created_by is NOT already a did:key
    // (i.e., it's "local-node", empty, or some other placeholder).
    // Verses with a real did:key created_by from a peer should NOT be claimed.
    let mut r = db
        .query(
            "UPDATE verse SET created_by = $did \
             WHERE created_by IS NONE \
             OR created_by !~ '^did:key:'"
        )
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    // Also fix fractals and petals with non-DID ownership
    let mut r = db
        .query(
            "UPDATE fractal SET owner_did = $did \
             WHERE owner_did IS NONE \
             OR owner_did !~ '^did:key:'"
        )
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    let mut r = db
        .query(
            "UPDATE petal SET node_id = $did \
             WHERE node_id IS NONE \
             OR node_id !~ '^did:key:'"
        )
        .bind(("did", did.clone()))
        .await?;
    let _ = r.check();

    Ok(())
}

/// Migration 004: Reassign locally-created verses from ephemeral seed DIDs.
///
/// The old seed_default_data generated its own NodeKeypair and used that DID
/// as created_by. This DID is valid (did:key:z6Mk...) but doesn't match the
/// node's persistent keypair. On a single-user node, if the verse has no
/// connected peers, claim it for the real node DID.
///
/// Safety: only reassigns verses where created_by != local_did AND there are
/// no verse_members from other peers (i.e., it's a purely local verse).
async fn migrate_004_claim_orphaned_seed(db: &Db, local_did: &str) -> anyhow::Result<()> {
    let did = local_did.to_string();

    // Find all verses NOT owned by local_did
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

        // Check if any OTHER peer (besides old_did) is a member.
        // If yes, this verse has real peers — don't claim it.
        let mut member_check = db
            .query(
                "SELECT count() AS cnt FROM verse_member \
                 WHERE verse_id = $vid AND peer_did != $old_did AND peer_did != $did \
                 GROUP ALL"
            )
            .bind(("vid", verse_id.to_string()))
            .bind(("old_did", old_did.to_string()))
            .bind(("did", did.clone()))
            .await?;
        let member_rows: Vec<serde_json::Value> = member_check.take(0)?;
        let other_members = member_rows
            .first()
            .and_then(|r| r.get("cnt"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        if other_members > 0 {
            tracing::info!(
                "Skipping verse {} — has {} other member(s)",
                verse_id, other_members
            );
            continue;
        }

        // Safe to claim: update created_by and fix all old_did references
        tracing::info!(
            "Claiming verse {} from orphaned DID {} → {}",
            verse_id, old_did, local_did
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
        db.query("UPDATE verse_member SET peer_did = $did WHERE verse_id = $vid AND peer_did = $old")
            .bind(("did", did.clone()))
            .bind(("vid", verse_id.to_string()))
            .bind(("old", old_did.to_string()))
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn migration_ids_are_unique() {
        let ids: Vec<&str> = super::MIGRATIONS.iter().map(|m| m.id).collect();
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len(), "duplicate migration IDs found");
    }

    #[test]
    fn migration_ids_are_ordered() {
        let ids: Vec<&str> = super::MIGRATIONS.iter().map(|m| m.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "migrations must be in sorted order");
    }
}
