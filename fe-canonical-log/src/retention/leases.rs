//! Storage roles, GC leases, and GC eligibility (SPEC-5 §5.1-§5.3); see
//! `src/retention/AGENTS.md` §leases.

use std::collections::BTreeMap;

use thiserror::Error;

use crate::envelope::{Author, Hash32, Hlc, Identifier32, Scope};

/// Who holds an artifact copy and what obligation, if any, that role carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StorageRole {
    /// A mobile client's local cache. No durability obligation: may drop artifacts freely once
    /// its own working set no longer needs them, subject to [`GcLeaseRegistry::gc_eligibility`].
    MobilePeer,
    /// A relay that has accepted a [`GcLeaseDescriptor`] and advertised itself as a seeder for
    /// the leased artifact set. Obligated to hold that set until the lease expires or is
    /// released; see [`GcLeaseRegistry::can_advertise`].
    RelaySeeder,
    /// A long-term archive. Obligated to retain the verse's full history subject to legal hold
    /// and crypto-shred (key destruction never implies byte deletion here).
    Archive,
}

/// A caller-supplied reason an artifact must not be garbage-collected regardless of lease state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegalHold {
    /// Free-form justification, retained for audit.
    pub reason: String,
}

/// A GC lease: a durable commitment that `holder` will retain the artifact set committed to by
/// `artifact_set_commitment` until `expires`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GcLeaseDescriptor {
    /// Identifier for this lease.
    pub lease_id: Hash32,
    /// Principal holding the lease (the seeder or archive).
    pub holder: Author,
    /// Principal that authorized issuing this lease.
    pub authorized_issuer: Author,
    /// Verse the leased artifacts belong to.
    pub verse_id: Identifier32,
    /// Authorization scope the lease covers.
    pub scope: Scope,
    /// Commitment binding the exact artifact set this lease covers.
    pub artifact_set_commitment: Hash32,
    /// When the lease was issued.
    pub issued: Hlc,
    /// When the lease expires absent a renewal.
    pub expires: Hlc,
    /// An optional legal hold pinned to this lease's artifact set.
    pub legal_hold: Option<LegalHold>,
}

/// The GC-lease acknowledgement state machine (SPEC-5 §5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// Accepted by [`GcLeaseRegistry::accept`] but not yet durably acknowledged.
    PendingAcknowledgement,
    /// Durably acknowledged by the holder; may be advertised as a seed source.
    Acknowledged,
    /// Renewed past its original expiry; may be advertised as a seed source.
    Renewed,
    /// Acknowledgement failed to reach durable storage; must not be advertised.
    FailedDurable,
    /// The lease's term has elapsed.
    Expired,
}

impl LeaseState {
    /// Whether the holder is still under a retention obligation in this state.
    ///
    /// `PendingAcknowledgement` counts: `accept` already committed the holder, and treating an
    /// un-acknowledged lease as collectable would let a GC pass race the acknowledgement it is
    /// waiting for. `FailedDurable` and `Expired` never carry an obligation.
    pub const fn holds_retention_obligation(self) -> bool {
        matches!(
            self,
            Self::PendingAcknowledgement | Self::Acknowledged | Self::Renewed
        )
    }
}

/// Resolves whether a lease's committed artifact set contains a given artifact.
///
/// `GcLeaseDescriptor::artifact_set_commitment` is an opaque commitment: only the segment
/// slice's manifest/HashSeq machinery can open it. It therefore arrives as a REQUIRED parameter
/// of [`GcLeaseRegistry::gc_eligibility`] rather than as a pre-filter the gate cannot see, so a
/// caller cannot satisfy the lease check by handing over an empty slice.
pub trait ArtifactSetMembership {
    /// Whether `artifact_set_commitment` covers `artifact_id`.
    fn set_contains(&self, artifact_set_commitment: Hash32, artifact_id: Hash32) -> bool;
}

/// Every reason a lease transition or query fails.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum LeaseError {
    /// `SeedAuthority` refused to authorize this holder for this scope.
    #[error("seed authority refused holder for the requested scope")]
    Unauthorized,
    /// No lease exists under the given ID.
    #[error("no lease registered under this ID")]
    UnknownLease,
    /// The requested transition is not valid from the lease's current state.
    #[error("invalid lease state transition")]
    InvalidTransition,
    /// The lease is not in a state that may be advertised for fetch.
    #[error("lease is not acknowledged or renewed")]
    NotAdvertisable,
    /// `FetchAuthority` refused this requester for this lease's scope.
    #[error("fetch authority refused the requester for this lease's scope")]
    FetchUnauthorized,
}

/// Authorizes accepting a lease for a given holder, verse, and scope. Implemented by a caller
/// with access to the capability/role state this crate does not hold.
pub trait SeedAuthority {
    /// Reports whether `holder` may hold a seed lease for `scope` within `verse_id`.
    fn authorizes_seed(&self, holder: &Author, verse_id: Identifier32, scope: &Scope) -> bool;
}

/// Authorizes fetching (byte transfer only, never decrypt or append) under a lease.
pub trait FetchAuthority {
    /// Reports whether `requester` may fetch artifacts within `scope`.
    fn authorizes_fetch(&self, requester: &Author, scope: &Scope) -> bool;
}

struct LeaseEntry {
    descriptor: GcLeaseDescriptor,
    state: LeaseState,
}

/// In-memory GC-lease registry driving the §5.2 acknowledgement state machine.
#[derive(Default)]
pub struct GcLeaseRegistry {
    leases: BTreeMap<Hash32, LeaseEntry>,
}

impl GcLeaseRegistry {
    /// Builds an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts `descriptor` into `PendingAcknowledgement`, requiring `authority` to authorize
    /// the holder for the descriptor's verse and scope first.
    pub fn accept(
        &mut self,
        descriptor: GcLeaseDescriptor,
        authority: &impl SeedAuthority,
    ) -> Result<(), LeaseError> {
        if !authority.authorizes_seed(&descriptor.holder, descriptor.verse_id, &descriptor.scope) {
            return Err(LeaseError::Unauthorized);
        }
        self.leases.insert(
            descriptor.lease_id,
            LeaseEntry {
                descriptor,
                state: LeaseState::PendingAcknowledgement,
            },
        );
        Ok(())
    }

    /// Moves a `PendingAcknowledgement` lease to `Acknowledged`.
    pub fn acknowledge(&mut self, lease_id: &Hash32) -> Result<(), LeaseError> {
        let entry = self
            .leases
            .get_mut(lease_id)
            .ok_or(LeaseError::UnknownLease)?;
        if entry.state != LeaseState::PendingAcknowledgement {
            return Err(LeaseError::InvalidTransition);
        }
        entry.state = LeaseState::Acknowledged;
        Ok(())
    }

    /// Moves an `Acknowledged` or already-`Renewed` lease to `Renewed` with a new expiry.
    pub fn renew(&mut self, lease_id: &Hash32, new_expires: Hlc) -> Result<(), LeaseError> {
        let entry = self
            .leases
            .get_mut(lease_id)
            .ok_or(LeaseError::UnknownLease)?;
        if !matches!(entry.state, LeaseState::Acknowledged | LeaseState::Renewed) {
            return Err(LeaseError::InvalidTransition);
        }
        entry.descriptor.expires = new_expires;
        entry.state = LeaseState::Renewed;
        Ok(())
    }

    /// Records that durable acknowledgement failed; the lease may never advertise.
    pub fn mark_failed_durable(&mut self, lease_id: &Hash32) -> Result<(), LeaseError> {
        let entry = self
            .leases
            .get_mut(lease_id)
            .ok_or(LeaseError::UnknownLease)?;
        entry.state = LeaseState::FailedDurable;
        Ok(())
    }

    /// Marks a lease expired.
    pub fn expire(&mut self, lease_id: &Hash32) -> Result<(), LeaseError> {
        let entry = self
            .leases
            .get_mut(lease_id)
            .ok_or(LeaseError::UnknownLease)?;
        entry.state = LeaseState::Expired;
        Ok(())
    }

    /// The lease's current state, if it exists.
    pub fn state_of(&self, lease_id: &Hash32) -> Option<LeaseState> {
        self.leases.get(lease_id).map(|entry| entry.state)
    }

    /// Reports whether this lease may currently be advertised as a seed source. `false` until
    /// `acknowledge` (or a subsequent `renew`) has run; never true while pending, failed, or
    /// expired.
    pub fn can_advertise(&self, lease_id: &Hash32) -> bool {
        matches!(
            self.leases.get(lease_id).map(|entry| entry.state),
            Some(LeaseState::Acknowledged) | Some(LeaseState::Renewed)
        )
    }

    /// Authorizes a fetch (byte transfer) under an advertisable lease. Never returns a decrypt
    /// key or an append permission: success here means "this relay may hand over the sealed
    /// bytes," nothing more.
    pub fn authorize_fetch(
        &self,
        lease_id: &Hash32,
        requester: &Author,
        authority: &impl FetchAuthority,
    ) -> Result<(), LeaseError> {
        let entry = self.leases.get(lease_id).ok_or(LeaseError::UnknownLease)?;
        if !self.can_advertise(lease_id) {
            return Err(LeaseError::NotAdvertisable);
        }
        if !authority.authorizes_fetch(requester, &entry.descriptor.scope) {
            return Err(LeaseError::FetchUnauthorized);
        }
        Ok(())
    }

    /// The SPEC-5 §5.3 rule 3 GC-eligibility gate for one artifact.
    ///
    /// Resolves lease coverage itself: it walks its own leases, keeps only those whose committed
    /// artifact set contains `artifact_id`, and among those counts only leases that both hold a
    /// retention obligation in their current [`LeaseState`] and have not expired against `now`.
    /// A legal hold blocks regardless of state or expiry — a hold outlives the lease that
    /// carried it. `now` is always the caller's parameter; this crate never reads a clock.
    ///
    /// Blocking priority: legal hold, then active lease, then branch replay, then tombstone.
    pub fn gc_eligibility(
        &self,
        artifact_id: Hash32,
        now: Hlc,
        membership: &impl ArtifactSetMembership,
        branch_replay_requirements: bool,
        tombstone_requirements: bool,
    ) -> GcEligibility {
        let covering = self.leases.values().filter(|entry| {
            membership.set_contains(entry.descriptor.artifact_set_commitment, artifact_id)
        });
        let mut held = false;
        let mut leased = false;
        for entry in covering {
            held |= entry.descriptor.legal_hold.is_some();
            leased |= entry.state.holds_retention_obligation() && now < entry.descriptor.expires;
        }
        if held {
            return GcEligibility::BlockedByLegalHold;
        }
        if leased {
            return GcEligibility::BlockedByLease;
        }
        if branch_replay_requirements {
            return GcEligibility::BlockedByBranchReplay;
        }
        if tombstone_requirements {
            return GcEligibility::BlockedByTombstoneRetention;
        }
        GcEligibility::Eligible
    }
}

/// The outcome of a pure GC-eligibility check for one artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcEligibility {
    /// Nothing blocks garbage-collecting this artifact.
    Eligible,
    /// An active GC lease still covers this artifact.
    BlockedByLease,
    /// A legal hold pins this artifact.
    BlockedByLegalHold,
    /// A branch replay still needs this artifact.
    BlockedByBranchReplay,
    /// Tombstone non-resurrection retention still needs this artifact.
    BlockedByTombstoneRetention,
}

/// Evidence a mobile peer must present before releasing (dropping) its copy of a selection.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MobileRetentionEvidence {
    /// A verified checkpoint ID covering the declared selection, if one exists.
    pub eligible_checkpoint: Option<Hash32>,
    /// An authorized tail commitment covering the declared selection, if one exists.
    pub authorized_tail: Option<Hash32>,
}

/// Every reason [`mobile_release_precondition`] refuses release.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum MobileReleaseError {
    /// Neither a verified checkpoint nor an authorized tail covers the declared selection.
    #[error("mobile release requires a verified checkpoint or an authorized tail")]
    NoVerifiedBootstrapPath,
}

/// Refuses mobile release unless `evidence` carries an eligible checkpoint or an authorized
/// tail for the declared selection (SPEC-5 §5.3).
pub fn mobile_release_precondition(
    evidence: &MobileRetentionEvidence,
) -> Result<(), MobileReleaseError> {
    if evidence.eligible_checkpoint.is_some() || evidence.authorized_tail.is_some() {
        Ok(())
    } else {
        Err(MobileReleaseError::NoVerifiedBootstrapPath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn author(filler: u8) -> Author {
        Author::from_public_key([filler; 32])
    }

    fn descriptor() -> GcLeaseDescriptor {
        GcLeaseDescriptor {
            lease_id: hash(1),
            holder: author(2),
            authorized_issuer: author(3),
            verse_id: identifier(4),
            scope: Scope::verse_wide(identifier(4)),
            artifact_set_commitment: hash(5),
            issued: Hlc::new(0, 0),
            expires: Hlc::new(1_000, 0),
            legal_hold: None,
        }
    }

    struct AllowAll;
    impl SeedAuthority for AllowAll {
        fn authorizes_seed(
            &self,
            _holder: &Author,
            _verse_id: Identifier32,
            _scope: &Scope,
        ) -> bool {
            true
        }
    }
    impl FetchAuthority for AllowAll {
        fn authorizes_fetch(&self, _requester: &Author, _scope: &Scope) -> bool {
            true
        }
    }

    struct DenyAll;
    impl SeedAuthority for DenyAll {
        fn authorizes_seed(
            &self,
            _holder: &Author,
            _verse_id: Identifier32,
            _scope: &Scope,
        ) -> bool {
            false
        }
    }

    struct AllowSeedDenyFetch;
    impl SeedAuthority for AllowSeedDenyFetch {
        fn authorizes_seed(
            &self,
            _holder: &Author,
            _verse_id: Identifier32,
            _scope: &Scope,
        ) -> bool {
            true
        }
    }
    impl FetchAuthority for AllowSeedDenyFetch {
        fn authorizes_fetch(&self, _requester: &Author, _scope: &Scope) -> bool {
            false
        }
    }

    #[test]
    fn lease_requires_seed_authority_and_durable_acknowledgement() {
        let mut registry = GcLeaseRegistry::new();
        let descriptor = descriptor();

        assert_eq!(
            registry.accept(descriptor.clone(), &DenyAll),
            Err(LeaseError::Unauthorized)
        );
        assert_eq!(registry.state_of(&descriptor.lease_id), None);
        assert!(!registry.can_advertise(&descriptor.lease_id));

        registry
            .accept(descriptor.clone(), &AllowAll)
            .expect("seed authority allows");
        assert_eq!(
            registry.state_of(&descriptor.lease_id),
            Some(LeaseState::PendingAcknowledgement)
        );
        // Accepted but not yet durably acknowledged: must not be advertised.
        assert!(!registry.can_advertise(&descriptor.lease_id));
        assert_eq!(
            registry.authorize_fetch(&descriptor.lease_id, &author(9), &AllowAll),
            Err(LeaseError::NotAdvertisable)
        );

        registry
            .acknowledge(&descriptor.lease_id)
            .expect("acknowledge");
        assert!(registry.can_advertise(&descriptor.lease_id));
        assert!(registry
            .authorize_fetch(&descriptor.lease_id, &author(9), &AllowAll)
            .is_ok());

        // A second acknowledge from a non-pending state is an invalid transition.
        assert_eq!(
            registry.acknowledge(&descriptor.lease_id),
            Err(LeaseError::InvalidTransition)
        );

        // Acknowledged and advertisable, but an unauthorized fetch requester is still refused.
        assert_eq!(
            registry.authorize_fetch(&descriptor.lease_id, &author(9), &AllowSeedDenyFetch),
            Err(LeaseError::FetchUnauthorized)
        );

        // Seeding never becomes append or decrypt authority: `authorize_fetch`'s only success
        // value is `()`, so a successful fetch grant structurally cannot carry a decrypt key or
        // an append permission out of this function.
        let fetch_grant: Result<(), LeaseError> =
            registry.authorize_fetch(&descriptor.lease_id, &author(9), &AllowAll);
        assert_eq!(fetch_grant, Ok(()));
    }

    #[test]
    fn failed_durable_acknowledgement_never_becomes_advertisable() {
        let mut registry = GcLeaseRegistry::new();
        let descriptor = descriptor();
        registry
            .accept(descriptor.clone(), &AllowAll)
            .expect("accept");
        registry
            .mark_failed_durable(&descriptor.lease_id)
            .expect("mark failed");
        assert!(!registry.can_advertise(&descriptor.lease_id));
        assert_eq!(
            registry.authorize_fetch(&descriptor.lease_id, &author(9), &AllowAll),
            Err(LeaseError::NotAdvertisable)
        );
    }

    /// The artifact under test belongs to every lease's committed set.
    struct EverySetCovers;
    impl ArtifactSetMembership for EverySetCovers {
        fn set_contains(&self, _commitment: Hash32, _artifact_id: Hash32) -> bool {
            true
        }
    }

    /// No lease's committed set contains the artifact under test.
    struct NoSetCovers;
    impl ArtifactSetMembership for NoSetCovers {
        fn set_contains(&self, _commitment: Hash32, _artifact_id: Hash32) -> bool {
            false
        }
    }

    fn accepted(state_transition: fn(&mut GcLeaseRegistry, &Hash32)) -> GcLeaseRegistry {
        let mut registry = GcLeaseRegistry::new();
        let descriptor = descriptor();
        let lease_id = descriptor.lease_id;
        registry.accept(descriptor, &AllowAll).expect("accept");
        state_transition(&mut registry, &lease_id);
        registry
    }

    fn acknowledged_lease(registry: &mut GcLeaseRegistry, lease_id: &Hash32) {
        registry.acknowledge(lease_id).expect("acknowledge");
    }

    #[test]
    fn gc_eligibility_blocks_in_priority_order_legal_hold_lease_replay_tombstone() {
        let before_expiry = Hlc::new(500, 0);
        let acknowledged = accepted(acknowledged_lease);

        let mut with_hold = GcLeaseRegistry::new();
        let mut held = descriptor();
        held.legal_hold = Some(LegalHold {
            reason: "audit".to_owned(),
        });
        with_hold.accept(held, &AllowAll).expect("accept");
        assert_eq!(
            with_hold.gc_eligibility(hash(1), before_expiry, &EverySetCovers, true, true),
            GcEligibility::BlockedByLegalHold
        );

        assert_eq!(
            acknowledged.gc_eligibility(hash(1), before_expiry, &EverySetCovers, true, true),
            GcEligibility::BlockedByLease
        );
        assert_eq!(
            acknowledged.gc_eligibility(hash(1), before_expiry, &NoSetCovers, true, true),
            GcEligibility::BlockedByBranchReplay,
            "a lease whose committed set excludes the artifact never blocks it"
        );
        assert_eq!(
            GcLeaseRegistry::new().gc_eligibility(
                hash(1),
                before_expiry,
                &EverySetCovers,
                true,
                true
            ),
            GcEligibility::BlockedByBranchReplay
        );
        assert_eq!(
            GcLeaseRegistry::new().gc_eligibility(
                hash(1),
                before_expiry,
                &EverySetCovers,
                false,
                true
            ),
            GcEligibility::BlockedByTombstoneRetention
        );
        assert_eq!(
            GcLeaseRegistry::new().gc_eligibility(
                hash(1),
                before_expiry,
                &EverySetCovers,
                false,
                false
            ),
            GcEligibility::Eligible
        );
    }

    #[test]
    fn gc_eligibility_resolves_lease_state_and_expiry_rather_than_trusting_the_caller() {
        let before_expiry = Hlc::new(500, 0);
        let after_expiry = Hlc::new(1_001, 0);
        let acknowledged = accepted(acknowledged_lease);

        // Same registry, same artifact, same membership: only `now` moves past `expires`.
        assert_eq!(
            acknowledged.gc_eligibility(hash(1), before_expiry, &EverySetCovers, false, false),
            GcEligibility::BlockedByLease
        );
        assert_eq!(
            acknowledged.gc_eligibility(hash(1), after_expiry, &EverySetCovers, false, false),
            GcEligibility::Eligible,
            "an elapsed lease term stops blocking without any caller assertion"
        );

        // A pending lease is already a commitment; a failed or expired one is not.
        let pending = accepted(|_registry: &mut GcLeaseRegistry, _lease_id: &Hash32| {});
        assert_eq!(
            pending.gc_eligibility(hash(1), before_expiry, &EverySetCovers, false, false),
            GcEligibility::BlockedByLease
        );
        let failed = accepted(|registry: &mut GcLeaseRegistry, lease_id: &Hash32| {
            registry.mark_failed_durable(lease_id).expect("mark failed");
        });
        assert_eq!(
            failed.gc_eligibility(hash(1), before_expiry, &EverySetCovers, false, false),
            GcEligibility::Eligible
        );
        let expired = accepted(|registry: &mut GcLeaseRegistry, lease_id: &Hash32| {
            registry.expire(lease_id).expect("expire");
        });
        assert_eq!(
            expired.gc_eligibility(hash(1), before_expiry, &EverySetCovers, false, false),
            GcEligibility::Eligible
        );

        // A renewal past the old expiry blocks again at a `now` that had freed the artifact.
        let renewed = accepted(|registry: &mut GcLeaseRegistry, lease_id: &Hash32| {
            registry.acknowledge(lease_id).expect("acknowledge");
            registry.renew(lease_id, Hlc::new(2_000, 0)).expect("renew");
        });
        assert_eq!(
            renewed.gc_eligibility(hash(1), after_expiry, &EverySetCovers, false, false),
            GcEligibility::BlockedByLease
        );

        // A legal hold outlives both the lease term and a failed acknowledgement.
        let mut held_registry = GcLeaseRegistry::new();
        let mut held = descriptor();
        held.legal_hold = Some(LegalHold {
            reason: "audit".to_owned(),
        });
        let held_id = held.lease_id;
        held_registry.accept(held, &AllowAll).expect("accept");
        held_registry
            .mark_failed_durable(&held_id)
            .expect("mark failed");
        assert_eq!(
            held_registry.gc_eligibility(hash(1), after_expiry, &EverySetCovers, false, false),
            GcEligibility::BlockedByLegalHold
        );
    }

    #[test]
    fn mobile_release_requires_verified_bootstrap_path() {
        assert_eq!(
            mobile_release_precondition(&MobileRetentionEvidence::default()),
            Err(MobileReleaseError::NoVerifiedBootstrapPath)
        );
        assert!(mobile_release_precondition(&MobileRetentionEvidence {
            eligible_checkpoint: Some(hash(1)),
            authorized_tail: None,
        })
        .is_ok());
        assert!(mobile_release_precondition(&MobileRetentionEvidence {
            eligible_checkpoint: None,
            authorized_tail: Some(hash(2)),
        })
        .is_ok());
    }
}
