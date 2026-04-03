# Implementation Plan: Transform Gizmos — Blender-Style Move/Rotate/Scale Handles

**Track ID:** `transform_gizmos_20260402`
**Spec:** [spec.md](./spec.md)

---

## Overview

This plan is divided into 6 phases:

1. **Core Math & State Machine** — Pure transform math functions and the gizmo interaction state machine (no Bevy rendering yet)
2. **Move Gizmo** — Rendering + interaction for the move (translate) gizmo
3. **Rotate & Scale Gizmos** — Rendering + interaction for rotate and scale gizmos
4. **Keyboard Shortcuts & Modal Transform** — G/R/S hotkeys, axis constraints, Escape to cancel
5. **Snapping, Undo, Inspector Sync** — Grid snapping, undo stack, bidirectional inspector updates
6. **Polish & Integration** — DB persistence, edge cases, final verification

Each task follows TDD: write failing test, implement to pass, refactor.

---

## Phase 1: Core Math & State Machine

**Goal:** Build all pure math utilities and the gizmo state machine as testable units with zero Bevy dependencies.

- [ ] Task 1.1: Create `fe-ui/src/gizmo/mod.rs` module structure with submodules `math`, `state`, `render`, `input`, `undo`
  - TDD: Write test that imports `fe_ui::gizmo::math` and `fe_ui::gizmo::state` — confirm modules exist
  - Implement: Create module files with placeholder public items
  - Refactor: Ensure `fe-ui/src/lib.rs` exports the gizmo module

- [ ] Task 1.2: Implement `ray_axis_closest_point(ray_origin, ray_dir, axis_origin, axis_dir) -> f32` — projects a pick ray onto a world axis and returns the parameter along the axis
  - TDD: Write tests for cardinal axes, oblique angles, parallel ray (degenerate case returns 0.0)
  - Implement: Closest-point-between-two-lines algorithm
  - Refactor: Extract shared vector math if needed

- [ ] Task 1.3: Implement `ray_plane_intersection(ray_origin, ray_dir, plane_point, plane_normal) -> Option<Vec3>` — returns the intersection point of a ray with a plane
  - TDD: Write tests for XY/XZ/YZ planes, ray parallel to plane (returns None), ray hitting from behind
  - Implement: Standard ray-plane intersection formula
  - Refactor: Ensure NaN/infinity edge cases return None

- [ ] Task 1.4: Implement `screen_space_scale(camera_transform, entity_position, base_size) -> f32` — returns a scale multiplier to keep gizmo at constant screen size
  - TDD: Write tests: entity at distance 1 -> scale 1.0, entity at distance 10 -> scale 10.0 (linear), zero distance clamped
  - Implement: `camera_distance * base_size / reference_distance`
  - Refactor: Clamp to min/max to prevent degenerate sizes

- [ ] Task 1.5: Implement `snap_value(value, grid_size) -> f32` — rounds a float to the nearest grid increment
  - TDD: Write tests for snap_value(0.7, 0.5) -> 0.5, snap_value(1.3, 0.5) -> 1.5, negative values, grid_size 0.0 returns value unchanged
  - Implement: `(value / grid_size).round() * grid_size`
  - Refactor: Handle grid_size <= 0 gracefully

- [ ] Task 1.6: Implement `snap_angle(radians, snap_degrees) -> f32` — snaps a rotation angle to the nearest degree increment
  - TDD: Write tests for snap to 15-degree increments, 0 degrees, 360 wrapping
  - Implement: Convert to degrees, snap, convert back
  - Refactor: Normalize angle to 0..2pi range

- [ ] Task 1.7: Define `GizmoInteraction` state machine enum and transition logic
  - States: `Idle`, `Hovering { axis: GizmoAxis, handle: HandleType }`, `Dragging { axis: GizmoAxis, handle: HandleType, origin: Vec3, initial_transform: Transform }`, `ModalActive { mode: Tool, constraint: Option<AxisConstraint> }`
  - TDD: Write tests for all valid transitions: Idle->Hovering, Hovering->Dragging, Dragging->Idle (commit), Dragging->Idle (cancel restores initial_transform), Idle->ModalActive, ModalActive->Idle
  - Implement: `GizmoInteraction` enum + `transition()` method
  - Refactor: Ensure invalid transitions are no-ops or return errors

- [ ] Task 1.8: Define `GizmoAxis` enum (X, Y, Z, XY, XZ, YZ, Free, Screen) and `AxisConstraint` type
  - TDD: Write tests for axis-to-direction conversion, axis-to-plane-normal conversion
  - Implement: Enum with `direction() -> Vec3` and `plane_normal() -> Vec3` methods
  - Refactor: Derive standard traits (Debug, Clone, Copy, PartialEq)

- [ ] Task 1.9: Implement `hit_test_axis_arrow(ray_origin, ray_dir, gizmo_origin, axis, arrow_length, arrow_radius) -> Option<f32>` — returns distance if ray hits the arrow cylinder
  - TDD: Write tests for direct hit on X arrow, miss, edge of cylinder, tip hit
  - Implement: Ray-cylinder intersection with capped length
  - Refactor: Parameterize radius for hover tolerance

- [ ] Task 1.10: Implement `hit_test_ring(ray_origin, ray_dir, gizmo_origin, ring_axis, ring_radius, ring_thickness) -> Option<f32>` — returns distance if ray hits a rotation ring (torus approximation)
  - TDD: Write tests for hit on ring surface, miss inside ring, miss outside
  - Implement: Ray-torus approximation (sample points on circle, test proximity)
  - Refactor: Consider using a flat disc with inner/outer radius for simpler math

- [ ] Verification: Run `cargo test -p fe-ui` — all math and state machine tests pass. Zero Bevy App dependencies in this phase. [checkpoint marker]

---

## Phase 2: Move Gizmo

**Goal:** Render the move gizmo in the viewport and make axis/plane drag functional.

- [ ] Task 2.1: Create `GizmoPlugin` Bevy plugin that registers resources (`GizmoInteraction`, `GizmoConfig`) and adds gizmo systems to the `Update` schedule
  - TDD: Write test that builds a minimal Bevy App with GizmoPlugin and confirms resources exist
  - Implement: Plugin struct with `build()` adding init_resource and systems
  - Refactor: Order systems with explicit system ordering labels

- [ ] Task 2.2: Implement `gizmo_visibility_system` — shows/hides gizmo based on `InspectorState.selected_entity` and `ToolState.active_tool`
  - TDD: Write test: no selection -> GizmoInteraction is Idle, selection + Tool::Move -> gizmo active
  - Implement: System that reads InspectorState and ToolState, updates GizmoInteraction
  - Refactor: Early-return pattern for clarity

- [ ] Task 2.3: Implement `render_move_gizmo_system` — uses Bevy `Gizmos` to draw three axis arrows with cones, plane handles, and center handle
  - TDD: Write integration test that spawns camera + entity, runs one frame with Tool::Move, confirms no panic (rendering tests are visual — correctness tested via math)
  - Implement: System taking `Gizmos`, `GizmoInteraction`, selected entity Transform. Draw arrows as `gizmos.arrow()`, plane squares as `gizmos.rect()`, apply screen-space scale
  - Refactor: Extract color constants to `gizmo::theme` submodule

- [ ] Task 2.4: Implement `gizmo_hover_system` — performs hit testing each frame to detect which handle the cursor is over
  - TDD: Write test with synthetic cursor position and camera: cursor over X arrow -> GizmoInteraction transitions to Hovering(X)
  - Implement: System reads cursor position, camera, computes pick ray via `Camera::viewport_to_world`, runs hit_test_axis_arrow for each axis, updates GizmoInteraction
  - Refactor: Sort hits by distance, take closest

- [ ] Task 2.5: Implement `gizmo_drag_system` — handles mouse-down to start drag, mouse-move to update entity position, mouse-up to commit
  - TDD: Write test simulating drag sequence: mouse_down on X arrow -> drag 2 units along X -> mouse_up. Assert entity X position changed by 2.0, Y and Z unchanged
  - Implement: On mouse_button_just_pressed + Hovering state -> transition to Dragging, save initial transform. Each frame during drag: project mouse ray onto constrained axis/plane, compute delta from drag origin, apply to entity Transform. On mouse_button_just_released -> transition to Idle
  - Refactor: Separate delta computation from application

- [ ] Task 2.6: Implement plane-constrained and free movement in drag system
  - TDD: Write test: drag on XY plane handle -> entity moves in XY, Z unchanged. Drag on center -> entity moves on camera-facing plane
  - Implement: Extend drag system to handle GizmoAxis::XY/XZ/YZ (ray-plane intersection) and GizmoAxis::Free (camera-forward plane)
  - Refactor: Unify axis and plane drag code paths

- [ ] Verification: Run full test suite. Manually verify: launch app, select entity, switch to Move tool, drag arrows and plane handles. [checkpoint marker]

---

## Phase 3: Rotate & Scale Gizmos

**Goal:** Implement rotate and scale gizmo rendering and interaction, reusing the state machine and math from Phase 1.

- [ ] Task 3.1: Implement `render_rotate_gizmo_system` — draws three rotation rings and an outer screen-aligned ring
  - TDD: Write integration test: spawn camera + entity + Tool::Rotate, run one frame, no panic
  - Implement: Use `gizmos.circle()` for each axis ring (rotated to face the axis), outer ring aligned to camera view direction. Apply screen-space scale
  - Refactor: Parameterize ring radius and segment count

- [ ] Task 3.2: Implement rotation drag interaction — dragging a ring rotates around that axis
  - TDD: Write test: simulate drag on Y ring with 90 degrees of mouse arc -> entity rotation around Y changes by ~90 degrees
  - Implement: On drag start, record initial angle from entity center to mouse ray intersection on the ring plane. Each frame, compute current angle, delta = current - initial. Apply as quaternion rotation around the axis. Accumulate and display arc angle
  - Refactor: Handle gimbal lock edge cases by using quaternion math throughout

- [ ] Task 3.3: Implement trackball rotation for the screen-aligned outer ring
  - TDD: Write test: drag on outer ring -> entity rotates freely (quaternion changes on multiple axes)
  - Implement: Compute rotation from mouse delta as two-axis rotation (horizontal delta -> Y rotation, vertical delta -> X rotation relative to camera)
  - Refactor: Normalize quaternion after applying delta

- [ ] Task 3.4: Implement `render_scale_gizmo_system` — draws three axis lines with cube endpoints and a center cube
  - TDD: Write integration test: spawn camera + entity + Tool::Scale, run one frame, no panic
  - Implement: Use `gizmos.line()` for axes with `gizmos.cuboid()` at endpoints. Center cuboid at entity origin. Apply screen-space scale
  - Refactor: Share axis color logic with move gizmo

- [ ] Task 3.5: Implement scale drag interaction — dragging an axis cube scales along that axis, center cube scales uniformly
  - TDD: Write test: drag X cube outward by 1 unit -> entity scale X increases. Drag center -> uniform scale increase. Assert minimum scale clamp at 0.01
  - Implement: Project mouse ray distance from entity center along the axis. Scale factor = current_distance / initial_distance. For uniform: use magnitude. Clamp minimum to 0.01
  - Refactor: Extract scale factor computation as a pure function

- [ ] Verification: Run full test suite. Manually verify rotate and scale gizmos with different entities. [checkpoint marker]

---

## Phase 4: Keyboard Shortcuts & Modal Transform

**Goal:** Wire G/R/S keyboard shortcuts, axis constraint toggling, and modal (keypress-initiated) transform mode.

- [ ] Task 4.1: Implement `keyboard_tool_switch_system` — G/R/S keys switch ToolState, ignored when egui has text focus
  - TDD: Write test: simulate G key press -> ToolState.active_tool == Tool::Move. Simulate with egui text focus active -> tool unchanged
  - Implement: System reads `ButtonInput<KeyCode>`, checks egui `wants_keyboard_input()`, sets ToolState
  - Refactor: Use a match table for key-to-tool mapping

- [ ] Task 4.2: Implement modal transform entry — pressing G/R/S with a selected entity enters ModalActive state
  - TDD: Write test: entity selected + G pressed -> GizmoInteraction == ModalActive { mode: Move, constraint: None }
  - Implement: In keyboard system, if entity selected and G/R/S pressed, transition to ModalActive, save initial transform
  - Refactor: Reuse drag state save logic from Phase 2

- [ ] Task 4.3: Implement `modal_transform_system` — in ModalActive state, mouse movement controls transform without click-drag
  - TDD: Write test: in ModalActive Move mode, mouse moves 3 units right -> entity X position changes by ~3 units
  - Implement: Each frame in ModalActive, compute transform delta from mouse movement projected onto constraint axis/plane (or free). Apply to entity. Left-click or Enter confirms (transition to Idle, commit). Escape cancels (restore initial_transform, transition to Idle)
  - Refactor: Share projection math with gizmo_drag_system

- [ ] Task 4.4: Implement axis constraint toggling in modal mode — X/Y/Z keys constrain, repeat toggles off, Shift+axis for perpendicular plane
  - TDD: Write tests for constraint state transitions: None->X, X->None (toggle), X->Y (switch), Shift+X->YZ plane
  - Implement: In modal_transform_system, read X/Y/Z key presses, update AxisConstraint in ModalActive state. Shift modifier -> perpendicular plane
  - Refactor: Visualize active constraint as a colored line through entity

- [ ] Task 4.5: Implement Escape cancel for both drag and modal modes
  - TDD: Write test: in Dragging state + Escape -> entity reverts to initial_transform, state -> Idle. Same for ModalActive
  - Implement: Check Escape key in drag and modal systems, restore transform, transition
  - Refactor: Unify cancel logic into a shared helper

- [ ] Verification: Run full test suite. Manually verify: press G with selection, move mouse, press X to constrain, Escape to cancel, Enter to confirm. [checkpoint marker]

---

## Phase 5: Snapping, Undo, Inspector Sync

**Goal:** Add grid snapping, an undo stack, and bidirectional inspector panel synchronization.

- [ ] Task 5.1: Define `SnapSettings` resource with default values (position: 0.5, rotation: 15.0 degrees, scale: 0.1)
  - TDD: Write test that SnapSettings::default() has the documented values
  - Implement: Resource struct with `position_grid`, `rotation_degrees`, `scale_grid` fields
  - Refactor: Add `snap_position`, `snap_rotation`, `snap_scale` convenience methods that delegate to snap_value/snap_angle

- [ ] Task 5.2: Integrate snapping into drag and modal systems — Ctrl held activates snapping
  - TDD: Write test: drag with Ctrl held, position 0.7 -> snaps to 0.5. Without Ctrl -> 0.7 exact
  - Implement: In drag/modal systems, check `ButtonInput<KeyCode>::pressed(KeyCode::ControlLeft)`, apply snap functions to computed values
  - Refactor: Draw visual grid overlay on constraint plane when snapping active (simple grid lines via Gizmos)

- [ ] Task 5.3: Implement `UndoStack` resource — stores previous transforms before each drag/modal operation
  - TDD: Write test: push transform, undo -> returns it. Push 16, push 17th -> oldest evicted. Empty stack undo -> None
  - Implement: `VecDeque<UndoEntry>` with max capacity 16. `UndoEntry { entity: Entity, transform: Transform }`
  - Refactor: Add `clear_for_entity(entity)` to remove stale entries if entity is deleted

- [ ] Task 5.4: Implement `undo_system` — Ctrl+Z pops the undo stack and applies the previous transform
  - TDD: Write test: entity at X=5, drag to X=10, Ctrl+Z -> entity at X=5. Inspector fields also reverted
  - Implement: On Ctrl+Z (not during active drag), pop UndoStack, set entity Transform, update InspectorState fields
  - Refactor: Ignore Ctrl+Z when egui has text focus

- [ ] Task 5.5: Implement gizmo-to-inspector sync — during drag, update InspectorState pos/rot/scale strings every frame
  - TDD: Write test: entity dragged to position (1.5, 2.3, 0.0) -> InspectorState.pos == ["1.50", "2.30", "0.00"]
  - Implement: In drag/modal systems, after updating entity Transform, format values to InspectorState string buffers
  - Refactor: Use a shared format helper `fn format_f32(v: f32) -> String` with 2 decimal places

- [ ] Task 5.6: Implement inspector-to-entity sync — editing a text field and pressing Enter applies the value to the entity Transform
  - TDD: Write test: set InspectorState.pos[0] to "3.00", trigger apply -> entity Transform.translation.x == 3.0
  - Implement: Detect when inspector text field loses focus or Enter is pressed (egui response), parse value, apply to entity Transform, push to undo stack
  - Refactor: Validate input (reject non-numeric), show red highlight on invalid input

- [ ] Verification: Run full test suite. Manually verify: snap with Ctrl, undo with Ctrl+Z, edit inspector field and see entity move. [checkpoint marker]

---

## Phase 6: DB Persistence & Polish

**Goal:** Wire transform changes to the database, handle edge cases, and finalize.

- [ ] Task 6.1: Extend `DbCommand` with `UpdateTransform` variant
  - TDD: Write test that `DbCommand::UpdateTransform` can be constructed and sent through a channel
  - Implement: Add `UpdateTransform { entity_id: String, position: [f32; 3], rotation: [f32; 4], scale: [f32; 3] }` to DbCommand enum in `fe-runtime/src/messages.rs`
  - Refactor: Ensure DbResult has a corresponding acknowledgment variant

- [ ] Task 6.2: Implement `transform_commit_system` — on drag/modal complete, send DbCommand::UpdateTransform
  - TDD: Write test: complete a drag -> DbCommand::UpdateTransform sent on the channel with correct values
  - Implement: System detects transition from Dragging/ModalActive to Idle (commit, not cancel), reads entity Transform, sends DbCommand via the existing channel infrastructure
  - Refactor: Debounce or batch if multiple transforms happen in quick succession

- [ ] Task 6.3: Handle edge cases — entity despawned during drag, camera moved during drag, window focus lost
  - TDD: Write test: entity despawned during drag -> state transitions to Idle gracefully, no panic
  - Implement: Add entity existence checks at the start of drag/modal systems. On missing entity, cancel drag and reset state
  - Refactor: Add tracing::warn for unexpected state transitions

- [ ] Task 6.4: Add gizmo input priority — gizmo hover consumes mouse input before camera orbit
  - TDD: Write test: cursor over gizmo handle -> camera orbit system does not receive the mouse delta
  - Implement: When GizmoInteraction is Hovering or Dragging, set a `GizmoCapturingInput` resource flag. Camera systems check this flag and skip processing
  - Refactor: Consider using Bevy's input consumption/event system if available in 0.18

- [ ] Task 6.5: Add hover highlight rendering — hovered axis renders brighter/thicker
  - TDD: Visual — verify manually that hovered axis changes color
  - Implement: In render systems, check GizmoInteraction state, use highlight color for hovered axis
  - Refactor: Define highlight colors alongside base colors in gizmo theme

- [ ] Task 6.6: Add doc comments to all public gizmo API items
  - TDD: Run `cargo doc -p fe-ui --no-deps` — no warnings
  - Implement: Add `///` doc comments to all public structs, enums, functions, and methods in the gizmo module
  - Refactor: Add module-level documentation with usage examples

- [ ] Task 6.7: Final integration test — full workflow: select entity, press G, press X, move mouse, press Enter, Ctrl+Z
  - TDD: Write integration test that exercises the complete gizmo workflow end-to-end
  - Implement: Fix any issues found during the integration test
  - Refactor: Clean up any test helpers, ensure test isolation

- [ ] Verification: `cargo test -p fe-ui`, `cargo clippy -p fe-ui -- -D warnings`, `cargo fmt -p fe-ui --check`, `cargo tarpaulin -p fe-ui`. Manually verify full gizmo workflow in running application. [checkpoint marker]
