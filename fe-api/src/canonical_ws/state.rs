//! Shared per-process state for the SPEC-7 `/ws/canonical` module.
//! See `AGENTS.md` for why this state lives beside, not inside, `crate::ws::ApiState`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{broadcast, Mutex as AsyncMutex};

use fe_canonical_log::capability::{AdmittedDecision, AuthorizationView, RevalidationGate};
use fe_canonical_log::envelope::{Hash32, Scope};
use fe_canonical_log::wire::commit::CanonicalCommitPipeline;
use fe_canonical_log::wire::cursor::{BranchRegistry, CommittedDelta};
use fe_canonical_log::wire::preview::PreviewDeltaBody;
use fe_canonical_log::wire::preview_limiter::PreviewRateLimiter;
use fe_canonical_log::wire::session::{
    CapabilityVerifier, ObjectClassBits, VerbBits, VerifiedAuthorization,
};
use fe_canonical_log::wire::snapshot::ScopeSnapshotSource;

/// Ceiling on process-wide cached verifications; see `AGENTS.md` §bounded-state.
const MAX_CACHED_VERIFICATIONS: usize = 1_024;

/// The exact question one cached verification answered: which chain bytes, granting which verb
/// and object class, over which scope. Keying the cache by the chain digest alone would serve a
/// decision made for one scope in answer to a request for another — see `AGENTS.md`
/// §5.3-the-cache-is-recomputed-not-remembered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VerificationRequest {
    chain_digest: Hash32,
    requested_verb: VerbBits,
    requested_object_class: ObjectClassBits,
    requested_scope: Scope,
}

impl VerificationRequest {
    /// The request key for `chain_bytes` presented against this verb, object class, and scope.
    pub fn new(
        chain_bytes: &[u8],
        requested_verb: VerbBits,
        requested_object_class: ObjectClassBits,
        requested_scope: Scope,
    ) -> Self {
        Self {
            chain_digest: Hash32::of(chain_bytes),
            requested_verb,
            requested_object_class,
            requested_scope,
        }
    }
}

/// The four view-independent dimensions of the decision a verified authorization records.
/// Deliberately not a [`fe_canonical_log::capability::CacheKey`]: the fifth dimension, the
/// authority-view version, belongs to the gate at the moment of the question.
fn admitted_decision(authorization: &VerifiedAuthorization) -> AdmittedDecision {
    AdmittedDecision {
        chain_id: authorization.chain_id,
        epoch_scope: authorization.epoch_scope,
        epoch: authorization.scope_epoch,
        expiry_ms: authorization.expires_at_ms,
    }
}

/// One cached verification: the decision the verifier reached, and the four dimensions the gate
/// needs to re-ask whether it still holds.
struct CachedVerification {
    authorization: VerifiedAuthorization,
    decision: AdmittedDecision,
}

/// The §5.3 rule 1 verification cache and the [`RevalidationGate`] guarding it, behind a single
/// lock. Two locks would let `cached_verification` and `admit_verification` acquire them in
/// opposite orders; one lock makes that unrepresentable.
#[derive(Default)]
struct VerificationCache {
    entries: HashMap<VerificationRequest, CachedVerification>,
    gate: RevalidationGate,
}

impl VerificationCache {
    /// Drops every entry whose stored decision `keep` rejects, returning how many were dropped.
    fn retain(&mut self, keep: impl Fn(&AdmittedDecision) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, entry| keep(&entry.decision));
        before - self.entries.len()
    }
}

/// Full per-process state for the `authorize`/`commit_submit`/`subscribe`/`resume`/
/// `snapshot_ack` surface (SPEC-7 §2, §4-§6).
///
/// The preview dispatch task is built from `handler::preview_task::PreviewTaskState` instead
/// of a reference to this type — see `AGENTS.md` §preview-disjointness for why that is a
/// distinct type rather than a runtime check over this one.
pub struct CanonicalLogState {
    /// SPEC-4 log-first commit pipeline (§4.1-4.3).
    pub commit_pipeline: Arc<dyn CanonicalCommitPipeline>,
    /// SPEC-7 §3 durable cursor registry.
    pub branch_registry: Arc<dyn BranchRegistry>,
    /// SPEC-7 §5.2 fresh scope snapshot source.
    pub snapshot_source: Arc<dyn ScopeSnapshotSource>,
    /// SPEC-3 capability chain verifier, consulted on `authorize` (§2.2) and again on every
    /// `subscribe` that presents a scope the handshake did not itself verify.
    pub capability_verifier: Arc<dyn CapabilityVerifier>,
    /// The persistent authorization view: durable epoch state and the cache-versioning
    /// dimension `capability::revalidation::CacheKey` requires. Backs both the `authorize`
    /// verification cache (`capability/AGENTS.md` §5.3 obligation 1) and the timer-based
    /// `PinnedSession::is_still_valid` re-check (obligation 3).
    pub authorization_view: Arc<dyn AuthorizationView>,
    /// Per-principal, per-scope preview rate limiter (§7.2.3). `Arc`-shared so connection
    /// setup can hand a clone to the structurally separate preview task without sharing this
    /// whole state; commit-class dispatch never locks it.
    pub preview_limiter: Arc<AsyncMutex<PreviewRateLimiter>>,
    /// Fan-out of freshly committed deltas; each connection's commit-class task filters this
    /// by its own subscriptions (§4.3 rule 3) before forwarding.
    pub committed_delta_tx: broadcast::Sender<CommittedDelta>,
    /// Fan-out of accepted previews. Held here only so connection setup can hand a clone to
    /// the preview task — commit-class dispatch never reads this field.
    pub preview_delta_tx: broadcast::Sender<PreviewDeltaBody>,
    /// §5.3 rule 1 process-wide verification cache plus its revalidation gate.
    verification_cache: StdMutex<VerificationCache>,
}

impl CanonicalLogState {
    /// Builds state from its five authorities plus preview policy and broadcast capacities.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commit_pipeline: Arc<dyn CanonicalCommitPipeline>,
        branch_registry: Arc<dyn BranchRegistry>,
        snapshot_source: Arc<dyn ScopeSnapshotSource>,
        capability_verifier: Arc<dyn CapabilityVerifier>,
        authorization_view: Arc<dyn AuthorizationView>,
        preview_limiter: PreviewRateLimiter,
        committed_delta_broadcast_capacity: usize,
        preview_broadcast_capacity: usize,
    ) -> Self {
        let (committed_delta_tx, _) = broadcast::channel(committed_delta_broadcast_capacity);
        let (preview_delta_tx, _) = broadcast::channel(preview_broadcast_capacity);
        Self {
            commit_pipeline,
            branch_registry,
            snapshot_source,
            capability_verifier,
            authorization_view,
            preview_limiter: Arc::new(AsyncMutex::new(preview_limiter)),
            committed_delta_tx,
            preview_delta_tx,
            verification_cache: StdMutex::new(VerificationCache::default()),
        }
    }

    /// §5.3 rule 1/rule 5: returns a cached verification for `request` only if
    /// [`RevalidationGate::admitted_now`] still allows it.
    ///
    /// Nothing about the admission is remembered on this side. The entry stores the four
    /// view-independent dimensions of the decision ([`AdmittedDecision`]); the gate reads the
    /// expiry, the live durable epoch, and the live authority-view version itself and rebuilds
    /// the `CacheKey` from them. A peer re-sending byte-identical `authorize` bytes after a
    /// revocation therefore misses and pays for a full verification, and a refused entry is
    /// evicted on the spot rather than left to be re-refused forever. See `AGENTS.md`
    /// §5.3-the-cache-is-recomputed-not-remembered.
    ///
    /// The cache lock is held across the `AuthorizationView` read, so an implementation of that
    /// trait must not call back into this state.
    pub(crate) fn cached_verification(
        &self,
        request: &VerificationRequest,
        now_ms: u64,
    ) -> Option<VerifiedAuthorization> {
        let mut cache = self
            .verification_cache
            .lock()
            .expect("verification cache lock");
        let decision = match cache.entries.get(request) {
            Some(entry) => entry.decision,
            None => return None,
        };
        if cache
            .gate
            .admitted_now(&decision, now_ms, self.authorization_view.as_ref())
            .is_err()
        {
            cache.entries.remove(request);
            return None;
        }
        cache
            .entries
            .get(request)
            .map(|entry| entry.authorization.clone())
    }

    /// §5.3 rule 1: admits a freshly verified chain into the cache and the gate that guards it.
    ///
    /// [`RevalidationGate::admit_verified`] reads the authority-view version itself, so no
    /// caller-built `CacheKey` — the one dimension the gate exists to invalidate on — is ever
    /// supplied from here. Already-expired entries are dropped first, and the map refuses to
    /// grow past [`MAX_CACHED_VERIFICATIONS`]: a full verification already succeeded, so
    /// declining to remember it costs latency, never authority.
    pub(crate) fn admit_verification(
        &self,
        request: VerificationRequest,
        authorization: VerifiedAuthorization,
        now_ms: u64,
    ) {
        let decision = admitted_decision(&authorization);
        if now_ms >= decision.expiry_ms {
            return;
        }
        let mut cache = self
            .verification_cache
            .lock()
            .expect("verification cache lock");
        cache.gate.drop_expired(now_ms);
        cache.retain(|held| held.expiry_ms > now_ms);
        if !cache.entries.contains_key(&request) && cache.entries.len() >= MAX_CACHED_VERIFICATIONS
        {
            return;
        }
        cache
            .gate
            .admit_verified(&decision, self.authorization_view.as_ref());
        cache.entries.insert(
            request,
            CachedVerification {
                authorization,
                decision,
            },
        );
    }

    /// Revocation path: drops every cached decision an epoch bump for `epoch_scope` invalidates,
    /// from both the gate and the map it guards. Returns how many cache entries were dropped.
    pub fn invalidate_on_epoch_bump(&self, epoch_scope: &Scope, new_epoch: u64) -> usize {
        let mut cache = self
            .verification_cache
            .lock()
            .expect("verification cache lock");
        cache.gate.on_epoch_bump(epoch_scope, new_epoch);
        cache.retain(|held| !(held.epoch_scope == *epoch_scope && held.epoch < new_epoch))
    }

    /// Revocation path: drops every cached decision whose chain has expired by `now_ms`.
    pub fn invalidate_expired(&self, now_ms: u64) -> usize {
        let mut cache = self
            .verification_cache
            .lock()
            .expect("verification cache lock");
        cache.gate.drop_expired(now_ms);
        cache.retain(|held| held.expiry_ms > now_ms)
    }

    /// Revocation path: drops every gate admission recorded against a superseded
    /// authorization-view version, reading the live version rather than a remembered one.
    ///
    /// Only the gate is touched: the map holds no version dimension by design, and an entry
    /// whose admission was dropped is inert — `cached_verification` refuses it and evicts it on
    /// the next touch. Returns how many gate admissions were dropped.
    pub fn invalidate_stale_view(&self) -> usize {
        let current_version = self.authorization_view.version();
        let mut cache = self
            .verification_cache
            .lock()
            .expect("verification cache lock");
        cache.gate.drop_stale_view(current_version)
    }

    /// The production revocation sweep: `handler`'s per-connection revalidation timer calls
    /// this on every tick, so expiry and authority-view advances evict cached decisions without
    /// waiting for a caller to happen to miss. Returns how many cache entries were dropped.
    pub fn sweep_revocations(&self, now_ms: u64) -> usize {
        self.invalidate_expired(now_ms) + self.invalidate_stale_view()
    }

    /// Number of cached verifications currently held; for tests and diagnostics.
    pub fn cached_verification_count(&self) -> usize {
        self.verification_cache
            .lock()
            .expect("verification cache lock")
            .entries
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use fe_canonical_log::envelope::{Author, Identifier32};

    /// A view whose epoch and version a test can advance, so revocation can be exercised the
    /// way a durable view would deliver it.
    struct MutableAuthorizationView {
        epoch: AtomicU64,
        version: AtomicU64,
    }

    impl MutableAuthorizationView {
        fn new() -> Self {
            Self {
                epoch: AtomicU64::new(1),
                version: AtomicU64::new(1),
            }
        }

        /// Advances the durable epoch and the view version, exactly as a real bump would.
        fn bump_epoch(&self) {
            self.epoch.fetch_add(1, Ordering::SeqCst);
            self.version.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl AuthorizationView for MutableAuthorizationView {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            Some(self.epoch.load(Ordering::SeqCst))
        }
        fn version(&self) -> u64 {
            self.version.load(Ordering::SeqCst)
        }
    }

    fn sample_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x33; 32]))
    }

    fn other_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x44; 32]))
    }

    fn sample_authorization() -> VerifiedAuthorization {
        VerifiedAuthorization {
            leaf_principal: Author::from_public_key([0x11; 32]),
            chain_id: Hash32([0x22; 32]),
            epoch_scope: sample_scope(),
            scope_epoch: 1,
            expires_at_ms: 10_000,
        }
    }

    fn state_with(view: Arc<dyn AuthorizationView>) -> CanonicalLogState {
        let branch_registry: Arc<dyn BranchRegistry> =
            Arc::new(fe_canonical_log::wire::test_support::InMemoryBranchRegistry::new());
        let snapshot_source: Arc<dyn ScopeSnapshotSource> = Arc::new(
            fe_canonical_log::wire::test_support::MockScopeSnapshotSource::new(
                Arc::new(fe_canonical_log::wire::test_support::InMemoryBranchRegistry::new()),
                Vec::new(),
            ),
        );
        CanonicalLogState::new(
            Arc::new(
                fe_canonical_log::wire::test_support::ScriptedCommitPipeline {
                    derived_op_id: Hash32([0; 32]),
                    result:
                        fe_canonical_log::wire::commit::PipelineResult::AcceptedPendingMaterialization,
                },
            ),
            branch_registry,
            snapshot_source,
            Arc::new(fe_canonical_log::wire::test_support::MockCapabilityVerifier::new()),
            view,
            PreviewRateLimiter::new(
                fe_canonical_log::wire::preview_limiter::PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        )
    }

    fn admit_sample(state: &CanonicalLogState, chain_bytes: &[u8]) -> VerificationRequest {
        let request = VerificationRequest::new(chain_bytes, 0x01, 0x01, sample_scope());
        state.admit_verification(request.clone(), sample_authorization(), 0);
        request
    }

    #[test]
    fn a_cached_verification_is_served_only_while_the_gate_still_admits_it_now() {
        let state = state_with(Arc::new(MutableAuthorizationView::new()));
        let chain_bytes = vec![1, 2, 3];
        let request = VerificationRequest::new(&chain_bytes, 0x01, 0x01, sample_scope());
        assert!(state.cached_verification(&request, 0).is_none());

        admit_sample(&state, &chain_bytes);
        assert_eq!(
            state.cached_verification(&request, 0),
            Some(sample_authorization())
        );
    }

    /// The C2 regression: a revoked capability must not be served from the cache even when the
    /// peer re-sends byte-identical `authorize` bytes.
    #[test]
    fn a_revoked_capability_is_not_served_from_the_cache() {
        let view = Arc::new(MutableAuthorizationView::new());
        let state = state_with(view.clone());
        let chain_bytes = vec![1, 2, 3];
        let request = admit_sample(&state, &chain_bytes);
        assert!(state.cached_verification(&request, 0).is_some());

        // Revocation as a durable view delivers it: the epoch moves and the view version with
        // it. Nothing calls any invalidation method here — the cache must refuse on its own,
        // because `admitted_now` re-reads the view instead of trusting a remembered key.
        view.bump_epoch();
        assert!(
            state.cached_verification(&request, 0).is_none(),
            "an epoch bump must miss the cache even with identical chain bytes"
        );
        assert_eq!(
            state.cached_verification_count(),
            0,
            "a refused entry evicts itself rather than lingering to be re-refused"
        );
    }

    /// The same revocation delivered as an explicit gate invalidation, which is the path a
    /// production `scope_epoch_bump` handler would call.
    #[test]
    fn the_explicit_epoch_bump_invalidation_path_evicts_both_the_gate_and_the_map() {
        let state = state_with(Arc::new(MutableAuthorizationView::new()));
        let chain_bytes = vec![4, 5, 6];
        let request = admit_sample(&state, &chain_bytes);
        assert_eq!(state.cached_verification_count(), 1);

        assert_eq!(state.invalidate_on_epoch_bump(&other_scope(), 99), 0);
        assert!(state.cached_verification(&request, 0).is_some());

        assert_eq!(state.invalidate_on_epoch_bump(&sample_scope(), 2), 1);
        assert_eq!(state.cached_verification_count(), 0);
        assert!(state.cached_verification(&request, 0).is_none());
    }

    #[test]
    fn an_expired_chain_is_neither_served_nor_retained_by_the_sweep() {
        let state = state_with(Arc::new(MutableAuthorizationView::new()));
        let chain_bytes = vec![7, 8, 9];
        let request = admit_sample(&state, &chain_bytes);

        // `expires_at_ms` is exclusive, matching `PinnedSession::is_still_valid`.
        assert!(state.cached_verification(&request, 9_999).is_some());

        // The timer-driven sweep evicts without waiting for a caller to miss.
        assert_eq!(state.sweep_revocations(10_000), 1);
        assert_eq!(state.cached_verification_count(), 0);
        assert!(state.cached_verification(&request, 10_000).is_none());
    }

    /// Keying by chain digest alone would answer a request for a scope the chain was never
    /// verified against with a decision made for a different one.
    #[test]
    fn a_different_requested_scope_verb_or_object_class_is_a_different_cache_entry() {
        let state = state_with(Arc::new(MutableAuthorizationView::new()));
        let chain_bytes = vec![1, 1, 1];
        admit_sample(&state, &chain_bytes);

        for widened in [
            VerificationRequest::new(&chain_bytes, 0x01, 0x01, other_scope()),
            VerificationRequest::new(&chain_bytes, 0xff, 0x01, sample_scope()),
            VerificationRequest::new(&chain_bytes, 0x01, 0xff, sample_scope()),
        ] {
            assert!(
                state.cached_verification(&widened, 0).is_none(),
                "identical chain bytes must not answer a different request"
            );
        }
    }

    #[test]
    fn the_cache_refuses_to_grow_past_its_ceiling() {
        let state = state_with(Arc::new(MutableAuthorizationView::new()));
        for index in 0..(MAX_CACHED_VERIFICATIONS + 16) {
            let chain_bytes = (index as u64).to_be_bytes().to_vec();
            admit_sample(&state, &chain_bytes);
        }
        assert_eq!(state.cached_verification_count(), MAX_CACHED_VERIFICATIONS);
    }
}
