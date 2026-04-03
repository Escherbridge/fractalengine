# Specification: Scene Graph Bridge -- DB Entity / Bevy ECS Synchronization

## Overview

This track delivers the bidirectional synchronization layer between SurrealDB entity records and Bevy ECS entities. When a Petal is loaded, the bridge reads all Nodes from the database, spawns corresponding Bevy entities with correct transforms and GLTF assets, and keeps the two worlds in sync: ECS transform changes flow back to the DB (debounced), and DB-side mutations (create/update/delete) are reflected in the ECS. This is the connective tissue between the data layer (Seedling Onboarding CRUD) and the rendered 3D world (Bloom Stage viewport).

## Background

### What exists

- **Database schema** (`fe-database/src/schema.rs`): The `node` table stores per-entity data including `node_id`, `petal_id`, `display_name`, `asset_id`, `position` (geometry point on XZ plane), `elevation` (Y float), `rotation` (array), `scale` (array), `interactive` flag, and `created_at`.
- **CRUD commands** (Seedling Onboarding track, Phase 1-2): `DbCommand::ListNodes`, `DbCommand::CreateNode`, `DbCommand::UpdateNode`, `DbCommand::DeleteNode` variants, with matching `DbResult` responses, routed through the crossbeam channel to the DB thread.
- **Asset pipeline** (`fe-renderer`): `GltfIngester` content-addresses GLB files via BLAKE3, `load_to_bevy()` resolves `AssetId` to a cached file path or falls back to `placeholder.glb`, returning `Handle<Gltf>`.
- **Dead reckoning** (`fe-renderer/src/dead_reckoning.rs`): `DeadReckoningPredictor` component and system for interpolated motion on networked entities.
- **Runtime channels** (`fe-runtime/src/app.rs`): `DbCommandSender` and `DbResultReceiver` resources with drain systems that convert crossbeam messages into Bevy `Message` events.
- **Atlas metadata** (`fe-database/src/atlas.rs`): `RoomBounds { min: [f32;3], max: [f32;3] }`, `RoomMetadata`, `SpawnPoint` structs.
- **Bloom Stage spec**: Defines `ModelMetadata` component, 3D orbit camera, GLTF scene spawning, and raycasting selection -- but targets the `model` table. This track targets the `node` table (the canonical entity record) and provides the lower-level bridge that Bloom Stage's spawning system will consume.

### What is missing

- No Bevy components that link ECS entities back to their SurrealDB record IDs.
- No system that reads `DbResult::Nodes` and spawns/updates/despawns ECS entities.
- No system that detects ECS transform changes and persists them back to the DB.
- No asset resolution pipeline that maps `asset_id` (string in DB) to `AssetId` (BLAKE3 hash) to `Handle<Gltf>`.
- No Room boundary visualization in the 3D viewport.
- No debounce mechanism for transform write-back to avoid flooding the DB channel.

## Functional Requirements

### FR-1: Scene Entity Marker Components

**Description:** Define Bevy marker components that link ECS entities to their corresponding SurrealDB records, enabling bidirectional lookup.

**Acceptance Criteria:**
- AC-1.1: `SceneNode` component contains `db_id: String` (the SurrealDB record ID, e.g. `node:ulid_value`) and `node_id: String` (the `node_id` field value).
- AC-1.2: `SceneRoom` component contains `db_id: String` and `room_name: String`.
- AC-1.3: `ScenePetal` component contains `db_id: String` and `petal_id: String`.
- AC-1.4: All three components derive `Component`, `Debug`, `Clone`, and `Reflect`.
- AC-1.5: A `SceneNodeBundle` type alias or helper function exists to spawn a node entity with all required components in one call.

**Priority:** P0

### FR-2: Scene Loading System

**Description:** When a Petal is selected (via `NavigationState::active_petal_id` change), query all Nodes belonging to that Petal from the DB and spawn corresponding Bevy entities.

**Acceptance Criteria:**
- AC-2.1: A Bevy system detects when `active_petal_id` changes (via a `Local<Option<String>>` comparison or `Changed` detection on a resource).
- AC-2.2: On change, the system sends `DbCommand::ListNodes { petal_id }` through the `DbCommandSender`.
- AC-2.3: A separate system listens for `DbResult::Nodes { petal_id, nodes }` messages and spawns one ECS entity per node record.
- AC-2.4: Each spawned entity receives: `Transform` (reconstructed from DB `position`, `elevation`, `rotation`, `scale`), `SceneNode` marker, a display name component (or field on `SceneNode`), and a GLTF scene root via asset resolution.
- AC-2.5: When the active Petal changes, all entities from the previous Petal are despawned before loading the new one.
- AC-2.6: A `LoadedScene` resource tracks which `petal_id` is currently loaded and maps `node_id -> Entity` for O(1) lookup.

**Priority:** P0

### FR-3: Scene Sync System (ECS to DB)

**Description:** When a Node entity's `Transform` is modified in Bevy (e.g., via gizmo manipulation or the transform editor), persist the change back to SurrealDB.

**Acceptance Criteria:**
- AC-3.1: A Bevy system queries all `(Entity, &SceneNode, &Transform, Option<&TransformDirty>)` each frame.
- AC-3.2: Transform changes are detected via a `TransformDirty` marker component or by comparing against a `PreviousTransform` component.
- AC-3.3: Dirty transforms are debounced: at most 10 updates per second per entity (100ms minimum interval between writes for the same `node_id`).
- AC-3.4: On write, the system sends `DbCommand::UpdateNode` with the serialized transform (position as GeoJSON point + elevation, rotation as array, scale as array).
- AC-3.5: After sending the command, the `TransformDirty` / `PreviousTransform` is updated so the same value is not re-sent.
- AC-3.6: The debounce timer is per-entity, stored in a `SyncTimer` component with `last_sync: f64`.

**Priority:** P0

### FR-4: Scene Sync System (DB to ECS)

**Description:** When the DB reports changes to node records (via polling or event), update the corresponding ECS entities.

**Acceptance Criteria:**
- AC-4.1: A periodic polling system sends `DbCommand::ListNodes { petal_id }` at a configurable interval (default: 2 seconds) for the currently loaded Petal.
- AC-4.2: The response handler compares the returned node list against the `LoadedScene` entity map.
- AC-4.3: New nodes (present in DB but not in `LoadedScene`) are spawned.
- AC-4.4: Removed nodes (present in `LoadedScene` but not in DB response) are despawned.
- AC-4.5: Updated nodes (present in both, but with different transform values) have their `Transform` updated -- but only if the entity is NOT currently being edited locally (check for a `LocallyEditing` marker component).
- AC-4.6: The polling interval is stored in a `SceneSyncConfig` resource and can be adjusted at runtime.

**Priority:** P1

### FR-5: Asset Resolution

**Description:** Resolve a node's `asset_id` string from the DB into a loaded Bevy GLTF handle, with placeholder fallback.

**Acceptance Criteria:**
- AC-5.1: An `resolve_asset` function takes an `asset_id: &str` (hex-encoded BLAKE3 hash) and an `&AssetServer`, returning `Handle<Gltf>`.
- AC-5.2: The function decodes the hex string to `[u8; 32]`, constructs an `AssetId`, and delegates to `fe_renderer::loader::load_to_bevy`.
- AC-5.3: If the hex string is invalid or the asset is not cached, a placeholder cube mesh is spawned instead of a GLTF scene.
- AC-5.4: When the actual asset finishes downloading (via iroh-blobs, wired in a future sprint), the placeholder is replaced with the real GLTF scene. This is tracked via an `AssetPending { asset_id: String }` component.
- AC-5.5: Placeholder cubes use a distinct material (translucent grey, `Color::srgba(0.5, 0.5, 0.5, 0.4)`) so they are visually distinguishable from real models.

**Priority:** P0

### FR-6: Entity Lifecycle Management

**Description:** Handle the full create/update/delete lifecycle for scene entities, keeping the DB as the source of truth.

**Acceptance Criteria:**
- AC-6.1: **Create**: When a `DbResult::NodeCreated { node }` message arrives, spawn a new entity with all components and add it to `LoadedScene`.
- AC-6.2: **Update**: When a `DbResult::NodeUpdated { node }` arrives, update the ECS entity's `Transform` and metadata (unless `LocallyEditing`).
- AC-6.3: **Delete**: When a `DbResult::NodeDeleted { node_id }` arrives, despawn the entity and remove from `LoadedScene`.
- AC-6.4: Despawn uses `commands.entity(e).despawn()` to clean up the entire entity hierarchy (GLTF scenes have child entities).
- AC-6.5: All lifecycle operations log at `tracing::debug!` level with the `node_id` and operation type.

**Priority:** P0

### FR-7: Room Boundary Visualization

**Description:** Visualize Room bounds as wireframe boxes in the 3D viewport, toggleable via UI.

**Acceptance Criteria:**
- AC-7.1: When a Petal is loaded, also query `DbCommand::ListRooms { petal_id }` and spawn wireframe box entities for rooms that have `bounds` defined.
- AC-7.2: Each room wireframe entity has a `SceneRoom` marker component.
- AC-7.3: Wireframe boxes use `Gizmos` line drawing (not mesh wireframe) with a distinct color per room (cycling through a predefined palette).
- AC-7.4: A `ShowRoomBounds` resource (bool) controls visibility. Default: `true`.
- AC-7.5: Room labels are drawn at the center of each bounding box using `Gizmos::text` or a billboard text entity.
- AC-7.6: Room wireframes are not pickable/selectable (they are visual guides only).

**Priority:** P2

## Non-Functional Requirements

### NFR-1: Performance
- Scene loading for a Petal with 100 nodes must complete in under 3 seconds (DB query + entity spawn, excluding asset download time).
- Transform sync debounce must ensure no more than 10 DB writes per second per entity under continuous manipulation.
- Polling interval must not cause frame drops: the `ListNodes` query for 100 nodes must return within 50ms.
- Entity map lookup (`node_id -> Entity`) must be O(1) via HashMap.

### NFR-2: Thread Safety
- All DB interaction flows through the `DbCommandSender` / `DbResultReceiver` channel. No `block_on()` calls in Bevy systems.
- No direct SurrealDB access from Bevy systems (T1 thread). All DB operations execute on T3.

### NFR-3: Data Integrity
- The DB is the source of truth. If an ECS entity's transform diverges from the DB (e.g., due to a failed write), the next poll cycle will reconcile it.
- Transform serialization/deserialization is lossless for f32 precision.
- GeoJSON Point format for position: `{ "type": "Point", "coordinates": [x, z] }` where X maps to longitude and Z maps to latitude per the schema doc comment.

### NFR-4: Observability
- All scene loading, sync, and lifecycle events emit `tracing` spans/events at `debug` or `info` level.
- A `SceneStats` resource tracks: total entities loaded, pending assets, sync writes/sec, last poll timestamp.

## User Stories

### US-1: Loading a Petal Scene
**As** an operator,
**I want** to select a Petal and see its 3D content appear in the viewport,
**So that** I can visually inspect and manage my 3D space.

**Given** I have a Petal with 5 nodes placed via the Seedling Onboarding CRUD,
**When** I select that Petal in the sidebar,
**Then** 5 entities appear in the 3D viewport at their stored positions with the correct GLTF models (or placeholder cubes if assets are not cached).

### US-2: Editing a Transform
**As** an operator,
**I want** to move/rotate/scale a 3D object in the viewport and have it persist,
**So that** my spatial arrangement is saved without manual DB editing.

**Given** I have selected a node entity in the viewport,
**When** I modify its transform via the transform editor or a future gizmo,
**Then** the new transform is written to SurrealDB within 200ms of releasing the control, and survives an application restart.

### US-3: Seeing External Changes
**As** an operator running two windows (or receiving peer updates),
**I want** entities to appear/disappear/move when another source modifies the DB,
**So that** the 3D viewport reflects the current state of the world.

**Given** a new node is created via the CRUD panel while the Petal is loaded,
**When** the next poll cycle completes (within 2 seconds),
**Then** a new entity appears in the viewport at the correct position.

### US-4: Asset Not Yet Available
**As** an operator,
**I want** to see a placeholder when a model's asset hasn't been downloaded yet,
**So that** I know an object exists at that position even before the full model loads.

**Given** a node references an `asset_id` that is not in the local cache,
**When** the scene loads,
**Then** a translucent grey cube appears at the node's position, and it is replaced by the real GLTF model when the asset becomes available.

### US-5: Room Boundaries
**As** an operator,
**I want** to see the boundaries of each Room as wireframe boxes,
**So that** I can understand the spatial organization of my Petal.

**Given** a Petal has 3 rooms with defined bounds,
**When** I load the Petal with room bounds toggled on,
**Then** I see 3 colored wireframe boxes with room name labels at their centers.

## Technical Considerations

### Transform Serialization Format

The `node` table uses a GeoJSON Point for XZ position and a separate `elevation` float for Y. The bridge must convert between Bevy's `Transform` (translation Vec3, rotation Quat, scale Vec3) and the DB format:

```
DB -> ECS:
  translation.x = position.coordinates[0]
  translation.y = elevation
  translation.z = position.coordinates[1]
  rotation = Quat::from_array(rotation)  // stored as [x,y,z,w]
  scale = Vec3::from_array(scale)        // stored as [x,y,z]

ECS -> DB:
  position = { "type": "Point", "coordinates": [translation.x, translation.z] }
  elevation = translation.y
  rotation = [quat.x, quat.y, quat.z, quat.w]
  scale = [scale.x, scale.y, scale.z]
```

### Relationship to Bloom Stage

The Bloom Stage track defines `ModelMetadata` targeting the `model` table. This track defines `SceneNode` targeting the `node` table. The `node` table is the canonical entity record for 3D objects in the scene graph (introduced in the Seedling Onboarding track's schema). The `model` table from the original Wave 1 schema stores asset metadata and browser interaction URLs -- it is not the same as a scene-graph node.

The Scene Graph Bridge provides the low-level spawn/sync infrastructure. Bloom Stage's selection, highlighting, and inspector systems will query `SceneNode`-marked entities.

### Placeholder Cube Strategy

When `asset_id` is `None` or the asset is not cached:
1. Spawn a `Mesh3d` cube (1x1x1) with a translucent `StandardMaterial`.
2. Attach an `AssetPending { asset_id: String }` component.
3. A background system periodically checks if the asset has become available (file appeared in cache dir).
4. On availability, despawn the cube children and spawn the GLTF `SceneRoot` children in their place.

### Debounce Implementation

```rust
#[derive(Component)]
pub struct SyncTimer {
    pub last_sync: f64,
    pub min_interval: f64, // default 0.1 (100ms = 10 updates/sec)
}
```

The sync system checks `time.elapsed_secs_f64() - sync_timer.last_sync >= sync_timer.min_interval` before sending a `DbCommand::UpdateNode`.

### Polling vs. Live Queries

V1 uses polling (`ListNodes` every 2 seconds). V2 will migrate to SurrealDB live queries when the embedded engine supports stable live-query subscriptions (tracked in tech-stack.md `ReplicationStore` trait future-proofing row).

## Dependencies

| Dependency | Track | Status | What We Need |
|---|---|---|---|
| DbCommand CRUD variants | Seedling Onboarding (Phase 1) | Planned | `ListNodes`, `CreateNode`, `UpdateNode`, `DeleteNode` command/result variants |
| Node table schema | Petal Soil + Seedling Onboarding | Complete/Planned | `node` table with position, elevation, rotation, scale fields |
| Asset loader | Bloom Renderer | Complete | `load_to_bevy()`, `cache_path()`, `AssetId` |
| 3D camera + viewport | Bloom Stage | Planned | `Camera3d`, orbit camera (scene bridge can work with any camera) |
| Room metadata | Fractal Atlas | Complete | `RoomBounds`, `RoomMetadata` in `fe-database/src/atlas.rs` |
| Runtime channels | Seed Runtime | Complete | `DbCommandSender`, `DbResultReceiver`, drain systems |

## Out of Scope

- Multi-user real-time sync (handled by Fractal Mesh + Dead Reckoning integration)
- Physics or collision detection on scene entities
- Asset upload or transcoding (Petal Seed track)
- Undo/redo for transform changes
- Transform gizmo handles in the viewport (Bloom Stage track provides egui sliders; a future Gizmo track adds 3D handles)
- Scene serialization to BSN format (v2, per tech-stack.md)
- Multi-select operations
- LOD (level of detail) switching

## Open Questions

1. **Node vs. Model naming**: The DB uses `node` table for scene-graph entities, but `product.md` calls them "Models." Should the Bevy component be `SceneNode` (matching the table) or `SceneModel` (matching product terminology)? *Proposed: `SceneNode` to match the DB table; add a type alias `SceneModel = SceneNode` if needed for clarity.*
2. **Polling frequency**: Is 2 seconds acceptable for the DB-to-ECS sync, or should it be configurable per-Petal? *Proposed: configurable via `SceneSyncConfig` resource, default 2 seconds.*
3. **Transform conflict resolution**: If the user is editing a transform locally while a remote update arrives, which wins? *Proposed: local edit wins (skip remote update while `LocallyEditing` marker is present); reconcile on edit completion.*
4. **Maximum scene size**: Should there be a hard cap on entities per Petal to prevent performance degradation? *Proposed: soft warning at 500 nodes, no hard cap in v1.*
