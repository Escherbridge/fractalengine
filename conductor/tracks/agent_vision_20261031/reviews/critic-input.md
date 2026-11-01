---
type: Review
title: Agent input critic review — clipboard chords escape the window; one error path on the REST door
track: agent_vision_20261031
role: critic
run: input
reviewed_commit: 53c25de
created: 2026-11-01
---

# Agent input critic review (run "input")

Lens: refute, don't confirm. Slices I1 (cc347b1), I2 (9882e1d), I3 (e3b37af),
I4 (94f34f3), I5 (ccc397a) and the advocate pass (53c25de). The questions were:
can injected input reach anything outside the window, can the input door be
reached without Owner + opt-in, do default-off and opt-out hold on every door,
can a remote caller crash or hang the host, and do the tests go red when the
covered line inverts. The default-on capture surface was already reviewed at
security depth in `critic-vision.md` (C1-C10); this pass re-checked only what
the input slices changed there (`GateRefusal::status`, `capture_after`).

## Findings

| # | Sev | Finding | Disposition |
|---|-----|---------|-------------|
| C1 | HIGH | **Synthetic input reached the host OS clipboard.** bevy_egui 0.39.1 is built with its default `manage_clipboard` feature. `write_keyboard_input_messages_system` turns a Control/Super + V press into `egui_clipboard.get_text()` (an arboard read of the host clipboard) delivered as `Event::Text`. It turns Control/Super + C/X into `Event::Copy`/`Cut`, which `process_output_system` writes back with `set_text`. The sticky-modifier planner made `[ControlLeft, KeyV]` (or `[ControlLeft, text_entry "v"]`, since `egui::Key::from_name("v")` is `V`) a working paste. An Owner agent could focus any text field, paste, and read the local user's clipboard back through `capture_after` or the default-on screenshot. Ctrl+C could overwrite the clipboard (clipboard poisoning). This contradicted the spec's "injected input can't escape the window (no OS-level input)" and the AGENTS.md claim "never becomes OS-level input". | **Fixed.** `fractalengine/src/input_bridge.rs` `plan_batch` refuses `clipboard_chord_refused` (422, before anything is injected) for a `key_press` of `KeyC`/`KeyV`/`KeyX`, and for any `text_entry`, while a synthetic Control/Super is held (`holds_command_modifier`, `CLIPBOARD_CODES`). Ctrl+A, Ctrl+S and similar chords stay legal. The Bevy journey test was extended (no new test fn): paste, Super+C and Ctrl+text "v" are refused with nothing held afterwards, and Ctrl+A still completes. Every assertion goes red if the guard is removed. Documented in fractalengine `src/AGENTS.md` §agent-input-surface "Clipboard chords are refused"; the MCP `inject_input` description states it too. |
| C2 | LOW | Residual for C1: bevy_egui's `ModifierKeysState` merges real and synthetic key events. If the local user is physically holding Control, a synthetic `KeyV` still pastes. | **Accepted.** The paste needs the local user's own action at that moment, and refusing every C/V/X would break ordinary typing. Documented next to C1. |
| C3 | LOW | UI-level effects that are not OS input: a synthetic click on the title bar's close button exits the app. A click on a "Browse…" button opens a synchronous native `rfd` dialog. Synthetic input cannot drive that dialog, and it blocks the frame loop until a human closes it; the job then replies `input_timed_out` and releases what it holds. There are no egui hyperlinks or `open_url` call sites, so `open_url` cannot launch a browser. | **Accepted and documented.** These fall within the "full remote control" posture that `INPUT_GATE.enabled_warning` names (default OFF, Owner, single-tenant). |
| C4 | LOW | Arrival order checks `camera_locked_by_capture` (409) before the scope check, so a foreign-scope Owner token learns that a posed capture is in flight. | **Accepted.** This is the same activity-only leak as critic-vision C8, and INPUT-ADDENDUM item 4 fixes this order. No pixels or existence are revealed. |
| C5 | LOW (simplification) | The REST door had three error-body builders (`InputErrorBody::gate`, `::refusal`, `::error`) and three early-return arms for one `InputError` type that already carries all three cases. | **Fixed.** `post_input` now runs gate → parse → `run_inject` in one `async` block with `?`, mapped through the single `InputErrorBody::error`. Status codes and wire bodies are unchanged (the vision_test input journey still pins 403/400/429/503/409 and the error text). |
| C6 | LOW (test) | `fe-api` `input` unit journey asserted restated constants (`INPUT_GATE.opt_in_env == "FE_API_INPUT_ALLOW"`, `rate_per_sec == 5`). Those are tautologies under the test policy. The 5/s budget is already pinned behaviourally by vision_test AC-4 (the sixth call is 429 with "5 input requests/sec"). | **Fixed.** Both asserts removed. The default-OFF tripwire (`black_box(INPUT_GATE.default_on)`) stays, because it is the load-bearing invariant. |

## Verified, no change needed

- **Default OFF holds on every door.** `INPUT_GATE.default_on == false`, and `opt_in_value_enables` lets only `1` enable it. REST runs `INPUT_GATE.admit` before parsing. MCP runs the dispatcher's `operator_opt_in_refusal` (Input seam) and the Owner floor, then the tool's own `check_opted_in` + `check_rate`. The relay and every harness `ApiState` use `InputSurface::disabled()`. A headless host gets 503, and the MCP row is hidden.
- **Composed gate for `capture_after` (INPUT-ADDENDUM 6).** `check_capture_after_gate` runs inside `run_inject`, which both doors share. It applies `SCREENSHOT_GATE`'s opt-in and its per-DID 1/s key (`screenshot:<did>`, the same budget a posed capture spends). So `FE_API_SCREENSHOT_ALLOW=0` also closes the input door's pixels. The journey pins this on REST and MCP.
- **Remote crash (INPUT-ADDENDUM 7).** Empty `text_entry` gets a 400 from `validate`, `plan_batch` refuses an empty plan, and `Injecting` uses `frames.get_mut`. I found no other unchecked index or `expect` reachable from request data: `LETTER_CODES`/`DIGIT_CODES` are indexed only after `is_ascii_uppercase`/`is_ascii_digit` filters on a one-char suffix, and the `expect("checked above")` calls follow an `as_mut()` `Some`.
- **Hang/DoS bounds.** At most 64 commands, text ≤ 4096 chars, `wait_frames` ≤ 120, scroll ≤ 1000 lines, a 64 KiB REST body, a bridge capacity of 4, single flight, `INPUT_WALL_CLOCK_CAP` 4.5 s (under the API's 5 s) on `Time<Real>`, and 5 req/s per DID.
- **Check at use.** Scope (`ViewScope::covers`), the camera lock, an abandoned caller and the wall-clock cap are checked every frame. Every exit goes through `finish`, which releases held buttons and keys, re-sends the real pointer and despawns an unread `capture_after` screenshot.
- **The user's own camera and cursor.** The override uses `bypass_change_detection` and is restored in PostUpdate, so it never warps the OS cursor. The idle path is one empty `try_recv`.
- **Logs.** The audit line carries did, scope, command count, wait_frames and capture_after. No text payloads, PNG bytes or tokens are logged.

## Gate

`cargo check -j4 -p fe-api -p fe-runtime -p fe-renderer -p fe-ui -p fractalengine --tests` and
`cargo +1.99.0 clippy … --all-targets -- -D warnings` both exit 0. Touched batches:
fractalengine `input_bridge` (1), fe-api lib `input::` (1), fe-api `vision_test` (10), fe-api
`mcp_integration` (31), fe-runtime `input::` (1). All pass.

VERDICT: PASS-WITH-FIXES (3)
