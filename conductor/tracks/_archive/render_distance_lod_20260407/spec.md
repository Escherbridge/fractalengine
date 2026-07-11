# Specification: Render Distance & Progressive LOD

## Overview

Adds a user-configurable render distance system within petals and progressive Level-of-Detail (LOD) for nodes, so only nearby objects consume GPU cycles and bandwidth. This is the **render interest** layer — what gets drawn and at what fidelity. It operates within the already-scoped petal volume.

This track is the spatial/visual half of the dual-interest model. The data/sync half lives in `relay_data_horizon_20260407`.

## Background

### Current State

- Nodes are spawned unconditionally when entering a petal — every node with an `asset_path` gets a full `SceneRoot` regardless of distance
- No Bevy `Visibility` components are used anywhere in the codebase
- No frustum culling, LOD, or distance-based filtering exists
- Camera is Blender-style orbit (`OrbitCameraController`) with `max_distance: 500.0` zoom clamp
- Each node stores a single blob hash (`content_hash`) — one resolution only
- No `AppSettings` or user preferences system exists

### Why This Matters

A petal with thousands of nodes (a dense city, a gallery with hundreds of models) will:
- Spawn thousands of Bevy entities with full-res GLB scenes
- Fetch thousands of blobs from the blob store (or network)
- Consume unbounded GPU memory

With render distance: only nearby nodes get full meshes. Distant nodes get low-poly or bounding boxes. Beyond the hard limit, nothing renders.

## Design

### Render Distance Tiers

```
┌─────────────────────────────────────────────────┐
│                  Petal Bounds                    │
│                                                 │
│    ┌─────────────────────────────────────┐      │
│    │         Hard Limit (1000m)          │      │
│    │   ┌─────────────────────────┐      │      │
│    │   │    User Distance (var)  │      │      │
│    │   │   ┌───────────────┐    │      │      │
│    │   │   │  Full Detail  │    │      │      │
│    │   │   │  (< 30% dist) │    │      │      │
│    │   │   └───────────────┘    │      │      │
│    │   │    Mid Detail          │      │      │
│    │   │    (30%-70% dist)      │      │      │
│    │   └─────────────────────────┘      │      │
│    │     Low Detail / Bounding Box      │      │
│    │     (70%-100% dist)                │      │
│    └─────────────────────────────────────┘      │
│       Hidden (beyond user distance)             │
└─────────────────────────────────────────────────┘
```

### LOD Tiers Per Node

Each node can have up to 3 blob resolutions stored in its metadata:

| Tier | Field | Content | When Used |
|------|-------|---------|-----------|
| `lod_high` | `content_hash` (existing) | Full-res GLB | Distance < 30% of render distance |
| `lod_mid` | `lod_mid_hash` | Decimated GLB (~25% polys) | Distance 30%-70% |
| `lod_low` | `lod_low_hash` | Bounding box / billboard | Distance 70%-100% |
| Hidden | — | Not rendered | Distance > render distance |

Nodes without LOD variants fall back gracefully: if `lod_mid_hash` is null, use `content_hash` at all visible distances. This makes LOD opt-in per node.

### Settings System

New `AppSettings` Bevy resource, persisted as RON to `~/.config/fractalengine/settings.ron`:

```rust
#[derive(Resource, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    /// User's render distance in world units. Clamped to [50, HARD_LIMIT].
    pub render_distance: f32,
    /// Camera sensitivity multiplier.
    pub camera_sensitivity: f32,
    /// Camera zoom speed multiplier.
    pub camera_zoom_speed: f32,
}

/// Absolute maximum render distance. Cannot be exceeded by user settings.
pub const RENDER_DISTANCE_HARD_LIMIT: f32 = 1000.0;
pub const RENDER_DISTANCE_DEFAULT: f32 = 300.0;
pub const RENDER_DISTANCE_MIN: f32 = 50.0;
```

## Phases

### Phase 1: Settings System

Create `AppSettings` resource with RON persistence. Wire to camera defaults.

**Files to create/modify:**
- `fe-ui/src/settings.rs` (new) — `AppSettings` struct, load/save, defaults
- `fe-ui/src/plugin.rs` — register resource, add settings panel to UI
- `fe-ui/src/lib.rs` — add `mod settings`

**Changes:**
1. `AppSettings` struct with `render_distance`, `camera_sensitivity`, `camera_zoom_speed`
2. `load_settings()` — reads from `~/.config/fractalengine/settings.ron`, returns defaults on missing/corrupt
3. `save_settings()` — writes RON on change (debounced, not every frame)
4. `SettingsPanel` — egui window with sliders for each setting
5. On startup: load settings, apply `camera_sensitivity` and `camera_zoom_speed` to `OrbitCameraController`
6. `render_distance` slider with hard limit clamp

**Tests:**
- Roundtrip: serialize → deserialize settings
- Corrupt file → defaults
- Clamp: setting render_distance to 2000 → clamped to 1000

**Exit criteria:** Settings persist across app restarts. Slider visible in UI.

---

### Phase 2: Render Distance Volume

Add `RenderVolume` resource that follows the camera. Drive Bevy `Visibility` from distance.

**Files to create/modify:**
- `fe-renderer/src/render_volume.rs` (new) — `RenderVolume` resource + update system
- `fe-renderer/src/lib.rs` — add module, register systems
- `fe-ui/src/plugin.rs` — tag spawned nodes with position data for distance checks

**Changes:**
1. `RenderVolume` resource: `center: Vec3` (from camera focus), `radius: f32` (from `AppSettings::render_distance`)
2. `update_render_volume` system: runs in `PostUpdate`, reads `OrbitCameraController::focus` + `AppSettings::render_distance`, updates `RenderVolume`
3. `apply_render_distance` system: queries `Query<(&mut Visibility, &Transform), With<SpawnedNodeMarker>>`, sets `Visibility::Hidden` for nodes beyond `render_distance`, `Visibility::Inherited` for nodes within
4. Add `Visibility::default()` to all spawned node entities in `apply_db_results_to_hierarchy`
5. Performance: only check distance for nodes that moved or when camera moves significantly (delta > 1m)

**Tests:**
- Unit test: node at distance 100, render_distance 200 → Visible
- Unit test: node at distance 300, render_distance 200 → Hidden
- Unit test: render_distance change from 200→100 hides previously visible node
- Unit test: RenderVolume updates when camera focus moves

**Exit criteria:** Nodes beyond render distance are not rendered. Changing the slider dynamically hides/shows nodes.

---

### Phase 3: LOD Schema & Storage

Extend the node/asset schema to support multiple resolution blobs.

**Files to modify:**
- `fe-database/src/schema.rs` — add LOD hash fields to node schema
- `fe-database/src/lib.rs` — update `import_gltf_handler` to accept LOD tier, update `LoadHierarchy` to return LOD hashes
- `fe-runtime/src/messages.rs` — extend `NodeHierarchyData` with LOD fields

**Changes:**
1. Schema migration: `DEFINE FIELD IF NOT EXISTS lod_mid_hash ON TABLE node TYPE option<string>;`
2. Schema migration: `DEFINE FIELD IF NOT EXISTS lod_low_hash ON TABLE node TYPE option<string>;`
3. `NodeHierarchyData`: add `lod_mid_hash: Option<String>`, `lod_low_hash: Option<String>`
4. `LoadHierarchy` handler: include LOD hashes in query
5. `ImportGltf` command: add optional `lod_tier: LodTier` enum (`High`, `Mid`, `Low`) — importing a model can target a specific LOD slot
6. UI: in model editor, allow uploading LOD variants per node

**Tests:**
- Schema migration is idempotent
- Node with all 3 LOD hashes roundtrips through hierarchy load
- Node with only `content_hash` (no LOD) still works (backward compatible)

**Exit criteria:** Nodes can store up to 3 blob hashes. Existing nodes without LOD continue to work.

---

### Phase 4: LOD Switching System

Switch which blob is loaded based on distance tier.

**Files to create/modify:**
- `fe-renderer/src/lod.rs` (new) — LOD tier computation + asset swapping
- `fe-ui/src/plugin.rs` — spawn nodes with LOD-aware components

**Changes:**
1. `LodState` component: attached to each spawned node, stores `current_tier: LodTier`, `high_hash`, `mid_hash`, `low_hash`
2. `compute_lod_tier(distance, render_distance) -> LodTier` pure function:
   - `< 0.3 * render_distance` → `High`
   - `< 0.7 * render_distance` → `Mid`
   - `<= render_distance` → `Low`
   - `> render_distance` → `Hidden`
3. `lod_switching_system`: queries `Query<(&mut LodState, &Transform, &mut Handle<Scene>)>`, computes tier from camera distance, if tier changed → swap the scene handle to the appropriate blob's asset
4. Hysteresis: don't switch tiers on every frame oscillation. Require the node to stay in the new tier for 0.5s (or 2m distance hysteresis band) before switching.
5. Blob prefetch: when a node enters the `Mid` tier, start a background `FetchBlob` for its `High` hash so it's ready when the user gets closer.

**Tests:**
- Unit test: `compute_lod_tier` at various distances
- Unit test: hysteresis prevents rapid switching
- Unit test: node without mid LOD falls back to high at all visible distances

**Exit criteria:** Nodes swap between LOD tiers based on camera distance. Transitions are smooth (no pop-in flicker).

---

### Phase 5: Spatial Blob Fetch Priority

Only fetch blobs for nodes within the render volume + prefetch ring. Prioritize by distance.

**Files to modify:**
- `fe-ui/src/plugin.rs` — filter blob fetches by distance
- `fe-sync/src/sync_thread.rs` — add priority queue for FetchBlob commands
- `fe-sync/src/messages.rs` — add priority field to FetchBlob

**Changes:**
1. `FetchBlob` command: add `priority: FetchPriority` enum (`Urgent`, `Background`, `Prefetch`)
2. Sync thread: process `Urgent` fetches before `Background` before `Prefetch`
3. In `apply_db_results_to_hierarchy`: when spawning nodes, only issue `FetchBlob` for nodes within `render_distance * 1.5` (prefetch ring). Nodes beyond → no fetch.
4. When `RenderVolume` moves: check newly-in-range nodes and issue `FetchBlob` for them.
5. When nodes leave the prefetch ring: cancel pending fetches (if not already complete).

**Tests:**
- Node inside volume: FetchBlob issued
- Node outside volume: no FetchBlob
- Node enters prefetch ring: background FetchBlob issued
- Priority ordering: Urgent processed before Background

**Exit criteria:** Blob fetches are distance-filtered and prioritized. No blobs fetched for nodes beyond 1.5× render distance.

## Dependencies

- `mycelium_scaling_20260407` Phase 2 (streaming asset loading) should land first — LOD switching will load/unload assets frequently
- `mycelium_scaling_20260407` Phase 1 (backpressure) should land first — FetchBlob priority queue needs proper channel handling

## Relationship to relay_data_horizon_20260407

This track controls **what gets rendered** (GPU interest). The relay/data horizon track controls **what gets synced** (network interest). They share:
- `AppSettings` resource (this track creates it, relay track extends it)
- `FetchBlob` priority system (this track adds priority, relay track adds source selection)
- Node hierarchy data (this track adds LOD hashes, relay track adds sync scope)
