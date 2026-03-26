# Specification: Petal Seed — GLTF Drag-and-Drop & Asset Seeding

## Overview

Enable operators to drag-and-drop GLB files onto the Bevy window to ingest, persist, and spawn 3D models into the active Petal. This track wires the existing `GltfIngester` and `place_model` infrastructure into a complete user-facing flow: file drop detection, database persistence via the channel architecture, a live asset browser panel, and 3D scene spawning.

## Background

FractalEngine already has the building blocks for asset ingestion (`GltfIngester` in `fe-renderer`), content addressing (BLAKE3 via `fe-renderer::addressing`), database model placement (`fe-database::queries::place_model`), and a stub asset browser panel (`fe-ui::panels::asset_browser_panel`). What is missing is the glue: Bevy event handling for file drops, `DbCommand`/`DbResult` variants to communicate model placement and listing across threads, a Bevy `Resource` to hold the live asset list, and a system to spawn the loaded GLTF into the 3D scene.

The three-thread architecture (Bevy main, network, DB) communicates exclusively via crossbeam `mpsc` channels. All database mutations go through the `DbCommand` channel and results come back via `DbResult`. This track must respect that boundary.

## Functional Requirements

### FR-1: File Drag-and-Drop Handler

**Description:** A Bevy system that listens for `FileDragAndDrop::DroppedFile` events, reads the file bytes, validates and ingests via `GltfIngester`, then sends a `DbCommand::PlaceModel` to the database thread.

**Acceptance Criteria:**
- System reads `FileDragAndDrop::DroppedFile` events from the Bevy event queue
- File bytes are read from disk using the provided path
- Files failing GLB magic validation are rejected with a logged warning (no crash, no panic)
- Files exceeding 256 MB are rejected with a logged warning
- Successfully ingested files produce a `DbCommand::PlaceModel` sent via the crossbeam channel
- The default transform is `{ translation: [0,0,0], rotation: [0,0,0,1], scale: [1,1,1] }`
- Non-GLB files (e.g., `.png`, `.txt`) are silently rejected with a `tracing::warn!`

**Priority:** P0

### FR-2: DbCommand and DbResult Variants for Model Placement

**Description:** Extend the `DbCommand` enum with `PlaceModel` and `ListAssets` variants, and `DbResult` with `ModelPlaced` and `AssetList` variants.

**Acceptance Criteria:**
- `DbCommand::PlaceModel { petal_id: String, asset_id: String, transform: serde_json::Value }` added
- `DbCommand::ListAssets { petal_id: String }` added
- `DbResult::ModelPlaced { asset_id: String }` added
- `DbResult::AssetList { assets: Vec<AssetInfo> }` added
- `AssetInfo` struct contains `asset_id: String`, `petal_id: String`, `transform: serde_json::Value`
- All new variants derive `Debug` and `Clone`
- Existing channel roundtrip tests still pass
- New variants have their own roundtrip tests

**Priority:** P0

### FR-3: Database Thread Command Handling

**Description:** The database thread's command loop handles the new `PlaceModel` and `ListAssets` commands by calling the existing `place_model` query function and a new `list_models` query function.

**Acceptance Criteria:**
- `DbCommand::PlaceModel` calls `fe_database::queries::place_model` and responds with `DbResult::ModelPlaced`
- `DbCommand::ListAssets` calls a new `fe_database::queries::list_models` and responds with `DbResult::AssetList`
- `list_models` queries the `model` table filtering by `petal_id` and returns all matching rows
- Errors during query execution produce `DbResult::Error` (no panics)

**Priority:** P0

### FR-4: Asset List Resource

**Description:** A Bevy `Resource` that holds the current list of assets for the active Petal, updated by a system that reads `DbResult::AssetList` from the channel.

**Acceptance Criteria:**
- `AssetRegistry` resource defined with `assets: Vec<AssetInfo>` field
- A Bevy system polls the `DbResult` channel and updates the `AssetRegistry` when `AssetList` arrives
- When `ModelPlaced` arrives, the system sends a `DbCommand::ListAssets` to refresh the list
- The resource is initialized empty at app startup via `init_resource`

**Priority:** P0

### FR-5: Live Asset Browser Panel

**Description:** Update `asset_browser_panel` to display entries from the `AssetRegistry` resource instead of the static "No assets" label.

**Acceptance Criteria:**
- Each asset entry displays the first 8 hex characters of the `asset_id` as an identifier
- Each entry shows the truncated asset ID and transform summary (translation values)
- When the registry is empty, the panel shows "No assets" (preserves current behavior)
- The panel is read-only in this track (editing transforms is a future track)

**Priority:** P1

### FR-6: 3D Scene Spawning

**Description:** When a `ModelPlaced` result is received, spawn the GLTF model into the Bevy 3D scene using `load_to_bevy` and a `SceneRoot` component.

**Acceptance Criteria:**
- A Bevy system reacts to `ModelPlaced` results and spawns a new entity
- The entity uses `SceneRoot` with the GLTF handle from `load_to_bevy`
- The entity's `Transform` matches the stored default transform (origin, identity rotation, unit scale)
- If `load_to_bevy` falls back to `placeholder.glb`, the placeholder is spawned (no error)
- Multiple drops produce multiple spawned entities (no deduplication in v1)

**Priority:** P1

## Non-Functional Requirements

### NFR-1: Performance

- File read and ingestion (BLAKE3 hash + disk write) must complete in under 2 seconds for a 50 MB GLB on an NVMe drive
- The drag-and-drop handler must not block the Bevy main thread for more than one frame (file I/O uses `AsyncComputeTaskPool`)
- Asset browser panel must render at 60 FPS with up to 100 asset entries

### NFR-2: Error Resilience

- No `unwrap()` or `expect()` in production code paths
- All file I/O errors are logged via `tracing::warn!` or `tracing::error!` and do not crash the application
- Invalid file drops (wrong format, too large, unreadable path) produce user-visible feedback via `tracing` (UI toast is a future track)

### NFR-3: Thread Safety

- File bytes never cross the crossbeam channel (only the resulting `AssetId` string does)
- All Bevy resources accessed via the ECS system parameter API (no raw Arc/Mutex)

## User Stories

### US-1: Operator Drops a GLB File

**As** a Node operator,
**I want** to drag a GLB file from my file explorer onto the Bevy window,
**So that** the 3D model is automatically ingested, stored, and appears in my Petal.

**Scenarios:**

**Given** the Bevy window is open and a Petal is active,
**When** I drag a valid `.glb` file onto the window,
**Then** the model is ingested (BLAKE3 addressed, written to cache), a `model` record is created in SurrealDB, the model appears in the 3D scene, and the asset browser shows the new entry.

**Given** the Bevy window is open,
**When** I drag a `.png` file onto the window,
**Then** nothing happens in the scene, a warning is logged, and no database record is created.

**Given** the Bevy window is open,
**When** I drag a GLB file larger than 256 MB onto the window,
**Then** the file is rejected, a warning is logged, and no database record is created.

### US-2: Operator Views Asset List

**As** a Node operator,
**I want** to see all placed models in the Asset Browser panel,
**So that** I can review what assets are in my Petal.

**Given** a Petal has 3 placed models,
**When** I look at the Asset Browser panel,
**Then** I see 3 entries, each showing a truncated BLAKE3 hash and transform summary.

**Given** a Petal has no placed models,
**When** I look at the Asset Browser panel,
**Then** I see "No assets".

## Technical Considerations

- **Thread architecture:** File I/O for reading dropped files should use `AsyncComputeTaskPool` to avoid blocking the Bevy main thread. The `GltfIngester::ingest()` call does synchronous file I/O (BLAKE3 + fs::write), so it must run off the main thread.
- **AssetId serialization:** `AssetId([u8; 32])` needs to be serialized as a hex string when crossing the channel to the DB thread, matching how `place_model` currently accepts `asset_id: &str`.
- **Petal context:** The drag-and-drop handler needs to know the current active Petal ID. This should be a Bevy `Resource` (`ActivePetal(Option<PetalId>)`). If no Petal is active, drops are rejected.
- **Bevy 0.18 `FileDragAndDrop`:** This is a Bevy `Event` read via `EventReader<FileDragAndDrop>`. The `DroppedFile` variant contains the file path and the window entity.
- **SceneRoot:** In Bevy 0.18, GLTF scenes are spawned via `commands.spawn(SceneRoot(scene_handle))` where the scene handle comes from the loaded `Gltf` asset's `default_scene` or first scene.

## Out of Scope

- Transform editing UI (future track)
- Drag-to-position (placing models at the drop cursor location in 3D space)
- Asset deduplication warnings (same file dropped twice creates two model records)
- Asset deletion from the browser panel
- Undo/redo for model placement
- Thumbnail previews in the asset browser
- P2P asset distribution (handled by iroh-blobs in a separate track)
- FBX/OBJ format support (project-wide non-goal)
- UI toast notifications for success/error feedback

## Open Questions

1. **Active Petal selection:** How does the operator select which Petal to drop assets into? The Petal List panel exists but has no selection logic. This track assumes an `ActivePetal` resource exists or is created as a simple prerequisite.
2. **Room assignment:** The `model` table has a `petal_id` but not a `room_id`. Should dropped models default to a specific room, or is room assignment a separate interaction? This track defaults to petal-level placement with no room assignment.
3. **Asset browser refresh cadence:** Should the asset list be refreshed on a timer, or only reactively when a `ModelPlaced` event arrives? This track uses reactive refresh only (on placement confirmation).
