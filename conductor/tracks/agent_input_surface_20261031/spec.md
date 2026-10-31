---
type: spec
title: Deny-by-default synthetic input surface for agent-assisted QA
track: agent_input_surface_20261031
tags: [feature, fe-api, fe-runtime, fractalengine, accessibility, agent-qa]
created: 2026-10-31
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: user grill 2026-10-31 (public seam S-AGENT-GUI)
depends_on: []
---

# Specification: Deny-by-default synthetic input surface

**Track ID:** `agent_input_surface_20261031` · **Type:** feature · **Priority:** P1
**Crates:** `fe-runtime` (bridge message types), `fe-api` (gate, surface, REST, MCP), `fractalengine` (binary: Bevy-side translation + wiring)
**Citations computed at:** `f55ca21`. The Lane B work these files came from has since been committed (`operator_gate.rs` added in `5405f4e`, Lane B closed in `8ee4a96`), and several line numbers below have moved — re-grep before editing; symbols are authoritative.

## 1. User vision

Public framing: **agent-assisted QA and accessibility automation** — an agent (or an accessibility tool) driving the API should be able to act on the GUI the way a human would, not only read it. This track adds the "act" half to a product that already has the "see" half (the screenshot surface).

## 2. Problem & evidence

The screenshot surface (landed, `f55ca21`) lets an agent SEE the live window but gives it no way to DO anything beyond what the ~31 always-advertised MCP tools / REST routes already cover headlessly. Several real GUI flows have no headless equivalent today — installing a map package through the Hexon Manager dialog, confirming a peer invite's role picker, dismissing a modal — because they are dialog/click-driven, not data-driven. An agent (or a real accessibility-automation client) that needs to drive these has no seam; it would have to fabricate OS-level input events outside the product, which this product cannot gate, rate-limit or audit.

**The precedent this track extends, verified at `f55ca21`:**
- `fe-api/src/screenshot.rs:26-37` declares `SCREENSHOT_GATE: OperatorGate` as a `const` — the gate is DATA, not a copied code path (`fe-api/AGENTS.md:1723-1724`: "A new gated endpoint adds a `const` and calls the gate — it does not copy the chain").
- `fe-api/src/operator_gate.rs:13-28` (`OperatorGate` struct) + `:60-115` (`opted_in_from_env`, `check_opted_in`, `check_rate`, `admit`) is the full reusable chain: opt-in -> typed Owner (`fe-api/src/operator_gate.rs:110`, `require_role_level(claims, fe_database::RoleLevel::Owner)`) -> per-DID rate limit.
- `fe-runtime/src/screenshot.rs:1-26` is the exact shape a new bridge needs: a bounded capacity const (`SCREENSHOT_BRIDGE_CAPACITY = 4`), a correlated request struct with an embedded oneshot reply sender, a reply struct echoing the correlation id, and a `screenshot_bridge()` constructor returning `(Sender, Receiver)`.
- `fractalengine/src/screenshot_bridge.rs::drain_screenshot_requests` (`:83` after `5405f4e`) is the Bevy-side pattern: drain everything queued with `try_iter()`, no-op on empty (no allocation in the common case), one real capture serves every queued request.
- `fractalengine/src/main.rs:413-415` (bridge construction), `:544-548` (`ApiState.screenshot` field), `:559-566` (Bevy resource + system registration) is where the wiring lands for any new bridge of this shape.
- `fe-api/src/mcp/mod.rs:184` (`static TOOLS`) + `:260-261` (the `screenshot` row, `requires Screenshot, content`) + `:662-668` (`TOOLS.len() == 37` assert with a message explaining the count) is the table a new tool row and a new `ToolSeam` variant both touch.
- `fe-api/tests/screenshot_test.rs:1-90` is the test precedent: a fake Bevy-side responder thread stands in for the real window (renderer-free), driven through `fractalengine_test_harness::api::ApiHarness::spawn_with_screenshot`.

**Gap this track closes:** none of the above lets an agent *act*. The screenshot bridge is strictly one-directional (Bevy -> API). This track adds the API -> Bevy direction for input, using the identical posture.

## 3. Scope / Non-goals

**In scope:**
- A new bounded `fe-runtime` channel carrying synthetic input commands from the `fe-api` async worker into the Bevy main thread, translated into real Bevy input early in the frame.
- Reuse of `operator_gate::OperatorGate` as-is: a new `const`, opt-in env var, Owner floor, per-DID rate limit — the exact three gates screenshot already proved.
- MCP tool + REST route (OD-1), deterministic `wait_frames` settle semantics, an optional post-action screenshot in the same correlated reply.
- Never wired on the headless relay (`fractalengine-relay` has no window, same as the screenshot bridge).
- Updating the MCP `TOOLS.len()` integrity assert.

**Non-goals:**
- No OS-level event injection outside this product's own Bevy input resources (no raw `winit`/driver-level synthesis; translation targets the same input surfaces `bevy_egui` already reads every frame).
- No new authorization model — Owner-of-any-scope passes, exactly like screenshot (`fe-api/AGENTS.md:1768`, §screenshot's posture note).
- No business framing or references to downstream consumers anywhere in this file, code, or commit messages (HARD RULE, `conductor/open-core.md`).
- No change to the existing screenshot surface's own behavior; `capture_primary_window_png` gains one visibility change (`pub(crate)`) and nothing else.
- No input *recording* or macro-playback UI — this is a one-shot inject-and-settle primitive, not a scripting layer.

## 4. Design

### 4.1 Seam name: `S-AGENT-GUI`

Mirrors `§screenshot` symbol-for-symbol, one layer added at a time:

| Layer | Screenshot (existing, proven) | Input (this track) |
|---|---|---|
| Bridge types (`fe-runtime`) | `ScreenshotRequest{correlation_id, reply_tx}`, `ScreenshotReply{correlation_id, result}`, `screenshot_bridge()`, `SCREENSHOT_BRIDGE_CAPACITY=4` | `InputRequest{correlation_id, commands: Vec<InputCommand>, wait_frames: u32, capture_after: bool, reply_tx}`, `InputReply{correlation_id, result: Result<(), String>, png: Option<Vec<u8>>}`, `input_bridge()`, a capacity const (OD-independent; sized for the same busy-refusal posture) |
| Gate (`fe-api::operator_gate`) | `SCREENSHOT_GATE: OperatorGate` with `opt_in_env="FE_API_SCREENSHOT_ALLOW"`, `rate_per_sec=1` | `INPUT_GATE: OperatorGate` with `opt_in_env="FE_API_INPUT_ALLOW"` (OD-1 confirms the exact env name), `rate_per_sec` per OD-2 — the SAME `OperatorGate` type, a new const only |
| Surface (`fe-api`) | `ScreenshotSurface{allowed, bridge, reply_timeout}`, `screenshot_allowed_from_env()`, `ScreenshotError` | `InputSurface{allowed, bridge, reply_timeout, wait_frames_cap}` (OD-3), `input_allowed_from_env()`, `InputError` — same `From<GateRefusal>` pattern |
| REST | `GET /api/v1/debug/screenshot` (`fe-api/src/server.rs:308-311`) | `POST /api/v1/debug/input` (OD-1), registered beside it |
| MCP | `screenshot` row, `requires Screenshot, content` (`mcp/mod.rs:260-261`) | `inject_input` row (OD-1), `requires Input`; `ToolSeam::Input` added beside `ToolSeam::Screenshot` (`mcp/mod.rs:124-129`), with matching `tool_available`/`operator_opt_in_refusal` arms (`:132-149`) |
| Bevy side (`fractalengine`) | `screenshot_bridge.rs::drain_screenshot_requests`, one `Screenshot::primary_window()` capture per batch | `input_bridge.rs::drain_input_requests` — translates each queued `InputCommand` into real Bevy input, tracks `wait_frames` countdown, optionally calls back into `capture_primary_window_png` (made `pub(crate)`) for `capture_after` |
| Headless host | `ScreenshotSurface::disabled()`; `fractalengine-relay` never wires the bridge | `InputSurface::disabled()`; same relay never wires this bridge either — there is no window to inject into |

### 4.2 Command vocabulary (OD-6)

`InputCommand` (v1, recommended): `PointerMove{x, y}`, `PointerClick{x, y, button}`, `PointerDrag{from: (f32,f32), to: (f32,f32), button}`, `KeyPress{key: KeyCode}`, `TextEntry{text: String}`, `Scroll{dx: f32, dy: f32}`. Coordinates are primary-window pixel space (OD-4), matching what an agent computing a target from a screenshot's own pixel dimensions would already have on hand.

### 4.3 Translation into real Bevy input (the part screenshot didn't need)

`drain_input_requests` runs in `PreUpdate`, ahead of whatever system `bevy_egui` 0.39 uses to collect its own frame input (the pinned version in this workspace, `fractalengine/Cargo.toml`; the exact ordering constraint — a system set or explicit `.before(...)` — is confirmed against that pinned version at implementation time, not assumed here). Each `InputCommand` is converted into the same resources real OS input populates: cursor position on the primary `Window`, `ButtonInput<MouseButton>` press/release, `ButtonInput<KeyCode>` press/release, and a scroll delta — i.e., synthetic input looks identical to real input to every downstream system, including `bevy_egui`'s. `TextEntry` is translated through whatever text-input path `bevy_egui` reads (IME-style text events on this Bevy/egui pairing) rather than a sequence of per-character key events, since multi-byte and IME-composed text cannot be faithfully reconstructed from `KeyCode` alone.

### 4.4 Deterministic settle + optional post-action screenshot

`InputRequest.wait_frames` tells the Bevy host how many frames to advance past the injected input before replying — the agent never races a half-applied UI update (a dialog mid-open-animation, a debounced system like the terrain preview's ~100 ms throttle) when it next reads state. A per-request countdown (tracked in a small resource/local, not per-frame-allocated) decrements each frame; at zero, if `capture_after` was set, the Bevy side calls the same `capture_primary_window_png` helper `screenshot_bridge.rs` already owns (extracted to `pub(crate)`, sole owner unchanged) and attaches the PNG to the reply; otherwise it replies immediately. This gives one correlated round trip for "click, settle, see" instead of two separate API calls.

### 4.5 Gate order (identical to screenshot)

REST: `check_opted_in` -> typed `require_role_level(Owner)` -> `check_rate` -> bridge `try_send` (full -> busy refusal, disconnected -> host-gone refusal) -> await reply with a timeout (-> `TimedOut`, never a hang). MCP: the dispatcher refuses the opt-in BEFORE `authorize` (mirrors `operator_opt_in_refusal`, `mcp/mod.rs:142-149`), then the tool row's own `min_role: Owner` enforces the floor, then the handler calls `check_rate` directly (FR-5 convention: `tools.rs` never calls a `require_*` guard itself).

## 5. Acceptance criteria (observable, real flows)

- **AC-1 (disabled):** With `FE_API_INPUT_ALLOW` unset, an Owner-token `POST /api/v1/debug/input` (or MCP `inject_input`) call is refused with the gate's `disabled_message` (REST 403) before role or rate is touched — proven by a fake-responder test that asserts the fake bridge never receives a request.
- **AC-2 (role floor):** With the env set, a below-Owner token's call is refused with `OWNER_REQUIRED` (REST 403) before the bridge is touched.
- **AC-3 (end-to-end flow):** With the env set and an Owner token: inject a click at a known on-screen coordinate that opens a specific panel, with `wait_frames` > 0 and `capture_after: true`; the returned PNG bytes differ from a pre-click baseline capture in a way consistent with the panel now being open (fake-responder test asserts the bridge receives the translated commands + the settle count; true pixel-level in-app confirmation is the standing user-gated in-app verify step, same as every other visual feature in this codebase).
- **AC-4 (rate limit):** A burst past the configured per-DID rate (OD-2) gets `GateRefusal::RateLimited` (REST 429) on the Nth call within the window.
- **AC-5 (headless host):** On `fractalengine-relay`, REST calls to the input route get the `Unavailable` ("this host has no window") error (503), identical in shape to screenshot's headless behavior; MCP's `tools/list` omits `inject_input` entirely because `ToolSeam::Input` is unwired (`tool_available` returns false when `state.input.bridge.is_none()`).
- **AC-6 (table integrity):** `cargo test -p fe-api table_rows_are_well_formed`-equivalent passes with `TOOLS.len() == 38` and the assert message updated to explain the new count (mirrors `mcp/mod.rs:662-668`'s existing self-documenting assert).
- **AC-7 (no new dependency edges):** `cargo tree` for `fe-runtime`, `fe-api`, `fractalengine` shows no new crate-to-crate edge beyond what screenshot already established (fe-api depends on fe-runtime; fractalengine depends on fe-api and fe-runtime) — this is purely an extension of an existing bridge shape, not a new coupling.

## 6. Open decisions (recommended defaults; proceed on them unless overridden)

| ID | Decision | Options | Gates |
|---|---|---|---|
| **OD-1** | Tool/route names | **(rec)** MCP `inject_input`, REST `POST /api/v1/debug/input` (mirrors `screenshot` / `GET /api/v1/debug/screenshot` naming exactly) · `synthetic_input` / `agent_input` | §4.1, AC-6 |
| **OD-2** | Rate-limit shape | **(rec)** per-REQUEST (a batch of commands counts once), 5/s per DID — screenshot is 1/s because a capture is heavier; a UI interaction is naturally multi-command · per-command metering | AC-4 |
| **OD-3** | `wait_frames` cap + timeout | **(rec)** a documented const (e.g. 120 frames, ~2 s at 60 fps); past it the reply is an error akin to `ScreenshotError::TimedOut`, never a held correlation slot · no cap (relies solely on the existing `reply_timeout` wall-clock bound) | NFR-5 |
| **OD-4** | Coordinate space | **(rec)** primary-window pixel coordinates, matching the dimensions a screenshot's own PNG already reports · normalized 0.0-1.0 | §4.2 |
| **OD-5** | Post-action screenshot mechanism | **(rec)** extract `capture_primary_window_png` to `pub(crate)` in `screenshot_bridge.rs`, call it from `input_bridge.rs` (sole-owner unchanged, one new call site) · require a second, separate `screenshot` call instead (no `capture_after` field at all) | §4.4 |
| **OD-6** | v1 command vocabulary | **(rec)** the full set in §4.2 (pointer move/click/drag, key press, text entry, scroll) — matches the brief's explicit list · pointer-only v1, keyboard deferred | §4.2 |
| **OD-7** | Test strategy for the Bevy-side translation | **(rec)** fake-responder API-layer tests only (mirrors `screenshot_test.rs`'s renderer-free precedent exactly); true input-to-pixel correctness is the standing user-gated in-app verify · add a renderer-backed integration test in this track | AC-3, §7 |

## 7. Risks

- **This surface can mutate state, not just observe it.** Screenshot is read-only by construction; a misdirected or abused input injection can actually change the scene (move a node, delete via a confirm dialog). The deny-by-default triad (opt-in, Owner, rate limit) is the *only* protection — there is no additional coordinate/action allow-list in v1. The review slice (S6) treats this as its primary finding category, not a footnote.
- **Translation drift from real OS input.** If the synthetic path diverges from what `bevy_egui` actually reads from winit (e.g., a text-entry mechanism that bypasses IME handling), an agent's "click landed" could be silently false. Mitigated by targeting the same input resources, verified against the pinned `bevy_egui` version at implementation time (§4.3), and by the standing user-gated in-app verify step before this is considered proven end-to-end.
- **Held correlation slots.** A `wait_frames` value with no cap could hold a reply (and its oneshot sender) indefinitely if the frame loop stalls. OD-3's cap is the mitigation; NFR-5 makes it a hard requirement, not just a recommendation.
- **Operator confusion with the screenshot surface's warning text.** Two similarly-named env vars (`FE_API_SCREENSHOT_ALLOW`, `FE_API_INPUT_ALLOW`) on the same host is easy to opt into one while meaning the other. Mitigated by distinct, explicit `enabled_warning` text per gate (`OperatorGate.enabled_warning` is already a per-const field, not shared).

## 8. Dependencies

- **External precondition (not a conductor `depends_on`, a landing-order constraint) — SATISFIED:** `polish_lane_20261031`'s Lane B work (frameless window, title bar, icon, the screenshot/operator-gate surface itself) was in flight at `f55ca21` and is now committed (`operator_gate.rs` added in `5405f4e`; Lane B closed in `8ee4a96`). S2 may start; re-grep line numbers first.
- **related:** `polish_lane_20261031` (shares the operator-gate pattern; S2 only ADDS a new const, never edits `operator_gate.rs`'s existing chain). Its later waves still own `fe-api/AGENTS.md` and `fe-runtime/src/AGENTS.md`, so S1/S3 wait for those waves to close.
- **Cross-track file order:** this track lands first on the files it shares with `relay_operability_20261031` and `plugin_host_revival_20261031` (`fe-api/src/{server.rs,lib.rs}`, `fe-api/AGENTS.md`, `fe-runtime/src/{lib.rs,AGENTS.md}`, `fractalengine/src/{main.rs,AGENTS.md}`); it alone moves the MCP `TOOLS.len()` assert (37 -> 38).
