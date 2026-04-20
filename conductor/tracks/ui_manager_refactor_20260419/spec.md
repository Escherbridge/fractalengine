# Specification: UI Manager Architecture Refactor

**Track ID:** `ui_manager_refactor_20260419`
**Type:** Chore (refactor)
**Crate:** `fe-ui`

---

## Overview

The `fe-ui` crate has grown organically through Waves 1 and 2. It currently suffers from several architectural problems:

1. **No system ordering guarantees** -- the 6 `Update` systems in `GardenerConsolePlugin` run in undefined order relative to the 7 chained systems in `NodeManagerPlugin`. This creates frame-lag artifacts where inspector state is one frame behind selection.
2. **Scattered one-frame action signals** -- `url_save_pending`, `WebViewOpenRequest.url`, `PortalPanelState.close`, `PortalPanelState.go_back` are consumed by 3 separate systems (`handle_url_save`, `forward_webview_open_request`, `drain_portal_panel_actions`). A single `UiAction` queue with a draining system is simpler and prevents missed/double-dispatch bugs.
3. **7 independent dialog-state resources** -- `ContextMenuState`, `CreateDialogState`, `GltfImportState`, `NodeOptionsState`, `InviteDialogState`, `JoinDialogState`, `PeerDebugPanelState` each have an `open: bool` field. An `ActiveDialog` enum enforces mutual exclusion and removes 7 `init_resource` calls.
4. **Duplicated selection state** -- `InspectorState.selected_entity`, `InspectorState.selected_node_id`, `SidebarState.selected_node_id`, and `PortalPanelState.opened_for_entity` all shadow `NodeManager.selected`. This causes sync bugs and inflates system parameter counts.

Phase 1 (egui guard fix on `handle_viewport_click`) was completed in a prior commit. This track covers Phases 2-5.

---

## Background

### Current Architecture

- **plugin.rs** registers 16 resources and 6 un-ordered `Update` systems (`apply_camera_focus`, `strip_gltf_embedded_cameras`, `update_viewport_cursor_world`, `forward_webview_open_request`, `drain_portal_panel_actions`, `handle_url_save`).
- **node_manager.rs** registers `NodeManager` and 7 `.chain()`-ed `Update` systems (`handle_tool_shortcuts` -> `sync_sidebar_to_manager` -> `handle_gimbal_interaction` -> `handle_viewport_click` -> `sync_manager_to_inspector` -> `draw_gimbal_system` -> `broadcast_transform`).
- **panels.rs** `gardener_console()` accepts 21 parameters.
- **dialogs.rs** renders 7 dialog types, each reading its own state resource.

### Target Architecture

- A `UiSet` system set enum provides deterministic ordering across all UI systems.
- A `UiManager` resource holds a `Vec<UiAction>` queue and the `PortalState` enum.
- An `ActiveDialog` enum replaces 7 dialog-state resources.
- `NodeManager` becomes the single source of truth for selection; `InspectorState.selected_entity`, `InspectorState.selected_node_id`, and `SidebarState.selected_node_id` are removed. Panel rendering reads from `Res<NodeManager>` directly.

---

## Functional Requirements

### FR-1: UiSet System Ordering

**Description:** Define a `UiSet` Bevy `SystemSet` enum with variants `ProcessActions`, `Selection`, `PostSelection`. Configure explicit ordering between them in `GardenerConsolePlugin::build`. Move all 6 un-ordered `Update` systems into appropriate sets. `NodeManagerPlugin` systems go into `UiSet::Selection`.

**Acceptance Criteria:**
- `UiSet` enum exists with `ProcessActions`, `Selection`, `PostSelection` variants.
- `ProcessActions` runs before `Selection`; `Selection` runs before `PostSelection`.
- Every `Update` system in `fe-ui` belongs to exactly one `UiSet` variant.
- `NodeManagerPlugin` configures its `.chain()` inside `UiSet::Selection`.
- `apply_camera_focus` and `strip_gltf_embedded_cameras` are in `PostSelection`.
- `update_viewport_cursor_world` is in `ProcessActions` (needed before selection).
- All existing tests continue to pass.

**Priority:** P0 (prerequisite for FR-2 through FR-4)

---

### FR-2: UiAction Queue

**Description:** Add a `UiManager` resource containing `actions: Vec<UiAction>` and a `PortalState` enum (`Closed`, `Open { url: String, entity: Entity }`). Replace the one-frame signal fields `InspectorState.url_save_pending`, `WebViewOpenRequest.url`, `PortalPanelState.close`, `PortalPanelState.go_back` with `UiManager::push_action(UiAction::...)` calls. Write a single `process_ui_actions` system that drains the vector and dispatches each action. Remove `drain_portal_panel_actions` and `forward_webview_open_request` as standalone systems.

**Acceptance Criteria:**
- `UiAction` enum has variants: `SaveUrl { node_id, url }`, `OpenPortal { url }`, `ClosePortal`, `GoBackPortal`.
- `UiManager` resource is initialized once; no more `WebViewOpenRequest` resource.
- `process_ui_actions` system drains `actions` vec each frame and dispatches `BrowserCommand`s.
- `PortalState` enum replaces `PortalPanelState.open`, `.current_url`, `.opened_for_entity`.
- `PortalPanelState` struct is removed; `UiManager` owns portal state via `PortalState`.
- Portal auto-close on entity deselection logic is preserved inside `process_ui_actions`.
- `handle_url_save` system is folded into `process_ui_actions` (or remains but reads from the action queue).
- All portal open/close/back behavior is unchanged from the user's perspective.

**Priority:** P1

---

### FR-3: ActiveDialog Enum

**Description:** Create an `ActiveDialog` enum with variants `None`, `ContextMenu { screen_pos, world_pos }`, `CreateDialog { kind, parent_id, name_buf }`, `GltfImport { file_path_buf, name_buf, position }`, `NodeOptions { node_id, node_name_buf, webpage_url_buf }`, `InviteDialog { invite_string, include_write_cap, expiry_hours }`, `JoinDialog { invite_buf }`, `PeerDebug`. Store it as a single `Res<ActiveDialog>` (or field on `UiManager`). Update dialog rendering in `panels.rs` / `dialogs.rs` to match on this enum. Delete the 7 separate state resources and their `init_resource` calls.

**Acceptance Criteria:**
- `ActiveDialog` enum exists with all 7+1 variants (including `None`).
- Only one dialog can be open at a time (enforced by the enum).
- All dialog open/close transitions produce the same user-visible behavior.
- The 7 separate resources (`ContextMenuState`, `CreateDialogState`, `GltfImportState`, `NodeOptionsState`, `InviteDialogState`, `JoinDialogState`, `PeerDebugPanelState`) are deleted.
- `gardener_console()` parameter count decreases by at least 5.
- All dialog rendering functions accept `&mut ActiveDialog` instead of individual state references.

**Priority:** P1

---

### FR-4: Selection Deduplication

**Description:** Remove `InspectorState.selected_entity`, `InspectorState.selected_node_id`, and `SidebarState.selected_node_id`. Panel rendering functions receive `Res<NodeManager>` (or its inner data) directly. The `sync_manager_to_inspector` system is simplified to only sync transform strings and URL. `PortalPanelState.opened_for_entity` is replaced by `PortalState::Open { entity }` from FR-2.

**Acceptance Criteria:**
- `InspectorState` has no `selected_entity` or `selected_node_id` fields.
- `SidebarState` has no `selected_node_id` field.
- All code that read `inspector.selected_entity` now reads `node_mgr.selected_entity()`.
- All code that read `sidebar.selected_node_id` now reads `node_mgr.selected.as_ref().map(|s| &s.node_id)`.
- `sync_manager_to_inspector` only writes transform/URL fields (no entity/node_id copy).
- `gardener_console()` parameter count decreases further (NodeManager is already passed).
- Toolbar deselect button still works via `node_mgr.deselect()`.
- Inspector panel open/close still reacts correctly to selection state changes.

**Priority:** P2 (depends on FR-2 for PortalState)

---

## Non-Functional Requirements

### NFR-1: No Behavior Regression

All user-facing interactions (selection, deselection, dialog open/close, portal open/close/back, URL save, gimbal drag, camera focus, sidebar node click) must behave identically before and after the refactor.

### NFR-2: Code Coverage

New and modified code must achieve >80% unit test coverage per `cargo tarpaulin`.

### NFR-3: Compilation and Lint Compliance

`cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` must all pass after each phase.

### NFR-4: No New Dependencies

This refactor introduces no new crate dependencies. All changes are internal to `fe-ui`.

### NFR-5: Parameter Count Reduction

`gardener_console()` in `panels.rs` must accept fewer than 15 parameters after all phases complete (down from 21).

---

## User Stories

### US-1: System Ordering Correctness

**As** a developer working on fe-ui,
**I want** all UI systems to execute in a deterministic, documented order,
**So that** I can reason about state flow without frame-lag race conditions.

**Scenarios:**
- **Given** the app is running, **When** a user clicks a viewport node, **Then** the selection propagates to inspector state within the same frame (no one-frame delay).
- **Given** systems are configured with UiSet, **When** I add a new system, **Then** I must assign it to a UiSet variant or compilation fails (if using `#[required]` or convention).

### US-2: Action Queue Simplicity

**As** a developer adding new UI actions,
**I want** to push a typed action variant onto a queue,
**So that** I do not need to create new one-frame-signal resources and drain systems for each feature.

**Scenarios:**
- **Given** the inspector Save button is clicked, **When** the frame processes, **Then** `UiAction::SaveUrl` is drained and the URL is persisted.
- **Given** the portal close button is clicked, **When** `process_ui_actions` runs, **Then** `BrowserCommand::Close` is sent and `PortalState` transitions to `Closed`.

### US-3: Dialog Mutual Exclusion

**As** a user,
**I want** only one dialog open at a time,
**So that** dialogs do not stack or overlap confusingly.

**Scenarios:**
- **Given** the Create dialog is open, **When** the user right-clicks the viewport, **Then** the Create dialog closes and the context menu opens.
- **Given** no dialog is open, **When** the user clicks "Join", **Then** the Join dialog opens.
- **Given** the Join dialog is open, **When** the user presses Escape, **Then** the dialog closes and `ActiveDialog` becomes `None`.

### US-4: Single Selection Source

**As** a developer,
**I want** `NodeManager` to be the sole owner of selection state,
**So that** I never have to debug sync issues between 4 different selection fields.

**Scenarios:**
- **Given** a node is selected via viewport click, **When** the inspector panel queries selection, **Then** it reads from `NodeManager.selected_entity()` and gets the correct entity.
- **Given** a node is deselected via toolbar button, **When** the sidebar renders, **Then** no node is highlighted (sidebar reads from `NodeManager`).

---

## Technical Considerations

- **Bevy SystemSet ordering**: Use `app.configure_sets(Update, (UiSet::ProcessActions.before(UiSet::Selection), UiSet::Selection.before(UiSet::PostSelection)))`.
- **EguiPrimaryContextPass schedule**: The `gardener_ui_system` runs in `EguiPrimaryContextPass`, not `Update`. It is unaffected by `UiSet` ordering. It must read the finalized `ActiveDialog` and `UiManager` state.
- **P2pDialogParams SystemParam**: This bundle currently includes `portal_panel: ResMut<PortalPanelState>`. After FR-2, it should include `ui_mgr: ResMut<UiManager>` instead.
- **Borrow conflicts**: `panels.rs` functions take `&mut` references. Switching to `Res<NodeManager>` in panel rendering means the system must split the borrow: `node_mgr` for read-only selection data, `inspector` for transform buffers.
- **`PortalPanelState.close` / `.go_back`**: These are one-frame signals currently set by egui button clicks inside `EguiPrimaryContextPass` and consumed by `Update` systems. With the action queue, the egui rendering code calls `ui_mgr.push_action(UiAction::ClosePortal)` and the drain system runs later in `Update`.

---

## Out of Scope

- Redesigning the toolbar, sidebar, or inspector visual layout.
- Adding new UI features (new dialogs, new tools, new panels).
- Changing the `NodeManager` selection algorithm or gimbal interaction logic.
- Modifying `NavigationManager` or `VerseManager`.
- Cross-crate changes outside `fe-ui` (except trivially updating system parameter types if needed in the main app crate).
- Bevy version upgrade.

---

## Open Questions

1. **PeerDebug mutual exclusion**: The peer debug panel is currently collapsible and can be open alongside other dialogs. Should it be folded into `ActiveDialog` (enforcing mutual exclusion) or kept as a separate toggleable overlay? -- **Proposed answer**: Include it in `ActiveDialog` for consistency; it is rarely used concurrently with other dialogs.

2. **`PortalPanelRect` update**: The egui rendering system currently writes portal rect coordinates to `fe_webview::plugin::PortalPanelRect`. This cross-crate resource write must remain in `gardener_ui_system`. Does it need to move into `UiManager`? -- **Proposed answer**: No, keep it as-is; it is a display concern, not an action concern.

3. **`InspectorState` survival**: After removing `selected_entity` and `selected_node_id`, `InspectorState` retains `is_admin`, `external_url`, `config_url`, `pos[3]`, `rot[3]`, `scale[3]`, `url_save_pending` (removed in FR-2). Should it be renamed to `InspectorFormState` or similar? -- **Proposed answer**: Yes, rename to `InspectorFormState` for clarity.
