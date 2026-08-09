//! Operation envelope grammar and admission (SPEC-1 §3); see `src/AGENTS.md` §module-ownership.

use std::cmp::Ordering;
use std::fmt;

use thiserror::Error;

use crate::cbor::{
    decode_canonical as decode_cbor, encode_canonical_checked as encode_cbor_checked, CborError,
    CborValue,
};
use crate::did_key::public_key_to_did;

/// Protocol version every v1 envelope MUST carry.
pub const PROTOCOL_VERSION: u64 = 1;

/// The only payload suite v1 admits on production paths (XChaCha20-Poly1305).
pub const PRODUCTION_SUITE_ID: u16 = 1;

/// Suite reserved solely for the repository encoding fixture; forbidden on production paths.
pub const FIXTURE_ONLY_SUITE_ID: u16 = 65535;

/// Unconditional XChaCha20-Poly1305 nonce length in bytes (192 bits).
pub const NONCE_LENGTH: usize = 24;

/// Ed25519 signature length in bytes.
pub const SIGNATURE_LENGTH: usize = 64;

/// Every reason an operation envelope fails to encode, decode, or satisfy §3.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EnvelopeError {
    /// The bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// A value that must be a map was not one.
    #[error("{context} is not a CBOR map")]
    ExpectedMap {
        /// Map being parsed.
        context: &'static str,
    },
    /// A value that must be an array was not one.
    #[error("{context} is not a CBOR array")]
    ExpectedArray {
        /// Value being parsed.
        context: &'static str,
    },
    /// A map did not hold exactly the listed integer keys, once each, and nothing else.
    #[error("{context} must hold exactly the integer keys 0..{expected_key_count} once each")]
    UnexpectedMapKeys {
        /// Map being parsed.
        context: &'static str,
        /// Number of integer keys the grammar lists.
        expected_key_count: u64,
    },
    /// A listed key was absent.
    #[error("{context} key {key} is missing")]
    MissingKey {
        /// Map being parsed.
        context: &'static str,
        /// The absent key.
        key: u64,
    },
    /// A key held something other than an unsigned integer.
    #[error("{context} key {key} is not an unsigned integer")]
    ExpectedUnsignedInteger {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A key held something other than a byte string.
    #[error("{context} key {key} is not a byte string")]
    ExpectedByteString {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A key held something other than a text string.
    #[error("{context} key {key} is not a text string")]
    ExpectedTextString {
        /// Map being parsed.
        context: &'static str,
        /// The offending key.
        key: u64,
    },
    /// A nullable identifier slot held neither `null` nor a byte string.
    #[error("{context} key {key} must be null or a byte string")]
    ExpectedNullOrByteString {
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
    /// `protocol_version` was not 1.
    #[error("protocol_version {version} is not 1")]
    UnsupportedProtocolVersion {
        /// The version actually present.
        version: u64,
    },
    /// `operation_kind` was zero.
    #[error("operation_kind must be non-zero")]
    ZeroOperationKind,
    /// A resource-scoped envelope had a null petal (§3.1).
    #[error("scope.resource_id requires a non-null scope.petal_id")]
    ResourceWithoutPetal,
    /// `parents` was not strictly ascending and unique (§6.1).
    #[error("parents[{index}] does not strictly follow its predecessor")]
    ParentsNotStrictlyAscending {
        /// Index of the offending parent.
        index: usize,
    },
    /// `encryption` was null without the exact no-payload form (§3.5).
    #[error("the no-payload form requires ciphertext_length 0 and ciphertext_hash BLAKE3(empty)")]
    MalformedNoPayloadForm,
    /// The fixture-only suite reached a production path.
    #[error("payload suite 65535 is fixture-only and is forbidden on production paths")]
    FixtureOnlySuite,
    /// A suite other than the v1 production suite was requested.
    #[error("payload suite {suite_id} is not the v1 production suite")]
    UnsupportedPayloadSuite {
        /// The rejected suite.
        suite_id: u16,
    },
    /// `encryption.suite_id` was zero, which no suite registry entry may ever use.
    #[error("payload suite identifier must be greater than zero")]
    ZeroPayloadSuite,
}

/// A 32-byte unkeyed BLAKE3 digest, ordered byte-lexicographically.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash32(
    /// Raw digest bytes.
    pub [u8; 32],
);

impl Hash32 {
    /// Unkeyed 32-byte BLAKE3 digest of `data`.
    pub fn of(data: &[u8]) -> Self {
        Self(*blake3::hash(data).as_bytes())
    }

    /// BLAKE3 of the empty byte string, the no-payload ciphertext commitment.
    pub fn of_empty() -> Self {
        Self::of(&[])
    }

    /// Raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for Hash32 {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for Hash32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Hash32(")?;
        write_hexadecimal(&self.0, formatter)?;
        formatter.write_str(")")
    }
}

/// A 32-byte opaque identifier, ordered byte-lexicographically.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Identifier32(
    /// Raw identifier bytes.
    pub [u8; 32],
);

impl Identifier32 {
    /// Raw identifier bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for Identifier32 {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for Identifier32 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Identifier32(")?;
        write_hexadecimal(&self.0, formatter)?;
        formatter.write_str(")")
    }
}

fn write_hexadecimal(bytes: &[u8], formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    for byte in bytes {
        write!(formatter, "{byte:02x}")?;
    }
    Ok(())
}

/// The `(verse, petal?, resource?)` authorization scope tuple of §3.1.
///
/// Fields are private so an invalid tuple cannot reach [`Scope::contains`]; see
/// `src/AGENTS.md` §scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Scope {
    verse_id: Identifier32,
    petal_id: Option<Identifier32>,
    resource_id: Option<Identifier32>,
}

impl Scope {
    /// Builds a scope, rejecting a resource without a petal.
    pub fn new(
        verse_id: Identifier32,
        petal_id: Option<Identifier32>,
        resource_id: Option<Identifier32>,
    ) -> Result<Self, EnvelopeError> {
        let scope = Self {
            verse_id,
            petal_id,
            resource_id,
        };
        scope.validate()?;
        Ok(scope)
    }

    /// Builds the verse-wide scope.
    pub fn verse_wide(verse_id: Identifier32) -> Self {
        Self {
            verse_id,
            petal_id: None,
            resource_id: None,
        }
    }

    /// Verse the operation belongs to.
    pub const fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// Petal within the verse, or `None` for verse-wide scope.
    pub const fn petal_id(&self) -> Option<Identifier32> {
        self.petal_id
    }

    /// Resource within the petal, or `None`; requires a non-null petal.
    pub const fn resource_id(&self) -> Option<Identifier32> {
        self.resource_id
    }

    /// Enforces §3.1: a resource identifier requires a non-null petal identifier.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.petal_id.is_none() && self.resource_id.is_some() {
            return Err(EnvelopeError::ResourceWithoutPetal);
        }
        Ok(())
    }

    /// Reports whether this scope satisfies §3.1.
    pub fn is_valid(&self) -> bool {
        self.validate().is_ok()
    }

    /// SPEC-3 §1.7 containment: a non-null parent field must match exactly, a null parent
    /// field may be narrowed by the child, and a non-null parent field is never widened.
    ///
    /// An invalid operand contains nothing and is contained by nothing.
    pub fn contains(&self, child: &Scope) -> bool {
        if !self.is_valid() || !child.is_valid() {
            return false;
        }
        if self.verse_id != child.verse_id {
            return false;
        }
        if self.petal_id.is_some() && self.petal_id != child.petal_id {
            return false;
        }
        if self.resource_id.is_some() && self.resource_id != child.resource_id {
            return false;
        }
        true
    }

    /// Encodes the §3.1 scope map.
    pub fn to_cbor(&self) -> Result<CborValue, EnvelopeError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.verse_id.0.to_vec()),
            ),
            (
                CborValue::Uint(1),
                optional_identifier_to_cbor(self.petal_id),
            ),
            (
                CborValue::Uint(2),
                optional_identifier_to_cbor(self.resource_id),
            ),
        ]))
    }

    /// Decodes the §3.1 scope map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "scope";
        require_numeric_keys(value, 3, CONTEXT)?;
        let scope = Self {
            verse_id: Identifier32(bytes_at::<32>(value, 0, CONTEXT)?),
            petal_id: optional_identifier_at(value, 1, CONTEXT)?,
            resource_id: optional_identifier_at(value, 2, CONTEXT)?,
        };
        scope.validate()?;
        Ok(scope)
    }
}

/// The §3.2 author map: a canonical `did:key` Ed25519 DID and the key it must bind to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Author {
    /// Canonical `did:key` Ed25519 DID.
    pub did: String,
    /// Raw Ed25519 public key the DID encodes.
    pub public_key: [u8; 32],
}

impl Author {
    /// Builds the author whose DID is the canonical encoding of `public_key`.
    pub fn from_public_key(public_key: [u8; 32]) -> Self {
        Self {
            did: public_key_to_did(&public_key),
            public_key,
        }
    }

    /// Encodes the §3.2 author map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::text(&self.did)),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.public_key.to_vec()),
            ),
        ])
    }

    /// Decodes the §3.2 author map without checking the DID-to-key binding.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "author";
        require_numeric_keys(value, 2, CONTEXT)?;
        Ok(Self {
            did: text_at(value, 0, CONTEXT)?,
            public_key: bytes_at::<32>(value, 1, CONTEXT)?,
        })
    }
}

/// The §3.3 capability reference: the exact chain artifact and the epoch relied upon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityRef {
    /// BLAKE3 identifier of the canonical binary capability chain.
    pub chain_id: Hash32,
    /// Epoch whose authorization the author relied on.
    pub scope_epoch: u64,
}

impl CapabilityRef {
    /// Encodes the §3.3 capability map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.chain_id.0.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.scope_epoch)),
        ])
    }

    /// Decodes the §3.3 capability map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "capability";
        require_numeric_keys(value, 2, CONTEXT)?;
        Ok(Self {
            chain_id: Hash32(bytes_at::<32>(value, 0, CONTEXT)?),
            scope_epoch: unsigned_at(value, 1, CONTEXT)?,
        })
    }
}

/// The §3.4 wire hybrid logical clock; never the packed local-storage integer of `fe-database`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hlc {
    /// Unix milliseconds.
    pub wall_ms: u64,
    /// Logical counter for this wall time.
    pub counter: u32,
}

impl Hlc {
    /// Builds a clock reading.
    pub const fn new(wall_ms: u64, counter: u32) -> Self {
        Self { wall_ms, counter }
    }

    /// §3.4 concurrent ordering: `wall_ms`, then `counter`, then author public key byte-compare.
    pub fn compare_with_author(
        &self,
        author_public_key: &[u8; 32],
        other: &Hlc,
        other_author_public_key: &[u8; 32],
    ) -> Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then(self.counter.cmp(&other.counter))
            .then_with(|| {
                author_public_key
                    .as_slice()
                    .cmp(other_author_public_key.as_slice())
            })
    }

    /// Encodes the §3.4 HLC map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.wall_ms)),
            (CborValue::Uint(1), CborValue::Uint(u64::from(self.counter))),
        ])
    }

    /// Decodes the §3.4 HLC map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "hlc";
        require_numeric_keys(value, 2, CONTEXT)?;
        Ok(Self {
            wall_ms: unsigned_at(value, 0, CONTEXT)?,
            counter: u32_at(value, 1, CONTEXT)?,
        })
    }
}

/// The §3.5 encryption sub-map; the nonce is unconditionally 24 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EncryptionParams {
    /// AEAD suite identifier; production v1 uses [`PRODUCTION_SUITE_ID`].
    pub suite_id: u16,
    /// Identifier for the scope payload key.
    pub key_id: Identifier32,
    /// Fresh CSPRNG 192-bit nonce for this encryption under `key_id`.
    pub nonce: [u8; NONCE_LENGTH],
}

impl EncryptionParams {
    /// Reports whether this is the repository-fixture suite 65535.
    pub const fn is_fixture_only_suite(&self) -> bool {
        self.suite_id == FIXTURE_ONLY_SUITE_ID
    }

    /// Rejects every suite other than the v1 production suite; call on every production path.
    pub fn assert_production_suite(&self) -> Result<(), EnvelopeError> {
        if self.suite_id == PRODUCTION_SUITE_ID {
            return Ok(());
        }
        if self.is_fixture_only_suite() {
            return Err(EnvelopeError::FixtureOnlySuite);
        }
        Err(EnvelopeError::UnsupportedPayloadSuite {
            suite_id: self.suite_id,
        })
    }

    /// Encodes the §3.5 encryption map.
    pub fn to_cbor(&self) -> CborValue {
        CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Uint(u64::from(self.suite_id)),
            ),
            (CborValue::Uint(1), CborValue::Bytes(self.key_id.0.to_vec())),
            (CborValue::Uint(2), CborValue::Bytes(self.nonce.to_vec())),
        ])
    }

    /// Decodes the §3.5 encryption map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "encryption";
        require_numeric_keys(value, 3, CONTEXT)?;
        let suite_id = u16_at(value, 0, CONTEXT)?;
        if suite_id == 0 {
            return Err(EnvelopeError::ZeroPayloadSuite);
        }
        Ok(Self {
            suite_id,
            key_id: Identifier32(bytes_at::<32>(value, 1, CONTEXT)?),
            nonce: bytes_at::<NONCE_LENGTH>(value, 2, CONTEXT)?,
        })
    }
}

/// The §3.5 payload reference; it commits to ciphertext, never to plaintext.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PayloadRef {
    /// BLAKE3 of the stored ciphertext artifact.
    pub ciphertext_hash: Hash32,
    /// Exact ciphertext byte length.
    pub ciphertext_length: u64,
    /// Encryption parameters, or `None` for the no-payload form.
    pub encryption: Option<EncryptionParams>,
}

impl PayloadRef {
    /// The exact no-payload form: BLAKE3(empty), length 0, no encryption.
    pub fn empty() -> Self {
        Self {
            ciphertext_hash: Hash32::of_empty(),
            ciphertext_length: 0,
            encryption: None,
        }
    }

    /// Reports whether this is the exact no-payload form.
    pub fn is_no_payload_form(&self) -> bool {
        self.encryption.is_none()
            && self.ciphertext_length == 0
            && self.ciphertext_hash == Hash32::of_empty()
    }

    /// Enforces §3.5: a null `encryption` demands length 0 and BLAKE3(empty).
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.encryption.is_none() && !self.is_no_payload_form() {
            return Err(EnvelopeError::MalformedNoPayloadForm);
        }
        Ok(())
    }

    /// Encodes the §3.5 payload map.
    pub fn to_cbor(&self) -> Result<CborValue, EnvelopeError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.ciphertext_hash.0.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.ciphertext_length)),
            (
                CborValue::Uint(2),
                match &self.encryption {
                    Some(encryption) => encryption.to_cbor(),
                    None => CborValue::Null,
                },
            ),
        ]))
    }

    /// Decodes the §3.5 payload map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        const CONTEXT: &str = "payload";
        require_numeric_keys(value, 3, CONTEXT)?;
        let encryption_slot = entry(value, 2, CONTEXT)?;
        let payload = Self {
            ciphertext_hash: Hash32(bytes_at::<32>(value, 0, CONTEXT)?),
            ciphertext_length: unsigned_at(value, 1, CONTEXT)?,
            encryption: if encryption_slot.is_null() {
                None
            } else {
                Some(EncryptionParams::from_cbor(encryption_slot)?)
            },
        };
        payload.validate()?;
        Ok(payload)
    }
}

/// The §3.4 equivocation identity `(author.public_key, wall_ms, counter)`.
///
/// The pair MUST identify at most one `op_id`. Two different `op_id`s sharing one
/// `EquivocationKey` is author equivocation: receivers MUST retain the evidence,
/// quarantine both candidates, and MUST NOT materialize either without an authorized
/// resolution operation. Downstream slices enforce that; this type is the primitive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EquivocationKey {
    /// Raw Ed25519 public key of the author.
    pub author_public_key: [u8; 32],
    /// Unix milliseconds from the author's HLC.
    pub wall_ms: u64,
    /// Logical counter from the author's HLC.
    pub counter: u32,
}

/// The §3 outer map with keys 0 through 9: exactly the bytes the signature covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsignedEnvelope {
    /// MUST be [`PROTOCOL_VERSION`].
    pub protocol_version: u64,
    /// Non-zero operation kind; §6 defines the v1 structural values.
    pub operation_kind: u16,
    /// Authorization scope tuple.
    pub scope: Scope,
    /// Signing author and its bound public key.
    pub author: Author,
    /// Capability chain and epoch the author relied on.
    pub capability: CapabilityRef,
    /// Hash of the schema validating the decrypted intent payload.
    pub schema_hash: Hash32,
    /// Branch identifier within the verse DAG.
    pub branch_id: Identifier32,
    /// Strictly ascending, unique DAG parent operation IDs.
    pub parents: Vec<Hash32>,
    /// Wire hybrid logical clock.
    pub hlc: Hlc,
    /// Payload reference.
    pub payload: PayloadRef,
}

impl UnsignedEnvelope {
    /// Enforces every §3 rule this envelope can check on its own.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(EnvelopeError::UnsupportedProtocolVersion {
                version: self.protocol_version,
            });
        }
        if self.operation_kind == 0 {
            return Err(EnvelopeError::ZeroOperationKind);
        }
        self.scope.validate()?;
        validate_parents(&self.parents)?;
        self.payload.validate()
    }

    /// Encodes the ten-key §3 map.
    pub fn to_cbor(&self) -> Result<CborValue, EnvelopeError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.protocol_version)),
            (
                CborValue::Uint(1),
                CborValue::Uint(u64::from(self.operation_kind)),
            ),
            (CborValue::Uint(2), self.scope.to_cbor()?),
            (CborValue::Uint(3), self.author.to_cbor()),
            (CborValue::Uint(4), self.capability.to_cbor()),
            (
                CborValue::Uint(5),
                CborValue::Bytes(self.schema_hash.0.to_vec()),
            ),
            (
                CborValue::Uint(6),
                CborValue::Bytes(self.branch_id.0.to_vec()),
            ),
            (
                CborValue::Uint(7),
                CborValue::Array(
                    self.parents
                        .iter()
                        .map(|parent| CborValue::Bytes(parent.0.to_vec()))
                        .collect(),
                ),
            ),
            (CborValue::Uint(8), self.hlc.to_cbor()),
            (CborValue::Uint(9), self.payload.to_cbor()?),
        ]))
    }

    /// Decodes the ten-key §3 map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        require_numeric_keys(value, 10, ENVELOPE_CONTEXT)?;
        parse_unsigned_fields(value)
    }

    /// Encodes to canonical CBOR bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, EnvelopeError> {
        Ok(encode_cbor_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// The §3.4 equivocation identity of this operation.
    pub fn equivocation_key(&self) -> EquivocationKey {
        EquivocationKey {
            author_public_key: self.author.public_key,
            wall_ms: self.hlc.wall_ms,
            counter: self.hlc.counter,
        }
    }

    /// §3.4 deterministic ordering between two concurrent operations.
    pub fn concurrent_order(&self, other: &UnsignedEnvelope) -> Ordering {
        self.hlc.compare_with_author(
            &self.author.public_key,
            &other.hlc,
            &other.author.public_key,
        )
    }
}

/// The §3 outer map with keys 0 through 10: the exact artifact peers store and relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteEnvelope {
    /// The signed ten-key map.
    pub unsigned: UnsignedEnvelope,
    /// Ed25519 signature over the §5.1 preimage.
    pub signature: [u8; SIGNATURE_LENGTH],
}

impl CompleteEnvelope {
    /// Enforces every §3 rule the unsigned map carries.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        self.unsigned.validate()
    }

    /// Encodes the eleven-key §3 map.
    pub fn to_cbor(&self) -> Result<CborValue, EnvelopeError> {
        let mut entries = match self.unsigned.to_cbor()? {
            CborValue::Map(entries) => entries,
            _ => unreachable!("UnsignedEnvelope::to_cbor always yields a map"),
        };
        entries.push((
            CborValue::Uint(10),
            CborValue::Bytes(self.signature.to_vec()),
        ));
        Ok(CborValue::Map(entries))
    }

    /// Decodes the eleven-key §3 map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, EnvelopeError> {
        require_numeric_keys(value, 11, ENVELOPE_CONTEXT)?;
        Ok(Self {
            unsigned: parse_unsigned_fields(value)?,
            signature: bytes_at::<SIGNATURE_LENGTH>(value, 10, ENVELOPE_CONTEXT)?,
        })
    }

    /// Encodes to canonical CBOR bytes; `op_id` is BLAKE3 over exactly these bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, EnvelopeError> {
        Ok(encode_cbor_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, EnvelopeError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// The §3.4 equivocation identity of this operation.
    pub fn equivocation_key(&self) -> EquivocationKey {
        self.unsigned.equivocation_key()
    }
}

const ENVELOPE_CONTEXT: &str = "envelope";

/// Reads keys 0 through 9 without asserting the outer map's arity.
fn parse_unsigned_fields(value: &CborValue) -> Result<UnsignedEnvelope, EnvelopeError> {
    let protocol_version = unsigned_at(value, 0, ENVELOPE_CONTEXT)?;
    if protocol_version != PROTOCOL_VERSION {
        return Err(EnvelopeError::UnsupportedProtocolVersion {
            version: protocol_version,
        });
    }
    let operation_kind = u16_at(value, 1, ENVELOPE_CONTEXT)?;
    if operation_kind == 0 {
        return Err(EnvelopeError::ZeroOperationKind);
    }

    let parents = entry(value, 7, ENVELOPE_CONTEXT)?
        .as_array()
        .ok_or(EnvelopeError::ExpectedArray {
            context: "envelope.parents",
        })?
        .iter()
        .enumerate()
        .map(|(index, parent)| {
            let raw = parent.as_bytes().ok_or(EnvelopeError::ExpectedByteString {
                context: "envelope.parents",
                key: index as u64,
            })?;
            Ok(Hash32(raw.try_into().map_err(|_| {
                EnvelopeError::WrongByteLength {
                    context: "envelope.parents",
                    key: index as u64,
                    expected: 32,
                    actual: raw.len(),
                }
            })?))
        })
        .collect::<Result<Vec<Hash32>, EnvelopeError>>()?;
    validate_parents(&parents)?;

    Ok(UnsignedEnvelope {
        protocol_version,
        operation_kind,
        scope: Scope::from_cbor(entry(value, 2, ENVELOPE_CONTEXT)?)?,
        author: Author::from_cbor(entry(value, 3, ENVELOPE_CONTEXT)?)?,
        capability: CapabilityRef::from_cbor(entry(value, 4, ENVELOPE_CONTEXT)?)?,
        schema_hash: Hash32(bytes_at::<32>(value, 5, ENVELOPE_CONTEXT)?),
        branch_id: Identifier32(bytes_at::<32>(value, 6, ENVELOPE_CONTEXT)?),
        parents,
        hlc: Hlc::from_cbor(entry(value, 8, ENVELOPE_CONTEXT)?)?,
        payload: PayloadRef::from_cbor(entry(value, 9, ENVELOPE_CONTEXT)?)?,
    })
}

fn validate_parents(parents: &[Hash32]) -> Result<(), EnvelopeError> {
    match parents.windows(2).position(|pair| pair[0] >= pair[1]) {
        Some(predecessor_index) => Err(EnvelopeError::ParentsNotStrictlyAscending {
            index: predecessor_index + 1,
        }),
        None => Ok(()),
    }
}

fn optional_identifier_to_cbor(identifier: Option<Identifier32>) -> CborValue {
    match identifier {
        Some(identifier) => CborValue::Bytes(identifier.0.to_vec()),
        None => CborValue::Null,
    }
}

/// Requires exactly the integer keys `0..expected_key_count`, once each and nothing else.
fn require_numeric_keys(
    value: &CborValue,
    expected_key_count: u64,
    context: &'static str,
) -> Result<(), EnvelopeError> {
    let entries = value
        .as_map()
        .ok_or(EnvelopeError::ExpectedMap { context })?;
    if entries.len() as u64 != expected_key_count {
        return Err(EnvelopeError::UnexpectedMapKeys {
            context,
            expected_key_count,
        });
    }
    for key in 0..expected_key_count {
        if value.get_uint_key(key).is_none() {
            return Err(EnvelopeError::MissingKey { context, key });
        }
    }
    Ok(())
}

fn entry<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, EnvelopeError> {
    value
        .get_uint_key(key)
        .ok_or(EnvelopeError::MissingKey { context, key })
}

fn unsigned_at(value: &CborValue, key: u64, context: &'static str) -> Result<u64, EnvelopeError> {
    entry(value, key, context)?
        .as_uint()
        .ok_or(EnvelopeError::ExpectedUnsignedInteger { context, key })
}

fn u16_at(value: &CborValue, key: u64, context: &'static str) -> Result<u16, EnvelopeError> {
    let raw = unsigned_at(value, key, context)?;
    u16::try_from(raw).map_err(|_| EnvelopeError::IntegerOutOfRange {
        context,
        key,
        value: raw,
    })
}

fn u32_at(value: &CborValue, key: u64, context: &'static str) -> Result<u32, EnvelopeError> {
    let raw = unsigned_at(value, key, context)?;
    u32::try_from(raw).map_err(|_| EnvelopeError::IntegerOutOfRange {
        context,
        key,
        value: raw,
    })
}

fn bytes_at<const LENGTH: usize>(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; LENGTH], EnvelopeError> {
    let raw = entry(value, key, context)?
        .as_bytes()
        .ok_or(EnvelopeError::ExpectedByteString { context, key })?;
    raw.try_into().map_err(|_| EnvelopeError::WrongByteLength {
        context,
        key,
        expected: LENGTH,
        actual: raw.len(),
    })
}

fn text_at(value: &CborValue, key: u64, context: &'static str) -> Result<String, EnvelopeError> {
    Ok(entry(value, key, context)?
        .as_text()
        .ok_or(EnvelopeError::ExpectedTextString { context, key })?
        .to_owned())
}

fn optional_identifier_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Option<Identifier32>, EnvelopeError> {
    let slot = entry(value, key, context)?;
    if slot.is_null() {
        return Ok(None);
    }
    let raw = slot
        .as_bytes()
        .ok_or(EnvelopeError::ExpectedNullOrByteString { context, key })?;
    Ok(Some(Identifier32(raw.try_into().map_err(|_| {
        EnvelopeError::WrongByteLength {
            context,
            key,
            expected: 32,
            actual: raw.len(),
        }
    })?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::op_id;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn sample_unsigned() -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 3,
            scope: Scope::verse_wide(identifier(0x11)),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: hash(0x33),
                scope_epoch: 7,
            },
            schema_hash: hash(0x44),
            branch_id: identifier(0x55),
            parents: vec![hash(0x66), hash(0x77)],
            hlc: Hlc::new(1_700_000_000_000, 4),
            payload: PayloadRef::empty(),
        }
    }

    fn sample_complete(unsigned: UnsignedEnvelope, signature_filler: u8) -> CompleteEnvelope {
        CompleteEnvelope {
            unsigned,
            signature: [signature_filler; SIGNATURE_LENGTH],
        }
    }

    #[test]
    fn unsigned_envelope_round_trips_through_canonical_bytes() {
        let envelope = sample_unsigned();
        let bytes = envelope.encode_canonical().expect("encode");
        assert_eq!(
            UnsignedEnvelope::decode_canonical(&bytes).expect("decode"),
            envelope
        );
        assert_eq!(
            UnsignedEnvelope::decode_canonical(&bytes)
                .expect("decode")
                .encode_canonical()
                .expect("re-encode"),
            bytes
        );
    }

    #[test]
    fn complete_envelope_round_trips_and_carries_eleven_keys() {
        let complete = sample_complete(sample_unsigned(), 0x99);
        let bytes = complete.encode_canonical().expect("encode");
        let decoded = CompleteEnvelope::decode_canonical(&bytes).expect("decode");
        assert_eq!(decoded, complete);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
        assert_eq!(
            complete
                .to_cbor()
                .expect("cbor")
                .as_map()
                .map(|entries| entries.len()),
            Some(11)
        );
    }

    #[test]
    fn rejects_an_envelope_map_with_a_missing_or_extra_key() {
        let mut entries = match sample_unsigned().to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        let complete = CborValue::Map(entries.clone());
        entries.pop();
        assert_eq!(
            UnsignedEnvelope::from_cbor(&CborValue::Map(entries.clone())),
            Err(EnvelopeError::UnexpectedMapKeys {
                context: "envelope",
                expected_key_count: 10,
            })
        );

        let mut extra = match complete {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        extra.push((CborValue::Uint(11), CborValue::Uint(0)));
        assert_eq!(
            UnsignedEnvelope::from_cbor(&CborValue::Map(extra)),
            Err(EnvelopeError::UnexpectedMapKeys {
                context: "envelope",
                expected_key_count: 10,
            })
        );
    }

    #[test]
    fn rejects_an_unlisted_key_that_keeps_the_arity() {
        let mut entries = match sample_unsigned().to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[9] = (CborValue::Uint(42), CborValue::Uint(0));
        assert_eq!(
            UnsignedEnvelope::from_cbor(&CborValue::Map(entries)),
            Err(EnvelopeError::MissingKey {
                context: "envelope",
                key: 9,
            })
        );
    }

    #[test]
    fn rejects_a_protocol_version_other_than_one() {
        let mut envelope = sample_unsigned();
        envelope.protocol_version = 2;
        assert_eq!(
            envelope.encode_canonical(),
            Err(EnvelopeError::UnsupportedProtocolVersion { version: 2 })
        );
    }

    #[test]
    fn rejects_a_zero_operation_kind() {
        let mut envelope = sample_unsigned();
        envelope.operation_kind = 0;
        assert_eq!(
            envelope.encode_canonical(),
            Err(EnvelopeError::ZeroOperationKind)
        );
    }

    #[test]
    fn rejects_parents_that_are_unsorted_or_duplicated() {
        let mut unsorted = sample_unsigned();
        unsorted.parents = vec![hash(0x77), hash(0x66)];
        assert_eq!(
            unsorted.encode_canonical(),
            Err(EnvelopeError::ParentsNotStrictlyAscending { index: 1 })
        );

        let mut duplicated = sample_unsigned();
        duplicated.parents = vec![hash(0x66), hash(0x66)];
        assert_eq!(
            duplicated.encode_canonical(),
            Err(EnvelopeError::ParentsNotStrictlyAscending { index: 1 })
        );
    }

    #[test]
    fn rejects_a_no_payload_form_that_is_not_exact() {
        let mut wrong_length = sample_unsigned();
        wrong_length.payload.ciphertext_length = 1;
        assert_eq!(
            wrong_length.encode_canonical(),
            Err(EnvelopeError::MalformedNoPayloadForm)
        );

        let mut wrong_hash = sample_unsigned();
        wrong_hash.payload.ciphertext_hash = hash(0x00);
        assert_eq!(
            wrong_hash.encode_canonical(),
            Err(EnvelopeError::MalformedNoPayloadForm)
        );
    }

    #[test]
    fn empty_payload_reference_commits_to_blake3_of_no_bytes() {
        let empty = PayloadRef::empty();
        assert!(empty.is_no_payload_form());
        assert_eq!(empty.ciphertext_length, 0);
        assert_eq!(empty.ciphertext_hash, Hash32::of(&[]));
        assert_eq!(
            hex::encode(empty.ciphertext_hash.0),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn scope_rejects_a_resource_without_a_petal() {
        assert_eq!(
            Scope::new(identifier(1), None, Some(identifier(2))),
            Err(EnvelopeError::ResourceWithoutPetal)
        );
        assert!(Scope::new(identifier(1), Some(identifier(2)), Some(identifier(3))).is_ok());
    }

    #[test]
    fn scope_containment_never_widens_a_non_null_parent_field() {
        let verse = identifier(1);
        let petal = identifier(2);
        let resource = identifier(3);
        let verse_wide = Scope::verse_wide(verse);
        let petal_scope = Scope::new(verse, Some(petal), None).expect("petal scope");
        let resource_scope = Scope::new(verse, Some(petal), Some(resource)).expect("resource");

        assert!(verse_wide.contains(&verse_wide));
        assert!(verse_wide.contains(&petal_scope));
        assert!(verse_wide.contains(&resource_scope));
        assert!(petal_scope.contains(&resource_scope));
        assert!(petal_scope.contains(&petal_scope));
        assert!(resource_scope.contains(&resource_scope));

        assert!(!petal_scope.contains(&verse_wide));
        assert!(!resource_scope.contains(&petal_scope));
        assert!(!resource_scope.contains(&verse_wide));
        assert!(
            !petal_scope.contains(&Scope::new(verse, Some(identifier(9)), None).expect("other"))
        );
        assert!(!verse_wide.contains(&Scope::verse_wide(identifier(9))));
    }

    /// Builds the §3.1-invalid tuple that [`Scope::new`] and [`Scope::from_cbor`] both refuse.
    fn resource_without_petal(verse_id: Identifier32, resource_id: Identifier32) -> Scope {
        Scope {
            verse_id,
            petal_id: None,
            resource_id: Some(resource_id),
        }
    }

    #[test]
    fn an_invalid_scope_contains_nothing_and_is_contained_by_nothing() {
        let verse = identifier(1);
        let petal = identifier(2);
        let resource = identifier(3);
        let invalid = resource_without_petal(verse, resource);
        let verse_wide = Scope::verse_wide(verse);
        let resource_scope = Scope::new(verse, Some(petal), Some(resource)).expect("resource");

        assert!(!invalid.is_valid());
        assert!(!invalid.contains(&invalid));
        assert!(!invalid.contains(&verse_wide));
        assert!(!invalid.contains(&resource_scope));
        assert!(!verse_wide.contains(&invalid));
        assert!(!resource_scope.contains(&invalid));
    }

    #[test]
    fn an_invalid_scope_never_reaches_the_wire() {
        let invalid = resource_without_petal(identifier(1), identifier(3));
        assert_eq!(invalid.to_cbor(), Err(EnvelopeError::ResourceWithoutPetal));

        let mut envelope = sample_unsigned();
        envelope.scope = invalid;
        assert_eq!(
            envelope.encode_canonical(),
            Err(EnvelopeError::ResourceWithoutPetal)
        );
    }

    #[test]
    fn scope_accessors_report_the_constructed_tuple() {
        let verse = identifier(1);
        let petal = identifier(2);
        let resource = identifier(3);
        let scope = Scope::new(verse, Some(petal), Some(resource)).expect("resource scope");

        assert_eq!(scope.verse_id(), verse);
        assert_eq!(scope.petal_id(), Some(petal));
        assert_eq!(scope.resource_id(), Some(resource));
        assert_eq!(Scope::verse_wide(verse).petal_id(), None);
        assert_eq!(Scope::verse_wide(verse).resource_id(), None);
    }

    #[test]
    fn hlc_orders_by_wall_then_counter_then_author_bytes() {
        let earlier = Hlc::new(10, 5);
        let later = Hlc::new(11, 0);
        let low_author = [0x00; 32];
        let high_author = [0x01; 32];

        assert_eq!(
            earlier.compare_with_author(&low_author, &later, &low_author),
            Ordering::Less
        );
        assert_eq!(
            Hlc::new(10, 4).compare_with_author(&high_author, &earlier, &low_author),
            Ordering::Less
        );
        assert_eq!(
            earlier.compare_with_author(&low_author, &earlier, &high_author),
            Ordering::Less
        );
        assert_eq!(
            earlier.compare_with_author(&low_author, &earlier, &low_author),
            Ordering::Equal
        );
    }

    #[test]
    fn encryption_params_reject_every_non_production_suite() {
        let mut encryption = EncryptionParams {
            suite_id: PRODUCTION_SUITE_ID,
            key_id: identifier(0xaa),
            nonce: [0xbb; NONCE_LENGTH],
        };
        assert!(encryption.assert_production_suite().is_ok());
        assert!(!encryption.is_fixture_only_suite());

        encryption.suite_id = FIXTURE_ONLY_SUITE_ID;
        assert!(encryption.is_fixture_only_suite());
        assert_eq!(
            encryption.assert_production_suite(),
            Err(EnvelopeError::FixtureOnlySuite)
        );

        encryption.suite_id = 2;
        assert_eq!(
            encryption.assert_production_suite(),
            Err(EnvelopeError::UnsupportedPayloadSuite { suite_id: 2 })
        );
    }

    #[test]
    fn encryption_suite_zero_is_rejected_on_decode() {
        let encryption = EncryptionParams {
            suite_id: PRODUCTION_SUITE_ID,
            key_id: identifier(0xaa),
            nonce: [0xbb; NONCE_LENGTH],
        };
        let mut entries = match encryption.to_cbor() {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[0] = (CborValue::Uint(0), CborValue::Uint(0));
        assert_eq!(
            EncryptionParams::from_cbor(&CborValue::Map(entries)),
            Err(EnvelopeError::ZeroPayloadSuite)
        );
        assert_eq!(
            EncryptionParams::from_cbor(&encryption.to_cbor()),
            Ok(encryption)
        );
    }

    #[test]
    fn encryption_nonce_must_be_twenty_four_bytes() {
        let encryption = EncryptionParams {
            suite_id: PRODUCTION_SUITE_ID,
            key_id: identifier(0xaa),
            nonce: [0xbb; NONCE_LENGTH],
        };
        let mut entries = match encryption.to_cbor() {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[2] = (CborValue::Uint(2), CborValue::Bytes(vec![0xbb; 12]));
        assert_eq!(
            EncryptionParams::from_cbor(&CborValue::Map(entries)),
            Err(EnvelopeError::WrongByteLength {
                context: "encryption",
                key: 2,
                expected: 24,
                actual: 12,
            })
        );
    }

    #[test]
    fn two_operations_at_one_hlc_share_an_equivocation_key_but_not_an_op_id() {
        let first = sample_unsigned();
        let mut second = sample_unsigned();
        second.branch_id = identifier(0x56);

        assert_eq!(first.equivocation_key(), second.equivocation_key());

        let first_complete = sample_complete(first, 0x99);
        let second_complete = sample_complete(second, 0x99);
        assert_eq!(
            first_complete.equivocation_key(),
            second_complete.equivocation_key()
        );
        assert_ne!(
            op_id(&first_complete.encode_canonical().expect("encode")),
            op_id(&second_complete.encode_canonical().expect("encode"))
        );
    }

    #[test]
    fn hash_and_identifier_order_byte_lexicographically() {
        assert!(Hash32([0x00; 32]) < Hash32([0x01; 32]));
        let mut high_first_byte = [0u8; 32];
        high_first_byte[0] = 0x01;
        let mut high_last_byte = [0u8; 32];
        high_last_byte[31] = 0xff;
        assert!(Hash32(high_last_byte) < Hash32(high_first_byte));
        assert!(Identifier32(high_last_byte) < Identifier32(high_first_byte));
    }
}
