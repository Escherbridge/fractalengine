---
type: Review
date: 2026-11-04
title: Preferred panel width and public divider design review
source_artifact_sha256: c0b683ac7bbd859a619870cbc9dd98976636b64aa561459ac1afd5d29846dd64
---

# VIS-003 panel width ownership: independent design review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.

**APPROVED as an implementation plan, subject to the concrete conditions below.**
The preferred/effective width split and an application-owned public divider
Response are the smallest reliable shared seam I found in the pinned API. This
is design approval, not source approval, a measured fix, or runtime acceptance.
No production implementation was edited or executed by this review.

## Reviewed evidence and API

Read `.omc/research/qa-ui-scale-panel-width-investigation.md`, the actual left
sidebar (`fe-ui/src/panels/sidebar.rs`), shell left/right managers and
`layout_policy.rs`, shell/plugin pointer routing, and pinned egui0.33.3
`containers/panel.rs`, `containers/frame.rs` and `response.rs`. Applied
`C:/Users/atooz/.gemini/UI-UX-guidance.md`.

Actually inspected the three retained PNGs in
`.qa/run/20261104-073857-380/shots/`:

- `073948-593-qa35-scale125-1024.png`;
- `073950-241-qa35-scale200-1024.png`;
- `074045-290-qa35-options-card.png`.

The first shows usable220/320 logical panel widths. The high-scale cut has
constrained panels, offscreen tree row actions and a cramped Verse header. The
restored normal-size cut visibly retains narrower panels. This supports the
session-memory defect; it does not prove disk persistence or horizontal action
reachability. A both-axis hierarchy ScrollArea does not cover the Verse header.

The existing protected viewport budget is a useful pure policy and should stay.
The shell's existing left-before-right render order is also appropriate: the
right effective budget sees the actual left allocation. No new layout manager,
global settings model, dependency or general docking system is warranted.

## [Value Object + Adapter] — severity: medium

**Location:** `fe-ui/src/panels/sidebar.rs:33`,
`fe-ui/src/ui_shell/right_sidebar.rs:1707`,
`fe-ui/src/ui_shell/layout_policy.rs:101`.

**Current:** Dynamic viewport-preserving caps feed the same egui PanelState that
also remembers user resizing. Pinned `panel.rs:251-257` loads that actual width;
`panel.rs:345` stores every final rectangle, including automatically constrained
frames. Once constrained, `default_width` is no longer a recovery preference.

**Suggestion:** Add a small shared `PanelWidthState` value object, held by the
existing left/right area manager resources. Keep its preferred width private;
only an owned resize or an explicit layout reset changes it. Derive effective
geometry through the existing finite budget functions. Defaults remain220/320
logical points. Put the small SidePanel/handle adapter in the layout-policy
module or one focused sibling if that module would become hard to scan.

**Example (design sketch):**

```rust
struct PanelWidthState {
    preferred: f32,
    drag: Option<PanelResize>,
}

impl PanelWidthState {
    fn effective(&self, min: f32, max: f32) -> f32 {
        self.preferred.clamp(min, max)
    }
    // Preference mutation belongs to the owned resize transition.
}

let effective = state.width.effective(min_width, available_budget);
let panel = egui::SidePanel::right("right_section")
    .resizable(false)
    .exact_width(effective)
    .frame(frame)
    .show(ctx, body);
// Shared adapter registers its own named divider, using the actual panel rect.
```

The example assumes a frame with no extra layout-cost stroke; see the next
section. Sanitize finite bounds and preserve min<=max before calling clamp.
Do not copy the code sketch into production without that preflight.

**Impact:** Two panels share one geometry/intent contract. Automatic layout can
no longer overwrite user preference, and the implementation remains editable
without introducing another domain manager or duplicating sizing arithmetic.

### Public API alternative assessment

`SidePanel::show` returns its Frame's InnerResponse, not its internal resize
Response (`panel.rs:345-376`). The returned `.response.dragged()` is therefore
not evidence of a user resize. The SDK's actual resize ID is private
`id.with("__resize")`; reconstructing it is a brittle implementation dependency.

Public `PanelState::load`, its `rect` field and typed Context storage do not
solve intent attribution: both automatic and user widths converge into that
same rectangle. Deleting PanelState or resetting it every frame would discard
legitimate user resizing. I found no simpler sound public SidePanel API that
exposes its resize intent. An app-owned drag Response is justified here.

## [State Machine] — severity: medium

**Location:** Proposed shared divider adapter; existing rendering at
`sidebar.rs:33-75`, `right_sidebar.rs:1707-1752` and
`ui_shell/left_sidebar.rs:69-101`.

**Current:** The proposed replacement must distinguish an actual resize from a
hover, ordinary press, automatic cap change, animation or coordinate-system
change. Pinned egui `Response::dragged` becomes true immediately on press for a
drag-only Sense (`response.rs:367-386`); it is not proof of movement.

**Suggestion:** Keep a small explicit drag state that captures the original
preference, actual displayed outer panel width, pointer origin and relevant
context/geometry at drag start. Read only the public app-owned Response. Use
the actual panel rectangle supplied by the renderer, never the resize
Response's strip width as the panel width.

**Example (design sketch):**

```rust
enum ResizeTransition {
    Idle,
    Begin { displayed_width: f32, original_preference: f32 },
    Move { signed_delta: f32 },
    Commit,
    Cancel,
}
```

The concrete state may use Option<PanelResize>; an additional enum hierarchy is
not required. Its transition rules must include:

1. Start from the **effective displayed width**, not the unconstrained
   preference. A panel preferred at220 but constrained to160 must respond to
   a10-point inward drag as150, rather than silently remaining at the cap.
2. Preserve the original preference for press/release with no displacement.
   Do not save a constrained width merely because `.dragged()` is true.
3. Clamp the requested visible result to current valid bounds. If the result
   never changes the displayed divider, retain the original preference rather
   than replacing it with the cap. Make rounding tolerance physical-pixel aware.
4. Define Escape/focus loss/collapse/modal or terminal loss as cancellation of
   this owned gesture, without changing selection or another widget's capture.
5. On scale, viewport/anchor/budget or actual egui Context change during a drag,
   conservatively cancel and restore the captured preference, then require a new
   press. Do not apply deltas from mixed point scales or silently resume an old
   drag; suppress the old held Response until release if necessary.
6. Keep a live preview responsive. A prepare/register split using the public
   `ctx.read_response` on the **app-owned** ID can mirror the SDK's current-pass
   approach if needed. Do not read the private SDK ID or add a second input
   authority alongside the Response.

**Impact:** The fix preserves real resize intent and makes cancellation and
scale changes testable. It avoids heuristics based on mutable panel rectangles.

## [Geometry Value + Existing Boundary Reuse] — severity: medium

**Location:** `fe-ui/src/ui_shell/topbar.rs:207-229`,
`fe-ui/src/plugin.rs:1224-1234`,
`fe-renderer/src/camera.rs:523-538`, and right Frame construction at
`right_sidebar.rs:1712-1718`.

**Current:** Camera and NodeManager routing already exclude actual panel grab
bands using the style's resize radius, with strict edge behavior covered by the
existing resize journey. The right panel also has a two-point Frame stroke.

**Suggestion:** Reuse the actual divider edge, vertical extent and
`style.interaction.resize_grab_radius_side` for the new Response. Keep it on the
same normal background UI layer with public egui hit testing, rendered after
panel contents like the SDK. A foreground modal must win. Do not use a
foreground always-on-top Area, global pointer polling or an enlarged invisible
region that steals adjacent content. Give the handle stable app-owned identity
and a semantic name/appropriate splitter role through existing accessibility
helpers where supported.

Preserve `viewport_interaction_rect` and the strict camera/selection boundary
contract. Only change its geometry if the owned handle's actual band changes,
and update the same existing endpoint journey together. A divider press must
never also pick/place/deselect or orbit the camera.

**Frame condition:** Pinned SidePanel sets its content minimum to
`width_range.min - frame.inner_margin.sum().x` (`panel.rs:296`), while Frame's
outer allocation also includes stroke/outer margins (`frame.rs:341-365`).
Therefore `.exact_width(effective)` plus the current right stroke can still
produce an outer rectangle larger than the intended budget. Do not compensate
by repeatedly storing observed widths or adding a feedback `+4` adjustment.
The simplest option is a zero-layout-cost side separator/background using the
existing SidePanel separator drawing; otherwise author and verify a precise
outer-width adapter against the pinned implementation. Assert actual allocated
outer geometry stays within the budget and does not drift each frame.

**Animation condition:** Keep `show_animated` and the user's sticky visibility
intent. Its public return is None for collapsed and fake in-between panels
(`panel.rs:414-440`); register a handle only for the actual expanded Some
response. Neither hiding, reopening nor animation may change the preference.
Use actual returned geometry for routing, not a hypothetical full-width panel.

**Impact:** Existing scene interaction guarantees remain intact, while width
calculation has one explicit outer-geometry convention.

## Required implementation and acceptance scope

Use the existing area resources and the narrow shared adapter. Keep width
preferences session-scoped like the currently observed state; do not add disk
settings or treat the observed narrow state as a recovered historical user
preference. Fresh state uses existing defaults; only an explicit layout reset
resets both preference and transient drag. Navigation, selection, opening right
sections, automatic collapse and responsive caps do not reset them.

Extend an existing meaningful shell/resize journey without adding test
declarations, then execute through the parent's final sweep. Required cases:

- Defaults220/320 -> high-scale/narrow automatic clamp -> original size/scale
  restores220/320 within pixel-rounding tolerance.
- Real left and right custom widths survive that same cycle and section
  switches; a stationary press preserves the preference.
- Resize begun while constrained uses actual displayed width and remains
  responsive; hitting the cap without a changed divider does not destroy the
  earlier preference.
- Scale/window changes, cancellation, collapse/reopen and modal overlap do not
  apply stale drag deltas or steal another control's input.
- Exact inner/outer grab-band endpoints preserve selection and camera state;
  ordinary interior viewport actions still work.
- The right outer frame stays in budget without a four-point drift, and the
  existing protected viewport share stays intact.
- Actual screenshots at the captured sizes/scales show restoration; real
  horizontal scrolling reveals and invokes a tree row action at2x. Check the
  Verse/Join header separately because it is outside the tree ScrollArea.

No compiler, formatter, tests, app/process operation, API/DB request, network
request, Git mutation or production-source edit occurred. Work consisted of
read-only source/SDK/report/image inspection and this design artifact. The
unimplemented proposal remains subject to independent source review, build and
live evidence before VIS-003 closure.
