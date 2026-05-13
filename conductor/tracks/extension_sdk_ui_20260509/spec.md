# Track: Extension SDK & UI Slots — Stable API Surface + Declared Extension Points

**Created:** 2026-05-09
**Status:** Draft
**Priority:** P1
**Depends on:** Plugin Host (Phase 9A), fe-ui (existing panels infrastructure)
**Blocks:** Community Plugins, Plugin Testing, Marketplace

---

## Problem Statement

Without a stable SDK and declared UI extension points:
1. Plugin authors program against internal APIs that break on every release (Blender's failure mode)
2. UI extension is impossible or hacked via undocumented injection (Revit's ribbon-only trap)
3. No versioned contract between engine and plugins (WIT interface needed)
4. No inter-plugin dependency resolution
5. First-party extensions (fe-terrain, fe-hexon) don't demonstrate the extension pattern

This track establishes the **permanent API surface** and **typed UI slots** before any external plugin ships.

---

## Goals

1. **`fe-sdk` crate** — stable API surface for extension authors (published to crates.io)
2. **WIT interface definition** (`hexon-plugin.wit`) — typed contract for WASM plugins
3. **6 declared UI extension slots** in fe-ui with typed registration
4. **Stable vs Proposed API tiers** — labeled on all public SDK types
5. **`UiExtensionRegistry`** Bevy Resource — plugins register UI contributions
6. **First-party migration** — fe-terrain implements FractalExtension + registers UI slots
7. **Plugin manifest `ui_slots` section** — declarative UI contribution
8. **Inter-plugin dependency** — `plugin_dependencies` in manifest with semver ranges
9. **ApiExtensionHandle** — plugins can register custom REST endpoints

## Non-Goals (this track)

- WASM debugging infrastructure (Phase 9C)
- Multi-language PDK (Extism layer — Phase 10)
- Plugin-to-plugin event bus (Phase 10)
- Dynamic theme/styling from plugins

---

## Architecture Overview

### fe-sdk Crate (Stable API Surface)

```
fe-sdk/
├── Cargo.toml
└── src/
    ├── lib.rs              # Re-exports, version constants, stability markers
    ├── node.rs             # NodeSnapshot, NodeHandle (read-only view)
    ├── property.rs         # PropertyValue, PropertyBag
    ├── scene.rs            # SceneChangeBatch, SceneChange (mirrors fe-entity-store)
    ├── query.rs            # PluginQueryBuilder (subset of fe-query)
    ├── transaction.rs      # PluginTransaction (write API)
    ├── context.rs          # PluginContext (capabilities, petal scope, config)
    ├── ui/
    │   ├── mod.rs          # UiSlot enum, UiContribution, UiExtensionRegistry
    │   ├── panel.rs        # InspectorPanel trait, SidebarTool trait
    │   ├── overlay.rs      # ViewportOverlay trait
    │   └── menu.rs         # ContextMenuItem, ToolbarAction
    ├── api.rs              # ApiExtensionHandle (register custom endpoints)
    ├── events.rs           # PluginEvent enum (subscribe/emit)
    └── stability.rs        # #[stable], #[proposed] attribute macros
```

### Stability Tiers (VSCode Pattern)

```rust
/// Stable API — guaranteed backward-compatible within major version
#[stable(since = "1.0")]
pub struct NodeSnapshot { ... }

/// Proposed API — may change or be removed. Not available in marketplace plugins.
#[proposed]
pub struct RawEcsAccess { ... }
```

- Stable types: NodeSnapshot, PropertyValue, SceneChange, PluginTransaction, UiSlot
- Proposed types: RawEcsAccess, DirectDbQuery, CustomRenderer
- Marketplace plugins can only import `#[stable]` types
- `sdk_api_version` in manifest gates which stable version the plugin was built against

### UI Extension Slots

| Slot ID | Location | Plugin Provides | Rendering |
|---------|----------|----------------|-----------|
| `fe:ui:inspector-section` | Node inspector bottom | `InspectorPanel` trait impl | egui panel below built-in properties |
| `fe:ui:sidebar-tool` | Left sidebar tool list | `SidebarTool` trait impl | Icon + label + tool activation |
| `fe:ui:viewport-overlay` | 3D viewport | `ViewportOverlay` trait impl | egui overlay drawn on viewport |
| `fe:ui:toolbar-action` | Top toolbar | `ToolbarAction` struct | Button with icon + tooltip |
| `fe:ui:context-menu` | Right-click menu | `ContextMenuItem` struct | Menu entry with handler |
| `fe:ui:dashboard-widget` | Dashboard panel | `DashboardWidget` trait impl | egui widget in dashboard grid |

### UiExtensionRegistry (Bevy Resource)

```rust
#[derive(Resource, Default)]
pub struct UiExtensionRegistry {
    inspector_panels: Vec<(HexonUri, Box<dyn InspectorPanel>)>,
    sidebar_tools: Vec<(HexonUri, Box<dyn SidebarTool>)>,
    viewport_overlays: Vec<(HexonUri, Box<dyn ViewportOverlay>)>,
    toolbar_actions: Vec<(HexonUri, ToolbarAction)>,
    context_menu_items: Vec<(HexonUri, ContextMenuItem)>,
    dashboard_widgets: Vec<(HexonUri, Box<dyn DashboardWidget>)>,
}
```

### InspectorPanel Trait (Example)

```rust
pub trait InspectorPanel: Send + Sync {
    /// Display name shown as section header
    fn label(&self) -> &str;

    /// Whether this panel applies to the given node
    fn matches(&self, node: &NodeSnapshot) -> bool;

    /// Render the panel content. Returns any UiActions to execute.
    fn render(&mut self, ui: &mut egui::Ui, node: &NodeSnapshot, ctx: &PluginContext) -> Vec<UiAction>;
}
```

### WIT Interface (hexon-plugin.wit)

```wit
package fractalengine:plugin@1.0.0;

interface node-api {
  record transform { x: f32, y: f32, z: f32 }
  record property { key: string, value: string }
  record node-snapshot {
    node-id: string,
    petal-id: string,
    name: string,
    position: transform,
    properties: list<property>,
  }

  get-node: func(node-id: string) -> option<node-snapshot>;
  set-property: func(node-id: string, key: string, value: string) -> result<_, string>;
  query-nodes: func(petal-id: string, filter: string) -> list<node-snapshot>;
  create-node: func(petal-id: string, name: string) -> result<string, string>;
  delete-node: func(node-id: string) -> result<_, string>;
}

interface scene-hooks {
  record scene-change-batch {
    added: list<node-snapshot>,
    removed: list<string>,
    property-changes: list<tuple<string, string, string>>,
  }

  on-install: func(hexon-uri: string) -> result<_, string>;
  on-activate: func(petal-id: string);
  on-deactivate: func(petal-id: string);
  on-scene-change: func(batch: scene-change-batch) -> scene-change-batch;
  on-tick: func(delta-ms: u32);
}

interface plugin-log {
  log-debug: func(msg: string);
  log-info: func(msg: string);
  log-warn: func(msg: string);
  log-error: func(msg: string);
}

world fractal-plugin {
  import node-api;
  import plugin-log;
  export scene-hooks;
}
```

### First-Party Migration: fe-terrain as Extension

fe-terrain already exists as a native Bevy plugin. This track adds:
- Implement `FractalExtension` trait on `TerrainPlugin`
- Register `fe:ui:inspector-section` for terrain config panel (petal inspector)
- Register `fe:ui:sidebar-tool` for GPX import tool
- Register `fe:ui:viewport-overlay` for terrain measurement overlay
- Register `fe:ui:toolbar-action` for "Toggle Terrain" button
- This validates the extension pattern with real code before external plugins ship

### ApiExtensionHandle

Plugins can register custom REST endpoints scoped under their namespace:

```rust
pub struct ApiExtensionHandle {
    pub base_path: String, // e.g., "/api/v1/ext/terrain"
    pub routes: Vec<ExtensionRoute>,
}

pub struct ExtensionRoute {
    pub method: HttpMethod,
    pub path: String,     // relative to base_path
    pub handler: ExtensionHandler,
    pub min_role: RoleLevel,
}
```

---

## Phases

### Phase 1: fe-sdk Crate + Stability Markers

- `fe-sdk` crate scaffolding with all type definitions
- NodeSnapshot, PropertyValue, SceneChangeBatch (mirrors fe-entity-store, stable)
- PluginContext, PluginTransaction (stable)
- PluginQueryBuilder (subset of fe-query, stable)
- `#[stable]` / `#[proposed]` attribute macros
- `sdk_api_version` constant and manifest validation

### Phase 2: UI Extension Slots + Registry

- UiExtensionRegistry Bevy Resource
- 6 UI slot traits: InspectorPanel, SidebarTool, ViewportOverlay, ToolbarAction, ContextMenuItem, DashboardWidget
- fe-ui integration: existing panels render extension contributions at correct locations
- Plugin manifest `ui_slots` section parsing
- Uninstall cleanup: remove UI registrations when plugin deactivated

### Phase 3: WIT Interface + First-Party Migration

- `hexon-plugin.wit` definition (node-api + scene-hooks + plugin-log)
- wit-bindgen integration for Rust guest SDK
- fe-terrain migrated to FractalExtension pattern
- fe-terrain registers 4 UI slots (inspector, sidebar, overlay, toolbar)
- ApiExtensionHandle for plugin-scoped REST endpoints
- Inter-plugin dependencies in manifest (plugin_dependencies with semver)
- Documentation: "Writing Your First Extension" guide

---

## Success Criteria

- [ ] `fe-sdk` compiles independently with only serde + egui as dependencies
- [ ] fe-terrain implements FractalExtension and registers 4 UI extension slots
- [ ] Inspector panel renders terrain config section for nodes with terrain properties
- [ ] Sidebar shows "GPX Import" tool contributed by fe-terrain extension
- [ ] A WASM plugin compiled against hexon-plugin.wit can call get-node and set-property
- [ ] Plugin with `sdk_api_version: "1.0"` rejected on engine with incompatible version
- [ ] Extension REST endpoints registered at /api/v1/ext/{plugin_namespace}/
- [ ] Removing an extension cleans up all UI registrations (no ghost panels)
