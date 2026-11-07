//! Role statements on the DB plane: the durable admitted/held store, placement, the
//! demote/promote partition worklist and the materialized `role` cache — see
//! src/AGENTS.md §role-propagation (R7, D-RP-1, plan §2.1–§2.4).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use fe_policy::{
    latest_per_grantor, materialize, ChangedDids, EffectiveRoles, GrantedRole, RoleLevel,
    RoleStatement, RoleStatementPayload,
};

use crate::repo::Db;
use crate::{replicate_row, BlobStoreHandle, ReplicationSender};

/// The doc-key table role statements replicate under (`role/{scope}/{peer}/{grantor}`).
pub const ROLE_DOC_TABLE: &str = "role";
/// Held keys one grantor may occupy per verse before a NEW held key is refused (A9).
pub const ROLE_PENDING_PER_GRANTOR: usize = 1024;
/// Bug guard on the promote worklist (each round moves ≥ 1 row, so it terminates anyway).
const MAX_PROMOTION_ROUNDS: usize = 4096;

const GRANT_TABLE: &str = "role_grant";
const PENDING_TABLE: &str = "role_statement_pending";

static ROLE_PENDING_REFUSED: AtomicU64 = AtomicU64::new(0);

/// Inbound statements refused because their grantor exceeded [`ROLE_PENDING_PER_GRANTOR`].
pub fn role_pending_refused_count() -> u64 {
    ROLE_PENDING_REFUSED.load(Ordering::Relaxed)
}

/// Where [`place_statement`] put a statement (plan §2.4 "one live version per key").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Admissible now: stored in `role_grant`.
    Admitted,
    /// Grantor lacks authority (or the manifest is missing): stored in `role_statement_pending`.
    Held,
    /// Not newer than the live version for its key: nothing written (idempotent).
    Superseded,
    /// A NEW held key beyond the grantor's budget: nothing written, counted.
    RefusedOverBudget,
}

/// The doc record id of a statement: `{scope}/{peer_did}/{grantor_did}`.
pub fn statement_record_id(stmt: &RoleStatement) -> String {
    format!("{}/{}/{}", stmt.scope, stmt.peer_did, stmt.grantor_did)
}

/// The canonical wire payload bytes of a statement (deterministic: serde field order).
pub fn statement_payload_bytes(stmt: &RoleStatement) -> Vec<u8> {
    let payload = RoleStatementPayload {
        scope: stmt.scope.clone(),
        peer_did: stmt.peer_did.clone(),
        grantor_did: stmt.grantor_did.clone(),
        role: stmt.role.as_str().to_string(),
        revoked: stmt.revoked,
        hlc: stmt.hlc,
        editor_since: stmt.editor_since,
        manager_since: stmt.manager_since,
    };
    serde_json::to_vec(&payload).unwrap_or_default()
}

/// True for the canonical-log interim authority verse key shape (64 lowercase hex) —
/// those `role` rows are structurally out of this module's reach (AC-45, T18).
pub fn is_interim_authority_verse_id(verse_id: &str) -> bool {
    verse_id.len() == 64
        && verse_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The verse id a hierarchy scope string belongs to, or `None` for anything that is not a
/// `build_scope` string of a non-interim verse.
pub fn verse_of_scope(scope: &str) -> Option<String> {
    let parts = crate::parse_scope(scope).ok()?;
    if is_interim_authority_verse_id(&parts.verse_id) {
        return None;
    }
    // `parse_scope` folds an unknown trailing segment (e.g. an interim `-RESOURCE#<hex>`
    // key) into the last id; no hierarchy id contains `#`, so such a scope is not ours.
    let ids = [
        Some(&parts.verse_id),
        parts.fractal_id.as_ref(),
        parts.petal_id.as_ref(),
    ];
    if ids.into_iter().flatten().any(|id| id.contains('#')) {
        return None;
    }
    Some(parts.verse_id)
}

/// Publish one statement on the replication seam (drop-and-count, never blocks).
pub fn publish_statement(
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    stmt: &RoleStatement,
) {
    replicate_row(
        repl_tx,
        blob_store,
        verse_id,
        ROLE_DOC_TABLE,
        &statement_record_id(stmt),
        &statement_payload_bytes(stmt),
    );
}

/// The verse owner (`verse.created_by`), or `None` before the manifest lands.
pub async fn verse_owner(db: &Db, verse_id: &str) -> anyhow::Result<Option<String>> {
    let mut res = db
        .query("SELECT created_by FROM verse WHERE verse_id = $v LIMIT 1")
        .bind(("v", verse_id.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role owner lookup failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|row| row.get("created_by"))
        .and_then(serde_json::Value::as_str)
        .filter(|did| !did.is_empty())
        .map(str::to_string))
}

fn statement_from_row(row: &serde_json::Value) -> Option<RoleStatement> {
    let text = |field: &str| row.get(field).and_then(serde_json::Value::as_str);
    Some(RoleStatement {
        scope: text("scope")?.to_string(),
        peer_did: text("peer_did")?.to_string(),
        grantor_did: text("grantor_did")?.to_string(),
        role: GrantedRole::parse(text("role")?)?,
        revoked: row.get("revoked").and_then(serde_json::Value::as_bool)?,
        hlc: text("hlc")?.parse().ok()?,
        editor_since: since(row, "editor_since")?,
        manager_since: since(row, "manager_since")?,
    })
}

/// An optional decimal-string continuity column: `Some(None)` when absent, `None` when malformed.
fn since(row: &serde_json::Value, field: &str) -> Option<Option<u64>> {
    match row.get(field) {
        None | Some(serde_json::Value::Null) => Some(None),
        Some(value) => value.as_str()?.parse().ok().map(Some),
    }
}

fn statements_from_rows(table: &str, rows: Vec<serde_json::Value>) -> Vec<RoleStatement> {
    rows.iter()
        .filter_map(|row| {
            let parsed = statement_from_row(row);
            if parsed.is_none() {
                tracing::warn!(table, ?row, "skipping malformed role statement row");
            }
            parsed
        })
        .collect()
}

async fn select_statements(
    db: &Db,
    sql: &str,
    table: &str,
    binds: Vec<(&'static str, String)>,
) -> anyhow::Result<Vec<RoleStatement>> {
    let mut query = db.query(sql.to_string());
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    let mut res = query
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("{table} read failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(statements_from_rows(table, rows))
}

/// Every ADMITTED statement of a verse.
pub async fn load_admitted(db: &Db, verse_id: &str) -> anyhow::Result<Vec<RoleStatement>> {
    select_statements(
        db,
        "SELECT * FROM role_grant WHERE verse_id = $v",
        GRANT_TABLE,
        vec![("v", verse_id.to_string())],
    )
    .await
}

/// Every HELD statement of a verse (diagnostics and tests — the partition never scans this).
pub async fn load_held(db: &Db, verse_id: &str) -> anyhow::Result<Vec<RoleStatement>> {
    select_statements(
        db,
        "SELECT * FROM role_statement_pending WHERE verse_id = $v",
        PENDING_TABLE,
        vec![("v", verse_id.to_string())],
    )
    .await
}

async fn load_held_by_grantor(
    db: &Db,
    verse_id: &str,
    grantor_did: &str,
) -> anyhow::Result<Vec<RoleStatement>> {
    select_statements(
        db,
        "SELECT * FROM role_statement_pending WHERE verse_id = $v AND grantor_did = $g",
        PENDING_TABLE,
        vec![("v", verse_id.to_string()), ("g", grantor_did.to_string())],
    )
    .await
}

async fn held_count_for_grantor(
    db: &Db,
    verse_id: &str,
    grantor_did: &str,
) -> anyhow::Result<usize> {
    let mut res = db
        .query(
            "SELECT count() AS c FROM role_statement_pending \
             WHERE verse_id = $v AND grantor_did = $g GROUP ALL",
        )
        .bind(("v", verse_id.to_string()))
        .bind(("g", grantor_did.to_string()))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("held-statement count failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .first()
        .and_then(|row| row.get("c"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize)
}

/// The live version of `stmt`'s key and whether it is admitted (in `role_grant`).
async fn live_version(
    db: &Db,
    stmt: &RoleStatement,
) -> anyhow::Result<Option<(bool, RoleStatement)>> {
    let key = statement_record_id(stmt);
    let mut res = db
        .query(
            "SELECT * FROM role_grant WHERE statement_key = $k; \
             SELECT * FROM role_statement_pending WHERE statement_key = $k;",
        )
        .bind(("k", key))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role statement lookup failed: {e}"))?;
    let admitted: Vec<serde_json::Value> = res.take(0)?;
    let held: Vec<serde_json::Value> = res.take(1)?;
    let admitted = statements_from_rows(GRANT_TABLE, admitted);
    let held = statements_from_rows(PENDING_TABLE, held);
    // One live version per key is the invariant; if a crash ever left two, the newer wins.
    let newest = latest_per_grantor(admitted.iter().chain(held.iter()))
        .into_iter()
        .next();
    Ok(newest.map(|winner| {
        let in_grant = admitted.contains(&winner);
        (in_grant, winner)
    }))
}

fn statement_row(verse_id: &str, stmt: &RoleStatement, held: bool) -> serde_json::Value {
    let now = chrono::Utc::now().to_rfc3339();
    let mut row = serde_json::json!({
        "statement_key": statement_record_id(stmt),
        "verse_id": verse_id,
        "scope": stmt.scope,
        "peer_did": stmt.peer_did,
        "grantor_did": stmt.grantor_did,
        "role": stmt.role.as_str(),
        "revoked": stmt.revoked,
        "hlc": stmt.hlc.to_string(),
        "updated_at": now,
    });
    for (field, since) in [("editor_since", stmt.editor_since), ("manager_since", stmt.manager_since)] {
        if let Some(since) = since {
            row[field] = serde_json::Value::String(since.to_string());
        }
    }
    if held {
        row["received_at"] = serde_json::Value::String(now);
    }
    row
}

/// Make `stmt` its key's one live version, in `role_grant` (`held = false`) or
/// `role_statement_pending` (`held = true`) — one transaction, so a key is never in both.
async fn write_live_version(
    db: &Db,
    verse_id: &str,
    stmt: &RoleStatement,
    held: bool,
) -> anyhow::Result<()> {
    let target = if held { PENDING_TABLE } else { GRANT_TABLE };
    let script = format!(
        "BEGIN TRANSACTION;\n\
         DELETE role_grant WHERE statement_key = $k;\n\
         DELETE role_statement_pending WHERE statement_key = $k;\n\
         CREATE {target} CONTENT $row;\n\
         COMMIT TRANSACTION;"
    );
    db.query(script)
        .bind(("k", statement_record_id(stmt)))
        .bind(("row", statement_row(verse_id, stmt, held)))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("writing role statement into {target} failed: {e}"))?;
    Ok(())
}

async fn delete_statement(db: &Db, stmt: &RoleStatement) -> anyhow::Result<()> {
    db.query(
        "BEGIN TRANSACTION;\n\
         DELETE role_grant WHERE statement_key = $k;\n\
         DELETE role_statement_pending WHERE statement_key = $k;\n\
         COMMIT TRANSACTION;",
    )
    .bind(("k", statement_record_id(stmt)))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("deleting role statement failed: {e}"))?;
    Ok(())
}

/// The current materialization of a verse over its admitted statements, or `None` before
/// the manifest (no owner known yet).
pub async fn load_effective_roles(
    db: &Db,
    verse_id: &str,
) -> anyhow::Result<Option<EffectiveRoles>> {
    let Some(owner) = verse_owner(db, verse_id).await? else {
        return Ok(None);
    };
    let admitted = load_admitted(db, verse_id).await?;
    Ok(Some(materialize(&owner, admitted.iter())))
}

/// QA-0131: false iff `author`'s Editor+ at `scope` comes from admitted statements
/// that did not yet grant it at write clock `hlc` — a row written while unauthorized
/// stays refused after a re-grant. Authority not derived from statements is left to
/// the current-role gate; src/AGENTS.md §write-time-role.
pub async fn written_while_authorized(
    db: &Db,
    verse_id: &str,
    author: &str,
    scope: &str,
    hlc: u64,
) -> anyhow::Result<bool> {
    let Some(owner) = verse_owner(db, verse_id).await? else {
        return Ok(true);
    };
    if owner == author {
        return Ok(true);
    }
    let admitted = load_admitted(db, verse_id).await?;
    if !materialize(&owner, admitted.iter())
        .role_at(author, scope)
        .is_at_least(RoleLevel::Editor)
    {
        return Ok(true);
    }
    Ok(fe_policy::materialize_at(&owner, admitted.iter(), hlc)
        .role_at(author, scope)
        .is_at_least(RoleLevel::Editor))
}

/// Place one VALIDATED statement (`fe_policy::validate_statement` already passed) under
/// §2.4's rules: one live version per key, admit if admissible, else hold within the
/// grantor's budget. Never touches the `role` cache — call [`repartition`] after `Admitted`.
pub async fn place_statement(
    db: &Db,
    verse_id: &str,
    stmt: &RoleStatement,
) -> anyhow::Result<Placement> {
    if verse_of_scope(&stmt.scope).as_deref() != Some(verse_id) {
        anyhow::bail!(
            "role statement scope {} is not in verse {verse_id}",
            stmt.scope
        );
    }
    let live = live_version(db, stmt).await?;
    if let Some((_, existing)) = &live {
        let newer = latest_per_grantor([existing, stmt]).into_iter().next();
        if existing == stmt || newer.as_ref() != Some(stmt) {
            return Ok(Placement::Superseded);
        }
    }

    let admissible = match verse_owner(db, verse_id).await? {
        None => false,
        Some(owner) => {
            if stmt.peer_did == owner {
                anyhow::bail!("role statement targets the verse owner (validate first)");
            }
            let admitted = load_admitted(db, verse_id).await?;
            materialize(&owner, admitted.iter()).admissible(stmt)
        }
    };
    if admissible {
        write_live_version(db, verse_id, stmt, false).await?;
        return Ok(Placement::Admitted);
    }

    // The budget applies only when a NEW held key would be created (§2.4).
    if live.is_none()
        && held_count_for_grantor(db, verse_id, &stmt.grantor_did).await?
            >= ROLE_PENDING_PER_GRANTOR
    {
        let refused = ROLE_PENDING_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            verse_id,
            grantor = %stmt.grantor_did,
            refused_total = refused,
            "role statement refused: grantor's held-statement budget is full"
        );
        return Ok(Placement::RefusedOverBudget);
    }
    write_live_version(db, verse_id, stmt, true).await?;
    Ok(Placement::Held)
}

/// [`place_statement`] then, when admitted, [`repartition`] against the pre-placement
/// materialization — the one call both the inbound path and tests use.
pub async fn submit_statement(
    db: &Db,
    verse_id: &str,
    stmt: &RoleStatement,
) -> anyhow::Result<(Placement, ChangedDids)> {
    let prior = load_effective_roles(db, verse_id).await?;
    let placement = place_statement(db, verse_id, stmt).await?;
    let changed = if placement == Placement::Admitted {
        repartition(db, verse_id, prior.as_ref()).await?
    } else {
        ChangedDids::default()
    };
    Ok((placement, changed))
}

/// Store a LOCAL statement (its caller passed the synchronous authority check) as the key's
/// admitted live version; [`repartition`] settles placement afterwards.
pub async fn admit_local_statement(
    db: &Db,
    verse_id: &str,
    stmt: &RoleStatement,
) -> anyhow::Result<()> {
    if let Some((_, existing)) = live_version(db, stmt).await? {
        let newer = latest_per_grantor([&existing, stmt]).into_iter().next();
        if newer.as_ref() != Some(stmt) {
            anyhow::bail!(
                "a newer statement already exists for {}",
                statement_record_id(stmt)
            );
        }
    }
    write_live_version(db, verse_id, stmt, false).await
}

/// Commit a locally authorized statement and its operation under captured fences.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit_local_statement(
    db: &Db, entry: crate::types::OpLogEntry, verse_id: &str, caller: &str,
    peer: &str, scope: &str, role: GrantedRole, revoked: bool,
    authority: crate::handlers::entity::ParentMutation, owner: &str,
) -> anyhow::Result<RoleStatement> {
    let mut candidate = RoleStatement { scope: scope.into(), peer_did: peer.into(), grantor_did: caller.into(), role, revoked, hlc: 0, editor_since: None, manager_since: None };
    let key = statement_record_id(&candidate);
    let grant_select = "SELECT statement_key,verse_id,scope,peer_did,grantor_did,role,revoked,hlc,updated_at FROM role_grant WHERE statement_key=$local_role_key";
    let pending_select = "SELECT statement_key,verse_id,scope,peer_did,grantor_did,role,revoked,hlc,updated_at,received_at FROM role_statement_pending WHERE statement_key=$local_role_key";
    let mut response=db.query(format!("{grant_select};{pending_select}"))
        .bind(("local_role_key",key.clone())).await?.check()?;
    let granted:Vec<serde_json::Value>=response.take(0)?;
    let pending:Vec<serde_json::Value>=response.take(1)?;
    anyhow::ensure!(granted.len() <= 1 && pending.len() <= 1,"Ambiguous role statement identity");
    let prior = live_version(db,&candidate).await?.map(|(_,statement)|statement);
    let verse_id=verse_id.to_owned();
    let (mut guards,mut bindings)=authority.into_parts();
    guards.push(format!("IF ({grant_select}) != $local_role_granted {{ THROW 'role statement changed'; }}"));
    guards.push(format!("IF ({pending_select}) != $local_role_pending {{ THROW 'role statement changed'; }}"));
    guards.push("IF array::len((SELECT verse_id FROM verse WHERE verse_id=$local_role_verse AND created_by=$local_role_owner)) != 1 { THROW 'role owner changed'; }".into());
    guards.push("IF $local_role_allowed = false { THROW 'newer role statement exists'; }".into());
    guards.push("DELETE role_grant WHERE statement_key=$local_role_key".into());
    guards.push("DELETE role_statement_pending WHERE statement_key=$local_role_key".into());
    guards.push("CREATE role_grant CONTENT $local_role_row".into());
    bindings.extend([
        ("local_role_key".into(),serde_json::json!(key)),
        ("local_role_granted".into(),serde_json::json!(granted)),
        ("local_role_pending".into(),serde_json::json!(pending)),
        ("local_role_verse".into(),serde_json::json!(verse_id)),
        ("local_role_owner".into(),serde_json::json!(owner)),
    ]);
    let mut committed = None;
    crate::op_log::commit_operation_atomic_with(db,entry,|clock| {
        candidate.hlc=clock;
        // QA-0131: a change that stays at Editor+/Manager keeps the key's continuity.
        candidate=candidate.clone().continuing(prior.as_ref());
        let allowed=prior.as_ref().is_none_or(|prior| latest_per_grantor([prior,&candidate]).first()==Some(&candidate));
        bindings.push(("local_role_allowed".into(),serde_json::json!(allowed)));
        bindings.push(("local_role_row".into(),statement_row(&verse_id,&candidate,false)));
        committed=Some(candidate.clone());
        (guards,bindings)
    }).await?;
    Ok(committed.expect("atomic statement builder ran"))
}

/// Each `(scope, peer)` cell the statements mention.
fn cells(statements: &[RoleStatement]) -> BTreeSet<(String, String)> {
    statements
        .iter()
        .map(|s| (s.scope.clone(), s.peer_did.clone()))
        .collect()
}

/// DIDs whose effective role rose / fell at any mentioned cell between two materializations.
/// A rise of authority at any scope implies a rise at some mentioned cell on its walk, so
/// `risen` is a superset of "whose authority rose" (src/AGENTS.md §role-propagation).
fn changed_between(
    prior: &EffectiveRoles,
    now: &EffectiveRoles,
    statements: &[RoleStatement],
) -> ChangedDids {
    let mut risen = BTreeSet::new();
    let mut fallen = BTreeSet::new();
    for (scope, peer) in cells(statements) {
        let (before, after) = (prior.role_at(&peer, &scope), now.role_at(&peer, &scope));
        if after > before {
            risen.insert(peer);
        } else if after < before {
            fallen.insert(peer);
        }
    }
    ChangedDids {
        risen: risen.into_iter().collect(),
        fallen: fallen.into_iter().collect(),
    }
}

/// §2.4 "partition by current authority": demote admitted statements that are no longer
/// admissible, promote held statements of every DID whose authority rose (indexed lookups
/// only — a flooder's held rows are never scanned unless it gains authority), repeat until
/// quiescent, then diff-write the `role` cache. `prior = None` (startup, migration,
/// manifest landing) seeds the risen set with the owner and every admitted peer.
pub async fn repartition(
    db: &Db,
    verse_id: &str,
    prior: Option<&EffectiveRoles>,
) -> anyhow::Result<ChangedDids> {
    if is_interim_authority_verse_id(verse_id) {
        return Ok(ChangedDids::default());
    }
    let Some(owner) = verse_owner(db, verse_id).await? else {
        // Pre-manifest: nobody has authority, everything stays held (§2.4).
        return Ok(ChangedDids::default());
    };

    let mut admitted = load_admitted(db, verse_id).await?;
    let mut current = materialize(&owner, admitted.iter());

    // Demote: never refused, never dropped, never moves `current` (the fixpoint already
    // ignores an inadmissible statement).
    let (keep, demote): (Vec<RoleStatement>, Vec<RoleStatement>) = admitted
        .into_iter()
        .partition(|stmt| current.admissible(stmt));
    for stmt in &demote {
        write_live_version(db, verse_id, stmt, true).await?;
    }
    admitted = keep;

    let mut worklist: BTreeSet<String> = match prior {
        Some(prior) => changed_between(prior, &current, &admitted)
            .risen
            .into_iter()
            .collect(),
        None => std::iter::once(owner.clone())
            .chain(admitted.iter().map(|s| s.peer_did.clone()))
            .collect(),
    };

    let mut rounds = 0;
    while !worklist.is_empty() {
        rounds += 1;
        if rounds > MAX_PROMOTION_ROUNDS {
            tracing::warn!(
                verse_id,
                rounds,
                "role partition hit its round bound — bug guard (plan §5 risk 1)"
            );
            break;
        }
        let mut promoted = Vec::new();
        for grantor in std::mem::take(&mut worklist) {
            for held in load_held_by_grantor(db, verse_id, &grantor).await? {
                if !current.admissible(&held) {
                    continue;
                }
                // Full re-validation: the owner-dependent check first runs here for a
                // statement held before the manifest landed.
                if held.peer_did == owner {
                    tracing::warn!(
                        verse_id,
                        key = %statement_record_id(&held),
                        "held role statement targets the owner — denied at promotion"
                    );
                    delete_statement(db, &held).await?;
                    continue;
                }
                write_live_version(db, verse_id, &held, false).await?;
                promoted.push(held);
            }
        }
        if promoted.is_empty() {
            break;
        }
        admitted.extend(promoted);
        let next = materialize(&owner, admitted.iter());
        worklist = changed_between(&current, &next, &admitted)
            .risen
            .into_iter()
            .collect();
        current = next;
    }

    write_cache(db, verse_id, &current).await?;

    Ok(match prior {
        Some(prior) => changed_between(prior, &current, &admitted),
        None => ChangedDids {
            risen: admitted
                .iter()
                .filter(|s| current.role_at(&s.peer_did, &s.scope) > RoleLevel::None)
                .map(|s| s.peer_did.clone())
                .chain(std::iter::once(owner))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            fallen: Vec::new(),
        },
    })
}

/// Recompute a verse's materialization from its admitted statements and diff-write the
/// `role` cache (no partition step — [`repartition`] is the normal entry point).
pub async fn rematerialize(db: &Db, verse_id: &str) -> anyhow::Result<Option<EffectiveRoles>> {
    if is_interim_authority_verse_id(verse_id) {
        return Ok(None);
    }
    let Some(effective) = load_effective_roles(db, verse_id).await? else {
        return Ok(None);
    };
    write_cache(db, verse_id, &effective).await?;
    Ok(Some(effective))
}

/// Diff-write the §2.2 step 4 projection for ONE verse in one transaction, touching only
/// `role` rows whose scope parses to this verse (never an interim `VERSE#<hex>` row).
async fn write_cache(db: &Db, verse_id: &str, effective: &EffectiveRoles) -> anyhow::Result<()> {
    if is_interim_authority_verse_id(verse_id) {
        return Ok(());
    }
    let desired: BTreeMap<(String, String), String> = effective
        .cache_rows()
        .into_iter()
        .filter(|row| verse_of_scope(&row.scope).as_deref() == Some(verse_id))
        .map(|row| ((row.scope, row.peer_did), row.role.to_string()))
        .collect();

    let mut res = db
        .query(
            "SELECT peer_did, scope, role FROM role \
             WHERE scope = $verse_scope OR string::starts_with(scope, $fractal_prefix)",
        )
        .bind(("verse_scope", crate::build_scope(verse_id, None, None)))
        .bind(("fractal_prefix", format!("VERSE#{verse_id}-FRACTAL#")))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role cache read failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let mut existing: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for row in rows {
        let (Some(scope), Some(peer), Some(role)) = (
            row.get("scope").and_then(serde_json::Value::as_str),
            row.get("peer_did").and_then(serde_json::Value::as_str),
            row.get("role").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        if verse_of_scope(scope).as_deref() != Some(verse_id) {
            continue;
        }
        existing
            .entry((scope.to_string(), peer.to_string()))
            .or_default()
            .push(role.to_string());
    }

    fn push_delete(
        statements: &mut Vec<String>,
        bindings: &mut Vec<(String, String)>,
        scope: &str,
        peer: &str,
    ) {
        let n = statements.len();
        statements.push(format!(
            "DELETE role WHERE scope = $s{n} AND peer_did = $p{n};"
        ));
        bindings.push((format!("s{n}"), scope.to_string()));
        bindings.push((format!("p{n}"), peer.to_string()));
    }

    let mut statements = Vec::new();
    let mut bindings: Vec<(String, String)> = Vec::new();
    for (scope, peer) in existing.keys() {
        if !desired.contains_key(&(scope.clone(), peer.clone())) {
            push_delete(&mut statements, &mut bindings, scope, peer);
        }
    }
    for ((scope, peer), role) in &desired {
        let key = (scope.clone(), peer.clone());
        if existing.get(&key).map(Vec::as_slice) == Some(std::slice::from_ref(role)) {
            continue;
        }
        if existing.contains_key(&key) {
            push_delete(&mut statements, &mut bindings, scope, peer);
        }
        let n = statements.len();
        statements.push(format!(
            "CREATE role CONTENT {{ peer_did: $p{n}, scope: $s{n}, role: $r{n} }};"
        ));
        bindings.push((format!("s{n}"), scope.clone()));
        bindings.push((format!("p{n}"), peer.clone()));
        bindings.push((format!("r{n}"), role.clone()));
    }
    if statements.is_empty() {
        return Ok(());
    }

    let mut script = String::from("BEGIN TRANSACTION;\n");
    for statement in &statements {
        script.push_str(statement);
        script.push('\n');
    }
    script.push_str("COMMIT TRANSACTION;");
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    query
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("role cache write for verse {verse_id} failed: {e}"))?;
    Ok(())
}

/// [`repartition`] every local verse (DB startup's crash repair; also after admin clears).
pub async fn repartition_all(db: &Db) -> anyhow::Result<usize> {
    let mut res = db
        .query("SELECT verse_id FROM verse")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("verse list failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    let mut done = 0;
    for verse_id in rows
        .iter()
        .filter_map(|row| row.get("verse_id").and_then(serde_json::Value::as_str))
    {
        if is_interim_authority_verse_id(verse_id) {
            continue;
        }
        match repartition(db, verse_id, None).await {
            Ok(_) => done += 1,
            Err(e) => tracing::error!(verse_id, "role repartition failed: {e:#}"),
        }
    }
    Ok(done)
}

/// AC-42 durable re-emit: publish every live statement this node authored (one per key —
/// admitted or held) for one verse or all verses. iroh dedupes identical bytes, so a
/// re-emit of an already-seen statement costs nothing; the live version is always the
/// newest for its key, so a re-emit can never replay a superseded grant.
pub async fn reemit_local_statements(
    db: &Db,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
    local_did: &str,
    verse_id: Option<&str>,
) -> anyhow::Result<usize> {
    if repl_tx.is_none() {
        return Ok(0);
    }
    let mut statements = Vec::new();
    for table in [GRANT_TABLE, PENDING_TABLE] {
        let (sql, binds) = match verse_id {
            Some(verse) => (
                format!("SELECT * FROM {table} WHERE verse_id = $v AND grantor_did = $g"),
                vec![("v", verse.to_string()), ("g", local_did.to_string())],
            ),
            None => (
                format!("SELECT * FROM {table} WHERE grantor_did = $g"),
                vec![("g", local_did.to_string())],
            ),
        };
        statements.extend(select_statements(db, &sql, table, binds).await?);
    }
    let mut live = latest_per_grantor(statements.iter());
    // Revocations take the first bridge slots: if the budget runs out, what is lost is a
    // grant (re-emitted next open), never a revocation (src/AGENTS.md §role-propagation).
    live.sort_by_key(|stmt| !stmt.revoked);
    let Some(tx) = repl_tx else { return Ok(0) };
    let deadline = tokio::time::Instant::now() + REEMIT_BACKPRESSURE_BUDGET;
    let (mut emitted, mut dropped, mut dropped_revocations) = (0, 0, 0);
    for stmt in &live {
        let Some(stmt_verse) = verse_of_scope(&stmt.scope) else {
            continue;
        };
        if publish_paced(tx, blob_store, &stmt_verse, stmt, deadline).await {
            emitted += 1;
        } else {
            dropped += 1;
            dropped_revocations += usize::from(stmt.revoked);
        }
    }
    if dropped > 0 {
        tracing::warn!(
            emitted,
            dropped,
            dropped_revocations,
            "role statement re-emit hit its backpressure budget — the rest retry at the next replica open"
        );
    }
    Ok(emitted)
}

/// How long one re-emit may wait on a full replication bridge before dropping the rest.
const REEMIT_BACKPRESSURE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
/// Pause between retries of a re-emitted statement while the bridge is full.
const REEMIT_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(5);

/// Publish one statement, waiting (async, never past `deadline`) while the bridge is full —
/// the re-emit is AC-42's only second chance, so it may not drop on a momentary burst the
/// way live publishes do. `false` = not delivered (counted as a replication drop).
async fn publish_paced(
    tx: &ReplicationSender,
    blob_store: &BlobStoreHandle,
    verse_id: &str,
    stmt: &RoleStatement,
    deadline: tokio::time::Instant,
) -> bool {
    let record_id = statement_record_id(stmt);
    let content_hash = match blob_store.add_blob(&statement_payload_bytes(stmt)) {
        Ok(hash) => hash,
        Err(e) => {
            tracing::error!(record_id, "role statement re-emit: blob write failed: {e}");
            return false;
        }
    };
    let mut event = crate::ReplicationEvent {
        verse_id: verse_id.to_string(),
        table: ROLE_DOC_TABLE.to_string(),
        record_id,
        content_hash,
        petal_id: None,
    };
    loop {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(crossbeam::channel::TrySendError::Full(back))
                if tokio::time::Instant::now() < deadline =>
            {
                event = back;
                tokio::time::sleep(REEMIT_RETRY_PAUSE).await;
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                crate::note_replication_drop();
                return false;
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => return false,
        }
    }
}

/// DB-startup sequence: the one-shot legacy migration, a repartition of every verse (crash
/// repair), then the AC-42 re-emit of this node's own statements.
pub async fn startup(
    db: &Db,
    local_did: &str,
    repl_tx: Option<&ReplicationSender>,
    blob_store: &BlobStoreHandle,
) -> anyhow::Result<()> {
    let report = crate::migration::role_grants::migrate_legacy_role_rows(db, local_did)
        .await
        .context("legacy role migration failed")?;
    if !report.already_applied {
        tracing::info!(?report, "Legacy role rows migrated to role statements");
    }
    let verses = repartition_all(db).await?;
    let emitted = reemit_local_statements(db, repl_tx, blob_store, local_did, None).await?;
    tracing::debug!(
        verses,
        emitted,
        "role statements repartitioned and re-emitted at startup"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interim_authority_verse_ids_are_recognised_and_ulids_are_not() {
        assert!(is_interim_authority_verse_id(&"a1".repeat(32)));
        assert!(!is_interim_authority_verse_id(&"A1".repeat(32)));
        assert!(!is_interim_authority_verse_id("01HZXK3V8Q4R5S6T7U8V9W0XYZ"));
        assert_eq!(
            verse_of_scope(&format!("VERSE#{}", "ab".repeat(32))),
            None,
            "an interim VERSE#<hex> key never belongs to a hierarchy verse"
        );
        assert_eq!(
            verse_of_scope("VERSE#v1-FRACTAL#f1-PETAL#p1").as_deref(),
            Some("v1")
        );
        assert_eq!(verse_of_scope("VERSE#v1-RESOURCE#r1"), None);
    }

    #[test]
    fn payload_bytes_round_trip_through_the_wire_payload_and_key() {
        let stmt = RoleStatement {
            scope: "VERSE#v1-FRACTAL#f1".into(),
            peer_did: "did:key:peer".into(),
            grantor_did: "did:key:owner".into(),
            role: GrantedRole::Editor,
            revoked: false,
            hlc: u64::MAX,
            editor_since: Some(u64::MAX - 1),
            manager_since: None,
        };
        let payload: RoleStatementPayload =
            serde_json::from_slice(&statement_payload_bytes(&stmt)).expect("payload");
        assert_eq!(payload.hlc, u64::MAX, "the full u64 HLC range survives");
        assert_eq!(payload.editor_since, Some(u64::MAX - 1), "continuity rides the wire");
        let stored = statement_row("v1", &stmt, false);
        assert_eq!(stored.get("manager_since"), None, "absent continuity stays absent");
        assert_eq!(statement_from_row(&stored), Some(stmt.clone()), "and survives storage");
        assert_eq!(payload.role, "editor");
        assert_eq!(
            statement_record_id(&stmt),
            "VERSE#v1-FRACTAL#f1/did:key:peer/did:key:owner"
        );
        let key = fe_policy::StatementKey::parse(&statement_record_id(&stmt)).expect("key");
        assert_eq!(key.grantor_did, stmt.grantor_did);
    }
}
