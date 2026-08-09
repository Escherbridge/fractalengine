//! Session authorization (§2.2): `authorize`/`authorized`/`authorization_revalidation_required`.

use std::collections::HashMap;

use async_trait::async_trait;

use crate::cbor::CborValue;
use crate::envelope::{Author, Hash32, Scope};

use super::error::WireError;
use super::{
    author_at, bytes_at, fixed_bytes_at, hash32_at, require_exact_keys, scope_at, scope_entry,
    uint_at,
};

/// Provisional stand-in for the sibling capability slice's `capability::verbs::VerbSet` bit
/// mask. Reconciled in the serial integration wave; see `src/wire/AGENTS.md`.
pub type VerbBits = u8;

/// Provisional stand-in for `capability::verbs::ObjectClassSet`; see [`VerbBits`].
pub type ObjectClassBits = u8;

const AUTHORIZE_CONTEXT: &str = "authorize";
const AUTHORIZED_CONTEXT: &str = "authorized";
const REVALIDATION_CONTEXT: &str = "authorization_revalidation_required";

/// §2.2.7 `authorize` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizeBody {
    /// Exact complete canonical capability-chain bytes (SPEC-3).
    pub capability_chain_bytes: Vec<u8>,
    /// Client-chosen correlation ID for this binding.
    pub authorization_binding_id: [u8; 16],
    /// Requested verb bitmask.
    pub requested_verb: VerbBits,
    /// Requested object-class bitmask.
    pub requested_object_class: ObjectClassBits,
    /// Requested exact scope.
    pub requested_scope: Scope,
}

impl AuthorizeBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.capability_chain_bytes.clone()),
            ),
            (
                CborValue::Uint(1),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (
                CborValue::Uint(2),
                CborValue::Uint(u64::from(self.requested_verb)),
            ),
            (
                CborValue::Uint(3),
                CborValue::Uint(u64::from(self.requested_object_class)),
            ),
            (CborValue::Uint(4), scope_entry(&self.requested_scope)?),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4], AUTHORIZE_CONTEXT)?;
        let requested_verb = uint_at(value, 2, AUTHORIZE_CONTEXT)?;
        let requested_object_class = uint_at(value, 3, AUTHORIZE_CONTEXT)?;
        Ok(Self {
            capability_chain_bytes: bytes_at(value, 0, AUTHORIZE_CONTEXT)?.to_vec(),
            authorization_binding_id: fixed_bytes_at::<16>(value, 1, AUTHORIZE_CONTEXT)?,
            requested_verb: VerbBits::try_from(requested_verb).map_err(|_| {
                WireError::WrongShape {
                    context: AUTHORIZE_CONTEXT,
                    key: 2,
                }
            })?,
            requested_object_class: ObjectClassBits::try_from(requested_object_class).map_err(
                |_| WireError::WrongShape {
                    context: AUTHORIZE_CONTEXT,
                    key: 3,
                },
            )?,
            requested_scope: scope_at(value, 4, AUTHORIZE_CONTEXT)?,
        })
    }
}

/// §2.2.7 `authorized` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedBody {
    /// The binding ID accepted from `authorize`.
    pub authorization_binding_id: [u8; 16],
    /// The freshly accepted session authorization generation.
    pub session_generation: u64,
    /// The leaf principal bound by the verified chain.
    pub leaf_principal: Author,
    /// The verified chain's identifier.
    pub chain_id: Hash32,
    /// The epoch scope the chain's root establishes.
    pub epoch_scope: Scope,
    /// The scope epoch the author relied on.
    pub scope_epoch: u64,
    /// Expiration, Unix milliseconds.
    pub expires_at_ms: u64,
}

impl AuthorizedBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (CborValue::Uint(1), CborValue::Uint(self.session_generation)),
            (CborValue::Uint(2), self.leaf_principal.to_cbor()),
            (
                CborValue::Uint(3),
                CborValue::Bytes(self.chain_id.as_bytes().to_vec()),
            ),
            (CborValue::Uint(4), scope_entry(&self.epoch_scope)?),
            (CborValue::Uint(5), CborValue::Uint(self.scope_epoch)),
            (CborValue::Uint(6), CborValue::Uint(self.expires_at_ms)),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3, 4, 5, 6], AUTHORIZED_CONTEXT)?;
        Ok(Self {
            authorization_binding_id: fixed_bytes_at::<16>(value, 0, AUTHORIZED_CONTEXT)?,
            session_generation: uint_at(value, 1, AUTHORIZED_CONTEXT)?,
            leaf_principal: author_at(value, 2, AUTHORIZED_CONTEXT)?,
            chain_id: hash32_at(value, 3, AUTHORIZED_CONTEXT)?,
            epoch_scope: scope_at(value, 4, AUTHORIZED_CONTEXT)?,
            scope_epoch: uint_at(value, 5, AUTHORIZED_CONTEXT)?,
            expires_at_ms: uint_at(value, 6, AUTHORIZED_CONTEXT)?,
        })
    }
}

/// §6.1 revalidation reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevalidationReason {
    /// The capability chain expired.
    CapabilityExpired,
    /// The persistent scope epoch advanced past what the chain relied on.
    ScopeEpochAdvanced,
    /// The persistent authorization view changed authority for this principal.
    AuthorityChanged,
    /// A replacement session generation superseded this one.
    SessionReplaced,
}

impl RevalidationReason {
    const fn to_u64(self) -> u64 {
        match self {
            Self::CapabilityExpired => 0,
            Self::ScopeEpochAdvanced => 1,
            Self::AuthorityChanged => 2,
            Self::SessionReplaced => 3,
        }
    }

    const fn from_u64(value: u64) -> Option<Self> {
        Some(match value {
            0 => Self::CapabilityExpired,
            1 => Self::ScopeEpochAdvanced,
            2 => Self::AuthorityChanged,
            3 => Self::SessionReplaced,
            _ => return None,
        })
    }
}

/// §6.1 `authorization_revalidation_required` body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationRevalidationRequiredBody {
    /// The affected binding.
    pub authorization_binding_id: [u8; 16],
    /// The affected scope.
    pub scope: Scope,
    /// The session generation this notice invalidates.
    pub invalidated_session_generation: u64,
    /// The public reason.
    pub reason: RevalidationReason,
}

impl AuthorizationRevalidationRequiredBody {
    /// Encodes the body map.
    pub fn to_cbor(&self) -> Result<CborValue, WireError> {
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                CborValue::Bytes(self.authorization_binding_id.to_vec()),
            ),
            (CborValue::Uint(1), scope_entry(&self.scope)?),
            (
                CborValue::Uint(2),
                CborValue::Uint(self.invalidated_session_generation),
            ),
            (CborValue::Uint(3), CborValue::Uint(self.reason.to_u64())),
        ]))
    }

    /// Decodes the body map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, WireError> {
        require_exact_keys(value, &[0, 1, 2, 3], REVALIDATION_CONTEXT)?;
        let reason_raw = uint_at(value, 3, REVALIDATION_CONTEXT)?;
        Ok(Self {
            authorization_binding_id: fixed_bytes_at::<16>(value, 0, REVALIDATION_CONTEXT)?,
            scope: scope_at(value, 1, REVALIDATION_CONTEXT)?,
            invalidated_session_generation: uint_at(value, 2, REVALIDATION_CONTEXT)?,
            reason: RevalidationReason::from_u64(reason_raw).ok_or(
                WireError::UnknownDiscriminant {
                    context: REVALIDATION_CONTEXT,
                    key: 3,
                    value: reason_raw,
                },
            )?,
        })
    }
}

/// What a verified capability chain establishes for a session (§2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedAuthorization {
    /// Leaf principal the chain binds.
    pub leaf_principal: Author,
    /// Verified chain identifier.
    pub chain_id: Hash32,
    /// Epoch scope the chain's root establishes.
    pub epoch_scope: Scope,
    /// Scope epoch relied upon.
    pub scope_epoch: u64,
    /// Expiration, Unix milliseconds.
    pub expires_at_ms: u64,
}

/// Consumed, not implemented, by this slice: verifies a capability chain against the
/// persistent authorization view. The concrete implementation binds to the sibling
/// capability slice's `chain::verify_chain` in the serial integration wave.
#[async_trait]
pub trait CapabilityVerifier: Send + Sync {
    /// Verifies `capability_chain_bytes` grants `requested_verb`/`requested_object_class` over
    /// `requested_scope`, returning the facts a session binding needs.
    async fn verify(
        &self,
        capability_chain_bytes: &[u8],
        requested_verb: VerbBits,
        requested_object_class: ObjectClassBits,
        requested_scope: &Scope,
    ) -> Result<VerifiedAuthorization, WireError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BindingRecord {
    chain_id: Hash32,
    leaf_principal_public_key: [u8; 32],
}

/// §2.2 session-wide monotonic authorization-generation bookkeeping.
///
/// One table per socket session. `current_generation` is the single value every other
/// stateful handler must match; it changes on every accepted `authorize` and on every
/// [`SessionAuthorizationTable::invalidate`] call, and `0` is never a generation any
/// `authorize` call can produce, so it is a safe "nothing is currently valid" sentinel.
pub struct SessionAuthorizationTable {
    next_generation: u64,
    current_generation: u64,
    bindings: HashMap<[u8; 16], BindingRecord>,
}

impl Default for SessionAuthorizationTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionAuthorizationTable {
    /// Builds an empty table with no currently valid generation.
    pub fn new() -> Self {
        Self {
            next_generation: 1,
            current_generation: 0,
            bindings: HashMap::new(),
        }
    }

    /// Accepts a verified binding, issuing a fresh session generation.
    ///
    /// Rejects a binding ID collision that names a different chain or principal (§2.2.7).
    pub fn accept_binding(
        &mut self,
        binding_id: [u8; 16],
        chain_id: Hash32,
        leaf_principal_public_key: [u8; 32],
    ) -> Result<u64, WireError> {
        if let Some(existing) = self.bindings.get(&binding_id) {
            if existing.chain_id != chain_id
                || existing.leaf_principal_public_key != leaf_principal_public_key
            {
                return Err(WireError::AuthorizationBindingCollision);
            }
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        self.current_generation = generation;
        self.bindings.insert(
            binding_id,
            BindingRecord {
                chain_id,
                leaf_principal_public_key,
            },
        );
        Ok(generation)
    }

    /// Reports whether `generation` is the current accepted session generation.
    pub fn is_generation_valid(&self, generation: u64) -> bool {
        generation != 0 && generation == self.current_generation
    }

    /// The current accepted generation, or `0` if authorization is not yet established.
    pub fn current_generation(&self) -> u64 {
        self.current_generation
    }

    /// Invalidates the current generation (epoch bump, expiry, or replacement) and returns it.
    pub fn invalidate(&mut self) -> u64 {
        let invalidated = self.current_generation;
        self.current_generation = 0;
        invalidated
    }

    /// Reports whether `binding_id` is bound, and to which chain and principal.
    pub fn binding(&self, binding_id: [u8; 16]) -> Option<(Hash32, [u8; 32])> {
        self.bindings
            .get(&binding_id)
            .map(|record| (record.chain_id, record.leaf_principal_public_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::{decode_canonical, encode_canonical_checked};
    use crate::envelope::Identifier32;

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    #[test]
    fn authorize_body_round_trips() {
        let body = AuthorizeBody {
            capability_chain_bytes: vec![1, 2, 3],
            authorization_binding_id: [0xaa; 16],
            requested_verb: 0x01,
            requested_object_class: 0x02,
            requested_scope: Scope::verse_wide(Identifier32([0x11; 32])),
        };
        let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            AuthorizeBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            body
        );
    }

    #[test]
    fn authorized_body_round_trips() {
        let body = AuthorizedBody {
            authorization_binding_id: [0xaa; 16],
            session_generation: 7,
            leaf_principal: Author::from_public_key([0x22; 32]),
            chain_id: hash(0x33),
            epoch_scope: Scope::verse_wide(Identifier32([0x11; 32])),
            scope_epoch: 4,
            expires_at_ms: 1_700_000_000_000,
        };
        let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
        assert_eq!(
            AuthorizedBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode"),
            body
        );
    }

    #[test]
    fn revalidation_body_round_trips_every_reason() {
        for reason in [
            RevalidationReason::CapabilityExpired,
            RevalidationReason::ScopeEpochAdvanced,
            RevalidationReason::AuthorityChanged,
            RevalidationReason::SessionReplaced,
        ] {
            let body = AuthorizationRevalidationRequiredBody {
                authorization_binding_id: [0xbb; 16],
                scope: Scope::verse_wide(Identifier32([0x11; 32])),
                invalidated_session_generation: 5,
                reason,
            };
            let bytes = encode_canonical_checked(&body.to_cbor().expect("cbor")).expect("encode");
            assert_eq!(
                AuthorizationRevalidationRequiredBody::from_cbor(
                    &decode_canonical(&bytes).expect("decode")
                )
                .expect("decode"),
                body
            );
        }
    }

    #[test]
    fn session_generation_is_monotonic_and_zero_is_never_valid() {
        let mut table = SessionAuthorizationTable::new();
        assert!(!table.is_generation_valid(0));

        let first = table
            .accept_binding([1; 16], hash(0x01), [0x02; 32])
            .expect("first accept");
        assert_eq!(first, 1);
        assert!(table.is_generation_valid(1));

        let second = table
            .accept_binding([2; 16], hash(0x03), [0x04; 32])
            .expect("second accept, distinct binding");
        assert!(second > first);
        assert!(table.is_generation_valid(second));
        assert!(!table.is_generation_valid(first));

        let invalidated = table.invalidate();
        assert_eq!(invalidated, second);
        assert!(!table.is_generation_valid(second));
        assert_eq!(table.current_generation(), 0);
    }

    #[test]
    fn a_binding_id_collision_with_a_different_chain_or_principal_is_rejected() {
        let mut table = SessionAuthorizationTable::new();
        table
            .accept_binding([9; 16], hash(0x01), [0x02; 32])
            .expect("first accept");

        assert_eq!(
            table.accept_binding([9; 16], hash(0x01), [0x02; 32]),
            Ok(2),
            "identical rebinding is accepted (a fresh generation), not a collision"
        );
        assert_eq!(
            table.accept_binding([9; 16], hash(0xff), [0x02; 32]),
            Err(WireError::AuthorizationBindingCollision)
        );
        assert_eq!(
            table.accept_binding([9; 16], hash(0x01), [0xff; 32]),
            Err(WireError::AuthorizationBindingCollision)
        );
    }
}
