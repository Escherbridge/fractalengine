# Implementation Plan: Petal Seed — GLTF Drag-and-Drop & Asset Seeding

## Overview

This plan is divided into 4 phases, progressing from data model changes (channel types), through database handling, to the Bevy systems that tie everything together. Each phase ends with a verification checkpoint. The approach follows strict TDD: write failing tests first, implement to pass, then refactor.

**Dependency note:** This track touches `fe-runtime` (messages), `fe-database` (queries), `fe-renderer` (loader interaction), and `fe-ui` (panels). Phase 1 is pure data types with no external dependencies. Phases 2-4 build on Phase 1.

---

## Phase 1: Channel Types and Data Model

Goal: Extend `DbCommand`, `DbResult`, and define `AssetInfo` so the Bevy and DB threads can communicate about model placement and asset listing.

Tasks:

- [ ] Task 1.1: Define `AssetInfo` struct in `fe-runtime/src/messages.rs` (TDD: Write test asserting `AssetInfo` has `asset_id`, `petal_id`, `transform` fields, derives `Debug`, `Clone`, and can serialize/deserialize via `serde_json`. Run test, confirm red. Implement struct. Confirm green.)

- [ ] Task 1.2: Add `DbCommand::PlaceModel` variant (TDD: Write test that constructs `DbCommand::PlaceModel { petal_id, asset_id, transform }`, clones it, and debug-formats it. Write channel roundtrip test sending `PlaceModel` through a bounded crossbeam channel. Run tests, confirm red. Add variant. Confirm green.)

- [ ] Task 1.3: Add `DbCommand::ListAssets` variant (TDD: Write test constructing `DbCommand::ListAssets { petal_id }`, clone + debug. Channel roundtrip test. Run tests, confirm red. Add variant. Confirm green.)

- [ ] Task 1.4: Add `DbResult::ModelPlaced` variant (TDD: Write test constructing `DbResult::ModelPlaced { asset_id }`, clone + debug. Channel roundtrip test. Run tests, confirm red. Add variant. Confirm green.)

- [ ] Task 1.5: Add `DbResult::AssetList` variant (TDD: Write test constructing `DbResult::AssetList { assets: vec![...] }` with one `AssetInfo` entry. Clone + debug. Channel roundtrip test. Run tests, confirm red. Add variant. Confirm green.)

- [ ] Verification: Run `cargo test -p fe-runtime`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. [checkpoint marker]

---

## Phase 2: Database Query Layer

Goal: Add `list_models` query to `fe-database` and wire `PlaceModel`/`ListAssets` handling into the DB thread's command loop.

Tasks:

- [ ] Task 2.1: Add `list_models` query function in `fe-database/src/queries.rs` (TDD: Write async test that creates an in-memory SurrealDB, runs schema, inserts 2 model records via `place_model`, then calls `list_models` and asserts it returns 2 entries with correct fields. Run test, confirm red. Implement `list_models` as `SELECT * FROM model WHERE petal_id = $petal_id`. Confirm green.)

- [ ] Task 2.2: Test `list_models` with empty results (TDD: Write async test calling `list_models` on a petal with no models, assert empty vec. Should pass immediately after Task 2.1 implementation, but validates the edge case.)

- [ ] Task 2.3: Wire `PlaceModel` handling in the DB thread command loop (TDD: Write test that sends `DbCommand::PlaceModel` through the channel, verify the DB thread responds with `DbResult::ModelPlaced`. This may be an integration test or a unit test depending on how the DB thread loop is structured. Implement the match arm. Confirm green.)

- [ ] Task 2.4: Wire `ListAssets` handling in the DB thread command loop (TDD: Write test that sends `DbCommand::ListAssets` through the channel, verify the DB thread responds with `DbResult::AssetList`. Implement the match arm. Confirm green.)

- [ ] Verification: Run `cargo test -p fe-database`, `cargo test -p fe-runtime`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. [checkpoint marker]

---

## Phase 3: Bevy Resources and Asset Browser UI

Goal: Create the `ActivePetal` and `AssetRegistry` resources, the `DbResult` polling system, and update the asset browser panel to display live data.

Tasks:

- [ ] Task 3.1: Define `ActivePetal` resource in `fe-runtime` or `fe-ui` (TDD: Write test that constructs `ActivePetal(Some(petal_id))` and `ActivePetal(None)`, verifies `Default` gives `None`. Run test, confirm red. Implement. Confirm green.)

- [ ] Task 3.2: Define `AssetRegistry` resource (TDD: Write test that constructs `AssetRegistry { assets: vec![] }`, verifies `Default` gives empty vec. Test adding an `AssetInfo` entry and reading it back. Run test, confirm red. Implement as `#[derive(Resource, Default)]`. Confirm green.)

- [ ] Task 3.3: Implement `poll_db_results` system (TDD: Write test that creates a crossbeam channel, sends `DbResult::AssetList` with 2 entries, calls the polling function, asserts the `AssetRegistry` resource now contains 2 entries. Write second test: send `DbResult::ModelPlaced`, assert the system sends `DbCommand::ListAssets` on the command channel. Implement system. Confirm green.)

- [ ] Task 3.4: Update `asset_browser_panel` to read from `AssetRegistry` (TDD: Write test that calls `asset_browser_panel` with an `AssetRegistry` containing 2 entries and verifies the panel function accepts the resource parameter without panic. Write test for empty registry showing "No assets" path. Refactor function signature to accept `&AssetRegistry`. Implement display logic. Confirm green.)

- [ ] Task 3.5: Integrate `AssetRegistry` into `gardener_console` call site (Update `gardener_console` to pass `AssetRegistry` to `asset_browser_panel`. Ensure `cargo build` and `cargo test` pass.)

- [ ] Verification: Run `cargo test -p fe-ui -p fe-runtime`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Confirm all pass. Manually verify: launch app, confirm Asset Browser panel renders "No assets" when no models are placed. [checkpoint marker]

---

## Phase 4: Drag-and-Drop Handler and 3D Spawning

Goal: Implement the file drop event handler and the 3D scene spawning system. This phase connects all previous work into the complete user flow.

Tasks:

- [ ] Task 4.1: Implement `handle_file_drop` system — validation path (TDD: Write test that constructs a `FileDragAndDrop::DroppedFile` event with a path to a temp file containing non-GLB bytes, calls the handler logic, asserts no `DbCommand` is sent. Write test with GLB magic bytes [0x67, 0x6C, 0x54, 0x46] in a temp file under 256MB, assert `DbCommand::PlaceModel` is sent. Write test with no `ActivePetal`, assert no command sent. Implement. Confirm green.)

- [ ] Task 4.2: Implement `handle_file_drop` system — async file I/O (TDD: Write test verifying that the file read and ingestion are dispatched to `AsyncComputeTaskPool` and do not block the calling thread. This may be a structural/design test ensuring the system uses `Task<>` or spawns onto the pool. Implement using `AsyncComputeTaskPool::get().spawn()`. Confirm green.)

- [ ] Task 4.3: Define default transform helper (TDD: Write test that calls `default_model_transform()` and asserts it returns `{ "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] }`. Run test, confirm red. Implement as a simple function returning `serde_json::Value`. Confirm green.)

- [ ] Task 4.4: Implement `spawn_placed_model` system (TDD: Write test that given a `ModelPlaced { asset_id }` result, the system spawns an entity with `SceneRoot` and a `Transform` at origin. This requires a minimal Bevy `App` test harness with `AssetServer` stub. Implement system that calls `load_to_bevy` and spawns `SceneRoot`. Confirm green.)

- [ ] Task 4.5: Register all new systems in the Bevy app plugin (Wire `handle_file_drop`, `poll_db_results`, and `spawn_placed_model` into the app's system schedule. Ensure `cargo build` passes. Add a smoke test that the plugin can be added to a minimal `App` without panic.)

- [ ] Task 4.6: End-to-end integration test (Write a test that: creates a temp GLB file with valid magic bytes, simulates a `FileDragAndDrop::DroppedFile` event, verifies the `DbCommand::PlaceModel` is produced with correct asset_id and default transform, simulates a `DbResult::ModelPlaced` response, verifies the `AssetRegistry` refresh is triggered. This validates the full pipeline minus actual SurrealDB.)

- [ ] Verification: Run `cargo test --workspace`, `cargo fmt --check`, `cargo clippy -- -D warnings`. Manually verify: launch app, drag a `.glb` file onto the window, confirm the model appears in the 3D scene and the Asset Browser panel updates. [checkpoint marker]
