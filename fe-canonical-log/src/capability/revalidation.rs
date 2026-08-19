//! Authorization-view cache keys, pinned sessions, and epoch-bump invalidation (§5.3).
//!
//! Everything here is pure and process-local; Wave 3 wires it into the API, relay, and
//! WebSocket paths. See `src/capability/AGENTS.md` §revalidation for the wiring obligation.

use std::collections::HashSet;

use crate::envelope::Scope;

use super::certificate::CertificateId;
use super::chain::ChainId;
use super::Principal;

/// The §5.3 rule 1 cache key: chain, epoch scope, epoch, expiry, and authority-view version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey {
    /// Identifier of the chain artifact the decision was made against.
    pub chain_id: ChainId,
    /// Scope whose epoch revokes the chain.
    pub epoch_scope: Scope,
    /// Epoch the decision relied on.
    pub epoch: u64,
    /// Effective expiry of the chain in Unix milliseconds.
    pub expiry_ms: u64,
    /// Monotonic version of the persistent authorization view the decision read.
    pub authority_view_version: u64,
}

/// The answer a pinned session gives when it re-checks itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionValidity {
    /// The pinned epoch still matches the view and the chain has not expired.
    Valid,
    /// The epoch moved or the view no longer knows the scope; a replacement chain is required.
    ReauthorizationRequired,
    /// The chain's effective expiry has passed.
    Expired,
}

/// The persistent, verified projection of identity, authority, and epoch state (§1.6).
///
/// Wave 3 implements this over durable storage; this crate only ever reads through it.
///
/// `Send + Sync` matches every sibling trait a transport holds behind an `Arc`
/// (`CanonicalCommitPipeline`, `BranchRegistry`, `CapabilityVerifier`, `ScopeSnapshotSource`):
/// a §5.3 revalidation timer reads this view from a task other than the one that pinned the
/// session, so the bound belongs here rather than at each `dyn` site a future author can forget.
pub trait AuthorizationView: Send + Sync {
    /// The epoch the view currently records for `epoch_scope`, or `None` if it knows none.
    fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64>;

    /// A monotonically increasing version that changes whenever the view changes.
    fn version(&self) -> u64;
}

/// What a WebSocket session pins at handshake time (§5.3 rule 3).
///
/// **It records identity and revocation state, not the grant.** There is no `effective_scope`,
/// no `effective_verbs`, no `effective_object_classes`, and no `effective_caveats` here, so a
/// holder of a `Valid` session cannot tell whether it may append, only fetch, or merely seed.
/// Deciding an action still requires verifying the chain for that specific
/// `chain::AuthorizationRequest`; see `src/capability/AGENTS.md`
/// §possession-is-never-authority for why the fields are absent and what it would take to add
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedSession {
    /// The verified leaf principal.
    pub leaf_principal: Principal,
    /// The verified chain artifact.
    pub chain_id: ChainId,
    /// The verified leaf certificate.
    pub leaf_certificate_id: CertificateId,
    /// Scope whose epoch revokes the session.
    pub epoch_scope: Scope,
    /// Epoch pinned at handshake.
    pub epoch: u64,
    /// Effective expiry of the chain in Unix milliseconds.
    pub expires_at_ms: u64,
    /// Scopes the session subscribed to.
    pub subscribed_scopes: Vec<Scope>,
}

impl PinnedSession {
    /// The §5.3 rule 1 cache key for this session at a **caller-supplied** view version.
    ///
    /// Hazardous by construction: the caller chooses the one dimension the gate exists to
    /// invalidate on, so a stale or cached version number produces a key that looks admitted.
    /// Use [`Self::decision`] with [`RevalidationGate::admitted_now`], which reads the version
    /// off the live view instead. This door survives for tests and for encoding a key whose
    /// version came from a `view.version()` call the caller can point at.
    pub fn cache_key(&self, authority_view_version: u64) -> CacheKey {
        CacheKey {
            chain_id: self.chain_id,
            epoch_scope: self.epoch_scope,
            epoch: self.epoch,
            expiry_ms: self.expires_at_ms,
            authority_view_version,
        }
    }

    /// The four view-independent dimensions of this session's authorization decision.
    pub fn decision(&self) -> AdmittedDecision {
        AdmittedDecision {
            chain_id: self.chain_id,
            epoch_scope: self.epoch_scope,
            epoch: self.epoch,
            expiry_ms: self.expires_at_ms,
        }
    }

    /// Re-checks expiry first, then the durable epoch; §5.3 rule 4 calls this on a timer.
    pub fn is_still_valid(&self, now_ms: u64, view: &dyn AuthorizationView) -> SessionValidity {
        if now_ms >= self.expires_at_ms {
            return SessionValidity::Expired;
        }
        match view.current_epoch(&self.epoch_scope) {
            Some(epoch) if epoch == self.epoch => SessionValidity::Valid,
            _ => SessionValidity::ReauthorizationRequired,
        }
    }

    /// Reports whether `scope` is inside a scope this session subscribed to (§5.3 rule 3).
    ///
    /// A necessary condition, never a sufficient one: it says nothing about verbs, object
    /// classes, or caveats, because [`PinnedSession`] does not carry them. `covers` plus
    /// [`Self::is_still_valid`] is not an authorization decision; see
    /// `src/capability/AGENTS.md` §possession-is-never-authority.
    pub fn covers(&self, scope: &Scope) -> bool {
        self.subscribed_scopes
            .iter()
            .any(|subscribed| subscribed.contains(scope))
    }
}

/// The four dimensions of an authorization decision that do NOT come from the live view.
///
/// The fifth, `authority_view_version`, is deliberately absent: it is the gate's job to read it
/// off the view at the moment of the question, not the caller's job to remember it. Pairing this
/// with [`RevalidationGate::admitted_now`] is what makes "read durable epoch state before any
/// cache-based allow" a property of the code rather than an instruction in a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AdmittedDecision {
    /// Identifier of the chain artifact the decision was made against.
    pub chain_id: ChainId,
    /// Scope whose epoch revokes the chain.
    pub epoch_scope: Scope,
    /// Epoch the decision relied on.
    pub epoch: u64,
    /// Effective expiry of the chain in Unix milliseconds.
    pub expiry_ms: u64,
}

impl AdmittedDecision {
    /// The §5.3 rule 1 cache key for this decision at `authority_view_version`.
    pub fn cache_key(&self, authority_view_version: u64) -> CacheKey {
        CacheKey {
            chain_id: self.chain_id,
            epoch_scope: self.epoch_scope,
            epoch: self.epoch,
            expiry_ms: self.expiry_ms,
            authority_view_version,
        }
    }
}

/// Why a cached authorization decision may not be reused. Never an allow, in any variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheRefusal {
    /// The chain's effective expiry has passed.
    Expired,
    /// The view records no epoch for the decision's epoch scope; an unanswered question denies.
    EpochScopeUnknown,
    /// The view's epoch no longer matches the epoch the decision relied on.
    EpochMoved {
        /// Epoch the durable view now reports.
        current_epoch: u64,
    },
    /// The authorization view changed version since the decision was recorded, so the decision
    /// was made against facts that no longer hold.
    AuthorityViewChanged {
        /// Version the view now reports.
        current_version: u64,
    },
    /// No full verification ever recorded this decision.
    NeverAdmitted,
}

/// A process-local set of authorization decisions that an epoch bump invalidates.
///
/// It is a cache, never an authority. §5.3 rule 5 requires every consumer to read durable epoch
/// state at startup and before any cache-based allow decision; [`Self::admitted_now`] is the
/// door that does that itself, so a caller cannot skip it by forgetting. [`Self::is_admitted`]
/// and [`Self::admit`] take a fully caller-built [`CacheKey`] and therefore do NOT read the
/// view -- they are set operations over keys, and a caller that reaches for them owes the view
/// read separately. Prefer the `_now` pair.
#[derive(Clone, Debug, Default)]
pub struct RevalidationGate {
    admitted: HashSet<CacheKey>,
}

impl RevalidationGate {
    /// An empty gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that a full verification allowed this **caller-built** key.
    ///
    /// Nothing here checks that the key's `authority_view_version` was ever read off a view; see
    /// [`Self::admit_verified`] for the door that reads it.
    pub fn admit(&mut self, key: CacheKey) {
        self.admitted.insert(key);
    }

    /// Reports whether a full verification already allowed this exact **caller-built** key.
    ///
    /// A set-membership test and nothing more: it consults no view, no clock, and no epoch, so
    /// a stale key answers `true` forever. [`Self::admitted_now`] is the door that refuses.
    pub fn is_admitted(&self, key: &CacheKey) -> bool {
        self.admitted.contains(key)
    }

    /// Records a decision against the version the view reports **now**, returning the key used.
    pub fn admit_verified(
        &mut self,
        decision: &AdmittedDecision,
        view: &dyn AuthorizationView,
    ) -> CacheKey {
        let key = decision.cache_key(view.version());
        self.admitted.insert(key);
        key
    }

    /// The §5.3 rule 5 door: re-reads the durable view, then consults the cache.
    ///
    /// Expiry, the view's current epoch, and the view's version are all read here rather than
    /// taken from the caller, so a cached decision cannot outlive the facts it was made on:
    /// an expired chain, a moved epoch, an unknown epoch scope, and a view that has changed
    /// version since admission are each a distinct refusal, and every one of them denies.
    pub fn admitted_now(
        &self,
        decision: &AdmittedDecision,
        now_ms: u64,
        view: &dyn AuthorizationView,
    ) -> Result<CacheKey, CacheRefusal> {
        if now_ms >= decision.expiry_ms {
            return Err(CacheRefusal::Expired);
        }
        let current_epoch = view
            .current_epoch(&decision.epoch_scope)
            .ok_or(CacheRefusal::EpochScopeUnknown)?;
        if current_epoch != decision.epoch {
            return Err(CacheRefusal::EpochMoved { current_epoch });
        }
        let current_version = view.version();
        let key = decision.cache_key(current_version);
        if self.admitted.contains(&key) {
            return Ok(key);
        }
        // Distinguish "admitted, but against an older view" from "never admitted at all", so a
        // caller can log a revalidation rather than a spurious authorization failure. The exact
        // key is absent, so any entry matching the other four dimensions differs on version.
        if self.admitted.iter().any(|admitted| {
            admitted.chain_id == decision.chain_id
                && admitted.epoch_scope == decision.epoch_scope
                && admitted.epoch == decision.epoch
                && admitted.expiry_ms == decision.expiry_ms
        }) {
            return Err(CacheRefusal::AuthorityViewChanged { current_version });
        }
        Err(CacheRefusal::NeverAdmitted)
    }

    /// Number of admitted keys held.
    pub fn len(&self) -> usize {
        self.admitted.len()
    }

    /// Reports whether the gate holds no admitted key.
    pub fn is_empty(&self) -> bool {
        self.admitted.is_empty()
    }

    /// Drops every entry for `epoch_scope` below `new_epoch`, returning how many were dropped.
    ///
    /// The match is on the exact epoch scope because §5.1 persists `current_epoch` per exact
    /// scope; a verse bump does not silently invalidate a petal-rooted chain's own epoch.
    pub fn on_epoch_bump(&mut self, epoch_scope: &Scope, new_epoch: u64) -> usize {
        let before = self.admitted.len();
        self.admitted
            .retain(|key| !(key.epoch_scope == *epoch_scope && key.epoch < new_epoch));
        before - self.admitted.len()
    }

    /// Drops every entry whose chain has expired by `now_ms`, returning how many were dropped.
    pub fn drop_expired(&mut self, now_ms: u64) -> usize {
        let before = self.admitted.len();
        self.admitted.retain(|key| key.expiry_ms > now_ms);
        before - self.admitted.len()
    }

    /// Drops every entry recorded against an older authority-view version.
    pub fn drop_stale_view(&mut self, current_version: u64) -> usize {
        let before = self.admitted.len();
        self.admitted
            .retain(|key| key.authority_view_version >= current_version);
        before - self.admitted.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Hash32, Identifier32};

    /// An in-memory stand-in for the durable view Wave 3 supplies.
    struct FakeAuthorizationView {
        scope: Scope,
        epoch: Option<u64>,
        version: u64,
    }

    impl AuthorizationView for FakeAuthorizationView {
        fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
            if *epoch_scope == self.scope {
                self.epoch
            } else {
                None
            }
        }

        fn version(&self) -> u64 {
            self.version
        }
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn verse_scope() -> Scope {
        Scope::verse_wide(identifier(0x11))
    }

    fn petal_scope() -> Scope {
        Scope::new(identifier(0x11), Some(identifier(0x22)), None).expect("petal scope")
    }

    fn session() -> PinnedSession {
        PinnedSession {
            leaf_principal: Principal::from_public_key([0x03; 32]),
            chain_id: Hash32([0xc1; 32]),
            leaf_certificate_id: Hash32([0xc2; 32]),
            epoch_scope: verse_scope(),
            epoch: 7,
            expires_at_ms: 2_000,
            subscribed_scopes: vec![petal_scope()],
        }
    }

    fn key(epoch_scope: Scope, epoch: u64, expiry_ms: u64, version: u64) -> CacheKey {
        CacheKey {
            chain_id: Hash32([0xc1; 32]),
            epoch_scope,
            epoch,
            expiry_ms,
            authority_view_version: version,
        }
    }

    #[test]
    fn a_pinned_session_reports_expiry_before_it_consults_the_view() {
        let view = FakeAuthorizationView {
            scope: verse_scope(),
            epoch: Some(7),
            version: 1,
        };
        let session = session();

        assert_eq!(session.is_still_valid(1_999, &view), SessionValidity::Valid);
        assert_eq!(
            session.is_still_valid(2_000, &view),
            SessionValidity::Expired,
            "expires_at_ms is exclusive"
        );

        let bumped = FakeAuthorizationView {
            scope: verse_scope(),
            epoch: Some(8),
            version: 2,
        };
        assert_eq!(
            session.is_still_valid(1_999, &bumped),
            SessionValidity::ReauthorizationRequired
        );

        let unknown = FakeAuthorizationView {
            scope: petal_scope(),
            epoch: Some(7),
            version: 2,
        };
        assert_eq!(
            session.is_still_valid(1_999, &unknown),
            SessionValidity::ReauthorizationRequired,
            "a view that knows no epoch for the scope is never an allow"
        );
        assert_eq!(view.version(), 1);
    }

    #[test]
    fn a_session_only_covers_the_scopes_it_subscribed_to() {
        let session = session();
        let resource = Scope::new(
            identifier(0x11),
            Some(identifier(0x22)),
            Some(identifier(0x33)),
        )
        .expect("resource scope");

        assert!(session.covers(&petal_scope()));
        assert!(session.covers(&resource));
        assert!(!session.covers(&verse_scope()));
        assert!(!session.covers(
            &Scope::new(identifier(0x11), Some(identifier(0x44)), None).expect("other petal")
        ));
    }

    #[test]
    fn the_cache_key_separates_every_dimension_of_a_decision() {
        let session = session();
        let base = session.cache_key(3);

        assert_eq!(base.chain_id, session.chain_id);
        assert_eq!(base.epoch_scope, session.epoch_scope);
        assert_eq!(base.epoch, session.epoch);
        assert_eq!(base.expiry_ms, session.expires_at_ms);
        assert_eq!(base.authority_view_version, 3);
        assert_ne!(base, session.cache_key(4));
        assert_ne!(base, key(petal_scope(), 7, 2_000, 3));
        assert_ne!(base, key(verse_scope(), 8, 2_000, 3));
        assert_ne!(base, key(verse_scope(), 7, 2_001, 3));
    }

    #[test]
    fn an_epoch_bump_invalidates_exactly_the_matching_entries() {
        let mut gate = RevalidationGate::new();
        let stale = key(verse_scope(), 7, 2_000, 1);
        let current = key(verse_scope(), 8, 2_000, 1);
        let other_scope = key(petal_scope(), 7, 2_000, 1);

        assert!(gate.is_empty());
        gate.admit(stale);
        gate.admit(current);
        gate.admit(other_scope);
        assert_eq!(gate.len(), 3);
        assert!(gate.is_admitted(&stale));

        assert_eq!(gate.on_epoch_bump(&verse_scope(), 8), 1);
        assert!(!gate.is_admitted(&stale));
        assert!(gate.is_admitted(&current));
        assert!(
            gate.is_admitted(&other_scope),
            "a verse bump does not invalidate a petal-rooted epoch"
        );

        assert_eq!(gate.on_epoch_bump(&petal_scope(), 8), 1);
        assert!(!gate.is_admitted(&other_scope));
    }

    #[test]
    fn the_gate_reads_the_view_itself_rather_than_trusting_a_caller_built_key() {
        fn fake_view(scope: Scope, epoch: Option<u64>, version: u64) -> FakeAuthorizationView {
            FakeAuthorizationView {
                scope,
                epoch,
                version,
            }
        }

        let session = session();
        let decision = session.decision();
        let view = fake_view(verse_scope(), Some(7), 1);

        let mut gate = RevalidationGate::new();
        assert_eq!(
            gate.admitted_now(&decision, 1_000, &view),
            Err(CacheRefusal::NeverAdmitted)
        );

        let admitted_key = gate.admit_verified(&decision, &view);
        assert_eq!(admitted_key, session.cache_key(1));
        assert_eq!(gate.admitted_now(&decision, 1_000, &view), Ok(admitted_key));

        // The three dimensions the caller no longer supplies, each denying on its own.
        assert_eq!(
            gate.admitted_now(&decision, 2_000, &view),
            Err(CacheRefusal::Expired),
            "expiry is read from the clock, not from whoever built the key"
        );

        let bumped = fake_view(verse_scope(), Some(8), 1);
        assert_eq!(
            gate.admitted_now(&decision, 1_000, &bumped),
            Err(CacheRefusal::EpochMoved { current_epoch: 8 }),
            "a moved epoch denies even though the stored key is still in the set"
        );
        assert!(
            gate.is_admitted(&admitted_key),
            "the raw set-membership door still answers true; that is exactly why it is hazardous"
        );

        let unknown_scope = fake_view(petal_scope(), Some(7), 1);
        assert_eq!(
            gate.admitted_now(&decision, 1_000, &unknown_scope),
            Err(CacheRefusal::EpochScopeUnknown)
        );

        let newer_view = fake_view(verse_scope(), Some(7), 2);
        assert_eq!(
            gate.admitted_now(&decision, 1_000, &newer_view),
            Err(CacheRefusal::AuthorityViewChanged { current_version: 2 }),
            "a decision made against an older view is a revalidation, not an allow"
        );
    }

    #[test]
    fn a_decision_and_its_session_agree_on_every_cache_dimension() {
        let session = session();
        let decision = session.decision();

        assert_eq!(decision.chain_id, session.chain_id);
        assert_eq!(decision.epoch_scope, session.epoch_scope);
        assert_eq!(decision.epoch, session.epoch);
        assert_eq!(decision.expiry_ms, session.expires_at_ms);
        assert_eq!(decision.cache_key(9), session.cache_key(9));
    }

    #[test]
    fn expiry_and_view_version_also_evict_cached_decisions() {
        let mut gate = RevalidationGate::new();
        gate.admit(key(verse_scope(), 7, 1_000, 1));
        gate.admit(key(verse_scope(), 7, 3_000, 1));
        assert_eq!(gate.drop_expired(1_000), 1);
        assert_eq!(gate.len(), 1);

        gate.admit(key(verse_scope(), 7, 3_000, 2));
        assert_eq!(gate.drop_stale_view(2), 1);
        assert!(gate.is_admitted(&key(verse_scope(), 7, 3_000, 2)));
    }
}
