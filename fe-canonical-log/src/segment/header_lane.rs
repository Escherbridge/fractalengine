//! The verse-wide, payload-free header lane (SPEC-6 §3.1).

use std::collections::BTreeMap;

use crate::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use crate::envelope::{CompleteEnvelope, EquivocationKey, Hash32, Identifier32, PayloadRef, Scope};
use crate::signing::{decode_and_admit, verify_envelope};

use super::artifact::LaneClass;
use super::{
    array_at, assert_canonical_bytes, identifier32_at, require_uint_keys, uint_at, SegmentError,
};

/// Read-only view of an admitted SPEC-1 envelope; the seam between SPEC-6 and SPEC-1.
pub trait EnvelopeView {
    /// The exact canonical bytes the record carries.
    fn canonical_bytes(&self) -> &[u8];

    /// `op_id = BLAKE3(canonical_bytes)`.
    fn op_id(&self) -> Hash32;

    /// The operation scope; SPEC-6 §3.1.2, SPEC-2 disavow matching and SPEC-3 cells need it.
    fn scope(&self) -> Scope;

    /// The verse the operation belongs to.
    fn verse_id(&self) -> Identifier32 {
        self.scope().verse_id()
    }

    /// Whether the operation carries a non-empty encrypted payload.
    fn has_payload(&self) -> bool;

    /// The §3.5 payload reference a shard record must match exactly.
    fn payload_ref(&self) -> PayloadRef;

    /// The operation's parents, strictly ascending.
    fn parents(&self) -> &[Hash32];

    /// The registry hash of the operation's schema.
    fn schema_hash(&self) -> Hash32;

    /// The §3.4 equivocation key: at most one `op_id` may ever carry it.
    fn equivocation_key(&self) -> EquivocationKey;

    /// Re-verifies the §5.1 signature.
    fn verify_signature(&self) -> Result<(), SegmentError>;
}

/// A SPEC-1 envelope that passed `signing::decode_and_admit`, kept with its received bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedEnvelope {
    bytes: Vec<u8>,
    envelope: CompleteEnvelope,
    op_id: Hash32,
}

impl AdmittedEnvelope {
    /// Runs the mandatory SPEC-1 ingress over received bytes and keeps them verbatim.
    pub fn admit(bytes: &[u8]) -> Result<Self, SegmentError> {
        let (envelope, op_id) = decode_and_admit(bytes)?;
        Ok(Self {
            bytes: bytes.to_vec(),
            envelope,
            op_id,
        })
    }

    /// The admitted envelope.
    pub fn envelope(&self) -> &CompleteEnvelope {
        &self.envelope
    }
}

impl EnvelopeView for AdmittedEnvelope {
    fn canonical_bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn op_id(&self) -> Hash32 {
        self.op_id
    }

    fn scope(&self) -> Scope {
        self.envelope.unsigned.scope
    }

    fn has_payload(&self) -> bool {
        !self.envelope.unsigned.payload.is_no_payload_form()
    }

    fn payload_ref(&self) -> PayloadRef {
        self.envelope.unsigned.payload
    }

    fn parents(&self) -> &[Hash32] {
        &self.envelope.unsigned.parents
    }

    fn schema_hash(&self) -> Hash32 {
        self.envelope.unsigned.schema_hash
    }

    fn equivocation_key(&self) -> EquivocationKey {
        self.envelope.equivocation_key()
    }

    fn verify_signature(&self) -> Result<(), SegmentError> {
        Ok(verify_envelope(&self.envelope)?)
    }
}

/// One header lane body: complete SPEC-1 headers for exactly one verse (§3.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderSegmentBody {
    verse_id: Identifier32,
    records: Vec<AdmittedEnvelope>,
}

const HEADER_BODY_CONTEXT: &str = "header segment body";
const HEADER_BODY_KEYS: &[u64] = &[0, 1, 2];

impl HeaderSegmentBody {
    /// Admits every header, enforces the §3.1 lane rules, and orders records by `op_id`.
    ///
    /// A payload-BEARING header belongs here: §3.1.3 excludes payload *ciphertext* from the
    /// lane, not operations that reference one, and §4.2.5 resolves those references against
    /// the payload-shard lane. The exclusion is structural — the body grammar holds nothing
    /// but complete SPEC-1 envelope byte strings, so ciphertext and capability-chain bytes
    /// have no slot to occupy.
    ///
    /// A byte-identical duplicate is a no-op; a duplicate whose bytes differ is refused, and
    /// two distinct operations sharing one equivocation key refuse the whole body (§3.4).
    pub fn seal<'a>(
        verse_id: Identifier32,
        headers: impl IntoIterator<Item = &'a [u8]>,
    ) -> Result<Self, SegmentError> {
        let mut admitted = Vec::new();
        for bytes in headers {
            admitted.push(AdmittedEnvelope::admit(bytes)?);
        }
        Self::from_admitted(verse_id, admitted)
    }

    /// Enforces the §3.1 lane rules over already-admitted envelopes.
    pub fn from_admitted(
        verse_id: Identifier32,
        admitted: Vec<AdmittedEnvelope>,
    ) -> Result<Self, SegmentError> {
        let mut records: BTreeMap<Hash32, AdmittedEnvelope> = BTreeMap::new();
        let mut slots: BTreeMap<EquivocationKey, Hash32> = BTreeMap::new();

        for record in admitted {
            let op_id = record.op_id();
            record.verify_signature()?;
            if record.verse_id() != verse_id {
                return Err(SegmentError::ForeignVerseId { op_id });
            }
            if let Some(existing) = records.get(&op_id) {
                if existing.canonical_bytes() != record.canonical_bytes() {
                    return Err(SegmentError::ConflictingDuplicateOperationId { op_id });
                }
                continue;
            }
            let equivocation_key = record.equivocation_key();
            if let Some(first_op_id) = slots.insert(equivocation_key, op_id) {
                return Err(SegmentError::AuthorEquivocation {
                    equivocation_key,
                    first_op_id,
                    second_op_id: op_id,
                });
            }
            records.insert(op_id, record);
        }

        Ok(Self {
            verse_id,
            records: records.into_values().collect(),
        })
    }

    /// The one verse this lane body covers.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// Records in strictly ascending `op_id` order.
    pub fn records(&self) -> &[AdmittedEnvelope] {
        &self.records
    }

    /// The record for one operation, if this body carries it.
    pub fn record(&self, op_id: Hash32) -> Option<&AdmittedEnvelope> {
        self.records
            .binary_search_by(|record| record.op_id().cmp(&op_id))
            .ok()
            .map(|index| &self.records[index])
    }

    /// Encodes the inner body; `op_id` stays derived and is never carried on the wire.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(LaneClass::HeaderSegment.to_wire()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.verse_id.0.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Array(
                    self.records
                        .iter()
                        .map(|record| CborValue::Bytes(record.canonical_bytes().to_vec()))
                        .collect(),
                ),
            ),
        ])
    }

    /// The exact plaintext bytes that get sealed.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, SegmentError> {
        Ok(encode_canonical_checked(&self.to_cbor())?)
    }

    /// Decodes a decrypted body, re-admitting every enclosed header (§3.1.4).
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, SegmentError> {
        let value = decode_canonical(bytes)?;
        require_uint_keys(&value, HEADER_BODY_KEYS, HEADER_BODY_CONTEXT)?;
        if uint_at(&value, 0, HEADER_BODY_CONTEXT)? != LaneClass::HeaderSegment.to_wire() {
            return Err(SegmentError::LaneBodyMismatch);
        }
        let verse_id = identifier32_at(&value, 1, HEADER_BODY_CONTEXT)?;
        let mut admitted = Vec::new();
        for record in array_at(&value, 2, HEADER_BODY_CONTEXT)? {
            let record = record.as_bytes().ok_or(SegmentError::FieldTypeMismatch {
                context: HEADER_BODY_CONTEXT,
                key: 2,
            })?;
            admitted.push(AdmittedEnvelope::admit(record)?);
        }
        let body = Self::from_admitted(verse_id, admitted)?;
        assert_canonical_bytes(&value, bytes, HEADER_BODY_CONTEXT)?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment::test_fixtures::{genesis_header, identifier, intent_header};

    #[test]
    fn header_lane_is_verse_wide_and_payload_free() {
        let verse = identifier(0x11);
        let first_petal = Scope::new(verse, Some(identifier(0x21)), None).expect("scope");
        let second_petal = Scope::new(verse, Some(identifier(0x22)), None).expect("scope");
        let resource =
            Scope::new(verse, Some(identifier(0x22)), Some(identifier(0x31))).expect("scope");

        let genesis = genesis_header(1, first_petal, 1);
        let across_petals = [
            genesis.clone(),
            genesis_header(2, second_petal, 1),
            genesis_header(3, resource, 1),
            intent_header(
                4,
                second_petal,
                vec![genesis.op_id],
                2,
                b"opaque-ciphertext",
            ),
        ];
        let body = HeaderSegmentBody::seal(
            verse,
            across_petals.iter().map(|header| header.bytes.as_slice()),
        )
        .expect("verse-wide header lane");
        assert_eq!(body.verse_id(), verse);
        assert_eq!(body.records().len(), 4);
        for header in &across_petals {
            assert_eq!(
                body.record(header.op_id).map(EnvelopeView::op_id),
                Some(header.op_id)
            );
        }
        let payload_bearing = body.record(across_petals[3].op_id).expect("intent");
        assert!(payload_bearing.has_payload());
        assert!(!body
            .encode_canonical()
            .expect("bytes")
            .windows(b"opaque-ciphertext".len())
            .any(|window| window == b"opaque-ciphertext"));

        let encoded = body.encode_canonical().expect("bytes");
        assert_eq!(
            HeaderSegmentBody::decode_canonical(&encoded).expect("round trip"),
            body
        );

        assert!(HeaderSegmentBody::seal(verse, [b"opaque-ciphertext".as_slice()]).is_err());

        let foreign_verse = identifier(0x99);
        let foreign = genesis_header(5, Scope::verse_wide(foreign_verse), 1);
        assert_eq!(
            HeaderSegmentBody::seal(verse, [foreign.bytes.as_slice()]),
            Err(SegmentError::ForeignVerseId {
                op_id: foreign.op_id
            })
        );
    }

    #[test]
    fn header_body_rejects_capability_bytes_and_foreign_fields() {
        let verse = identifier(0x11);
        let header = genesis_header(1, Scope::verse_wide(verse), 1);
        let body = HeaderSegmentBody::seal(verse, [header.bytes.as_slice()]).expect("body");

        let CborValue::Map(mut entries) = body.to_cbor() else {
            unreachable!()
        };
        entries.push((CborValue::Uint(3), CborValue::Bytes(b"capability".to_vec())));
        let with_capability = encode_canonical_checked(&CborValue::Map(entries)).expect("bytes");
        assert_eq!(
            HeaderSegmentBody::decode_canonical(&with_capability),
            Err(SegmentError::UnknownField {
                context: HEADER_BODY_CONTEXT
            })
        );

        let mut mislabelled = body.to_cbor();
        if let CborValue::Map(entries) = &mut mislabelled {
            entries[0].1 = CborValue::Uint(LaneClass::PayloadShard.to_wire());
        }
        let mislabelled = encode_canonical_checked(&mislabelled).expect("bytes");
        assert_eq!(
            HeaderSegmentBody::decode_canonical(&mislabelled),
            Err(SegmentError::LaneBodyMismatch)
        );
    }

    #[test]
    fn identical_duplicate_is_a_no_op_and_equivocation_refuses_the_body() {
        let verse = identifier(0x11);
        let header = genesis_header(1, Scope::verse_wide(verse), 1);
        let body =
            HeaderSegmentBody::seal(verse, [header.bytes.as_slice(), header.bytes.as_slice()])
                .expect("identical duplicate is a no-op");
        assert_eq!(body.records().len(), 1);

        let twin = genesis_header(
            1,
            Scope::new(verse, Some(identifier(0x21)), None).expect("scope"),
            1,
        );
        assert_ne!(twin.op_id, header.op_id);
        assert_eq!(
            twin.envelope.equivocation_key(),
            header.envelope.equivocation_key()
        );
        let equivocation =
            HeaderSegmentBody::seal(verse, [header.bytes.as_slice(), twin.bytes.as_slice()]);
        assert_eq!(
            equivocation,
            Err(SegmentError::AuthorEquivocation {
                equivocation_key: header.envelope.equivocation_key(),
                first_op_id: header.op_id,
                second_op_id: twin.op_id,
            })
        );
    }
}
