---
type: Review
status: source_approved
---

# Residual layout v3: independent host and interaction boundary review

Reviewer: root, 2026-11-04. Verdict: APPROVED for exact source integration,
subject to the separate full functional review and matching execution gates.
Root did not author this isolated source slice. This is a later review pass,
separate from its implementation and from the earlier design discussion.

## Exact scope and preservation

Reviewed the complete production changes and host/interaction dependencies in
the nineteen-file packet at `.omc/worktrees/qa-residual-layout`, based on
`d8da3a46e9861e9191845ea283e37fb7e5ea3472`. The complete manifest SHA256 is
`62c34c5ac10fa0faf536877e766d023034b6fe889e807077e9756889179db59c`.
Full patch SHA256:
`611890ec0a3ab56f37556e6b7295b1ce93b32b49f24bd2614a7bbbbeec799d52`.

All nineteen raw snapshot sizes/hashes match both their manifest and current
isolated files. Seventeen previously reviewed v2 files are byte-identical.
The added production change is the actual vision writer ordering; its existing
journey and host directory guide also change. V3 preserves the bounded chrome,
tree, divider and compact-navigation source accepted within the earlier review
limits. The separate functional reviews cover the full UI and authored journeys.

Main's nineteen corresponding paths still match the d8 baseline after ordinary
CRLF/LF normalization. Thus main's later Owner-observation ordering correction
and documentation-only commits need no source merge or replacement. These
pre-integration anchors are recorded in
`qa41-residual-layout-v3-root-anchors.json`; repeat the check before copying.

## Interaction, state and privacy boundaries

Tree width is reserved before allocating the original icon/name/eye/Options
responses. The name has an actual target minimum; horizontal scrolling owns
constrained depth. No competing row target, selection source or navigation
callback is introduced. Complete labels and stable explicit IDs remain.

Divider strip endpoints round outward and use conservative matching clearance
and gutters. This changes production geometry rather than relaxing the measured
twenty-pixel threshold. Existing ownership, cancellation and strict editor
pointer admission remain the relevant behavioral gates.

Compact Inspector controls reuse actual native menus and the same tab/section
activation functions. Tab activation preserves draft/secret lifecycle and API
page reset. Pending controls move into the existing body scroll; NodeManager,
permissions, primary selection and the body scroll identity retain ownership.
Popup publication remains explicitly unsupported for the positive native
profile; no new credential or capture permission follows from compact rendering.

## Actual geometry and pointer preservation

The cosmetic accessor copies only the existing four-slot completed chrome,
after real finish_pass. It checks the current UI pass and existing context,
window, scale, screen and navigation validity. Context getters occur after the
temp-data closure releases its lock. The new pass field is outside the existing
ScenePointerStamp equality and every physical-input validity predicate.
Prior-completed-frame Update input behavior is preserved.

The plugin clears geometry before its context early return and captures chrome
only after completion. The host bridge requires current geometry and chrome.
Terrain bounds the copied rectangles and either places the complete text/stroke
footprint above actual intersections or omits it. There is no guessed hint,
whole-window fallback, arbitrary Area search or native authority in this seam.

## Camera calibration and actual writer ordering

The ruler uses the active primary egui orbit camera's physical viewport height,
with explicit egui-point/physical-pixel conversion. The central scene rectangle
controls placement, not the projection-height denominator. Effective terrain
scale and the existing perspective approximation remain explicit limitations.

The actual host installer orders outer PostUpdate EndPass after camera updates
and transform propagation. V3 additionally orders the existing production
apply_vision_pose/advance_vision_job chain before CameraUpdateSystems, retaining
within-chain order and the propagation edge. Both initial raw FOV writes and
snapshot restoration occur inside that chain. This closes the legal stale order
in which the ruler previously read new raw FOV while extraction used an older
Camera::clip_from_view cache.

The source dependency is now vision apply/advance -> camera update -> transform
propagation -> EndPass, with the existing camera/UI layout edges retained.
ProcessOutput -> Owner observation -> native handoff and annotation still follow
EndPass. No inspected return edge makes this cyclic. Runtime schedule
materialization remains mandatory; this source review does not execute the DAG.

The real vision journey now observes first applied and first restored FOV/cache/
global-pose state using the actual installer and SDK CameraPlugin. It retains
the existing screenshot-delivery fixture. Those assertions are authored, not
executed, and do not establish GPU/native capture freshness.

## Remaining acceptance

No compiler, parser, formatter, test, application, native API, HTTP, DB, network
or Git mutation was performed in this review. Only source/artifact reads, hashes
and this report were used. Integrated compilation, real schedule construction,
Circle/Brush chrome regression, ordinary scene positive controls, camera/resize/
focus restoration, Results, menus/modals, tree horizontal scrolling and native
navigation remain live gates. Inspect actual screenshots at the required sizes
and scales. A clean layout checker alone is insufficient visual acceptance.

Standalone terrain integrators intentionally need a same-pass geometry publisher
and matching outer ordering. Native capture remains unproved and must refuse.
The audit is not acceptance of every overlay, projection, role, feature family,
FULL pass, final sweep, CI workflow, release artifact or the overall goal.
