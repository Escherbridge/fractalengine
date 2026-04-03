# Specification: Transform Gizmos — Blender-Style Move/Rotate/Scale Handles

**Track ID:** `transform_gizmos_20260402`
**Type:** Feature
**Date:** 2026-04-02
**Status:** Draft

---

## Overview

Add interactive 3D transform gizmos to the FractalEngine viewport, enabling operators to move, rotate, and scale selected entities using Blender-style visual handles. The gizmos render as overlays with fixed screen-space size, respond to mouse drag interactions, support axis constraints and snapping, and synchronize with the inspector panel and database in real time.

## Background

The current UI (`fe-ui`) defines a `Tool` enum with `Select`, `Move`, `Rotate`, `Scale` variants and toolbar buttons with keyboard hints (S/G/R/X). The inspector panel shows editable transform fields (Position, Rotation, Scale as X/Y/Z text inputs). However, there is no in-viewport 3D manipulation — all transforms are text-only. The Bloom Stage spec explicitly deferred 3D viewport gizmo handles to a later track. This track delivers that capability.

The existing `InspectorState` resource holds `pos`, `rot`, and `scale` as `[String; 3]` buffers. The `ToolState` resource holds the `active_tool: Tool`. The `DbCommand` enum in `fe-runtime` currently supports `Ping`, `Seed`, and `Shutdown` — transform persistence will require new variants.

Bevy 0.18 ships with a built-in `Gizmos` system parameter for debug line drawing. This can be used for gizmo rendering. Interaction (hit testing, dragging) must be custom-built.

## Functional Requirements

### FR-1: Move Gizmo

**Description:** When the active tool is `Tool::Move` and an entity is selected, render a move gizmo at the entity's world position consisting of three axis arrows (X=red, Y=green, Z=blue) and three plane handles (XY, XZ, YZ).

**Acceptance Criteria:**
- Three arrow shafts with conical tips render along entity local X (red), Y (green), Z (blue) axes
- A small center square allows free (unconstrained) translation
- Three plane-indicator squares at axis pair intersections allow planar movement (XY, XZ, YZ)
- Dragging an arrow constrains movement to that world axis
- Dragging a plane handle constrains movement to that plane
- Dragging the center allows free movement on the camera-facing plane
- The entity's `Transform.translation` updates every frame during drag

**Priority:** P0 (Critical)

### FR-2: Rotate Gizmo

**Description:** When the active tool is `Tool::Rotate` and an entity is selected, render a rotation gizmo consisting of three circular rings around the entity.

**Acceptance Criteria:**
- Three rings render around the entity: X-axis ring (red), Y-axis ring (green), Z-axis ring (blue)
- An outer screen-aligned ring allows trackball/free rotation
- Dragging a ring rotates the entity around that axis
- Rotation direction follows the right-hand rule
- A visual arc indicator shows the accumulated rotation angle during drag
- The entity's `Transform.rotation` updates every frame during drag

**Priority:** P0 (Critical)

### FR-3: Scale Gizmo

**Description:** When the active tool is `Tool::Scale` and an entity is selected, render a scale gizmo with cube-tipped axis handles.

**Acceptance Criteria:**
- Three axis lines with cube endpoints render along X (red), Y (green), Z (blue)
- A center cube allows uniform scaling on all axes
- Dragging an axis handle scales along that axis only
- Dragging the center cube scales uniformly
- Minimum scale clamped to 0.01 on any axis (prevent zero/negative scale)
- The entity's `Transform.scale` updates every frame during drag

**Priority:** P0 (Critical)

### FR-4: Gizmo Overlay Rendering

**Description:** Gizmos render as overlays that are always visible on top of scene geometry, with consistent screen-space size.

**Acceptance Criteria:**
- Gizmos are not occluded by scene meshes (rendered on top / depth-test disabled)
- Gizmo apparent size remains constant regardless of camera distance to the entity
- Hovered axis/handle highlights (brighter color or thicker line) to indicate interaction target
- Only one gizmo type renders at a time (determined by active tool)
- Gizmo disappears when no entity is selected

**Priority:** P0 (Critical)

### FR-5: Keyboard Shortcuts

**Description:** Blender-convention keyboard shortcuts switch the active tool and optionally begin immediate transform mode.

**Acceptance Criteria:**
- `G` key sets `ToolState.active_tool = Tool::Move`
- `R` key sets `ToolState.active_tool = Tool::Rotate`
- `S` key sets `ToolState.active_tool = Tool::Scale`
- `Escape` cancels an in-progress drag and reverts the entity to its pre-drag transform
- Keyboard shortcuts are ignored when an egui text field has focus (prevent typing conflicts)
- When an entity is already selected and G/R/S is pressed, the transform begins immediately in mouse-follow mode (modal transform)

**Priority:** P1 (High)

### FR-6: Axis Constraints

**Description:** During a modal transform (initiated by G/R/S key), pressing X/Y/Z constrains the operation to that axis. Pressing the same axis key again removes the constraint. Shift+axis constrains to the perpendicular plane.

**Acceptance Criteria:**
- After pressing G then X: movement constrained to world X axis
- After pressing G then X then X: constraint removed (free movement)
- After pressing G then Shift+X: movement constrained to YZ plane (perpendicular to X)
- Constraint axis visualized as a colored line through the entity
- Constraints apply to all three transform types (move, rotate, scale)
- Pressing a different axis key switches constraint (G, X, Y -> constrained to Y)

**Priority:** P1 (High)

### FR-7: Grid Snapping

**Description:** Holding Ctrl during a transform operation snaps values to a configurable grid.

**Acceptance Criteria:**
- Ctrl held during move: snaps position to nearest 0.5m increment (default)
- Ctrl held during rotate: snaps rotation to nearest 15-degree increment (default)
- Ctrl held during scale: snaps scale to nearest 0.1 (10%) increment (default)
- Snap increments stored in a `SnapSettings` resource (configurable)
- Visual grid lines appear on the active constraint plane when snapping is active

**Priority:** P2 (Medium)

### FR-8: Inspector Live Update

**Description:** The inspector panel transform fields update in real time as a gizmo is dragged. Releasing the gizmo commits the final transform.

**Acceptance Criteria:**
- During drag, `InspectorState.pos`, `rot`, and `scale` string buffers update every frame
- Values display with 2 decimal places
- On drag release, the final transform is sent to the database via `DbCommand`
- Editing a value in the inspector text field and pressing Enter applies that transform to the entity (bidirectional sync)

**Priority:** P0 (Critical)

### FR-9: Undo

**Description:** Ctrl+Z undoes the last completed transform operation.

**Acceptance Criteria:**
- Before each drag begins, the entity's full `Transform` is saved to an undo stack
- Ctrl+Z restores the most recent saved transform and removes it from the stack
- Undo applies to the currently selected entity
- Minimum: single-level undo (one operation). Stretch: multi-level undo stack (16 entries)
- Undo also updates the inspector fields and sends the reverted transform to the database

**Priority:** P1 (High)

---

## Non-Functional Requirements

### NFR-1: Performance

- Gizmo rendering must not drop the frame below 60 FPS on a scene with 100 entities
- Hit-testing for gizmo interaction must complete within 1ms per frame
- Inspector string formatting must not allocate excessively (reuse buffers where possible)

### NFR-2: Visual Consistency

- Gizmo colors follow the Blender convention: X=Red (#FF4444), Y=Green (#44FF44), Z=Blue (#4488FF)
- Highlighted/hovered axis uses a brighter variant of the same hue
- Gizmo line thickness: 2px for normal, 3px for hovered

### NFR-3: Input Priority

- Gizmo interaction takes priority over camera orbit/pan when the cursor is over a gizmo handle
- egui input takes priority over gizmo interaction (if an egui widget has focus, gizmo does not capture input)

### NFR-4: Testability

- All transform math (axis projection, plane intersection, snap rounding) must be pure functions testable without a Bevy App
- Gizmo state machine transitions must be unit-testable

---

## User Stories

### US-1: Move an Object Along One Axis

**As** a Petal operator, **I want** to grab an axis arrow and drag it **so that** I can precisely reposition a 3D model along a single axis.

**Given** a selected entity and Tool::Move active
**When** I click and drag the red (X) arrow
**Then** the entity moves only along the world X axis, the inspector Position X field updates in real time, and releasing the mouse commits the position to the database.

### US-2: Rotate Using Keyboard Shortcut

**As** a Petal operator, **I want** to press R then Y to rotate around the Y axis **so that** I can quickly orient objects without switching tools via the toolbar.

**Given** a selected entity
**When** I press R, then press Y, then move the mouse
**Then** the entity rotates around the world Y axis proportional to mouse movement, with a visual arc showing the angle, and Escape cancels the rotation.

### US-3: Uniform Scale

**As** a Petal operator, **I want** to drag the center cube of the scale gizmo **so that** I can resize an object proportionally on all axes.

**Given** a selected entity and Tool::Scale active
**When** I click the center cube and drag outward
**Then** the entity scales uniformly on X, Y, and Z, the inspector Scale fields all update together.

### US-4: Snap to Grid

**As** a Petal operator, **I want** to hold Ctrl while moving **so that** objects align to a consistent grid.

**Given** a selected entity being moved
**When** I hold Ctrl during drag
**Then** the position snaps to the nearest 0.5m grid point and visual grid lines appear.

### US-5: Undo a Mistake

**As** a Petal operator, **I want** to press Ctrl+Z after an accidental transform **so that** the object returns to its previous position/rotation/scale.

**Given** I just completed a drag transform
**When** I press Ctrl+Z
**Then** the entity reverts to its transform before the drag, the inspector updates, and the database receives the reverted transform.

---

## Technical Considerations

- **Bevy Gizmos API:** Use Bevy 0.18's built-in `Gizmos` system parameter for line/arc/circle drawing. This handles overlay rendering and depth. Custom interaction logic is required on top.
- **Hit Testing:** Implement ray-cast from cursor against gizmo geometry (cylinders for arrows, tori for rings, cubes for scale handles). Use `Camera::viewport_to_world` for the pick ray.
- **Screen-Space Sizing:** Calculate gizmo scale factor each frame as `constant / camera_distance_to_entity` to maintain fixed visual size.
- **State Machine:** Model the gizmo interaction as a state machine: `Idle -> Hovering(Axis) -> Dragging(Axis, origin, initial_transform) -> Idle`. This is testable independently of Bevy.
- **DbCommand Extension:** Add `DbCommand::UpdateTransform { entity_id, position, rotation, scale }` variant to `fe-runtime::messages`.
- **Inspector Sync Direction:** Gizmo drag writes to `InspectorState` (gizmo -> inspector). Inspector text edit writes to entity `Transform` (inspector -> entity). Avoid feedback loops by checking which source initiated the change.
- **Modal Transform:** When G/R/S is pressed with a selection, enter a special "modal" state where mouse movement directly controls the transform without requiring a click-drag on the gizmo. Click or Enter confirms; Escape cancels.
- **Crate Boundary:** All gizmo logic lives in `fe-ui`. No new crate needed. The gizmo systems run in the Bevy main schedule, not in `EguiPrimaryContextPass`.

## Out of Scope

- Local vs. World coordinate space toggle (world-only for v1)
- Custom pivot point (always entity origin)
- Multi-entity transform (transform only the single selected entity)
- Gizmo size preference in settings UI
- Proportional editing / soft selection
- Animation keyframing from transforms
- Network replication of in-progress drags (only final commit replicates)

## Open Questions

1. **Selection system dependency:** Is there a raycasting/picking system already implemented, or does this track need to build entity selection from scratch? (The Bloom Stage spec deferred gizmos but may have selection.)
2. **Transform space:** Should we plan for local-space transforms in a future track, and if so, should the state machine be designed to accommodate it now?
3. **Multi-level undo scope:** Should the undo stack be global (all entities) or per-entity? Global is simpler and matches Blender behavior.
