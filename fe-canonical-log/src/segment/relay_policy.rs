//! Relay seed/fetch authorization decisions (SPEC-6 §6) and scope-key currency (§9.2).
//!
//! This module contains NO socket, listener, connection, iroh, or libp2p code. It does not
//! reference `fe-network` or `fe-sync`, opens nothing, and cannot change
//! `fe-sync`'s `IrohDocsEngineHolder::is_available()`, which stays `false`. It decides
//! whether a hypothetical transport WOULD be permitted to seed or disclose an artifact; the
//! transport itself remains owner-gated and unbuilt.

use crate::envelope::Identifier32;

use super::artifact::EncryptionDescriptor;
use super::hashseq::LaneKey;
use super::SegmentError;

/// A peer or recipient device, identified by its Ed25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerIdentity(
    /// Raw public key bytes.
    pub [u8; 32],
);

/// The persistent SPEC-3 authorization view a relay MUST consult, including after restart.
///
/// Every method reads current state. A cached answer, a previous disclosure, or possession of
/// an `artifact_id` is never a substitute (§6.2, §6.4).
pub trait RelayAuthorizationView {
    /// The scope epoch the persistent view currently reports for a lane.
    fn current_scope_epoch(&self, lane: &LaneKey) -> Option<u64>;

    /// The key identifier currently in force for a lane.
    fn current_key_id(&self, lane: &LaneKey) -> Option<Identifier32>;

    /// Whether the peer holds a currently valid `seed` capability for the lane and epoch.
    fn has_seed_capability(&self, peer: &PeerIdentity, lane: &LaneKey, scope_epoch: u64) -> bool;

    /// Whether the peer holds a currently valid `fetch` capability for the lane and epoch.
    fn has_fetch_capability(&self, peer: &PeerIdentity, lane: &LaneKey, scope_epoch: u64) -> bool;

    /// Whether a Manager+ issuer may wrap the lane's current scope key for this peer (§9.3).
    ///
    /// `peer` is an Ed25519 **principal**, not a device key: this answers "may this principal
    /// receive a wrap at all", never "is this device enrolled to that principal". The
    /// recipient-device binding is `crypto::key_wrap::RecipientDeviceBinding`. The former
    /// parameter name `device` is what made SPEC-3 §10.2's device binding look already
    /// enforced here; it never was. See `src/AGENTS.md` §possession-is-never-authority.
    fn may_wrap_scope_key_for_peer(
        &self,
        peer: &PeerIdentity,
        lane: &LaneKey,
        scope_epoch: u64,
    ) -> bool;
}

/// Why a relay refused to seed or disclose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayRefusal {
    /// The persistent view knows no epoch for this lane.
    UnknownLane,
    /// The request named a superseded scope epoch (§6.4).
    StaleScopeEpoch {
        /// The epoch the requester used.
        requested: u64,
        /// The epoch the persistent view reports.
        current: u64,
    },
    /// The peer holds no current `seed` capability (§6.2).
    NoSeedCapability,
    /// The peer holds no current `fetch` capability (§6.2).
    NoFetchCapability,
    /// The artifact was sealed under a superseded key and is not served as current (§9.2).
    SupersededKey {
        /// The superseded key identifier.
        key_id: Identifier32,
    },
    /// The relay does not hold the artifact; an explicit unavailable result (§6.6).
    Unavailable,
}

/// The outcome of a seed request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedDecision {
    /// The relay may accept the seed commitment.
    Accept,
    /// The relay refuses; the reason is an explicit outcome, never a silent drop.
    Refuse(RelayRefusal),
}

/// The outcome of a fetch request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchDecision {
    /// The relay may disclose the opaque sealed bytes.
    Disclose,
    /// The relay refuses; the reason is an explicit outcome, never a silent drop.
    Refuse(RelayRefusal),
}

/// What storing bytes does and does not confer on a relay (§6.1, §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayAuthorities {
    seed: bool,
    fetch: bool,
}

impl RelayAuthorities {
    /// Whether the peer may commit new opaque bytes to this relay.
    pub const fn may_seed(&self) -> bool {
        self.seed
    }

    /// Whether the peer may have opaque bytes disclosed to it.
    pub const fn may_fetch(&self) -> bool {
        self.fetch
    }

    /// Always false: a relay is a ciphertext seeder and never gains decrypt authority.
    pub const fn may_decrypt(&self) -> bool {
        false
    }

    /// Always false: a relay is never an operation author.
    pub const fn may_append(&self) -> bool {
        false
    }

    /// Always false: a relay is never a branch or checkpoint authority.
    pub const fn may_authorize_checkpoint(&self) -> bool {
        false
    }
}

/// Resolves the current epoch and refuses a stale or unknown lane.
fn current_epoch(
    view: &impl RelayAuthorizationView,
    lane: &LaneKey,
    requested_epoch: u64,
) -> Result<u64, RelayRefusal> {
    let current = view
        .current_scope_epoch(lane)
        .ok_or(RelayRefusal::UnknownLane)?;
    if requested_epoch != current {
        return Err(RelayRefusal::StaleScopeEpoch {
            requested: requested_epoch,
            current,
        });
    }
    Ok(current)
}

/// The seed and fetch authorities the persistent view currently grants; they are independent.
pub fn relay_authorities(
    view: &impl RelayAuthorizationView,
    peer: &PeerIdentity,
    lane: &LaneKey,
    scope_epoch: u64,
) -> RelayAuthorities {
    RelayAuthorities {
        seed: view.has_seed_capability(peer, lane, scope_epoch),
        fetch: view.has_fetch_capability(peer, lane, scope_epoch),
    }
}

/// Decides whether a relay may accept a seed commitment (§6.2).
pub fn decide_seed(
    view: &impl RelayAuthorizationView,
    peer: &PeerIdentity,
    lane: &LaneKey,
    requested_epoch: u64,
) -> SeedDecision {
    let epoch = match current_epoch(view, lane, requested_epoch) {
        Ok(epoch) => epoch,
        Err(refusal) => return SeedDecision::Refuse(refusal),
    };
    if relay_authorities(view, peer, lane, epoch).may_seed() {
        SeedDecision::Accept
    } else {
        SeedDecision::Refuse(RelayRefusal::NoSeedCapability)
    }
}

/// Decides whether a relay may disclose an artifact (§6.2, §6.6, §9.2).
///
/// Presence is checked last and reported explicitly: a cache hit is not authority, and a
/// refusal to serve is a normal availability outcome rather than a silent drop.
pub fn decide_fetch(
    view: &impl RelayAuthorizationView,
    peer: &PeerIdentity,
    lane: &LaneKey,
    requested_epoch: u64,
    artifact_key_id: Identifier32,
    artifact_present: bool,
) -> FetchDecision {
    let epoch = match current_epoch(view, lane, requested_epoch) {
        Ok(epoch) => epoch,
        Err(refusal) => return FetchDecision::Refuse(refusal),
    };
    if !relay_authorities(view, peer, lane, epoch).may_fetch() {
        return FetchDecision::Refuse(RelayRefusal::NoFetchCapability);
    }
    match view.current_key_id(lane) {
        Some(current) if current == artifact_key_id => {}
        Some(_) => {
            return FetchDecision::Refuse(RelayRefusal::SupersededKey {
                key_id: artifact_key_id,
            })
        }
        None => return FetchDecision::Refuse(RelayRefusal::UnknownLane),
    }
    if artifact_present {
        FetchDecision::Disclose
    } else {
        FetchDecision::Refuse(RelayRefusal::Unavailable)
    }
}

/// Refuses to seal under anything but the lane's current epoch key (§9.1, §9.2).
pub fn assert_seals_under_current_key(
    view: &impl RelayAuthorizationView,
    lane: &LaneKey,
    scope_epoch: u64,
    descriptor: &EncryptionDescriptor,
) -> Result<(), SegmentError> {
    descriptor.assert_production_suite()?;
    let current = view
        .current_scope_epoch(lane)
        .ok_or(SegmentError::UnknownLane)?;
    if scope_epoch != current {
        return Err(SegmentError::StaleScopeEpoch {
            requested: scope_epoch,
            current,
        });
    }
    let current_key = view.current_key_id(lane).ok_or(SegmentError::UnknownLane)?;
    descriptor.assert_current_key(current_key)
}

/// Refuses a wrap of a scope key that is not the lane's current authorized key (§9.3).
///
/// Epoch and principal only. Device enrolment is not checked here and never was.
pub fn authorize_scope_key_wrap(
    view: &impl RelayAuthorizationView,
    peer: &PeerIdentity,
    lane: &LaneKey,
    requested_epoch: u64,
) -> Result<Identifier32, SegmentError> {
    let current = view
        .current_scope_epoch(lane)
        .ok_or(SegmentError::UnknownLane)?;
    if requested_epoch != current {
        return Err(SegmentError::StaleScopeEpoch {
            requested: requested_epoch,
            current,
        });
    }
    if !view.may_wrap_scope_key_for_peer(peer, lane, current) {
        return Err(SegmentError::Unauthorized);
    }
    view.current_key_id(lane).ok_or(SegmentError::UnknownLane)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::capability::topic::{derive_topic_name, TopicLabel, TopicLane};
    use crate::capability::verbs::ObjectClass;
    use crate::envelope::{NONCE_LENGTH, PRODUCTION_SUITE_ID};
    use crate::segment::discovery_labels::{
        authorize_lane_subscription, BlindedTopic, BlindedTopicDerivation,
    };
    use crate::segment::test_fixtures::identifier;

    fn header_lane() -> LaneKey {
        LaneKey::Header {
            verse_id: identifier(0x11),
        }
    }

    struct View {
        epoch: u64,
        key_id: Identifier32,
        seeders: BTreeSet<PeerIdentity>,
        fetchers: BTreeSet<PeerIdentity>,
        wrappable_peers: BTreeSet<PeerIdentity>,
    }

    impl View {
        fn at_epoch(epoch: u64, key_id: Identifier32) -> Self {
            Self {
                epoch,
                key_id,
                seeders: BTreeSet::new(),
                fetchers: BTreeSet::new(),
                wrappable_peers: BTreeSet::new(),
            }
        }
    }

    impl RelayAuthorizationView for View {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(self.epoch)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(self.key_id)
        }

        fn has_seed_capability(
            &self,
            peer: &PeerIdentity,
            _lane: &LaneKey,
            scope_epoch: u64,
        ) -> bool {
            scope_epoch == self.epoch && self.seeders.contains(peer)
        }

        fn has_fetch_capability(
            &self,
            peer: &PeerIdentity,
            _lane: &LaneKey,
            scope_epoch: u64,
        ) -> bool {
            scope_epoch == self.epoch && self.fetchers.contains(peer)
        }

        fn may_wrap_scope_key_for_peer(
            &self,
            peer: &PeerIdentity,
            _lane: &LaneKey,
            scope_epoch: u64,
        ) -> bool {
            scope_epoch == self.epoch && self.wrappable_peers.contains(peer)
        }
    }

    struct KeyedTopics;

    impl BlindedTopicDerivation for KeyedTopics {
        fn blinded_topic(&self, label: &TopicLabel) -> Result<String, SegmentError> {
            Ok(derive_topic_name(&[0x5a; 32], label)?)
        }
    }

    fn subscribe(
        view: &impl RelayAuthorizationView,
        peer: &PeerIdentity,
        lane: &LaneKey,
        requested_epoch: u64,
    ) -> Result<BlindedTopic, SegmentError> {
        authorize_lane_subscription(
            view,
            peer,
            lane,
            TopicLane::Header,
            ObjectClass::Operation,
            requested_epoch,
            &KeyedTopics,
        )
    }

    #[test]
    fn relay_seed_and_fetch_capabilities_are_independent() {
        let lane = header_lane();
        let key = identifier(0x41);
        let seeder = PeerIdentity([1; 32]);
        let fetcher = PeerIdentity([2; 32]);
        let bystander = PeerIdentity([3; 32]);

        let mut view = View::at_epoch(7, key);
        view.seeders.insert(seeder);
        view.fetchers.insert(fetcher);

        assert_eq!(decide_seed(&view, &seeder, &lane, 7), SeedDecision::Accept);
        assert_eq!(
            decide_fetch(&view, &seeder, &lane, 7, key, true),
            FetchDecision::Refuse(RelayRefusal::NoFetchCapability)
        );
        assert_eq!(
            decide_fetch(&view, &fetcher, &lane, 7, key, true),
            FetchDecision::Disclose
        );
        assert_eq!(
            decide_seed(&view, &fetcher, &lane, 7),
            SeedDecision::Refuse(RelayRefusal::NoSeedCapability)
        );
        assert_eq!(
            decide_seed(&view, &bystander, &lane, 7),
            SeedDecision::Refuse(RelayRefusal::NoSeedCapability)
        );
        assert_eq!(
            decide_fetch(&view, &bystander, &lane, 7, key, true),
            FetchDecision::Refuse(RelayRefusal::NoFetchCapability)
        );
        assert_eq!(
            decide_fetch(&view, &fetcher, &lane, 7, key, false),
            FetchDecision::Refuse(RelayRefusal::Unavailable)
        );

        for peer in [seeder, fetcher, bystander] {
            let authorities = relay_authorities(&view, &peer, &lane, 7);
            assert!(!authorities.may_decrypt());
            assert!(!authorities.may_append());
            assert!(!authorities.may_authorize_checkpoint());
        }
        assert!(relay_authorities(&view, &seeder, &lane, 7).may_seed());
        assert!(!relay_authorities(&view, &seeder, &lane, 7).may_fetch());
        assert!(relay_authorities(&view, &fetcher, &lane, 7).may_fetch());
    }

    #[test]
    fn scope_epoch_bump_stops_old_lane_service() {
        let lane = header_lane();
        let old_key = identifier(0x41);
        let peer = PeerIdentity([1; 32]);

        let mut before = View::at_epoch(7, old_key);
        before.seeders.insert(peer);
        before.fetchers.insert(peer);
        assert_eq!(decide_seed(&before, &peer, &lane, 7), SeedDecision::Accept);
        assert_eq!(
            decide_fetch(&before, &peer, &lane, 7, old_key, true),
            FetchDecision::Disclose
        );
        let old_topic = subscribe(&before, &peer, &lane, 7).expect("subscribed");

        let mut after = View::at_epoch(8, identifier(0x42));
        after.seeders.insert(peer);
        after.fetchers.insert(peer);

        let stale = RelayRefusal::StaleScopeEpoch {
            requested: 7,
            current: 8,
        };
        assert_eq!(
            decide_seed(&after, &peer, &lane, 7),
            SeedDecision::Refuse(stale)
        );
        assert_eq!(
            decide_fetch(&after, &peer, &lane, 7, old_key, true),
            FetchDecision::Refuse(stale)
        );
        assert_eq!(
            decide_fetch(&after, &peer, &lane, 8, old_key, true),
            FetchDecision::Refuse(RelayRefusal::SupersededKey { key_id: old_key })
        );
        assert_eq!(
            subscribe(&after, &peer, &lane, 7),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            })
        );

        let rotated = subscribe(&after, &peer, &lane, 8).expect("rotated");
        assert_ne!(rotated, old_topic);
    }

    #[test]
    fn key_wrap_rotation_blocks_old_epoch_segment_service() {
        let lane = header_lane();
        let old_key = identifier(0x41);
        let new_key = identifier(0x42);
        let device = PeerIdentity([1; 32]);
        let removed_device = PeerIdentity([2; 32]);

        let mut before = View::at_epoch(7, old_key);
        before.fetchers.insert(device);
        before.wrappable_peers.insert(device);
        before.wrappable_peers.insert(removed_device);
        let old_descriptor =
            EncryptionDescriptor::new(PRODUCTION_SUITE_ID, old_key, [3; NONCE_LENGTH]);
        assert_eq!(
            assert_seals_under_current_key(&before, &lane, 7, &old_descriptor),
            Ok(())
        );
        assert_eq!(
            authorize_scope_key_wrap(&before, &removed_device, &lane, 7),
            Ok(old_key)
        );

        let mut after = View::at_epoch(8, new_key);
        after.fetchers.insert(device);
        after.wrappable_peers.insert(device);

        assert_eq!(
            assert_seals_under_current_key(&after, &lane, 8, &old_descriptor),
            Err(SegmentError::StaleScopeKey { key_id: old_key })
        );
        assert_eq!(
            assert_seals_under_current_key(&after, &lane, 7, &old_descriptor),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            })
        );
        let new_descriptor =
            EncryptionDescriptor::new(PRODUCTION_SUITE_ID, new_key, [4; NONCE_LENGTH]);
        assert_eq!(
            assert_seals_under_current_key(&after, &lane, 8, &new_descriptor),
            Ok(())
        );

        assert_eq!(
            decide_fetch(&after, &device, &lane, 8, old_key, true),
            FetchDecision::Refuse(RelayRefusal::SupersededKey { key_id: old_key })
        );
        assert_eq!(
            decide_fetch(&after, &device, &lane, 8, new_key, true),
            FetchDecision::Disclose
        );

        assert_eq!(
            authorize_scope_key_wrap(&after, &device, &lane, 8),
            Ok(new_key)
        );
        assert_eq!(
            authorize_scope_key_wrap(&after, &removed_device, &lane, 8),
            Err(SegmentError::Unauthorized)
        );
        assert_eq!(
            authorize_scope_key_wrap(&after, &device, &lane, 7),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            })
        );
    }
}
