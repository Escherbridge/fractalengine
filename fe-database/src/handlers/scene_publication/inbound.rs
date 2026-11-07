//! Full snapshot apply with original commit clocks; see AGENTS.md.

use super::{
    clock_of, read_clock,
    wire::{self, SceneClock},
    CLOCK_FENCE, CLOCK_VERIFY, CLOCK_WRITE,
};
use crate::repo::Db;
use fe_runtime::messages::{ReplicatedRowOutcome, SceneChange};
use serde_json::{json, Value};

/// What [`rejudge`] did to one node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejudged {
    /// The current version's author still holds Editor (or no clock to judge).
    Unchanged,
    /// The latest admissible replica version replaced an inadmissible current one.
    Restored(ReplicatedRowOutcome),
    /// No author of any replica version holds Editor: the node is removed locally.
    Removed { petal_id: String },
}

/// QA-0121/QA-0131 re-judge: a version is admissible when its publisher holds
/// Editor at its scope now AND held it when the version was written. An
/// admissible current version is kept; otherwise the newest admissible
/// snapshot among `candidates` (every author's replica entry for this key)
/// replaces it — or is restored when the node is absent locally — and a node
/// with no admissible version is removed. A pure function of the replicated
/// entries and role rows, so every peer converges whatever order it saw them in.
/// `restore_only` (a role ROSE, which never makes a kept version inadmissible)
/// skips every node that is present locally.
pub(crate) async fn rejudge(
    db: &Db,
    verse: &str,
    id: &str,
    candidates: &[(String, Vec<u8>)],
    local_publisher: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    restore_only: bool,
) -> anyhow::Result<Rejudged> {
    let current = clock_of(&read_clock(db, id).await?)?;
    if restore_only && current.is_some() {
        return Ok(Rejudged::Unchanged);
    }
    let editor = |did: String, scope: String, hlc: u64| async move {
        Ok::<_, anyhow::Error>(
            crate::role_manager::resolve_role(db, &did, &scope)
                .await?
                .is_at_least(fe_policy::RoleLevel::Editor)
                && crate::role_grants::written_while_authorized(db, verse, &did, &scope, hlc).await?,
        )
    };
    if let Some(current) = &current {
        let Some(scope) = super::super::crud::resolve_node_scope_handler(db, id).await? else {
            return Ok(Rejudged::Unchanged);
        };
        if editor(current.publisher.clone(), scope, current.hlc).await? {
            return Ok(Rejudged::Unchanged);
        }
    }
    let mut best: Option<(SceneClock, &str, &[u8])> = None;
    for (author, bytes) in candidates {
        let Ok(row) = serde_json::from_slice::<Value>(bytes) else { continue };
        let Ok(Some(version)) = SceneClock::from_marker(&row) else { continue };
        let Some(petal) = row["petal_id"].as_str() else { continue };
        if version.publisher != *author
            || row["node_id"].as_str() != Some(id)
            || wire::validate_complete(&row).is_err()
            || best.as_ref().is_some_and(|(kept, _, _)| *kept >= version)
        {
            continue;
        }
        let Some(row_scope) = super::super::crud::resolve_petal_scope_handler(db, petal).await? else {
            continue;
        };
        if editor(author.clone(), row_scope, version.hlc).await? {
            best = Some((version, author.as_str(), bytes.as_slice()));
        }
    }
    if let Some((_, author, bytes)) = best {
        let replacing = current.is_some();
        let outcome = apply_complete(db, verse, id, bytes, author, Some(local_publisher), changes, replacing)
            .await?
            .unwrap_or(ReplicatedRowOutcome::NotApplicable);
        return Ok(Rejudged::Restored(outcome));
    }
    if current.is_none() {
        return Ok(Rejudged::Unchanged);
    }
    let mut response = db
        .query("SELECT VALUE petal_id FROM node WHERE node_id=$scene_id LIMIT 1; DELETE node WHERE node_id=$scene_id; DELETE node_scene_clock WHERE node_id=$scene_id;")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let petals: Vec<String> = response.take(0)?;
    let petal_id = petals.into_iter().next().unwrap_or_default();
    super::super::replicated_row::emit_scene_change(
        changes,
        SceneChange::NodeRemoved {
            node_id: id.into(),
            petal_id: petal_id.clone(),
        },
    );
    Ok(Rejudged::Removed { petal_id })
}

/// Handle tagged full snapshots, retaining legacy partial-row behavior only before a known version.
/// `rejudge` (QA-0121) replaces a current version whose publisher lost Editor even
/// when it is newer, including this node's own inadmissible local commit.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_complete(
    db: &Db,
    verse: &str,
    id: &str,
    bytes: &[u8],
    author: &str,
    local_publisher: Option<&str>,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    rejudge: bool,
) -> anyhow::Result<Option<ReplicatedRowOutcome>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > wire::MAX_ROW_BYTES {
        return Ok(Some(ReplicatedRowOutcome::Denied));
    }
    let mut row: Value = serde_json::from_slice(bytes)?;
    if row
        .get("node_id")
        .is_some_and(|value| value.as_str() != Some(id))
    {
        return Ok(Some(ReplicatedRowOutcome::Denied));
    }
    let version = match SceneClock::from_marker(&row) {
        Ok(Some(version)) => version,
        Ok(None) => {
            let tombstone = row.get("tombstone").is_some_and(|value| !value.is_null());
            if !tombstone && !read_clock(db, id).await?.is_empty() {
                tracing::warn!(
                    node_id = id,
                    "unversioned live row deferred: current scene has an original commit clock"
                );
                return Ok(Some(ReplicatedRowOutcome::NotApplicable));
            }
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(node_id=id,%error,"invalid scene snapshot marker");
            return Ok(Some(ReplicatedRowOutcome::Denied));
        }
    };
    if version.publisher != author
        || row["node_id"].as_str() != Some(id)
        || wire::validate_complete(&row).is_err()
    {
        return Ok(Some(ReplicatedRowOutcome::Denied));
    }
    let admitted_hlc = match crate::op_log::admit_scene_hlc(version.hlc) {
        Ok(clock) => clock,
        Err(error) => {
            tracing::warn!(node_id=id,%error,"scene clock refused");
            return Ok(Some(ReplicatedRowOutcome::Denied));
        }
    };
    let petal = row["petal_id"].as_str().unwrap().to_owned();
    let Some(scope) = super::super::crud::resolve_petal_scope_handler(db, &petal).await? else {
        return Ok(Some(ReplicatedRowOutcome::Deferred(
            fe_runtime::messages::DeferralReason::Parent {
                table: "petal".into(),
                id: petal,
            },
        )));
    };
    anyhow::ensure!(
        crate::parse_scope(&scope)?.verse_id == verse,
        "scene ancestry is outside replica"
    );
    let authority = match super::super::entity::prepare_scope_permission(
        db,
        &scope,
        author,
        fe_policy::RoleLevel::Editor,
    )
    .await
    {
        Ok(authority) => authority,
        Err(error) => {
            if let Some(reason) = changed_admission(db, &petal, &scope, author).await? {
                return Ok(Some(ReplicatedRowOutcome::Deferred(reason)));
            }
            return Err(error);
        }
    };
    if !crate::role_grants::written_while_authorized(db, verse, author, &scope, version.hlc).await? {
        tracing::warn!(node_id = id, author, hlc = version.hlc, "scene row stamped before its author held Editor here; denied (QA-0131)");
        return Ok(Some(ReplicatedRowOutcome::Denied));
    }
    let (mut guards, mut bindings) = authority.into_parts();
    let mut response = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let before: Vec<Value> = response.take(0)?;
    anyhow::ensure!(before.len() <= 1, "ambiguous node identity");
    if let Some(old) = before.first() {
        if old["petal_id"] != petal {
            return Ok(Some(ReplicatedRowOutcome::Denied));
        }
    }
    let incoming_deleted = row.get("tombstone").is_some_and(|value| !value.is_null());
    let local_deleted = before
        .first()
        .is_some_and(|row| row.get("tombstone").is_some_and(|value| !value.is_null()));
    // A re-judge replaces an inadmissible tombstone too: peers that refused it hold the live row.
    if local_deleted && !incoming_deleted && !rejudge {
        return Ok(Some(ReplicatedRowOutcome::SkippedTombstoned));
    }
    let clocks = read_clock(db, id).await?;
    let mut current = clock_of(&clocks)?;
    // Fence the same journal capture used to order unpublished local commits.
    let local_fact = capture_ordering_fact(db, id, &mut guards, &mut bindings).await?;
    if let (false, Some(old), Some(fact)) = (rejudge, before.first(), local_fact.as_ref()) {
        if fact["payload"]["journal_version"] == super::super::node_history::JOURNAL_VERSION
            && fact["payload"]["scope"].as_str() == Some(scope.as_str())
            && fact["payload"]["after_state"]
                == super::super::node_history::normalize_snapshot(old.clone())?
        {
            if let Some(hlc) = fact["hlc_timestamp"].as_u64().filter(|hlc| *hlc > 0) {
                if let Some(publisher) = local_publisher {
                    let local = SceneClock {
                        hlc,
                        publisher: publisher.into(),
                    };
                    if current.as_ref().is_none_or(|clock| *clock < local) {
                        current = Some(local);
                    }
                } else if current.as_ref().is_none_or(|clock| clock.hlc < hlc)
                    && hlc >= version.hlc
                    && (!incoming_deleted || local_deleted)
                {
                    return Ok(Some(ReplicatedRowOutcome::NotApplicable));
                }
            }
        }
    }
    if !rejudge
        && current.as_ref().is_some_and(|clock| version <= *clock)
        && (!incoming_deleted || local_deleted)
    {
        tracing::debug!(
            node_id = id,
            hlc = version.hlc,
            "older or duplicate complete scene snapshot ignored"
        );
        return Ok(Some(ReplicatedRowOutcome::NotApplicable));
    }
    // A CBOR fence preserves exact NONE/null and geometry without rebinding JSON as geometry.
    let mut fingerprint=db.query("SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id",id.to_owned())).await?.check()?;
    let fingerprint: Vec<Value> = fingerprint.take(0)?;
    // Both reads must describe the same row before the use-time fence is captured.
    let mut reread = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let reread: Vec<Value> = reread.take(0)?;
    anyhow::ensure!(
        reread == before,
        "scene changed while capturing apply fence"
    );
    guards.push("IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id) != $scene_before { THROW 'scene row changed'; }".into());
    let without_position: Vec<Value> = before
        .iter()
        .cloned()
        .map(|mut row| {
            row.as_object_mut().unwrap().remove("position");
            row
        })
        .collect();
    guards.push("IF (SELECT * OMIT id, position FROM node WHERE node_id=$scene_id) != $scene_before_without_position { THROW 'scene row changed'; }".into());
    if let Some(before) = before.first() {
        guards.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$scene_id AND position=<geometry<point>> [$scene_before_x,$scene_before_z])) != 1 { THROW 'scene point changed'; }".into());
        bindings.extend([
            (
                "scene_before_x".into(),
                before["position"]["coordinates"][0].clone(),
            ),
            (
                "scene_before_z".into(),
                before["position"]["coordinates"][1].clone(),
            ),
        ]);
    }
    bindings.push((
        "scene_before_without_position".into(),
        json!(without_position),
    ));
    guards.push(CLOCK_FENCE.into());
    if !incoming_deleted
        && !capture_schema_fence(db, &scope, &row, &mut guards, &mut bindings).await?
    {
        return Ok(Some(ReplicatedRowOutcome::Denied));
    }
    row.as_object_mut().unwrap().remove(wire::MARKER);
    wire::normalize_optional_columns(&mut row);
    let point = row.as_object_mut().unwrap().remove("position").unwrap();
    let statement = if before.is_empty() {
        "CREATE node CONTENT object::extend($scene_row,{position:<geometry<point>> [$scene_x,$scene_z]})"
    } else {
        "UPDATE node CONTENT object::extend($scene_row,{position:<geometry<point>> [$scene_x,$scene_z]}) WHERE node_id=$scene_id"
    };
    guards.extend([statement.into(), CLOCK_WRITE.into(), CLOCK_VERIFY.into()]);
    bindings.extend([
        ("scene_id".into(), json!(id)),
        ("scene_before".into(), json!(fingerprint)),
        ("scene_clock_before".into(), json!(clocks)),
        ("scene_row".into(), row.clone()),
        ("scene_x".into(), point["coordinates"][0].clone()),
        ("scene_z".into(), point["coordinates"][1].clone()),
        ("scene_hlc".into(), json!(i64::try_from(version.hlc)?)),
        ("scene_publisher".into(), json!(version.publisher)),
    ]);
    let script = format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        guards.join(";\n")
    );
    anyhow::ensure!(
        script
            .len()
            .checked_add(serde_json::to_vec(&bindings)?.len())
            .is_some_and(|bytes| bytes <= 4 * 1024 * 1024),
        "complete scene receive transaction exceeds 4 MiB"
    );
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    let result = async {
        query.await?.check()?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        if let Some(reason) = changed_admission(db, &petal, &scope, author).await? {
            return Ok(Some(ReplicatedRowOutcome::Deferred(reason)));
        }
        return Err(error);
    }
    crate::op_log::observe_scene_hlc(admitted_hlc);
    if incoming_deleted {
        super::super::replicated_row::emit_scene_change(
            changes,
            SceneChange::NodeRemoved {
                node_id: id.into(),
                petal_id: petal,
            },
        );
        Ok(Some(ReplicatedRowOutcome::AppliedTombstone))
    } else {
        row["position"] = point;
        super::super::replicated_row::emit_scene_change(
            changes,
            SceneChange::NodeAdded {
                node: super::super::replicated_row::node_dto_from_row(&row, id, &petal),
            },
        );
        Ok(Some(ReplicatedRowOutcome::Applied))
    }
}

/// Capture the exact latest local fact or absence and append its transactional ordering fence.
pub(crate) async fn capture_ordering_fact(
    db: &Db,
    id: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<Option<Value>> {
    let mut response = db.query("SELECT *, encoding::base64::encode(encoding::cbor::encode($this)) AS scene_fact_fingerprint OMIT id FROM node_log WHERE node_id=$scene_id ORDER BY row_version DESC LIMIT 1 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned())).await?.check()?;
    let mut rows: Vec<Value> = response.take(0)?;
    let mut ordering_rows = Vec::with_capacity(rows.len());
    for row in &mut rows {
        let fingerprint = row
            .as_object_mut()
            .and_then(|row| row.remove("scene_fact_fingerprint"))
            .filter(Value::is_string)
            .ok_or_else(|| anyhow::anyhow!("local scene fact fingerprint missing"))?;
        ordering_rows.push(json!({
            "row_version": row["row_version"],
            "scene_fact_fingerprint": fingerprint,
        }));
    }
    guards.push("IF (SELECT row_version, encoding::base64::encode(encoding::cbor::encode($this)) AS scene_fact_fingerprint FROM node_log WHERE node_id=$scene_id ORDER BY row_version DESC LIMIT 1) != $scene_local_fact_before { THROW 'local scene ordering fact changed'; }".into());
    bindings.push(("scene_local_fact_before".into(), json!(ordering_rows)));
    Ok(rows.into_iter().next())
}

async fn changed_admission(
    db: &Db,
    petal: &str,
    scope: &str,
    author: &str,
) -> anyhow::Result<Option<fe_runtime::messages::DeferralReason>> {
    use fe_runtime::messages::DeferralReason;
    if super::super::crud::resolve_petal_scope_handler(db, petal)
        .await?
        .as_deref()
        != Some(scope)
    {
        return Ok(Some(DeferralReason::Parent {
            table: "petal".into(),
            id: petal.into(),
        }));
    }
    if !crate::role_manager::resolve_role(db, author, scope)
        .await?
        .is_at_least(fe_policy::RoleLevel::Editor)
    {
        return Ok(Some(DeferralReason::AuthorRole { did: author.into() }));
    }
    Ok(None)
}

const SCHEMA_RULES_SQL:&str="SELECT key,field_def_id,scope,enforced_schema FROM field_def WHERE entity_type='node' AND key IN $scene_schema_keys AND enforced_schema!=NONE AND scope IN $scene_schema_scopes ORDER BY key,field_def_id,scope LIMIT 257 TIMEOUT 5s";

async fn capture_schema_fence(
    db: &Db,
    scope: &str,
    row: &Value,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<bool> {
    let keys = row
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let scopes = super::super::field_def::property_rule_scopes(scope);
    let mut response = db
        .query(SCHEMA_RULES_SQL)
        .bind(("scene_schema_keys", keys.clone()))
        .bind(("scene_schema_scopes", scopes.clone()))
        .await?
        .check()?;
    let rules: Vec<Value> = response.take(0)?;
    if rules.len() > 256 || serde_json::to_vec(&rules)?.len() > 4 * 1024 * 1024 {
        tracing::warn!("complete scene receive exceeds known-schema rule bounds");
        return Ok(false);
    }
    for key in &keys {
        let applicable = rules
            .iter()
            .filter(|rule| rule["key"].as_str() == Some(key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if let Err(error) = super::super::field_def::validate_captured_property_rules(
            scope,
            key,
            &row["properties"][key],
            &applicable,
        ) {
            tracing::warn!(property=key,%error,"complete scene row violates a known enforced schema");
            return Ok(false);
        }
    }
    guards.push(format!("IF ({SCHEMA_RULES_SQL}) != $scene_schema_rules {{ THROW 'scene property schema changed'; }}"));
    bindings.extend([
        ("scene_schema_keys".into(), json!(keys)),
        ("scene_schema_scopes".into(), json!(scopes)),
        ("scene_schema_rules".into(), json!(rules)),
    ]);
    Ok(true)
}
