# Implementation Plan: Selection System -- Raycasting, Highlighting, and Inspector Sync

## Overview

This track is implemented in 5 phases:

1. **Core Selection Infrastructure** -- Selection resource, events, Selectable marker, plugin skeleton.
2. **Raycast Selection** -- Wire Bevy MeshPickingPlugin, egui input gating, single-click select/deselect.
3. **Selection Highlight** -- Visual feedback on selected entities via emissive material boost.
4. **Multi-Select and Box Select** -- Shift+click toggle, rectangle drag selection.
5. **Inspector Sync and Tool Integration** -- Bidirectional inspector data flow, keyboard shortcuts, tool-mode behavior.

Each phase ends with a verification checkpoint. All tasks follow TDD (Red -> Green -> Refactor).

---

## Phase 1: Core Selection Infrastructure

**Goal:** Establish the foundational types, resources, events, and plugin that the rest of the system builds on. No rendering or input handling yet -- purely data structures and event plumbing.

### Tasks

- [ ] **Task 1.1: Define `Selectable` marker component and `Selection` resource**
  Create `fe-ui/src/selection.rs`. Define:
  - `Selectable` -- empty marker `#[derive(Component)]`.
  - `Selection` -- resource holding `HashSet<Entity>` with methods: `select(Entity)`, `deselect(Entity)`, `toggle(Entity)`, `clear()`, `contains(Entity) -> bool`, `primary() -> Option<Entity>`, `count() -> usize`, `iter()`.
  (TDD: Write tests for all `Selection` methods -- empty state, add, remove, toggle, clear, primary returns last-added.)

- [ ] **Task 1.2: Define selection events**
  In `fe-ui/src/selection.rs`, define:
  - `EntitySelected(pub Entity)` -- `#[derive(Event)]`
  - `EntityDeselected(pub Entity)` -- `#[derive(Event)]`
  - `SelectionCleared` -- `#[derive(Event)]`
  (TDD: Write tests that events can be constructed and carry the correct entity.)

- [ ] **Task 1.3: Create `SelectionPlugin` and register types**
  Define `SelectionPlugin` implementing `Plugin`. Register:
  - `Selection` resource (init default).
  - All three events.
  - Add the plugin to `fe-ui/src/lib.rs` module exports.
  (TDD: Write test that builds a minimal `App` with `SelectionPlugin`, verifies resource exists and events are registered.)

- [ ] **Task 1.4: Selection state-change system that emits events**
  Create a system `emit_selection_events` that runs in `Update`. It compares the current `Selection` contents against a `Local<HashSet<Entity>>` snapshot from the previous frame and emits the appropriate events for any differences.
  (TDD: Write test that modifies `Selection` resource, runs the system, and asserts correct events are emitted via `EventReader`.)

- [ ] **Verification: Phase 1** -- Run `cargo test -p fe-ui`. All selection data structure and event tests pass. No rendering required. `cargo clippy -- -D warnings` clean. [checkpoint marker]

---

## Phase 2: Raycast Selection

**Goal:** Clicking a 3D object in the viewport selects it. Clicking empty space deselects. Input is gated so clicks on egui panels are ignored.

### Tasks

- [ ] **Task 2.1: Add `MeshPickingPlugin` and configure picking**
  In `SelectionPlugin::build`, add Bevy's `MeshPickingPlugin` (if not already added by the default plugins). Ensure `Selectable` entities also get `PickableBundle` or the required picking components.
  (TDD: Write test that spawns a camera + mesh entity with `Selectable`, verifies picking components are present after plugin initialization.)

- [ ] **Task 2.2: Implement egui input gating resource**
  Create `EguiInputGate` resource (or a system) that each frame checks `EguiContexts::ctx_mut().is_pointer_over_area()` and stores a `pub pointer_over_ui: bool` flag. Other systems read this to skip viewport interaction.
  (TDD: Write test with mock egui context behavior -- verify gate defaults to false when egui is not claiming pointer.)

- [ ] **Task 2.3: Implement click-to-select system**
  Create system `handle_viewport_click` that:
  1. Reads `EguiInputGate` -- skip if pointer is over UI.
  2. Reads `ToolState` -- only process in Select mode (or select-on-click in Move/Rotate/Scale).
  3. Listens for `Pointer<Click>` events from Bevy picking on `Selectable` entities.
  4. On click: if entity is selectable, set `Selection` to contain only that entity (single-select, no Shift).
  5. If no entity was hit (click on empty space): call `Selection::clear()`.
  (TDD: Write test that simulates a pick hit event, verifies Selection contains the entity. Write test for empty-space click clearing selection.)

- [ ] **Task 2.4: Implement Escape key deselect**
  Create system `handle_escape_deselect` that reads `ButtonInput<KeyCode>` for `KeyCode::Escape` just-pressed and calls `Selection::clear()` when no egui text field is focused.
  (TDD: Write test that simulates Escape press, verifies selection is cleared.)

- [ ] **Task 2.5: Sync Selection to InspectorState.selected_entity**
  Create system `sync_selection_to_inspector` that runs after selection changes. Sets `InspectorState.selected_entity` to `Selection::primary()`. This bridges the new multi-select resource with the existing single-entity inspector.
  (TDD: Write test that sets Selection, runs system, verifies InspectorState.selected_entity matches.)

- [ ] **Verification: Phase 2** -- Run `cargo test -p fe-ui`. Manually verify: launch app, click a mesh entity (if test scene exists) -- entity appears in InspectorState. Click empty space -- deselected. Press Escape -- deselected. Click on egui panel -- no selection change. [checkpoint marker]

---

## Phase 3: Selection Highlight

**Goal:** Selected entities get a visible emissive highlight. Deselected entities revert to their original appearance.

### Tasks

- [ ] **Task 3.1: Define `OriginalMaterial` and `SelectionHighlight` components**
  - `OriginalMaterial(pub Handle<StandardMaterial>)` -- stores the entity's material handle before highlight.
  - `SelectionHighlight` -- marker component indicating highlight is currently applied.
  (TDD: Write test that components can be inserted and queried.)

- [ ] **Task 3.2: Implement highlight application system**
  Create system `apply_selection_highlight` that runs after selection changes:
  1. For each entity in `Selection` that does NOT have `SelectionHighlight`:
     - Read its current `MeshMaterial3d<StandardMaterial>` handle.
     - Store it as `OriginalMaterial`.
     - Clone the material asset, boost `emissive` by a configurable color (e.g., `Color::srgba(0.4, 0.6, 1.0, 1.0) * 2.0`).
     - Insert the cloned material handle and `SelectionHighlight` marker.
  2. For each entity WITH `SelectionHighlight` that is NOT in `Selection`:
     - Restore `OriginalMaterial` handle.
     - Remove `OriginalMaterial` and `SelectionHighlight` components.
  (TDD: Write test that selects entity, runs system, verifies `SelectionHighlight` is added. Deselect, run system, verify it is removed and original material is restored.)

- [ ] **Task 3.3: Handle entity despawn cleanup**
  Create system `cleanup_despawned_selections` that removes despawned entities from the `Selection` resource. Use `RemovedComponents<Selectable>` or entity validity checks.
  (TDD: Write test that adds entity to Selection, despawns it, runs cleanup, verifies entity is removed from Selection.)

- [ ] **Verification: Phase 3** -- Run `cargo test -p fe-ui`. Manually verify: select an entity -- it glows with blue-ish emissive tint. Deselect -- glow disappears. Select two entities (once Phase 4 is done) -- both glow. [checkpoint marker]

---

## Phase 4: Multi-Select and Box Select

**Goal:** Shift+click toggles entities in/out of the selection. Click-drag in empty space draws a rectangle; entities within it are selected on release.

### Tasks

- [ ] **Task 4.1: Implement Shift+click toggle**
  Modify `handle_viewport_click` (Task 2.3): if Shift is held during a pick-hit, call `Selection::toggle(entity)` instead of replacing the selection. If Shift is held during an empty-space click, do NOT clear the selection.
  (TDD: Write test for Shift+click adding entity. Write test for Shift+click removing already-selected entity. Write test that Shift+click on empty space does not clear.)

- [ ] **Task 4.2: Define BoxSelect state resource**
  Create `BoxSelectState` resource:
  - `active: bool`
  - `start_pos: Option<Vec2>` (screen-space)
  - `current_pos: Option<Vec2>`
  (TDD: Write test for default state, starting a box, updating position.)

- [ ] **Task 4.3: Implement box select input handling**
  Create system `handle_box_select_input`:
  1. Only active in Select mode, pointer not over UI.
  2. On left mouse press in empty space (no pick hit): set `BoxSelectState.active = true`, record `start_pos`.
  3. While dragging: update `current_pos`.
  4. On release: compute screen-space rectangle, query all `Selectable` entities, project their world position to screen-space via `Camera::world_to_viewport()`. Entities whose screen position falls within the rectangle are selected. If Shift held, add to selection; otherwise replace.
  5. Reset `BoxSelectState`.
  (TDD: Write test with known entity screen positions and a box rect, verify correct entities are selected.)

- [ ] **Task 4.4: Render box select rectangle overlay**
  Create system `draw_box_select_rect` that runs in the egui rendering pass. When `BoxSelectState.active`, draw a translucent rectangle using `egui::Painter::rect_filled` with a border.
  (TDD: Write test that BoxSelectState.active triggers rectangle data -- verify start/end coords are used. Visual verification is manual.)

- [ ] **Task 4.5: Update inspector for multi-select summary**
  Modify the inspector entity section: when `Selection::count() > 1`, display "N entities selected" instead of a single entity's ID. `InspectorState.selected_entity` still holds `Selection::primary()` for backward compatibility.
  (TDD: Write test that with 3 entities selected, inspector shows count. With 1 entity, shows entity ID.)

- [ ] **Verification: Phase 4** -- Run `cargo test -p fe-ui`. Manually verify: Shift+click to add/remove from selection. Drag rectangle in viewport to select multiple entities. Inspector shows count when multiple selected. [checkpoint marker]

---

## Phase 5: Inspector Sync and Tool Integration

**Goal:** Inspector fields reflect the selected entity's live transform. Editing fields updates the entity. Keyboard shortcuts for tools. Tool-mode behavior determines click semantics.

### Tasks

- [ ] **Task 5.1: Populate inspector from selected entity's Transform**
  Create system `sync_entity_to_inspector`:
  1. Runs each frame when `Selection` is non-empty.
  2. Reads `primary()` entity's `Transform`.
  3. Formats position/rotation/scale to 2 decimal places and writes to `InspectorState` buffers.
  4. Also reads any metadata component (e.g., `ModelMetadata { display_name, external_url, config_url }`) and populates URL fields.
  (TDD: Write test that spawns entity with known Transform, selects it, runs system, verifies InspectorState buffers match.)

- [ ] **Task 5.2: Implement `InspectorSaveRequested` event and apply system**
  Define `InspectorSaveRequested` event. Create system `apply_inspector_to_entity`:
  1. On `InspectorSaveRequested`, parse `InspectorState` pos/rot/scale strings to `f32`.
  2. If parsing succeeds, update the entity's `Transform`.
  3. If parsing fails for any field, revert that field's string buffer to the entity's current value.
  4. Emit a `TransformUpdated(Entity)` event for downstream DB sync.
  (TDD: Write test with valid inputs -- verify Transform updated. Write test with invalid input "abc" -- verify field reverts and Transform unchanged.)

- [ ] **Task 5.3: Wire inspector Save button to emit `InspectorSaveRequested`**
  In `panels.rs`, modify the Save button click handler to send `InspectorSaveRequested` via a Bevy `EventWriter` (requires passing the writer into the UI system or using a flag resource that the Bevy system reads).
  (TDD: Write test that simulates save flag being set, verifies event is emitted.)

- [ ] **Task 5.4: Implement keyboard shortcuts for tool switching**
  Create system `handle_tool_shortcuts` that reads `ButtonInput<KeyCode>`:
  - `S` -> `Tool::Select`
  - `G` -> `Tool::Move`
  - `R` -> `Tool::Rotate`
  - `X` -> `Tool::Scale`
  Only fires when no egui text field is focused.
  (TDD: Write test that simulates key press, verifies ToolState changes.)

- [ ] **Task 5.5: Tool-mode click behavior differentiation**
  Modify `handle_viewport_click` to handle Move/Rotate/Scale modes:
  1. If clicking an unselected entity in Move/Rotate/Scale mode, select it (same as Select mode).
  2. If clicking an already-selected entity in Move/Rotate/Scale mode, emit `GizmoInteractionRequested(Entity, Tool)` event (stub -- Transform Gizmos track will implement the handler).
  3. Define `GizmoInteractionRequested(pub Entity, pub Tool)` event.
  (TDD: Write test for selecting unselected entity in Move mode. Write test for clicking selected entity in Move mode emitting gizmo event.)

- [ ] **Task 5.6: Database bridge for transform persistence**
  Create system `persist_transform_changes` that listens for `TransformUpdated(Entity)` events and sends the new transform data through the existing `fe-database` channel for SurrealDB persistence. Uses `AsyncComputeTaskPool` to avoid blocking the main thread.
  (TDD: Write test that TransformUpdated event triggers a message on the database channel -- mock the channel receiver.)

- [ ] **Verification: Phase 5** -- Run `cargo test -p fe-ui`. Run `cargo test` (full workspace). Manually verify: select entity, inspector shows correct transform. Edit position, click Save, entity moves. Press S/G/R/X to switch tools. In Move mode, click unselected entity to select it. `cargo clippy -- -D warnings` and `cargo fmt --check` clean. [checkpoint marker]

---

## Summary

| Phase | Tasks | Estimated Effort |
|-------|-------|-----------------|
| 1. Core Infrastructure | 4 + verification | 2-3 hours |
| 2. Raycast Selection | 5 + verification | 3-4 hours |
| 3. Selection Highlight | 3 + verification | 2-3 hours |
| 4. Multi-Select & Box Select | 5 + verification | 4-5 hours |
| 5. Inspector Sync & Tools | 6 + verification | 4-5 hours |
| **Total** | **23 tasks + 5 checkpoints** | **15-20 hours** |

### Key Dependencies

- **Bevy 0.18 `MeshPickingPlugin`:** Must be available in the pinned Bevy version. If the API differs from expected, Task 2.1 will need adjustment.
- **Entities with meshes in scene:** At least test entities must be spawnable for integration testing. Use a simple cube/sphere mesh for tests.
- **`fe-database` channel:** Task 5.6 requires the existing database write channel to be operational.

### Risks

1. **Bevy picking API surface:** Bevy 0.18 picking may have changed from 0.15-0.17. Mitigate by reading Bevy 0.18 migration guide before starting Phase 2.
2. **egui input conflict:** Bevy picking and egui may both consume the same pointer events. Mitigate with explicit `is_pointer_over_area()` gating in Task 2.2.
3. **Material cloning performance:** Cloning materials per selection could cause stutter with many entities. Mitigate by caching highlight materials and only cloning once per unique source material.
