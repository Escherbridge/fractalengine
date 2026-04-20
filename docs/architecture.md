# Architecture

Technical architecture reference for FractalEngine.

---

## Crate Dependency Diagram

```
fractalengine (binary)
        |
        +---> fe-ui ──────────────► fe-renderer
        |          └─────────────► fe-runtime ◄──── fe-database
        |          └─────────────► fe-database       (shared types)
        |          └─────────────► fe-sync ────────► fe-network
        |                               └──────────► fe-runtime
        |                               └──────────► fe-database
        +---> fe-renderer
        +---> fe-database
        +---> fe-sync
        +---> fe-runtime

External dependencies (workspace-level):
  bevy, bevy_egui, iroh, crossbeam, tokio, anyhow, serde/serde_json, rfd
```

Build direction: `fe-runtime` and `fe-database` are leaves (no FE dependencies).
`fe-sync` depends on both. `fe-ui` depends on everything except `fe-network` directly.
`fe-network` is an iroh networking helper used only by `fe-sync`.

---

## Manager Responsibility Table

| Manager | Resource | File | What it Owns |
|---------|----------|------|--------------|
| NavigationManager | `NavigationManager` | `fe-ui/src/navigation_manager.rs` | Active verse/fractal/petal cursor; P2P replica open/close lifecycle |
| VerseManager | `VerseManager` | `fe-ui/src/verse_manager.rs` | Full in-memory Verse→Fractal→Petal→Node tree; entity spawn/despawn on petal change |
| NodeManager | `NodeManager` | `fe-ui/src/node_manager.rs` | Node selection; gimbal drag state; transform broadcast to DB and P2P |

---

## Resource Ownership Table

| Resource | Owner | Purpose |
|----------|-------|---------|
| `NavigationManager` | NavigationManagerPlugin | Active hierarchy cursor |
| `VerseManager` | VerseManagerPlugin | In-memory content tree |
| `NodeManager` | NodeManagerPlugin | Selection + drag state (sole source of selection truth) |
| `UiManager` | GardenerConsolePlugin | Centralized UI state: active dialogs, portal panel, UI action queue |
| `SidebarState` | GardenerConsolePlugin | Sidebar open/close + selected_node_id (UI mirror) |
| `ToolState` | GardenerConsolePlugin | Active editor tool (Select/Move/Rotate/Scale) |
| `InspectorFormState` | GardenerConsolePlugin | Transform buffers for inspector panel (form buffers only, no selection) |
| `ActiveDialog` | UiManager | Active dialog variant (on UiManager enum); formerly separate dialog state resources |
| `PortalState` | UiManager | Portal panel state (formerly separate PortalPanelState resource) |
| `CameraFocusTarget` | GardenerConsolePlugin | One-shot focus target consumed by orbit camera |
| `ViewportCursorWorld` | GardenerConsolePlugin | Cursor projected onto Y=0 plane (for model placement) |
| `ViewportRect` | GardenerConsolePlugin | Screen-space rect of the 3D viewport (for click rejection) |
| `PeerDebugPanelState` | GardenerConsolePlugin | Peer debug panel visibility |
| `UiAction` queue | UiManager | Action queue for deferred UI operations (replaces WebViewOpenRequest) |
| `fe_runtime::app::DbCommandSender` | fe-runtime | Channel to DB thread |
| `fe_sync::SyncCommandSenderRes` | fe-sync | Channel to P2P sync thread (optional — absent if no sync) |

---

## System Execution Order

### NodeManagerPlugin — chained systems (run in order, same Update frame)

```
Update schedule:
  1. handle_tool_shortcuts        — keyboard: S/G/R/X=tool switch, Escape=deselect
  2. sync_sidebar_to_manager      — SidebarState.selected_node_id → NodeManager.select/deselect
  3. handle_gimbal_interaction    — axis hit-test on press; apply drag on hold; commit on release
  4. handle_viewport_click        — ray-cast pick on click (skipped if drag started this frame)
  5. sync_manager_to_inspector    — NodeManager → InspectorState transform buffers
  6. draw_gimbal_system           — render gimbal gizmos for selected entity
  7. broadcast_transform          — on drag_committed: → DB channel, → sync channel, → VerseManager
```

All 7 are chained with `.chain()` so their order is deterministic.

### VerseManagerPlugin — systems (not chained, run in parallel)

```
Update schedule:
  - apply_db_results           — reads MessageReader<DbResult>; updates VerseManager + NavigationManager
  - respawn_on_petal_change    — detects active_petal_id change; despawn old, spawn new entities
```

### NavigationManagerPlugin — systems

```
Update schedule:
  - handle_verse_replica_lifecycle  — detects active_verse_id change; sends SyncCommand to open/close replica
```

### GardenerConsolePlugin — systems

```
EguiPrimaryContextPass:
  - gardener_ui_system         — renders all egui panels via panels::gardener_console()

Update schedule:
  - apply_camera_focus         — CameraFocusTarget → OrbitCameraController.focus
  - strip_gltf_embedded_cameras — despawns camera entities from loaded GLB files
  - update_viewport_cursor_world — projects cursor onto Y=0 plane
```

### CameraControllerPlugin (fe-renderer)

```
Startup:
  - spawn_orbit_camera         — spawns Camera3d + OrbitCameraController

Update:
  - orbit_camera_system        — mouse orbit/pan/zoom + WASD movement
```

---

## Plugin Registration Order

Plugins are added in this order in the binary entry point (`fractalengine/src/main.rs`):

```
1. DefaultPlugins (Bevy builtins)
2. EguiPlugin
3. CameraControllerPlugin          (fe-renderer)
4. GardenerConsolePlugin           (fe-ui)
   └── NavigationManagerPlugin
   └── VerseManagerPlugin
   └── NodeManagerPlugin
5. (fe-sync plugin, if feature enabled)
```

The domain manager plugins are added as sub-plugins inside `GardenerConsolePlugin::build()`,
so they are always registered after the base Bevy plugins and before any sync plugins.

---

## Bevy ECS Data Flow Overview

```
User Input (keyboard / mouse)
        │
        ▼
[handle_tool_shortcuts]      ─► ToolState (active_tool)
[orbit_camera_system]        ─► OrbitCameraController (focus/yaw/pitch/distance)
        │
        ▼
[sync_sidebar_to_manager]    ─► NodeManager.select / deselect
        │
        ▼
[handle_gimbal_interaction]  ─► NodeManager.selected.drag  (AxisDrag)
                             ─► Transform component (entity moved/rotated/scaled)
        │
        ▼
[handle_viewport_click]      ─► NodeManager.select / deselect
                             ─► SidebarState.selected_node_id (mirror)
        │
        ▼
[sync_manager_to_inspector]  ─► InspectorState.pos/rot/scale (display buffers)
        │
        ▼
[draw_gimbal_system]         ─► Gizmos (pure visual output, no state)
        │
        ▼
[broadcast_transform]        ─► DbCommandSender (DbCommand::UpdateNodeTransform)
                             ─► SyncCommandSenderRes (SyncCommand::UpdateNodeTransform)
                             ─► VerseManager.update_node_position (in-memory sync)

DB Thread / Sync Thread
        │
        ▼  (MessageReader<DbResult>)
[apply_db_results]           ─► VerseManager.verses (tree rebuild / append)
                             ─► NavigationManager.active_verse_id etc. (auto-nav)
                             ─► Commands.spawn (entity spawn for active petal)
        │
        ▼  (Local<Option<String>> comparison)
[respawn_on_petal_change]    ─► Commands.despawn (old petal entities)
                             ─► Commands.spawn (new petal entities from in-memory tree)

NavigationManager changes
        │
        ▼  (Local<Option<String>> comparison)
[handle_verse_replica_lifecycle]
                             ─► SyncCommand::CloseVerseReplica (old verse)
                             ─► SyncCommand::OpenVerseReplica (new verse)
```
