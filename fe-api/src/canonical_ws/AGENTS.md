# canonical_ws — SPEC-7 commit/preview wire transport (W3-api-canonical-ws)

Implements `docs/spec/canonical-log/commit-preview-wire.md` §2, §4-§7 as an axum WebSocket
surface over `fe_canonical_log::wire`'s frame codec and body types. No socket, database, or
network code lives in `fe_canonical_log` itself — that crate is transport-agnostic by design
(see `fe-canonical-log/src/wire/AGENTS.md`); this module is the wave-3 transport it names.

## Module map

| File | Contents |
| --- | --- |
| `state.rs` | `CanonicalLogState`: the five commit-side `Arc<dyn Trait>` authorities, the preview limiter/broadcast channel, and the §5.3 verification cache + revalidation gate (behind one lock) with its invalidation methods |
| `handler.rs` | Connection lifecycle: commit-class dispatch functions, the structurally separate `preview_task` submodule, and `handle_socket`'s frame loop |
| `router.rs` | `build_router_with_canonical_log` — nests the module at `/ws/canonical` on top of the existing router, unchanged |

## §deliberately-unmounted

`build_router_with_canonical_log` is never called from `fractalengine/src/main.rs`,
`fractalengine-relay`, or any other startup file. Wiring it into a running binary — choosing an
`ApiConfig`-analogous construction site, picking real `CanonicalCommitPipeline` /
`BranchRegistry` / `ScopeSnapshotSource` / `CapabilityVerifier` / `AuthorizationView`
implementations (all four wire traits plus the capability view are dormant test-double-only
here; the real ones are `fe-database`'s job, a different wave-3 slice), and deciding the
`/ws/canonical` route's auth story — is an explicit, owner-gated decision this slice does not
make. The endpoint ships written, compiled, colocated-tested, and structurally sound; it is not
reachable from any process today. `router.rs`'s own tests exercise this via `tower::oneshot`
rather than a live listener, so the "unmounted" property is verified without contradicting it.

## Why the legacy `/ws` protocol is untouched rather than extended

`crate::ws` speaks a JSON, `serde(tag = "type")` protocol keyed on `WsClientMsg`/`WsServerMsg`
with its own auth handshake, `scene_version` counter, and `SceneChange` broadcast. SPEC-7 is a
different protocol: deterministic CBOR frames, SPEC-3 capability chains instead of bearer JWTs,
opaque registry-issued durable cursors instead of an incrementing counter, and a preview class
that must be structurally incapable of reaching commit machinery — none of which the JSON
protocol's message enum or auth flow can express without becoming a second protocol wearing the
first one's clothes. Extending `WsClientMsg`/`WsServerMsg` with SPEC-7 variants would put both
protocols behind one decode path and one auth gate, defeating the preview isolation and
capability-scoping properties SPEC-7 requires. `crate::ws::WsClientMsg`, `WsServerMsg`,
`ws_handler`, and every function in `fe-api/src/ws.rs` are unedited by this slice; the two
protocols share only the underlying `axum::extract::ws::WebSocket` transport.

## No new networking

This module adds no listener, no port, no relay, no iroh/P2P surface. `/ws/canonical` would
ride the same `axum::serve(listener, router)` call in `fe-api/src/lib.rs::run_server` that
already serves `/ws`, `/api/v1/*`, and everything else — a new `.nest()` on the same router,
nothing more. `router.rs` calls `crate::server::build_router(state)` unchanged and only adds a
sibling route tree; `fe-api/src/server.rs` is not edited by this slice.

## §preview-disjointness: a distinct task, not a runtime flag

`handler.rs`'s top-level `use` block imports `wire::commit`, `wire::cursor`, `wire::snapshot`,
and `wire::subscription` for the commit-class dispatch functions (`dispatch_authorize`,
`dispatch_commit_submit`, `dispatch_subscribe`, `dispatch_resume`, `dispatch_snapshot_ack`,
`dispatch_snapshot_fanout`). The nested `pub mod preview_task` has its **own** `use` block that
imports only `wire::error`, `wire::preview`, `wire::preview_limiter`, and `wire::session` (for
`SessionAuthorizationTable`) — Rust module scoping means `preview_task` does not inherit
`handler`'s imports, so no unqualified name inside it resolves to `CanonicalCommitPipeline`,
`BranchRegistry`, `ScopeSnapshotSource`, `CommittedDelta`, or `SubscriptionTable`. Its only
public type, `PreviewTaskState`, holds exactly four fields — a shared `SessionAuthorizationTable`,
a shared pin-status cell, the preview rate limiter, and the preview broadcast sender — and its
constructor is `pub(super)`, callable only from `handle_socket`'s connection setup, never from
inside `preview_task` itself.

`handle_socket` makes this a real `tokio::spawn`ed task, not merely a separate function: a
`preview_task::run(state, inbox)` future is spawned once per connection, and the connection's
main loop only ever sends it `(PreviewSendBody, oneshot::Sender<...>)` pairs over an
`mpsc::UnboundedSender`. The spawned task's captured environment is therefore *exactly*
`PreviewTaskState` plus the channel receiver — it cannot reach `CanonicalLogState`,
`ConnectionAuthState`, or any of the four commit-side trait objects, because nothing in its
compilation unit ever named them. This mirrors `fe-canonical-log/src/wire/AGENTS.md`'s own
"module boundary, not a runtime check" argument for `preview.rs`/`preview_limiter.rs`, one layer
up at the transport.

`SessionAuthorizationTable` and the pin-status cell are intentionally the two exceptions,
`Arc<RwLock<_>>`-shared between the commit-class task and the preview task: both halves must
agree on one session generation and one pin status (SPEC-7 §6 rule 3; `capability/AGENTS.md`
§5.3 obligation 3), and duplicating either into two independently-mutated copies would let them
drift — a stale preview task believing a session still valid after the commit-class task
revoked it. Sharing exactly these two, and nothing else, is what keeps the preview task
"structurally separate" rather than merely "differently named."

## The four gate obligations this slice owns

1. **`wire::cursor::verify_frontier_commitment`** — called at `handler.rs`'s
   `verify_resume_cursor_frontier`, the first thing `dispatch_resume` does with a peer-supplied
   cursor, before `resolve_resume`/`BranchRegistry::replay_after` ever sees it. See
   §wave-3-obligation-frontier-commitment below for what this connection-scoped check can and
   cannot prove.
2. **`SessionAuthorizationTable` as a required parameter on all four protected paths** — commit
   (`dispatch_commit_submit` → `handle_commit_submit`), replay (`dispatch_resume` →
   `resolve_resume`), snapshot (`dispatch_snapshot_fanout` →
   `snapshot_all_authorized_subscriptions`), and preview
   (`preview_task::dispatch_preview_send` → `PreviewRateLimiter::check_and_record`) all take
   `&SessionAuthorizationTable`, never `Option<&SessionAuthorizationTable>` — the wire-layer
   functions themselves already refuse to compile without it.
3. **`capability::AdmittedDecision` + `RevalidationGate::admitted_now`** —
   `CanonicalLogState::cached_verification` (called from `dispatch_authorize` before
   `CapabilityVerifier::verify`) never returns a cached `VerifiedAuthorization` without going
   through `admitted_now`, which reads expiry, the live durable epoch, and the live
   authority-view version itself. fe-api stores no `CacheKey` and builds none;
   `CanonicalLogState::admit_verification` (via `RevalidationGate::admit_verified`) is the only
   path that inserts into either the cache or the gate. See
   §5.3-the-cache-is-recomputed-not-remembered.
4. **`PinnedSession::is_still_valid` + `PinnedSession::covers`** — every protected path calls
   `is_still_valid` against the live `AuthorizationView` *inline*, on the frame itself:
   `dispatch_commit_submit`, `dispatch_subscribe`, `dispatch_resume`,
   `dispatch_snapshot_fanout`'s per-subscription closure, and
   `subscriptions_authorized_for_delta`. The five-second `tokio::time::interval` in
   `handle_socket` is a second, traffic-independent line: `revalidate_pinned_session` flips the
   shared status to `Invalidated` and invalidates the session generation so a client is *told*
   about a revocation it would otherwise only discover by being refused.
   `PinnedSession::covers` gates `dispatch_resume`, `dispatch_snapshot_fanout`,
   `subscriptions_authorized_for_delta`, and `preview_task::dispatch_preview_send`; what may
   *enter* the set it reads is §subscribe-re-verifies-the-chain below.

   Exception, stated rather than papered over: `preview_task` holds no `AuthorizationView`
   (see §preview-disjointness for why its captured state is deliberately minimal), so
   `dispatch_preview_send` has no inline live re-check. Preview revocation latency is therefore
   bounded by the five-second timer, not immediate. Previews never touch committed history.

## §subscribe-re-verifies-the-chain

`subscribed_scopes` is the only set `PinnedSession::covers` consults, so whatever enters it
*is* the authorization decision for delta fan-out, replay, snapshot, and preview. A
peer-supplied `subscribe.scope` is therefore treated as authorization input, never as an
authorization decision. `dispatch_subscribe` clears, in order:

1. the session generation is the current accepted one;
2. `authorization_binding_id` **resolves** in this connection's `SessionAuthorizationTable`
   *and* names the pinned session's own `chain_id` and leaf-principal public key
   (§binding-ids-are-resolved-not-echoed);
3. `PinnedSession::is_still_valid` against the live `AuthorizationView`;
4. `session.epoch_scope.contains(&body.scope)` — the requested scope is inside the scope the
   handshake's chain root establishes;
5. `CapabilityVerifier::verify` re-runs over the handshake's **exact chain bytes** (kept in
   `PinnedSessionStatus::Valid`'s `established: EstablishedAuthorization`) against
   `body.scope`, using the verb and object-class bits the handshake was verified for.

Only then does the scope enter `subscribed_scopes`. Check 5 is why the chain bytes are held for
the life of the pin: without them a `subscribe` for a scope the handshake never presented could
only ever be checked against fe-api's own bookkeeping, which is exactly the shape of the defect
this replaced. Check 5 is conservative by construction — it demands the same verb/object-class
authority the handshake demanded, so it can never grant more than `authorize` granted.

Note what check 5 is *not*: `authorize`'s own `requested_scope` is not re-derived from the
subscribe scope, so a connection that authorized narrowly and subscribes narrowly pays one
extra `verify` per `subscribe`. That call is served by the §5.3 cache only when the request
tuple matches exactly (see below), which for a differing scope it does not — the cost is
deliberate.

## §binding-ids-are-resolved-not-echoed

`authorization_binding_id` appears on `commit_submit`, `subscribe`, and `preview_send`.
`binding_matches_pinned_session` is the single function that gives it meaning: it looks the ID
up in this connection's `SessionAuthorizationTable` and requires the record to name the pinned
session's `chain_id` and leaf-principal public key. An ID this connection never accepted, or one
accepted under a co-resident binding for a different principal, fails.

This matters most in `preview_task::dispatch_preview_send`, where the emitted
`PreviewDeltaBody::sender_principal` is taken from the **pinned session**, never from the
binding the frame named — a connection may hold more than one accepted binding, and reading the
sender out of the named one is an impersonation primitive.

## §5.3-the-cache-is-recomputed-not-remembered

`CacheKey` has five dimensions (chain, epoch scope, epoch, expiry, authority-view version).
Storing one and later testing set membership of the stored value proves only that it was once
admitted — the stored version dimension makes the key answer `true` forever, however far the
view has moved.

fe-api therefore never stores a `CacheKey` and never builds one. A cache entry holds the four
view-independent dimensions (`capability::AdmittedDecision`) and nothing else;
`RevalidationGate::admitted_now` reads the expiry, the live `current_epoch`, and the live
`version` itself and rebuilds the key from them. `RevalidationGate::admit_verified` does the
same on the write side. Both doors are the sibling capability slice's, chosen over
`is_admitted`/`admit` precisely because the caller cannot supply — and so cannot staleness — the
dimension the gate exists to invalidate on. Any refusal is a miss; a miss costs a full
`CapabilityVerifier::verify`, so re-sending byte-identical `authorize` bytes after a revocation
cannot inherit the pre-revocation answer. A refused entry is evicted on the spot rather than
left to be re-refused.

The cache is keyed by `VerificationRequest` — `(BLAKE3(chain bytes), requested_verb,
requested_object_class, requested_scope)` — not by the chain digest alone. A digest-only key
would answer a request for one scope with a decision made for another, which is the same class
of confusion as trusting the stored key.

`RevalidationGate` and the cache map live behind **one** mutex (`VerificationCache`), because
two locks are two acquisition orders and `cached_verification`/`admit_verification` traverse
them in opposite directions.

Invalidation is a real path, not a latent capability: `CanonicalLogState::invalidate_on_epoch_bump`
(for a future `scope_epoch_bump` handler), `invalidate_expired`, `invalidate_stale_view`, and
`sweep_revocations` — the last of which `revalidate_pinned_session` calls on every five-second
tick, so expiry and authority-view advances evict without waiting for a caller to happen to
miss.

## §bounded-state

Every peer-driven collection has a ceiling, because an unmounted endpoint that a future caller
mounts should not also be a memory sink:

- `MAX_SUBSCRIPTIONS_PER_CONNECTION` (64) caps `ConnectionAuthState::subscriptions`, and with
  it `frontier_ledgers` and `minted_snapshot_cursors`, which are only ever keyed by a bound
  subscription.
- `MAX_OBSERVED_OP_IDS_PER_SUBSCRIPTION` (4096) caps one ledger's `observed_op_ids`. On
  overflow the ledger sets `observation_overflowed` and `verify_resume_cursor_frontier` refuses
  every cursor but the trusted baseline — recomputing a commitment over a knowingly truncated
  set would produce a mismatch that *looks* like a verdict.
- `MAX_CACHED_VERIFICATIONS` (1024) caps the process-wide verification cache. At the ceiling
  `admit_verification` simply declines to remember a decision that already succeeded: latency,
  never authority.
- A `subscribe` fans a snapshot out to the record it just bound (`dispatch_snapshot_fanout`'s
  `only` argument), not to every subscription — one frame must not multiply into N
  snapshot-source reads.

## §wave-3-obligation-frontier-commitment

The wire crate's own `DurableCursor` carries only a `frontier_commitment` hash and an opaque
`delivery_position` — never the member `op_id`s the hash was computed over (`wire/AGENTS.md`'s
"cursor opacity is deliberate"), and `BranchRegistry`'s five methods (`validate`/`compare`/
`replay_after`/`snapshot_cursor`/`issue_cursor`) never return that member set either. A
`fe-api`-only caller therefore cannot recompute a cursor's commitment from genuine first
principles the way `InMemoryBranchRegistry::validate` can (it holds the full committed-delta
list itself).

What fe-api genuinely can verify is narrower and is exactly what `SubscriptionFrontierLedger`
does: each connection tracks, per subscription, the exact cursor it last told the client to
treat as a trusted baseline and the `op_id` of every delta it has itself forwarded since. A
`resume` cursor that is byte-identical to that trusted baseline needs no recomputation (it never
left this connection's own hands unverified); any other claimed cursor is checked with the real
`verify_frontier_commitment` against the connection's own observed `op_id` set, and an
unrecognized subscription (no ledger at all) is refused outright rather than treated as an
implicit pass.

**The baseline shortcut is only sound because `snapshot_ack` refuses to adopt a cursor this
connection did not mint.** `handle_frame` and `recover_from_broadcast_lag` record every
`scene_snapshot` cursor they actually send in `ConnectionAuthState::minted_snapshot_cursors`,
and `dispatch_snapshot_ack` promotes `body.snapshot_cursor` to `trusted_baseline` only when it
equals the recorded one; anything else is `CursorInvalid`. Taking the peer's cursor at its word
would let the peer nominate its own trusted baseline and then present that same cursor to
`resume`, walking past the gate entirely — the shortcut's premise ("it never left this
connection") would simply be false.

This catches a cursor whose claimed commitment is inconsistent with what *this connection*
witnessed being delivered. It does **not** independently prove a cursor minted in a prior
session or another connection, before this ledger existed — that guarantee still requires the
real `BranchRegistry::validate` (an `fe-database` implementation, not built here) to hold, since
only a registry backed by durable storage knows the true member set at an arbitrary position.
Both checks are meant to compose: this one is cheap and connection-local,
`registry.validate()` (already called inside `resolve_resume`) remains authoritative.

## Known limitations

- **`PinnedSession::leaf_certificate_id` stands in as `chain_id`.** `wire::session::
  VerifiedAuthorization` (what `CapabilityVerifier::verify` returns) does not carry a
  certificate ID distinct from the chain ID, unlike `capability::chain::VerifiedCapability`
  (the sibling capability crate's real verification result, which does). Until the wire-local
  `VerifiedAuthorization` is widened with a `leaf_certificate_id` field — an integration-wave
  change to `wire/session.rs`, which this slice does not own — `dispatch_authorize` uses
  `chain_id` for both fields. Flagged inline at the construction site.
- **`capability/AGENTS.md` §5.3 obligation 4 ("read durable epoch state at startup") is out of
  this slice's scope.** It requires a real, persistent `AuthorizationView` backed by storage
  this crate does not own; `CanonicalLogState.authorization_view` is a trait object whose
  production implementation is deferred the same way the other four traits are. Obligation 2 is
  now met halfway: `CanonicalLogState::invalidate_on_epoch_bump` exists and evicts both the gate
  and the cache map, and `sweep_revocations` runs on the connection revalidation timer — but
  nothing in fe-api *observes* a `scope_epoch_bump` event, because no such event source exists
  yet. Until one does, epoch-driven revocation reaches the cache through
  `cached_verification`'s own recomputation against the live view, not through a notification.
- **`subscribe` re-verification is scoped to the handshake's verb and object class.** A future
  wire change that lets `subscribe` name its own verb/object-class bits would allow a tighter
  check than "at least the authority `authorize` demanded"; `SubscribeBody` carries no such
  fields today.
- **The revalidation timer is per-connection, and so is the cache sweep it drives.** A process
  with no open `/ws/canonical` connection never sweeps. That is harmless while the route is
  unmounted and remains correct once it is not (`cached_verification` refuses stale entries on
  its own), but a process-wide sweep task is the right shape if this endpoint ever carries
  meaningful traffic.
- **No live-socket test coverage.** Per this slice's brief, dispatch functions are tested with
  `fe_canonical_log::wire::test_support` doubles and no real `WebSocket`; `handle_socket`'s
  frame-decode loop and the `tokio::spawn` preview task are exercised only by code review and
  the type system, not a running connection. `router.rs`'s tests use `tower::oneshot` to prove
  the route exists/doesn't exist, not full protocol behavior over a socket. Delta fan-out's
  authorization decision is testable anyway because it is a pure function
  (`subscriptions_authorized_for_delta`); the send loop around it is not.
- **`MockCapabilityVerifier` ignores `requested_scope`.** It answers by chain bytes alone, so no
  test in this module can prove that the `subscribe` re-verification is *scope-sensitive* — only
  that the verifier is consulted and that its verdict is load-bearing (an unregistered chain is
  refused). Scope sensitivity in this module is carried by the `epoch_scope.contains` check,
  which is tested directly. A scope-aware double would need `async-trait`, which `fe-api` does
  not depend on.
