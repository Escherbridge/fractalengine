---
type: ImplementationReport
status: source_frozen_changes_requested
---

# QA41 residual layout source handoff

Status: the four approved residual corrections are source-authored in the
isolated qa/residual-layout checkout based on
d8da3a46e9861e9191845ea283e37fb7e5ea3472. This packet is frozen for independent
review, not approval, build evidence or live acceptance. The main checkout and
the native helper C++ packet were untouched. No compiler, parser, formatter,
tests, application, OS/native/input, API, DB or network execution occurred;
no Git mutations occurred. Read-only source/Git inspection and raw artifact
copy/hash generation were used. git diff --check reported no whitespace errors.

Applied the MAIN qa41-residual-layout-design-review and shared
C:/Users/atooz/.gemini/UI-UX-guidance.md. Ownership was split before authoring:
this author owns the six UI source files and four UI directory doc changes;
the ruler_layout child owns the six terrain/host files. Other work was preserved.
The adjacent manifest identifies all sixteen actual source/doc files by raw
SHA-256 and byte count. The patch is the full sixteen-file diff from the stated
base; the raw/ tree retains every frozen file without newline conversion.

## Tree row allocation

tree_entry.rs now computes one complete width after actual local indentation:
icon + name + eye + Options and exactly three two-point gaps. It reserves that
explicit child before native controls are allocated. The flexible name Button
also has the shared square target minimum. Frame margins are outside that
calculation. Existing both-axis Hierarchy scrolling owns overflow instead of
allowing a clamped name to push a neighbor out of an undersized row.

No competing whole-row click target was added. Explicit entry IDs, original
Button/menu responses, full accessible labels/hover, selected/primary/partial
styling, tiny painted glyphs, role/scope checks and navigation callbacks remain.
The production sidebar's existing gutter and scroll clip remain in force.

## Dividers and routing

DividerMetrics takes the required half-target plus one physical pixel per side,
then rounds upward on public GUI_ROUNDING. Side strip endpoints round outward
on the physical grid using f64 intermediates. The final finite clipped actual
span must still meet twenty physical pixels; unsupported geometry suspends the
existing owner rather than accepting a threshold epsilon.

Shared clearance conservatively reserves two physical pixels beyond the radius
for endpoint conversion/rounding; the interior gutter reserves another physical
pixel beyond clearance. Representability checks reject collapsed margins as
well as invalid/oversized geometry. Native Results retains the shared radius
through its existing restoring scope. Camera exclusion uses the corresponding
clearance; overlay content and panel interiors use the shared gutter. Existing
source/scale/context invalidation, held-input cancellation and release ownership
are preserved. Preferred visual panel widths and camera projection are unchanged.

## Compact native navigation

layout_policy::compact_navigation uses local visible width/height, normal chrome
cost and a body target of max(three target rows, 96 points). Narrow Inspector
shows a native Section button menu, a short selected-object/deselect row and a
native Tab button menu. Ordinary spacious rails/tabs keep their prior behavior.
Very short compact headers have their own bounded native vertical scroll fallback;
the main Inspector scroll minimum is still bounded to real remaining height.

Both tab presentations call activate_inspector_tab, which preserves the existing
set_active_tab draft/secret lifecycle, API page reset, scope population and query
on activation. Failed token-list channel sends now log instead of being swallowed.
Both section presentations use the existing state.toggle operation. Selected
options and accessible current labels are explicit, native popups retain their
surface marker, and every menu option keeps the shared target minimum.

Pending batch controls now scroll with the main body rather than consuming fixed
header space. The existing right_section_scroll_Inspector identity is unchanged.
No new selection/draft authority, role exception, direct action, privacy grant
or alternate mutation path was introduced. Deselect still calls NodeManager.

## Actual ruler producer and camera calibration

ViewportRect is preserved. Separate ViewportGeometry stamps actual egui Context,
cumulative pass, pixels-per-point and final scene rectangle. It is cleared before
the gardener's context-access early return and published after shell geometry is
assigned. current rejects stale/invalid geometry; overlay_current applies the
same divider-safe inset used by existing overlay contents.

The actual same-pass schedule is GardenerUiSet::GeometryReady -> host HUD Bridge
-> terrain Draw. The host clears/publishes terrain-owned RulerHudInput with that
validated anchor; stale/missing context, pass, scale or geometry omits the HUD.
The actual numeric painter and unscaled chip fit their complete text mesh/layout,
shadow/stroke/padding bounds inside the inset and clip to the actual scene.
Insufficient room omits painting. There is no whole-window/cached fallback.

Metric math requires the real active perspective orbit Camera owning the unique
primary egui context. Camera::physical_viewport_size().y calibrates meters per
physical pixel. The desired 120-point bar is converted using actual current egui
pixels-per-point, snapped to a metric length and converted back for painting.
Scene width can bound placement without becoming the camera projection-height
denominator. Effective terrain world scale and the existing down-looking
perspective approximation remain; unsupported projections omit the metric bar.
Camera crop and world grid behavior are unchanged.

The native direct-painter inventory now names draw_scale_bar -> paint_scale_bar
and the bounded fixed chip. This remains separately source-audited fixed/numeric
Foreground ruler_scale_bar admission, independent of Areas enumeration. No ninth
mandatory producer, registration after surface finish, dynamic capture grant or
Owner-before-provider ordering change was added. Edited path-label refusal remains.

## Existing journey extensions and verification limits

No new test declaration was added. Existing registered journeys now author:

- Real production tree Button dimensions through every hierarchy depth, both-axis
  overflow, actual horizontal wheel movement and deepest Options clip reachability
  at 0.75/1/1.25/2 scales; existing tiny glyph/selection/AX checks are retained,
  with a name-versus-eye/Options neighbor allocation assertion.
- Actual side-strip physical span checks without the old threshold epsilon and
  actual strip endpoints outside camera geometry in existing scale/restoration,
  modal/resize/Results and populated-scroll profiles.
- Actual narrow native Section/Tab menu pointer activation, several real body
  rows at scale 2, current named controls, main scroll-content identity, retained
  property/query drafts and selection, API page reset and secret cleanup. Existing
  spacious navigation, multiple-selection and role/privacy cases remain.
- Actual production ruler calibration/painter calls in real egui passes across
  0.75/1/1.25/2 and return to 1, full output-shape bounds/clip, numeric/fixed states,
  stale context/pass refusal, insufficient room and unchanged projection math
  when only the sidebar anchor width changes.

These are unexecuted source assertions. The compact section-menu journey checks
the actual selector callback/state, not every section renderer; full dispatch,
pending edits, both scroll ends, empty/plural selection, disabled roles and native
keyboard operation still require the parent's integrated/live matrix. Actual
divider endpoint/neighbor clicks, high-scale hscroll/invoke, ruler scene bounds
after panel/Results/window/scale/navigation changes, actual Camera/ppp calibration
and native/Owner gates remain acceptance work. No capture, numerical volume,
oblique ground-distance, release or goal-completion claim follows from this packet.

## Primary local SDK anchors

- egui 0.33.3 ui.rs:1554-1580: allocate_ui_with_layout reserves an explicit child
  rect and accounts for its actual min_rect in the parent.
- egui 0.33.3 containers/menu.rs:321-336: MenuButton uses the actual native Button
  and Popup::menu response; no synthetic interaction is introduced.
- egui 0.33.3 id.rs:83-84: native AccessKit IDs retain the real egui Id value;
  existing journeys locate actual named responses rather than guessing geometry.
- bevy_camera 0.18.1 camera.rs:458: physical_viewport_size reflects a custom
  physical viewport or the full physical render target.
- bevy_egui 0.39.1 lib.rs:703,1448,1916: unique primary context/camera association
  and full egui primary pass execution before output completion.
- epaint 0.33.3 shapes/text_shape.rs:65: text visual bounds use galley mesh bounds,
  which the actual ruler fit calculation includes.

The delegated ruler-only raw manifest/patch/report remain separately preserved
under qa41-ruler-layout-source* in this checkout's research directory. The full
combined packet supersedes those only as the integration scope, not as evidence
that execution occurred.

A separate read-only critique by the ruler author inspected the six UI source
changes against pinned native APIs and found no concrete compile/API or behavioral
blocker. That lane made no edits and did not approve the full feature. The root's
two independent full reviews remain required before integration/execution.
