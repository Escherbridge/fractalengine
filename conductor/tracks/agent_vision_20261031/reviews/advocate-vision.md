---
type: Review
title: Agent vision advocate review — shared bridge envelope, shared framing helpers, scope addenda
track: agent_vision_20261031
role: advocate
reviewed_commit: 6665ea1
created: 2026-11-01
---

# Agent vision advocate review

Lens: one shape per repeated seam, pure math in the crates that own it, one
authorization path for both doors (REST + MCP), and every §SEC-ADDENDUM (1-5)
and §UI-ADDENDUM (1-4) behaviour kept intact. Edits are applied, not proposed.
The work landed on top of V5 (`6665ea1`) together with the V5 addenda, because
the shared envelope is what carries the caller's scope.

## What was reconciled

Two lanes wrote into the tree at the same time: V5's addenda work and a partial
advocate refactor written by two concurrent instances. The result now has one
design:

- **One envelope.** `fe-runtime/src/bridge.rs` `BridgeRequest<Op, Output>`
  (`correlation_id`, `caller_scope`, `op`, `reply_tx`) plus `BridgeReply` and
  `bounded_bridge`. `ScreenshotRequest` and `VisionRequest` are type aliases of
  it. The per-surface `reply()` copies are gone.
- **One round trip.** `fe-api/src/bridge_client.rs` `round_trip(bridge,
  caller_scope, op, timeout, name)`. Both `request_capture` and
  `request_vision` call it and only map `RoundTripError` to their own typed
  errors. The caller scope is a required argument, so no request can leave
  without it.
- **One parse path.** `vision::validate_capture_request` serves both doors
  (REST bytes, MCP arguments). Both doors pass `claims.scope` into the same
  `request_capture` / `run_capture` / `request_vision`.
- **The collision fixed.** The second instance added
  `VisionError::coded_message`, which prefixes MCP errors with their `code`.
  That turned the MCP posed-door denial into `not_found_or_denied: not found
  or not permitted`, while the plain door still said `not found or not
  permitted`. This broke `vision_scope_test` (SEC-ADDENDUM 3 and 5, the
  uniform denial). The fix keeps the code prefix for typed refusals and leaves
  the 404 denial as the bare §existence-oracle text. A table test
  (`coded_message_prefixes_refusals_but_never_the_denial`) pins this.
- **Duplicates dropped.** `vision_bridge.rs` no longer has its own
  `frame_world_box`, `FRAMING_MARGIN`, `MIN_FOCUS_RADIUS`, `union_bounds` or
  its own copy of the framing test. It uses `fe_renderer::camera::
  frame_world_box` / `LookAtPose` (through a 3-line `frame_box` adapter) and
  `fe_terrain::camera_framing::union_bounds`.

## The one-frame gap (closed)

The plain `GET` path checked scope once, when `drain_screenshot_requests`
drained the request, and then queued the capture. The vision path checks
again right before the frame is queued. The plain path is now two stages, the
same as the vision path:

- `drain_screenshot_requests` (Update) is the arrival check. An uncovered
  caller is denied at once. Covered requests wait in `PendingScreenshots`.
- `capture_pending_screenshots` (PostUpdate) is the frame-time check.
  `scope_covers_active_verse` runs again against the verse that is open now.
  Then one capture serves all covered requests.

Flow test: `switching_to_an_uncovered_verse_between_drain_and_capture_refuses`.
A request admitted while verse A is open gets the uniform denial, and no
`Screenshot` is spawned, when the user switches to verse B before capture.
Staying in verse A serves it.

A related hole closed at the same time. The capture stage also checks
`CameraCaptureLock`. Before, a plain GET drained in the same frame that a
posed job started could capture the half-posed camera, because the job's lock
is a deferred insert that the drain cannot see. The lock test now covers this
case.

## Patterns applied

- **Request-Reply with a Correlation Identifier**: one generic envelope and
  one client round trip (`bridge.rs`, `bridge_client.rs`).
- **Humble Object**: pose and framing math stays pure and table-tested in
  fe-renderer and fe-terrain. The Bevy orchestrator only gathers ECS inputs
  and applies the result.
- **Required-field authorization carrier**: `caller_scope` is part of the
  envelope, not an optional extra, so a new seam cannot forget it.
- **Check-at-use (TOCTOU close)**: scope is checked when the request arrives
  and again at frame time, on both capture paths.
- **Uniform denial (no existence oracle)**: one text and one 404 on every
  door, so an out-of-scope target and a missing one look the same.

## Addenda verified (tests)

- SEC 1 / 3 / 5: `vision_scope_test::a_verse_b_token_is_denied_every_capture_and_camera_read_on_both_doors`,
  `screenshot_bridge::tests::{plain_captures_are_refused_unless_the_token_covers_the_active_verse, switching_to_an_uncovered_verse_between_drain_and_capture_refuses}`,
  `vision_bridge::tests::{a_token_outside_the_active_verse_is_denied_capture_and_camera_read, leaving_the_covered_verse_mid_settle_denies_the_capture}`.
- SEC 2 / 5 (no oracle, cross-verse navigate):
  `vision_scope_test::foreign_and_nonexistent_focus_targets_are_indistinguishable_on_both_doors`,
  `vision_bridge::tests::foreign_and_nonexistent_focus_targets_get_the_same_denial`,
  `navigate_then_fail_returns_navigation_to_the_original_petal`.
- UI 1-4: `fe_runtime::vision::tests::include_ui_and_mode_matrix`,
  `vision_scope_test::include_ui_selects_the_mode_and_is_reported_in_the_metadata`.

Gate: `cargo check` (6 crates, `--tests`) passed. `cargo +1.99.0 clippy
--all-targets -D warnings` passed after one `type_complexity` fix in
`union_bounds_table`. `cargo test -p fe-runtime -p fe-api -p fe-renderer -p
fractalengine` ran 671 passed, 0 failed, 1 ignored. `cargo test -p fe-terrain
--features render camera_framing` ran 9 passed.

## Deferred

- `fe_runtime::vision::tests::vision_bridge_is_bounded_at_its_capacity` still
  builds requests by hand. It only proves that `VISION_BRIDGE_CAPACITY` is
  wired, and `bridge.rs` already tests the generic bound. Fold it when
  `vision.rs` is next touched.
- If the same frame has an F12 press and an API capture, two `Screenshot`s
  are taken and one is dropped. This was a known issue before this pass and is
  unchanged.

VERDICT: PASS — one envelope, one round trip, one parse path, shared framing
helpers. Both capture paths now check scope at arrival and at frame time. All
§SEC and §UI addendum tests are green.
