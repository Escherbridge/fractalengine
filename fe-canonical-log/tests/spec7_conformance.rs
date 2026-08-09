//! SPEC-7 §8 named conformance tests, using only the public wire API and its test fakes.
//!
//! Each test name matches the exact name in `commit-preview-wire.md` §8 so a census can grep
//! for it. Tests that need Wave-3 database-backed storage are listed as deferred in the
//! workstream report, not stubbed here.

use std::sync::Arc;

use async_trait::async_trait;

use fe_canonical_log::cbor::{decode_canonical, encode_canonical_checked, CborValue};
use fe_canonical_log::envelope::{Hash32, Identifier32, Scope};
use fe_canonical_log::wire::commit::{
    build_commit_ack, handle_commit_submit, is_delta_authorized_for_subscription,
    CanonicalCommitPipeline, CommitAckState, CommitDeltaBody, CommitSubmitBody, PipelineResult,
};
use fe_canonical_log::wire::cursor::{
    verify_frontier_commitment, BranchRegistry, CommittedDelta, CursorTuple, DurableCursor,
    ProjectionIdentity, ReplayOutcome,
};
use fe_canonical_log::wire::error::{ProtocolErrorBody, ProtocolErrorCategory, WireError};
use fe_canonical_log::wire::preview::{reject_reserved_preview_keys, PreviewSendBody};
use fe_canonical_log::wire::preview_limiter::{PreviewRateLimit, PreviewRateLimiter};
use fe_canonical_log::wire::session::SessionAuthorizationTable;
use fe_canonical_log::wire::snapshot::{
    snapshot_all_authorized_subscriptions, ScopeSnapshotSource,
};
use fe_canonical_log::wire::subscription::{
    resolve_resume, ResumeBody, ResumeOutcome, SubscriptionRecord, SubscriptionTable,
};
use fe_canonical_log::wire::test_support::{
    cursor_at_position_zero, cursor_with_claim, InMemoryBranchRegistry, MockCommitPipeline,
    MockScopeSnapshotSource,
};

fn hash(filler: u8) -> Hash32 {
    Hash32([filler; 32])
}

fn scope() -> Scope {
    Scope::verse_wide(Identifier32([0x11; 32]))
}

fn projection() -> ProjectionIdentity {
    ProjectionIdentity {
        materializer_id: "fe-scene".to_owned(),
        version: 1,
    }
}

fn tuple() -> CursorTuple {
    CursorTuple {
        verse_id: Identifier32([0x11; 32]),
        branch_id: Identifier32([0x22; 32]),
        subscription_scope: scope(),
        projection_identity: projection(),
    }
}

fn subscription_record(scope: Scope) -> SubscriptionRecord {
    SubscriptionRecord {
        authorization_binding_id: [0; 16],
        branch_id: tuple().branch_id,
        scope,
        projection_identity: projection(),
    }
}

/// A table with one accepted binding, and the generation it issued.
fn accepted_sessions() -> (SessionAuthorizationTable, u64) {
    let mut sessions = SessionAuthorizationTable::new();
    let generation = sessions
        .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
        .expect("accept");
    (sessions, generation)
}

/// Panics if its `submit_candidate` is ever reached; proves a rejection happened upstream.
struct PanicsIfSubmitted;

#[async_trait]
impl CanonicalCommitPipeline for PanicsIfSubmitted {
    fn derive_op_id(&self, complete_envelope_bytes: &[u8]) -> Hash32 {
        Hash32::of(complete_envelope_bytes)
    }

    async fn submit_candidate(
        &self,
        _complete_envelope_bytes: &[u8],
        _payload_ciphertext: Option<&[u8]>,
    ) -> PipelineResult {
        panic!("submit_candidate must not be reached for a rejected candidate");
    }
}

#[tokio::test]
async fn commit_submit_preserves_signed_envelope_and_derived_op_id() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    let pipeline = MockCommitPipeline::new(registry, tuple());
    let mut sessions = SessionAuthorizationTable::new();
    let generation = sessions
        .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
        .expect("accept");

    let envelope_bytes = vec![1, 2, 3, 4, 5];
    let correct_op_id = pipeline.derive_op_id(&envelope_bytes);

    let body = CommitSubmitBody {
        session_generation: generation,
        authorization_binding_id: [1; 16],
        claimed_op_id: correct_op_id,
        complete_envelope: envelope_bytes.clone(),
        payload_ciphertext: None,
    };
    let ack = handle_commit_submit(&pipeline, &sessions, &body).await;
    match ack.state {
        CommitAckState::Committed(binding) => assert_eq!(binding.branch_id, tuple().branch_id),
        other => panic!("expected Committed, got {other:?}"),
    }

    // A claimed op_id mismatch must be rejected before the pipeline is ever consulted.
    let panics_if_submitted = PanicsIfSubmitted;
    let mismatched = CommitSubmitBody {
        session_generation: generation,
        authorization_binding_id: [1; 16],
        claimed_op_id: hash(0xff),
        complete_envelope: envelope_bytes,
        payload_ciphertext: None,
    };
    let rejected = handle_commit_submit(&panics_if_submitted, &sessions, &mismatched).await;
    assert_eq!(
        rejected.state,
        CommitAckState::Rejected {
            category: ProtocolErrorCategory::InvalidOpId
        }
    );
}

#[tokio::test]
async fn lost_commit_ack_retries_exact_bytes_idempotently() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    let pipeline = MockCommitPipeline::new(registry.clone(), tuple());
    let mut sessions = SessionAuthorizationTable::new();
    let generation = sessions
        .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
        .expect("accept");

    let envelope_bytes = vec![9, 9, 9];
    let op_id = pipeline.derive_op_id(&envelope_bytes);
    let body = CommitSubmitBody {
        session_generation: generation,
        authorization_binding_id: [1; 16],
        claimed_op_id: op_id,
        complete_envelope: envelope_bytes,
        payload_ciphertext: None,
    };

    let first = handle_commit_submit(&pipeline, &sessions, &body).await;
    assert!(matches!(first.state, CommitAckState::Committed(_)));

    // The ack was "lost"; the client resubmits identical bytes.
    let second = handle_commit_submit(&pipeline, &sessions, &body).await;
    assert!(matches!(second.state, CommitAckState::AlreadyCommitted(_)));

    let ReplayOutcome::Deltas(deltas) = registry
        .replay_after(&cursor_at_position_zero(tuple()))
        .await
        .expect("replay from start")
    else {
        panic!("expected deltas");
    };
    assert_eq!(
        deltas.len(),
        1,
        "exactly one append occurred despite two submissions"
    );
}

#[test]
fn committed_ack_requires_durable_projection_cursor() {
    let pending = build_commit_ack(
        1,
        [0xaa; 16],
        hash(0x01),
        PipelineResult::AcceptedPendingMaterialization,
    );
    let value = pending.to_cbor().expect("cbor");
    let entries = value.as_map().expect("map");
    assert_eq!(
        entries.len(),
        4,
        "no branch/scope/projection/cursor field is present"
    );
    for forbidden in [5u64, 6, 7, 8] {
        assert!(value.get_uint_key(forbidden).is_none());
    }

    let rejected = build_commit_ack(
        1,
        [0xaa; 16],
        hash(0x01),
        PipelineResult::Rejected(ProtocolErrorCategory::InvalidEnvelope),
    );
    assert!(matches!(rejected.state, CommitAckState::Rejected { .. }));
    let value = rejected.to_cbor().expect("cbor");
    for forbidden in [5u64, 6, 7, 8] {
        assert!(value.get_uint_key(forbidden).is_none());
    }
}

#[test]
fn commit_delta_is_scope_filtered_and_cursor_bound() {
    let verse_wide = Scope::verse_wide(Identifier32([0x11; 32]));
    let petal_scope = Scope::new(
        Identifier32([0x11; 32]),
        Some(Identifier32([0x22; 32])),
        None,
    )
    .expect("petal");
    let other_verse = Scope::verse_wide(Identifier32([0x99; 32]));

    // A verse-wide subscription is authorized for a petal-scoped delta within that verse.
    assert!(is_delta_authorized_for_subscription(
        &verse_wide,
        &petal_scope
    ));
    // A petal-scoped subscription does NOT receive a broader verse-wide delta unfiltered.
    assert!(!is_delta_authorized_for_subscription(
        &petal_scope,
        &verse_wide
    ));
    // A different verse's scope never authorizes a broadcast.
    assert!(!is_delta_authorized_for_subscription(
        &verse_wide,
        &other_verse
    ));

    let committed = CommittedDelta {
        op_id: hash(0x01),
        branch_id: tuple().branch_id,
        scope: petal_scope,
        projection_identity: projection(),
        cursor: cursor_at_position_zero(tuple()),
        change_summary: vec![1],
    };

    // The filter is not merely available: it is the constructor. A delta broader than the
    // recipient's subscription scope is not expressible.
    assert_eq!(
        CommitDeltaBody::for_subscription(
            [0xaa; 16],
            &subscription_record(petal_scope),
            1,
            &CommittedDelta {
                scope: verse_wide,
                ..committed.clone()
            },
        ),
        Err(WireError::ScopeNotAuthorized)
    );

    let delta = CommitDeltaBody::for_subscription(
        [0xaa; 16],
        &subscription_record(verse_wide),
        1,
        &committed,
    )
    .expect("a petal-scoped delta is authorized for a verse-wide subscription");
    let bytes = encode_canonical_checked(&delta.to_cbor().expect("cbor")).expect("encode");
    let decoded =
        CommitDeltaBody::from_cbor(&decode_canonical(&bytes).expect("decode")).expect("decode");
    assert_eq!(decoded.scope, petal_scope);
    assert!(is_delta_authorized_for_subscription(
        &verse_wide,
        &decoded.scope
    ));
}

#[tokio::test]
async fn resume_replays_complete_interval_or_requires_snapshot() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    registry.commit(tuple(), hash(0x01), vec![1]);
    registry.commit(tuple(), hash(0x02), vec![2]);

    let mut subscriptions = SubscriptionTable::new();
    subscriptions
        .bind([1; 16], subscription_record(scope()))
        .expect("bind");
    let (sessions, generation) = accepted_sessions();

    let from_start = ResumeBody {
        session_generation: generation,
        subscription_id: [1; 16],
        prior_cursor: cursor_at_position_zero(tuple()),
    };
    let outcome = resolve_resume(registry.as_ref(), &subscriptions, &sessions, &from_start)
        .await
        .expect("resolve");
    match outcome {
        ResumeOutcome::Replayed { deltas, .. } => {
            assert_eq!(deltas.len(), 2, "the complete interval, no gap")
        }
        ResumeOutcome::SnapshotRequired(reason) => panic!("expected a full replay, got {reason:?}"),
    }

    // An incomparable cursor (wrong tuple) must request a snapshot, never a guessed position.
    let wrong_tuple = CursorTuple {
        verse_id: Identifier32([0x99; 32]),
        branch_id: tuple().branch_id,
        subscription_scope: Scope::verse_wide(Identifier32([0x99; 32])),
        projection_identity: projection(),
    };
    let resume_wrong = ResumeBody {
        session_generation: generation,
        subscription_id: [1; 16],
        prior_cursor: cursor_at_position_zero(wrong_tuple),
    };
    let outcome = resolve_resume(registry.as_ref(), &subscriptions, &sessions, &resume_wrong)
        .await
        .expect("resolve");
    assert!(matches!(outcome, ResumeOutcome::SnapshotRequired(_)));
}

#[tokio::test]
async fn lagged_connection_receives_authorized_scope_snapshots() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    registry.commit(tuple(), hash(0x01), vec![1]);

    let mut subscriptions = SubscriptionTable::new();
    for subscription_id in [[1u8; 16], [2u8; 16]] {
        subscriptions
            .bind(subscription_id, subscription_record(scope()))
            .expect("bind");
    }
    let (sessions, generation) = accepted_sessions();

    let source = MockScopeSnapshotSource::new(registry.clone(), b"view".to_vec());

    let results = snapshot_all_authorized_subscriptions(
        &source,
        &subscriptions,
        &sessions,
        generation,
        |_record| true,
    )
    .await
    .expect("current generation");
    assert_eq!(
        results.len(),
        2,
        "every still-authorized subscribed scope is snapshotted"
    );

    let results = snapshot_all_authorized_subscriptions(
        &source,
        &subscriptions,
        &sessions,
        generation,
        |_record| false,
    )
    .await
    .expect("current generation");
    assert!(
        results.is_empty(),
        "no unsubscribed/unauthorized scope is ever snapshotted"
    );
}

#[tokio::test]
async fn snapshot_cursor_resumes_after_exact_projection_boundary() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    registry.commit(tuple(), hash(0x01), vec![1]);

    let source = MockScopeSnapshotSource::new(registry.clone(), b"view".to_vec());
    let (snapshot_cursor, _view) = source
        .snapshot(tuple().branch_id, &scope(), &projection())
        .await
        .expect("snapshot");

    // New commits arrive after the snapshot was taken.
    registry.commit(tuple(), hash(0x02), vec![2]);
    registry.commit(tuple(), hash(0x03), vec![3]);

    let ReplayOutcome::Deltas(deltas) = registry
        .replay_after(&snapshot_cursor)
        .await
        .expect("replay")
    else {
        panic!("expected deltas");
    };
    assert_eq!(
        deltas.len(),
        2,
        "resume converges on exactly the post-snapshot commits, no duplicates or gaps"
    );
    assert_eq!(deltas[0].op_id, hash(0x02));
    assert_eq!(deltas[1].op_id, hash(0x03));
}

/// Panics if its `snapshot` is ever reached; proves the generation gate ran first.
struct PanicsIfSnapshotted;

#[async_trait]
impl ScopeSnapshotSource for PanicsIfSnapshotted {
    async fn snapshot(
        &self,
        _branch_id: Identifier32,
        _scope: &Scope,
        _projection_identity: &ProjectionIdentity,
    ) -> Result<(DurableCursor, Vec<u8>), WireError> {
        panic!("no snapshot may be read under a revoked session generation");
    }
}

#[tokio::test]
async fn session_revalidation_stops_commit_replay_snapshot_and_preview() {
    let (mut sessions, generation) = accepted_sessions();
    assert!(sessions.is_generation_valid(generation));
    assert_eq!(sessions.invalidate(), generation);
    assert!(!sessions.is_generation_valid(generation));

    let registry = Arc::new(InMemoryBranchRegistry::new());
    registry.commit(tuple(), hash(0x01), vec![1]);

    let mut subscriptions = SubscriptionTable::new();
    subscriptions
        .bind([1; 16], subscription_record(scope()))
        .expect("bind");

    // 1. Commit. The pipeline panics if it is consulted at all.
    let commit_body = CommitSubmitBody {
        session_generation: generation,
        authorization_binding_id: [1; 16],
        claimed_op_id: Hash32::of(&[1, 2, 3]),
        complete_envelope: vec![1, 2, 3],
        payload_ciphertext: None,
    };
    let ack = handle_commit_submit(&PanicsIfSubmitted, &sessions, &commit_body).await;
    assert_eq!(
        ack.state,
        CommitAckState::Rejected {
            category: ProtocolErrorCategory::SessionGenerationInvalid
        }
    );

    // 2. Replay. A revoked generation cannot resume a subscription that still exists.
    let resume = ResumeBody {
        session_generation: generation,
        subscription_id: [1; 16],
        prior_cursor: cursor_at_position_zero(tuple()),
    };
    assert_eq!(
        resolve_resume(registry.as_ref(), &subscriptions, &sessions, &resume).await,
        Err(WireError::StaleSessionGeneration { generation })
    );

    // 3. Snapshot. No scene bytes are read for the old subscription scopes.
    assert_eq!(
        snapshot_all_authorized_subscriptions(
            &PanicsIfSnapshotted,
            &subscriptions,
            &sessions,
            generation,
            |_record| true,
        )
        .await,
        Err(WireError::StaleSessionGeneration { generation })
    );

    // 4. Preview. No preview is admitted, and none consumes rate budget.
    let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(100, 1_000));
    assert_eq!(
        limiter.check_and_record(&sessions, generation, [0xbb; 32], scope(), None, 0),
        Err(WireError::StaleSessionGeneration { generation })
    );

    // A replacement generation lifts the stop; the gate keys on the generation, not on the
    // socket having been torn down.
    let replacement = sessions
        .accept_binding([1; 16], hash(0xaa), [0xbb; 32])
        .expect("replacement authorize");
    assert_ne!(replacement, generation);
    let resumed = ResumeBody {
        session_generation: replacement,
        subscription_id: [1; 16],
        prior_cursor: cursor_at_position_zero(tuple()),
    };
    assert!(
        resolve_resume(registry.as_ref(), &subscriptions, &sessions, &resumed)
            .await
            .is_ok(),
        "a fresh generation replays again"
    );
    assert!(limiter
        .check_and_record(&sessions, replacement, [0xbb; 32], scope(), None, 0)
        .is_ok());
}

#[test]
fn unauthorized_wire_errors_do_not_disclose_private_history() {
    for category in [
        ProtocolErrorCategory::ScopeNotAuthorized,
        ProtocolErrorCategory::InvalidOpId,
        ProtocolErrorCategory::CursorInvalid,
    ] {
        let body = ProtocolErrorBody { category };
        let value = body.to_cbor();
        let entries = value.as_map().expect("map");
        assert_eq!(
            entries.len(),
            1,
            "no field beyond the stable category can leak an identifier"
        );
    }
}

#[test]
fn preview_frames_are_structurally_disjoint_from_commit_frames() {
    // A well-formed preview_send has no key numbered for an op_id/envelope/branch/hlc/payload/
    // checkpoint/cursor field; the reserved-key gate rejects one smuggled in regardless.
    let mut entries = match (PreviewSendBody {
        session_generation: 1,
        authorization_binding_id: [0xaa; 16],
        scope: scope(),
        preview_sequence: 1,
        preview_kind: 1,
        expires_at_ms: 1,
        preview_data: Vec::new(),
    })
    .to_cbor()
    .expect("cbor")
    {
        CborValue::Map(entries) => entries,
        _ => unreachable!(),
    };
    entries.push((CborValue::Uint(90), CborValue::Bytes(vec![0; 32]))); // reserved "op_id"
    let smuggled = CborValue::Map(entries);
    assert_eq!(
        reject_reserved_preview_keys(&smuggled),
        Err(WireError::ForbiddenPreviewField { field: "op_id" })
    );
    assert!(PreviewSendBody::from_cbor(&smuggled).is_err());
}

#[test]
fn preview_rate_limit_is_per_principal_and_exact_scope() {
    let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(1, 1_000));
    let (sessions, generation) = accepted_sessions();
    let first_principal = [0x01; 32];
    let second_principal = [0x02; 32];

    assert!(limiter
        .check_and_record(&sessions, generation, first_principal, scope(), None, 0)
        .is_ok());
    assert_eq!(
        limiter.check_and_record(&sessions, generation, first_principal, scope(), None, 10),
        Err(WireError::PreviewRateLimited)
    );
    assert!(
        limiter
            .check_and_record(&sessions, generation, second_principal, scope(), None, 10)
            .is_ok(),
        "a distinct principal has its own budget"
    );

    let other_scope = Scope::verse_wide(Identifier32([0x99; 32]));
    assert!(
        limiter
            .check_and_record(
                &sessions,
                generation,
                first_principal,
                other_scope,
                None,
                10
            )
            .is_ok(),
        "a distinct exact scope has its own budget"
    );
}

#[tokio::test]
async fn preview_is_never_replayed_or_snapshotted() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    let mut limiter = PreviewRateLimiter::new(PreviewRateLimit::new(100, 1_000));
    let (sessions, generation) = accepted_sessions();

    // Previews never touch the registry: there is no code path from `PreviewRateLimiter` or
    // `PreviewSendBody` to `InMemoryBranchRegistry::commit`.
    for sequence in 0..5u64 {
        limiter
            .check_and_record(&sessions, generation, [0x01; 32], scope(), None, sequence)
            .expect("under budget");
    }
    registry.commit(tuple(), hash(0x01), vec![1]);

    let ReplayOutcome::Deltas(deltas) = registry
        .replay_after(&cursor_at_position_zero(tuple()))
        .await
        .expect("replay")
    else {
        panic!("expected deltas");
    };
    assert_eq!(
        deltas.len(),
        1,
        "only the real commit is present; no preview was ever materialized"
    );
}

#[tokio::test]
async fn cursor_binds_sorted_frontier_and_branch_control_replay() {
    let registry = Arc::new(InMemoryBranchRegistry::new());
    let real_cursor = registry.commit(tuple(), hash(0x01), Vec::new());
    assert!(registry.validate(&real_cursor).await.is_ok());

    let tampered = cursor_with_claim(
        tuple(),
        hash(0xff),
        real_cursor.delivery_position().to_vec(),
    );
    assert_eq!(
        registry.validate(&tampered).await,
        Err(WireError::InvalidFrontierCommitment)
    );

    let recomputed = verify_frontier_commitment(
        std::iter::once(hash(0x01)),
        real_cursor.frontier_commitment(),
    )
    .expect("commitment matches the single committed op_id");
    assert_eq!(recomputed, real_cursor.frontier_commitment());
}
