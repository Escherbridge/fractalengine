# Implementation Plan: Viewport Foundation — 3D Camera, Infinite Ground Plane, and Bevy Scene Setup

**Track ID:** `viewport_foundation_20260402`
**Spec:** [spec.md](./spec.md)
**Estimated effort:** 3-4 sessions

---

## Overview

Four phases: (1) 3D camera with orbit controls and input separation, (2) ground grid plane, (3) environment and axis gizmo, (4) integration and polish. Each phase ends with a verification checkpoint. All camera math is implemented as pure functions for TDD.

---

## Phase 1: Orbit Camera Controller and 3D Scene Bootstrap

**Goal:** Replace Camera2d with a 3D orbit camera. Implement Blender-style orbit/pan/zoom with egui input gating.

**Relevant files:** `fe-renderer/src/camera.rs` (new), `fe-renderer/src/lib.rs`, `fractalengine/src/main.rs`

Tasks:

- [ ] Task 1.1: Create `OrbitCameraController` component and pure math functions
  - Define `OrbitCameraController` struct: `focus: Vec3`, `distance: f32`, `yaw: f32`, `pitch: f32`, with sensible defaults (distance=8, yaw=45deg, pitch=30deg, focus=origin).
  - Implement `compute_transform(focus, distance, yaw, pitch) -> Transform` as a pure function.
  - Implement `apply_orbit(yaw, pitch, delta_x, delta_y, sensitivity) -> (yaw, pitch)` with pitch clamping (-89..+89 deg).
  - Implement `apply_zoom(distance, scroll_delta, min, max) -> distance`.
  - Implement `apply_pan(focus, camera_transform, delta_x, delta_y, pan_speed) -> Vec3`.
  - TDD: Write tests for `compute_transform` (correct position and look-at), `apply_orbit` (clamping at poles), `apply_zoom` (clamping at min/max), `apply_pan` (lateral translation in camera plane).

- [ ] Task 1.2: Create `CameraControllerPlugin` with Bevy systems
  - Create `fe-renderer/src/camera.rs` module with `CameraControllerPlugin`.
  - Add startup system: spawn `Camera3d` entity with `OrbitCameraController` component, `Transform` computed from defaults, and `Projection::Perspective` with 45-degree FOV.
  - Add update system: read `MouseMotion`, `MouseWheel`, `ButtonInput<MouseButton>`, `ButtonInput<KeyCode>` (for Shift detection). Check `bevy_egui` context for `wants_pointer_input()` — if true, skip camera input. Apply orbit/pan/zoom via the pure functions from Task 1.1. Write resulting `Transform`.
  - TDD: Write system test that verifies camera entity is spawned with correct initial components. Write test that verifies input gating logic (mock egui wants_pointer = true => no transform change).

- [ ] Task 1.3: Wire plugin into application and remove Camera2d
  - Add `pub mod camera;` to `fe-renderer/src/lib.rs`.
  - In `fractalengine/src/main.rs`: remove `setup_camera` system and `Camera2d` spawn. Add `fe_renderer::camera::CameraControllerPlugin` to the app.
  - TDD: Verify app builds and runs. Write a test that the old `Camera2d` system is gone (no 2D camera entity in a test world with the new plugin).

- [ ] Task 1.4: Verify egui input separation with bevy_egui 0.39
  - Investigate and confirm the correct API for `wants_pointer_input` in bevy_egui 0.39 / Bevy 0.18.
  - If the API differs from expected, adapt the gating logic.
  - TDD: Write a focused test: when egui reports pointer wanted, camera state must not change.
  - Document the resolved API in a code comment.

- [ ] Verification: Launch application, confirm 3D perspective camera renders, orbit with middle-mouse, zoom with scroll, pan with Shift+middle-mouse, camera does not move when hovering egui panels. [checkpoint marker]

---

## Phase 2: Infinite Ground Grid

**Goal:** Render a procedural grid at Y=0 that fades with distance, providing spatial reference.

**Relevant files:** `fe-renderer/src/grid.rs` (new), `fe-renderer/src/lib.rs`

Tasks:

- [ ] Task 2.1: Research Bevy 0.18 grid rendering options
  - Check if Bevy 0.18 provides a built-in `InfiniteGrid` or `Gizmos::grid_3d()` with fade support.
  - If a suitable built-in exists, use it. Otherwise, plan a custom `Material` implementation.
  - Document findings as a code comment in `grid.rs`.
  - No TDD for pure research — output is a decision document / code comment.

- [ ] Task 2.2: Implement `GridPlugin` with ground grid rendering
  - If using custom material: create `GridMaterial` implementing `Material` trait with a WGSL fragment shader. The shader computes grid lines from world-space XZ, with minor lines (1 unit) and major lines (10 units), alpha fade based on distance from camera.
  - If using Bevy built-in: configure the grid with appropriate colors, spacing, and fade.
  - Spawn a large quad mesh (or fullscreen plane) at Y=0 with the grid material.
  - Color scheme: minor lines `rgba(255,255,255,0.05)`, major lines `rgba(255,255,255,0.15)`.
  - TDD: Write tests for grid entity spawning (correct transform at Y=0, correct material attached). If custom shader, test material uniform values. Shader correctness is verified visually in Phase 2 verification.

- [ ] Task 2.3: Grid z-fighting mitigation
  - Apply a small polygon offset or render the grid slightly below Y=0 (e.g., Y=-0.001) so objects at Y=0 do not z-fight.
  - If using custom shader, add depth bias in the material.
  - TDD: Write test asserting grid entity Y position is at or slightly below 0.

- [ ] Verification: Launch application, confirm grid visible at ground level with minor/major lines, grid fades toward horizon, no z-fighting with a test cube at Y=0, grid colors match dark theme. [checkpoint marker]

---

## Phase 3: Environment and Axis Gizmo

**Goal:** Set up dark background, basic lighting, and a corner axis orientation indicator.

**Relevant files:** `fe-renderer/src/axis_gizmo.rs` (new), `fe-renderer/src/viewport.rs` (new), `fe-renderer/src/lib.rs`

Tasks:

- [ ] Task 3.1: Set dark background and default lighting
  - Insert `ClearColor` resource with a dark color matching the UI theme (e.g., `Color::srgb(0.08, 0.08, 0.10)` or similar to `theme::BG_PANEL`).
  - Spawn a `DirectionalLight` pointing downward at an angle (e.g., rotation from `(-45deg, 30deg, 0)`), with moderate intensity and soft shadows disabled (performance).
  - Add ambient light via `AmbientLight` resource with low intensity so shadow areas are not fully black.
  - TDD: Write test asserting `ClearColor` resource is inserted with expected value. Write test asserting directional light entity exists with expected direction. Write test asserting ambient light resource exists.

- [ ] Task 3.2: Implement axis gizmo with overlay camera
  - Create `fe-renderer/src/axis_gizmo.rs` with `AxisGizmoPlugin`.
  - Strategy: Use a second `Camera3d` with a small fixed `Viewport` (e.g., 100x100 pixels in the bottom-left corner), rendering only entities on a dedicated `RenderLayers` layer.
  - Spawn three short arrow/line meshes (or use Bevy `Gizmos` drawn to overlay camera): X=red, Y=green, Z=blue, centered at local origin.
  - Each frame, rotate the gizmo entity to match the inverse of the main camera's rotation (so it shows world orientation from the camera's perspective).
  - The overlay camera has `clear_color: ClearColorConfig::None` (transparent) and `order: 1` (renders after main camera).
  - TDD: Write test asserting overlay camera entity exists with correct viewport dimensions and render layer. Write test for gizmo rotation math: given a main camera quaternion, the gizmo entities rotate correctly (pure function test).

- [ ] Task 3.3: Ensure gizmo does not interfere with input or picking
  - The overlay camera's viewport is small and positioned in a corner. Verify that mouse events in that corner still route to the main camera for orbit (the gizmo is display-only, not interactive).
  - If Bevy routes pointer events to the overlay camera, add `InputFocus` or camera ordering to prevent interference.
  - TDD: Write test verifying that pointer events in the gizmo viewport region do not block main camera orbit (integration-level; may be manual verification).

- [ ] Verification: Launch application, confirm dark background visible, grid illuminated subtly, directional light casts visual depth on objects, axis gizmo visible in bottom-left corner rotating with camera orbit. [checkpoint marker]

---

## Phase 4: Integration, Polish, and Composition

**Goal:** Compose all viewport components into a single `ViewportPlugin`, verify egui transparency, ensure clean integration with the existing UI, and finalize.

**Relevant files:** `fe-renderer/src/viewport.rs` (new), `fe-renderer/src/lib.rs`, `fractalengine/src/main.rs`, `fe-ui/src/panels.rs`

Tasks:

- [ ] Task 4.1: Create `ViewportPlugin` composing all sub-plugins
  - Create `fe-renderer/src/viewport.rs` with `ViewportPlugin` struct implementing `Plugin`.
  - `ViewportPlugin::build()` adds: `CameraControllerPlugin`, `GridPlugin`, `AxisGizmoPlugin`, and inserts `ClearColor` + `AmbientLight` resources.
  - Update `fractalengine/src/main.rs` to add only `ViewportPlugin` instead of `CameraControllerPlugin` directly.
  - TDD: Write test that `ViewportPlugin` adds all expected sub-plugins (verify resources and entities exist after plugin build).

- [ ] Task 4.2: Verify and ensure egui CentralPanel transparency
  - Confirm that the `CentralPanel` in `fe-ui/src/panels.rs` with `Frame::NONE` is fully transparent in practice (Bevy 3D renders through).
  - If the central panel has any default fill, explicitly set `fill(Color32::TRANSPARENT)`.
  - Verify that the viewport hint text in `viewport_hints()` still renders legibly over the 3D scene (may need a text shadow or semi-transparent background behind the hint).
  - TDD: Write test asserting `CentralPanel` frame has no fill / transparent fill. Visual verification for text legibility.

- [ ] Task 4.3: Add `fe-renderer` dependency to `fractalengine` binary crate if missing
  - Check `fractalengine/Cargo.toml` for `fe-renderer` dependency. Add if absent.
  - Ensure `bevy_egui` is accessible from `fe-renderer` for the `wants_pointer_input` check (add to `fe-renderer/Cargo.toml` if needed).
  - TDD: `cargo build` succeeds with no errors.

- [ ] Task 4.4: Update `fe-renderer/src/lib.rs` module exports
  - Add: `pub mod camera;`, `pub mod grid;`, `pub mod axis_gizmo;`, `pub mod viewport;`.
  - Ensure all public types have `///` doc comments.
  - TDD: `cargo doc --no-deps` succeeds for `fe-renderer` with no warnings.

- [ ] Task 4.5: Final quality gate pass
  - Run `cargo fmt --check` and fix any formatting issues.
  - Run `cargo clippy -- -D warnings` and resolve all warnings.
  - Run `cargo test` and ensure all tests pass.
  - Run `cargo tarpaulin` on `fe-renderer` and verify >80% coverage for new code.
  - Verify no `unwrap()` or `expect()` in production code paths (test code is fine).
  - Verify all public functions have `///` doc comments.

- [ ] Verification: Full integration test — launch application, verify: (1) dark background, (2) ground grid with fade, (3) orbit/pan/zoom camera, (4) input separation from egui, (5) axis gizmo in corner, (6) egui panels render on top with correct theme, (7) viewport hint text is legible. All automated tests pass, clippy clean, fmt clean, coverage >80%. [checkpoint marker]

---

## Dependency Notes

- **No external crates** for camera control (tech-stack constraint). Implement orbit math directly.
- **Grid rendering** may use a Bevy built-in if available in 0.18, otherwise custom shader. Decision made in Task 2.1.
- **bevy_egui 0.39** provides the egui context and input consumption checks. Must be added as a dependency to `fe-renderer`.
- This track has **no upstream dependencies** and **blocks**: Petal Seed (drag-drop needs raycasting camera), Bloom Stage (3D scene rendering), Scene Graph Bridge, Selection System, Transform Gizmos.
