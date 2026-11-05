---
type: SourceReport
title: Camera command ownership source author report
date: 2026-11-04
---

# QA49 camera command ownership: source author freeze v1

Implemented the authorized narrow keyboard-ownership correction. **This is source authorship, not approval or repaired runtime acceptance.** The parent owns independent review, builds, final integrated checks, live QA, Git and the provisional convention ledger.

Baseline: **`71cd5f8e9f05bd39dbb1a61c17a95ca054dda5fe`**. Exactly two source/doc paths are included:

| Path | Bytes | SHA256 |
| --- | ---: | --- |
| `fe-renderer/src/camera.rs` | 55858 | `136006c24d5faa6fa773872f44dad90c91fa31c7ae518acd1de5944b0b595ff2` |
| `fe-renderer/src/AGENTS.md` | 20846 | `ec938c058d0fc09c8ca13a6692f29750406bc4338d26106824fedfddb473ca7f` |

Frozen packet: `.omc/research/qa49-camera-command-ownership-v1/`.

- `manifest.json`: **1069 bytes**, SHA256 **`8001920495601fde46cbebf737271f8e94d82fd52c02af55cf7bcd90e63494c2`**.
- `source.patch`: **10897 bytes**, SHA256 **`57f003210bff97956490b3fe4f52ba62da41c82295bf9e4eb405322acbfe0d9e`**.
- `raw/`: complete exact bytes of both files, matching the working sources. The manifest separately records baseline Git blob byte counts/hashes; those baseline blobs use Git's representation, not a claim about former working-tree EOL bytes.

## Problem and resulting behavior

Actual QA49 established successful valid Position save/Undo/Redo and version4→5→6→7, alongside camera focus Y0→-0.740883→-2.252640. The source trace `.omc/research/qa49-undo-camera-source-trace.md` identified held ControlLeft as both camera descent and an editor command modifier, with Shift accelerating the camera during Redo. Correct modifier-prefix injection exposes this collision before Z is pressed; no stuck-key or missing-scene-residency theory is required.

`camera.rs:519` introduces one private pure `keyboard_fly_intent` helper. It returns an optional normalized direction and speed multiplier, without mutating keys, camera/controller state, managers or resources. Existing egui keyboard ownership or any held ControlLeft/ControlRight/SuperLeft/SuperRight returns no movement intent, including modifier-only prefix frames. ControlLeft is removed from descent; plain Q remains. Alt behavior is unchanged, as required by the authorized narrow scope.

The same helper contains the extracted existing WASD and Q/E/Space mapping, S just-pressed reservation, diagonal normalization and ShiftLeft2.5 speed multiplier. The system uses its optional result at :685–690, preserving the existing arithmetic order: speed=move_speed×delta_secs, then normalized_direction×speed×sprint. Only actual nonzero fly intent clears target_focus. The helper allocates nothing and is used by both the live camera system and the existing test journey.

The rest of the camera system still runs when keyboard intent is reserved. No early system return, capture-lock change, mouse orbit/pan/zoom change, pointer geometry/gutter routing change, easing/terrain-clamp change or fake camera baseline was introduced. The existing egui boolean and whole-system CameraCaptureLock run_if are retained. Editor shortcut decoding, injected modifier lifetimes, selected-set managers, checked Undo/Redo/history and authority gates are untouched.

## Source journey coverage, not executed

The existing `capture_lock_stops_a_run_if_gated_system_from_running` test at :1352 calls added test-only assertions at :1252 before retaining its original completed-chrome and capture run_if checks. The test declaration/name is unchanged, and the file still has **40 existing #[test] declarations**. No new registered test or harness was added.

The production helper is exercised through all four Ctrl/Super modifier variants, each with ordinary Undo and Shift-Redo phases: modifier-only prefix, optional Shift prefix, Z press, Z release, modifiers release. Assertions require no camera intent at every phase and verify the command key remains pressed for the editor consumer. A separate held-Q sequence proves command ownership suppresses an already-held movement key and plain descent resumes when the modifier releases. Plain W/A/D/Q/E/Space directions, ShiftLeft2.5 speed, UI text ownership, S first-press/held behavior and normalized W+D diagonal movement are retained in the authored assertions. The existing generic capture-lock schedule check still tests its real run_if helper; it is not claimed to execute the full render-feature camera system headlessly.

These cases were authored from the primary local Bevy0.18.1 ButtonInput contract (press/pressed/release/clear), already inspected during the trace. No tests, compiler, parser, formatter or application was executed. Runtime acceptance must still demonstrate pose-stable Undo/Redo plus plain navigation/Shift and focused text/capture behavior on the reviewed rebuilt stage.

## Navigation help and documentation scope

Read root/module AGENTS, shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`, Conductor product/stack/workflow and current completion scope/plan under the previously applied conductor-context skill. Rationale and the provisional behavior/evidence limits live in the new directory documentation section `AGENTS.md:62`, with only a one-line pointer on the helper.

Targeted source/help searches over fe-ui, fractalengine, fe-renderer, README and docs found no current user-facing Ctrl-descent label requiring an extra edit. `docs/architecture.md:117` says WASD movement, which remains correct; `fe-ui/src/panels/toolbar.rs:109` advertises Ctrl/Cmd+Z Undo/Shift Redo, which remains correct; `tool_inspector.rs:52` advertises Ctrl-drag for path-point height, a separate retained interaction. No navigation-help path outside the two owned files was changed. Historical QA/source traces remain immutable. The parent owns the separate provisional convention ledger entry.

## Checks, preservation and approval boundary

Read-only scoped Git diff was inspected, and scoped `git diff --check` reports no whitespace errors. The freeze contains complete raw bytes and a two-path diff against the explicit baseline; final hash comparison matches both live working files to both raw snapshots. These are source/evidence checks, not a parser or test run.

Other live edits in native worker/doc paths, findings, Conductor ledger/session log and QA47–50 evidence were observed and preserved; none are included in this author packet. No Git staging/commit, production app/input replay, compiler/test/parser/formatter, OS/process/native/API/DB/network operation occurred. No independent approval, finding closure, positive camera acceptance, FULL, native-provider, metric, peer, CI or release result is claimed. Source review and parent-owned integrated/runtime evidence remain required before acceptance.
