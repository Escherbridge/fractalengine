---
type: plan
track: agent_input_surface_20261031
title: "Implementation plan: deny-by-default synthetic input surface"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: f55ca21
partitions_confidence: hypothesis
---

# Implementation plan: agent_input_surface_20261031

> Citations computed @`f55ca21`. Partitions are a hypothesis (not yet `/slice`-verified against a moved HEAD). `polish_lane_20261031`'s Lane B work has since landed (`5405f4e`, closed `8ee4a96`), so re-grep every "owns" path and line cite before launching a slice. Cross-track order on shared files: this track first, then `relay_operability_20261031`, then `plugin_host_revival_20261031`.

## 0. Assumptions — D-AIS-1..7, proceeding on spec §6's recommendation unless the user overrides

- **D-AIS-1** names: MCP `inject_input`, REST `POST /api/v1/debug/input`. *Reversal cost: low* (string constants + one assert message).
- **D-AIS-2** per-request rate limiting, 5/s per DID. *Low* (one field on the `OperatorGate` const).
- **D-AIS-3** `wait_frames` capped at a documented const; past it, an error. *Low* (one constant + one branch).
- **D-AIS-4** pixel coordinate space. *Medium if reversed later* (every caller and test would need rescaling).
- **D-AIS-5** `capture_primary_window_png` extracted to `pub(crate)`, reused by `input_bridge.rs`. *Low* (one visibility keyword).
- **D-AIS-6** full v1 command vocabulary (pointer + keyboard + scroll) in one slice, not phased. *Medium* (dropping keyboard later would be a seam change, not just a flag).
- **D-AIS-7** fake-responder tests only this track; renderer-backed verification stays user-gated in-app, same as every other visual feature here. *Low.*

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** No slice runs cargo build/test/clippy itself. The orchestrator gates once per wave (`-p fe-runtime -p fe-api -p fractalengine`), and runs the full workspace sweep once at track end.
- **The gate is reused as data, never forked.** S2 adds exactly one new `OperatorGate` const; it must not touch the struct, `admit`, `check_opted_in` or `check_rate` in `operator_gate.rs` — they are `polish_lane_20261031`'s Lane B code (landed in `5405f4e`; external precondition satisfied, metadata.json).
- **Mutation posture > read posture.** Every review pass (S6) treats "this can change state, not just show it" (spec §7) as the headline risk category, ahead of style or reuse nits.
- **Docs convention:** terse doc-comments in code; rationale in the directory `AGENTS.md` section named per slice.
- **Tests model real flows**, fake only the process edge (here: the Bevy render host itself, via the fake-responder thread — exactly the precedent `screenshot_test.rs` already set). No tautologies: every assertion must go red if the covered branch were inverted.
- **No read-only reviewers:** S6 fixes its own findings before recording a verdict.

## 2. Phase / slice table

| Phase | Slice | Wave | Model | Task | Owns (files) | Depends |
|---|---|---|---|---|---|---|
| Bridge | **S1** | W1 | sonnet | `fe-runtime` message types: `InputCommand` enum (`PointerMove`/`PointerClick`/`PointerDrag`/`KeyPress`/`TextEntry`/`Scroll`), `InputRequest{correlation_id, commands, wait_frames, capture_after, reply_tx}`, `InputReply{correlation_id, result, png: Option<Vec<u8>>}`, `input_bridge()` constructor, capacity const — direct structural mirror of `fe-runtime/src/screenshot.rs` | `fe-runtime/src/input.rs` (new), `fe-runtime/src/lib.rs` (`pub mod input;`), `fe-runtime/src/AGENTS.md` (§input-bridge) | none |
| Gate+surface | **S2** | W2 | sonnet | `fe-api` gate + surface + REST handler: `INPUT_ALLOW_ENV`, `INPUT_GATE: OperatorGate` const (D-AIS-1/2), `InputSurface{allowed, bridge, reply_timeout, wait_frames_cap}` (D-AIS-3), `input_allowed_from_env()`, `InputError` (`From<GateRefusal>`, same pattern as `ScreenshotError`), `request_inject()`/`gated_inject()`, the REST handler. Does NOT touch `operator_gate.rs` itself | `fe-api/src/input.rs` (new) | S1 |
| Wiring | **S3** | W3 | sonnet | `fe-api` wiring: `ApiState.input: InputSurface` field (beside `screenshot`), route registration beside `/api/v1/debug/screenshot` (`server.rs:308-311`), `pub mod input;` in `lib.rs`, MCP: `ToolSeam::Input` variant + `tool_available`/`operator_opt_in_refusal` match arms + new `TOOLS` row `inject_input` (`requires Input`, `min_role: Owner`) + `TOOLS.len()` assert bumped 37->38 with an updated explanatory message; `AGENTS.md` §agent-input-surface section (mirrors §screenshot's structure) | `fe-api/src/server.rs`, `fe-api/src/lib.rs`, `fe-api/src/mcp/mod.rs`, `fe-api/AGENTS.md` | S2 |
| Bevy translation | **S4** | W4 | sonnet | `fractalengine` binary: new `input_bridge.rs` (`InputBridge` resource, `drain_input_requests` system in `PreUpdate` ahead of `bevy_egui`'s own input collection, command translation into `ButtonInput<MouseButton>`/`ButtonInput<KeyCode>`/cursor position/scroll, `wait_frames` countdown tracked without per-frame allocation in the no-op path, optional `capture_after` call); `screenshot_bridge.rs` gets `capture_primary_window_png` changed to `pub(crate)` only (D-AIS-5, sole owner unchanged); `main.rs` wires `input_bridge()` + `ApiState.input` + resource/system registration beside the existing screenshot wiring (`:413-415`, `:544-548`, `:559-566`); `AGENTS.md` §agent-input-surface mirroring §screenshot | `fractalengine/src/input_bridge.rs` (new), `fractalengine/src/screenshot_bridge.rs`, `fractalengine/src/main.rs`, `fractalengine/src/AGENTS.md` | S1, S3 |
| Tests | **S5** | W4 | sonnet | `fe-api` tests: `input_test.rs` mirroring `screenshot_test.rs`'s exact shape — a fake Bevy-side responder thread standing in for the bridge (renderer-free); covers AC-1..AC-6 | `fe-api/tests/input_test.rs` (new) | S3 |
| **gate W4** | orchestrator | n/a | n/a | Serial build + `-p fe-runtime -p fe-api -p fractalengine` tests once | n/a | S4, S5 |
| Review | **S6** | W5 | opus | Review-and-fix (fresh context, `/code-review high`): deny-by-default posture parity with §screenshot, with the mutation-risk lens as the headline (spec §7); gate-order correctness (opt-in before authz on MCP, matching `operator_opt_in_refusal`'s existing contract); no allocation in the empty-channel path; `wait_frames` cap actually enforced (D-AIS-3); no new crate dependency edge (AC-7); fixes its own findings | `conductor/tracks/agent_input_surface_20261031/review.md` (new) + fix-only edits to any S1-S5 file | S4, S5 |
| Docs | **S7** | W6 | haiku | Docs consolidation: root `AGENTS.md` pointer (if this repo's convention adds one for new seams — check before adding), `conductor/tracks.md` row, plan ticks | `AGENTS.md`, `conductor/tracks.md`, this `plan.md` | S6 |
| **sweep** | orchestrator | n/a | n/a | Full workspace test + clippy `-D warnings` once; in-app verify (user-gated, standing convention): enable `FE_API_INPUT_ALLOW=1`, mint an Owner token, inject a click that opens a known panel, confirm via screenshot | n/a | S7 |

## 3. Flow tests per slice

Each name is the test to write; each asserts an observable outcome, not an internal call count.

**S1 (`fe-runtime/src/input.rs`; unit, pure)**
- `input_bridge_is_bounded_at_its_capacity`: a full channel reports `TrySendError::Full`, mirroring the screenshot bridge's busy-refusal contract.
- `reply_echoes_the_request_correlation_id`.

**S2 (`fe-api/src/input.rs`; unit)**
- `only_the_exact_value_one_opts_the_input_surface_in`: table-driven, same cases as `operator_gate.rs`'s existing `only_the_exact_value_one_opts_a_node_in` test, run against `INPUT_GATE`.
- `disabled_surface_refuses_before_role_or_rate`.

**S3 (`fe-api/src/mcp/mod.rs`; existing test file extended)**
- `table_rows_are_well_formed` (existing test) passes at `TOOLS.len() == 38` with the new row's description containing "owner role" (the existing generic assertion already enforces this — the regression this slice must not break).
- `inject_input_hidden_when_seam_unwired`: `tool_available` returns `false` when `state.input.bridge` is `None` (the AC-5 regression for MCP specifically).

**S4 (Bevy `App` flow, `input_bridge.rs`)**
- `queued_pointer_click_becomes_a_real_button_input_press_next_frame`: the R1-equivalent regression — asserts the translated resource, not an internal call count.
- `wait_frames_countdown_holds_the_reply_until_settled`.
- `empty_channel_allocates_nothing` (the NFR-2-equivalent discipline check, same shape as the terrain track's `nfr1_base_untouched` style invariant test).
- `capture_after_attaches_png_bytes_matching_a_direct_capture`.

**S5 (`fe-api/tests/input_test.rs`, full-router flow, renderer-free)**
- `disabled_without_opt_in_env` (AC-1).
- `refused_below_owner_role` (AC-2).
- `admitted_inject_round_trips_through_the_fake_bridge_with_capture_after_bytes` (AC-3, fake-responder equivalent).
- `rate_limited_past_the_per_did_budget` (AC-4).
- `headless_host_reports_unavailable_and_hides_the_mcp_row` (AC-5).

**Cut / declined:** no tests for the `InputCommand` enum's `Debug`/`Clone` derives, no snapshot tests of the AGENTS.md prose, no test asserting the exact English wording of a warning log line (only that a warning fires on opt-in, mirroring the existing `opt_in_value_enables` test's scope).

## 4. Task checklist

### Bridge + gate + wiring
- [ ] S1: `fe-runtime` input message types (+ §input-bridge doc)
- [ ] S2: `fe-api` gate + surface + REST handler
- [ ] S3: `fe-api` wiring (ApiState field, route, MCP table/seam/assert, §agent-input-surface doc)
- [ ] Gate W3 (orchestrator)

### Bevy translation + tests
- [ ] S4: `input_bridge.rs` + `screenshot_bridge.rs` visibility change + `main.rs` wiring (+ §agent-input-surface doc)
- [ ] S5: `input_test.rs` fake-responder flows
- [ ] Gate W4 (orchestrator)

### Close
- [ ] S6: review-and-fix (opus) with recorded verdict, mutation-risk lens first
- [ ] S7: docs consolidation
- [ ] Full sweep (orchestrator, once)
- [ ] In-app verify (user-gated): enable env, inject a real click, confirm via screenshot
- [ ] Retro + archive

## 5. Tripwires

- `InputSurface::disabled()` stays the default on every host without a window; never flip it to opt-in-by-default anywhere, including tests' own defaults.
- S2 must not edit `operator_gate.rs`'s existing struct or methods — only add a new `const` in `input.rs`. (The Lane B precondition is satisfied: `operator_gate.rs` is committed since `5405f4e`.)
- `capture_primary_window_png` keeps exactly one real owner (`screenshot_bridge.rs`); S4 calls it, never forks a second copy.
- `drain_input_requests` must run strictly before whatever system `bevy_egui` uses to collect its own input that frame — confirm the actual ordering constraint against the pinned `bevy_egui` version (`fractalengine/Cargo.toml`) at S4 implementation time; do not assume the terrain track's `PreUpdate`-adjacent scheduling transfers unchanged.
- Never let a `wait_frames` value exceed the cap (D-AIS-3) silently — past it, the request returns a timeout-shaped error, the same way a stalled screenshot does.

## 6. Commit strategy

One commit per slice after its wave gate is green: `feat(fe-runtime): S1 input bridge types`, etc. S6 lands as `fix(...)` commits plus `review.md`. S7 is a `docs(...)` commit. Trailer per the repo's current attribution rule. Pushes are held for user approval (standing directive) — the Lane B precondition is already met (`5405f4e`), so ordering against `polish_lane_20261031` now only concerns its later waves that own `fe-api/AGENTS.md` and `fe-runtime/src/AGENTS.md`.
