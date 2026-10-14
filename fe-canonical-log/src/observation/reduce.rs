//! SPEC-10 §4.4 kind-5 reduction: one `observation_batch` reference row, nothing read from any
//! artifact; see `src/observation/AGENTS.md` §reduction.

use async_trait::async_trait;

use crate::cbor::{encode_canonical, CborValue};
use crate::envelope::{CompleteEnvelope, Hash32, Hlc, Scope};
use crate::kind::OBSERVATION_BATCH_OPERATION_KIND;
use crate::materialize::identity::MaterializerVersion;
use crate::materialize::traits::{
    CausalMaterializer, PayloadFacts, ProjectionMutation, VerifiedEnvelopeMeta,
};

use super::observation_batch_schema_hash;
use super::payload::ObservationBatchPayload;

/// The hot-side table the reference row is written to (CDL-SPEC10 B3 defines its schema).
pub const OBSERVATION_BATCH_TABLE: &str = "observation_batch";
/// The one binding name the reference statement reads its row from.
pub const OBSERVATION_BATCH_ROW_BINDING: &str = "observation_batch";
/// Provisional materializer name of the reference projection (register: `src/AGENTS.md`).
pub const OBSERVATION_BATCH_MATERIALIZER_ID: &str = "observation-batch-reference";
/// Version of the reference row layout; a layout change is a new version.
pub const OBSERVATION_BATCH_MATERIALIZER_VERSION: u32 = 1;

/// The §4.4 rule 1 reference row: the operation's identity, scope, author, and HLC, plus every
/// §4.3 payload field. Built only from the admitted operation's own facts (rule 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationBatchReference {
    /// Content address of the kind-5 operation; the row key.
    pub op_id: Hash32,
    /// The operation's petal scope.
    pub scope: Scope,
    /// The author's raw Ed25519 public key.
    pub author_public_key: [u8; 32],
    /// The envelope HLC.
    pub envelope_hlc: Hlc,
    /// The decrypted, validated payload.
    pub payload: ObservationBatchPayload,
}

impl ObservationBatchReference {
    /// Pairs an admitted operation's facts with its own validated payload.
    pub fn from_admitted(meta: &VerifiedEnvelopeMeta, payload: ObservationBatchPayload) -> Self {
        Self {
            op_id: meta.op_id,
            scope: meta.scope,
            author_public_key: meta.author_public_key,
            envelope_hlc: Hlc::new(meta.wall_ms, meta.counter),
            payload,
        }
    }

    /// The flat, text-keyed row: byte fields stay bytes (a storage layer renders them as hex),
    /// `sources` are their canonical signed encodings, in signed order.
    pub fn to_row(&self) -> CborValue {
        let parts = self.payload.parts();
        let bytes = |value: &[u8]| CborValue::Bytes(value.to_vec());
        let entry = |name: &str, value: CborValue| (CborValue::text(name), value);
        let sources = parts
            .sources
            .iter()
            .map(|source| CborValue::Bytes(encode_canonical(&source.to_cbor())))
            .collect();
        CborValue::Map(vec![
            entry("op_id", bytes(&self.op_id.0)),
            entry("verse_id", bytes(&self.scope.verse_id().0)),
            entry(
                "petal_id",
                self.scope
                    .petal_id()
                    .map_or(CborValue::Null, |petal| bytes(&petal.0)),
            ),
            entry("author_public_key", bytes(&self.author_public_key)),
            entry("hlc_wall_ms", CborValue::Uint(self.envelope_hlc.wall_ms)),
            entry(
                "hlc_counter",
                CborValue::Uint(u64::from(self.envelope_hlc.counter)),
            ),
            entry("hexon_artifact_id", bytes(&parts.hexon_artifact_id.0)),
            entry(
                "hexon_stored_length",
                CborValue::Uint(parts.hexon_stored_length),
            ),
            entry(
                "hlc_min_wall_ms",
                CborValue::Uint(parts.hlc_range.min.wall_ms),
            ),
            entry(
                "hlc_min_counter",
                CborValue::Uint(u64::from(parts.hlc_range.min.counter)),
            ),
            entry(
                "hlc_max_wall_ms",
                CborValue::Uint(parts.hlc_range.max.wall_ms),
            ),
            entry(
                "hlc_max_counter",
                CborValue::Uint(u64::from(parts.hlc_range.max.counter)),
            ),
            entry(
                "sensor_time_min_ms",
                CborValue::integer(parts.sensor_time_range.min_ms),
            ),
            entry(
                "sensor_time_max_ms",
                CborValue::integer(parts.sensor_time_range.max_ms),
            ),
            entry("stats_hash", bytes(&parts.stats_hash.0)),
            entry("count", CborValue::Uint(parts.count)),
            entry(
                "anchors",
                CborValue::Array(
                    parts
                        .anchors
                        .iter()
                        .map(|anchor| bytes(&anchor.0))
                        .collect(),
                ),
            ),
            entry(
                "asset_refs",
                CborValue::Array(
                    parts
                        .asset_refs
                        .iter()
                        .map(|asset| bytes(&asset.0))
                        .collect(),
                ),
            ),
            entry("sources", CborValue::Array(sources)),
            entry(
                "hexon_format_version",
                CborValue::Uint(u64::from(parts.hexon_format_version)),
            ),
            entry(
                "stats_stored_length",
                CborValue::Uint(parts.stats_stored_length),
            ),
        ])
    }

    /// The one `Apply` mutation writing this row.
    pub fn mutation(&self) -> ProjectionMutation {
        ProjectionMutation::Apply {
            statements: vec![format!(
                "CREATE {OBSERVATION_BATCH_TABLE} CONTENT ${OBSERVATION_BATCH_ROW_BINDING}"
            )],
            bindings: vec![(OBSERVATION_BATCH_ROW_BINDING.to_string(), self.to_row())],
        }
    }
}

/// The kind-5 reduction (SPEC-10 §4.4). Its inputs are exactly `meta`, `envelope_bytes`, and the
/// operation's own `payload_facts`; it has no store to consult, so whether a peer holds the
/// hexon, the statistics artifact, or any asset blob cannot change its output (rule 2).
///
/// - Another kind: `Excluded` (this projection does not project it).
/// - Facts absent or not this operation's: pending on the operation's OWN payload ciphertext,
///   `ReferencedArtifactUnavailable { PayloadRef::ciphertext_hash }` -- never on the hexon, which
///   rule 3 forbids, and never a fabricated field (`src/observation/AGENTS.md` §reduction).
/// - Facts under another schema, or that do not decode: `Excluded` with the reason.
/// - Otherwise exactly one `Apply` writing the reference row.
pub fn reduce_observation_batch(
    meta: &VerifiedEnvelopeMeta,
    envelope_bytes: &[u8],
    payload_facts: Option<&PayloadFacts>,
) -> ProjectionMutation {
    if meta.operation_kind != OBSERVATION_BATCH_OPERATION_KIND {
        return ProjectionMutation::Excluded {
            reason: format!(
                "operation kind {} is not an observation_batch",
                meta.operation_kind
            ),
        };
    }
    let Some(facts) = payload_facts.filter(|facts| facts.op_id == meta.op_id) else {
        return own_payload_pending(meta, envelope_bytes);
    };
    let schema = observation_batch_schema_hash();
    if meta.schema_hash != schema || facts.schema_hash != schema {
        return ProjectionMutation::Excluded {
            reason: "observation_batch under an unregistered schema".to_string(),
        };
    }
    match ObservationBatchPayload::from_cbor(&facts.fields) {
        Ok(payload) => ObservationBatchReference::from_admitted(meta, payload).mutation(),
        Err(error) => ProjectionMutation::Excluded {
            reason: format!("invalid observation_batch payload: {error}"),
        },
    }
}

/// Pending on the payload ciphertext the operation's own signed bytes commit to. Bytes that are
/// not the admitted operation name the operation itself, so the position still stays pending.
fn own_payload_pending(meta: &VerifiedEnvelopeMeta, envelope_bytes: &[u8]) -> ProjectionMutation {
    let artifact_id = match CompleteEnvelope::decode_canonical(envelope_bytes) {
        Ok(envelope) if Hash32::of(envelope_bytes) == meta.op_id => {
            envelope.unsigned.payload.ciphertext_hash
        }
        _ => meta.op_id,
    };
    ProjectionMutation::ReferencedArtifactUnavailable { artifact_id }
}

/// The reference-row [`CausalMaterializer`]; stateless by design.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObservationBatchMaterializer;

impl ObservationBatchMaterializer {
    /// The provisional `(name, version)` this projection is identified by.
    pub fn version() -> MaterializerVersion {
        MaterializerVersion::new(
            OBSERVATION_BATCH_MATERIALIZER_ID,
            OBSERVATION_BATCH_MATERIALIZER_VERSION,
        )
    }
}

#[async_trait]
impl CausalMaterializer for ObservationBatchMaterializer {
    async fn reduce(
        &self,
        meta: &VerifiedEnvelopeMeta,
        envelope_bytes: &[u8],
        payload_facts: Option<&PayloadFacts>,
    ) -> ProjectionMutation {
        reduce_observation_batch(meta, envelope_bytes, payload_facts)
    }
}
