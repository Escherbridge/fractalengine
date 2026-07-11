# Specification: Viewport Foundation — 3D Camera, Infinite Ground Plane, and Bevy Scene Setup

**Track ID:** `viewport_foundation_20260402`
**Type:** Feature
**Created:** 2026-04-02
**Status:** Draft

---

## Overview

Replace the current Camera2d placeholder with a full 3D viewport foundation: an orbit camera controller, an infinite-look ground grid, a dark environment/sky, an origin axis gizmo, and proper egui/Bevy input separation. This track delivers the spatial canvas on which all subsequent 3D features (model placement, selection, gizmos, drag-and-drop) will operate.

## Background

The application currently spawns a `Camera2d` in `fractalengine/src/main.rs::setup_camera` and renders egui panels over a blank background. The `CentralPanel` in `fe-ui/src/panels.rs` uses `Frame::NONE`, which already allows the Bevy render to show through. However, there is no 3D scene, no camera controller, and no spatial reference. Every subsequent Wave 2 track (Petal Seed, Bloom Stage, Petal Portal, selection, gizmos) depends on having a working 3D viewport with camera controls.

---

## Functional Requirements

### FR-1: Orbit Camera Controller

**Description:** A Bevy plugin that provides Blender-style orbit camera controls. The camera targets the world origin by default and supports orbit, pan, and zoom via mouse input.

**Acceptance Criteria:**
- AC-1.1: Middle-mouse-button drag orbits the camera around the focus point (azimuth + elevation).
- AC-1.2: Scroll wheel zooms toward/away from the focus point (distance changes, not FOV).
- AC-1.3: Shift + middle-mouse-button drag pans the camera (translates focus point in the camera's local XY plane).
- AC-1.4: Camera starts at a default position looking at the origin (e.g., position `(5, 4, 5)` looking at `(0, 0, 0)`).
- AC-1.5: Orbit is clamped so the camera cannot flip past the poles (elevation clamped to approximately -89 to +89 degrees).
- AC-1.6: Zoom distance is clamped to a minimum (0.5 units) and maximum (500 units) to prevent clipping or losing the scene.
- AC-1.7: Camera motion is smooth (no snapping or jitter at normal frame rates).

**Priority:** P0 (Critical)

### FR-2: Infinite Ground Plane with Grid

**Description:** A visible ground plane at Y=0 that provides spatial reference. Renders as a grid with major and minor lines, extending to the horizon.

**Acceptance Criteria:**
- AC-2.1: A grid is visible at Y=0 with minor gridlines every 1 unit and major gridlines every 10 units.
- AC-2.2: Grid fades out with distance (alpha attenuation) so it appears infinite without a hard edge.
- AC-2.3: Grid is rendered as a full-screen quad with a fragment shader (not geometry lines) for performance and clean appearance.
- AC-2.4: Grid colors match the dark UI theme: subtle gray lines on a dark background.
- AC-2.5: Grid does not z-fight with objects placed at Y=0.

**Priority:** P0 (Critical)

### FR-3: Sky / Environment Background

**Description:** A dark background environment that matches the UI theme and provides depth cues.

**Acceptance Criteria:**
- AC-3.1: The 3D viewport background is a dark solid color or subtle vertical gradient, matching the egui dark theme palette.
- AC-3.2: The background is set via Bevy's `ClearColor` resource or camera background configuration.
- AC-3.3: Ambient light is set so that objects placed in the scene are visible without requiring explicit point/directional lights (a default directional light is acceptable).

**Priority:** P1 (High)

### FR-4: Viewport Integration (egui Pass-Through)

**Description:** The 3D Bevy scene renders behind the egui overlay. The egui CentralPanel is transparent and passes through to the 3D viewport.

**Acceptance Criteria:**
- AC-4.1: The Bevy 3D scene is visible through the egui CentralPanel area.
- AC-4.2: Side panels, toolbar, and status bar render opaquely on top of the 3D scene with their themed backgrounds.
- AC-4.3: The `Camera2d` in `main.rs` is replaced with the new 3D camera — no 2D camera remains unless specifically needed for egui (bevy_egui manages its own).

**Priority:** P0 (Critical)

### FR-5: Input Separation (egui vs. Camera)

**Description:** Mouse and keyboard input is correctly routed: egui consumes events when the cursor is over UI panels; the camera controller only processes input when the cursor is in the viewport area.

**Acceptance Criteria:**
- AC-5.1: When the mouse is over any egui panel (sidebar, toolbar, inspector, status bar), camera orbit/pan/zoom does not activate.
- AC-5.2: When the mouse is over the transparent CentralPanel (viewport area), camera controls work normally.
- AC-5.3: Input gating uses `bevy_egui`'s `EguiContext::wants_pointer_input()` (or equivalent Bevy 0.18 API) -- not manual hit-testing.

**Priority:** P0 (Critical)

### FR-6: Origin Axis Indicator (XYZ Gizmo)

**Description:** A small, fixed-position axis indicator in the bottom-left corner of the viewport showing the current camera orientation relative to world XYZ axes. Similar to Blender's navigation gizmo.

**Acceptance Criteria:**
- AC-6.1: Three colored lines/arrows are drawn: X (red), Y (green), Z (blue).
- AC-6.2: The gizmo rotates to reflect the current camera orientation (it shows which way the world axes point from the camera's perspective).
- AC-6.3: The gizmo is rendered at a fixed screen position (bottom-left corner) and fixed pixel size — it does not move with the camera in world space.
- AC-6.4: The gizmo does not interfere with scene picking or camera controls.

**Priority:** P1 (High)

---

## Non-Functional Requirements

### NFR-1: Performance

- The orbit camera system must not add more than 0.1ms per frame at 60 FPS.
- The ground grid shader must render in a single draw call.
- The axis gizmo must render in at most 1 additional draw call (or use Bevy Gizmos API).

### NFR-2: Code Organization

- Camera controller lives in a new module: `fe-renderer/src/camera.rs` with a `CameraControllerPlugin`.
- Ground grid lives in: `fe-renderer/src/grid.rs` with a `GridPlugin`.
- Axis gizmo lives in: `fe-renderer/src/axis_gizmo.rs` with an `AxisGizmoPlugin`.
- All plugins are composed into a `ViewportPlugin` in `fe-renderer/src/viewport.rs`.

### NFR-3: Testability

- Camera controller math (orbit, pan, zoom) is implemented as pure functions that take current state + input deltas and return new state. These are unit-testable without a Bevy World.
- The Bevy system wrappers call these pure functions and apply results to `Transform`.

### NFR-4: Thread Compliance

- All rendering and camera logic runs on T1 (Bevy ECS main thread). No async, no `block_on()`, no spawned threads.

---

## User Stories

### US-1: First Launch Viewport

**As** a Node operator launching FractalEngine for the first time,
**I want** to see a 3D viewport with a ground grid and dark background,
**So that** I have a spatial reference and know the application is a 3D environment.

**Scenarios:**
- **Given** the application starts, **When** the window appears, **Then** a dark 3D viewport is visible behind the egui panels with a grid at ground level.
- **Given** no models are loaded, **When** I look at the viewport, **Then** I see the grid, background, and an axis gizmo in the corner.

### US-2: Camera Navigation

**As** a Node operator viewing a Petal,
**I want** to orbit, pan, and zoom the camera using mouse controls,
**So that** I can navigate the 3D space to inspect objects from any angle.

**Scenarios:**
- **Given** the viewport is visible, **When** I hold middle-mouse and drag, **Then** the camera orbits around the focus point.
- **Given** the viewport is visible, **When** I scroll the mouse wheel, **Then** the camera zooms in/out.
- **Given** the viewport is visible, **When** I hold Shift+middle-mouse and drag, **Then** the camera pans laterally.
- **Given** I am hovering over the sidebar, **When** I scroll the mouse wheel, **Then** the camera does NOT zoom (egui consumes the input).

### US-3: Orientation Awareness

**As** a Node operator navigating the 3D space,
**I want** to see an XYZ axis indicator in the viewport corner,
**So that** I always know my camera's orientation relative to world axes.

**Scenarios:**
- **Given** the camera is at the default position, **When** I look at the axis gizmo, **Then** X points right, Y points up, Z points toward me (approximately).
- **Given** I orbit the camera 90 degrees to the right, **When** I look at the axis gizmo, **Then** the axes have rotated correspondingly.

---

## Technical Considerations

### Camera Architecture

The `OrbitCameraController` component stores: `focus: Vec3`, `distance: f32`, `yaw: f32`, `pitch: f32`. A Bevy system reads mouse input events (`MouseMotion`, `MouseWheel`, `ButtonInput<MouseButton>`), checks `EguiContext::wants_pointer_input()`, computes new orbit parameters via pure functions, and writes the resulting `Transform` to the camera entity.

### Ground Grid Approach

Two viable approaches:
1. **Custom shader material** — a `Material` impl with a fragment shader that computes grid lines analytically from world-space XZ coordinates. Single full-screen-ish quad, infinite appearance, built-in fade. This is the preferred approach.
2. **Bevy Gizmos** — `gizmos.grid()` API if available in Bevy 0.18. Simpler but may lack fade and may have draw-call overhead.

The implementer should evaluate Bevy 0.18's `InfiniteGrid` or `Gizmos::grid` availability. If neither exists natively, use approach (1) with a custom `ExtendedMaterial` or raw `Material` implementation.

### Axis Gizmo Approach

Use Bevy's `Gizmos` API to draw three short colored lines each frame, projected to a fixed screen-space position. Alternatively, use a second camera with a fixed-size viewport in the corner rendering only the gizmo entities (overlay camera approach). The overlay camera approach is more robust and Blender-like.

### Integration Points

- `fractalengine/src/main.rs`: Replace `Camera2d` spawn with `ViewportPlugin` registration.
- `fe-runtime/src/app.rs`: No changes needed (plugin added at binary level).
- `fe-ui/src/panels.rs`: CentralPanel already uses `Frame::NONE` — verify it is fully transparent. May need `fill(Color32::TRANSPARENT)` explicitly.
- `fe-renderer/src/lib.rs`: Add `pub mod camera; pub mod grid; pub mod axis_gizmo; pub mod viewport;`

### Bevy 0.18 Specifics

- `Camera3dBundle` is replaced by `Camera3d` component in Bevy 0.18 (bundle removal migration).
- Use `Camera { clear_color: ClearColorConfig::Custom(...), .. }` or the `ClearColor` resource.
- `MouseMotion` events and `ButtonInput<MouseButton>` are the standard input APIs.

---

## Out of Scope

- Object selection / raycasting (Selection System track)
- Transform gizmos / manipulators (Transform Gizmos track)
- Model spawning or drag-and-drop (Petal Seed track)
- Scene graph hierarchy (Scene Graph Bridge track)
- Lighting beyond basic ambient + one directional light
- Camera animation / smooth transitions / focus-on-object
- VR or gamepad camera controls
- Camera bookmarks or saved viewpoints

---

## Open Questions

1. **Bevy 0.18 InfiniteGrid:** Does Bevy 0.18 ship a built-in infinite grid, or should we use `bevy_infinite_grid` crate? Tech stack says no external camera crate, but does that extend to grid crates? -- **Resolution needed before Phase 2.**
2. **Gizmo overlay camera:** Bevy 0.18 supports render layers and multiple cameras. Confirm the API for a fixed-viewport overlay camera that only sees specific render layers. -- **Investigate in Phase 3.**
3. **egui wants_pointer_input:** Confirm the exact API name in bevy_egui 0.39 for checking if egui wants to consume pointer events. Likely `EguiContext::get().wants_pointer_input()` but must verify. -- **Investigate in Phase 1.**
