---
type: Specification
date: 2026-11-04
status: in_progress
title: FractalEngine correctness, full acceptance and verified release
---

# Project completion scope

The user adopted the entire remaining-work assessment on 2026-11-04 as the new
completion goal and requested continued execution with all progress persisted in
Conductor. This track coordinates the existing QA, rich-data and polish tracks;
their requirements and evidence remain valid. It does not reopen archived work or
turn every historical roadmap checkbox into new implementation scope.

Deliver a usable spatial editor and reporting workflow in which complex typed
objects can be created, edited, copied and deleted; measurements can be appended;
positions and other transforms can change; and committed changes can be replayed
alongside charts of timestamped measurements. Keep the tree, groups, right
Inspector/Options panels and tools consistent around one ordered selected set.
Complete the original planned QA surfaces, polish waves 2-6, five live UX audits
and their synthesized waves 7+, followed by verified tagged release delivery.

## Required outcomes

| Workstream | Completion evidence |
| --- | --- |
| Terrain and spatial reports | Missing/partial terrain remains unavailable rather than a saved zero; complete true zero is supported; cut/fill save atomically without navigation history churn. Delayed results preserve newer gestures, selection and Undo. Known geometry independently validates volume, units, material and intersections, with exact versus conservative results labeled. Resolve the unintended brush edit. |
| Peer collaboration and assets | Actual Owner/Editor/Manager and recipient Viewer flows prove hierarchy plus ordinary nodes, byte transfer, native rendering/picking, updates, delegation, expiry/revocation and restart/recovery. Explicit legacy adoption preserves asset identity/bytes and node history while recording new provenance. Join acceptance alone is insufficient. |
| Selection and groups | Plain/Ctrl-or-Cmd/Shift behavior, primary fallback, partial/hidden members, overlapping groups, empty petals, membership, mixed properties, navigation and persistence agree across tree, Inspector/Options and tools. Supported multi-object edits use the common capability intersection, selection-center pivot, atomic save and one Undo; unsupported operations explain the refusal. Cover paths, stamps and derived objects. |
| Rich data and simulation | Final-build Alpha/Beta/Gamma creation, typed nested edits, appended out-of-order measurements, full transforms, completed-save replay and chart/table results match independent expectations. Cover schema rejection, exact integer boundaries, concurrent streams, journal rollback, pagination, unavailable/malformed history and same-root/distributed restart. |
| Permissions and lifecycle | Source fixes receive actual live verification for scope containment, credential generations, invite logging/import and derived materialization. Exercise role loss, queued authority, late/malformed receipts, cancellation, shutdown and disposable developer reset. Accepted unknown writes retain exact outcome ownership without replay. |
| Native QA and accessibility | Verify normal OS accessibility delivery and complete reviewed recipient-role automation without weakening Owner QA gates. Correct stale completed-output authority, suspend/resume, positive access eligibility and painted-producer accounting. Implement only reviewed helper/navigation/capture profiles; uncertain capture freshness remains unavailable. |
| UI and feature completeness | Verify layouts, focus, keyboard, scrolling, confirmations, hit targets and visibility at three sizes and relevant scales. Complete import/export, paths, stamps, terrain/Hexon streaming, webviews and real plugin/device flows in the original catalog. Keep text-sized thin glyphs and clear primary/additional/partial selection independent of chosen colors. |
| Acceptance and release | Reconcile the final source/catalog; obtain full coverage and two consecutive full passes with no new S1/S2; complete independent reviews, five live UX audits plus synthesis, final integrated checks, exact-commit green CI and verified tagged artifacts. |

## Working contract

Use readable, small interaction boundaries and existing managers, checked batches,
history and authority seams. Keep rationale in directory documentation. Commit
coherent reviewed slices frequently. Apply all changes in a batch before the one
final integrated test/lint sweep; production builds and focused live evidence are
distinct from that sweep. Test authorship and source review do not count as
executed assertions. Keep immutable raw evidence and recorded failed attempts.

Record genuine product choices and reversible defaults in
[the decision ledger](../../decisions/qa-chain-20261102.md). Continue routine
authorized work without repeated permission requests. Decisions cannot waive
defects, permission checks, independent review or release gates. Preserve the
retained campaign store; destructive reset uses a disposable owned fixture.

## Completion boundary

No percentage is inferred from focused pass numbers or finding counts. A source
fix is not live acceptance, a successful build is not passing tests, and an API
response is not proof of native rendering. Current-state reads do not establish
historical authority or peer convergence. Existing exclusions and user checks
(including AC-11, opted-in input and delta-viz P1/P2) stay explicit.

The final release must refer to the exact locally verified commit. Wait for green
Build Artifacts CI before creating the immutable tag, then wait for the entire
Release workflow including Docker. Independently download/hash all seven archives
and SHA256SUMS, inspect Windows GUI assets/QUICKSTART/licenses and print permanent
release/download links and checksum results. Choose the tag date at actual
completion; do not preassign a release date or claim an unsigned package is signed.

## Source of truth

[metadata.json](./metadata.json) records machine status, [plan.md](./plan.md)
records the acceptance queue, and [checkpoint.md](./checkpoint.md) records the
latest evidence and next actions. The original [QA plan](../qa_chain_20261101/plan.md),
[rich-data plan](../rich_data_history_20261101/plan.md), [polish plan](../polish_lane_20261031/plan.md)
and [findings](../../../qa/findings.jsonl) retain their detailed scope.


## Screenshot-driven visual acceptance (user steering 2026-11-04)

Inspect actual screenshots during every changed UI workflow and fix visible
layout defects as work proceeds. The supplied example's one-character-per-line
Petal label is reproduced at 1280x720 in the retained baseline. Broken wrapping,
clipped or overlapping labels/controls, cramped cards/navigation, unreadable
text, inaccessible actions, unexpected blank/black output and illegible selection
states are defects, even when action/API assertions pass. Record the affected
state/size/scale and before/after screenshots, examine the images, and verify
neighboring responsive states. Automated geometry/locator checks supplement image
inspection; screenshot existence alone is not visual acceptance. Apply shared
C:/Users/atooz/.gemini/UI-UX-guidance.md. Carry unresolved visual findings into
Conductor and the QA catalog; do not give a clean-pass verdict while required
visual cases still fail.
