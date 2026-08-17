# canonical_ws — SPEC-7 commit/preview wire transport (W3-api-canonical-ws)

Implements `docs/spec/canonical-log/commit-preview-wire.md` §2, §4-§7 as an axum WebSocket
surface over `fe_canonical_log::wire`'s frame codec and body types. No socket, database, or
network code lives in `fe_canonical_log` itself — that crate is transport-agnostic by design
(see `fe-canonical-log/src/wire/AGENTS.md`); this module is the wave-3 transport it names.

## Module map

| File | Contents |
| --- | --- |
| `state.rs` | `CanonicalLogState`: the five commit-side `Arc<dyn Trait>` authorities, the preview limiter/broadcast channel, and the §5.3 verification cache + revalidation gate |
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
3. **`capability::revalidation::CacheKey` + `RevalidationGate::is_admitted`** —
   `CanonicalLogState::cached_verification` (called from `dispatch_authorize` before
   `CapabilityVerifier::verify`) never returns a cached `VerifiedAuthorization` without
   consulting `RevalidationGate::is_admitted` first; `CanonicalLogState::admit_verification`
   is the only path that inserts into either the cache or the gate.
4. **`PinnedSession::is_still_valid` on a timer + `PinnedSession::covers`** —
   `handle_socket` runs a five-second `tokio::time::interval` independent of traffic;
   `revalidate_pinned_session` calls `is_still_valid` on every tick and flips the shared status
   to `Invalidated` (stopping commit, resume, snapshot, and preview — each dispatch function
   checks the shared status before doing any of that work) the moment it stops being `Valid`.
   `PinnedSession::covers` gates `dispatch_resume`, `dispatch_snapshot_fanout`'s per-subscription
   closure, `dispatch_subscribe`'s scope bookkeeping, and `preview_task::dispatch_preview_send`.

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
treat as a trusted baseline (set on `snapshot_ack`) and the `op_id` of every delta it has itself
forwarded since. A `resume` cursor that is byte-identical to that trusted baseline needs no
recomputation (it never left this connection's own hands unverified); any other claimed cursor
is checked with the real `verify_frontier_commitment` against the connection's own observed
`op_id` set, and an unrecognized subscription (no ledger at all) is refused outright rather than
treated as an implicit pass. This catches a cursor whose claimed commitment is inconsistent with
what *this connection* witnessed being delivered. It does **not** independently prove a cursor
minted in a prior session or another connection, before this ledger existed — that guarantee
still requires the real `BranchRegistry::validate` (an `fe-database` implementation, not built
here) to hold, since only a registry backed by durable storage knows the true member set at an
arbitrary position. Both checks are meant to compose: this one is cheap and connection-local,
`registry.validate()` (already called inside `resolve_resume`) remains authoritative.

## Known limitations

- **`PinnedSession::leaf_certificate_id` stands in as `chain_id`.** `wire::session::
  VerifiedAuthorization` (what `CapabilityVerifier::verify` returns) does not carry a
  certificate ID distinct from the chain ID, unlike `capability::chain::VerifiedCapability`
  (the sibling capability crate's real verification result, which does). Until the wire-local
  `VerifiedAuthorization` is widened with a `leaf_certificate_id` field — an integration-wave
  change to `wire/session.rs`, which this slice does not own — `dispatch_authorize` uses
  `chain_id` for both fields. Flagged inline at the construction site.
- **`capability/AGENTS.md` §5.3 obligations 2 and 4 are out of this slice's scope.**
  `RevalidationGate::on_epoch_bump` (call on every admitted `scope_epoch_bump`) and "read
  durable epoch state at startup" both require a real, persistent `AuthorizationView` backed by
  storage this crate does not own; `CanonicalLogState.authorization_view` is a trait object
  whose production implementation is deferred the same way the other four traits are.
- **No live-socket test coverage.** Per this slice's brief, dispatch functions are tested with
  `fe_canonical_log::wire::test_support` doubles and no real `WebSocket`; `handle_socket`'s
  frame-decode loop and the `tokio::spawn` preview task are exercised only by code review and
  the type system, not a running connection. `router.rs`'s tests use `tower::oneshot` to prove
  the route exists/doesn't exist, not full protocol behavior over a socket.
