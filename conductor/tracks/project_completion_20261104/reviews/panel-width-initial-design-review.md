---
type: Review
date: 2026-11-04
title: Initial preferred width design source review
source_artifact_sha256: e842e1bf94e0519ae3fc612ede72cb66620c0621a82bc65a857f701f2e693673
---

# VIS-003 panel width preference: independent design/source review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.

Frozen source: the five dirty files in
`.omc/worktrees/qa-panel-width-preference`, baseline `2bce88af`, as recorded in
that checkout's `qa-panel-width-preference-source-receipt.json`.

**CHANGES REQUIRED for one narrow cancellation gap.** The shared value object,
public response adapter and the three principal sizing requirements are sound at
source level. This verdict does not report compilation, passing authored journeys,
actual resizing or VIS-003 closure.

## [State Machine Eligibility] — severity: medium

**Location:** `fe-ui/src/panels/sidebar.rs:36-38`,
`fe-ui/src/ui_shell/right_sidebar.rs:314-315`,
`fe-ui/src/ui_shell/layout_policy.rs:109-167`,
`fe-ui/src/plugin.rs:1002`.

**Current:** The right panel disables its owned resizer when
`TopbarState.close_pending` is true. The left panel only checks the UiManager
dialog enum and portal state. The common adapter additionally checks egui's
already-established top modal layer, but the close confirmation is rendered by
`render_close_confirmation.after(gardener_ui_system)`. An OS close request can
set close_pending before this UI pass while the previous egui modal memory is
still empty. A held left-divider drag released in this pass can therefore commit
its preview before the newly pending close modal is rendered.

This is a local UI cancellation-contract gap, not an authorization or document
mutation bypass. It contradicts the planned/source-documented close-loss
cancellation and leaves the two sides with different eligibility semantics.

**Suggestion:** Propagate the actual current close-pending state to left-panel
resize admission, or supply one small shared shell interaction-eligibility value
to both callers. Disable/suspend before `prepare`, without resetting the stored
preference or releasing another widget's input. Do not rely solely on last-pass
modal memory or broaden this into a new global input manager.

**Example (schematic):**

```rust
let resize_allowed = normal_dialog_state
    && !portal_open
    && !topbar.close_pending;
left.width.set_interactive(resize_allowed);
```

Extend the existing meaningful shell/resize journey with an actual pending-close
transition while a left preview is owned, before a modal layer exists, then a
release. The preference must remain unchanged and the held input must not revive
the retired gesture. Mirror right-side coverage without declaring a new test.

**Impact:** Both sidebars share current shell eligibility and preserve the
promised cancellation behavior at the real scheduling boundary.

## Positively reviewed design and production paths

- `PanelWidthState` is held by the existing left/right area resources; no new
  domain manager, disk settings or speculative docking framework was added.
  Left defaults220, right defaults320 logical points. Preferred width is private;
  automatic budgets flow through `effective` and cannot overwrite it.
- The owned public `ctx.read_response(id)` / `ui.interact` adapter uses explicit
  app IDs, not egui's private `__resize` ID. Pinned Context source documents
  reading current interactions before the widget is re-registered from its
  previous rectangle. No frame.response.dragged inference is used.
- A drag starts from the previous actual displayed outer panel rectangle and
  original pointer origin. A preferred220/constrained160 left panel therefore
  responds to an inward10-point movement as150. The response strip's width is
  never confused with the panel width.
- Preference remains unchanged during preview. Primary release commits only a
  changed visible result beyond half a physical pixel. Stationary press and
  cap-only motion preserve the prior unconstrained preference.
- Actual Context identity, viewport, pre-panel available rectangle, scale,
  finite bounds and pass continuity are captured and checked. A changed geometry
  cancels preview and suppresses held input until release. Registration carries
  the original pre-allocation geometry rather than comparing it with the
  post-SidePanel remaining rectangle.
- Escape, focus loss, current popup/modal state and caller availability cancel
  the owned gesture without changing preference or clearing another widget's
  capture. The close-pending exception above is the outstanding gap.
- The right frame's layout-cost stroke is removed. Exact width uses the fill
  and existing native separator, avoiding the pinned SidePanel content-minimum
  versus Frame-stroke arithmetic that produced the earlier four-point drift.
  There is no observed-width feedback correction.
- Left `show_animated` creates a handle only for the actual expanded Some
  response. Collapsed/fake animation suspends the transient owner without
  overwriting preference. Right unavailability, portal and quarantine paths
  explicitly suspend/disable their owner. Right sections share one preference.
- The public handle stays on normal Background UI with egui hit testing and
  disabled state. Its strip uses the actual panel edge/vertical extent and the
  current style resize radius. The existing camera/NodeManager interaction-rect
  functions and strict endpoint semantics are unchanged. Splitter role,
  orientation and stable names use existing accessibility observation hooks;
  no new keyboard/native resize action or A1 authority route is advertised.
- The outside-scroll hierarchy header now gives the truncated Verse name/full
  hover its own row and Join/Create a separate row. Original action names,
  dialog construction and authority routes remain. Both-axis tree scrolling
  and the220-point content floor remain; actual offscreen row-action
  reachability is still a live gate.

The existing journey authors meaningful public Response drags, default/custom
restoration, stationary/cap-only behavior, actual constrained baseline,
modal/Escape/focus/scale cancellation, reopen, outer budget and viewport exclusion
checks. These are authored source checks, not execution. Production section
switches, replacement Context, terminal/quarantine and new close-pending coverage
remain integrated acceptance requirements.

## Verification and limits

All five raw working-file SHA256 values match the frozen receipt. The changed
Rust test declaration counts remain1/0/3/13, as recorded. `git diff --check`
returned0. Targeted source inspection found the existing changed-signature caller
passes mutable LeftSidebarState and visible struct literals use Default updates.
No compiler readiness is inferred from those reads.

Read the author report, all production additions, directory rationale, existing
design approval and shared UI guidance. Inspected relevant pinned egui
SidePanel/Frame/Response/Context APIs, shell close scheduling, normal modal
handling, camera and selection boundary sources. No production edit, compiler,
formatter, tests, application/process operation, API/DB request, network request
or Git mutation was performed. This review artifact is the only file authored.
