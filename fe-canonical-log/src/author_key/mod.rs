//! Author identity lifecycle: key rotation, lineage, disavow, and continuity grants (SPEC-2).
//!
//! Rationale, the provisional wire numbering, and the deferred fork resolver live in
//! `src/author_key/AGENTS.md`.

pub mod admission;
pub mod continuity_grant;
pub mod disavow;
pub mod disavow_rescind;
pub mod lineage;
pub mod payloads;
pub mod rotation_proof;

use crate::cbor::CborValue;
use crate::envelope::EnvelopeError;

/// Requires exactly the integer keys `0..expected_key_count`, once each and nothing else.
pub(crate) fn require_numeric_keys(
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

/// Reads one listed key out of a numeric-keyed map.
pub(crate) fn entry<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, EnvelopeError> {
    value
        .get_uint_key(key)
        .ok_or(EnvelopeError::MissingKey { context, key })
}

/// Reads an unsigned integer slot.
pub(crate) fn unsigned_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u64, EnvelopeError> {
    entry(value, key, context)?
        .as_uint()
        .ok_or(EnvelopeError::ExpectedUnsignedInteger { context, key })
}

/// Reads an unsigned integer slot that must fit in `u16`.
pub(crate) fn u16_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u16, EnvelopeError> {
    let raw = unsigned_at(value, key, context)?;
    u16::try_from(raw).map_err(|_| EnvelopeError::IntegerOutOfRange {
        context,
        key,
        value: raw,
    })
}

/// Reads a byte-string slot of exactly `LENGTH` bytes.
pub(crate) fn bytes_at<const LENGTH: usize>(
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

/// Reads a text-string slot.
pub(crate) fn text_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<String, EnvelopeError> {
    Ok(entry(value, key, context)?
        .as_text()
        .ok_or(EnvelopeError::ExpectedTextString { context, key })?
        .to_owned())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::{BTreeMap, BTreeSet};

    use ed25519_dalek::SigningKey;

    use super::admission::{
        AuthorityState, AuthorizationState, CapabilityAuthorityView, RegisteredSchemas,
    };
    use super::disavow_rescind::AuthorityLevel;
    use super::lineage::CausalOperationView;
    use crate::envelope::{
        Author, CapabilityRef, EncryptionParams, Hash32, Hlc, Identifier32, PayloadRef, Scope,
        UnsignedEnvelope, NONCE_LENGTH, PRODUCTION_SUITE_ID, PROTOCOL_VERSION,
    };

    /// Deterministic Ed25519 signing key for a one-byte seed.
    pub(crate) fn signing_key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    /// Public key of [`signing_key`].
    pub(crate) fn public_key(seed: u8) -> [u8; 32] {
        signing_key(seed).verifying_key().to_bytes()
    }

    /// Filler hash.
    pub(crate) fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    /// Filler identifier.
    pub(crate) fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    /// The verse used by every author-key test unless a case needs a second verse.
    pub(crate) fn verse_scope() -> Scope {
        Scope::verse_wide(identifier(0x11))
    }

    /// A petal scope inside [`verse_scope`].
    pub(crate) fn petal_scope() -> Scope {
        Scope::new(identifier(0x11), Some(identifier(0x12)), None).expect("petal scope")
    }

    /// A resource scope inside [`petal_scope`].
    pub(crate) fn resource_scope() -> Scope {
        Scope::new(
            identifier(0x11),
            Some(identifier(0x12)),
            Some(identifier(0x13)),
        )
        .expect("resource scope")
    }

    /// Schema hashes the tests register; production callers supply their own.
    pub(crate) fn schemas() -> RegisteredSchemas {
        RegisteredSchemas {
            rotation: hash(0xa1),
            disavow: hash(0xa2),
            disavow_rescind: hash(0xa3),
            continuity_grant: hash(0xa4),
        }
    }

    /// In-memory causal DAG: parent edges plus an admitted set.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct FakeDag {
        parents: BTreeMap<Hash32, Vec<Hash32>>,
        admitted: BTreeSet<Hash32>,
    }

    impl FakeDag {
        /// Adds an admitted operation with the given parents.
        pub(crate) fn insert(&mut self, operation: Hash32, parents: Vec<Hash32>) -> &mut Self {
            self.parents.insert(operation, parents);
            self.admitted.insert(operation);
            self
        }

        /// Adds an operation that is present but not yet admitted.
        pub(crate) fn insert_unadmitted(
            &mut self,
            operation: Hash32,
            parents: Vec<Hash32>,
        ) -> &mut Self {
            self.parents.insert(operation, parents);
            self.admitted.remove(&operation);
            self
        }
    }

    impl CausalOperationView for FakeDag {
        fn parents(&self, operation: Hash32) -> Option<Vec<Hash32>> {
            self.parents.get(&operation).cloned()
        }

        fn is_admitted(&self, operation: Hash32) -> bool {
            self.admitted.contains(&operation)
        }

        fn reaches(&self, from: Hash32, target: Hash32) -> bool {
            let mut pending = vec![from];
            let mut seen = BTreeSet::new();
            while let Some(current) = pending.pop() {
                if current == target {
                    return true;
                }
                if !seen.insert(current) {
                    continue;
                }
                if let Some(parents) = self.parents.get(&current) {
                    pending.extend(parents.iter().copied());
                }
            }
            false
        }
    }

    /// In-memory authorization answers keyed by author public key.
    #[derive(Clone, Debug, Default)]
    pub(crate) struct FakeAuthority {
        authorized: BTreeSet<([u8; 32], u64)>,
        capability_unavailable: BTreeSet<[u8; 32]>,
        epoch_unavailable: BTreeSet<[u8; 32]>,
        roles: BTreeMap<[u8; 32], AuthorityLevel>,
        role_unavailable: BTreeSet<[u8; 32]>,
    }

    impl FakeAuthority {
        /// Marks `key` authorized at `scope_epoch`.
        pub(crate) fn authorize(&mut self, key: [u8; 32], scope_epoch: u64) -> &mut Self {
            self.authorized.insert((key, scope_epoch));
            self
        }

        /// Makes the capability chain for `key` unresolvable.
        pub(crate) fn hide_capability_chain(&mut self, key: [u8; 32]) -> &mut Self {
            self.capability_unavailable.insert(key);
            self
        }

        /// Makes the epoch state for `key` unresolvable.
        pub(crate) fn hide_epoch_state(&mut self, key: [u8; 32]) -> &mut Self {
            self.epoch_unavailable.insert(key);
            self
        }

        /// Sets the resolved authority level of `key`.
        pub(crate) fn set_role(&mut self, key: [u8; 32], role: AuthorityLevel) -> &mut Self {
            self.roles.insert(key, role);
            self
        }

        /// Makes the authority level of `key` unresolvable.
        pub(crate) fn hide_role(&mut self, key: [u8; 32]) -> &mut Self {
            self.role_unavailable.insert(key);
            self
        }
    }

    impl CapabilityAuthorityView for FakeAuthority {
        fn predecessor_authorized(
            &self,
            author_public_key: &[u8; 32],
            _scope: &Scope,
            scope_epoch: u64,
        ) -> AuthorizationState {
            if self.capability_unavailable.contains(author_public_key) {
                return AuthorizationState::CapabilityChainUnavailable;
            }
            if self.epoch_unavailable.contains(author_public_key) {
                return AuthorizationState::EpochStateUnavailable;
            }
            if self.authorized.contains(&(*author_public_key, scope_epoch)) {
                AuthorizationState::Authorized
            } else {
                AuthorizationState::Unauthorized
            }
        }

        fn issuer_role(
            &self,
            author_public_key: &[u8; 32],
            _scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            if self.role_unavailable.contains(author_public_key) {
                return AuthorityState::Unavailable;
            }
            AuthorityState::Resolved(
                self.roles
                    .get(author_public_key)
                    .copied()
                    .unwrap_or(AuthorityLevel::None),
            )
        }
    }

    /// A kind-1 envelope carrying an encrypted payload, which is what every SPEC-2 lifecycle
    /// operation is.
    pub(crate) fn intent_envelope(
        author_public_key: [u8; 32],
        scope: Scope,
        branch_id: Identifier32,
        parents: Vec<Hash32>,
        scope_epoch: u64,
        schema_hash: Hash32,
        hlc: Hlc,
    ) -> UnsignedEnvelope {
        let ciphertext = [0xc0u8, 0xff, 0xee];
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 1,
            scope,
            author: Author::from_public_key(author_public_key),
            capability: CapabilityRef {
                chain_id: hash(0x33),
                scope_epoch,
            },
            schema_hash,
            branch_id,
            parents,
            hlc,
            payload: PayloadRef {
                ciphertext_hash: Hash32::of(&ciphertext),
                ciphertext_length: ciphertext.len() as u64,
                encryption: Some(EncryptionParams {
                    suite_id: PRODUCTION_SUITE_ID,
                    key_id: identifier(0x77),
                    nonce: [0x88; NONCE_LENGTH],
                }),
            },
        }
    }
}
