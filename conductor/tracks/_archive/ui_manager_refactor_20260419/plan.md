# Implementation Plan: UI Manager Architecture Refactor

**Track ID:** `ui_manager_refactor_20260419`
**Type:** Chore (refactor)
**Crate:** `fe-ui`

---

## Overview

This plan covers 4 phases (Phases 2-5 of the overall refactor; Phase 1 was completed previously). Each phase is independently shippable and passes all quality gates before proceeding.

**Phase dependency chain:** Phase 2 (UiSet) -> Phase 3 (UiAction Queue) -> Phase 4 (ActiveDialog) -> Phase 5 (Selection Dedup)

Phase 2 is a prerequisite because later phases add new systems that need ordering. Phase 3 must precede Phase 5 because Phase 5 removes `PortalPanelState.opened_for_entity` which moves into `PortalState` from Phase 3. Phase 4 is independent of Phase 5 but is sequenced between them for incremental complexity.

---

## Phase 2: System Ordering (UiSet)

**Goal:** Introduce deterministic system ordering across all `Update` systems in `fe-ui` using a `UiSet` system set enum.

**Files touched:** `plugin.rs`, `node_manager.rs`

### Tasks

- [ ] Task 2.1: Define `UiSet` enum and configure set ordering (TDD: Write test asserting `UiSet` variants derive required traits; implement enum with `SystemSet`, `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`; configure `.before()` ordering in a helper function)

- [ ] Task 2.2: Move `GardenerConsolePlugin` Update systems into UiSet variants (TDD: Write test that the plugin builds without panic in a minimal Bevy app; move `update_viewport_cursor_world` into `ProcessActions`; move `forward_webview_open_request`, `drain_portal_panel_actions`, `handle_url_save` into `ProcessActions`; move `apply_camera_focus`, `strip_gltf_embedded_cameras` into `PostSelection`)

- [ ] Task 2.3: Move `NodeManagerPlugin` systems into `UiSet::Selection` (TDD: Write test that NodeManagerPlugin builds without panic; add `.in_set(UiSet::Selection)` to the existing `.chain()` tuple; verify the chain still works within the set)

- [ ] Task 2.4: Verify full build and existing tests pass (TDD: `cargo test -p fe-ui`; `cargo clippy -p fe-ui -- -D warnings`; `cargo fmt -p fe-ui --check`)

- [ ] Verification: Build the app, click nodes in the viewport, verify selection and inspector update in the same frame with no visual flicker. Drag a gimbal axis and confirm transform updates are smooth. Open and close the portal webview. [checkpoint marker]

---

## Phase 3: UiAction Queue

**Goal:** Replace scattered one-frame signal fields with a centralized `UiAction` queue on a `UiManager` resource. Consolidate `drain_portal_panel_actions`, `forward_webview_open_request`, and `handle_url_save` into a single `process_ui_actions` system.

**Files touched:** `plugin.rs`, `panels.rs`, `node_manager.rs` (portal close-on-deselect logic)

### Tasks

- [ ] Task 3.1: Define `UiAction` enum and `PortalState` enum (TDD: Write tests for `UiAction` variant construction, `PortalState` default is `Closed`, `PortalState::Open` carries url and entity; implement enums)

- [ ] Task 3.2: Define `UiManager` resource with `actions: Vec<UiAction>` and `portal: PortalState` (TDD: Write test for `UiManager::default()`, `push_action()` appends to vec, `drain_actions()` returns and clears; implement struct and methods)

- [ ] Task 3.3: Write `process_ui_actions` system shell (TDD: Write Bevy system test that creates `UiManager` with pre-pushed actions, runs `process_ui_actions`, and asserts actions are drained; implement the drain loop with match arms stubbed)

- [ ] Task 3.4: Implement `UiAction::OpenPortal` dispatch (TDD: Test that pushing `OpenPortal { url }` transitions `PortalState` to `Open` and sends `BrowserCommand::Navigate`; implement match arm; update `inspector_url_meta_section` in `panels.rs` to call `ui_mgr.push_action()` instead of setting `webview_request.url`)

- [ ] Task 3.5: Implement `UiAction::ClosePortal` and `UiAction::GoBackPortal` dispatch (TDD: Test `ClosePortal` transitions to `Closed` and sends `BrowserCommand::Close`; test `GoBackPortal` sends `BrowserCommand::GoBack`; update `right_portal_toolbar` in `panels.rs`)

- [ ] Task 3.6: Implement `UiAction::SaveUrl` dispatch (TDD: Test that `SaveUrl { node_id, url }` calls `verse_mgr.update_node_url()` and sends `DbCommand::UpdateNodeUrl`; move logic from `handle_url_save`; update inspector Save button to push action)

- [ ] Task 3.7: Implement portal auto-close on deselection (TDD: Test that when `PortalState::Open { entity }` and `NodeManager.selected_entity() != Some(entity)`, the system pushes `ClosePortal`; move logic from `drain_portal_panel_actions`)

- [ ] Task 3.8: Remove `WebViewOpenRequest`, `PortalPanelState`, `forward_webview_open_request`, `drain_portal_panel_actions`, `handle_url_save` (TDD: Verify removal compiles; grep for old type names and fix any remaining references; remove `InspectorState.url_save_pending` field; update `P2pDialogParams` to include `UiManager` instead of `WebViewOpenRequest` and `PortalPanelState`)

- [ ] Task 3.9: Register `UiManager` in plugin, wire `process_ui_actions` into `UiSet::ProcessActions` (TDD: Full `cargo test -p fe-ui`; `cargo clippy -p fe-ui -- -D warnings`)

- [ ] Verification: Run the app. Test the full portal flow: select a node with a URL, click "Open Portal", verify webview opens, click back, click close. Select a different node, verify portal auto-closes. Click Save on the URL field, verify DB persistence. Verify no regressions in selection or gimbal. [checkpoint marker]

---

## Phase 4: ActiveDialog Enum

**Goal:** Replace 7 separate dialog-state resources with a single `ActiveDialog` enum. Enforce mutual exclusion: only one dialog can be open at a time.

**Files touched:** `plugin.rs`, `panels.rs`, `dialogs.rs`

### Tasks

- [ ] Task 4.1: Define `ActiveDialog` enum with all variants (TDD: Write test for default `None`, construction of each variant with expected fields; implement enum with `None`, `ContextMenu`, `CreateDialog`, `GltfImport`, `NodeOptions`, `InviteDialog`, `JoinDialog`, `PeerDebug` variants)

- [ ] Task 4.2: Add `ActiveDialog` as a Bevy resource, keep old resources temporarily for parallel operation (TDD: Test that `ActiveDialog` can be inserted as resource in a minimal app; add `init_resource::<ActiveDialog>()` to plugin alongside existing resources)

- [ ] Task 4.3: Migrate `ContextMenuState` into `ActiveDialog::ContextMenu` (TDD: Test that `render_context_menu` with `ActiveDialog::ContextMenu { .. }` renders and `ActiveDialog::None` does not; refactor `render_context_menu` to accept `&mut ActiveDialog`; update call site in `panels.rs`; remove `ContextMenuState` resource)

- [ ] Task 4.4: Migrate `CreateDialogState` into `ActiveDialog::CreateDialog` (TDD: Same pattern; refactor `render_create_dialog`; update all call sites that set `create_dialog.open = true` to set `ActiveDialog::CreateDialog { .. }`; remove `CreateDialogState` resource)

- [ ] Task 4.5: Migrate `GltfImportState` into `ActiveDialog::GltfImport` (TDD: Same pattern; refactor `render_gltf_import_dialog`; update context menu "Add GLTF Model" to transition `ActiveDialog`; remove `GltfImportState` resource)

- [ ] Task 4.6: Migrate `NodeOptionsState` into `ActiveDialog::NodeOptions` (TDD: Same pattern; refactor `render_node_options_dialog`; update sidebar alt-click handler; remove `NodeOptionsState` resource)

- [ ] Task 4.7: Migrate `InviteDialogState` into `ActiveDialog::InviteDialog` (TDD: Same pattern; refactor `render_invite_dialog`; update sidebar header button; remove `InviteDialogState` resource)

- [ ] Task 4.8: Migrate `JoinDialogState` into `ActiveDialog::JoinDialog` (TDD: Same pattern; refactor `render_join_dialog`; update sidebar Join button; remove `JoinDialogState` resource)

- [ ] Task 4.9: Migrate `PeerDebugPanelState` into `ActiveDialog::PeerDebug` (TDD: Same pattern; refactor `render_peer_debug_panel`; update status bar peer count click; remove `PeerDebugPanelState` resource)

- [ ] Task 4.10: Remove old dialog resources from plugin, update `P2pDialogParams` (TDD: Verify all 7 `init_resource` calls for dialog states are gone; update `P2pDialogParams` to no longer carry individual dialog states; verify `gardener_console()` parameter count has decreased; `cargo test -p fe-ui`; `cargo clippy -p fe-ui -- -D warnings`)

- [ ] Verification: Run the app. Open every dialog type one at a time: right-click context menu, Create Verse, Import GLTF, alt-click Node Options, Generate Invite, Join, Peer Debug. Verify each opens and closes correctly. Verify opening one dialog while another is open replaces it (mutual exclusion). [checkpoint marker]

---

## Phase 5: Selection Deduplication

**Goal:** Remove duplicated selection fields from `InspectorState` and `SidebarState`. All selection reads go through `NodeManager`. Rename `InspectorState` to `InspectorFormState`.

**Files touched:** `plugin.rs`, `panels.rs`, `node_manager.rs`, `dialogs.rs`, `viewport.rs`

### Tasks

- [ ] Task 5.1: Remove `SidebarState.selected_node_id` and update sidebar rendering (TDD: Write test that sidebar node highlight reads from `NodeManager`; remove `selected_node_id` from `SidebarState`; update `render_nodes()` in `panels.rs` to accept `&NodeManager` and read `node_mgr.selected.as_ref().map(|s| &s.node_id)` for highlight; update `handle_viewport_click` in `node_manager.rs` to no longer write `sidebar.selected_node_id`)

- [ ] Task 5.2: Remove `InspectorState.selected_entity` and update inspector rendering (TDD: Write test that inspector open/close reads from `NodeManager`; remove `selected_entity` from `InspectorState`; update `right_inspector()` to accept selected entity from `NodeManager`; update `inspector_entity_section()` likewise; update toolbar deselect check to read from `NodeManager`)

- [ ] Task 5.3: Remove `InspectorState.selected_node_id` and update URL sync (TDD: Write test that `sync_manager_to_inspector` no longer copies `selected_node_id`; remove field; update `handle_url_save` / `UiAction::SaveUrl` to read `node_id` from `NodeManager` instead of `InspectorState`)

- [ ] Task 5.4: Simplify `sync_manager_to_inspector` (TDD: Test that the system only writes transform string buffers and URL; remove entity/node_id copy lines; verify inspector still populates on selection)

- [ ] Task 5.5: Move `opened_for_entity` tracking into `PortalState::Open` (TDD: Test that portal auto-close logic uses `PortalState::Open { entity }` from `UiManager`; verify that this was already done in Phase 3 Task 3.7; if not, migrate now; remove any remaining `opened_for_entity` fields)

- [ ] Task 5.6: Rename `InspectorState` to `InspectorFormState` (TDD: Grep for all usages of `InspectorState`, rename to `InspectorFormState`; update all imports; verify no remaining references to the old name; `cargo test -p fe-ui`; `cargo clippy -p fe-ui -- -D warnings`)

- [ ] Task 5.7: Audit `gardener_console()` parameter count and `P2pDialogParams` (TDD: Count parameters; assert fewer than 15; update doc comments; `cargo fmt -p fe-ui --check`)

- [ ] Verification: Run the app. Select a node via viewport click -- verify inspector opens with correct transform and URL. Select a different node via sidebar -- verify inspector updates. Click Deselect on toolbar -- verify inspector closes and sidebar highlight clears. Verify portal open/close/auto-close still works. Verify gimbal drag still commits transforms. Run `cargo tarpaulin -p fe-ui` and verify >80% coverage on changed files. [checkpoint marker]

---

## Summary

| Phase | FR | Systems Changed | Resources Added | Resources Removed | Est. Effort |
|-------|-----|----------------|-----------------|-------------------|-------------|
| 2 | FR-1 | 0 new, 13 reconfigured | 0 | 0 | 1-2 hours |
| 3 | FR-2 | 1 new (`process_ui_actions`), 3 removed | `UiManager` | `WebViewOpenRequest`, `PortalPanelState` | 3-4 hours |
| 4 | FR-3 | 0 new, 7 refactored (dialog renders) | `ActiveDialog` (or field on `UiManager`) | 7 dialog states | 3-4 hours |
| 5 | FR-4 | 1 refactored (`sync_manager_to_inspector`) | 0 | 3 fields removed from 2 resources | 2-3 hours |
| **Total** | | | | | **9-13 hours** |
