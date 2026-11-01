---
type: Review
title: Agent input advocate review: shared envelope confirmed, composed gates, crash backstop, one gate-status mapping
track: agent_vision_20261031
role: advocate
reviewed_commits: [cc347b1, 9882e1d, e3b37af, 94f34f3, ccc397a]
created: 2026-11-01
---

# Agent input advocate review (I1-I5)

Lens: one request/reply bridge shape for screenshot, camera read-back and
input; `OperatorGate` reused as data, not forked; pure planning with a thin
ECS shell; one typed parameter model for REST and MCP; full-word names; the
reasoning recorded in the right AGENTS.md sections; the open-core S-AGENT-GUI
row and the RUNBOOK decision accurate. Edits were applied, not just proposed.

## What already held (no change needed)

- **Request-Reply with a Correlation Identifier.** `InputRequest` is a
  `BridgeRequest<InputOp, InputOutcome>` alias (INPUT-ADDENDUM 1). fe-api
  `request_inject` uses the shared `bridge_client::round_trip`. There is no
  third bridge shape.
- **Guard chain as data.** `INPUT_GATE` is a `const OperatorGate` with
  `default_on: false`. I1-I5 did not edit `operator_gate.rs`.
- **Validate-once newtype / one parse path.** `ValidInputSpec` is built only
  by `InputSpec::validate`. `input::validate_input_request` serves both
  doors, and `run_inject` is the one audit + bridge path.
- **Humble Object.** `plan_batch` / `resolve_key` / `text_frame` are pure
  functions from `ValidInputSpec` + window size to frames.
  `drive_synthetic_input` only gathers ECS inputs, replays frames and replies.
- **Check-at-use + uniform denial.** Scope is checked when a request
  arrives and again on every frame the job runs. The denial text is
  byte-identical to vision's and REST's, which the tests assert.

## Applied in this pass

1. **INPUT-ADDENDUM 7 (crash, binding, had not been implemented).** A batch whose only
   command was `text_entry: ""` planned zero frames. `Phase::Injecting`
   then indexed `frames[0]`, which panics the Bevy main thread. The app is
   `panic=abort`, so any input-enabled caller could kill the host. Fixed in
   three layers (Defense in Depth):
   - `fe-runtime/src/input.rs` `check_command`: an empty `text_entry` is a
     typed 400 `no_effective_input` (`NO_EFFECTIVE_INPUT_CODE`, field
     `commands[i].text`).
   - `fractalengine/src/input_bridge.rs` `plan_batch`: an empty plan is
     refused `no_effective_input`. This is a backstop that is unreachable
     through `validate()`.
   - `Injecting` takes its frame with `frames.get_mut(next_frame).map(std::mem::take)`
     and moves to `Settling` once `next_frame >= frames.len()`. No per-frame
     input system indexes without a check.
2. **INPUT-ADDENDUM 6 (pixel leak, binding, had not been implemented): Composed Gates.**
   `capture_after` returned a PNG without the screenshot gate. With
   `FE_API_SCREENSHOT_ALLOW=0`, an input-enabled Owner could still get
   pixels, and it skipped the 1 capture/sec budget. The new
   `fe-api/src/input.rs::check_capture_after_gate` runs inside `run_inject`,
   so the REST and MCP doors share one helper. It runs
   `SCREENSHOT_GATE.check_opted_in` and `check_rate` (`screenshot:<sub>`).
   The Owner floor is already the input gate's. The host's per-frame
   view-scope check covers the frame the capture is queued in. A batch
   without `capture_after` never touches the screenshot gate. This makes
   the security posture stricter and loosens nothing.
3. **Single Source of Truth for gate → HTTP status.** Added
   `GateRefusal::status()` (403 for opt-in or role, 429 for rate) in
   `operator_gate.rs`. The copies of `gate_status` and the per-enum
   `Refused(..)` match arms in `input.rs`, `vision.rs` and `screenshot.rs`
   now delegate to it. The chain itself is unchanged.
4. **One failure builder.** `InputFailure::new(class, code, message)` in
   fe-runtime. `denied()`, `camera_locked_by_capture()`, fe-api's transport
   mapping and the Bevy orchestrator all use it. The orchestrator's local
   `failure()` helper is gone.
5. **Full-word names / accuracy.** The MCP schema helper `vec3_xy`, which
   described a 2-D point, is now `pointer_point_schema(description)`. The
   MCP `screenshot` description said "not scope-confined", which the vision
   critic pass had made false. It now says the tool is scope-confined. The
   `inject_input` description and schema now state that input is
   scope-confined and that `capture_after` needs the screenshot gate. The
   `text` field's range is now 1..=4096. Stale "lands in I3/I4" doc
   comments in `fe-api/src/input.rs` were removed. `restore_real_cursor`
   lost a redundant `is_none` check before its `let-else`.
6. **Docs.** fe-api AGENTS.md: the §operator-gate section gains
   `status()` and a "Composed gates" rule, and §agent-input-surface gains
   the `capture_after` bullet, a routes-row update and an accurate Tests
   paragraph. fe-runtime §input-bridge gains `no_effective_input` in the
   table order and `InputFailure::new`. fractalengine §agent-input-surface
   gains the backstop refusal, "no unchecked index" and where
   `capture_after` gating lives. In `conductor/open-core.md` the
   S-AGENT-GUI row now says every surface is scope-confined and that
   `capture_after` passes the screenshot gate. `conductor/RUNBOOK.md` §3 gets
   a 2026-11-01 input-posture decision line.

## Tests (no new test functions; journeys extended)

- `fe_runtime::input::tests::input_spec_validation_defaults_and_bridge_journey`
  gains row 3d': an empty text entry gives `no_effective_input` at
  `commands[1].text`.
- `fe-api/tests/vision_test.rs::input_injection_..._journey` gains a block
  with input on and screenshots off: an empty-text 400 `no_effective_input`,
  then `capture_after` refused 403 with `SCREENSHOT_GATE.disabled_message`
  on REST and MCP, with nothing reaching the bridge, then a plain batch that
  still gets 200 (the host is alive). AC-3 now runs on a GUI-shaped host
  through the new `ApiHarness::spawn_with_screenshot_and_input`.

## Considered, declined

- **Folding `InputError` / `VisionError` / `ScreenshotError` into one generic
  error.** Their `status`/`code`/`coded_message` methods have the same
  shape, but each surface has its own variants (vision `Busy`, screenshot
  `Denied`) and its own field type (`&'static str` vs owned `String`).
  `GateRefusal::status()` removes the part that really was duplicated.
  Revisit when a fourth gated surface appears.
- **A shared `RoundTripError` → text mapping.** The texts are specific to
  each surface ("vision bridge busy", "input bridge busy"), and the targets
  are different typed errors. A helper would only rename five arms.
- **A table test for `plan_batch` / `resolve_key`.** The test policy
  (2026-11-01) says swap, don't add. The Bevy journey already drives every
  planner branch through a real `InputPlugin`.

## Gate

`cargo check -j4 -p fe-api -p fe-runtime -p fe-renderer -p fe-ui -p fractalengine --tests`
passed. `cargo +1.99.0 clippy` (same crates, `--all-targets -D warnings`)
passed. R9 fe-api integration batch: 15 binaries, 127 passed. fe-api lib R9
filters + `input::` `screenshot::`: 125 passed. fe-runtime lib `vision::`
`input::` `bridge::`: 8 passed. fractalengine bin `screenshot_bridge`
`vision_bridge` `input_bridge`: 33 passed. 0 failed throughout.

VERDICT: APPROVE-WITH-FIXES (6). The shared envelope, the gate-as-data and
the one parse path already held. This pass implemented the two binding
addenda the slices had missed (7: the remote crash; 6: `capture_after`
bypassing the screenshot gate) and consolidated the gate-status mapping and
the failure builder. It also corrected the stale names and descriptions.
