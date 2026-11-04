---
type: Review
date: 2026-11-04
title: Physical target functional review
---

# QA0039 physical targets: independent functional source review

Reviewer: `/root/brush_ingress_trace`, 2026-11-04.

**APPROVED for source integration. No blocking functional, inspected API/type,
identity or edit-binding defect found in the frozen revision.** This approves
the inspected source contribution, not compilation, journey execution, actual
clipped target counts, complete accessibility or final QA0039 closure.

Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Inspected the actual27-file
patch, receipt, report, changed source and relevant pinned egui/epaint/emath0.33.3
implementations. The prior design review was read as context; the identity,
allocation, width-budget and native API conclusions below were independently
checked. Only this review artifact was written. No production source edit, Git
operation, build/compiler/formatter/test, app/process/OS/input/capture, API/DB/
store or network operation was performed.

## Exact scope

Checkout `.omc/worktrees/qa-physical-hit-targets`, declared baseline
`35485826b7008745133918e7ca67f3a278d39a12`.
Patch `.omc/research/qa-physical-hit-targets-source.patch` inside that checkout:
66,450 bytes, SHA256
`ae0bef24326de00466462f75a694a62764a985b330a67861f19d9fd945343ca9`.

Independently computed all27 actual raw file hashes and matched each receipt
entry. All25 current Rust-file test declaration counts match the receipt,
78 total; no test attribute is added or removed by the inspected patch. The
historical tag_panel CRLF is retained rather than normalized wholesale.
The existing authored journeys and their new helper assertions were read but
not executed. No inference about their passing is made.

## Central minimum and scale changes

`layout_policy::quantized_target` rejects nonpositive/nonfinite scale and
unrepresentable logical target calculations. It rounds
`max(24,20/pixels_per_point)` upward on the public `egui::emath::GUI_ROUNDING`
lattice, before native UI allocation. The pinned quantum is1/32point. At0.75x
the requested logical minimum is26.6875, giving20.015625 physical pixels;
the old raw reciprocal could round to26.65625/19.9921875. At1/1.25/2x it returns
24points and keeps normal control density. The application QA scale validator
still admits0.75..2.0; no supported scale or checker rule is removed.

`apply_style` updates ordinary interaction height before the existing one-time
style return on every pass. It calculates from the stable24point baseline,
not the previously enlarged context style, so0.75-to1x restoration has no
size ratchet. Fonts and painted glyphs are unchanged. Existing context-level
divider metrics and per-pass Results-edge reset remain in their original order.

The separate constructors follow actual native contracts: ordinary Button
inherits interaction height, native `small()` bypasses that default and now has
an explicit two-axis minimum, and explicit allocations use `control_size` so
larger requested dimensions survive. Single-line TextEdit receives a two-axis
minimum because its native size contract does not inherit ordinary button height.
The invalid-context fallback is finite24points, not a claim that invalid scale
has a certified physical measurement.

This is a minimum **allocation** policy. It cannot make an arbitrarily clipped
or offscreen fragment20pixels wide/high. Partially visible controls, scrollbars,
other-crate producers and unvisited custom widgets retain actual live acceptance
requirements; the source does not justify waiving those measurements.

## Native identities, buffers and form behavior

The TextEdit helper returns the original native builder and introduces no child
Ui, allocation or auxiliary state. The small-button helper uses the same native
`Button::new(...).small()` path as pinned `Ui::small_button`, with a larger
minimum. `Ui::text_edit_singleline` likewise delegates directly to native
TextEdit; replacing that convenience method retains the ordinary allocation
route. Existing explicit IDs/id_salt, password masking, char limits, pending/
enabled conditions, buffers, changed/lost-focus handling and action dispatch
remain at their inspected call sites. No permission, history, group-target or
token ownership path is replaced by these adapters.

SectionHeader prepares a truncated galley without allocating a widget, then
uses the native CollapsingHeader and returns its native CollapsingResponse.
Pinned `Galley::text()` returns the **full non-elided input job text**
(epaint text_layout_types.rs:792-793), and `WidgetText::Galley` preserves it.
Pinned CollapsingHeader derives its salt from `text.text()` (:411-418), uses
the same persistent header ID/state (:536 onward), and supplies `galley.text()`
to native WidgetInfo (:576). Thus a truncated visible title does not become a
different collapse ID or accessible name. Full-title hover changes presentation,
not the collapse controller or response's action semantics.

The title width subtracts the same native indent and trailing button padding
that native header geometry adds. This addresses the actual Extend behavior
instead of depending on an ignored parent wrap style. At extremely insufficient
parent widths the ordinary native minimum can still exceed available space;
the realistic constrained shell remains a required live check rather than an
universal zero-overflow claim.

The schema form's horizontal-to-vertical change retains the same widget order
and native scope route. Pinned horizontal layout ultimately calls `scope_dyn`
with a default UiBuilder, as does vertical; layout direction is not an added
ID salt. Its named key input, type ComboBox ID and state buffers are retained.
Responsive wrapping changes coordinates, so operators must observe fresh bounds
instead of replaying old geometry or claiming old held gestures survive a resize.

## Inspector budgets and reachability

`visible_scroll_height` intersects the actual remaining parent rectangle with
the current clip and clamps the resulting height at zero. Applying that value
to maximum height and `min(64,visible)` to the native minimum removes the
specific default64point viewport inflation under the smaller parent clip.
The outer Inspector remains one vertical scroll below pinned navigation; the
helper is not imposed on each offscreen nested property editor.

Native section-title truncation, stacked schema controls, wrapped bounded
annotation color input and existing responsive axis rows address local X
inflation sources. Native TextEdit minima use the same target after any further
call-site `min_size`. The property table preserves real horizontal scrolling
and Edit/Copy/Delete/confirmation controls. No overflow action or authorization
gate was deleted to make a size check pass.

The retained Add property action's recorded bottom position is not an
unreachability proof; the source report correctly distinguishes its visible
rectangle from the older viewport-versus-clip mismatch. The new compact shell
journey meaningfully renders actual Inspector/navigation, populated properties
and observed native scroll geometry at1024physical width across the four scales.
It does not execute a bottom wheel gesture or valid form submission. Root must
verify those behaviors and actual horizontal action access after integration.

## Results handles versus sorting

Each existing column width remains the total slot budget. Its sort header now
subtracts the actual shared handle allocation; the handle occupies that reserved
space beside it, with item spacing set to zero inside the column. The line is
still painted one point wide. No inflated hit rectangle is overlaid on the sort
button or row below.

At the existing60point column minimum and smallest supported0.75x, a26.6875point
handle leaves33.3125points for sorting, above the same requested minimum. At
1/1.25/2x the24point slot leaves36points. Native row height is at least the
refreshed interaction height. Sorting, row IDs, selected-row behavior, clipboard
logic and the existing60..600 resize clamp remain unchanged.

The extension to the populated Results journey checks actual complete draggable
targets and non-overlap with clickable sort/cell targets, retaining the earlier
side/top/scrollbar clearance and offset-drag assertions. That is meaningful
authored coverage, not a runtime result. Horizontal scroll can partially clip
columns, and native scrollbar targets retain separate live checks.

## Required execution and integration limits

- Independently verify exact main integration, especially any concurrent
  pointer/native changes. This review inspects the isolated frozen contribution.
- Compile the integrated source and run the required final checks at the user's
  agreed sweep point; all78 declarations/new helper assertions remain unexecuted.
- Repeat actual0.75/1/1.25/2x UI/layout/PNG capture in one context, including
  scale restoration, narrow/short Inspector, menus/popups and native headers.
  Record real clipped targets and remaining failures; do not infer a new failure
  count from these constructor changes.
- Type into ordinary and password fields, retain drafts/pending behavior, invoke
  valid form actions, check native collapse state/full-title hover and repeat
  Inspector bottom/horizontal scrolling with actual action access.
- Invoke Sort and resize neighboring Results columns at minimum width and after
  horizontal scrolling. Verify actual edge ownership and native scrollbar state.
- Preserve v4 panel preference/cancellation, selected identity/transform and
  camera behavior, and inspect actual screenshots for new crowding or clipping.
- Keep the separately owned ruler/viewport issue and unvisited producers open.
  No complete UI/accessibility, native automation, QA0039 closure, CI/release or
  project-completion credit is granted here.
