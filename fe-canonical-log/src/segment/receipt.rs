//! The receipt pipeline: re-hash, validate, decrypt, cross-check, then store (SPEC-6 §5).
//!
//! The ORDER is the contract. Re-hash before recording or serving; validate the sealed outer
//! form before decryption; resolve the scope key and authenticate before trusting any inner
//! lane, scope, predecessor, index, or record field; cross-check header and shard references
//! in both directions; only then admit. Every failure routes to quarantine.

use crate::envelope::Hash32;

use super::artifact::{
    admit_sealed, verify_artifact_id, BulkArtifactBody, LaneClass, SealedArtifact,
};
use super::hashseq::HashSeqNode;
use super::header_lane::{EnvelopeView, HeaderSegmentBody};
use super::manifest::SegmentManifestBody;
use super::payload_shard::{HeaderPayloadIndex, PayloadShardBody};
use super::store::{Quarantine, QuarantineEntry, QuarantineReason, SealedArtifactStore};
use super::SegmentError;

/// A decrypted, canonically decoded lane body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenedBody {
    /// A verse-wide header lane body.
    HeaderSegment(HeaderSegmentBody),
    /// A petal-affine payload shard body.
    PayloadShard(PayloadShardBody),
    /// A HashSeq node body.
    HashSeqNode(HashSeqNode),
    /// A segment manifest body.
    SegmentManifest(SegmentManifestBody),
    /// A petal-affine bulk artifact body: one shard, tile, or asset (§3.4).
    BulkArtifact(BulkArtifactBody),
}

impl OpenedBody {
    /// The lane class this body belongs to.
    pub fn lane_class(&self) -> LaneClass {
        match self {
            Self::HeaderSegment(_) => LaneClass::HeaderSegment,
            Self::PayloadShard(_) => LaneClass::PayloadShard,
            Self::HashSeqNode(_) => LaneClass::HashSeqNode,
            Self::SegmentManifest(_) => LaneClass::SegmentManifest,
            Self::BulkArtifact(_) => LaneClass::BulkArtifact,
        }
    }
}

/// Decodes a decrypted body under its declared lane (§2.1.5).
///
/// Each body's own first field restates its lane, so a body decoded under the wrong outer lane
/// class fails with [`SegmentError::LaneBodyMismatch`] rather than being reinterpreted.
pub fn decode_opened_body(
    lane_class: LaneClass,
    plaintext: &[u8],
) -> Result<OpenedBody, SegmentError> {
    Ok(match lane_class {
        LaneClass::HeaderSegment => {
            OpenedBody::HeaderSegment(HeaderSegmentBody::decode_canonical(plaintext)?)
        }
        LaneClass::PayloadShard => {
            OpenedBody::PayloadShard(PayloadShardBody::decode_canonical(plaintext)?)
        }
        LaneClass::HashSeqNode => {
            OpenedBody::HashSeqNode(HashSeqNode::decode_canonical(plaintext)?)
        }
        LaneClass::SegmentManifest => {
            OpenedBody::SegmentManifest(SegmentManifestBody::decode_canonical(plaintext)?)
        }
        LaneClass::BulkArtifact => {
            OpenedBody::BulkArtifact(BulkArtifactBody::decode_canonical(plaintext)?)
        }
    })
}

/// Authenticated decryption of a sealed body; wave 3 supplies the XChaCha20-Poly1305 backing.
pub trait SealedBodyOpener {
    /// Whether the caller currently holds decrypt authority for this artifact's scope key.
    ///
    /// `false` is normal, not a failure: a relay stores and serves opaque bytes without ever
    /// holding a scope key (§6.3).
    fn has_decrypt_authority(&self, sealed: &SealedArtifact) -> bool;

    /// Authenticates and decrypts the sealed body under `associated_data` (§9.1).
    fn open(
        &self,
        sealed: &SealedArtifact,
        associated_data: &[u8],
    ) -> Result<Vec<u8>, SegmentError>;
}

/// Resolves what the receiver already knows, for the §5.4 bidirectional cross-check.
pub trait CrossCheckView: HeaderPayloadIndex {
    /// The `(ciphertext_hash, ciphertext_length)` of a shard record the receiver already holds.
    fn known_shard_record(&self, op_id: Hash32) -> Option<(Hash32, u64)>;
}

/// What a receipt did with an artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptOutcome {
    /// Verified, opened, cross-checked, and admitted to the store.
    Stored {
        /// Content address of the admitted artifact.
        artifact_id: Hash32,
        /// Lane the decrypted body belongs to.
        lane: LaneClass,
    },
    /// Outer form verified and stored opaquely; the receiver holds no decrypt authority (§6.3).
    StoredOpaque {
        /// Content address of the admitted artifact.
        artifact_id: Hash32,
    },
    /// Refused and retained as untrusted input; never served, never proof material (§5.5).
    Quarantined {
        /// Classification of the refusal.
        reason: QuarantineReason,
    },
}

/// Reassembles a ranged transfer and refuses to hand back anything but a complete buffer (§5.2).
///
/// Per-range checksums are a transport nicety; they never replace the final `artifact_id`
/// check, which happens in [`ReceiptPipeline::receive`] over the reassembled bytes.
#[derive(Clone, Debug)]
pub struct RangeReassembly {
    expected_length: u64,
    buffer: Vec<u8>,
    filled: Vec<bool>,
}

impl RangeReassembly {
    /// Prepares reassembly for an artifact of a declared length.
    pub fn new(expected_length: u64) -> Self {
        let length = expected_length as usize;
        Self {
            expected_length,
            buffer: vec![0; length],
            filled: vec![false; length],
        }
    }

    /// Accepts one range after checking its own checksum; overlapping ranges must agree.
    pub fn accept_range(
        &mut self,
        offset: u64,
        bytes: &[u8],
        range_checksum: Hash32,
    ) -> Result<(), SegmentError> {
        if Hash32::of(bytes) != range_checksum {
            return Err(SegmentError::RangeChecksumMismatch { offset });
        }
        let start = offset as usize;
        let end = start
            .checked_add(bytes.len())
            .filter(|end| *end <= self.buffer.len())
            .ok_or(SegmentError::IncompleteRangeReassembly { offset })?;
        self.buffer[start..end].copy_from_slice(bytes);
        for slot in &mut self.filled[start..end] {
            *slot = true;
        }
        Ok(())
    }

    /// Yields the complete buffer, or the offset of the first byte never delivered.
    pub fn finish(self) -> Result<Vec<u8>, SegmentError> {
        if let Some(offset) = self.filled.iter().position(|filled| !filled) {
            return Err(SegmentError::IncompleteRangeReassembly {
                offset: offset as u64,
            });
        }
        debug_assert_eq!(self.buffer.len() as u64, self.expected_length);
        Ok(self.buffer)
    }
}

/// The only path by which bytes reach the verified store (§5.1, §5.6).
#[derive(Clone, Debug)]
pub struct ReceiptPipeline<S: SealedArtifactStore> {
    store: S,
    quarantine: Quarantine,
}

impl<S: SealedArtifactStore> ReceiptPipeline<S> {
    /// Builds a pipeline over a store and a caller-sized quarantine.
    pub fn new(store: S, quarantine: Quarantine) -> Self {
        Self { store, quarantine }
    }

    /// The verified store; presence here is validity, presence anywhere else is not.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Refused receipts, oldest first.
    pub fn quarantine(&self) -> &Quarantine {
        &self.quarantine
    }

    /// Consumes the pipeline and returns its store.
    pub fn into_store(self) -> S {
        self.store
    }

    fn refuse(
        &mut self,
        error: &SegmentError,
        claimed_id: Option<Hash32>,
        bytes: &[u8],
    ) -> ReceiptOutcome {
        let reason = QuarantineReason::classify(error);
        self.quarantine.retain(QuarantineEntry {
            reason: reason.clone(),
            claimed_id,
            bytes: bytes.to_vec(),
        });
        ReceiptOutcome::Quarantined { reason }
    }

    /// Runs the §5 receipt order over one complete artifact.
    pub fn receive(
        &mut self,
        claimed_id: Hash32,
        declared_length: u64,
        bytes: &[u8],
        opener: &impl SealedBodyOpener,
        cross_check: &impl CrossCheckView,
    ) -> ReceiptOutcome {
        if bytes.len() as u64 != declared_length {
            let error = SegmentError::StoredLengthMismatch {
                declared: declared_length,
                received: bytes.len() as u64,
            };
            return self.refuse(&error, Some(claimed_id), bytes);
        }
        if let Err(error) = verify_artifact_id(bytes, claimed_id) {
            return self.refuse(&error, Some(claimed_id), bytes);
        }
        let sealed = match admit_sealed(bytes) {
            Ok((sealed, _)) => sealed,
            Err(error) => return self.refuse(&error, Some(claimed_id), bytes),
        };

        if !opener.has_decrypt_authority(&sealed) {
            let stored = self.store.put_verified(claimed_id, bytes);
            return match stored {
                Ok(artifact_id) => ReceiptOutcome::StoredOpaque { artifact_id },
                Err(error) => self.refuse(&error, Some(claimed_id), bytes),
            };
        }

        let associated_data = match sealed.associated_data() {
            Ok(associated_data) => associated_data,
            Err(error) => return self.refuse(&error, Some(claimed_id), bytes),
        };
        let plaintext = match opener.open(&sealed, &associated_data) {
            Ok(plaintext) => plaintext,
            Err(error) => return self.refuse(&error, Some(claimed_id), bytes),
        };
        let body = match decode_opened_body(sealed.lane_class, &plaintext) {
            Ok(body) => body,
            Err(error) => return self.refuse(&error, Some(claimed_id), bytes),
        };
        if let Err(error) = cross_check_body(&body, cross_check) {
            return self.refuse(&error, Some(claimed_id), bytes);
        }
        if let OpenedBody::BulkArtifact(bulk) = &body {
            if let Err(error) = bulk.assert_sealed_under(claimed_id, sealed.encryption.key_id()) {
                return self.refuse(&error, Some(claimed_id), bytes);
            }
        }

        let stored = self.store.put_verified(claimed_id, bytes);
        match stored {
            Ok(artifact_id) => ReceiptOutcome::Stored {
                artifact_id,
                lane: sealed.lane_class,
            },
            Err(error) => self.refuse(&error, Some(claimed_id), bytes),
        }
    }
}

/// Cross-checks header and shard references in both directions (§5.4).
///
/// Shard direction: no record may introduce a payload the header set does not reference.
/// Header direction: a payload-bearing header whose shard record the receiver already holds
/// must match that record's hash and length exactly; a record not yet received is an
/// availability gap that §4.2 resolves, not a receipt failure. A bulk artifact has no header
/// cross-reference: its content is validated at `materialize/<class>`, not at receipt (§3.4
/// rule 8), and its committing operation names it, not the other way round (§3.4 rule 5).
fn cross_check_body(body: &OpenedBody, view: &impl CrossCheckView) -> Result<(), SegmentError> {
    match body {
        OpenedBody::PayloadShard(shard) => shard.verify_against_headers(view),
        OpenedBody::HeaderSegment(headers) => {
            for header in headers.records() {
                if !header.has_payload() {
                    continue;
                }
                let Some((ciphertext_hash, ciphertext_length)) =
                    view.known_shard_record(header.op_id())
                else {
                    continue;
                };
                let payload = header.payload_ref();
                if payload.ciphertext_hash != ciphertext_hash
                    || payload.ciphertext_length != ciphertext_length
                {
                    return Err(SegmentError::PayloadRecordHeaderMismatch {
                        op_id: header.op_id(),
                    });
                }
            }
            Ok(())
        }
        OpenedBody::HashSeqNode(_)
        | OpenedBody::SegmentManifest(_)
        | OpenedBody::BulkArtifact(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::num::NonZeroUsize;

    use crate::capability::verbs::ObjectClass;
    use crate::envelope::{
        EncryptionParams, Identifier32, PayloadRef, Scope, NONCE_LENGTH, PRODUCTION_SUITE_ID,
    };
    use crate::segment::artifact::EncryptionDescriptor;
    use crate::segment::payload_shard::{PayloadShardRecord, PayloadTopicScope};
    use crate::segment::store::InMemorySealedArtifactStore;
    use crate::segment::test_fixtures::{genesis_header, identifier, intent_header};

    /// Stands in for XChaCha20-Poly1305: appends a 16-byte tag over the §9.1 AAD.
    ///
    /// It performs no confidentiality work, which is fine here — what the receipt pipeline
    /// owes is the ORDER and the AAD binding, and both are exercised faithfully.
    pub(crate) struct TagOnlyOpener {
        pub authorized_keys: Vec<Identifier32>,
    }

    const TAG_LENGTH: usize = 16;

    pub(crate) fn seal_for_test(
        lane: LaneClass,
        descriptor: EncryptionDescriptor,
        plaintext: &[u8],
    ) -> SealedArtifact {
        let ciphertext_length = (plaintext.len() + TAG_LENGTH) as u64;
        let placeholder = SealedArtifact {
            format_version: 1,
            lane_class: lane,
            ciphertext_length,
            encryption: descriptor,
            ciphertext: vec![0; ciphertext_length as usize],
        };
        let associated_data = placeholder.associated_data().expect("aad");
        let tag = blake3::hash(&associated_data);
        let mut ciphertext = plaintext.to_vec();
        ciphertext.extend_from_slice(&tag.as_bytes()[..TAG_LENGTH]);
        SealedArtifact::seal(lane, descriptor, ciphertext).expect("sealed")
    }

    impl SealedBodyOpener for TagOnlyOpener {
        fn has_decrypt_authority(&self, sealed: &SealedArtifact) -> bool {
            self.authorized_keys.contains(&sealed.encryption.key_id())
        }

        fn open(
            &self,
            sealed: &SealedArtifact,
            associated_data: &[u8],
        ) -> Result<Vec<u8>, SegmentError> {
            if sealed.ciphertext.len() < TAG_LENGTH {
                return Err(SegmentError::DecryptionFailed);
            }
            let split = sealed.ciphertext.len() - TAG_LENGTH;
            let expected = blake3::hash(associated_data);
            if sealed.ciphertext[split..] != expected.as_bytes()[..TAG_LENGTH] {
                return Err(SegmentError::DecryptionFailed);
            }
            Ok(sealed.ciphertext[..split].to_vec())
        }
    }

    #[derive(Default)]
    pub(crate) struct KnownReferences {
        pub payload_refs: BTreeMap<Hash32, PayloadRef>,
        pub scopes: BTreeMap<Hash32, Scope>,
        pub shard_records: BTreeMap<Hash32, (Hash32, u64)>,
    }

    impl HeaderPayloadIndex for KnownReferences {
        fn payload_ref(&self, op_id: Hash32) -> Option<PayloadRef> {
            self.payload_refs.get(&op_id).copied()
        }

        fn header_scope(&self, op_id: Hash32) -> Option<Scope> {
            self.scopes.get(&op_id).copied()
        }
    }

    impl CrossCheckView for KnownReferences {
        fn known_shard_record(&self, op_id: Hash32) -> Option<(Hash32, u64)> {
            self.shard_records.get(&op_id).copied()
        }
    }

    fn descriptor(key: u8, nonce: u8) -> EncryptionDescriptor {
        EncryptionDescriptor::new(PRODUCTION_SUITE_ID, identifier(key), [nonce; NONCE_LENGTH])
    }

    fn pipeline() -> ReceiptPipeline<InMemorySealedArtifactStore> {
        ReceiptPipeline::new(
            InMemorySealedArtifactStore::new(),
            Quarantine::with_capacity(NonZeroUsize::new(8).expect("capacity")),
        )
    }

    #[test]
    fn receipt_rehashes_reassembled_range_before_serving() {
        let verse = identifier(0x11);
        let header = genesis_header(1, Scope::verse_wide(verse), 1);
        let body = HeaderSegmentBody::seal(verse, [header.bytes.as_slice()]).expect("body");
        let sealed = seal_for_test(
            LaneClass::HeaderSegment,
            descriptor(0x41, 3),
            &body.encode_canonical().expect("plaintext"),
        );
        let bytes = sealed.encode_canonical().expect("bytes");
        let artifact_id = sealed.artifact_id().expect("id");
        let opener = TagOnlyOpener {
            authorized_keys: vec![identifier(0x41)],
        };
        let view = KnownReferences::default();

        let midpoint = bytes.len() / 2;
        let mut reassembly = RangeReassembly::new(bytes.len() as u64);
        reassembly
            .accept_range(0, &bytes[..midpoint], Hash32::of(&bytes[..midpoint]))
            .expect("first range");
        assert_eq!(
            reassembly.clone().finish(),
            Err(SegmentError::IncompleteRangeReassembly {
                offset: midpoint as u64
            })
        );
        let mut tampered = bytes[midpoint..].to_vec();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        reassembly
            .accept_range(midpoint as u64, &tampered, Hash32::of(&tampered))
            .expect("per-range checksum still passes on substituted bytes");
        let reassembled = reassembly.finish().expect("complete");

        let mut receiver = pipeline();
        let outcome = receiver.receive(
            artifact_id,
            reassembled.len() as u64,
            &reassembled,
            &opener,
            &view,
        );
        assert!(matches!(
            outcome,
            ReceiptOutcome::Quarantined {
                reason: QuarantineReason::ArtifactIdMismatch { .. }
            }
        ));
        assert!(!receiver.store().has(artifact_id));
        assert_eq!(receiver.store().get(artifact_id), None);
        assert_eq!(receiver.quarantine().len(), 1);

        let mut honest = RangeReassembly::new(bytes.len() as u64);
        honest
            .accept_range(0, &bytes[..midpoint], Hash32::of(&bytes[..midpoint]))
            .expect("first range");
        honest
            .accept_range(
                midpoint as u64,
                &bytes[midpoint..],
                Hash32::of(&bytes[midpoint..]),
            )
            .expect("second range");
        let honest = honest.finish().expect("complete");
        assert_eq!(
            receiver.receive(artifact_id, honest.len() as u64, &honest, &opener, &view),
            ReceiptOutcome::Stored {
                artifact_id,
                lane: LaneClass::HeaderSegment,
            }
        );
        assert!(receiver.store().has(artifact_id));

        assert_eq!(
            RangeReassembly::new(4).accept_range(0, b"data", Hash32::of(b"other")),
            Err(SegmentError::RangeChecksumMismatch { offset: 0 })
        );
    }

    #[test]
    fn receipt_order_gates_decryption_cross_check_and_admission() {
        let verse = identifier(0x11);
        let petal = Scope::new(verse, Some(identifier(0x21)), None).expect("scope");
        let root = genesis_header(1, Scope::verse_wide(verse), 1);
        let ciphertext = b"payload-ciphertext".to_vec();
        let intent = intent_header(2, petal, vec![root.op_id], 1, &ciphertext);

        let topic = PayloadTopicScope {
            verse_id: verse,
            petal_id: identifier(0x21),
            scope_epoch: 7,
            key_id: identifier(0x41),
        };
        let shard = PayloadShardBody::new(
            topic,
            vec![PayloadShardRecord::of(intent.op_id, ciphertext.clone())],
        )
        .expect("shard");
        let sealed = seal_for_test(
            LaneClass::PayloadShard,
            descriptor(0x41, 4),
            &shard.encode_canonical().expect("plaintext"),
        );
        let bytes = sealed.encode_canonical().expect("bytes");
        let artifact_id = sealed.artifact_id().expect("id");

        let authorized = TagOnlyOpener {
            authorized_keys: vec![identifier(0x41)],
        };
        let relay = TagOnlyOpener {
            authorized_keys: Vec::new(),
        };

        let mut known = KnownReferences::default();
        known.payload_refs.insert(
            intent.op_id,
            PayloadRef {
                ciphertext_hash: Hash32::of(&ciphertext),
                ciphertext_length: ciphertext.len() as u64,
                encryption: Some(EncryptionParams {
                    suite_id: PRODUCTION_SUITE_ID,
                    key_id: identifier(0x4E),
                    nonce: [0x2A; NONCE_LENGTH],
                }),
            },
        );
        known.scopes.insert(intent.op_id, petal);

        let mut relay_receiver = pipeline();
        assert_eq!(
            relay_receiver.receive(
                artifact_id,
                bytes.len() as u64,
                &bytes,
                &relay,
                &KnownReferences::default()
            ),
            ReceiptOutcome::StoredOpaque { artifact_id }
        );
        assert!(relay_receiver.store().has(artifact_id));

        let mut member = pipeline();
        assert!(matches!(
            member.receive(
                artifact_id,
                bytes.len() as u64,
                &bytes,
                &authorized,
                &KnownReferences::default()
            ),
            ReceiptOutcome::Quarantined {
                reason: QuarantineReason::CrossCheckFailure { .. }
            }
        ));
        assert!(!member.store().has(artifact_id));

        assert_eq!(
            member.receive(artifact_id, bytes.len() as u64, &bytes, &authorized, &known),
            ReceiptOutcome::Stored {
                artifact_id,
                lane: LaneClass::PayloadShard,
            }
        );

        let mut tampered_metadata = sealed.clone();
        tampered_metadata.encryption = descriptor(0x41, 5);
        let tampered_bytes = tampered_metadata.encode_canonical().expect("bytes");
        let tampered_id = tampered_metadata.artifact_id().expect("id");
        let mut tampered_receiver = pipeline();
        assert!(matches!(
            tampered_receiver.receive(
                tampered_id,
                tampered_bytes.len() as u64,
                &tampered_bytes,
                &authorized,
                &known
            ),
            ReceiptOutcome::Quarantined {
                reason: QuarantineReason::FailedDecryption
            }
        ));

        let mut short_declaration = pipeline();
        assert!(matches!(
            short_declaration.receive(artifact_id, 1, &bytes, &authorized, &known),
            ReceiptOutcome::Quarantined {
                reason: QuarantineReason::StoredLengthMismatch
            }
        ));
    }

    #[test]
    fn bulk_lane_body_mismatch_rejected() {
        let topic = PayloadTopicScope {
            verse_id: identifier(0x11),
            petal_id: identifier(0x21),
            scope_epoch: 7,
            key_id: identifier(0x41),
        };
        let bulk = BulkArtifactBody::new(topic, ObjectClass::Shard, b"hexon".to_vec())
            .expect("bulk body")
            .encode_canonical()
            .expect("bulk plaintext");
        let shard = PayloadShardBody::new(
            topic,
            vec![PayloadShardRecord::of(
                Hash32([0x0A; 32]),
                b"ciphertext".to_vec(),
            )],
        )
        .expect("shard body")
        .encode_canonical()
        .expect("shard plaintext");

        assert_eq!(
            decode_opened_body(LaneClass::BulkArtifact, &shard),
            Err(SegmentError::LaneBodyMismatch)
        );
        assert!(decode_opened_body(LaneClass::PayloadShard, &bulk).is_err());

        let opener = TagOnlyOpener {
            authorized_keys: vec![identifier(0x41)],
        };
        let view = KnownReferences::default();
        for (nonce, lane, plaintext) in [
            (1u8, LaneClass::BulkArtifact, &shard),
            (2, LaneClass::PayloadShard, &bulk),
        ] {
            let sealed = seal_for_test(lane, descriptor(0x41, nonce), plaintext);
            let bytes = sealed.encode_canonical().expect("bytes");
            let artifact_id = sealed.artifact_id().expect("id");
            let mut receiver = pipeline();
            assert_eq!(
                receiver.receive(artifact_id, bytes.len() as u64, &bytes, &opener, &view),
                ReceiptOutcome::Quarantined {
                    reason: QuarantineReason::MalformedLaneBody
                },
                "{lane:?} outer map over the other lane's body"
            );
            assert!(!receiver.store().has(artifact_id));
        }

        // A body naming one topic key, sealed under another, mixes key identifiers (§3.4.3).
        let other_key = TagOnlyOpener {
            authorized_keys: vec![identifier(0x41), identifier(0x42)],
        };
        let sealed = seal_for_test(LaneClass::BulkArtifact, descriptor(0x42, 4), &bulk);
        let bytes = sealed.encode_canonical().expect("bytes");
        let artifact_id = sealed.artifact_id().expect("id");
        let mut receiver = pipeline();
        assert_eq!(
            receiver.receive(artifact_id, bytes.len() as u64, &bytes, &other_key, &view),
            ReceiptOutcome::Quarantined {
                reason: QuarantineReason::MalformedLaneBody
            }
        );
        assert!(!receiver.store().has(artifact_id));

        // The honest pairing is admitted with no header cross-reference (§3.4 rules 5 and 8).
        let sealed = seal_for_test(LaneClass::BulkArtifact, descriptor(0x41, 3), &bulk);
        let bytes = sealed.encode_canonical().expect("bytes");
        let artifact_id = sealed.artifact_id().expect("id");
        let mut receiver = pipeline();
        assert_eq!(
            receiver.receive(artifact_id, bytes.len() as u64, &bytes, &opener, &view),
            ReceiptOutcome::Stored {
                artifact_id,
                lane: LaneClass::BulkArtifact,
            }
        );
    }
}
