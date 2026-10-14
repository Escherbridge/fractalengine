//! SPEC-4 §1.5 materializer version identity and the keys derived from it, plus the SPEC-10 §7
//! observation source identity (erratum G6); see `src/AGENTS.md` §module-ownership,
//! §materializer-identity, and `materialize/AGENTS.md` §g6-source-identity.

use fe_sdk::source_class::is_valid_format_id;
use thiserror::Error;
use unicode_normalization::is_nfc;

use crate::cbor::{
    decode_canonical, encode_canonical, encode_canonical_checked, CborError, CborValue,
};
use crate::envelope::{Hash32, Identifier32};
use crate::signing::domain_preimage;

use super::checkpoint_binding::CheckpointBinding;

/// NUL-terminated ASCII domain of the SPEC-5 materializer registry identifier. Provisional
/// (D-CL24): see `src/AGENTS.md` §Provisional wire numbering.
pub const MATERIALIZER_ID_DOMAIN: &[u8] = b"fe-materializer-id-v1\0";

/// An explicit, hand-authored materializer identity (SPEC-4 §1.5).
///
/// MUST NOT be derived from `CARGO_PKG_VERSION`, `env!`, or any other build-time identifier:
/// §1.5 requires a materializer change to be an explicit author decision, not a side effect of
/// a crate version bump that never touched reduction logic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MaterializerVersion {
    /// Stable name of the materializer implementation, for example `"scene-graph"`.
    pub materializer_id: String,
    /// Hand-incremented version of that materializer's reduction logic.
    pub version: u32,
}

impl MaterializerVersion {
    /// Builds an explicit materializer identity.
    pub fn new(materializer_id: impl Into<String>, version: u32) -> Self {
        Self {
            materializer_id: materializer_id.into(),
            version,
        }
    }

    /// The 32-byte identifier a SPEC-5 checkpoint claim names this materializer by:
    /// `BLAKE3(MATERIALIZER_ID_DOMAIN || canonical_cbor({0: materializer_id, 1: version}))`.
    ///
    /// The name is NFC-normalized by the CBOR text constructor, so the identifier commits to the
    /// normalized name. A version bump changes the identifier.
    pub fn registry_id(&self) -> Identifier32 {
        let body = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::text(&self.materializer_id)),
            (CborValue::Uint(1), CborValue::Uint(u64::from(self.version))),
        ]);
        let preimage = domain_preimage(MATERIALIZER_ID_DOMAIN, &encode_canonical(&body));
        Identifier32(*blake3::hash(&preimage).as_bytes())
    }
}

/// A materializer version bound to the branch it projects (SPEC-4 §1.5, D-CL19).
///
/// Part of checkpoint identity: two checkpoints for the same branch under different
/// materializer versions are checkpoints of different projections and must never be compared
/// or substituted for one another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionIdentity {
    /// The materializer whose reduction logic produced this projection.
    pub materializer_version: MaterializerVersion,
    /// The branch this projection tracks.
    pub branch_id: Identifier32,
}

impl ProjectionIdentity {
    /// Builds a projection identity.
    pub fn new(materializer_version: MaterializerVersion, branch_id: Identifier32) -> Self {
        Self {
            materializer_version,
            branch_id,
        }
    }
}

/// Binds a projection identity to one admitted operation, for the durable apply marker
/// (SPEC-4 §3.2-§3.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ApplyMarkerKey {
    /// The projection this marker records progress for.
    pub projection_identity: ProjectionIdentity,
    /// The operation this marker attests was applied, or explicitly excluded.
    pub op_id: Hash32,
}

impl ApplyMarkerKey {
    /// Builds an apply-marker key.
    pub fn new(projection_identity: ProjectionIdentity, op_id: Hash32) -> Self {
        Self {
            projection_identity,
            op_id,
        }
    }
}

// ---- SPEC-10 §7 observation source identity (erratum G6); see `materialize/AGENTS.md` §g6 ----

/// The SPEC-10 §7.1 source class, re-exported from fe-sdk: one enum for the whole workspace.
pub use fe_sdk::source_class::SourceClass;

/// NUL-terminated ASCII domain of the SPEC-10 §7.2 `source_id`. Provisional (D-CL34).
pub const OBSERVATION_SOURCE_ID_DOMAIN: &[u8] = b"fe-observation-source-id-v1\0";
/// Longest `DecoderIdentity::plugin_id`, in bytes (SPEC-10 §7.1).
pub const PLUGIN_ID_MAX_BYTES: usize = 128;
/// Longest `DecoderIdentity::plugin_version`, in bytes (SPEC-10 §7.1).
pub const PLUGIN_VERSION_MAX_BYTES: usize = 64;

const SOURCE_CONTEXT: &str = "observation source";
const SOURCE_KEYS: &[u64] = &[0, 1, 2, 3, 4];
const DECODER_CONTEXT: &str = "decoder identity";
const DECODER_KEYS: &[u64] = &[0, 1, 2];

/// Every reason an `ObservationSource` or `DecoderIdentity` is refused (SPEC-10 §7.1).
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ObservationSourceError {
    /// The bytes were not canonical CBOR: a float, a duplicate key, a non-shortest length...
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A value that must be a map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// Map being parsed.
        context: &'static str,
    },
    /// A key outside the grammar, or a non-integer key, was present (§7.1 rule 1).
    #[error("{context} carries an unknown or non-integer key")]
    UnknownKey {
        /// Map being parsed.
        context: &'static str,
    },
    /// A listed key was absent.
    #[error("{context} key {key} is missing")]
    MissingKey {
        /// Map being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key held the wrong CBOR type or a byte string of the wrong length.
    #[error("{context} key {key} holds the wrong CBOR type or length")]
    WrongType {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// `source_class` was `0` or above `5` (§7.1 rule 2).
    #[error("source class {value} is not one of 1 to 5")]
    InvalidSourceClass {
        /// The refused wire value.
        value: u64,
    },
    /// A decoder identity on a non-decoder class, or a decoder class without one (§7.1 rule 3).
    #[error("a decoder identity must be present exactly when the source class is decoder")]
    DecoderClassMismatch,
    /// A decoder-class source carried no `format_id` (§7.1 rule 3).
    #[error("a decoder-class source requires a format_id")]
    DecoderWithoutFormatId,
    /// A `format_id` broke the SPEC-10 §2.2 rule 4 grammar (§7.1 rule 4).
    #[error("format_id is not <namespace>/<name>@<major>")]
    InvalidFormatId,
    /// A decoder text field was empty or over its byte limit.
    #[error("{field} is {length} byte(s); 1 to {maximum} are allowed")]
    TextLengthOutOfRange {
        /// The offending field.
        field: &'static str,
        /// Its byte length.
        length: usize,
        /// The field's limit.
        maximum: usize,
    },
    /// A text field was not Unicode NFC (§7.1 rule 1).
    #[error("{field} is not Unicode NFC")]
    NotNormalizedNfc {
        /// The offending field.
        field: &'static str,
    },
    /// The bytes decoded but were not their own canonical re-encoding.
    #[error("{context} bytes are not their own canonical re-encoding")]
    NonCanonicalEncoding {
        /// Map being parsed.
        context: &'static str,
    },
}

fn require_source_keys(
    value: &CborValue,
    expected: &[u64],
    context: &'static str,
) -> Result<(), ObservationSourceError> {
    let entries = value
        .as_map()
        .ok_or(ObservationSourceError::ExpectedMap { context })?;
    for (key, _) in entries {
        match key.as_uint() {
            Some(key) if expected.contains(&key) => {}
            _ => return Err(ObservationSourceError::UnknownKey { context }),
        }
    }
    for &key in expected {
        if value.get_uint_key(key).is_none() {
            return Err(ObservationSourceError::MissingKey { context, key });
        }
    }
    Ok(())
}

fn source_field<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, ObservationSourceError> {
    value
        .get_uint_key(key)
        .ok_or(ObservationSourceError::MissingKey { context, key })
}

fn source_bytes32(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; 32], ObservationSourceError> {
    source_field(value, key, context)?
        .as_bytes()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or(ObservationSourceError::WrongType { context, key })
}

fn source_text(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<String, ObservationSourceError> {
    source_field(value, key, context)?
        .as_text()
        .map(str::to_owned)
        .ok_or(ObservationSourceError::WrongType { context, key })
}

fn check_decoder_text(
    field: &'static str,
    text: &str,
    maximum: usize,
) -> Result<(), ObservationSourceError> {
    if text.is_empty() || text.len() > maximum {
        return Err(ObservationSourceError::TextLengthOutOfRange {
            field,
            length: text.len(),
            maximum,
        });
    }
    if !is_nfc(text) {
        return Err(ObservationSourceError::NotNormalizedNfc { field });
    }
    Ok(())
}

/// Refuses bytes that decoded but are not their own canonical re-encoding.
fn assert_source_canonical(
    value: &CborValue,
    bytes: &[u8],
    context: &'static str,
) -> Result<(), ObservationSourceError> {
    if encode_canonical_checked(value)? != bytes {
        return Err(ObservationSourceError::NonCanonicalEncoding { context });
    }
    Ok(())
}

/// The decoder plugin that produced a decoded reading (SPEC-10 §7.1, `DecoderIdentity`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecoderIdentity {
    plugin_id: String,
    plugin_version: String,
    module_hash: Hash32,
}

impl DecoderIdentity {
    /// Builds a decoder identity, refusing empty, over-long, or non-NFC text.
    pub fn new(
        plugin_id: impl Into<String>,
        plugin_version: impl Into<String>,
        module_hash: Hash32,
    ) -> Result<Self, ObservationSourceError> {
        let plugin_id = plugin_id.into();
        let plugin_version = plugin_version.into();
        check_decoder_text("plugin_id", &plugin_id, PLUGIN_ID_MAX_BYTES)?;
        check_decoder_text("plugin_version", &plugin_version, PLUGIN_VERSION_MAX_BYTES)?;
        Ok(Self {
            plugin_id,
            plugin_version,
            module_hash,
        })
    }

    /// The decoder plugin's identifier; also its §7.2 `source_label`.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// The decoder plugin's version string.
    pub fn plugin_version(&self) -> &str {
        &self.plugin_version
    }

    /// BLAKE3 of the exact decoder module bytes (Wasm module or Rhai source).
    pub fn module_hash(&self) -> Hash32 {
        self.module_hash
    }

    /// Encodes the `{0 plugin_id, 1 plugin_version, 2 module_hash}` map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::text(&self.plugin_id)),
            (CborValue::Uint(1), CborValue::text(&self.plugin_version)),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.module_hash.0.to_vec()),
            ),
        ])
    }

    /// Decodes the map, refusing unknown keys and every §7.1 text rule.
    pub fn from_cbor(value: &CborValue) -> Result<Self, ObservationSourceError> {
        require_source_keys(value, DECODER_KEYS, DECODER_CONTEXT)?;
        Self::new(
            source_text(value, 0, DECODER_CONTEXT)?,
            source_text(value, 1, DECODER_CONTEXT)?,
            Hash32(source_bytes32(value, 2, DECODER_CONTEXT)?),
        )
    }
}

/// Who produced an observation and which principal the ingest door authenticated
/// (SPEC-10 §7.1, erratum G6). Every invariant holds by construction: the fields are private and
/// [`Self::new`] and [`Self::from_cbor`] are the only doors.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ObservationSource {
    source_class: SourceClass,
    source_id: Identifier32,
    ingest_principal: [u8; 32],
    format_id: Option<String>,
    decoder: Option<DecoderIdentity>,
}

impl ObservationSource {
    /// Builds a source, enforcing §7.1 rules 3 and 4.
    ///
    /// `ingest_principal` is the Ed25519 key of the `did:key` the door recorded as `source_did`
    /// (rule 5); this codec checks only its length, because the door owns that derivation.
    pub fn new(
        source_class: SourceClass,
        source_id: Identifier32,
        ingest_principal: [u8; 32],
        format_id: Option<String>,
        decoder: Option<DecoderIdentity>,
    ) -> Result<Self, ObservationSourceError> {
        if decoder.is_some() != (source_class == SourceClass::Decoder) {
            return Err(ObservationSourceError::DecoderClassMismatch);
        }
        if source_class == SourceClass::Decoder && format_id.is_none() {
            return Err(ObservationSourceError::DecoderWithoutFormatId);
        }
        if format_id
            .as_deref()
            .is_some_and(|format_id| !is_valid_format_id(format_id))
        {
            return Err(ObservationSourceError::InvalidFormatId);
        }
        Ok(Self {
            source_class,
            source_id,
            ingest_principal,
            format_id,
            decoder,
        })
    }

    /// `source_id = BLAKE3("fe-observation-source-id-v1" || 00 || canonical_cbor([class,
    /// label]))` (SPEC-10 §7.2). The class is inside the hash, so a device and a decoder that
    /// share a label stay distinct. The label is NFC-normalized by the CBOR text constructor.
    pub fn source_id_for(source_class: SourceClass, source_label: &str) -> Identifier32 {
        let body = CborValue::Array(vec![
            CborValue::Uint(source_class.wire()),
            CborValue::text(source_label),
        ]);
        let preimage = domain_preimage(OBSERVATION_SOURCE_ID_DOMAIN, &encode_canonical(&body));
        Identifier32(*blake3::hash(&preimage).as_bytes())
    }

    /// Who produced the observation.
    pub fn source_class(&self) -> SourceClass {
        self.source_class
    }

    /// The §7.2 source identifier.
    pub fn source_id(&self) -> Identifier32 {
        self.source_id
    }

    /// The Ed25519 public key of the principal the ingest door authenticated.
    pub fn ingest_principal(&self) -> [u8; 32] {
        self.ingest_principal
    }

    /// The descriptor format of the observation's payload, if any.
    pub fn format_id(&self) -> Option<&str> {
        self.format_id.as_deref()
    }

    /// The decoder identity; present exactly for the decoder class.
    pub fn decoder(&self) -> Option<&DecoderIdentity> {
        self.decoder.as_ref()
    }

    /// Encodes the `{0 class, 1 source_id, 2 ingest_principal, 3 format_id, 4 decoder}` map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(self.source_class.wire()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.source_id.0.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.ingest_principal.to_vec()),
            ),
            (
                CborValue::Uint(3),
                self.format_id
                    .as_deref()
                    .map_or(CborValue::Null, CborValue::text),
            ),
            (
                CborValue::Uint(4),
                self.decoder
                    .as_ref()
                    .map_or(CborValue::Null, DecoderIdentity::to_cbor),
            ),
        ])
    }

    /// The exact canonical bytes; these enter a kind-5 payload's signed `sources` list.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, ObservationSourceError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes an embedded source map, enforcing every §7.1 rule.
    pub fn from_cbor(value: &CborValue) -> Result<Self, ObservationSourceError> {
        require_source_keys(value, SOURCE_KEYS, SOURCE_CONTEXT)?;
        let class_value = source_field(value, 0, SOURCE_CONTEXT)?.as_uint().ok_or(
            ObservationSourceError::WrongType {
                context: SOURCE_CONTEXT,
                key: 0,
            },
        )?;
        let source_class = SourceClass::from_wire(class_value)
            .ok_or(ObservationSourceError::InvalidSourceClass { value: class_value })?;
        let source_id = Identifier32(source_bytes32(value, 1, SOURCE_CONTEXT)?);
        let ingest_principal = source_bytes32(value, 2, SOURCE_CONTEXT)?;
        let format_id = match source_field(value, 3, SOURCE_CONTEXT)? {
            CborValue::Null => None,
            CborValue::Text(text) => Some(text.clone()),
            _ => {
                return Err(ObservationSourceError::WrongType {
                    context: SOURCE_CONTEXT,
                    key: 3,
                })
            }
        };
        let decoder = match source_field(value, 4, SOURCE_CONTEXT)? {
            CborValue::Null => None,
            decoder @ CborValue::Map(_) => Some(DecoderIdentity::from_cbor(decoder)?),
            _ => {
                return Err(ObservationSourceError::WrongType {
                    context: SOURCE_CONTEXT,
                    key: 4,
                })
            }
        };
        Self::new(
            source_class,
            source_id,
            ingest_principal,
            format_id,
            decoder,
        )
    }

    /// Decodes standalone bytes: canonical CBOR first, then every §7.1 rule.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, ObservationSourceError> {
        let value = decode_canonical(bytes)?;
        let source = Self::from_cbor(&value)?;
        assert_source_canonical(&value, bytes, SOURCE_CONTEXT)?;
        Ok(source)
    }
}

impl PartialOrd for ObservationSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ObservationSource {
    /// Canonical order: bytewise over the canonical encoding, the order a signed `sources` list
    /// is sorted in. Canonical encoding is injective, so this agrees with `Eq`.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        encode_canonical(&self.to_cbor()).cmp(&encode_canonical(&other.to_cbor()))
    }
}

/// SPEC-4 §7 rule 1 analytics input identity extended by rule 5 (erratum G6) with the
/// observation input set: the sorted, deduplicated kind-5 `op_id`s a computation read.
///
/// The set is private and kept canonical on every insert, so two computations that read the same
/// batches in any order, any number of times, record equal identities.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnalyticsSourceIdentity {
    /// Branch the source projection tracks.
    pub branch_id: Identifier32,
    /// D-CL19 commitment of the frontier the computation read at.
    pub frontier_commitment: Hash32,
    /// Segment manifest backing that frontier.
    pub segment_manifest_id: Hash32,
    /// Materializer identity of the source projection.
    pub materializer_version: MaterializerVersion,
    observation_input_set: Vec<Hash32>,
}

impl AnalyticsSourceIdentity {
    /// Builds an identity, sorting and deduplicating the observation batches read.
    pub fn new(
        branch_id: Identifier32,
        frontier_commitment: Hash32,
        segment_manifest_id: Hash32,
        materializer_version: MaterializerVersion,
        observation_batch_op_ids: impl IntoIterator<Item = Hash32>,
    ) -> Self {
        let mut observation_input_set: Vec<Hash32> = observation_batch_op_ids.into_iter().collect();
        observation_input_set.sort_unstable();
        observation_input_set.dedup();
        Self {
            branch_id,
            frontier_commitment,
            segment_manifest_id,
            materializer_version,
            observation_input_set,
        }
    }

    /// Builds an identity from a checkpoint that binds the rule 1 tuple (SPEC-4 §7 rule 1).
    pub fn from_checkpoint(
        binding: &CheckpointBinding,
        observation_batch_op_ids: impl IntoIterator<Item = Hash32>,
    ) -> Self {
        Self::new(
            binding.branch_id,
            binding.frontier_commitment,
            binding.segment_manifest_id,
            binding.materializer_version.clone(),
            observation_batch_op_ids,
        )
    }

    /// Records one more batch read, keeping the set sorted and duplicate-free.
    pub fn record_observation_batch(&mut self, op_id: Hash32) {
        if let Err(position) = self.observation_input_set.binary_search(&op_id) {
            self.observation_input_set.insert(position, op_id);
        }
    }

    /// The observation input set, strictly ascending.
    pub fn observation_input_set(&self) -> &[Hash32] {
        &self.observation_input_set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn op(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    #[test]
    fn a_materializer_version_bump_yields_a_distinct_projection_identity_and_apply_marker_key() {
        let branch_id = branch(0x11);
        let op_id = op(0x22);
        let v1 = MaterializerVersion::new("scene-graph", 1);
        let v2 = MaterializerVersion::new("scene-graph", 2);

        let identity_v1 = ProjectionIdentity::new(v1, branch_id);
        let identity_v2 = ProjectionIdentity::new(v2, branch_id);
        assert_ne!(identity_v1, identity_v2);

        let marker_v1 = ApplyMarkerKey::new(identity_v1.clone(), op_id);
        let marker_v2 = ApplyMarkerKey::new(identity_v2.clone(), op_id);
        assert_ne!(marker_v1, marker_v2);

        // Same materializer id and version, same branch: identity is stable and reproducible.
        let identity_v1_again =
            ProjectionIdentity::new(MaterializerVersion::new("scene-graph", 1), branch_id);
        assert_eq!(identity_v1, identity_v1_again);
    }

    #[test]
    fn materializer_id_alone_also_distinguishes_projection_identity() {
        let branch_id = branch(0x33);
        let a = ProjectionIdentity::new(MaterializerVersion::new("scene-graph", 1), branch_id);
        let b = ProjectionIdentity::new(MaterializerVersion::new("terrain-index", 1), branch_id);
        assert_ne!(a, b);
    }

    #[test]
    fn the_registry_id_hashes_the_domain_and_the_hand_encoded_name_version_map() {
        // {0: "scene-graph", 1: 3}: map(2), key 0, text(11), key 1, uint 3 -- RFC 8949 §4.2.1.
        let mut expected_body = vec![0xa2, 0x00, 0x6b];
        expected_body.extend_from_slice(b"scene-graph");
        expected_body.extend_from_slice(&[0x01, 0x03]);
        let mut preimage = b"fe-materializer-id-v1".to_vec();
        preimage.push(0x00);
        preimage.extend_from_slice(&expected_body);

        assert_eq!(
            MaterializerVersion::new("scene-graph", 3).registry_id(),
            Identifier32(*blake3::hash(&preimage).as_bytes())
        );
    }

    #[test]
    fn the_registry_id_separates_every_name_and_version_pair() {
        let cases = [
            ("scene-graph", 3),
            ("scene-graph", 4),
            ("terrain-index", 3),
            ("scene-graph-", 3),
        ];
        for (index, (name, version)) in cases.iter().enumerate() {
            let id = MaterializerVersion::new(*name, *version).registry_id();
            assert_eq!(id, MaterializerVersion::new(*name, *version).registry_id());
            for (other_name, other_version) in &cases[index + 1..] {
                assert_ne!(
                    id,
                    MaterializerVersion::new(*other_name, *other_version).registry_id(),
                    "{name} v{version} collides with {other_name} v{other_version}"
                );
            }
        }
    }

    #[test]
    fn a_different_branch_also_distinguishes_projection_identity() {
        let version = MaterializerVersion::new("scene-graph", 1);
        let a = ProjectionIdentity::new(version.clone(), branch(0x01));
        let b = ProjectionIdentity::new(version, branch(0x02));
        assert_ne!(a, b);
    }

    // ---- G6: SPEC-10 §7 conformance tests 17 and 18 ----

    fn principal() -> [u8; 32] {
        [0x7A; 32]
    }

    fn device_source() -> ObservationSource {
        ObservationSource::new(
            SourceClass::Device,
            ObservationSource::source_id_for(SourceClass::Device, "thermo-17"),
            principal(),
            None,
            None,
        )
        .expect("device source")
    }

    fn decoder_source() -> ObservationSource {
        ObservationSource::new(
            SourceClass::Decoder,
            ObservationSource::source_id_for(SourceClass::Decoder, "acme.frame-decoder"),
            principal(),
            Some("acme.thermo/frame@1".to_owned()),
            Some(
                DecoderIdentity::new("acme.frame-decoder", "1.2.0", Hash32([0x4D; 32]))
                    .expect("decoder"),
            ),
        )
        .expect("decoder source")
    }

    /// Replaces the first occurrence of `from` in `bytes` with `to`.
    fn splice(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        let at = bytes
            .windows(from.len())
            .position(|window| window == from)
            .expect("pattern present");
        let mut out = bytes[..at].to_vec();
        out.extend_from_slice(to);
        out.extend_from_slice(&bytes[at + from.len()..]);
        out
    }

    fn source_map(entries: Vec<(u64, CborValue)>) -> Vec<u8> {
        encode_canonical(&CborValue::Map(
            entries
                .into_iter()
                .map(|(key, value)| (CborValue::Uint(key), value))
                .collect(),
        ))
    }

    fn source_entries(class: u64, format_id: CborValue, decoder: CborValue) -> Vec<u8> {
        source_map(vec![
            (0, CborValue::Uint(class)),
            (1, CborValue::Bytes(vec![0x11; 32])),
            (2, CborValue::Bytes(principal().to_vec())),
            (3, format_id),
            (4, decoder),
        ])
    }

    #[test]
    fn observation_source_cbor_canonical_round_trip_and_refusals() {
        for source in [device_source(), decoder_source()] {
            let bytes = source.encode_canonical().expect("bytes");
            assert_eq!(
                ObservationSource::decode_canonical(&bytes).expect("round trip"),
                source
            );
        }

        let device = device_source().encode_canonical().expect("bytes");
        assert_eq!(device[0], 0xA5, "a five-entry map");
        let decoder_map = decoder_source().decoder().expect("decoder").to_cbor();
        let mut float_at_key_4 = device.clone();
        let last = float_at_key_4.len() - 1;
        assert_eq!(float_at_key_4[last], 0xF6, "key 4 is null for a device");
        float_at_key_4.truncate(last);
        float_at_key_4.extend_from_slice(&[0xFB, 0x3F, 0xF0, 0, 0, 0, 0, 0, 0]);

        let mut duplicate_key = vec![0xA6, 0x00, 0x01];
        duplicate_key.extend_from_slice(&device[1..]);

        let cases: Vec<(&str, Vec<u8>, ObservationSourceError)> = vec![
            (
                "float",
                float_at_key_4,
                ObservationSourceError::Cbor(CborError::UnsupportedSimpleValue {
                    additional: 27,
                    offset: device.len() - 1,
                }),
            ),
            (
                "unknown key",
                source_map(vec![
                    (0, CborValue::Uint(1)),
                    (1, CborValue::Bytes(vec![0x11; 32])),
                    (2, CborValue::Bytes(principal().to_vec())),
                    (3, CborValue::Null),
                    (4, CborValue::Null),
                    (5, CborValue::Null),
                ]),
                ObservationSourceError::UnknownKey {
                    context: SOURCE_CONTEXT,
                },
            ),
            (
                "duplicate key",
                duplicate_key,
                ObservationSourceError::Cbor(CborError::DuplicateMapKey { offset: 3 }),
            ),
            (
                "non-canonical length",
                splice(&device, &[0x58, 0x20], &[0x59, 0x00, 0x20]),
                ObservationSourceError::Cbor(CborError::NonMinimalArgument {
                    value: 32,
                    width: 2,
                    offset: 4,
                }),
            ),
            (
                "class 0",
                source_entries(0, CborValue::Null, CborValue::Null),
                ObservationSourceError::InvalidSourceClass { value: 0 },
            ),
            (
                "class 6",
                source_entries(6, CborValue::Null, CborValue::Null),
                ObservationSourceError::InvalidSourceClass { value: 6 },
            ),
            (
                "decoder on a non-decoder class",
                source_entries(
                    SourceClass::Connector.wire(),
                    CborValue::text("acme.thermo/frame@1"),
                    decoder_map.clone(),
                ),
                ObservationSourceError::DecoderClassMismatch,
            ),
            (
                "decoder class without a decoder",
                source_entries(
                    SourceClass::Decoder.wire(),
                    CborValue::text("acme.thermo/frame@1"),
                    CborValue::Null,
                ),
                ObservationSourceError::DecoderClassMismatch,
            ),
            (
                "decoder class without a format_id",
                source_entries(SourceClass::Decoder.wire(), CborValue::Null, decoder_map),
                ObservationSourceError::DecoderWithoutFormatId,
            ),
            (
                "malformed format_id",
                source_entries(
                    SourceClass::Device.wire(),
                    CborValue::text("Acme/frame@01"),
                    CborValue::Null,
                ),
                ObservationSourceError::InvalidFormatId,
            ),
            (
                "short ingest principal",
                source_map(vec![
                    (0, CborValue::Uint(1)),
                    (1, CborValue::Bytes(vec![0x11; 32])),
                    (2, CborValue::Bytes(vec![0x7A; 31])),
                    (3, CborValue::Null),
                    (4, CborValue::Null),
                ]),
                ObservationSourceError::WrongType {
                    context: SOURCE_CONTEXT,
                    key: 2,
                },
            ),
        ];
        for (name, bytes, expected) in cases {
            assert_eq!(
                ObservationSource::decode_canonical(&bytes),
                Err(expected),
                "{name}"
            );
        }

        assert!(matches!(
            DecoderIdentity::new("", "1", Hash32([0; 32])),
            Err(ObservationSourceError::TextLengthOutOfRange { .. })
        ));
        assert!(matches!(
            DecoderIdentity::new(
                "p",
                "v".repeat(PLUGIN_VERSION_MAX_BYTES + 1),
                Hash32([0; 32])
            ),
            Err(ObservationSourceError::TextLengthOutOfRange { .. })
        ));
        assert_eq!(
            DecoderIdentity::new("e\u{0301}", "1", Hash32([0; 32])),
            Err(ObservationSourceError::NotNormalizedNfc { field: "plugin_id" })
        );
    }

    #[test]
    fn source_id_binds_the_class_and_the_label() {
        let device = ObservationSource::source_id_for(SourceClass::Device, "label");
        assert_ne!(
            device,
            ObservationSource::source_id_for(SourceClass::Decoder, "label"),
            "a device and a decoder sharing a label stay distinct"
        );
        assert_ne!(
            device,
            ObservationSource::source_id_for(SourceClass::Device, "label2")
        );

        // [1, "label"]: array(2), uint 1, text(5) -- RFC 8949 §4.2.1.
        let mut preimage = b"fe-observation-source-id-v1".to_vec();
        preimage.push(0x00);
        preimage.extend_from_slice(&[0x82, 0x01, 0x65]);
        preimage.extend_from_slice(b"label");
        assert_eq!(device, Identifier32(*blake3::hash(&preimage).as_bytes()));
    }

    #[test]
    fn analytics_source_identity_dedupes_and_sorts_batch_ids() {
        let version = MaterializerVersion::new("observation-catalog", 1);
        let identity = |ids: &[u8]| {
            AnalyticsSourceIdentity::new(
                branch(0x01),
                op(0x02),
                op(0x03),
                version.clone(),
                ids.iter().map(|&filler| op(filler)),
            )
        };
        let canonical = identity(&[0x10, 0x20, 0x30]);
        assert_eq!(
            canonical.observation_input_set(),
            &[op(0x10), op(0x20), op(0x30)]
        );
        for read_order in [
            &[0x30, 0x10, 0x20][..],
            &[0x20, 0x20, 0x30, 0x10, 0x30],
            &[0x10, 0x30, 0x20, 0x10],
        ] {
            assert_eq!(identity(read_order), canonical, "{read_order:?}");
        }

        let mut incremental = identity(&[]);
        for filler in [0x30, 0x10, 0x30, 0x20] {
            incremental.record_observation_batch(op(filler));
        }
        assert_eq!(incremental, canonical);

        assert_ne!(
            identity(&[0x10, 0x20]),
            canonical,
            "a batch left out is a different input set"
        );
        assert!(identity(&[]).observation_input_set().is_empty());
    }

    #[test]
    fn analytics_source_identity_from_a_checkpoint_binds_its_tuple() {
        let binding = CheckpointBinding {
            branch_id: branch(0x01),
            frontier_commitment: op(0x02),
            segment_manifest_id: op(0x03),
            materializer_version: MaterializerVersion::new("observation-catalog", 1),
            projection_root_hash: op(0x04),
        };
        assert_eq!(
            AnalyticsSourceIdentity::from_checkpoint(&binding, [op(0x20), op(0x10)]),
            AnalyticsSourceIdentity::new(
                branch(0x01),
                op(0x02),
                op(0x03),
                MaterializerVersion::new("observation-catalog", 1),
                [op(0x10), op(0x20)],
            )
        );
    }

    #[test]
    fn observation_sources_sort_by_canonical_bytes() {
        let mut sources = [decoder_source(), device_source()];
        sources.sort();
        let encoded: Vec<Vec<u8>> = sources
            .iter()
            .map(|source| source.encode_canonical().expect("bytes"))
            .collect();
        assert!(encoded.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(sources[0].source_class(), SourceClass::Device);
    }
}
