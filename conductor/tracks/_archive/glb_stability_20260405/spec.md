# Specification: GLB Stability — Import, Render, and Navigation Fixes

## Overview

This track addresses four interrelated bugs in the current GLTF viewing and petal navigation flow that prevent fractalengine from functioning as a useful 3D editor. All four bugs surface simultaneously during normal use: importing a GLB, selecting nodes in the sidebar, and navigating between petals. These are independent defects that happen to compound, producing user-facing confusion about what state the application is actually in.

Intent: every GLB import is persisted across app restarts, rendered exactly once in the viewport at its stored world position, co-existing with the viewport grid, and isolated to the Petal that owns it.

## Background

### What exists

- **GLTF import pipeline**: right-click → "Add GLTF Model" opens file picker, writes to SurrealDB `asset` table (base64 `data`) + `node` table (geometry point position), copies file to `fractalengine/assets/imported/{asset_id}.glb` for Bevy to load
- **Hierarchy → scene spawn**: `fe-ui/src/plugin.rs::apply_db_results_to_hierarchy` reacts to `DbResult::GltfImported` and `DbResult::HierarchyLoaded`, spawning `SceneRoot` with the GLB handle + `Transform` from the node's `position`
- **Sidebar node click → camera focus**: `render_nodes` sends the node's position to `CameraFocusTarget`, which drives `OrbitCameraController::focus`
- **Viewport**: `fe-renderer::ViewportPlugin` adds `GridPlugin`, `CameraControllerPlugin`, `AxisGizmoPlugin`, lighting
- **Nav state**: `NavigationState { active_verse_id, active_fractal_id, active_petal_id }` drives which viewport overlay is shown (verse browser / fractal browser / petal browser / petal 3D space)

### Observed defects

#### Bug 1 — GLB not persisted across restarts
**Symptom:** import a GLB, close app, reopen → model is gone from viewport and sidebar even though DB was written.

**Suspected root cause:** likely one of (a) the `assets/imported/` write lands at the wrong path on Windows and the file is missing on reopen, (b) the `load_hierarchy_handler` query fails to detect the file exists (path check mismatch), (c) the content-hash format mismatch between write-side and read-side. Needs diagnosis.

#### Bug 2 — Visual duplication: static image + 3D model
**Symptom:** clicking a node in the sidebar shows a static 2D image of the model layered over the 3D scene, then the 3D model renders in place. User sees two different artifacts.

**Suspected root cause:** Bevy's GLTF loader is producing BOTH a scene AND a thumbnail/image asset that gets rendered as a UI element, OR the viewport overlay is painting a cached snapshot. Likely related to the petal-browser card UI (which shows a duck image on petal select/hover) leaking into the petal-space viewport.

#### Bug 3 — Grid disappears when GLB renders
**Symptom:** as soon as a GLB model becomes visible, the infinite grid disappears; the GLB replaces it visually.

**Suspected root cause:** `GridPlugin` shader or mesh is being culled/covered by the GLTF scene's bounding volume, OR the GLTF scene has an opaque ground plane that visually blocks the grid, OR the GLB handle spawn introduces lighting/camera changes that affect grid rendering.

#### Bug 4 — Petal navigation doesn't swap scene
**Symptom:** creating a new petal and entering it shows the same nodes/models from the previous petal. The viewport scene does not refresh based on `active_petal_id`.

**Suspected root cause:** no system despawns entities from the old petal on navigation change, AND/OR the initial `HierarchyLoaded` spawns all nodes from all petals globally instead of scoping to the active petal. The spawn path currently iterates all `verses → fractals → petals → nodes` and spawns regardless of petal membership.

### What is missing

- A diagnostic step to verify where GLB files actually land on Windows (the import path has been changed recently across several commits)
- A `SpawnedSceneNode` marker component with `petal_id` so we can scope despawn on petal change
- A system that despawns scene entities when `NavigationState::active_petal_id` changes
- A filter in the spawn path that only instantiates nodes belonging to the active petal
- Investigation into the "static image" artifact — likely a Bevy GLTF asset ghost or UI layer leak

## Functional Requirements

### FR-1: GLB Persists Across Restarts

**Description:** An imported GLB file, its DB row, and its scene entity survive a clean app close/reopen.

**Acceptance Criteria:**
- AC-1.1: After importing `duck.glb` and quitting, the file exists at a stable, verified path (`fractalengine/assets/imported/{asset_id}.glb` on Windows with forward-slash-safe path construction)
- AC-1.2: On app restart, the hierarchy loader finds the file at the expected path and populates `NodeHierarchyData::asset_path` with `Some("imported/{asset_id}.glb")`
- AC-1.3: The viewport renders the imported model at its stored `position`/`elevation` within 2 seconds of app open
- AC-1.4: The sidebar tree shows the node under its correct petal with `has_asset: true` (diamond icon)
- AC-1.5: A debug log line `Spawned persisted GLTF '{name}' at {pos}` appears for each persisted node on startup

**Priority:** P0

### FR-2: No Visual Duplication

**Description:** Each node renders exactly one visual artifact — the 3D GLTF scene. No static 2D images, no ghost thumbnails, no card previews layered over the 3D viewport.

**Acceptance Criteria:**
- AC-2.1: Clicking a node in the sidebar produces exactly one visible 3D model at the correct position
- AC-2.2: No 2D image or thumbnail artifact is visible in the viewport for spawned nodes
- AC-2.3: Verify no UI layer (petal card, hover preview, context menu preview) paints inside the viewport rect when a petal is active
- AC-2.4: When the viewport is in `viewport_petal_space` (active petal), no `viewport_petal_browser` elements leak through

**Priority:** P0

### FR-3: Grid Co-Exists With Rendered Models

**Description:** The infinite ground grid remains visible even when GLTF scenes are spawned. Models do not visually replace or cull the grid.

**Acceptance Criteria:**
- AC-3.1: After spawning one or more GLTF models, the grid is still visible at its expected plane (Y=0)
- AC-3.2: Models render ABOVE the grid (z-ordering correct) but do not cull or fade the grid
- AC-3.3: Camera rotation/pan/zoom works normally with both grid and models visible
- AC-3.4: If the GLB contains its own ground plane, document the behavior (engine grid still renders beneath or alongside)

**Priority:** P1

### FR-4: Petal-Scoped Scene Rendering

**Description:** The viewport renders only nodes belonging to the currently active petal. Navigating to a different petal swaps the rendered scene.

**Acceptance Criteria:**
- AC-4.1: When `NavigationState::active_petal_id` is `Some(petal_A)`, only nodes where `node.petal_id == petal_A` are spawned as scene entities
- AC-4.2: When the active petal changes from `petal_A` to `petal_B`, all scene entities from `petal_A` are despawned BEFORE nodes from `petal_B` are spawned
- AC-4.3: Creating a new empty petal and entering it shows an empty viewport (just the grid, no inherited models from the parent fractal)
- AC-4.4: Navigating back (Back button) from a petal to the fractal browser despawns that petal's scene entities
- AC-4.5: A `SpawnedNodeMarker { node_id, petal_id }` component is attached to every GLTF scene entity spawned by the hierarchy system, enabling scoped despawn

**Priority:** P0

## Non-Functional Requirements

### NFR-1: No Regressions
The bug-fix track must not break any currently working flow: right-click import, sidebar hierarchy display, camera orbit/pan/zoom, node click → camera focus.

### NFR-2: Diagnostic Logging
All four fixes include tracing::debug! logs at key decision points so future regressions can be diagnosed from logs alone.

### NFR-3: No Architectural Rewrites
This track is a pure bug-fix. No new abstractions, no refactoring, no new dependencies. Minimal, targeted changes.

## Out of Scope

- Migrating to content-hash / blob-store architecture (that's `p2p_mycelium_20260405`)
- Adding a proper scene graph bridge system (that's `scene_graph_bridge_20260402`)
- Improving the camera navigation animation (camera snaps directly today; smoothing is future work)
- Fixing the base64-in-DB size bloat
- Grid customization (size, color, snap) — out of scope here

## Success Criteria

A user can:
1. Import a duck.glb into Genesis Petal → duck renders in viewport with grid visible
2. Close the app, reopen → duck is still there at the same position, exactly one visible
3. Create a new petal "Workshop", enter it → viewport is empty (just grid)
4. Navigate back to Genesis Petal → duck reappears, still exactly one
5. Create Workshop > import a different GLB → both petals render their own models in isolation
