# Implementation Plan: Scene Graph Bridge -- DB Entity / Bevy ECS Synchronization

## Overview

This plan is divided into 5 phases, progressing from data types and conversion logic through spawn/sync systems to room visualization. Each phase ends with a verification checkpoint.

- **Phase 1**: Bridge components and transform serialization (pure data, no Bevy systems)
- **Phase 2**: Scene loading -- spawn ECS entities from DB query results
- **Phase 3**: ECS-to-DB sync -- debounced transform write-back
- **Phase 4**: DB-to-ECS sync -- polling and entity lifecycle
- **Phase 5**: Asset resolution and room boundary visualization

Estimated total: 18 tasks across 5 phases.

**Prerequisite:** Seedling Onboarding Phase 1 (DbCommand CRUD variants) must be complete or stubbed before Phase 2 can execute against real channels.

---

## Phase 1: Bridge Components & Transform Serialization

Goal: Define all Bevy components, resources, and the bidirectional transform conversion between DB format (GeoJSON Point + elevation + rotation array + scale array) and Bevy `Transform`.

Tasks:

- [ ] Task 1.1: Define `SceneNode`, `SceneRoom`, `ScenePetal` marker components in new `fe-renderer/src/scene_bridge/components.rs` (TDD: write tests that each component derives `Component`, `Debug`, `Clone`, `Reflect`; assert `SceneNode` has `db_id` and `node_id` fields; implement; refactor)
- [ ] Task 1.2: Define `LoadedScene` resource with `HashMap<String, Entity>` mapping `node_id -> Entity`, plus `active_petal_id: Option<String>` (TDD: write tests for insert, lookup, remove, clear operations; implement in `fe-renderer/src/scene_bridge/resources.rs`; refactor)
- [ ] Task 1.3: Define `SceneSyncConfig` resource with `poll_interval_secs: f64` (default 2.0), `max_writes_per_sec: f64` (default 10.0) (TDD: write test for default values; implement; refactor)
- [ ] Task 1.4: Define `SyncTimer` component with `last_sync: f64` and `min_interval: f64` (TDD: write test for default min_interval = 0.1; implement; refactor)
- [ ] Task 1.5: Implement `transform_to_db()` function: `Transform -> (serde_json::Value /*GeoJSON Point*/, f64 /*elevation*/, Vec<f64> /*rotation*/, Vec<f64> /*scale*/)` (TDD: write test with known Transform, assert GeoJSON Point has correct coordinates [x, z], elevation = y, rotation = [qx, qy, qz, qw], scale = [sx, sy, sz]; implement in `fe-renderer/src/scene_bridge/convert.rs`; refactor)
- [ ] Task 1.6: Implement `db_to_transform()` function: `(GeoJSON point value, f64 elevation, &[f64] rotation, &[f64] scale) -> Transform` (TDD: write round-trip test -- convert Transform to DB format and back, assert approximate equality within f32 epsilon; test edge cases: zero scale, identity rotation; implement; refactor)
- [ ] Task 1.7: Define `PreviousTransform` component wrapping `Transform` for change detection (TDD: write test that `PreviousTransform` can be compared to current `Transform` for equality within epsilon; implement with a helper `is_changed(&self, current: &Transform, epsilon: f32) -> bool`; refactor)
- [ ] Task 1.8: Register `scene_bridge` module in `fe-renderer/src/lib.rs` and export public types (TDD: write test that all public types are importable from `fe_renderer::scene_bridge`; implement `pub mod scene_bridge`; refactor)
- [ ] Verification: `cargo test -p fe-renderer`, `cargo clippy -p fe-renderer -- -D warnings`, `cargo fmt -p fe-renderer --check` [checkpoint marker]

---

## Phase 2: Scene Loading System

Goal: When the active Petal changes, query the DB for nodes and spawn Bevy entities with the correct transforms and asset handles.

Tasks:

- [ ] Task 2.1: Implement `detect_petal_change` system that compares `NavigationState::active_petal_id` against `LoadedScene::active_petal_id` and sends `DbCommand::ListNodes` on change (TDD: write test that given a changed petal_id, the system sends exactly one `DbCommand::ListNodes` through a mock channel; when unchanged, no command is sent; implement in `fe-renderer/src/scene_bridge/systems.rs`; refactor)
- [ ] Task 2.2: Implement `despawn_current_scene` helper that despawns all entities in `LoadedScene` and clears the map (TDD: write test using a minimal Bevy `World` -- insert 3 entities into `LoadedScene`, call despawn, assert world entity count decreased and map is empty; implement; refactor)
- [ ] Task 2.3: Implement `spawn_scene_nodes` system that listens for `DbResult::Nodes` messages and spawns entities (TDD: write test that given a `DbResult::Nodes` with 2 node records, 2 entities are spawned with `SceneNode` components and correct `Transform` values; implement using `db_to_transform` and `resolve_asset`; refactor)
- [ ] Task 2.4: Implement `resolve_asset` function: `asset_id: Option<&str>, AssetServer -> Handle<Gltf> or placeholder` (TDD: write test that `None` asset_id returns placeholder handle; valid hex string delegates to `load_to_bevy`; invalid hex returns placeholder; implement in `fe-renderer/src/scene_bridge/assets.rs`; refactor)
- [ ] Task 2.5: Define `AssetPending` component for placeholder tracking and implement placeholder cube spawning (TDD: write test that when asset_id is not cached, entity gets `AssetPending` component and a `Mesh3d` cube child; implement; refactor)
- [ ] Verification: `cargo test -p fe-renderer -p fe-runtime`, manual test -- select a Petal with seeded nodes, verify entities appear in viewport with placeholder cubes. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase 3: ECS-to-DB Sync (Transform Write-Back)

Goal: Detect transform changes on scene entities and persist them to SurrealDB with debouncing.

Tasks:

- [ ] Task 3.1: Implement `detect_transform_changes` system that compares `Transform` against `PreviousTransform` and marks dirty entities (TDD: write test that modifying an entity's `Transform` while `PreviousTransform` differs causes a `TransformDirty` marker to be inserted; no marker when transforms match within epsilon; implement in `fe-renderer/src/scene_bridge/sync.rs`; refactor)
- [ ] Task 3.2: Implement `sync_dirty_transforms` system with per-entity debounce via `SyncTimer` (TDD: write test that a dirty entity with `last_sync` older than `min_interval` triggers a `DbCommand::UpdateNode` send; an entity within the cooldown window does NOT trigger a send; implement; refactor)
- [ ] Task 3.3: Implement `LocallyEditing` marker component and ensure sync-from-DB skips entities with this marker (TDD: write test that an entity with `LocallyEditing` is excluded from DB-to-ECS transform updates; implement component definition and query filter; refactor)
- [ ] Task 3.4: Implement `update_previous_transform` system that copies current `Transform` to `PreviousTransform` after a successful sync send (TDD: write test that after sync, `PreviousTransform` matches current `Transform`; implement; refactor)
- [ ] Verification: `cargo test -p fe-renderer`, manual test -- modify a node's transform via Bloom Stage editor, confirm DB record updates within 200ms. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase 4: DB-to-ECS Sync (Polling & Lifecycle)

Goal: Periodically poll the DB for the current Petal's nodes and reconcile additions, removals, and remote transform updates.

Tasks:

- [ ] Task 4.1: Implement `poll_scene_nodes` system that sends `DbCommand::ListNodes` at `SceneSyncConfig::poll_interval_secs` intervals (TDD: write test that at `elapsed >= poll_interval`, a `ListNodes` command is sent; before the interval, no command is sent; uses a `PollTimer` local resource; implement in `fe-renderer/src/scene_bridge/poll.rs`; refactor)
- [ ] Task 4.2: Implement `reconcile_scene` system that processes `DbResult::Nodes` and diffs against `LoadedScene` (TDD: write test with 3 scenarios -- new node in DB spawns entity, missing node in DB despawns entity, changed transform updates entity; entity with `LocallyEditing` is skipped for transform update; implement; refactor)
- [ ] Task 4.3: Implement `handle_node_created` system for `DbResult::NodeCreated` events (TDD: write test that a `NodeCreated` result spawns one entity and adds it to `LoadedScene`; implement; refactor)
- [ ] Task 4.4: Implement `handle_node_deleted` system for `DbResult::NodeDeleted` events (TDD: write test that a `NodeDeleted` result despawns the entity and removes from `LoadedScene`; handle case where entity already despawned gracefully; implement; refactor)
- [ ] Task 4.5: Add `SceneStats` resource tracking `total_entities`, `pending_assets`, `syncs_per_sec`, `last_poll_timestamp` and update it from the reconcile system (TDD: write test that after spawning 5 entities, `SceneStats::total_entities` is 5; after despawning 2, it is 3; implement; refactor)
- [ ] Verification: `cargo test -p fe-renderer`, manual test -- create a node via CRUD panel while Petal is loaded, confirm it appears in viewport within 2 seconds. Delete a node, confirm it disappears. `cargo clippy -- -D warnings` [checkpoint marker]

---

## Phase 5: Asset Resolution & Room Boundaries

Goal: Wire up GLTF asset replacement for pending placeholders and implement room boundary wireframe visualization.

Tasks:

- [ ] Task 5.1: Implement `check_pending_assets` system that polls `AssetPending` entities for cache availability (TDD: write test that when the cache file for an `AssetPending` entity appears on disk, the system removes `AssetPending`, despawns placeholder cube children, and spawns `SceneRoot` GLTF children; implement in `fe-renderer/src/scene_bridge/assets.rs`; refactor)
- [ ] Task 5.2: Implement `load_room_boundaries` system triggered alongside scene loading (TDD: write test that `DbResult::Rooms` with 2 rooms having bounds spawns 2 `SceneRoom` entities; rooms without bounds are skipped; implement in `fe-renderer/src/scene_bridge/rooms.rs`; refactor)
- [ ] Task 5.3: Implement `draw_room_wireframes` Gizmo system that draws AABB wireframes for `SceneRoom` entities (TDD: write test that given a `SceneRoom` entity with known bounds, the gizmo system produces 12 line segments forming a box; implement using `Gizmos::line` calls; refactor)
- [ ] Task 5.4: Implement `ShowRoomBounds` resource (bool, default true) and wire it as a run condition for the wireframe system (TDD: write test that when `ShowRoomBounds` is false, the wireframe system does not run; implement; refactor)
- [ ] Task 5.5: Register all scene bridge systems in a `SceneBridgePlugin` and add it to the Bevy app in `fe-runtime/src/app.rs` (TDD: write test that `SceneBridgePlugin` registers all expected systems and resources; implement `impl Plugin for SceneBridgePlugin`; refactor)
- [ ] Verification: Full integration -- load a Petal with nodes and rooms, verify GLTF models appear (or placeholders), room wireframes visible, transform edits persist, external changes reconcile. `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo tarpaulin -p fe-renderer --out Html --output-dir coverage/` (target >80%) [checkpoint marker]
