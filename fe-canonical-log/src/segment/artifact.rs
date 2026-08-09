//! Sealed stored artifacts: outer form, `artifact_id`, and per-lane AAD (SPEC-6 §2, §9.1).

use std::collections::{BTreeSet, VecDeque};
use std::num::NonZeroUsize;

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{EncryptionParams, Hash32, Identifier32, NONCE_LENGTH};
use crate::signing::domain_preimage;

use super::hashseq::LaneKey;
use super::relay_policy::{assert_seals_under_current_key, RelayAuthorizationView};
use super::{
    assert_canonical_bytes, byte_string_at, require_uint_keys, uint_at, value_at, SegmentError,
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

/// The four SPEC-6 lane classes a sealed artifact may carry.
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
}

impl LaneClass {
    /// Provisional wire encoding of the lane class.
    pub const fn to_wire(self) -> u64 {
        match self {
            Self::HeaderSegment => 1,
            Self::PayloadShard => 2,
            Self::HashSeqNode => 3,
            Self::SegmentManifest => 4,
        }
    }

    /// Decodes the provisional wire lane class, rejecting anything outside the four lanes.
    pub fn from_wire(lane_class: u64) -> Result<Self, SegmentError> {
        match lane_class {
            1 => Ok(Self::HeaderSegment),
            2 => Ok(Self::PayloadShard),
            3 => Ok(Self::HashSeqNode),
            4 => Ok(Self::SegmentManifest),
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
    /// Which of the four §2.1 body forms the ciphertext holds.
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
    /// manifest; a payload lane carries only petal-affine shards and its own HashSeq nodes.
    fn assert_lane_carries_class(&self) -> Result<(), SegmentError> {
        let permitted = match self.lane {
            LaneKey::Header { .. } => matches!(
                self.lane_class,
                LaneClass::HeaderSegment | LaneClass::HashSeqNode | LaneClass::SegmentManifest
            ),
            LaneKey::Payload(_) => {
                matches!(
                    self.lane_class,
                    LaneClass::PayloadShard | LaneClass::HashSeqNode
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

/// The ONE seal entry point for all four SPEC-6 lanes (§9.1, §9.2).
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

        fn may_wrap_scope_key_for_device(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
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

        assert_eq!(
            LaneClass::from_wire(5),
            Err(SegmentError::UnknownLaneClass { lane_class: 5 })
        );
    }
}
