# Specification: Drag & Drop Asset Placement -- File Drop + Scene Placement Flow

## Overview

Enable operators to drag GLB/GLTF files from the OS file explorer onto the FractalEngine viewport, see visual drop zone feedback, ingest the asset through the existing `GltfIngester` pipeline, enter a placement preview mode where the model follows the mouse cursor as a translucent ghost, click to confirm placement at a raycast position on the ground plane, and manage previously ingested assets through an Asset Library panel. This track also adds Alt+Drag duplication of existing placed entities.

## Background

FractalEngine has the core building blocks in place:

- **Asset ingestion**: `GltfIngester` in `fe-renderer/src/ingester.rs` validates GLB magic bytes, enforces the 256 MB limit, computes the BLAKE3 content address, and caches the file to disk.
- **Content addressing**: `fe-renderer/src/addressing.rs` provides `content_address()` and `validate_glb_magic()`.
- **Asset loading**: `fe-renderer/src/loader.rs` provides `load_to_bevy()` which loads from cache or falls back to placeholder.
- **Database schema**: `fe-database/src/schema.rs` defines the `node` table (with `position`, `elevation`, `rotation`, `scale`, `asset_id`) and the `asset` table (with `asset_id`, `name`, `content_type`, `size_bytes`, `data`).
- **Model placement query**: `fe-database/src/queries.rs` provides `place_model()` which writes an op-log entry and creates a `model` record.
- **UI shell**: `fe-ui` has a toolbar with tool buttons, a left sidebar, a right inspector panel, and a central viewport area. The `InspectorState` resource tracks selected entity, transform buffers, and URL fields.
- **Channel architecture**: The three-thread topology (Bevy main, network, DB) communicates via crossbeam `mpsc` channels using `DbCommand`/`DbResult` enums.

What is missing is the complete placement UX: drop zone feedback overlay, a placement preview mode with translucent ghost model, raycast-to-ground-plane positioning, click-to-confirm, escape-to-cancel, Alt+Drag duplication, and an Asset Library panel for re-entering placement mode from previously ingested assets.

**Dependencies**: This track depends on the Bloom Stage track (bloom_stage_20260322) for the ground plane entity, `GroundPlane` marker component, orbit camera, and raycasting infrastructure. It depends on the Petal Seed track (petal_seed_20260322) for `DbCommand::PlaceModel`/`ListAssets` variants, `AssetRegistry` resource, and `poll_db_results` system.

## Functional Requirements

### FR-1: OS File Drop Detection

**Description:** A Bevy system listens for `FileDragAndDrop` events from winit. On `DroppedFile`, read the file bytes, validate via `GltfIngester`, and transition to placement preview mode. On `HoveredFile`, show drop zone overlay. On `HoveredFileCancelled`, hide the overlay.

**Acceptance Criteria:**
- System reads all `FileDragAndDrop` variants from the Bevy event queue
- `HoveredFile` triggers the drop zone overlay (FR-2)
- `HoveredFileCancelled` hides the drop zone overlay
- `DroppedFile` reads file bytes from the provided path
- Files failing GLB magic validation are rejected with `tracing::warn!` (no crash)
- Files exceeding 256 MB are rejected with `tracing::warn!`
- Non-GLB extensions (.png, .txt, .fbx) are rejected with `tracing::warn!`
- Successfully ingested files create an `asset` record in SurrealDB via DbCommand
- After successful ingestion, the system enters placement preview mode (FR-4)
- If no `ActivePetal` is set, drops are rejected with `tracing::warn!`
- File I/O runs on `AsyncComputeTaskPool`, never blocking the main thread

**Priority:** P0

### FR-2: Drop Zone Feedback Overlay

**Description:** When a file is being dragged over the Bevy window (between `HoveredFile` and `HoveredFileCancelled` or `DroppedFile`), render a semi-transparent overlay on the viewport with the text "Drop GLB to add to scene" and a highlight border.

**Acceptance Criteria:**
- A `DragHoverActive` resource (bool flag) tracks whether a file hover is in progress
- When `DragHoverActive` is true, an egui overlay renders on the viewport center panel
- The overlay uses a semi-transparent dark background (alpha ~0.4)
- The overlay displays "Drop GLB to add to scene" in large centered text
- The overlay shows a dashed or solid highlight border around the viewport edges
- The overlay disappears immediately when the hover ends or a file is dropped
- The overlay does not interfere with other egui panels (toolbar, sidebar, inspector)
- The overlay renders at 60 FPS with no visible lag

**Priority:** P1

### FR-3: Asset Record Creation on Ingestion

**Description:** When a file is successfully ingested, create an `asset` record in SurrealDB containing the BLAKE3 content address, original filename, content type, and file size. This is separate from the `node` record which is created on placement confirmation.

**Acceptance Criteria:**
- A new `DbCommand::CreateAsset` variant is added (or reuse existing if available)
- The asset record includes: `asset_id` (hex-encoded BLAKE3), `name` (original filename), `content_type` ("model/gltf-binary"), `size_bytes` (file length)
- The `data` field in the asset table stores base64-encoded GLB content (for P2P distribution later)
- Duplicate asset IDs (same file dropped twice) do not create duplicate records (upsert behavior)
- The asset record is created before entering placement preview mode
- Asset creation errors are logged but do not block entering placement preview (the file is already cached on disk)

**Priority:** P0

### FR-4: Placement Preview Mode

**Description:** After successful ingestion (or when clicking an asset in the Asset Library), the application enters a "placement preview" mode. A translucent copy of the model follows the mouse cursor in the 3D viewport, positioned via raycast against the ground plane at Y=0.

**Acceptance Criteria:**
- A `PlacementMode` resource is defined as an enum: `Inactive`, `Previewing { asset_id: String, preview_entity: Entity }`
- Entering placement mode spawns a translucent preview entity using `load_to_bevy()` with modified material (alpha ~0.5, emissive tint)
- The preview entity has a `PlacementPreview` marker component (excluded from selection, saving, picking)
- A Bevy system raycasts from the mouse cursor position through the camera to the Y=0 ground plane
- The preview entity's `Transform.translation` is updated every frame to the raycast hit point
- If the raycast misses the ground plane (mouse outside viewport or pointing at sky), the preview entity is hidden
- The cursor changes to a crosshair or placement indicator during preview mode
- Other tools (Select, Move, Rotate, Scale) are suspended during placement mode
- The toolbar shows a visual indicator that placement mode is active

**Priority:** P0

### FR-5: Click to Place (Confirm Placement)

**Description:** While in placement preview mode, left-clicking confirms the placement. The preview entity is replaced with a permanent entity and a `node` record is created in SurrealDB.

**Acceptance Criteria:**
- Left mouse click during `PlacementMode::Previewing` triggers placement confirmation
- The preview entity is despawned
- A permanent entity is spawned at the confirmed position with full-opacity materials
- The permanent entity has `ModelMetadata` component with the asset_id and petal_id
- A `DbCommand::PlaceModel` is sent with `petal_id`, `asset_id`, and `transform` (position from raycast, identity rotation, unit scale)
- The transform JSON uses the format: `{ "translation": [x, 0, z], "rotation": [0, 0, 0, 1], "scale": [1, 1, 1] }` where x,z come from the raycast hit
- After placement, `PlacementMode` returns to `Inactive`
- The AssetRegistry is refreshed via `DbCommand::ListAssets`
- Multiple rapid clicks do not produce duplicate placements (debounce or state guard)

**Priority:** P0

### FR-6: Escape to Cancel Placement

**Description:** Pressing Escape during placement preview mode cancels the operation, removes the preview entity, and returns to normal mode.

**Acceptance Criteria:**
- Pressing `KeyCode::Escape` during `PlacementMode::Previewing` triggers cancellation
- The preview entity is despawned
- `PlacementMode` returns to `Inactive`
- No `node` record is created in SurrealDB
- The cached asset file remains on disk (the `asset` record persists)
- The toolbar placement indicator is removed
- Normal tool mode (Select) is restored

**Priority:** P0

### FR-7: Alt+Drag Duplicate

**Description:** While an existing entity is selected, holding Alt and dragging initiates a duplication. A new entity is created with the same `asset_id` but at the new position.

**Acceptance Criteria:**
- Alt+left-mouse-button-down on a selected entity enters duplication mode
- A translucent preview of the selected entity's model appears attached to the cursor (same as placement preview)
- The preview follows the mouse via ground-plane raycast
- Releasing the mouse button confirms placement at the current position
- A new `node` record is created in SurrealDB with the same `asset_id` as the source entity but the new position
- Pressing Escape during Alt+Drag cancels the duplication (no new entity, no DB record)
- The original entity remains unchanged at its original position
- The `PlacementMode` resource is reused with an additional variant: `Duplicating { source_asset_id: String, preview_entity: Entity }`

**Priority:** P2

### FR-8: Asset Library Panel

**Description:** A collapsible section in the left sidebar showing all previously ingested assets. Clicking an asset entry enters placement preview mode for that asset.

**Acceptance Criteria:**
- A new "Asset Library" collapsing header section is added to the left sidebar
- The section reads from the `AssetRegistry` resource
- Each entry displays: truncated asset_id (first 8 hex chars), original filename (from asset record), file size in human-readable format
- Clicking an entry enters `PlacementMode::Previewing` for that asset_id
- If the asset's cached file is missing, `load_to_bevy` falls back to placeholder (no error)
- When the Asset Library is empty, display "No assets -- drop a GLB file to get started"
- The panel scrolls when there are more than ~10 entries
- A hover tooltip shows the full asset_id

**Priority:** P1

## Non-Functional Requirements

### NFR-1: Performance

- File read and ingestion (BLAKE3 hash + disk write) must complete in under 2 seconds for a 50 MB GLB on an NVMe drive
- The drag-and-drop handler must not block the Bevy main thread for more than one frame (file I/O uses `AsyncComputeTaskPool`)
- Placement preview raycast must run every frame at 60 FPS with no visible lag
- The drop zone overlay must appear within 1 frame of the `HoveredFile` event
- Asset Library panel must render at 60 FPS with up to 200 asset entries

### NFR-2: Error Resilience

- No `unwrap()` or `expect()` in production code paths
- All file I/O errors are logged via `tracing::warn!` or `tracing::error!` and do not crash the application
- Invalid file drops (wrong format, too large, unreadable path) produce a `tracing::warn!` message
- Raycast miss (no ground plane hit) gracefully hides the preview rather than crashing
- Corrupted cached assets fall back to placeholder model

### NFR-3: Thread Safety

- File bytes never cross the crossbeam channel (only the resulting `AssetId` hex string does)
- All Bevy resources accessed via the ECS system parameter API (no raw Arc/Mutex)
- Preview entity spawning and despawning happen only on the main thread via Commands

### NFR-4: UX Consistency

- Placement preview must be visually distinct from placed entities (translucent, emissive tint)
- Drop zone overlay must not obscure the 3D viewport excessively (semi-transparent)
- Escape always cancels any active placement/duplication mode
- Tool state is cleanly restored after placement or cancellation

## User Stories

### US-1: Operator Drops a GLB File onto the Viewport

**As** a Node operator,
**I want** to drag a .glb file from my file explorer onto the FractalEngine window and see it appear as a movable preview,
**So that** I can choose exactly where to place the model in my Petal.

**Scenarios:**

**Given** the Bevy window is open, a Petal is active, and no placement is in progress,
**When** I drag a valid .glb file over the window,
**Then** a semi-transparent overlay appears saying "Drop GLB to add to scene".

**Given** the drop zone overlay is showing,
**When** I release the file onto the window,
**Then** the overlay disappears, the file is ingested, and a translucent preview of the model appears at my cursor position.

**Given** a placement preview is active,
**When** I move my mouse over the viewport,
**Then** the preview model follows my cursor along the ground plane (Y=0).

**Given** a placement preview is active,
**When** I left-click,
**Then** the model is placed permanently at the clicked position, a node record is created in the database, and I return to normal select mode.

### US-2: Operator Cancels Placement

**As** a Node operator,
**I want** to press Escape to cancel a placement in progress,
**So that** I can abort a mistaken drop without creating a node in the scene.

**Given** a placement preview is active,
**When** I press Escape,
**Then** the preview disappears, no node record is created, and I return to normal mode. The asset remains in the Asset Library for future use.

### US-3: Operator Places Asset from Library

**As** a Node operator,
**I want** to click an asset in the Asset Library panel to enter placement mode,
**So that** I can place previously imported models without re-dropping the file.

**Given** I have previously dropped 3 GLB files into the scene,
**When** I open the Asset Library section in the sidebar,
**Then** I see 3 entries with truncated IDs and filenames.

**Given** the Asset Library shows entries,
**When** I click on one entry,
**Then** I enter placement preview mode with that asset's model attached to my cursor.

### US-4: Operator Duplicates an Entity

**As** a Node operator,
**I want** to Alt+Drag a selected entity to duplicate it,
**So that** I can quickly populate my scene with copies of the same model.

**Given** I have selected an entity in the scene,
**When** I hold Alt and drag,
**Then** a translucent preview appears at my cursor and follows the mouse.

**Given** I am in Alt+Drag duplication mode,
**When** I release the mouse button,
**Then** a new entity is placed at the cursor position with the same asset, and a new node record is created.

### US-5: Operator Drops an Invalid File

**As** a Node operator,
**I want** invalid file drops to be silently rejected with a warning,
**So that** I don't crash the application by accidentally dropping a non-GLB file.

**Given** the Bevy window is open,
**When** I drag a .png file onto the window,
**Then** the drop zone overlay shows normally, but when I release, nothing happens in the scene and a warning is logged.

**Given** the Bevy window is open,
**When** I drag a .glb file larger than 256 MB onto the window,
**Then** the file is rejected, a warning is logged, and no asset or node record is created.

## Technical Considerations

- **Bevy 0.18 `FileDragAndDrop`**: This is a Bevy `Event` with variants `HoveredFile { path, window }`, `DroppedFile { path, window }`, and `HoveredFileCancelled { window }`. Read via `EventReader<FileDragAndDrop>`.
- **Ground plane raycast**: Requires a `Camera` query and viewport-to-ray conversion. Use `Camera::viewport_to_world()` to get a `Ray3d`, then intersect with the Y=0 plane: `t = -ray.origin.y / ray.direction.y; point = ray.origin + ray.direction * t`.
- **Translucent preview materials**: After spawning the GLTF scene, traverse all child entities with `MeshMaterial3d<StandardMaterial>`, clone their materials, set `alpha_mode: AlphaMode::Blend`, `base_color.a = 0.5`, and add a subtle emissive tint. Store original handles for cleanup.
- **PlacementMode state machine**: Use a Bevy `Resource` enum. Systems check this resource to decide behavior. Only one placement can be active at a time.
- **Async ingestion**: The `GltfIngester::ingest()` call does synchronous BLAKE3 + fs::write. Wrap in `AsyncComputeTaskPool::get().spawn()` and poll the resulting `Task<Result<AssetId>>` in a subsequent system.
- **AssetId serialization**: `AssetId([u8; 32])` serialized as hex string when crossing the channel. The DB stores asset_id as a string.
- **Active Petal context**: The `NavigationState` resource already has `active_petal_id: Option<String>`. Use this to determine which petal receives the placed node.
- **SceneRoot spawning**: In Bevy 0.18, GLTF scenes are spawned via `commands.spawn(SceneRoot(scene_handle))`. The scene handle comes from the loaded `Gltf` asset's `default_scene` or first scene.

## Out of Scope

- Transform editing during placement (position is raycast-only; rotation and scale use defaults)
- Snap-to-grid placement
- Multi-object placement (placing multiple copies in one session without re-entering placement mode)
- Undo/redo for placement or duplication
- Thumbnail previews in the Asset Library (text-only entries)
- Asset deletion from the Asset Library
- P2P asset distribution (handled by iroh-blobs in a separate track)
- FBX/OBJ format support (project-wide non-goal)
- UI toast notifications for success/error feedback (future track)
- Room assignment for placed nodes (defaults to petal-level)

## Open Questions

1. **Preview material strategy**: Should the translucent preview clone all child materials (expensive for complex models with many meshes), or apply a single override material to all meshes? Cloning preserves visual fidelity; override is simpler and cheaper. Recommendation: start with single override material, optimize later if needed.
2. **Asset Library data source**: Should the Asset Library query the `asset` table (all ingested assets) or the `model` table (only placed assets)? The `asset` table represents the full catalog of ingested files, which is more useful for the library. The `model` table tracks placements. Recommendation: query `asset` table.
3. **Ground plane visibility during placement**: Should the ground plane render a grid pattern during placement mode to help with spatial orientation, or remain as a flat surface? Recommendation: add a subtle grid shader in a future track; flat surface for now.
4. **Alt+Drag threshold**: Should Alt+Drag require a minimum drag distance before entering duplication mode (to prevent accidental duplicates)? Recommendation: 5px minimum drag distance.
