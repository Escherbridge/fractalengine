//! Record-relay custody sets and signed custody receipts (SPEC-9 relay scope, §2-§3).
//!
//! Pure local logic: no transport, no clock, no storage beyond the `SealedArtifactStore` seam.

use ed25519_dalek::SigningKey;
use thiserror::Error;

use crate::capability::Principal;
use crate::cbor::{decode_canonical, encode_canonical_checked, CborError, CborValue};
use crate::envelope::{EnvelopeError, Hash32, Identifier32, SIGNATURE_LENGTH};
use crate::signing::{sign_domain, verify_author_binding, verify_domain, SigningError};

use super::store::SealedArtifactStore;
use super::{
    assert_canonical_bytes, bytes_at, hash32_at, identifier32_at, require_uint_keys,
    uint_at, value_at, SegmentError,
};

/// NUL-terminated signature domain of a custody receipt (SPEC-9 §3.1, provisional).
pub const CUSTODY_RECEIPT_DOMAIN: &[u8] = b"fe-custody-receipt-v1\0";

/// NUL-terminated hash domain of a custody set identifier (SPEC-9 §2 rule 1, provisional).
pub const CUSTODY_SET_DOMAIN: &[u8] = b"fe-custody-set-v1\0";

/// The only receipt version v1 admits.
pub const CUSTODY_RECEIPT_VERSION: u64 = 1;

/// This binary's record format epoch (SPEC-9 §6 rule 1); bump it to force a pre-GA reset.
pub const WAREHOUSE_FORMAT_EPOCH: u64 = 1;

const RECEIPT_CONTEXT: &str = "custody receipt";
const CUSTODY_SET_CONTEXT: &str = "custody set";

const RECEIPT_VERSION_KEY: u64 = 0;
const VERSE_ID_KEY: u64 = 1;
const MANIFEST_ID_KEY: u64 = 2;
const CUSTODY_SET_ID_KEY: u64 = 3;
const RELAY_PRINCIPAL_KEY: u64 = 4;
const HELD_AT_MS_KEY: u64 = 5;
const WAREHOUSE_FORMAT_EPOCH_KEY: u64 = 6;
const SIGNATURE_KEY: u64 = 7;

const COMPLETE_KEYS: [u64; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// Every reason a custody set or receipt is refused, built, or issued.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CustodyReceiptError {
    /// The bytes or a field failed the SPEC-6 grammar helpers.
    #[error("custody: {0}")]
    Segment(#[from] SegmentError),
    /// The bytes were not canonical CBOR.
    #[error("custody: canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// The relay principal map did not parse.
    #[error("custody: relay principal: {0}")]
    Envelope(#[from] EnvelopeError),
    /// The DID binding or the signature failed.
    #[error("custody: {0}")]
    Signing(#[from] SigningError),
    /// The receipt declared a version other than 1.
    #[error("custody receipt version {version} is not 1")]
    UnsupportedVersion {
        /// The version actually present.
        version: u64,
    },
    /// A custody set listed no artifact.
    #[error("custody set is empty")]
    EmptyCustodySet,
    /// A custody set did not contain its own manifest (§3.3 rule 1).
    #[error("custody set does not contain its manifest {manifest_id:?}")]
    ManifestNotInCustodySet {
        /// The manifest the set was built for.
        manifest_id: Hash32,
    },
    /// One artifact was listed with two different stored lengths.
    #[error("custody set lists {artifact_id:?} with two different stored lengths")]
    ConflictingStoredLength {
        /// The contested identifier.
        artifact_id: Hash32,
    },
    /// Received custody-set entries were not strictly ascending by artifact ID.
    #[error("custody set entries are not strictly ascending by artifact ID")]
    CustodySetNotAscending,
    /// A custody-set entry was not a two-element `[artifact_id, stored_length]` array.
    #[error("custody set entry is not a two-element array")]
    MalformedCustodySetEntry,
    /// The relay does not durably hold a listed artifact, so it must not sign (§3.3 rule 1).
    #[error("relay does not hold custody-set artifact {artifact_id:?}")]
    CustodyMemberMissing {
        /// The absent artifact.
        artifact_id: Hash32,
    },
    /// The relay's ledger stamp is not this binary's epoch; it must wipe before signing (§6).
    #[error("relay ledger stamp {stamped} is not format epoch {current}; reset before signing")]
    StaleFormatEpoch {
        /// The ledger's stamp.
        stamped: u64,
        /// This binary's epoch.
        current: u64,
    },
    /// A held artifact's bytes did not re-hash to its ID or match its stored length.
    #[error("held artifact {artifact_id:?} does not re-hash to its ID and length")]
    CustodyMemberMismatch {
        /// The offending artifact.
        artifact_id: Hash32,
    },
}

/// Where a custody set's member list came from; only a walked list may count for the floor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustodySetOrigin {
    /// Built by a header-key holder from its own walk of the manifest (§2 rule 2).
    Walked,
    /// Decoded from a peer's seed commitment: issuance input only, never floor evidence (§2 rule 4).
    SeedCommitment,
}

/// One manifest's custody set: the exact artifacts and lengths it requires (§1 rule 3, §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodySet {
    manifest_id: Hash32,
    members: Vec<(Hash32, u64)>,
    origin: CustodySetOrigin,
}

impl CustodySet {
    /// Builds the set a header-key holder computed by walking `M` (§2 rule 2); never feed it a
    /// peer's list. Sorts, dedups and refuses conflicts.
    pub fn for_manifest(
        manifest_id: Hash32,
        members: impl IntoIterator<Item = (Hash32, u64)>,
    ) -> Result<Self, CustodyReceiptError> {
        let mut members: Vec<(Hash32, u64)> = members.into_iter().collect();
        members.sort();
        members.dedup();
        if let Some(pair) = members.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(CustodyReceiptError::ConflictingStoredLength {
                artifact_id: pair[0].0,
            });
        }
        Self::checked(manifest_id, members, CustodySetOrigin::Walked)
    }

    /// Decodes a received `canonical_custody_set`, refusing any non-canonical order or duplicate.
    ///
    /// The result is marked as a seed commitment: a relay may receipt it, but a floor assessment
    /// treats it as unknown, since §2 rule 2 forbids computing the set from a peer's statement.
    pub fn decode_canonical(manifest_id: Hash32, bytes: &[u8]) -> Result<Self, CustodyReceiptError> {
        let value = decode_canonical(bytes)?;
        assert_canonical_bytes(&value, bytes, CUSTODY_SET_CONTEXT)?;
        let entries = value.as_array().ok_or(SegmentError::ExpectedArray {
            context: CUSTODY_SET_CONTEXT,
        })?;
        let mut members = Vec::with_capacity(entries.len());
        for entry in entries {
            let pair = entry
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or(CustodyReceiptError::MalformedCustodySetEntry)?;
            let artifact_id = pair[0]
                .as_bytes()
                .and_then(|raw| <[u8; 32]>::try_from(raw).ok())
                .ok_or(CustodyReceiptError::MalformedCustodySetEntry)?;
            let stored_length = pair[1]
                .as_uint()
                .ok_or(CustodyReceiptError::MalformedCustodySetEntry)?;
            members.push((Hash32(artifact_id), stored_length));
        }
        if members.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err(CustodyReceiptError::CustodySetNotAscending);
        }
        Self::checked(manifest_id, members, CustodySetOrigin::SeedCommitment)
    }

    fn checked(
        manifest_id: Hash32,
        members: Vec<(Hash32, u64)>,
        origin: CustodySetOrigin,
    ) -> Result<Self, CustodyReceiptError> {
        if members.is_empty() {
            return Err(CustodyReceiptError::EmptyCustodySet);
        }
        if !members.iter().any(|(artifact_id, _)| *artifact_id == manifest_id) {
            return Err(CustodyReceiptError::ManifestNotInCustodySet { manifest_id });
        }
        Ok(Self {
            manifest_id,
            members,
            origin,
        })
    }

    /// Whether the set came from the holder's own walk rather than a decoded seed commitment.
    pub fn is_walked(&self) -> bool {
        self.origin == CustodySetOrigin::Walked
    }

    /// The manifest this set belongs to.
    pub fn manifest_id(&self) -> Hash32 {
        self.manifest_id
    }

    /// `[artifact_id, stored_length]` pairs, strictly ascending by artifact ID.
    pub fn members(&self) -> &[(Hash32, u64)] {
        &self.members
    }

    /// Whether `artifact_id` is in this set.
    pub fn contains(&self, artifact_id: Hash32) -> bool {
        self.members
            .binary_search_by(|(member, _)| member.cmp(&artifact_id))
            .is_ok()
    }

    /// The deterministic-CBOR `canonical_custody_set` array.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyReceiptError> {
        let array = CborValue::Array(
            self.members
                .iter()
                .map(|(artifact_id, stored_length)| {
                    CborValue::Array(vec![
                        CborValue::Bytes(artifact_id.0.to_vec()),
                        CborValue::Uint(*stored_length),
                    ])
                })
                .collect(),
        );
        Ok(encode_canonical_checked(&array)?)
    }

    /// `BLAKE3("fe-custody-set-v1" || 00 || canonical_custody_set)`.
    pub fn custody_set_id(&self) -> Result<Hash32, CustodyReceiptError> {
        let mut preimage = CUSTODY_SET_DOMAIN.to_vec();
        preimage.extend_from_slice(&self.encode_canonical()?);
        Ok(Hash32::of(&preimage))
    }
}

/// A record relay's one-verse binding and its ledger's format-epoch stamp (§5, §6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordRelayBinding {
    verse_id: Identifier32,
    warehouse_format_epoch: u64,
}

impl RecordRelayBinding {
    /// A binding stamped with this binary's [`WAREHOUSE_FORMAT_EPOCH`].
    pub fn current(verse_id: Identifier32) -> Self {
        Self::stamped(verse_id, WAREHOUSE_FORMAT_EPOCH)
    }

    /// A binding carrying the stamp read from a relay ledger (which may be stale, §6 rule 2).
    pub fn stamped(verse_id: Identifier32, warehouse_format_epoch: u64) -> Self {
        Self {
            verse_id,
            warehouse_format_epoch,
        }
    }

    /// The one verse this relay records.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// The ledger's format-epoch stamp.
    pub fn warehouse_format_epoch(&self) -> u64 {
        self.warehouse_format_epoch
    }

    /// Whether the stamp differs from this binary's epoch, i.e. the §6 rule 2 wipe is due.
    pub fn requires_reset(&self) -> bool {
        self.warehouse_format_epoch != WAREHOUSE_FORMAT_EPOCH
    }
}

/// A complete signed custody receipt (§3.1); obtainable only from [`issue_custody_receipt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyReceipt {
    verse_id: Identifier32,
    manifest_id: Hash32,
    custody_set_id: Hash32,
    relay_principal: Principal,
    held_at_ms: u64,
    warehouse_format_epoch: u64,
    signature: [u8; SIGNATURE_LENGTH],
}

impl CustodyReceipt {
    /// The exact canonical eight-key receipt bytes.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CustodyReceiptError> {
        let mut entries = unsigned_entries(
            self.verse_id,
            self.manifest_id,
            self.custody_set_id,
            &self.relay_principal,
            self.held_at_ms,
            self.warehouse_format_epoch,
        );
        entries.push((
            CborValue::Uint(SIGNATURE_KEY),
            CborValue::Bytes(self.signature.to_vec()),
        ));
        Ok(encode_canonical_checked(&CborValue::Map(entries))?)
    }

    /// `receipt_id = BLAKE3(complete_receipt)`.
    pub fn receipt_id(&self) -> Result<Hash32, CustodyReceiptError> {
        Ok(Hash32::of(&self.encode_canonical()?))
    }
}

fn unsigned_entries(
    verse_id: Identifier32,
    manifest_id: Hash32,
    custody_set_id: Hash32,
    relay_principal: &Principal,
    held_at_ms: u64,
    warehouse_format_epoch: u64,
) -> Vec<(CborValue, CborValue)> {
    vec![
        (
            CborValue::Uint(RECEIPT_VERSION_KEY),
            CborValue::Uint(CUSTODY_RECEIPT_VERSION),
        ),
        (
            CborValue::Uint(VERSE_ID_KEY),
            CborValue::Bytes(verse_id.0.to_vec()),
        ),
        (
            CborValue::Uint(MANIFEST_ID_KEY),
            CborValue::Bytes(manifest_id.0.to_vec()),
        ),
        (
            CborValue::Uint(CUSTODY_SET_ID_KEY),
            CborValue::Bytes(custody_set_id.0.to_vec()),
        ),
        (
            CborValue::Uint(RELAY_PRINCIPAL_KEY),
            relay_principal.to_cbor(),
        ),
        (CborValue::Uint(HELD_AT_MS_KEY), CborValue::Uint(held_at_ms)),
        (
            CborValue::Uint(WAREHOUSE_FORMAT_EPOCH_KEY),
            CborValue::Uint(warehouse_format_epoch),
        ),
    ]
}

/// The relay-side door (§3.3 rule 1): re-hash and length-check every held member, then sign.
///
/// The receipt names only the binding's verse and the current epoch, so a relay cannot sign for
/// another verse or under a stale stamp (§3.3 rule 2, §6 rule 2). A missing, altered, or
/// wrong-length member yields no receipt.
pub fn issue_custody_receipt(
    relay_key: &SigningKey,
    binding: &RecordRelayBinding,
    custody_set: &CustodySet,
    held: &impl SealedArtifactStore,
    held_at_ms: u64,
) -> Result<CustodyReceipt, CustodyReceiptError> {
    if binding.requires_reset() {
        return Err(CustodyReceiptError::StaleFormatEpoch {
            stamped: binding.warehouse_format_epoch,
            current: WAREHOUSE_FORMAT_EPOCH,
        });
    }
    for (artifact_id, stored_length) in custody_set.members() {
        let bytes = held
            .get(*artifact_id)
            .ok_or(CustodyReceiptError::CustodyMemberMissing {
                artifact_id: *artifact_id,
            })?;
        if bytes.len() as u64 != *stored_length || Hash32::of(&bytes) != *artifact_id {
            return Err(CustodyReceiptError::CustodyMemberMismatch {
                artifact_id: *artifact_id,
            });
        }
    }
    let relay_principal = Principal::from_public_key(relay_key.verifying_key().to_bytes());
    let custody_set_id = custody_set.custody_set_id()?;
    let unsigned = encode_canonical_checked(&CborValue::Map(unsigned_entries(
        binding.verse_id,
        custody_set.manifest_id,
        custody_set_id,
        &relay_principal,
        held_at_ms,
        binding.warehouse_format_epoch,
    )))?;
    Ok(CustodyReceipt {
        verse_id: binding.verse_id,
        manifest_id: custody_set.manifest_id,
        custody_set_id,
        relay_principal,
        held_at_ms,
        warehouse_format_epoch: binding.warehouse_format_epoch,
        signature: sign_domain(relay_key, CUSTODY_RECEIPT_DOMAIN, &unsigned),
    })
}

/// A receipt that passed §3.2; verified is not qualified (see `retention::durability_floor`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifiedCustodyReceipt {
    receipt_id: Hash32,
    verse_id: Identifier32,
    manifest_id: Hash32,
    custody_set_id: Hash32,
    relay_principal: Principal,
    held_at_ms: u64,
    warehouse_format_epoch: u64,
}

impl VerifiedCustodyReceipt {
    /// BLAKE3 of the complete received bytes.
    pub fn receipt_id(&self) -> Hash32 {
        self.receipt_id
    }

    /// The verse the receipt names.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// The manifest the receipt names.
    pub fn manifest_id(&self) -> Hash32 {
        self.manifest_id
    }

    /// The custody set identifier the relay attests.
    pub fn custody_set_id(&self) -> Hash32 {
        self.custody_set_id
    }

    /// The signing relay, DID bound to key.
    pub fn relay_principal(&self) -> &Principal {
        &self.relay_principal
    }

    /// Advisory only; never a qualification input (§4 rule 7).
    pub fn held_at_ms(&self) -> u64 {
        self.held_at_ms
    }

    /// The relay's format-epoch stamp.
    pub fn warehouse_format_epoch(&self) -> u64 {
        self.warehouse_format_epoch
    }
}

/// The §3.2 verification order: canonical re-encode, DID-to-key binding, then signature.
pub fn verify_custody_receipt(bytes: &[u8]) -> Result<VerifiedCustodyReceipt, CustodyReceiptError> {
    let value = decode_canonical(bytes)?;
    assert_canonical_bytes(&value, bytes, RECEIPT_CONTEXT)?;
    require_uint_keys(&value, &COMPLETE_KEYS, RECEIPT_CONTEXT)?;
    let version = uint_at(&value, RECEIPT_VERSION_KEY, RECEIPT_CONTEXT)?;
    if version != CUSTODY_RECEIPT_VERSION {
        return Err(CustodyReceiptError::UnsupportedVersion { version });
    }
    let verse_id = identifier32_at(&value, VERSE_ID_KEY, RECEIPT_CONTEXT)?;
    let manifest_id = hash32_at(&value, MANIFEST_ID_KEY, RECEIPT_CONTEXT)?;
    let custody_set_id = hash32_at(&value, CUSTODY_SET_ID_KEY, RECEIPT_CONTEXT)?;
    let relay_principal =
        Principal::from_cbor(value_at(&value, RELAY_PRINCIPAL_KEY, RECEIPT_CONTEXT)?)?;
    let held_at_ms = uint_at(&value, HELD_AT_MS_KEY, RECEIPT_CONTEXT)?;
    let warehouse_format_epoch = uint_at(&value, WAREHOUSE_FORMAT_EPOCH_KEY, RECEIPT_CONTEXT)?;
    let signature = bytes_at::<SIGNATURE_LENGTH>(&value, SIGNATURE_KEY, RECEIPT_CONTEXT)?;

    verify_author_binding(&relay_principal.did, &relay_principal.public_key)?;
    let unsigned = encode_canonical_checked(&CborValue::Map(unsigned_entries(
        verse_id,
        manifest_id,
        custody_set_id,
        &relay_principal,
        held_at_ms,
        warehouse_format_epoch,
    )))?;
    verify_domain(
        &relay_principal.public_key,
        CUSTODY_RECEIPT_DOMAIN,
        &unsigned,
        &signature,
    )?;
    Ok(VerifiedCustodyReceipt {
        receipt_id: Hash32::of(bytes),
        verse_id,
        manifest_id,
        custody_set_id,
        relay_principal,
        held_at_ms,
        warehouse_format_epoch,
    })
}
