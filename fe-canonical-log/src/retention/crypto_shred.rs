//! Crypto-shredding (SPEC-5 §5.5, §5.5.4); see `src/retention/AGENTS.md` §crypto-shred.
//!
//! Erasure here is key destruction only. This module never claims byte deletion of a
//! replicated segment: immutable ciphertext and any uncontrolled copy of a scope key are
//! explicitly outside what `crypto_shred` can affect.

use thiserror::Error;
use zeroize::Zeroize;

use crate::envelope::{Hlc, Scope};

/// Identifies one scope key by its scope and epoch.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScopeKeyId {
    /// The authorization scope this key encrypts payloads for.
    pub scope: Scope,
    /// The epoch this key belongs to.
    pub epoch: u64,
}

/// Record that a scope key was destroyed. MUST NOT and structurally cannot carry the destroyed
/// key's bytes: this type has no byte-array field, only the key's identity, when destruction
/// happened, and a free-form disposition note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestructionDisposition {
    /// The key that was destroyed.
    pub key: ScopeKeyId,
    /// When destruction was recorded.
    pub destroyed_at: Hlc,
    /// Free-form disposition note (e.g. which local stores were affected).
    pub note: String,
}

/// Every reason a crypto-shred step fails.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CryptoShredError {
    /// The store holds no controlled copy of this key.
    #[error("scope key {key:?} is not controlled by this store")]
    KeyNotControlled {
        /// The key that was not found.
        key: ScopeKeyId,
    },
    /// The store failed to record the destruction disposition.
    #[error("failed to record destruction disposition for {key:?}")]
    DispositionRecordingFailed {
        /// The key whose disposition failed to record.
        key: ScopeKeyId,
    },
}

/// Storage seam for scope keys and their wraps; Wave 3 implements this in `fe-database`.
pub trait ScopeKeyStore {
    /// Stops issuing new wraps of this key. Idempotent; does not require the key be present.
    fn stop_reissue(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError>;
    /// Destroys the controlled copy of this key and any ephemeral wrap secrets derived from it,
    /// and removes the controlled wraps this store holds (best-effort, local only; §10.3.3).
    fn destroy_scope_key_and_wraps(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError>;
    /// Records a destruction disposition for audit.
    fn record_destruction_disposition(
        &mut self,
        disposition: DestructionDisposition,
    ) -> Result<(), CryptoShredError>;
    /// Reports whether this store still holds a controlled copy of `key`.
    fn has_controlled_key(&self, key: &ScopeKeyId) -> bool;
}

/// Runs the §5.5.4 crypto-shred sequence in order: stop reissue, then destroy the key and its
/// wraps, then record the disposition. Returns the recorded disposition.
pub fn crypto_shred(
    store: &mut impl ScopeKeyStore,
    key: &ScopeKeyId,
    now: Hlc,
) -> Result<DestructionDisposition, CryptoShredError> {
    store.stop_reissue(key)?;
    store.destroy_scope_key_and_wraps(key)?;
    let disposition = DestructionDisposition {
        key: key.clone(),
        destroyed_at: now,
        note: "best-effort local wrap removal; ciphertext and uncontrolled copies unaffected"
            .to_owned(),
    };
    store.record_destruction_disposition(disposition.clone())?;
    Ok(disposition)
}

/// Bumps a scope's key epoch by exactly one after a member is removed (D-CL24-adjacent access
/// revocation trigger). The caller is responsible for issuing and distributing the new epoch's
/// key; this function only derives the successor epoch number.
pub fn on_member_removed_bump_epoch(epoch: u64) -> u64 {
    epoch + 1
}

/// Explicitly wipes a 32-byte key buffer. A bare `[u8; 32]` drop does not guarantee the
/// compiler will not have already left copies on the stack; `zeroize::Zeroize` does.
pub fn zero_key_bytes(key: &mut [u8; 32]) {
    key.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    use crate::envelope::Identifier32;

    fn scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    #[derive(Default)]
    struct InMemoryScopeKeyStore {
        controlled: HashSet<ScopeKeyId>,
        reissue_stopped: HashSet<ScopeKeyId>,
        dispositions: HashMap<ScopeKeyId, DestructionDisposition>,
    }

    impl ScopeKeyStore for InMemoryScopeKeyStore {
        fn stop_reissue(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError> {
            self.reissue_stopped.insert(key.clone());
            Ok(())
        }

        fn destroy_scope_key_and_wraps(
            &mut self,
            key: &ScopeKeyId,
        ) -> Result<(), CryptoShredError> {
            if !self.controlled.remove(key) {
                return Err(CryptoShredError::KeyNotControlled { key: key.clone() });
            }
            Ok(())
        }

        fn record_destruction_disposition(
            &mut self,
            disposition: DestructionDisposition,
        ) -> Result<(), CryptoShredError> {
            self.dispositions
                .insert(disposition.key.clone(), disposition);
            Ok(())
        }

        fn has_controlled_key(&self, key: &ScopeKeyId) -> bool {
            self.controlled.contains(key)
        }
    }

    #[test]
    fn crypto_shredding_preserves_ciphertext_but_revokes_future_key_access() {
        let key = ScopeKeyId {
            scope: scope(),
            epoch: 4,
        };
        let mut store = InMemoryScopeKeyStore::default();
        store.controlled.insert(key.clone());
        assert!(store.has_controlled_key(&key));

        let disposition = crypto_shred(&mut store, &key, Hlc::new(1_000, 0)).expect("shred");

        // Future key access is revoked: the store no longer holds a controlled copy.
        assert!(!store.has_controlled_key(&key));
        assert!(store.reissue_stopped.contains(&key));
        assert_eq!(disposition.key, key);
        assert_eq!(disposition.destroyed_at, Hlc::new(1_000, 0));
        assert_eq!(store.dispositions.get(&key), Some(&disposition));

        // This module never models ciphertext at all: `ScopeKeyStore` has no byte-store method,
        // so nothing crypto_shred does can touch a stored artifact. The disposition itself is
        // structurally incapable of carrying key bytes (no byte-array field on the type).
    }

    #[test]
    fn shredding_an_uncontrolled_key_fails_without_recording_a_disposition() {
        let key = ScopeKeyId {
            scope: scope(),
            epoch: 1,
        };
        let mut store = InMemoryScopeKeyStore::default();
        assert_eq!(
            crypto_shred(&mut store, &key, Hlc::new(1, 0)),
            Err(CryptoShredError::KeyNotControlled { key: key.clone() })
        );
        assert!(!store.dispositions.contains_key(&key));
    }

    #[test]
    fn member_removal_bumps_epoch_by_exactly_one() {
        assert_eq!(on_member_removed_bump_epoch(7), 8);
        assert_eq!(on_member_removed_bump_epoch(0), 1);
    }

    #[test]
    fn zero_key_bytes_wipes_the_buffer() {
        let mut key = [0xabu8; 32];
        zero_key_bytes(&mut key);
        assert_eq!(key, [0u8; 32]);
    }
}
