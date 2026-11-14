//! Petal visibility-Group registry (`petal.visibility_groups`) and persisted
//! scope eyes (`verse`/`fractal`/`petal.view_hidden`) — hierarchy_visibility_groups
//! Phase 4. Lifecycle order + rationale: fe-database/src/AGENTS.md §lifecycle
//! "Visibility groups + scope eyes".

use tracing::instrument;

use fe_runtime::messages::{CallerAuth, DbResult, ScopeLevel, VisibilityGroupDto};

use crate::handlers::correlated_failure;
use crate::handlers::node_view::MAX_VIEW_GROUP_ID_LEN;
use crate::repo::Db;

/// Upper bound on Groups in one petal's registry.
pub const MAX_VISIBILITY_GROUPS: usize = 256;
/// Upper bound on a Group name's length (chars, after trimming).
pub const MAX_GROUP_NAME_CHARS: usize = 80;

/// Exact checked-request exclusion; legacy registry callers use None.
pub(crate) struct NewGroupFence<'a> {
    pub prefix: &'a str,
    pub checked_request_id: Option<&'a str>,
}

/// Fence retained references and checked historical IDs; see editor_batch/AGENTS.md.
pub(crate) async fn fence_new_group_ids(
    db: &Db,
    petal_id: &str,
    existing: &[VisibilityGroupDto],
    incoming: &[String],
    context: NewGroupFence<'_>,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, serde_json::Value)>,
) -> anyhow::Result<()> {
    let NewGroupFence {
        prefix,
        checked_request_id,
    } = context;
    fence_unreferenced_group_ids(db, petal_id, existing, incoming, prefix, guards, bindings)
        .await?;
    for (index, id) in incoming
        .iter()
        .filter(|id| !existing.iter().any(|group| &group.id == *id))
        .enumerate()
    {
        let name = format!("{prefix}_deleted_{index}");
        let select = format!("SELECT VALUE id FROM op_log WHERE op_type='EditorTransaction' AND payload.receipt.petal_id=${name}_petal AND (payload.group_delete_before.target['Group'].group_id=${name}_group OR payload.receipt.created_group_id=${name}_group) AND payload.editor_batch_request_id != ${name}_current LIMIT 1");
        let mut response = db
            .query(select.clone())
            .bind((format!("{name}_petal"), petal_id.to_owned()))
            .bind((format!("{name}_group"), id.clone()))
            .bind((format!("{name}_current"), String::new()))
            .await?
            .check()?;
        let deleted: Vec<serde_json::Value> = response.take(0)?;
        anyhow::ensure!(deleted.is_empty(), "historical Group IDs require their exact durable lifecycle proof; create a fresh ID instead");
        guards.push(format!("IF array::len(({select})) != 0 {{ THROW 'historical Group ID requires its lifecycle proof'; }}"));
        bindings.extend([
            (format!("{name}_petal"), serde_json::json!(petal_id)),
            (format!("{name}_group"), serde_json::json!(id)),
            (
                format!("{name}_current"),
                serde_json::json!(checked_request_id.unwrap_or("")),
            ),
        ]);
    }
    Ok(())
}

pub(crate) fn retained_group_select(prefix: &str) -> String {
    format!("SELECT VALUE node_id FROM node WHERE petal_id=${prefix}_petal AND properties['view.groups'] CONTAINS ${prefix}_group LIMIT 1")
}

/// Retained-reference fence for proof-backed restoration; see editor_batch/AGENTS.md.
pub(crate) async fn fence_unreferenced_group_ids(
    db: &Db,
    petal_id: &str,
    existing: &[VisibilityGroupDto],
    incoming: &[String],
    prefix: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, serde_json::Value)>,
) -> anyhow::Result<()> {
    use serde_json::{Value, json};
    for (index, id) in incoming
        .iter()
        .filter(|id| !existing.iter().any(|group| &group.id == *id))
        .enumerate()
    {
        let name = format!("{prefix}_{index}");
        let select = retained_group_select(&name);
        let mut response = db
            .query(select.clone())
            .bind((format!("{name}_petal"), json!(petal_id)))
            .bind((format!("{name}_group"), json!(id)))
            .await?
            .check()?;
        let refs: Vec<Value> = response.take(0)?;
        anyhow::ensure!(
            refs.is_empty(),
            "group ID has retained memberships; create a fresh ID instead of resurrecting a removed group"
        );
        guards.push(format!(
            "IF array::len(({select})) != 0 {{ THROW 'group ID has retained memberships'; }}"
        ));
        bindings.extend([
            (format!("{name}_petal"), json!(petal_id)),
            (format!("{name}_group"), json!(id)),
        ]);
    }
    Ok(())
}

/// Rejects a malformed registry before any auth or write: ≤256 groups, unique
/// non-empty ids (≤ the per-node membership id cap), names 1..=80 chars,
/// finite `order`. Pure.
pub fn validate_groups(groups: &[VisibilityGroupDto]) -> Result<(), String> {
    if groups.len() > MAX_VISIBILITY_GROUPS {
        return Err(format!(
            "too many groups ({} > {MAX_VISIBILITY_GROUPS})",
            groups.len()
        ));
    }
    let mut seen = std::collections::HashSet::with_capacity(groups.len());
    for group in groups {
        if group.id.trim().is_empty() || group.id.chars().count() > MAX_VIEW_GROUP_ID_LEN {
            return Err(format!(
                "invalid group id {:?} (must be non-empty, at most {MAX_VIEW_GROUP_ID_LEN} chars)",
                group.id
            ));
        }
        if !seen.insert(group.id.as_str()) {
            return Err(format!("duplicate group id {:?}", group.id));
        }
        let name_chars = group.name.trim().chars().count();
        if name_chars == 0 || name_chars > MAX_GROUP_NAME_CHARS {
            return Err(format!(
                "invalid group name {:?} (1..={MAX_GROUP_NAME_CHARS} chars)",
                group.name
            ));
        }
        if !group.order.is_finite() {
            return Err(format!("group {:?} order must be finite", group.id));
        }
    }
    Ok(())
}

/// The stored registry as DTOs (`NONE`/missing = empty). Malformed entries are
/// skipped rather than failing the whole read.
/// Strictest-reader check for an inbound petal row's registry (QA-0123): the
/// shape and limits the local writer enforces.
pub(crate) fn is_valid_inbound_registry(value: &serde_json::Value) -> bool {
    serde_json::from_value::<Vec<VisibilityGroupDto>>(value.clone())
        .is_ok_and(|groups| validate_groups(&groups).is_ok())
}

fn groups_from_value(value: Option<&serde_json::Value>) -> Vec<VisibilityGroupDto> {
    value
        .and_then(|v| v.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `(table, id column)` holding `level`'s rows.
fn level_table(level: ScopeLevel) -> (&'static str, &'static str) {
    match level {
        ScopeLevel::Verse => ("verse", "verse_id"),
        ScopeLevel::Fractal => ("fractal", "fractal_id"),
        ScopeLevel::Petal => ("petal", "petal_id"),
    }
}

/// Scope string of an existing verse/fractal/petal; `None` when the row (or
/// its parent chain) is missing.
async fn resolve_level_scope(db: &Db, level: ScopeLevel, id: &str) -> Option<String> {
    let resolved = match level {
        ScopeLevel::Verse => crate::handlers::crud::resolve_verse_scope_handler(db, id).await,
        ScopeLevel::Fractal => crate::handlers::crud::resolve_fractal_scope_handler(db, id).await,
        ScopeLevel::Petal => crate::handlers::crud::resolve_petal_scope_handler(db, id).await,
    };
    resolved.ok().flatten()
}

/// Resolve the caller at `scope` and require Editor+ (`authorize_node_edit` is
/// `Write` at any scope string, so it covers verse/fractal/petal too). `pub(crate)`
/// so sibling petal-scoped writers (`petal_render_distance`) reuse the same gate.
pub(crate) async fn authorize_edit(
    db: &Db,
    local_did: &str,
    auth: &CallerAuth,
    scope: String,
    what: &str,
) -> Result<(), String> {
    let ctx = crate::lifecycle_auth_context(db, local_did, auth, &scope).await;
    let decision = fe_policy::authorize_node_edit(&ctx, &fe_policy::Scope::new(scope));
    if decision.is_allow() {
        Ok(())
    } else {
        Err(format!(
            "{what} denied for {}: {}",
            ctx.subject_label(),
            decision.reason().unwrap_or("unauthorized")
        ))
    }
}

/// Replicate a committed registry change (QA-0123): the petal row's catalog
/// columns, under the Editor floor every registry writer already cleared — the
/// same row shape `petal_terrain` publishes. Best-effort, never fails the write.
pub(crate) async fn publish_registry(
    db: &Db,
    repl_tx: Option<&crate::ReplicationSender>,
    blob_store: &crate::BlobStoreHandle,
    local_did: &str,
    petal_id: &str,
) {
    if repl_tx.is_none() {
        return;
    }
    let published = async {
        let scope = super::crud::resolve_petal_scope_handler(db, petal_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("committed Petal ancestry is unavailable"))?;
        let verse = crate::parse_scope(&scope)?.verse_id;
        crate::field_clock::publish_clocked_row(db, blob_store, repl_tx, local_did, &verse, "petal", petal_id, true)
            .await?;
        Ok::<_, anyhow::Error>(())
    };
    if let Err(error) = published.await {
        tracing::warn!(petal_id, "visibility group registry committed; publication deferred: {error:#}");
    }
}

/// Write the whole registry, returning what the row now holds.
#[instrument(skip(db, groups))]
async fn write_groups(
    db: &Db,
    petal_id: &str,
    groups: &[VisibilityGroupDto],
    scope: &str,
    local_did: &str,
    _auth: &CallerAuth,
    expected: Option<&[VisibilityGroupDto]>,
) -> anyhow::Result<Vec<VisibilityGroupDto>> {
    use serde_json::{Value, json};
    let (mut guards, mut bindings) =
        super::entity::prepare_scope_permission(db, scope, local_did, fe_policy::RoleLevel::Editor)
            .await?
            .into_parts();
    let mut read = db
        .query(format!(
            "SELECT *, {} AS stamp_petal_snapshot FROM petal WHERE petal_id=$pid LIMIT 2",
            super::preconditions::PETAL_TYPED_SNAPSHOT_SQL
        ))
        .bind(("pid", petal_id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = read.take(0)?;
    anyhow::ensure!(rows.len() == 1, "Group Petal unavailable");
    let existing = groups_from_value(rows[0].get("visibility_groups"));
    anyhow::ensure!(
        expected.is_none_or(|wanted| wanted == existing.as_slice()),
        "Group registry changed since its read"
    );
    let ids = groups
        .iter()
        .map(|group| group.id.clone())
        .collect::<Vec<_>>();
    fence_new_group_ids(
        db,
        petal_id,
        &existing,
        &ids,
        NewGroupFence { prefix: "registry_new", checked_request_id: None },
        &mut guards,
        &mut bindings,
    )
    .await?;
    guards.push(format!("IF (SELECT VALUE {} FROM petal WHERE petal_id=$pid) != [$registry_snapshot] {{ THROW 'Group registry changed'; }}",super::preconditions::PETAL_TYPED_SNAPSHOT_SQL));
    bindings.extend([
        (
            "registry_snapshot".into(),
            json!(super::preconditions::petal_typed_snapshot(&rows[0])?),
        ),
        ("groups".into(), json!(groups)),
        ("pid".into(), json!(petal_id)),
    ]);
    let slot = guards.len() + 1;
    guards.push(
        "UPDATE petal SET visibility_groups=$groups WHERE petal_id=$pid RETURN visibility_groups"
            .into(),
    );
    let mut query = db.query(format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        guards.join("; ")
    ));
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut res = query
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("SetPetalVisibilityGroups update failed: {e}"))?;
    let rows: Vec<serde_json::Value> = res.take(slot)?;
    let Some(row) = rows.first() else {
        anyhow::bail!("SetPetalVisibilityGroups: no petal row matched");
    };
    Ok(groups_from_value(row.get("visibility_groups")))
}

/// The DB thread's reply to `SetPetalVisibilityGroups`: validate → resolve the
/// petal scope (missing → failure) → caller's real role → Editor+ → replace the
/// whole registry → echo the stored registry as `PetalVisibilityGroupsLoaded`.
pub async fn set_petal_visibility_groups_reply(
    db: &Db,
    local_did: &str,
    petal_id: String,
    groups: Vec<VisibilityGroupDto>,
    auth: CallerAuth,
    correlation_id: Option<String>,
) -> DbResult {
    set_petal_visibility_groups_checked_reply(
        db,
        local_did,
        petal_id,
        groups,
        auth,
        correlation_id,
        None,
    )
    .await
}

pub async fn set_petal_visibility_groups_checked_reply(
    db: &Db,
    local_did: &str,
    petal_id: String,
    groups: Vec<VisibilityGroupDto>,
    auth: CallerAuth,
    correlation_id: Option<String>,
    expected: Option<Vec<VisibilityGroupDto>>,
) -> DbResult {
    if let Err(e) = validate_groups(&groups) {
        return correlated_failure(
            &correlation_id,
            format!("SetPetalVisibilityGroups rejected: {e}"),
        );
    }
    let Some(scope) = resolve_level_scope(db, ScopeLevel::Petal, &petal_id).await else {
        return correlated_failure(
            &correlation_id,
            format!("SetPetalVisibilityGroups: no petal {petal_id}"),
        );
    };
    if let Err(e) = authorize_edit(
        db,
        local_did,
        &auth,
        scope.clone(),
        "SetPetalVisibilityGroups",
    )
    .await
    {
        return correlated_failure(&correlation_id, e);
    }
    match write_groups(
        db,
        &petal_id,
        &groups,
        &scope,
        local_did,
        &auth,
        expected.as_deref(),
    )
    .await
    {
        Ok(groups) => DbResult::PetalVisibilityGroupsLoaded {
            petal_id,
            groups,
            correlation_id,
        },
        Err(e) => correlated_failure(
            &correlation_id,
            format!("SetPetalVisibilityGroups failed: {e}"),
        ),
    }
}

/// The DB thread's reply to `GetPetalVisibilityGroups` (read-only; the REST
/// layer gates Viewer+). A missing petal is a failure, not an empty registry.
pub async fn get_petal_visibility_groups_reply(
    db: &Db,
    petal_id: String,
    correlation_id: Option<String>,
) -> DbResult {
    let read = async {
        let mut res = db
            .query("SELECT visibility_groups FROM petal WHERE petal_id = $pid LIMIT 1")
            .bind(("pid", petal_id.clone()))
            .await?
            .check()?;
        let rows: Vec<serde_json::Value> = res.take(0)?;
        Ok::<_, anyhow::Error>(
            rows.first()
                .map(|row| groups_from_value(row.get("visibility_groups"))),
        )
    };
    match read.await {
        Ok(Some(groups)) => DbResult::PetalVisibilityGroupsLoaded {
            petal_id,
            groups,
            correlation_id,
        },
        Ok(None) => correlated_failure(
            &correlation_id,
            format!("GetPetalVisibilityGroups: no petal {petal_id}"),
        ),
        Err(e) => correlated_failure(
            &correlation_id,
            format!("GetPetalVisibilityGroups failed: {e}"),
        ),
    }
}

/// The DB thread's reply to `SetScopeHidden`: resolve the row's scope (missing →
/// failure) → caller's real role → Editor+ → write `view_hidden` (`true`, or
/// `NONE` for visible — absence is the default) → `ScopeHiddenSet`.
pub async fn set_scope_hidden_reply(
    db: &Db,
    local_did: &str,
    level: ScopeLevel,
    id: String,
    hidden: bool,
    auth: CallerAuth,
    correlation_id: Option<String>,
) -> DbResult {
    let Some(scope) = resolve_level_scope(db, level, &id).await else {
        return correlated_failure(
            &correlation_id,
            format!("SetScopeHidden: no {level:?} {id}"),
        );
    };
    if let Err(e) = authorize_edit(db, local_did, &auth, scope, "SetScopeHidden").await {
        return correlated_failure(&correlation_id, e);
    }
    let (table, id_column) = level_table(level);
    let value = if hidden { "true" } else { "NONE" };
    let statement = format!("UPDATE {table} SET view_hidden = {value} WHERE {id_column} = $id");
    let written = async {
        db.query(statement)
            .bind(("id", id.clone()))
            .await?
            .check()?;
        Ok::<_, anyhow::Error>(())
    };
    match written.await {
        Ok(()) => DbResult::ScopeHiddenSet {
            level,
            id,
            hidden,
            correlation_id,
        },
        Err(e) => correlated_failure(&correlation_id, format!("SetScopeHidden failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, name: &str, order: f64) -> VisibilityGroupDto {
        VisibilityGroupDto {
            id: id.to_string(),
            name: name.to_string(),
            visible: true,
            order,
        }
    }

    #[test]
    fn validate_groups_accepts_a_sane_registry() {
        assert_eq!(validate_groups(&[]), Ok(()));
        assert_eq!(
            validate_groups(&[group("g1", "Utilities", 0.0), group("g2", "Phase 2", 1.5)]),
            Ok(())
        );
    }

    #[test]
    fn validate_groups_rejects_each_malformed_shape() {
        let too_many: Vec<_> = (0..=MAX_VISIBILITY_GROUPS)
            .map(|i| group(&format!("g{i}"), "n", i as f64))
            .collect();
        let long_name = "x".repeat(MAX_GROUP_NAME_CHARS + 1);
        let long_id = "x".repeat(MAX_VIEW_GROUP_ID_LEN + 1);
        let cases: Vec<(&str, Vec<VisibilityGroupDto>)> = vec![
            ("too many", too_many),
            (
                "duplicate id",
                vec![group("g", "a", 0.0), group("g", "b", 1.0)],
            ),
            ("blank id", vec![group("  ", "a", 0.0)]),
            ("long id", vec![group(&long_id, "a", 0.0)]),
            ("blank name", vec![group("g", "   ", 0.0)]),
            ("long name", vec![group("g", &long_name, 0.0)]),
            ("nan order", vec![group("g", "a", f64::NAN)]),
            ("inf order", vec![group("g", "a", f64::INFINITY)]),
        ];
        for (case, groups) in cases {
            assert!(validate_groups(&groups).is_err(), "{case} must be rejected");
        }
    }

    #[test]
    fn groups_from_value_treats_none_as_empty_and_skips_junk() {
        assert!(groups_from_value(None).is_empty());
        let stored = serde_json::json!([
            {"id": "g1", "name": "A", "visible": false, "order": 2.0},
            {"oops": true}
        ]);
        assert_eq!(
            groups_from_value(Some(&stored)),
            vec![VisibilityGroupDto {
                id: "g1".into(),
                name: "A".into(),
                visible: false,
                order: 2.0
            }]
        );
    }
}
