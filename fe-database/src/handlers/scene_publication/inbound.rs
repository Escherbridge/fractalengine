//! Full snapshot apply: per-key (HLC, author) merge judged by the role held at write time; see AGENTS.md.

use super::{
    clock_of, read_clock,
    wire::{self, SceneClock},
    CLOCK_FENCE, CLOCK_VERIFY, CLOCK_WRITE,
};
use crate::field_clock::judge::VerseView;
use crate::field_clock::merge::{self, canonical_value, version_order};
use crate::field_clock::store::{self, stored_hlc};
use crate::field_clock::wire::{apply_keys, keys_of, parse_own, FieldVersion, TRANSFORM_KEY};
use crate::handlers::clocked_row::{cached_winners, clock_fence, is_fence_error, FENCE_ATTEMPTS};
use crate::repo::Db;
use fe_runtime::messages::{DeferralReason, ReplicatedRowOutcome, SceneChange};
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Clock key of the node tombstone marker (delete-wins, spec §3.9).
const TOMBSTONE_KEY: &str = "tombstone";
/// The node's stored versions must be the ones the merge folded against (THROW text = clocked_row's fence).
const NODE_VERSIONS_FENCE: &str = "IF array::sort((SELECT VALUE [field_key, author, hlc] FROM row_field_version WHERE tbl=$fe_row_tbl AND rid=$fe_row_rid)) != array::sort($fe_versions_before) { THROW 'clocked row changed'; }";

/// What [`rejudge`] did to one node.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejudged {
    /// Every winner is still admissible (or there is nothing to judge).
    Unchanged,
    /// The node was rewritten from its admissible versions, or restored when absent.
    Restored(ReplicatedRowOutcome),
    /// No admissible transform remains: the node is removed locally (its versions are kept).
    Removed { petal_id: String },
}

/// Result of one per-key merge pass.
enum Merged {
    Outcome(ReplicatedRowOutcome),
    Removed { petal_id: String },
}

/// An inbound complete snapshot that passed the strictest reader.
struct Incoming {
    author: String,
    clock: SceneClock,
    row: Value,
    /// The clocked row's own versions; `None` for a v1 snapshot, fanned out at merge time.
    own: Option<Vec<FieldVersion>>,
    /// Highest HLC the snapshot carries (marker and own versions), observed after commit.
    max_hlc: u64,
}

/// The node being merged and its confined ancestry.
struct Target<'a> {
    verse: &'a str,
    id: &'a str,
    petal: String,
    scope: String,
}

/// Re-judge one node (QA-0165: write-time role only, never "Editor now"). A node with stored
/// versions is recomputed from them and `candidates` are only used to restore it when absent;
/// a legacy node without versions keeps the candidate path judged by `written_while_authorized`.
/// `restore_only` (a role ROSE) skips every node that is present locally.
pub(crate) async fn rejudge(
    db: &Db,
    verse: &str,
    id: &str,
    candidates: &[(String, Vec<u8>)],
    local_publisher: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    restore_only: bool,
) -> anyhow::Result<Rejudged> {
    let present = !read_node(db, id).await?.is_empty();
    if restore_only && present {
        return Ok(Rejudged::Unchanged);
    }
    if !store::load_versions(db, "node", id).await?.is_empty() {
        if present {
            return rematerialize(db, verse, id, local_publisher, changes).await;
        }
        return restore_from_candidates(db, verse, id, candidates, local_publisher, changes).await;
    }
    rejudge_legacy(db, verse, id, candidates, local_publisher, changes).await
}

/// Recompute a present node from its stored versions alone (keys without an admissible version are dropped).
pub(crate) async fn rematerialize(
    db: &Db,
    verse: &str,
    id: &str,
    local_publisher: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
) -> anyhow::Result<Rejudged> {
    let Some(petal) = super::super::crud::resolve_node_petal_id_handler(db, id).await? else {
        return Ok(Rejudged::Unchanged);
    };
    let Some(scope) = super::super::crud::resolve_petal_scope_handler(db, &petal).await? else {
        return Ok(Rejudged::Unchanged);
    };
    if crate::parse_scope(&scope)?.verse_id != verse {
        return Ok(Rejudged::Unchanged);
    }
    let target = Target { verse, id, petal, scope };
    let merged =
        merge_with_retry(db, &target, None, local_publisher, Some(local_publisher), changes, true).await?;
    Ok(rejudged_of(merged))
}

/// Restore an absent node with stored versions: the newest candidate that applies wins the race.
async fn restore_from_candidates(
    db: &Db,
    verse: &str,
    id: &str,
    candidates: &[(String, Vec<u8>)],
    local_publisher: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
) -> anyhow::Result<Rejudged> {
    let mut ordered: Vec<(SceneClock, &str, &[u8])> = candidates
        .iter()
        .filter_map(|(author, bytes)| {
            let row = serde_json::from_slice::<Value>(bytes).ok()?;
            let clock = SceneClock::from_marker(&row).ok()??;
            (clock.publisher == *author && row["node_id"].as_str() == Some(id))
                .then_some((clock, author.as_str(), bytes.as_slice()))
        })
        .collect();
    ordered.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, author, bytes) in ordered {
        let merged =
            apply_complete_merged(db, verse, id, bytes, author, Some(local_publisher), changes, true).await?;
        if let Some(Merged::Outcome(outcome @ (ReplicatedRowOutcome::Applied | ReplicatedRowOutcome::AppliedTombstone))) =
            merged
        {
            return Ok(Rejudged::Restored(outcome));
        }
    }
    Ok(Rejudged::Unchanged)
}

/// QA-0131 candidate path for a node with no stored versions: a version is admissible when its
/// publisher held Editor when it was written (no current-role half, QA-0165).
async fn rejudge_legacy(
    db: &Db,
    verse: &str,
    id: &str,
    candidates: &[(String, Vec<u8>)],
    local_publisher: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
) -> anyhow::Result<Rejudged> {
    let current = clock_of(&read_clock(db, id).await?)?;
    let authorized = |did: String, scope: String, hlc: u64| async move {
        crate::role_grants::written_while_authorized(db, verse, &did, &scope, hlc).await
    };
    if let Some(current) = &current {
        let Some(scope) = super::super::crud::resolve_node_scope_handler(db, id).await? else {
            return Ok(Rejudged::Unchanged);
        };
        if authorized(current.publisher.clone(), scope, current.hlc).await? {
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
        if authorized(author.clone(), row_scope, version.hlc).await? {
            best = Some((version, author.as_str(), bytes.as_slice()));
        }
    }
    if let Some((_, author, bytes)) = best {
        return Ok(
            match apply_complete_merged(db, verse, id, bytes, author, Some(local_publisher), changes, true).await? {
                Some(Merged::Removed { petal_id }) => Rejudged::Removed { petal_id },
                Some(Merged::Outcome(outcome)) => Rejudged::Restored(outcome),
                None => Rejudged::Restored(ReplicatedRowOutcome::NotApplicable),
            },
        );
    }
    if current.is_none() {
        return Ok(Rejudged::Unchanged);
    }
    remove_node(db, id, changes).await.map(rejudged_of)
}

/// Map a merge result onto the re-judge vocabulary.
fn rejudged_of(merged: Merged) -> Rejudged {
    match merged {
        Merged::Removed { petal_id } => Rejudged::Removed { petal_id },
        Merged::Outcome(outcome @ (ReplicatedRowOutcome::Applied | ReplicatedRowOutcome::AppliedTombstone)) => {
            Rejudged::Restored(outcome)
        }
        Merged::Outcome(_) => Rejudged::Unchanged,
    }
}

/// Handle tagged full snapshots; markerless rows keep the legacy path (`None`).
/// `rejudge` rebuilds the row from admissible versions only and skips local-edit and tombstone dominance.
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
    Ok(
        apply_complete_merged(db, verse, id, bytes, author, local_publisher, changes, rejudge)
            .await?
            .map(|merged| match merged {
                Merged::Outcome(outcome) => outcome,
                Merged::Removed { .. } => ReplicatedRowOutcome::NotApplicable,
            }),
    )
}

/// [`apply_complete`] keeping the re-judge removal distinct.
#[allow(clippy::too_many_arguments)]
async fn apply_complete_merged(
    db: &Db,
    verse: &str,
    id: &str,
    bytes: &[u8],
    author: &str,
    local_publisher: Option<&str>,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    rejudge: bool,
) -> anyhow::Result<Option<Merged>> {
    let denied = || Ok(Some(Merged::Outcome(ReplicatedRowOutcome::Denied)));
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > wire::MAX_ROW_BYTES {
        return denied();
    }
    let row: Value = serde_json::from_slice(bytes)?;
    if row
        .get("node_id")
        .is_some_and(|value| value.as_str() != Some(id))
    {
        return denied();
    }
    let clock = match SceneClock::from_marker(&row) {
        Ok(Some(clock)) => clock,
        Ok(None) => {
            let tombstone = row.get("tombstone").is_some_and(|value| !value.is_null());
            if !tombstone && !read_clock(db, id).await?.is_empty() {
                tracing::warn!(
                    node_id = id,
                    "unversioned live row deferred: current scene has an original commit clock"
                );
                return Ok(Some(Merged::Outcome(ReplicatedRowOutcome::NotApplicable)));
            }
            return Ok(None);
        }
        Err(error) => {
            tracing::warn!(node_id=id,%error,"invalid scene snapshot marker");
            return denied();
        }
    };
    if clock.publisher != author
        || row["node_id"].as_str() != Some(id)
        || wire::validate_complete(&row).is_err()
    {
        return denied();
    }
    let own = match read_own_versions(&row, author) {
        Ok(own) => own,
        Err(reason) => {
            tracing::warn!(node_id = id, author, %reason, "node field clock refused by the strictest reader");
            return denied();
        }
    };
    let max_hlc = own
        .iter()
        .flatten()
        .map(|version| version.hlc)
        .fold(clock.hlc, u64::max);
    if let Err(error) = crate::op_log::admit_scene_hlc(max_hlc) {
        tracing::warn!(node_id=id,%error,"scene clock refused");
        return denied();
    }
    let petal = row["petal_id"].as_str().unwrap_or_default().to_owned();
    let Some(scope) = super::super::crud::resolve_petal_scope_handler(db, &petal).await? else {
        return Ok(Some(Merged::Outcome(ReplicatedRowOutcome::Deferred(DeferralReason::Parent {
            table: "petal".into(),
            id: petal,
        }))));
    };
    anyhow::ensure!(
        crate::parse_scope(&scope)?.verse_id == verse,
        "scene ancestry is outside replica"
    );
    // A v1 snapshot carries no per-key clocks: the whole row is judged at its marker clock (QA-0131).
    if own.is_none()
        && !crate::role_grants::written_while_authorized(db, verse, author, &scope, clock.hlc).await?
    {
        tracing::warn!(node_id = id, author, hlc = clock.hlc, "scene row stamped before its author held Editor here; denied (QA-0131)");
        return denied();
    }
    let incoming = Incoming {
        author: author.to_owned(),
        clock,
        row,
        own,
        max_hlc,
    };
    let target = Target { verse, id, petal, scope };
    let fence_actor = local_publisher.unwrap_or(author);
    merge_with_retry(db, &target, Some(&incoming), fence_actor, local_publisher, changes, rejudge)
        .await
        .map(Some)
}

/// The clocked row's own versions, each within the HLC bound, a tombstone matching its clock, and renderable.
fn read_own_versions(row: &Value, author: &str) -> Result<Option<Vec<FieldVersion>>, String> {
    let Some(own) = parse_own("node", row, author).map_err(|error| format!("{error:#}"))? else {
        return Ok(None);
    };
    for version in &own {
        crate::op_log::admit_scene_hlc(version.hlc)
            .map_err(|error| format!("key {:?}: {error:#}", version.field))?;
        if version.field == TOMBSTONE_KEY {
            if let Some(marker) = version.value.as_ref().filter(|_| !version.deleted) {
                let actor = marker["source_did"]
                    .as_str()
                    .is_some_and(|source| !source.is_empty() && source.len() <= 512);
                if marker["hlc"].as_u64() != Some(version.hlc) || !actor {
                    return Err("tombstone version does not match its clock or actor".into());
                }
            }
        }
    }
    // Every own value must render: overlaid on the validated row, it stays a valid complete snapshot.
    wire::validate_complete(&apply_keys("node", row, &own))
        .map_err(|error| format!("own versions do not render: {error:#}"))?;
    Ok(Some(own))
}

/// [`merge_once`] retried while a version/winner fence finds the clock state moved underneath it.
async fn merge_with_retry(
    db: &Db,
    target: &Target<'_>,
    incoming: Option<&Incoming>,
    fence_actor: &str,
    local_publisher: Option<&str>,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    rejudge: bool,
) -> anyhow::Result<Merged> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match merge_once(db, target, incoming, fence_actor, local_publisher, changes, rejudge).await {
            Err(error) if attempt < FENCE_ATTEMPTS && is_fence_error(&error) => {
                tracing::debug!(node_id = target.id, attempt, "node versions moved under the merge; re-reading");
            }
            result => return result,
        }
    }
}

/// Winner of one key's admissible versions; an admissible live tombstone beats any deletion (delete-wins).
fn key_winner(field: &str, admissible: &[FieldVersion]) -> Option<FieldVersion> {
    if field == TOMBSTONE_KEY {
        let live: Vec<FieldVersion> = admissible.iter().filter(|version| !version.deleted).cloned().collect();
        if let Some(won) = merge::winner(&live, |_| true) {
            return Some(won.clone());
        }
    }
    merge::winner(admissible, |_| true).cloned()
}

/// A v1 snapshot as `author`'s versions at its marker clock; keys the author owned and now omits are deleted.
fn fan_out(incoming: &Incoming, stored: &BTreeMap<(String, String), FieldVersion>) -> Vec<FieldVersion> {
    let hlc = incoming.clock.hlc;
    let version = |field: String, value: Option<Value>| FieldVersion {
        field,
        author: incoming.author.clone(),
        hlc,
        deleted: value.is_none(),
        value,
    };
    let present: BTreeMap<String, Value> = keys_of("node", &incoming.row).into_iter().collect();
    let omitted: Vec<FieldVersion> = stored
        .values()
        .filter(|owned| owned.author == incoming.author && !owned.deleted && !present.contains_key(&owned.field))
        .map(|owned| version(owned.field.clone(), None))
        .collect();
    present
        .into_iter()
        .map(|(field, value)| version(field, Some(value)))
        .chain(omitted)
        .collect()
}

/// Keys whose local, still-unpublished value (journal fact at `hlc` by `publisher`) outranks the merge winner.
fn pending_local_keys(
    old: &Value,
    cached: &[FieldVersion],
    versions: &BTreeMap<(String, String), FieldVersion>,
    winners: &BTreeMap<String, FieldVersion>,
    publisher: &str,
    hlc: u64,
) -> BTreeSet<String> {
    let local: BTreeMap<String, Value> = keys_of("node", old)
        .into_iter()
        .map(|(field, value)| (field, canonical_value(&value)))
        .collect();
    let published = |field: &str| -> Option<Option<Value>> {
        let cached = cached.iter().find(|winner| winner.field == field)?;
        let version = versions
            .get(&(field.to_owned(), cached.author.clone()))
            .filter(|version| version.hlc == cached.hlc)?;
        Some(version.value.as_ref().filter(|_| !version.deleted).map(canonical_value))
    };
    let fields: BTreeSet<&str> = local
        .keys()
        .map(String::as_str)
        .chain(cached.iter().map(|winner| winner.field.as_str()))
        .collect();
    let mut pending = BTreeSet::new();
    for field in fields {
        let value = local.get(field).cloned();
        let in_cache = cached.iter().any(|winner| winner.field == field);
        // A key the local row never had is not a local deletion unless it was a published winner.
        if (value.is_none() && !in_cache) || published(field) == Some(value.clone()) {
            continue;
        }
        let local_version = FieldVersion {
            field: field.to_owned(),
            author: publisher.to_owned(),
            hlc,
            deleted: value.is_none(),
            value,
        };
        if winners
            .get(field)
            .is_none_or(|winner| version_order(&local_version, winner) == Ordering::Greater)
        {
            pending.insert(field.to_owned());
        }
    }
    pending
}

/// Insert-once identity columns of a node row.
fn identity_of(row: &Value) -> Value {
    let mut identity = Map::new();
    for column in ["node_id", "petal_id", "created_at"] {
        if let Some(value) = row.get(column).filter(|value| !value.is_null()) {
            identity.insert(column.to_owned(), value.clone());
        }
    }
    Value::Object(identity)
}

fn is_tombstoned(row: &Value) -> bool {
    row.get(TOMBSTONE_KEY).is_some_and(|value| !value.is_null())
}

fn tombstone_hlc(row: &Value) -> u64 {
    row[TOMBSTONE_KEY]["hlc"].as_u64().unwrap_or(0)
}

async fn read_node(db: &Db, id: &str) -> anyhow::Result<Vec<Value>> {
    let mut response = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(rows.len() <= 1, "ambiguous node identity");
    Ok(rows)
}

/// Remove a node locally (re-judge): row, scene clock and winner cache; its versions stay for a later restore.
async fn remove_node(
    db: &Db,
    id: &str,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
) -> anyhow::Result<Merged> {
    let mut response = db
        .query("SELECT VALUE petal_id FROM node WHERE node_id=$scene_id LIMIT 1")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let petals: Vec<String> = response.take(0)?;
    let petal_id = petals.into_iter().next().unwrap_or_default();
    db.query("BEGIN TRANSACTION; DELETE node WHERE node_id=$scene_id; DELETE node_scene_clock WHERE node_id=$scene_id; DELETE row_field_clock WHERE tbl='node' AND rid=$scene_id; COMMIT TRANSACTION;")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    super::super::replicated_row::emit_scene_change(
        changes,
        SceneChange::NodeRemoved {
            node_id: id.into(),
            petal_id: petal_id.clone(),
        },
    );
    Ok(Merged::Removed { petal_id })
}

/// One read-judge-write pass: fold `incoming` into the stored versions, recompute each key's
/// admissible winner and write row + versions + winner cache + scene clock in ONE fenced transaction.
#[allow(clippy::too_many_arguments)]
async fn merge_once(
    db: &Db,
    target: &Target<'_>,
    incoming: Option<&Incoming>,
    fence_actor: &str,
    local_publisher: Option<&str>,
    changes: Option<&super::super::replicated_row::SceneChangeSender>,
    rejudge: bool,
) -> anyhow::Result<Merged> {
    let (verse, id, scope) = (target.verse, target.id, target.scope.as_str());
    let outcome = |outcome| Ok(Merged::Outcome(outcome));
    let legacy_author = incoming.filter(|incoming| incoming.own.is_none()).map(|incoming| incoming.author.as_str());
    // Ancestry and verse fences only: the remote author's current role is never required (QA-0165).
    let authority = match super::super::entity::prepare_scope_permission(db, scope, fence_actor, fe_policy::RoleLevel::None).await {
        Ok(authority) => authority,
        Err(error) => {
            if let Some(reason) = changed_admission(db, &target.petal, scope, legacy_author).await? {
                return outcome(ReplicatedRowOutcome::Deferred(reason));
            }
            return Err(error);
        }
    };
    let (mut guards, mut bindings) = authority.into_parts();
    let before = read_node(db, id).await?;
    if before.first().is_some_and(|old| old["petal_id"] != target.petal) {
        return outcome(ReplicatedRowOutcome::Denied);
    }

    // Merge: fold the incoming author's versions into the stored set (keep-newer per key and author).
    let stored = store::load_versions(db, "node", id).await?;
    let cached = cached_winners(db, "node", id).await?;
    let versions_before = stored
        .iter()
        .map(|version| -> anyhow::Result<Value> { Ok(json!([version.field, version.author, stored_hlc(version.hlc)?])) })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut versions: BTreeMap<(String, String), FieldVersion> = stored
        .into_iter()
        .map(|version| ((version.field.clone(), version.author.clone()), version))
        .collect();
    let mut written = Vec::new();
    if let Some(incoming) = incoming {
        let offered = match &incoming.own {
            Some(own) => own.clone(),
            None => fan_out(incoming, &versions),
        };
        for version in offered {
            let key = (version.field.clone(), version.author.clone());
            if let Some(kept) = merge::upsert_own(versions.get(&key), version) {
                versions.insert(key, kept.clone());
                written.push(kept);
            }
        }
    }

    // Judge: a version wins only if its author held Editor at its HLC (write-time role, QA-0165).
    let view = VerseView::load(db, verse).await?;
    let mut verdicts: HashMap<(String, u64), bool> = HashMap::new();
    let mut admissible: BTreeMap<String, Vec<FieldVersion>> = BTreeMap::new();
    for version in versions.values() {
        let admitted = *verdicts
            .entry((version.author.clone(), version.hlc))
            .or_insert_with(|| view.admits_at("node", scope, version));
        if admitted {
            admissible.entry(version.field.clone()).or_default().push(version.clone());
        }
    }
    let overlay: BTreeMap<String, FieldVersion> = admissible
        .iter()
        .filter_map(|(field, list)| key_winner(field, list).map(|won| (field.clone(), won)))
        .collect();
    let mut winners = overlay.clone();
    // Inbound never demotes: a key with no admissible version keeps its cached winner (re-judge owns demotions).
    if !rejudge {
        for winner in &cached {
            winners.entry(winner.field.clone()).or_insert_with(|| winner.clone());
        }
    }
    let is_cached = |winner: &FieldVersion| {
        cached
            .iter()
            .any(|old| old.field == winner.field && old.hlc == winner.hlc && old.author == winner.author)
    };
    let cache_changed = winners.values().any(|winner| !is_cached(winner)) || winners.len() != cached.len();

    // Local journal fact not yet published: its keys keep the local value where it outranks the winner.
    let local_fact = capture_ordering_fact(db, id, &mut guards, &mut bindings).await?;
    let incoming_deleted = incoming.is_some_and(|incoming| is_tombstoned(&incoming.row));
    let local_deleted = before.first().is_some_and(is_tombstoned);
    let mut pending = BTreeSet::new();
    if let (false, Some(old), Some(fact)) = (rejudge, before.first(), local_fact.as_ref()) {
        if fact["payload"]["journal_version"] == super::super::node_history::JOURNAL_VERSION
            && fact["payload"]["scope"].as_str() == Some(scope)
            && fact["payload"]["after_state"] == super::super::node_history::normalize_snapshot(old.clone())?
        {
            if let Some(hlc) = fact["hlc_timestamp"].as_u64().filter(|hlc| *hlc > 0) {
                match local_publisher {
                    Some(publisher) => {
                        pending = pending_local_keys(old, &cached, &versions, &overlay, publisher, hlc);
                    }
                    None => {
                        let stored_clock = clock_of(&read_clock(db, id).await?)?;
                        if stored_clock.as_ref().is_none_or(|clock| clock.hlc < hlc)
                            && incoming.is_some_and(|incoming| hlc >= incoming.clock.hlc)
                            && (!incoming_deleted || local_deleted)
                        {
                            return outcome(ReplicatedRowOutcome::NotApplicable);
                        }
                    }
                }
            }
        }
    }

    // Rebuild the row: identity plus winners (re-judge, create) or the stored row overlaid by winners.
    let identity_source = before.first().or(incoming.map(|incoming| &incoming.row));
    let base = match (rejudge, before.first()) {
        (false, Some(old)) => old.clone(),
        _ => identity_of(identity_source.unwrap_or(&Value::Null)),
    };
    let mut merged = apply_keys(
        "node",
        &base,
        overlay.values().filter(|winner| !pending.contains(&winner.field)),
    );
    let merged_deleted = is_tombstoned(&merged);
    if !rejudge && local_deleted {
        // N-4: a local tombstone dominates a live merge; an older tombstone never replaces a later one.
        if !merged_deleted {
            return outcome(ReplicatedRowOutcome::SkippedTombstoned);
        }
        if let Some(old) = before.first().filter(|old| tombstone_hlc(old) > tombstone_hlc(&merged)) {
            merged[TOMBSTONE_KEY] = old[TOMBSTONE_KEY].clone();
        }
    }
    let has_point = merged["position"]["coordinates"]
        .as_array()
        .is_some_and(|coordinates| coordinates.len() == 2);
    if !has_point {
        if rejudge && !before.is_empty() {
            return remove_node(db, id, changes).await;
        }
        if before.is_empty() {
            // A new node needs an admissible transform; an edit that outran the create waits for it.
            let offered = versions.keys().any(|(field, _)| field == TRANSFORM_KEY);
            let reason = match (offered, incoming) {
                (true, Some(incoming)) => DeferralReason::AuthorRole { did: incoming.author.clone() },
                _ => DeferralReason::Parent { table: "node".into(), id: id.into() },
            };
            return outcome(ReplicatedRowOutcome::Deferred(reason));
        }
        anyhow::bail!("node {id} has no position to write");
    }

    // Scene clock: the newest non-floor winner, plus the snapshot marker when it contributed a winner.
    let clocks = read_clock(db, id).await?;
    let stored_clock = clock_of(&clocks)?;
    let winner_clock = overlay
        .values()
        .filter(|winner| !winner.is_legacy_floor() && !winner.author.is_empty())
        .map(|winner| SceneClock { hlc: winner.hlc, publisher: winner.author.clone() })
        .max();
    let contributed = incoming
        .filter(|incoming| {
            overlay.values().any(|winner| {
                winner.author == incoming.author
                    && written.iter().any(|kept| kept.field == winner.field && kept.hlc == winner.hlc)
            })
        })
        .map(|incoming| incoming.clock.clone());
    let target_clock = if rejudge {
        winner_clock.max(contributed).or_else(|| stored_clock.clone())
    } else {
        [stored_clock.clone(), winner_clock, contributed].into_iter().flatten().max()
    };
    let clock_changed = target_clock != stored_clock;
    let row_changed = before.is_empty() || cache_changed;
    let skipped = !rejudge && local_deleted && !incoming_deleted;
    if written.is_empty() && !row_changed && !clock_changed {
        return outcome(if skipped {
            ReplicatedRowOutcome::SkippedTombstoned
        } else {
            ReplicatedRowOutcome::NotApplicable
        });
    }

    // Write: fences, then row, scene clock, versions and winner cache in ONE transaction.
    let (clock_statements, clock_bindings) = clock_fence("node", id, &cached)?;
    guards.push(NODE_VERSIONS_FENCE.into());
    guards.extend(clock_statements);
    bindings.extend(clock_bindings);
    bindings.push(("fe_versions_before".into(), Value::Array(versions_before)));
    // A CBOR fence preserves exact NONE/null and geometry without rebinding JSON as geometry.
    let mut fingerprint = db
        .query("SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let fingerprint: Vec<Value> = fingerprint.take(0)?;
    // Both reads must describe the same row before the use-time fence is captured.
    anyhow::ensure!(read_node(db, id).await? == before, "scene changed while capturing apply fence");
    guards.push("IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM node WHERE node_id=$scene_id) != $scene_before { THROW 'scene row changed'; }".into());
    let without_position: Vec<Value> = before
        .iter()
        .cloned()
        .map(|mut row| {
            if let Some(object) = row.as_object_mut() {
                object.remove("position");
            }
            row
        })
        .collect();
    guards.push("IF (SELECT * OMIT id, position FROM node WHERE node_id=$scene_id) != $scene_before_without_position { THROW 'scene row changed'; }".into());
    if let Some(old) = before.first() {
        guards.push("IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$scene_id AND position=<geometry<point>> [$scene_before_x,$scene_before_z])) != 1 { THROW 'scene point changed'; }".into());
        bindings.extend([
            ("scene_before_x".into(), old["position"]["coordinates"][0].clone()),
            ("scene_before_z".into(), old["position"]["coordinates"][1].clone()),
        ]);
    }
    bindings.push(("scene_before_without_position".into(), json!(without_position)));
    guards.push(CLOCK_FENCE.into());
    if row_changed && !merged_deleted && !capture_schema_fence(db, scope, &merged, &mut guards, &mut bindings).await? {
        return outcome(ReplicatedRowOutcome::Denied);
    }
    wire::normalize_optional_columns(&mut merged);
    let point = merged
        .as_object_mut()
        .and_then(|row| row.remove("position"))
        .unwrap_or(Value::Null);
    if row_changed {
        guards.push(if before.is_empty() {
            "CREATE node CONTENT object::extend($scene_row,{position:<geometry<point>> [$scene_x,$scene_z]})".into()
        } else {
            "UPDATE node CONTENT object::extend($scene_row,{position:<geometry<point>> [$scene_x,$scene_z]}) WHERE node_id=$scene_id".into()
        });
        bindings.extend([
            ("scene_row".into(), merged.clone()),
            ("scene_x".into(), point["coordinates"][0].clone()),
            ("scene_z".into(), point["coordinates"][1].clone()),
        ]);
    }
    if let Some(clock) = target_clock.as_ref().filter(|_| row_changed || clock_changed) {
        guards.extend([CLOCK_WRITE.into(), CLOCK_VERIFY.into()]);
        bindings.extend([
            ("scene_hlc".into(), json!(i64::try_from(clock.hlc)?)),
            ("scene_publisher".into(), json!(clock.publisher)),
        ]);
    }
    if !written.is_empty() {
        let (statements, fragment) = store::versions_write_sql("node", id, verse, &written)?;
        guards.extend(statements);
        bindings.extend(fragment);
    }
    if cache_changed {
        let winner_refs: Vec<&FieldVersion> = winners.values().collect();
        let (statements, fragment) = store::clock_write_sql("node", id, &winner_refs)?;
        guards.extend(statements);
        bindings.extend(fragment);
    }
    bindings.extend([
        ("scene_id".into(), json!(id)),
        ("scene_before".into(), json!(fingerprint)),
        ("scene_clock_before".into(), json!(clocks)),
    ]);
    let script = format!("BEGIN TRANSACTION; {}; COMMIT TRANSACTION;", guards.join(";\n"));
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
        if !is_fence_error(&error) {
            if let Some(reason) = changed_admission(db, &target.petal, scope, legacy_author).await? {
                return outcome(ReplicatedRowOutcome::Deferred(reason));
            }
        }
        return Err(error);
    }
    if let Some(incoming) = incoming {
        // Order later local writes after the remote clock, only once it is committed.
        if let Ok(clock) = crate::op_log::admit_scene_hlc(incoming.max_hlc) {
            crate::op_log::observe_scene_hlc(clock);
        }
    }
    if skipped {
        return outcome(ReplicatedRowOutcome::SkippedTombstoned);
    }
    if !row_changed {
        return outcome(ReplicatedRowOutcome::NotApplicable);
    }
    if merged_deleted {
        super::super::replicated_row::emit_scene_change(
            changes,
            SceneChange::NodeRemoved {
                node_id: id.into(),
                petal_id: target.petal.clone(),
            },
        );
        return outcome(ReplicatedRowOutcome::AppliedTombstone);
    }
    merged["position"] = point;
    super::super::replicated_row::emit_scene_change(
        changes,
        SceneChange::NodeAdded {
            node: super::super::replicated_row::node_dto_from_row(&merged, id, &target.petal),
        },
    );
    outcome(ReplicatedRowOutcome::Applied)
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

/// Why a failed apply should wait rather than fail: the petal moved, or (v1 rows only) the author lost Editor.
async fn changed_admission(
    db: &Db,
    petal: &str,
    scope: &str,
    legacy_author: Option<&str>,
) -> anyhow::Result<Option<DeferralReason>> {
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
    if let Some(author) = legacy_author {
        if !crate::role_manager::resolve_role(db, author, scope)
            .await?
            .is_at_least(fe_policy::RoleLevel::Editor)
        {
            return Ok(Some(DeferralReason::AuthorRole { did: author.into() }));
        }
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
