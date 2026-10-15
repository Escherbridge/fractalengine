//! One-shot legacy `role`-row migration to per-grantor role statements (RP-2 step 7) —
//! see src/AGENTS.md §role-propagation "Migration".

use std::collections::HashMap;

use fe_policy::{GrantedRole, RoleStatement};

use crate::op_log;
use crate::repo::Db;
use crate::role_grants::{admit_local_statement, is_interim_authority_verse_id};
use crate::types::{NodeId, OpLogEntry, OpType};

/// The `local_migration_marker` key this migration writes once it has run.
pub const ROLE_MIGRATION_MARKER: &str = "role_grants_v1";

/// What one run of [`migrate_legacy_role_rows`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoleMigrationReport {
    /// The marker was already present: nothing ran.
    pub already_applied: bool,
    /// `role = "owner"` rows dropped (every implicit petal-creator row, R7).
    pub dropped_owner_rows: usize,
    /// Explicit rows in a verse this node owns, re-issued as owner statements.
    pub converted_to_statements: usize,
    /// Every other legacy row in a local verse (foreign owner, unknown role, owner target).
    pub dropped_other_rows: usize,
}

async fn marker_present(db: &Db) -> anyhow::Result<bool> {
    let mut res = db
        .query("SELECT marker_key FROM local_migration_marker WHERE marker_key = $k LIMIT 1")
        .bind(("k", ROLE_MIGRATION_MARKER))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role migration marker read failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
}

/// Clear the marker so the next startup re-runs the migration (admin loading a legacy dump).
pub async fn clear_marker(db: &Db) -> anyhow::Result<()> {
    db.query("DELETE local_migration_marker WHERE marker_key = $k")
        .bind(("k", ROLE_MIGRATION_MARKER))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role migration marker clear failed: {e}"))?;
    Ok(())
}

/// Convert this store's legacy `role` rows once (marker-guarded, crash-safe to re-run):
/// drop owner rows, re-issue the explicit rows of verses this node owns as owner
/// statements (op-logged, so their HLC survives restarts), drop every other legacy row.
/// Touches ONLY rows whose scope parses to an existing, non-interim verse (AC-45, T18).
/// The caller repartitions afterwards; the startup re-emit publishes the new statements.
pub async fn migrate_legacy_role_rows(
    db: &Db,
    local_did: &str,
) -> anyhow::Result<RoleMigrationReport> {
    if marker_present(db).await? {
        return Ok(RoleMigrationReport {
            already_applied: true,
            ..Default::default()
        });
    }

    let mut res = db
        .query("SELECT verse_id, created_by FROM verse")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role migration verse read failed: {e}"))?;
    let verse_rows: Vec<serde_json::Value> = res.take(0)?;
    let owners: HashMap<String, String> = verse_rows
        .iter()
        .filter_map(|row| {
            Some((
                row.get("verse_id")?.as_str()?.to_string(),
                row.get("created_by")?.as_str()?.to_string(),
            ))
        })
        .collect();

    let mut res = db
        .query("SELECT peer_did, scope, role FROM role")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role migration row read failed: {e}"))?;
    let legacy_rows: Vec<serde_json::Value> = res.take(0)?;

    let mut report = RoleMigrationReport::default();
    for row in legacy_rows {
        let (Some(peer_did), Some(scope), Some(role)) = (
            row.get("peer_did").and_then(serde_json::Value::as_str),
            row.get("scope").and_then(serde_json::Value::as_str),
            row.get("role").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let Ok(parts) = crate::parse_scope(scope) else {
            continue; // not a hierarchy scope (e.g. an interim `-RESOURCE#` key): untouched
        };
        if is_interim_authority_verse_id(&parts.verse_id) {
            continue;
        }
        let Some(owner) = owners.get(&parts.verse_id) else {
            continue; // no such local verse: untouched (only verse-confined rows move)
        };

        if role == "owner" {
            report.dropped_owner_rows += 1;
        } else if owner == local_did && peer_did != owner {
            match GrantedRole::parse(role) {
                Some(granted) => {
                    convert_to_owner_statement(
                        db,
                        &parts.verse_id,
                        owner,
                        scope,
                        peer_did,
                        granted,
                    )
                    .await?;
                    report.converted_to_statements += 1;
                }
                None => {
                    tracing::warn!(
                        scope,
                        peer_did,
                        role,
                        "dropping legacy role row with an unknown role"
                    );
                    report.dropped_other_rows += 1;
                }
            }
        } else {
            tracing::warn!(
                scope,
                peer_did,
                role,
                "dropping legacy role row (verse not owned by this node, or it targets the owner)"
            );
            report.dropped_other_rows += 1;
        }
        // Statement first, row delete second: a crash between them re-runs safely.
        delete_row(db, scope, peer_did, role).await?;
    }

    db.query("CREATE local_migration_marker CONTENT { marker_key: $k, applied_at: $at }")
        .bind(("k", ROLE_MIGRATION_MARKER))
        .bind(("at", chrono::Utc::now().to_rfc3339()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role migration marker write failed: {e}"))?;
    Ok(report)
}

async fn convert_to_owner_statement(
    db: &Db,
    verse_id: &str,
    owner: &str,
    scope: &str,
    peer_did: &str,
    role: GrantedRole,
) -> anyhow::Result<()> {
    let entry = OpLogEntry {
        lamport_clock: 0,
        node_id: NodeId(owner.to_string()),
        op_type: OpType::AssignRole,
        payload: serde_json::json!({
            "peer_did": peer_did,
            "scope": scope,
            "role": role.as_str(),
            "grantor_did": owner,
            "revoked": false,
            "migrated": true,
        }),
        sig: "00".repeat(64),
        hlc_timestamp: String::new(),
    };
    let (verse_id, owner, scope, peer_did) = (
        verse_id.to_string(),
        owner.to_string(),
        scope.to_string(),
        peer_did.to_string(),
    );
    op_log::commit_operation(db, entry, move |lamport| async move {
        let stmt = RoleStatement {
            scope,
            peer_did,
            grantor_did: owner,
            role,
            revoked: false,
            hlc: lamport,
        };
        admit_local_statement(db, &verse_id, &stmt).await
    })
    .await
}

async fn delete_row(db: &Db, scope: &str, peer_did: &str, role: &str) -> anyhow::Result<()> {
    db.query("DELETE role WHERE scope = $s AND peer_did = $p AND role = $r")
        .bind(("s", scope.to_string()))
        .bind(("p", peer_did.to_string()))
        .bind(("r", role.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("legacy role row delete ({scope}, {peer_did}) failed: {e}"))?;
    Ok(())
}
