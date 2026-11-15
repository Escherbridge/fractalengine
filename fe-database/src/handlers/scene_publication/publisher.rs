//! Publish durable local readback only; see AGENTS.md.

use super::{
    clock_of, read_clock,
    wire::{self, SceneClock},
    CLOCK_FENCE, CLOCK_VERIFY, CLOCK_WRITE,
};
use crate::field_clock::{
    self, merge::canonical_value, store::stored_hlc, FieldVersion,
};
use crate::{repo::Db, BlobStoreHandle, ReplicationEvent, ReplicationSender};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Rows per recovery page and per recovery batch.
const RECOVERY_LIMIT: usize = 1024;
/// Prepared wire bytes (rows plus dependencies) per recovery batch.
const RECOVERY_BYTES: usize = 8 * 1024 * 1024;
/// Longest a later recovery batch waits for the replication bridge to drain below half capacity.
const BATCH_DRAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
pub(crate) const PUBLICATION_ASSET_COLUMNS: &str =
    "asset_id, name, content_type, size_bytes, content_hash, created_at, owner_did, petal_id, publication_provenance";

#[derive(Default, Debug)]
pub(crate) struct RecoveryReport {
    pub examined: usize,
    pub enqueued: usize,
    pub deferred: usize,
    /// Deferred rows that are over the row bound: they stay unpublished until the node shrinks.
    pub oversized: usize,
    pub dependency_deferred: usize,
    /// Proven scene uploads no node references yet (QA-0158).
    pub uploads_enqueued: usize,
    /// Node batches sent (AGENTS.md §recovery-batches); zero when nothing was publishable.
    pub batches: usize,
}

#[derive(Debug)]
pub(crate) struct PreparedSceneRow {
    node_id: String,
    petal_id: String,
    verse_id: String,
    bytes: Vec<u8>,
    statements: Vec<String>,
    guard_labels: Vec<&'static str>,
    bindings: Vec<(String, Value)>,
    dependencies: Vec<(String, String, Vec<u8>)>,
    deferred_dependencies: usize,
    /// DID of the local edit (journal fact or tombstone operation) this row publishes.
    cause: String,
}

/// One node row a commit enqueued, for the write budget (handlers/AGENTS.md §write-budget).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Published {
    pub verse_id: String,
    /// DID of the edit the row publishes (its journal `source_did`).
    pub cause: String,
    /// Wire bytes stored in the canonical blob store and sent to peers.
    pub bytes: usize,
}

/// A local journal snapshot is publication provenance, never a claim of historical replication.
pub(crate) async fn prepare(db: &Db, id: &str, actor: &str) -> anyhow::Result<PreparedSceneRow> {
    anyhow::ensure!(!id.is_empty() && id.len() <= 1024, "invalid scene identity");
    let mut response = db
        .query("SELECT * OMIT id FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(rows.len() == 1, "committed scene row missing or ambiguous");
    let mut row = rows[0].clone();
    let petal = row["petal_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("scene Petal missing"))?
        .to_owned();
    let scope = super::super::crud::resolve_petal_scope_handler(db, &petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("scene ancestry missing"))?;
    let verse = crate::parse_scope(&scope)?.verse_id;
    let authority = super::super::entity::prepare_scope_permission(
        db,
        &scope,
        actor,
        fe_policy::RoleLevel::Editor,
    )
    .await?;
    let (mut statements, mut bindings) = authority.into_parts();
    let mut guard_labels = vec!["authority_ancestry"; statements.len()];
    let current = super::super::node_history::normalize_snapshot(row.clone())?;
    let latest = super::super::node_history::latest(db, id).await?;
    let stored = load_stored_clock(db, id).await?;
    let (hlc, proof, proof_binding, journaled) = if let Some(fact) = latest.filter(|fact| {
        fact["payload"]["journal_version"] == super::super::node_history::JOURNAL_VERSION
            && fact["payload"]["scope"].as_str() == Some(scope.as_str())
            && journal_covers(&fact["payload"]["after_state"], &current, &row, &stored.winners)
    }) {
        anyhow::ensure!(
            fact["node_id"].as_str() == Some(id)
                && fact["row_version"]
                    .as_i64()
                    .is_some_and(|version| version > 0)
                && fact["source_did"]
                    .as_str()
                    .is_some_and(|source| !source.is_empty())
                && fact["log_id"].as_str().is_some_and(|id| !id.is_empty()),
            "invalid local journal publication proof"
        );
        super::super::node_history::require_committed_boundary(db, &fact, id, actor).await?;
        let hlc = fact["hlc_timestamp"]
            .as_u64()
            .filter(|hlc| *hlc > 0)
            .ok_or_else(|| anyhow::anyhow!("local journal HLC is unavailable"))?;
        (hlc,"IF (SELECT * OMIT id FROM node_log WHERE node_id=$scene_id ORDER BY row_version DESC LIMIT 1) != [$scene_proof] { THROW 'local scene journal changed'; }",fact,true)
    } else {
        let marker = row
            .get("tombstone")
            .filter(|value| value.is_object())
            .ok_or_else(|| {
                anyhow::anyhow!("scene state has no matching durable local journal; not re-signed")
            })?;
        let hlc = marker["hlc"]
            .as_u64()
            .filter(|hlc| *hlc > 0)
            .ok_or_else(|| anyhow::anyhow!("tombstone has no original HLC"))?;
        let mut response=db.query("SELECT * OMIT id FROM op_log WHERE op_type='NodeTombstoned' AND lamport_clock=$scene_original_hlc LIMIT 2 TIMEOUT 5s")
            .bind(("scene_original_hlc",i64::try_from(hlc)?)).await?.check()?;
        let ops: Vec<Value> = response.take(0)?;
        anyhow::ensure!(ops.len() == 1, "tombstone has no unique local operation");
        let op = &ops[0];
        let payload = &op["payload"];
        let root = payload["node_id"].as_str().unwrap_or_default();
        let member = root == id
            || payload["tombstoned_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|candidate| candidate.as_str() == Some(id)));
        anyhow::ensure!(
            member
                && !root.is_empty()
                && payload["petal_id"].as_str() == Some(petal.as_str())
                && marker["source_did"]
                    .as_str()
                    .is_some_and(|source| !source.is_empty())
                && payload["source_did"] == marker["source_did"],
            "tombstone local provenance mismatch"
        );
        bindings.push(("scene_original_hlc".into(), json!(i64::try_from(hlc)?)));
        (hlc,"IF (SELECT * OMIT id FROM op_log WHERE op_type='NodeTombstoned' AND lamport_clock=$scene_original_hlc) != [$scene_proof] { THROW 'local tombstone proof changed'; }",op.clone(),false)
    };
    crate::op_log::admit_scene_hlc(hlc)?;
    let cause = proof_binding["source_did"]
        .as_str()
        .or_else(|| proof_binding["payload"]["source_did"].as_str())
        .unwrap_or(actor)
        .to_owned();
    let clock = SceneClock {
        hlc,
        publisher: actor.into(),
    };
    let clocks = read_clock(db, id).await?;
    let known = clock_of(&clocks)?;
    // Over a stale cached winner only a key an unpublished local fact changed is stamped (§rejudge-pending).
    let local_writes: BTreeSet<String> = if journaled {
        let published: BTreeSet<u64> = stored.versions.iter().map(|version| version.hlc).collect();
        super::inbound::local_writes(db, id, &scope, &published).await?.into_keys().collect()
    } else {
        BTreeSet::new()
    };
    let stamped = stamp_own_versions(stored, id, &verse, actor, hlc, &row, Some(&local_writes))?;
    // Per-key rule: below a known later clock a journaled edit still publishes while it holds a winning key;
    // a tombstone keeps the strict rule. See AGENTS.md §Wire and ordering "Per-key clock".
    anyhow::ensure!(
        known.as_ref().is_none_or(|known| *known <= clock) || (journaled && stamped.holds_winner),
        "local snapshot was superseded by a known scene version"
    );
    // The clock row is never lowered: below a known later clock it is rewritten unchanged.
    let scene_clock = known.filter(|known| *known > clock).unwrap_or_else(|| clock.clone());
    let mut without_position = row.clone();
    without_position.as_object_mut().unwrap().remove("position");
    statements.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$scene_id) != [$scene_without_position] { THROW 'committed scene row changed'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$scene_id AND petal_id=$scene_petal AND position=<geometry<point>> [$scene_x,$scene_z])) != 1 { THROW 'committed scene ancestry or point changed'; }".into(),
        proof.into(),CLOCK_FENCE.into(),CLOCK_WRITE.into(),CLOCK_VERIFY.into(),
        FIELD_CLOCK_FENCE.into(),OWN_VERSIONS_FENCE.into(),
    ]);
    guard_labels.extend([
        "node_projection",
        "point_ancestry",
        "local_proof",
        "scene_clock",
        "clock_write",
        "clock_write",
        "field_clock",
        "field_clock",
    ]);
    bindings.extend([
        ("scene_field_clock_before".into(), stamped.clock_before),
        ("scene_own_before".into(), stamped.own_before),
    ]);
    for (fragment_statements, fragment_bindings) in stamped.writes {
        guard_labels.extend(vec!["field_clock_write"; fragment_statements.len()]);
        statements.extend(fragment_statements);
        bindings.extend(fragment_bindings);
    }
    bindings.extend([
        ("scene_id".into(), json!(id)),
        ("scene_petal".into(), json!(petal)),
        ("scene_without_position".into(), without_position),
        ("scene_x".into(), row["position"]["coordinates"][0].clone()),
        ("scene_z".into(), row["position"]["coordinates"][1].clone()),
        ("scene_proof".into(), proof_binding),
        ("scene_clock_before".into(), json!(clocks)),
        ("scene_hlc".into(), json!(i64::try_from(scene_clock.hlc)?)),
        ("scene_publisher".into(), json!(scene_clock.publisher)),
        ("scene_own_author".into(), json!(actor)),
    ]);
    let (dependencies, deferred_dependencies) =
        dependencies(db, &row, &scope, &mut statements, &mut bindings).await?;
    guard_labels.resize(statements.len(), "asset_metadata");
    wire::normalize_optional_columns(&mut row);
    row[wire::MARKER] = clock.marker();
    row[field_clock::CLOCK_COLUMN] = field_clock::encode_own(actor, &stamped.own);
    wire::validate_complete(&row)?;
    Ok(PreparedSceneRow {
        node_id: id.into(),
        petal_id: petal,
        verse_id: verse,
        bytes: serde_json::to_vec(&row)?,
        statements,
        guard_labels,
        bindings,
        dependencies,
        deferred_dependencies,
        cause,
    })
}

/// Hypothetical HLC for a projected row: the widest positive stored HLC, so the projection is never short.
const PROJECTED_HLC: u64 = i64::MAX as u64;

/// The complete wire row `publisher` would emit for `snapshot` (journal form) over `stored`, as the publisher builds it.
fn projected_row(stored: &StoredClock, id: &str, publisher: &str, snapshot: &Value) -> anyhow::Result<(Value, usize)> {
    let mut row = snapshot.clone();
    // The journal snapshot carries `[x, y, z]`; the stored row a point plus `elevation`.
    let point = match row["position"].as_array().map(Vec::as_slice) {
        Some([x, _, z]) => Some(json!({"type": "Point", "coordinates": [x, z]})),
        _ => None,
    };
    if let Some(point) = point {
        row["position"] = point;
    }
    let stamped = stamp_own_versions(stored.clone(), id, "", publisher, PROJECTED_HLC, &row, None)?;
    wire::normalize_optional_columns(&mut row);
    row[wire::MARKER] = SceneClock { hlc: PROJECTED_HLC, publisher: publisher.into() }.marker();
    row[field_clock::CLOCK_COLUMN] = field_clock::encode_own(publisher, &stamped.own);
    let keys = stamped.own.len();
    Ok((row, keys))
}

/// Why a projected row could not be published: over the key cap, over the row bound, or an unclockable key.
fn projection_failure(row: &Value, keys: usize, publisher: &str) -> Option<String> {
    let max_keys = field_clock::wire::MAX_OWN_KEYS;
    if keys > max_keys {
        return Some(format!(
            "{}: it would carry {keys} replicated keys (properties plus name, transform, interactive, asset and deleted properties); the limit is {max_keys}",
            wire::UNPUBLISHABLE
        ));
    }
    let bytes = serde_json::to_vec(row).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
    if bytes > wire::WRITE_ROW_BYTES {
        return Some(format!(
            "{}: its replicated row would be {bytes} bytes; the limit is {} (node plus own-version copy)",
            wire::UNPUBLISHABLE,
            wire::WRITE_ROW_BYTES
        ));
    }
    field_clock::parse_own("node", row, publisher)
        .err()
        .map(|error| format!("{}: {error:#}", wire::UNPUBLISHABLE))
}

/// Refuse a local write whose future published row could never be emitted, measured with the
/// publisher's own projection (AGENTS.md §row-bound "write-time check"). `before` is the current
/// journal snapshot (`None` for a new node); a write that does not make an already unpublishable
/// row worse is let through, so a node over the bound can still shrink.
pub(crate) async fn check_publishable(
    db: &Db,
    id: &str,
    publisher: &str,
    before: Option<&Value>,
    after: &Value,
) -> anyhow::Result<()> {
    let stored = match before {
        Some(_) => load_stored_clock(db, id).await?,
        None => StoredClock { versions: Vec::new(), cached: Vec::new(), winners: BTreeMap::new() },
    };
    let (row, keys) = projected_row(&stored, id, publisher, after)?;
    let Some(failure) = projection_failure(&row, keys, publisher) else {
        return Ok(());
    };
    if let Some(before) = before {
        let (previous, previous_keys) = projected_row(&stored, id, publisher, before)?;
        let size = |row: &Value| serde_json::to_vec(row).map(|bytes| bytes.len()).unwrap_or(usize::MAX);
        if projection_failure(&previous, previous_keys, publisher).is_some()
            && keys <= previous_keys
            && size(&row) <= size(&previous)
        {
            return Ok(());
        }
    }
    anyhow::bail!(failure)
}

/// [`check_publishable`] for journal changes not yet applied to the stored node (property writes).
pub(crate) async fn check_changes_publishable(
    db: &Db,
    id: &str,
    publisher: &str,
    scope: &str,
    changes: &[(String, Value, Value)],
) -> anyhow::Result<()> {
    let before = super::super::node_history::snapshot(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("node not found"))?;
    let mut after = before.clone();
    for (op, data, schema_versions) in changes {
        after = super::super::node_history::prepare_change(after, None, scope, op, data.clone(), schema_versions.clone())?.after;
    }
    check_publishable(db, id, publisher, Some(&before), &after).await
}

/// The node's cached per-key winners must be the ones the stamp was computed from (order-independent).
const FIELD_CLOCK_FENCE: &str = "IF array::sort((SELECT VALUE [field_key, hlc, author] FROM row_field_clock WHERE tbl='node' AND rid=$scene_id)) != array::sort($scene_field_clock_before) { THROW 'field clock changed'; }";
/// The publisher's own stored versions must be the ones the stamp was folded against.
const OWN_VERSIONS_FENCE: &str = "IF array::sort((SELECT VALUE [field_key, hlc] FROM row_field_version WHERE tbl='node' AND rid=$scene_id AND author=$scene_own_author)) != array::sort($scene_own_before) { THROW 'own field versions changed'; }";

/// The publisher's per-key own versions for one committed node row, plus the in-transaction writes.
struct StampedOwnVersions {
    /// Every own version after stamping (losing and deleted ones included), sorted by key.
    own: Vec<FieldVersion>,
    clock_before: Value,
    own_before: Value,
    writes: Vec<field_clock::store::SqlFragment>,
    /// Whether `actor` holds at least one key's winner once the stamp is folded in.
    holds_winner: bool,
}

/// One node's stored per-key versions and cached winners, read once per publication.
#[derive(Clone)]
struct StoredClock {
    versions: Vec<FieldVersion>,
    cached: Vec<Value>,
    /// Cached winners backed by their stored version; a stale cache entry is left out.
    winners: BTreeMap<String, FieldVersion>,
}

/// Read the node's stored versions and the cached winners they back.
async fn load_stored_clock(db: &Db, id: &str) -> anyhow::Result<StoredClock> {
    let versions = field_clock::load_versions(db, "node", id).await?;
    let mut response = db
        .query("SELECT field_key, hlc, author FROM row_field_clock WHERE tbl='node' AND rid=$scene_id TIMEOUT 5s")
        .bind(("scene_id", id.to_owned()))
        .await?
        .check()?;
    let cached: Vec<Value> = response.take(0)?;
    let mut winners: BTreeMap<String, FieldVersion> = BTreeMap::new();
    for entry in &cached {
        let (Some(field), Some(author), Some(winner_hlc)) = (
            entry["field_key"].as_str(),
            entry["author"].as_str(),
            entry["hlc"].as_u64(),
        ) else {
            anyhow::bail!("stored field clock row is invalid");
        };
        // A cached winner without its version is stale; the stamp below claims the key only over a local write.
        if let Some(version) = versions.iter().find(|version| {
            version.field == field && version.author == author && version.hlc == winner_hlc
        }) {
            winners.insert(field.to_owned(), version.clone());
        }
    }
    Ok(StoredClock { versions, cached, winners })
}

/// The journal's after-state is the committed state except on keys a stored non-floor winner now
/// holds (merged after the edit); those keys are never stamped as the publisher's own.
fn journal_covers(
    after_state: &Value,
    current: &Value,
    row: &Value,
    winners: &BTreeMap<String, FieldVersion>,
) -> bool {
    if after_state == current {
        return true;
    }
    let canonical_keys = |source: &Value| -> BTreeMap<String, Value> {
        field_clock::keys_of("node", source)
            .into_iter()
            .map(|(field, value)| (field, canonical_value(&value)))
            .collect()
    };
    let journal = canonical_keys(after_state);
    let normalized = canonical_keys(current);
    let committed = canonical_keys(row);
    let fields: BTreeSet<&String> = journal.keys().chain(normalized.keys()).collect();
    let mut merged = Vec::new();
    for field in fields {
        let value = normalized.get(field);
        if journal.get(field) == value {
            continue;
        }
        // A merged tombstone is never re-published under a live journal fact.
        if field == "tombstone" {
            return false;
        }
        // A key a re-judge demoted (no committed value, no winner left) was merged away; it is never stamped.
        let demoted = value.is_none() && !committed.contains_key(field) && !winners.contains_key(field);
        let held = demoted || winners.get(field).is_some_and(|winner| {
            !winner.is_legacy_floor()
                && match committed.get(field) {
                    Some(stored) => {
                        !winner.deleted && winner.value.as_ref().map(canonical_value).as_ref() == Some(stored)
                    }
                    None => winner.deleted || winner.value.is_none(),
                }
        });
        if !held {
            return false;
        }
        merged.push(FieldVersion {
            field: field.clone(),
            author: String::new(),
            hlc: 0,
            deleted: value.is_none(),
            value: value.cloned(),
        });
    }
    // Every column outside the merged keys must still match the journal exactly.
    field_clock::apply_keys("node", after_state, &merged) == *current
}

/// Stamp every key whose committed value is not its cached winner as `actor`'s version at `hlc`;
/// unchanged keys keep their versions (never restamped). See AGENTS.md §Wire and ordering "Per-key clock".
/// With `local_writes`, a key whose cached winner is stale is stamped only if listed (§rejudge-pending).
fn stamp_own_versions(
    stored: StoredClock,
    id: &str,
    verse: &str,
    actor: &str,
    hlc: u64,
    row: &Value,
    local_writes: Option<&BTreeSet<String>>,
) -> anyhow::Result<StampedOwnVersions> {
    let StoredClock { versions, cached, mut winners } = stored;
    let mut own: BTreeMap<String, FieldVersion> = versions
        .iter()
        .filter(|version| version.author == actor)
        .map(|version| (version.field.clone(), version.clone()))
        .collect();
    let own_before = own
        .values()
        .map(|version| -> anyhow::Result<Value> { Ok(json!([version.field, stored_hlc(version.hlc)?])) })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let committed: BTreeMap<String, Value> = field_clock::keys_of("node", row).into_iter().collect();
    let stamp = |field: &str, value: Option<Value>| FieldVersion {
        field: field.to_owned(),
        author: actor.to_owned(),
        hlc,
        deleted: value.is_none(),
        value,
    };
    let mut candidates = Vec::new();
    for (field, value) in &committed {
        // The legacy floor never travels on the wire, so a locally proven state claims it.
        let unchanged = winners.get(field).is_some_and(|winner| {
            !winner.is_legacy_floor()
                && !winner.deleted
                && winner.value.as_ref().map(canonical_value) == Some(canonical_value(value))
        });
        // A stale cached winner proves nothing was published; an unwritten value there is not the host's.
        let stale = !winners.contains_key(field)
            && cached.iter().any(|entry| entry["field_key"].as_str() == Some(field.as_str()));
        let unwritten = stale && local_writes.is_some_and(|keys| !keys.contains(field));
        if !unchanged && !unwritten {
            // Canonical so a retry re-reading the stored version emits identical bytes.
            candidates.push(stamp(field.as_str(), Some(canonical_value(value))));
        }
    }
    for (field, winner) in &winners {
        if !winner.deleted && !committed.contains_key(field) {
            candidates.push(stamp(field.as_str(), None));
        }
    }
    let mut written = Vec::new();
    for candidate in candidates {
        if let Some(kept) = field_clock::upsert_own(own.get(&candidate.field), candidate) {
            own.insert(kept.field.clone(), kept.clone());
            written.push(kept);
        }
    }
    let mut writes = Vec::new();
    if !written.is_empty() {
        for version in &written {
            let pair = match winners.get(&version.field) {
                Some(current) => vec![current.clone(), version.clone()],
                None => vec![version.clone()],
            };
            if field_clock::winner(&pair, |_| true) == pair.last() {
                winners.insert(version.field.clone(), version.clone());
            }
        }
        let winner_refs: Vec<&FieldVersion> = winners.values().collect();
        writes.push(field_clock::versions_write_sql("node", id, verse, &written)?);
        writes.push(field_clock::clock_write_sql("node", id, &winner_refs)?);
    }
    Ok(StampedOwnVersions {
        own: own.into_values().collect(),
        clock_before: Value::Array(
            cached
                .iter()
                .map(|entry| json!([entry["field_key"], entry["hlc"], entry["author"]]))
                .collect(),
        ),
        own_before: Value::Array(own_before),
        writes,
        holds_winner: winners.values().any(|winner| winner.author == actor),
    })
}

/// Capture publication metadata and its exact typed projection before interpretation.
pub(crate) async fn capture_publication_asset_metadata(
    db: &Db,
    id: &str,
) -> anyhow::Result<(Vec<Value>, Vec<String>, Vec<Value>)> {
    let projection = format!("SELECT {PUBLICATION_ASSET_COLUMNS} FROM asset WHERE asset_id=$scene_asset LIMIT 2 TIMEOUT 5s");
    let mut response = db.query(format!("SELECT VALUE {{ metadata: $this, fingerprint: encoding::base64::encode(encoding::cbor::encode($this)) }} FROM ({projection}) TIMEOUT 5s; SELECT asset_id FROM model WHERE asset_id=$scene_asset LIMIT 1 TIMEOUT 5s"))
        .bind(("scene_asset", id.to_owned())).await?.check()?;
    let captured: Vec<Value> = response.take(0)?;
    let models: Vec<Value> = response.take(1)?;
    let mut rows = Vec::with_capacity(captured.len());
    let mut fingerprints = Vec::with_capacity(captured.len());
    for captured in captured {
        let metadata = captured.get("metadata").filter(|value| value.is_object())
            .ok_or_else(|| anyhow::anyhow!("scene publication asset projection unavailable"))?;
        let fingerprint = captured["fingerprint"].as_str().filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("scene publication asset witness unavailable"))?;
        rows.push(metadata.clone());
        fingerprints.push(fingerprint.to_owned());
    }
    Ok((rows, fingerprints, models))
}

/// Fence all nine publication fields and exact row count without JSON rebinding.
pub(crate) fn publication_asset_metadata_fence() -> String {
    format!("IF (SELECT VALUE encoding::base64::encode(encoding::cbor::encode($this)) FROM (SELECT {PUBLICATION_ASSET_COLUMNS} FROM asset WHERE asset_id=$scene_asset)) != $scene_asset_fingerprints {{ THROW 'scene asset changed'; }}")
}

/// One `petal`-anchored, upload/adoption-proven asset as marked scene wire bytes plus its
/// metadata fence; `None` when unproven. The flag reports an unreplicated Portal model row.
async fn proven_asset_wire(
    db: &Db,
    id: &str,
    petal: &Value,
    scope: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
    let (assets, fingerprints, models) = capture_publication_asset_metadata(db, id).await?;
    if !(assets.len() == 1
        && assets[0]["petal_id"] == *petal
        && super::super::scene_asset_transfer::publication_proof(db,&assets[0],scope,guards,bindings).await?
        && assets[0]["content_hash"]
            .as_str()
            .is_some_and(|hash| crate::hash_from_hex(hash).is_ok()))
    {
        return Ok((None, !models.is_empty()));
    }
    let mut asset = assets[0].clone();
    if let Some(object) = asset.as_object_mut() {
        object.remove("owner_did");
        object.remove("publication_provenance");
        object.retain(|_, value| !value.is_null());
    }
    asset[fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER] = json!(1);
    let bytes = serde_json::to_vec(&asset)?;
    anyhow::ensure!(
        bytes.len() <= 32 * 1024,
        "scene asset metadata exceeds 32 KiB"
    );
    guards.push(publication_asset_metadata_fence());
    bindings.extend([
        ("scene_asset".into(), json!(id)),
        ("scene_asset_fingerprints".into(), json!(fingerprints)),
    ]);
    Ok((Some(bytes), !models.is_empty()))
}

async fn dependencies(
    db: &Db,
    row: &Value,
    scope: &str,
    guards: &mut Vec<String>,
    bindings: &mut Vec<(String, Value)>,
) -> anyhow::Result<(Vec<(String, String, Vec<u8>)>, usize)> {
    let mut dependencies = Vec::new();
    let mut deferred = 0;
    if let Some(id) = row["asset_id"].as_str() {
        let (wire, modelled) =
            proven_asset_wire(db, id, &row["petal_id"], scope, guards, bindings).await?;
        match wire {
            Some(bytes) => dependencies.push(("asset".into(), id.into(), bytes)),
            None => deferred += 1,
        }
        if modelled {
            deferred += 1;
        }
    }
    let scopes = super::super::field_def::property_rule_scopes(scope);
    let mut response=db.query("SELECT field_def_id FROM field_def WHERE scope IN $scene_schema_scopes AND entity_type='node' LIMIT 1 TIMEOUT 5s")
        .bind(("scene_schema_scopes",scopes)).await?.check()?;
    let schemas: Vec<Value> = response.take(0)?;
    if !schemas.is_empty() {
        deferred += 1;
    }
    Ok((dependencies, deferred))
}

impl PreparedSceneRow {
    /// Wire bytes this row and its dependencies enqueue.
    fn wire_bytes(&self) -> anyhow::Result<usize> {
        self.dependencies
            .iter()
            .try_fold(self.bytes.len(), |sum, (_, _, bytes)| sum.checked_add(bytes.len()))
            .ok_or_else(|| anyhow::anyhow!("scene recovery size overflow"))
    }

    pub(crate) async fn emit(
        self,
        db: &Db,
        blobs: &BlobStoreHandle,
        sender: &ReplicationSender,
    ) -> anyhow::Result<usize> {
        Ok(self.emit_hashed(db, blobs, sender).await?.0)
    }

    /// [`Self::emit`], also returning the node row's canonical blob hash.
    async fn emit_hashed(
        self,
        db: &Db,
        blobs: &BlobStoreHandle,
        sender: &ReplicationSender,
    ) -> anyhow::Result<(usize, crate::BlobHash)> {
        run_fences(db, &self.statements, self.bindings, &self.guard_labels).await?;
        for (table, id, bytes) in self.dependencies {
            enqueue(
                blobs,
                sender,
                &self.verse_id,
                &self.petal_id,
                &table,
                &id,
                &bytes,
            )?;
        }
        let hash = enqueue(
            blobs,
            sender,
            &self.verse_id,
            &self.petal_id,
            "node",
            &self.node_id,
            &self.bytes,
        )?;
        if self.deferred_dependencies > 0 {
            tracing::warn!(node_id=self.node_id,deferred_dependencies=self.deferred_dependencies,
                "scene row enqueued; model/schema, unanchored or locally unproven asset dependencies remain deferred; blob delivery is not certified");
        }
        Ok((self.deferred_dependencies, hash))
    }
}

fn enqueue(
    blobs: &BlobStoreHandle,
    sender: &ReplicationSender,
    verse: &str,
    petal: &str,
    table: &str,
    id: &str,
    bytes: &[u8],
) -> anyhow::Result<crate::BlobHash> {
    let content_hash = blobs.add_blob(bytes)?;
    match sender.try_send(ReplicationEvent {
        verse_id: verse.into(),
        table: table.into(),
        record_id: id.into(),
        content_hash,
        petal_id: Some(petal.into()),
    }) {
        Ok(()) => Ok(content_hash),
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            crate::note_replication_drop();
            anyhow::bail!("scene replication bridge full; committed source remains recoverable")
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => anyhow::bail!(
            "scene replication bridge disconnected; committed source remains recoverable"
        ),
    }
}

/// Execute publication fences in one transaction; nothing is enqueued unless all hold.
async fn run_fences(
    db: &Db,
    statements: &[String],
    bindings: Vec<(String, Value)>,
    guard_labels: &[&'static str],
) -> anyhow::Result<()> {
    let script = format!(
        "BEGIN TRANSACTION; {}; COMMIT TRANSACTION;",
        statements.join(";\n")
    );
    let mut query = db.query(script);
    for binding in bindings {
        query = query.bind(binding);
    }
    let mut response = query
        .await
        .map_err(|error| super::diagnostics::Transaction::Publication.request_failed(error))?;
    let errors = response.take_errors();
    if !errors.is_empty() {
        return Err(super::diagnostics::Transaction::Publication.failed(errors, guard_labels));
    }
    Ok(())
}

/// QA-0158: an anchored scene upload is Verse content at registration, not at first placement.
pub(crate) async fn publish_uploaded_asset(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    asset_id: &str,
) {
    let Some(sender) = sender else { return };
    match emit_uploaded_asset(db, blobs, sender, actor, asset_id).await {
        Ok(true) => tracing::info!(asset_id, "uploaded scene asset metadata enqueued for publication"),
        Ok(false) => tracing::debug!(asset_id, "non-scene upload: published by the device payload that references it"),
        Err(error) => tracing::warn!(asset_id, "uploaded scene asset publication deferred: {error:#}"),
    }
}

/// Fence actual Editor authority, upload/adoption proof and exact metadata, then enqueue
/// the marked row; `Ok(false)` for a non-scene MIME (see AGENTS.md §uploaded-assets).
async fn emit_uploaded_asset(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: &ReplicationSender,
    actor: &str,
    asset_id: &str,
) -> anyhow::Result<bool> {
    let mut response = db
        .query("SELECT petal_id, content_type FROM asset WHERE asset_id=$scene_asset LIMIT 2 TIMEOUT 5s")
        .bind(("scene_asset", asset_id.to_owned()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(rows.len() == 1, "uploaded asset missing or ambiguous");
    if !rows[0]["content_type"]
        .as_str()
        .is_some_and(|mime| fe_runtime::scene_asset::SCENE_ASSET_MIMES.contains(&mime))
    {
        return Ok(false);
    }
    let petal = rows[0]["petal_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("uploaded asset has no anchor Petal; it never replicates"))?
        .to_owned();
    let scope = super::super::crud::resolve_petal_scope_handler(db, &petal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("uploaded asset anchor ancestry missing"))?;
    let verse = crate::parse_scope(&scope)?.verse_id;
    let (mut statements, mut bindings) = super::super::entity::prepare_scope_permission(
        db,
        &scope,
        actor,
        fe_policy::RoleLevel::Editor,
    )
    .await?
    .into_parts();
    let mut guard_labels = vec!["authority_ancestry"; statements.len()];
    let (wire, _) =
        proven_asset_wire(db, asset_id, &json!(petal), &scope, &mut statements, &mut bindings)
            .await?;
    let wire = wire.ok_or_else(|| {
        anyhow::anyhow!("uploaded asset lacks local upload/adoption proof or a valid digest")
    })?;
    guard_labels.resize(statements.len(), "asset_metadata");
    run_fences(db, &statements, bindings, &guard_labels).await?;
    enqueue(blobs, sender, &verse, &petal, "asset", asset_id, &wire)?;
    Ok(true)
}

/// Replica-open republication of proven scene uploads no node references yet; returns
/// `(enqueued, deferred)`. Referenced assets travel as their nodes' dependencies.
pub(crate) async fn recover_uploaded_assets(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: &ReplicationSender,
    actor: &str,
    verse: &str,
) -> anyhow::Result<(usize, usize)> {
    let mut response = db.query("SELECT asset_id FROM asset WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$scene_verse)) AND content_type IN $scene_mimes AND (owner_did != NONE OR publication_provenance != NONE) AND asset_id NOT IN (SELECT VALUE asset_id FROM node WHERE asset_id != NONE) ORDER BY asset_id LIMIT 1025 TIMEOUT 5s")
        .bind(("scene_verse", verse.to_owned()))
        .bind(("scene_mimes", fe_runtime::scene_asset::SCENE_ASSET_MIMES.map(String::from).to_vec()))
        .await?
        .check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() <= RECOVERY_LIMIT,
        "upload recovery exceeds 1024 unplaced assets; no prefix published"
    );
    let (mut enqueued, mut deferred) = (0, 0);
    for row in rows {
        let id = row["asset_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid stored asset identity"))?;
        match emit_uploaded_asset(db, blobs, sender, actor, id).await {
            Ok(true) => enqueued += 1,
            Ok(false) => deferred += 1,
            Err(error) => {
                deferred += 1;
                tracing::debug!(asset_id = id, "upload recovery row deferred: {error:#}");
            }
        }
    }
    Ok((enqueued, deferred))
}

/// Commit success stays success when publication fails; the durable source is retried on open.
/// Once the new row is enqueued, the node's previous row blob is reclaimed (AGENTS.md §publish-reclaim).
pub(crate) async fn publish_after_commit(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    id: &str,
) -> Option<Published> {
    let sender = sender?;
    let result = async {
        let prepared = prepare(db, id, actor).await?;
        let published = Published {
            verse_id: prepared.verse_id.clone(),
            cause: prepared.cause.clone(),
            bytes: prepared.bytes.len(),
        };
        let (_, hash) = prepared.emit_hashed(db, blobs, sender).await?;
        Ok::<_, anyhow::Error>((published, hash))
    }
    .await;
    match result {
        Ok((published, hash)) => {
            super::reclaim::superseded(db, blobs, id, hash).await;
            Some(published)
        }
        Err(error) => {
            tracing::warn!(
                node_id = id,
                "committed scene publication deferred: {error:#}"
            );
            None
        }
    }
}

/// Publish the transforms the DB lane deferred (QA-0161), each node's current committed state once.
pub(crate) async fn publish_deferred(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    ids: Vec<String>,
) -> Vec<(String, Published)> {
    let started = std::time::Instant::now();
    let count = ids.len();
    let mut published = Vec::with_capacity(count);
    for id in ids {
        if let Some(row) = publish_after_commit(db, blobs, sender, actor, &id).await {
            published.push((id, row));
        }
    }
    tracing::debug!(count, elapsed_ms = started.elapsed().as_millis() as u64, "deferred scene publications sent");
    published
}

/// Recover every row stamped by this exact committed delete, including direct waypoint cascades;
/// each row's `tombstone` key becomes an own version at the tombstone HLC.
pub(crate) async fn publish_tombstones_after_commit(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    id: &str,
) {
    if sender.is_none() {
        return;
    }
    let result = async {
        let mut response = db
            .query("SELECT tombstone FROM node WHERE node_id=$scene_id LIMIT 2 TIMEOUT 5s")
            .bind(("scene_id", id.to_owned()))
            .await?
            .check()?;
        let root: Vec<Value> = response.take(0)?;
        anyhow::ensure!(root.len() == 1, "delete root unavailable");
        let marker = root[0]["tombstone"].clone();
        anyhow::ensure!(marker.is_object(), "delete root is not tombstoned");
        let mut response = db
            .query("SELECT node_id FROM node WHERE tombstone=$scene_marker LIMIT 1025 TIMEOUT 5s")
            .bind(("scene_marker", marker))
            .await?
            .check()?;
        let rows: Vec<Value> = response.take(0)?;
        anyhow::ensure!(
            rows.len() <= RECOVERY_LIMIT,
            "delete recovery exceeds 1024 rows; no prefix published"
        );
        for row in rows {
            let id = row["node_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid delete member"))?;
            let (_, hash) = prepare(db, id, actor)
                .await?
                .emit_hashed(db, blobs, sender.unwrap())
                .await?;
            super::reclaim::superseded(db, blobs, id, hash).await;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(
            node_id = id,
            "committed scene tombstone publication deferred: {error:#}"
        );
    }
}

/// Re-publish every stored node of a Verse in bounded batches (AGENTS.md §recovery-batches);
/// never infer local provenance from author names. Existing own versions are re-emitted as
/// stored (never restamped); only uncommitted gaps are stamped.
pub(crate) async fn recover(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    verse: &str,
) -> anyhow::Result<RecoveryReport> {
    let Some(sender) = sender else {
        return Ok(RecoveryReport::default());
    };
    let mut report = RecoveryReport::default();
    let mut batch = Vec::new();
    let mut batch_bytes = 0usize;
    // Keyset pagination: each page starts strictly after the last examined id, so every page progresses.
    let mut after = String::new();
    loop {
        let mut response=db.query("SELECT node_id FROM node WHERE node_id > $scene_after AND petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$scene_verse)) ORDER BY node_id LIMIT 1024 TIMEOUT 5s")
            .bind(("scene_verse",verse.to_owned())).bind(("scene_after",after.clone())).await?.check()?;
        let rows: Vec<Value> = response.take(0)?;
        let page = rows.len();
        for row in rows {
            let id = row["node_id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("invalid stored scene identity"))?
                .to_owned();
            report.examined += 1;
            match prepare(db, &id, actor).await {
                Ok(prepared) => {
                    anyhow::ensure!(prepared.verse_id == verse, "scene recovery ancestry changed");
                    let size = prepared.wire_bytes()?;
                    if size > RECOVERY_BYTES {
                        // Unreachable while the row bound is far below the batch budget; never loops.
                        report.deferred += 1;
                        report.oversized += 1;
                        tracing::warn!(node_id = %id, size, "scene row exceeds the recovery batch budget and cannot be published");
                    } else {
                        if batch.len() == RECOVERY_LIMIT || batch_bytes + size > RECOVERY_BYTES {
                            emit_batch(db, blobs, sender, &mut batch, &mut report).await?;
                            batch_bytes = 0;
                        }
                        batch_bytes += size;
                        batch.push(prepared);
                    }
                }
                Err(error) => {
                    report.deferred += 1;
                    if format!("{error:#}").contains(wire::ROW_TOO_LARGE) {
                        report.oversized += 1;
                        tracing::warn!(node_id = %id, "scene row is over the row bound and cannot be published");
                    } else {
                        tracing::debug!(node_id = %id, "scene recovery row deferred: {error:#}");
                    }
                }
            }
            after = id;
        }
        if page < RECOVERY_LIMIT {
            break;
        }
    }
    emit_batch(db, blobs, sender, &mut batch, &mut report).await?;
    match recover_uploaded_assets(db, blobs, sender, actor, verse).await {
        Ok((enqueued, deferred)) => {
            report.uploads_enqueued = enqueued;
            report.dependency_deferred += deferred;
        }
        Err(error) => {
            report.dependency_deferred += 1;
            tracing::warn!(verse, "unplaced scene upload recovery refused: {error:#}");
        }
    }
    Ok(report)
}

/// Emit one prepared recovery batch in id order; each row re-runs its own fences at emission.
async fn emit_batch(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: &ReplicationSender,
    batch: &mut Vec<PreparedSceneRow>,
    report: &mut RecoveryReport,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    if report.batches > 0 {
        drain_pause(sender).await;
    }
    for row in batch.drain(..) {
        let node_id = row.node_id.clone();
        let (dependencies, hash) = row.emit_hashed(db, blobs, sender).await.map_err(|error| {
            anyhow::anyhow!(
                "scene recovery stopped after {} rows enqueued in batch {}; not complete: {error:#}",
                report.enqueued,
                report.batches + 1
            )
        })?;
        // Recovery only remembers what it re-sent; the next local publish reclaims it (AGENTS.md §publish-reclaim).
        super::reclaim::record(blobs, &node_id, hash);
        report.enqueued += 1;
        report.dependency_deferred += dependencies;
    }
    report.batches += 1;
    Ok(())
}

/// Bounded wait for the replication bridge to fall below half capacity before the next batch.
async fn drain_pause(sender: &ReplicationSender) {
    let Some(capacity) = sender.capacity() else {
        return;
    };
    let deadline = tokio::time::Instant::now() + BATCH_DRAIN_WAIT;
    while sender.len() > capacity / 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}
