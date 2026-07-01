---
type: track-spec
---

# Track: Tauri IPC/Asset Bridge — Shared Node Structure + egui-Led Event Bridge

**Created:** 2026-06-30
**Status:** Draft
**Priority:** P1
**Depends on:** Tauri WebView Backend (track 1)
**Blocks:** Tauri Backend Cutover (track 3)

---

## Problem Statement

Track 1 added a Tauri-powered webview backend to `fe-webview`. This track adds the **interoperability layer**:

1. **Shared "node" data structure** — a unified representation of scene nodes that flows between Bevy (Rust) and the Tauri webview (JS)
2. **IPC command bridge** — typed commands via `#[tauri::command]` + `invoke()`
3. **Custom asset protocol** — serve local files via `asset://` protocol
4. **Event bridge** — bidirectional events between egui-originated and webview-originated interactions

**Core principle: egui LEADS, Tauri integrates via commands.** The shared node structure is the "seam" that bridges Tauri↔Bevy actions/interactions.

---

## Goals

1. Design and implement the **shared "node" data structure** that bridges Tauri↔Bevy
2. Add `#[tauri::command]` handlers for node queries, transforms, and interactions
3. Implement custom `asset://` protocol for local asset serving
4. Establish the event bridge pattern: egui events → Tauri commands → webview; webview events → Tauri events → Bevy
5. PetalPortal parity on the new backend

---

## Non-Goals (this track)

- Making Tauri the default entry point (track 3)
- Replacing bevy_egui with web UI (egui remains leading)
- Full shell inversion (track 4 SPIKE)
- Pear P2P integration (track 5 SPIKE)

---

## The Shared Node Structure (Centerpiece)

This is the key innovation: a data structure that is **passed alongside events** to bridge Tauri↔Bevy interactions.

### Concept

```
┌─────────────────────────────────────────────────────────────┐
│                      Shared Node                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │ node_id: String          // Unique node identifier   │  │
│  │ verse_id: String         // Parent verse             │  │
│  │ fractal_id: String       // Parent fractal           │  │
│  │ petal_id: String         // Parent petal             │  │
│  │ position: [f32; 3]       // Transform position       │  │
│  │ rotation: [f32; 4]       // Transform rotation       │  │
│  │ scale: [f32; 3]          // Transform scale          │  │
│  │ webpage_url: Option<String>  // PetalPortal URL      │  │
│  │ asset_path: Option<String>   // GLTF/GLB path        │  │
│  │ properties: Map<String, PropertyValue>  // Custom    │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### Why This Matters

1. **Bevy → WebView**: When a node is selected in egui, the shared node is passed to the webview so it knows what node to display
2. **WebView → Bevy**: When the user interacts with content in the webview, the shared node is passed back to trigger Bevy actions
3. **Pear Runtime Ready** (track 5): The shared node structure is the seam where Pear's P2P events will plug in

---

## Architecture

### IPC Flow (egui-LEADS)

```
┌─────────────────────────────────────────────────────────────┐
│                        Bevy (HOST)                          │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  NodeManager / VerseManager                         │   │
│  │    - Owns authoritative node data                   │   │
│  │    - Selection state                                │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  #[tauri::command] handlers                         │   │
│  │    - get_node_data(node_id) -> SharedNode           │   │
│  │    - notify_interaction(Interaction) -> ()          │   │
│  │    - resolve_asset(petal_id, path) -> Vec<u8>       │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            │ invoke()
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                     Tauri WebView                           │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  JavaScript: window.__TAURI__.invoke('get_node_data')│  │
│  │             → receives SharedNode                    │   │
│  │             → renders PetalPortal                    │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Event Bridge

Bidirectional:
- **egui → WebView**: Selection triggers `navigate` command with node data
- **WebView → egui**: WebView interaction emits event caught by command handler, forwarded to Bevy systems

---

## Functional Requirements

### FR-1: Shared Node Serialization

Define the shared node in a location accessible to both Rust and (via codegen) JS:

```rust
// fe-runtime/src/shared_node.rs (new module)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SharedNode {
    pub node_id: String,
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub webpage_url: Option<String>,
    pub asset_path: Option<String>,
    pub properties: HashMap<String, PropertyValue>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<PropertyValue>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum WebViewInteraction {
    NodeSelected { node: SharedNode },
    NodeDeselected { node_id: String },
    TransformChanged { node_id: String, position: [f32; 3], rotation: [f32; 4], scale: [f32; 3] },
    PropertyChanged { node_id: String, key: String, value: PropertyValue },
    UrlChanged { node_id: String, url: String },
}
```

### FR-2: Tauri Command Handlers

```rust
// In fe-webview or fractalengine

#[tauri::command]
fn get_node_data(node_id: String) -> Result<SharedNode, String> {
    // Query from VerseManager
    let node = verse_manager.find_node(&node_id)
        .ok_or_else(|| format!("Node {} not found", node_id))?;
    Ok(node_to_shared(node))
}

#[tauri::command]
fn notify_interaction(interaction: WebViewInteraction) -> Result<(), String> {
    // Convert WebViewInteraction to Bevy event
    // Forward to appropriate Bevy system
    Ok(())
}

#[tauri::command]
fn list_nodes_for_petal(petal_id: String) -> Result<Vec<SharedNode>, String> {
    // Return all nodes in a petal for webview UI
}
```

### FR-3: Custom Asset Protocol

```rust
#[tauri::command]
fn resolve_asset(petal_id: String, asset_path: String) -> Result<Vec<u8>, String> {
    let base = get_petal_assets_dir(&petal_id);
    let resolved = base.join(&asset_path);

    // Security: path traversal protection
    if !resolved.starts_with(&base) {
        return Err("Path traversal blocked".to_string());
    }

    std::fs::read(&resolved).map_err(|e| e.to_string())
}
```

Register in Tauri config:

```json
{
  "app": {
    "security": {
      "assetProtocol": {
        "enable": true,
        "scope": ["asset://localhost:0/**"]
      }
    }
  }
}
```

### FR-4: JavaScript Integration

```typescript
// frontend/src/tauri-api.ts

export interface SharedNode {
  node_id: string;
  verse_id: string;
  fractal_id: string;
  petal_id: string;
  position: [number, number, number];
  rotation: [number, number, number, number];
  scale: [number, number, number];
  webpage_url?: string;
  asset_path?: string;
  properties: Record<string, any>;
}

export const getNodeData = async (nodeId: string): Promise<SharedNode> => {
  return await window.__TAURI__.invoke('get_node_data', { nodeId });
};

export const notifyInteraction = async (interaction: any): Promise<void> => {
  await window.__TAURI__.invoke('notify_interaction', { interaction });
};

export const resolveAsset = async (petalId: string, path: string): Promise<Uint8Array> => {
  return await window.__TAURI__.invoke('resolve_asset', { petalId, assetPath: path });
};
```

---

## The Pear Connection (Track 5 Preview)

The shared node structure is designed to be the **seam for Pear Runtime**:

- Pear runs in the JS context (inside Tauri webview)
- Pear's hypercore/hyperswarm events can be converted to `WebViewInteraction`
- The IPC bridge passes them to Bevy via the same `notify_interaction` command
- This is why the shared node structure is designed now — to be Pear-ready

---

## Testing Strategy

- **Unit tests**: Shared node serialization/deserialization
- **IPC tests**: Command handler inputs/outputs
- **Protocol tests**: Asset protocol path traversal protection
- **Integration tests**: Full node selection → IPC → webview flow

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Serialization compatibility Rust/JS | Medium | High | Use standard types; test codegen |
| Path traversal in asset protocol | High | High | Strict `starts_with` validation |
| Event loop conflicts (egui vs webview) | Medium | Medium | Use pointer-events CSS |
| Shared node version drift | Low | Medium | Version the structure |

---

## Design Decisions

### DD-1: egui LEADS

**Chosen**: All interactions flow through Bevy/egui as the source of truth. Tauri webview is a display/render target, not an independent controller. Commands flow from egui → Tauri → webview.

### DD-2: Shared Structure Location

**Chosen**: `fe-runtime` crate — shared between all components (fe-ui, fe-webview, eventual frontend).

### DD-3: IPC Pattern

**Chosen**: Commands (invoke) for queries, events (emit) for notifications. This matches Tauri's design.

### DD-4: Asset Protocol Security

**Chosen**: Strict path validation with `starts_with` check. Path traversal = error.

---

## Documentation

This track produces:
1. Shared node structure docs (in fe-runtime)
2. IPC API docs (in fe-webview)
3. Asset protocol spec (in fe-webview)
