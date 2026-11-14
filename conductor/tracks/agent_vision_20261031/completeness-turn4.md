---
type: Review
track: agent_vision_20261031
critic: C-UI
round: 4
head: 50c50d29
date: 2026-11-13
---

# C-UI completeness: agent_vision_20261031 (round 4, with agent_input_surface_20261031)

Checked at HEAD `50c50d29` against spec.md AC-1..AC-11 and plan.md §5, plus round-4 item UI-11 (same-frame F12 + API capture) and the agent_input_surface fold. Gate read: `polish_lane_20261031/review-turn4/R-UI-1.md` (UI-11, F1 FIXED, F14 KEPT). Earlier verdicts: `reviews/` (ADVOCATE, CRITIC with security depth). No cargo was run.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file owned by this critic or a file of this track's round-4 items; nothing to close.

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| V1-V5, I1-I5, G1-G4, ADVOCATE, CRITIC (plan §5) | complete (carried) | Ticked with gate records in `metadata.json` (`runs.vision.gate`) and `reviews/`. |
| AC-1 default-on gate, `=0` refuses, garbage fails closed | complete (carried) | `fe-api/src/screenshot.rs:22`, `:100`; `fe-api/src/vision.rs:48`. |
| AC-2/AC-3/AC-4 validation, round trip, single flight | complete (carried) | `vision.rs:36` pose header, `:106` `capture_in_progress` (409). |
| AC-5..AC-8 framing, routing, streaming, camera read-back | complete (carried) | `fractalengine/src/vision_bridge.rs`; MCP `get_camera` `fe-api/src/mcp/mod.rs:178`. |
| AC-9 headless 503 / tools omitted | complete (carried) | CRITIC verdict in `reviews/`. |
| AC-10 input surface (agent_input_surface AC-1..AC-7) | complete (carried) | `fe-api/src/input.rs:37` `FE_API_INPUT_ALLOW`, `:50` `INPUT_GATE` default off; `fractalengine/src/input_bridge.rs`. |
| UI-11 same-frame F12 + API capture both delivered | complete | `fractalengine/src/screenshot_bridge.rs:176-182` `capture_pending_screenshots` holds the queue while a `Screenshot` was added this frame (`Added<Screenshot>, Without<Capturing>`), both system orders covered by journey `:588` `f12_waits_a_frame_while_an_api_capture_is_pending` (R9 `screenshot_bridge vision_bridge`). Residual: continuous per-frame API captures could postpone a held F12 (R-UI-1 F14 KEPT). |
| UI-11 docs | partial | `fractalengine/src/AGENTS.md:1186-1189` still lists the same-frame drop as "Known edge (not fixed)". |
| CLOSE: root `AGENTS.md` pointer to fe-api §screenshot/§camera-state and fractalengine §vision | missing | Root `AGENTS.md` has no such line (sections exist: `fe-api/AGENTS.md:1781` §screenshot, `fractalengine/src/AGENTS.md:1135` §vision). |
| CLOSE: `conductor/tracks.md` tick and status | missing | `conductor/tracks.md:88-95` both entries still `[ ]`; metadata says `done-pending-user-verify` (`metadata.json:6`). |
| CLOSE: plan ticks + metadata progress | partial | T-UI `13c82cf7` updated both plans and metadata; the CLOSE box `plan.md:305` stays open until the two lines above land. |
| Retro + archive (with agent_input_surface) | missing | `plan.md:308` open; archive waits on the user-gated AC-11. |

## Excluded by the scoper (not gaps)

- blocked-user: In-app verify AC-11, including minimized-window scene capture (plan.md:307).
- blocked-user: in-app input verification with `FE_API_INPUT_ALLOW=1` (Ctrl+V refused).
- blocked-user: Full sweep (once) (plan.md:306).
- obsolete: per-layer hide for scene captures (spec deferred).
- obsolete: critic C9 per-capture toast.
- obsolete: agent_input_surface plan S1-S7, Gate W3/W4 (all 12 tasks; merged into I1-I5 here).

## HANDED-OFF (owner = next round)

- HANDED-OFF (I-DOC-API next round, owner of `fractalengine/src/AGENTS.md`): replace the "Known edge (not fixed)" paragraph at `:1186-1189` with the UI-11 rule (F12 waits a frame while an API capture is queued or a `Screenshot` was added this frame; both delivered) and the F14 starvation note.
- HANDED-OFF (I-DOC-CORE / root AGENTS owner next round): add the one-line root `AGENTS.md` pointer to fe-api §screenshot / §camera-state and fractalengine §vision (plan CLOSE).
- HANDED-OFF (I-CONDUCTOR next round): set `conductor/tracks.md:88-95` agent_vision and agent_input_surface to done-pending-user-verify.
- HANDED-OFF (T-UI next round): tick CLOSE once the two lines above land; Retro + archive after the user's AC-11 verify.

Verdict: PARTIAL - every acceptance criterion an agent can verify is in code, including UI-11; only close-out docs and ledger lines remain, and AC-11 and the sweep are user-gated.
