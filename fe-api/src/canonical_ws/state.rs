//! Shared per-process state for the SPEC-7 `/ws/canonical` module.
//! See `AGENTS.md` for why this state lives beside, not inside, `crate::ws::ApiState`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::{broadcast, Mutex as AsyncMutex};

use fe_canonical_log::capability::{AuthorizationView, CacheKey, RevalidationGate};
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::wire::commit::CanonicalCommitPipeline;
use fe_canonical_log::wire::cursor::{BranchRegistry, CommittedDelta};
use fe_canonical_log::wire::preview::PreviewDeltaBody;
use fe_canonical_log::wire::preview_limiter::PreviewRateLimiter;
use fe_canonical_log::wire::session::{CapabilityVerifier, VerifiedAuthorization};
use fe_canonical_log::wire::snapshot::ScopeSnapshotSource;

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
    /// SPEC-3 capability chain verifier, consulted on `authorize` (§2.2).
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
    /// §5.3 rule 1 process-wide cache of verified capability chains, keyed by the BLAKE3
    /// digest of the raw chain bytes, alongside the [`CacheKey`] each was admitted under.
    verified_chain_cache: StdMutex<HashMap<Hash32, (VerifiedAuthorization, CacheKey)>>,
    /// §5.3 rule 1 revalidation gate: the set of [`CacheKey`]s a full verification has
    /// admitted. [`Self::cached_verification`] is the only reader; it never returns a cached
    /// result without consulting [`RevalidationGate::is_admitted`] first.
    revalidation_gate: StdMutex<RevalidationGate>,
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
            verified_chain_cache: StdMutex::new(HashMap::new()),
            revalidation_gate: StdMutex::new(RevalidationGate::new()),
        }
    }

    /// §5.3 rule 1: returns a still-admitted cached verification for `chain_bytes`, or `None`
    /// if uncached or no longer admitted by the revalidation gate. Never returns a cached
    /// result without consulting [`RevalidationGate::is_admitted`] — see
    /// `capability/AGENTS.md` §5.3 obligation 1, exercised by `handler`'s `authorize` dispatch.
    pub(crate) fn cached_verification(&self, chain_bytes: &[u8]) -> Option<VerifiedAuthorization> {
        let digest = Hash32::of(chain_bytes);
        let cache = self.verified_chain_cache.lock().expect("chain cache lock");
        let (authorization, cache_key) = cache.get(&digest)?;
        let gate = self
            .revalidation_gate
            .lock()
            .expect("revalidation gate lock");
        if gate.is_admitted(cache_key) {
            Some(authorization.clone())
        } else {
            None
        }
    }

    /// §5.3 rule 1: admits a freshly verified chain into both the cache and the gate that
    /// guards it, so a later epoch bump or expiry can revoke trust without touching the cache
    /// entry directly.
    pub(crate) fn admit_verification(
        &self,
        chain_bytes: &[u8],
        authorization: VerifiedAuthorization,
        cache_key: CacheKey,
    ) {
        self.revalidation_gate
            .lock()
            .expect("revalidation gate lock")
            .admit(cache_key);
        self.verified_chain_cache
            .lock()
            .expect("chain cache lock")
            .insert(Hash32::of(chain_bytes), (authorization, cache_key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_canonical_log::envelope::{Author, Identifier32, Scope};

    struct FixedAuthorizationView {
        version: u64,
    }

    impl AuthorizationView for FixedAuthorizationView {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            Some(1)
        }
        fn version(&self) -> u64 {
            self.version
        }
    }

    fn sample_authorization() -> VerifiedAuthorization {
        VerifiedAuthorization {
            leaf_principal: Author::from_public_key([0x11; 32]),
            chain_id: Hash32([0x22; 32]),
            epoch_scope: Scope::verse_wide(Identifier32([0x33; 32])),
            scope_epoch: 1,
            expires_at_ms: 10_000,
        }
    }

    #[test]
    fn a_verification_is_cached_only_while_the_gate_still_admits_its_key() {
        let view: Arc<dyn AuthorizationView> = Arc::new(FixedAuthorizationView { version: 1 });
        let commit_pipeline: Arc<dyn CanonicalCommitPipeline> = Arc::new(
            fe_canonical_log::wire::test_support::ScriptedCommitPipeline {
                derived_op_id: Hash32([0; 32]),
                result:
                    fe_canonical_log::wire::commit::PipelineResult::AcceptedPendingMaterialization,
            },
        );
        let branch_registry: Arc<dyn BranchRegistry> =
            Arc::new(fe_canonical_log::wire::test_support::InMemoryBranchRegistry::new());
        let snapshot_source: Arc<dyn ScopeSnapshotSource> = Arc::new(
            fe_canonical_log::wire::test_support::MockScopeSnapshotSource::new(
                Arc::new(fe_canonical_log::wire::test_support::InMemoryBranchRegistry::new()),
                Vec::new(),
            ),
        );
        let capability_verifier: Arc<dyn CapabilityVerifier> =
            Arc::new(fe_canonical_log::wire::test_support::MockCapabilityVerifier::new());

        let state = CanonicalLogState::new(
            commit_pipeline,
            branch_registry,
            snapshot_source,
            capability_verifier,
            view,
            PreviewRateLimiter::new(
                fe_canonical_log::wire::preview_limiter::PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        );

        let chain_bytes = vec![1, 2, 3];
        assert!(state.cached_verification(&chain_bytes).is_none());

        let authorization = sample_authorization();
        let cache_key = CacheKey {
            chain_id: authorization.chain_id,
            epoch_scope: authorization.epoch_scope,
            epoch: authorization.scope_epoch,
            expiry_ms: authorization.expires_at_ms,
            authority_view_version: 1,
        };
        state.admit_verification(&chain_bytes, authorization.clone(), cache_key);
        assert_eq!(state.cached_verification(&chain_bytes), Some(authorization));

        // An epoch bump for a DIFFERENT scope leaves this key admitted; one for the SAME
        // scope at or above its epoch evicts it, and the cache must honor that immediately.
        state
            .revalidation_gate
            .lock()
            .expect("gate lock")
            .on_epoch_bump(&cache_key.epoch_scope, cache_key.epoch + 1);
        assert!(state.cached_verification(&chain_bytes).is_none());
    }
}
