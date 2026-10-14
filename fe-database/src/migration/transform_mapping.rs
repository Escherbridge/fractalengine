//! SPEC-8 §4.2 reviewed mapping #1: `UpdateNodeTransform` -> provisional intent schema
//! `node-transform-v1`. Derivation, shadow reduction, legacy export and the one dual-emit
//! composition the equivalence harness drives. Rationale: `migration/AGENTS.md`
//! §transform-mapping and §equivalence-harness.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use ed25519_dalek::SigningKey;
use fe_canonical_log::cbor::{decode_canonical, encode_canonical, CborValue};
use fe_canonical_log::compose::{Clock, PayloadCiphertextSource};
use fe_canonical_log::crypto::aead::{self, FreshNonce, SealingKey};
use fe_canonical_log::crypto::key_id::derive_key_id;
use fe_canonical_log::crypto::scope_key::ScopeKeyStore;
use fe_canonical_log::envelope::{
    Author, CapabilityRef, Hash32, Hlc, Identifier32, PayloadRef, Scope, UnsignedEnvelope,
    PROTOCOL_VERSION,
};
use fe_canonical_log::materialize::identity::MaterializerVersion;
use fe_canonical_log::materialize::traits::{verify_candidate, VerifiedEnvelopeMeta};
use fe_canonical_log::payload_aad::payload_aad_preimage;
use fe_canonical_log::retention::crypto_shred::ScopeKeyId;
use fe_canonical_log::signing::sign_envelope;
use fe_canonical_log::units_codec::{nano_vec3_from_cbor, nano_vec3_to_cbor};
use fe_sdk::legacy_identifier::{legacy_identifier, LegacyTable};
use fe_sdk::units::NanoVec3;

use crate::canon_log::{identifier_to_hex, op_id_to_hex};
use crate::migration::boundary::{commit_operation_dual_emit, DualEmitOutcome};
use crate::migration::candidate::{
    CandidateDerivation, CandidateDerivationError, IntentSnapshot, MigrationCandidateMember,
    MigrationCandidateSet,
};
use crate::migration::comparator::{CanonicalExport, QuantizedValue};
use crate::migration::mode::MigrationMode;
use crate::migration::rebuild::{
    ShadowMaterializationError, ShadowMaterializer, ShadowProjectionEffect,
};
use crate::migration::shadow_store::ShadowLedgerWriter;
use crate::repo::Db;
use crate::types::{NodeId, OpLogEntry, OpType};

/// The `DbCommand` mutation kind this mapping covers.
pub const TRANSFORM_MUTATION_KIND: &str = "UpdateNodeTransform";
/// The provisional canonical intent schema name (CDL-PROV register).
pub const NODE_TRANSFORM_SCHEMA_NAME: &str = "node-transform-v1";
/// The bytes `node-transform-v1`'s schema hash is taken over (SPEC-10 §4.1 rule 3 convention).
pub const NODE_TRANSFORM_SCHEMA_DOMAIN: &[u8] = b"fe-schema-node-transform-v1\0";
/// Every `node-transform-v1` candidate is a SPEC-1 kind-1 normal intent.
pub const NODE_TRANSFORM_OPERATION_KIND: u16 = 1;

/// Payload map key: the node's §4.5 legacy identifier (32 bytes).
pub const PAYLOAD_KEY_NODE_ANCHOR: u64 = 0;
/// Payload map key: position, three nanometre integers.
pub const PAYLOAD_KEY_POSITION: u64 = 1;
/// Payload map key: Euler XYZ rotation, three nanodegree integers.
pub const PAYLOAD_KEY_ROTATION: u64 = 2;
/// Payload map key: scale, three parts-per-billion integers.
pub const PAYLOAD_KEY_SCALE: u64 = 3;

const NANO_PER_UNIT: f64 = 1_000_000_000.0;
/// The first f64 magnitude whose rounding no longer fits an `i64` (2^63).
const I64_LIMIT: f64 = 9_223_372_036_854_775_808.0;

/// `BLAKE3("fe-schema-node-transform-v1\0")`, provisional.
pub fn node_transform_schema_hash() -> Hash32 {
    Hash32::of(NODE_TRANSFORM_SCHEMA_DOMAIN)
}

/// The shadow materializer identity a validation run pins (SPEC-8 §5.1 rule 1).
pub fn node_transform_materializer_version() -> MaterializerVersion {
    MaterializerVersion::new("node-transform-shadow", 1)
}

/// The comparator field path of one node's transform, keyed by its legacy identifier.
pub fn transform_field_path(node_anchor: Identifier32) -> String {
    format!("node/{}/transform", identifier_to_hex(node_anchor))
}

/// The §4.5 identifier of a legacy row id, as the canonical-log type.
pub fn legacy_anchor(table: LegacyTable, id: &str) -> Result<Identifier32, String> {
    legacy_identifier(table, id)
        .map(Identifier32)
        .map_err(|error| format!("{} id {id:?}: {error}", table.as_str()))
}

// ---------------------------------------------------------------------------
// Intent
// ---------------------------------------------------------------------------

/// One `UpdateNodeTransform` intent as the future boundary request carries it (SPEC-8 §4.1
/// rule 2): the caller's target scope plus the command's own fields, never a stored row value.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformIntent {
    /// Caller-supplied target scope, `VERSE#..-FRACTAL#..-PETAL#..`.
    pub petal_scope: String,
    /// The node the transform targets.
    pub node_id: String,
    /// Position in metres, as the command carries it.
    pub position: [f32; 3],
    /// Euler XYZ rotation in radians, as the command carries it.
    pub rotation: [f32; 3],
    /// Scale factors, as the command carries it.
    pub scale: [f32; 3],
}

impl IntentSnapshot for TransformIntent {
    fn mutation_kind(&self) -> &str {
        TRANSFORM_MUTATION_KIND
    }

    fn intent_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"fe-migration-intent-update-node-transform\0");
        for text in [&self.petal_scope, &self.node_id] {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        for axis in self
            .position
            .iter()
            .chain(&self.rotation)
            .chain(&self.scale)
        {
            hasher.update(&axis.to_bits().to_le_bytes());
        }
        *hasher.finalize().as_bytes()
    }

    fn intent_fields(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// Quantization (SPEC-1 §4; SPEC-8 §4.2 rule 2 "integer quantization")
// ---------------------------------------------------------------------------

/// A transform in SPEC-1 §4 nano-base units; the integers are authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantizedTransform {
    /// Nanometres, X/Y/Z.
    pub position_nm: NanoVec3,
    /// Euler XYZ nanodegrees.
    pub rotation_ndeg: NanoVec3,
    /// Parts per billion, X/Y/Z.
    pub scale_ppb: NanoVec3,
}

impl QuantizedTransform {
    /// The comparator value: every integer, nothing else, in a fixed textual order.
    pub fn comparator_value(&self) -> QuantizedValue {
        let render = |vector: NanoVec3| {
            let [x, y, z] = vector.as_array();
            format!("[{x},{y},{z}]")
        };
        QuantizedValue::Text(format!(
            "position_nm={};rotation_ndeg={};scale_ppb={}",
            render(self.position_nm),
            render(self.rotation_ndeg),
            render(self.scale_ppb)
        ))
    }
}

/// Why a display transform has no canonical integer form.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum QuantizationError {
    /// A component was NaN or infinite.
    #[error("{component} is not finite: {value}")]
    NotFinite {
        /// Which component.
        component: &'static str,
        /// The offending value.
        value: f64,
    },
    /// A component's nano-unit integer would not fit an `i64`.
    #[error("{component} is out of the i64 nano-unit range: {value}")]
    OutOfRange {
        /// Which component.
        component: &'static str,
        /// The offending value.
        value: f64,
    },
}

/// The mapping's ONE quantization rule, shared by the candidate and the legacy export so the
/// comparator compares like with like (SPEC-8 §5.3 rule 3).
pub trait TransformQuantizer: Send + Sync {
    /// Quantizes a display transform (metres, Euler radians, scale factors).
    fn quantize(
        &self,
        position: [f64; 3],
        rotation_radians: [f64; 3],
        scale: [f64; 3],
    ) -> Result<QuantizedTransform, QuantizationError>;
}

/// The reviewed quantization: `round(value * 1e9)`, rotation converted to degrees first.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExactTransformQuantizer;

impl TransformQuantizer for ExactTransformQuantizer {
    fn quantize(
        &self,
        position: [f64; 3],
        rotation_radians: [f64; 3],
        scale: [f64; 3],
    ) -> Result<QuantizedTransform, QuantizationError> {
        Ok(QuantizedTransform {
            position_nm: nano_vec3("position", position)?,
            rotation_ndeg: nano_vec3("rotation", rotation_radians.map(f64::to_degrees))?,
            scale_ppb: nano_vec3("scale", scale)?,
        })
    }
}

/// `round(axis * 1e9)` per axis, refusing a non-finite or `i64`-overflowing result.
pub fn nano_vec3(component: &'static str, axes: [f64; 3]) -> Result<NanoVec3, QuantizationError> {
    let mut quantized = [0i64; 3];
    for (slot, value) in quantized.iter_mut().zip(axes) {
        if !value.is_finite() {
            return Err(QuantizationError::NotFinite { component, value });
        }
        let scaled = (value * NANO_PER_UNIT).round();
        if !(-I64_LIMIT..I64_LIMIT).contains(&scaled) {
            return Err(QuantizationError::OutOfRange { component, value });
        }
        *slot = scaled as i64;
    }
    Ok(NanoVec3::from(quantized))
}

fn widen(axes: [f32; 3]) -> [f64; 3] {
    axes.map(f64::from)
}

// ---------------------------------------------------------------------------
// node-transform-v1 payload (provisional keys 0..3, CDL-PROV)
// ---------------------------------------------------------------------------

/// The decrypted `node-transform-v1` payload: intent only, no previous value (SPEC-8 §4.2 r3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeTransformPayload {
    /// `legacy_identifier("node", node_id)`.
    pub node_anchor: Identifier32,
    /// The desired transform.
    pub transform: QuantizedTransform,
}

/// Why bytes are not a `node-transform-v1` payload.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeTransformPayloadError {
    /// Not canonical CBOR at all.
    #[error("payload is not canonical CBOR: {0}")]
    NotCanonical(String),
    /// Not a map with exactly the keys 0..=3.
    #[error("payload must be a map with exactly the keys 0, 1, 2, 3")]
    WrongShape,
    /// Key 0 is not 32 bytes.
    #[error("node anchor must be a 32-byte string")]
    BadAnchor,
    /// A vector key did not decode.
    #[error("payload key {key} is not a canonical nano-unit vector")]
    BadVector {
        /// The offending key.
        key: u64,
    },
}

impl NodeTransformPayload {
    /// The canonical CBOR bytes this payload seals to.
    pub fn encode_canonical(&self) -> Vec<u8> {
        encode_canonical(&CborValue::Map(vec![
            (
                CborValue::Uint(PAYLOAD_KEY_NODE_ANCHOR),
                CborValue::Bytes(self.node_anchor.as_bytes().to_vec()),
            ),
            (
                CborValue::Uint(PAYLOAD_KEY_POSITION),
                nano_vec3_to_cbor(self.transform.position_nm),
            ),
            (
                CborValue::Uint(PAYLOAD_KEY_ROTATION),
                nano_vec3_to_cbor(self.transform.rotation_ndeg),
            ),
            (
                CborValue::Uint(PAYLOAD_KEY_SCALE),
                nano_vec3_to_cbor(self.transform.scale_ppb),
            ),
        ]))
    }

    /// Strictest reader: canonical CBOR, exactly four keys, and a byte-identical re-encode.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, NodeTransformPayloadError> {
        let value = decode_canonical(bytes)
            .map_err(|error| NodeTransformPayloadError::NotCanonical(error.to_string()))?;
        let entries = value
            .as_map()
            .ok_or(NodeTransformPayloadError::WrongShape)?;
        let keys: BTreeSet<Option<u64>> = entries.iter().map(|(key, _)| key.as_uint()).collect();
        let expected: BTreeSet<Option<u64>> = [
            PAYLOAD_KEY_NODE_ANCHOR,
            PAYLOAD_KEY_POSITION,
            PAYLOAD_KEY_ROTATION,
            PAYLOAD_KEY_SCALE,
        ]
        .into_iter()
        .map(Some)
        .collect();
        if entries.len() != expected.len() || keys != expected {
            return Err(NodeTransformPayloadError::WrongShape);
        }
        let anchor: [u8; 32] = value
            .get_uint_key(PAYLOAD_KEY_NODE_ANCHOR)
            .and_then(CborValue::as_bytes)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(NodeTransformPayloadError::BadAnchor)?;
        let vector = |key: u64| {
            value
                .get_uint_key(key)
                .and_then(|cbor| nano_vec3_from_cbor(cbor).ok())
                .ok_or(NodeTransformPayloadError::BadVector { key })
        };
        let payload = Self {
            node_anchor: Identifier32(anchor),
            transform: QuantizedTransform {
                position_nm: vector(PAYLOAD_KEY_POSITION)?,
                rotation_ndeg: vector(PAYLOAD_KEY_ROTATION)?,
                scale_ppb: vector(PAYLOAD_KEY_SCALE)?,
            },
        };
        if payload.encode_canonical() != bytes {
            return Err(NodeTransformPayloadError::NotCanonical(
                "payload does not re-encode byte-identically".to_string(),
            ));
        }
        Ok(payload)
    }
}

/// Registers `node-transform-v1` on a schema registry (kind 1), the W4-7 critic-C3 call.
pub fn register_node_transform_schema(
    schemas: &crate::canon_log::verifier::RegisteredSchemaSet,
) -> Result<(), crate::canon_log::verifier::SchemaRegistrationError> {
    schemas.register(
        NODE_TRANSFORM_OPERATION_KIND,
        node_transform_schema_hash(),
        |bytes| NodeTransformPayload::decode_canonical(bytes).is_ok(),
    )
}

// ---------------------------------------------------------------------------
// Retained candidate evidence (SPEC-8 §5.1 rule 4)
// ---------------------------------------------------------------------------

/// The run's isolated, local, content-addressed retention of exact candidate inputs: each
/// derived set (by intent digest) and each payload ciphertext (by `BLAKE3(ciphertext)`).
#[derive(Debug, Default)]
pub struct RetainedCandidates {
    ciphertexts: RwLock<HashMap<Hash32, Vec<u8>>>,
    sets: RwLock<BTreeMap<String, Vec<MigrationCandidateSet>>>,
}

impl RetainedCandidates {
    /// An empty retention store.
    pub fn new() -> Self {
        Self::default()
    }

    fn retain(&self, intent_digest_hex: String, set: MigrationCandidateSet, ciphertext: Vec<u8>) {
        self.ciphertexts
            .write()
            .expect("retained ciphertexts lock")
            .insert(Hash32::of(&ciphertext), ciphertext);
        self.sets
            .write()
            .expect("retained sets lock")
            .entry(intent_digest_hex)
            .or_default()
            .push(set);
    }

    /// The most recent set derived for this intent digest.
    pub fn latest_set_for(&self, intent_digest_hex: &str) -> Option<MigrationCandidateSet> {
        self.sets
            .read()
            .expect("retained sets lock")
            .get(intent_digest_hex)
            .and_then(|sets| sets.last().cloned())
    }

    /// Every retained set, in intent-digest order.
    pub fn all_sets(&self) -> Vec<MigrationCandidateSet> {
        self.sets
            .read()
            .expect("retained sets lock")
            .values()
            .flatten()
            .cloned()
            .collect()
    }
}

impl PayloadCiphertextSource for RetainedCandidates {
    fn ciphertext(&self, hash: &Hash32) -> Option<Vec<u8>> {
        self.ciphertexts
            .read()
            .expect("retained ciphertexts lock")
            .get(hash)
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// Candidate derivation
// ---------------------------------------------------------------------------

/// What a validation run pins for its candidates (SPEC-8 §5.1 rule 1): who signs, under which
/// chain and epoch, on which branch, sealed under which verse key, quantized how.
pub struct TransformRunCharter<Q, T> {
    /// The author key every candidate is signed with.
    pub author: SigningKey,
    /// The chain and epoch every candidate relies on.
    pub capability: CapabilityRef,
    /// The branch every candidate is on.
    pub branch_id: Identifier32,
    /// The verse-wide scope key's identity; its epoch must equal `capability.scope_epoch`.
    pub verse_key_scope: ScopeKeyId,
    /// The verse-wide scope key material.
    pub verse_key: SealingKey,
    /// The quantization rule.
    pub quantizer: Q,
    /// The HLC wall-clock source.
    pub clock: T,
}

/// Why a run charter cannot derive candidates.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransformCharterError {
    /// The key is not verse-wide, so the verifier's op->petal->verse key search cannot reach it.
    #[error("the run's scope key must be verse-wide")]
    KeyNotVerseWide,
    /// The key's epoch is not the epoch the candidates rely on.
    #[error("scope key epoch {key_epoch} differs from capability epoch {capability_epoch}")]
    EpochMismatch {
        /// The key's epoch.
        key_epoch: u64,
        /// The capability's epoch.
        capability_epoch: u64,
    },
    /// No starting parent: a kind-1 intent needs one (SPEC-1 §6).
    #[error("a run needs at least one admitted starting parent")]
    EmptyFrontier,
}

#[derive(Debug)]
struct CausalCursor {
    frontier: BTreeSet<Hash32>,
    last_hlc: Option<Hlc>,
}

/// Derives one signed, sealed `node-transform-v1` candidate per `UpdateNodeTransform` intent,
/// from the intent alone (no DB handle is reachable here).
///
/// Parents are the run's admitted frontier: two intents derived with no admission between them
/// are causal siblings, which is how concurrent intents arise (`record_admitted` advances it).
pub struct TransformCandidateDerivation<Q, T> {
    charter: TransformRunCharter<Q, T>,
    cursor: Mutex<CausalCursor>,
    retained: Arc<RetainedCandidates>,
}

impl<Q: TransformQuantizer, T: Clock> TransformCandidateDerivation<Q, T> {
    /// A derivation whose first candidates descend from `starting_frontier`.
    pub fn new(
        charter: TransformRunCharter<Q, T>,
        starting_frontier: impl IntoIterator<Item = Hash32>,
        retained: Arc<RetainedCandidates>,
    ) -> Result<Self, TransformCharterError> {
        let key_scope = charter.verse_key_scope.scope;
        if key_scope != Scope::verse_wide(key_scope.verse_id()) {
            return Err(TransformCharterError::KeyNotVerseWide);
        }
        if charter.verse_key_scope.epoch != charter.capability.scope_epoch {
            return Err(TransformCharterError::EpochMismatch {
                key_epoch: charter.verse_key_scope.epoch,
                capability_epoch: charter.capability.scope_epoch,
            });
        }
        let frontier: BTreeSet<Hash32> = starting_frontier.into_iter().collect();
        if frontier.is_empty() {
            return Err(TransformCharterError::EmptyFrontier);
        }
        Ok(Self {
            charter,
            cursor: Mutex::new(CausalCursor {
                frontier,
                last_hlc: None,
            }),
            retained,
        })
    }

    /// The run's retention store.
    pub fn retained(&self) -> &RetainedCandidates {
        &self.retained
    }

    /// The admitted frontier the next candidate will name as its parents.
    pub fn frontier(&self) -> Vec<Hash32> {
        self.lock_cursor().frontier.iter().copied().collect()
    }

    /// Advances the frontier past one ADMITTED candidate: its parents leave, it joins.
    pub fn record_admitted(&self, meta: &VerifiedEnvelopeMeta) {
        let mut cursor = self.lock_cursor();
        for parent in &meta.parents {
            cursor.frontier.remove(parent);
        }
        cursor.frontier.insert(meta.op_id);
    }

    fn lock_cursor(&self) -> std::sync::MutexGuard<'_, CausalCursor> {
        self.cursor.lock().expect("causal cursor lock")
    }

    /// A monotone author HLC: the clock's wall time, never behind the last reading.
    fn next_hlc(cursor: &mut CausalCursor, now_ms: u64) -> Hlc {
        let next = match cursor.last_hlc {
            Some(last) if last.wall_ms >= now_ms => Hlc::new(last.wall_ms, last.counter + 1),
            _ => Hlc::new(now_ms, 0),
        };
        cursor.last_hlc = Some(next);
        next
    }

    fn derive_transform(
        &self,
        intent: &TransformIntent,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError> {
        let invalid = |reason: String| CandidateDerivationError::Invalid {
            mutation_kind: TRANSFORM_MUTATION_KIND.to_string(),
            reason,
        };
        let parts = crate::scope::parse_scope(&intent.petal_scope)
            .map_err(|error| invalid(format!("target scope: {error:?}")))?;
        let petal_id = parts
            .petal_id
            .ok_or_else(|| invalid("target scope names no petal".to_string()))?;
        let verse = legacy_anchor(LegacyTable::Verse, &parts.verse_id).map_err(invalid)?;
        let petal = legacy_anchor(LegacyTable::Petal, &petal_id).map_err(invalid)?;
        let key_scope = self.charter.verse_key_scope.scope;
        if verse != key_scope.verse_id() {
            return Err(invalid("target verse is not this run's verse".to_string()));
        }
        let scope = Scope::new(verse, Some(petal), None)
            .map_err(|error| invalid(format!("scope: {error}")))?;
        let node_anchor = legacy_anchor(LegacyTable::Node, &intent.node_id).map_err(invalid)?;
        let transform = self
            .charter
            .quantizer
            .quantize(
                widen(intent.position),
                widen(intent.rotation),
                widen(intent.scale),
            )
            .map_err(|error| invalid(error.to_string()))?;
        let plaintext = NodeTransformPayload {
            node_anchor,
            transform,
        }
        .encode_canonical();

        let (parents, hlc) = {
            let mut cursor = self.lock_cursor();
            let parents: Vec<Hash32> = cursor.frontier.iter().copied().collect();
            let hlc = Self::next_hlc(&mut cursor, self.charter.clock.now_ms());
            (parents, hlc)
        };

        let key_id = derive_key_id(
            &key_scope,
            self.charter.verse_key_scope.epoch,
            &self.charter.verse_key,
        )
        .map_err(|error| invalid(format!("key id: {error}")))?;
        let nonce = FreshNonce::draw(key_id).map_err(|error| invalid(format!("nonce: {error}")))?;
        let mut unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: NODE_TRANSFORM_OPERATION_KIND,
            scope,
            author: Author::from_public_key(self.charter.author.verifying_key().to_bytes()),
            capability: self.charter.capability,
            schema_hash: node_transform_schema_hash(),
            branch_id: self.charter.branch_id,
            parents: parents.clone(),
            hlc,
            payload: PayloadRef {
                encryption: Some(nonce.params()),
                ..PayloadRef::empty()
            },
        };
        let aad =
            payload_aad_preimage(&unsigned).map_err(|error| invalid(format!("aad: {error}")))?;
        let sealed = aead::seal(&self.charter.verse_key, nonce, &aad, &plaintext)
            .map_err(|error| invalid(format!("seal: {error}")))?;
        unsigned.payload = sealed.payload_ref();
        let envelope_bytes = sign_envelope(&self.charter.author, &unsigned)
            .map_err(|error| invalid(format!("sign: {error}")))?
            .encode_canonical()
            .map_err(|error| invalid(format!("encode: {error}")))?;

        let set = MigrationCandidateSet {
            run_local_correlation_id: ulid::Ulid::new().to_string(),
            members: vec![MigrationCandidateMember {
                scope: intent.petal_scope.clone(),
                canonical_intent_schema_name: NODE_TRANSFORM_SCHEMA_NAME.to_string(),
                envelope_bytes,
                required_parent_op_ids: parents.into_iter().map(op_id_to_hex).collect(),
            }],
        };
        self.retained
            .retain(intent.intent_digest_hex(), set.clone(), sealed.ciphertext);
        Ok(set)
    }
}

impl<Q: TransformQuantizer, T: Clock> CandidateDerivation for TransformCandidateDerivation<Q, T> {
    fn derive(
        &self,
        intent: &dyn IntentSnapshot,
    ) -> Result<MigrationCandidateSet, CandidateDerivationError> {
        if intent.mutation_kind() != TRANSFORM_MUTATION_KIND {
            return Err(CandidateDerivationError::Unmapped {
                mutation_kind: intent.mutation_kind().to_string(),
            });
        }
        let fields = intent
            .intent_fields()
            .and_then(|fields| fields.downcast_ref::<TransformIntent>())
            .ok_or_else(|| CandidateDerivationError::Invalid {
                mutation_kind: TRANSFORM_MUTATION_KIND.to_string(),
                reason: "intent carries no TransformIntent fields".to_string(),
            })?;
        self.derive_transform(fields)
    }
}

// ---------------------------------------------------------------------------
// Shadow materializer
// ---------------------------------------------------------------------------

/// Reduces one `node-transform-v1` member to `node/<anchor>/transform = <quantized transform>`.
///
/// Reads only `member` plus two content-addressed sources the run pinned: its retained (or
/// durably stored) ciphertexts and its scope keys. Neither is DB state or ordering, so three
/// replays stay deterministic (`migration/AGENTS.md` §materializer-purity).
pub struct TransformShadowMaterializer<'a, C: ?Sized, K: ?Sized> {
    ciphertexts: &'a C,
    scope_keys: &'a K,
}

impl<'a, C: ?Sized, K: ?Sized> TransformShadowMaterializer<'a, C, K> {
    /// A materializer over these ciphertext and key sources.
    pub fn new(ciphertexts: &'a C, scope_keys: &'a K) -> Self {
        Self {
            ciphertexts,
            scope_keys,
        }
    }
}

impl<C, K> TransformShadowMaterializer<'_, C, K>
where
    C: PayloadCiphertextSource + ?Sized,
    K: ScopeKeyStore + ?Sized,
{
    /// Verifies, opens and decodes one member's payload.
    pub fn open_payload(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<NodeTransformPayload, ShadowMaterializationError> {
        let unreducible = |reason: String| ShadowMaterializationError::Unreducible {
            scope: member.scope.clone(),
            reason,
        };
        let candidate = verify_candidate(&member.envelope_bytes)
            .map_err(|outcome| unreducible(format!("envelope: {outcome:?}")))?;
        let unsigned = &candidate.envelope.unsigned;
        if unsigned.operation_kind != NODE_TRANSFORM_OPERATION_KIND
            || unsigned.schema_hash != node_transform_schema_hash()
        {
            return Err(unreducible("not a node-transform-v1 operation".to_string()));
        }
        let encryption = unsigned
            .payload
            .encryption
            .ok_or_else(|| unreducible("no encrypted payload".to_string()))?;
        let ciphertext = self
            .ciphertexts
            .ciphertext(&unsigned.payload.ciphertext_hash)
            .ok_or_else(|| unreducible("payload ciphertext not retained".to_string()))?;
        if Hash32::of(&ciphertext) != unsigned.payload.ciphertext_hash
            || ciphertext.len() as u64 != unsigned.payload.ciphertext_length
        {
            return Err(unreducible(
                "retained ciphertext does not match the payload reference".to_string(),
            ));
        }
        let key = payload_key(self.scope_keys, unsigned, encryption.key_id)
            .ok_or_else(|| unreducible("payload key not held".to_string()))?;
        let aad = payload_aad_preimage(unsigned).map_err(|error| unreducible(error.to_string()))?;
        let mut plaintext = aead::open(key, &encryption, &aad, &ciphertext)
            .map_err(|error| unreducible(format!("open: {error}")))?;
        let decoded = NodeTransformPayload::decode_canonical(&plaintext);
        // Best-effort wipe: fe-database has no `zeroize` dependency (canon_log/AGENTS.md).
        plaintext.fill(0);
        std::hint::black_box(&plaintext);
        decoded.map_err(|error| unreducible(error.to_string()))
    }
}

impl<C, K> ShadowMaterializer for TransformShadowMaterializer<'_, C, K>
where
    C: PayloadCiphertextSource + ?Sized,
    K: ScopeKeyStore + ?Sized,
{
    fn reduce(
        &self,
        member: &MigrationCandidateMember,
    ) -> Result<ShadowProjectionEffect, ShadowMaterializationError> {
        let payload = self.open_payload(member)?;
        Ok(ShadowProjectionEffect {
            field_path: transform_field_path(payload.node_anchor),
            value: payload.transform.comparator_value(),
        })
    }
}

/// The key whose derived `key_id` matches, searched op scope -> petal -> verse at the
/// candidate's epoch (the composed verifier's own search order).
fn payload_key<'k, K: ScopeKeyStore + ?Sized>(
    keys: &'k K,
    unsigned: &UnsignedEnvelope,
    key_id: Identifier32,
) -> Option<&'k SealingKey> {
    let epoch = unsigned.capability.scope_epoch;
    let scope = unsigned.scope;
    let mut scopes = vec![scope];
    if let Some(petal) = scope.petal_id() {
        if let Ok(petal_scope) = Scope::new(scope.verse_id(), Some(petal), None) {
            scopes.push(petal_scope);
        }
    }
    scopes.push(Scope::verse_wide(scope.verse_id()));
    scopes.dedup();
    scopes.into_iter().find_map(|scope| {
        let key = keys.scope_key(&ScopeKeyId { scope, epoch })?;
        (derive_key_id(&scope, epoch, key).ok()? == key_id).then_some(key)
    })
}

/// The `VerifiedEnvelopeMeta` of every member, for deterministic causal ordering.
pub fn member_metas(
    members: &[MigrationCandidateMember],
) -> Result<Vec<VerifiedEnvelopeMeta>, String> {
    members
        .iter()
        .map(|member| {
            verify_candidate(&member.envelope_bytes)
                .map(|candidate| candidate.meta)
                .map_err(|outcome| format!("member for {}: {outcome:?}", member.scope))
        })
        .collect()
}

/// `members` in SPEC-4 deterministic causal order, independent of the order they arrived in.
pub fn in_causal_order(
    members: &[MigrationCandidateMember],
) -> Result<Vec<MigrationCandidateMember>, String> {
    let metas = member_metas(members)?;
    let order = fe_canonical_log::materialize::ordering::deterministic_causal_order(&metas)
        .map_err(|error| error.to_string())?;
    let by_op_id: HashMap<Hash32, &MigrationCandidateMember> = metas
        .iter()
        .zip(members)
        .map(|(meta, member)| (meta.op_id, member))
        .collect();
    Ok(order
        .into_iter()
        .map(|op_id| by_op_id[&op_id].clone())
        .collect())
}

// ---------------------------------------------------------------------------
// Legacy projection export (SPEC-8 §5.3 rules 1-3)
// ---------------------------------------------------------------------------

/// One explicit legacy normalization: the stored source and what it quantized to (§5.3 r3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyNormalization {
    /// The comparator field path.
    pub field_path: String,
    /// The stored legacy row fields, as read.
    pub source: String,
    /// What the mapping's quantization made of them.
    pub normalized: QuantizedValue,
}

/// The legacy side of a comparison and every normalization that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LegacyTransformExport {
    /// The canonical export.
    pub export: CanonicalExport,
    /// One entry per exported field.
    pub normalizations: Vec<LegacyNormalization>,
}

/// Exports the stored transform (or tombstone) of each node through `quantizer`. A node with no
/// row is absent; a tombstoned node is `Tombstoned`; a row that cannot be quantized is
/// `Unquantizable`, which always compares as a difference.
pub async fn export_legacy_transforms(
    db: &Db,
    node_ids: &[String],
    quantizer: &dyn TransformQuantizer,
) -> anyhow::Result<LegacyTransformExport> {
    let mut exported = LegacyTransformExport::default();
    for node_id in node_ids {
        let anchor = legacy_anchor(LegacyTable::Node, node_id).map_err(anyhow::Error::msg)?;
        let field_path = transform_field_path(anchor);
        let mut response = db
            .query(
                "SELECT position, elevation, rotation, scale, tombstone FROM node \
                 WHERE node_id = $nid LIMIT 1",
            )
            .bind(("nid", node_id.clone()))
            .await?
            .check()?;
        let rows: Vec<serde_json::Value> = response.take(0)?;
        let Some(row) = rows.into_iter().next() else {
            continue;
        };
        let value = legacy_row_value(&row, quantizer);
        exported.normalizations.push(LegacyNormalization {
            field_path: field_path.clone(),
            source: row.to_string(),
            normalized: value.clone(),
        });
        exported.export.fields.insert(field_path, value);
    }
    Ok(exported)
}

fn legacy_row_value(row: &serde_json::Value, quantizer: &dyn TransformQuantizer) -> QuantizedValue {
    if !row["tombstone"].is_null() {
        return QuantizedValue::Tombstoned;
    }
    let unquantizable = |reason: &str| QuantizedValue::Unquantizable {
        raw_debug: format!("{reason}: {row}"),
    };
    let coordinates = &row["position"]["coordinates"];
    let axis = |value: &serde_json::Value| value.as_f64();
    let triple = |value: &serde_json::Value| -> Option<[f64; 3]> {
        let array = value.as_array().filter(|array| array.len() == 3)?;
        Some([axis(&array[0])?, axis(&array[1])?, axis(&array[2])?])
    };
    let position = match (
        axis(&coordinates[0]),
        axis(&row["elevation"]),
        axis(&coordinates[1]),
    ) {
        (Some(x), Some(y), Some(z)) => [x, y, z],
        _ => return unquantizable("position/elevation missing"),
    };
    let (Some(rotation), Some(scale)) = (triple(&row["rotation"]), triple(&row["scale"])) else {
        return unquantizable("rotation/scale missing");
    };
    match quantizer.quantize(position, rotation, scale) {
        Ok(transform) => transform.comparator_value(),
        Err(error) => unquantizable(&error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The dual-emit composition (harness only; no live handler calls it)
// ---------------------------------------------------------------------------

/// Why an intent never reached the dual-emit boundary.
#[derive(Debug, thiserror::Error)]
pub enum TransformPreconditionError {
    /// The legacy precondition refused it, exactly as the live handler would.
    #[error(transparent)]
    Legacy(anyhow::Error),
    /// A path-derived stamp is outside this mapping (D-A5 position lock lives in the handler).
    #[error("node {node_id} is a stamp; stamps are outside the node-transform-v1 mapping")]
    StampOutsideMapping {
        /// The stamp.
        node_id: String,
    },
}

/// `UpdateNodeTransform` through `commit_operation_dual_emit`: the live handler's precondition,
/// then the boundary with the handler's exact `OpLogEntry` and `UPDATE` as its legacy leg.
///
/// Not called by any `handlers/` file (CANON-DUAL-EMIT is unratified). The harness pins it to the
/// live handler with a drift guard (`tests/migration_equivalence_harness.rs`).
#[allow(clippy::too_many_arguments)]
pub async fn dual_emit_update_node_transform(
    db: &Db,
    mode: MigrationMode,
    run_id: &str,
    intent: &TransformIntent,
    derivation: &dyn CandidateDerivation,
    materializer: &dyn ShadowMaterializer,
    ledger: &dyn ShadowLedgerWriter,
) -> Result<anyhow::Result<DualEmitOutcome<()>>, TransformPreconditionError> {
    let node_id = intent.node_id.as_str();
    crate::handlers::preconditions::require_node_exists(db, node_id, TRANSFORM_MUTATION_KIND)
        .await
        .map_err(TransformPreconditionError::Legacy)?;
    if is_stamp(db, node_id)
        .await
        .map_err(TransformPreconditionError::Legacy)?
    {
        return Err(TransformPreconditionError::StampOutsideMapping {
            node_id: node_id.to_string(),
        });
    }

    let (position, rotation, scale) = (intent.position, intent.rotation, intent.scale);
    let entry = OpLogEntry {
        lamport_clock: 0,
        hlc_timestamp: String::new(),
        node_id: NodeId(node_id.to_string()),
        op_type: OpType::TransformUpdate,
        payload: serde_json::json!({
            "node_id": node_id,
            "position": position,
            "rotation": rotation,
            "scale": scale,
        }),
        sig: "00".repeat(64),
    };
    Ok(commit_operation_dual_emit(
        db,
        mode,
        run_id,
        entry,
        |_| async {
            let rot: Vec<f64> = rotation.iter().map(|&v| v as f64).collect();
            let sc: Vec<f64> = scale.iter().map(|&v| v as f64).collect();
            let mut result = db
                .query(
                    "UPDATE node SET
                position = <geometry<point>> [$x, $z],
                elevation = $y,
                rotation = $rot,
                scale = $sc,
                edit_seq = edit_seq + 1
            WHERE node_id = $node_id",
                )
                .bind(("x", position[0] as f64))
                .bind(("z", position[2] as f64))
                .bind(("y", position[1] as f64))
                .bind(("rot", rot))
                .bind(("sc", sc))
                .bind(("node_id", node_id.to_string()))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("UpdateNodeTransform DB query failed: {e}"))?;
            let updated: Vec<serde_json::Value> = result
                .take(0)
                .map_err(|e| anyhow::anyhow!("UpdateNodeTransform result read failed: {e}"))?;
            if updated.is_empty() {
                anyhow::bail!("UpdateNodeTransform matched no node with node_id = {node_id}");
            }
            Ok(())
        },
        intent,
        derivation,
        materializer,
        ledger,
    )
    .await)
}

async fn is_stamp(db: &Db, node_id: &str) -> anyhow::Result<bool> {
    let mut response = db
        .query("SELECT properties.node_kind AS node_kind FROM node WHERE node_id = $nid LIMIT 1")
        .bind(("nid", node_id.to_string()))
        .await?
        .check()?;
    let rows: Vec<serde_json::Value> = response.take(0)?;
    Ok(rows
        .first()
        .is_some_and(|row| row["node_kind"].as_str() == Some("stamp")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_quantization_scales_by_a_billion_and_refuses_what_cannot_be_an_i64() {
        let cases: [(f64, Result<i64, ()>); 6] = [
            (1.0, Ok(1_000_000_000)),
            (-0.5, Ok(-500_000_000)),
            (0.25, Ok(250_000_000)),
            (9.3e9, Err(())),
            (f64::NAN, Err(())),
            (f64::INFINITY, Err(())),
        ];
        for (value, expected) in cases {
            let got = nano_vec3("position", [value, 0.0, 0.0]).map(|vector| vector.x);
            assert_eq!(got.map_err(|_| ()), expected, "{value}");
        }
    }

    #[test]
    fn a_payload_round_trips_and_a_fifth_key_is_refused() {
        let payload = NodeTransformPayload {
            node_anchor: Identifier32([0x0a; 32]),
            transform: QuantizedTransform {
                position_nm: NanoVec3::new(1, -2, 3),
                rotation_ndeg: NanoVec3::new(0, 90_000_000_000, -1),
                scale_ppb: NanoVec3::new(1_000_000_000, 1_000_000_000, 2),
            },
        };
        let bytes = payload.encode_canonical();
        assert_eq!(NodeTransformPayload::decode_canonical(&bytes), Ok(payload));

        let mut value = decode_canonical(&bytes).expect("canonical");
        if let CborValue::Map(entries) = &mut value {
            entries.push((CborValue::Uint(4), CborValue::Uint(0)));
        }
        assert_eq!(
            NodeTransformPayload::decode_canonical(&encode_canonical(&value)),
            Err(NodeTransformPayloadError::WrongShape),
            "a previous-value or any other extra field is refused (SPEC-8 §4.2 rule 3)"
        );
    }
}
