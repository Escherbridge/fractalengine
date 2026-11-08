//! Publish durable local readback only; see AGENTS.md.

use super::{
    clock_of, read_clock,
    wire::{self, SceneClock},
    CLOCK_FENCE, CLOCK_VERIFY, CLOCK_WRITE,
};
use crate::{repo::Db, BlobStoreHandle, ReplicationEvent, ReplicationSender};
use serde_json::{json, Value};

const RECOVERY_LIMIT: usize = 1024;
const RECOVERY_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const PUBLICATION_ASSET_COLUMNS: &str =
    "asset_id, name, content_type, size_bytes, content_hash, created_at, owner_did, petal_id, publication_provenance";

#[derive(Default, Debug)]
pub(crate) struct RecoveryReport {
    pub examined: usize,
    pub enqueued: usize,
    pub deferred: usize,
    pub dependency_deferred: usize,
    /// Proven scene uploads no node references yet (QA-0158).
    pub uploads_enqueued: usize,
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
    let (hlc, proof, proof_binding) = if let Some(fact) = latest.filter(|fact| {
        fact["payload"]["journal_version"] == super::super::node_history::JOURNAL_VERSION
            && fact["payload"]["scope"].as_str() == Some(scope.as_str())
            && fact["payload"]["after_state"] == current
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
        (hlc,"IF (SELECT * OMIT id FROM node_log WHERE node_id=$scene_id ORDER BY row_version DESC LIMIT 1) != [$scene_proof] { THROW 'local scene journal changed'; }",fact)
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
        (hlc,"IF (SELECT * OMIT id FROM op_log WHERE op_type='NodeTombstoned' AND lamport_clock=$scene_original_hlc) != [$scene_proof] { THROW 'local tombstone proof changed'; }",op.clone())
    };
    crate::op_log::admit_scene_hlc(hlc)?;
    let clock = SceneClock {
        hlc,
        publisher: actor.into(),
    };
    let clocks = read_clock(db, id).await?;
    anyhow::ensure!(
        clock_of(&clocks)?
            .as_ref()
            .is_none_or(|known| *known <= clock),
        "local snapshot was superseded by a known scene version"
    );
    let mut without_position = row.clone();
    without_position.as_object_mut().unwrap().remove("position");
    statements.extend([
        "IF (SELECT * OMIT id, position FROM node WHERE node_id=$scene_id) != [$scene_without_position] { THROW 'committed scene row changed'; }".into(),
        "IF array::len((SELECT VALUE node_id FROM node WHERE node_id=$scene_id AND petal_id=$scene_petal AND position=<geometry<point>> [$scene_x,$scene_z])) != 1 { THROW 'committed scene ancestry or point changed'; }".into(),
        proof.into(),CLOCK_FENCE.into(),CLOCK_WRITE.into(),CLOCK_VERIFY.into(),
    ]);
    guard_labels.extend([
        "node_projection",
        "point_ancestry",
        "local_proof",
        "scene_clock",
        "clock_write",
        "clock_write",
    ]);
    bindings.extend([
        ("scene_id".into(), json!(id)),
        ("scene_petal".into(), json!(petal)),
        ("scene_without_position".into(), without_position),
        ("scene_x".into(), row["position"]["coordinates"][0].clone()),
        ("scene_z".into(), row["position"]["coordinates"][1].clone()),
        ("scene_proof".into(), proof_binding),
        ("scene_clock_before".into(), json!(clocks)),
        ("scene_hlc".into(), json!(i64::try_from(hlc)?)),
        ("scene_publisher".into(), json!(actor)),
    ]);
    let (dependencies, deferred_dependencies) =
        dependencies(db, &row, &scope, &mut statements, &mut bindings).await?;
    guard_labels.resize(statements.len(), "asset_metadata");
    wire::normalize_optional_columns(&mut row);
    row[wire::MARKER] = clock.marker();
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
    pub(crate) async fn emit(
        self,
        db: &Db,
        blobs: &BlobStoreHandle,
        sender: &ReplicationSender,
    ) -> anyhow::Result<usize> {
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
        enqueue(
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
        Ok(self.deferred_dependencies)
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
) -> anyhow::Result<()> {
    let content_hash = blobs.add_blob(bytes)?;
    match sender.try_send(ReplicationEvent {
        verse_id: verse.into(),
        table: table.into(),
        record_id: id.into(),
        content_hash,
        petal_id: Some(petal.into()),
    }) {
        Ok(()) => Ok(()),
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
pub(crate) async fn publish_after_commit(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    id: &str,
) {
    let Some(sender) = sender else { return };
    let result = async { prepare(db, id, actor).await?.emit(db, blobs, sender).await }.await;
    if let Err(error) = result {
        tracing::warn!(
            node_id = id,
            "committed scene publication deferred: {error:#}"
        );
    }
}

/// Publish the transforms the DB lane deferred (QA-0161), each node's current committed state once.
pub(crate) async fn publish_deferred(
    db: &Db,
    blobs: &BlobStoreHandle,
    sender: Option<&ReplicationSender>,
    actor: &str,
    ids: Vec<String>,
) {
    let started = std::time::Instant::now();
    let count = ids.len();
    for id in ids {
        publish_after_commit(db, blobs, sender, actor, &id).await;
    }
    tracing::debug!(count, elapsed_ms = started.elapsed().as_millis() as u64, "deferred scene publications sent");
}

/// Recover every row stamped by this exact committed delete, including direct waypoint cascades.
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
            prepare(db, id, actor)
                .await?
                .emit(db, blobs, sender.unwrap())
                .await?;
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

/// Refuse over-limit recovery before emitting any prefix; never infer local provenance from author names.
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
    let mut response=db.query("SELECT node_id FROM node WHERE petal_id IN (SELECT VALUE petal_id FROM petal WHERE fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE verse_id=$scene_verse)) ORDER BY node_id LIMIT 1025 TIMEOUT 5s")
        .bind(("scene_verse",verse.to_owned())).await?.check()?;
    let rows: Vec<Value> = response.take(0)?;
    anyhow::ensure!(
        rows.len() <= RECOVERY_LIMIT,
        "scene recovery exceeds 1024 rows; no prefix published"
    );
    let mut report = RecoveryReport {
        examined: rows.len(),
        ..Default::default()
    };
    let mut prepared = Vec::new();
    let mut bytes = 0usize;
    for row in rows {
        let id = row["node_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("invalid stored scene identity"))?;
        match prepare(db, id, actor).await {
            Ok(row) => {
                anyhow::ensure!(row.verse_id == verse, "scene recovery ancestry changed");
                bytes = bytes
                    .checked_add(
                        row.bytes.len()
                            + row
                                .dependencies
                                .iter()
                                .map(|(_, _, bytes)| bytes.len())
                                .sum::<usize>(),
                    )
                    .ok_or_else(|| anyhow::anyhow!("scene recovery size overflow"))?;
                anyhow::ensure!(
                    bytes <= RECOVERY_BYTES,
                    "scene recovery exceeds 8 MiB; no prefix published"
                );
                prepared.push(row);
            }
            Err(error) => {
                report.deferred += 1;
                tracing::debug!(node_id = id, "scene recovery row deferred: {error:#}");
            }
        }
    }
    for row in prepared {
        let dependencies = row.emit(db, blobs, sender).await.map_err(|error| {
            anyhow::anyhow!(
                "scene recovery stopped after {} rows enqueued; not complete: {error:#}",
                report.enqueued
            )
        })?;
        report.enqueued += 1;
        report.dependency_deferred += dependencies;
    }
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
