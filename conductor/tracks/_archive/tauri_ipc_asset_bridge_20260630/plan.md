---
type: track-plan
---

# Implementation Plan: Tauri IPC/Asset Bridge

## Overview

Three-phase implementation:
1. Design and implement shared node data structure
2. Add Tauri command handlers
3. Implement event bridge + asset protocol

**TDD is mandatory** where applicable.

**Core principle**: egui LEADS, Tauri integrates via commands. The shared node structure is the centerpiece — the seam that bridges Tauri↔Bevy and will later plug in Pear (track 5).

---

## Phase 1: Shared Node Data Structure

**Goal:** Unified node representation accessible to both Rust and JS.

---

### Task 1.1 — Create shared_node module [ ]

Create `fe-runtime/src/shared_node.rs`:

```rust
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
    TransformChanged {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    },
    PropertyChanged {
        node_id: String,
        key: String,
        value: PropertyValue,
    },
    UrlChanged {
        node_id: String,
        url: String,
    },
}
```

**Verification:** Types compile and serialize correctly.

**Files:** New `fe-runtime/src/shared_node.rs`

---

### Task 1.2 — Add conversion utilities [ ]

Add helpers to convert between internal types and shared node:

```rust
impl SharedNode {
    pub fn from_node_entry(entry: &NodeEntry, verse_id: &str, fractal_id: &str, petal_id: &str) -> Self {
        Self {
            node_id: entry.id.clone(),
            verse_id: verse_id.to_string(),
            fractal_id: fractal_id.to_string(),
            petal_id: petal_id.to_string(),
            position: entry.position,
            rotation: entry.rotation,
            scale: entry.scale,
            webpage_url: entry.webpage_url.clone(),
            asset_path: entry.asset_path.clone(),
            properties: entry.properties.clone(),
        }
    }
}
```

**Verification:** Conversion compiles and preserves data.

**Files:** `fe-runtime/src/shared_node.rs`

---

### Task 1.3 — Export from fe-runtime [ ]

Update `fe-runtime/src/lib.rs`:

```rust
pub mod shared_node;
pub use shared_node::{SharedNode, PropertyValue, WebViewInteraction};
```

**Verification:** Module is re-exported.

**Files:** `fe-runtime/src/lib.rs`

---

### Task 1.4 — Generate TypeScript types [ ]

Generate TypeScript from Rust types for the frontend:

```rust
// Or use a tool like ` TypeScriptify` or manual copy
// frontend/src/types/shared-node.ts

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

export type WebViewInteraction =
  | { NodeSelected: { node: SharedNode } }
  | { NodeDeselected: { node_id: string } }
  | { TransformChanged: { node_id: string; position: [number, number, number]; rotation: [number, number, number, number]; scale: [number, number, number] } }
  | { PropertyChanged: { node_id: string; key: string; value: any } }
  | { UrlChanged: { node_id: string; url: string } };
```

**Verification:** TypeScript compiles.

**Files:** New `frontend/src/types/shared-node.ts`

---

### Phase 1 Checkpoint

- SharedNode struct defined and serializable
- Conversion utilities work
- TypeScript types generated and match Rust

---

## Phase 2: IPC Command Handlers

**Goal:** Add `#[tauri::command]` handlers that use the shared node structure.

---

### Task 2.1 — Add Tauri dependencies [ ]

Ensure Tauri deps are available where commands are defined:

```toml
# fe-webview/Cargo.toml
[dependencies]
tauri = { version = "2", optional = true }
```

**Verification:** Dependencies resolve.

**Files:** `fe-webview/Cargo.toml`

---

### Task 2.2 — Implement get_node_data command [ ]

In `fe-webview/src/ipc.rs` or new module:

```rust
use fe_runtime::shared_node::{SharedNode, WebViewInteraction};

#[tauri::command]
pub fn get_node_data(
    node_id: String,
    verse_manager: Res<VerseManager>,
) -> Result<SharedNode, String> {
    verse_manager
        .find_node(&node_id)
        .map(|node| SharedNode::from_node_entry(
            &node,
            &node.verse_id,
            &node.fractal_id,
            &node.petal_id,
        ))
        .ok_or_else(|| format!("Node {} not found", node_id))
}
```

**Verification:** Command compiles and returns data.

**Files:** New IPC module in fe-webview

---

### Task 2.3 — Implement notify_interaction command [ ]

```rust
#[tauri::command]
pub fn notify_interaction(
    interaction: WebViewInteraction,
    mut events: EventWriter<WebViewInteractionEvent>,
) -> Result<(), String> {
    events.send(WebViewInteractionEvent(interaction));
    Ok(())
}

// Define the Bevy event
#[derive(Event)]
pub struct WebViewInteractionEvent(pub WebViewInteraction);
```

**Verification:** Command compiles, event is sent.

**Files:** IPC module

---

### Task 2.4 — Register commands with Tauri [ ]

In the Tauri backend setup:

```rust
tauri::Builder::default()
    .invoke_handler(tauri::generate_handler![
        get_node_data,
        notify_interaction,
        resolve_asset,
    ])
    .build(...)
```

**Verification:** All commands register.

**Files:** Backend setup code

---

### Phase 2 Checkpoint

- get_node_data returns SharedNode
- notify_interaction forwards to Bevy events
- Commands are registered with Tauri

---

## Phase 3: Event Bridge + Asset Protocol

**Goal:** Complete the IPC layer with asset protocol and event routing.

---

### Task 3.1 — Implement asset protocol handler [ ]

```rust
#[tauri::command]
pub fn resolve_asset(
    petal_id: String,
    asset_path: String,
    app: AppHandle,
) -> Result<Vec<u8>, String> {
    let data_dir = app.path().app_data_dir()
        .map_err(|e| e.to_string())?;
    let base = data_dir
        .join("verses")
        .join(&petal_id)
        .join("assets");

    let resolved = base.join(&asset_path);

    // Security: path traversal protection
    if !resolved.starts_with(&base) {
        return Err("Path traversal blocked".to_string());
    }

    std::fs::read(&resolved).map_err(|e| e.to_string())
}
```

**Verification:** Handler compiles, path traversal is blocked.

**Files:** IPC module

---

### Task 3.2 — Add webview interaction event handler [ ]

Create a Bevy system that handles webview interactions:

```rust
fn handle_webview_interaction(
    mut events: EventReader<WebViewInteractionEvent>,
    mut node_manager: ResMut<NodeManager>,
    mut verse_manager: ResMut<VerseManager>,
) {
    for event in events.read() {
        match &event.0 {
            WebViewInteraction::NodeSelected { node } => {
                // Select the node in NodeManager
            }
            WebViewInteraction::TransformChanged { node_id, position, rotation, scale } => {
                // Update node transform in VerseManager
                verse_manager.update_node_position(node_id, *position);
                // ...
            }
            WebViewInteraction::UrlChanged { node_id, url } => {
                verse_manager.update_node_url(node_id, url);
            }
            _ => {}
        }
    }
}
```

**Verification:** System compiles and handles events.

**Files:** New system in fe-webview or fe-ui

---

### Task 3.3 — Add frontend invoke wrapper [ ]

```typescript
// frontend/src/tauri-api.ts

export const getNodeData = async (nodeId: string): Promise<SharedNode> => {
  return await window.__TAURI__.invoke('get_node_data', { nodeId });
};

export const notifyInteraction = async (interaction: WebViewInteraction): Promise<void> => {
  await window.__TAURI__.invoke('notify_interaction', { interaction });
};

export const resolveAsset = async (petalId: string, path: string): Promise<Uint8Array> => {
  return await window.__TAURI__.invoke('resolve_asset', {
    petalId,
    assetPath: path,
  });
};
```

**Verification:** TypeScript compiles.

**Files:** `frontend/src/tauri-api.ts`

---

### Task 3.4 — Full integration test [ ]

Test the complete flow:
1. Select node in egui → get_node_data called
2. SharedNode passed to webview
3. User interacts in webview → notify_interaction called
4. Event routes to Bevy system → node state updates

**Verification:** Full flow works.

**Files:** Integration test

---

### Phase 3 Checkpoint

- Asset protocol serves files securely
- Event bridge routes webview → Bevy
- Frontend uses invoke API
- Full integration test passes

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|--------------|
| 1 | Shared node structure | Types compile, serialization works |
| 2 | IPC commands | Commands register, return correct data |
| 3 | Event bridge + asset protocol | Full flow works |

## Quality Gates

- [ ] SharedNode serializes to JSON correctly
- [ ] get_node_data returns correct node
- [ ] notify_interaction routes to Bevy
- [ ] Asset protocol blocks path traversal
- [ ] Full integration test passes

## Notes for Track 5 (Pear)

The shared node structure + IPC bridge is designed to accept Pear P2P events:
- Pear runs in JS context (Tauri webview)
- Pear peer events → WebViewInteraction → notify_interaction → Bevy
- The seam is already designed — Pear just needs to emit the right interaction type
