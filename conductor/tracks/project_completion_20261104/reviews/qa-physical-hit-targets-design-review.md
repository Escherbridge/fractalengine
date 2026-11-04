---
type: Review
date: 2026-11-04
title: Physical target design review
---

# QA0039 physical targets: independent design and functional source review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.
Verdict: **APPROVED for source integration. No blocking design/readability,
inspected API or functional source defect found in this frozen revision.**
This is source approval, not a compiler/test result or a complete layout pass.

Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`. Read the actual frozen27-file
patch/source/report/receipt, relevant pinned egui/epaint/emath0.33.3 source and
retained QA38 measurements. Actually inspected the retained0.75x and1024/2x PNGs.
Only this MAIN review artifact was written. No production source edit, Git
mutation, compiler/formatter/build/test, application/process/input/capture,
API/DB/store or network action occurred. Root-owned live evidence, main build
and the independently owned pointer/native work were untouched.

## Exact source and evidence anchors

Checkout: `.omc/worktrees/qa-physical-hit-targets`.
Base: `35485826b7008745133918e7ca67f3a278d39a12`.
Patch inside checkout:
`.omc/research/qa-physical-hit-targets-source.patch`,66,450 raw bytes, SHA256
`ae0bef24326de00466462f75a694a62764a985b330a67861f19d9fd945343ca9`.
All27 actual raw source hashes independently match the frozen receipt. All25
Rust-file declaration counts independently match both old/new receipt counts,
78 declarations total; no test declaration was added.

Retained observations remain failures/partial coverage, not new acceptance:

- `qa/passes/38/owner-scale075-layout.jsonl` has101 measured small_hit_target
  failures. Actual examples include19.9921875px hierarchy/help widths and
  18.4921875px ordinary topbar control heights. The screenshot is
  `.qa/run/20261104-104626-392/shots/105236-156-qa38-scale075.png`.
- `owner-size1024-scale200-layout.jsonl` has10 clipping failures, including
  Custom Properties and its form/table content. Its screenshot is
  `.qa/run/20261104-104626-392/shots/105510-425-qa38-1024-scale200.png`.
- `owner-scroll-bottom-confirm-ui.jsonl` records the Add property button at
  y613.5..663.5 inside clip y606..664, outer offset3530.5. The button is disabled
  in that captured form state; its rectangle is nevertheless visibly reachable
  at the bottom. The same packet exposes viewport height128 against clip height58.
  Do not turn the viewport mismatch into a claim that the final action could not
  be reached, or turn visible placement into a claim that an edit was authorized.

## Shared metric and actual native allocations

The central `quantized_target` is an appropriate small shared policy. Pinned
emath publicly exports GUI_ROUNDING=1/32; native round_ui rounds to that lattice.
Rounding max(24,20/scale) UP before allocation produces26.6875logical points at
0.75x, or20.015625physical pixels. The earlier26.6667 request could round down
to26.65625/19.9921875. This corrects real geometry instead of adding measurement
tolerance or a role exemption. At1/1.25/2x the minimum returns to24points.

apply_style recomputes ordinary interact_size.y EVERY pass before the existing
one-time padding return, from the stable baseline rather than an earlier enlarged
style. It therefore handles same-Context scale changes and restoration. Font and
painted-icon sizing remain separate. Finite invalid/unrepresentable math is
rejected internally; the UI adapter's normal24point fallback is documented as
not establishing a valid physical measurement for an invalid context.

The correction follows the actual widget contracts rather than assuming one
style field governs everything. Ordinary buttons/checkboxes/radio controls inherit
the refreshed height. Small buttons have an explicit minimum after small(), short
actions reserve square space, and explicit allocations call control_size to
retain larger requested dimensions while imposing the shared minimum. The helper
reserves layout space; it does not inflate hit rectangles over adjacent widgets.

Single-line TextEdit uses its own desired/minimum outer-size calculation in the
pinned implementation, independently of interact_size.y. The text_input adapter
sets both axes and returns the same native builder, allowing call sites to retain
password, ID/id_salt, char limit, desired width, enabled/pending flags and buffers.
The further minimums at the inspected call sites use the same shared target seam.
Tag input's ui.add(TextEdit) replacement preserves the ordinary native widget
allocation route. No new editor state machine or authority path is introduced.

The local tree/grid/axis/rail/footer/copy formulas delegate to target_size;
object_data and editor_batch retain their existing adapter ownership. The actual
small_button and singleline producer inventory under fe-ui now reaches the
shared constructors. This source inventory is not a certificate for unvisited
custom controls, native SDK scrollbars or producers in other crates.

## Header adapter: bounded view, native identity and response

SectionHeader is a useful narrow Adapter rather than a new collapse controller.
Pinned CollapsingHeader explicitly asks for Extend, so changing only a parent
wrap style could not fix this X inflation. The adapter prepares a Truncate
galley using the real available title width less the existing native indent and
padding, then hands that galley to the ORIGINAL native CollapsingHeader.

Verified pinned WidgetText::Galley preserves the supplied galley instead of
laying it out again with Extend. Its text accessor uses the galley's full job
text. CollapsingHeader's native ID salt is Id::new(text.text()), and its native
accessible WidgetInfo uses that same full galley text. Consequently visual
truncation does not substitute an abbreviated title as identity or accessible
name. Full-title hover is added to the original returned header response.

All changed call sites require only default_open/show, which the adapter exposes
with matching native CollapsingResponse semantics. Existing body closures,
open defaults and widget/selection state remain at their original owners.
There is no parallel collapse cache or fake native response. The retained
full-title-ID assertion is meaningful authored coverage, not proof it ran.

## Constrained Inspector and scroll budgets

The outer Inspector still has pinned navigation followed by ONE vertical body
scroll. visible_scroll_height computes the intersection of actual remaining
available geometry and current parent clip, clamped at zero. Setting both the
native maximum and min_scrolled_height=min(64,visible budget) prevents the SDK's
default64point minimum from making a larger scroll viewport under a shorter
visible clip. Pinned native APIs accept these f32 budgets. This does not apply
viewport clipping as a size cap to every nested editor when it scrolls offscreen.

The change addresses known horizontal sources locally: native titles truncate,
schema key/type controls stack and type captions truncate, and annotation color
controls wrap with a bounded input. The custom property table retains real
horizontal scrolling; there is no deletion of overflow actions or blanket
disabled-X checker waiver. Existing v4 content gutters/clips, width preferences
and selection authority are unchanged.

The authored compact-shell extension renders real section_chrome/right_inspector
with populated properties, actual navigation/footer/side geometry and recorded
outer scroll observations at1024physical width across the four requested scales.
It checks actual viewport bottom versus clip, disabled-X content extent, retained
selection and fixture values. This meaningfully exercises the changed seam.
It does not perform a bottom-wheel or horizontal action invocation, and it is
not execution evidence. Root must repeat those real reachability checks.

## Results resize slots and neighboring controls

Results keeps each existing column width as the layout budget. Its header now
subtracts the ACTUAL shared handle width from the sort button, then allocates
that handle slot beside it. The line still paints at one point. This is sound
space ownership: no expanded resize response overlays the original sort button
or the row below. At the minimum60point column and0.75x, the26.6875point handle
leaves33.3125points for sorting, above the same physical target minimum. The
supported1/1.25/2x cases leave36points. Larger columns retain their budget.

Column width range60..600, ordinary drag handling, sorting/row IDs and clipboard
paths are unchanged. The populated Results extension inspects actual complete
draggable header targets, requires both physical dimensions>=20 and checks
non-overlap with clickable sort/cell allocations. Existing side/top/scrollbar
clearance and offset-drag assertions remain. Partially clipped columns still
need actual scrolling before invocation; this patch does not certify native
scrollbar target sizes or exempt incomplete targets.

## Readability and scope assessment

The shared metric and three small native adapters express the relevant interaction
contracts without a generic custom-widget framework. Returning the native TextEdit
builder and native CollapsingResponse keeps editing/extension familiar. Mechanical
delegates are narrow; permission checks, drafts, pending mutations, dispatch and
selected-group ownership remain at existing call sites. Browser cards subtract
the actual Options width, avoiding a new neighbor overlap when0.75x requires a
larger target. No readability refactor is required for this bounded correction.

Directory docs carry rationale and distinguish the source-authored checks from
execution. The historical tag_panel CRLF/encoding is preserved rather than
rewriting unrelated content. No camera/NodeManager/topbar input routing, native
A1/A2 implementation, ruler producer or independently owned pointer patch was
changed. The ruler's whole-window anchor remains an explicit separate terrain/
viewport integration task; this source approval supplies no ruler acceptance.

## Required execution and live acceptance

- Compile the exactly integrated source and execute the relevant integrated
  sweep; all78 declarations and new helper assertions are unexecuted here.
- Repeat the actual0.75x checker/UI/PNG collection and inspect real rounded,
  clipped controls. Confirm the previous101 failures are resolved or retain
  each remaining failure explicitly; never infer the new count from this review.
- Exercise1x baseline density,1.25/2x and actual high-DPI combinations, including
  same-Context scale restoration, menus/popups, native headers, short/small
  actions, text input focus/typing, password concealment and pending drafts.
- At the constrained Inspector, repeat bottom scrolling and horizontal table
  action access, inspect the native viewport/clip and disabled-X extents, and
  test real buttons after valid form input. Preserve the accurate older Add
  property placement evidence. Check title hover and retained collapse state.
- Invoke Sort and drag Results column handles at neighbor edges/minimum widths,
  with horizontal scrolling and clipped columns. Keep native scrollbar sizing
  and unvisited custom-control states as separate observed/unknown cases.
- Retain v4 preferred-width, actual selection/camera, divider/cancellation and
  neighboring-control acceptance. Inspect PNGs for new wrapping, obscured actions,
  excessive control density or geometry beyond the clip.
- Preserve failed QA38 packets, ruler deferral and incomplete layout coverage.
  This exact source approval does not close QA0039 or certify full application
  accessibility, native automation, CI/release or project completion.
