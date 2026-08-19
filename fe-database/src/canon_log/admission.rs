//! SPEC-4 §2 validate-then-log pipeline, wired to the SurrealDB verified log.
//! Rationale: `canon_log/AGENTS.md` §admission-order.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use async_trait::async_trait;
use fe_canonical_log::materialize::errors::{
    AdmissionDisposition, AdmissionOutcome, AppendOutcome,
};
use fe_canonical_log::materialize::traits::{
    admit_candidate, CandidateVerifier, EquivocationIndex, ParentVerseLookup, VerifiedEnvelopeMeta,
};

use crate::canon_log::append_store::{SurrealVerifiedLogStore, VerifiedLogStoreError};

/// The future a cheap precondition returns.
///
/// Deliberately `'static`: a closure copies the identifiers it needs out of the operation facts
/// and then awaits, which keeps the adapter's bound a plain `Fn(&VerifiedEnvelopeMeta) ->
/// PreconditionFuture` instead of a higher-ranked one whose output borrows its argument. Every
/// field a precondition reads is `Copy`, so nothing is lost.
pub type PreconditionFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>;

/// A locally decidable target/scope/lifecycle check that runs BEFORE an appendable envelope
/// exists (§2.1, `fe-database/src/AGENTS.md` §log-first-commit).
#[async_trait]
pub trait CheapPrecondition: Send + Sync {
    /// Returns the caller's original command error when the target is missing or out of scope.
    async fn check(&self, meta: &VerifiedEnvelopeMeta) -> anyhow::Result<()>;
}

/// The fault channel every admission-evidence source must expose.
///
/// `EquivocationIndex::op_id_at` answers `Option<Hash32>` and `None` **admits**: a source that
/// could not distinguish "no conflicting operation" from "I could not read" would fail OPEN on
/// a storage fault, letting a D-CL25 equivocation through exactly when the substrate is sick.
/// [`admit_and_append`] therefore refuses to take an index or a parent-verse lookup that does
/// not implement this trait — the fault channel is a type requirement, not a caller's promise.
///
/// [`admit_and_append`] drains this channel before it starts, so a fault raised earlier cannot
/// decide a later candidate; a caller that wants those earlier faults drains them first.
pub trait EvidenceAvailability {
    /// Drains and returns every evidence fault observed since the last drain, in order.
    fn take_evidence_faults(&self) -> Vec<String>;
}

/// The explicit "this operation has no cheap local precondition" choice.
///
/// [`admit_and_append`] takes a precondition by required parameter rather than `Option`, so
/// skipping the check is a visible decision at the call site instead of an omission.
pub struct NoPrecondition;

#[async_trait]
impl CheapPrecondition for NoPrecondition {
    async fn check(&self, _meta: &VerifiedEnvelopeMeta) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Adapts a closure to [`CheapPrecondition`], so `require_node_exists`, `require_petal_scope`,
/// and the rest of `handlers::preconditions` can be passed unchanged.
pub struct PreconditionFn<F>(F);

impl<F> PreconditionFn<F>
where
    F: Fn(&VerifiedEnvelopeMeta) -> PreconditionFuture + Send + Sync,
{
    /// Wraps `check` as a precondition.
    pub fn new(check: F) -> Self {
        Self(check)
    }
}

#[async_trait]
impl<F> CheapPrecondition for PreconditionFn<F>
where
    F: Fn(&VerifiedEnvelopeMeta) -> PreconditionFuture + Send + Sync,
{
    async fn check(&self, meta: &VerifiedEnvelopeMeta) -> anyhow::Result<()> {
        (self.0)(meta).await
    }
}

/// One admitted, durably appended operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedAppend {
    /// The facts the materializer reduces from.
    pub meta: VerifiedEnvelopeMeta,
    /// Whether this call created the log entry or found the same bytes already there.
    pub append_outcome: AppendOutcome,
}

/// Every way [`admit_and_append`] declines to produce an admitted operation.
///
/// Each variant is a state in which NOTHING was durably appended: the append call is
/// structurally unreachable until admission returns `Ok`.
#[derive(Debug, thiserror::Error)]
pub enum AdmissionRejection {
    /// §5.1: permanently invalid. Never appended, never retried as-is.
    #[error("candidate rejected: {outcome}")]
    Rejected {
        /// The stable §5 conformance category.
        outcome: AdmissionOutcome,
        /// The original command error, when a caller-supplied precondition produced one.
        detail: Option<String>,
    },
    /// §5.2: retryable availability state. Retained as evidence outside the verified log,
    /// re-evaluated only once the missing parent, schema, or key arrives.
    #[error("candidate quarantined: {outcome}")]
    Quarantined {
        /// The stable §5 conformance category.
        outcome: AdmissionOutcome,
    },
    /// A post-append outcome surfaced during admission; the operation is not appended here.
    #[error("candidate deferred: {outcome}")]
    Deferred {
        /// The stable §5 conformance category.
        outcome: AdmissionOutcome,
    },
    /// The evidence admission relies on could not be read, so no decision was reachable.
    ///
    /// Retryable, and NOT a §5.1 reject: an unreadable equivocation index answers `None`, which
    /// would otherwise admit. Nothing is appended; the caller retries the same immutable bytes
    /// once the source recovers (§3.4).
    #[error("admission evidence was unavailable: {detail}")]
    EvidenceUnavailable {
        /// The faults the evidence sources reported, joined in order.
        detail: String,
    },
    /// §2.3 log-first-strict: the append failed, so the mutation fails and SurrealDB is
    /// unchanged. The caller retries the exact same immutable bytes; it never mints a
    /// replacement operation (§3.4).
    #[error(transparent)]
    AppendFailed(#[from] VerifiedLogStoreError),
}

impl AdmissionRejection {
    /// The §5 category behind this rejection, when admission (not append) produced it.
    pub fn outcome(&self) -> Option<&AdmissionOutcome> {
        match self {
            Self::Rejected { outcome, .. }
            | Self::Quarantined { outcome }
            | Self::Deferred { outcome } => Some(outcome),
            Self::AppendFailed(_) | Self::EvidenceUnavailable { .. } => None,
        }
    }

    fn from_outcome(outcome: AdmissionOutcome, detail: Option<String>) -> Self {
        match outcome.disposition() {
            AdmissionDisposition::Reject => Self::Rejected { outcome, detail },
            AdmissionDisposition::Quarantine => Self::Quarantined { outcome },
            AdmissionDisposition::DeferredMaterialization => Self::Deferred { outcome },
        }
    }
}

/// Runs the §2 pipeline in order and appends only when every check passed.
///
/// Order: envelope verification, SPEC-1 §6.2 same-verse parents, the D-CL25 author-equivocation
/// check, authorization, the caller's cheap precondition, payload verification, then the durable
/// append. The precondition is injected between authorization and payload verification by
/// wrapping the caller's verifier, so the crate-owned `admit_candidate` sequence stays the one
/// implementation of the parent and equivocation rules rather than being copied here.
///
/// `missing_parent`, `unknown_schema`, and `opaque_payload` reach the caller as
/// [`AdmissionRejection::Quarantined`]; §5 note 2 forbids provisionally applying any of them.
///
/// An `Ok` from `admit_candidate` is only trusted when the evidence sources report no fault:
/// see [`EvidenceAvailability`] for why an unreadable equivocation index would otherwise admit.
/// The check guards the `Ok` path only — a genuine §5.1 reject is a decision about the
/// candidate's own bytes and must stay permanent rather than becoming retryable.
pub async fn admit_and_append<V, E, P, C>(
    verifier: &V,
    equivocation_index: &E,
    parent_verse_lookup: &P,
    precondition: &C,
    store: &SurrealVerifiedLogStore,
    candidate_bytes: &[u8],
) -> Result<AdmittedAppend, AdmissionRejection>
where
    V: CandidateVerifier + ?Sized,
    E: EquivocationIndex + EvidenceAvailability + ?Sized,
    P: ParentVerseLookup + EvidenceAvailability + ?Sized,
    C: CheapPrecondition + ?Sized,
{
    let guarded = PreconditionGuardedVerifier {
        inner: verifier,
        precondition,
        failure: Mutex::new(None),
    };

    // Only faults raised while deciding THIS candidate may decide it.
    drop(equivocation_index.take_evidence_faults());
    drop(parent_verse_lookup.take_evidence_faults());

    let meta = match admit_candidate(
        &guarded,
        equivocation_index,
        parent_verse_lookup,
        candidate_bytes,
    )
    .await
    {
        Ok(meta) => meta,
        Err(outcome) => {
            return Err(AdmissionRejection::from_outcome(
                outcome,
                guarded.take_failure(),
            ))
        }
    };

    let mut faults = equivocation_index.take_evidence_faults();
    faults.extend(parent_verse_lookup.take_evidence_faults());
    if !faults.is_empty() {
        return Err(AdmissionRejection::EvidenceUnavailable {
            detail: faults.join("; "),
        });
    }

    let append_outcome = store.append_admitted(&meta, candidate_bytes).await?;
    Ok(AdmittedAppend {
        meta,
        append_outcome,
    })
}

/// Wraps a verifier so the caller's cheap precondition runs immediately after authorization.
struct PreconditionGuardedVerifier<'a, V: ?Sized, P: ?Sized> {
    inner: &'a V,
    precondition: &'a P,
    failure: Mutex<Option<String>>,
}

impl<'a, V: ?Sized, P: ?Sized> PreconditionGuardedVerifier<'a, V, P> {
    fn take_failure(&self) -> Option<String> {
        self.failure
            .lock()
            .expect("precondition failure lock")
            .take()
    }
}

#[async_trait]
impl<'a, V, P> CandidateVerifier for PreconditionGuardedVerifier<'a, V, P>
where
    V: CandidateVerifier + ?Sized + 'a,
    P: CheapPrecondition + ?Sized + 'a,
{
    async fn verify_envelope(
        &self,
        candidate_bytes: &[u8],
    ) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome> {
        self.inner.verify_envelope(candidate_bytes).await
    }

    async fn verify_authorization(
        &self,
        meta: &VerifiedEnvelopeMeta,
    ) -> Result<(), AdmissionOutcome> {
        self.inner.verify_authorization(meta).await?;
        match self.precondition.check(meta).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // §5 `precondition_failed` requires the ORIGINAL command error to reach the
                // caller; the outcome enum carries no payload, so it is kept alongside.
                *self.failure.lock().expect("precondition failure lock") =
                    Some(format!("{error:#}"));
                Err(AdmissionOutcome::PreconditionFailed)
            }
        }
    }

    async fn verify_payload(&self, meta: &VerifiedEnvelopeMeta) -> Result<(), AdmissionOutcome> {
        self.inner.verify_payload(meta).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_canonical_log::envelope::{Hash32, Identifier32, Scope};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_meta() -> VerifiedEnvelopeMeta {
        VerifiedEnvelopeMeta {
            op_id: Hash32([0x01; 32]),
            operation_kind: 100,
            scope: Scope::verse_wide(Identifier32([0x02; 32])),
            branch_id: Identifier32([0x03; 32]),
            schema_hash: Hash32([0x04; 32]),
            parents: Vec::new(),
            author_public_key: [0x05; 32],
            wall_ms: 1_000,
            counter: 0,
        }
    }

    struct CountingVerifier {
        meta: VerifiedEnvelopeMeta,
        payload_calls: AtomicUsize,
    }

    #[async_trait]
    impl CandidateVerifier for CountingVerifier {
        async fn verify_envelope(
            &self,
            _candidate_bytes: &[u8],
        ) -> Result<VerifiedEnvelopeMeta, AdmissionOutcome> {
            Ok(self.meta.clone())
        }

        async fn verify_authorization(
            &self,
            _meta: &VerifiedEnvelopeMeta,
        ) -> Result<(), AdmissionOutcome> {
            Ok(())
        }

        async fn verify_payload(
            &self,
            _meta: &VerifiedEnvelopeMeta,
        ) -> Result<(), AdmissionOutcome> {
            self.payload_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Builds the closure form of `handlers::preconditions::require_node_exists`.
    fn require_node_exists_closure(
        db: crate::repo::Db,
        node_id: &str,
    ) -> impl Fn(&VerifiedEnvelopeMeta) -> PreconditionFuture + Send + Sync {
        let node_id = node_id.to_string();
        move |_meta| {
            let db = db.clone();
            let node_id = node_id.clone();
            Box::pin(async move {
                crate::handlers::preconditions::require_node_exists(
                    &db,
                    &node_id,
                    "CanonicalIntent",
                )
                .await
            })
        }
    }

    struct AlwaysFails;

    #[async_trait]
    impl CheapPrecondition for AlwaysFails {
        async fn check(&self, _meta: &VerifiedEnvelopeMeta) -> anyhow::Result<()> {
            anyhow::bail!("SetNodeProperty matched no node with node_id = missing")
        }
    }

    #[tokio::test]
    async fn a_failed_precondition_stops_the_pipeline_before_payload_verification() {
        let verifier = CountingVerifier {
            meta: sample_meta(),
            payload_calls: AtomicUsize::new(0),
        };
        let guarded = PreconditionGuardedVerifier {
            inner: &verifier,
            precondition: &AlwaysFails,
            failure: Mutex::new(None),
        };

        let outcome = guarded
            .verify_authorization(&sample_meta())
            .await
            .expect_err("precondition must fail admission");
        assert_eq!(outcome, AdmissionOutcome::PreconditionFailed);
        assert!(outcome.is_reject());
        assert_eq!(
            verifier.payload_calls.load(Ordering::SeqCst),
            0,
            "payload verification runs only after the precondition passes"
        );

        let detail = guarded.take_failure().expect("original command error");
        assert!(detail.contains("matched no node"));
        assert_eq!(guarded.take_failure(), None, "the detail is drained once");
    }

    #[tokio::test]
    async fn no_precondition_is_an_explicit_pass() {
        assert!(NoPrecondition.check(&sample_meta()).await.is_ok());
    }

    #[tokio::test]
    async fn a_closure_precondition_can_call_an_existing_handler_helper() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("canon_log")
            .use_db("canon_log")
            .await
            .expect("ns/db");
        crate::schema::apply_all(&db).await.expect("apply schema");

        // The shape the brief requires: an existing `handlers::preconditions` helper passed
        // through unchanged, with no signature adaptation on its side.
        let precondition =
            PreconditionFn::new(require_node_exists_closure(db, "node-that-does-not-exist"));

        let error = precondition
            .check(&sample_meta())
            .await
            .expect_err("absent node must fail the precondition");
        assert!(format!("{error:#}").contains("matched no node"));
    }

    #[test]
    fn rejection_classification_follows_the_spec_five_disposition() {
        let rejected = AdmissionRejection::from_outcome(
            AdmissionOutcome::UnauthorizedOperation,
            Some("denied".to_string()),
        );
        assert!(matches!(rejected, AdmissionRejection::Rejected { .. }));

        let quarantined = AdmissionRejection::from_outcome(
            AdmissionOutcome::MissingParent {
                missing: Hash32([0x09; 32]),
            },
            None,
        );
        assert!(matches!(
            quarantined,
            AdmissionRejection::Quarantined { .. }
        ));
        assert_eq!(
            quarantined.outcome(),
            Some(&AdmissionOutcome::MissingParent {
                missing: Hash32([0x09; 32])
            })
        );

        let deferred =
            AdmissionRejection::from_outcome(AdmissionOutcome::MaterializationFailed, None);
        assert!(matches!(deferred, AdmissionRejection::Deferred { .. }));
    }
}
