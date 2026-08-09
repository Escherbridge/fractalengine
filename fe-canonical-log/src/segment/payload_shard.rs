//! The petal-affine payload-shard lane and its size caveat (SPEC-6 §3.2, D-CL2).

use std::collections::BTreeSet;

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Identifier32, PayloadRef, Scope};

use super::artifact::{EncryptionDescriptor, LaneClass};
use super::hashseq::LaneKey;
use super::header_lane::{EnvelopeView, HeaderSegmentBody};
use super::relay_policy::{assert_seals_under_current_key, RelayAuthorizationView};
use super::{
    array_at, assert_canonical_bytes, byte_string_at, hash32_at, identifier32_at,
    require_uint_keys, uint_at, value_at, SegmentError,
};

/// The one canonical payload-topic scope a shard is bound to (§3.2.2).
///
/// Petal-affine by construction: there is no verse-wide payload topic, and a shard cannot mix
/// verses, petals, scope epochs, or key identifiers because it carries exactly one of these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PayloadTopicScope {
    /// Verse the petal belongs to.
    pub verse_id: Identifier32,
    /// The one petal this topic covers.
    pub petal_id: Identifier32,
    /// SPEC-3 scope epoch in force for this topic.
    pub scope_epoch: u64,
    /// Identifier of the topic key the shard is sealed under.
    pub key_id: Identifier32,
}

const TOPIC_CONTEXT: &str = "payload topic scope";
const TOPIC_KEYS: &[u64] = &[0, 1, 2, 3];

impl PayloadTopicScope {
    /// The SPEC-1 scope this topic covers; resource records narrow inside it.
    pub fn scope(&self) -> Scope {
        Scope::new(self.verse_id, Some(self.petal_id), None)
            .expect("a petal scope without a resource is always valid")
    }

    /// Reports whether a header scope resolves into this topic's single petal.
    pub fn admits(&self, scope: &Scope) -> bool {
        self.scope().contains(scope)
    }

    /// Encodes the topic map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.verse_id.0.to_vec()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.petal_id.0.to_vec()),
            ),
            (CborValue::Uint(2), CborValue::Uint(self.scope_epoch)),
            (CborValue::Uint(3), CborValue::Bytes(self.key_id.0.to_vec())),
        ])
    }

    /// Decodes the topic map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, TOPIC_KEYS, TOPIC_CONTEXT)?;
        Ok(Self {
            verse_id: identifier32_at(value, 0, TOPIC_CONTEXT)?,
            petal_id: identifier32_at(value, 1, TOPIC_CONTEXT)?,
            scope_epoch: uint_at(value, 2, TOPIC_CONTEXT)?,
            key_id: identifier32_at(value, 3, TOPIC_CONTEXT)?,
        })
    }
}

/// One `(op_id, ciphertext_hash, ciphertext_length, ciphertext_bytes)` record (§3.2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadShardRecord {
    /// Operation whose payload this is.
    pub op_id: Hash32,
    /// `BLAKE3(ciphertext)`, which must equal the header's payload reference.
    pub ciphertext_hash: Hash32,
    /// Exact ciphertext byte length, which must equal the header's payload reference.
    pub ciphertext_length: u64,
    /// The payload ciphertext; this crate never decrypts it.
    pub ciphertext: Vec<u8>,
}

const RECORD_CONTEXT: &str = "payload shard record";
const RECORD_KEYS: &[u64] = &[0, 1, 2, 3];

impl PayloadShardRecord {
    /// Builds a record whose hash and length are derived from the ciphertext.
    pub fn of(op_id: Hash32, ciphertext: Vec<u8>) -> Self {
        Self {
            op_id,
            ciphertext_hash: Hash32::of(&ciphertext),
            ciphertext_length: ciphertext.len() as u64,
            ciphertext,
        }
    }

    /// Enforces §3.2.3: the record's own hash and length must verify before it is indexed.
    pub fn verify_self(&self) -> Result<(), SegmentError> {
        if self.ciphertext_hash != Hash32::of(&self.ciphertext)
            || self.ciphertext_length != self.ciphertext.len() as u64
        {
            return Err(SegmentError::PayloadRecordSelfMismatch { op_id: self.op_id });
        }
        Ok(())
    }

    /// Encodes the record map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Bytes(self.op_id.0.to_vec())),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.ciphertext_hash.0.to_vec()),
            ),
            (CborValue::Uint(2), CborValue::Uint(self.ciphertext_length)),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.ciphertext.clone()),
            ),
        ])
    }

    /// Decodes the record map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, SegmentError> {
        require_uint_keys(value, RECORD_KEYS, RECORD_CONTEXT)?;
        Ok(Self {
            op_id: hash32_at(value, 0, RECORD_CONTEXT)?,
            ciphertext_hash: hash32_at(value, 1, RECORD_CONTEXT)?,
            ciphertext_length: uint_at(value, 2, RECORD_CONTEXT)?,
            ciphertext: byte_string_at(value, 3, RECORD_CONTEXT)?,
        })
    }
}

/// Resolves the payload reference and scope a header states, for the §5.4 cross-check.
pub trait HeaderPayloadIndex {
    /// The header's §3.5 payload reference, if the header is known.
    fn payload_ref(&self, op_id: Hash32) -> Option<PayloadRef>;

    /// The header's operation scope, if the header is known.
    fn header_scope(&self, op_id: Hash32) -> Option<Scope>;
}

impl HeaderPayloadIndex for HeaderSegmentBody {
    fn payload_ref(&self, op_id: Hash32) -> Option<PayloadRef> {
        self.record(op_id).map(EnvelopeView::payload_ref)
    }

    fn header_scope(&self, op_id: Hash32) -> Option<Scope> {
        self.record(op_id).map(EnvelopeView::scope)
    }
}

/// One payload shard body: records for exactly one payload-topic scope (§3.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadShardBody {
    topic: PayloadTopicScope,
    records: Vec<PayloadShardRecord>,
}

const SHARD_BODY_CONTEXT: &str = "payload shard body";
const SHARD_BODY_KEYS: &[u64] = &[0, 1, 2];

impl PayloadShardBody {
    /// Orders records by `op_id`, refusing an empty body, a duplicate, or a self-inconsistent record.
    pub fn new(
        topic: PayloadTopicScope,
        records: Vec<PayloadShardRecord>,
    ) -> Result<Self, SegmentError> {
        if records.is_empty() {
            return Err(SegmentError::EmptyLaneBody {
                context: SHARD_BODY_CONTEXT,
            });
        }
        let mut seen = BTreeSet::new();
        for record in &records {
            record.verify_self()?;
            if !seen.insert(record.op_id) {
                return Err(SegmentError::DuplicateOperationId {
                    op_id: record.op_id,
                });
            }
        }
        let mut records = records;
        records.sort_by(|left, right| left.op_id.cmp(&right.op_id));
        Ok(Self { topic, records })
    }

    /// The one topic scope this shard is bound to.
    pub fn topic(&self) -> PayloadTopicScope {
        self.topic
    }

    /// Records in strictly ascending `op_id` order.
    pub fn records(&self) -> &[PayloadShardRecord] {
        &self.records
    }

    /// The record for one operation, if this shard carries it.
    pub fn record(&self, op_id: Hash32) -> Option<&PayloadShardRecord> {
        self.records
            .binary_search_by(|record| record.op_id.cmp(&op_id))
            .ok()
            .map(|index| &self.records[index])
    }

    /// Cross-checks every record against the header that references it (§3.2.1, §5.4).
    ///
    /// An unreferenced record is refused: a shard may not introduce a payload as a committed
    /// operation, and a header's payload reference must resolve to exactly matching bytes.
    pub fn verify_against_headers(
        &self,
        index: &impl HeaderPayloadIndex,
    ) -> Result<(), SegmentError> {
        for record in &self.records {
            record.verify_self()?;
            let op_id = record.op_id;
            let (Some(payload), Some(scope)) =
                (index.payload_ref(op_id), index.header_scope(op_id))
            else {
                return Err(SegmentError::UnreferencedPayloadRecord { op_id });
            };
            if payload.ciphertext_hash != record.ciphertext_hash
                || payload.ciphertext_length != record.ciphertext_length
            {
                return Err(SegmentError::PayloadRecordHeaderMismatch { op_id });
            }
            if !self.topic.admits(&scope) {
                return Err(SegmentError::MixedPayloadTopicScope { op_id });
            }
        }
        Ok(())
    }

    /// Encodes the inner body.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::PayloadShard.to_wire()),
            ),
            (CborValue::Uint(1), self.topic.to_cbor()),
            (
                CborValue::Uint(2),
                CborValue::Array(
                    self.records
                        .iter()
                        .map(PayloadShardRecord::to_cbor)
                        .collect(),
                ),
            ),
        ])
    }

    /// The exact plaintext bytes that get sealed.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes a decrypted body, refusing a lane mismatch and re-verifying every record.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        require_uint_keys(&value, SHARD_BODY_KEYS, SHARD_BODY_CONTEXT)?;
        if uint_at(&value, 0, SHARD_BODY_CONTEXT)? != LaneClass::PayloadShard.to_wire() {
            return Err(SegmentError::LaneBodyMismatch);
        }
        let topic = PayloadTopicScope::from_cbor(value_at(&value, 1, SHARD_BODY_CONTEXT)?)?;
        let mut records = Vec::new();
        let mut previous: Option<Hash32> = None;
        for record in array_at(&value, 2, SHARD_BODY_CONTEXT)? {
            let record = PayloadShardRecord::from_cbor(record)?;
            if previous.is_some_and(|earlier| earlier >= record.op_id) {
                return Err(SegmentError::RecordsNotStrictlyAscending {
                    context: SHARD_BODY_CONTEXT,
                });
            }
            previous = Some(record.op_id);
            records.push(record);
        }
        let body = Self::new(topic, records)?;
        assert_canonical_bytes(&value, bytes, SHARD_BODY_CONTEXT)?;
        Ok(body)
    }
}

/// The effective capability's `max_segment_bytes` caveat (§3.2.5).
///
/// Both numbers are caller policy under D-CL24; there is deliberately no `Default` that would
/// invent one. `sealed_overhead_bytes` covers the AEAD tag and the sealed outer map, so the
/// bound is checked against the projected STORED size rather than the plaintext size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentSizeCaveat {
    /// Largest stored artifact the effective capability permits.
    pub max_segment_bytes: u64,
    /// Bytes the sealing step adds on top of the plaintext body.
    pub sealed_overhead_bytes: u64,
}

impl SegmentSizeCaveat {
    /// Builds a caveat from the caller's two explicit numbers.
    pub const fn new(max_segment_bytes: u64, sealed_overhead_bytes: u64) -> Self {
        Self {
            max_segment_bytes,
            sealed_overhead_bytes,
        }
    }
}

/// A shard body accepted for sealing: its plaintext and the stored size it will occupy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadShardPacking {
    /// The canonical plaintext body the AEAD will seal.
    pub plaintext: Vec<u8>,
    /// Projected stored artifact length, sealing overhead included.
    pub projected_stored_length: u64,
}

/// Prepares a shard for sealing, REJECTING an oversized candidate (§3.2.5).
///
/// Never truncates and never omits a record: an oversized candidate is the caller's problem to
/// repack, and silently dropping a record would change which payloads a manifest claims.
/// Performs no encryption; wave 3 supplies the AEAD behind [`super::receipt::SealedBodyOpener`].
///
/// The lane and epoch come from the body's own topic, so this cannot be asked about a lane the
/// shard does not belong to. Nonce freshness is NOT recorded here: the nonce belongs to the
/// sealed artifact, and [`super::artifact::seal_artifact`] is the one place that records it, so
/// packing a candidate twice never burns a nonce the AEAD has not actually used.
pub fn seal_payload_shard(
    view: &impl RelayAuthorizationView,
    body: &PayloadShardBody,
    caveat: &SegmentSizeCaveat,
    descriptor: &EncryptionDescriptor,
) -> Result<PayloadShardPacking, SegmentError> {
    let topic = body.topic();
    let lane = LaneKey::Payload(topic);
    assert_seals_under_current_key(view, &lane, topic.scope_epoch, descriptor)?;
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
    Ok(PayloadShardPacking {
        plaintext,
        projected_stored_length,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::envelope::{NONCE_LENGTH, PRODUCTION_SUITE_ID};
    use crate::segment::relay_policy::PeerIdentity;
    use crate::segment::test_fixtures::{genesis_header, identifier, intent_header, HeaderFixture};

    fn topic(petal: u8, epoch: u64, key: u8) -> PayloadTopicScope {
        PayloadTopicScope {
            verse_id: identifier(0x11),
            petal_id: identifier(petal),
            scope_epoch: epoch,
            key_id: identifier(key),
        }
    }

    struct Lane {
        headers: HeaderSegmentBody,
        records: Vec<PayloadShardRecord>,
        fixtures: Vec<HeaderFixture>,
    }

    /// One genesis plus one payload-bearing intent per supplied scope.
    fn lane(scopes: &[Scope]) -> Lane {
        let verse = identifier(0x11);
        let root = genesis_header(1, Scope::verse_wide(verse), 1);
        let mut fixtures = vec![root.clone()];
        let mut records = Vec::new();
        for (index, scope) in scopes.iter().enumerate() {
            let ciphertext = format!("ciphertext-{index}").into_bytes();
            let intent = intent_header(
                10 + index as u8,
                *scope,
                vec![root.op_id],
                index as u32 + 1,
                &ciphertext,
            );
            records.push(PayloadShardRecord::of(intent.op_id, ciphertext));
            fixtures.push(intent);
        }
        let headers = HeaderSegmentBody::seal(
            verse,
            fixtures.iter().map(|fixture| fixture.bytes.as_slice()),
        )
        .expect("header lane");
        Lane {
            headers,
            records,
            fixtures,
        }
    }

    #[test]
    fn payload_shard_is_petal_affine_and_scope_pure() {
        let verse = identifier(0x11);
        let petal = identifier(0x21);
        let first_resource = Scope::new(verse, Some(petal), Some(identifier(0x31))).expect("scope");
        let second_resource =
            Scope::new(verse, Some(petal), Some(identifier(0x32))).expect("scope");
        let lane = lane(&[first_resource, second_resource]);

        let shard = PayloadShardBody::new(topic(0x21, 7, 0x41), lane.records.clone())
            .expect("one-petal shard");
        shard
            .verify_against_headers(&lane.headers)
            .expect("resource records from one petal share a shard");

        let foreign_petal =
            PayloadShardBody::new(topic(0x22, 7, 0x41), lane.records.clone()).expect("shard body");
        assert!(matches!(
            foreign_petal.verify_against_headers(&lane.headers),
            Err(SegmentError::MixedPayloadTopicScope { .. })
        ));

        let foreign_verse = PayloadShardBody::new(
            PayloadTopicScope {
                verse_id: identifier(0x99),
                ..topic(0x21, 7, 0x41)
            },
            lane.records.clone(),
        )
        .expect("shard body");
        assert!(matches!(
            foreign_verse.verify_against_headers(&lane.headers),
            Err(SegmentError::MixedPayloadTopicScope { .. })
        ));

        assert_ne!(topic(0x21, 7, 0x41), topic(0x21, 8, 0x41));
        assert_ne!(topic(0x21, 7, 0x41), topic(0x21, 7, 0x42));
        assert_eq!(
            shard.encode_canonical().and_then(|bytes| {
                PayloadShardBody::decode_canonical(&bytes).map(|decoded| decoded.topic())
            }),
            Ok(topic(0x21, 7, 0x41))
        );
    }

    #[test]
    fn payload_shard_rehashes_each_record_against_header() {
        let verse = identifier(0x11);
        let petal = Scope::new(verse, Some(identifier(0x21)), None).expect("scope");
        let lane = lane(&[petal]);
        let op_id = lane.records[0].op_id;
        let good =
            PayloadShardBody::new(topic(0x21, 7, 0x41), lane.records.clone()).expect("shard");
        good.verify_against_headers(&lane.headers).expect("matches");

        let mut altered = lane.records[0].clone();
        altered.ciphertext = b"tampered".to_vec();
        assert_eq!(
            PayloadShardBody::new(topic(0x21, 7, 0x41), vec![altered]),
            Err(SegmentError::PayloadRecordSelfMismatch { op_id })
        );

        let mut wrong_length = lane.records[0].clone();
        wrong_length.ciphertext_length += 1;
        assert_eq!(
            PayloadShardBody::new(topic(0x21, 7, 0x41), vec![wrong_length]),
            Err(SegmentError::PayloadRecordSelfMismatch { op_id })
        );

        let mut wrong_hash = lane.records[0].clone();
        wrong_hash.ciphertext_hash = Hash32([0; 32]);
        assert_eq!(
            PayloadShardBody::new(topic(0x21, 7, 0x41), vec![wrong_hash]),
            Err(SegmentError::PayloadRecordSelfMismatch { op_id })
        );

        assert_eq!(
            PayloadShardBody::new(
                topic(0x21, 7, 0x41),
                vec![lane.records[0].clone(), lane.records[0].clone()]
            ),
            Err(SegmentError::DuplicateOperationId { op_id })
        );

        let unreferenced = PayloadShardRecord::of(Hash32([0xAB; 32]), b"orphan".to_vec());
        let orphan_shard =
            PayloadShardBody::new(topic(0x21, 7, 0x41), vec![unreferenced]).expect("shard");
        assert_eq!(
            orphan_shard.verify_against_headers(&lane.headers),
            Err(SegmentError::UnreferencedPayloadRecord {
                op_id: Hash32([0xAB; 32])
            })
        );

        let relabelled = PayloadShardRecord::of(op_id, b"different-ciphertext".to_vec());
        let relabelled_shard =
            PayloadShardBody::new(topic(0x21, 7, 0x41), vec![relabelled]).expect("shard");
        assert_eq!(
            relabelled_shard.verify_against_headers(&lane.headers),
            Err(SegmentError::PayloadRecordHeaderMismatch { op_id })
        );

        assert_eq!(lane.fixtures.len(), 2);
    }

    /// A persistent view reporting one current epoch and key for every lane.
    struct View {
        epoch: u64,
        key_id: Identifier32,
    }

    impl RelayAuthorizationView for View {
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

    #[test]
    fn sealing_rejects_an_oversized_candidate_rather_than_omitting_records() {
        let verse = identifier(0x11);
        let petal = Scope::new(verse, Some(identifier(0x21)), None).expect("scope");
        let lane = lane(&[petal]);
        let shard =
            PayloadShardBody::new(topic(0x21, 7, 0x41), lane.records.clone()).expect("shard");
        let descriptor =
            EncryptionDescriptor::new(PRODUCTION_SUITE_ID, identifier(0x41), [3; NONCE_LENGTH]);
        let view = View {
            epoch: 7,
            key_id: identifier(0x41),
        };

        let plaintext_length = shard.encode_canonical().expect("bytes").len() as u64;
        let generous = SegmentSizeCaveat::new(plaintext_length + 64, 16);
        let packing = seal_payload_shard(&view, &shard, &generous, &descriptor).expect("fits");
        assert_eq!(packing.projected_stored_length, plaintext_length + 16);
        assert_eq!(packing.plaintext.len() as u64, plaintext_length);

        let tight = SegmentSizeCaveat::new(plaintext_length, 16);
        assert_eq!(
            seal_payload_shard(&view, &shard, &tight, &descriptor),
            Err(SegmentError::OversizedSegment {
                candidate: plaintext_length + 16,
                maximum: plaintext_length,
            })
        );
        assert_eq!(shard.records().len(), lane.records.len());

        // The lane's own epoch and key come from the persistent view, not from a bare identifier
        // the caller passes alongside: a superseded key or epoch is refused here, not only at
        // the seal.
        let rekeyed = View {
            epoch: 7,
            key_id: identifier(0x42),
        };
        assert_eq!(
            seal_payload_shard(&rekeyed, &shard, &generous, &descriptor),
            Err(SegmentError::StaleScopeKey {
                key_id: identifier(0x41)
            })
        );

        let bumped = View {
            epoch: 8,
            key_id: identifier(0x41),
        };
        assert_eq!(
            seal_payload_shard(&bumped, &shard, &generous, &descriptor),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            })
        );

        // The shard's own topic key still has to be the key the descriptor seals under, even
        // when the persistent view agrees with the descriptor.
        let foreign_topic =
            PayloadShardBody::new(topic(0x21, 7, 0x42), lane.records.clone()).expect("shard");
        assert_eq!(
            seal_payload_shard(&view, &foreign_topic, &generous, &descriptor),
            Err(SegmentError::StaleScopeKey {
                key_id: identifier(0x41)
            })
        );
    }
}
