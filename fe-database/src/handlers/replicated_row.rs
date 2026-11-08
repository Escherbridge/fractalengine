//! Inbound replicated-row apply (A4) — the DB thread's single-writer
//! enforcement point for rows arriving over a verse replica.
//!
//! Loop prevention is **structural**: this handler takes no replication
//! sender, so an applied row can never re-enter the outbound bridge — the
//! dispatch arm is the only caller and it passes none either.
//!
//! A3: admission is gated here (deny-by-default): verse id → enrolment → manifest wait →
//! referential confinement → the author's role, resolved from the local tables (never the
//! wire) at the row's deepest scope — see [`admit_inbound_row`] and src/AGENTS.md §handlers
//! "replicated_row". `canon_device_enrolment` is gated on signed self-authorship plus a
//! membership floor for enrolments (`canon_log/AGENTS.md` §device-enrolment).
//!
//! Role propagation RP-3: a row that fails only because something has not converged is
//! `Deferred` (never applied now, retried by [`super::inbound_admissions`]); `role` doc
//! entries are per-grantor role statements with their own gate (`apply_role_statement`) —
//! see src/AGENTS.md §inbound-admissions.
//!
//! M2/F5: dispatch is keyed on [`ReplicationMode::for_table`] — `Row` tables
//! take the tombstone-aware merge, `Timeseries` tables (`iot_reading`) the
//! append-only union CRDT (see [`apply_timeseries_row`]).

use fe_runtime::messages::{DeferralReason, NodeDto, ReplicatedRowOutcome, SceneChange};

use crate::merge::{apply_replicated_node_by, LegacyAuthor, MergeApplied};
use crate::replication_mode::ReplicationMode;
use crate::repo::Db;

/// The DB thread's scene-change seam (entity_change_tx).
pub type SceneChangeSender = tokio::sync::broadcast::Sender<SceneChange>;

/// Apply one inbound replicated row to the durable store (A4).
///
/// The row arrives from the sync thread's inbound pump as the payload bytes
/// the authoring peer published. Geometry columns (`node.position`,
/// `petal.bounds`) are stripped before any CONTENT/MERGE bind and written
/// back with explicit SurrealQL casts (§geometry-inserts); tombstone
/// dominance is delegated to [`crate::merge::apply_replicated_node`] (N-4).
///
/// `author_did` (the entry author the transport attests) is what the A3
/// role gate resolves against the local tables — wire payloads are never
/// consulted for roles. `pub` so the test harness's
/// simplified DB loop can drive the **real** apply path against its in-memory
/// DB (the real dispatch loop lives in fe-database's own thread and is not
/// callable against `Mem` — statement-parity isn't enough here, the apply
/// logic itself must be the production one).
///
/// A `Deferred` outcome from this one-shot entry point is NOT retried: the
/// dispatch loops go through [`super::inbound_admissions::InboundAdmissions::apply`],
/// which queues deferred content rows and retries them when the awaited fact lands.
#[allow(clippy::too_many_arguments)]
pub async fn apply_replicated_row_handler(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    apply_replicated_row_reporting(
        db,
        verse_id,
        table,
        record_id,
        row_bytes,
        author_did,
        entity_change_tx,
    )
    .await
    .map(|report| report.outcome)
}

/// One inbound apply's outcome plus the retry triggers it fires for the
/// deferred-admission queue (src/AGENTS.md §inbound-admissions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    pub outcome: ReplicatedRowOutcome,
    /// DIDs whose effective role rose because of this apply (an admitted role
    /// statement and the partition it ran, or a manifest that set the owner).
    pub risen_dids: Vec<String>,
    /// The verse manifest landed with this apply (the verse row was unknown before).
    pub manifest_landed: bool,
}

impl ApplyReport {
    /// A report that fires no retry trigger.
    pub fn of(outcome: ReplicatedRowOutcome) -> Self {
        Self {
            outcome,
            risen_dids: Vec::new(),
            manifest_landed: false,
        }
    }
}

/// [`apply_replicated_row_handler`] plus the retry triggers the apply fired.
#[allow(clippy::too_many_arguments)]
pub async fn apply_replicated_row_reporting(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ApplyReport> {
    apply_replicated_row_reporting_with_publisher(db,verse_id,table,record_id,row_bytes,author_did,None,entity_change_tx).await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_replicated_row_reporting_with_publisher(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    local_publisher: Option<&str>,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ApplyReport> {
    tracing::debug!(
        verse_id,
        table,
        record_id,
        author = author_did,
        "Applying inbound replicated row"
    );
    // RP-3: role statements bypass the Editor+ write gate — their gate is the
    // fe-policy validator plus durable placement (plan §2.4).
    if table == crate::role_grants::ROLE_DOC_TABLE {
        return apply_role_statement(db, verse_id, record_id, row_bytes, author_did).await;
    }
    // The manifest-apply path must notice the owner becoming known (or
    // changing) so the role partition promotes pre-manifest holds.
    let manifest_before = if table == "verse" {
        Some((
            row_exists(db, "verse", "verse_id", verse_id).await?,
            crate::role_grants::verse_owner(db, verse_id).await?,
        ))
    } else {
        None
    };
    // A3: admission precedes dispatch — a denied or deferred row is never
    // applied and never re-emitted (there is no replication sender in scope).
    match admit_inbound_row(db, verse_id, table, record_id, row_bytes, author_did).await? {
        Admission::Allow => {}
        Admission::Deny(reason) => {
            tracing::warn!(
                verse_id,
                table,
                record_id,
                author = author_did,
                reason = %reason,
                "Inbound replicated row DENIED at the admission gate — not applied"
            );
            return Ok(ApplyReport::of(ReplicatedRowOutcome::Denied));
        }
        Admission::Defer(reason) => {
            tracing::debug!(
                verse_id,
                table,
                record_id,
                author = author_did,
                ?reason,
                "Inbound replicated row DEFERRED until it can be judged — not applied"
            );
            return Ok(ApplyReport::of(ReplicatedRowOutcome::Deferred(reason)));
        }
    }
    if table == "node" {
        if let Some(outcome) = super::scene_publication::apply_complete(
            db, verse_id, record_id, row_bytes, author_did, local_publisher, entity_change_tx, false,
        ).await? {
            return Ok(ApplyReport::of(outcome));
        }
        if !legacy_node_row_provable(db, verse_id, record_id, row_bytes, author_did).await? {
            tracing::debug!(verse_id, record_id, author = author_did, "markerless node row from a statement-derived Editor carries no write clock; denied (QA-0131)");
            return Ok(ApplyReport::of(ReplicatedRowOutcome::Denied));
        }
    }

    // Per-table semantics: `Timeseries` tables take the append-only union
    // path (A12), everything else the tombstone-aware row merge (N-4).
    let outcome =
        dispatch_by_mode(db, verse_id, table, record_id, row_bytes, author_did, entity_change_tx).await?;
    let mut report = ApplyReport::of(outcome);
    if let (Some((known_before, owner_before)), ReplicatedRowOutcome::Applied) =
        (manifest_before, &report.outcome)
    {
        report.manifest_landed = !known_before;
        let owner_after = crate::role_grants::verse_owner(db, verse_id).await?;
        if owner_after != owner_before {
            // `prior = None` seeds the owner and every admitted peer as risen,
            // which promotes every held statement the new owner empowers.
            let changed = crate::role_grants::repartition(db, verse_id, None).await?;
            report.risen_dids = changed.risen;
        }
    }
    Ok(report)
}

/// QA-0131: a markerless (legacy) node row, live or tombstone, has no write
/// clock, so an author whose Editor comes from role statements cannot show it
/// held that role when writing — judged as of clock 0, only authority that is
/// not statement-derived (the owner) applies it. Current publishers always
/// send complete versioned snapshots; src/AGENTS.md §write-time-role.
async fn legacy_node_row_provable(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<bool> {
    let petal = serde_json::from_slice::<serde_json::Value>(row_bytes)
        .ok()
        .and_then(|row| row.get("petal_id")?.as_str().map(str::to_owned));
    let scope = match petal {
        Some(petal) => super::crud::resolve_petal_scope_handler(db, &petal).await?,
        None => super::crud::resolve_node_scope_handler(db, record_id).await?,
    };
    match scope {
        Some(scope) => {
            crate::role_grants::written_while_authorized(db, verse_id, author_did, &scope, 0).await
        }
        None => Ok(true),
    }
}

// ---------------------------------------------------------------------------
// RP-3: inbound role statements (plan §2.1, §2.4)
// ---------------------------------------------------------------------------

/// Apply one inbound `role/{scope}/{peer}/{grantor}` statement: validate it
/// against the attested author (fe-policy; a failure is a permanent deny),
/// then place it durably — admitted, held in `role_statement_pending`,
/// superseded, or refused over its grantor's budget — repartitioning on
/// admission. Never enters the in-memory content queue and never waits on
/// `Parent`: the materializer works on scope strings (src/AGENTS.md
/// §inbound-admissions).
async fn apply_role_statement(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<ApplyReport> {
    // A `Doc::del` carries no HLC and iroh's `del` is a prefix delete;
    // revocation is a non-empty `revoked: true` statement (§2.1).
    if row_bytes.is_empty() {
        tracing::debug!(
            verse_id,
            record_id,
            "empty-entry delete on the role table — not applicable"
        );
        return Ok(ApplyReport::of(ReplicatedRowOutcome::NotApplicable));
    }
    let deny = |reason: String| {
        tracing::warn!(
            verse_id,
            record_id,
            author = author_did,
            reason = %reason,
            "Inbound role statement DENIED — not applied"
        );
        Ok(ApplyReport::of(ReplicatedRowOutcome::Denied))
    };
    let key = match fe_policy::StatementKey::parse(record_id) {
        Ok(key) => key,
        Err(reason) => return deny(reason.to_string()),
    };
    let payload: fe_policy::RoleStatementPayload = match serde_json::from_slice(row_bytes) {
        Ok(payload) => payload,
        Err(e) => return deny(format!("role statement payload is malformed: {e}")),
    };
    let owner = crate::role_grants::verse_owner(db, verse_id).await?;
    let stmt =
        match fe_policy::validate_statement(verse_id, owner.as_deref(), &key, author_did, &payload)
        {
            Ok(stmt) => stmt,
            Err(reason) => return deny(reason.to_string()),
        };
    // An interim `VERSE#<hex>` authority key is structurally out of reach (AC-45, T18).
    if crate::role_grants::verse_of_scope(&stmt.scope).as_deref() != Some(verse_id) {
        return deny("role statement scope is not a hierarchy scope of this verse".to_string());
    }

    let waiting = match owner {
        None => DeferralReason::Manifest,
        Some(_) => DeferralReason::AuthorRole {
            did: stmt.grantor_did.clone(),
        },
    };
    let (placement, changed) = crate::role_grants::submit_statement(db, verse_id, &stmt).await?;
    crate::op_log::observe_role_statement_hlc(stmt.hlc);
    use crate::role_grants::Placement;
    let report = match placement {
        Placement::Admitted => ApplyReport {
            outcome: ReplicatedRowOutcome::Applied,
            risen_dids: changed.risen,
            manifest_landed: false,
        },
        // Not newer than the live version: idempotent, nothing written. The
        // key is still deferred while that live version sits in the held store.
        Placement::Superseded => {
            if held_statement_key_exists(db, &stmt).await? {
                ApplyReport::of(ReplicatedRowOutcome::Deferred(waiting))
            } else {
                ApplyReport::of(ReplicatedRowOutcome::Applied)
            }
        }
        // Persisted in `role_statement_pending`; the partition promotes it.
        Placement::Held => ApplyReport::of(ReplicatedRowOutcome::Deferred(waiting)),
        // Not persisted (counted); the replica keeps it and the sync-plane
        // re-read forwards it again once its grantor is empowered (§2.5).
        Placement::RefusedOverBudget => ApplyReport::of(ReplicatedRowOutcome::Deferred(waiting)),
    };
    Ok(report)
}

/// Whether `stmt`'s key currently lives in the held store.
async fn held_statement_key_exists(
    db: &Db,
    stmt: &fe_policy::RoleStatement,
) -> anyhow::Result<bool> {
    let mut res = db
        .query("SELECT statement_key FROM role_statement_pending WHERE statement_key = $k LIMIT 1")
        .bind(("k", crate::role_grants::statement_record_id(stmt)))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("held role statement lookup failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
}

/// Apply one inbound row according to its table's [`ReplicationMode`].
///
/// `Timeseries` tables (currently just `iot_reading`) use the union CRDT
/// contract — see [`apply_timeseries_row`]; everything else keeps the
/// tombstone-aware row merge path.
async fn dispatch_by_mode(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    match ReplicationMode::for_table(table) {
        ReplicationMode::Timeseries => {
            apply_timeseries_row(db, verse_id, table, record_id, row_bytes).await
        }
        ReplicationMode::Row => {
            apply_row_mode(db, verse_id, table, record_id, row_bytes, author_did, entity_change_tx).await
        }
    }
}

/// Union-CRDT apply for append-only timeseries tables (A12; CDL-SPEC10 A3
/// widens this from `iot_reading`-only to every [`ReplicationMode::Timeseries`]
/// table, each keyed on its own [`ReplicationMode::union_key_field`]).
///
/// A row is an immutable fact: it is inserted **only when its union key is
/// absent**, so re-delivery is idempotent, an existing row is never
/// overwritten (no LWW), and the entry is never deleted — an empty
/// `Doc::del` tombstone on a timeseries key is deliberately `NotApplicable`
/// (union semantics keep the fact). Delivery order and count therefore cannot
/// change the converged set, which is what makes offline-then-rejoin
/// convergence work with no coordinator.
///
/// The A3 role gate has already run (it precedes dispatch for every table), so
/// an unauthorized peer's row never reaches here.
async fn apply_timeseries_row(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<ReplicatedRowOutcome> {
    // Union CRDT: deletes do not exist on this path. An empty entry is either a
    // stray `del` or a truncated payload — never apply it, never remove a fact.
    if row_bytes.is_empty() {
        tracing::debug!(
            verse_id,
            table,
            record_id,
            "empty-entry delete on a timeseries table — union CRDT keeps the fact (not applicable)"
        );
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let Some(key_field) = ReplicationMode::union_key_field(table) else {
        // Unreachable through `dispatch_by_mode` (it only routes Timeseries
        // tables here), kept as a defensive fallback rather than a panic.
        tracing::warn!(table, "timeseries apply with no known union key field");
        return Ok(ReplicatedRowOutcome::NotApplicable);
    };

    let mut row: serde_json::Value = serde_json::from_slice(row_bytes).map_err(|e| {
        anyhow::anyhow!("replicated timeseries row {table}/{record_id}: bad JSON: {e}")
    })?;
    strip_toplevel_nulls(&mut row);

    // The union key is the row's own key field; the wire key is the same
    // value by construction (the emission seam keys the doc entry on it), so a
    // mismatch means a malformed publisher — trust the payload and log.
    let key_value = row
        .get(key_field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| record_id.to_string());
    if key_value != record_id {
        tracing::warn!(
            table,
            record_id,
            key_field,
            key_value = %key_value,
            "replicated timeseries row key differs from its own key field — trusting the payload"
        );
    }

    if row_exists(db, table, key_field, &key_value).await? {
        tracing::debug!(
            verse_id,
            table,
            key_value,
            "timeseries row already present — union re-delivery is idempotent (not overwritten)"
        );
        // Reported `Applied` like the node path's idempotent re-merge: the
        // converged state is correct, which is what the outcome asserts.
        return Ok(ReplicatedRowOutcome::Applied);
    }

    db.query(format!("CREATE {table} CONTENT $row").as_str())
        .bind(("row", row))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated timeseries {table} create failed: {e}"))?;
    Ok(ReplicatedRowOutcome::Applied)
}

/// Row-mode dispatch: tombstone-aware (N-4) insert-or-merge for the tables the
/// inbound path has row semantics for.
async fn apply_row_mode(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    match table {
        "verse" | "fractal" | "petal" => {
            // An empty entry is the iroh-docs `del` marker. Static hierarchy
            // rows have no tombstone semantics in this model (N-4 covers
            // nodes) — a peer-side delete of a verse/petal is not a thing we
            // apply, so report it as not-applicable rather than an error.
            if row_bytes.is_empty() {
                tracing::debug!(
                    table,
                    record_id,
                    "empty-entry delete on a static hierarchy table — no tombstone semantics"
                );
                return Ok(ReplicatedRowOutcome::NotApplicable);
            }
            apply_static_row(db, table, record_id, row_bytes).await
        }
        "node" => apply_node_row(db, verse_id, record_id, row_bytes, author_did, entity_change_tx).await,
        // R5 (CDL-SPEC10 A3): insert-if-absent, never overwrite, anchored
        // rows only — see `apply_anchored_asset_row`.
        "asset" => apply_anchored_asset_row(db, verse_id, record_id, row_bytes, author_did).await,
        // R5 (CDL-SPEC10 A3): Row mode, insert-if-absent, immutable per key
        // (the admission gate already enforced the Manager+ floor and the
        // verse match) — see `apply_device_format_row`.
        "device_format" => apply_device_format_row(db, record_id, row_bytes).await,
        // Revocation dominates; the unsigned `del` marker is NotApplicable (§device-enrolment).
        crate::canon_log::device_enrolment::DEVICE_ENROLMENT_TABLE => {
            crate::canon_log::device_enrolment::apply_replicated_enrolment(db, record_id, row_bytes)
                .await
        }
        other => {
            tracing::debug!(
                table = other,
                "Inbound row for a table the replicated path does not handle"
            );
            Ok(ReplicatedRowOutcome::NotApplicable)
        }
    }
}

// ---------------------------------------------------------------------------
// A3: the inbound admission gate — verse id → enrolment → manifest wait →
// referential confinement → role at the row's deepest scope (RP-4)
// ---------------------------------------------------------------------------

/// Verdict of the inbound admission gate.
enum Admission {
    /// The row may be applied.
    Allow,
    /// The row is denied — never applied, never re-emitted.
    Deny(String),
    /// The row cannot be judged yet (plan §2.4) — not applied now, retried
    /// when the awaited fact lands.
    Defer(DeferralReason),
}

/// The role floor a confined row must clear.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Floor {
    /// The fe-policy standard write gate (Editor+) at every gated scope.
    Write,
    /// Manager+ at the VERSE scope: verse governance fields and `device_format`.
    ManageVerse,
    /// Manager+ at each exact current/requested Petal scope.
    ManageScope,
}

/// What referential confinement concluded about one row.
enum Confined {
    /// Every reference is in this verse: gate the author against `floor` at
    /// each of `scopes` (the row's deepest scope, plus a durable row's current
    /// scope when the row moves it).
    Gate { scopes: Vec<String>, floor: Floor },
    /// Settled before the role gate (denied or deferred).
    Stop(Admission),
}

/// Where a referenced row sits relative to the replica's verse (AC-43: three
/// outcomes, never a boolean that conflates "foreign" with "not here yet").
enum Located {
    /// Known locally and in this verse; carries its hierarchy scope (or id).
    Here(String),
    /// Known locally but in another verse, or mismatched — Denied.
    Foreign(String),
    /// Not known locally yet — Deferred until it lands.
    Unknown(DeferralReason),
}

/// `Located::Here`'s payload, or return the stop the other two outcomes imply.
macro_rules! here_or_stop {
    ($located:expr) => {
        match $located {
            Located::Here(value) => value,
            Located::Foreign(reason) => return Ok(Confined::Stop(Admission::Deny(reason))),
            Located::Unknown(reason) => return Ok(Confined::Stop(Admission::Defer(reason))),
        }
    };
}

fn deny(reason: impl Into<String>) -> anyhow::Result<Confined> {
    Ok(Confined::Stop(Admission::Deny(reason.into())))
}

fn defer(reason: DeferralReason) -> anyhow::Result<Confined> {
    Ok(Confined::Stop(Admission::Defer(reason)))
}

fn parent(table: &str, id: &str) -> DeferralReason {
    DeferralReason::Parent {
        table: table.to_string(),
        id: id.to_string(),
    }
}

/// Verse fields a replicated manifest may never change (T9).
const VERSE_IDENTITY_FIELDS: [&str; 4] = ["verse_id", "created_by", "created_at", "namespace_id"];
/// Verse fields whose change needs Manager+ at the verse scope (RP-4 step 2).
const VERSE_GOVERNANCE_FIELDS: [&str; 4] = [
    "default_access",
    "ts_mode",
    "ts_replication_factor",
    "ts_bucket_width_ms",
];

/// Tables the inbound path writes; any other table is `NotApplicable` at dispatch.
fn is_applied_table(table: &str) -> bool {
    matches!(
        table,
        "verse" | "fractal" | "petal" | "node" | "asset" | "device_format"
    ) || ReplicationMode::for_table(table) == ReplicationMode::Timeseries
}

/// A3: admit one inbound row. Gate order (RP-4, src/AGENTS.md §handlers
/// "replicated_row"): verse id check → enrolment verify → manifest wait →
/// referential confinement → the author's role at the row's deepest scope.
/// Roles come from the local tables only, never the wire; a sub-threshold
/// author is `Defer(AuthorRole)`, a missing manifest or parent `Defer`, and
/// only structural violations `Deny`.
async fn admit_inbound_row(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<Admission> {
    // A verse row claiming a different verse than the replica it arrived on
    // is a scope-injection attempt: the gate would judge the replica's verse
    // while the row lands under the payload's. Never trust it. The wire key
    // too: `apply_static_row` falls back to it when the payload names no
    // `verse_id`, so a key naming another verse would MERGE into that verse.
    if table == "verse" && record_id != verse_id {
        return Ok(Admission::Deny(
            "verse row key does not match the replica's verse".to_string(),
        ));
    }
    if table == "verse" {
        if let Some(claimed) = serde_json::from_slice::<serde_json::Value>(row_bytes)
            .ok()
            .and_then(|row| {
                row.get("verse_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
        {
            if claimed != verse_id {
                tracing::warn!(
                    verse_id,
                    claimed_verse_id = %claimed,
                    record_id,
                    author = author_did,
                    "verse row claims a foreign verse_id — denied"
                );
                return Ok(Admission::Deny(
                    "verse_id does not match the replica's verse".to_string(),
                ));
            }
        }
    }

    // Device enrolment (owner R2, W4-8): admitted iff the signed artifact verifies AND its
    // principal is the entry author — verified FIRST, so a forged enrolment is Denied even
    // before the manifest. An ENROLMENT (never a revocation) then waits for the manifest and
    // for membership (AC-44); see canon_log/AGENTS.md §device-enrolment "Membership floor".
    // Principal-keyed and verse-agnostic, so exempt from referential confinement.
    if table == crate::canon_log::device_enrolment::DEVICE_ENROLMENT_TABLE {
        use crate::canon_log::device_enrolment::{admit_replicated_enrolment, SelfAuthoredRow};
        let row_kind = match admit_replicated_enrolment(record_id, row_bytes, author_did) {
            Ok(row_kind) => row_kind,
            Err(reason) => return Ok(Admission::Deny(reason)),
        };
        if row_kind == SelfAuthoredRow::Enrolment {
            if !row_exists(db, "verse", "verse_id", verse_id).await? {
                return Ok(Admission::Defer(DeferralReason::Manifest));
            }
            if !is_verse_member(db, verse_id, author_did).await? {
                return Ok(Admission::Defer(DeferralReason::AuthorRole {
                    did: author_did.to_string(),
                }));
            }
        }
        return Ok(Admission::Allow);
    }

    // A table the inbound path never writes, or an empty `del` entry on a
    // table without tombstone semantics, is a no-op at dispatch
    // (`NotApplicable`): there is nothing to judge, and nothing to queue.
    if !is_applied_table(table) || (row_bytes.is_empty() && table != "node") {
        return Ok(Admission::Allow);
    }

    // First-manifest rule (RP-4 step 3): before a manifest exists, ONLY the
    // verse row is admitted — trust on first use; a joined store already
    // holds its invite-pinned manifest (`handlers::invite`), so a manifest
    // naming another owner meets the immutable-identity check instead. Every
    // other row waits for the manifest (role statements are held durably by
    // `apply_role_statement`, which runs before this gate).
    if !row_exists(db, "verse", "verse_id", verse_id).await? {
        if table == "verse" {
            // OF-4: a TOFU manifest must name a did:key owner. A placeholder
            // creator would be claimed by reconcile's `orphaned_creator` at the
            // next startup — the invitee-becomes-owner corruption.
            let creator = serde_json::from_slice::<serde_json::Value>(row_bytes)
                .ok()
                .and_then(|row| str_field(&row, "created_by").map(str::to_string));
            if !creator
                .as_deref()
                .is_some_and(crate::reconcile::is_owner_identity)
            {
                return Ok(Admission::Deny(
                    "first verse manifest does not name a did:key owner".to_string(),
                ));
            }
            tracing::debug!(
                verse_id,
                record_id,
                "first verse manifest — trust on first use"
            );
            return Ok(Admission::Allow);
        }
        return Ok(Admission::Defer(DeferralReason::Manifest));
    }

    let (scopes, floor) = match confine(db, verse_id, table, record_id, row_bytes).await? {
        Confined::Gate { scopes, floor } => (scopes, floor),
        Confined::Stop(Admission::Deny(reason)) => {
            tracing::warn!(
                verse_id,
                table,
                record_id,
                author = author_did,
                reason = %reason,
                "referential confinement denied an inbound row"
            );
            return Ok(Admission::Deny(reason));
        }
        Confined::Stop(other) => return Ok(other),
    };
    role_gate(db, verse_id, table, record_id, author_did, &scopes, floor).await
}

/// The role gate (RP-4 step 6): the author's role, resolved from the local
/// tables at each gated scope — `resolve_role` walks up the chain over the
/// materialized cache, so this is `role_at`, the function grant authority and
/// the sync-plane gates use. An Editor appointed by a petal Manager passes in
/// that petal and is deferred elsewhere. A sub-threshold author has not
/// converged yet — `Defer(AuthorRole)`, retried when their role rises.
async fn role_gate(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    author_did: &str,
    scopes: &[String],
    floor: Floor,
) -> anyhow::Result<Admission> {
    let below_threshold = || {
        Admission::Defer(DeferralReason::AuthorRole {
            did: author_did.to_string(),
        })
    };
    match floor {
        Floor::ManageVerse => {
            let verse_scope = crate::scope::build_scope(verse_id, None, None);
            let role = crate::role_manager::resolve_role(db, author_did, &verse_scope).await?;
            if let fe_policy::Decision::Deny(reason) =
                crate::rbac::evaluate_manage(author_did, role, &verse_scope)
            {
                tracing::debug!(
                    verse_id,
                    table,
                    record_id,
                    %role,
                    reason = %reason,
                    "needs Manager+ at the verse scope — deferred"
                );
                return Ok(below_threshold());
            }
        }
        Floor::ManageScope => {
            for scope in scopes {
                let role = crate::role_manager::resolve_role(db, author_did, scope).await?;
                if !role.is_at_least(fe_policy::RoleLevel::Manager) { return Ok(below_threshold()); }
            }
        }
        Floor::Write => {
            for scope in scopes {
                let role = crate::role_manager::resolve_role(db, author_did, scope).await?;
                if let fe_policy::Decision::Deny(reason) =
                    crate::rbac::evaluate_write(author_did, &role.to_string(), scope)
                {
                    tracing::debug!(
                        verse_id,
                        table,
                        record_id,
                        scope = %scope,
                        reason = %reason,
                        "author below the write threshold at the row's scope — deferred"
                    );
                    return Ok(below_threshold());
                }
            }
        }
    }
    Ok(Admission::Allow)
}

/// AC-44(c) enrolment membership: the principal resolves to a role at the
/// verse scope (owner, an explicit row or `default_access`), or holds any
/// effective role at any scope of the verse — a petal-scoped member resolves
/// to `None` at the verse scope yet must still be able to enrol.
async fn is_verse_member(db: &Db, verse_id: &str, did: &str) -> anyhow::Result<bool> {
    let verse_scope = crate::scope::build_scope(verse_id, None, None);
    if crate::role_manager::resolve_role(db, did, &verse_scope).await? != fe_policy::RoleLevel::None
    {
        return Ok(true);
    }
    Ok(crate::role_grants::load_effective_roles(db, verse_id)
        .await?
        .is_some_and(|effective| effective.has_any_role(did)))
}

// ---------------------------------------------------------------------------
// Referential confinement (P2P-REF-CONFINE, RP-4 step 5) — the ONE
// parent-chain resolver every table shares (CDL-SPEC10 A3's included)
// ---------------------------------------------------------------------------

/// Confine one row of a known verse: every reference it carries, AND the
/// current parent of the durable row it would change, must resolve to this
/// replica's verse. Returns the scopes and floor its author must clear.
async fn confine(
    db: &Db,
    verse_id: &str,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<Confined> {
    // The empty `del` entry reaches here only for `node` (the wire tombstone).
    let row: serde_json::Value = if row_bytes.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_slice(row_bytes) {
            Ok(row @ serde_json::Value::Object(_)) => row,
            _ => return deny(format!("{table} row is not a JSON object")),
        }
    };
    match table {
        "verse" => confine_verse(db, verse_id, &row).await,
        "fractal" => confine_fractal(db, verse_id, record_id, &row).await,
        "petal" => confine_petal(db, verse_id, record_id, &row).await,
        "node" => confine_node(db, verse_id, record_id, &row).await,
        "asset" => confine_asset(db, verse_id, &row).await,
        "device_payload" => confine_device_payload(db, verse_id, &row).await,
        "device_format" => confine_device_format(verse_id, &row),
        "iot_reading" => confine_reading(db, verse_id, &row).await,
        other => deny(format!("no confinement rule for table '{other}'")),
    }
}

/// Immutable identity (T9) and governance fields (RP-4 steps 1-2).
async fn confine_verse(
    db: &Db,
    verse_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let Some(stored) = lookup_row(db, "verse", "verse_id", verse_id, "*").await? else {
        return defer(DeferralReason::Manifest);
    };
    let mut namespace_fill = false;
    for field in VERSE_IDENTITY_FIELDS {
        let Some(new) = present(row, field) else {
            continue;
        };
        match present(&stored, field) {
            Some(old) if json_eq(old, new) => {}
            // A replica id the local row never had may be filled in once — by a verse
            // Manager+ only: it picks the namespace this node opens next (refine R2).
            None if field == "namespace_id" => namespace_fill = true,
            _ => {
                return deny(format!(
                    "verse {field} is immutable — a replicated manifest may not change it"
                ))
            }
        }
    }
    let governance_change = namespace_fill
        || VERSE_GOVERNANCE_FIELDS.iter().any(|field| {
            present(row, field)
                .is_some_and(|new| !present(&stored, field).is_some_and(|old| json_eq(old, new)))
        });
    Ok(Confined::Gate {
        scopes: vec![crate::scope::build_scope(verse_id, None, None)],
        floor: if governance_change {
            Floor::ManageVerse
        } else {
            Floor::Write
        },
    })
}

async fn confine_fractal(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let fractal_id = str_field(row, "fractal_id").unwrap_or(record_id);
    if !is_scope_safe_id(fractal_id) {
        return deny(format!("fractal id '{fractal_id}' cannot form a scope"));
    }
    let claimed = str_field(row, "verse_id");
    if claimed.is_some_and(|claimed| claimed != verse_id) {
        return deny("fractal verse_id does not match the replica's verse");
    }
    match lookup_row(db, "fractal", "fractal_id", fractal_id, "verse_id").await? {
        Some(durable) if str_field(&durable, "verse_id") != Some(verse_id) => {
            return deny(format!(
                "fractal '{fractal_id}' currently belongs to another verse"
            ))
        }
        None if claimed.is_none() => return deny("fractal row names no verse"),
        _ => {}
    }
    Ok(Confined::Gate {
        scopes: vec![crate::scope::build_scope(verse_id, Some(fractal_id), None)],
        floor: Floor::Write,
    })
}

async fn confine_petal(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let petal_id = str_field(row, "petal_id").unwrap_or(record_id);
    if !is_scope_safe_id(petal_id) {
        return deny(format!("petal id '{petal_id}' cannot form a scope"));
    }
    let Some(fractal_id) = str_field(row, "fractal_id") else {
        return deny("petal row has no fractal_id");
    };
    here_or_stop!(locate_fractal(db, verse_id, fractal_id).await?);
    let mut scopes = vec![crate::scope::build_scope(
        verse_id,
        Some(fractal_id),
        Some(petal_id),
    )];
    let mut requirements_changed = row.get("hexon_manifest").is_some_and(|value| !value.is_null());
    if let Some(durable) = lookup_row(db, "petal", "petal_id", petal_id, "fractal_id, hexon_manifest").await? {
        if let Some(incoming) = row.get("hexon_manifest") {
            requirements_changed = Some(incoming).filter(|v| !v.is_null()) != durable.get("hexon_manifest").filter(|v| !v.is_null());
        }
        let Some(durable_fractal) = str_field(&durable, "fractal_id") else {
            return deny(format!("petal '{petal_id}' has no durable fractal"));
        };
        if durable_fractal != fractal_id {
            here_or_stop!(match locate_fractal(db, verse_id, durable_fractal).await? {
                Located::Foreign(reason) => Located::Foreign(format!(
                    "petal '{petal_id}' currently belongs to another verse: {reason}"
                )),
                other => other,
            });
            scopes.push(crate::scope::build_scope(
                verse_id,
                Some(durable_fractal),
                Some(petal_id),
            ));
        }
    }
    Ok(Confined::Gate {
        scopes,
        floor: if requirements_changed { Floor::ManageScope } else { Floor::Write },
    })
}

/// A node row, a partial row, or the empty-entry tombstone: the payload's
/// petal and the durable row's current petal must both be in this verse; a
/// row naming no petal for a node unknown locally waits for that node.
async fn confine_node(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let node_id = str_field(row, "node_id").unwrap_or(record_id);
    let mut scopes = Vec::new();
    match present(row, "petal_id") {
        None => {}
        Some(serde_json::Value::String(petal_id)) => {
            scopes.push(here_or_stop!(locate_petal(db, verse_id, petal_id).await?));
        }
        Some(_) => return deny("node petal_id is not a string"),
    }
    match lookup_row(db, "node", "node_id", node_id, "petal_id").await? {
        Some(durable) => match str_field(&durable, "petal_id") {
            Some(durable_petal) => {
                let scope = here_or_stop!(match locate_petal(db, verse_id, durable_petal).await? {
                    Located::Foreign(reason) => Located::Foreign(format!(
                        "node '{node_id}' currently belongs to another verse: {reason}"
                    )),
                    other => other,
                });
                if !scopes.contains(&scope) {
                    scopes.push(scope);
                }
            }
            None if scopes.is_empty() => {
                return deny(format!("node '{node_id}' has no petal to confine it"))
            }
            None => {}
        },
        None if scopes.is_empty() => return defer(parent("node", node_id)),
        None => {}
    }
    Ok(Confined::Gate {
        scopes,
        floor: Floor::Write,
    })
}

/// `asset` (CDL-SPEC10 A3): anchored rows only; the anchor petal must be here.
async fn confine_asset(
    db: &Db,
    verse_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let Some(petal_id) = str_field(row, "petal_id") else {
        return deny("asset row is not anchored to a petal (un-anchored rows never replicate)");
    };
    let scope = here_or_stop!(locate_petal(db, verse_id, petal_id).await?);
    Ok(Confined::Gate {
        scopes: vec![scope],
        floor: Floor::Write,
    })
}

/// `device_payload` (CDL-SPEC10 A3, SPEC-10 §2.2 rule 5 as the strictest
/// reader): petal → anchor node → every listed asset → the declared format →
/// each asset's MIME. A reference list that is absent, empty, oversized or not
/// all strings is denied: a check that skips what it cannot read is a check a
/// peer can step around (`src/AGENTS.md` §device-payload).
async fn confine_device_payload(
    db: &Db,
    verse_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let Some(petal_id) = str_field(row, "petal_id") else {
        return deny("device_payload row has no petal_id");
    };
    let scope = here_or_stop!(locate_petal(db, verse_id, petal_id).await?);
    match present(row, "verse_id") {
        None => {}
        Some(v) if v.as_str() == Some(verse_id) => {}
        Some(_) => return deny("device_payload verse_id does not match the replica's verse"),
    }
    let Some(node_id) = str_field(row, "node_id") else {
        return deny("device_payload row has no node_id");
    };
    here_or_stop!(locate_node_in_petal(db, node_id, petal_id).await?);
    let Some(asset_ids) = row.get("asset_ids").and_then(|v| v.as_array()) else {
        return deny("device_payload row has no asset_ids list");
    };
    if asset_ids.is_empty() || asset_ids.len() > fe_runtime::messages::DEVICE_PAYLOAD_MAX_ASSETS {
        return deny(format!(
            "device_payload asset_ids must hold 1..={} entries",
            fe_runtime::messages::DEVICE_PAYLOAD_MAX_ASSETS
        ));
    }
    let Some(asset_ids) = asset_ids
        .iter()
        .map(|entry| entry.as_str())
        .collect::<Option<Vec<&str>>>()
    else {
        return deny("device_payload asset_ids entry is not a string");
    };
    let mut content_types = Vec::with_capacity(asset_ids.len());
    for asset_id in &asset_ids {
        let Some(asset) =
            lookup_row(db, "asset", "asset_id", asset_id, "petal_id, content_type").await?
        else {
            return defer(parent("asset", asset_id));
        };
        if str_field(&asset, "petal_id") != Some(petal_id) {
            return deny(format!(
                "device_payload asset_ids entry '{asset_id}' is not anchored to petal '{petal_id}'"
            ));
        }
        content_types.push(str_field(&asset, "content_type").map(str::to_string));
    }
    let Some(format_id) = str_field(row, "format_id") else {
        return deny("device_payload row has no format_id");
    };
    let format = db
        .query("SELECT mime FROM device_format WHERE verse_id = $vid AND format_id = $fid LIMIT 1")
        .bind(("vid", verse_id.to_string()))
        .bind(("fid", format_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("device_format lookup failed: {e}"))?
        .take::<Vec<serde_json::Value>>(0)?;
    let Some(mime) = format
        .first()
        .and_then(|f| str_field(f, "mime"))
        .map(str::to_string)
    else {
        return defer(parent("device_format", format_id));
    };
    for (asset_id, content_type) in asset_ids.iter().zip(&content_types) {
        if content_type.as_deref() != Some(mime.as_str()) {
            return deny(format!(
                "device_payload asset '{asset_id}' does not carry format '{format_id}''s MIME"
            ));
        }
    }
    Ok(Confined::Gate {
        scopes: vec![scope],
        floor: Floor::Write,
    })
}

/// `device_format` (R5, critic C3): schema-defining, so Manager+ at the verse.
fn confine_device_format(verse_id: &str, row: &serde_json::Value) -> anyhow::Result<Confined> {
    match str_field(row, "verse_id") {
        Some(row_verse) if row_verse == verse_id => Ok(Confined::Gate {
            scopes: vec![crate::scope::build_scope(verse_id, None, None)],
            floor: Floor::ManageVerse,
        }),
        Some(row_verse) => deny(format!(
            "device_format verse_id '{row_verse}' does not match the replica's verse '{verse_id}'"
        )),
        None => deny("device_format row has no verse_id"),
    }
}

/// `iot_reading`: its petal is here, its anchor node is a node of that petal,
/// and an R5 `payload_id` link names a payload of the same petal.
async fn confine_reading(
    db: &Db,
    verse_id: &str,
    row: &serde_json::Value,
) -> anyhow::Result<Confined> {
    let Some(petal_id) = str_field(row, "petal_id") else {
        return deny("iot_reading row has no petal_id");
    };
    let scope = here_or_stop!(locate_petal(db, verse_id, petal_id).await?);
    let Some(node_id) = str_field(row, "node_id") else {
        return deny("iot_reading row has no node_id");
    };
    here_or_stop!(locate_node_in_petal(db, node_id, petal_id).await?);
    if let Some(link) = present(row, "payload_id") {
        let Some(payload_id) = link.as_str() else {
            return deny("iot_reading payload_id link is malformed");
        };
        match lookup_row(db, "device_payload", "payload_id", payload_id, "petal_id").await? {
            None => return defer(parent("device_payload", payload_id)),
            Some(payload) if str_field(&payload, "petal_id") == Some(petal_id) => {}
            Some(_) => {
                return deny(format!(
                    "iot_reading payload_id '{payload_id}' is not a payload of petal '{petal_id}'"
                ))
            }
        }
    }
    Ok(Confined::Gate {
        scopes: vec![scope],
        floor: Floor::Write,
    })
}

/// A fractal: here (its verse scope), foreign, or unknown locally.
async fn locate_fractal(db: &Db, verse_id: &str, fractal_id: &str) -> anyhow::Result<Located> {
    if !is_scope_safe_id(fractal_id) {
        return Ok(Located::Foreign(format!(
            "fractal id '{fractal_id}' cannot form a scope"
        )));
    }
    Ok(
        match lookup_row(db, "fractal", "fractal_id", fractal_id, "verse_id").await? {
            None => Located::Unknown(parent("fractal", fractal_id)),
            Some(fractal) => match str_field(&fractal, "verse_id") {
                Some(owner) if owner == verse_id => {
                    Located::Here(crate::scope::build_scope(verse_id, Some(fractal_id), None))
                }
                Some(owner) => Located::Foreign(format!(
                    "fractal '{fractal_id}' belongs to verse '{owner}', not the replica's '{verse_id}'"
                )),
                None => Located::Foreign(format!("fractal '{fractal_id}' names no verse")),
            },
        },
    )
}

/// A petal: here (its full scope), foreign, or unknown locally (it, or its fractal).
async fn locate_petal(db: &Db, verse_id: &str, petal_id: &str) -> anyhow::Result<Located> {
    if !is_scope_safe_id(petal_id) {
        return Ok(Located::Foreign(format!(
            "petal id '{petal_id}' cannot form a scope"
        )));
    }
    let Some(petal) = lookup_row(db, "petal", "petal_id", petal_id, "fractal_id").await? else {
        return Ok(Located::Unknown(parent("petal", petal_id)));
    };
    let Some(fractal_id) = str_field(&petal, "fractal_id") else {
        return Ok(Located::Foreign(format!(
            "petal '{petal_id}' has no fractal"
        )));
    };
    Ok(match locate_fractal(db, verse_id, fractal_id).await? {
        Located::Here(_) => Located::Here(crate::scope::build_scope(
            verse_id,
            Some(fractal_id),
            Some(petal_id),
        )),
        other => other,
    })
}

/// A node of `petal_id`: here, a node of another petal (mismatch), or unknown.
async fn locate_node_in_petal(db: &Db, node_id: &str, petal_id: &str) -> anyhow::Result<Located> {
    Ok(
        match lookup_row(db, "node", "node_id", node_id, "petal_id").await? {
            None => Located::Unknown(parent("node", node_id)),
            Some(node) if str_field(&node, "petal_id") == Some(petal_id) => {
                Located::Here(node_id.to_string())
            }
            Some(_) => Located::Foreign(format!(
                "node '{node_id}' is not a node of petal '{petal_id}'"
            )),
        },
    )
}

/// An id that cannot smuggle a scope boundary (`#`) into `build_scope`.
fn is_scope_safe_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('#')
}

/// `fields` of the row keyed `id_field = id` in `table`, or `None` when no
/// such row exists locally. `table`/`id_field`/`fields` are static names;
/// tolerates an absent table like [`row_exists`].
async fn lookup_row(
    db: &Db,
    table: &str,
    id_field: &str,
    id: &str,
    fields: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let res = db
        .query(format!("SELECT {fields} FROM {table} WHERE {id_field} = $rid LIMIT 1").as_str())
        .bind(("rid", id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("replicated {table} lookup failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match res {
        Ok(mut r) => r
            .take(0)
            .map_err(|e| anyhow::anyhow!("replicated {table} lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("replicated {table} lookup failed: {e}")),
    };
    Ok(rows.into_iter().next())
}

/// A string field of a JSON row.
fn str_field<'a>(row: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    row.get(field).and_then(|v| v.as_str())
}

/// A field that is present and not null (absence and `null` both mean `NONE`).
fn present<'a>(row: &'a serde_json::Value, field: &str) -> Option<&'a serde_json::Value> {
    row.get(field).filter(|v| !v.is_null())
}

/// JSON equality that treats numerically equal numbers as equal (a stored
/// `int` and a wire `2.0` are the same setting).
fn json_eq(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// Static hierarchy tables (verse / fractal / petal)
// ---------------------------------------------------------------------------

/// Insert-or-merge one replicated row into a static hierarchy table.
///
/// Geometry columns are extracted first (never bound through CONTENT/MERGE —
/// §geometry-inserts) and cast-written once the row exists.
async fn apply_static_row(
    db: &Db,
    table: &str,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<ReplicatedRowOutcome> {
    let (id_field, geometry_field) = match table {
        "verse" => ("verse_id", None),
        "fractal" => ("fractal_id", None),
        "petal" => ("petal_id", Some("bounds")),
        _ => unreachable!("apply_static_row only receives the static hierarchy tables"),
    };

    let mut row: serde_json::Value = serde_json::from_slice(row_bytes)
        .map_err(|e| anyhow::anyhow!("replicated row {table}/{record_id}: bad JSON: {e}"))?;
    // QA-0123: a projected catalog row carries NONE columns as null — absent, not
    // malformed — so nulls go before any per-field check.
    strip_toplevel_nulls(&mut row);
    if table == "petal" {
        if let Some(document) = row.get("hexon_manifest") {
            if let Err(reason) =
                fe_runtime::petal_requirements::PetalRequirements::from_document(Some(document))
            {
                tracing::warn!(
                    table,
                    record_id,
                    reason,
                    "Inbound petal row carries malformed requirements — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
    }
    // The SurrealDB `id` is the AUTHOR's local record identity (a `RETURN AFTER` read-back
    // carries it), never wire data: MERGEing it into an existing row fails, and on CREATE it
    // would let a peer choose the local record id. Rows are keyed by `id_field` alone.
    if let Some(object) = row.as_object_mut() {
        object.remove("id");
    }

    // A3 deny-by-default (F20 fold-in): an inbound verse manifest is the
    // only writer that could hand an unknown peer a write role — `resolve_role`
    // falls back to `default_access` for peers with no explicit row, and the
    // schema only DEFAULTs the column (no ASSERT). The local writers are
    // pinned ("viewer" at creation, `set_default_access` rejects anything
    // else), so the replicated path validates the same set here: a manifest
    // carrying anything outside {"viewer", "none"} is denied outright, never
    // applied.
    if table == "verse" {
        if let Some(access) = row.get("default_access") {
            let valid = access
                .as_str()
                .is_some_and(|v| v == "viewer" || v == "none");
            if !valid {
                tracing::warn!(
                    verse_id = record_id,
                    default_access = ?access,
                    "Inbound verse manifest carries a forbidden default_access — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
    }

    // p2p_asset_streaming_20260718 S6 (critic step 8 + "strictest reader" rule,
    // RUNBOOK §5): a writer of a shared petal column must satisfy the
    // strictest reader. The admission gate (A3 Editor+) already ran; this is
    // the per-field shape/range check an admitted-but-malicious or buggy
    // writer could otherwise slip past (`apply_static_row` MERGEs the whole
    // row — see campaign plan §3 S6 step 4/8).
    if table == "petal" {
        if let Some(rd) = row.get("render_distance") {
            if !is_valid_inbound_render_distance(rd) {
                tracing::warn!(
                    table,
                    record_id,
                    render_distance = ?rd,
                    "Inbound petal row carries an out-of-range render_distance — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
        if let Some(groups) = row.get("visibility_groups") {
            if !groups.is_null() && !super::visibility_groups::is_valid_inbound_registry(groups) {
                tracing::warn!(
                    table,
                    record_id,
                    "Inbound petal row carries a malformed visibility group registry — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
        if let Some(terrain) = row.get("terrain") {
            if !terrain.is_null() && !is_valid_inbound_petal_terrain(terrain) {
                tracing::warn!(
                    table,
                    record_id,
                    "Inbound petal row carries a malformed terrain config — denied"
                );
                return Ok(ReplicatedRowOutcome::Denied);
            }
        }
    }

    // Extract the geometry column now, write it with an explicit cast after
    // the row exists (a raw GeoJSON bind against a SCHEMAFULL table fails).
    let geometry_value = match geometry_field {
        None => None,
        Some(field) => row.as_object_mut().and_then(|o| o.remove(field)),
    };

    // Prefer the payload's own id field; fall back to the wire record id.
    let row_id = row
        .get(id_field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| record_id.to_string());
    if row_id != record_id {
        tracing::warn!(
            table,
            record_id,
            row_id = %row_id,
            "Replicated row id differs from the wire key — trusting the payload"
        );
    }

    if row_exists(db, table, id_field, &row_id).await? {
        db.query(format!("UPDATE {table} MERGE $row WHERE {id_field} = $rid").as_str())
            .bind(("row", row.clone()))
            .bind(("rid", row_id.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("replicated {table} merge failed: {e}"))?;
    } else {
        db.query(format!("CREATE {table} CONTENT $row").as_str())
            .bind(("row", row.clone()))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("replicated {table} create failed: {e}"))?;
    }

    if let Some(geometry) = geometry_value {
        let field = geometry_field.expect("geometry_value implies a geometry field");
        // §geometry-inserts, strictest form: a cast around a BOUND GeoJSON
        // object — or even around an object literal with a bound rings
        // parameter — is rejected by the schema check ("could not cast into
        // `geometry`"). The only form this SurrealDB line casts is the inline
        // numeric literal the local petal paths use (crud.rs, seed.rs). So:
        // extract the payload's coordinate rings and render them inline.
        // Only validated finite floats are formatted — no injection surface.
        let rings = geometry.get("coordinates").cloned().ok_or_else(|| {
            anyhow::anyhow!("replicated {table} row {row_id}: bounds lacks coordinates rings")
        })?;
        let literal = render_polygon_rings(&rings).ok_or_else(|| {
            anyhow::anyhow!(
                "replicated {table} row {row_id}: bounds coordinates are not numeric rings"
            )
        })?;
        db.query(
            format!(
                "UPDATE {table} SET {field} = \
                 <geometry<polygon>> {{ type: 'Polygon', coordinates: {literal} }} \
                 WHERE {id_field} = $rid"
            )
            .as_str(),
        )
        .bind(("rid", row_id))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated {table} geometry cast failed: {e}"))?;
    }

    Ok(ReplicatedRowOutcome::Applied)
}

// ---------------------------------------------------------------------------
// R5 generic asset / device format — insert-if-absent, never overwrite
// (CDL-SPEC10 A3)
// ---------------------------------------------------------------------------

/// Insert-if-absent, never-overwrite apply for an inbound `asset` row —
/// R4 override D-5/D-17 (campaign plan §3 A3 step 3): anchored rows only
/// (an un-anchored `petal_id: NONE` legacy row is never applied, so a legacy
/// local-only row can never be shadowed by a peer's). The admission gate
/// (`admit_inbound_row`) has already confirmed the row's `petal_id` resolves
/// to THIS replica's verse before this function runs.
async fn apply_anchored_asset_row(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
) -> anyhow::Result<ReplicatedRowOutcome> {
    // An empty entry is the iroh-docs `del` marker; `asset` has no tombstone
    // semantics (GC is the only deletion path) — not applicable, like the
    // static hierarchy tables.
    if row_bytes.is_empty() {
        tracing::debug!(record_id, "empty-entry delete on `asset` — not applicable");
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let mut row: serde_json::Value = serde_json::from_slice(row_bytes)
        .map_err(|e| anyhow::anyhow!("replicated asset row {record_id}: bad JSON: {e}"))?;
    strip_toplevel_nulls(&mut row);

    if row.get("petal_id").and_then(|v| v.as_str()).is_none() {
        tracing::warn!(
            record_id,
            "inbound asset row has no anchor petal_id — denied (un-anchored rows never replicate)"
        );
        return Ok(ReplicatedRowOutcome::Denied);
    }
    if let Some(reason) = inbound_asset_shape_violation(&row, record_id) {
        tracing::warn!(record_id, reason, "inbound asset row denied");
        return Ok(ReplicatedRowOutcome::Denied);
    }
    // A peer must not charge (or, with a negative size, credit) a local
    // principal's upload quota, which sums `size_bytes` by `owner_did`
    // (fe-api §asset-ingest): an inbound row is never attributed to a local
    // uploader (`src/AGENTS.md` §device-payload).
    if let Some(fields) = row.as_object_mut() {
        fields.remove("owner_did");
        fields.remove("publication_provenance");
        fields.remove(fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER);
    }

    let asset_id = row
        .get("asset_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| record_id.to_string());
    if asset_id != record_id {
        return Ok(ReplicatedRowOutcome::Denied);
    }

    if row_exists(db, "asset", "asset_id", &asset_id).await? {
        tracing::debug!(
            asset_id,
            "asset row already present — insert-if-absent never overwrites it"
        );
        return Ok(ReplicatedRowOutcome::Applied);
    }

    let petal = row["petal_id"].as_str().unwrap();
    let Some(scope) = super::crud::resolve_petal_scope_handler(db, petal).await? else {
        return Ok(ReplicatedRowOutcome::Deferred(DeferralReason::Parent {
            table: "petal".into(),
            id: petal.into(),
        }));
    };
    if crate::parse_scope(&scope)?.verse_id != verse_id {
        return Ok(ReplicatedRowOutcome::Denied);
    }
    let permission = super::entity::prepare_scope_permission(
        db,
        &scope,
        author_did,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let (mut guards, mut bindings) = permission.into_parts();
    let Some((reference_guards, reference_bindings)) = prepare_asset_reference_fence(db, &asset_id, petal).await? else {
        return Ok(ReplicatedRowOutcome::Denied);
    };
    guards.extend(reference_guards);
    bindings.extend(reference_bindings);
    guards.extend([
        "IF array::len((SELECT asset_id FROM asset WHERE asset_id=$inbound_scene_asset)) != 0 { THROW 'asset identity appeared'; }".into(),
        "CREATE asset CONTENT $inbound_scene_row".into(),
    ]);
    bindings.push(("inbound_scene_row".into(), row));
    let script = format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        guards.join(";\n")
    );
    anyhow::ensure!(
        script.len() + serde_json::to_vec(&bindings)?.len() <= 4 * 1024 * 1024,
        "asset receive transaction exceeds 4 MiB"
    );
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    query.await?.check()?;
    Ok(ReplicatedRowOutcome::Applied)
}

/// Capture all retained asset reference identities for the same use-time CREATE fence.
pub(crate) async fn prepare_asset_reference_fence(
    db: &Db,
    asset_id: &str,
    petal: &str,
) -> anyhow::Result<Option<(Vec<String>, Vec<(String, serde_json::Value)>)>> {
    let references="SELECT node_id,petal_id FROM node WHERE asset_id=$inbound_scene_asset ORDER BY node_id LIMIT 257 TIMEOUT 2s";
    let payloads="SELECT payload_id,petal_id FROM device_payload WHERE asset_ids CONTAINS $inbound_scene_asset ORDER BY payload_id LIMIT 257 TIMEOUT 2s";
    let models = "SELECT petal_id, encoding::base64::encode(encoding::cbor::encode($this)) AS fingerprint FROM model WHERE asset_id=$inbound_scene_asset LIMIT 2 TIMEOUT 2s";
    let mut response = db
        .query(format!("{references}; {payloads}; {models}"))
        .bind(("inbound_scene_asset", asset_id.to_owned()))
        .await?
        .check()?;
    let nodes: Vec<serde_json::Value> = response.take(0)?;
    let payload_rows: Vec<serde_json::Value> = response.take(1)?;
    let model_rows: Vec<serde_json::Value> = response.take(2)?;
    if model_rows.len() > 1
        || model_rows
            .iter()
            .any(|reference| reference["petal_id"] != petal)
        || nodes.len() > 256
        || payload_rows.len() > 256
        || nodes
            .iter()
            .chain(&payload_rows)
            .any(|reference| reference["petal_id"] != petal)
    {
        return Ok(None);
    }
    let guards = vec![
        format!("IF ({references}) != $inbound_scene_nodes {{ THROW 'asset reference set changed'; }}"),
        format!("IF ({payloads}) != $inbound_scene_payloads {{ THROW 'asset payload reference set changed'; }}"),
        format!("IF ({models}) != $inbound_scene_models {{ THROW 'asset Portal reference changed'; }}"),
    ];
    let bindings = vec![
        ("inbound_scene_asset".into(), serde_json::json!(asset_id)),
        ("inbound_scene_nodes".into(), serde_json::json!(nodes)),
        (
            "inbound_scene_payloads".into(),
            serde_json::json!(payload_rows),
        ),
        ("inbound_scene_models".into(), serde_json::json!(model_rows)),
    ];
    Ok(Some((guards, bindings)))
}
/// Why an inbound `asset` row's own fields are unsafe to store, or `None`.
/// The id is joined into a filesystem path by the legacy backfill, the hash
/// names a blob-store file, the size feeds the upload quota and the
/// `content_type` becomes a response `Content-Type`; each is checked with the
/// shared guard the local path uses.
fn inbound_asset_shape_violation(row: &serde_json::Value, record_id: &str) -> Option<&'static str> {
    let asset_id = row
        .get("asset_id")
        .and_then(|v| v.as_str())
        .unwrap_or(record_id);
    if !fe_runtime::safe_id::is_safe_path_id(asset_id) {
        return Some("asset_id is not a safe path id");
    }
    let hash_ok = row
        .get("content_hash")
        .and_then(|v| v.as_str())
        .is_some_and(|hex| crate::hash_from_hex(hex).is_ok());
    if !hash_ok {
        return Some("content_hash is absent or not a 32-byte hex digest");
    }
    if row
        .get("size_bytes")
        .and_then(|v| v.as_i64())
        .is_none_or(|size| size < 0)
    {
        return Some("size_bytes is absent or negative");
    }
    if !row
        .get("content_type")
        .and_then(|v| v.as_str())
        .is_some_and(fe_sdk::device_format::is_valid_mime)
    {
        return Some("content_type is not a type/subtype mime");
    }
    None
}

/// Insert-if-absent, never-overwrite, immutable-per-key apply for an inbound
/// `device_format` row (R5: "immutable per key; re-declaring with a
/// different MIME is a conflict" — the admission gate, not this function,
/// enforces the Manager+ floor and the verse match).
async fn apply_device_format_row(
    db: &Db,
    record_id: &str,
    row_bytes: &[u8],
) -> anyhow::Result<ReplicatedRowOutcome> {
    if row_bytes.is_empty() {
        tracing::debug!(
            record_id,
            "empty-entry delete on `device_format` — immutable, not applicable"
        );
        return Ok(ReplicatedRowOutcome::NotApplicable);
    }
    let mut row: serde_json::Value = serde_json::from_slice(row_bytes)
        .map_err(|e| anyhow::anyhow!("replicated device_format row {record_id}: bad JSON: {e}"))?;
    strip_toplevel_nulls(&mut row);

    let format_id = row
        .get("format_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| record_id.to_string());
    // The local door's grammar, applied to the peer's row (strictest reader).
    let mime_ok = row
        .get("mime")
        .and_then(|v| v.as_str())
        .is_some_and(fe_sdk::device_format::is_valid_mime);
    if !fe_sdk::source_class::is_valid_format_id(&format_id) || !mime_ok {
        tracing::warn!(
            record_id,
            format_id,
            "inbound device_format row has a malformed format_id or mime — denied"
        );
        return Ok(ReplicatedRowOutcome::Denied);
    }
    let Some(verse_id) = row
        .get("verse_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        tracing::warn!(
            record_id,
            "inbound device_format row has no verse_id — denied"
        );
        return Ok(ReplicatedRowOutcome::Denied);
    };

    let existing = db
        .query("SELECT mime FROM device_format WHERE verse_id = $vid AND format_id = $fid LIMIT 1")
        .bind(("vid", verse_id))
        .bind(("fid", format_id.clone()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated device_format lookup failed: {e}"))?
        .take::<Vec<serde_json::Value>>(0)
        .unwrap_or_default();
    if !existing.is_empty() {
        tracing::debug!(
            format_id,
            "device_format already declared — immutable, never overwritten"
        );
        return Ok(ReplicatedRowOutcome::Applied);
    }

    db.query("CREATE device_format CONTENT $row")
        .bind(("row", row))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("replicated device_format create failed: {e}"))?;
    Ok(ReplicatedRowOutcome::Applied)
}

/// Inbound `petal.render_distance` strictest-reader check (S6 step 4):
/// `NONE`/null (inherit) is always fine; a numeric value must be finite and
/// within `[0, RENDER_DISTANCE_MAX]`, the same ceiling the schema `ASSERT`
/// and the local write path (`petal_render_distance::validate_render_distance`)
/// enforce — duplicated here because this check runs BEFORE the merge, on
/// the raw wire JSON, not after a typed local write.
fn is_valid_inbound_render_distance(value: &serde_json::Value) -> bool {
    if value.is_null() {
        return true;
    }
    value.as_f64().is_some_and(|v| {
        v.is_finite()
            && (0.0..=crate::handlers::petal_render_distance::RENDER_DISTANCE_MAX).contains(&v)
    })
}

/// Inbound `petal.terrain` strictest-reader check (S6 step 8, critic C7;
/// S8b): fe-database has no fe-terrain dependency, so this is a STRUCTURAL
/// check — see `src/AGENTS.md` §handlers "Petal terrain trust".
/// `terrain_bridge::terrain_config_from_petal_json` is the second line of
/// defence that actually parses the `TerrainConfig`.
const MAX_INBOUND_PETAL_TERRAIN_BYTES: usize = 64 * 1024;

/// Whether `terrain` is safe for the strictest reader; the local writer (`SetPetalTerrain`) applies it too.
pub(crate) fn is_valid_inbound_petal_terrain(terrain: &serde_json::Value) -> bool {
    let Some(obj) = terrain.as_object() else {
        return false;
    };
    let Ok(bytes) = serde_json::to_vec(terrain) else {
        return false;
    };
    if bytes.len() > MAX_INBOUND_PETAL_TERRAIN_BYTES {
        return false;
    }
    let uris_ok = match obj.get("tileset_hexon_uris") {
        None => true,
        Some(v) => match v.as_array() {
            Some(arr) => arr
                .iter()
                .all(|x| x.as_str().is_some_and(fe_runtime::safe_id::is_safe_path_id)),
            None => false,
        },
    };
    // S8b: a filesystem path read by the disk tile cache — relative safe components only.
    let cache_dir_ok = match obj.get("cache_dir") {
        None | Some(serde_json::Value::Null) => true,
        Some(v) => v.as_str().is_some_and(|dir| {
            !dir.is_empty() && dir.split('/').all(fe_runtime::safe_id::is_safe_path_id)
        }),
    };
    // S8b: which environment variable a node reads a secret from is never a peer's choice.
    let api_key_env_ok = obj
        .get("elevation_api_key_env")
        .is_none_or(serde_json::Value::is_null);
    uris_ok
        && cache_dir_ok
        && api_key_env_ok
        && is_valid_inbound_publisher_pins(obj.get("tileset_publisher_dids"), obj)
}

/// RP-4b: the petal-authored streamed-tileset publisher pin map
/// (`tileset_id → did:key`). Absent is fine; present, every key must be a
/// tileset this terrain binds and every value a decodable Ed25519 `did:key`
/// — a malformed pin Denies the row, never silently ignored.
fn is_valid_inbound_publisher_pins(
    pins: Option<&serde_json::Value>,
    terrain: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let Some(pins) = pins else {
        return true;
    };
    let Some(pins) = pins.as_object() else {
        return false;
    };
    let bound: Vec<&str> = terrain
        .get("tileset_hexon_uris")
        .and_then(serde_json::Value::as_array)
        .map(|uris| uris.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default();
    pins.iter().all(|(tileset_id, did)| {
        bound.contains(&tileset_id.as_str())
            && did
                .as_str()
                .is_some_and(|did| fe_identity::did_key::public_key_from_did_key(did).is_ok())
    })
}

/// Render a GeoJSON `coordinates` value as an inline SurrealQL numeric
/// literal (`[[[x, z], …]]`) — the only statement form this SurrealDB line
/// accepts under a `<geometry<polygon>>` cast (see apply_static_row). Every
/// rendered token comes from a validated finite f64, so the interpolation
/// carries no injection surface; `None` on any non-numeric shape.
fn render_polygon_rings(coords: &serde_json::Value) -> Option<String> {
    let mut out = String::from("[");
    for (ri, ring) in coords.as_array()?.iter().enumerate() {
        if ri > 0 {
            out.push(',');
        }
        out.push('[');
        for (pi, point) in ring.as_array()?.iter().enumerate() {
            if pi > 0 {
                out.push(',');
            }
            let pair = point.as_array()?;
            let x = pair.first()?.as_f64()?;
            let z = pair.get(1)?.as_f64()?;
            if !x.is_finite() || !z.is_finite() {
                return None;
            }
            out.push_str(&format!("[{x}, {z}]"));
        }
        out.push(']');
    }
    out.push(']');
    Some(out)
}

/// Whether a row with `row_id` exists in `table`. Tolerates an absent table
/// (fresh peer DB before the schema applies) — same shape as merge.rs.
async fn row_exists(db: &Db, table: &str, id_field: &str, row_id: &str) -> anyhow::Result<bool> {
    let res = db
        .query(format!("SELECT id FROM {table} WHERE {id_field} = $rid LIMIT 1").as_str())
        .bind(("rid", row_id.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("replicated {table} lookup failed: {e}"))?
        .check();
    let rows: Vec<serde_json::Value> = match res {
        Ok(mut r) => r
            .take(0)
            .map_err(|e| anyhow::anyhow!("replicated {table} lookup take failed: {e}"))?,
        Err(e) if e.to_string().contains("does not exist") => Vec::new(),
        Err(e) => return Err(anyhow::anyhow!("replicated {table} lookup failed: {e}")),
    };
    Ok(!rows.is_empty())
}

// ---------------------------------------------------------------------------
// Node table — tombstone-honoring, geometry-safe
// ---------------------------------------------------------------------------

/// Apply one replicated node row: tombstone dominance **and geometry-safe
/// writes** via [`apply_replicated_node_by`] (N-4 / §geometry-inserts — see
/// merge.rs), plus a petal-scoped `SceneChange` for live applies and
/// tombstone applies (A4).
async fn apply_node_row(
    db: &Db,
    verse_id: &str,
    record_id: &str,
    row_bytes: &[u8],
    author_did: &str,
    entity_change_tx: Option<&SceneChangeSender>,
) -> anyhow::Result<ReplicatedRowOutcome> {
    // An empty payload is the wire tombstone — the iroh-docs `del` marker's
    // empty entry (`RowChange::is_tombstone` mirrors this at the seam).
    // Synthesize the node payload the merge path expects; the petal scope
    // for the scene change is resolved from the durable row below, since
    // the wire form carries nothing.
    let mut row: serde_json::Value = if row_bytes.is_empty() {
        serde_json::json!({
            "node_id": record_id,
            "tombstone": { "source": "remote-del" },
        })
    } else {
        serde_json::from_slice(row_bytes)
            .map_err(|e| anyhow::anyhow!("replicated node row {record_id}: bad JSON: {e}"))?
    };

    // merge.rs keys on the payload's `node_id`; fall back to the wire id.
    let node_id = row
        .get("node_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| record_id.to_string());
    if let Some(obj) = row.as_object_mut() {
        obj.insert("node_id".to_string(), serde_json::json!(node_id.clone()));
    }
    let petal_id = row
        .get("petal_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let payload = serde_json::to_vec(&row)?;
    let owner = crate::role_grants::verse_owner(db, verse_id).await?;
    let author = LegacyAuthor { is_owner: owner.as_deref() == Some(author_did), did: author_did };
    let outcome = match apply_replicated_node_by(db, &payload, Some(author)).await? {
        MergeApplied::NotANode | MergeApplied::SkippedLegacyPrecedence => return Ok(ReplicatedRowOutcome::NotApplicable),
        MergeApplied::SkippedTombstoned => ReplicatedRowOutcome::SkippedTombstoned,
        MergeApplied::AppliedTombstone => {
            // The empty-entry tombstone carries no petal — resolve the owning
            // petal from the durable row for the scene change (§scene-change
            // attribution: an absent lookup suppresses the event rather than
            // broadcasting an unscoped delta).
            let petal_id = match petal_id {
                Some(p) => Some(p),
                None => node_petal_id(db, &node_id).await,
            };
            if let Some(petal_id) = petal_id {
                emit_scene_change(
                    entity_change_tx,
                    SceneChange::NodeRemoved {
                        node_id: node_id.clone(),
                        petal_id,
                    },
                );
            }
            ReplicatedRowOutcome::AppliedTombstone
        }
        applied @ (MergeApplied::Applied | MergeApplied::AppliedKeptLocalTransform) => {
            if let Some(petal_id) = petal_id {
                // QA-0100: a losing remote transform must not repaint the scene;
                // describe the durable row, whose transform stayed local.
                let durable = if applied == MergeApplied::AppliedKeptLocalTransform {
                    durable_node_row(db, &node_id).await
                } else {
                    None
                };
                emit_scene_change(
                    entity_change_tx,
                    SceneChange::NodeAdded {
                        node: node_dto_from_row(durable.as_ref().unwrap_or(&row), &node_id, &petal_id),
                    },
                );
            }
            ReplicatedRowOutcome::Applied
        }
    };
    Ok(outcome)
}

/// The durable row's owning petal, for scene-change attribution when the
/// inbound payload carries none (the empty-entry tombstone form). Best-effort:
/// a failed or absent lookup yields `None`, which suppresses the scene event
/// (§scene-change attribution) rather than broadcasting an unscoped delta.
async fn node_petal_id(db: &Db, node_id: &str) -> Option<String> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT petal_id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .ok()?
        .check()
        .ok()?
        .take(0)
        .ok()?;
    rows.first()
        .and_then(|r| r.get("petal_id"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// The durable node row (best-effort), for a scene change that must describe
/// stored state rather than the inbound payload.
async fn durable_node_row(db: &Db, node_id: &str) -> Option<serde_json::Value> {
    let rows: Vec<serde_json::Value> = db
        .query("SELECT * OMIT id FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await
        .ok()?
        .check()
        .ok()?
        .take(0)
        .ok()?;
    rows.into_iter().next()
}

/// Read the payload's `position` as `(x, z)` for the scene-change DTO.
/// Accepts the GeoJSON Point Surreal returns from a `geometry<point>`
/// column or a plain `[x, z]` array (read-only twin of merge.rs's extractor).
fn point_2d_of(row: &serde_json::Value) -> Option<(f64, f64)> {
    let coords = match row.get("position")? {
        serde_json::Value::Array(a) if a.len() >= 2 => Some(a),
        serde_json::Value::Object(o) => o.get("coordinates").and_then(|c| c.as_array()),
        _ => None,
    }?;
    let x = coords.first().and_then(|v| v.as_f64())?;
    let z = coords.get(1).and_then(|v| v.as_f64())?;
    Some((x, z))
}

/// Build the petal-scoped [`NodeDto`] for a live node apply's scene change.
pub(crate) fn node_dto_from_row(row: &serde_json::Value, node_id: &str, petal_id: &str) -> NodeDto {
    let name = row
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(node_id)
        .to_string();
    let elevation = row.get("elevation").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (x, z) = point_2d_of(row).unwrap_or((0.0, 0.0));
    let asset_id = row.get("asset_id").and_then(|v| v.as_str());
    NodeDto {
        node_id: node_id.to_string(),
        petal_id: petal_id.to_string(),
        name,
        position: [x as f32, elevation as f32, z as f32],
        rotation: extract_f32_array(row.get("rotation"), [0.0, 0.0, 0.0, 1.0]),
        scale: extract_f32_array(row.get("scale"), [1.0, 1.0, 1.0]),
        has_asset: asset_id.is_some(),
        // The blob bytes do not ride the row; the asset arrives via the
        // blob-transfer seam and is linked by `asset_id`.
        asset_path: None,
    }
}

/// Read a fixed-size `f32` array field with a default for absent/short values.
fn extract_f32_array<const N: usize>(
    value: Option<&serde_json::Value>,
    default: [f32; N],
) -> [f32; N] {
    let mut out = [0.0f32; N];
    let mut filled = 0usize;
    if let Some(arr) = value.and_then(|v| v.as_array()) {
        for (slot, item) in out.iter_mut().zip(arr.iter()) {
            if let Some(f) = item.as_f64() {
                *slot = f as f32;
                filled += 1;
            }
        }
    }
    if filled == N {
        out
    } else {
        default
    }
}

/// Best-effort scene-change send — a closed channel is shutdown, not an error.
pub(crate) fn emit_scene_change(tx: Option<&SceneChangeSender>, change: SceneChange) {
    if let Some(tx) = tx {
        if tx.send(change).is_err() {
            tracing::warn!("Scene-change channel closed — scene subscriber may have shut down");
        }
    }
}

/// Strip top-level JSON nulls: SurrealDB `option<T>` rejects explicit `null`,
/// and absence means `NONE` (same contract as `Repo`'s create paths, §repo).
fn strip_toplevel_nulls(row: &mut serde_json::Value) {
    if let Some(obj) = row.as_object_mut() {
        obj.retain(|_, v| !v.is_null());
    }
}

// ---------------------------------------------------------------------------
// Tests — durable apply READ-BACK against the real schema (A4)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh in-memory DB with the **real schema** applied (the production
    /// table shapes, geometry columns included) so READ-BACK assertions
    /// exercise the same SCHEMAFULL constraints as the live store.
    async fn schema_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema apply");
        db
    }

    async fn select_json(db: &Db, stmt: &str) -> Vec<serde_json::Value> {
        let mut res = db.query(stmt).await.unwrap();
        res.take::<Vec<serde_json::Value>>(0).unwrap()
    }

    #[tokio::test]
    async fn verse_row_applies_and_reads_back() {
        let db = schema_db().await;
        // Fresh store: the verse manifest itself arrives over the replica —
        // bootstrap admission (see admit_inbound_row), the A2 convergence case.
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-e2e",
            "name": "Replicated Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "namespace_id": null,
            "default_access": "viewer",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-e2e",
            "verse",
            "verse-e2e",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK (not handler-Ok): the durable store holds the row.
        let rows = select_json(
            &db,
            "SELECT verse_id, name FROM verse WHERE verse_id = 'verse-e2e'",
        )
        .await;
        assert_eq!(rows.len(), 1, "verse row must be durably present");
        assert_eq!(rows[0]["name"], "Replicated Verse");
    }

    /// OF-4 (refine R2): a trust-on-first-use manifest naming a placeholder
    /// creator is denied — reconcile's `orphaned_creator` would otherwise hand
    /// the replicated verse to this node at the next startup. The real owner's
    /// manifest then lands, and a startup reconcile leaves its owner alone.
    #[tokio::test]
    async fn a_first_manifest_without_a_did_key_owner_is_denied_and_never_claimed() {
        let db = schema_db().await;
        let local = "did:key:z6MkLocalNode";
        let placeholder = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v-tofu", "name": "V", "created_by": "local-placeholder",
            "created_at": "2026-10-07T00:00:00Z", "default_access": "viewer",
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "v-tofu",
            "verse",
            "v-tofu",
            &placeholder,
            "did:key:peer-x",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        assert!(
            select_json(&db, "SELECT verse_id FROM verse WHERE verse_id = 'v-tofu'")
                .await
                .is_empty(),
            "a denied manifest leaves no verse row to claim"
        );

        let real = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v-tofu", "name": "V", "created_by": "did:key:z6MkRealOwner",
            "created_at": "2026-10-07T00:00:00Z", "default_access": "viewer",
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "v-tofu",
            "verse",
            "v-tofu",
            &real,
            "did:key:z6MkRealOwner",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        crate::reconcile::reconcile(&db, local).await.unwrap();
        let rows = select_json(
            &db,
            "SELECT created_by FROM verse WHERE verse_id = 'v-tofu'",
        )
        .await;
        assert_eq!(rows[0]["created_by"], "did:key:z6MkRealOwner");
    }

    #[tokio::test]
    async fn verse_row_merge_updates_existing() {
        let db = schema_db().await;
        // The verse creator (created_by) — resolves to Owner, so the update
        // passes the A3 gate. The unknown-peer update is covered by
        // `unknown_peer_verse_update_is_denied`.
        let first = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "First",
            "created_by": "did:key:a", "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
        }))
        .unwrap();
        apply_replicated_row_handler(&db, "v1", "verse", "v1", &first, "did:key:a", None)
            .await
            .unwrap();

        let update = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "Renamed by owner",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v1", &update, "did:key:a", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT name, created_by FROM verse WHERE verse_id = 'v1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Renamed by owner", "MERGE updates fields");
        assert_eq!(
            rows[0]["created_by"], "did:key:a",
            "MERGE preserves untouched fields"
        );
    }

    /// F20 fold-in (A3 deny-by-default): an inbound verse manifest carrying a
    /// `default_access` outside {viewer, none} is denied at the replicated
    /// write path — without this, a capability-holding peer could plant
    /// `default_access: "editor"` and make every unknown peer resolve to a
    /// writer for that verse (the schema column has only a DEFAULT, no ASSERT).
    #[tokio::test]
    async fn inbound_verse_manifest_with_editor_default_access_is_denied() {
        let db = schema_db().await;
        // Fresh store: the gate's bootstrap window admits the manifest (this
        // is exactly how a verse converges on a joining peer — A2), so the
        // default_access validation is what must reject it.
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-hijack",
            "name": "Hijacked Access Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "editor",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-hijack",
            "verse",
            "verse-hijack",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        // READ-BACK: the manifest never landed — unknown peers cannot gain a
        // write role through this verse.
        let rows = select_json(
            &db,
            "SELECT verse_id FROM verse WHERE verse_id = 'verse-hijack'",
        )
        .await;
        assert!(
            rows.is_empty(),
            "forbidden default_access manifest must not be applied"
        );
    }

    /// `none` is a valid inbound `default_access` (deny-by-default in the
    /// strictest form) — the validation must not reject safe values.
    #[tokio::test]
    async fn inbound_verse_manifest_with_none_default_access_applies() {
        let db = schema_db().await;
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-none",
            "name": "Private Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "none",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-none",
            "verse",
            "verse-none",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        let rows = select_json(
            &db,
            "SELECT verse_id, default_access FROM verse WHERE verse_id = 'verse-none'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["default_access"], "none");
    }

    /// M2/F6 (A13): a verse manifest carrying the `ts_*` settings columns
    /// applies through the inbound path and READS BACK — this is how a
    /// settings change converges to every peer (the DB handler re-emits the
    /// manifest; each peer's inbound apply persists it).
    #[tokio::test]
    async fn inbound_verse_manifest_with_timeseries_settings_applies_and_reads_back() {
        let db = schema_db().await;
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-ts",
            "name": "Sharded Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
            "ts_mode": "balanced",
            "ts_replication_factor": 2,
            "ts_bucket_width_ms": 3_600_000,
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "verse-ts",
            "verse",
            "verse-ts",
            &row,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the settings persisted on the peer's verse row.
        let rows = select_json(
            &db,
            "SELECT ts_mode, ts_replication_factor, ts_bucket_width_ms FROM verse \
             WHERE verse_id = 'verse-ts'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ts_mode"], "balanced");
        assert_eq!(rows[0]["ts_replication_factor"], 2);
        assert_eq!(rows[0]["ts_bucket_width_ms"], 3_600_000);

        // A settings CHANGE re-delivered as a fresh manifest MERGES over the
        // existing row (the update leg).
        let changed = serde_json::to_vec(&serde_json::json!({
            "verse_id": "verse-ts",
            "name": "Sharded Verse",
            "created_by": "did:key:peer-a",
            "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
            "ts_mode": "sharded",
            "ts_replication_factor": 1,
            "ts_bucket_width_ms": 3_600_000,
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "verse-ts",
            "verse",
            "verse-ts",
            &changed,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        let rows = select_json(&db, "SELECT ts_mode FROM verse WHERE verse_id = 'verse-ts'").await;
        assert_eq!(rows[0]["ts_mode"], "sharded");
    }

    #[tokio::test]
    async fn node_row_with_geojson_position_applies_geometry_safe() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-a", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);
        // Position arrives as the GeoJSON Point Surreal serializes from a
        // geometry<point> column.
        let row = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-geo-1",
            "petal_id": "petal-1",
            "display_name": "Replicated Node",
            "position": { "type": "Point", "coordinates": [4.5, -2.5] },
            "elevation": 12.0,
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "interactive": false,
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-geo-1",
            &row,
            "did:key:peer-a",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the geometry column holds a real point (a raw GeoJSON
        // bind would have failed the SCHEMAFULL write entirely).
        let rows = select_json(
            &db,
            "SELECT position, elevation, display_name FROM node WHERE node_id = 'node-geo-1'",
        )
        .await;
        assert_eq!(rows.len(), 1, "node row must be durably present");
        assert_eq!(rows[0]["elevation"], 12.0);
        let coords = rows[0]["position"]["coordinates"].as_array().unwrap();
        assert_eq!(coords[0].as_f64().unwrap(), 4.5);
        assert_eq!(coords[1].as_f64().unwrap(), -2.5);

        // A4: the node apply emits the petal-scoped SceneChange.
        match scene_rx.try_recv().expect("NodeAdded scene change") {
            SceneChange::NodeAdded { node } => {
                assert_eq!(node.node_id, "node-geo-1");
                assert_eq!(node.petal_id, "petal-1");
                assert_eq!(node.position, [4.5, 12.0, -2.5]);
            }
            other => panic!("expected NodeAdded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn node_tombstone_row_emits_node_removed() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-a", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);
        let row = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-del-1",
            "petal_id": "petal-1",
            "tombstone": { "hlc": 42, "source_did": "did:key:peer-a" },
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del-1",
            &row,
            "did:key:peer-a",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);

        match scene_rx.try_recv().expect("NodeRemoved scene change") {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "node-del-1");
                assert_eq!(petal_id, "petal-1");
            }
            other => panic!("expected NodeRemoved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unhandled_table_is_not_applicable() {
        let db = schema_db().await;
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "some_unreplicated_table",
            "whatever",
            br#"{"role":"editor"}"#,
            "did:key:peer-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::NotApplicable);
    }

    // -------------------------------------------------------------------
    // A3: the inbound role gate — deny-by-default, roles from local tables
    // -------------------------------------------------------------------

    /// Seed a verse row (owner = `created_by`) with `fractal-1` → `petal-1` →
    /// nodes `node-a`/`node-b` (so confinement resolves every reference the
    /// fixtures below carry) plus optional explicit role rows at the verse
    /// scope, exactly as the production paths write them.
    async fn seed_verse_with_roles(db: &Db, verse_id: &str, roles: &[(&str, &str)]) {
        let _: Option<serde_json::Value> = db
            .create("verse")
            .content(serde_json::json!({
                "verse_id": verse_id,
                "name": "Gated Verse",
                "created_by": "did:key:owner-a",
                "created_at": "2026-10-07T00:00:00Z",
                "default_access": "viewer",
            }))
            .await
            .expect("seed verse");
        seed_interior(db, verse_id).await;
        let scope = crate::scope::build_scope(verse_id, None, None);
        for (did, role) in roles {
            crate::rbac::assign_role(db, did, &scope, role)
                .await
                .expect("seed role");
        }
    }

    /// `fractal-1` (in `verse_id`) → `petal-1` → nodes `node-a` and `node-b`.
    async fn seed_interior(db: &Db, verse_id: &str) {
        db.query(
            "CREATE fractal CONTENT { fractal_id: 'fractal-1', verse_id: $vid, \
             owner_did: 'did:key:owner-a', name: 'F', created_at: '2026-10-07T00:00:00Z' };
             CREATE petal CONTENT { petal_id: 'petal-1', fractal_id: 'fractal-1', name: 'P', \
             node_id: 'did:key:owner-a', created_at: '2026-10-07T00:00:00Z' };
             CREATE node CONTENT { node_id: 'node-a', petal_id: 'petal-1', display_name: 'a', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
             scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-07T00:00:00Z' };
             CREATE node CONTENT { node_id: 'node-b', petal_id: 'petal-1', display_name: 'b', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
             scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-07T00:00:00Z' };",
        )
        .bind(("vid", verse_id.to_string()))
        .await
        .expect("seed interior")
        .check()
        .expect("seed interior check");
    }

    fn gated_node_row(node_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "node_id": node_id,
            "petal_id": "petal-1",
            "display_name": "Gated Node",
            "position": { "type": "Point", "coordinates": [1.5, 2.5] },
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn editor_role_row_applies_and_reads_back() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-1",
            &gated_node_row("node-gated-1"),
            "did:key:peer-ed",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the Editor peer's row is durable.
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        // A4 travels with admission: the gated apply still emits the
        // petal-scoped scene change.
        assert!(matches!(
            scene_rx.try_recv(),
            Ok(SceneChange::NodeAdded { .. })
        ));
    }

    /// RP-3: a Viewer's row is not converged-yet, not structural — Deferred
    /// (the dispatch loop's `InboundAdmissions` retries it), never applied now.
    #[tokio::test]
    async fn viewer_role_row_is_deferred_never_applied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-view", "viewer")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-2",
            &gated_node_row("node-gated-2"),
            "did:key:peer-view",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: "did:key:peer-view".to_string()
            })
        );

        // READ-BACK: the Viewer's row never touched the durable store…
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-2'",
        )
        .await;
        assert!(rows.is_empty(), "deferred row must not be persisted");
        // …and emitted no scene change (never re-emitted anywhere).
        assert!(scene_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unknown_peer_row_is_deferred_never_applied() {
        let db = schema_db().await;
        // No role row for the author: resolution falls to the verse's
        // default_access ("viewer"), which must not admit a writer.
        seed_verse_with_roles(&db, "v1", &[]).await;

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-gated-3",
            &gated_node_row("node-gated-3"),
            "did:key:peer-unknown",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: "did:key:peer-unknown".to_string()
            })
        );

        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-gated-3'",
        )
        .await;
        assert!(rows.is_empty(), "unknown peer's row must not be persisted");
    }

    /// F20/M1 finding 2 (reconciliation second chance): a row deferred because
    /// its author's role had not yet converged locally must apply when the
    /// same row is **replayed** (the startup reconciliation path re-applies a
    /// replica's snapshot) with the same authenticated author DID — this only
    /// works because live delivery and snapshot replay attribute authorship
    /// identically (fe-sync imports the endpoint identity as the docs
    /// author). A deferred delivery leaves no trace. (The in-process retry
    /// without any replay is `InboundAdmissions`, tests/role_propagation_inbound_test.rs.)
    #[tokio::test]
    async fn deferred_row_converges_on_replay_after_role_lands() {
        let db = schema_db().await;
        // The verse manifest has converged; carol's Editor role row has NOT.
        seed_verse_with_roles(&db, "v1", &[]).await;
        let row = gated_node_row("node-replay-1");

        // First delivery (live): carol resolves to default_access viewer → deferred.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-replay-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ReplicatedRowOutcome::Deferred(_)));
        assert!(
            select_json(
                &db,
                "SELECT node_id FROM node WHERE node_id = 'node-replay-1'"
            )
            .await
            .is_empty(),
            "deferred row must not be persisted"
        );

        // The role row lands (e.g. it replicated later than carol's rows).
        let scope = crate::scope::build_scope("v1", None, None);
        crate::rbac::assign_role(&db, "did:key:carol", &scope, "editor")
            .await
            .expect("seed carol's editor role");

        // Replay (restart reconciliation): the SAME row, the SAME author DID.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-replay-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Applied,
            "the second chance must converge the previously deferred row"
        );

        // READ-BACK: converged durably.
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-replay-1'",
        )
        .await;
        assert_eq!(
            rows.len(),
            1,
            "replayed row is durable after the role landed"
        );
    }

    /// Re-delivery of an already-applied row is idempotent-harmless (the
    /// reconciliation pass replays the whole snapshot on every open): the
    /// second apply MERGEs onto the existing row and stays `Applied` — the
    /// "denial on re-delivery must stay harmless" contract for rows authored
    /// under a stale identity.
    #[tokio::test]
    async fn already_applied_row_redelivery_stays_applied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:carol", "editor")]).await;
        let row = gated_node_row("node-idem-1");

        let first = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-idem-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(first, ReplicatedRowOutcome::Applied);

        let second = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-idem-1",
            &row,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(second, ReplicatedRowOutcome::Applied);
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-idem-1'",
        )
        .await;
        assert_eq!(rows.len(), 1, "re-delivery does not duplicate the row");
    }

    #[tokio::test]
    async fn unknown_peer_verse_update_is_deferred() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;

        let update = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "Hijacked Name",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v1", &update, "did:key:peer-b", None)
                .await
                .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: "did:key:peer-b".to_string()
            })
        );

        // READ-BACK: the verse row is untouched.
        let rows = select_json(&db, "SELECT name FROM verse WHERE verse_id = 'v1'").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "Gated Verse");
    }

    #[tokio::test]
    async fn verse_row_claiming_foreign_verse_id_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;

        // The payload claims a different verse than the replica it arrived
        // on — the gate would judge verse v1 while the row lands as v2.
        let row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v2", "name": "Injected Verse",
            "created_by": "did:key:owner-a", "created_at": "2026-10-07T00:00:00Z",
            "default_access": "viewer",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "verse", "v2", &row, "did:key:owner-a", None)
                .await
                .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);

        let rows = select_json(&db, "SELECT verse_id FROM verse WHERE verse_id = 'v2'").await;
        assert!(rows.is_empty(), "foreign verse_id must not be planted");
    }

    /// RP-4 step 3 (first-manifest rule): before the manifest only the verse
    /// row is admitted. Out-of-order interior rows are Deferred — never
    /// applied unchecked — and land, with no second delivery, once the
    /// manifest and their parents do (manifest → fractal → petal → node).
    #[tokio::test]
    async fn node_row_before_verse_manifest_is_deferred_then_applied_when_the_manifest_lands() {
        use crate::handlers::inbound_admissions::InboundAdmissions;
        let db = schema_db().await;
        let mut queue = InboundAdmissions::new();
        const OWNER: &str = "did:key:owner-a";

        let early = queue
            .apply(
                &db,
                "v1",
                "node",
                "node-boot-1",
                &gated_node_row("node-boot-1"),
                OWNER,
                None,
            )
            .await;
        assert_eq!(
            early[0].outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::Manifest)
        );
        assert!(select_json(&db, "SELECT node_id FROM node")
            .await
            .is_empty());

        let fractal = serde_json::to_vec(&serde_json::json!({
            "fractal_id": "fractal-1", "verse_id": "v1", "owner_did": OWNER,
            "name": "F", "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();
        let petal = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-1", "fractal_id": "fractal-1", "name": "P",
            "node_id": OWNER, "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();
        // The petal also arrives before the manifest; the fractal after it.
        queue
            .apply(&db, "v1", "petal", "petal-1", &petal, OWNER, None)
            .await;
        let manifest = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v1", "name": "V", "created_by": OWNER,
            "created_at": "2026-10-07T00:00:00Z", "default_access": "viewer",
        }))
        .unwrap();
        let reports = queue
            .apply(&db, "v1", "verse", "v1", &manifest, OWNER, None)
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(
            queue.waiting_reason("v1", "petal", "petal-1", OWNER),
            Some(&DeferralReason::Parent {
                table: "fractal".to_string(),
                id: "fractal-1".to_string()
            }),
            "after the manifest the petal waits on its fractal"
        );

        let reports = queue
            .apply(&db, "v1", "fractal", "fractal-1", &fractal, OWNER, None)
            .await;
        assert!(
            reports
                .iter()
                .any(|r| r.record_id == "node-boot-1" && r.outcome == ReplicatedRowOutcome::Applied),
            "fractal → petal → node settle in one worklist: {reports:?}"
        );
        assert!(queue.is_empty());
        let rows = select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-boot-1'",
        )
        .await;
        assert_eq!(rows.len(), 1);
    }

    // -------------------------------------------------------------------
    // RP-4: owner integrity, referential confinement, scope-precise roles
    // -------------------------------------------------------------------

    fn manifest(fields: serde_json::Value) -> Vec<u8> {
        let mut row = serde_json::json!({
            "verse_id": "v1", "name": "Gated Verse", "created_by": "did:key:owner-a",
            "created_at": "2026-10-07T00:00:00Z", "default_access": "viewer",
        });
        for (key, value) in fields.as_object().unwrap() {
            row[key] = value.clone();
        }
        serde_json::to_vec(&row).unwrap()
    }

    async fn verse_field(db: &Db, field: &str) -> serde_json::Value {
        let rows = select_json(
            db,
            &format!("SELECT {field} FROM verse WHERE verse_id = 'v1'"),
        )
        .await;
        rows[0][field].clone()
    }

    /// T9: the manifest MERGE used to let any admitted Editor rewrite
    /// `created_by` (owner takeover). Identity fields are immutable for
    /// everyone — the owner included — and only a real change is refused.
    #[tokio::test]
    async fn an_editor_cannot_rewrite_created_by_through_a_replicated_manifest() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;

        for (field, value, author) in [
            (
                "created_by",
                serde_json::json!("did:key:peer-ed"),
                "did:key:peer-ed",
            ),
            (
                "created_at",
                serde_json::json!("2030-01-01T00:00:00Z"),
                "did:key:owner-a",
            ),
        ] {
            let outcome = apply_replicated_row_handler(
                &db,
                "v1",
                "verse",
                "v1",
                &manifest(serde_json::json!({ field: value })),
                author,
                None,
            )
            .await
            .unwrap();
            assert_eq!(outcome, ReplicatedRowOutcome::Denied, "{field}");
        }
        assert_eq!(verse_field(&db, "created_by").await, "did:key:owner-a");
        assert_eq!(
            crate::role_manager::resolve_role(&db, "did:key:peer-ed", "VERSE#v1")
                .await
                .unwrap(),
            fe_policy::RoleLevel::Editor,
            "the Editor never became the owner"
        );

        // The same manifest carrying the unchanged identity still applies (a rename).
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "verse",
            "v1",
            &manifest(serde_json::json!({ "name": "Renamed" })),
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(verse_field(&db, "name").await, "Renamed");
    }

    /// Refine R2: filling a namespace_id the local verse row never had picks the
    /// replica this node opens next — an Editor's fill is deferred (it would let
    /// any Editor redirect the verse), a verse Manager's applies once.
    #[tokio::test]
    async fn only_a_verse_manager_may_fill_in_a_missing_namespace_id() {
        let db = schema_db().await;
        seed_verse_with_roles(
            &db,
            "v1",
            &[
                ("did:key:editor", "editor"),
                ("did:key:verse-manager", "manager"),
            ],
        )
        .await;
        let fill = |ns: &str| manifest(serde_json::json!({ "namespace_id": ns }));

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "verse",
            "v1",
            &fill("ns-attacker"),
            "did:key:editor",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: "did:key:editor".to_string()
            })
        );
        assert!(verse_field(&db, "namespace_id").await.is_null());

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "verse",
            "v1",
            &fill("ns-real"),
            "did:key:verse-manager",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(verse_field(&db, "namespace_id").await, "ns-real");
    }

    /// RP-4 step 2: governance fields need Manager+ at the VERSE scope — an
    /// Editor and a petal Manager are deferred (verse settings lie above a
    /// petal Manager's subtree); a verse Manager's change applies.
    #[tokio::test]
    async fn an_editor_or_petal_manager_cannot_flip_ts_mode_or_default_access_but_a_verse_manager_can(
    ) {
        let db = schema_db().await;
        seed_verse_with_roles(
            &db,
            "v1",
            &[
                ("did:key:editor", "editor"),
                ("did:key:verse-manager", "manager"),
            ],
        )
        .await;
        crate::rbac::assign_role(
            &db,
            "did:key:petal-manager",
            "VERSE#v1-FRACTAL#fractal-1-PETAL#petal-1",
            "manager",
        )
        .await
        .unwrap();

        for change in [
            serde_json::json!({ "ts_mode": "sharded" }),
            serde_json::json!({ "default_access": "none" }),
            serde_json::json!({ "ts_replication_factor": 3 }),
        ] {
            for author in ["did:key:editor", "did:key:petal-manager"] {
                let outcome = apply_replicated_row_handler(
                    &db,
                    "v1",
                    "verse",
                    "v1",
                    &manifest(change.clone()),
                    author,
                    None,
                )
                .await
                .unwrap();
                assert_eq!(
                    outcome,
                    ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                        did: author.to_string()
                    }),
                    "{author} {change}"
                );
            }
        }
        assert_eq!(
            verse_field(&db, "ts_mode").await,
            "mirror",
            "schema default kept"
        );
        assert_eq!(verse_field(&db, "default_access").await, "viewer");

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "verse",
            "v1",
            &manifest(serde_json::json!({ "ts_mode": "sharded", "default_access": "none" })),
            "did:key:verse-manager",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(verse_field(&db, "ts_mode").await, "sharded");
        assert_eq!(verse_field(&db, "default_access").await, "none");
    }

    /// Seed a second verse `v2` with `fractal-2` → `petal-2` → `node-foreign`.
    async fn seed_foreign_verse(db: &Db) {
        db.query(
            "CREATE verse CONTENT { verse_id: 'v2', name: 'Other', created_by: 'did:key:owner-a', \
             created_at: '2026-10-07T00:00:00Z', default_access: 'viewer' };
             CREATE fractal CONTENT { fractal_id: 'fractal-2', verse_id: 'v2', \
             owner_did: 'did:key:owner-a', name: 'F2', created_at: '2026-10-07T00:00:00Z' };
             CREATE petal CONTENT { petal_id: 'petal-2', fractal_id: 'fractal-2', name: 'P2', \
             node_id: 'did:key:owner-a', created_at: '2026-10-07T00:00:00Z' };
             CREATE node CONTENT { node_id: 'node-foreign', petal_id: 'petal-2', display_name: 'f', \
             position: <geometry<point>> [0.0, 0.0], elevation: 0.0, rotation: [0.0, 0.0, 0.0, 1.0], \
             scale: [1.0, 1.0, 1.0], interactive: false, created_at: '2026-10-07T00:00:00Z' };",
        )
        .await
        .expect("seed foreign verse")
        .check()
        .expect("seed foreign verse check");
    }

    fn node_in(node_id: &str, petal_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "node_id": node_id, "petal_id": petal_id, "display_name": "N",
            "position": { "type": "Point", "coordinates": [1.0, 1.0] },
            "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0],
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap()
    }

    /// [`node_in`] as the complete versioned snapshot current publishers send.
    fn complete_node_in(node_id: &str, petal_id: &str, publisher: &str) -> Vec<u8> {
        let mut row: serde_json::Value = serde_json::from_slice(&node_in(node_id, petal_id)).unwrap();
        row["elevation"] = serde_json::json!(0.0);
        row["interactive"] = serde_json::json!(true);
        row["edit_seq"] = serde_json::json!(0);
        row["_fe_scene"] = serde_json::json!({
            "version": 1, "complete": true, "hlc": (2u64 << 16).to_string(), "publisher": publisher,
        });
        serde_json::to_vec(&row).unwrap()
    }

    /// P2P-REF-CONFINE acceptance: an Editor admitted in v1 cannot plant a
    /// node under v2's petal through v1's replica — known-foreign is Denied.
    #[tokio::test]
    async fn a_node_row_referencing_a_foreign_verses_petal_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        seed_foreign_verse(&db).await;
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-planted",
            &node_in("node-planted", "petal-2"),
            "did:key:owner-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        assert!(select_json(
            &db,
            "SELECT node_id FROM node WHERE node_id = 'node-planted'"
        )
        .await
        .is_empty());
    }

    /// T11: a tombstone (the empty `del` entry, and a payload tombstone that
    /// names a v1 petal) aimed through v1's replica at v2's node is Denied —
    /// the durable row's parent is confined too — and the node survives.
    #[tokio::test]
    async fn a_tombstone_aimed_at_a_foreign_verses_node_is_denied_and_the_node_survives() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        seed_foreign_verse(&db).await;
        let payload_tombstone = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-foreign", "petal_id": "petal-1",
            "tombstone": { "hlc": 99, "source_did": "did:key:peer-ed" },
        }))
        .unwrap();
        for bytes in [Vec::new(), payload_tombstone] {
            let outcome = apply_replicated_row_handler(
                &db,
                "v1",
                "node",
                "node-foreign",
                &bytes,
                "did:key:peer-ed",
                None,
            )
            .await
            .unwrap();
            assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        }
        let rows = select_json(
            &db,
            "SELECT tombstone, petal_id FROM node WHERE node_id = 'node-foreign'",
        )
        .await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["tombstone"].is_null(), "the foreign node survives");
        assert_eq!(rows[0]["petal_id"], "petal-2");
    }

    /// T11: moving v2's node into a v1 petal through v1's replica is Denied
    /// (the payload's petal is here, but the durable row's is not).
    #[tokio::test]
    async fn a_node_reparent_from_a_foreign_verse_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        seed_foreign_verse(&db).await;
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-foreign",
            &node_in("node-foreign", "petal-1"),
            "did:key:owner-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        let rows = select_json(
            &db,
            "SELECT petal_id FROM node WHERE node_id = 'node-foreign'",
        )
        .await;
        assert_eq!(rows[0]["petal_id"], "petal-2", "the node was not stolen");
    }

    /// RP-4 step 5: an unknown parent is Deferred `Parent` (never Denied) and
    /// the row applies with no second delivery when the parent lands; a
    /// petal row naming no fractal is structural and Denied.
    #[tokio::test]
    async fn a_petal_row_whose_fractal_arrives_later_is_deferred_then_applied() {
        use crate::handlers::inbound_admissions::InboundAdmissions;
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;
        let mut queue = InboundAdmissions::new();
        const OWNER: &str = "did:key:owner-a";
        let petal = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-late", "fractal_id": "fractal-late", "name": "Late",
            "node_id": OWNER, "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();
        let reports = queue
            .apply(&db, "v1", "petal", "petal-late", &petal, OWNER, None)
            .await;
        assert_eq!(
            reports[0].outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::Parent {
                table: "fractal".to_string(),
                id: "fractal-late".to_string()
            })
        );

        let fractal = serde_json::to_vec(&serde_json::json!({
            "fractal_id": "fractal-late", "verse_id": "v1", "owner_did": OWNER,
            "name": "F", "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();
        let reports = queue
            .apply(&db, "v1", "fractal", "fractal-late", &fractal, OWNER, None)
            .await;
        assert!(reports
            .iter()
            .any(|r| r.record_id == "petal-late" && r.outcome == ReplicatedRowOutcome::Applied));
        assert_eq!(
            select_json(
                &db,
                "SELECT petal_id FROM petal WHERE petal_id = 'petal-late'"
            )
            .await
            .len(),
            1
        );

        let orphan = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-orphan", "name": "O", "node_id": OWNER,
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();
        let outcome =
            apply_replicated_row_handler(&db, "v1", "petal", "petal-orphan", &orphan, OWNER, None)
                .await
                .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Denied,
            "no fractal_id is structural"
        );
    }

    /// RP-4b (AC-47, T24): an inbound petal row whose terrain carries a
    /// publisher pin that is not a decodable Ed25519 `did:key` — or that pins
    /// a tileset the terrain does not bind — is Denied and never lands; the
    /// same row with a real did:key pin for a bound tileset applies.
    #[tokio::test]
    async fn an_inbound_petal_terrain_with_a_malformed_publisher_pin_is_denied() {
        const OWNER: &str = "did:key:owner-a";
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;
        let real_pin = fe_identity::NodeKeypair::generate().to_did_key();
        let petal_row = |petal_id: &str, pins: serde_json::Value| {
            serde_json::to_vec(&serde_json::json!({
                "petal_id": petal_id, "fractal_id": "fractal-1", "name": "Pinned",
                "node_id": OWNER, "created_at": "2026-10-07T00:00:00Z",
                "terrain": {
                    "enabled": true,
                    "origin": { "origin_lat": 0.0, "origin_lon": 0.0, "origin_ele": 0.0 },
                    "tile_source_url": "", "elevation_source": "none",
                    "tileset_hexon_uris": ["ts-a"],
                    "tileset_publisher_dids": pins,
                },
            }))
            .unwrap()
        };
        let petal_count = |petal_id: &'static str| {
            let db = db.clone();
            async move {
                select_json(
                    &db,
                    &format!("SELECT petal_id FROM petal WHERE petal_id = '{petal_id}'"),
                )
                .await
                .len()
            }
        };

        for (petal_id, pins) in [
            (
                "petal-bad-grammar",
                serde_json::json!({ "ts-a": "did:key:zNotAKey" }),
            ),
            ("petal-bad-type", serde_json::json!({ "ts-a": 42 })),
            ("petal-unbound", serde_json::json!({ "ts-other": real_pin })),
            ("petal-not-a-map", serde_json::json!([real_pin])),
        ] {
            let outcome = apply_replicated_row_handler(
                &db,
                "v1",
                "petal",
                petal_id,
                &petal_row(petal_id, pins),
                OWNER,
                None,
            )
            .await
            .unwrap();
            assert_eq!(outcome, ReplicatedRowOutcome::Denied, "{petal_id}");
            assert_eq!(petal_count(petal_id).await, 0, "{petal_id} never lands");
        }

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "petal",
            "petal-good-pin",
            &petal_row("petal-good-pin", serde_json::json!({ "ts-a": real_pin })),
            OWNER,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        let rows = select_json(
            &db,
            "SELECT terrain FROM petal WHERE petal_id = 'petal-good-pin'",
        )
        .await;
        assert_eq!(
            rows[0]["terrain"]["tileset_publisher_dids"]["ts-a"],
            serde_json::json!(real_pin)
        );
    }

    /// RP-4 step 6: the author's role is resolved at the row's DEEPEST scope.
    /// An Editor appointed for petal-1 only (by the owner's statement, so the
    /// role is real materialized state) is admitted in petal-1 and deferred in
    /// a sibling petal — before RP-4 the verse-scope gate deferred them everywhere.
    #[tokio::test]
    async fn a_petal_scoped_editor_is_admitted_for_that_petal_and_deferred_elsewhere() {
        use crate::handlers::inbound_admissions::InboundAdmissions;
        const OWNER: &str = "did:key:owner-a";
        const EDITOR: &str = "did:key:petal-editor";
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;
        db.query(
            "CREATE petal CONTENT { petal_id: 'petal-sibling', fractal_id: 'fractal-1', name: 'S', \
             node_id: 'did:key:owner-a', created_at: '2026-10-07T00:00:00Z' }",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let scope = "VERSE#v1-FRACTAL#fractal-1-PETAL#petal-1";
        let statement = fe_policy::RoleStatementPayload {
            scope: scope.to_string(),
            peer_did: EDITOR.to_string(),
            grantor_did: OWNER.to_string(),
            role: "editor".to_string(),
            revoked: false,
            hlc: 1,
            editor_since: None,
            manager_since: None,
        };
        let mut queue = InboundAdmissions::new();
        let reports = queue
            .apply(
                &db,
                "v1",
                "role",
                &format!("{scope}/{EDITOR}/{OWNER}"),
                &serde_json::to_vec(&statement).unwrap(),
                OWNER,
                None,
            )
            .await;
        assert_eq!(reports[0].outcome, ReplicatedRowOutcome::Applied);

        // QA-0131: a markerless row carries no write clock, so a statement-derived
        // Editor cannot show it held the role when writing; the versioned snapshot lands.
        for (row, expected) in [
            (node_in("node-in-petal", "petal-1"), ReplicatedRowOutcome::Denied),
            (complete_node_in("node-in-petal", "petal-1", EDITOR), ReplicatedRowOutcome::Applied),
        ] {
            let here = apply_replicated_row_handler(&db, "v1", "node", "node-in-petal", &row, EDITOR, None)
                .await
                .unwrap();
            assert_eq!(here, expected);
        }

        let elsewhere = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-in-sibling",
            &node_in("node-in-sibling", "petal-sibling"),
            EDITOR,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            elsewhere,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: EDITOR.to_string()
            })
        );

        // Moving their node into the sibling petal needs Editor at BOTH ends.
        let moved = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-in-petal",
            &node_in("node-in-petal", "petal-sibling"),
            EDITOR,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(moved, ReplicatedRowOutcome::Deferred(_)));
        let rows = select_json(
            &db,
            "SELECT petal_id FROM node WHERE node_id = 'node-in-petal'",
        )
        .await;
        assert_eq!(rows[0]["petal_id"], "petal-1");
    }

    /// The scope-injection guard covers the wire key too: a payload naming no
    /// `verse_id` falls back to the key in `apply_static_row`, so a v1 row
    /// keyed `v2` would otherwise MERGE into verse v2 while v1 was judged.
    #[tokio::test]
    async fn a_verse_row_keyed_to_another_verse_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        seed_foreign_verse(&db).await;
        let rename = serde_json::to_vec(&serde_json::json!({ "name": "Hijacked" })).unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "verse",
            "v2",
            &rename,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
        let rows = select_json(&db, "SELECT name FROM verse WHERE verse_id = 'v2'").await;
        assert_eq!(rows[0]["name"], "Other");
    }

    /// A node row whose `petal_id` would smuggle a scope boundary (`#`) into
    /// the role scope is structural and Denied.
    #[tokio::test]
    async fn a_reference_id_carrying_a_scope_boundary_is_denied() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[]).await;
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-x",
            &node_in("node-x", "petal-1-PETAL#other"),
            "did:key:owner-a",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Denied);
    }

    // -------------------------------------------------------------------
    // A5: tombstone dominance through the real inbound path (N-4)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn stale_live_row_cannot_resurrect_locally_tombstoned_node() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let live = gated_node_row("node-n4");

        // 1. The live node converges.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &live,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // 2. An incoming tombstone converges the local row to deleted.
        let tombstone = serde_json::to_vec(&serde_json::json!({
            "node_id": "node-n4", "petal_id": "petal-1",
            "tombstone": { "hlc": 42, "source_did": "did:key:peer-ed" },
        }))
        .unwrap();
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &tombstone,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-n4'").await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]["tombstone"].is_object(),
            "row must read back tombstoned"
        );

        // 3. A stale live row (a replica that never saw the delete) must NOT
        //    resurrect it — N-4 holds through the real inbound path.
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-n4",
            &live,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::SkippedTombstoned);
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-n4'").await;
        assert_eq!(rows.len(), 1);
        assert!(
            rows[0]["tombstone"].is_object(),
            "the tombstone must survive the stale live write"
        );

        // QA-0156: two authors' concurrent writes to one node converge to one result on
        // every peer whatever the arrival order: complete snapshots by (HLC, publisher),
        // clockless markerless rows by the legacy-writer precedence, tombstone markers by value.
        let (left, right) = (schema_db().await, schema_db().await);
        for peer in [&left, &right] {
            seed_verse_with_roles(peer, "v1", &[("did:key:peer-ed", "editor"), ("did:key:peer-ez", "editor")]).await;
        }
        let edit = |node: &str, author: &str, name: &str, hlc: Option<u64>| {
            let mut row: serde_json::Value = serde_json::from_slice(&gated_node_row(node)).unwrap();
            row["display_name"] = serde_json::json!(name);
            row["properties"] = serde_json::json!({ "by": author });
            if let Some(hlc) = hlc {
                row["elevation"] = serde_json::json!(0.0);
                row["interactive"] = serde_json::json!(true);
                row["edit_seq"] = serde_json::json!(0);
                row["_fe_scene"] = serde_json::json!({ "version": 1, "complete": true, "hlc": hlc.to_string(), "publisher": author });
            }
            (author.to_string(), serde_json::to_vec(&row).unwrap())
        };
        let gone = |by: &str| serde_json::to_vec(&serde_json::json!({
            "node_id": "node-q156-gone", "petal_id": "petal-1", "tombstone": { "hlc": 7, "source_did": by },
        })).unwrap();
        let writes = [
            ("node-q156-legacy", vec![edit("node-q156-legacy", "did:key:peer-ed", "ed", None), edit("node-q156-legacy", "did:key:peer-ez", "ez", None)]),
            ("node-q156-complete", vec![edit("node-q156-complete", "did:key:peer-ez", "older", Some(2 << 16)), edit("node-q156-complete", "did:key:peer-ed", "newer", Some(3 << 16))]),
            ("node-q156-gone", vec![edit("node-q156-gone", "did:key:peer-ed", "live", None), ("did:key:peer-ed".to_string(), gone("did:key:peer-ed")), ("did:key:peer-ez".to_string(), gone("did:key:peer-ez"))]),
        ];
        for (node, rows) in &writes {
            for (peer, order) in [(&left, rows.clone()), (&right, rows.iter().rev().cloned().collect::<Vec<_>>())] {
                for (author, row) in order {
                    apply_replicated_row_handler(peer, "v1", "node", node, &row, &author, None).await.unwrap();
                }
            }
            // A deleted node is judged on its marker only: a never-live replica has no name to compare.
            let fields = if *node == "node-q156-gone" { "tombstone" } else { "display_name, properties, tombstone" };
            let read = format!("SELECT {fields} FROM node WHERE node_id = '{node}'");
            assert_eq!(select_json(&left, &read).await, select_json(&right, &read).await, "{node} converges in either arrival order");
        }
        for (node, name) in [("node-q156-complete", "newer"), ("node-q156-legacy", "ez")] {
            let read = format!("SELECT VALUE display_name FROM node WHERE node_id = '{node}'");
            assert_eq!(select_json(&left, &read).await, vec![serde_json::json!(name)], "the later clock and the higher-ranked legacy writer win");
        }
    }

    #[tokio::test]
    async fn empty_entry_tombstone_converges_local_node_and_emits_scene_change() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let (scene_tx, mut scene_rx) = tokio::sync::broadcast::channel::<SceneChange>(8);

        // The node is live locally first.
        apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del",
            &gated_node_row("node-del"),
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();

        // An EMPTY payload is the iroh-docs `del` marker — the wire tombstone
        // form (`RowChange::is_tombstone` at the seam). It must converge the
        // local row to deleted and emit the petal-scoped NodeRemoved, with the
        // petal resolved from the durable row (the wire form carries none).
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "node",
            "node-del",
            b"",
            "did:key:peer-ed",
            Some(&scene_tx),
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::AppliedTombstone);

        // READ-BACK: tombstoned.
        let rows = select_json(&db, "SELECT tombstone FROM node WHERE node_id = 'node-del'").await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["tombstone"].is_object());

        match scene_rx.try_recv().expect("NodeRemoved scene change") {
            SceneChange::NodeRemoved { node_id, petal_id } => {
                assert_eq!(node_id, "node-del");
                assert_eq!(petal_id, "petal-1");
            }
            other => panic!("expected NodeRemoved, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // A6: petal bounds — the polygon geometry twin of the node point case
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn petal_row_with_bounds_applies_geometry_safe() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:peer-ed", "editor")]).await;
        let row = serde_json::to_vec(&serde_json::json!({
            "petal_id": "petal-bounds",
            "fractal_id": "fractal-1",
            "name": "Replicated Petal",
            "node_id": "did:key:peer-ed",
            "bounds": {
                "type": "Polygon",
                "coordinates": [[
                    [-12.0, -8.0], [12.0, -8.0], [12.0, 8.0], [-12.0, 8.0], [-12.0, -8.0]
                ]]
            },
            "created_at": "2026-10-07T00:00:00Z",
        }))
        .unwrap();

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "petal",
            "petal-bounds",
            &row,
            "did:key:peer-ed",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);

        // READ-BACK: the geometry column holds the real polygon (a raw
        // GeoJSON bind would have failed the SCHEMAFULL write entirely).
        let rows = select_json(
            &db,
            "SELECT bounds FROM petal WHERE petal_id = 'petal-bounds'",
        )
        .await;
        assert_eq!(rows.len(), 1, "petal row must be durably present");
        let bounds = &rows[0]["bounds"];
        assert_eq!(bounds["type"], "Polygon");
        let ring = bounds["coordinates"][0].as_array().unwrap();
        assert_eq!(
            ring.len(),
            5,
            "the polygon ring survives the cast round-trip"
        );
        assert_eq!(ring[0][0].as_f64().unwrap(), -12.0);
        assert_eq!(ring[0][1].as_f64().unwrap(), -8.0);
        assert_eq!(ring[2][0].as_f64().unwrap(), 12.0);
        assert_eq!(ring[2][1].as_f64().unwrap(), 8.0);
    }

    // -----------------------------------------------------------------------
    // A12 — timeseries union CRDT (`iot_reading`)
    // -----------------------------------------------------------------------

    /// One `iot_reading` payload shaped exactly as the ingestion seam emits it
    /// (`insert_readings_with_replication`).
    fn reading_row(reading_id: &str, node_id: &str, value: f64) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "reading_id": reading_id,
            "node_id": node_id,
            "petal_id": "petal-1",
            "metric": "temperature_c",
            "value": value,
            "units": "C",
            "recorded_at": "2026-10-07T00:00:00Z",
            "recorded_at_ms": 1_757_000_000_000i64,
            "hlc_timestamp": 1_757_000_000_000i64,
            "source_did": "did:key:carol",
        }))
        .unwrap()
    }

    async fn reading_count(db: &Db) -> usize {
        select_json(db, "SELECT reading_id FROM iot_reading")
            .await
            .len()
    }

    /// A12: re-delivery is idempotent — no duplicates — and an existing reading
    /// is **never** overwritten (union CRDT, not LWW), however many times and
    /// in whatever order the same key arrives.
    #[tokio::test]
    async fn timeseries_reading_redelivery_is_idempotent_and_never_overwrites() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:carol", "editor")]).await;
        let row = reading_row("reading-1", "node-a", 21.5);

        for _ in 0..3 {
            let outcome = apply_replicated_row_handler(
                &db,
                "v1",
                "iot_reading",
                "reading-1",
                &row,
                "did:key:carol",
                None,
            )
            .await
            .unwrap();
            assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        }

        assert_eq!(reading_count(&db).await, 1, "no duplicates");
        let rows = select_json(
            &db,
            "SELECT * FROM iot_reading WHERE reading_id = 'reading-1'",
        )
        .await;
        assert_eq!(rows[0]["value"].as_f64().unwrap(), 21.5);

        // A *conflicting* re-delivery of the same key (a later value) must not
        // overwrite the first fact: the union keeps both writers' facts but a
        // single key is written once.
        let conflicting = reading_row("reading-1", "node-a", 99.0);
        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "iot_reading",
            "reading-1",
            &conflicting,
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        assert_eq!(reading_count(&db).await, 1);
        let rows = select_json(
            &db,
            "SELECT * FROM iot_reading WHERE reading_id = 'reading-1'",
        )
        .await;
        assert_eq!(
            rows[0]["value"].as_f64().unwrap(),
            21.5,
            "an existing reading is never overwritten"
        );
    }

    /// A12: the converged set does not depend on delivery order or count — the
    /// same three readings delivered in any interleaving converge to exactly
    /// those three rows.
    #[tokio::test]
    async fn timeseries_reading_union_converges_regardless_of_delivery_order() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:carol", "editor")]).await;

        let r1 = reading_row("reading-1", "node-a", 1.0);
        let r2 = reading_row("reading-2", "node-a", 2.0);
        let r3 = reading_row("reading-3", "node-b", 3.0);
        // Interleaved with repeats, as a replica pump would deliver them.
        for (id, row) in [
            ("reading-1", &r1),
            ("reading-2", &r2),
            ("reading-1", &r1),
            ("reading-3", &r3),
            ("reading-2", &r2),
            ("reading-3", &r3),
        ] {
            let outcome = apply_replicated_row_handler(
                &db,
                "v1",
                "iot_reading",
                id,
                row,
                "did:key:carol",
                None,
            )
            .await
            .unwrap();
            assert_eq!(outcome, ReplicatedRowOutcome::Applied);
        }

        assert_eq!(reading_count(&db).await, 3, "exactly the union of facts");
    }

    /// A12: readings are never tombstoned — the empty iroh-docs `del` entry is
    /// `NotApplicable` on a timeseries key and the fact survives (union CRDT
    /// has no delete; only a whole-verse teardown removes rows).
    #[tokio::test]
    async fn timeseries_reading_empty_entry_never_removes_the_fact() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:carol", "editor")]).await;

        apply_replicated_row_handler(
            &db,
            "v1",
            "iot_reading",
            "reading-1",
            &reading_row("reading-1", "node-a", 7.0),
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(reading_count(&db).await, 1);

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "iot_reading",
            "reading-1",
            &[],
            "did:key:carol",
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReplicatedRowOutcome::NotApplicable);
        assert_eq!(
            reading_count(&db).await,
            1,
            "a delete entry never removes a timeseries fact"
        );
    }

    /// A3 still governs the timeseries path: a peer below Editor at the verse
    /// scope cannot inject readings (uniform across modes) — Deferred, never applied.
    #[tokio::test]
    async fn timeseries_reading_from_viewer_is_deferred() {
        let db = schema_db().await;
        seed_verse_with_roles(&db, "v1", &[("did:key:viewer", "viewer")]).await;

        let outcome = apply_replicated_row_handler(
            &db,
            "v1",
            "iot_reading",
            "reading-1",
            &reading_row("reading-1", "node-a", 1.0),
            "did:key:viewer",
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole {
                did: "did:key:viewer".to_string()
            })
        );
        assert_eq!(
            reading_count(&db).await,
            0,
            "deferred rows are never applied"
        );
    }
}
