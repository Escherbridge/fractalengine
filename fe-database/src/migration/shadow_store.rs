//! SPEC-8 §5.1.3, §5.1.6 bounded shadow ledger: append-only evidence against the
//! `migration_shadow_ledger` table. The table's schema is owned by the parallel
//! `W3-db-canon-log` slice (`fe-database/src/schema.rs`) — this module writes
//! against its contractual column names without depending on its Rust type. See
//! `fe-database/src/migration/AGENTS.md` §shadow-ledger and §testing.

use crate::migration::candidate::CorrelationDisposition;
use crate::repo::Db;

/// The `migration_shadow_ledger` table name (schema defined elsewhere; see
/// module docs).
const TABLE_NAME: &str = "migration_shadow_ledger";

/// One durable row of shadow evidence to append (§5.1.3). `entry_id` and
/// `created_at` are stamped by [`append_ledger_entry`] itself, never chosen by
/// the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowLedgerEntry {
    pub run_id: String,
    pub intent_digest_hex: String,
    pub mutation_kind: String,
    /// Diagnostic-only — see `MigrationCandidateSet::run_local_correlation_id`
    /// (§5.1.3).
    pub run_local_correlation_id: String,
    /// JSON-encoded array of member `op_id`s — the canonical evidence once the
    /// canonical envelope/append layer assigns them (§5.1.3). This scaffolding
    /// does not implement that layer, so callers currently pass `"[]"` — see
    /// AGENTS.md §member-op-ids.
    pub member_op_ids_json: String,
    /// JSON-encoded array of candidate byte hashes (§5.1.4).
    pub candidate_byte_hashes_json: String,
    pub disposition: CorrelationDisposition,
}

/// A persisted ledger row, as read back by [`load_ledger_entries_for_run`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShadowLedgerRow {
    pub entry_id: String,
    pub run_id: String,
    pub intent_digest_hex: String,
    pub mutation_kind: String,
    pub run_local_correlation_id: String,
    pub member_op_ids_json: String,
    pub candidate_byte_hashes_json: String,
    pub disposition: CorrelationDisposition,
    /// RFC3339 timestamp string (house convention — see e.g. `schema::Verse::created_at`).
    pub created_at: String,
}

/// Predeclared local byte/entry/age bounds (§5.1.6). Exceeding one of these
/// makes [`append_ledger_entry`] return [`AppendOutcome::Exhausted`] — it never
/// evicts, rewrites, or merges an existing row to make room; an archival or
/// disposition action beyond that requires owner approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerBounds {
    pub max_entries_per_run: u64,
    pub max_content_bytes_per_run: u64,
    pub max_run_age: chrono::Duration,
}

impl Default for LedgerBounds {
    fn default() -> Self {
        Self {
            max_entries_per_run: 100_000,
            max_content_bytes_per_run: 256 * 1024 * 1024,
            max_run_age: chrono::Duration::days(30),
        }
    }
}

/// Which predeclared bound [`AppendOutcome::Exhausted`] hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerBoundKind {
    EntryCount,
    ContentBytes,
    RunAge,
}

/// Outcome of one [`append_ledger_entry`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendOutcome {
    Appended {
        entry_id: String,
    },
    /// A predeclared bound was exceeded; nothing was written, evicted, or merged
    /// (§5.1.6). The operator must stop the run or mark subsequent intents
    /// uncovered.
    Exhausted {
        bound: LedgerBoundKind,
        run_id: String,
    },
}

/// Append one bounded ledger row for `entry.run_id` (§5.1.3, §5.1.6). Stamps a
/// fresh `entry_id` and `created_at`. Never evicts, rewrites, or merges an
/// existing row — the only failure mode on bound exhaustion is refusing the new
/// one.
pub async fn append_ledger_entry(
    db: &Db,
    entry: ShadowLedgerEntry,
    bounds: &LedgerBounds,
) -> anyhow::Result<AppendOutcome> {
    let existing = load_ledger_entries_for_run(db, &entry.run_id).await?;

    if existing.len() as u64 + 1 > bounds.max_entries_per_run {
        return Ok(AppendOutcome::Exhausted {
            bound: LedgerBoundKind::EntryCount,
            run_id: entry.run_id,
        });
    }

    let entry_id = ulid::Ulid::new().to_string();
    let created_at = chrono::Utc::now();

    let row = ShadowLedgerRow {
        entry_id: entry_id.clone(),
        run_id: entry.run_id.clone(),
        intent_digest_hex: entry.intent_digest_hex,
        mutation_kind: entry.mutation_kind,
        run_local_correlation_id: entry.run_local_correlation_id,
        member_op_ids_json: entry.member_op_ids_json,
        candidate_byte_hashes_json: entry.candidate_byte_hashes_json,
        disposition: entry.disposition,
        created_at: created_at.to_rfc3339(),
    };

    let new_row_bytes = approx_row_bytes(&row)?;
    let mut existing_bytes: u64 = 0;
    for existing_row in &existing {
        existing_bytes += approx_row_bytes(existing_row)?;
    }
    if existing_bytes + new_row_bytes > bounds.max_content_bytes_per_run {
        return Ok(AppendOutcome::Exhausted {
            bound: LedgerBoundKind::ContentBytes,
            run_id: row.run_id,
        });
    }

    let oldest = existing
        .iter()
        .filter_map(|row| chrono::DateTime::parse_from_rfc3339(&row.created_at).ok())
        .min();
    if let Some(oldest) = oldest {
        let age = created_at.signed_duration_since(oldest.with_timezone(&chrono::Utc));
        if age > bounds.max_run_age {
            return Ok(AppendOutcome::Exhausted {
                bound: LedgerBoundKind::RunAge,
                run_id: row.run_id,
            });
        }
    }

    let val = serde_json::to_value(&row)?;
    let _: Option<serde_json::Value> = db.create(TABLE_NAME).content(val).await?;

    Ok(AppendOutcome::Appended { entry_id })
}

/// Every retained ledger row for `run_id`, oldest first (§5.1.3).
pub async fn load_ledger_entries_for_run(
    db: &Db,
    run_id: &str,
) -> anyhow::Result<Vec<ShadowLedgerRow>> {
    let sql = format!("SELECT * FROM {TABLE_NAME} WHERE run_id = $run_id ORDER BY created_at ASC");
    let mut res: surrealdb::IndexedResults =
        db.query(&sql).bind(("run_id", run_id.to_string())).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    rows.into_iter()
        .map(|v| serde_json::from_value(v).map_err(Into::into))
        .collect()
}

fn approx_row_bytes(row: &ShadowLedgerRow) -> anyhow::Result<u64> {
    Ok(serde_json::to_string(row)?.len() as u64)
}

// ---------------------------------------------------------------------------
// ShadowLedgerWriter: the DB-agnostic trait `boundary::commit_operation_dual_emit`
// depends on, so its tests can inject an in-memory fake instead of a real
// database (see AGENTS.md §testing). `SurrealShadowLedgerWriter` is the
// production adapter over the two free functions above.
// ---------------------------------------------------------------------------

/// Durable shadow-ledger append, abstracted so the dual-emit boundary can be
/// exercised with a local test double instead of a real database.
#[async_trait::async_trait]
pub trait ShadowLedgerWriter: Send + Sync {
    async fn append(&self, entry: ShadowLedgerEntry) -> anyhow::Result<AppendOutcome>;
}

/// Production [`ShadowLedgerWriter`] wired to a real `Db` handle and
/// [`append_ledger_entry`]. Not yet wired into any real handler call site —
/// that wiring is integration-wave work (see AGENTS.md §boundary).
pub struct SurrealShadowLedgerWriter<'a> {
    db: &'a Db,
    bounds: LedgerBounds,
}

impl<'a> SurrealShadowLedgerWriter<'a> {
    pub fn new(db: &'a Db, bounds: LedgerBounds) -> Self {
        Self { db, bounds }
    }
}

#[async_trait::async_trait]
impl<'a> ShadowLedgerWriter for SurrealShadowLedgerWriter<'a> {
    async fn append(&self, entry: ShadowLedgerEntry) -> anyhow::Result<AppendOutcome> {
        append_ledger_entry(self.db, entry, &self.bounds).await
    }
}
