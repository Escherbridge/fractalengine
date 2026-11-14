---
type: Review
---

# R-NEW-1: adversarial review of NEW-02, NEW-03, NEW-04 (security review included)

Scope: commits 98d1056c (NEW-02), 0fcbffdd (NEW-03), 69382781 (NEW-04) on top of f5706d04 (NEW-01), read against
the NEW tasks in turn4-dag.json and turn4-plan.md §4/§7. Security inbox: no entry names any owned file. No cargo run.

- N1 FIXED - fe-ui/src/actions/node.rs:539 - NEW-04's petal-hint test leg ran through the process-global GUI_DOOR, so a write left gating by a parallel fe-ui test would hold it (FIFO) and the refusal advisory leaked into other tests' drains; added private `submit_in(doors, ..)` behind `submit_scene_write_in_through` and the leg now uses the journey's own queue, plus asserts a gate refusal is never dead-lettered.
- N2 FIXED - fe-ui/src/panels/unsaved_edits.rs:116 - Retry ignored `retry_dead_letter`'s answer: a closed channel (letter kept) still toasted "Retrying", and a gate refusal toasted beside the door's advisory; now Sent/Waiting toast, Closed shows an error that it stays listed, Refused relies on the door's advisory.
- N3 FIXED - fe-ui/src/panels/unsaved_edits.rs:61 - the popup skipped `native_accessibility::surface::popup` (every other footer popup calls it) and had no scroll bound for up to 32 rows; it now registers the popup surface and reuses `status_bar::bounded_details` (made `pub(super)`, status_bar.rs:133).
- N4 FIXED - fe-ui/src/panels/unsaved_edits.rs:52 - chip used an ad-hoc amber literal; now the contrast-checked advisory notice palette (`theme::notice_fill(Advisory)` + `NOTICE_TEXT`).
- N5 KEPT (task-sanctioned signature change) - fe-ui/src/panels/unsaved_edits.rs:17 - `unsaved_edits_segment(ui, minimum) -> Option<UnsavedEditAction>` differs from the task's `(ui, ui_mgr, now)` form; the task's own "preferred" return-action approach requires it and status_bar.rs:442 applies it after the panel.
- N6 KEPT (ordering is the invariant) - fe-ui/src/actions/node.rs:504 - a backing-off head holds later gated writes; plan §7 "FIFO never blocked" is met as "never blocked unboundedly": the hold is now capped at about 7.15 s (50..2000 ms, 9th failed send dead-letters), and reordering same-node writes would be worse.
- N7 DEFERRED (trigger: a plain write observed overtaking a queued same-node write) - fe-ui/src/actions/node.rs:303 - plain writes (no petal or NotASceneWrite) skip the FIFO and can overtake a backing-off scene write; pre-existing at 7af0a144, but retries now make the window up to 7 s.
- N8 DEFERRED (trigger: frame stall reported after an "Unsaved edits" error) - fe-ui/src/undo.rs:919 - `reconcile` uses a blocking `send`; after a "Database stayed busy" dead letter the undo RequestFailed arm reconciles on a channel that may still be full, stalling the frame for one DB command; pre-existing, a try_send needs a re-arm rule for the timeout branch.
- N9 KEPT (verified) - fe-ui/src/undo.rs:943,1311 - both SAVE_TIMEOUT reconcile branches skip while `scene_write_in_door(request_id)`; the path-read branch is untouched; `scene_write_in_door` is evaluated last so the lock is only taken past the timeout.
- N10 KEPT (verified) - fe-ui/src/undo.rs:1146 - a dead letter's RequestFailed reaches the pending arm, which reconciles; the resolve reports "not applied" and restores the transform; "Move was not saved" does not trip the conflict test ("moved").
- N11 KEPT (verified) - fe-ui/src/node_manager/stamp_selection.rs:593 - stamp promotion passes `None` hint and is judged against `capture.petal_id` (carried_petal, node.rs:220); Waiting keeps `selection_promotion` as the pending state.
- N12 HANDED-OFF (I-UI-2) - fe-ui/src/node_manager/mod.rs:859 - NEW-02 test leg (a) (full bounded(1) channel -> Waiting, same correlation id delivered, history recorded) is not present at HEAD; it is NEW-02's NEEDS line. Note: an app-level leg shares the global door queue with parallel tests.
- N13 DEFERRED (trigger: GUI door queue made injectable per App, e.g. a Resource) - fe-ui/src/node_manager/viewport_pick.rs:300 - NEW-02 leg (b) (full-channel stamp promotion) not written: the door queue is a process static, so an app journey cannot pump it without sharing state with parallel tests or sleeping past the 50 ms backoff.
- N14 KEPT (security, verified) - fe-ui/src/verse_manager/db_results/nodes.rs:70, fe-ui/src/actions/dimension_callout.rs:322 - follow-up SetNodeProperty writes are now judged against the node's own petal (from the local DB thread's NodeCreated, not a peer), so a hook bound on the node's petal can no longer be bypassed by viewing another petal; the petal is never a caller-chosen value.
- N15 KEPT (security, verified) - fe-ui/src/actions/node.rs:643 - a dead-letter Retry re-gates the ORIGINAL pre-gate command against its recorded petal (not the now-active one) with the same correlation id; DB idempotency and the checked-mutation precondition reject a stale replay; a refusal is final.
- N16 KEPT (security, verified) - fe-ui/src/actions/node.rs:148,703 - dead-letter logs carry only the static label and reason, never command payloads; list capped at 32 and session-only; retained Sender clones do not block shutdown (DB loop exits on DbCommand::Shutdown, fe-database/src/lib.rs:2465).
- N17 KEPT (blocking by task default) - fe-ui/src/actions/node.rs:524 - `submit_scene_write_in` stays blocking from inside apply_db_results; the warn text now matches (only Closed can fail).

Verdict: PASS-WITH-FIXES (4 fixed, 2 deferred with triggers, 1 handed off to I-UI-2; no security defect found).
