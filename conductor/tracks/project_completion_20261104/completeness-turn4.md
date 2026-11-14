---
type: Review
---

# C-NEW: completeness of project_completion_20261104 at round 4 (group NEW)

Read at HEAD `c878679d`, read-only, no cargo. Inputs: NEW-01..05 task text (`.omc/state/turn4-slices/`),
gate verdicts `review-turn4/R-NEW-1.md` (PASS-WITH-FIXES) and `review-turn4/R-NEW-2.md` (PASS-WITH-FIXES),
`R-INT-4.md`, turn4-plan.md §4/§6/§9. Leak scan of the files read for this review: clean.

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names this file or any NEW-owned file
(`actions/node.rs`, `undo.rs`, `stamp_selection.rs`, `viewport_pick.rs`, `unsaved_edits.rs`, `status_bar.rs`,
`db_results/nodes.rs`, `dimension_callout.rs`, `registry_source.rs`, `remote.rs`, `terrain_bridge.rs`).

## Round 4 items (plan.md "Round 4 additions", NEW-01..05)

| Requirement / task | Status | Evidence |
|---|---|---|
| NEW-01 retry bounds: 8 attempts, 50 ms doubling to 2 s, dead-letter cap 32, session-only | complete | `fe-ui/src/actions/node.rs:23-33` consts; `retry_delay` `:271`; cap + oldest-dropped warn `:146-153`; letters live only in the static `GuiDoorQueue` (never persisted) |
| NEW-01 `WaitingWrite` carries door, petal, original command, label, attempts, next_attempt; `write_label` | complete | `node.rs:81`, `:96-102`, `:112-114`, `write_label` `:247` |
| NEW-01 `advance` honours next_attempt; Full -> backoff, past the bound -> `DeadLettered("Database stayed busy")`; Disconnected -> `DeadLettered("Database connection closed")`; refusals final | complete | `node.rs:476-500` (`attempts > RETRY_ATTEMPTS` = 9th failed send, R-NEW-1 N6); `Settled::Refused` `:476-480`; no `Settled::Closed` remains |
| NEW-01 opt-in `submit_scene_write_retrying(door, sender, petal_hint, command) -> GuiSend` (pinned seam), `FullChannel { Report, Retry }`; existing callers keep Busy | complete | `node.rs:575-582`, `FullChannel` `:59`, `queue_retry` `:424-433`; plain/try variants pass `FullChannel::Report` `:520`, `:548-557` |
| NEW-01 dead-letter API: `scene_door_counts`, `DeadLetterView`, `dead_letters`, `retry_dead_letter` (re-gated, same correlation id, closed keeps listed), `discard_dead_letter`, `scene_write_in_door` | complete | `node.rs:126`, `:585-608`; retry re-submits `origin.original` against `origin.petal` with `FullChannel::Retry` `:643-667` |
| NEW-01 owner notification: one error toast, `RequestFailed` on the correlation id | complete | `drain_gui_scene_writes` `node.rs:709-728` |
| NEW-01 FIFO order, dead head leaves, retry re-enters at back | complete | `pump_locked` `node.rs:504-515`; retry goes through `submit_with` (back of queue) |
| NEW-01 test seam + journey (R2 `actions::node::`) | complete | `pump_scene_door_at` `node.rs:733`; `scene_write_door_retry_and_dead_letter_journey` `:894-1010+` on its own queue; R2 filter `conductor/test-batches.md:85` |
| NEW-01 docs: actions/AGENTS.md §scene-write-door opt-in row, bounds, dead letters, callers | partial | `fe-ui/src/actions/AGENTS.md:608-616` covers opt-in, bounds, cap, refusal-final, opt-in callers; missing: the unowned-retry consequence (user default 36), retry re-enters at the back, owner contract wording for Busy-keeping callers |
| NEW-02 undo `start` and `commit_path` use the retrying door with the entry's own petal | complete | `fe-ui/src/undo.rs:560-563`, `:875-878`; Busy folded to an error message `:577-578`, `:892-893` |
| NEW-02 SAVE_TIMEOUT guard skips reconcile while the write is in the door; path-read branch untouched | complete | `undo.rs:944-945`, `:1311-1314`; path-read branch `:1305` unchanged |
| NEW-02 dead letter -> RequestFailed restores the transform | complete | R-NEW-1 N10 verified `undo.rs:1146`; source path read, run pending |
| NEW-02 stamp promotion opt-in (petal from capture, Waiting keeps pending) | complete | `fe-ui/src/node_manager/stamp_selection.rs:593-609` |
| NEW-02 test leg (a): undo journey full-channel leg in `node_manager/mod.rs:859` | missing | R-NEW-1 N12 HANDED-OFF I-UI-2; R-INT-4 F5 DEFERRED (global door not pumpable from an app test) |
| NEW-02 test leg (b): stamp full-channel leg in `viewport_pick.rs:300` | missing | R-NEW-1 N13 DEFERRED (trigger: door queue injectable per App) |
| NEW-02 docs: undo timeout-guard rationale | missing | no `scene_write_in_door`/`SAVE_TIMEOUT` mention in `fe-ui/src/AGENTS.md` or `fe-ui/src/actions/AGENTS.md` |
| NEW-03 "Saving {n}..." spinner with hover text; renders nothing at (0, 0) | complete | `fe-ui/src/panels/unsaved_edits.rs:21-36` |
| NEW-03 "{n} unsaved" chip, accessible name "Unsaved edits", popup list with Retry/Discard, advisory palette, bounded scroll | complete | `unsaved_edits.rs:48-90` (R-NEW-1 N2-N4 fixes); applied after the panel `fe-ui/src/panels/status_bar.rs:410`, `:442`; `apply_unsaved_edit_action` `unsaved_edits.rs:101-131` |
| NEW-03 module registration | complete | `fe-ui/src/panels/mod.rs:24` |
| NEW-04 `submit_scene_write_in` (blocking, petal-hinted) + `_through` twin | complete | `node.rs:524-545` |
| NEW-04 earthwork and callout follow-ups judged against the node's petal | complete | `fe-ui/src/verse_manager/db_results/nodes.rs:70-83`; `fe-ui/src/actions/dimension_callout.rs:322`; `CreateNode` at `:365` carries its own `petal_id` |
| NEW-04 journey petal-hint leg (Refused for "p-node", Sent for "p-active") | complete | `node.rs:968-1010+` via private `submit_in` on the journey's queue (R-NEW-1 N1) |
| NEW-04 docs sentence (follow-ups name the node's petal via `submit_scene_write_in`) | missing | not in `fe-ui/src/actions/AGENTS.md` §scene-write-door |
| NEW-05 `download_with_progress` (provided) + `fetch_hexon_with_progress`, same pre-checks, verify on complete bytes, `fetch_hexon` delegates | complete | `fe-hexon/src/registry_source.rs:1869`, `:1964-1980` |
| NEW-05 remote progress reader: Content-Length > cap refused unread; total = CL, else listed size, else 0; throttle max(64 KiB, total/100); final (n, n) | complete | `fe-hexon/src/remote.rs:365-395`; `expected_total` from listed size `:489-497`; `download_cap` `:432` |
| NEW-05 `RegistryProgress { uri, bytes_received, total_bytes }`, monotonic forwarder, final N/N before `RegistryInstalled` | complete | `fractalengine/src/terrain_bridge.rs:459-463`, `:958-983`, install after `:984` |
| NEW-05 drain arm sets byte fields only | complete | `terrain_bridge.rs:1134-1142` |
| NEW-05 chunked core kept test-only | complete | `#[cfg(test)]` `terrain_bridge.rs:1027` |
| NEW-05 journey extended (ordered multi-step progress) | complete | `install_from_registry_installs_a_sealed_container_end_to_end` `terrain_bridge.rs:3287`, assertions `:3343-3363`; R4 filter `conductor/test-batches.md:162` |
| NEW-05 Downloads tab byte bar (HEXON-16 seam consumer) | complete | `fe-ui/src/dialogs/hexon_manager.rs:761-768` |
| NEW-05 docs: fe-hexon §registry-source-remote progress + CL refusal | missing | `fe-hexon/src/AGENTS.md:231-237` unchanged (R-NEW-2 F14 to I-DOC-CORE did not land) |
| NEW-05 docs: fractalengine "Install from registry" paragraph | missing | `fractalengine/src/AGENTS.md:762-785` still describes `FetchStrategy::fetch_header` as production (R-NEW-2 F15 to I-DOC-API did not land) |
| plan.md Round 4 additions section, ticks only on PASS / PASS-WITH-FIXES | complete | `plan.md:135-143` (both gates PASS-WITH-FIXES) |

## User-question defaults (turn4-plan.md §9)

| Q | Default | Code does it? | Evidence |
|---|---|---|---|
| 36 | Retry = "send it again" (re-gated, same correlation id, not recorded in undo history, no re-bind), documented | code yes, doc no | `node.rs:643-667`; the "unowned retry" consequence is absent from `actions/AGENTS.md` |
| 37 | 8 attempts, 50 ms -> 2 s (~7.2 s), 32 letters, session-only | yes | `node.rs:23-29`, `:146`, `:271`; docs `actions/AGENTS.md:609-613` |
| 38 | whole-package fetch only, 64 MiB cap, no chunked production path | yes | `registry_source.rs:58`, `remote.rs:432-436`, chunked core `#[cfg(test)]` `terrain_bridge.rs:1027` |
| 39 | H4 follow-ups stay blocking with the node's petal | yes | `submit_in` passes `blocking = true`, `node.rs:539-545` |

## Gate HANDED-OFF / DEFERRED lines and whether they reached the owner

- R-NEW-1 N12 (I-UI-2, undo test leg a): reached; R-INT-4 F5 deferred it (same trigger as N13). Still open.
- R-NEW-1 N13 (deferred, stamp test leg b): open, trigger "door queue injectable per App".
- R-NEW-1 N7 (plain writes can overtake a backing-off scene write), N8 (undo `reconcile` blocking send after a busy dead letter): deferred with triggers, open.
- R-NEW-2 F13 (HEXON-16, byte progress bar): landed, `hexon_manager.rs:761-768`.
- R-NEW-2 F14 (I-DOC-CORE): did not land (`4cc24404` touched fe-hexon docs elsewhere).
- R-NEW-2 F15 (I-DOC-API): did not land (`5459eb15`; paragraph at `fractalengine/src/AGENTS.md:762` still stale).
- NEW-01 docs NEEDS (I-DOC-UI): landed in part (`7635d9b6`, `actions/AGENTS.md:608-616`).

## Pre-existing plan tasks (scoper classes, turn4-plan.md §6.2 lines 493-504; owned by group REL / C-REL, not gaps)

- blocked-user: P0 feature-catalog reconciliation; P1 native/UIA items (provider root, NAV23, QA-0072, accessibility delivery, native exact style); require-existing startup; QA-0063 / QA0073 / QA0074 / VIS-001..003 / QA47 live acceptance; every P2/P3 live-acceptance item; P4 fmt/test/harness/iot-bridge sweep; all of P5 (push, `v0.1.0-qa.*` tag, Release workflow, downloads/hash checks, GHCR, retrospective).
- done (source; plan boxes still open for T-REL / live re-check): QA-0071 pointer arbitration and QA-0070 terrain credential/ancestry (`plan.md:31`, `:33`); QA0075 1160/1161 topbar overlap.
- obsolete: queued polish waves 2-6 (`plan.md:73`); P4 clippy line (`plan.md:90`) and the iot-bridge clippy clause.
- Spec "Required outcomes" rows (`spec.md:24-35`) are live-acceptance outcomes; their source work is tracked by the items above and stays user-gated.

## HANDED-OFF (owner = next round)

- HANDED-OFF (next-round I-DOC-UI) - `fe-ui/src/actions/AGENTS.md` §scene-write-door: add (1) a retried dead letter is unowned: applied by the general verse_manager arms, not recorded in Undo history, a stamp retry does not re-bind the selection (default 36); (2) Retry re-enters at the back of the FIFO; (3) follow-up writes built from a `DbResult` name the node's petal via `submit_scene_write_in` (NEW-04); (4) undo's SAVE_TIMEOUT reconcile waits while `scene_write_in_door` is true (NEW-02).
- HANDED-OFF (next-round I-DOC-CORE) - `fe-hexon/src/AGENTS.md` §registry-source-remote: document `download_with_progress` / `fetch_hexon_with_progress`, the Content-Length refusal before the body, and `expected_total` (listed size, else 0).
- HANDED-OFF (next-round I-DOC-API) - `fractalengine/src/AGENTS.md:762`: rewrite "Install from registry (PAS-P5 S7b, A9)": production uses `fetch_hexon_with_progress` (whole sealed package, byte progress, PACKAGE_MAX_BYTES 64 MiB); the chunked `FetchStrategy` core is `#[cfg(test)]`.
- HANDED-OFF (next-round NEW owner of `actions/node.rs` + I-UI-2) - NEW-02 test legs (a) `node_manager/mod.rs:859` and (b) `viewport_pick.rs:300`: needs a test-visible pump of the GUI door (a `pub(crate) #[cfg(test)]` wrapper over `pump_scene_door_at(&GUI_DOOR, ..)` with unique correlation ids, or an injectable door queue) before either leg can be written without timing.
- HANDED-OFF (next-round NEW owner) - R-NEW-1 N7 and N8 stay deferred with their triggers.

Verdict: PARTIAL - all five NEW items are code-complete at HEAD with gate PASS-WITH-FIXES and the four §9 defaults match the code; open are NEW-02's two test legs and four directory-doc gaps (I-DOC-UI, I-DOC-CORE, I-DOC-API), all handed off.
