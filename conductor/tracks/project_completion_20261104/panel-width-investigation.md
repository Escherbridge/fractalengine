---
type: Investigation
date: 2026-11-04
title: Session preferred panel width loss at changed scale
---

# VIS-003: panel width preference lost during automatic scale/window clamp

Investigator: /root/native_witness_corrections, 2026-11-04.
Read-only inspection of main `9cdd3132`, retained source/SDK and the specified
actual PNG/state/layout artifacts. No source edit, compiler/tests, app/process,
API/DB, network or input operation was performed. Shared UI guidance applied.

## Finding

The narrow-time clamp is expected to preserve the viewport. Persisting its actual
result as the next unconstrained preferred width is a separate UX defect: the
panels remain narrowed after returning to the original scale/window. The app has
no distinct preferred-width owner; it delegates both preference and effective
geometry to egui's persistent PanelState under the stable IDs sidebar and
right_section. This is actual session-memory preference loss, not evidence of
settings-file or cross-restart persistence.

## Exact source chain

- `fe-ui/src/panels/sidebar.rs:33-36`: resizable SidePanel::left("sidebar"),
  default_width220 and width_range120..layout_policy::left_width(ctx).
- `fe-ui/src/ui_shell/right_sidebar.rs:1707-1712`: resizable
  SidePanel::right("right_section"), default_width320, dynamic
  width_range(max_w.min260..max_w).
- `layout_policy.rs:101-130`: protected_width is max45% of point width and
  360 physical pixels divided by pixels_per_point. Left dynamic cap subtracts
  that protected width plus120 points reserved for the right; right cap uses the
  remaining available width after the left panel.
- `panels/mod.rs:198-217` renders left before right, so the right cap responds to
  the actual left width.
- Pinned egui0.33.3 `containers/panel.rs:251-257` loads PanelState.rect.width
  whenever it exists, then clamps to width_range and available width.
  `panel.rs:345` stores the final actual Frame response rect on EVERY shown
  frame, not only an explicit drag. `PanelState::store` at47-48 inserts it into
  the Context persisted typed data. A later default_width is only a fallback
  when this ID has no state; it is not a recovery preference.
- Native user dragging goes through a private resize Response at266-274/323.
  Its latest geometry and automatically clamped geometry converge into the same
  stored rect; the app currently has no separate intent record.

## Actual cuts and arithmetic

The inspected PNGs show the expected temporary viewport protection AND subsequent
persistent narrowing. Corresponding state is in qa/passes/35.

| Retained cut | Physical window / scale | Left physical / logical | Right physical / logical |
| --- | --- | --- | --- |
| owner-scale125-state | 1280x720 /1.25 |275 /220 |400 /320 |
| owner-scale125-1024-state |1024x720 /1.25 |275 /220 |288.21875 /230.575 |
| owner-scale200-1024-state |1024x720 /2 |323.1875 /161.59375 |240 /120 |
| owner-options-card-state after scale1 and1280 restore |1280x720 /1 |161.59375 /161.59375 |264 /264 |

Widths are derived from physical viewport.x and
window.width - viewport.x - viewport.w, not guessed from text extents.
At1024/2 the screen is512 points; protected width is230.4; the left cap is
512 -230.4 -120 =161.6 points. The observed161.59375 is consistent with egui
pixel/UI rounding. When room returns, that stored161.6 already lies within the
larger allowed range and never selects the default220 again.

The right panel similarly saves its120-point constrained state. On return it
hits the newly feasible260-point minimum, rather than recovering320. Its observed
264 outer points are consistent with the frame's two-point stroke on each side:
SidePanel's content minimum subtracts only inner_margin, while Frame's outer
rect includes stroke widths (`panel.rs:298`; `frame.rs:341-365,460-468`). This is
an explanatory source correspondence, not a separate measured inner-rect trace.

`owner-scale200-1024-layout` reports viewport share0.4500122 and the125% compact
cut reports0.4499817, consistent with the intended45% floor within rounding.
Both report coverage_complete=false. Zero listed violations does not establish
all responsive controls usable.

## Screenshot and horizontal-scroll observations

Inspected these retained originals:

- shots/073948-593-qa35-scale125-1024.png: card subtitle is now a readable single
  line; left and right panels are still readable at this cut.
- shots/073950-241-qa35-scale200-1024.png: tree row eye/options controls are beyond
  the left panel's visible right edge; labels and Inspector are highly compact.
- shots/074045-290-qa35-options-card.png: after normal scale/1280 restoration,
  left panel remains about162px, right about264px; the Verse header is cramped
  against Join and the tree is visibly much narrower than the initial cut.

The Hierarchy ScrollArea is explicitly both-axis and uses a220-point minimum
content width (`sidebar.rs:47-59`), so horizontal overflow is intentional in this
narrow state. At2x the layout packet classifies19 Hierarchy controls as
intentional_scroll_content. Examples: row eyes x332..380 and options x384..432,
while the panel ends at323.1875. This confirms offscreen current controls; it
DOES NOT prove the controls can be revealed and invoked. A real horizontal
scroll plus semantic locator/menu action and restoration is still required.
The Verse header sits outside that ScrollArea; horizontal scrolling does not
explain away its cramped text/Join layout. Its own responsive behavior needs a
separate visual/action check. No speculative control-removal fix is proposed.

## Minimal shared seam proposal (not implemented)

Keep user preferred width separate from the current effective width, in logical
points under a small shared layout-policy owner keyed by the app's explicit left
and right panel identities. Defaults remain220/320. Effective width is the finite
preferred width clamped to the current available budget. Automatic constraints
change only effective geometry; reopening/widening restores the preference.

Do not reconstruct the SDK's private __resize ID, reverse-search widget IDs, or
reset arbitrary Context layout. Public PanelState::load/rect are available;
store is private (typed Context data insertion is public), but that alone cannot
tell why an actual width changed. Also, SidePanel::show returns the Frame's
InnerResponse, NOT its internal resize Response (`panel.rs:345-376`); checking
returned response.dragged() would silently fail to preserve actual user resizing.

The smallest reliable ownership seam therefore needs an explicit app-owned
resize Response rather than inferring user intent from automatic width changes:

1. Let the shared helper own preferred logical width and a real resize handle
   with an app-owned stable ID and accessible label. Disable the SDK resizer for
   these two panels, retain native SidePanel layout/rendering and explicit IDs.
2. Apply the preferred/effective width through public SidePanel sizing APIs,
   so stale SDK PanelState cannot become the preference. Handle the right frame's
   total-margin/minimum interaction explicitly; do not accumulate outer strokes
   into the stored preference.
3. Use the same actual divider strip and style.resize_grab_radius_side as the
   current camera/NodeManager pointer exclusions. Only an actual owned drag
   updates preference; hover, ordinary click, clamp and animation do not.
4. Preserve existing selected-set/resize-strip arbitration and foreground/modal
   refusal. Do not capture a divider drag through an overlapping dialog or turn
   a divider press into a viewport deselect. A scale/window change during a drag
   must retire/rebase that owned gesture explicitly, not apply mixed coordinates.
5. Retain left open/close animation and user intent; create a handle only for
   the actual expanded panel response, not the SDK's temporary animated panel.
6. Extend an existing shell/resize journey:220/320 -> small/high-scale clamp ->
   normal restores220/320, explicit resize survives the same cycle, ordinary
   clicks do not change preference, camera/selection stay unchanged, drag
   cancellation/scale change is honest. Parent owns final execution/screenshots.

This modest shared owned-handle seam is preferable to a heuristic that treats
any PanelState change as a user drag. It does add control ownership, so review
and an isolated implementation are required before replacing the current
resizer. No production change has been made by this investigation.
