# Specification: Selection System -- Raycasting, Highlighting, and Inspector Sync

**Track ID:** `selection_system_20260402`
**Type:** Feature
**Date:** 2026-04-02
**Depends on:** Viewport Foundation + Scene Graph Bridge (entities must exist in the scene)
**Blocks:** Transform Gizmos

---

## Overview

The Selection System provides the core interaction model for the FractalEngine editor viewport. Users click 3D objects to select them, see a visual highlight on selected entities, and have the Inspector panel populated with the selected entity's transform, metadata, and portal URLs. The system supports multi-select (Shift+click and box select), deselection (Escape, empty-space click, toolbar button), and emits Bevy events so downstream systems (Transform Gizmos, future context menus) can react.

---

## Background

The Gardener Console UI (`fe-ui`) already provides:
- A `ToolState` resource with `Tool::Select`, `Tool::Move`, `Tool::Rotate`, `Tool::Scale` variants.
- An `InspectorState` resource with `selected_entity: Option<Entity>`, transform string buffers, and URL fields.
- A right inspector panel that shows/hides based on `selected_entity.is_some()`.
- A "Deselect" toolbar button that sets `selected_entity = None`.
- Viewport hint text prompting the user to click an object.

What is missing: there is no connection between a mouse click in the 3D viewport and the ECS. No raycasting, no highlighting, no multi-select, no selection events.

Bevy 0.18 includes `MeshPickingPlugin` in `bevy::picking` which provides built-in mesh raycasting. This track will use that system rather than rolling a custom raycast.

---

## Functional Requirements

### FR-1: Raycast Single Selection

**Description:** When the user clicks (left mouse button, press-and-release) in the 3D viewport while in Select mode, a ray is cast from the camera through the click position. The nearest mesh-bearing entity hit by the ray becomes the selected entity.

**Acceptance Criteria:**
- Clicking a mesh entity in Select mode sets it as the selected entity.
- Clicking does NOT fire when the pointer is over an egui panel (sidebar, inspector, toolbar, status bar). Use `egui::Context::is_pointer_over_area()` to gate input.
- Only entities marked with a `Selectable` component are candidates.
- If no selectable entity is hit, treat as a deselect (see FR-5).
- Selection works regardless of camera angle or distance (within the camera frustum).

**Priority:** P0 (Critical)

---

### FR-2: Selection Highlight

**Description:** When an entity is selected, it receives a clear visual distinction from unselected entities. V1 uses an emissive color boost applied via a `SelectionHighlight` component that modifies the entity's `StandardMaterial`.

**Acceptance Criteria:**
- A selected entity has a visible highlight that is distinguishable from its unselected state.
- When deselected, the entity returns to its original appearance (no permanent material mutation).
- The highlight is applied within one frame of the selection event.
- Multiple selected entities (multi-select) each display the highlight independently.
- The highlight must not break transparency or PBR lighting on the model.

**Priority:** P0 (Critical)

---

### FR-3: Multi-Select

**Description:** Users can select multiple entities simultaneously using Shift+click (toggle individual entities) and box select (click-drag a rectangle in the viewport to select all entities within it).

**Acceptance Criteria:**
- **Shift+Click:** Holding Shift and clicking a selectable entity toggles it in/out of the current selection set. If not selected, it is added; if already selected, it is removed.
- **Box Select:** In Select mode, click-dragging (without Shift) in empty space draws a translucent selection rectangle. On mouse release, all selectable entities whose screen-space bounding boxes intersect the rectangle are selected (replacing the current selection).
- **Shift+Box Select:** Adds the box-selected entities to the existing selection rather than replacing it.
- The inspector shows the last-selected entity's details when multiple are selected (or shows a "N entities selected" summary in the Entity section).

**Priority:** P1 (High)

---

### FR-4: Inspector Sync

**Description:** When an entity is selected, its `Transform` component values populate `InspectorState` (position, rotation, scale string buffers). When the user edits those fields and clicks Save, the values are written back to the entity's `Transform` and persisted to SurrealDB.

**Acceptance Criteria:**
- On selection, `InspectorState.pos`, `.rot`, `.scale` reflect the entity's current `Transform`, formatted to 2 decimal places.
- `InspectorState.external_url` and `.config_url` are populated from the entity's metadata component (if present).
- Editing a transform field in the inspector and triggering save updates the entity's `Transform` in the ECS.
- Transform changes are persisted to SurrealDB via the existing database channel (fire-and-forget write, not blocking the main thread).
- If the entity's transform is changed externally (e.g., by another system), the inspector updates on the next frame while the entity is still selected.
- Invalid input (non-numeric text) in transform fields is rejected; the field reverts to the last valid value.

**Priority:** P0 (Critical)

---

### FR-5: Deselect

**Description:** The user can clear the selection through multiple methods.

**Acceptance Criteria:**
- **Click empty space:** Clicking in the viewport where no selectable entity is hit clears the entire selection.
- **Escape key:** Pressing Escape clears the selection (only when no egui text field is focused).
- **Toolbar button:** The existing "Deselect" button clears the selection (already wired to set `selected_entity = None`; extend for multi-select).
- **Inspector close button:** The existing X button in the inspector header clears the selection.
- On deselect, all highlights are removed and `SelectionCleared` event is sent.

**Priority:** P0 (Critical)

---

### FR-6: Selection Events

**Description:** The selection system emits Bevy events that other systems can observe.

**Acceptance Criteria:**
- `EntitySelected(Entity)` is sent when an entity is added to the selection.
- `EntityDeselected(Entity)` is sent when an entity is removed from the selection.
- `SelectionCleared` is sent when the entire selection is cleared (in addition to individual `EntityDeselected` events for each entity).
- Events are sent in the same frame as the selection change.
- Events use standard Bevy `Event` derive and are registered in the plugin.

**Priority:** P0 (Critical)

---

### FR-7: Tool Integration

**Description:** The active tool from `ToolState` determines how mouse interaction behaves in the viewport.

**Acceptance Criteria:**
- In **Select mode:** Click selects, Shift+click toggles, click-drag starts box select.
- In **Move/Rotate/Scale modes:** Click on an unselected entity selects it. Click-drag on a selected entity is reserved for the Transform Gizmos track (this track emits a `GizmoInteractionRequested` event but does not implement the gizmo behavior).
- Tool switching via toolbar buttons or keyboard shortcuts (S=Select, G=Move, R=Rotate, X=Scale) is already handled by the UI; this track does not change that.
- When switching tools, the current selection is preserved.

**Priority:** P1 (High)

---

## Non-Functional Requirements

### NFR-1: Performance

- Raycasting must complete within 1ms for scenes with up to 1000 selectable entities.
- Highlight material changes must not cause frame drops (no per-frame material cloning).
- Box select rectangle rendering must not allocate on each frame.

### NFR-2: Responsiveness

- Selection highlight must appear within the same frame as the click (no 1-frame delay visible to the user).
- Inspector sync must complete within the same frame as the selection change.

### NFR-3: Testability

- All selection logic must be testable without a rendered window (headless Bevy app).
- Event emission must be verifiable in unit tests.
- Highlight application/removal must be verifiable by querying ECS components.

---

## User Stories

### US-1: Select a Model

**As** a Node operator editing a Petal,
**I want** to click a 3D model in the viewport to select it,
**So that** I can inspect and modify its properties.

**Given** the viewport contains selectable 3D models and the Select tool is active,
**When** I click on a model,
**Then** it becomes highlighted and the Inspector panel opens with its transform and metadata.

### US-2: Multi-Select Models

**As** a Node operator,
**I want** to select multiple models at once,
**So that** I can perform batch operations or review a group.

**Given** one model is already selected,
**When** I Shift+click another model,
**Then** both models are highlighted and the inspector shows a multi-selection summary.

### US-3: Box Select

**As** a Node operator,
**I want** to drag a rectangle in the viewport to select all models within it,
**So that** I can quickly select a group of nearby models.

**Given** the Select tool is active and I click-drag in empty space,
**When** I release the mouse button,
**Then** all selectable models within the rectangle become selected.

### US-4: Deselect

**As** a Node operator,
**I want** to clear my selection by clicking empty space or pressing Escape,
**So that** I can start fresh or dismiss the Inspector.

**Given** one or more models are selected,
**When** I click empty viewport space (or press Escape),
**Then** all highlights are removed, the Inspector closes, and a SelectionCleared event fires.

### US-5: Edit Transform via Inspector

**As** a Node operator,
**I want** to edit a selected model's position/rotation/scale in the Inspector and have those changes apply immediately,
**So that** I can precisely place models in the Petal.

**Given** a model is selected and the Inspector shows its transform,
**When** I change the Position X field to "5.00" and click Save,
**Then** the model's Transform component updates to x=5.0 and the change is persisted to the database.

---

## Technical Considerations

1. **Bevy 0.18 Picking:** Use `MeshPickingPlugin` from `bevy::picking`. Entities need a `Mesh3d` component (standard for GLTF scenes) and the `Selectable` marker. Bevy's picking emits `Pointer<Click>` observers that this system will listen to.

2. **egui Input Gating:** Before processing viewport clicks, check `egui::Context::is_pointer_over_area()` via `bevy_egui::EguiContexts`. If egui claims the pointer, skip all selection logic.

3. **Selection State Resource:** Introduce a `Selection` resource holding a `HashSet<Entity>` to support multi-select. `InspectorState.selected_entity` becomes a derived view (the "primary" selected entity for inspector display).

4. **Highlight Strategy:** Store original material handles in a `OriginalMaterial(Handle<StandardMaterial>)` component. On select, clone the material, boost emissive, and assign the clone. On deselect, restore the original handle. This avoids mutating shared materials.

5. **Module Location:** New module `fe-ui/src/selection.rs` for the selection plugin. This keeps selection logic co-located with the UI crate that already owns `InspectorState` and `ToolState`.

6. **Database Sync:** Inspector save triggers an event (`InspectorSaveRequested`) that a bridge system converts to a SurrealDB write via the existing `fe-database` channel. This track implements the event and the ECS-side transform update; the DB write integration is a thin bridge.

7. **Keyboard Shortcuts:** Escape for deselect. Tool shortcuts (S, G, R, X) are already implied by the toolbar but not yet wired to keyboard input -- this track adds keyboard bindings for tool switching as well.

---

## Out of Scope

- Transform Gizmo rendering and interaction (separate track, blocked by this one).
- Undo/redo for selection or transform changes.
- Right-click context menus on selected entities.
- Selection persistence across Petal switches or app restarts.
- Group/hierarchy-aware selection (selecting a parent selects children).
- Selection filtering by entity type or tag.

---

## Open Questions

1. **Highlight visual:** Emissive boost is the simplest V1 approach. Should we invest in an outline post-process shader instead? (Decision: start with emissive boost; outline shader can be added as a polish task later.)

2. **Multi-select inspector:** When N > 1 entities are selected, should the inspector show shared fields (e.g., if all have the same URL) or just a count? (Decision: show count + "N entities selected" for V1; shared-field editing is a future enhancement.)

3. **Box select in non-Select tools:** Should box select work in Move/Rotate/Scale modes? (Decision: no, box select is Select-mode only for V1.)
