# Implementation Plan: GLB Stability — Import, Render, Navigation Fixes

## Overview

4 phases, one per bug. Each phase is a diagnose → fix → verify cycle. Phase 4 (petal scoping) is the most likely to require a new component and system; the others should be targeted small edits.

- **Phase 1**: Diagnose and fix GLB persistence
- **Phase 2**: Eliminate visual duplication
- **Phase 3**: Restore grid visibility
- **Phase 4**: Petal-scoped scene entity lifecycle

Estimated total: 14 tasks across 4 phases.

---

## Phase 1: Fix GLB Persistence Across Restarts

Goal: Diagnose where GLB files actually land, ensure consistent path construction between write-side and read-side, and verify persistence end-to-end.

Tasks:

- [ ] Task 1.1: Add diagnostic logging to `import_gltf_handler` in `fe-database/src/lib.rs`: log the absolute path of the written GLB file and the CWD at write time (TDD: write test asserting the log contains `absolute:` prefix and canonical path; implement; refactor)
- [ ] Task 1.2: Add diagnostic logging to `load_hierarchy_handler` when checking asset file existence: log the resolved path and whether `glb_path.exists()` returned true/false (TDD: write test for log content; implement; refactor)
- [ ] Task 1.3: Fix path construction to use `std::path::Path::join` consistently rather than `format!("{}/{}", ...)` so Windows path separators are correct (TDD: write unit test that on Windows, a path built via `join` contains `\\` not `/`; implement; refactor)
- [ ] Task 1.4: Ensure the imported-assets directory is computed from a single constant (`IMPORTED_ASSETS_SUBDIR: &str = "imported"`) used by both writer and reader — no duplicated literal (TDD: write test that writer and reader both reference the same constant via a canary string; implement; refactor)
- [ ] Verification: cargo test, manual test: import duck.glb, quit app, verify file exists at `fractalengine/assets/imported/*.glb`, reopen, confirm duck is visible. Repeat 3x to rule out flakiness. [checkpoint marker]

---

## Phase 2: Eliminate Visual Duplication

Goal: Identify and remove the source of the static-image artifact that appears alongside the 3D model.

Tasks:

- [ ] Task 2.1: Audit `viewport_petal_browser` and `viewport_petal_space` in `fe-ui/src/panels.rs` for any code path that paints an image or card when `active_petal_id` is `Some` — petal browser should only render when `active_petal_id` is `None` (TDD: write unit test that when `active_petal_id.is_some()`, the `viewport_petal_browser` function is never called; implement correct branching in `viewport_overlay`; refactor)
- [ ] Task 2.2: Verify the petal card hover/preview does not paint its GLB thumbnail into the shared viewport rect — it should be clipped to its own card rect (TDD: visual test — manual; implement any clipping needed in the card rendering code; refactor)
- [ ] Task 2.3: Check whether Bevy's GLTF loader asset path with `#Scene0` suffix is producing a secondary `Image` asset that Bevy paints somewhere by default; if so, explicitly load only `Gltf` scene root not the whole `Gltf` asset (TDD: write test asserting only one `SceneRoot` spawns per imported GLB; implement; refactor)
- [ ] Verification: cargo test, manual test: import GLB, click node in sidebar, assert exactly one visual artifact is visible. Check for the static-image artifact at every camera angle. [checkpoint marker]

---

## Phase 3: Restore Grid Visibility

Goal: The grid continues to render after one or more GLTF scenes are spawned.

Tasks:

- [ ] Task 3.1: Diagnose grid disappearance with a minimal repro: spawn one GLTF at origin, check grid with/without model, check grid shader compile logs (TDD: write Bevy test that spawns a known-good GLB and asserts the grid mesh entity still exists post-spawn; implement; refactor)
- [ ] Task 3.2: If the GLB has an opaque ground plane, document + move the grid Y offset to `-0.01` so it stays visible beneath (TDD: write test asserting grid plane is at correct Y; implement in `fe-renderer/src/grid.rs`; refactor)
- [ ] Task 3.3: If the issue is render ordering, assign the grid material to a later render stage (`alpha_mode: AlphaMode::Blend` or `render_queue: RenderQueue::Transparent`) (TDD: write integration test; implement; refactor)
- [ ] Task 3.4: Verify that spawning a scene does not remove or replace the grid entity by accident (e.g., no system is deduping entities by name and removing the grid) (TDD: write test counting grid entities before/after scene spawn; implement; refactor)
- [ ] Verification: cargo test, manual test: fresh DB, import duck.glb → grid visible alongside duck. Orbit camera 360° → grid remains visible at all angles. [checkpoint marker]

---

## Phase 4: Petal-Scoped Scene Lifecycle

Goal: Spawn scene entities scoped to the active petal, despawn on petal change, spawn fresh on petal entry.

Tasks:

- [ ] Task 4.1: Define `SpawnedNodeMarker { node_id: String, petal_id: String }` component in `fe-ui/src/plugin.rs` (TDD: write test that the component derives `Component, Debug, Clone`; implement; refactor)
- [ ] Task 4.2: Update the GLTF spawn sites in `apply_db_results_to_hierarchy` to attach `SpawnedNodeMarker { node_id, petal_id }` to every `SceneRoot` spawn (TDD: write test that a spawned entity has the marker with correct field values; implement at both `GltfImported` and `HierarchyLoaded` spawn paths; refactor)
- [ ] Task 4.3: Add a new system `despawn_nodes_on_petal_change` that watches `NavigationState` and despawns any entity with `SpawnedNodeMarker` whose `petal_id` does not match the active petal (or despawns ALL when `active_petal_id` is `None`) (TDD: write test: spawn 2 markers for petal A + 1 for petal B, set active_petal_id = B, assert only the B entity remains; implement; refactor)
- [ ] Task 4.4: Ensure `HierarchyLoaded` only spawns nodes whose `petal_id` matches the current `active_petal_id` — filter the inner loop (TDD: write test that given a hierarchy with 2 petals × 2 nodes each, and active_petal_id = A, only 2 entities are spawned not 4; implement; refactor)
- [ ] Task 4.5: When `active_petal_id` becomes `Some(x)` (e.g., entering a petal for the first time this session), re-request the nodes for that petal and spawn them — handled by a small "petal-enter spawn" system that runs after `despawn_nodes_on_petal_change` (TDD: write test: start with active_petal_id = None, set to Some(A), assert spawn system requests + spawns nodes for A; implement; refactor)
- [ ] Verification: cargo test, manual test: create petal A with duck, petal B empty, petal C with cube. Navigate A → B → C → A. At each step: viewport shows only the correct petal's models. Grid remains visible. `cargo clippy -- -D warnings` [final checkpoint]

---

## Verification Strategy

Each phase ends with its own manual test. Cumulative final manual script:

1. Delete `data/fractalengine.db` and `fractalengine/assets/imported/` → start from clean
2. Run app → Genesis hierarchy loads, grid visible, no models
3. Right-click in viewport → "Add GLTF Model" → pick a GLB → Import
4. Verify: exactly one model visible at the picked position, grid still visible, no static image artifact
5. Close app
6. Reopen app → navigate into Genesis Petal → same model renders at same position
7. Navigate back → create new petal "Workshop" → enter it → viewport empty except grid
8. Import a second GLB into Workshop
9. Navigate: Genesis Petal (duck) → Workshop (new GLB) → Genesis Petal (duck) — each swap works cleanly
10. Assert no console errors, no `DB error:` logs

## Risk Register

- **R1: Grid shader issue might require material-level changes** (Low prob, Low impact): isolate in Phase 3; use standard Bevy material properties
- **R2: Despawning + respawning causes flicker during petal navigation** (Medium prob, Low impact): acceptable for v1; smooth transitions are future work
- **R3: Static-image artifact turns out to be a Bevy engine quirk** (Medium prob, Medium impact): may need to upstream a bug report; workaround by loading scene directly

## Dependencies on Other Tracks

- **None required as prerequisites** — this track is self-contained
- **Should land before p2p_mycelium_20260405** — the latter refactors the import path and benefits from stable baseline rendering

## Estimated Effort

- Phase 1: ~0.5 days (path audit + logging)
- Phase 2: ~1 day (requires investigation of the static artifact)
- Phase 3: ~0.5 days (grid z-order or alpha fix)
- Phase 4: ~1 day (new component + 2 new systems)

**Total: ~3 days of focused bug-fixing.** Each phase can be shipped independently.
