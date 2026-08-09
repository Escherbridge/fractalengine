//! Content-addressed sealed-artifact storage and the bounded quarantine (SPEC-6 §2.1.6, §5.5).
//!
//! Storage is a trait seam only: this crate has no database and no I/O.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::num::NonZeroUsize;

use crate::envelope::Hash32;

use super::artifact::verify_artifact_id;
use super::SegmentError;

/// Local content-addressed store of sealed artifacts; wave 3 implements it over fe-database.
pub trait SealedArtifactStore {
    /// Recomputes BLAKE3, refuses a mismatched claim or a conflicting rewrite, then stores.
    ///
    /// The only library caller is [`super::receipt::ReceiptPipeline::receive`]; a `have`,
    /// announcement, or cache hit is never a reason to reach this method.
    fn put_verified(&mut self, claimed_id: Hash32, bytes: &[u8]) -> Result<Hash32, SegmentError>;

    /// Returns the exact stored bytes, or `None`; quarantined bytes are never reachable here.
    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>>;

    /// Reports presence without disclosing bytes; presence is not validity or authority (§5.6).
    fn has(&self, artifact_id: Hash32) -> bool;
}

/// Enforces §2.1.6: an existing artifact ID is never re-aliased to a different byte sequence.
pub fn assert_immutable_write(
    existing: Option<&[u8]>,
    artifact_id: Hash32,
    incoming: &[u8],
) -> Result<(), SegmentError> {
    match existing {
        Some(stored) if stored != incoming => {
            Err(SegmentError::ImmutableArtifactConflict { artifact_id })
        }
        _ => Ok(()),
    }
}

/// In-memory `SealedArtifactStore` for unit tests and for local, non-durable caches.
#[derive(Clone, Debug, Default)]
pub struct InMemorySealedArtifactStore {
    artifacts: BTreeMap<Hash32, Vec<u8>>,
}

impl InMemorySealedArtifactStore {
    /// Builds an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct artifacts retained.
    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    /// Reports whether the store holds nothing.
    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }
}

impl SealedArtifactStore for InMemorySealedArtifactStore {
    fn put_verified(&mut self, claimed_id: Hash32, bytes: &[u8]) -> Result<Hash32, SegmentError> {
        let artifact_id = verify_artifact_id(bytes, claimed_id)?;
        assert_immutable_write(
            self.artifacts.get(&artifact_id).map(Vec::as_slice),
            artifact_id,
            bytes,
        )?;
        self.artifacts.insert(artifact_id, bytes.to_vec());
        Ok(artifact_id)
    }

    fn get(&self, artifact_id: Hash32) -> Option<Vec<u8>> {
        self.artifacts.get(&artifact_id).cloned()
    }

    fn has(&self, artifact_id: Hash32) -> bool {
        self.artifacts.contains_key(&artifact_id)
    }
}

/// Why a receipt was refused; retained with the offending bytes for diagnostics only (§5.5).
///
/// The crate's single quarantine vocabulary, shared with SPEC-2 lifecycle admission and SPEC-5
/// pre-admission holding; see `src/AGENTS.md` §unified-vocabularies.
pub use crate::compose::QuarantineReason;

impl QuarantineReason {
    /// Classifies a refusal so the quarantine reason survives without the full error chain.
    pub fn classify(error: &SegmentError) -> Self {
        match error {
            SegmentError::ArtifactIdMismatch { claimed, computed } => Self::ArtifactIdMismatch {
                claimed: *claimed,
                computed: *computed,
            },
            SegmentError::StoredLengthMismatch { .. }
            | SegmentError::IncompleteRangeReassembly { .. }
            | SegmentError::RangeChecksumMismatch { .. } => Self::StoredLengthMismatch,
            SegmentError::AuthorEquivocation {
                equivocation_key,
                first_op_id,
                second_op_id,
            } => Self::AuthorEquivocation {
                equivocation_key: *equivocation_key,
                first_op_id: *first_op_id,
                second_op_id: *second_op_id,
            },
            SegmentError::DecryptionFailed => Self::FailedDecryption,
            SegmentError::ScopeKeyUnavailable
            | SegmentError::Unauthorized
            | SegmentError::StaleScopeEpoch { .. }
            | SegmentError::StaleScopeKey { .. } => Self::Unauthorized,
            SegmentError::UnreferencedPayloadRecord { op_id }
            | SegmentError::PayloadRecordHeaderMismatch { op_id }
            | SegmentError::MixedPayloadTopicScope { op_id } => {
                Self::CrossCheckFailure { op_id: *op_id }
            }
            SegmentError::Cbor(_)
            | SegmentError::UnsupportedArtifactFormatVersion { .. }
            | SegmentError::UnknownLaneClass { .. }
            | SegmentError::CiphertextLengthMismatch { .. }
            | SegmentError::EmptyCiphertext
            | SegmentError::Envelope(_) => Self::MalformedSealedArtifact,
            _ => Self::MalformedLaneBody,
        }
    }
}

/// A refused receipt retained for diagnostics or retry; never verified, never served (§5.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarantineEntry {
    /// Classification of the refusal.
    pub reason: QuarantineReason,
    /// The identifier the sender claimed, when it supplied one.
    pub claimed_id: Option<Hash32>,
    /// The untrusted bytes.
    pub bytes: Vec<u8>,
}

/// Capacity-bounded FIFO of refused receipts.
///
/// Capacity and retention are SPEC-5 policy numbers under D-CL24 and stay caller-supplied;
/// there is deliberately no `Default` that would pick one.
#[derive(Clone, Debug)]
pub struct Quarantine {
    capacity: usize,
    entries: VecDeque<QuarantineEntry>,
}

impl Quarantine {
    /// Builds a quarantine retaining at most `capacity` most-recent refusals.
    pub fn with_capacity(capacity: NonZeroUsize) -> Self {
        Self {
            capacity: capacity.get(),
            entries: VecDeque::new(),
        }
    }

    /// Retains a refusal, evicting the oldest entry once the caller's bound is reached.
    pub fn retain(&mut self, entry: QuarantineEntry) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Number of retained refusals.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Reports whether nothing is quarantined.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The caller's retention bound.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Retained refusals, oldest first.
    pub fn entries(&self) -> impl Iterator<Item = &QuarantineEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{EquivocationKey, Identifier32, NONCE_LENGTH, PRODUCTION_SUITE_ID};
    use crate::segment::artifact::{EncryptionDescriptor, LaneClass, SealedArtifact};

    fn sealed_bytes(body: &[u8]) -> (Hash32, Vec<u8>) {
        let artifact = SealedArtifact::seal(
            LaneClass::HeaderSegment,
            EncryptionDescriptor::new(
                PRODUCTION_SUITE_ID,
                Identifier32([1; 32]),
                [7; NONCE_LENGTH],
            ),
            body.to_vec(),
        )
        .expect("sealed");
        (
            artifact.artifact_id().expect("id"),
            artifact.encode_canonical().expect("bytes"),
        )
    }

    #[test]
    fn immutable_segment_never_overwrites_prior_bytes() {
        let mut store = InMemorySealedArtifactStore::new();
        let (artifact_id, bytes) = sealed_bytes(b"first-body");
        let (other_id, other_bytes) = sealed_bytes(b"other-body");

        assert_eq!(store.put_verified(artifact_id, &bytes), Ok(artifact_id));
        assert!(store.has(artifact_id));
        assert_eq!(store.get(artifact_id).as_deref(), Some(bytes.as_slice()));

        assert_eq!(store.put_verified(artifact_id, &bytes), Ok(artifact_id));
        assert_eq!(store.len(), 1);

        assert_eq!(
            store.put_verified(artifact_id, &other_bytes),
            Err(SegmentError::ArtifactIdMismatch {
                claimed: artifact_id,
                computed: other_id,
            })
        );
        assert_eq!(store.get(artifact_id).as_deref(), Some(bytes.as_slice()));
        assert!(!store.has(other_id));

        assert_eq!(
            assert_immutable_write(Some(bytes.as_slice()), artifact_id, &other_bytes),
            Err(SegmentError::ImmutableArtifactConflict { artifact_id })
        );
        assert_eq!(
            assert_immutable_write(Some(bytes.as_slice()), artifact_id, &bytes),
            Ok(())
        );
    }

    #[test]
    fn quarantine_is_bounded_and_never_reachable_through_get() {
        let mut store = InMemorySealedArtifactStore::new();
        let mut quarantine = Quarantine::with_capacity(NonZeroUsize::new(2).expect("capacity"));
        let (artifact_id, bytes) = sealed_bytes(b"refused-body");

        for filler in 0u8..3 {
            quarantine.retain(QuarantineEntry {
                reason: QuarantineReason::MalformedLaneBody,
                claimed_id: Some(Hash32([filler; 32])),
                bytes: bytes.clone(),
            });
        }

        assert_eq!(quarantine.capacity(), 2);
        assert_eq!(quarantine.len(), 2);
        assert!(!quarantine.is_empty());
        let claimed: Vec<Option<Hash32>> = quarantine.entries().map(|e| e.claimed_id).collect();
        assert_eq!(claimed, vec![Some(Hash32([1; 32])), Some(Hash32([2; 32]))]);

        assert!(!store.has(artifact_id));
        assert_eq!(store.get(artifact_id), None);
        assert!(store.is_empty());
        assert_eq!(store.put_verified(artifact_id, &bytes), Ok(artifact_id));
    }

    #[test]
    fn equivocation_is_representable_and_quarantines_both_candidates() {
        let first = Hash32([1; 32]);
        let second = Hash32([2; 32]);
        let equivocation_key = EquivocationKey {
            author_public_key: [9; 32],
            wall_ms: 42,
            counter: 3,
        };
        let reason = QuarantineReason::classify(&SegmentError::AuthorEquivocation {
            equivocation_key,
            first_op_id: first,
            second_op_id: second,
        });
        let QuarantineReason::AuthorEquivocation {
            equivocation_key: retained,
            first_op_id,
            second_op_id,
        } = reason
        else {
            panic!("equivocation must classify as author equivocation")
        };
        assert_eq!(retained, equivocation_key);
        assert_eq!((first_op_id, second_op_id), (first, second));

        let mut quarantine = Quarantine::with_capacity(NonZeroUsize::new(4).expect("capacity"));
        for candidate in [first, second] {
            quarantine.retain(QuarantineEntry {
                reason: reason.clone(),
                claimed_id: Some(candidate),
                bytes: Vec::new(),
            });
        }
        assert_eq!(quarantine.len(), 2);
    }
}
