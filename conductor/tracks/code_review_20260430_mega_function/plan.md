# Implementation Plan: Refactor `apply_db_results` Mega-Function

## Overview

Extract each `DbResult` match arm into a standalone private function, leaving `apply_db_results` as a thin dispatcher. This is a pure refactor — no logic changes.

---

## Phase 1: Prepare Extraction

Goal: Understand dependencies and group variants by shared params.

Tasks:

- [ ] Task 1.1: Audit each match arm's parameter needs
  - Read `apply_db_results` and list every resource/system-param used by each arm.
  - Group arms into categories:
    - **Tree mutators** (need `verse_mgr`, sometimes `nav`): `HierarchyLoaded`, `GltfImported`, `NodeCreated`, `VerseCreated`, `FractalCreated`, `PetalCreated`, `EntityDeleted`, `EntityRenamed`
    - **Dialog drivers** (need `ui_mgr`, `inspector`): `VerseInviteGenerated`, `PeerRolesResolved`, `RoleAssigned`, `RoleRevoked`, `ScopedInviteGenerated`, `ApiTokenMinted`, `ApiTokenRevoked`, `ApiTokensListed`, `ScopedApiTokensListed`
    - **Nav + misc** (need `nav`, `db_sender`): `Seeded`, `DatabaseReset`, `VerseJoined`
    - **Simple logging**: `VerseDefaultAccessSet`, `FractalDescriptionUpdated`, `LocalRoleResolved`, `Error`
  - Document the minimal param set each handler needs.

- [ ] Task 1.2: Write a dispatcher-only smoke test (TDD Red)
  - Create a test that verifies the dispatcher calls the right handler for each `DbResult` variant.
  - This test will be modified as we extract functions — it ensures we don't drop any arms.
  - Confirm the test compiles (it will be updated throughout the refactor).

---

## Phase 2: Extract Tree Mutator Handlers

Goal: Extract arms that mutate `VerseManager` / `NavigationManager`.

Tasks:

- [ ] Task 2.1: Extract `handle_hierarchy_loaded`
  - Signature: `fn handle_hierarchy_loaded(verses: &[VerseHierarchyData], verse_mgr: &mut VerseManager, nav: &mut NavigationManager, commands: &mut Commands, asset_server: &AssetServer)`
  - Move the ~80-line `HierarchyLoaded` arm into this function.
  - Call it from the match in `apply_db_results`.
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.2: Extract `handle_gltf_imported`
  - Signature: `fn handle_gltf_imported(node_id, name, petal_id, asset_path, position, verse_mgr, nav, commands, asset_server)`
  - Move the arm body.
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.3: Extract `handle_entity_created` handlers
  - `handle_verse_created`, `handle_fractal_created`, `handle_petal_created`, `handle_node_created`
  - Each is small (5–15 lines). Extract individually or as a grouped helper.
  - Run `cargo test -p fe-ui`.

- [ ] Task 2.4: Extract `handle_entity_renamed` and `handle_entity_deleted`
  - These mutate both `verse_mgr` and `nav`.
  - Run `cargo test -p fe-ui`.

---

## Phase 3: Extract Dialog Driver Handlers

Goal: Extract arms that drive `UiManager` / `InspectorFormState`.

Tasks:

- [ ] Task 3.1: Extract `handle_verse_invite_generated`
  - Signature: `fn handle_verse_invite_generated(invite_string, include_write_cap, expiry_hours, ui_mgr)`
  - Run `cargo test -p fe-ui`.

- [ ] Task 3.2: Extract role + invite handlers
  - `handle_peer_roles_resolved`, `handle_role_assigned`, `handle_role_revoked`, `handle_scoped_invite_generated`
  - Run `cargo test -p fe-ui`.

- [ ] Task 3.3: Extract API token handlers
  - `handle_api_token_minted`, `handle_api_token_revoked`, `handle_api_tokens_listed`, `handle_scoped_api_tokens_listed`
  - Each touches both `ui_mgr.active_dialog` and `inspector` fields.
  - Run `cargo test -p fe-ui`.

---

## Phase 4: Extract Remaining Simple Handlers

Goal: Extract misc arms and final cleanup.

Tasks:

- [ ] Task 4.1: Extract `handle_seeded`, `handle_database_reset`, `handle_verse_joined`
  - These send follow-up `DbCommand`s. Extract with `db_sender` param.
  - Run `cargo test -p fe-ui`.

- [ ] Task 4.2: Extract logging-only handlers
  - `handle_error`, `handle_verse_default_access_set`, `handle_fractal_description_updated`, `handle_local_role_resolved`
  - These are one-liners — extract for consistency or leave inline with a comment.
  - Run `cargo test -p fe-ui`.

- [ ] Task 4.3: Final dispatcher cleanup
  - Ensure `apply_db_results` is now a thin match block (~60 lines max).
  - Remove any unused imports.
  - Run `cargo fmt --check` and `cargo clippy -- -D warnings -p fe-ui`.

---

## Phase 5: Verification

Tasks:

- [ ] Task 5.1: Run full test suite
  - `cargo test -p fe-ui`
  - All existing tests must pass unchanged.

- [ ] Task 5.2: Line count check
  - `wc -l fe-ui/src/verse_manager.rs` — should be reduced by ~200–250 lines due to function signature overhead.
  - `wc -l` on `apply_db_results` itself — should be < 80 lines.

- [ ] Verification: Phase 5 Final Verification [checkpoint marker]
  - `cargo test` (all crates) passes.
  - `cargo clippy -- -D warnings -p fe-ui` passes.
  - `cargo fmt --check` passes.
  - `apply_db_results` is under 80 lines.
  - Each `DbResult` variant has its own handler function.

---

## Completion Criteria

- [ ] `apply_db_results` is a thin dispatcher (< 80 lines).
- [ ] Each `DbResult` variant handled by a named private function.
- [ ] All existing tests pass.
- [ ] `cargo clippy -- -D warnings -p fe-ui` passes.
