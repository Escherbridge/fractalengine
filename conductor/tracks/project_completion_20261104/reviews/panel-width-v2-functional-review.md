---
type: Review
date: 2026-11-04
title: Corrected preferred width functional source review
source_artifact_sha256: 4cea2fbff7594768e5a677211fb3eaea1c9df464419535a73193dc4d317e27a0
---

# VIS-003 v2: independent functional/design correction review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for the frozen six-file v2 source correction atop `2bce88aff512a9998eb120ddbcde8b78b4f7aa84`.** The previous review's MED-1 is corrected. No additional blocking functional/design defect was found. This is source approval only; VIS-003 remains open pending integration, build and executed acceptance.

## Exact freeze and preservation

Read the worktree's `qa-panel-width-preference-v2-source-report.md`, receipt and patch, the preserved v1 report/receipt/review-anchor patch, the prior functional review and relevant production calls. Independently verified all six raw working-file SHA-256 values against the v2 receipt. The current full Git diff bytes exactly equal the frozen patch, SHA-256 `63cc9180e43bd860a53e2e42944014a01499c06095ebb35d72f8bc46a2b6b90f`.

Verified the preserved v1 patch hash `e5dd49550a06adc49f269672801fd41e2ec944f328fb2e35e2a007e2fb92282a`. Reconstructed the v1 working sources in memory from that patch and HEAD and matched their original receipt hashes, then inspected the actual v1-to-v2 differences. They are limited to the new current close-pending argument/predicate and removal of its old renderer overwrite, the focused authored journey extension, and additive directory rationale. Right-sidebar source is byte-identical to the reviewed v1. The rest of the width/geometry/header implementation remains preserved.

Existing test-attribute counts match before/after for every changed Rust file, with no new test declaration. `git diff --check` returns 0. No source execution is implied by these checks.

## MED-1 correction

`panels/mod.rs` passes the actual current `topbar_state.close_pending` into `render_left_sidebar`. That manager invokes `resize_interaction_allowed(ui_mgr, close_pending)` and sets the width owner's interactivity **before** calling the sidebar and its `width.prepare`.

The production predicate requires `!close_pending` together with the existing allowed UiManager dialog state and no-portal condition. The sidebar no longer re-enables the owner with its former dialog-only predicate. Thus the first close-pending pass is refused even when the close-confirmation Modal has not yet been painted after gardener. `set_interactive(false)` uses the existing suspension seam, retires preview without changing preference, and preserves held-input suppression. The existing shared `prepare`/register flow cannot commit that preview or revive the canceled held gesture merely because close pending is removed. Primary release is still required before a new admission. Right close-pending eligibility is unchanged.

The fix uses a narrow current-state argument and one shared production predicate; it adds no alternate modal/close authority and does not consume a close action or release another widget's capture. Current native A2 producer hooks and the existing camera/selection exclusion geometry are preserved.

## Authored journey

The existing width journey now carries actual `TopbarState` and `UiManager` values and uses the same production left predicate. It starts the owned left drag and preview, sets `close_pending` while explicitly asserting no egui modal layer exists, then releases in that first pending frame and asserts unchanged preference and no retained drag. A second case cancels while held, removes close pending before release and verifies suppression continues until release without preference mutation.

These assertions target the previously missing interval rather than relying on a generic blocked Boolean alone. They remain unexecuted source. The previous approval limits still apply to default/custom preference restoration, actual allocated widths, cap/stationary behavior, geometry/context/focus/Escape cancellation, animation and action reachability. Parent compilation, the integrated journey sweep and real responsive/gesture screenshots—including high-scale horizontal-scroll-and-invoke and actual close/modal cancellation—remain required before VIS-003 closure.

No compiler/formatter/Cargo/tests, apps, OS/process inspection, input, API/DB/store access, network requests, Git mutations or production edits were performed. Review consisted of read-only source/receipt/patch inspection, in-memory reconstruction/hash/declaration checks and this artifact. No helper, UIA/WGC/capture or full visual-clean acceptance is supplied.
