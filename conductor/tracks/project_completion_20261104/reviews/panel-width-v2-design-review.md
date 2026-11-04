---
type: Review
date: 2026-11-04
title: Corrected preferred width design source review
source_artifact_sha256: fef257e86089445fccb69824a3480e2b8b1dc66290005277958e222016258ccb
---

# VIS-003 v2: independent design/source correction review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.

**APPROVED for source integration.** The v2 correction resolves the sole MED
pending-close blocker in `qa-panel-width-preference-design-review.md`. That
initial changes-required report is preserved. No new blocking design/source
finding was identified. This is not compilation, test execution, live resize
acceptance or VIS-003 closure.

## Exact frozen scope

Reviewed the six dirty files in `.omc/worktrees/qa-panel-width-preference`, based
on `2bce88aff512a9998eb120ddbcde8b78b4f7aa84`, against its v2 source report,
receipt and patch. All six raw file SHA256 values match the receipt. The patch is
32606 bytes and has the expected SHA256:

`63cc9180e43bd860a53e2e42944014a01499c06095ebb35d72f8bc46a2b6b90f`

`git diff --check` returned0. Changed Rust test declaration counts remain
1/1/0/3/13 for panels/mod, panels/sidebar, layout_policy, left_sidebar and
right_sidebar, respectively. No new test declaration was introduced.

## Corrected cancellation boundary

The actual current `topbar_state.close_pending` is copied at the production
`gardener_console` call into `render_left_sidebar`. That manager evaluates
`resize_interaction_allowed(ui_mgr, close_pending)` and calls
`state.width.set_interactive` before the sidebar renderer and `width.prepare`.
The predicate preserves the original None/inline-HexonManager dialog and
no-portal checks while adding the missing close-pending gate.

The renderer no longer recomputes a weaker dialog-only eligibility value, so it
cannot overwrite the manager's refusal. This fixes the actual scheduling gap:
`render_close_confirmation` may paint the modal after gardener, but the left
owner already sees the pending-close Resource during its prepare phase.

When close becomes pending on a held drag's release frame, set_interactive(false)
suspends preview before the shared adapter can commit preference. During a
still-held cancellation, wait_for_release remains set; removing pending close
does not revive that owner before release. The right panel's existing pending
close gate, actual close behavior and normal modal priority are unchanged.

The authored existing journey now owns an actual TopbarState and calls the same
production left predicate. It covers a newly pending close while the previous
modal layer is absent, release in that frame, unchanged preference and retired
drag. A second case removes pending close while still held and checks that
suppression survives until release. These are source assertions, not passing
runtime evidence.

## Preserved reviewed design

Independent text comparison against the preserved v1 patch confirms the entire
production PanelWidthState/prepare/register implementation is unchanged; only its
existing journey was extended. The right-sidebar raw hash is also identical to
v1. Remaining changes are the one caller argument, left manager predicate/wiring,
removal of the old renderer override and additive directory documentation.

The positively reviewed properties therefore remain:

- Session-only preferred widths in existing left/right resources, defaults220/320;
  finite automatic budgets change effective geometry without saving preference.
- Actual displayed outer width and pointer origin as drag baseline; stationary
  and cap-only motion preserve the old preference; visible movement commits only
  on primary release beyond half a physical pixel.
- Actual Context, scale, viewport, pre-panel available geometry, bounds and pass
  continuity guard an owned gesture; changes cancel and suppress held input.
- Public app-owned response IDs and normal Background hit testing, with semantic
  Splitter names/orientation and existing accessibility observation hooks. No
  private egui resize-ID lookup, frame.dragged heuristic or new native authority.
- Fully expanded show_animated-only handle registration and preserved visibility
  intent; unavailable/portal/quarantine paths retire transient owners.
- No layout-cost right frame stroke or observed-width feedback, preserving the
  outer budget convention; existing camera/selection exclusion functions and
  strict grab-band boundary semantics remain untouched.
- A separate truncating Verse/name row plus Join/Create row, retaining original
  action names/routes and the both-axis hierarchy scroll/content floor.

The narrow caller addition preserves the existing native surface producer hooks
around the tree and the ordinary A1/A2 source contracts. It adds no input API,
role override, capture path or authority change.

## Remaining acceptance

Parent integration must preserve the exact reviewed contribution, followed by
compilation and actual default/custom width restoration, divider drag/cancellation,
camera/selection endpoint and screenshot/action checks. The required 2x horizontal
scroll-and-invoke and separate Verse/Join header inspection remain live gates.
The existing authored journey and final integrated sweep must actually execute.

This review used read-only source/SDK/report/patch/hash inspection and wrote only
this artifact. No compiler, formatter, tests, application/process operation,
API/DB request, network request, Git mutation or production-source edit occurred.
