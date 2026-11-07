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

## Same-pass cosmetic exclusions (QA41 L1)

completed_exclusions_current_ui copies only the existing four-slot completed
response rectangles, after actual finish_pass and only in its exact cumulative
egui pass. It independently checks the existing window/screen/scale/navigation
validity and refuses a pending build. Context getters run after the temp-data
closure releases its lock. The pass stamp is used only by this cosmetic accessor;
it does not enter any existing pointer validity predicate or derived comparison.
Normal Update input still uses the last completed visible geometry exactly as
before. No native authority, new exclusion producer or input exception is added.

## §passive-hud — direct-painted HUD after finish (QA-0024, QA69)

The terrain ruler/unscaled chip is a raw Foreground layer painter, not an Area,
so egui's layer_id_at never reports it and the four-slot collector cannot hold
it (it is placed from that collector's finished rects). QA69 showed a press on
it with Brush or an armed Circle persisted a region. exclude_completed_hud adds
one union slot to the completed snapshot of the SAME pass, only after finish
and never while building; non-finite geometry fails the snapshot closed. The
slot joins pointer/secondary admission via contains() but is never returned by
completed_exclusions_current_ui, so ruler placement input and the native
producer inventory are unchanged. Each begin_pass resets it, so a hidden ruler
frees its scene on the next completed pass.
