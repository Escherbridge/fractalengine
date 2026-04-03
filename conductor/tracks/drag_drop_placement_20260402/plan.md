# Implementation Plan: Drag & Drop Asset Placement -- File Drop + Scene Placement Flow

## Overview

This plan is divided into 6 phases, progressing from state management foundations, through drop zone feedback and ingestion, to the placement preview system, click-to-place confirmation, Alt+Drag duplication, and the Asset Library panel. Each phase ends with a verification checkpoint. The approach follows strict TDD: write failing tests first, implement to pass, then refactor.

**Dependency note:** This track depends on Bloom Stage (ground plane, orbit camera, raycasting) and Petal Seed (DbCommand/DbResult variants, AssetRegistry, poll_db_results). If those tracks are incomplete, Phases 3-5 will need stubs for the ground plane raycast and DbCommand channel.

**Crates touched:** `fe-runtime` (messages, resources), `fe-renderer` (preview spawning, material manipulation), `fe-ui` (overlay, asset library panel), `fe-database` (asset queries).

---

## Phase 1: Placement State Machine and Resources

Goal: Define the `PlacementMode` state machine, `DragHoverActive` flag, and supporting types so all subsequent phases have a clean state foundation.

Tasks:

- [ ] Task 1.1: Define `PlacementMode` enum resource (TDD: Write test asserting `PlacementMode::default()` is `Inactive`. Write test constructing `Previewing { asset_id, preview_entity }` and `Duplicating { source_asset_id, preview_entity }` variants. Test `Debug` and `Clone` derive. Run tests, confirm red. Implement as `#[derive(Resource, Debug, Clone)]` enum in `fe-renderer/src/placement.rs`. Confirm green.)

- [ ] Task 1.2: Define `PlacementPreview` marker component (TDD: Write test that constructs `PlacementPreview` and verifies it derives `Component`. Run test, confirm red. Implement as `#[derive(Component, Default)]` in `fe-renderer/src/placement.rs`. Confirm green.)

- [ ] Task 1.3: Define `DragHoverActive` resource (TDD: Write test that `DragHoverActive::default()` is `DragHoverActive(false)`. Test toggling to true. Run test, confirm red. Implement as `#[derive(Resource, Default)] pub struct DragHoverActive(pub bool)` in `fe-renderer/src/placement.rs`. Confirm green.)

- [ ] Task 1.4: Define `PlacementMode` helper methods (TDD: Write test `is_active()` returns false for `Inactive`, true for `Previewing` and `Duplicating`. Write test `asset_id()` returns `Some` for `Previewing`/`Duplicating`, `None` for `Inactive`. Write test `preview_entity()` returns the entity for active states. Run tests, confirm red. Implement methods. Confirm green.)

- [ ] Task 1.5: Add `DbCommand::CreateAsset` variant (TDD: Write test constructing `DbCommand::CreateAsset { asset_id, name, content_type, size_bytes }` with clone + debug. Channel roundtrip test. Run tests, confirm red. Add variant to `fe-runtime/src/messages.rs`. Confirm green.)

- [ ] Task 1.6: Add `DbResult::AssetCreated` variant (TDD: Write test constructing `DbResult::AssetCreated { asset_id }` with clone + debug. Channel roundtrip test. Run tests, confirm red. Add variant. Confirm green.)

- [ ] Verification: Run `cargo test -p fe-renderer -p fe-runtime`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. [checkpoint marker]

---

## Phase 2: Drop Zone Overlay and File Event Handling

Goal: Detect file drag hover and drop events from winit, show a visual overlay during hover, and trigger async file ingestion on drop.

Tasks:

- [ ] Task 2.1: Implement `handle_file_hover` system (TDD: Write test that simulates `FileDragAndDrop::HoveredFile` event, calls the system, asserts `DragHoverActive` is set to true. Write test for `HoveredFileCancelled` setting it to false. Write test for `DroppedFile` setting it to false. Run tests, confirm red. Implement system. Confirm green.)

- [ ] Task 2.2: Implement drop zone overlay UI (TDD: Write test that calls `drop_zone_overlay(ctx, true)` and verifies no panic. Write test calling with `false` and verifying no overlay is rendered. Use egui `Context::default()` + `ctx.run()` test harness. Run tests, confirm red. Implement `drop_zone_overlay(ctx: &egui::Context, active: bool)` in `fe-ui/src/drop_zone.rs` rendering a semi-transparent `Area` with centered text "Drop GLB to add to scene". Confirm green.)

- [ ] Task 2.3: Wire drop zone overlay into `gardener_console` (Update `gardener_console` to accept `DragHoverActive` and call `drop_zone_overlay`. Ensure `cargo build` and `cargo test` pass. No separate TDD cycle -- wiring task.)

- [ ] Task 2.4: Implement async file ingestion task spawner (TDD: Write test that given a path to a temp file with valid GLB magic bytes, the ingestion function returns `Ok(AssetId)`. Write test with invalid magic returning `Err`. Write test with oversized bytes returning `Err`. Run tests, confirm red. Implement `ingest_dropped_file(path: &Path) -> anyhow::Result<(AssetId, String, usize)>` that reads bytes, calls `GltfIngester::ingest()`, returns asset_id + filename + size. Confirm green.)

- [ ] Task 2.5: Implement `handle_file_drop` system (TDD: Write test that simulates `FileDragAndDrop::DroppedFile` with a valid GLB temp file path, calls the system, asserts a `Task` is spawned on `AsyncComputeTaskPool`. Write test with no `active_petal_id` set, assert no task spawned. Run tests, confirm red. Implement system that reads `DroppedFile`, checks `NavigationState.active_petal_id`, spawns async ingestion task. Confirm green.)

- [ ] Task 2.6: Implement `poll_ingestion_tasks` system (TDD: Write test that given a completed ingestion task returning `Ok(asset_id, filename, size)`, the system sends `DbCommand::CreateAsset` and enters `PlacementMode::Previewing`. Write test for a failed task that logs error and does not enter placement mode. Run tests, confirm red. Implement system using `PendingIngestionTasks` resource holding `Vec<Task<Result<...>>>`. Confirm green.)

- [ ] Verification: Run `cargo test -p fe-renderer -p fe-ui -p fe-runtime`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: launch app, drag a file over the window, confirm overlay appears and disappears. [checkpoint marker]

---

## Phase 3: Ground Plane Raycast and Placement Preview

Goal: Implement the cursor-to-ground-plane raycast, spawn a translucent preview entity, and update its position every frame.

Tasks:

- [ ] Task 3.1: Implement `cursor_to_ground_plane` pure function (TDD: Write test with a `Ray3d` pointing straight down from (0, 10, 0), assert hit point is (0, 0, 0). Write test with ray at 45-degree angle, assert correct XZ intersection. Write test with ray pointing upward (parallel or away from ground), assert `None`. Write test with ray parallel to ground plane, assert `None`. Run tests, confirm red. Implement as `cursor_to_ground_plane(ray: Ray3d) -> Option<Vec3>` computing `t = -ray.origin.y / ray.direction.y` with guard for `t <= 0` and direction.y ~= 0. Confirm green.)

- [ ] Task 3.2: Implement `viewport_cursor_ray` helper (TDD: Write test that given a known camera Transform and Projection, and a cursor position at viewport center, the returned ray points forward from the camera. Run test, confirm red. Implement using `Camera::viewport_to_world()`. Confirm green.)

- [ ] Task 3.3: Implement preview entity spawning (TDD: Write test that when `PlacementMode` transitions from `Inactive` to `Previewing`, a new entity is spawned with `PlacementPreview` component and `SceneRoot`. Write test that the spawned entity has `PickingBehavior::IGNORE` to prevent selection interference. Run tests, confirm red. Implement `spawn_placement_preview` system. Confirm green.)

- [ ] Task 3.4: Implement preview material override (TDD: Write test that entities with `PlacementPreview` marker have their `StandardMaterial` children set to `alpha_mode: AlphaMode::Blend` and `base_color.a = 0.5`. Run test, confirm red. Implement `apply_preview_materials` system that traverses `Children` of `PlacementPreview` entities and overrides materials. Confirm green.)

- [ ] Task 3.5: Implement `update_preview_position` system (TDD: Write test that given an active `PlacementMode::Previewing`, a mock cursor position, and a camera, the `PlacementPreview` entity's Transform is updated to the raycast hit point. Write test that when raycast misses, the preview entity's `Visibility` is set to `Hidden`. Run tests, confirm red. Implement system using `cursor_to_ground_plane`. Confirm green.)

- [ ] Task 3.6: Implement toolbar placement indicator (TDD: Write test that when `PlacementMode` is `Previewing`, the toolbar shows "Placing..." indicator. Write test that when `Inactive`, the indicator is hidden. Run tests, confirm red. Add placement indicator to `top_toolbar` function in `fe-ui/src/panels.rs`. Confirm green.)

- [ ] Verification: Run `cargo test -p fe-renderer -p fe-ui`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: drop a .glb file, confirm translucent preview appears and follows cursor over ground plane. [checkpoint marker]

---

## Phase 4: Click to Place and Escape to Cancel

Goal: Implement placement confirmation via left-click and cancellation via Escape. Wire up the full file-drop-to-placed-entity pipeline.

Tasks:

- [ ] Task 4.1: Implement `confirm_placement` system (TDD: Write test that when `PlacementMode::Previewing` and left mouse button is pressed, the preview entity is despawned and a new permanent entity is spawned at the preview position. Write test that a `DbCommand::PlaceModel` is sent with correct asset_id and transform. Write test that `PlacementMode` transitions to `Inactive`. Run tests, confirm red. Implement system. Confirm green.)

- [ ] Task 4.2: Implement `cancel_placement` system (TDD: Write test that when `PlacementMode::Previewing` and Escape is pressed, the preview entity is despawned. Write test that `PlacementMode` transitions to `Inactive`. Write test that no `DbCommand::PlaceModel` is sent. Run tests, confirm red. Implement system. Confirm green.)

- [ ] Task 4.3: Implement transform builder for placement (TDD: Write test that `build_placement_transform(Vec3::new(5.0, 0.0, -3.0))` returns JSON with translation [5.0, 0.0, -3.0], rotation [0, 0, 0, 1], scale [1, 1, 1]. Run test, confirm red. Implement `build_placement_transform(position: Vec3) -> serde_json::Value`. Confirm green.)

- [ ] Task 4.4: Implement click debounce guard (TDD: Write test that rapid double-click during `Previewing` produces only one `PlaceModel` command. Run test, confirm red. Implement by transitioning `PlacementMode` to `Inactive` immediately on first click, so second click sees `Inactive` and does nothing. Confirm green.)

- [ ] Task 4.5: Wire `CreateAsset` handling in DB thread command loop (TDD: Write async test that sends `DbCommand::CreateAsset`, verifies the DB thread creates an asset record and responds with `DbResult::AssetCreated`. Write test for duplicate asset_id using upsert behavior. Run tests, confirm red. Implement the match arm in the DB command loop calling a new `create_or_upsert_asset` query. Confirm green.)

- [ ] Task 4.6: Implement `create_or_upsert_asset` query (TDD: Write async test creating an asset, then upserting the same asset_id, asserting only one record exists. Run test, confirm red. Implement using `INSERT ... ON DUPLICATE KEY UPDATE` or SurrealDB equivalent. Confirm green.)

- [ ] Task 4.7: Register all Phase 2-4 systems in the Bevy app plugin (Wire `handle_file_hover`, `handle_file_drop`, `poll_ingestion_tasks`, `spawn_placement_preview`, `apply_preview_materials`, `update_preview_position`, `confirm_placement`, `cancel_placement` into the system schedule with correct ordering. Ensure `cargo build` passes. Add smoke test that the plugin registers without panic.)

- [ ] Task 4.8: End-to-end integration test for drop-to-place pipeline (Write a test that: creates a temp GLB file with valid magic bytes, simulates `FileDragAndDrop::DroppedFile`, verifies `PlacementMode` transitions to `Previewing`, simulates left-click, verifies `DbCommand::PlaceModel` is produced with correct asset_id and raycast-derived position, verifies `PlacementMode` returns to `Inactive`.)

- [ ] Verification: Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: drop a .glb file, see preview, move cursor, click to place, confirm model appears permanently. Press Escape during preview, confirm it cancels cleanly. [checkpoint marker]

---

## Phase 5: Alt+Drag Duplication

Goal: Implement entity duplication via Alt+Drag on a selected entity.

Tasks:

- [ ] Task 5.1: Implement `detect_alt_drag` system (TDD: Write test that when an entity is selected via `InspectorState.selected_entity`, Alt key is held, and left mouse button is pressed with 5+ pixel drag, the system enters `PlacementMode::Duplicating`. Write test that without Alt held, no duplication starts. Write test that with no selection, no duplication starts. Write test that drag less than 5px does not trigger. Run tests, confirm red. Implement system. Confirm green.)

- [ ] Task 5.2: Reuse preview spawning for duplication (TDD: Write test that `PlacementMode::Duplicating` triggers the same preview entity spawn as `Previewing`. Write test that the preview entity gets the source entity's asset_id. Run tests, confirm red. Modify `spawn_placement_preview` to handle both `Previewing` and `Duplicating` variants. Confirm green.)

- [ ] Task 5.3: Implement duplication confirm on mouse release (TDD: Write test that when `PlacementMode::Duplicating` and left mouse button is released, a permanent entity is spawned and `DbCommand::PlaceModel` is sent. Write test that `PlacementMode` returns to `Inactive`. Run tests, confirm red. Implement by extending `confirm_placement` system to also handle `Duplicating` with mouse-up trigger instead of mouse-down. Confirm green.)

- [ ] Task 5.4: Implement duplication cancel on Escape (TDD: Write test that Escape during `Duplicating` despawns the preview and returns to `Inactive`. Run tests, confirm red. Verify `cancel_placement` already handles `Duplicating` or extend it. Confirm green.)

- [ ] Verification: Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: select an entity, hold Alt and drag, see preview, release to place duplicate. Press Escape to cancel. [checkpoint marker]

---

## Phase 6: Asset Library Panel

Goal: Build the sidebar Asset Library panel that lists all ingested assets and allows re-entering placement mode by clicking an entry.

Tasks:

- [ ] Task 6.1: Add `list_assets` query for the asset table (TDD: Write async test that creates 3 asset records via `create_or_upsert_asset`, calls `list_assets`, asserts 3 entries returned with correct fields (asset_id, name, size_bytes). Write test for empty result. Run tests, confirm red. Implement `list_assets(db) -> Vec<AssetSummary>` querying `SELECT asset_id, name, size_bytes FROM asset`. Confirm green.)

- [ ] Task 6.2: Define `AssetSummary` struct (TDD: Write test that `AssetSummary` has fields `asset_id: String`, `name: String`, `size_bytes: u64`. Test `Debug`, `Clone`, serde round-trip. Run tests, confirm red. Implement in `fe-runtime/src/messages.rs`. Confirm green.)

- [ ] Task 6.3: Add `DbCommand::ListAllAssets` and `DbResult::AllAssetsList` variants (TDD: Channel roundtrip tests. Run tests, confirm red. Add variants. Confirm green.)

- [ ] Task 6.4: Wire `ListAllAssets` in DB thread command loop (TDD: Write test sending `DbCommand::ListAllAssets`, verify response `DbResult::AllAssetsList` with correct entries. Run test, confirm red. Implement match arm. Confirm green.)

- [ ] Task 6.5: Define `AssetLibrary` Bevy resource (TDD: Write test that `AssetLibrary::default()` has empty `assets: Vec<AssetSummary>`. Test adding entries. Run test, confirm red. Implement as `#[derive(Resource, Default)]`. Confirm green.)

- [ ] Task 6.6: Implement `poll_asset_library_results` system (TDD: Write test that when `DbResult::AllAssetsList` is received, `AssetLibrary` resource is updated. Write test that `DbResult::AssetCreated` triggers a refresh via `DbCommand::ListAllAssets`. Run tests, confirm red. Implement system. Confirm green.)

- [ ] Task 6.7: Implement `asset_library_panel` UI function (TDD: Write test calling `asset_library_panel(ui, &asset_library, &mut placement_mode)` with 3 entries, verify no panic. Write test with empty library showing "No assets" message. Write test that clicking an entry sets `PlacementMode::Previewing`. Run tests, confirm red. Implement in `fe-ui/src/asset_library.rs`. Confirm green.)

- [ ] Task 6.8: Implement `format_file_size` helper (TDD: Write test `format_file_size(1024)` returns "1.0 KB". Test `format_file_size(1_048_576)` returns "1.0 MB". Test `format_file_size(500)` returns "500 B". Run tests, confirm red. Implement. Confirm green.)

- [ ] Task 6.9: Integrate asset library panel into left sidebar (Update `left_sidebar` in `fe-ui/src/panels.rs` to include `sidebar_section_asset_library` calling `asset_library_panel`. Wire `AssetLibrary` and `PlacementMode` into `gardener_console` signature. Ensure `cargo build` and `cargo test` pass.)

- [ ] Task 6.10: Send initial `ListAllAssets` on app startup (TDD: Write test that on first frame after plugin registration, a `DbCommand::ListAllAssets` is sent on the channel. Run test, confirm red. Implement as a startup system or `run_once` system. Confirm green.)

- [ ] Verification: Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: drop several .glb files, open Asset Library in sidebar, confirm all appear. Click an entry, confirm placement preview starts. [checkpoint marker]

---

## Cross-Phase Notes

### Test Harness Pattern

For Bevy system tests, use `MinimalPlugins` plus the relevant plugin(s). Avoid `DefaultPlugins` in unit tests to prevent windowing/GPU dependencies. For egui panel tests, use `egui::Context::default()` with `ctx.run()`.

### Async Task Testing

Ingestion tasks run on `AsyncComputeTaskPool`. For unit tests, create temp files with known GLB magic bytes (`[0x67, 0x6C, 0x54, 0x46]` followed by padding). Use `tempfile` crate for cleanup.

### Raycast Testing

The `cursor_to_ground_plane` function is pure math and testable without Bevy. The `viewport_cursor_ray` helper requires a `Camera` component but can be tested with a minimal `App` and synthetic cursor coordinates.

### Material Override Testing

Preview material overrides require `Assets<StandardMaterial>`. Use Bevy's `AssetPlugin::default()` in tests that need material manipulation. For unit tests of the override logic, test the pure function that modifies `StandardMaterial` fields.

### DB Channel Tests

For tasks that write to the DB channel, assert the `DbCommand` variant received by the channel receiver rather than asserting DB state. Full DB round-trip tests belong in `fe-database` integration tests.

### System Ordering

Systems must run in this order within a frame:
1. `handle_file_hover` (reads events, sets `DragHoverActive`)
2. `handle_file_drop` (reads events, spawns tasks)
3. `poll_ingestion_tasks` (polls tasks, enters placement mode)
4. `detect_alt_drag` (reads input, enters duplication mode)
5. `spawn_placement_preview` (spawns preview if mode just changed)
6. `apply_preview_materials` (overrides materials on new previews)
7. `update_preview_position` (raycasts, moves preview)
8. `confirm_placement` / `cancel_placement` (reads input, transitions state)
9. `poll_asset_library_results` (updates AssetLibrary resource)

Use Bevy `SystemSet` or explicit `.before()`/`.after()` ordering.
