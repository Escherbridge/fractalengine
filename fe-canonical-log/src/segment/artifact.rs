//! Sealed stored artifacts: outer form, `artifact_id`, and per-lane AAD (SPEC-6 §2, §9.1).

use std::collections::{BTreeSet, VecDeque};
use std::num::NonZeroUsize;

use crate::capability::verbs::ObjectClass;
use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{EncryptionParams, Hash32, Identifier32, NONCE_LENGTH};
use crate::signing::domain_preimage;

use super::hashseq::LaneKey;
use super::payload_shard::{PayloadTopicScope, SegmentSizeCaveat};
use super::relay_policy::{assert_seals_under_current_key, RelayAuthorizationView};
use super::{
    assert_canonical_bytes, byte_string_at, map_entries, require_uint_keys, uint_at, value_at,
    SegmentError,
};

/// Format version of the SPEC-6 sealed outer map.
pub const ARTIFACT_FORMAT_VERSION: u64 = 1;

/// AAD domain separator for a sealed header segment (§9.1); provisional, see `segment/AGENTS.md`.
pub const HEADER_SEGMENT_AAD_DOMAIN: &[u8] = b"fe-segment-header-v1\0";
/// AAD domain separator for a sealed payload shard (§9.1); provisional.
pub const PAYLOAD_SHARD_AAD_DOMAIN: &[u8] = b"fe-segment-payload-shard-v1\0";
/// AAD domain separator for a sealed HashSeq node (§9.1); provisional.
pub const HASHSEQ_NODE_AAD_DOMAIN: &[u8] = b"fe-segment-hashseq-v1\0";
/// AAD domain separator for a sealed segment manifest (§9.1); provisional.
pub const SEGMENT_MANIFEST_AAD_DOMAIN: &[u8] = b"fe-segment-manifest-v1\0";
/// AAD domain separator for a sealed bulk artifact (§3.4, §9.1); provisional (D-CL34).
pub const BULK_ARTIFACT_AAD_DOMAIN: &[u8] = b"fe-segment-bulk-artifact-v1\0";

/// The five SPEC-6 lane classes a sealed artifact may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LaneClass {
    /// Verse-wide complete SPEC-1 headers, payload-free (§3.1).
    HeaderSegment,
    /// Payload ciphertext for exactly one payload-topic scope (§3.2).
    PayloadShard,
    /// A link in an immutable delivery sequence (§4.1).
    HashSeqNode,
    /// The sealed index binding HashSeq roots for one branch (§3.3).
    SegmentManifest,
    /// One independently addressable shard, tile, or asset (§3.4, erratum G3).
    BulkArtifact,
}

impl LaneClass {
    /// Provisional wire encoding of the lane class.
    pub const fn to_wire(self) -> u64 {
        match self {
            Self::HeaderSegment => 1,
            Self::PayloadShard => 2,
            Self::HashSeqNode => 3,
            Self::SegmentManifest => 4,
            Self::BulkArtifact => 5,
        }
    }

    /// Decodes the provisional wire lane class, rejecting anything outside the five lanes.
    pub fn from_wire(lane_class: u64) -> Result<Self, SegmentError> {
        match lane_class {
            1 => Ok(Self::HeaderSegment),
            2 => Ok(Self::PayloadShard),
            3 => Ok(Self::HashSeqNode),
            4 => Ok(Self::SegmentManifest),
            5 => Ok(Self::BulkArtifact),
            other => Err(SegmentError::UnknownLaneClass { lane_class: other }),
        }
    }

    /// The domain-separated AAD prefix this lane's authenticated encryption binds (§9.1).
    pub const fn aad_domain(self) -> &'static [u8] {
        match self {
            Self::HeaderSegment => HEADER_SEGMENT_AAD_DOMAIN,
            Self::PayloadShard => PAYLOAD_SHARD_AAD_DOMAIN,
            Self::HashSeqNode => HASHSEQ_NODE_AAD_DOMAIN,
            Self::SegmentManifest => SEGMENT_MANIFEST_AAD_DOMAIN,
            Self::BulkArtifact => BULK_ARTIFACT_AAD_DOMAIN,
        }
    }
}

/// The §2.1.3 encryption descriptor: the SPEC-1 §3.5 parameters reused without a second codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EncryptionDescriptor(
    /// Suite, scope key identifier, and fresh 24-byte nonce.
    pub EncryptionParams,
);

impl EncryptionDescriptor {
    /// Builds a descriptor for the current scope key and a fresh nonce.
    pub const fn new(suite_id: u16, key_id: Identifier32, nonce: [u8; NONCE_LENGTH]) -> Self {
        Self(EncryptionParams {
            suite_id,
            key_id,
            nonce,
        })
    }

    /// The underlying SPEC-1 §3.5 parameters.
    pub const fn params(&self) -> &EncryptionParams {
        &self.0
    }

    /// Scope key identifier this artifact was sealed under.
    pub const fn key_id(&self) -> Identifier32 {
        self.0.key_id
    }

    /// Rejects every suite other than the v1 production suite, fixture suite 65535 included.
    pub fn assert_production_suite(&self) -> Result<(), SegmentError> {
        Ok(self.0.assert_production_suite()?)
    }

    /// Rejects a descriptor sealed under a key the authorization view has already superseded.
    pub fn assert_current_key(&self, current_key_id: Identifier32) -> Result<(), SegmentError> {
        if self.0.key_id != current_key_id {
            return Err(SegmentError::StaleScopeKey {
                key_id: self.0.key_id,
            });
        }
        Ok(())
    }

    /// Encodes the descriptor map.
    pub fn to_cbor(&self) -> CborValue {
        self.0.to_cbor()
    }

    /// Decodes the descriptor map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        Ok(Self(EncryptionParams::from_cbor(value)?))
    }
}

/// A sealed stored artifact: the exact byte sequence retained, fetched, or seeded (§2.1).
///
/// Constructible only through [`seal_artifact`]; see `src/segment/AGENTS.md` §sealing.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SealedArtifact {
    /// Outer format version; always [`ARTIFACT_FORMAT_VERSION`].
    pub format_version: u64,
    /// Which lane the decrypted body belongs to.
    pub lane_class: LaneClass,
    /// Declared ciphertext length, cross-checked against the ciphertext actually carried.
    pub ciphertext_length: u64,
    /// Suite, scope key identifier, and nonce.
    pub encryption: EncryptionDescriptor,
    /// Opaque sealed body; this crate never decrypts it.
    pub ciphertext: Vec<u8>,
}

const SEALED_CONTEXT: &str = "sealed artifact";
const SEALED_KEYS: &[u64] = &[0, 1, 2, 3, 4];

impl SealedArtifact {
    /// Wraps an already-encrypted body, rejecting the plaintext (empty ciphertext) form.
    ///
    /// Deliberately `pub(crate)`: [`seal_artifact`] is the only entry point a caller outside this
    /// crate may reach, so no future Wave-3 sealing path can skip the §9.1/§9.2 checks it runs.
    pub(crate) fn seal(
        lane_class: LaneClass,
        encryption: EncryptionDescriptor,
        ciphertext: Vec<u8>,
    ) -> Result<Self, SegmentError> {
        let sealed = Self {
            format_version: ARTIFACT_FORMAT_VERSION,
            lane_class,
            ciphertext_length: ciphertext.len() as u64,
            encryption,
            ciphertext,
        };
        sealed.validate()?;
        Ok(sealed)
    }

    /// Enforces §2.1.4 length agreement, §2.2.1 non-empty ciphertext, and the format version.
    pub fn validate(&self) -> Result<(), SegmentError> {
        if self.format_version != ARTIFACT_FORMAT_VERSION {
            return Err(SegmentError::UnsupportedArtifactFormatVersion {
                version: self.format_version,
            });
        }
        if self.ciphertext.is_empty() {
            return Err(SegmentError::EmptyCiphertext);
        }
        if self.ciphertext_length != self.ciphertext.len() as u64 {
            return Err(SegmentError::CiphertextLengthMismatch {
                declared: self.ciphertext_length,
                actual: self.ciphertext.len() as u64,
            });
        }
        Ok(())
    }

    /// The §2.1.2 outer metadata map, ciphertext excluded; this is what the AAD binds.
    fn metadata_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.format_version)),
            (
                CborValue::Uint(1),
                CborValue::Uint(self.lane_class.to_wire()),
            ),
            (CborValue::Uint(2), CborValue::Uint(self.ciphertext_length)),
            (CborValue::Uint(3), self.encryption.to_cbor()),
        ])
    }

    /// Encodes the complete outer map.
    pub fn to_cbor(&self) -> Result<CborValue, SegmentError> {
        self.validate()?;
        let CborValue::Map(mut entries) = self.metadata_cbor() else {
            unreachable!("metadata_cbor always builds a map")
        };
        entries.push((
            CborValue::Uint(4),
            CborValue::Bytes(self.ciphertext.clone()),
        ));
        Ok(CborValue::Map(entries))
    }

    /// The exact stored bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// `artifact_id = BLAKE3(stored_artifact_bytes)`, descriptor and ciphertext included (§2.1.4).
    pub fn artifact_id(&self) -> Result<Hash32, SegmentError> {
        Ok(Hash32::of(&self.encode_canonical()?))
    }

    /// The §9.1 domain-separated AAD binding this artifact's canonical outer metadata.
    pub fn associated_data(&self) -> Result<Vec<u8>, SegmentError> {
        self.validate()?;
        let metadata = encode_canonical_checked(&self.metadata_cbor())?;
        Ok(domain_preimage(self.lane_class.aad_domain(), &metadata))
    }

    /// Decodes the outer map, refusing unknown fields and a non-canonical re-encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        require_uint_keys(&value, SEALED_KEYS, SEALED_CONTEXT)?;
        let sealed = Self {
            format_version: uint_at(&value, 0, SEALED_CONTEXT)?,
            lane_class: LaneClass::from_wire(uint_at(&value, 1, SEALED_CONTEXT)?)?,
            ciphertext_length: uint_at(&value, 2, SEALED_CONTEXT)?,
            encryption: EncryptionDescriptor::from_cbor(value_at(&value, 3, SEALED_CONTEXT)?)?,
            ciphertext: byte_string_at(&value, 4, SEALED_CONTEXT)?,
        };
        sealed.validate()?;
        assert_canonical_bytes(&value, bytes, SEALED_CONTEXT)?;
        Ok(sealed)
    }
}

/// Recomputes BLAKE3 over the received bytes and refuses a claimed ID that does not match.
pub fn verify_artifact_id(bytes: &[u8], claimed: Hash32) -> Result<Hash32, SegmentError> {
    let computed = Hash32::of(bytes);
    if computed != claimed {
        return Err(SegmentError::ArtifactIdMismatch { claimed, computed });
    }
    Ok(computed)
}

/// Validates the sealed outer form of received bytes before any decryption (§5.1, §5.3).
///
/// Refuses fixture suite 65535 unconditionally; the rejection is a runtime check, never a
/// `#[cfg]`, so no build configuration can compile it away.
pub fn admit_sealed(bytes: &[u8]) -> Result<(SealedArtifact, Hash32), SegmentError> {
    let sealed = SealedArtifact::decode_canonical(bytes)?;
    sealed.encryption.assert_production_suite()?;
    Ok((sealed, Hash32::of(bytes)))
}

/// One lane's sealing request: which lane, which epoch, which body form, and the sealed bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealRequest<'a> {
    /// The lane the artifact is sealed into.
    pub lane: &'a LaneKey,
    /// The scope epoch the caller believes is current for that lane.
    pub scope_epoch: u64,
    /// Which of the five §2.1/§3.4 body forms the ciphertext holds.
    pub lane_class: LaneClass,
    /// Suite, scope key identifier, and the fresh 24-byte nonce.
    pub descriptor: EncryptionDescriptor,
    /// The already-encrypted body; this crate performs no AEAD.
    pub ciphertext: Vec<u8>,
}

impl SealRequest<'_> {
    /// Refuses a lane class the requested lane cannot carry (§4.1.3).
    ///
    /// A header lane carries verse-wide header segments, its own HashSeq nodes, and the branch
    /// manifest; a payload lane carries petal-affine shards, its own HashSeq nodes, and bulk
    /// artifacts (§3.4 rule 3: a verse-wide bulk artifact is not representable).
    ///
    /// A `matches!`, not an exhaustive `match`: a new lane class compiles here without being
    /// permitted anywhere, so every lane a class may ride is listed explicitly.
    fn assert_lane_carries_class(&self) -> Result<(), SegmentError> {
        let permitted = match self.lane {
            LaneKey::Header { .. } => matches!(
                self.lane_class,
                LaneClass::HeaderSegment | LaneClass::HashSeqNode | LaneClass::SegmentManifest
            ),
            LaneKey::Payload(_) => {
                matches!(
                    self.lane_class,
                    LaneClass::PayloadShard | LaneClass::HashSeqNode | LaneClass::BulkArtifact
                )
            }
        };
        if permitted {
            Ok(())
        } else {
            Err(SegmentError::SealLaneMismatch {
                lane_class: self.lane_class.to_wire(),
            })
        }
    }
}

/// The ONE seal entry point for all five SPEC-6 lanes (§9.1, §9.2, §3.4 rule 4).
///
/// In order it refuses a lane/body mismatch, then the production suite, the lane's current scope
/// epoch and the lane's current key through [`assert_seals_under_current_key`], then a nonce
/// already seen under that key through [`NonceLedger::record_fresh`], and only then builds the
/// artifact. Nonce reuse under XChaCha20-Poly1305 is a keystream-recovery break and sealing under
/// a retired epoch key defeats D-CL17 revocation, so neither check may be reachable only by
/// opt-in: `SealedArtifact::seal` is `pub(crate)` precisely so this is the only door.
pub fn seal_artifact(
    view: &impl RelayAuthorizationView,
    nonce_ledger: &mut NonceLedger,
    request: SealRequest<'_>,
) -> Result<SealedArtifact, SegmentError> {
    request.assert_lane_carries_class()?;
    assert_seals_under_current_key(view, request.lane, request.scope_epoch, &request.descriptor)?;
    nonce_ledger.record_fresh(&request.descriptor)?;
    SealedArtifact::seal(request.lane_class, request.descriptor, request.ciphertext)
}

/// A bounded record of `(key_id, nonce)` pairs already used, guarding XChaCha nonce reuse.
///
/// Capacity is a caller policy number under D-CL24; there is deliberately no default. The
/// ledger is a local guard, not a global proof: a pair evicted by the bound is no longer seen.
#[derive(Clone, Debug)]
pub struct NonceLedger {
    capacity: usize,
    order: VecDeque<(Identifier32, [u8; NONCE_LENGTH])>,
    seen: BTreeSet<(Identifier32, [u8; NONCE_LENGTH])>,
}

impl NonceLedger {
    /// Builds a ledger remembering at most `capacity` most-recent pairs.
    pub fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            capacity: capacity.get(),
            order: VecDeque::new(),
            seen: BTreeSet::new(),
        }
    }

    /// Records a descriptor's nonce, refusing a reuse under the same key identifier.
    pub fn record_fresh(&mut self, descriptor: &EncryptionDescriptor) -> Result<(), SegmentError> {
        let pair = (descriptor.key_id(), descriptor.params().nonce);
        if !self.seen.insert(pair) {
            return Err(SegmentError::NonceReuse {
                key_id: descriptor.key_id(),
            });
        }
        self.order.push_back(pair);
        if self.order.len() > self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.seen.remove(&evicted);
            }
        }
        Ok(())
    }
}

/// Whether an object class may ride the bulk-artifact lane: exactly `shard`, `tile`, `asset`.
pub const fn is_bulk_object_class(object_class: ObjectClass) -> bool {
    matches!(
        object_class,
        ObjectClass::Shard | ObjectClass::Tile | ObjectClass::Asset
    )
}

/// The decrypted body of a bulk artifact: one immutable shard, tile, or asset (§3.4, G3).
///
/// Its scope is a [`PayloadTopicScope`], so a verse-wide bulk artifact is unrepresentable
/// (§3.4 rule 3). `content_bytes` is opaque here; its interior is validated at
/// `materialize/<class>` by the spec whose operation commits it (§3.4 rule 8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkArtifactBody {
    topic: PayloadTopicScope,
    object_class: ObjectClass,
    content: Vec<u8>,
}

const BULK_BODY_CONTEXT: &str = "bulk artifact body";
const BULK_BODY_KEYS: &[u64] = &[0, 1, 2, 3, 4];

impl BulkArtifactBody {
    /// Builds a body, refusing any object class other than shard, tile, or asset (§3.4 rule 2).
    pub fn new(
        topic: PayloadTopicScope,
        object_class: ObjectClass,
        content: Vec<u8>,
    ) -> Result<Self, SegmentError> {
        if !is_bulk_object_class(object_class) {
            return Err(SegmentError::BulkObjectClassRefused {
                object_class: u64::from(object_class.bit()),
            });
        }
        Ok(Self {
            topic,
            object_class,
            content,
        })
    }

    /// The one petal-affine payload-topic scope this artifact is sealed under.
    pub fn topic(&self) -> PayloadTopicScope {
        self.topic
    }

    /// The lane this artifact seals into and is discovered on: always a payload lane.
    pub fn lane_key(&self) -> LaneKey {
        LaneKey::Payload(self.topic)
    }

    /// The §3.2 class of the enclosed object.
    pub fn object_class(&self) -> ObjectClass {
        self.object_class
    }

    /// The enclosed object's bytes.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Refuses a body whose topic does not name the key the artifact was sealed under.
    ///
    /// §3.4 rule 3: one artifact MUST NOT mix key identifiers. Without this a holder of petal Y's
    /// key could seal a body that claims petal X, and a consumer trusting `topic()` would file
    /// it under X. Returns [`SegmentError::ForeignLaneArtifact`].
    pub fn assert_sealed_under(
        &self,
        artifact_id: Hash32,
        sealed_key_id: Identifier32,
    ) -> Result<(), SegmentError> {
        if self.topic.key_id != sealed_key_id {
            return Err(SegmentError::ForeignLaneArtifact { artifact_id });
        }
        Ok(())
    }

    /// `BLAKE3(content_bytes)`: the plaintext object's address, which is NOT the `artifact_id`
    /// (§3.4 rule 9). Only an operation's own encrypted payload may name it.
    pub fn content_hash(&self) -> Hash32 {
        Hash32::of(&self.content)
    }

    /// Encodes the inner body: lane class 5, topic, class bit, length, bytes.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::BulkArtifact.to_wire()),
            ),
            (CborValue::Uint(1), self.topic.to_cbor()),
            (
                CborValue::Uint(2),
                CborValue::Uint(u64::from(self.object_class.bit())),
            ),
            (
                CborValue::Uint(3),
                CborValue::Uint(self.content.len() as u64),
            ),
            (CborValue::Uint(4), CborValue::Bytes(self.content.clone())),
        ])
    }

    /// The exact plaintext bytes that get sealed.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes a decrypted body, refusing a lane mismatch, a non-bulk or multi-bit class, and a
    /// `content_length` that disagrees with the bytes carried (§3.4 rule 2).
    ///
    /// The lane restated at key 0 is checked before the key set, so another lane's body is
    /// reported as [`SegmentError::LaneBodyMismatch`] rather than as a missing field.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        map_entries(&value, BULK_BODY_CONTEXT)?;
        if value.get_uint_key(0).and_then(CborValue::as_uint)
            != Some(LaneClass::BulkArtifact.to_wire())
        {
            return Err(SegmentError::LaneBodyMismatch);
        }
        require_uint_keys(&value, BULK_BODY_KEYS, BULK_BODY_CONTEXT)?;
        let topic = PayloadTopicScope::from_cbor(value_at(&value, 1, BULK_BODY_CONTEXT)?)?;
        let class_bits = uint_at(&value, 2, BULK_BODY_CONTEXT)?;
        let object_class = u8::try_from(class_bits)
            .ok()
            .and_then(ObjectClass::from_bit)
            .ok_or(SegmentError::BulkObjectClassRefused {
                object_class: class_bits,
            })?;
        let declared = uint_at(&value, 3, BULK_BODY_CONTEXT)?;
        let content = byte_string_at(&value, 4, BULK_BODY_CONTEXT)?;
        if declared != content.len() as u64 {
            return Err(SegmentError::BulkContentLengthMismatch {
                declared,
                actual: content.len() as u64,
            });
        }
        let body = Self::new(topic, object_class, content)?;
        assert_canonical_bytes(&value, bytes, BULK_BODY_CONTEXT)?;
        Ok(body)
    }
}

/// A bulk artifact body accepted for sealing: its plaintext and projected stored size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulkArtifactPacking {
    /// The canonical plaintext body the AEAD will seal.
    pub plaintext: Vec<u8>,
    /// Projected stored artifact length, sealing overhead included.
    pub projected_stored_length: u64,
}

/// Prepares a bulk artifact for sealing, REJECTING an oversized candidate (§3.4 rule 6).
///
/// Mirrors [`super::payload_shard::seal_payload_shard`]: the lane and epoch come from the body's
/// own topic, the descriptor must name that topic's key, and nothing is truncated. It records no
/// nonce; [`seal_artifact`] is the one place that does, so the seal itself still goes through
/// the single door.
pub fn pack_bulk_artifact(
    view: &impl RelayAuthorizationView,
    body: &BulkArtifactBody,
    caveat: &SegmentSizeCaveat,
    descriptor: &EncryptionDescriptor,
) -> Result<BulkArtifactPacking, SegmentError> {
    let topic = body.topic();
    assert_seals_under_current_key(view, &body.lane_key(), topic.scope_epoch, descriptor)?;
    if descriptor.key_id() != topic.key_id {
        return Err(SegmentError::StaleScopeKey {
            key_id: descriptor.key_id(),
        });
    }
    let plaintext = body.encode_canonical()?;
    let projected_stored_length = plaintext.len() as u64 + caveat.sealed_overhead_bytes;
    if projected_stored_length > caveat.max_segment_bytes {
        return Err(SegmentError::OversizedSegment {
            candidate: projected_stored_length,
            maximum: caveat.max_segment_bytes,
        });
    }
    Ok(BulkArtifactPacking {
        plaintext,
        projected_stored_length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{FIXTURE_ONLY_SUITE_ID, PRODUCTION_SUITE_ID};
    use crate::segment::payload_shard::PayloadTopicScope;
    use crate::segment::relay_policy::PeerIdentity;

    fn key_id(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn descriptor(key: u8, nonce_filler: u8) -> EncryptionDescriptor {
        EncryptionDescriptor::new(
            PRODUCTION_SUITE_ID,
            key_id(key),
            [nonce_filler; NONCE_LENGTH],
        )
    }

    fn sealed(lane: LaneClass, ciphertext: &[u8]) -> SealedArtifact {
        SealedArtifact::seal(lane, descriptor(1, 7), ciphertext.to_vec()).expect("sealed")
    }

    #[test]
    fn segment_id_hashes_exact_stored_ciphertext() {
        let original = sealed(LaneClass::HeaderSegment, b"sealed-body");
        let bytes = original.encode_canonical().expect("bytes");
        let artifact_id = original.artifact_id().expect("id");
        assert_eq!(artifact_id, Hash32::of(&bytes));
        assert_eq!(
            verify_artifact_id(&bytes, artifact_id).expect("verified"),
            artifact_id
        );

        let mut different_lane = original.clone();
        different_lane.lane_class = LaneClass::PayloadShard;
        let mut different_descriptor = original.clone();
        different_descriptor.encryption = descriptor(1, 8);
        let different_ciphertext = sealed(LaneClass::HeaderSegment, b"sealed-bodx");
        let mut different_length = original.clone();
        different_length.ciphertext_length = original.ciphertext_length + 1;

        for variant in [
            different_lane.artifact_id().expect("id"),
            different_descriptor.artifact_id().expect("id"),
            different_ciphertext.artifact_id().expect("id"),
        ] {
            assert_ne!(variant, artifact_id);
        }
        assert_eq!(
            different_length.artifact_id(),
            Err(SegmentError::CiphertextLengthMismatch {
                declared: original.ciphertext_length + 1,
                actual: original.ciphertext_length,
            })
        );
        assert_eq!(
            verify_artifact_id(
                &different_ciphertext.encode_canonical().expect("bytes"),
                artifact_id
            ),
            Err(SegmentError::ArtifactIdMismatch {
                claimed: artifact_id,
                computed: different_ciphertext.artifact_id().expect("id"),
            })
        );
    }

    #[test]
    fn uniform_encryption_has_no_plaintext_segment_fallback() {
        for lane in [
            LaneClass::HeaderSegment,
            LaneClass::PayloadShard,
            LaneClass::HashSeqNode,
            LaneClass::SegmentManifest,
            LaneClass::BulkArtifact,
        ] {
            assert_eq!(
                SealedArtifact::seal(lane, descriptor(1, 7), Vec::new()),
                Err(SegmentError::EmptyCiphertext)
            );
        }

        let plaintext_outer = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(ARTIFACT_FORMAT_VERSION)),
            (
                CborValue::Uint(1),
                CborValue::Uint(LaneClass::HeaderSegment.to_wire()),
            ),
            (CborValue::Uint(2), CborValue::Uint(4)),
            (CborValue::Uint(4), CborValue::Bytes(b"body".to_vec())),
        ]);
        let bytes = encode_canonical_checked(&plaintext_outer).expect("bytes");
        assert_eq!(
            admit_sealed(&bytes),
            Err(SegmentError::MissingField {
                context: SEALED_CONTEXT,
                key: 3,
            })
        );
    }

    #[test]
    fn segment_uses_scope_key_and_fresh_xchacha_nonce() {
        let current = key_id(1);
        let artifact = sealed(LaneClass::PayloadShard, b"sealed-body");
        let bytes = artifact.encode_canonical().expect("bytes");
        let (admitted, artifact_id) = admit_sealed(&bytes).expect("admitted");
        assert_eq!(artifact_id, Hash32::of(&bytes));
        assert_eq!(admitted.encryption.params().suite_id, PRODUCTION_SUITE_ID);
        assert_eq!(admitted.encryption.params().nonce.len(), NONCE_LENGTH);
        admitted
            .encryption
            .assert_current_key(current)
            .expect("current key");

        let fixture = SealedArtifact::seal(
            LaneClass::PayloadShard,
            EncryptionDescriptor::new(FIXTURE_ONLY_SUITE_ID, current, [7; NONCE_LENGTH]),
            b"sealed-body".to_vec(),
        )
        .expect("sealed");
        assert_eq!(
            admit_sealed(&fixture.encode_canonical().expect("bytes")),
            Err(SegmentError::Envelope(
                crate::envelope::EnvelopeError::FixtureOnlySuite
            ))
        );

        let superseded = key_id(2);
        assert_eq!(
            admitted.encryption.assert_current_key(superseded),
            Err(SegmentError::StaleScopeKey { key_id: current })
        );

        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(8).expect("capacity"));
        ledger.record_fresh(&descriptor(1, 7)).expect("fresh");
        ledger.record_fresh(&descriptor(1, 8)).expect("fresh");
        ledger.record_fresh(&descriptor(2, 7)).expect("other key");
        assert_eq!(
            ledger.record_fresh(&descriptor(1, 7)),
            Err(SegmentError::NonceReuse { key_id: current })
        );
    }

    #[test]
    fn associated_data_separates_lanes_and_binds_outer_metadata() {
        let header = sealed(LaneClass::HeaderSegment, b"sealed-body");
        let shard = sealed(LaneClass::PayloadShard, b"sealed-body");
        assert_ne!(
            header.associated_data().expect("aad"),
            shard.associated_data().expect("aad")
        );
        assert!(header
            .associated_data()
            .expect("aad")
            .starts_with(HEADER_SEGMENT_AAD_DOMAIN));

        let mut rekeyed = header.clone();
        rekeyed.encryption = descriptor(3, 7);
        assert_ne!(
            header.associated_data().expect("aad"),
            rekeyed.associated_data().expect("aad")
        );
        assert!(!header
            .associated_data()
            .expect("aad")
            .ends_with(b"sealed-body"));
    }

    /// A persistent view holding one epoch and one current key for every lane.
    struct Lane {
        epoch: u64,
        key_id: Identifier32,
    }

    impl RelayAuthorizationView for Lane {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(self.epoch)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(self.key_id)
        }

        fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }

        fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }

        fn may_wrap_scope_key_for_peer(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }
    }

    fn header_lane() -> LaneKey {
        LaneKey::Header {
            verse_id: key_id(0x11),
        }
    }

    fn payload_lane() -> LaneKey {
        LaneKey::Payload(PayloadTopicScope {
            verse_id: key_id(0x11),
            petal_id: key_id(0x21),
            scope_epoch: 7,
            key_id: key_id(1),
        })
    }

    fn request<'a>(
        lane: &'a LaneKey,
        lane_class: LaneClass,
        descriptor: EncryptionDescriptor,
    ) -> SealRequest<'a> {
        SealRequest {
            lane,
            scope_epoch: 7,
            lane_class,
            descriptor,
            ciphertext: b"sealed-body".to_vec(),
        }
    }

    #[test]
    fn every_lane_seals_through_one_epoch_checked_nonce_checked_entry_point() {
        let view = Lane {
            epoch: 7,
            key_id: key_id(1),
        };
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));
        let header = header_lane();
        let payload = payload_lane();

        for (lane, lane_class, nonce) in [
            (&header, LaneClass::HeaderSegment, 1u8),
            (&header, LaneClass::HashSeqNode, 2),
            (&header, LaneClass::SegmentManifest, 3),
            (&payload, LaneClass::PayloadShard, 4),
            (&payload, LaneClass::HashSeqNode, 5),
        ] {
            let sealed = seal_artifact(
                &view,
                &mut ledger,
                request(lane, lane_class, descriptor(1, nonce)),
            )
            .expect("the current epoch and a fresh nonce seal");
            assert_eq!(sealed.lane_class, lane_class);
        }

        // A body form the lane cannot carry never reaches `SealedArtifact::seal`.
        assert_eq!(
            seal_artifact(
                &view,
                &mut ledger,
                request(&payload, LaneClass::SegmentManifest, descriptor(1, 6)),
            ),
            Err(SegmentError::SealLaneMismatch {
                lane_class: LaneClass::SegmentManifest.to_wire(),
            })
        );
        assert_eq!(
            seal_artifact(
                &view,
                &mut ledger,
                request(&header, LaneClass::PayloadShard, descriptor(1, 7)),
            ),
            Err(SegmentError::SealLaneMismatch {
                lane_class: LaneClass::PayloadShard.to_wire(),
            })
        );
    }

    #[test]
    fn the_seal_entry_point_refuses_a_stale_epoch_stale_key_reused_nonce_or_fixture_suite() {
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));
        let header = header_lane();
        let bumped = Lane {
            epoch: 8,
            key_id: key_id(2),
        };

        assert_eq!(
            seal_artifact(
                &bumped,
                &mut ledger,
                request(&header, LaneClass::HeaderSegment, descriptor(2, 1)),
            ),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            }),
            "a retired epoch never seals, which is the whole D-CL17 revocation story"
        );

        let mut at_current_epoch = request(&header, LaneClass::HeaderSegment, descriptor(1, 1));
        at_current_epoch.scope_epoch = 8;
        assert_eq!(
            seal_artifact(&bumped, &mut ledger, at_current_epoch),
            Err(SegmentError::StaleScopeKey { key_id: key_id(1) }),
            "the current epoch under a superseded key is still refused"
        );

        let current = Lane {
            epoch: 7,
            key_id: key_id(1),
        };
        let fixture =
            EncryptionDescriptor::new(FIXTURE_ONLY_SUITE_ID, key_id(1), [9; NONCE_LENGTH]);
        assert_eq!(
            seal_artifact(
                &current,
                &mut ledger,
                request(&header, LaneClass::HeaderSegment, fixture),
            ),
            Err(SegmentError::Envelope(
                crate::envelope::EnvelopeError::FixtureOnlySuite
            ))
        );

        seal_artifact(
            &current,
            &mut ledger,
            request(&header, LaneClass::HeaderSegment, descriptor(1, 4)),
        )
        .expect("first use of this nonce");
        assert_eq!(
            seal_artifact(
                &current,
                &mut ledger,
                request(&header, LaneClass::HashSeqNode, descriptor(1, 4)),
            ),
            Err(SegmentError::NonceReuse { key_id: key_id(1) }),
            "one ledger spans every lane, so a nonce cannot be replayed across lanes"
        );
    }

    #[test]
    fn decode_rejects_unknown_fields_and_unknown_lanes() {
        let artifact = sealed(LaneClass::HashSeqNode, b"sealed-body");
        let CborValue::Map(mut entries) = artifact.to_cbor().expect("cbor") else {
            unreachable!()
        };
        entries.push((CborValue::Uint(9), CborValue::Uint(0)));
        let with_extra = encode_canonical_checked(&CborValue::Map(entries)).expect("bytes");
        assert_eq!(
            SealedArtifact::decode_canonical(&with_extra),
            Err(SegmentError::UnknownField {
                context: SEALED_CONTEXT
            })
        );

        assert_eq!(LaneClass::from_wire(5), Ok(LaneClass::BulkArtifact));
        for unknown in [0, 6, u64::MAX] {
            assert_eq!(
                LaneClass::from_wire(unknown),
                Err(SegmentError::UnknownLaneClass {
                    lane_class: unknown
                })
            );
        }
    }

    // ---- G3 bulk-artifact lane (SPEC-6 §3.4, conformance tests 18-22) ----

    use crate::capability::verbs::ObjectClass;
    use crate::crypto::aead::{self, FreshNonce, SealingKey, SEALING_KEY_LENGTH};
    use crate::crypto::artifact_aad::StoredArtifactAad;
    use crate::segment::payload_shard::SegmentSizeCaveat;
    use crate::segment::receipt::{decode_opened_body, OpenedBody};

    const BULK_CLASSES: [ObjectClass; 3] =
        [ObjectClass::Shard, ObjectClass::Tile, ObjectClass::Asset];

    fn bulk_topic() -> PayloadTopicScope {
        PayloadTopicScope {
            verse_id: key_id(0x11),
            petal_id: key_id(0x21),
            scope_epoch: 7,
            key_id: key_id(1),
        }
    }

    fn bulk_body(object_class: ObjectClass, content: &[u8]) -> BulkArtifactBody {
        BulkArtifactBody::new(bulk_topic(), object_class, content.to_vec()).expect("bulk body")
    }

    fn generous_caveat() -> SegmentSizeCaveat {
        SegmentSizeCaveat::new(1 << 20, 64)
    }

    #[test]
    fn bulk_artifact_seals_through_single_door_and_round_trips() {
        let view = Lane {
            epoch: 7,
            key_id: key_id(1),
        };
        let scope_key = SealingKey::from_bytes([0x5a; SEALING_KEY_LENGTH]);
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));

        for object_class in BULK_CLASSES {
            let body = bulk_body(object_class, b"observation hexon bytes");
            let nonce = FreshNonce::draw(key_id(1)).expect("nonce");
            let params = nonce.params();
            let bulk_descriptor = EncryptionDescriptor(params);
            let packing = pack_bulk_artifact(&view, &body, &generous_caveat(), &bulk_descriptor)
                .expect("fits the caveat under the current key");
            let aad = StoredArtifactAad::bulk_artifact(packing.plaintext.len(), params)
                .preimage()
                .expect("aad");
            let sealed_payload =
                aead::seal(&scope_key, nonce, &aad, &packing.plaintext).expect("aead seal");
            let lane = body.lane_key();
            let sealed = seal_artifact(
                &view,
                &mut ledger,
                SealRequest {
                    lane: &lane,
                    scope_epoch: 7,
                    lane_class: LaneClass::BulkArtifact,
                    descriptor: bulk_descriptor,
                    ciphertext: sealed_payload.ciphertext,
                },
            )
            .expect("the single door seals a bulk artifact");

            let stored = sealed.encode_canonical().expect("stored bytes");
            let artifact_id = sealed.artifact_id().expect("id");
            assert_eq!(artifact_id, Hash32::of(&stored), "{object_class:?}");
            assert_ne!(
                artifact_id,
                body.content_hash(),
                "the content address is never the artifact id (§3.4 rule 9)"
            );
            assert!(sealed
                .associated_data()
                .expect("aad")
                .starts_with(BULK_ARTIFACT_AAD_DOMAIN));

            let (admitted, admitted_id) = admit_sealed(&stored).expect("admitted");
            assert_eq!(admitted_id, artifact_id);
            assert_eq!(admitted.lane_class, LaneClass::BulkArtifact);
            let plaintext = aead::open(
                &scope_key,
                admitted.encryption.params(),
                &admitted.associated_data().expect("aad"),
                &admitted.ciphertext,
            )
            .expect("authenticated open");
            assert_eq!(
                decode_opened_body(admitted.lane_class, &plaintext).expect("decoded"),
                OpenedBody::BulkArtifact(body.clone())
            );
            assert_eq!(
                BulkArtifactBody::decode_canonical(&plaintext).expect("decoded"),
                body
            );
        }

        let body = bulk_body(ObjectClass::Shard, b"observation hexon bytes");
        assert!(matches!(
            pack_bulk_artifact(
                &view,
                &body,
                &SegmentSizeCaveat::new(16, 64),
                &descriptor(1, 9)
            ),
            Err(SegmentError::OversizedSegment { .. })
        ));
        assert_eq!(
            pack_bulk_artifact(&view, &body, &generous_caveat(), &descriptor(2, 9)),
            Err(SegmentError::StaleScopeKey { key_id: key_id(2) }),
            "the descriptor must name the body's own topic key"
        );
    }

    #[test]
    fn bulk_artifact_refused_under_retired_epoch_key() {
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));
        let payload = payload_lane();
        let bumped = Lane {
            epoch: 8,
            key_id: key_id(2),
        };

        assert_eq!(
            seal_artifact(
                &bumped,
                &mut ledger,
                request(&payload, LaneClass::BulkArtifact, descriptor(1, 1)),
            ),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            }),
            "lane 5 gets the epoch gate with no opt-in"
        );

        let mut at_current_epoch = request(&payload, LaneClass::BulkArtifact, descriptor(1, 2));
        at_current_epoch.scope_epoch = 8;
        assert_eq!(
            seal_artifact(&bumped, &mut ledger, at_current_epoch),
            Err(SegmentError::StaleScopeKey { key_id: key_id(1) }),
            "the current epoch under the superseded key is still refused"
        );

        assert_eq!(
            pack_bulk_artifact(
                &bumped,
                &bulk_body(ObjectClass::Tile, b"tile"),
                &generous_caveat(),
                &descriptor(1, 3)
            ),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            })
        );
    }

    #[test]
    fn bulk_artifact_nonce_reuse_refused() {
        let view = Lane {
            epoch: 7,
            key_id: key_id(1),
        };
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));
        let payload = payload_lane();

        seal_artifact(
            &view,
            &mut ledger,
            request(&payload, LaneClass::BulkArtifact, descriptor(1, 4)),
        )
        .expect("first use of this nonce");
        assert_eq!(
            seal_artifact(
                &view,
                &mut ledger,
                request(&payload, LaneClass::BulkArtifact, descriptor(1, 4)),
            ),
            Err(SegmentError::NonceReuse { key_id: key_id(1) })
        );
        assert_eq!(
            seal_artifact(
                &view,
                &mut ledger,
                request(&payload, LaneClass::PayloadShard, descriptor(1, 4)),
            ),
            Err(SegmentError::NonceReuse { key_id: key_id(1) }),
            "the ledger spans lane 5 and the shard lane alike"
        );
    }

    #[test]
    fn bulk_lane_key_permits_bulk_artifact() {
        let view = Lane {
            epoch: 7,
            key_id: key_id(1),
        };
        let mut ledger = NonceLedger::with_capacity(NonZeroUsize::new(16).expect("capacity"));
        let sealed = seal_artifact(
            &view,
            &mut ledger,
            request(&payload_lane(), LaneClass::BulkArtifact, descriptor(1, 5)),
        )
        .expect("a payload lane carries a bulk artifact");
        assert_eq!(sealed.lane_class, LaneClass::BulkArtifact);

        assert_eq!(
            seal_artifact(
                &view,
                &mut ledger,
                request(&header_lane(), LaneClass::BulkArtifact, descriptor(1, 6)),
            ),
            Err(SegmentError::SealLaneMismatch {
                lane_class: LaneClass::BulkArtifact.to_wire(),
            }),
            "a verse-wide bulk artifact is not representable (§3.4 rule 3)"
        );
    }

    /// A bulk body map with arbitrary class bits and declared length, for the refusal table.
    fn raw_bulk_body(class_bits: u64, declared_length: u64, content: &[u8]) -> Vec<u8> {
        encode_canonical_checked(&CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::BulkArtifact.to_wire()),
            ),
            (CborValue::Uint(1), bulk_topic().to_cbor()),
            (CborValue::Uint(2), CborValue::Uint(class_bits)),
            (CborValue::Uint(3), CborValue::Uint(declared_length)),
            (CborValue::Uint(4), CborValue::Bytes(content.to_vec())),
        ]))
        .expect("bytes")
    }

    #[test]
    fn bulk_artifact_object_class_table() {
        let content = b"object";
        let length = content.len() as u64;
        let cases: &[(u64, Option<ObjectClass>)] = &[
            (0x08, Some(ObjectClass::Shard)),
            (0x10, Some(ObjectClass::Tile)),
            (0x20, Some(ObjectClass::Asset)),
            (0x01, None),
            (0x02, None),
            (0x04, None),
            (0x18, None),
            (0x28, None),
            (0x00, None),
            (0x40, None),
            (0x108, None),
        ];
        for &(class_bits, expected) in cases {
            let decoded =
                BulkArtifactBody::decode_canonical(&raw_bulk_body(class_bits, length, content));
            match expected {
                Some(object_class) => {
                    let body = decoded.expect("a bulk class decodes");
                    assert_eq!(body.object_class(), object_class);
                    assert_eq!(body.content(), content);
                }
                None => assert_eq!(
                    decoded,
                    Err(SegmentError::BulkObjectClassRefused {
                        object_class: class_bits
                    }),
                    "class bits {class_bits:#x}"
                ),
            }
        }

        for refused in [
            ObjectClass::Operation,
            ObjectClass::Segment,
            ObjectClass::Checkpoint,
        ] {
            assert_eq!(
                BulkArtifactBody::new(bulk_topic(), refused, content.to_vec()),
                Err(SegmentError::BulkObjectClassRefused {
                    object_class: u64::from(refused.bit())
                })
            );
        }

        assert_eq!(
            BulkArtifactBody::decode_canonical(&raw_bulk_body(0x08, length + 1, content)),
            Err(SegmentError::BulkContentLengthMismatch {
                declared: length + 1,
                actual: length,
            })
        );
    }
}
