---
type: Review
track: extension_platform_20261107
gate: R-GATE
date: 2026-11-13
reviews: [X-GATE, I-GATE-GUI]
security_review: true
---

# R-GATE: one scene gate, GUI doors (turn 3)

Scope: `fe-runtime/src/{scene_hook_gate,scene_write}.rs`, the GUI door files of I-GATE-GUI, `fractalengine/src/gpx_bridge.rs`.
Mandatory security review run in a fresh context, prompted to refute "no GUI write path bypasses the gate".
Verdict: **core sound; the GUI is NOT yet bypass-free.** Six GUI paths outside this gate's partition still send hooked commands raw (R-GATE-07).

## Findings

- R-GATE-01 FIXED: GUI door queue could reorder writes. A write that settled at once was sent while an earlier gated write to the same node/key was still waiting, so the stale value landed last. The queue is now FIFO (`fe-ui/src/actions/node.rs` `GuiDoorQueue.waiting: VecDeque`); a write submitted behind a queued one queues too, and the drain stops at the first head still gating.
- R-GATE-02 FIXED: the drain sent admitted queued writes with `try_send` and dropped the result, so a full DB channel silently lost an admitted edit (AGENTS anti-pattern 6). A full channel now keeps the write at the head as `Queued::Admitted` and retries next frame; a closed channel is logged.
- R-GATE-03 FIXED: a queued write refused later (or lost to a closed channel) never answered its owner. GPX property/track outcomes stayed until `MAX_HOST_OUTCOMES` blocked further imports (an extension-driven DoS), and stamp saves stayed pending. The drain now writes `DbResult::RequestFailed { correlation_id }` for the write's id (`SceneWrite::correlation_id`, `fe-runtime/src/scene_write.rs`). The owners' existing `RequestFailed` routes release them.
- R-GATE-04 FIXED: per-frame allocation. `drain_gui_scene_writes` cloned the active petal every frame, and `SceneWriteTicket::poll` built a fresh `Vec` on every poll while a guest answered. The drain now compares before `clone_from`, and `poll` settles checks in place.
- R-GATE-05 FIXED: a stale active petal could authorize node writes. The drain that records the active petal ran unordered with the action handlers, so a write in the frame after navigation used the previous petal's bindings. It now runs `.before(process_ui_actions)` (`fe-ui/src/node_manager/mod.rs`).
- R-GATE-06 FIXED: a door refusal or closed channel leaked pending stashes. Dimension callouts leaked their pending geometry and earthwork regions their pending material. Both now take the stash back on `Refused`/`Closed` (`dimension_callout.rs` `save_measure_as_callout`, `terrain_proposal.rs` `create_region_node`).
- R-GATE-07 HANDED-OFF (orchestrator / C-EXT; no turn-3 owner): **GUI paths that still bypass the door.** Each one sends a hooked command raw:
  - `fe-ui/src/undo.rs:558` (`UndoState::start`): undo/redo `replay` and path `commit_path` send `CommitCheckedNodeMutation` (scene.property / scene.transform).
  - `fe-ui/src/undo.rs:857` (`commit_path`): sends `CommitCheckedNodeMutation`.
  - `fe-ui/src/editor_batch/mod.rs:755`: multi-select batch `CommitCheckedEditorBatch`.
  - `fe-ui/src/actions/earthwork_volumes.rs:650`: `CommitCheckedEditorBatch`.
  - `fe-ui/src/node_manager/stamp_selection.rs:592`: `PromoteCheckedStamp` (scene.create).
  - `fe-ui/src/dialogs/gltf_import.rs:253` via `send_or_toast`: `ImportGltf` (scene.create).
  - `fe-ui/src/verse_manager/db_results/nodes.rs:71`: earthwork `SetNodeProperty` follow-ups.
- R-GATE-08 KEPT: the gimbal asks the gate through a proxy. `transform_broadcast.rs` asks scene.transform with an `UpdateNodeTransform` proxy, while `UndoState::transform` sends the checked commit. The hook inputs are identical and a refusal rolls back (journey in `node_manager/mod.rs`). The proxy goes away when R-GATE-07's undo.rs NEEDS lands.
- R-GATE-09 KEPT: GUI node writes that carry no petal are authorized against the active petal, not the node's own petal. The GUI spawns and edits only active-petal nodes, and undo history is per petal. Residual: a navigation and a write inside the same `process_ui_actions` call. Trigger: any GUI path that edits non-active-petal nodes.
- R-GATE-10 KEPT: with no active petal, a GUI write is a plain send. No binding can resolve without a petal, and no node can be edited then.
- R-GATE-11 DEFERRED: GUI create augments are dropped with a warning because the created node id is unknown at send time. Trigger: correlate `NodeCreated` back to the door so `SceneAugments::stage(Some(id))` can run.
- R-GATE-12 KEPT: Rename, delete, view, url and stamp-override writes pass the door as plain sends. The bindings.md §6 v1 catalogue has no hook for them, and this is recorded in `SCENE_WRITE_CENSUS`.
- R-GATE-13 KEPT: the gate is installed before the API serves (`fractalengine/src/main.rs:740-750`). Without a plugin host the gate is disabled, and no binding exists to bypass.
- R-GATE-14 KEPT: verified the fail-open rules. Faults and timeouts fail open (OQ-B4), counted in `scene_hook_gate.rs` `Waiting::settle`. Over-cap caller-sized input refuses (`begin` at the cap check; `SceneWriteDoor::over_call_cap` refuses before any call). Deadlines are absolute from `begin`, so the API's sequential `wait` is bounded by one hook deadline.
- R-GATE-15 DEFERRED: no test drives the GUI queue. `GUI_DOOR` is a process static shared by parallel test threads, so a held ticket would leak `Waiting` into unrelated tests. Trigger: move the queue into a Bevy `Resource`, then extend the node_manager refusing-gate journey with a held-then-refused property write that asserts `RequestFailed` on its id.
- R-GATE-16 HANDED-OFF (doc owner): `fe-ui/src/actions/node.rs` points at `actions/AGENTS.md` §scene-write-door, and that section does not exist.
