//! §2-§7 connection lifecycle for `/ws/canonical` (unmounted by design; see `AGENTS.md`).
//!
//! Dispatch is split into two structurally separate halves at this module's top level: the
//! commit-class functions below (`authorize`, `commit_submit`, `subscribe`, `resume`,
//! `snapshot_ack`) close over the full [`CanonicalLogState`], while [`preview_task`] is a
//! nested module whose own `use` block never names `wire::commit`, `wire::cursor`,
//! `wire::snapshot`, or `wire::subscription` — see `AGENTS.md` §preview-disjointness.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};

use fe_canonical_log::capability::{CacheKey, PinnedSession, SessionValidity};
use fe_canonical_log::cbor::CborValue;
use fe_canonical_log::envelope::Hash32;
use fe_canonical_log::wire::commit::{
    build_commit_ack, handle_commit_submit, is_delta_authorized_for_subscription, CommitAckBody,
    CommitDeltaBody, CommitSubmitBody, PipelineResult,
};
use fe_canonical_log::wire::cursor::{
    verify_frontier_commitment, CommittedDelta, DurableCursor, SnapshotReason,
};
use fe_canonical_log::wire::error::{ProtocolErrorBody, ProtocolErrorCategory, WireError};
use fe_canonical_log::wire::frame::{decode_frame, encode_frame, Direction, Frame, MessageType};
use fe_canonical_log::wire::session::{
    AuthorizationRevalidationRequiredBody, AuthorizeBody, AuthorizedBody, RevalidationReason,
    SessionAuthorizationTable,
};
use fe_canonical_log::wire::snapshot::{
    snapshot_all_authorized_subscriptions, SnapshotAckBody, SnapshotDispatchOutcome,
};
use fe_canonical_log::wire::subscription::{
    resolve_resume, ResumeBody, ResumeOutcome, SubscribeBody, SubscriptionRecord, SubscriptionTable,
};

use crate::canonical_ws::state::CanonicalLogState;
use preview_task::PreviewTaskState;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// axum entrypoint: upgrades to a WebSocket and hands off to [`handle_socket`].
pub async fn canonical_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<CanonicalLogState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

// ---------------------------------------------------------------------------
// Per-connection status shared between the commit-class task and the preview task
// ---------------------------------------------------------------------------

/// What this connection currently believes about its pinned capability session (§5.3 rule 3).
/// Read by both the commit-class dispatch below and the structurally separate preview task;
/// only commit-class dispatch (via `authorize` and the revalidation timer) ever writes it.
///
/// `large_enum_variant` is allowed rather than satisfied: `Valid` is ~280 bytes, but exactly
/// one of these exists per connection, it lives behind an `RwLock`, it is written only on
/// `authorize` and the revalidation tick, and every reader matches it by reference. Boxing
/// would add an allocation and an indirection to the per-frame dispatch path to save ~264
/// bytes per connection. That is the opposite of the trade the lint exists to prompt — unlike
/// `SegmentError`/`CapabilityError`, which are boxed because a `Result` is returned by value
/// from every call.
#[allow(clippy::large_enum_variant)]
pub(crate) enum PinnedSessionStatus {
    /// No `authorize` has been accepted yet on this connection.
    Unauthorized,
    /// A pinned session is current, alongside the binding ID it was accepted under (needed to
    /// name it in a later `authorization_revalidation_required` frame).
    Valid {
        session: PinnedSession,
        authorization_binding_id: [u8; 16],
    },
    /// A pinned session was valid but the timer-based re-check (§5.3 rule 3) found it no
    /// longer `Valid`; every protected path must refuse until a fresh `authorize`.
    Invalidated,
}

// ---------------------------------------------------------------------------
// Commit-class connection state
// ---------------------------------------------------------------------------

/// Per-subscription record of every `op_id` this connection has independently witnessed since
/// its most recent `snapshot_ack`, used to corroborate a peer-supplied `resume` cursor before
/// it selects a replay range. A connection-scoped ledger, not a full historical
/// reconstruction, is what fe-api can honestly verify without owning durable frontier storage
/// — see `AGENTS.md` §wave-3-obligation-frontier-commitment.
#[derive(Default)]
struct SubscriptionFrontierLedger {
    /// The exact cursor this connection was told to treat as its replay base.
    trusted_baseline: Option<DurableCursor>,
    /// Every `op_id` shown to this connection for the subscription since `trusted_baseline`.
    observed_op_ids: BTreeSet<Hash32>,
}

impl SubscriptionFrontierLedger {
    fn reset_to_snapshot(&mut self, cursor: DurableCursor) {
        self.trusted_baseline = Some(cursor);
        self.observed_op_ids.clear();
    }

    fn record_delivered(&mut self, op_id: Hash32) {
        self.observed_op_ids.insert(op_id);
    }
}

/// Gate: `wire::cursor::verify_frontier_commitment` MUST be called on every peer-supplied
/// resume cursor before it selects a replay range (`wire/AGENTS.md` §"Wave 3 obligation").
/// Rejects a cursor this connection cannot corroborate rather than trusting it; the exact
/// cursor this connection last minted for the client (`trusted_baseline`) needs no
/// recomputation since it did not travel through the client's own hands unverified.
fn verify_resume_cursor_frontier(
    ledger: Option<&SubscriptionFrontierLedger>,
    prior_cursor: &DurableCursor,
) -> Result<(), WireError> {
    let ledger = ledger.ok_or(WireError::CursorInvalid)?;
    if ledger.trusted_baseline.as_ref() == Some(prior_cursor) {
        return Ok(());
    }
    verify_frontier_commitment(
        ledger.observed_op_ids.iter().copied(),
        prior_cursor.frontier_commitment(),
    )
    .map(|_| ())
}

/// Everything `authorize`, `commit_submit`, `subscribe`, `resume`, and `snapshot_ack` read or
/// mutate. `sessions` and `pinned` are `Arc`-shared with the preview task so both halves
/// enforce the same session-generation and pin gates against one source of truth.
struct ConnectionAuthState {
    sessions: Arc<RwLock<SessionAuthorizationTable>>,
    pinned: Arc<RwLock<PinnedSessionStatus>>,
    subscriptions: SubscriptionTable,
    frontier_ledgers: HashMap<[u8; 16], SubscriptionFrontierLedger>,
}

// ---------------------------------------------------------------------------
// Commit-class dispatch (pure enough to unit test without a socket)
// ---------------------------------------------------------------------------

/// Handles one `authorize` (§2.2.7). Consults the §5.3 verification cache
/// (`capability/AGENTS.md` §5.3 obligation 1) before calling the potentially expensive
/// [`CapabilityVerifier`](fe_canonical_log::wire::session::CapabilityVerifier), then accepts a
/// fresh session generation and pins the session (§5.3 rule 3).
async fn dispatch_authorize(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    body: &AuthorizeBody,
) -> Result<AuthorizedBody, ProtocolErrorCategory> {
    let authorization = match canonical.cached_verification(&body.capability_chain_bytes) {
        Some(cached) => cached,
        None => {
            let verified = canonical
                .capability_verifier
                .verify(
                    &body.capability_chain_bytes,
                    body.requested_verb,
                    body.requested_object_class,
                    &body.requested_scope,
                )
                .await
                .map_err(|_| ProtocolErrorCategory::ScopeNotAuthorized)?;
            let cache_key = CacheKey {
                chain_id: verified.chain_id,
                epoch_scope: verified.epoch_scope,
                epoch: verified.scope_epoch,
                expiry_ms: verified.expires_at_ms,
                authority_view_version: canonical.authorization_view.version(),
            };
            canonical.admit_verification(&body.capability_chain_bytes, verified.clone(), cache_key);
            verified
        }
    };

    let generation = {
        let mut sessions = auth.sessions.write().await;
        sessions
            .accept_binding(
                body.authorization_binding_id,
                authorization.chain_id,
                authorization.leaf_principal.public_key,
            )
            .map_err(|_| ProtocolErrorCategory::ScopeNotAuthorized)?
    };

    // NOTE: `wire::session::VerifiedAuthorization` does not carry a `leaf_certificate_id`
    // distinct from `chain_id` (unlike `capability::chain::VerifiedCapability`, which does).
    // Until that wire-local type is widened, `chain_id` stands in for it here — see
    // `AGENTS.md` §known-limitations.
    let pinned_session = PinnedSession {
        leaf_principal: authorization.leaf_principal.clone(),
        chain_id: authorization.chain_id,
        leaf_certificate_id: authorization.chain_id,
        epoch_scope: authorization.epoch_scope,
        epoch: authorization.scope_epoch,
        expires_at_ms: authorization.expires_at_ms,
        subscribed_scopes: Vec::new(),
    };
    *auth.pinned.write().await = PinnedSessionStatus::Valid {
        session: pinned_session,
        authorization_binding_id: body.authorization_binding_id,
    };

    Ok(AuthorizedBody {
        authorization_binding_id: body.authorization_binding_id,
        session_generation: generation,
        leaf_principal: authorization.leaf_principal,
        chain_id: authorization.chain_id,
        epoch_scope: authorization.epoch_scope,
        scope_epoch: authorization.scope_epoch,
        expires_at_ms: authorization.expires_at_ms,
    })
}

/// Handles one `commit_submit` (§4.1). [`handle_commit_submit`] itself stops a stale session
/// generation before any pipeline work; this wrapper additionally refuses an unpinned or
/// invalidated session (`capability/AGENTS.md` §5.3 obligation 3) before even that check runs.
async fn dispatch_commit_submit(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    body: &CommitSubmitBody,
) -> CommitAckBody {
    let pinned_ok = matches!(
        &*auth.pinned.read().await,
        PinnedSessionStatus::Valid { .. }
    );
    if !pinned_ok {
        return build_commit_ack(
            body.session_generation,
            body.authorization_binding_id,
            body.claimed_op_id,
            PipelineResult::Rejected(ProtocolErrorCategory::AuthorizationRevalidationRequired),
        );
    }
    let sessions = auth.sessions.read().await;
    handle_commit_submit(canonical.commit_pipeline.as_ref(), &sessions, body).await
}

/// Handles one `subscribe` (§5.1.1): binds the subscription, seeds its frontier ledger, and
/// records the scope for later `PinnedSession::covers` checks (obligation 3).
async fn dispatch_subscribe(
    auth: &mut ConnectionAuthState,
    body: &SubscribeBody,
) -> Result<(), ProtocolErrorCategory> {
    {
        let sessions = auth.sessions.read().await;
        if !sessions.is_generation_valid(body.session_generation) {
            return Err(ProtocolErrorCategory::SessionGenerationInvalid);
        }
    }
    let record = SubscriptionRecord {
        authorization_binding_id: body.authorization_binding_id,
        branch_id: body.branch_id,
        scope: body.scope,
        projection_identity: body.projection_identity.clone(),
    };
    auth.subscriptions
        .bind(body.subscription_id, record)
        .map_err(|_| ProtocolErrorCategory::ScopeNotAuthorized)?;
    auth.frontier_ledgers
        .entry(body.subscription_id)
        .or_default();

    let mut pinned = auth.pinned.write().await;
    if let PinnedSessionStatus::Valid { session, .. } = &mut *pinned {
        if !session.subscribed_scopes.contains(&body.scope) {
            session.subscribed_scopes.push(body.scope);
        }
    }
    Ok(())
}

/// Handles one `resume` (§5.1.3-5.1.6). Verifies the peer-supplied cursor's frontier
/// commitment (gate: `wire::cursor::verify_frontier_commitment`, `wire/AGENTS.md` §"Wave 3
/// obligation") before the cursor is ever handed to the branch registry to select a replay
/// range, and enforces `PinnedSession::covers` (`capability/AGENTS.md` §5.3 obligation 3)
/// before that.
async fn dispatch_resume(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    body: &ResumeBody,
) -> Result<ResumeOutcome, ProtocolErrorCategory> {
    {
        let pinned = auth.pinned.read().await;
        let scope = auth
            .subscriptions
            .get(body.subscription_id)
            .map(|record| record.scope);
        let covered = match (&*pinned, scope) {
            (PinnedSessionStatus::Valid { session, .. }, Some(scope)) => session.covers(&scope),
            _ => false,
        };
        if !covered {
            return Err(ProtocolErrorCategory::AuthorizationRevalidationRequired);
        }
    }

    let ledger = auth.frontier_ledgers.get(&body.subscription_id);
    if verify_resume_cursor_frontier(ledger, &body.prior_cursor).is_err() {
        return Ok(ResumeOutcome::SnapshotRequired(
            fe_canonical_log::wire::snapshot::SnapshotRequiredBody {
                subscription_id: body.subscription_id,
                session_generation: body.session_generation,
                reason: SnapshotReason::CursorInvalid,
            },
        ));
    }

    let sessions = auth.sessions.read().await;
    resolve_resume(
        canonical.branch_registry.as_ref(),
        &auth.subscriptions,
        &sessions,
        body,
    )
    .await
    .map_err(|_| ProtocolErrorCategory::CursorInvalid)
}

/// Handles one `snapshot_ack` (§5.2.5): establishes the snapshot cursor as this connection's
/// trusted frontier baseline for later `resume` corroboration (gate obligation above).
fn dispatch_snapshot_ack(auth: &mut ConnectionAuthState, body: &SnapshotAckBody) {
    auth.frontier_ledgers
        .entry(body.subscription_id)
        .or_default()
        .reset_to_snapshot(body.snapshot_cursor.clone());
}

/// Fresh-subscribe and D-CL15 lag-recovery snapshot fan-out (§5.2), reusing the existing
/// `snapshot_all_authorized_subscriptions` helper — which already stops a stale session
/// generation before any snapshot source read — and additionally gating each scope on
/// `PinnedSession::covers` (obligation 3).
async fn dispatch_snapshot_fanout(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    session_generation: u64,
) -> Result<Vec<([u8; 16], SnapshotDispatchOutcome)>, WireError> {
    let sessions = auth.sessions.read().await;
    let pinned = auth.pinned.read().await;
    snapshot_all_authorized_subscriptions(
        canonical.snapshot_source.as_ref(),
        &auth.subscriptions,
        &sessions,
        session_generation,
        |record| match &*pinned {
            PinnedSessionStatus::Valid { session, .. } => session.covers(&record.scope),
            _ => false,
        },
    )
    .await
}

/// Builds the delta one subscription is authorized to receive (§4.3 rule 3, via
/// [`CommitDeltaBody::for_subscription`]) and records its `op_id` into that subscription's
/// frontier ledger. `None` when the subscription is unknown or the delta is out of its scope.
fn build_delta_for_forwarding(
    auth: &mut ConnectionAuthState,
    subscription_id: [u8; 16],
    session_generation: u64,
    delta: &CommittedDelta,
) -> Option<CommitDeltaBody> {
    let record = auth.subscriptions.get(subscription_id)?.clone();
    let body =
        CommitDeltaBody::for_subscription(subscription_id, &record, session_generation, delta)
            .ok()?;
    auth.frontier_ledgers
        .entry(subscription_id)
        .or_default()
        .record_delivered(delta.op_id);
    Some(body)
}

// ---------------------------------------------------------------------------
// Preview dispatch — a structurally separate task
// ---------------------------------------------------------------------------

/// Preview dispatch (§7). This submodule's own `use` block never imports `wire::commit`,
/// `wire::cursor`, `wire::snapshot`, or `wire::subscription`, so no function in it can name
/// [`CanonicalCommitPipeline`](fe_canonical_log::wire::commit::CanonicalCommitPipeline) or
/// [`BranchRegistry`](fe_canonical_log::wire::cursor::BranchRegistry), let alone call verified
/// append, materialization, projection persistence, segment sealing, durable replay, or commit
/// fanout. See `AGENTS.md` §preview-disjointness. It is spawned as its own `tokio` task by
/// `handle_socket`, so its captured environment is exactly [`PreviewTaskState`] plus its inbox
/// — nothing from [`super::CanonicalLogState`] or [`super::ConnectionAuthState`].
pub mod preview_task {
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tokio::sync::{broadcast, mpsc, oneshot, Mutex, RwLock};

    use fe_canonical_log::wire::error::WireError;
    use fe_canonical_log::wire::preview::{PreviewDeltaBody, PreviewSendBody};
    use fe_canonical_log::wire::preview_limiter::PreviewRateLimiter;
    use fe_canonical_log::wire::session::SessionAuthorizationTable;

    use super::PinnedSessionStatus;

    /// The only state a preview dispatch task may capture. Its fields are exhaustively the
    /// shared session table (for the §6 rule 3 generation gate), the shared pin status (for
    /// `capability/AGENTS.md` §5.3 obligation 3), the rate limiter, and the preview broadcast
    /// sender — nothing here can construct or reach a commit pipeline, branch registry, or
    /// snapshot source.
    pub struct PreviewTaskState {
        sessions: Arc<RwLock<SessionAuthorizationTable>>,
        pinned: Arc<RwLock<PinnedSessionStatus>>,
        preview_limiter: Arc<Mutex<PreviewRateLimiter>>,
        preview_delta_tx: broadcast::Sender<PreviewDeltaBody>,
    }

    impl PreviewTaskState {
        pub(super) fn new(
            sessions: Arc<RwLock<SessionAuthorizationTable>>,
            pinned: Arc<RwLock<PinnedSessionStatus>>,
            preview_limiter: Arc<Mutex<PreviewRateLimiter>>,
            preview_delta_tx: broadcast::Sender<PreviewDeltaBody>,
        ) -> Self {
            Self {
                sessions,
                pinned,
                preview_limiter,
                preview_delta_tx,
            }
        }
    }

    /// Handles one `preview_send` (§7.1, §7.2.3). Stops a stale session generation and a
    /// not-`Valid` or non-covering pinned session (`capability/AGENTS.md` §5.3 obligation 3)
    /// before the rate limiter is even consulted, so a revoked or unauthorized caller cannot
    /// use it as an oracle for remaining preview budget.
    pub async fn dispatch_preview_send(
        state: &PreviewTaskState,
        body: &PreviewSendBody,
        now_ms: u64,
    ) -> Result<PreviewDeltaBody, WireError> {
        let sender_public_key = {
            let sessions = state.sessions.read().await;
            if !sessions.is_generation_valid(body.session_generation) {
                return Err(WireError::StaleSessionGeneration {
                    generation: body.session_generation,
                });
            }
            let (_, public_key) = sessions
                .binding(body.authorization_binding_id)
                .ok_or(WireError::ScopeNotAuthorized)?;
            public_key
        };

        let covered = match &*state.pinned.read().await {
            PinnedSessionStatus::Valid { session, .. } => session.covers(&body.scope),
            _ => false,
        };
        if !covered {
            return Err(WireError::ScopeNotAuthorized);
        }

        {
            let sessions = state.sessions.read().await;
            let mut limiter = state.preview_limiter.lock().await;
            limiter.check_and_record(
                &sessions,
                body.session_generation,
                sender_public_key,
                body.scope,
                None,
                now_ms,
            )?;
        }

        let sender = fe_canonical_log::envelope::Author::from_public_key(sender_public_key);
        let delta = PreviewDeltaBody {
            sender_principal: sender,
            scope: body.scope,
            preview_sequence: body.preview_sequence,
            preview_kind: body.preview_kind,
            expires_at_ms: body.expires_at_ms,
            preview_data: body.preview_data.clone(),
        };
        let _ = state.preview_delta_tx.send(delta.clone());
        Ok(delta)
    }

    /// The distinct preview task's body: drains preview requests and replies with the
    /// dispatch result, entirely independent of the connection's commit-class task.
    pub(super) async fn run(
        state: PreviewTaskState,
        mut inbox: mpsc::UnboundedReceiver<(
            PreviewSendBody,
            oneshot::Sender<Result<PreviewDeltaBody, WireError>>,
        )>,
    ) {
        while let Some((body, reply_tx)) = inbox.recv().await {
            let now_ms = current_unix_ms();
            let result = dispatch_preview_send(&state, &body, now_ms).await;
            let _ = reply_tx.send(result);
        }
    }

    fn current_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fe_canonical_log::envelope::{Hash32, Identifier32, Scope};
        use fe_canonical_log::wire::preview_limiter::PreviewRateLimit;

        fn sample_body(session_generation: u64, scope: Scope) -> PreviewSendBody {
            PreviewSendBody {
                session_generation,
                authorization_binding_id: [1; 16],
                scope,
                preview_sequence: 1,
                preview_kind: 1,
                expires_at_ms: 1_000,
                preview_data: vec![9],
            }
        }

        fn scope() -> Scope {
            Scope::verse_wide(Identifier32([0x11; 32]))
        }

        async fn state_with_binding(
            valid_scope: Option<Scope>,
        ) -> (PreviewTaskState, Arc<RwLock<SessionAuthorizationTable>>) {
            let mut table = SessionAuthorizationTable::new();
            let generation = table
                .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
                .expect("accept binding");
            let sessions = Arc::new(RwLock::new(table));
            let pinned_status = match valid_scope {
                Some(scope) => PinnedSessionStatus::Valid {
                    session: fe_canonical_log::capability::PinnedSession {
                        leaf_principal: fe_canonical_log::envelope::Author::from_public_key(
                            [0x02; 32],
                        ),
                        chain_id: Hash32([0xaa; 32]),
                        leaf_certificate_id: Hash32([0xaa; 32]),
                        epoch_scope: scope,
                        epoch: 1,
                        expires_at_ms: u64::MAX,
                        subscribed_scopes: vec![scope],
                    },
                    authorization_binding_id: [1; 16],
                },
                None => PinnedSessionStatus::Unauthorized,
            };
            let state = PreviewTaskState::new(
                Arc::clone(&sessions),
                Arc::new(RwLock::new(pinned_status)),
                Arc::new(Mutex::new(PreviewRateLimiter::new(PreviewRateLimit::new(
                    10, 1_000,
                )))),
                broadcast::channel(4).0,
            );
            let _ = generation;
            (state, sessions)
        }

        #[tokio::test]
        async fn a_stale_session_generation_is_rejected_before_the_rate_limiter_runs() {
            let (state, sessions) = state_with_binding(Some(scope())).await;
            let current = sessions.read().await.current_generation();
            let stale = current + 1;
            let result = dispatch_preview_send(&state, &sample_body(stale, scope()), 0).await;
            assert_eq!(
                result,
                Err(WireError::StaleSessionGeneration { generation: stale })
            );
        }

        #[tokio::test]
        async fn an_unpinned_session_is_rejected_even_with_a_current_generation() {
            let (state, sessions) = state_with_binding(None).await;
            let current = sessions.read().await.current_generation();
            let result = dispatch_preview_send(&state, &sample_body(current, scope()), 0).await;
            assert_eq!(result, Err(WireError::ScopeNotAuthorized));
        }

        #[tokio::test]
        async fn a_valid_pinned_session_may_preview_its_covered_scope() {
            let (state, sessions) = state_with_binding(Some(scope())).await;
            let current = sessions.read().await.current_generation();
            let result = dispatch_preview_send(&state, &sample_body(current, scope()), 0).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn a_scope_the_handshake_did_not_pin_is_refused() {
            let (state, sessions) = state_with_binding(Some(scope())).await;
            let current = sessions.read().await.current_generation();
            let other_scope = Scope::verse_wide(Identifier32([0x99; 32]));
            let result = dispatch_preview_send(&state, &sample_body(current, other_scope), 0).await;
            assert_eq!(result, Err(WireError::ScopeNotAuthorized));
        }
    }
}

// ---------------------------------------------------------------------------
// Socket lifecycle
// ---------------------------------------------------------------------------

/// Per-connection message dispatch, entered once per decoded frame.
async fn handle_frame(
    socket: &mut WebSocket,
    canonical: &CanonicalLogState,
    auth: &mut ConnectionAuthState,
    preview_tx: &mpsc::UnboundedSender<(
        fe_canonical_log::wire::preview::PreviewSendBody,
        oneshot::Sender<Result<fe_canonical_log::wire::preview::PreviewDeltaBody, WireError>>,
    )>,
    frame: Frame,
) {
    if frame.message_type.direction() != Direction::ClientToService {
        return send_protocol_error(socket, ProtocolErrorCategory::WrongMessageDirection).await;
    }

    match frame.message_type {
        MessageType::Authorize => {
            let Ok(body) = AuthorizeBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            match dispatch_authorize(canonical, auth, &body).await {
                Ok(authorized) => match authorized.to_cbor() {
                    Ok(cbor) => {
                        send_frame(socket, MessageType::Authorized, frame.request_id, cbor).await
                    }
                    Err(_) => {
                        send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await
                    }
                },
                Err(category) => send_protocol_error(socket, category).await,
            }
        }
        MessageType::CommitSubmit => {
            let Ok(body) = CommitSubmitBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            let ack = dispatch_commit_submit(canonical, auth, &body).await;
            match ack.to_cbor() {
                Ok(cbor) => {
                    send_frame(socket, MessageType::CommitAck, frame.request_id, cbor).await
                }
                Err(_) => send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await,
            }
        }
        MessageType::Subscribe => {
            let Ok(body) = SubscribeBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            if let Err(category) = dispatch_subscribe(auth, &body).await {
                return send_protocol_error(socket, category).await;
            }
            // §5.1.2: a newly accepted subscription MUST begin with a fresh scene_snapshot.
            if let Ok(outcomes) =
                dispatch_snapshot_fanout(canonical, auth, body.session_generation).await
            {
                for (subscription_id, outcome) in outcomes {
                    if subscription_id != body.subscription_id {
                        continue;
                    }
                    if let SnapshotDispatchOutcome::Snapshot(snapshot) = outcome {
                        if let Ok(cbor) = snapshot.to_cbor() {
                            send_frame(socket, MessageType::SceneSnapshot, frame.request_id, cbor)
                                .await;
                        }
                    }
                }
            }
        }
        MessageType::Resume => {
            let Ok(body) = ResumeBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            match dispatch_resume(canonical, auth, &body).await {
                Ok(ResumeOutcome::Replayed {
                    deltas,
                    replay_complete,
                }) => {
                    for delta in &deltas {
                        if let Some(delta_body) = build_delta_for_forwarding(
                            auth,
                            body.subscription_id,
                            body.session_generation,
                            delta,
                        ) {
                            if let Ok(cbor) = delta_body.to_cbor() {
                                send_frame(socket, MessageType::CommitDelta, None, cbor).await;
                            }
                        }
                    }
                    if let Ok(cbor) = replay_complete.to_cbor() {
                        send_frame(socket, MessageType::ReplayComplete, frame.request_id, cbor)
                            .await;
                    }
                }
                Ok(ResumeOutcome::SnapshotRequired(required)) => {
                    send_frame(
                        socket,
                        MessageType::SnapshotRequired,
                        frame.request_id,
                        required.to_cbor(),
                    )
                    .await;
                }
                Err(category) => send_protocol_error(socket, category).await,
            }
        }
        MessageType::SnapshotAck => {
            let Ok(body) = SnapshotAckBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            {
                let sessions = auth.sessions.read().await;
                if !sessions.is_generation_valid(body.session_generation) {
                    return send_protocol_error(
                        socket,
                        ProtocolErrorCategory::SessionGenerationInvalid,
                    )
                    .await;
                }
            }
            dispatch_snapshot_ack(auth, &body);
        }
        MessageType::PreviewSend => {
            let Ok(body) = fe_canonical_log::wire::preview::PreviewSendBody::from_cbor(&frame.body)
            else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            let (reply_tx, reply_rx) = oneshot::channel();
            if preview_tx.send((body, reply_tx)).is_err() {
                return send_protocol_error(socket, ProtocolErrorCategory::PreviewRateLimited)
                    .await;
            }
            match reply_rx.await {
                Ok(Ok(delta)) => {
                    if let Ok(cbor) = delta.to_cbor() {
                        send_frame(socket, MessageType::PreviewDelta, frame.request_id, cbor).await;
                    }
                }
                Ok(Err(WireError::PreviewRateLimited)) => {
                    send_protocol_error(socket, ProtocolErrorCategory::PreviewRateLimited).await;
                }
                Ok(Err(WireError::StaleSessionGeneration { .. })) => {
                    send_protocol_error(socket, ProtocolErrorCategory::SessionGenerationInvalid)
                        .await;
                }
                _ => send_protocol_error(socket, ProtocolErrorCategory::ScopeNotAuthorized).await,
            }
        }
        _ => send_protocol_error(socket, ProtocolErrorCategory::WrongMessageDirection).await,
    }
}

/// Forwards one process-wide committed delta to this connection's own matching subscriptions
/// (§4.3 rule 3): scope-contained, same branch, same projection identity.
async fn forward_committed_delta(
    socket: &mut WebSocket,
    auth: &mut ConnectionAuthState,
    delta: &CommittedDelta,
) {
    let matching: Vec<[u8; 16]> = auth
        .subscriptions
        .iter()
        .filter(|(_, record)| {
            is_delta_authorized_for_subscription(&record.scope, &delta.scope)
                && record.branch_id == delta.branch_id
                && record.projection_identity == delta.projection_identity
        })
        .map(|(id, _)| *id)
        .collect();

    if matching.is_empty() {
        return;
    }
    let generation = auth.sessions.read().await.current_generation();
    for subscription_id in matching {
        if let Some(delta_body) =
            build_delta_for_forwarding(auth, subscription_id, generation, delta)
        {
            if let Ok(cbor) = delta_body.to_cbor() {
                send_frame(socket, MessageType::CommitDelta, None, cbor).await;
            }
        }
    }
}

/// D-CL15 broadcast-lag recovery for this connection: re-snapshots every still-authorized
/// subscribed scope rather than continuing from an unknown delta boundary — generalizes the
/// per-petal pattern at `crate::ws::send_scene_snapshots`.
async fn recover_from_broadcast_lag(
    socket: &mut WebSocket,
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
) {
    let generation = auth.sessions.read().await.current_generation();
    if let Ok(outcomes) = dispatch_snapshot_fanout(canonical, auth, generation).await {
        for (_, outcome) in outcomes {
            if let SnapshotDispatchOutcome::Snapshot(snapshot) = outcome {
                if let Ok(cbor) = snapshot.to_cbor() {
                    send_frame(socket, MessageType::SceneSnapshot, None, cbor).await;
                }
            }
        }
    }
}

/// §5.3 rule 3, on a timer: re-checks the pinned session against the persistent authorization
/// view independent of traffic, and stops all four protected paths by flipping the shared
/// status to `Invalidated` before telling the client.
async fn revalidate_pinned_session(
    socket: &mut WebSocket,
    canonical: &CanonicalLogState,
    sessions: &Arc<RwLock<SessionAuthorizationTable>>,
    pinned: &Arc<RwLock<PinnedSessionStatus>>,
    now_ms: u64,
) {
    let outcome = {
        let guard = pinned.read().await;
        match &*guard {
            PinnedSessionStatus::Valid {
                session,
                authorization_binding_id,
            } => Some((
                session.epoch_scope,
                *authorization_binding_id,
                session.is_still_valid(now_ms, canonical.authorization_view.as_ref()),
            )),
            _ => None,
        }
    };
    let Some((scope, authorization_binding_id, validity)) = outcome else {
        return;
    };
    if matches!(validity, SessionValidity::Valid) {
        return;
    }

    *pinned.write().await = PinnedSessionStatus::Invalidated;
    let invalidated_generation = sessions.write().await.invalidate();
    let reason = match validity {
        SessionValidity::Expired => RevalidationReason::CapabilityExpired,
        SessionValidity::ReauthorizationRequired => RevalidationReason::ScopeEpochAdvanced,
        SessionValidity::Valid => return,
    };
    let body = AuthorizationRevalidationRequiredBody {
        authorization_binding_id,
        scope,
        invalidated_session_generation: invalidated_generation,
        reason,
    };
    if let Ok(cbor) = body.to_cbor() {
        send_frame(
            socket,
            MessageType::AuthorizationRevalidationRequired,
            None,
            cbor,
        )
        .await;
    }
}

async fn handle_socket(mut socket: WebSocket, canonical: Arc<CanonicalLogState>) {
    let sessions = Arc::new(RwLock::new(SessionAuthorizationTable::new()));
    let pinned = Arc::new(RwLock::new(PinnedSessionStatus::Unauthorized));
    let mut auth = ConnectionAuthState {
        sessions: Arc::clone(&sessions),
        pinned: Arc::clone(&pinned),
        subscriptions: SubscriptionTable::new(),
        frontier_ledgers: HashMap::new(),
    };

    // The preview task's captured state is exactly `PreviewTaskState` — see its module doc.
    let preview_state = PreviewTaskState::new(
        Arc::clone(&sessions),
        Arc::clone(&pinned),
        Arc::clone(&canonical.preview_limiter),
        canonical.preview_delta_tx.clone(),
    );
    let (preview_tx, preview_rx) = mpsc::unbounded_channel();
    tokio::spawn(preview_task::run(preview_state, preview_rx));

    let mut committed_delta_rx = canonical.committed_delta_tx.subscribe();
    let mut revalidation_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    revalidation_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                let Message::Binary(bytes) = message else { continue; };
                let Ok(frame) = decode_frame(bytes.as_ref()) else { continue; };
                handle_frame(&mut socket, &canonical, &mut auth, &preview_tx, frame).await;
            }
            delta = committed_delta_rx.recv() => {
                match delta {
                    Ok(delta) => forward_committed_delta(&mut socket, &mut auth, &delta).await,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        recover_from_broadcast_lag(&mut socket, &canonical, &auth).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = revalidation_interval.tick() => {
                let now_ms = current_unix_ms();
                revalidate_pinned_session(&mut socket, &canonical, &sessions, &pinned, now_ms).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wire helpers
// ---------------------------------------------------------------------------

async fn send_frame(
    socket: &mut WebSocket,
    message_type: MessageType,
    request_id: Option<[u8; 16]>,
    body: CborValue,
) {
    let frame = Frame::new(message_type, request_id, body);
    if let Ok(bytes) = encode_frame(&frame) {
        let _ = socket.send(Message::Binary(bytes.into())).await;
    }
}

async fn send_protocol_error(socket: &mut WebSocket, category: ProtocolErrorCategory) {
    send_frame(
        socket,
        MessageType::ProtocolError,
        None,
        ProtocolErrorBody { category }.to_cbor(),
    )
    .await;
}

fn current_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Colocated tests: dispatch + stale-generation rejection (no live socket)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // Trait names the doubles below implement or coerce to. Scoped to the test module because
    // the production code above reaches them only through `CanonicalLogState`'s stored `Arc`s.
    use fe_canonical_log::capability::AuthorizationView;
    use fe_canonical_log::envelope::{Identifier32, Scope};
    use fe_canonical_log::wire::commit::CanonicalCommitPipeline;
    use fe_canonical_log::wire::cursor::{BranchRegistry, CursorTuple, ProjectionIdentity};
    use fe_canonical_log::wire::preview_limiter::PreviewRateLimit;
    use fe_canonical_log::wire::test_support::{
        cursor_with_claim, test_principal, InMemoryBranchRegistry, MockCapabilityVerifier,
        MockCommitPipeline, MockScopeSnapshotSource, ScriptedCommitPipeline,
    };

    struct FixedAuthorizationView;
    impl AuthorizationView for FixedAuthorizationView {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            Some(1)
        }
        fn version(&self) -> u64 {
            1
        }
    }

    fn sample_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    fn sample_projection() -> ProjectionIdentity {
        ProjectionIdentity {
            materializer_id: "fe-scene".to_owned(),
            version: 1,
        }
    }

    fn sample_tuple() -> CursorTuple {
        CursorTuple {
            verse_id: Identifier32([0x11; 32]),
            branch_id: Identifier32([0x22; 32]),
            subscription_scope: sample_scope(),
            projection_identity: sample_projection(),
        }
    }

    fn test_canonical_state() -> CanonicalLogState {
        let commit_pipeline: Arc<dyn CanonicalCommitPipeline> = Arc::new(ScriptedCommitPipeline {
            derived_op_id: Hash32([0; 32]),
            result: PipelineResult::AcceptedPendingMaterialization,
        });
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let branch_registry: Arc<dyn BranchRegistry> = registry.clone();
        let snapshot_source: Arc<dyn fe_canonical_log::wire::snapshot::ScopeSnapshotSource> =
            Arc::new(MockScopeSnapshotSource::new(registry, Vec::new()));
        let capability_verifier: Arc<dyn fe_canonical_log::wire::session::CapabilityVerifier> =
            Arc::new(MockCapabilityVerifier::new());
        let view: Arc<dyn AuthorizationView> = Arc::new(FixedAuthorizationView);
        CanonicalLogState::new(
            commit_pipeline,
            branch_registry,
            snapshot_source,
            capability_verifier,
            view,
            fe_canonical_log::wire::preview_limiter::PreviewRateLimiter::new(
                PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        )
    }

    fn empty_auth() -> ConnectionAuthState {
        ConnectionAuthState {
            sessions: Arc::new(RwLock::new(SessionAuthorizationTable::new())),
            pinned: Arc::new(RwLock::new(PinnedSessionStatus::Unauthorized)),
            subscriptions: SubscriptionTable::new(),
            frontier_ledgers: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn a_stale_session_generation_is_rejected_before_commit_pipeline_work() {
        let canonical = test_canonical_state();
        let auth = empty_auth();
        *auth.pinned.write().await = PinnedSessionStatus::Valid {
            session: PinnedSession {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0xaa; 32]),
                leaf_certificate_id: Hash32([0xaa; 32]),
                epoch_scope: sample_scope(),
                epoch: 1,
                expires_at_ms: u64::MAX,
                subscribed_scopes: vec![sample_scope()],
            },
            authorization_binding_id: [1; 16],
        };

        let body = CommitSubmitBody {
            session_generation: 999, // never accepted
            authorization_binding_id: [1; 16],
            claimed_op_id: Hash32([1; 32]),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body).await;
        assert_eq!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Rejected {
                category: ProtocolErrorCategory::SessionGenerationInvalid
            }
        );
    }

    #[tokio::test]
    async fn an_unpinned_connection_cannot_commit_even_with_a_valid_generation() {
        let canonical = test_canonical_state();
        let auth = empty_auth();
        let generation = auth
            .sessions
            .write()
            .await
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
            .expect("accept");

        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id: Hash32([1; 32]),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body).await;
        assert_eq!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Rejected {
                category: ProtocolErrorCategory::AuthorizationRevalidationRequired
            }
        );
    }

    #[tokio::test]
    async fn a_pinned_and_current_commit_reaches_the_pipeline() {
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let commit_pipeline = Arc::new(MockCommitPipeline::new(registry.clone(), sample_tuple()));
        let canonical = CanonicalLogState::new(
            commit_pipeline,
            registry.clone(),
            Arc::new(MockScopeSnapshotSource::new(registry, Vec::new())),
            Arc::new(MockCapabilityVerifier::new()),
            Arc::new(FixedAuthorizationView),
            fe_canonical_log::wire::preview_limiter::PreviewRateLimiter::new(
                PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        );
        let auth = empty_auth();
        let generation = auth
            .sessions
            .write()
            .await
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
            .expect("accept");
        *auth.pinned.write().await = PinnedSessionStatus::Valid {
            session: PinnedSession {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0xaa; 32]),
                leaf_certificate_id: Hash32([0xaa; 32]),
                epoch_scope: sample_scope(),
                epoch: 1,
                expires_at_ms: u64::MAX,
                subscribed_scopes: vec![sample_scope()],
            },
            authorization_binding_id: [1; 16],
        };

        let bytes = vec![9, 9, 9];
        let claimed_op_id = Hash32::of(&bytes);
        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id,
            complete_envelope: bytes,
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body).await;
        assert!(matches!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Committed(_)
        ));
    }

    #[tokio::test]
    async fn resume_with_no_frontier_ledger_falls_back_to_snapshot_required() {
        let canonical = test_canonical_state();
        let mut auth = empty_auth();
        let generation = auth
            .sessions
            .write()
            .await
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
            .expect("accept");
        *auth.pinned.write().await = PinnedSessionStatus::Valid {
            session: PinnedSession {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0xaa; 32]),
                leaf_certificate_id: Hash32([0xaa; 32]),
                epoch_scope: sample_scope(),
                epoch: 1,
                expires_at_ms: u64::MAX,
                subscribed_scopes: vec![sample_scope()],
            },
            authorization_binding_id: [1; 16],
        };
        auth.subscriptions
            .bind(
                [7; 16],
                SubscriptionRecord {
                    authorization_binding_id: [1; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind");
        // Deliberately no `snapshot_ack`/delivered deltas recorded — the ledger is empty, so
        // the peer-supplied cursor cannot be corroborated (gate obligation 1).

        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [7; 16],
            prior_cursor: cursor_with_claim(sample_tuple(), Hash32([0x55; 32]), vec![9]),
        };
        let outcome = dispatch_resume(&canonical, &auth, &body)
            .await
            .expect("no protocol error");
        assert!(matches!(
            outcome,
            ResumeOutcome::SnapshotRequired(required) if required.reason == SnapshotReason::CursorInvalid
        ));
    }

    #[tokio::test]
    async fn resume_with_the_exact_trusted_baseline_needs_no_recomputation() {
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let canonical = CanonicalLogState::new(
            Arc::new(ScriptedCommitPipeline {
                derived_op_id: Hash32([0; 32]),
                result: PipelineResult::AcceptedPendingMaterialization,
            }),
            registry.clone(),
            Arc::new(MockScopeSnapshotSource::new(registry.clone(), Vec::new())),
            Arc::new(MockCapabilityVerifier::new()),
            Arc::new(FixedAuthorizationView),
            fe_canonical_log::wire::preview_limiter::PreviewRateLimiter::new(
                PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        );
        let mut auth = empty_auth();
        let generation = auth
            .sessions
            .write()
            .await
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
            .expect("accept");
        *auth.pinned.write().await = PinnedSessionStatus::Valid {
            session: PinnedSession {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0xaa; 32]),
                leaf_certificate_id: Hash32([0xaa; 32]),
                epoch_scope: sample_scope(),
                epoch: 1,
                expires_at_ms: u64::MAX,
                subscribed_scopes: vec![sample_scope()],
            },
            authorization_binding_id: [1; 16],
        };
        auth.subscriptions
            .bind(
                [7; 16],
                SubscriptionRecord {
                    authorization_binding_id: [1; 16],
                    branch_id: Identifier32([0x22; 32]),
                    scope: sample_scope(),
                    projection_identity: sample_projection(),
                },
            )
            .expect("bind");
        // `snapshot_cursor` resolves a scope only once the registry holds a delta for it, and
        // `resolve_resume` rebuilds this exact tuple from the subscription record above.
        registry.commit(sample_tuple(), Hash32([0x33; 32]), Vec::new());
        let snapshot_cursor = registry
            .snapshot_cursor(&sample_scope())
            .await
            .expect("snapshot cursor");
        dispatch_snapshot_ack(
            &mut auth,
            &fe_canonical_log::wire::snapshot::SnapshotAckBody {
                session_generation: generation,
                subscription_id: [7; 16],
                snapshot_cursor: snapshot_cursor.clone(),
            },
        );

        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [7; 16],
            prior_cursor: snapshot_cursor,
        };
        let outcome = dispatch_resume(&canonical, &auth, &body)
            .await
            .expect("no protocol error");
        assert!(matches!(outcome, ResumeOutcome::Replayed { .. }));
    }

    #[tokio::test]
    async fn authorize_consults_the_cache_before_re_verifying_an_identical_chain() {
        let auth = empty_auth();
        // Held as `Arc<MockCapabilityVerifier>` (not yet erased to `Arc<dyn CapabilityVerifier>`)
        // so this test can still call the concrete `.calls()` inspector after handing a clone
        // to `CanonicalLogState`.
        let verifier = Arc::new(MockCapabilityVerifier::new());
        let chain_bytes = vec![7, 7, 7];
        verifier.register(
            chain_bytes.clone(),
            fe_canonical_log::wire::session::VerifiedAuthorization {
                leaf_principal: test_principal(0x01),
                chain_id: Hash32([0xaa; 32]),
                epoch_scope: sample_scope(),
                scope_epoch: 1,
                expires_at_ms: u64::MAX,
            },
        );
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let canonical = CanonicalLogState::new(
            Arc::new(ScriptedCommitPipeline {
                derived_op_id: Hash32([0; 32]),
                result: PipelineResult::AcceptedPendingMaterialization,
            }),
            registry.clone(),
            Arc::new(MockScopeSnapshotSource::new(registry, Vec::new())),
            verifier.clone(),
            Arc::new(FixedAuthorizationView),
            fe_canonical_log::wire::preview_limiter::PreviewRateLimiter::new(
                PreviewRateLimit::new(10, 1_000),
            ),
            4,
            4,
        );

        let body = AuthorizeBody {
            capability_chain_bytes: chain_bytes,
            authorization_binding_id: [3; 16],
            requested_verb: 0x01,
            requested_object_class: 0x01,
            requested_scope: sample_scope(),
        };
        let first = dispatch_authorize(&canonical, &auth, &body).await;
        assert!(first.is_ok());
        let second = dispatch_authorize(&canonical, &auth, &body).await;
        assert!(second.is_ok());
        assert_ne!(
            first.unwrap().session_generation,
            second.unwrap().session_generation,
            "each accepted authorize still mints its own fresh session generation"
        );

        // The obligation-1 assertion: the second `authorize` reused the §5.3 cache instead of
        // re-verifying the identical chain bytes — the verifier itself was consulted once.
        assert_eq!(verifier.calls().len(), 1);
    }

    #[test]
    fn frontier_ledger_rejects_an_uncorroborated_cursor() {
        let mut ledger = SubscriptionFrontierLedger::default();
        let baseline = cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]);
        ledger.reset_to_snapshot(baseline.clone());

        // The baseline itself always verifies (it is exactly what we minted).
        assert!(verify_resume_cursor_frontier(Some(&ledger), &baseline).is_ok());

        // A cursor this connection never witnessed, with a fabricated commitment, is rejected.
        let tampered = cursor_with_claim(sample_tuple(), Hash32([0xee; 32]), vec![5]);
        assert!(verify_resume_cursor_frontier(Some(&ledger), &tampered).is_err());

        // No ledger at all (never snapshotted in this connection) cannot be corroborated.
        assert!(verify_resume_cursor_frontier(None, &tampered).is_err());
    }

    #[test]
    fn frontier_ledger_verifies_a_cursor_whose_members_match_what_was_delivered() {
        let mut ledger = SubscriptionFrontierLedger::default();
        let baseline = cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]);
        ledger.reset_to_snapshot(baseline);
        let delivered = Hash32([0x01; 32]);
        ledger.record_delivered(delivered);

        let expected_commitment = fe_canonical_log::frontier::SortedFrontier::try_new([delivered])
            .expect("frontier")
            .commitment();
        let claimed = cursor_with_claim(sample_tuple(), expected_commitment, vec![1]);
        assert!(verify_resume_cursor_frontier(Some(&ledger), &claimed).is_ok());
    }
}
