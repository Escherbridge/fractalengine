//! Scope-key custody, lane resolution, and epoch lifecycle (SPEC-3 §10.1, §10.3); see
//! `src/crypto/AGENTS.md` §scope-key-lifecycle.

use std::collections::{HashMap, HashSet};

use zeroize::Zeroize;

use crate::capability::topic::derive_topic_key;
use crate::envelope::{Hlc, Identifier32, Scope};
use crate::retention::crypto_shred::{
    crypto_shred as run_crypto_shred, CryptoShredError, DestructionDisposition, ScopeKeyId,
    ScopeKeyStore as RetentionScopeKeyStore,
};
use crate::segment::checkpoint_proof::ScopeKeyResolver;
use crate::segment::hashseq::LaneKey;

use super::aead::SealingKey;
use super::key_id::derive_key_id;
use super::CryptoError;

/// Local custody of scope-key material, keyed by `(scope, epoch)` (§10.1.4).
///
/// Reads hand out a borrowed [`SealingKey`], never a copy, so no implementation can produce key
/// bytes that outlive the store's zeroizing drop. Wave 3's `fe-database` store implements this
/// trait and [`RetentionScopeKeyStore`] over persistent storage.
pub trait ScopeKeyStore {
    /// The controlled key for this scope and epoch, or `None` when this store holds none.
    fn scope_key(&self, key: &ScopeKeyId) -> Option<&SealingKey>;

    /// Takes custody of freshly generated material, refusing to overwrite an existing key.
    fn insert_scope_key(
        &mut self,
        key: ScopeKeyId,
        material: SealingKey,
    ) -> Result<(), CryptoError>;

    /// Stops issuing new keys and wraps for this scope and epoch; idempotent (§10.3.2).
    fn stop_issuance(&mut self, key: &ScopeKeyId);

    /// Whether issuance for this scope and epoch has stopped.
    fn issuance_stopped(&self, key: &ScopeKeyId) -> bool;
}

/// A process-local [`ScopeKeyStore`] that also satisfies the SPEC-5 crypto-shred contract.
///
/// Every value is a [`SealingKey`], so removal and drop zeroize the buffer without the store
/// needing to know it happened.
#[derive(Debug, Default)]
pub struct InMemoryScopeKeyStore {
    keys: HashMap<ScopeKeyId, SealingKey>,
    issuance_stopped: HashSet<ScopeKeyId>,
    dispositions: Vec<DestructionDisposition>,
}

impl InMemoryScopeKeyStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Destruction dispositions this store recorded, oldest first.
    pub fn dispositions(&self) -> &[DestructionDisposition] {
        &self.dispositions
    }

    /// How many controlled keys this store currently holds.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether this store holds no controlled key at all.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

impl ScopeKeyStore for InMemoryScopeKeyStore {
    fn scope_key(&self, key: &ScopeKeyId) -> Option<&SealingKey> {
        self.keys.get(key)
    }

    fn insert_scope_key(
        &mut self,
        key: ScopeKeyId,
        material: SealingKey,
    ) -> Result<(), CryptoError> {
        if self.keys.contains_key(&key) {
            return Err(CryptoError::ScopeKeyAlreadyPresent { epoch: key.epoch });
        }
        self.keys.insert(key, material);
        Ok(())
    }

    fn stop_issuance(&mut self, key: &ScopeKeyId) {
        self.issuance_stopped.insert(key.clone());
    }

    fn issuance_stopped(&self, key: &ScopeKeyId) -> bool {
        self.issuance_stopped.contains(key)
    }
}

impl RetentionScopeKeyStore for InMemoryScopeKeyStore {
    fn stop_reissue(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError> {
        ScopeKeyStore::stop_issuance(self, key);
        Ok(())
    }

    fn destroy_scope_key_and_wraps(&mut self, key: &ScopeKeyId) -> Result<(), CryptoShredError> {
        let Some(mut material) = self.keys.remove(key) else {
            return Err(CryptoShredError::KeyNotControlled { key: key.clone() });
        };
        // `SealingKey::drop` would zeroize anyway; doing it here makes the erasure visible at
        // the one place SPEC-5 §5.5.4 calls destruction, rather than an inherited side effect.
        material.zeroize();
        drop(material);
        Ok(())
    }

    fn record_destruction_disposition(
        &mut self,
        disposition: DestructionDisposition,
    ) -> Result<(), CryptoShredError> {
        self.dispositions.push(disposition);
        Ok(())
    }

    fn has_controlled_key(&self, key: &ScopeKeyId) -> bool {
        self.keys.contains_key(key)
    }
}

/// Answers the segment slice's §4.2.4 decrypt-authority question from real custody.
///
/// A [`LaneKey::Header`] carries no epoch, so the verse-wide header epoch is supplied once at
/// construction; a payload lane carries its own epoch inside its topic scope.
#[derive(Debug)]
pub struct LaneScopeKeyResolver<'a, S> {
    store: &'a S,
    header_scope_epoch: u64,
}

impl<'a, S: ScopeKeyStore> LaneScopeKeyResolver<'a, S> {
    /// Binds a resolver to a store and the verse-wide header epoch currently in force.
    pub fn new(store: &'a S, header_scope_epoch: u64) -> Self {
        Self {
            store,
            header_scope_epoch,
        }
    }

    /// The `(scope, epoch)` identity a lane's artifacts are sealed under.
    pub fn lane_scope_key_id(&self, lane: &LaneKey) -> ScopeKeyId {
        match lane {
            LaneKey::Header { verse_id } => ScopeKeyId {
                scope: Scope::verse_wide(*verse_id),
                epoch: self.header_scope_epoch,
            },
            LaneKey::Payload(topic) => ScopeKeyId {
                scope: topic.scope(),
                epoch: topic.scope_epoch,
            },
        }
    }
}

impl<S: ScopeKeyStore> ScopeKeyResolver for LaneScopeKeyResolver<'_, S> {
    /// Custody of the lane's current key is the authority; a cached "yes" never is.
    fn has_decrypt_authority(&self, lane: &LaneKey) -> bool {
        self.store
            .scope_key(&self.lane_scope_key_id(lane))
            .is_some()
    }
}

/// What one §10.3.2 epoch bump produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochBump {
    /// The epoch whose issuance stopped.
    pub retired: ScopeKeyId,
    /// The successor epoch now in force.
    pub current: ScopeKeyId,
    /// Identifier of the successor epoch's freshly generated key.
    pub current_key_id: Identifier32,
}

/// Generation, rotation, and destruction of scope keys over one store (§10.1, §10.3).
#[derive(Debug, Default)]
pub struct ScopeKeyLifecycle<S> {
    store: S,
}

impl<S: ScopeKeyStore> ScopeKeyLifecycle<S> {
    /// Wraps a store.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Borrows the underlying store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// Mutably borrows the underlying store.
    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    /// Unwraps the underlying store.
    pub fn into_store(self) -> S {
        self.store
    }

    /// The key identifier for `(scope, epoch)`, generating the key on first use (§10.1.1).
    ///
    /// Refuses an epoch whose issuance a bump or crypto-shred already stopped, so a stale caller
    /// cannot resurrect a revoked epoch by simply asking for it again.
    pub fn get_or_generate(
        &mut self,
        scope: &Scope,
        epoch: u64,
    ) -> Result<Identifier32, CryptoError> {
        let identity = ScopeKeyId {
            scope: *scope,
            epoch,
        };
        if self.store.issuance_stopped(&identity) {
            return Err(CryptoError::IssuanceStopped { epoch });
        }
        if self.store.scope_key(&identity).is_none() {
            self.store
                .insert_scope_key(identity.clone(), SealingKey::generate()?)?;
        }
        let material = self
            .store
            .scope_key(&identity)
            .ok_or(CryptoError::ScopeKeyUnavailable { epoch })?;
        derive_key_id(scope, epoch, material)
    }

    /// Rotates `epoch` to `epoch + 1`: generates the successor key, then stops `epoch` issuance.
    ///
    /// Ordering matters. Stopping first would make the successor unreachable if generation
    /// failed, leaving the scope with no issuable key at all.
    pub fn on_epoch_bump(&mut self, scope: &Scope, epoch: u64) -> Result<EpochBump, CryptoError> {
        let successor = epoch
            .checked_add(1)
            .ok_or(CryptoError::EpochOverflow { epoch })?;
        let current_key_id = self.get_or_generate(scope, successor)?;
        let retired = ScopeKeyId {
            scope: *scope,
            epoch,
        };
        self.store.stop_issuance(&retired);
        Ok(EpochBump {
            retired,
            current: ScopeKeyId {
                scope: *scope,
                epoch: successor,
            },
            current_key_id,
        })
    }

    /// The §6 blinded topic key for a scope and epoch, derived from that epoch's scope key.
    ///
    /// An epoch bump therefore rotates the topic label as a consequence of rotating the scope
    /// key, rather than as a second step an issuer could forget.
    pub fn topic_key(&self, scope: &Scope, epoch: u64) -> Result<SealingKey, CryptoError> {
        let identity = ScopeKeyId {
            scope: *scope,
            epoch,
        };
        let material = self
            .store
            .scope_key(&identity)
            .ok_or(CryptoError::ScopeKeyUnavailable { epoch })?;
        Ok(SealingKey::from_bytes(derive_topic_key(
            material.expose_bytes(),
            scope,
            epoch,
        )?))
    }
}

impl<S: ScopeKeyStore + RetentionScopeKeyStore> ScopeKeyLifecycle<S> {
    /// Runs the SPEC-5 §5.5.4 crypto-shred sequence against this store (§10.3.3).
    ///
    /// Erasure is key destruction only: immutable ciphertext, already-delivered wraps, and any
    /// uncontrolled copy of the key are outside what this can affect, and this crate never
    /// describes them as deleted.
    pub fn crypto_shred(
        &mut self,
        key: &ScopeKeyId,
        now: Hlc,
    ) -> Result<DestructionDisposition, CryptoShredError> {
        run_crypto_shred(&mut self.store, key, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::crypto::aead::SEALING_KEY_LENGTH;
    use crate::segment::payload_shard::PayloadTopicScope;

    fn verse(filler: u8) -> Scope {
        Scope::verse_wide(Identifier32([filler; 32]))
    }

    fn identity(scope: Scope, epoch: u64) -> ScopeKeyId {
        ScopeKeyId { scope, epoch }
    }

    fn lifecycle() -> ScopeKeyLifecycle<InMemoryScopeKeyStore> {
        ScopeKeyLifecycle::new(InMemoryScopeKeyStore::new())
    }

    /// Copies raw key bytes so a rotation can be compared after the store has moved on.
    fn material_of(
        lifecycle: &ScopeKeyLifecycle<InMemoryScopeKeyStore>,
        scope: Scope,
        epoch: u64,
    ) -> [u8; SEALING_KEY_LENGTH] {
        *lifecycle
            .store()
            .scope_key(&identity(scope, epoch))
            .expect("controlled key")
            .expose_bytes()
    }

    #[test]
    fn first_use_generates_a_scope_key_and_returns_its_identifier() {
        let mut lifecycle = lifecycle();
        assert!(lifecycle.store().is_empty());

        let key_id = lifecycle.get_or_generate(&verse(0x11), 4).expect("key id");
        assert_eq!(lifecycle.store().len(), 1);
        assert_eq!(
            key_id,
            derive_key_id(
                &verse(0x11),
                4,
                lifecycle
                    .store()
                    .scope_key(&identity(verse(0x11), 4))
                    .expect("key")
            )
            .expect("key id")
        );
    }

    #[test]
    fn get_or_generate_is_idempotent_for_one_scope_and_epoch() {
        let mut lifecycle = lifecycle();
        let first = lifecycle.get_or_generate(&verse(0x11), 4).expect("key id");
        let material = material_of(&lifecycle, verse(0x11), 4);

        let second = lifecycle.get_or_generate(&verse(0x11), 4).expect("key id");
        assert_eq!(first, second);
        assert_eq!(material_of(&lifecycle, verse(0x11), 4), material);
        assert_eq!(lifecycle.store().len(), 1);
    }

    #[test]
    fn distinct_scopes_and_epochs_get_distinct_keys() {
        let mut lifecycle = lifecycle();
        let first = lifecycle.get_or_generate(&verse(0x11), 4).expect("key id");
        let other_epoch = lifecycle.get_or_generate(&verse(0x11), 5).expect("key id");
        let other_scope = lifecycle.get_or_generate(&verse(0x12), 4).expect("key id");

        assert_ne!(first, other_epoch);
        assert_ne!(first, other_scope);
        assert_ne!(
            material_of(&lifecycle, verse(0x11), 4),
            material_of(&lifecycle, verse(0x11), 5)
        );
        assert_eq!(lifecycle.store().len(), 3);
    }

    #[test]
    fn generated_scope_keys_are_never_the_zero_key() {
        let mut lifecycle = lifecycle();
        lifecycle.get_or_generate(&verse(0x11), 4).expect("key id");
        assert_ne!(
            material_of(&lifecycle, verse(0x11), 4),
            [0u8; SEALING_KEY_LENGTH]
        );
    }

    #[test]
    fn scope_epoch_bump_rotates_scope_and_topic_keys() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        let before_key_id = lifecycle.get_or_generate(&scope, 4).expect("key id");
        let before_material = material_of(&lifecycle, scope, 4);
        let before_topic = lifecycle.topic_key(&scope, 4).expect("topic key");

        let bump = lifecycle.on_epoch_bump(&scope, 4).expect("bump");
        assert_eq!(bump.retired, identity(scope, 4));
        assert_eq!(bump.current, identity(scope, 5));

        let after_material = material_of(&lifecycle, scope, 5);
        let after_topic = lifecycle.topic_key(&scope, 5).expect("topic key");

        assert_ne!(bump.current_key_id, before_key_id);
        assert_ne!(after_material, before_material);
        assert_ne!(after_topic, before_topic);

        // The topic half is the capability slice's own derivation, not a second construction.
        assert_eq!(
            after_topic,
            SealingKey::from_bytes(
                derive_topic_key(&after_material, &scope, 5).expect("topic key")
            )
        );
    }

    #[test]
    fn a_bumped_epoch_stops_issuing_its_predecessor() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        lifecycle.get_or_generate(&scope, 4).expect("key id");
        lifecycle.on_epoch_bump(&scope, 4).expect("bump");

        assert!(lifecycle.store().issuance_stopped(&identity(scope, 4)));
        assert_eq!(
            lifecycle.get_or_generate(&scope, 4),
            Err(CryptoError::IssuanceStopped { epoch: 4 })
        );
        // The successor is unaffected and still issuable.
        lifecycle.get_or_generate(&scope, 5).expect("key id");
    }

    #[test]
    fn a_bump_of_an_unused_scope_still_produces_the_successor_key() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        let bump = lifecycle.on_epoch_bump(&scope, 0).expect("bump");
        assert_eq!(bump.current, identity(scope, 1));
        assert!(lifecycle.store().scope_key(&identity(scope, 1)).is_some());
    }

    #[test]
    fn a_bump_refuses_an_epoch_with_no_successor() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        assert_eq!(
            lifecycle.on_epoch_bump(&scope, u64::MAX),
            Err(CryptoError::EpochOverflow { epoch: u64::MAX })
        );
    }

    #[test]
    fn a_store_refuses_to_overwrite_a_controlled_key() {
        let mut store = InMemoryScopeKeyStore::new();
        let key = identity(verse(0x11), 4);
        store
            .insert_scope_key(
                key.clone(),
                SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            )
            .expect("insert");
        assert_eq!(
            store.insert_scope_key(key, SealingKey::from_bytes([0x02; SEALING_KEY_LENGTH])),
            Err(CryptoError::ScopeKeyAlreadyPresent { epoch: 4 })
        );
    }

    #[test]
    fn crypto_shred_destroys_the_key_and_blocks_every_reissue_path() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        lifecycle.get_or_generate(&scope, 4).expect("key id");
        let key = identity(scope, 4);

        let disposition = lifecycle
            .crypto_shred(&key, Hlc::new(1_000, 0))
            .expect("shred");
        assert_eq!(disposition.key, key);
        assert_eq!(lifecycle.store().dispositions().len(), 1);
        assert_eq!(lifecycle.store().dispositions()[0], disposition);

        assert!(lifecycle.store().scope_key(&key).is_none());
        assert!(!RetentionScopeKeyStore::has_controlled_key(
            lifecycle.store(),
            &key
        ));
        assert_eq!(
            lifecycle.get_or_generate(&scope, 4),
            Err(CryptoError::IssuanceStopped { epoch: 4 })
        );
        assert_eq!(
            lifecycle.topic_key(&scope, 4),
            Err(CryptoError::ScopeKeyUnavailable { epoch: 4 })
        );
    }

    #[test]
    fn shredding_an_uncontrolled_key_is_refused_by_name() {
        let scope = verse(0x11);
        let mut lifecycle = lifecycle();
        let key = identity(scope, 4);
        assert_eq!(
            lifecycle.crypto_shred(&key, Hlc::new(1, 0)),
            Err(CryptoShredError::KeyNotControlled { key: key.clone() })
        );
        assert!(lifecycle.store().dispositions().is_empty());
    }

    #[test]
    fn the_resolver_reports_decrypt_authority_only_for_lanes_it_holds_keys_for() {
        let mut store = InMemoryScopeKeyStore::new();
        let header_lane = LaneKey::Header {
            verse_id: Identifier32([0x11; 32]),
        };
        let payload_lane = LaneKey::Payload(PayloadTopicScope {
            verse_id: Identifier32([0x11; 32]),
            petal_id: Identifier32([0x22; 32]),
            scope_epoch: 4,
            key_id: Identifier32([0x77; 32]),
        });

        store
            .insert_scope_key(
                identity(verse(0x11), 4),
                SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            )
            .expect("insert header key");

        let resolver = LaneScopeKeyResolver::new(&store, 4);
        assert!(resolver.has_decrypt_authority(&header_lane));
        assert!(!resolver.has_decrypt_authority(&payload_lane));

        // Header visibility never implies payload access (SPEC-6 §2.2.3).
        let petal = Scope::new(
            Identifier32([0x11; 32]),
            Some(Identifier32([0x22; 32])),
            None,
        )
        .expect("petal scope");
        store
            .insert_scope_key(
                identity(petal, 4),
                SealingKey::from_bytes([0x02; SEALING_KEY_LENGTH]),
            )
            .expect("insert payload key");
        let resolver = LaneScopeKeyResolver::new(&store, 4);
        assert!(resolver.has_decrypt_authority(&payload_lane));
    }

    #[test]
    fn the_resolver_refuses_a_header_lane_at_a_different_epoch() {
        let mut store = InMemoryScopeKeyStore::new();
        store
            .insert_scope_key(
                identity(verse(0x11), 4),
                SealingKey::from_bytes([0x01; SEALING_KEY_LENGTH]),
            )
            .expect("insert");
        let header_lane = LaneKey::Header {
            verse_id: Identifier32([0x11; 32]),
        };

        assert!(LaneScopeKeyResolver::new(&store, 4).has_decrypt_authority(&header_lane));
        assert!(!LaneScopeKeyResolver::new(&store, 5).has_decrypt_authority(&header_lane));
    }
}
