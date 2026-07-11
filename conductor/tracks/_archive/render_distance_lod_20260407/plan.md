# Implementation Plan: Render Distance & Progressive LOD

## Phase 1: Settings System
Status: pending

### Tasks
- [ ] 1.1 Create `fe-ui/src/settings.rs` with `AppSettings` struct (render_distance, camera_sensitivity, camera_zoom_speed)
- [ ] 1.2 Implement `load_settings()` from `~/.config/fractalengine/settings.ron` with defaults fallback
- [ ] 1.3 Implement `save_settings()` with debounced write on change
- [ ] 1.4 Add settings panel UI (egui sliders with hard limit clamp)
- [ ] 1.5 Wire `AppSettings` into `OrbitCameraController` defaults on startup
- [ ] 1.6 Write tests: roundtrip, corrupt file, clamp validation

## Phase 2: Render Distance Volume
Status: pending

### Tasks
- [ ] 2.1 Create `fe-renderer/src/render_volume.rs` with `RenderVolume` resource
- [ ] 2.2 Implement `update_render_volume` system (camera focus + settings → volume)
- [ ] 2.3 Implement `apply_render_distance` system (toggle Bevy Visibility by distance)
- [ ] 2.4 Add `Visibility::default()` to spawned node entities in plugin.rs
- [ ] 2.5 Add delta threshold to avoid per-frame distance checks
- [ ] 2.6 Write tests: visible/hidden by distance, dynamic resize, volume tracking

## Phase 3: LOD Schema & Storage
Status: pending

### Tasks
- [ ] 3.1 Add `lod_mid_hash` and `lod_low_hash` fields to node schema
- [ ] 3.2 Extend `NodeHierarchyData` with LOD hash fields
- [ ] 3.3 Update `LoadHierarchy` handler to query LOD hashes
- [ ] 3.4 Add `LodTier` enum and extend `ImportGltf` to accept LOD tier
- [ ] 3.5 Add LOD variant upload UI in model editor
- [ ] 3.6 Write tests: schema idempotency, backward compatibility, roundtrip

## Phase 4: LOD Switching System
Status: pending

### Tasks
- [ ] 4.1 Create `LodState` component with tier tracking
- [ ] 4.2 Implement `compute_lod_tier(distance, render_distance)` pure function
- [ ] 4.3 Implement `lod_switching_system` (tier change → scene handle swap)
- [ ] 4.4 Add hysteresis (0.5s / 2m band) to prevent rapid switching
- [ ] 4.5 Add LOD prefetch (entering Mid tier triggers High blob fetch)
- [ ] 4.6 Write tests: tier computation, hysteresis, fallback without LOD variants

## Phase 5: Spatial Blob Fetch Priority
Status: pending

### Tasks
- [ ] 5.1 Add `FetchPriority` enum to `SyncCommand::FetchBlob`
- [ ] 5.2 Implement priority queue in sync thread (Urgent > Background > Prefetch)
- [ ] 5.3 Filter blob fetches by render_distance * 1.5 prefetch ring
- [ ] 5.4 Issue FetchBlob when nodes enter prefetch ring on camera move
- [ ] 5.5 Cancel pending fetches for nodes leaving prefetch ring
- [ ] 5.6 Write tests: distance filtering, priority ordering, ring enter/leave
