# Canonical log commit and preview wire protocol v1

**Status:** Draft — owner approval required before implementation.

This document defines the API and WebSocket behavior for submitting and
observing Canonical Fractal Data Log operations. It implements D-CL13 and
D-CL15 and applies the authorization and materialization boundaries from
[capabilities-and-revocation.md](capabilities-and-revocation.md) (SPEC-3) and
[log-first-materialization.md](log-first-materialization.md) (SPEC-4).

It does not select an HTTP route, a WebSocket library, or a relay protocol.
D-CL17 and D-CL19 fix the encryption and branch-control contracts consumed
here, but this specification does not authorize network, relay, or iroh
changes.

## 1. Conformance vocabulary and boundary

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **commit** is an admitted operation that has also been applied to the
   selected materialized projection at a durable cursor. A durable append that
   is awaiting materialization is not a commit for this protocol.
3. A **commit acknowledgement** is a response about one claimed SPEC-1
   `op_id`. It is evidence of service state, not a new authority or a
   replacement for the author's signed complete envelope.
4. A **subscription** is an authorization-bound request to receive committed
   projection changes and, separately, optional previews for one exact scope.
5. A **preview** is a transient visual or interaction hint. It has no `op_id`,
   parents, branch identity, HLC, signed envelope, payload artifact, durable
   cursor, checkpoint, or materializer route.
6. A **projection identity** is the materializer identity and version required
   by SPEC-4. A cursor is valid only for the projection identity it names.
7. A **session authorization generation** is a monotonically increasing local
   session number. It changes whenever the server accepts replacement
   authorization or invalidates prior authorization. It is distinct from a
   persistent SPEC-3 scope epoch.

## 2. Wire framing and authorization session

### 2.1 Common frame

1. Logical WebSocket frames use deterministic CBOR with the restrictions in
   SPEC-1 section 2. Transport implementations MAY carry the same logical
   request and response through an HTTP API, but MUST preserve every signed
   byte string exactly.
2. Each frame is a map with exactly these keys:

   | Key | Name | Representation | Rule |
   | ---: | --- | --- | --- |
   | 0 | `wire_version` | unsigned integer | MUST be `1`. |
   | 1 | `message_type` | unsigned `u16` | Registered in section 2.3. |
   | 2 | `request_id` | 16-byte byte string or `null` | Client correlation ID; `null` only for unsolicited server messages. |
   | 3 | `body` | map | Exact grammar for `message_type`; unknown keys are rejected. |

3. `request_id` correlates a request and response only. It has no ordering,
   authorization, idempotence, or durable-history meaning.
4. A receiver MUST reject a frame with an unsupported version, unknown type,
   wrong direction, non-canonical encoding, duplicate key, or extra body key.
   It MUST NOT reinterpret an unknown message as a commit or preview.

### 2.2 Session authorization

1. A client begins a socket session, replaces expired authority, or adds a
   subscription by presenting the exact canonical capability-chain bytes from
   SPEC-3. The service verifies the chain against the persistent authorization
   view, binds the leaf principal, and assigns a session authorization
   generation.
2. Every stateful client frame after authorization MUST name the current
   session authorization generation and an applicable subscription or
   authorization binding. Frames from an invalidated generation MUST be
   rejected without reaching commit, replay, snapshot, or preview handlers.
3. For a commit, the leaf principal MUST equal the operation author and the
   presented chain ID MUST equal `envelope.capability.chain_id`. The effective
   capability MUST allow `append/op` for the exact envelope scope, schema,
   operation kind, and payload size.
4. For subscription, resume, snapshot, and preview actions, the service MUST
   verify the relevant SPEC-3 verb and exact requested scope. A client MAY
   hold distinct bindings for differently scoped grants; it MUST NOT use a
   broader cached decision as proof of a narrower or different binding.
5. The service MUST revalidate expiry on a timer and revalidate affected
   bindings when the persistent authorization view advances. On a matching
   scope-epoch bump or expiration it MUST immediately stop commits, previews,
   payload delivery, replay, and snapshots for that scope, as required by
   SPEC-3 section 5.3.
6. A restarted API or WebSocket service MUST load the persistent authorization
   view before accepting a cached session decision. Missed invalidation
   notifications are cache misses, never authorization.
7. `authorize` contains the exact complete capability-chain bytes, a
   client-chosen 16-byte `authorization_binding_id`, and the requested verb,
   object class, and scope. `authorized` returns that binding ID, the accepted
   session authorization generation, leaf principal, chain ID, epoch scope,
   scope epoch, and expiration. The service MUST reject a binding ID collision
   with a different chain or principal.

### 2.3 Message classes

The following type values are structurally disjoint. A conforming decoder MUST
use separate commit and preview body types rather than a shared mutable-event
type with optional fields.

| Type | Direction | Name | Purpose |
| ---: | --- | --- | --- |
| 1 | client → service | `authorize` | Present or replace a capability binding. |
| 2 | service → client | `authorized` | Return an accepted binding and session generation. |
| 10 | client → service | `commit_submit` | Submit one exact complete operation envelope and payload artifact. |
| 11 | service → client | `commit_ack` | Report append/materialization state for one `op_id`. |
| 12 | service → client | `commit_delta` | Deliver one committed operation at a durable cursor. |
| 13 | client → service | `subscribe` | Create or replace a committed-change subscription. |
| 14 | client → service | `resume` | Request replay after a prior durable cursor. |
| 15 | service → client | `replay_complete` | Confirm replay through a durable cursor. |
| 16 | service → client | `snapshot_required` | State that replay cannot safely continue. |
| 17 | service → client | `scene_snapshot` | Send a fresh scope projection snapshot under D-CL15. |
| 18 | client → service | `snapshot_ack` | Establish the snapshot cursor as the next replay base. |
| 19 | service → client | `authorization_revalidation_required` | Stop traffic until replacement authority is accepted. |
| 20 | client → service | `preview_send` | Send one lossy preview. |
| 21 | service → client | `preview_delta` | Deliver one lossy preview. |
| 22 | service → client | `preview_dropped` | Optional rate-limit or overload notice; never a durable acknowledgement. |
| 255 | service → client | `protocol_error` | Report a stable error state. |

## 3. Durable cursor abstraction

1. A durable cursor names an observed, materialized point for one tuple of
   `(verse_id, branch_id, subscription_scope, projection_identity)`. It MUST
   carry a `frontier_commitment` and a `delivery_position` produced by the
   branch registry.
2. `frontier_commitment` is the D-CL19 32-byte
   `BLAKE3(sorted frontier op_id list)` value. `delivery_position` is an opaque
   canonical byte string whose branch registry comparison is valid only within
   the same cursor tuple; it supplies no global order.
3. The branch registry interface used by this protocol MUST provide all of the
   following before a cursor is issued:

   | Function | Required result |
   | --- | --- |
   | `validate(cursor)` | Proves that every field binds one verse, branch, scope view, projection identity, and valid frontier commitment. |
   | `compare(left, right)` | Establishes whether two same-tuple cursors are equal, ordered, or incomparable. |
   | `replay_after(cursor)` | Returns a complete ordered set of committed deltas after the cursor, or `snapshot_required`; it never silently omits a gap. |
   | `snapshot_cursor(scope)` | Returns the exact cursor represented by a fresh scope snapshot. |

4. An implementation MUST NOT substitute an arrival sequence, per-connection
   counter, wall clock, HLC, local database row ID, or a single latest
   `branch_head_op_id` for this commitment. None represents a concurrent
   tracking frontier safely.
5. D-CL19 requires the canonical sorted-frontier hash and Manager+-authorized
   verse-scoped create, pause, retarget, and detach operations defined by
   SPEC-5. A cursor whose branch registry cannot prove those bindings is
   invalid and MUST use the snapshot-required path instead of approximation.
6. This wire contract requires the exact commitment boundary above.
   It does not choose which concurrent tracking heads are effective or collapse
   a frontier into one head.
7. When carried on the wire, a durable cursor contains the tuple fields from
   rule 1 plus `frontier_commitment` and `delivery_position`. It is an opaque
   capability-checked value, not a bearer token: a receiver MUST reauthorize
   the requested scope before revealing any replay result. Its byte encoding,
   equality, and order are validated by the branch registry; no peer may infer
   them from receipt order or a single head.

## 4. Commit submission and acknowledgement

### 4.1 `commit_submit`

1. The body of `commit_submit` contains exactly:

   | Key | Name | Representation | Rule |
   | ---: | --- | --- | --- |
   | 0 | `session_generation` | unsigned `u64` | Current accepted generation. |
   | 1 | `authorization_binding_id` | 16-byte byte string | Binding that proves `append/op`. |
   | 2 | `claimed_op_id` | `Hash32` | MUST equal BLAKE3 of the complete envelope bytes. |
   | 3 | `complete_envelope` | byte string | Exact signed SPEC-1 complete-envelope bytes. |
   | 4 | `payload_ciphertext` | byte string or `null` | Exact referenced artifact; `null` only for SPEC-1 no-payload operations. |

2. The service MUST decode, re-encode, and verify `complete_envelope` exactly
   as SPEC-1 requires; derive `op_id`; and reject a claimed mismatch. It MUST
   verify the payload ciphertext hash, length, encryption binding, and all
   SPEC-3 admission conditions before durable append.
3. The service MUST use the SPEC-4 log-first pipeline. It MUST NOT make a
   direct SurrealDB mutation, emit a `commit_delta`, or call the operation
   committed until the exact immutable bytes are durably appended and the
   selected projection is durably applied.
4. A client that loses an acknowledgement MAY submit the same exact bytes and
   `claimed_op_id` again. The service MUST deduplicate by `op_id` and exact
   bytes, then return current state. It MUST NOT require or encourage minting
   a replacement operation for the same intent.
5. A different byte string for a claimed existing `op_id` is an integrity
   violation. The service MUST reject it and MUST NOT substitute either byte
   string into a projection.

### 4.2 `commit_ack`

1. A `commit_ack` correlates to `commit_submit` and contains its
   `claimed_op_id`, one state below, and either a durable cursor or a stable
   error category.

   | State | Required meaning |
   | --- | --- |
   | `rejected` | Admission or authorization failed; no verified append occurred. |
   | `accepted_pending_materialization` | Exact bytes are durably admitted and appended, but no projection cursor has committed them yet. This is not a committed success. |
   | `committed` | The operation is applied in the named projection and visible at the supplied durable cursor. |
   | `already_committed` | Exact bytes were previously committed; the supplied cursor identifies that committed observation or a later compatible observation. |

2. `accepted_pending_materialization` MUST NOT include a durable cursor,
   `commit_delta`, snapshot claim, or analytics-success assertion. Recovery is
   deterministic replay under SPEC-4; the client may resubmit the exact bytes
   or resume from a valid earlier cursor.
3. `committed` and `already_committed` MUST include the branch ID, exact
   operation scope, projection identity, and durable cursor. A receiver MUST
   not treat an acknowledgement as current for a different branch, scope view,
   or materializer version.
4. The normalized API response uses the same states: `committed` is a created
   or successful idempotent result, `accepted_pending_materialization` is an
   accepted-but-not-committed result, and `rejected` is a protocol error. HTTP
   status selection MUST NOT alter these state meanings.
5. A `commit_ack` body contains the session generation, authorization binding
   ID, claimed op ID, state, and—only for `committed` or
   `already_committed`—the branch ID, scope, projection identity, and durable
   cursor. For `rejected`, it instead contains exactly one public error
   category from section 6. For `accepted_pending_materialization`, it contains
   no cursor and no private diagnostic beyond the pending state.

### 4.3 `commit_delta`

1. A `commit_delta` contains the committed `op_id`, branch ID, exact operation
   scope, projection identity, durable cursor, and a projection-change summary
   authorized for the recipient's subscription scope. It MAY reference the
   exact envelope by `op_id`; it MUST NOT invent unsigned replacement content.
2. A service MUST send a delta only after the operation reaches the supplied
   cursor. An operation that had no visible scene change still advances durable
   history; its authorized delta may therefore be a no-visible-change summary.
3. A service MUST filter every delta by the recipient's active exact scope and
   current capability. It MUST NOT use a missing scope value as permission to
   broadcast a delta. An authorized verse-wide header observation does not
   authorize a petal payload, scene summary, or resource detail.
4. A client MUST advance local committed state only from a complete replay or
   an ordered `commit_delta` chain validated against its current cursor. A
   preview never advances that cursor.
5. A `commit_delta` body contains the subscription ID, session generation,
   op ID, branch ID, scope, projection identity, resulting durable cursor, and
   authorized projection-change summary. A `replay_complete` body contains the
   subscription ID, session generation, and final durable cursor only.

## 5. Subscription, resume, replay, and snapshots

### 5.1 Subscription and resume

1. `subscribe` binds one `subscription_id`, branch ID, exact requested scope,
   projection identity, current session generation, and authorization binding.
   It requests committed deltas; previews require separate `preview` authority
   and are not implied by a committed subscription.
2. A newly accepted subscription MUST begin with a fresh `scene_snapshot` for
   its exact scope. The server MUST wait for the matching `snapshot_ack` before
   treating that cursor as the client's replay base; commits that occur while
   the snapshot is assembled are recovered by resume after that cursor.
3. `resume` names an existing subscription and a durable cursor. Before replay,
   the service MUST revalidate the binding, expiry, current scope epoch, cursor
   tuple, and projection identity.
4. The service MUST then either deliver every authorized committed delta after
   the cursor in cursor order followed by `replay_complete`, or send
   `snapshot_required`. It MUST NOT silently truncate an interval because of
   retention, compaction, authorization change, backpressure, or an unknown
   cursor.
5. `replay_complete` names the final durable cursor and proves only that the
   service delivered the complete authorized interval from the requested base
   to that cursor. It does not waive client-side envelope, artifact, or
   materializer verification required for an offline canonical projection.
6. If a cursor is older than retained replay, belongs to another tuple, is
   incomparable, or cannot be verified, the service MUST send
   `snapshot_required`; it MUST NOT guess a nearest cursor.
7. A `subscribe` body contains the session generation, authorization binding
   ID, client-chosen 16-byte subscription ID, branch ID, exact requested scope,
   and requested projection identity. A `resume` body contains the session
   generation, subscription ID, and prior durable cursor. The service MUST
   reject a duplicate subscription ID that changes its branch, scope, or
   projection identity without an explicit replacement subscription.

### 5.2 Snapshot recovery

1. `snapshot_required` contains the affected subscription, an explicit reason,
   and no unqualified scene data. Reasons include `broadcast_lagged`,
   `cursor_unavailable`, `cursor_invalid`, `projection_changed`,
   `replay_limit`, and `authorization_changed`.
2. Under D-CL15, a service that detects broadcast lag for a connection MUST
   send a fresh `scene_snapshot` for every still-authorized subscribed scope.
   It MUST NOT merely log the lag, keep sending deltas from an unknown point,
   or snapshot an unsubscribed scope.
3. A `scene_snapshot` contains the subscription ID, exact scope, branch ID,
   projection identity, a `snapshot_cursor`, and only the materialized view
   data authorized for that scope. It is a transport recovery response, not a
   new canonical checkpoint or authority claim.
4. The snapshot cursor MUST identify the durable projection position from
   which the snapshot was read. A server MAY continue to commit operations
   while composing a snapshot, but then the client MUST resume after that exact
   cursor before it treats its view as current.
5. Before accepting later deltas, a client MUST discard committed local state
   for the snapshot scope, apply the complete snapshot, set its replay base to
   `snapshot_cursor`, and send `snapshot_ack`. Previews for that scope are
   discarded on snapshot application.
6. A snapshot that cannot be produced from a durable materialized projection,
   is no longer authorized, or lacks a valid cursor MUST fail explicitly. It
   MUST NOT be replaced with stale local rows, an unauthenticated cache, or
   cross-scope content.
7. A `snapshot_required` body contains the subscription ID, session
   generation, and one reason from rule 1. A `scene_snapshot` body contains
   the subscription ID, session generation, branch ID, scope, projection
   identity, snapshot cursor, and authorized materialized-view bytes. A
   `snapshot_ack` contains the session generation, subscription ID, and the
   exact snapshot cursor; it authorizes no replay until the service validates
   that tuple again.

## 6. Authorization revalidation and error states

1. `authorization_revalidation_required` identifies the affected binding and
   scope, the invalidated session generation, and one public reason:
   `capability_expired`, `scope_epoch_advanced`, `authority_changed`, or
   `session_replaced`. It MUST stop all traffic for the affected scope before
   the frame is sent or the socket is closed.
2. A client may present a replacement chain with `authorize`. The service MUST
   issue a new session authorization generation and require a fresh
   subscription or resume check; it MUST NOT revive an old cursor or queued
   preview solely because the principal appears unchanged.
3. `protocol_error` carries one stable category from this table. For an
   unauthorized request, diagnostics MUST NOT disclose whether a private
   operation, artifact, branch, or cursor exists.

   | Category | Required result |
   | --- | --- |
   | `malformed_frame` | Reject the frame; no handler side effect. |
   | `unsupported_wire_version` | Reject the frame; no downgrade or reinterpretation. |
   | `wrong_message_direction` | Reject the frame; no handler side effect. |
   | `session_generation_invalid` | Stop the frame before commit, replay, snapshot, or preview work. |
   | `authorization_revalidation_required` | Stop affected traffic; require replacement authority or close. |
   | `scope_not_authorized` | Deny without private-object existence details. |
   | `invalid_op_id` | Reject; do not append or materialize. |
   | `conflicting_op_bytes` | Reject both claimed substitution and any projection effect. |
   | `invalid_envelope`, `unauthorized_operation`, `invalid_payload` | Use the corresponding SPEC-4 admission disposition. |
    | `unsupported_payload_suite` | Reject a non-empty candidate unless it uses D-CL17 `suite_id = 1`, a current authorized key, and a valid 24-byte nonce. |
   | `missing_parent`, `unknown_schema`, `opaque_payload` | Use SPEC-4 quarantine disposition; do not emit a committed success. |
   | `materialization_pending` | Report only `accepted_pending_materialization`; no cursor or delta. |
   | `cursor_invalid` | Require a valid compatible cursor or snapshot; do not approximate. |
   | `snapshot_required` | Stop replay at a known boundary and send the recovery path. |
    | `frontier_commitment_mismatch` | Reject cursor, resume, checkpoint, or bootstrap use when the sorted-frontier hash or branch-control replay does not match. |
   | `preview_rate_limited` | Drop the preview only; never affect committed history. |

## 7. Preview protocol

### 7.1 `preview_send` and `preview_delta`

1. A `preview_send` body contains exactly the session generation,
   authorization binding ID, exact scope, a sender-local `preview_sequence`, a
   registered non-zero `preview_kind`, an expiry time, and opaque preview data.
   It contains none of the fields prohibited by section 1.5.
2. A `preview_delta` repeats only the sender identity, exact authorized scope,
   sequence, kind, expiry, and preview data needed for rendering. It is not a
   commit acknowledgement and carries no durable cursor.
3. A receiver MAY reorder, coalesce, supersede, or drop previews by sender,
   scope, and sequence. It MUST discard an expired preview and all previews
   when their scope loses authorization, a subscription closes, or a snapshot
   is applied.
4. Preview data is untrusted display input. It MUST NOT be converted into a
   canonical operation intent, fed to an operation encoder, copied into a
   payload shard, persisted in SurrealDB, or used as an analytics, checkpoint,
   branch, materializer, or undo input.
5. A service MUST validate `preview` authority for both sender and recipient
   at delivery time. Previews exist only as same-socket message classes; an
   HTTP commit API MUST reject them rather than provide a durable or polling
   fallback.

### 7.2 Isolation and rate limit

1. Preview handling MUST use a distinct frame type, decoder body type,
   dispatch path, queue, and rate limiter from commit handling. A preview
   handler MUST have no capability to call verified append, materialization,
   projection persistence, segment sealing, durable replay, or commit fanout.
2. A preview requires the effective SPEC-3 `preview` verb for the exact scope.
   `append`, `fetch`, `decrypt`, `materialize`, or `seed` does not imply it.
3. The service MUST enforce the effective `max_preview_hz` caveat per leaf
   principal and exact scope. If the caveat is absent, the service's explicit
   finite default applies. Rate-limited or overloaded previews are dropped;
   `preview_dropped` is advisory and MUST NOT trigger retry as a commit.
4. Previews are not retained for offline delivery, resume, replay, checkpoint,
   segment, seeding, or snapshot. A service MUST NOT promise delivery,
   ordering, exactly-once behavior, or availability for them.
5. No preview type may contain `op_id`, signed envelope bytes, parent IDs,
   branch IDs, HLC, payload ciphertext, payload hash, checkpoint identity, or
   durable cursor. A decoder MUST reject such a field rather than ignore it.

## 8. Required conformance tests

A future implementation MUST provide deterministic fixtures and at least these
named tests:

1. **`commit_submit_preserves_signed_envelope_and_derived_op_id`** — exact
   complete-envelope bytes survive API and WS handling; a claimed ID mismatch
   is rejected before append.
2. **`lost_commit_ack_retries_exact_bytes_idempotently`** — a lost response
   followed by identical submission produces one append, one materialized
   effect, and the current acknowledgement state.
3. **`committed_ack_requires_durable_projection_cursor`** — append-only and
   materialization-pending states never emit a committed acknowledgement,
   delta, snapshot claim, or analytics success.
4. **`commit_delta_is_scope_filtered_and_cursor_bound`** — every delivered
   delta has an authorized exact scope and compatible branch/projection cursor;
   no missing scope broadcasts to all subscribers.
5. **`resume_replays_complete_interval_or_requires_snapshot`** — replay
   delivers all authorized deltas in cursor order or returns an explicit
   snapshot path for every unavailable, invalid, or incomparable cursor.
6. **`lagged_connection_receives_authorized_scope_snapshots`** — broadcast lag
   sends fresh snapshots for each still-authorized subscribed scope, no others,
   and never continues from an unknown delta boundary.
7. **`snapshot_cursor_resumes_after_exact_projection_boundary`** — a snapshot
   taken while new commits arrive resets state at its bound cursor and replay
   converges without duplicate or skipped committed state.
8. **`session_revalidation_stops_commit_replay_snapshot_and_preview`** — an
   affected epoch bump, expiry, restart, or binding replacement blocks every
   listed action until a fresh valid authorization is accepted.
9. **`unauthorized_wire_errors_do_not_disclose_private_history`** — rejected
   requests reveal neither private operation/artifact existence nor unowned
   branch/cursor details.
10. **`preview_frames_are_structurally_disjoint_from_commit_frames`** — a
    preview containing an operation field is rejected and no conversion path
    can call append, materialize, projection persistence, or commit fanout.
11. **`preview_rate_limit_is_per_principal_and_exact_scope`** — effective
    `max_preview_hz`, default finite limits, and scope separation are enforced;
    a dropped preview never changes durable state.
12. **`preview_is_never_replayed_or_snapshotted`** — reconnect, cursor resume,
    checkpoint bootstrap, and snapshot recovery omit prior previews.
13. **`cursor_binds_sorted_frontier_and_branch_control_replay`** — a cursor,
     replay-complete claim, or checkpoint/bootstrap result is accepted only
     when the D-CL19 hash and Manager+-authorized branch-control history match.

## 9. Design notes

- **One signed artifact, many delivery paths:** API and WebSocket submission
  carry exact existing envelope bytes. Neither a service receipt nor a
  convenient JSON representation gets to become the canonical operation.
- **A commit is stronger than an append:** SPEC-4 permits durable operations to
  await deterministic replay after a crash. Naming that state explicitly
  prevents a UI or analytics surface from calling an unprojected append the
  current scene.
- **Cursor opacity is deliberate:** D-CL19 fixes the sorted-frontier hash while
  preserving a registry-defined, tuple-scoped delivery position. The wire
  contract never substitutes a receipt sequence or silently chooses one head.
- **Snapshots heal delivery, not history:** A D-CL15 scene snapshot restores a
  subscribed rendering view. It does not replace a signed, replay-verifiable
  checkpoint under SPEC-4.
- **Preview isolation is a security boundary:** A drag hint can be useful even
  when it is dropped. Letting that hint enter durable storage would recreate
  the unsafe write path this protocol is designed to remove.
