//! Sealed delivery artifacts, lanes, HashSeq reachability, and relay policy (SPEC-6).
//!
//! Pure local artifact handling: no socket, no listener, no iroh, no libp2p, no fe-network and
//! no fe-sync. Rationale and every provisional wire choice live in `src/segment/AGENTS.md`.

pub mod artifact;
pub mod checkpoint_proof;
pub mod discovery_labels;
pub mod hashseq;
pub mod header_lane;
pub mod manifest;
pub mod payload_shard;
pub mod receipt;
pub mod relay_policy;
pub mod store;

use thiserror::Error;

use crate::capability::CapabilityError;
use crate::cbor::{CborError, CborValue};
use crate::checkpoint::CheckpointError;
use crate::envelope::{EnvelopeError, EquivocationKey, Hash32, Identifier32};
use crate::frontier::FrontierError;
use crate::signing::SigningError;

/// Every reason a sealed artifact, lane body, reachability walk, or relay decision is refused.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SegmentError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// An enclosed SPEC-1 envelope was malformed.
    #[error("envelope: {0}")]
    Envelope(#[from] EnvelopeError),
    /// An enclosed SPEC-1 envelope failed admission.
    #[error("signing: {0}")]
    Signing(#[from] SigningError),
    /// A frontier handed to a proof was not a well-formed D-CL19 frontier.
    #[error("frontier: {0}")]
    Frontier(#[from] FrontierError),
    /// The bound SPEC-5 checkpoint claim failed to decode, bind, or authenticate.
    #[error("checkpoint claim: {0}")]
    Checkpoint(Box<CheckpointError>),
    /// The SPEC-3 topic label or its keyed derivation was refused.
    #[error("capability: {0}")]
    Capability(Box<CapabilityError>),
    /// A value that must be a CBOR map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// Map being parsed.
        context: &'static str,
    },
    /// A value that must be a CBOR array was not one.
    #[error("{context} is not a CBOR array")]
    ExpectedArray {
        /// Value being parsed.
        context: &'static str,
    },
    /// A listed key was absent.
    #[error("{context} key {key} is missing")]
    MissingField {
        /// Map being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key the grammar does not list was present, or a key was not an unsigned integer.
    #[error("{context} carries an unknown or non-integer key")]
    UnknownField {
        /// Map being parsed.
        context: &'static str,
    },
    /// A key held a value of the wrong CBOR type.
    #[error("{context} key {key} holds the wrong CBOR type")]
    FieldTypeMismatch {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A byte string had the wrong fixed length.
    #[error("{context} key {key} holds {actual} byte(s), expected {expected}")]
    WrongByteLength {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// Length the grammar requires.
        expected: usize,
        /// Length actually present.
        actual: usize,
    },
    /// An integer exceeded the width its grammar row declares.
    #[error("{context} key {key} value {value} exceeds its declared integer width")]
    IntegerOutOfRange {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
        /// The out-of-range value.
        value: u64,
    },
    /// The bytes were not the canonical encoding of the value they decoded to (§2.1.5).
    #[error("{context} bytes are not their own canonical re-encoding")]
    NonCanonicalEncoding {
        /// Artifact or body being parsed.
        context: &'static str,
    },
    /// The outer map declared a format version this build does not implement.
    #[error("sealed artifact format version {version} is not 1")]
    UnsupportedArtifactFormatVersion {
        /// The version actually present.
        version: u64,
    },
    /// The outer map declared a lane class outside the four SPEC-6 lanes.
    #[error("lane class {lane_class} is not a SPEC-6 lane")]
    UnknownLaneClass {
        /// The unrecognized lane class.
        lane_class: u64,
    },
    /// The decrypted body was not the body form its lane class declares (§2.1.5).
    #[error("sealed body does not match its declared lane class")]
    LaneBodyMismatch,
    /// The declared ciphertext length disagreed with the ciphertext actually carried (§2.1.4).
    #[error("declared ciphertext length {declared} but carried {actual} byte(s)")]
    CiphertextLengthMismatch {
        /// Length the outer map declared.
        declared: u64,
        /// Length actually carried.
        actual: u64,
    },
    /// A stored artifact carried an empty ciphertext, which no sealed lane may (§2.2.1, §9.1).
    #[error("a sealed artifact must carry a non-empty ciphertext; there is no plaintext lane")]
    EmptyCiphertext,
    /// The recomputed BLAKE3 digest disagreed with the requested artifact ID (§2.1.4).
    #[error("artifact ID mismatch: claimed {claimed:?}, recomputed {computed:?}")]
    ArtifactIdMismatch {
        /// The requested identifier.
        claimed: Hash32,
        /// The identifier the bytes actually have.
        computed: Hash32,
    },
    /// The declared stored byte length disagreed with the bytes received (§2.1.4).
    #[error("stored length mismatch: declared {declared}, received {received}")]
    StoredLengthMismatch {
        /// Length the transport declared.
        declared: u64,
        /// Length actually received.
        received: u64,
    },
    /// A different byte sequence already exists at this artifact ID (§2.1.6).
    #[error("artifact {artifact_id:?} already holds different bytes and is immutable")]
    ImmutableArtifactConflict {
        /// The contested identifier.
        artifact_id: Hash32,
    },
    /// A reassembled range transfer was incomplete or overlapped inconsistently (§5.2).
    #[error("range reassembly is incomplete or inconsistent at offset {offset}")]
    IncompleteRangeReassembly {
        /// Offset at which reassembly failed.
        offset: u64,
    },
    /// A per-range checksum did not match the range bytes (§5.2).
    #[error("per-range checksum mismatch at offset {offset}")]
    RangeChecksumMismatch {
        /// Offset of the offending range.
        offset: u64,
    },
    /// A header record belonged to a different verse than its header segment (§3.1.2).
    #[error("header {op_id:?} belongs to a foreign verse")]
    ForeignVerseId {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A lane body carried no records, which no sealed lane may (§3.1.1, §3.2.1).
    #[error("{context} carries no records")]
    EmptyLaneBody {
        /// Body being parsed.
        context: &'static str,
    },
    /// One operation ID appeared twice with differing bytes (§3.1.4).
    #[error("operation {op_id:?} appears twice with differing bytes")]
    ConflictingDuplicateOperationId {
        /// The repeated operation.
        op_id: Hash32,
    },
    /// One operation ID appeared twice in a lane body that admits each exactly once (§3.2.3).
    #[error("operation {op_id:?} appears more than once in this lane body")]
    DuplicateOperationId {
        /// The repeated operation.
        op_id: Hash32,
    },
    /// Two distinct operations shared one equivocation key (§3.4, D-CL25).
    ///
    /// The rule is quarantine both candidates and materialize neither.
    #[error(
        "author equivocation: {first_op_id:?} and {second_op_id:?} share {equivocation_key:?}"
    )]
    AuthorEquivocation {
        /// The author, wall clock, and counter the two operations share.
        equivocation_key: EquivocationKey,
        /// The first observed operation.
        first_op_id: Hash32,
        /// The conflicting operation.
        second_op_id: Hash32,
    },
    /// Records in one lane body were not in strictly ascending operation-ID order.
    #[error("{context} records are not in strictly ascending operation-ID order")]
    RecordsNotStrictlyAscending {
        /// Body being parsed.
        context: &'static str,
    },
    /// A shard record's ciphertext hash or length disagreed with the record bytes (§3.2.3).
    #[error("payload record {op_id:?} does not match its own ciphertext hash or length")]
    PayloadRecordSelfMismatch {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A shard record disagreed with the payload reference in its header (§3.2.1).
    #[error("payload record {op_id:?} does not match the payload reference in its header")]
    PayloadRecordHeaderMismatch {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A shard record named an operation no known header references (§5.4).
    #[error("payload record {op_id:?} is not referenced by any known header")]
    UnreferencedPayloadRecord {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A shard mixed petals, verses, scope epochs, or key identifiers (§3.2.2).
    #[error("payload record {op_id:?} resolves outside this shard's single payload-topic scope")]
    MixedPayloadTopicScope {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A sealing candidate exceeded the effective `max_segment_bytes` caveat (§3.2.5).
    #[error("sealing candidate is {candidate} byte(s), over the {maximum}-byte caveat")]
    OversizedSegment {
        /// Projected stored size of the candidate.
        candidate: u64,
        /// Effective caveat bound.
        maximum: u64,
    },
    /// A lane's HashSeq covered an artifact bound to a different lane or verse (§4.1.3).
    #[error("artifact {artifact_id:?} is bound to a different lane or verse")]
    ForeignLaneArtifact {
        /// The offending artifact.
        artifact_id: Hash32,
    },
    /// A manifest listed a payload topic belonging to another verse (§3.3.3).
    #[error("manifest payload topic for petal {petal_id:?} belongs to another verse")]
    ManifestForeignPayloadTopic {
        /// Petal of the offending topic.
        petal_id: Identifier32,
    },
    /// A manifest lane carried roots without an availability boundary, or the reverse (§3.3.1).
    #[error("manifest availability boundary does not cover exactly the lanes that carry roots")]
    ManifestBoundaryLaneMismatch,
    /// A manifest listed one artifact ID twice with differing stored lengths (§3.3.2).
    #[error("manifest root {artifact_id:?} is listed with two different stored lengths")]
    ConflictingStoredLength {
        /// The contested identifier.
        artifact_id: Hash32,
    },
    /// A manifest's clear statistics disagreed with the lanes it claims (§3.3.5).
    #[error("manifest clear statistics are inconsistent with the lanes the manifest claims")]
    ManifestStatisticsInconsistent,
    /// A statistics petal set was not strictly ascending, so its bytes are non-canonical
    /// (§3.3.5).
    #[error("manifest statistics petal set is not strictly ascending")]
    ManifestStatisticsPetalsNotAscending,
    /// A sealed statistics reference named a lane the manifest does not claim (§3.3.6).
    #[error("manifest sealed statistics reference names a lane the manifest does not claim")]
    ManifestStatisticsForeignLane,
    /// A HashSeq node carried no entries, which §4.1.1 forbids.
    #[error("a HashSeq node must carry at least one entry")]
    EmptyHashSeqNode,
    /// One artifact ID appeared twice inside a single HashSeq node (§4.1.2).
    #[error("HashSeq node lists artifact {artifact_id:?} twice")]
    DuplicateHashSeqEntry {
        /// The repeated identifier.
        artifact_id: Hash32,
    },
    /// A predecessor changed the lane or the exact scope binding (§4.1.3).
    #[error("HashSeq predecessor {node_id:?} changes the lane or scope binding")]
    HashSeqBindingMismatch {
        /// The offending node.
        node_id: Hash32,
    },
    /// A HashSeq path revisited a node (§4.1.3).
    #[error("HashSeq path revisits node {node_id:?}")]
    HashSeqCycle {
        /// The revisited node.
        node_id: Hash32,
    },
    /// A required predecessor could not be opened (§4.1.3).
    #[error("HashSeq predecessor {node_id:?} is missing")]
    MissingHashSeqPredecessor {
        /// The absent node.
        node_id: Hash32,
    },
    /// A walk reached a null predecessor without meeting the declared boundary (§4.1.4).
    #[error("HashSeq walk ended before reaching boundary node {boundary:?}")]
    BoundaryUnreachable {
        /// The boundary the manifest declared.
        boundary: Hash32,
    },
    /// A caller-supplied traversal bound was exhausted.
    #[error("traversal exceeded the caller's bound of {maximum} step(s)")]
    TraversalBoundExceeded {
        /// The caller's bound.
        maximum: usize,
    },
    /// An artifact the proof required was not available.
    #[error("artifact {artifact_id:?} is unavailable")]
    ArtifactUnavailable {
        /// The absent identifier.
        artifact_id: Hash32,
    },
    /// A header the selected-frontier parent closure requires was absent (§4.2.5).
    #[error("header {op_id:?} in the selected parent closure is missing")]
    MissingRequiredHeader {
        /// The absent operation.
        op_id: Hash32,
    },
    /// A payload the projection requires was absent (§4.2.5).
    #[error("payload for operation {op_id:?} is missing")]
    MissingRequiredPayload {
        /// The operation whose payload is absent.
        op_id: Hash32,
    },
    /// A checkpoint bound a manifest, branch, or verse other than the one supplied (§4.2.1).
    #[error("checkpoint does not bind the supplied manifest, branch, or verse")]
    CheckpointBindingMismatch,
    /// The frontier the proof would walk is not the one the checkpoint signature covers (§4.2.1).
    #[error(
        "frontier commitment mismatch: claim carries {claimed:?}, heads commit to {computed:?}"
    )]
    FrontierCommitmentMismatch {
        /// Commitment the signed claim carries.
        claimed: Hash32,
        /// Commitment recomputed over the heads the proof would walk.
        computed: Hash32,
    },
    /// A seal request named a lane class the lane key cannot carry (§4.1.3).
    #[error("lane class {lane_class} does not belong to the requested lane")]
    SealLaneMismatch {
        /// The requested lane class.
        lane_class: u64,
    },
    /// The scope key for a lane was unavailable or not currently authorized (§4.2.5, §5.3).
    #[error("no current decrypt authority for this lane")]
    ScopeKeyUnavailable,
    /// Authenticated decryption of a sealed body failed (§5.3).
    #[error("sealed body failed authenticated decryption")]
    DecryptionFailed,
    /// A request named a scope epoch the persistent authorization view has superseded (§6.4).
    #[error("scope epoch {requested} is stale; the authorization view is at {current}")]
    StaleScopeEpoch {
        /// The epoch the requester used.
        requested: u64,
        /// The epoch the persistent view reports.
        current: u64,
    },
    /// A sealing or serving path used a key identifier that is not the lane's current key (§9.2).
    #[error("key identifier {key_id:?} is not the current key for this lane")]
    StaleScopeKey {
        /// The superseded key identifier.
        key_id: Identifier32,
    },
    /// A nonce was reused under one key identifier, which XChaCha20-Poly1305 forbids (§9.1).
    #[error("nonce reuse under key {key_id:?}")]
    NonceReuse {
        /// The key the nonce was reused under.
        key_id: Identifier32,
    },
    /// The persistent authorization view reports no epoch for a lane (§6.4).
    #[error("the authorization view knows no epoch for this lane")]
    UnknownLane,
    /// A capability the operation requires was absent or revoked.
    #[error("the requester holds no current capability for this operation")]
    Unauthorized,
}

impl From<CheckpointError> for SegmentError {
    /// Boxed so one large sibling taxonomy does not widen every `Result` in this module.
    fn from(error: CheckpointError) -> Self {
        Self::Checkpoint(Box::new(error))
    }
}

impl From<CapabilityError> for SegmentError {
    /// Boxed for the same reason as [`SegmentError::Checkpoint`].
    fn from(error: CapabilityError) -> Self {
        Self::Capability(Box::new(error))
    }
}

/// Borrows a map's entries or refuses the value.
fn map_entries<'a>(
    value: &'a CborValue,
    context: &'static str,
) -> Result<&'a [(CborValue, CborValue)], SegmentError> {
    value.as_map().ok_or(SegmentError::ExpectedMap { context })
}

/// Rejects a map holding any key outside `expected`, a non-integer key, or a missing key.
fn require_uint_keys(
    value: &CborValue,
    expected: &[u64],
    context: &'static str,
) -> Result<(), SegmentError> {
    let entries = map_entries(value, context)?;
    for (key, _) in entries {
        let Some(key) = key.as_uint() else {
            return Err(SegmentError::UnknownField { context });
        };
        if !expected.contains(&key) {
            return Err(SegmentError::UnknownField { context });
        }
    }
    for key in expected {
        if value.get_uint_key(*key).is_none() {
            return Err(SegmentError::MissingField { context, key: *key });
        }
    }
    Ok(())
}

/// Borrows the value at an integer key.
fn value_at<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, SegmentError> {
    value
        .get_uint_key(key)
        .ok_or(SegmentError::MissingField { context, key })
}

/// Reads an unsigned integer at an integer key.
fn uint_at(value: &CborValue, key: u64, context: &'static str) -> Result<u64, SegmentError> {
    value_at(value, key, context)?
        .as_uint()
        .ok_or(SegmentError::FieldTypeMismatch { context, key })
}

/// Reads a fixed-length byte string at an integer key.
fn bytes_at<const N: usize>(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; N], SegmentError> {
    let raw = value_at(value, key, context)?
        .as_bytes()
        .ok_or(SegmentError::FieldTypeMismatch { context, key })?;
    <[u8; N]>::try_from(raw).map_err(|_| SegmentError::WrongByteLength {
        context,
        key,
        expected: N,
        actual: raw.len(),
    })
}

/// Reads a variable-length byte string at an integer key.
fn byte_string_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Vec<u8>, SegmentError> {
    Ok(value_at(value, key, context)?
        .as_bytes()
        .ok_or(SegmentError::FieldTypeMismatch { context, key })?
        .to_vec())
}

/// Reads a 32-byte digest at an integer key.
fn hash32_at(value: &CborValue, key: u64, context: &'static str) -> Result<Hash32, SegmentError> {
    Ok(Hash32(bytes_at::<32>(value, key, context)?))
}

/// Reads a 32-byte identifier at an integer key.
fn identifier32_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Identifier32, SegmentError> {
    Ok(Identifier32(bytes_at::<32>(value, key, context)?))
}

/// Borrows an array at an integer key.
fn array_at<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a [CborValue], SegmentError> {
    value_at(value, key, context)?
        .as_array()
        .ok_or(SegmentError::FieldTypeMismatch { context, key })
}

/// Borrows a map's entries at an integer key.
fn map_at<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a [(CborValue, CborValue)], SegmentError> {
    value_at(value, key, context)?
        .as_map()
        .ok_or(SegmentError::FieldTypeMismatch { context, key })
}

/// Encodes canonically and refuses bytes that are not their own canonical re-encoding.
fn assert_canonical_bytes(
    value: &CborValue,
    received: &[u8],
    context: &'static str,
) -> Result<(), SegmentError> {
    if crate::cbor::encode_canonical_checked(value)? != received {
        return Err(SegmentError::NonCanonicalEncoding { context });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    //! Deterministic SPEC-1 envelopes the SPEC-6 lane tests seal, walk, and cross-check.

    use ed25519_dalek::SigningKey;

    use crate::envelope::{
        Author, CapabilityRef, CompleteEnvelope, EncryptionParams, Hash32, Hlc, Identifier32,
        PayloadRef, Scope, UnsignedEnvelope, NONCE_LENGTH, PRODUCTION_SUITE_ID, PROTOCOL_VERSION,
    };
    use crate::signing::sign_envelope;

    /// A signed envelope together with the exact bytes and identifier a lane record carries.
    #[derive(Clone, Debug)]
    pub(crate) struct HeaderFixture {
        pub op_id: Hash32,
        pub bytes: Vec<u8>,
        pub envelope: CompleteEnvelope,
        pub ciphertext: Vec<u8>,
    }

    pub(crate) fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    pub(crate) fn digest(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    pub(crate) fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed.wrapping_add(1); 32])
    }

    fn finish(key: &SigningKey, unsigned: UnsignedEnvelope, ciphertext: Vec<u8>) -> HeaderFixture {
        let envelope = sign_envelope(key, &unsigned).expect("signed envelope");
        let bytes = envelope.encode_canonical().expect("canonical bytes");
        HeaderFixture {
            op_id: Hash32::of(&bytes),
            bytes,
            envelope,
            ciphertext,
        }
    }

    /// A kind-2 branch genesis: parentless and payload-free.
    pub(crate) fn genesis_header(seed: u8, scope: Scope, counter: u32) -> HeaderFixture {
        let key = signing_key(seed);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 2,
            scope,
            author: Author::from_public_key(key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: digest(0xC0),
                scope_epoch: 1,
            },
            schema_hash: digest(0x5C),
            branch_id: identifier(0xB1),
            parents: Vec::new(),
            hlc: Hlc::new(1_700_000_000_000, counter),
            payload: PayloadRef::empty(),
        };
        finish(&key, unsigned, Vec::new())
    }

    /// A kind-1 normal intent carrying an encrypted non-empty payload.
    pub(crate) fn intent_header(
        seed: u8,
        scope: Scope,
        parents: Vec<Hash32>,
        counter: u32,
        ciphertext: &[u8],
    ) -> HeaderFixture {
        let key = signing_key(seed);
        let unsigned = UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 1,
            scope,
            author: Author::from_public_key(key.verifying_key().to_bytes()),
            capability: CapabilityRef {
                chain_id: digest(0xC0),
                scope_epoch: 1,
            },
            schema_hash: digest(0x5C),
            branch_id: identifier(0xB1),
            parents,
            hlc: Hlc::new(1_700_000_000_001, counter),
            payload: PayloadRef {
                ciphertext_hash: Hash32::of(ciphertext),
                ciphertext_length: ciphertext.len() as u64,
                encryption: Some(EncryptionParams {
                    suite_id: PRODUCTION_SUITE_ID,
                    key_id: identifier(0x4E),
                    nonce: [0x2A; NONCE_LENGTH],
                }),
            },
        };
        finish(&key, unsigned, ciphertext.to_vec())
    }
}
