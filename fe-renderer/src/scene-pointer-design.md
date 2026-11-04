# Shared scene pointer geometry

ScenePointerChrome is a fixed-size egui-context publication of actual clipped
viewport chrome responses. It supplements CameraPointerRegion's existing strict
scene rectangle, physical-pixel conversion and floating-layer rejection. It does
not shrink the visual viewport, alter camera framing or own selection/navigation.
The only navigation binding is NavigationManager's existing intent revision.

The normal router observes the current window entity, physical dimensions and
native scale plus navigation revision before pointer claims. Camera input follows
that narrow ScenePointerContextSet and independently checks its actual primary
window against the same observation. The existing camera capture-lock run_if,
divider geometry and consumer priority order remain. CameraPointerRegion retains
its public geometry-only contract for standalone renderer clients, with or without
a configured region. The editor explicitly installs EditorScenePointerRequired
and uses accepts_editor_pointer / accepts_editor_window_pointer for its scene
router and cursor. Opted-in camera routing requires a region, egui context and
matching completed chrome; absent or invalid witnesses consume mouse input and
refuse admission. Standalone camera clients retain the prior missing-context
fallback. No editor failure falls back to geometry-only admission.

Build and completed snapshots are separate. UI begin keeps the last completed
visible geometry; only a valid complete nonempty pass publishes new geometry.
Readers use the completed snapshot with the current physical point, rather than
the preceding egui active-widget pointer observation. This matches the UI actually
visible when a normal physical press arrives, even when the UI pass is later in
PostUpdate. No synthetic-input origin, cursor pre-move or QA-only exception exists.

Window/context/viewport/screen/scale/scene-rectangle/navigation mismatches invalidate
completion until a fresh matching paint. Changes cannot revive older geometry by
returning to its previous dimensions or scope. An unfinished previous pass, absent
snapshot, invalid rectangle, empty publication or overflow also fails closed.
There are four slots; current producers use at most two in a Petal or browser.
No per-frame Vec/String allocation or unbounded widget inventory is introduced.
Context temp-state reads copy fixed-size state and release the data closure before
calling any Context geometry or input API, including the secondary-click gate.

The UI publishes the navigation header, actual browser allocation and noninteractive
hint Area response. Floating areas keep their existing layer priority. The viewport
context-menu producer requires matching completed geometry plus current painted
chrome and floating layers. World-cursor previews use the same window gate after
the shared context observation, so chrome does not show a world placement cursor. Passive
world-space track labels remain scene content; they are not interactive UI chrome.

This gate changes admission/ray availability over chrome, not any tool's captured
gesture state machine. A gesture begun in uncovered scene retains the existing
hold/release/cancel behavior when its ray becomes unavailable at UI boundaries.
No click-through is authorized merely because the egui active-widget state is old.
Root's final source review, integrated checks and real input/camera acceptance
remain required; authored journeys are not executed evidence.
