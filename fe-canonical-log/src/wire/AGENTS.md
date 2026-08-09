# wire — SPEC-7 commit/preview wire protocol (W2-wire)

Transport-agnostic message types for `docs/spec/canonical-log/commit-preview-wire.md`. No
socket, no axum, no tower, no `fe-api`. Everything here is a pure value + a handful of
DB-free pure functions and traits the future transport layer (`fe-api`, wave 3) implements
against.

## Module map

| File | SPEC-7 section | Contents |
| --- | --- | --- |
| `mod.rs` | §2.1, §6 | Shared body-decoding helpers, `require_current_session_generation` |
| `frame.rs` | §2.1, §2.3 | `MessageType` registry, common frame encode/decode |
| `error.rs` | §6.3 | `WireError`, `ProtocolErrorCategory`, `ProtocolErrorBody` |
| `cursor.rs` | §3 | `ProjectionIdentity`, `CursorTuple`, `DurableCursor`, `BranchRegistry` |
| `session.rs` | §2.2, §6.1 | `authorize`/`authorized`/revalidation bodies, `SessionAuthorizationTable`, `CapabilityVerifier` |
| `commit.rs` | §4 | `commit_submit`/`commit_ack`/`commit_delta`, `CanonicalCommitPipeline`, `build_commit_ack` |
| `subscription.rs` | §5.1 | `subscribe`/`resume`/`replay_complete`, `SubscriptionTable`, `resolve_resume` |
| `snapshot.rs` | §5.2 | `snapshot_required`/`scene_snapshot`/`snapshot_ack`, `ScopeSnapshotSource`, lag fan-out |
| `preview.rs` | §7.1 | `preview_send`/`preview_delta`/`preview_dropped`, reserved-key gate |
| `preview_limiter.rs` | §7.2.3 | `PreviewRateLimiter` |
| `test_support.rs` | — | In-memory fakes, always compiled (not `#[cfg(test)]`) so `tests/spec7_conformance.rs` can link them |

## Why preview isolation is a module boundary, not a runtime check

`preview.rs` and `preview_limiter.rs` import nothing from `commit.rs`, `subscription.rs`,
`snapshot.rs`, or `cursor.rs`. That is enforced by the compiler, not by a lint or a review
checklist: there is no `use` statement anywhere in either file that could reach
`CanonicalCommitPipeline::submit_candidate`, `DurableCursor`, `BranchRegistry`, or
`SubscriptionTable`. A preview handler built from these two files is structurally incapable
of calling verified append, materialization, projection persistence, segment sealing, durable
replay, or commit fanout, because the types it would need are simply not in scope. `preview.rs`
also carries `reject_reserved_preview_keys`, a second, independent gate over a fixed key range
(90..98) that a future edit could not accidentally satisfy just by keeping the exact-key check
passing.

## The session-generation gate

SPEC-7 §6 rule 3 requires `session_generation_invalid` to stop a frame **before commit, replay,
snapshot, or preview work**. All four of those paths take a `&SessionAuthorizationTable` as a
**required** parameter and call `wire::require_current_session_generation` as their first
statement:

| Path | Entry point | Result on a revoked generation |
| --- | --- | --- |
| commit | `commit::handle_commit_submit` | `commit_ack` `rejected` / `session_generation_invalid` |
| replay | `subscription::resolve_resume` | `Err(WireError::StaleSessionGeneration)` |
| snapshot | `snapshot::snapshot_all_authorized_subscriptions` | `Err(WireError::StaleSessionGeneration)` |
| preview | `preview_limiter::PreviewRateLimiter::check_and_record` | `Err(WireError::StaleSessionGeneration)` |

Required, not optional, is the whole point. An earlier revision enforced the rule on the commit
path only; the other three never received the table, so `ResumeBody.session_generation` and
`SnapshotRequiredBody.session_generation` were decorative wire fields nothing compared against
the accepted generation. A client holding an open socket across an epoch bump or capability
expiry could still resume, replay, be served a full scene snapshot for its old subscription
scopes, and push previews — authorized private history delivered under a revoked generation. An
`Option<&SessionAuthorizationTable>` would have reproduced exactly that: a future caller passing
`None` reads as "not applicable here" rather than as a skipped authorization check.

`StaleSessionGeneration` is the single error vocabulary for all four; it corresponds one-to-one
with `ProtocolErrorCategory::SessionGenerationInvalid`, which is what the commit path reports
because §4.2 makes a rejected commit an ack state rather than a transport error. Three of the
four return the error before touching a registry, snapshot source, or rate-limit bucket, so a
stale caller cannot use them as an oracle for whether a subscription exists or how much preview
budget remains.

`preview_limiter.rs` imports `SessionAuthorizationTable` and nothing else new. That type reaches
none of `commit`, `subscription`, `snapshot`, or `cursor`, so the compiler-enforced preview
isolation described below is unchanged.

## The delta scope filter

`commit::is_delta_authorized_for_subscription` is the only code implementing §4.3 rule 3 ("MUST
filter every delta by the recipient's active exact scope and current capability; MUST NOT use a
missing scope value as permission to broadcast"). It previously had no non-test caller, and
`CommitDeltaBody` was a plain public struct, so a wave-3 fan-out loop could build a delta whose
scope lay outside the recipient's subscription and never touch the predicate.

`CommitDeltaBody::for_subscription(subscription_id, subscription, session_generation, delta)` is
now the construction path, and `CommitDeltaBody` is `#[non_exhaustive]`. Outside this crate the
struct literal does not compile, so the only two ways to obtain a body are that fallible
constructor and `from_cbor` — and `from_cbor` is the *receiving* side, decoding a delta someone
else already authorized. The predicate is therefore unavoidable on the sending path by
construction rather than by a call a future author has to remember.

The constructor checks two things. The scope check is §4.3 rule 3 and fails with
`WireError::ScopeNotAuthorized`. It also requires the delta's branch and projection identity to
be the ones the recipient's subscription is bound to, failing with
`WireError::DeltaNotForSubscription`: §4.2 rule 3's "MUST not treat an acknowledgement as current
for a different branch, scope view, or materializer version" is the same leak one field over, and
since the constructor already holds both records the check is free. Every other field of the body
is copied from the `CommittedDelta` the registry produced, so no caller can substitute a scope,
cursor, or summary the log did not commit.

`is_delta_authorized_for_subscription` stays public: a fan-out loop should pre-filter recipients
before building bodies, and the predicate is the sanctioned way to do that.

## `DurableCursor` opacity

`DurableCursor`'s fields are private and its only constructor,
`pub(crate) fn new(...)`, is unreachable from outside this crate. The sanctioned path is
`BranchRegistry::issue_cursor`, a **provided** trait method whose body is the only caller of
that constructor; every `BranchRegistry` implementor (including the eventual `fe-database`
one in wave 3) receives it for free and should not override it. This is one method beyond the
four SPEC-7 §3 rule 3 requires (`validate`/`compare`/`replay_after`/`snapshot_cursor`); the
fifth exists only to make the opacity requirement literally true across crate boundaries — see
"Deviations" below.

`test_support.rs` additionally exposes `cursor_at_position_zero` and `cursor_with_claim` as
test-only escape hatches into the same private constructor. They exist so
`tests/spec7_conformance.rs` — a separate crate that only sees this library's public surface —
can build fixture cursors, including deliberately tampered ones for the frontier-commitment
negative tests. Production code has no equivalent path.

## Provisional wire numbering (M8)

**Owner note:** D-CL24/M8 directs these numbers into `fe-canonical-log/src/AGENTS.md`, which
this slice was forbidden to edit. The crate-root file now carries the register that indexes
this table under its own "Provisional wire numbering" heading; the rows below stay here and
remain the source of truth for the numbers themselves.

Only the common frame (§2.1) and `commit_submit` (§4.1) carry a normative integer-key table in
the spec text. Every other body below is this slice's own assignment, in declaration order,
and is **not** an interop claim.

**Common frame** (normative, §2.1): `0` wire_version, `1` message_type, `2` request_id, `3` body.

**`commit_submit`** (normative, §4.1): `0` session_generation, `1` authorization_binding_id,
`2` claimed_op_id, `3` complete_envelope, `4` payload_ciphertext.

**`authorize`** (provisional): `0` capability_chain_bytes, `1` authorization_binding_id,
`2` requested_verb, `3` requested_object_class, `4` requested_scope.

**`authorized`** (provisional): `0` authorization_binding_id, `1` session_generation,
`2` leaf_principal, `3` chain_id, `4` epoch_scope, `5` scope_epoch, `6` expires_at_ms.

**`authorization_revalidation_required`** (provisional): `0` authorization_binding_id,
`1` scope, `2` invalidated_session_generation, `3` reason.

**`commit_ack`** (provisional, variable key set by discriminant): `0` session_generation,
`1` authorization_binding_id, `2` claimed_op_id, `3` state (`0`=rejected,
`1`=accepted_pending_materialization, `2`=committed, `3`=already_committed), `4` category
(rejected only), `5` branch_id, `6` scope, `7` projection_identity, `8` cursor (committed/
already_committed only — keys `5`..`8` are structurally absent, not null, for every other
state).

**`commit_delta`** (provisional): `0` subscription_id, `1` session_generation, `2` op_id,
`3` branch_id, `4` scope, `5` projection_identity, `6` cursor, `7` change_summary.

**`subscribe`** (provisional): `0` session_generation, `1` authorization_binding_id,
`2` subscription_id, `3` branch_id, `4` scope, `5` projection_identity.

**`resume`** (provisional): `0` session_generation, `1` subscription_id, `2` prior_cursor.

**`replay_complete`** (provisional): `0` subscription_id, `1` session_generation, `2` cursor.

**`snapshot_required`** (provisional): `0` subscription_id, `1` session_generation, `2` reason
(`0`=broadcast_lagged, `1`=cursor_unavailable, `2`=cursor_invalid, `3`=projection_changed,
`4`=replay_limit, `5`=authorization_changed).

**`scene_snapshot`** (provisional): `0` subscription_id, `1` session_generation, `2` branch_id,
`3` scope, `4` projection_identity, `5` snapshot_cursor, `6` view_bytes.

**`snapshot_ack`** (provisional): `0` session_generation, `1` subscription_id, `2` snapshot_cursor.

**`preview_send`** (provisional): `0` session_generation, `1` authorization_binding_id,
`2` scope, `3` preview_sequence, `4` preview_kind, `5` expires_at_ms, `6` preview_data.

**`preview_delta`** (provisional): `0` sender_principal, `1` scope, `2` preview_sequence,
`3` preview_kind, `4` expires_at_ms, `5` preview_data.

**`preview_dropped`** (provisional, variable arity): `0` scope, `1` preview_sequence (optional
— key absent, not null, when unknown), `2` reason (`0`=rate_limited, `1`=overloaded).

**`protocol_error`** (provisional): `0` category. `ProtocolErrorCategory` discriminants `0`..`19`
are listed in `error.rs`'s `to_u64`/`from_u64`; no other field exists, by design (§6.3 rule 3 —
a diagnostic beyond the category could disclose whether a private operation, artifact, branch,
or cursor exists).

**Reserved preview keys** (`preview.rs::RESERVED_PREVIEW_KEYS`, permanent, never assignable to
a real field in any preview body): `90` op_id, `91` signed_envelope_bytes, `92` parent_ids,
`93` branch_id, `94` hlc, `95` payload_ciphertext, `96` payload_hash, `97` checkpoint_identity,
`98` durable_cursor.

## The two undefined product numbers (M7, D-CL24)

SPEC-7 §7.2.3 requires "the service's explicit finite default" `max_preview_hz` when a
capability carries no caveat, but never states the number, and D-CL24 reserves it as
caller-set policy. `PreviewRateLimiter::new` therefore takes a required `PreviewRateLimit`
constructor argument and this crate defines **no** `Default` impl for either
`PreviewRateLimiter` or `PreviewRateLimit` that would pick one.

§5.1.6/§6.3's `replay_limit` snapshot/error reason implies a maximum replay interval length,
also never stated. `resolve_resume` and `InMemoryBranchRegistry::replay_after` do not enforce
one — they always return the complete authorized interval, per §5.1.4's "MUST NOT silently
truncate". A finite `replay_limit` is therefore Wave-3/materializer policy applied on top of
`BranchRegistry::replay_after`, not something this slice invented a number for.

## Deviations from the literal brief

- `BranchRegistry` has **five** methods, not the four §3 rule 3 names. The fifth,
  `issue_cursor`, is a provided default method whose only job is to be the sole caller of
  `DurableCursor::new` from outside this module, which is what makes "no public constructor
  other than through the registry" true for a `BranchRegistry` implemented in a different
  crate (`fe-database`, wave 3). Flagged for the integration wave in case the owner wants this
  folded into `validate`/`snapshot_cursor` instead.
- `cursor::ProjectionIdentity` (`materializer_id: String, version: u32`) is a wire-local type
  per this slice's own brief text, not `materialize::identity::MaterializerVersion` /
  `ProjectionIdentity` (which binds a materializer version to a *branch id*, a different
  shape). The two are NOT the same type and are not reconciled here; see
  `needs_from_other_slices` in the workstream report.
- `session::VerbBits` / `ObjectClassBits` are `u8` type aliases standing in for
  `capability::verbs::VerbSet` / `ObjectClassSet`, which this slice does not import (the
  capability module did not exist yet when this slice ran). The integration wave should widen
  `AuthorizeBody`/`AuthorizedBody`/`CapabilityVerifier` to the real bitset types.
- `CapabilityVerifier::verify` is consumed, not implemented, by this slice; its concrete
  binding to `capability::chain::verify_chain` is explicitly the integration wave's job per
  this slice's own brief.
- `commit::is_delta_authorized_for_subscription` is a pure `Scope::contains`-based helper this
  slice added beyond the brief's literal list, because §4.3 rule 3's "MUST filter every delta
  by the recipient's active exact scope" is not self-enforcing without one. It is reached
  through `CommitDeltaBody::for_subscription`; see "The delta scope filter" above.

## Wave 3 obligation: `cursor::verify_frontier_commitment`

`verify_frontier_commitment` has no in-crate production caller and that is structural, not an
oversight: nothing here ever receives a peer-supplied cursor. Wave 3 MUST call it on every
resume cursor arriving from a client, before the cursor is used to select a replay range —
a client that supplies member op_ids inconsistent with the committed frontier would otherwise
choose its own replay window. It is listed here so the enforcement census records an owner
rather than reporting a dormant gate.

## Deferred to wave 3 (named tests this slice cannot complete alone)

- **`websocket_revalidates_on_epoch_bump_and_expiry`** (SPEC-3 §8, assigned to this slice "at
  the type level" per the wave-wide instructions): `SessionAuthorizationTable::invalidate`,
  `RevalidationReason`, and `AuthorizationRevalidationRequiredBody` exist, and
  `session_revalidation_stops_commit_replay_snapshot_and_preview`
  (`tests/spec7_conformance.rs`) now drives all four protected paths across one `invalidate`
  and one replacement `authorize`, with panicking doubles standing in for the commit pipeline
  and the snapshot source so a gate that ran too late fails the test rather than passing it.
  The *live-session* half — an actual open WebSocket connection continuing to be rejected
  across a real epoch bump delivered over the wire, through `fe-api`'s dispatch loop — needs
  the wave-3 transport and is not implemented here. Wave 3 must pass its per-connection
  `SessionAuthorizationTable` into each of the four entry points; the signatures no longer
  allow it to be omitted.
