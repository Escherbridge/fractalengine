//! SPEC-10 §5 hydrator: admitted kind-5 batches expand into a derived, rebuildable local catalog,
//! validated in the §5.2 order; plus the G6 analytics read over it. See `canon_log/AGENTS.md`
//! §observation.

use std::collections::{BTreeMap, BTreeSet};

use fe_canonical_log::capability::{
    verify_chain_bytes, AuthorizationRequest, AuthorizationSnapshot, ObjectClass, Principal, Verb,
    VerificationOptions,
};
use fe_canonical_log::compose::SealedProofEnvironment;
use fe_canonical_log::compose::{CapabilityChainSource, Clock, ContentByteStore};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::ScopeKeyStore;
use fe_canonical_log::did_key::public_key_to_did;
use fe_canonical_log::envelope::{CompleteEnvelope, Hash32, Identifier32, Scope};
use fe_canonical_log::materialize::identity::{AnalyticsSourceIdentity, MaterializerVersion};
use fe_canonical_log::observation::{
    node_anchor, observation_batch_schema_hash, ObservationBatchPayload,
    ObservationBatchPayloadParts, OBSERVATION_BATCH_OPERATION_KIND,
};
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::segment::payload_shard::PayloadTopicScope;
use fe_canonical_log::segment::SegmentError;
use fe_query::columnar::observation::{
    read_observation_hexon, write_observation_hexon, ObservationHexon, PayloadRefRow, ReadingRow,
};
use serde::{Deserialize, Serialize};

use super::sealer::chain_leaf;
use super::{hydrates_observations, require_mode, ObservationPlaneError, ObservationTable};
use crate::canon_log::append_store::SurrealVerifiedLogStore;
use crate::canon_log::replay::{ReplayFactsLookup, ReplayPayloadFacts, VerifierReplayFacts};
use crate::canon_log::verifier::DurableCandidateVerifier;
use crate::canon_log::{identifier_to_hex, op_id_from_hex, op_id_to_hex, StorageError};
use crate::migration::mode::MigrationMode;
use crate::repo::Db;

/// What to hydrate and who hydrates it.
pub struct HydrationRequest<'a> {
    /// Only batches in this verse are hydrated.
    pub verse_id: Identifier32,
    /// The branch whose kind-5 operations are hydrated.
    pub branch_id: Identifier32,
    /// A locally held chain granting `fetch`, `decrypt` and `materialize` on `shard` (§5.1 rule 1).
    pub hydrator_chain_id: Hash32,
    /// The principal that chain is for.
    pub hydrator: &'a Principal,
}

/// SPEC-10 §5.3 rule 1: the one outcome of a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HydrationOutcome {
    /// Every §5.2 step passed; its rows entered the catalog.
    Hydrated,
    /// Retryable: an artifact, the scope key, the batch's own payload, or the hydrator's
    /// capability is not available here. No catalog row is substituted.
    ReferencedArtifactUnavailable {
        /// Which and why.
        reason: String,
    },
    /// Permanent for these bytes: authentic artifacts disagree with the profile or the signed
    /// batch. Its rows are excluded; the operation and artifacts stay as evidence.
    ObservationBatchInvalid {
        /// The failed rule.
        reason: String,
    },
}

impl HydrationOutcome {
    /// The `observation_hydration.outcome` label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Hydrated => "hydrated",
            Self::ReferencedArtifactUnavailable { .. } => "referenced_artifact_unavailable",
            Self::ObservationBatchInvalid { .. } => "observation_batch_invalid",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Hydrated => "",
            Self::ReferencedArtifactUnavailable { reason }
            | Self::ObservationBatchInvalid { reason } => reason,
        }
    }
}

/// One catalog reading (SPEC-10 §5.3 rule 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogReading {
    /// The row key.
    pub reading_id: String,
    /// Every hydrated batch that carried this exact row, lowercase hex, ascending.
    pub op_ids: Vec<String>,
    /// Batch verse identifier, lowercase hex.
    pub verse_id: String,
    /// Batch petal identifier, lowercase hex.
    pub petal_id: String,
    /// §4.5 anchor, lowercase hex.
    pub anchor: String,
    /// The anchor's legacy node id.
    pub anchor_node_id: String,
    /// The metric.
    pub metric: String,
    /// The value.
    pub value: f64,
    /// `value.to_bits()` as 16 lowercase hex digits.
    pub value_bits: String,
    /// The units.
    pub units: String,
    /// Sensor time text.
    pub recorded_at: String,
    /// Sensor time, Unix ms.
    pub recorded_at_ms: i64,
    /// Ingestion HLC wall ms.
    pub hlc_wall_ms: i64,
    /// Ingestion HLC counter.
    pub hlc_counter: i64,
    /// The payload this reading travelled with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_id: Option<String>,
    /// The canonical `ObservationSource`, lowercase hex.
    pub source: String,
}

/// One catalog device-payload reference.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPayload {
    /// The row key.
    pub payload_id: String,
    /// Every hydrated batch that carried this exact row, lowercase hex, ascending.
    pub op_ids: Vec<String>,
    /// Batch verse identifier, lowercase hex.
    pub verse_id: String,
    /// Batch petal identifier, lowercase hex.
    pub petal_id: String,
    /// §4.5 anchor, lowercase hex.
    pub anchor: String,
    /// The anchor's legacy node id.
    pub anchor_node_id: String,
    /// The declared format.
    pub format_id: String,
    /// Sensor time text.
    pub recorded_at: String,
    /// Sensor time, Unix ms.
    pub recorded_at_ms: i64,
    /// Ingestion HLC wall ms.
    pub hlc_wall_ms: i64,
    /// Ingestion HLC counter.
    pub hlc_counter: i64,
    /// Ordered asset ids.
    pub asset_ids: Vec<String>,
    /// `asset_content_hashes[i]` of `asset_ids[i]`, lowercase hex.
    pub asset_content_hashes: Vec<String>,
    /// The canonical `ObservationSource`, lowercase hex.
    pub source: String,
}

/// What one hydration pass produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HydrationReport {
    /// One outcome per hydrated-or-attempted batch.
    pub outcomes: BTreeMap<Hash32, HydrationOutcome>,
    /// A deterministic commitment to the whole catalog and every outcome (§5.3 rule 5).
    pub catalog_root: Hash32,
    /// Readings in the catalog.
    pub readings: usize,
    /// Payload references in the catalog.
    pub payloads: usize,
    /// Conflicting identifiers, each with every batch that carried a copy (§5.3 rule 4).
    pub conflicts: Vec<(ObservationTable, String, Vec<Hash32>)>,
}

enum BatchHydration {
    Settled(HydrationOutcome),
    Hydrated(Vec<CatalogReading>, Vec<CatalogPayload>),
}

fn unavailable(reason: impl Into<String>) -> BatchHydration {
    BatchHydration::Settled(HydrationOutcome::ReferencedArtifactUnavailable {
        reason: reason.into(),
    })
}

fn invalid(reason: impl Into<String>) -> BatchHydration {
    BatchHydration::Settled(HydrationOutcome::ObservationBatchInvalid {
        reason: reason.into(),
    })
}

/// Rebuilds the whole catalog from the verified log and the artifacts held here (SPEC-10 §5).
///
/// `shadow_rebuild` only. Drops the previous catalog and hydrates every kind-5 operation of the
/// branch again, so the result is a function of the admitted batch set and the outcomes alone:
/// hydrating twice gives the same `catalog_root`. The catalog is never an input to `reduce`.
pub async fn hydrate_observation_catalog(
    mode: MigrationMode,
    db: &Db,
    verifier: &DurableCandidateVerifier,
    request: HydrationRequest<'_>,
) -> Result<HydrationReport, ObservationPlaneError> {
    require_mode(mode, hydrates_observations)?;
    let store = SurrealVerifiedLogStore::new(db.clone());
    let chain_bytes = verifier
        .parts()
        .chains
        .chain_bytes(&request.hydrator_chain_id);

    let mut outcomes = BTreeMap::new();
    let mut readings: BTreeMap<String, Vec<CatalogReading>> = BTreeMap::new();
    let mut payloads: BTreeMap<String, Vec<CatalogPayload>> = BTreeMap::new();
    for op_id in kind5_operations(db, request.branch_id).await? {
        let Some(hydration) =
            hydrate_batch(&store, verifier, chain_bytes.as_deref(), &request, op_id).await?
        else {
            continue;
        };
        match hydration {
            BatchHydration::Settled(outcome) => {
                outcomes.insert(op_id, outcome);
            }
            BatchHydration::Hydrated(batch_readings, batch_payloads) => {
                for row in batch_readings {
                    readings
                        .entry(row.reading_id.clone())
                        .or_default()
                        .push(row);
                }
                for row in batch_payloads {
                    payloads
                        .entry(row.payload_id.clone())
                        .or_default()
                        .push(row);
                }
                outcomes.insert(op_id, HydrationOutcome::Hydrated);
            }
        }
    }

    let mut conflicts = Vec::new();
    let catalog_readings = collapse(readings, ObservationTable::Reading, &mut conflicts, |row| {
        &mut row.op_ids
    });
    let catalog_payloads = collapse(payloads, ObservationTable::Payload, &mut conflicts, |row| {
        &mut row.op_ids
    });
    conflicts.sort();
    write_catalog(
        db,
        &catalog_readings,
        &catalog_payloads,
        &conflicts,
        &outcomes,
    )
    .await?;

    let catalog_root = catalog_root(&catalog_readings, &catalog_payloads, &conflicts, &outcomes)
        .map_err(|error| {
            ObservationPlaneError::Storage(StorageError::malformed("catalog", error))
        })?;
    Ok(HydrationReport {
        outcomes,
        catalog_root,
        readings: catalog_readings.len(),
        payloads: catalog_payloads.len(),
        conflicts,
    })
}

/// SPEC-10 §5.3 rules 3-4: byte-identical copies collapse into one row naming every batch;
/// copies that differ contribute nothing and become conflict evidence naming every batch.
fn collapse<T: Clone + PartialEq>(
    rows: BTreeMap<String, Vec<T>>,
    table: ObservationTable,
    conflicts: &mut Vec<(ObservationTable, String, Vec<Hash32>)>,
    op_ids_of: impl Fn(&mut T) -> &mut Vec<String>,
) -> Vec<T> {
    let mut catalog = Vec::with_capacity(rows.len());
    for (row_id, copies) in rows {
        let mut op_ids: Vec<String> = Vec::new();
        let mut contents: Vec<T> = Vec::with_capacity(copies.len());
        for mut copy in copies {
            op_ids.append(op_ids_of(&mut copy));
            contents.push(copy);
        }
        op_ids.sort();
        op_ids.dedup();
        let mut first = contents[0].clone();
        if contents.iter().all(|content| *content == first) {
            *op_ids_of(&mut first) = op_ids;
            catalog.push(first);
        } else {
            conflicts.push((
                table,
                row_id,
                op_ids
                    .iter()
                    .filter_map(|hex| op_id_from_hex(hex))
                    .collect(),
            ));
        }
    }
    catalog
}

async fn kind5_operations(db: &Db, branch_id: Identifier32) -> Result<Vec<Hash32>, StorageError> {
    let ids: Vec<String> = db
        .query(
            "SELECT VALUE op_id_hex FROM verified_op_log \
             WHERE operation_kind = $kind AND branch_id = $branch ORDER BY op_id_hex ASC",
        )
        .bind(("kind", i64::from(OBSERVATION_BATCH_OPERATION_KIND)))
        .bind(("branch", identifier_to_hex(branch_id)))
        .await
        .map_err(|error| StorageError::query("kind-5 operation read", error))?
        .check()
        .map_err(|error| StorageError::query("kind-5 operation read", error))?
        .take(0)
        .map_err(|error| StorageError::query("kind-5 operation read", error))?;
    Ok(ids.iter().filter_map(|hex| op_id_from_hex(hex)).collect())
}

/// One batch through §5.2, in order. `None` for a batch outside the requested verse.
async fn hydrate_batch(
    store: &SurrealVerifiedLogStore,
    verifier: &DurableCandidateVerifier,
    chain_bytes: Option<&[u8]>,
    request: &HydrationRequest<'_>,
    op_id: Hash32,
) -> Result<Option<BatchHydration>, StorageError> {
    // The index said kind 5; the re-verified bytes decide (canon_log §verified-log).
    let Some(meta) = store.meta_of(op_id).await? else {
        return Ok(Some(unavailable("operation bytes not readable")));
    };
    if meta.scope.verse_id() != request.verse_id
        || meta.operation_kind != OBSERVATION_BATCH_OPERATION_KIND
    {
        return Ok(None);
    }
    let Some(bytes) = store.envelope_bytes(op_id).await? else {
        return Ok(Some(unavailable("operation bytes not readable")));
    };
    let Ok(envelope) = CompleteEnvelope::decode_canonical(&bytes) else {
        return Ok(Some(unavailable("operation bytes do not decode")));
    };
    let Some(petal) = meta.scope.petal_id() else {
        return Ok(Some(invalid("an observation batch must be petal-scoped")));
    };
    let scope = meta.scope;
    let epoch = envelope.unsigned.capability.scope_epoch;

    let payload = match VerifierReplayFacts::new(verifier)
        .payload_facts(op_id, &bytes)
        .await
    {
        ReplayFactsLookup::Derived(facts) => {
            match ObservationBatchPayload::from_cbor(&facts.fields) {
                Ok(payload) => payload,
                Err(error) => return Ok(Some(invalid(format!("payload: {error}")))),
            }
        }
        ReplayFactsLookup::Unavailable { gap, .. } => {
            return Ok(Some(unavailable(format!("own payload facts: {gap:?}"))))
        }
        ReplayFactsLookup::NoPayload | ReplayFactsLookup::NotConsulted => {
            return Ok(Some(unavailable("own payload facts not derivable")))
        }
    };
    let parts = payload.parts();

    // §5.1 rule 1: fetch/shard, decrypt/shard, materialize/shard for the batch's scope.
    let Some(chain_bytes) = chain_bytes else {
        return Ok(Some(unavailable(
            "the hydrator's capability chain is not held",
        )));
    };
    if let Err(reason) = check_hydrator_authority(verifier, chain_bytes, request.hydrator, scope) {
        return Ok(Some(unavailable(reason)));
    }

    // §5.2 steps 1-2 for both artifacts.
    let hexon = match open_shard(
        verifier,
        scope,
        petal,
        epoch,
        parts.hexon_artifact_id,
        parts.hexon_stored_length,
    ) {
        Ok(content) => content,
        Err(settled) => return Ok(Some(settled)),
    };
    let stats = match open_shard(
        verifier,
        scope,
        petal,
        epoch,
        parts.stats_hash,
        parts.stats_stored_length,
    ) {
        Ok(content) => content,
        Err(settled) => return Ok(Some(settled)),
    };

    // §5.2 step 3: archive and Parquet profile.
    let decoded = match read_observation_hexon(&hexon) {
        Ok(decoded) => decoded,
        Err(error) => return Ok(Some(invalid(format!("hexon profile: {error}")))),
    };
    // §5.2 step 4: the hexon against its signed payload.
    if let Err(reason) = check_against_payload(parts, &decoded) {
        return Ok(Some(invalid(reason)));
    }
    // §5.2 steps 3 and 5: the pinned writer must reproduce both artifacts byte for byte from
    // these rows (§3.2 rule 8), which also proves every §3.3 rule 3 statistic.
    let author_did = public_key_to_did(&meta.author_public_key);
    if let Err(reason) = check_rederivation(&author_did, parts, &decoded, &hexon, &stats) {
        return Ok(Some(invalid(reason)));
    }

    let verse_hex = identifier_to_hex(scope.verse_id());
    let petal_hex = identifier_to_hex(petal);
    let op_hex = op_id_to_hex(op_id);
    let source_hex = |index: u32| {
        parts
            .sources
            .get(index as usize)
            .and_then(|source| source.encode_canonical().ok())
            .map(hex::encode)
            .unwrap_or_default()
    };
    let readings = decoded
        .readings
        .iter()
        .map(|row| CatalogReading {
            reading_id: row.reading_id.clone(),
            op_ids: vec![op_hex.clone()],
            verse_id: verse_hex.clone(),
            petal_id: petal_hex.clone(),
            anchor: hex::encode(row.anchor),
            anchor_node_id: row.anchor_node_id.clone(),
            metric: row.metric.clone(),
            value: row.value,
            value_bits: format!("{:016x}", row.value.to_bits()),
            units: row.units.clone(),
            recorded_at: row.recorded_at.clone(),
            recorded_at_ms: row.recorded_at_ms,
            hlc_wall_ms: row.hlc_wall_ms,
            hlc_counter: i64::from(row.hlc_counter),
            payload_id: row.payload_id.clone(),
            source: source_hex(row.source_index),
        })
        .collect();
    let payloads = decoded
        .payload_refs
        .iter()
        .map(|row| CatalogPayload {
            payload_id: row.payload_id.clone(),
            op_ids: vec![op_hex.clone()],
            verse_id: verse_hex.clone(),
            petal_id: petal_hex.clone(),
            anchor: hex::encode(row.anchor),
            anchor_node_id: row.anchor_node_id.clone(),
            format_id: row.format_id.clone(),
            recorded_at: row.recorded_at.clone(),
            recorded_at_ms: row.recorded_at_ms,
            hlc_wall_ms: row.hlc_wall_ms,
            hlc_counter: i64::from(row.hlc_counter),
            asset_ids: row.asset_ids.clone(),
            asset_content_hashes: row.asset_content_hashes.iter().map(hex::encode).collect(),
            source: source_hex(row.source_index),
        })
        .collect();
    Ok(Some(BatchHydration::Hydrated(readings, payloads)))
}

fn check_hydrator_authority(
    verifier: &DurableCandidateVerifier,
    chain_bytes: &[u8],
    hydrator: &Principal,
    scope: Scope,
) -> Result<(), String> {
    let leaf = chain_leaf(chain_bytes).map_err(|error| error.to_string())?;
    for verb in [Verb::Fetch, Verb::Decrypt, Verb::Materialize] {
        verify_chain_bytes(
            chain_bytes,
            &AuthorizationRequest {
                verb,
                object_class: ObjectClass::Shard,
                target_scope: scope,
                resource_id: None,
                requester: hydrator.clone(),
                operation_author: hydrator.clone(),
                operation_scope_epoch: leaf.scope_epoch,
                operation_kind: Some(OBSERVATION_BATCH_OPERATION_KIND),
                is_structural_operation: false,
                payload_bytes: None,
                segment_bytes: None,
                schema_hash: Some(observation_batch_schema_hash()),
                preview_hz: None,
                now_ms: verifier.parts().clock.now_ms(),
            },
            &verifier.parts().authority,
            &AuthorizationSnapshot {
                epoch_causally_valid_for_operation: false,
            },
            &VerificationOptions::live(),
        )
        .map_err(|error| format!("the hydrator lacks {verb:?}/shard: {error}"))?;
    }
    Ok(())
}

/// §5.2 steps 1-2 for one artifact: held, re-hashed, the signed length, then authenticated and
/// decrypted under the batch's petal topic as a `BulkArtifact` of class `shard`.
fn open_shard(
    verifier: &DurableCandidateVerifier,
    scope: Scope,
    petal: Identifier32,
    epoch: u64,
    artifact_id: Hash32,
    signed_length: u64,
) -> Result<Vec<u8>, BatchHydration> {
    let store = &verifier.parts().ciphertexts;
    let Some(bytes) = ContentByteStore::get(store, artifact_id) else {
        return Err(unavailable(format!(
            "artifact {} is not held",
            op_id_to_hex(artifact_id)
        )));
    };
    if Hash32::of(&bytes) != artifact_id || bytes.len() as u64 != signed_length {
        return Err(unavailable(format!(
            "held bytes for {} are not the signed artifact",
            op_id_to_hex(artifact_id)
        )));
    }
    let keys = &verifier.parts().scope_keys;
    let Some(key) = keys.scope_key(&ScopeKeyId { scope, epoch }) else {
        return Err(unavailable("the batch's scope key is not held"));
    };
    let Ok(key_id) = derive_key_id(&scope, epoch, key) else {
        return Err(unavailable(
            "the batch's scope key does not derive a key id",
        ));
    };
    let topics = [PayloadTopicScope {
        verse_id: scope.verse_id(),
        petal_id: petal,
        scope_epoch: epoch,
        key_id,
    }];
    let lookup = store.sealed();
    let environment = SealedProofEnvironment::new(&lookup, keys, scope.verse_id(), epoch, &topics);
    match environment.open_bulk_artifact(artifact_id) {
        Ok(body) if body.object_class() == ObjectClass::Shard => Ok(body.content().to_vec()),
        Ok(_) => Err(invalid("the artifact's object class is not shard")),
        Err(
            SegmentError::ScopeKeyUnavailable
            | SegmentError::ArtifactUnavailable { .. }
            | SegmentError::ArtifactIdMismatch { .. },
        ) => Err(unavailable(
            "the artifact cannot be opened under a held key",
        )),
        Err(error) => Err(invalid(format!("sealed artifact: {error}"))),
    }
}

/// §5.2 step 4: totals, anchor set, both ranges, asset set, sources and format version.
fn check_against_payload(
    parts: &ObservationBatchPayloadParts,
    hexon: &ObservationHexon,
) -> Result<(), String> {
    let rows = (hexon.readings.len() + hexon.payload_refs.len()) as u64;
    if rows != parts.count {
        return Err(format!(
            "{rows} rows but the batch signs count {}",
            parts.count
        ));
    }
    if hexon.hexon_format_version != parts.hexon_format_version {
        return Err("hexon_format_version differs from the signed batch".to_string());
    }

    let mut anchors = BTreeSet::new();
    let mut hlc = Vec::with_capacity(rows as usize);
    let mut sensor = Vec::with_capacity(rows as usize);
    let mut used_sources = BTreeSet::new();
    let mut asset_refs = BTreeSet::new();
    // §3.4 rule 3: an observation appears at most once per batch. A batch repeating an id is
    // malformed as a whole, never a §5.3 rule 4 conflict between the batch and itself.
    let mut reading_ids = BTreeSet::new();
    let mut payload_ids = BTreeSet::new();
    for row in &hexon.readings {
        if !reading_ids.insert(row.reading_id.as_str()) {
            return Err(format!(
                "reading {} appears twice in one batch",
                row.reading_id
            ));
        }
        if !row.value.is_finite() {
            return Err(format!(
                "reading {} holds a non-finite value",
                row.reading_id
            ));
        }
        check_anchor(&row.anchor, &row.anchor_node_id)?;
        anchors.insert(row.anchor);
        hlc.push((row.hlc_wall_ms, row.hlc_counter));
        sensor.push(row.recorded_at_ms);
        used_sources.insert(row.source_index);
    }
    for row in &hexon.payload_refs {
        if !payload_ids.insert(row.payload_id.as_str()) {
            return Err(format!(
                "payload {} appears twice in one batch",
                row.payload_id
            ));
        }
        check_anchor(&row.anchor, &row.anchor_node_id)?;
        if row.asset_ids.is_empty() || row.asset_ids.len() != row.asset_content_hashes.len() {
            return Err(format!(
                "payload {} lists asset ids and hashes of different lengths",
                row.payload_id
            ));
        }
        anchors.insert(row.anchor);
        hlc.push((row.hlc_wall_ms, row.hlc_counter));
        sensor.push(row.recorded_at_ms);
        used_sources.insert(row.source_index);
        asset_refs.extend(row.asset_content_hashes.iter().copied());
    }

    let signed_anchors: BTreeSet<[u8; 32]> = parts.anchors.iter().map(|anchor| anchor.0).collect();
    if anchors != signed_anchors {
        return Err("the hexon's anchor set differs from the signed anchors".to_string());
    }
    let signed_hlc = |wall: u64, counter: u32| (wall as i64, counter as i32);
    let hlc_min = hlc.iter().min().copied();
    let hlc_max = hlc.iter().max().copied();
    if hlc_min
        != Some(signed_hlc(
            parts.hlc_range.min.wall_ms,
            parts.hlc_range.min.counter,
        ))
        || hlc_max
            != Some(signed_hlc(
                parts.hlc_range.max.wall_ms,
                parts.hlc_range.max.counter,
            ))
    {
        return Err("the hexon's HLC extrema differ from the signed hlc_range".to_string());
    }
    if sensor.iter().min().copied() != Some(parts.sensor_time_range.min_ms)
        || sensor.iter().max().copied() != Some(parts.sensor_time_range.max_ms)
    {
        return Err("the hexon's sensor-time extrema differ from the signed range".to_string());
    }
    let signed_assets: BTreeSet<[u8; 32]> = parts.asset_refs.iter().map(|hash| hash.0).collect();
    if asset_refs != signed_assets {
        return Err("the hexon's asset hashes differ from the signed asset_refs".to_string());
    }
    if used_sources
        .iter()
        .any(|index| *index as usize >= parts.sources.len())
    {
        return Err("a source_index is out of range of the signed sources".to_string());
    }
    if used_sources.len() != parts.sources.len() {
        return Err("a signed source is used by no row".to_string());
    }
    Ok(())
}

fn check_anchor(anchor: &[u8; 32], node_id: &str) -> Result<(), String> {
    match node_anchor(node_id) {
        Ok(expected) if expected.0 == *anchor => Ok(()),
        _ => Err(format!(
            "anchor of node {node_id} is not its §4.5 identifier"
        )),
    }
}

fn pack_hlc(wall_ms: i64, counter: i32) -> Result<u64, String> {
    let wall = u64::try_from(wall_ms).map_err(|_| "negative HLC wall time".to_string())?;
    let counter = u16::try_from(counter).map_err(|_| "HLC counter out of range".to_string())?;
    if wall >= 1 << 48 {
        return Err("HLC wall time out of range".to_string());
    }
    Ok((wall << 16) | u64::from(counter))
}

/// The pinned writer must reproduce the held hexon and statistics bytes from the hexon's rows.
fn check_rederivation(
    author_did: &str,
    parts: &ObservationBatchPayloadParts,
    hexon: &ObservationHexon,
    hexon_bytes: &[u8],
    stats_bytes: &[u8],
) -> Result<(), String> {
    let readings = hexon
        .readings
        .iter()
        .map(|row| {
            Ok(ReadingRow {
                reading_id: row.reading_id.clone(),
                node_id: row.anchor_node_id.clone(),
                metric: row.metric.clone(),
                value: row.value,
                units: row.units.clone(),
                recorded_at: row.recorded_at.clone(),
                recorded_at_ms: row.recorded_at_ms,
                hlc_timestamp: pack_hlc(row.hlc_wall_ms, row.hlc_counter)?,
                payload_id: row.payload_id.clone(),
                source_index: row.source_index,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let payloads = hexon
        .payload_refs
        .iter()
        .map(|row| {
            Ok(PayloadRefRow {
                payload_id: row.payload_id.clone(),
                node_id: row.anchor_node_id.clone(),
                format_id: row.format_id.clone(),
                recorded_at: row.recorded_at.clone(),
                recorded_at_ms: row.recorded_at_ms,
                hlc_timestamp: pack_hlc(row.hlc_wall_ms, row.hlc_counter)?,
                asset_ids: row.asset_ids.clone(),
                asset_content_hashes: row.asset_content_hashes.clone(),
                source_index: row.source_index,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let rederived = write_observation_hexon(author_did, &readings, &payloads, parts.sources.len())
        .map_err(|error| format!("rows do not re-derive: {error}"))?;
    if rederived.hexon_bytes != hexon_bytes {
        return Err("the hexon is not the pinned writer's output for its own rows".to_string());
    }
    if rederived.stats_bytes != stats_bytes {
        return Err("the statistics artifact differs from the hexon's aggregates".to_string());
    }
    Ok(())
}

async fn write_catalog(
    db: &Db,
    readings: &[CatalogReading],
    payloads: &[CatalogPayload],
    conflicts: &[(ObservationTable, String, Vec<Hash32>)],
    outcomes: &BTreeMap<Hash32, HydrationOutcome>,
) -> Result<(), StorageError> {
    let conflict_rows: Vec<serde_json::Value> = conflicts
        .iter()
        .map(|(table, row_id, op_ids)| {
            serde_json::json!({
                "conflict_key": format!("{}:{row_id}", table.as_str()),
                "observation_table": table.as_str(),
                "row_id": row_id,
                "op_ids": op_ids.iter().map(|op| op_id_to_hex(*op)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let outcome_rows: Vec<serde_json::Value> = outcomes
        .iter()
        .map(|(op_id, outcome)| {
            serde_json::json!({
                "op_id": op_id_to_hex(*op_id),
                "outcome": outcome.label(),
                "detail": outcome.detail(),
            })
        })
        .collect();

    let mut script = String::from(
        "BEGIN TRANSACTION;\n\
         DELETE observation_catalog_reading;\n\
         DELETE observation_catalog_payload;\n\
         DELETE observation_catalog_conflict;\n\
         DELETE observation_hydration;\n",
    );
    for (table, binding, empty) in [
        (
            "observation_catalog_reading",
            "readings",
            readings.is_empty(),
        ),
        (
            "observation_catalog_payload",
            "payloads",
            payloads.is_empty(),
        ),
        (
            "observation_catalog_conflict",
            "conflicts",
            conflict_rows.is_empty(),
        ),
        ("observation_hydration", "outcomes", outcome_rows.is_empty()),
    ] {
        if !empty {
            script.push_str(&format!("INSERT INTO {table} ${binding};\n"));
        }
    }
    script.push_str("COMMIT TRANSACTION;");
    db.query(script)
        .bind((
            "readings",
            serde_json::to_value(readings).unwrap_or_default(),
        ))
        .bind((
            "payloads",
            serde_json::to_value(payloads).unwrap_or_default(),
        ))
        .bind(("conflicts", conflict_rows))
        .bind(("outcomes", outcome_rows))
        .await
        .map_err(|error| StorageError::query("observation catalog write", error))?
        .check()
        .map_err(|error| StorageError::query("observation catalog write", error))?;
    Ok(())
}

fn catalog_root(
    readings: &[CatalogReading],
    payloads: &[CatalogPayload],
    conflicts: &[(ObservationTable, String, Vec<Hash32>)],
    outcomes: &BTreeMap<Hash32, HydrationOutcome>,
) -> Result<Hash32, serde_json::Error> {
    let conflicts: Vec<(&str, &str, Vec<String>)> = conflicts
        .iter()
        .map(|(table, row_id, op_ids)| {
            (
                table.as_str(),
                row_id.as_str(),
                op_ids.iter().map(|op| op_id_to_hex(*op)).collect(),
            )
        })
        .collect();
    let outcomes: Vec<(String, &str, &str)> = outcomes
        .iter()
        .map(|(op_id, outcome)| (op_id_to_hex(*op_id), outcome.label(), outcome.detail()))
        .collect();
    let bytes = serde_json::to_vec(&(readings, payloads, conflicts, outcomes))?;
    Ok(Hash32::of(&bytes))
}

/// The SPEC-4 §7 rule 1 tuple an analytics read is computed against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalyticsReadBase {
    /// Branch of the source projection.
    pub branch_id: Identifier32,
    /// D-CL19 commitment of the frontier read at.
    pub frontier_commitment: Hash32,
    /// Segment manifest backing that frontier.
    pub segment_manifest_id: Hash32,
    /// Materializer identity of the source projection.
    pub materializer_version: MaterializerVersion,
}

/// The rows an analytics computation read and the identity that makes it reproducible (G6).
#[derive(Clone, Debug, PartialEq)]
pub struct ObservationAnalyticsRead {
    /// SPEC-4 §7 rule 5: the tuple plus the sorted, deduplicated batches read.
    pub identity: AnalyticsSourceIdentity,
    /// Catalog readings carried by any read batch, by `reading_id`.
    pub readings: Vec<CatalogReading>,
    /// Catalog payload references carried by any read batch, by `payload_id`.
    pub payloads: Vec<CatalogPayload>,
}

/// Why an analytics read did not produce a result.
#[derive(Debug, thiserror::Error)]
pub enum AnalyticsReadError {
    /// A listed batch is not hydrated here; the computation fails rather than shrink its input
    /// set (SPEC-10 §10 test 19).
    #[error("observation batch {op_id:?} is not hydrated (outcome {outcome:?})")]
    BatchNotHydrated {
        /// The batch.
        op_id: Hash32,
        /// Its recorded outcome, if any.
        outcome: Option<String>,
    },
    /// The substrate failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Reads the catalog rows of exactly `op_ids` and records them as the observation input set.
pub async fn read_observation_batches(
    db: &Db,
    base: AnalyticsReadBase,
    op_ids: impl IntoIterator<Item = Hash32>,
) -> Result<ObservationAnalyticsRead, AnalyticsReadError> {
    let requested: BTreeSet<Hash32> = op_ids.into_iter().collect();
    let requested_hex: Vec<String> = requested.iter().map(|op| op_id_to_hex(*op)).collect();

    #[derive(Deserialize)]
    struct OutcomeRow {
        op_id: String,
        outcome: String,
    }
    let outcome_rows: Vec<serde_json::Value> = select(
        db,
        "SELECT op_id, outcome FROM observation_hydration WHERE op_id IN $ops",
        &requested_hex,
    )
    .await?;
    let mut outcomes = BTreeMap::new();
    for row in outcome_rows {
        let row: OutcomeRow = serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("observation_hydration", error))?;
        outcomes.insert(row.op_id, row.outcome);
    }
    for (op_id, op_hex) in requested.iter().zip(&requested_hex) {
        let outcome = outcomes.get(op_hex);
        if outcome.map(String::as_str) != Some("hydrated") {
            return Err(AnalyticsReadError::BatchNotHydrated {
                op_id: *op_id,
                outcome: outcome.cloned(),
            });
        }
    }

    let readings = select(
        db,
        "SELECT * OMIT id FROM observation_catalog_reading WHERE op_ids CONTAINSANY $ops \
         ORDER BY reading_id ASC",
        &requested_hex,
    )
    .await?
    .into_iter()
    .map(|row| {
        serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("observation_catalog_reading", error))
    })
    .collect::<Result<Vec<CatalogReading>, _>>()?;
    let payloads = select(
        db,
        "SELECT * OMIT id FROM observation_catalog_payload WHERE op_ids CONTAINSANY $ops \
         ORDER BY payload_id ASC",
        &requested_hex,
    )
    .await?
    .into_iter()
    .map(|row| {
        serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("observation_catalog_payload", error))
    })
    .collect::<Result<Vec<CatalogPayload>, _>>()?;

    Ok(ObservationAnalyticsRead {
        identity: AnalyticsSourceIdentity::new(
            base.branch_id,
            base.frontier_commitment,
            base.segment_manifest_id,
            base.materializer_version,
            requested,
        ),
        readings,
        payloads,
    })
}

async fn select(
    db: &Db,
    statement: &str,
    op_ids: &[String],
) -> Result<Vec<serde_json::Value>, StorageError> {
    db.query(statement)
        .bind(("ops", op_ids.to_vec()))
        .await
        .map_err(|error| StorageError::query("observation catalog read", error))?
        .check()
        .map_err(|error| StorageError::query("observation catalog read", error))?
        .take(0)
        .map_err(|error| StorageError::query("observation catalog read", error))
}

/// Every catalog reading, by `reading_id` (for the parity comparison).
pub(crate) async fn catalog_readings(db: &Db) -> Result<Vec<CatalogReading>, StorageError> {
    select(
        db,
        "SELECT * OMIT id FROM observation_catalog_reading ORDER BY reading_id ASC",
        &[],
    )
    .await?
    .into_iter()
    .map(|row| {
        serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("observation_catalog_reading", error))
    })
    .collect()
}

/// Every catalog payload reference, by `payload_id` (for the parity comparison).
pub(crate) async fn catalog_payloads(db: &Db) -> Result<Vec<CatalogPayload>, StorageError> {
    select(
        db,
        "SELECT * OMIT id FROM observation_catalog_payload ORDER BY payload_id ASC",
        &[],
    )
    .await?
    .into_iter()
    .map(|row| {
        serde_json::from_value(row)
            .map_err(|error| StorageError::malformed("observation_catalog_payload", error))
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_canonical_log::envelope::Hlc;
    use fe_canonical_log::materialize::identity::{ObservationSource, SourceClass};
    use fe_canonical_log::observation::{HlcRange, SensorTimeRange};
    use fe_query::columnar::observation::ReadingOut;

    /// SPEC-10 §3.4 rule 3 at hydration: a batch that carries one `reading_id` twice (under two
    /// HLCs, so the per-anchor order still holds) is `observation_batch_invalid` as a whole;
    /// the same rows with distinct ids agree with the signed payload.
    #[test]
    fn a_batch_repeating_an_observation_id_disagrees_with_its_payload() {
        let node = "01HNODE0000000000000000000";
        let anchor = node_anchor(node).expect("anchor").0;
        let row = |reading_id: &str, wall_ms: i64| ReadingOut {
            reading_id: reading_id.to_string(),
            anchor,
            anchor_node_id: node.to_string(),
            metric: "t".to_string(),
            value: 1.0,
            units: String::new(),
            recorded_at: String::new(),
            recorded_at_ms: 5,
            hlc_wall_ms: wall_ms,
            hlc_counter: 0,
            payload_id: None,
            source_index: 0,
        };
        let source = ObservationSource::new(
            SourceClass::Device,
            ObservationSource::source_id_for(SourceClass::Device, "device"),
            [7; 32],
            None,
            None,
        )
        .expect("source");
        let parts = ObservationBatchPayloadParts {
            hexon_artifact_id: Hash32([1; 32]),
            hexon_stored_length: 1,
            hlc_range: HlcRange {
                min: Hlc::new(10, 0),
                max: Hlc::new(20, 0),
            },
            sensor_time_range: SensorTimeRange {
                min_ms: 5,
                max_ms: 5,
            },
            stats_hash: Hash32([2; 32]),
            count: 2,
            anchors: vec![Identifier32(anchor)],
            asset_refs: Vec::new(),
            sources: vec![source],
            hexon_format_version: 1,
            stats_stored_length: 1,
        };
        let hexon = |second: &str| ObservationHexon {
            readings: vec![row("r1", 10), row(second, 20)],
            payload_refs: Vec::new(),
            hexon_format_version: 1,
        };
        assert_eq!(check_against_payload(&parts, &hexon("r2")), Ok(()));
        let refused = check_against_payload(&parts, &hexon("r1")).expect_err("repeated id");
        assert!(refused.contains("appears twice"), "{refused}");
    }

    fn reading(op: &str, value: f64) -> CatalogReading {
        CatalogReading {
            reading_id: "r1".to_string(),
            op_ids: vec![op.to_string()],
            verse_id: String::new(),
            petal_id: String::new(),
            anchor: String::new(),
            anchor_node_id: "n1".to_string(),
            metric: "t".to_string(),
            value,
            value_bits: format!("{:016x}", value.to_bits()),
            units: String::new(),
            recorded_at: String::new(),
            recorded_at_ms: 0,
            hlc_wall_ms: 1,
            hlc_counter: 0,
            payload_id: None,
            source: String::new(),
        }
    }

    /// SPEC-10 §5.3 rules 3-4: identical copies collapse naming both batches; differing copies
    /// contribute nothing and become evidence naming both, whatever order they arrive in.
    #[test]
    fn duplicates_collapse_and_conflicts_name_every_batch() {
        let a = op_id_to_hex(Hash32([1; 32]));
        let b = op_id_to_hex(Hash32([2; 32]));
        let mut conflicts = Vec::new();
        let same = collapse(
            BTreeMap::from([("r1".to_string(), vec![reading(&b, 1.0), reading(&a, 1.0)])]),
            ObservationTable::Reading,
            &mut conflicts,
            |row| &mut row.op_ids,
        );
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].op_ids, vec![a.clone(), b.clone()]);
        assert!(conflicts.is_empty());

        let differ = collapse(
            BTreeMap::from([("r1".to_string(), vec![reading(&a, 1.0), reading(&b, -1.0)])]),
            ObservationTable::Reading,
            &mut conflicts,
            |row| &mut row.op_ids,
        );
        assert!(differ.is_empty());
        assert_eq!(
            conflicts,
            vec![(
                ObservationTable::Reading,
                "r1".to_string(),
                vec![Hash32([1; 32]), Hash32([2; 32])]
            )]
        );
    }
}
