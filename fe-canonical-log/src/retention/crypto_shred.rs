//! Crypto-shredding (SPEC-5 §5.5, §5.5.4); see `src/retention/AGENTS.md` §crypto-shred.
//!
//! Erasure here is key destruction only. This module never claims byte deletion of a
//! replicated segment: immutable ciphertext and any uncontrolled copy of a scope key are
//! explicitly outside what `crypto_shred` can affect.

use thiserror::Error;
use zeroize::Zeroize;

use crate::crypto::scope_key::ScopeKeyStore;
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

/// The destruction/audit half of crypto-shred custody (SPEC-5 §5.5.4).
///
/// Kept separate from [`crate::crypto::scope_key::ScopeKeyStore`] (the one custody trait: point
/// lookup, insertion, and issuance control) rather than merged into it, so crypto code never
/// needs disposition bookkeeping and retention code never needs read/generate methods.
/// [`crypto_shred`] takes the blanket bound `S: ScopeKeyStore + ShredLedger`. Before W4-4 this
/// module defined its OWN, near-duplicate `ScopeKeyStore` trait (point lookup absent, four
/// destruction-shaped methods instead) — the exact "two traits, one name, two modules" shape
/// `src/AGENTS.md` §unified-vocabularies forbids for a wire vocabulary, and forbids here too for
/// the same reason: a reviewer who checked one had not checked the other. `has_controlled_key`
/// is gone with it; it was exactly `ScopeKeyStore::scope_key(key).is_some()` restated.
pub trait ShredLedger {
    /// Destroys the controlled copy of this key and any ephemeral wrap secrets derived from it,
    /// and removes the controlled wraps this store holds (best-effort, local only; §10.3.3).
    fn destroy_scope_key_and_wraps(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError>;
    /// Records a destruction disposition for audit.
    fn record_destruction_disposition(
        &mut self,
        disposition: DestructionDisposition,
    ) -> Result<(), CryptoShredError>;
}

/// Runs the §5.5.4 crypto-shred sequence in order: stop reissue, then destroy the key and its
/// wraps, then record the disposition. Returns the recorded disposition.
pub fn crypto_shred<S>(
    store: &mut S,
    key: &ScopeKeyId,
    now: Hlc,
) -> Result<DestructionDisposition, CryptoShredError>
where
    S: ScopeKeyStore + ShredLedger,
{
    store.stop_issuance(key);
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

    use crate::crypto::aead::{SealingKey, SEALING_KEY_LENGTH};
    use crate::crypto::scope_key::InMemoryScopeKeyStore;
    use crate::envelope::Identifier32;

    fn scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    #[test]
    fn crypto_shredding_preserves_ciphertext_but_revokes_future_key_access() {
        let key = ScopeKeyId {
            scope: scope(),
            epoch: 4,
        };
        let mut store = InMemoryScopeKeyStore::new();
        store
            .insert_scope_key(
                key.clone(),
                SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            )
            .expect("insert");
        assert!(store.scope_key(&key).is_some());

        let disposition = crypto_shred(&mut store, &key, Hlc::new(1_000, 0)).expect("shred");

        // Future key access is revoked: the store no longer holds a controlled copy, and
        // issuance for this (scope, epoch) has stopped -- `crypto_shred` runs
        // `ScopeKeyStore::stop_issuance` first, exactly as `stop_reissue` used to.
        assert!(store.scope_key(&key).is_none());
        assert!(store.issuance_stopped(&key));
        assert_eq!(disposition.key, key);
        assert_eq!(disposition.destroyed_at, Hlc::new(1_000, 0));
        assert_eq!(store.dispositions().to_vec(), vec![disposition]);

        // This module never models ciphertext at all: neither trait has a byte-store method, so
        // nothing crypto_shred does can touch a stored artifact. The disposition itself is
        // structurally incapable of carrying key bytes (no byte-array field on the type).
    }

    #[test]
    fn shredding_an_uncontrolled_key_fails_without_recording_a_disposition() {
        let key = ScopeKeyId {
            scope: scope(),
            epoch: 1,
        };
        let mut store = InMemoryScopeKeyStore::new();
        assert_eq!(
            crypto_shred(&mut store, &key, Hlc::new(1, 0)),
            Err(CryptoShredError::KeyNotControlled { key: key.clone() })
        );
        assert!(store.dispositions().is_empty());
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
