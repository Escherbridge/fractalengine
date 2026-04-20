# Implementation Plan: Portal URL Persistence + Inspector Settings

## Overview

Four phases, progressing from foundational persistence to full hierarchical inspection and access control. Phase 1 completes the URL persistence round-trip. Phase 2 adds the tab system to the inspector. Phase 3 extends inspection to all hierarchy levels. Phase 4 builds the access control UI.

---

## Phase 1: Portal URL Persistence

Goal: Ensure portal URLs survive application restarts by completing the load-on-select and save-on-click round-trip through SurrealDB.

Tasks:

- [ ] Task 1.1: Write URL load-on-selection test (TDD Red)
  - Write a test in `fe-ui` that asserts: when `NodeManager.selected` changes to a node with a stored URL, `InspectorFormState.external_url` is populated with that URL.
  - Write a test that asserts: when a node with no URL is selected, `InspectorFormState.external_url` is empty.
  - Confirm tests fail (current code may not fully populate on selection).

- [ ] Task 1.2: Implement URL loading on node selection (TDD Green)
  - In the system that handles node selection changes (likely in `node_manager.rs` or `inspector` update code), read the selected node's URL from `VerseManager` and write it to `InspectorFormState.external_url`.
  - Ensure `VerseManager` caches URLs loaded from the initial petal data fetch.
  - Run tests, confirm green.

- [ ] Task 1.3: Write URL save persistence test (TDD Red)
  - Write a test that asserts: `UiAction::SaveUrl` processing sends `DbCommand::UpdateNodeUrl` to the database channel.
  - Write a test that asserts: saving an empty URL sends `None` as the URL value.
  - Confirm tests fail or pass (verify existing implementation in `process_ui_actions`).

- [ ] Task 1.4: Verify and fix save-to-DB path (TDD Green)
  - Verify that `process_ui_actions` correctly sends `DbCommand::UpdateNodeUrl` for `UiAction::SaveUrl`. (Based on the current code reading, this appears to already be implemented -- confirm and add tests.)
  - Verify the database handler (`fe-runtime`) processes `UpdateNodeUrl` and persists to SurrealDB.
  - If gaps exist, implement the missing pieces.
  - Run tests, confirm green.

- [ ] Task 1.5: Write round-trip integration test (TDD Red -> Green)
  - Write an integration test (in `tests/` at the workspace or crate level) that:
    1. Saves a URL via the DB command channel.
    2. Simulates a node selection.
    3. Asserts the saved URL is loaded into the inspector form.
  - This may require a mock DB or in-memory SurrealDB instance.

- [ ] Task 1.6: Refactor and add URL validation feedback
  - Add user-visible feedback when a blocked URL is rejected on save (e.g., set a validation state field on `InspectorFormState` that the egui renderer shows as red text).
  - Ensure empty strings are normalized to `None` before persistence.
  - Run `cargo clippy` and `cargo fmt --check`.

- [ ] Verification: Phase 1 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application, select a node, enter a URL, click Save.
  - Restart the application, select the same node -- confirm URL is displayed.
  - Enter a blocked URL (e.g., `http://192.168.1.1`), click Save -- confirm rejection feedback.
  - Enter an empty URL, click Save -- confirm previous URL is cleared.

---

## Phase 2: Inspector Tab System

Goal: Add Info/Settings/Access tabs to the inspector panel.

Tasks:

- [ ] Task 2.1: Define InspectorTab enum and state (TDD Red)
  - Write tests for a new `InspectorTab` enum: `Info`, `Settings`, `Access`.
  - Write tests for tab state management: default is `Info`, tab switches correctly, resets to `Info` on entity change.
  - Confirm tests fail (enum doesn't exist yet).

- [ ] Task 2.2: Implement InspectorTab enum and state resource (TDD Green)
  - Create `InspectorTab` enum in `fe-ui/src/plugin.rs` (or a new `inspector.rs` module).
  - Add `active_tab: InspectorTab` field to `InspectorFormState` (or create a separate `InspectorTabState` resource).
  - Implement tab reset logic: reset to `Info` when the selected entity changes.
  - Run tests, confirm green.

- [ ] Task 2.3: Render tab bar in inspector panel (TDD: implement + visual test)
  - Modify the inspector panel rendering code to add a horizontal tab bar using `egui::SelectableLabel` or similar.
  - The Info tab renders existing content (transform fields, URL fields).
  - Settings tab renders placeholder text.
  - Access tab renders placeholder text.
  - Tab clicks update `InspectorTabState`.

- [ ] Task 2.4: Write tab interaction tests (TDD Red -> Green)
  - Write tests that: clicking Settings tab changes state, Info content is not rendered when Settings is active, tab resets to Info on deselection.
  - Implement any fixes needed.

- [ ] Task 2.5: Refactor inspector rendering for modularity
  - Extract each tab's content into separate functions: `render_info_tab()`, `render_settings_tab()`, `render_access_tab()`.
  - This prepares for Phase 3 where each hierarchy level has different tab content.
  - Run tests.

- [ ] Verification: Phase 2 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application, select a node.
  - Verify three tabs appear in the inspector.
  - Click each tab -- confirm content switches.
  - Deselect and reselect -- confirm tab resets to Info.

---

## Phase 3: Inspectable Hierarchy Levels

Goal: Allow inspecting petals, fractals, and verses from the sidebar, each with their own Info/Settings/Access tabs.

Tasks:

- [ ] Task 3.1: Define InspectedEntity enum (TDD Red)
  - Write tests for `InspectedEntity` enum with variants: `Node { id }`, `Petal { id }`, `Fractal { id }`, `Verse { id }`.
  - Write tests: setting `InspectedEntity::Petal` should make the inspector render petal-specific content.
  - Confirm tests fail.

- [ ] Task 3.2: Implement InspectedEntity and selection routing (TDD Green)
  - Create `InspectedEntity` enum.
  - Add `inspected: Option<InspectedEntity>` to `UiManager` or `InspectorFormState`.
  - Modify sidebar click handlers to set `InspectedEntity` when a petal/fractal/verse name is clicked.
  - Node selection (existing `NodeManager.selected`) continues to set `InspectedEntity::Node`.
  - Run tests, confirm green.

- [ ] Task 3.3: Implement petal inspection Info tab (TDD Red -> Green)
  - Write tests: when `InspectedEntity::Petal` is active, Info tab shows petal name, ID, node count.
  - Implement `render_petal_info_tab()` function.
  - Pull petal data from `VerseManager` (which already tracks the hierarchy).
  - Run tests.

- [ ] Task 3.4: Implement fractal inspection Info tab (TDD Red -> Green)
  - Write tests: when `InspectedEntity::Fractal` is active, Info tab shows fractal name, ID, petal count.
  - Implement `render_fractal_info_tab()` function.
  - Run tests.

- [ ] Task 3.5: Implement verse inspection Info tab (TDD Red -> Green)
  - Write tests: when `InspectedEntity::Verse` is active, Info tab shows verse name, ID, fractal count, peer count.
  - Implement `render_verse_info_tab()` function.
  - Run tests.

- [ ] Task 3.6: Update inspector header to show entity type
  - Modify the inspector panel header to display the entity type and name (e.g., "Node: MyModel", "Petal: MainWorld").
  - Derive the display name from the `InspectedEntity` variant.
  - Run tests.

- [ ] Task 3.7: Refactor and polish
  - Ensure Settings and Access tabs show appropriate placeholder text per hierarchy level.
  - Ensure tab state resets when switching between hierarchy levels.
  - Run `cargo clippy` and `cargo fmt --check`.

- [ ] Verification: Phase 3 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - Launch the application.
  - Click a petal in the sidebar -- confirm inspector shows petal info.
  - Click a fractal -- confirm inspector shows fractal info.
  - Click a verse -- confirm inspector shows verse info.
  - Click a node -- confirm inspector shows node info with transform/URL fields.
  - Switch between entities -- confirm tab resets and header updates.

---

## Phase 4: Peer Management UI (Access Tab)

Goal: Build dedicated peer management surfaces at every hierarchy level with hierarchical role resolution, invite generation, and audit trails.

**Depends on:** Shared Peer Infrastructure track (RoleManager, PeerRegistry, RoleLevel, scope utilities, invite system).

Tasks:

- [ ] Task 4.1: Define AccessTabState with hierarchy awareness (TDD Red)
  - Write tests for:
    - `AccessTabState` that derives its view from `RoleManager::get_scope_members(scope)` and `PeerRegistry`.
    - Each peer entry shows: `peer_did`, `display_name` (from PeerRegistry), `explicit_role` (at current scope), `effective_role` (resolved via cascade), `is_online`.
    - Sorting: owner first, then by effective role level, then alphabetically.
    - `AccessTabState` tracks the current scope (built via `build_scope` from the `InspectedEntity`).
  - Confirm tests fail.

- [ ] Task 4.2: Implement AccessTabState (TDD Green)
  - Implement `AccessTabState` that reads from `RoleManager` and `PeerRegistry`.
  - Build the scope string from the current `InspectedEntity` (verse/fractal/petal).
  - Resolve both explicit and effective roles for each peer.
  - Admin determination uses `LocalUserRole` resource (hierarchical resolution).
  - Run tests, confirm green.

- [ ] Task 4.3: Render verse-level Access tab (TDD: implement + visual test)
  - Full member list with verse-level roles and online indicators.
  - Owner controls: role dropdowns (manager/editor/viewer), revoke button, invite generation.
  - Default access toggle: "New members can view everything" / "New members need explicit grants".
  - Verse owner row shows "owner" badge, no dropdown.
  - Non-owner members see read-only list.
  - Role changes go through `RoleManager::assign_role` with scope `VERSE#<id>`.
  - Run tests.

- [ ] Task 4.4: Render fractal-level Access tab (TDD: implement + visual test)
  - Peer list showing "Explicit Role" and "Effective Role" columns.
  - Inherited verse roles shown dimmed with "(inherited from verse)" label.
  - Manager/owner controls: role dropdowns (manager/editor/viewer/none), invite generation.
  - `none` role option explicitly blocks inherited access for that peer.
  - Verse owner row always shows "owner", cannot be overridden.
  - Role changes go through `RoleManager::assign_role` with scope `VERSE#<id>-FRACTAL#<id>`.
  - Run tests.

- [ ] Task 4.5: Render petal-level Access tab (TDD: implement + visual test)
  - Peer list showing "Explicit Role" and "Effective Role" columns.
  - Inherited roles shown dimmed with "(inherited from fractal)" or "(inherited from verse)" label.
  - Manager/owner controls: role dropdowns (manager/editor/viewer/none), invite generation.
  - Role changes go through `RoleManager::assign_role` with scope `VERSE#<id>-FRACTAL#<id>-PETAL#<id>`.
  - Run tests.

- [ ] Task 4.6: Implement invite link UI (TDD Red -> Green)
  - Write tests: "Generate Invite" button calls `RoleManager::generate_invite` with correct scope and role.
  - Implement invite dialog: role dropdown, expiry dropdown (1h/24h/7d/30d/never), generated link with "Copy" button.
  - Privilege check: only users with manager/owner at current scope see the "Generate Invite" button.
  - Invite scope is built from current `InspectedEntity` via `build_scope`.
  - Run tests.

- [ ] Task 4.7: Implement permission presets (TDD Red -> Green)
  - Write tests: clicking "Public" preset calls `RoleManager::assign_role` for all peers with viewer role.
  - Implement three preset buttons: "Public", "Members Only", "Admin Only".
  - Each preset modifies role assignments at the current scope via `RoleManager`.
  - Run tests.

- [ ] Task 4.8: Write role change audit trail (TDD Red -> Green)
  - Write tests: changing a peer's role writes an OpType::AssignRole entry to the op_log with scope in payload.
  - Implement: `RoleManager::assign_role` already writes to op_log — verify the scope string is included.
  - Add "History" link to Access tab that queries recent op_log entries for the current scope.
  - Run tests.

- [ ] Task 4.9: Refactor and polish
  - Add loading state indicator while peer roles are being fetched.
  - Add error handling for failed role changes.
  - Ensure Access tab updates reactively when roles change (not requiring manual refresh).
  - Run `cargo clippy` and `cargo fmt --check`.

- [ ] Verification: Phase 4 Manual Verification [checkpoint marker]
  - Run `cargo test`.
  - **Verse Access:** Select a verse, open Access tab. Verify full member list. As owner, change a peer's role, generate an invite link, toggle default access.
  - **Fractal Access:** Select a fractal, open Access tab. Verify explicit + inherited roles displayed. Set a peer to "none" — verify they lose access at this fractal while retaining verse-level access.
  - **Petal Access:** Select a petal, open Access tab. Verify the full inheritance chain displayed. Change a peer's petal-level role — verify effective role updates.
  - **Invite:** Generate invite links at each level. Verify the scope is correctly encoded in the link.
  - **Presets:** Click each permission preset — verify roles update accordingly.
  - **Audit:** Verify role changes appear in the op_log with scope string in payload.
  - **Read-only:** As a non-manager peer, verify the Access tab is read-only (no dropdowns, no invite button).
