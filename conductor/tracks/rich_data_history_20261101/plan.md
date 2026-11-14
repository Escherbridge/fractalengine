---
type: Plan
date: 2026-11-01
status: in_progress
title: Rich data and history execution
---

# Execution

- [x] Source-grounded inventory and corrected dispatcher journal coverage.
- [x] User confirms both object replay and appended measurement charts.
- [x] RD1 typed editor source and independent static review; merged into main.
- [~] RD1 focused live typed value/draft/error/persistence proof in passes06/11/13; final all-layout editing proof pending.
- [~] Property grid source and independent review; fresh-stage three-size/action verification pending. DATA-01 (Viewer gating, QA-0091; typed validation): code done (fe-ui/src/panels/inspector.rs:388, 2204); run pending (user-gated).
- [x] Tree appearance source: adjacent icon palette and circular wheel/hex, local persistence and explicit selection.
- [x] Parent hierarchy authorization/atomic cascade correction before enabling confirmed tree branch deletion.
- [~] Independent tree/current-state adapter review complete; four-level appearance/cancel and same-root persistence proved in11/13; final menu/destructive layout checks pending. DATA-03 (Save/Cancel/Reset at 1280x720, QA-0112): code done (fe-ui/src/panels/tree_appearance.rs, action row outside the body scroll); live layout proof run pending (user-gated).
- [x] RD2 bounded versioned descriptors and authoritative enforcement; source committed, review fixes applied.
- [x] RD3 atomic existing-journal mutations/baselines and scoped replay; final Local-RBAC/history cleanup source approved.
- [x] Independent backend security/design approval and integration production compilation, including flexible revision migration.
- [~] RD4 current-state materialization/receipt adapter integrated; native history-v3 preview and calibrated chart/table verified in13; final coverage pending.
- [x] RD5 declarative driver/offline smoke and independent rechecks, including qualified provenance namespace and shallow optional snapshot defaults.
- [~] Real Both scenario: all38 functional checks passed in13; exact object/history/local-data restart passed, distributed restart failed QA-0035. Repair integrated, fresh build/live proof pending.
- [ ] Catalog reconciliation and source fingerprint review; changed-region and final full-coverage passes. DATA-06 census/reconciliation half: code done (round-4 slice); catalog_source_fingerprint stays null until the staged build (tools/qa/AGENTS.md:325); full-coverage passes user-gated.
- [ ] Final integrated checks and original campaign CI/tagged artifact publication.
- [~] Text-sized thin tree icons and explicit primary/additional/partial row states: source integrated and focused26 size/scale/primary evidence independently approved; multiple/partial behavior and final coverage pending.
- [~] Ordered selection and shared Inspector/Options/tool routing: focused30 independently verifies stable sampled tree rows, numeric Undo2/Redo2, whole-pair Move gimbal/Undo and nested shared-property replay. QA-0064/65 are fixed_verified_focused; focused32 adds Ctrl/Shift range, primary fallback, partial Group cues, right-panel/tool agreement and navigation clearing. Alias/derived/concurrency-stream and final coverage remain; QA-0068 is fixed_verified_focused after reviewed33 preserves scrolled row positions through plain/Ctrl/Shift selection. QA-0069 remains open:33 exposes the cut/fill history churn and source traces missing-coverage zeros plus unowned non-atomic persistence; its repair is being authored.
- [~] Complete checked Group lifecycle: source-approved through92039350; focused30 verifies Create/Delete/UndoDelete/UndoCreate/RedoCreate/RedoDelete and a second replay cycle with exact original IDs/memberships and retained objects. Empty/hidden/tombstoned targets, rollback and envelope boundaries remain execution gates. QA-0066 is fixed_verified_focused: reviewed31 confirms readable horizontal controls and actual Cancel preserving five complete responses. Zero-member Group in a ten-node Petal and its unchanged objects/history also pass; zero-node Petal remains.
  - Group lifecycle code notes (2026-11-14): DATA-04 stamp override write-through is code done (fe-database/src/handlers/stamp_transform.rs; live run user-gated). DATA-05 tombstone-hiding copied reads are code done (fe-ui/src/gis/egress_strings.rs:420 round-trip; `tombstone = NONE` rule documented by DATA5-02).
- [ ] Verify group creation/membership, mixed properties, capability refusals, selection navigation, pivot behavior and persistence; reconcile group sharing with the documented unmapped replication gap. DATA-02 selection auto-read and the "Reading selected objects" label (QA-0111): code done (fe-ui/src/editor_batch/ui.rs:547; journey fe-ui/src/panels/inspector.rs:5446 asserts no automatic retry, so manual Refresh stays the default, editor_batch/AGENTS.md:209); live reproduction user-gated. Registry publication is QA-0123 (fe-database/src/handlers/visibility_groups.rs:216 publish_registry, called at fe-database/src/lib.rs:1843, 1854, 2106; fixed_verified); membership convergence stays deferred per fe-database/src/handlers/scene_publication/AGENTS.md:120-122; live paired run user-gated.
- [x] Addressed native Viewer Join: focused28 independently verifies one-use input submission, actual Viewer role and matching hierarchy; shared nodes remain absent (Owner25/recipient0), tracked as QA-0063.
- [~] Committed-node publication/recovery and admitted-current native projection are source-approved, integrated and compiled, preserving source identity, later drafts and accepted host outcomes. QA-0067 is fixed_verified_focused: reviewed32 confirms the four affected native current reads admit verified bytes with expected transforms and settle. Fresh paired execution remains pending. Remote current state never substitutes for local journal versions.
- [~] Actual mesh-bound closed-path witness and lossless shared source rows are independently source/integration approved and compiled through7fc4445e. Closed-path transform/Group/Undo and actual GPU/picking proof remain; renderer source precision stays separate from rounded drafts.
- [~] Verified local-copy adoption, admitted byte transfer and native availability/adoption UI are independently source-reviewed and compiled through284ef388. Actual Owner adoption, peer bytes, failed-loader recovery and rendered GLB remain required; REST node arrival alone is insufficient.
- [~] QA30 records a named two-member Group baseline for the next retained-store restart. Independent QA31 review confirms the exact registry and both complete member responses survived restart. Native Viewer/Editor automation has an observation gap; local observer/accessibility design requires review without weakening Owner HTTP gates.
- [~] Completed-save history playback: source integrated and focused26 actual boundary/page/native refusal/playback evidence independently approved; malformed-journal journey execution and final coverage pending.

Current checkout ownership and immutable source commits are tracked in the campaign ledger and
`.omc/state/qa-chain-session.json`. Only the main owner runs Cargo or applications. Other authors remain in
isolated code-only checkouts. Review authors and executing authors remain separate; unresolved findings block
acceptance. Preserve `conductor/.conductor_session_log`.

Pass13's summary retains coverage_ready=false and expected negative-test warnings.
The old populated Petal is retained for read-only repair proof without re-ingestion.
A new full scenario and same-new-stage restart are required after repair; focused
passes do not replace the campaign's two clean full-coverage passes.


## Current coordination (2026-11-04)

The user adopted the full remaining-work assessment as the new completion goal.
Use [project completion plan](../project_completion_20261104/plan.md) and
[checkpoint](../project_completion_20261104/checkpoint.md) for the shared queue.
Focused pass33 is the latest committed evidence; QA-0063 and QA-0069 remain open.
The volume correction and native witness are unapproved isolated work, not accepted
features. Preserve all earlier rich-data acceptance requirements and evidence.

**Status 2026-11-13.** Per qa/findings.jsonl: QA-0035 fixed_verified_focused; QA-0063 closed; QA-0064, QA-0066 and QA-0068 closed; QA-0065 and QA-0067 fixed_verified_focused; QA-0069 (old plan meaning: cut/fill churn) is the earthwork volume work, now fixed_verified under project_completion_20261104. Every remaining [~] task waits only on a fresh staged build, a fresh full-scenario pass and the two clean full-coverage passes, which are blocked on the user-run campaign sweep.
