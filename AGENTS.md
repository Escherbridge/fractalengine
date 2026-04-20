# AGENTS.md — FractalEngine Architecture Guide

This file is the canonical reference for AI agents (Claude, Codex, etc.) working on this codebase.
Read this before touching any source file.

---

## Project Overview

FractalEngine is a Bevy + egui 3D collaborative scene editor with peer-to-peer sync via iroh-docs.
Users navigate a 4-level content hierarchy (Verse > Fractal > Petal > Node), import GLTF/GLB assets
as scene entities, and can share a live scene with peers over a P2P replica backed by iroh-docs
namespaces. The editor is a single desktop application built with Bevy's ECS, using egui panels
for all UI chrome.

Key technologies:
- **Bevy 0.15+**: ECS, asset loading, gizmos, events (MessageReader)
- **bevy_egui**: All UI panels rendered via egui inside a Bevy app
- **iroh-docs**: P2P document sync — each Verse has a namespace_id that identifies its replica
- **SQLite** (via `fe-database`): local persistence of the full hierarchy and asset metadata
- **crossbeam channels**: DB thread and sync thread communicate back to Bevy via channels + MessageReader

---

## Crate Structure

```
fractalengine/          (workspace root)
├── fractalengine/      Binary entry point — wires all plugins together
├── fe-ui/              All egui UI, domain managers, and Bevy plugin wiring
├── fe-renderer/        Orbit camera controller (OrbitCameraController + CameraControllerPlugin)
├── fe-database/        Async DB functions (SQLite), called from a spawned thread
├── fe-sync/            P2P sync thread (iroh-docs); SyncCommand / SyncCommandSenderRes
├── fe-runtime/         Shared message types: DbCommand, DbResult, SyncCommand
├── fe-network/         Low-level iroh networking helpers
├── fe-webview/         Embedded webview for node webpage_url display
├── fe-auth/            Authentication / identity helpers
├── fe-identity/        Identity key management
└── fe-test-harness/    Integration test utilities
```

Dependency direction (fe-ui is the leaf that imports everything):

```
fe-ui ──► fe-renderer
      ──► fe-database
      ──► fe-runtime
      ──► fe-sync ──► fe-network
                  ──► fe-runtime
                  ──► fe-database
```

---

## The 4-Level Hierarchy

```
Verse  (namespace_id → iroh-docs P2P replica)
  └── Fractal  (logical grouping)
        └── Petal  (one active scene at a time — entities spawned for this level)
              └── Node  (a GLTF/GLB scene entity with Transform, asset_path, webpage_url)
```

Only one Petal is "active" at a time (tracked by `NavigationManager::active_petal_id`).
When the active petal changes, all entities from the old petal are despawned and entities
belonging to the new petal are spawned — directly from the in-memory tree, no DB round-trip.

---

## The 3 Manager Pattern

All domain state is owned by one of three manager Resources. No other system should mutate
manager state directly; call the manager's methods.

### NavigationManager (`fe-ui/src/navigation_manager.rs`)

Owns the active cursor: which verse/fractal/petal the user is currently viewing.

Fields:
- `active_verse_id: Option<String>`
- `active_verse_name: String`
- `active_fractal_id: Option<String>`
- `active_fractal_name: String`
- `active_petal_id: Option<String>`

Key methods (all clear downstream selections when navigating up):
- `navigate_to_verse(id, name)` — clears fractal + petal
- `navigate_to_fractal(id, name)` — clears petal
- `navigate_to_petal(id)` — sets petal
- `back_from_verse()` / `back_from_fractal()` / `back_from_petal()` — clear respective levels

Side effect: `NavigationManagerPlugin` runs `handle_verse_replica_lifecycle` every frame.
When `active_verse_id` changes, it sends `SyncCommand::CloseVerseReplica` for the old verse
and `SyncCommand::OpenVerseReplica` for the new one (using the namespace_id from VerseManager).

### VerseManager (`fe-ui/src/verse_manager.rs`)

Owns the complete in-memory content tree (all Verses, Fractals, Petals, Nodes).

Key data: `verses: Vec<VerseEntry>` — fully denormalised tree with asset_path retained on NodeEntry.

Key methods:
- `all_nodes()` — iterator over every NodeEntry across the whole tree
- `find_petal(id)` — immutable petal lookup
- `find_verse_mut(id)` — mutable verse lookup for push operations
- `update_node_position(node_id, [f32; 3])` — walks tree and updates position in-memory
- `update_node_url(node_id, url)` — walks tree and updates webpage_url in-memory

`VerseManagerPlugin` runs two systems:
1. `apply_db_results` — processes `DbResult` messages; rebuilds tree or appends entries;
   also auto-navigates on first load
2. `respawn_on_petal_change` — detects `active_petal_id` changes, despawns old entities,
   spawns new entities from in-memory data (no DB round-trip)

### NodeManager (`fe-ui/src/node_manager.rs`)

Owns node selection and the active gimbal drag state.

Key data: `selected: Option<NodeSelection>` where `NodeSelection` holds entity, node_id, and
an optional `AxisDrag` (in-progress gimbal interaction).

Key methods:
- `select(entity, node_id)` — selects; preserves drag if same entity, resets if different
- `deselect()` — clears selection
- `is_selected()`, `selected_entity()`, `is_dragging()` — read-only queries

`NodeManagerPlugin` chains 7 systems deterministically:
1. `handle_tool_shortcuts` — S/G/R/X/Escape keyboard shortcuts
2. `sync_sidebar_to_manager` — sidebar selection drives NodeManager
3. `handle_gimbal_interaction` — axis pick + drag + commit (writes Transform component)
4. `handle_viewport_click` — ray-cast pick / deselect
5. `sync_manager_to_inspector` — NodeManager drives InspectorFormState (display only)
6. `draw_gimbal_system` — calls gimbal.rs pure drawing functions
7. `broadcast_transform` — on drag_committed: write DB + P2P + update VerseManager position

---

## Key Bevy Patterns Used

### Plugin structs

Every domain area exposes a Plugin. The main plugin (`GardenerConsolePlugin` in `plugin.rs`)
adds the three domain manager plugins then registers UI-only resources and systems.

### System chains

`NodeManagerPlugin` uses `.chain()` to guarantee the 7 systems run in order within the same
`Update` schedule. This prevents the classic "reads stale state" bug where a later system
would see data from the previous frame.

### MessageReader (Bevy Events)

DB results arrive via `MessageReader<DbResult>` — not a shared Mutex. The DB thread sends
`DbResult` values through a channel; the runtime converts them to Bevy events before the
Update schedule runs.

### Local<T> for per-system state

`respawn_on_petal_change` and `handle_verse_replica_lifecycle` both use `Local<Option<String>>`
to track "what petal/verse was active last frame" without exposing that value globally.

### Commands (deferred ECS mutations)

Entities are spawned/despawned via `Commands`. Despawns are deferred — entities despawned in
a system are still visible in queries for the remainder of that frame. The respawn logic
collects `kept_node_ids` before issuing despawn commands to avoid relying on query state
after commands are issued.

### SpawnedNodeMarker component

Every scene entity carries `SpawnedNodeMarker { node_id, petal_id }` so the petal-switch
system can identify which entities to keep and which to despawn.

### OrbitCameraController (fe-renderer)

The 3D viewport camera uses a Blender-style orbit controller. All math (orbit, pan, zoom)
is in pure functions in `fe-renderer/src/camera.rs` — no ECS dependencies, fully unit-testable.

---

## File Ownership Map

| File | Owns |
|------|------|
| `fe-ui/src/navigation_manager.rs` | `NavigationManager` resource + `NavigationManagerPlugin` + P2P lifecycle system |
| `fe-ui/src/verse_manager.rs` | `VerseManager` resource + `VerseManagerPlugin` + hierarchy tree types + spawn helper |
| `fe-ui/src/node_manager.rs` | `NodeManager` resource + `NodeManagerPlugin` + all 7 chained systems |
| `fe-ui/src/gimbal.rs` | `GimbalAxis` enum + pure visual drawing functions (no state) |
| `fe-ui/src/plugin.rs` | `GardenerConsolePlugin` + all UI-only resources (InspectorState, SidebarState, etc.) |
| `fe-ui/src/panels.rs` | `gardener_console()` egui panel function + `Tool` enum |
| `fe-renderer/src/camera.rs` | `OrbitCameraController` + `CameraControllerPlugin` + pure math functions |
| `fe-database/src/lib.rs` | All async DB functions; called from spawned thread |
| `fe-sync/src/lib.rs` | `SyncCommand` + `SyncCommandSenderRes` + iroh-docs replica management |
| `fe-runtime/src/messages.rs` | `DbCommand`, `DbResult` shared message enums |

---

## What NOT to Do (Anti-Patterns Found and Fixed)

1. **Do not write directly to NavigationManager fields from UI panels.**
   Previously panels wrote `nav.active_verse_id = Some(...)` directly.
   Now call `nav.navigate_to_verse(id, name)` which enforces clearing downstream selections.

2. **Do not drop `asset_path` when converting DB results to in-memory NodeEntry.**
   The old code set `asset_path: None` in NodeEntry. This broke petal-switch respawn because
   the manager had no path to load the asset from. Always propagate `n.asset_path.clone()`.

3. **Do not round-trip through `DbCommand::LoadHierarchy` when switching petals.**
   The old `despawn_and_spawn_on_petal_change` sent a LoadHierarchy command every time the
   petal changed — a DB query per navigation. Use the in-memory tree in VerseManager instead.

4. **Do not scatter selection state across multiple Resources.**
   `InspectorFormState` holds form buffers only (position/rotation/scale), not selection. It is written by
   `sync_manager_to_inspector`, never by UI code. The authoritative source for selection is `NodeManager`.

5. **Do not use `despawn_recursive()` on spawned node entities.**
   Nodes are flat `SceneRoot` entities; use `.despawn()`. Reserve `despawn_recursive()` for
   actual parent-child hierarchies.

6. **Do not silently swallow channel send errors.**
   Bare `.ok()` on a send hides DB thread crashes. Always `warn!()` on send failure.

7. **Do not allocate (String, Vec, clone) inside hot-path systems.**
   Systems like `handle_gimbal_interaction` run every frame for every selected entity.
   Keep per-frame work minimal.

---

## Entry Points for Common Tasks

### Add a new DbResult variant

1. Add the variant to `fe-runtime/src/messages.rs` (`DbResult` enum).
2. Handle it in `fe-ui/src/verse_manager.rs` in the `apply_db_results` system match block.
3. Add the corresponding `DbCommand` variant if a command triggers it.

### Add a new UI panel

1. Add any new Resource to `plugin.rs` and register it with `app.init_resource::<T>()`.
2. Add the panel rendering function to `panels.rs` (or a new file for large panels).
3. Pass required resources into `gardener_console()` (watch the 16-param Bevy limit — use
   `SystemParam` bundle structs like `P2pDialogParams` if needed).

### Add a new property to NodeEntry

1. Add the field to `NodeEntry` in `verse_manager.rs`.
2. Update `apply_db_results` `HierarchyLoaded` arm to populate it.
3. Update `apply_db_results` `GltfImported` arm if the field is set on import.
4. Update `update_node_position` / add a new `update_node_*` method as needed.
5. Update `spawn_node_entity` if the field affects spawning.

### Change petal navigation behavior

- Edit `NavigationManager::navigate_to_petal` in `navigation_manager.rs`.
- The respawn is driven by `respawn_on_petal_change` in `verse_manager.rs` — it detects
  the change via `Local<Option<String>>` comparison.

### Add a new editor tool (beyond Move/Rotate/Scale/Select)

1. Add the variant to `Tool` enum in `panels.rs`.
2. Add keyboard shortcut in `handle_tool_shortcuts` in `node_manager.rs`.
3. Add rendering in `draw_gimbal_for_entity` in `gimbal.rs`.
4. Add drag handling in `handle_gimbal_interaction` in `node_manager.rs`.
