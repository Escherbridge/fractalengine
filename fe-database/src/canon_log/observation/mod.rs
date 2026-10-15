//! SPEC-10 observation plane, storage half (CDL-SPEC10 B3): the door's mode gate and window
//! grain, the shadow sealer, the kind-5 reference projection, the hydrator, and the parity
//! comparison. PARALLEL AND DORMANT; see `canon_log/AGENTS.md` §observation.

pub mod hydrator;
pub mod parity;
pub mod sealer;

pub use fe_query::columnar::observation::{PayloadRefRow, ReadingRow};
pub use hydrator::{
    hydrate_observation_catalog, read_observation_batches, AnalyticsReadBase, AnalyticsReadError,
    CatalogPayload, CatalogReading, HydrationOutcome, HydrationReport, HydrationRequest,
    ObservationAnalyticsRead,
};
pub use parity::{compare_catalog_with_legacy, ObservationParityReport};
pub use sealer::{
    branch_heads, seal_shard, sealing_context, sign_observation_batch, ObservationSealer,
    ObservationSealerConfig, SealPolicy, SealReport, SealedBatch, SealedShard, SealingContext,
    SignedObservationBatch, UncoveredObservation,
};

use std::collections::HashMap;

use async_trait::async_trait;
use fe_canonical_log::did_key::did_to_public_key;
use fe_canonical_log::envelope::{Hash32, Identifier32};
use fe_canonical_log::frontier::SortedFrontier;
use fe_canonical_log::materialize::identity::{ObservationSource, ProjectionIdentity, SourceClass};
use fe_canonical_log::observation::{node_anchor, ObservationBatchMaterializer};
use fe_sdk::legacy_identifier::{legacy_identifier, LegacyTable};
use serde::Deserialize;

use crate::canon_log::append_store::{SurrealVerifiedLogStore, SurrealVerseDagView};
use crate::canon_log::apply_marker_store::SurrealApplyMarkerStore;
use crate::canon_log::rebuild::{
    rebuild_projection_with_facts, ProjectionSurface, RebuildError, RebuildReport, RebuildRequest,
};
use crate::canon_log::replay::{replay_to_frontier_with_facts, ReplayOutcome, VerifierReplayFacts};
use crate::canon_log::verifier::DurableCandidateVerifier;
use crate::canon_log::StorageError;
use crate::handlers::device_payload::OBSERVATION_WINDOW_BUCKET_MS;
use crate::migration::mode::{MigrationFlags, MigrationMode};
use crate::repo::Db;

/// Every reason an observation-plane step did not finish. None of them is a decision about an
/// observation: a refused seal leaves its members pending, a refused hydration call writes no
/// catalog row.
#[derive(Debug, thiserror::Error)]
pub enum ObservationPlaneError {
    /// The SPEC-8 mode does not enable this step (SPEC-10 §8 table).
    #[error("migration mode {mode} does not enable this observation-plane step")]
    ModeDisabled {
        /// The mode in force.
        mode: MigrationMode,
    },
    /// The substrate failed.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// The sealing capability chain is not in the local chain store.
    #[error("capability chain {0:?} is not held locally")]
    ChainUnavailable(Hash32),
    /// The chain bytes do not decode.
    #[error("capability chain does not decode: {0}")]
    ChainMalformed(String),
    /// The chain's epoch scope has no durable epoch state.
    #[error("the chain's epoch scope has no durable epoch state")]
    EpochUnknown,
    /// The chain certifies an epoch that is no longer current; reissue it.
    #[error("chain certifies epoch {chain_epoch} but the current epoch is {current_epoch}")]
    ChainEpochStale {
        /// The leaf's epoch.
        chain_epoch: u64,
        /// The durable current epoch.
        current_epoch: u64,
    },
    /// The scope epoch moved while a batch was being sealed (SPEC-10 §4.2 rule 4) more often
    /// than the retry budget allows.
    #[error("the scope epoch kept moving while the batch was sealed")]
    EpochMovedDuringSeal,
    /// No held scope key for the batch's payload topic.
    #[error("no held scope key for the petal payload topic at epoch {epoch}")]
    ScopeKeyUnavailable {
        /// The epoch asked for.
        epoch: u64,
    },
    /// The chain does not authorize a step (SPEC-10 §4.2 rule 2 self-check).
    #[error("capability refused: {0}")]
    Unauthorized(String),
    /// A legacy id could not be mapped (SPEC-10 §4.5).
    #[error("legacy identifier: {0}")]
    Identifier(String),
    /// The hexon writer refused its rows.
    #[error("observation hexon: {0}")]
    Hexon(String),
    /// Sealing, AEAD, or envelope signing failed.
    #[error("sealing: {0}")]
    Seal(String),
    /// The kind-5 payload refused its parts.
    #[error("observation_batch payload: {0}")]
    Payload(String),
    /// The branch has no operation for a kind-5 batch to descend from (SPEC-10 §4.1 rule 2.1).
    #[error("branch has no head for the batch to descend from")]
    NoBranchHeads,
    /// Another sealer claimed some of the members first; nothing was appended.
    #[error("window members were claimed concurrently; nothing was appended")]
    ConcurrentClaim,
    /// Admission or the durable append refused the batch; its members were released.
    #[error("admission refused the batch: {0}")]
    Admission(String),
    /// The reference replay could not run.
    #[error("reference replay: {0}")]
    Replay(String),
}

/// Whether `mode` seals windows and materializes reference rows (`dual_emit_shadow`,
/// `shadow_rebuild`). The door records membership in exactly these modes too.
pub const fn seals_observations(mode: MigrationMode) -> bool {
    matches!(
        mode,
        MigrationMode::DualEmitShadow | MigrationMode::ShadowRebuild
    )
}

/// Whether `mode` hydrates and compares (`shadow_rebuild` only, SPEC-10 §8 table).
pub const fn hydrates_observations(mode: MigrationMode) -> bool {
    matches!(mode, MigrationMode::ShadowRebuild)
}

/// The door's gate (SPEC-10 §8 rule 2), read from the local process flag on every call: in
/// `legacy_only` and `seam_ready` nothing is recorded. An unreadable flag records nothing, the
/// direction in which this node signs nothing.
pub fn door_records_window_membership() -> bool {
    match MigrationFlags::from_env() {
        Ok(flags) => seals_observations(flags.mode()),
        Err(error) => {
            tracing::warn!("observation window membership not recorded: {error}");
            false
        }
    }
}

/// The ingestion-HLC window a door accept at `hlc_wall_ms` belongs to (SPEC-10 §3.4 rule 2).
pub const fn window_bucket_of(hlc_wall_ms: i64) -> i64 {
    hlc_wall_ms.div_euclid(OBSERVATION_WINDOW_BUCKET_MS) * OBSERVATION_WINDOW_BUCKET_MS
}

/// Refuses `mode` unless `enabled(mode)`.
pub(crate) fn require_mode(
    mode: MigrationMode,
    enabled: fn(MigrationMode) -> bool,
) -> Result<(), ObservationPlaneError> {
    if enabled(mode) {
        Ok(())
    } else {
        Err(ObservationPlaneError::ModeDisabled { mode })
    }
}

/// The SPEC-10 §4.5 identifier of a legacy verse/petal/node id.
pub fn legacy_id(table: LegacyTable, id: &str) -> Result<Identifier32, ObservationPlaneError> {
    legacy_identifier(table, id)
        .map(Identifier32)
        .map_err(|error| ObservationPlaneError::Identifier(error.to_string()))
}

/// Which hot table an observation lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObservationTable {
    /// `iot_reading`, keyed by `reading_id`.
    Reading,
    /// `device_payload`, keyed by `payload_id`.
    Payload,
}

impl ObservationTable {
    /// The hot table name, as `observation_window_member.table` stores it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reading => "iot_reading",
            Self::Payload => "device_payload",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "iot_reading" => Some(Self::Reading),
            "device_payload" => Some(Self::Payload),
            _ => None,
        }
    }
}

/// One `observation_window_member` row: an observation this node's own door accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WindowMember {
    pub(crate) table: ObservationTable,
    pub(crate) row_id: String,
    pub(crate) petal_id: String,
    pub(crate) source_class: String,
    pub(crate) window_bucket: i64,
    pub(crate) sealed_op_id: Option<String>,
    pub(crate) uncovered: Option<String>,
}

#[derive(Deserialize)]
struct WindowMemberRow {
    #[serde(rename = "table")]
    observation_table: String,
    row_id: String,
    petal_id: String,
    source_class: String,
    window_bucket: i64,
    #[serde(default)]
    sealed_op_id: Option<String>,
    #[serde(default)]
    uncovered: Option<String>,
}

const MEMBER_FIELDS: &str =
    "`table`, row_id, petal_id, source_class, window_bucket, sealed_op_id, uncovered";

/// Reads members matching `condition` (a SurrealQL boolean over the member row).
pub(crate) async fn load_members(
    db: &Db,
    condition: &str,
    bindings: Vec<(&'static str, serde_json::Value)>,
) -> Result<Vec<WindowMember>, StorageError> {
    let mut query = db.query(format!(
        "SELECT {MEMBER_FIELDS} FROM observation_window_member WHERE {condition}"
    ));
    for (name, value) in bindings {
        query = query.bind((name, value));
    }
    let rows: Vec<serde_json::Value> = query
        .await
        .map_err(|error| StorageError::query("window member read", error))?
        .check()
        .map_err(|error| StorageError::query("window member read", error))?
        .take(0)
        .map_err(|error| StorageError::query("window member read", error))?;
    rows.into_iter()
        .map(|row| {
            let parsed: WindowMemberRow = serde_json::from_value(row)
                .map_err(|error| StorageError::malformed("window member", error))?;
            let table = ObservationTable::parse(&parsed.observation_table).ok_or_else(|| {
                StorageError::malformed("window member", "unknown observation table")
            })?;
            Ok(WindowMember {
                table,
                row_id: parsed.row_id,
                petal_id: parsed.petal_id,
                source_class: parsed.source_class,
                window_bucket: parsed.window_bucket,
                sealed_op_id: parsed.sealed_op_id,
                uncovered: parsed.uncovered,
            })
        })
        .collect()
}

/// An `iot_reading` row as the door wrote it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct HotReading {
    pub(crate) reading_id: String,
    pub(crate) node_id: String,
    pub(crate) petal_id: String,
    pub(crate) metric: String,
    pub(crate) value: f64,
    #[serde(default)]
    pub(crate) units: String,
    pub(crate) recorded_at: String,
    pub(crate) recorded_at_ms: i64,
    pub(crate) hlc_timestamp: i64,
    #[serde(default)]
    pub(crate) payload_id: Option<String>,
    #[serde(default)]
    pub(crate) source_did: String,
}

/// A `device_payload` row as the door wrote it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct HotPayload {
    pub(crate) payload_id: String,
    pub(crate) node_id: String,
    pub(crate) petal_id: String,
    pub(crate) format_id: String,
    pub(crate) recorded_at: String,
    pub(crate) recorded_at_ms: i64,
    pub(crate) hlc_timestamp: i64,
    #[serde(default)]
    pub(crate) asset_ids: Vec<String>,
    #[serde(default)]
    pub(crate) source_did: String,
}

/// A member resolved into everything the hexon and the comparison need.
#[derive(Clone, Debug)]
pub(crate) enum ResolvedObservation {
    Reading {
        hot: HotReading,
        source: ObservationSource,
    },
    Payload {
        hot: HotPayload,
        asset_content_hashes: Vec<[u8; 32]>,
        source: ObservationSource,
    },
}

impl ResolvedObservation {
    pub(crate) fn node_id(&self) -> &str {
        match self {
            Self::Reading { hot, .. } => &hot.node_id,
            Self::Payload { hot, .. } => &hot.node_id,
        }
    }

    pub(crate) fn hlc_and_id(&self) -> (i64, &str) {
        match self {
            Self::Reading { hot, .. } => (hot.hlc_timestamp, &hot.reading_id),
            Self::Payload { hot, .. } => (hot.hlc_timestamp, &hot.payload_id),
        }
    }

    pub(crate) fn source(&self) -> &ObservationSource {
        match self {
            Self::Reading { source, .. } | Self::Payload { source, .. } => source,
        }
    }
}

/// The SPEC-10 §7.1 source of one door accept: the class the caller declared at the door, never
/// inferred (rule 6), and the door's recorded `source_did` as both the ingest principal (rule 5)
/// and the §7.2 label. A `decoder` class has no `DecoderIdentity` at the door, so it is refused.
pub fn observation_source(
    source_class: SourceClass,
    source_did: &str,
    format_id: Option<&str>,
) -> Result<ObservationSource, String> {
    if source_class == SourceClass::Decoder {
        return Err("a decoder observation carries no DecoderIdentity at the door".to_string());
    }
    let ingest_principal = did_to_public_key(source_did)
        .map_err(|error| format!("source_did is not an Ed25519 did:key: {error}"))?;
    ObservationSource::new(
        source_class,
        ObservationSource::source_id_for(source_class, source_did),
        ingest_principal,
        format_id.map(str::to_string),
        None,
    )
    .map_err(|error| error.to_string())
}

fn parse_source_class(text: &str) -> Option<SourceClass> {
    serde_json::from_value(serde_json::Value::String(text.to_string())).ok()
}

/// Resolves members against the hot rows the door wrote under the recorded identifiers (SPEC-10
/// §3.4 rule 6: a read-back by id, never a scan). Each member gets its observation or the reason
/// it can never be sealed (§8 rule 3).
pub(crate) async fn resolve_members(
    db: &Db,
    members: &[WindowMember],
) -> Result<Vec<(WindowMember, Result<ResolvedObservation, String>)>, StorageError> {
    let reading_ids: Vec<String> = ids_of(members, ObservationTable::Reading);
    let payload_ids: Vec<String> = ids_of(members, ObservationTable::Payload);
    let readings: HashMap<String, HotReading> = select_by_ids(
        db,
        "SELECT reading_id, node_id, petal_id, metric, value, units, recorded_at, recorded_at_ms, \
         hlc_timestamp, payload_id, source_did FROM iot_reading WHERE reading_id IN $ids",
        &reading_ids,
    )
    .await?
    .into_iter()
    .map(|row: HotReading| (row.reading_id.clone(), row))
    .collect();
    let payloads: HashMap<String, HotPayload> = select_by_ids(
        db,
        "SELECT payload_id, node_id, petal_id, format_id, recorded_at, recorded_at_ms, \
         hlc_timestamp, asset_ids, source_did FROM device_payload WHERE payload_id IN $ids",
        &payload_ids,
    )
    .await?
    .into_iter()
    .map(|row: HotPayload| (row.payload_id.clone(), row))
    .collect();
    let asset_ids: Vec<String> = payloads
        .values()
        .flat_map(|payload| payload.asset_ids.iter().cloned())
        .collect();
    let asset_hashes = asset_content_hashes(db, &asset_ids).await?;

    Ok(members
        .iter()
        .map(|member| {
            let resolved = resolve_one(member, &readings, &payloads, &asset_hashes);
            (member.clone(), resolved)
        })
        .collect())
}

fn ids_of(members: &[WindowMember], table: ObservationTable) -> Vec<String> {
    let mut ids: Vec<String> = members
        .iter()
        .filter(|member| member.table == table)
        .map(|member| member.row_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

async fn select_by_ids<T: for<'de> Deserialize<'de>>(
    db: &Db,
    statement: &str,
    ids: &[String],
) -> Result<Vec<T>, StorageError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<serde_json::Value> = db
        .query(statement)
        .bind(("ids", ids.to_vec()))
        .await
        .map_err(|error| StorageError::query("observation hot-row read", error))?
        .check()
        .map_err(|error| StorageError::query("observation hot-row read", error))?
        .take(0)
        .map_err(|error| StorageError::query("observation hot-row read", error))?;
    rows.into_iter()
        .map(|row| {
            serde_json::from_value(row)
                .map_err(|error| StorageError::malformed("observation hot row", error))
        })
        .collect()
}

#[derive(Deserialize)]
struct AssetHashRow {
    asset_id: String,
    content_hash: String,
}

async fn asset_content_hashes(
    db: &Db,
    asset_ids: &[String],
) -> Result<HashMap<String, [u8; 32]>, StorageError> {
    let mut ids = asset_ids.to_vec();
    ids.sort();
    ids.dedup();
    let rows: Vec<AssetHashRow> = select_by_ids(
        db,
        "SELECT asset_id, content_hash FROM asset WHERE asset_id IN $ids",
        &ids,
    )
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let bytes: [u8; 32] = hex::decode(&row.content_hash).ok()?.try_into().ok()?;
            Some((row.asset_id, bytes))
        })
        .collect())
}

fn resolve_one(
    member: &WindowMember,
    readings: &HashMap<String, HotReading>,
    payloads: &HashMap<String, HotPayload>,
    asset_hashes: &HashMap<String, [u8; 32]>,
) -> Result<ResolvedObservation, String> {
    let source_class = parse_source_class(&member.source_class)
        .ok_or_else(|| format!("unknown source class {:?}", member.source_class))?;
    match member.table {
        ObservationTable::Reading => {
            let hot = readings
                .get(&member.row_id)
                .ok_or("the accepted iot_reading row is gone")?;
            check_common(member, &hot.petal_id, &hot.node_id, hot.hlc_timestamp)?;
            if !hot.value.is_finite() {
                return Err("non-finite value".to_string());
            }
            let source = observation_source(source_class, &hot.source_did, None)?;
            Ok(ResolvedObservation::Reading {
                hot: hot.clone(),
                source,
            })
        }
        ObservationTable::Payload => {
            let hot = payloads
                .get(&member.row_id)
                .ok_or("the accepted device_payload row is gone")?;
            check_common(member, &hot.petal_id, &hot.node_id, hot.hlc_timestamp)?;
            if hot.asset_ids.is_empty() {
                return Err("device payload lists no asset".to_string());
            }
            let asset_content_hashes = hot
                .asset_ids
                .iter()
                .map(|asset_id| {
                    asset_hashes
                        .get(asset_id)
                        .copied()
                        .ok_or_else(|| format!("asset {asset_id} has no content hash"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let source = observation_source(source_class, &hot.source_did, Some(&hot.format_id))?;
            Ok(ResolvedObservation::Payload {
                hot: hot.clone(),
                asset_content_hashes,
                source,
            })
        }
    }
}

fn check_common(
    member: &WindowMember,
    hot_petal: &str,
    node_id: &str,
    hlc_timestamp: i64,
) -> Result<(), String> {
    if hot_petal != member.petal_id {
        return Err("the hot row's petal differs from the door's record".to_string());
    }
    if hlc_timestamp <= 0 || (hlc_timestamp >> 16) == 0 {
        return Err("missing ingestion HLC".to_string());
    }
    node_anchor(node_id).map_err(|error| format!("anchor: {error}"))?;
    Ok(())
}

/// The reference projection's identity on `branch_id`.
pub fn reference_identity(branch_id: Identifier32) -> ProjectionIdentity {
    ProjectionIdentity::new(ObservationBatchMaterializer::version(), branch_id)
}

/// Every `observation_batch` reference row (storage id omitted), by `op_id`.
pub async fn observation_reference_rows(db: &Db) -> Result<Vec<serde_json::Value>, StorageError> {
    db.query("SELECT * OMIT id FROM observation_batch ORDER BY op_id ASC")
        .await
        .map_err(|error| StorageError::query("observation_batch read", error))?
        .check()
        .map_err(|error| StorageError::query("observation_batch read", error))?
        .take(0)
        .map_err(|error| StorageError::query("observation_batch read", error))
}

/// The reference projection's rows, for the SPEC-4 §6 rebuild seam. One reference projection
/// per store: a reset drops every `observation_batch` row.
pub struct ObservationReferenceSurface {
    db: Db,
}

impl ObservationReferenceSurface {
    /// The surface over `db`.
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ProjectionSurface for ObservationReferenceSurface {
    async fn reset_to_empty(&self, _identity: &ProjectionIdentity) -> anyhow::Result<()> {
        self.db
            .query("DELETE FROM observation_batch")
            .await?
            .check()?;
        Ok(())
    }

    async fn projection_root_hash(&self, _identity: &ProjectionIdentity) -> anyhow::Result<Hash32> {
        let rows = observation_reference_rows(&self.db).await?;
        Ok(Hash32::of(&serde_json::to_vec(&rows)?))
    }
}

/// Replays the reference projection to `frontier`, re-deriving each kind-5 operation's own
/// payload facts through `verifier` (B2b): this is how a sealed batch's reference row
/// materializes (SPEC-10 §4.4).
pub async fn materialize_observation_references(
    mode: MigrationMode,
    db: &Db,
    verifier: &DurableCandidateVerifier,
    branch_id: Identifier32,
    frontier: &SortedFrontier,
) -> Result<ReplayOutcome, ObservationPlaneError> {
    require_mode(mode, seals_observations)?;
    replay_to_frontier_with_facts(
        &SurrealVerifiedLogStore::new(db.clone()),
        &SurrealApplyMarkerStore::new(db.clone()),
        &ObservationBatchMaterializer,
        &reference_identity(branch_id),
        frontier,
        &VerifierReplayFacts::new(verifier),
    )
    .await
    .map_err(|error| ObservationPlaneError::Replay(format!("{error:?}")))
}

/// Rebuilds the reference projection from the verified log through B2b's facts source (SPEC-10
/// §10 test 16). A pre-shred row whose facts cannot be re-derived withholds the rebuild.
pub async fn rebuild_observation_references(
    mode: MigrationMode,
    db: &Db,
    verifier: &DurableCandidateVerifier,
    verse_id: Identifier32,
    branch_id: Identifier32,
    frontier: &SortedFrontier,
) -> Result<Result<RebuildReport, RebuildError>, ObservationPlaneError> {
    require_mode(mode, seals_observations)?;
    let store = SurrealVerifiedLogStore::new(db.clone());
    let markers = SurrealApplyMarkerStore::new(db.clone());
    let mut dag_view = SurrealVerseDagView::load(&store, verse_id)
        .await
        .map_err(ObservationPlaneError::Storage)?;
    let identity = reference_identity(branch_id);
    Ok(rebuild_projection_with_facts(
        &store,
        &markers,
        &ObservationBatchMaterializer,
        &ObservationReferenceSurface::new(db.clone()),
        &mut dag_view,
        RebuildRequest {
            identity: &identity,
            frontier,
            checkpoint: None,
        },
        &VerifierReplayFacts::new(verifier),
    )
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-10 §8 table: the door and the sealer run in the two shadow modes only, hydration in
    /// `shadow_rebuild` only.
    #[test]
    fn only_the_shadow_modes_enable_the_observation_plane() {
        let table = [
            (MigrationMode::LegacyOnly, false, false),
            (MigrationMode::SeamReady, false, false),
            (MigrationMode::DualEmitShadow, true, false),
            (MigrationMode::ShadowRebuild, true, true),
            (MigrationMode::CanonicalAuthoritativeLocal, false, false),
        ];
        for (mode, seals, hydrates) in table {
            assert_eq!(seals_observations(mode), seals, "{mode}");
            assert_eq!(hydrates_observations(mode), hydrates, "{mode}");
        }
    }

    /// SPEC-10 §3.4 rule 2: windows are half-open ingestion-HLC intervals.
    #[test]
    fn a_window_is_a_half_open_hlc_interval() {
        let width = OBSERVATION_WINDOW_BUCKET_MS;
        assert_eq!(window_bucket_of(5 * width), 5 * width);
        assert_eq!(window_bucket_of(6 * width - 1), 5 * width);
        assert_eq!(window_bucket_of(6 * width), 6 * width);
    }

    /// SPEC-10 §7.1 rules 5-6: the class is the door's, the principal the recorded did:key; a
    /// non-did:key or a decoder without its identity has no valid source.
    #[test]
    fn a_source_needs_a_did_key_principal_and_never_a_bare_decoder_class() {
        let did = fe_canonical_log::did_key::public_key_to_did(&[7u8; 32]);
        let device = observation_source(SourceClass::Device, &did, None).expect("device");
        assert_eq!(device.ingest_principal(), [7u8; 32]);
        let simulated = observation_source(SourceClass::Simulation, &did, None).expect("sim");
        assert_ne!(device.source_id(), simulated.source_id());
        assert!(observation_source(SourceClass::Device, "did:web:example.com", None).is_err());
        assert!(observation_source(SourceClass::Decoder, &did, Some("acme.x/y@1")).is_err());
    }
}
