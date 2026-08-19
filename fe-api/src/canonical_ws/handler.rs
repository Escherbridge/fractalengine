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

use fe_canonical_log::capability::{AuthorizationView, PinnedSession, SessionValidity};
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
    AuthorizationRevalidationRequiredBody, AuthorizeBody, AuthorizedBody, ObjectClassBits,
    RevalidationReason, SessionAuthorizationTable, VerbBits,
};
use fe_canonical_log::wire::snapshot::{
    snapshot_all_authorized_subscriptions, SnapshotAckBody, SnapshotDispatchOutcome,
};
use fe_canonical_log::wire::subscription::{
    resolve_resume, ResumeBody, ResumeOutcome, SubscribeBody, SubscriptionRecord, SubscriptionTable,
};

use crate::canonical_ws::state::{CanonicalLogState, VerificationRequest};
use preview_task::PreviewTaskState;

// ---------------------------------------------------------------------------
// Per-connection bounds (see `AGENTS.md` §bounded-state)
// ---------------------------------------------------------------------------

/// Ceiling on subscriptions one connection may bind.
const MAX_SUBSCRIPTIONS_PER_CONNECTION: usize = 64;

/// Ceiling on `op_id`s one subscription's frontier ledger will remember.
const MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION: usize = 4_096;

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

/// Exactly what `authorize` verified, kept so a later frame can re-verify a peer-supplied
/// scope against the real capability chain instead of trusting the pin's bookkeeping.
/// See `AGENTS.md` §subscribe-re-verifies-the-chain.
pub(crate) struct EstablishedAuthorization {
    /// The exact capability-chain bytes `CapabilityVerifier::verify` accepted.
    capability_chain_bytes: Vec<u8>,
    /// The verb bitmask that verification was performed for.
    requested_verb: VerbBits,
    /// The object-class bitmask that verification was performed for.
    requested_object_class: ObjectClassBits,
}

impl EstablishedAuthorization {
    /// Records what one accepted `authorize` verified.
    pub(crate) fn new(
        capability_chain_bytes: Vec<u8>,
        requested_verb: VerbBits,
        requested_object_class: ObjectClassBits,
    ) -> Self {
        Self {
            capability_chain_bytes,
            requested_verb,
            requested_object_class,
        }
    }
}

/// What this connection currently believes about its pinned capability session (§5.3 rule 3).
/// Read by both the commit-class dispatch below and the structurally separate preview task;
/// only commit-class dispatch (via `authorize` and the revalidation timer) ever writes it.
///
/// `large_enum_variant` is allowed rather than satisfied: `Valid` is ~320 bytes, but exactly
/// one of these exists per connection, it lives behind an `RwLock`, it is written only on
/// `authorize` and the revalidation tick, and every reader matches it by reference. Boxing
/// would add an allocation and an indirection to the per-frame dispatch path to save ~300
/// bytes per connection. That is the opposite of the trade the lint exists to prompt — unlike
/// `SegmentError`/`CapabilityError`, which are boxed because a `Result` is returned by value
/// from every call.
#[allow(clippy::large_enum_variant)]
pub(crate) enum PinnedSessionStatus {
    /// No `authorize` has been accepted yet on this connection.
    Unauthorized,
    /// A pinned session is current, alongside the binding ID it was accepted under (needed to
    /// name it in a later `authorization_revalidation_required` frame) and the chain bytes it
    /// was accepted from (needed to re-verify any scope the handshake did not itself verify).
    Valid {
        session: PinnedSession,
        authorization_binding_id: [u8; 16],
        established: EstablishedAuthorization,
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
    /// The exact cursor this connection minted and the client acknowledged as its replay base.
    trusted_baseline: Option<DurableCursor>,
    /// Every `op_id` shown to this connection for the subscription since `trusted_baseline`.
    observed_op_ids: BTreeSet<Hash32>,
    /// Set once the observed set reached [`MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION`]: the ledger
    /// is no longer a complete record, so it says so instead of letting a recomputation over a
    /// truncated set look like a verdict.
    observation_overflowed: bool,
}

impl SubscriptionFrontierLedger {
    fn reset_to_snapshot(&mut self, cursor: DurableCursor) {
        self.trusted_baseline = Some(cursor);
        self.observed_op_ids.clear();
        self.observation_overflowed = false;
    }

    fn record_delivered(&mut self, op_id: Hash32) {
        if self.observed_op_ids.contains(&op_id) {
            return;
        }
        if self.observed_op_ids.len() >= MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION {
            self.observation_overflowed = true;
            return;
        }
        self.observed_op_ids.insert(op_id);
    }
}

/// Gate: `wire::cursor::verify_frontier_commitment` MUST be called on every peer-supplied
/// resume cursor before it selects a replay range (`wire/AGENTS.md` §"Wave 3 obligation").
/// Rejects a cursor this connection cannot corroborate rather than trusting it; the exact
/// cursor this connection last minted for the client (`trusted_baseline`) needs no
/// recomputation since it did not travel through the client's own hands unverified — a
/// property `dispatch_snapshot_ack` is what actually establishes, by refusing to adopt any
/// baseline other than a cursor this connection itself sent.
fn verify_resume_cursor_frontier(
    ledger: Option<&SubscriptionFrontierLedger>,
    prior_cursor: &DurableCursor,
) -> Result<(), WireError> {
    let ledger = ledger.ok_or(WireError::CursorInvalid)?;
    if ledger.trusted_baseline.as_ref() == Some(prior_cursor) {
        return Ok(());
    }
    if ledger.observation_overflowed {
        return Err(WireError::CursorInvalid);
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
    /// The last `scene_snapshot` cursor this connection actually sent per subscription. The
    /// only cursor a `snapshot_ack` may promote to a trusted frontier baseline.
    minted_snapshot_cursors: HashMap<[u8; 16], DurableCursor>,
}

impl ConnectionAuthState {
    /// Drops every subscription, ledger, and minted cursor. Called when a fresh `authorize`
    /// supersedes the pinned session that authorized them: a subscription established under a
    /// replaced session carries no authority under its replacement.
    fn clear_subscription_state(&mut self) {
        self.subscriptions = SubscriptionTable::new();
        self.frontier_ledgers.clear();
        self.minted_snapshot_cursors.clear();
    }
}

/// Resolves `authorization_binding_id` against this connection's session table and proves it
/// names the pinned session's own chain and leaf principal. A binding this connection never
/// accepted, or one accepted for some other chain or principal, is not this session's
/// authority — so this, not the presence of the field, is what makes a binding ID mean
/// anything (`AGENTS.md` §binding-ids-are-resolved-not-echoed).
fn binding_matches_pinned_session(
    sessions: &SessionAuthorizationTable,
    session: &PinnedSession,
    authorization_binding_id: [u8; 16],
) -> bool {
    match sessions.binding(authorization_binding_id) {
        Some((chain_id, leaf_principal_public_key)) => {
            chain_id == session.chain_id
                && leaf_principal_public_key == session.leaf_principal.public_key
        }
        None => false,
    }
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
    auth: &mut ConnectionAuthState,
    body: &AuthorizeBody,
    now_ms: u64,
) -> Result<AuthorizedBody, ProtocolErrorCategory> {
    let request = VerificationRequest::new(
        &body.capability_chain_bytes,
        body.requested_verb,
        body.requested_object_class,
        body.requested_scope,
    );
    // Bound before the `match` so the borrow of `request` ends here and the miss arm may
    // move it into the cache.
    let cached = canonical.cached_verification(&request, now_ms);
    let authorization = match cached {
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
            // No `CacheKey` is built here on purpose: `admit_verification` hands the decision to
            // `RevalidationGate::admit_verified`, which reads the authority-view version itself.
            canonical.admit_verification(request, verified.clone(), now_ms);
            verified
        }
    };

    // The verifier is an authority over the chain, not over the clock or the durable epoch, and
    // a cache hit answers with a decision made earlier. Neither may pin a session the live view
    // has already revoked, so the two facts a pin depends on are re-read here.
    if now_ms >= authorization.expires_at_ms
        || canonical
            .authorization_view
            .current_epoch(&authorization.epoch_scope)
            != Some(authorization.scope_epoch)
    {
        return Err(ProtocolErrorCategory::AuthorizationRevalidationRequired);
    }

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

    // Whatever the superseded session subscribed to was authorized by the superseded session.
    auth.clear_subscription_state();

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
        established: EstablishedAuthorization::new(
            body.capability_chain_bytes.clone(),
            body.requested_verb,
            body.requested_object_class,
        ),
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

/// Handles one `commit_submit` (§4.1). Refuses a stale session generation first, then an
/// unpinned or invalidated session, then a binding ID that does not resolve to this session's
/// own chain and principal, then a session the live authorization view no longer honors
/// (`capability/AGENTS.md` §5.3 obligation 3) — all before [`handle_commit_submit`] runs its
/// own generation check and reaches the pipeline.
async fn dispatch_commit_submit(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    body: &CommitSubmitBody,
    now_ms: u64,
) -> CommitAckBody {
    let reject = |category| {
        build_commit_ack(
            body.session_generation,
            body.authorization_binding_id,
            body.claimed_op_id,
            PipelineResult::Rejected(category),
        )
    };

    let sessions = auth.sessions.read().await;
    if !sessions.is_generation_valid(body.session_generation) {
        return reject(ProtocolErrorCategory::SessionGenerationInvalid);
    }
    {
        let pinned = auth.pinned.read().await;
        let PinnedSessionStatus::Valid { session, .. } = &*pinned else {
            return reject(ProtocolErrorCategory::AuthorizationRevalidationRequired);
        };
        if !binding_matches_pinned_session(&sessions, session, body.authorization_binding_id) {
            return reject(ProtocolErrorCategory::ScopeNotAuthorized);
        }
        if !matches!(
            session.is_still_valid(now_ms, canonical.authorization_view.as_ref()),
            SessionValidity::Valid
        ) {
            return reject(ProtocolErrorCategory::AuthorizationRevalidationRequired);
        }
    }
    handle_commit_submit(canonical.commit_pipeline.as_ref(), &sessions, body).await
}

/// Handles one `subscribe` (§5.1.1).
///
/// A peer-supplied scope is authorization input, never an authorization decision. Before
/// `body.scope` may enter `subscribed_scopes` — the one set `PinnedSession::covers` consults —
/// it must clear four checks, in this order: the session generation is current; the named
/// `authorization_binding_id` resolves in this connection's session table to the pinned
/// session's own chain and principal; the live authorization view still honors the pinned
/// session; the scope is inside the pinned `epoch_scope`; and finally the real
/// [`CapabilityVerifier`](fe_canonical_log::wire::session::CapabilityVerifier) re-verifies the
/// handshake's own chain bytes against this scope. See `AGENTS.md`
/// §subscribe-re-verifies-the-chain.
async fn dispatch_subscribe(
    canonical: &CanonicalLogState,
    auth: &mut ConnectionAuthState,
    body: &SubscribeBody,
    now_ms: u64,
) -> Result<(), ProtocolErrorCategory> {
    let (capability_chain_bytes, requested_verb, requested_object_class) = {
        let sessions = auth.sessions.read().await;
        if !sessions.is_generation_valid(body.session_generation) {
            return Err(ProtocolErrorCategory::SessionGenerationInvalid);
        }
        let pinned = auth.pinned.read().await;
        let PinnedSessionStatus::Valid {
            session,
            established,
            ..
        } = &*pinned
        else {
            return Err(ProtocolErrorCategory::AuthorizationRevalidationRequired);
        };
        if !binding_matches_pinned_session(&sessions, session, body.authorization_binding_id) {
            return Err(ProtocolErrorCategory::ScopeNotAuthorized);
        }
        if !matches!(
            session.is_still_valid(now_ms, canonical.authorization_view.as_ref()),
            SessionValidity::Valid
        ) {
            return Err(ProtocolErrorCategory::AuthorizationRevalidationRequired);
        }
        if !session.epoch_scope.contains(&body.scope) {
            return Err(ProtocolErrorCategory::ScopeNotAuthorized);
        }
        (
            established.capability_chain_bytes.clone(),
            established.requested_verb,
            established.requested_object_class,
        )
    };

    // The chain bytes are cloned out above so no lock is held across this await.
    canonical
        .capability_verifier
        .verify(
            &capability_chain_bytes,
            requested_verb,
            requested_object_class,
            &body.scope,
        )
        .await
        .map_err(|_| ProtocolErrorCategory::ScopeNotAuthorized)?;

    if auth.subscriptions.get(body.subscription_id).is_none()
        && auth.subscriptions.iter().count() >= MAX_SUBSCRIPTIONS_PER_CONNECTION
    {
        return Err(ProtocolErrorCategory::ScopeNotAuthorized);
    }

    let record = SubscriptionRecord {
        authorization_binding_id: body.authorization_binding_id,
        branch_id: body.branch_id,
        scope: body.scope,
        projection_identity: body.projection_identity.clone(),
    };
    // `bind` fails only on a subscription ID reused with a different identity, and rejects it
    // without mutating the table — a malformed frame, not a denied authorization. Reporting it
    // as `ScopeNotAuthorized` would put an authorization word on a bookkeeping outcome.
    auth.subscriptions
        .bind(body.subscription_id, record)
        .map_err(|_| ProtocolErrorCategory::MalformedFrame)?;
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
    now_ms: u64,
) -> Result<ResumeOutcome, ProtocolErrorCategory> {
    {
        let pinned = auth.pinned.read().await;
        let scope = auth
            .subscriptions
            .get(body.subscription_id)
            .map(|record| record.scope);
        let covered = match (&*pinned, scope) {
            (PinnedSessionStatus::Valid { session, .. }, Some(scope)) => {
                session.covers(&scope)
                    && matches!(
                        session.is_still_valid(now_ms, canonical.authorization_view.as_ref()),
                        SessionValidity::Valid
                    )
            }
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

/// Handles one `snapshot_ack` (§5.2.5): promotes a snapshot cursor to this connection's trusted
/// frontier baseline for later `resume` corroboration (gate obligation above).
///
/// Only a cursor recorded in `minted_snapshot_cursors` — one this connection itself sent in a
/// `scene_snapshot` — may be promoted. Adopting the peer's own cursor would let the peer
/// nominate its own trusted baseline and walk straight past `verify_resume_cursor_frontier`,
/// whose baseline shortcut exists precisely because that cursor never left this connection.
fn dispatch_snapshot_ack(
    auth: &mut ConnectionAuthState,
    body: &SnapshotAckBody,
) -> Result<(), ProtocolErrorCategory> {
    let acknowledges_a_minted_cursor = auth
        .minted_snapshot_cursors
        .get(&body.subscription_id)
        .is_some_and(|minted| minted == &body.snapshot_cursor);
    if !acknowledges_a_minted_cursor {
        return Err(ProtocolErrorCategory::CursorInvalid);
    }
    auth.frontier_ledgers
        .entry(body.subscription_id)
        .or_default()
        .reset_to_snapshot(body.snapshot_cursor.clone());
    Ok(())
}

/// Records the cursor of a `scene_snapshot` this connection has actually sent, so a later
/// `snapshot_ack` naming it can be recognized as acknowledging our own cursor.
fn record_minted_snapshot_cursor(
    auth: &mut ConnectionAuthState,
    subscription_id: [u8; 16],
    cursor: DurableCursor,
) {
    if auth.subscriptions.get(subscription_id).is_none() {
        return;
    }
    auth.minted_snapshot_cursors.insert(subscription_id, cursor);
}

/// Fresh-subscribe and D-CL15 lag-recovery snapshot fan-out (§5.2), reusing the existing
/// `snapshot_all_authorized_subscriptions` helper — which already stops a stale session
/// generation before any snapshot source read — and additionally gating each scope on
/// `PinnedSession::covers` plus a live `is_still_valid` re-check (obligation 3), and on
/// `only` when the caller wants exactly one subscription re-snapshotted rather than all of
/// them.
async fn dispatch_snapshot_fanout(
    canonical: &CanonicalLogState,
    auth: &ConnectionAuthState,
    session_generation: u64,
    only: Option<&SubscriptionRecord>,
    now_ms: u64,
) -> Result<Vec<([u8; 16], SnapshotDispatchOutcome)>, WireError> {
    let sessions = auth.sessions.read().await;
    let pinned = auth.pinned.read().await;
    snapshot_all_authorized_subscriptions(
        canonical.snapshot_source.as_ref(),
        &auth.subscriptions,
        &sessions,
        session_generation,
        |record| {
            if only.is_some_and(|wanted| wanted != record) {
                return false;
            }
            match &*pinned {
                PinnedSessionStatus::Valid { session, .. } => {
                    session.covers(&record.scope)
                        && matches!(
                            session.is_still_valid(now_ms, canonical.authorization_view.as_ref()),
                            SessionValidity::Valid
                        )
                }
                _ => false,
            }
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
    ///
    /// The preview's `sender_principal` is taken from the pinned session, never from whichever
    /// binding the frame happened to name: a connection may have accepted more than one
    /// binding, and reading the sender out of the named one would let a peer sign previews with
    /// a co-resident principal's key. The named binding must resolve to the pinned session's
    /// own chain and principal — see `AGENTS.md` §binding-ids-are-resolved-not-echoed.
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
            let pinned = state.pinned.read().await;
            let PinnedSessionStatus::Valid { session, .. } = &*pinned else {
                return Err(WireError::ScopeNotAuthorized);
            };
            if !super::binding_matches_pinned_session(
                &sessions,
                session,
                body.authorization_binding_id,
            ) {
                return Err(WireError::ScopeNotAuthorized);
            }
            if !session.covers(&body.scope) {
                return Err(WireError::ScopeNotAuthorized);
            }
            session.leaf_principal.public_key
        };

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
                    established: crate::canonical_ws::handler::EstablishedAuthorization::new(
                        Vec::new(),
                        0,
                        0,
                    ),
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

        /// A second binding accepted on the same connection names a different principal. Naming
        /// it must be refused outright rather than silently minting a preview attributed to
        /// that principal.
        #[tokio::test]
        async fn a_binding_that_is_not_the_pinned_session_cannot_send_a_preview() {
            let (state, sessions) = state_with_binding(Some(scope())).await;
            let co_resident_generation = sessions
                .write()
                .await
                .accept_binding([2; 16], Hash32([0xbb; 32]), [0x99; 32])
                .expect("accept co-resident binding");

            let mut body = sample_body(co_resident_generation, scope());
            body.authorization_binding_id = [2; 16];
            assert_eq!(
                dispatch_preview_send(&state, &body, 0).await,
                Err(WireError::ScopeNotAuthorized)
            );

            // The pinned session's own binding is still accepted, and the delta it produces is
            // attributed to the pinned principal.
            body.authorization_binding_id = [1; 16];
            let delta = dispatch_preview_send(&state, &body, 0)
                .await
                .expect("pinned binding may preview");
            assert_eq!(delta.sender_principal.public_key, [0x02; 32]);
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
    now_ms: u64,
) {
    if frame.message_type.direction() != Direction::ClientToService {
        return send_protocol_error(socket, ProtocolErrorCategory::WrongMessageDirection).await;
    }

    match frame.message_type {
        MessageType::Authorize => {
            let Ok(body) = AuthorizeBody::from_cbor(&frame.body) else {
                return send_protocol_error(socket, ProtocolErrorCategory::MalformedFrame).await;
            };
            match dispatch_authorize(canonical, auth, &body, now_ms).await {
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
            let ack = dispatch_commit_submit(canonical, auth, &body, now_ms).await;
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
            if let Err(category) = dispatch_subscribe(canonical, auth, &body, now_ms).await {
                return send_protocol_error(socket, category).await;
            }
            // §5.1.2: a newly accepted subscription MUST begin with a fresh scene_snapshot.
            // Scoped to the record just bound: fanning out over every subscription would let a
            // repeated `subscribe` multiply one frame into N snapshot-source reads.
            let just_bound = SubscriptionRecord {
                authorization_binding_id: body.authorization_binding_id,
                branch_id: body.branch_id,
                scope: body.scope,
                projection_identity: body.projection_identity.clone(),
            };
            let fanout = dispatch_snapshot_fanout(
                canonical,
                auth,
                body.session_generation,
                Some(&just_bound),
                now_ms,
            )
            .await;
            if let Ok(outcomes) = fanout {
                for (subscription_id, outcome) in outcomes {
                    if subscription_id != body.subscription_id {
                        continue;
                    }
                    if let SnapshotDispatchOutcome::Snapshot(snapshot) = outcome {
                        record_minted_snapshot_cursor(
                            auth,
                            subscription_id,
                            snapshot.snapshot_cursor.clone(),
                        );
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
            match dispatch_resume(canonical, auth, &body, now_ms).await {
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
            if let Err(category) = dispatch_snapshot_ack(auth, &body) {
                return send_protocol_error(socket, category).await;
            }
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

/// The subscriptions on this connection a committed delta may reach (§4.3 rule 3): the pinned
/// session must still be `Valid` under the live authorization view AND still cover the
/// subscription's own scope, and only then is the delta's exact scope, branch, and projection
/// identity matched against that subscription.
///
/// Pulled out of [`forward_committed_delta`] so the authorization decision is a value a test
/// can inspect without a live socket.
fn subscriptions_authorized_for_delta(
    pinned: &PinnedSessionStatus,
    subscriptions: &SubscriptionTable,
    delta: &CommittedDelta,
    now_ms: u64,
    authorization_view: &dyn AuthorizationView,
) -> Vec<[u8; 16]> {
    let PinnedSessionStatus::Valid { session, .. } = pinned else {
        return Vec::new();
    };
    if !matches!(
        session.is_still_valid(now_ms, authorization_view),
        SessionValidity::Valid
    ) {
        return Vec::new();
    }
    subscriptions
        .iter()
        .filter(|(_, record)| {
            session.covers(&record.scope)
                && is_delta_authorized_for_subscription(&record.scope, &delta.scope)
                && record.branch_id == delta.branch_id
                && record.projection_identity == delta.projection_identity
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Forwards one process-wide committed delta to this connection's own matching subscriptions
/// (§4.3 rule 3): scope-contained, same branch, same projection identity — and only while the
/// pinned session still covers that subscription's scope and the live authorization view still
/// honors the session. Delta fan-out is a protected path like commit, replay, snapshot, and
/// preview: without this check a revoked session keeps receiving committed state until the
/// socket happens to close.
async fn forward_committed_delta(
    socket: &mut WebSocket,
    canonical: &CanonicalLogState,
    auth: &mut ConnectionAuthState,
    delta: &CommittedDelta,
    now_ms: u64,
) {
    let generation = {
        let sessions = auth.sessions.read().await;
        let generation = sessions.current_generation();
        if !sessions.is_generation_valid(generation) {
            return;
        }
        generation
    };

    let matching = {
        let pinned = auth.pinned.read().await;
        subscriptions_authorized_for_delta(
            &pinned,
            &auth.subscriptions,
            delta,
            now_ms,
            canonical.authorization_view.as_ref(),
        )
    };

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
    auth: &mut ConnectionAuthState,
    now_ms: u64,
) {
    let generation = auth.sessions.read().await.current_generation();
    let fanout = dispatch_snapshot_fanout(canonical, auth, generation, None, now_ms).await;
    if let Ok(outcomes) = fanout {
        for (subscription_id, outcome) in outcomes {
            if let SnapshotDispatchOutcome::Snapshot(snapshot) = outcome {
                record_minted_snapshot_cursor(
                    auth,
                    subscription_id,
                    snapshot.snapshot_cursor.clone(),
                );
                if let Ok(cbor) = snapshot.to_cbor() {
                    send_frame(socket, MessageType::SceneSnapshot, None, cbor).await;
                }
            }
        }
    }
}

/// §5.3 rule 3, on a timer: sweeps the process-wide verification cache for revoked decisions,
/// then re-checks this connection's pinned session against the persistent authorization view
/// independent of traffic, stopping every protected path by flipping the shared status to
/// `Invalidated` before telling the client.
///
/// The sweep is what makes the §5.3 cache invalidable in production rather than only in
/// principle: without a caller, `RevalidationGate`'s eviction methods would never run.
async fn revalidate_pinned_session(
    socket: &mut WebSocket,
    canonical: &CanonicalLogState,
    sessions: &Arc<RwLock<SessionAuthorizationTable>>,
    pinned: &Arc<RwLock<PinnedSessionStatus>>,
    now_ms: u64,
) {
    canonical.sweep_revocations(now_ms);

    let outcome = {
        let guard = pinned.read().await;
        match &*guard {
            PinnedSessionStatus::Valid {
                session,
                authorization_binding_id,
                ..
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
        minted_snapshot_cursors: HashMap::new(),
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
                let now_ms = current_unix_ms();
                handle_frame(&mut socket, &canonical, &mut auth, &preview_tx, frame, now_ms).await;
            }
            delta = committed_delta_rx.recv() => {
                let now_ms = current_unix_ms();
                match delta {
                    Ok(delta) => {
                        forward_committed_delta(&mut socket, &canonical, &mut auth, &delta, now_ms)
                            .await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        recover_from_broadcast_lag(&mut socket, &canonical, &mut auth, now_ms).await;
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
    // Names the doubles below implement or coerce to. Scoped to the test module because the
    // production code above reaches them only through `CanonicalLogState`'s stored `Arc`s.
    use fe_canonical_log::envelope::{Identifier32, Scope};
    use fe_canonical_log::wire::commit::CanonicalCommitPipeline;
    use fe_canonical_log::wire::cursor::{BranchRegistry, CursorTuple, ProjectionIdentity};
    use fe_canonical_log::wire::preview_limiter::{PreviewRateLimit, PreviewRateLimiter};
    use fe_canonical_log::wire::session::{CapabilityVerifier, VerifiedAuthorization};
    use fe_canonical_log::wire::snapshot::ScopeSnapshotSource;
    use fe_canonical_log::wire::test_support::{
        cursor_with_claim, test_principal, InMemoryBranchRegistry, MockCapabilityVerifier,
        MockCommitPipeline, MockScopeSnapshotSource, ScriptedCommitPipeline,
    };

    /// The durable view as it stands while a session is live: epoch 1, version 1.
    struct FixedAuthorizationView;
    impl AuthorizationView for FixedAuthorizationView {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            Some(1)
        }
        fn version(&self) -> u64 {
            1
        }
    }

    /// The same view after a revocation: the epoch moved past what every fixture session pinned.
    struct RevokedAuthorizationView;
    impl AuthorizationView for RevokedAuthorizationView {
        fn current_epoch(&self, _epoch_scope: &Scope) -> Option<u64> {
            Some(2)
        }
        fn version(&self) -> u64 {
            2
        }
    }

    /// The verse-wide scope every fixture session pins as its `epoch_scope`.
    fn sample_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x11; 32]))
    }

    /// A petal inside [`sample_scope`], so containment can be exercised in both directions.
    fn narrow_scope() -> Scope {
        Scope::new(
            Identifier32([0x11; 32]),
            Some(Identifier32([0x22; 32])),
            None,
        )
        .expect("petal scope")
    }

    /// A scope in another verse entirely — outside any fixture session's `epoch_scope`.
    fn foreign_scope() -> Scope {
        Scope::verse_wide(Identifier32([0x99; 32]))
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

    /// The chain bytes every fixture handshake presents.
    fn sample_chain_bytes() -> Vec<u8> {
        vec![7, 7, 7]
    }

    fn sample_authorization() -> VerifiedAuthorization {
        VerifiedAuthorization {
            leaf_principal: test_principal(0x02),
            chain_id: Hash32([0xaa; 32]),
            epoch_scope: sample_scope(),
            scope_epoch: 1,
            expires_at_ms: u64::MAX,
        }
    }

    /// A pinned session over `epoch_scope`, whose chain and principal match the binding
    /// [`accept_sample_binding`] accepts.
    fn pinned_session(epoch_scope: Scope, subscribed_scopes: Vec<Scope>) -> PinnedSession {
        PinnedSession {
            leaf_principal: test_principal(0x02),
            chain_id: Hash32([0xaa; 32]),
            leaf_certificate_id: Hash32([0xaa; 32]),
            epoch_scope,
            epoch: 1,
            expires_at_ms: u64::MAX,
            subscribed_scopes,
        }
    }

    fn pinned_status(epoch_scope: Scope, subscribed_scopes: Vec<Scope>) -> PinnedSessionStatus {
        PinnedSessionStatus::Valid {
            session: pinned_session(epoch_scope, subscribed_scopes),
            authorization_binding_id: [1; 16],
            established: EstablishedAuthorization::new(sample_chain_bytes(), 0x01, 0x01),
        }
    }

    fn canonical_state_with(
        capability_verifier: Arc<dyn CapabilityVerifier>,
        commit_pipeline: Arc<dyn CanonicalCommitPipeline>,
        registry: Arc<InMemoryBranchRegistry>,
    ) -> CanonicalLogState {
        let branch_registry: Arc<dyn BranchRegistry> = registry.clone();
        let snapshot_source: Arc<dyn ScopeSnapshotSource> =
            Arc::new(MockScopeSnapshotSource::new(registry, Vec::new()));
        let view: Arc<dyn AuthorizationView> = Arc::new(FixedAuthorizationView);
        CanonicalLogState::new(
            commit_pipeline,
            branch_registry,
            snapshot_source,
            capability_verifier,
            view,
            PreviewRateLimiter::new(PreviewRateLimit::new(10, 1_000)),
            4,
            4,
        )
    }

    fn inert_pipeline() -> Arc<dyn CanonicalCommitPipeline> {
        Arc::new(ScriptedCommitPipeline {
            derived_op_id: Hash32([0; 32]),
            result: PipelineResult::AcceptedPendingMaterialization,
        })
    }

    fn test_canonical_state() -> CanonicalLogState {
        canonical_state_with(
            Arc::new(MockCapabilityVerifier::new()),
            inert_pipeline(),
            Arc::new(InMemoryBranchRegistry::new()),
        )
    }

    /// A state whose verifier already accepts [`sample_chain_bytes`], plus a handle to that
    /// verifier so a test can count the calls the dispatch path actually made.
    fn state_with_registered_chain() -> (CanonicalLogState, Arc<MockCapabilityVerifier>) {
        let verifier = Arc::new(MockCapabilityVerifier::new());
        verifier.register(sample_chain_bytes(), sample_authorization());
        let state = canonical_state_with(
            verifier.clone(),
            inert_pipeline(),
            Arc::new(InMemoryBranchRegistry::new()),
        );
        (state, verifier)
    }

    fn empty_auth() -> ConnectionAuthState {
        ConnectionAuthState {
            sessions: Arc::new(RwLock::new(SessionAuthorizationTable::new())),
            pinned: Arc::new(RwLock::new(PinnedSessionStatus::Unauthorized)),
            subscriptions: SubscriptionTable::new(),
            frontier_ledgers: HashMap::new(),
            minted_snapshot_cursors: HashMap::new(),
        }
    }

    /// Accepts the binding every fixture pinned session names, returning its generation.
    async fn accept_sample_binding(auth: &ConnectionAuthState) -> u64 {
        auth.sessions
            .write()
            .await
            .accept_binding([1; 16], Hash32([0xaa; 32]), [0x02; 32])
            .expect("accept sample binding")
    }

    fn sample_subscription_record(scope: Scope) -> SubscriptionRecord {
        SubscriptionRecord {
            authorization_binding_id: [1; 16],
            branch_id: Identifier32([0x22; 32]),
            scope,
            projection_identity: sample_projection(),
        }
    }

    fn subscribe_body(session_generation: u64, scope: Scope) -> SubscribeBody {
        SubscribeBody {
            session_generation,
            authorization_binding_id: [1; 16],
            subscription_id: [7; 16],
            branch_id: Identifier32([0x22; 32]),
            scope,
            projection_identity: sample_projection(),
        }
    }

    /// Whether the pinned session would now let `scope` through `PinnedSession::covers`.
    async fn covers(auth: &ConnectionAuthState, scope: Scope) -> bool {
        match &*auth.pinned.read().await {
            PinnedSessionStatus::Valid { session, .. } => session.covers(&scope),
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // commit_submit
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn a_stale_session_generation_is_rejected_before_commit_pipeline_work() {
        let canonical = test_canonical_state();
        let auth = empty_auth();
        *auth.pinned.write().await = pinned_status(sample_scope(), vec![sample_scope()]);

        let body = CommitSubmitBody {
            session_generation: 999, // never accepted
            authorization_binding_id: [1; 16],
            claimed_op_id: Hash32([1; 32]),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body, 0).await;
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
        let generation = accept_sample_binding(&auth).await;

        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id: Hash32([1; 32]),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body, 0).await;
        assert_eq!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Rejected {
                category: ProtocolErrorCategory::AuthorizationRevalidationRequired
            }
        );
    }

    /// A binding ID the connection never accepted names no authority, however current the
    /// session generation is.
    #[tokio::test]
    async fn a_commit_naming_a_binding_that_is_not_the_pinned_session_is_refused() {
        let canonical = test_canonical_state();
        let auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), vec![sample_scope()]);

        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [2; 16], // never accepted on this connection
            claimed_op_id: Hash32([1; 32]),
            complete_envelope: vec![1, 2, 3],
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body, 0).await;
        assert_eq!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Rejected {
                category: ProtocolErrorCategory::ScopeNotAuthorized
            }
        );
    }

    #[tokio::test]
    async fn a_pinned_and_current_commit_reaches_the_pipeline() {
        let registry = Arc::new(InMemoryBranchRegistry::new());
        let canonical = canonical_state_with(
            Arc::new(MockCapabilityVerifier::new()),
            Arc::new(MockCommitPipeline::new(registry.clone(), sample_tuple())),
            registry,
        );
        let auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), vec![sample_scope()]);

        let bytes = vec![9, 9, 9];
        let claimed_op_id = Hash32::of(&bytes);
        let body = CommitSubmitBody {
            session_generation: generation,
            authorization_binding_id: [1; 16],
            claimed_op_id,
            complete_envelope: bytes,
            payload_ciphertext: None,
        };
        let ack = dispatch_commit_submit(&canonical, &auth, &body, 0).await;
        assert!(matches!(
            ack.state,
            fe_canonical_log::wire::commit::CommitAckState::Committed(_)
        ));
    }

    // -----------------------------------------------------------------------
    // subscribe — the C1 authorization gate
    // -----------------------------------------------------------------------

    /// The C1 regression. `subscribe` used to push its peer-supplied scope straight into
    /// `subscribed_scopes`, the set `PinnedSession::covers` reads; a scope outside the pinned
    /// `epoch_scope` must never reach it.
    #[tokio::test]
    async fn subscribe_refuses_a_scope_outside_the_pinned_epoch_scope() {
        let (canonical, verifier) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(narrow_scope(), Vec::new());

        for outside in [sample_scope(), foreign_scope()] {
            assert_eq!(
                dispatch_subscribe(
                    &canonical,
                    &mut auth,
                    &subscribe_body(generation, outside),
                    0
                )
                .await,
                Err(ProtocolErrorCategory::ScopeNotAuthorized)
            );
            assert!(!covers(&auth, outside).await);
        }
        assert!(auth.subscriptions.get([7; 16]).is_none());
        assert!(
            verifier.calls().is_empty(),
            "containment is checked before the verifier is consulted"
        );
    }

    /// A binding ID the connection never accepted, or one accepted for another chain, is not
    /// this session's authority — the check `subscribe` never performed.
    #[tokio::test]
    async fn subscribe_refuses_a_binding_id_that_does_not_resolve_to_the_pinned_session() {
        let (canonical, _) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), Vec::new());

        let mut body = subscribe_body(generation, sample_scope());
        body.authorization_binding_id = [9; 16];
        assert_eq!(
            dispatch_subscribe(&canonical, &mut auth, &body, 0).await,
            Err(ProtocolErrorCategory::ScopeNotAuthorized)
        );
        assert!(!covers(&auth, sample_scope()).await);
    }

    #[tokio::test]
    async fn subscribe_refuses_an_unpinned_or_invalidated_connection() {
        let (canonical, _) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;

        for status in [
            PinnedSessionStatus::Unauthorized,
            PinnedSessionStatus::Invalidated,
        ] {
            *auth.pinned.write().await = status;
            assert_eq!(
                dispatch_subscribe(
                    &canonical,
                    &mut auth,
                    &subscribe_body(generation, sample_scope()),
                    0
                )
                .await,
                Err(ProtocolErrorCategory::AuthorizationRevalidationRequired)
            );
        }
    }

    /// The verifier's verdict is load-bearing, not decorative: a pinned session whose chain the
    /// verifier will not re-verify cannot subscribe even to a scope inside its `epoch_scope`.
    #[tokio::test]
    async fn subscribe_refuses_when_the_verifier_will_not_re_verify_the_chain() {
        // No chain is registered with this verifier, so every `verify` call fails.
        let canonical = test_canonical_state();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), Vec::new());

        assert_eq!(
            dispatch_subscribe(
                &canonical,
                &mut auth,
                &subscribe_body(generation, narrow_scope()),
                0
            )
            .await,
            Err(ProtocolErrorCategory::ScopeNotAuthorized)
        );
        assert!(auth.subscriptions.get([7; 16]).is_none());
        assert!(!covers(&auth, narrow_scope()).await);
    }

    /// The positive path: the scope becomes covered only after the real verifier re-verified
    /// the handshake's own chain bytes against it.
    #[tokio::test]
    async fn an_authorized_subscribe_re_verifies_the_chain_before_the_scope_becomes_covered() {
        let (canonical, verifier) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), Vec::new());
        assert!(!covers(&auth, narrow_scope()).await);

        assert_eq!(
            dispatch_subscribe(
                &canonical,
                &mut auth,
                &subscribe_body(generation, narrow_scope()),
                0
            )
            .await,
            Ok(())
        );
        assert_eq!(
            verifier.calls(),
            vec![sample_chain_bytes()],
            "the subscribe path presented the handshake's chain bytes to the real verifier"
        );
        assert!(covers(&auth, narrow_scope()).await);
        assert!(auth.subscriptions.get([7; 16]).is_some());
    }

    #[tokio::test]
    async fn subscribe_refuses_a_stale_session_generation() {
        let (canonical, _) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), Vec::new());

        assert_eq!(
            dispatch_subscribe(
                &canonical,
                &mut auth,
                &subscribe_body(generation + 1, sample_scope()),
                0
            )
            .await,
            Err(ProtocolErrorCategory::SessionGenerationInvalid)
        );
    }

    #[tokio::test]
    async fn subscribe_stops_at_the_per_connection_subscription_ceiling() {
        let (canonical, _) = state_with_registered_chain();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), Vec::new());

        for index in 0..MAX_SUBSCRIPTIONS_PER_CONNECTION {
            let mut body = subscribe_body(generation, sample_scope());
            body.subscription_id = [index as u8; 16];
            body.branch_id = Identifier32([index as u8; 32]);
            assert_eq!(
                dispatch_subscribe(&canonical, &mut auth, &body, 0).await,
                Ok(())
            );
        }
        let mut overflow = subscribe_body(generation, sample_scope());
        overflow.subscription_id = [0xfe; 16];
        assert_eq!(
            dispatch_subscribe(&canonical, &mut auth, &overflow, 0).await,
            Err(ProtocolErrorCategory::ScopeNotAuthorized)
        );
        assert_eq!(
            auth.subscriptions.iter().count(),
            MAX_SUBSCRIPTIONS_PER_CONNECTION
        );
    }

    // -----------------------------------------------------------------------
    // authorize
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn authorize_consults_the_cache_before_re_verifying_an_identical_chain() {
        let mut auth = empty_auth();
        // The helper hands `CanonicalLogState` an erased clone and keeps an
        // `Arc<MockCapabilityVerifier>` here, so the concrete `.calls()` inspector stays
        // reachable — that count is the whole assertion.
        let (canonical, verifier) = state_with_registered_chain();

        let body = AuthorizeBody {
            capability_chain_bytes: sample_chain_bytes(),
            authorization_binding_id: [3; 16],
            requested_verb: 0x01,
            requested_object_class: 0x01,
            requested_scope: sample_scope(),
        };
        let first = dispatch_authorize(&canonical, &mut auth, &body, 0).await;
        assert!(first.is_ok());
        let second = dispatch_authorize(&canonical, &mut auth, &body, 0).await;
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

    /// A subscription is authority granted by one session; a replacement session must not
    /// inherit it. Without this, a re-`authorize` under a narrower chain would keep every
    /// subscription the superseded session established.
    #[tokio::test]
    async fn a_fresh_authorize_drops_the_superseded_sessions_subscriptions() {
        let (canonical, _) = state_with_registered_chain();
        let mut auth = empty_auth();
        auth.subscriptions
            .bind([7; 16], sample_subscription_record(sample_scope()))
            .expect("bind");
        auth.frontier_ledgers.entry([7; 16]).or_default();
        auth.minted_snapshot_cursors.insert(
            [7; 16],
            cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]),
        );

        let body = AuthorizeBody {
            capability_chain_bytes: sample_chain_bytes(),
            authorization_binding_id: [3; 16],
            requested_verb: 0x01,
            requested_object_class: 0x01,
            requested_scope: sample_scope(),
        };
        assert!(dispatch_authorize(&canonical, &mut auth, &body, 0)
            .await
            .is_ok());

        assert_eq!(auth.subscriptions.iter().count(), 0);
        assert!(auth.frontier_ledgers.is_empty());
        assert!(auth.minted_snapshot_cursors.is_empty());
        assert!(!covers(&auth, sample_scope()).await);
    }

    /// The pin depends on two facts the verifier is not an authority over — the clock and the
    /// durable epoch — so both are re-read against the live view before a session is pinned.
    #[tokio::test]
    async fn authorize_refuses_to_pin_a_session_the_live_view_no_longer_honors() {
        let verifier = Arc::new(MockCapabilityVerifier::new());
        let mut expiring = sample_authorization();
        expiring.expires_at_ms = 1_000;
        verifier.register(sample_chain_bytes(), expiring);
        let canonical = canonical_state_with(
            verifier,
            inert_pipeline(),
            Arc::new(InMemoryBranchRegistry::new()),
        );
        let mut auth = empty_auth();

        let body = AuthorizeBody {
            capability_chain_bytes: sample_chain_bytes(),
            authorization_binding_id: [3; 16],
            requested_verb: 0x01,
            requested_object_class: 0x01,
            requested_scope: sample_scope(),
        };
        assert_eq!(
            dispatch_authorize(&canonical, &mut auth, &body, 1_000).await,
            Err(ProtocolErrorCategory::AuthorizationRevalidationRequired)
        );
        assert!(matches!(
            &*auth.pinned.read().await,
            PinnedSessionStatus::Unauthorized
        ));
    }

    // -----------------------------------------------------------------------
    // resume, snapshot_ack, and the frontier ledger
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resume_with_no_frontier_ledger_falls_back_to_snapshot_required() {
        let canonical = test_canonical_state();
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), vec![sample_scope()]);
        auth.subscriptions
            .bind([7; 16], sample_subscription_record(sample_scope()))
            .expect("bind");
        // Deliberately no `snapshot_ack`/delivered deltas recorded — the ledger is empty, so
        // the peer-supplied cursor cannot be corroborated (gate obligation 1).

        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [7; 16],
            prior_cursor: cursor_with_claim(sample_tuple(), Hash32([0x55; 32]), vec![9]),
        };
        let outcome = dispatch_resume(&canonical, &auth, &body, 0)
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
        let canonical = canonical_state_with(
            Arc::new(MockCapabilityVerifier::new()),
            inert_pipeline(),
            registry.clone(),
        );
        let mut auth = empty_auth();
        let generation = accept_sample_binding(&auth).await;
        *auth.pinned.write().await = pinned_status(sample_scope(), vec![sample_scope()]);
        auth.subscriptions
            .bind([7; 16], sample_subscription_record(sample_scope()))
            .expect("bind");
        // `snapshot_cursor` resolves a scope only once the registry holds a delta for it, and
        // `resolve_resume` rebuilds this exact tuple from the subscription record above.
        registry.commit(sample_tuple(), Hash32([0x33; 32]), Vec::new());
        let snapshot_cursor = registry
            .snapshot_cursor(&sample_scope())
            .await
            .expect("snapshot cursor");
        // Only a cursor this connection actually sent may become a trusted baseline.
        record_minted_snapshot_cursor(&mut auth, [7; 16], snapshot_cursor.clone());
        dispatch_snapshot_ack(
            &mut auth,
            &SnapshotAckBody {
                session_generation: generation,
                subscription_id: [7; 16],
                snapshot_cursor: snapshot_cursor.clone(),
            },
        )
        .expect("acknowledging our own cursor is accepted");

        let body = ResumeBody {
            session_generation: generation,
            subscription_id: [7; 16],
            prior_cursor: snapshot_cursor,
        };
        let outcome = dispatch_resume(&canonical, &auth, &body, 0)
            .await
            .expect("no protocol error");
        assert!(matches!(outcome, ResumeOutcome::Replayed { .. }));
    }

    /// `verify_resume_cursor_frontier` trusts a baseline because it never left this
    /// connection's hands. That only holds if `snapshot_ack` refuses to adopt a cursor the peer
    /// invented — otherwise a peer nominates its own baseline and walks past the gate.
    #[tokio::test]
    async fn a_snapshot_ack_naming_a_cursor_this_connection_never_minted_is_refused() {
        let mut auth = empty_auth();
        auth.subscriptions
            .bind([7; 16], sample_subscription_record(sample_scope()))
            .expect("bind");

        let fabricated = cursor_with_claim(sample_tuple(), Hash32([0xee; 32]), vec![5]);
        assert_eq!(
            dispatch_snapshot_ack(
                &mut auth,
                &SnapshotAckBody {
                    session_generation: 1,
                    subscription_id: [7; 16],
                    snapshot_cursor: fabricated.clone(),
                },
            ),
            Err(ProtocolErrorCategory::CursorInvalid)
        );
        assert!(auth.frontier_ledgers.is_empty());

        // Even after a real snapshot was minted, only that exact cursor is acknowledgeable.
        let minted = cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]);
        record_minted_snapshot_cursor(&mut auth, [7; 16], minted.clone());
        assert_eq!(
            dispatch_snapshot_ack(
                &mut auth,
                &SnapshotAckBody {
                    session_generation: 1,
                    subscription_id: [7; 16],
                    snapshot_cursor: fabricated,
                },
            ),
            Err(ProtocolErrorCategory::CursorInvalid)
        );
        assert_eq!(
            dispatch_snapshot_ack(
                &mut auth,
                &SnapshotAckBody {
                    session_generation: 1,
                    subscription_id: [7; 16],
                    snapshot_cursor: minted.clone(),
                },
            ),
            Ok(())
        );
        assert!(
            verify_resume_cursor_frontier(auth.frontier_ledgers.get(&[7; 16]), &minted).is_ok()
        );
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

    /// An unbounded `observed_op_ids` is a per-connection memory sink. Once the ledger stops
    /// being a complete record it refuses to corroborate rather than recomputing over a
    /// truncated set, which would silently look like a verdict.
    #[test]
    fn an_overflowed_frontier_ledger_corroborates_nothing_but_its_own_baseline() {
        let mut ledger = SubscriptionFrontierLedger::default();
        let baseline = cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]);
        ledger.reset_to_snapshot(baseline.clone());
        for index in 0..(MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION + 8) {
            let mut op_id = [0u8; 32];
            op_id[..8].copy_from_slice(&(index as u64).to_be_bytes());
            ledger.record_delivered(Hash32(op_id));
        }
        assert_eq!(
            ledger.observed_op_ids.len(),
            MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION
        );
        assert!(ledger.observation_overflowed);

        assert!(verify_resume_cursor_frontier(Some(&ledger), &baseline).is_ok());
        let claimed = cursor_with_claim(sample_tuple(), Hash32([0x01; 32]), vec![1]);
        assert!(verify_resume_cursor_frontier(Some(&ledger), &claimed).is_err());

        // A fresh snapshot restores a complete record.
        ledger.reset_to_snapshot(baseline);
        assert!(!ledger.observation_overflowed);
    }

    // -----------------------------------------------------------------------
    // committed-delta fan-out
    // -----------------------------------------------------------------------

    fn sample_delta(scope: Scope) -> CommittedDelta {
        CommittedDelta {
            op_id: Hash32([0x77; 32]),
            branch_id: Identifier32([0x22; 32]),
            scope,
            projection_identity: sample_projection(),
            cursor: cursor_with_claim(sample_tuple(), Hash32::of_empty(), vec![0]),
            change_summary: Vec::new(),
        }
    }

    /// Delta fan-out is a protected path. A session the live view has revoked, and a
    /// subscription the pinned session does not cover, must both stop receiving committed
    /// state — not merely stop being able to commit.
    #[test]
    fn a_revoked_or_uncovered_session_is_forwarded_no_committed_deltas() {
        let mut subscriptions = SubscriptionTable::new();
        subscriptions
            .bind([7; 16], sample_subscription_record(sample_scope()))
            .expect("bind");
        let delta = sample_delta(sample_scope());

        let covered = pinned_status(sample_scope(), vec![sample_scope()]);
        assert_eq!(
            subscriptions_authorized_for_delta(
                &covered,
                &subscriptions,
                &delta,
                0,
                &FixedAuthorizationView
            ),
            vec![[7; 16]],
        );

        // Same subscription table, same delta — only the live view changed.
        assert!(subscriptions_authorized_for_delta(
            &covered,
            &subscriptions,
            &delta,
            0,
            &RevokedAuthorizationView
        )
        .is_empty());

        // A subscription bound to a scope the pinned session never covered.
        let uncovered = pinned_status(sample_scope(), vec![narrow_scope()]);
        assert!(subscriptions_authorized_for_delta(
            &uncovered,
            &subscriptions,
            &delta,
            0,
            &FixedAuthorizationView
        )
        .is_empty());

        for status in [
            PinnedSessionStatus::Unauthorized,
            PinnedSessionStatus::Invalidated,
        ] {
            assert!(subscriptions_authorized_for_delta(
                &status,
                &subscriptions,
                &delta,
                0,
                &FixedAuthorizationView
            )
            .is_empty());
        }
    }
}
