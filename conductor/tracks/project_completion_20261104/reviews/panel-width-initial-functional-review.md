---
type: Review
date: 2026-11-04
title: Initial preferred width functional review and cancellation finding
source_artifact_sha256: 92e25099c339488aa73c3747e7c6ff022cf24dddf746533967c9fc2e2e80fcd4
---

# VIS-003 panel width preference: independent functional/design review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**CHANGES REQUIRED.** One medium functional gap remains in first-frame close-confirmation cancellation for the left divider. The preferred/effective geometry and owned divider design otherwise follow the approved plan in the inspected paths. No execution or live fix acceptance is supplied.

## Exact reviewed source

Reviewed `.omc/worktrees/qa-panel-width-preference` at `2bce88af`, with exactly the five dirty files in `.omc/research/qa-panel-width-preference-source-receipt.json`. Independently recalculated all five working-file SHA-256 values; all match. Existing test-attribute counts remain 1/0/3/13 across the four changed Rust files. `git diff --check` returns 0.

Read the author report, main design approval and scale-width investigation, complete shared adapter and changed renderers, actual call sites and current native producer hooks, existing viewport exclusion helper, close-confirmation scheduling, and pinned egui 0.33.3 SidePanel/Response/Context/Ui behavior. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`.

## MED-1: the left divider can commit on the first pending-close frame

`fe-ui/src/panels/sidebar.rs:36` sets width interactivity from UiManager's dialog/portal state only. Shared `PanelWidthState::interaction_allowed` adds focus/Escape, popup and the already-known egui modal layer. It has no access to the actual current `TopbarState.close_pending`. The right renderer explicitly adds `!topbar.close_pending`, so the two sides currently differ.

This matters before the close Modal has been rendered. `receive_close_requests` runs in Update and sets `close_pending=true` for an actual primary-window request. `render_close_confirmation` runs in the primary egui pass **after** `gardener_ui_system`. On that first pending-close pass, the left renderer can therefore see UiManager dialog None and no prior modal layer. If a previously admitted left drag is released in this pass, `prepare` can save its preview as preferred width before the close-confirmation Modal is created. Existing modal rejection only protects subsequent passes. This is a source-identified cancellation gap, not a runtime reproduction or an authority bypass.

Required correction: thread the actual current close-pending eligibility through the left render/adapter boundary, matching the right side, so it cancels the owned preview and suppresses the held response before any release commit. A narrow Boolean/current-state argument is sufficient; do not invent a second modal manager or rely on last-frame egui state. Extend the existing meaningful journey with current close pending arriving while the drag is held, immediately before release, with no preexisting egui Modal. Assert preference remains the original value and the held gesture cannot resume until release. Keep the normal right-side gate and existing shared cancellation behavior.

## Sound inspected behavior

- Private session preference is held by the existing left/right area Resources with defaults 220/320 logical points. Automatic caps affect effective width only. No SDK PanelState deletion/private resize-ID reconstruction, observed-width feedback or disk preference is introduced.
- Native SidePanels retain stable IDs and use exact effective widths with SDK resizing disabled. The right layout-cost stroke is removed, so native outer allocation does not acquire the previous extra four points. Native separator drawing remains available without adding layout margin.
- The adapter reads its own public Response under app-owned stable IDs and registers its normal Background-layer strip after panel contents. It uses the actual displayed panel edge/vertical extent and the existing resize radius. Viewport/camera/NodeManager exclusion functions and strict boundary behavior are unchanged.
- Drag admission captures actual displayed outer width and pointer origin. Preview does not alter preference. Commit requires a changed visible width beyond a physical-pixel-aware tolerance; stationary and cap-only motion preserve the earlier preference. Inward movement while constrained starts from actual displayed width.
- Context, viewport, pre-panel available geometry/budget, scale and pass continuity are captured consistently before allocation. Their change, focus loss, Escape, popup/modal state or explicit unavailability cancels preview and suppresses held input. Register retains that prepared geometry instead of comparing it with the post-panel available rectangle. Cancellation does not release another widget's capture.
- Left animation registers a handle only for the real expanded `show_animated` Some response. Collapse/fake intermediate panels suspend this owner and preserve preference; actual native allocation continues to define the scene viewport. Right portal/unavailable/quarantined paths suspend or disable the owner, and sections share the same preference.
- The new divider has a stable accessible name, vertical Splitter role and orientation. It does not invent keyboard/native resize actions; prior SDK drag-only resizing also supplied no such contract. Existing keyboard navigation/actions are preserved. New native producer hooks are unchanged; named divider responses reuse normal observation.
- The outside-scroll Verse/name and Join/Create rows are separated. Truncation plus full-name hover preserves the long label, and explicit existing Join/Create semantic names/routes remain. Both-axis hierarchy scrolling and the 220-point content floor remain; no row action was removed. Actual high-scale horizontal-scroll-and-invoke remains required.

## Authored coverage and acceptance limits

The existing visibility/layout journey calls the new shared extension and authors native-equivalent SidePanel allocation/public owned Response drags: default/custom restoration across caps, stationary/cap-only movement, constrained inward resize, modal/Escape/focus/scale cancellation, reopen and right outer-budget/no-stroke-drift checks. Its viewport assertion uses the existing exclusion helper; it does not execute live camera/NodeManager input. The close-pending first-frame case above is absent. Native section switches, a new actual Context, terminal/quarantine behavior and full screenshots/actions remain integrated acceptance gates.

After the correction is refrozen and independently reviewed, parent integration/build and actual responsive/gesture screenshots are still required before VIS-003 closure. No normal UIA/WGC helper, capture freshness, full visual-clean or test-pass claim follows from this source review.

No compiler/formatter/Cargo/tests, apps, OS/process inspection, input, API/DB/store access, network requests, Git mutations or production edits were performed. Work was read-only source/SDK/report/hash inspection and this review artifact.
