---
type: track-spec
---

# Track: Tauri WebView Backend — Robust Tauri Browser for fe-webview

**Created:** 2026-06-30
**Status:** Draft
**Priority:** P0
**Depends on:** none
**Blocks:** Tauri IPC/Asset Bridge (track 2), Tauri Backend Cutover (track 3)

---

## Problem Statement

The current `fe-webview` crate uses raw `wry` FFI for the PetalPortal browser overlay. This approach has limitations:

- **Fragility**: Raw wry requires manual window parenting and event handling
- **Limited IPC**: No typed command system, only evaluate_script
- **No plugin ecosystem**: Can't leverage Tauri plugins (shell, dialog, notification)
- **Custom protocol complexity**: Asset serving requires manual implementation

**The PRIMARY reason for Tauri is the in-app BROWSER/webview** — we need a more robust and interoperable browser backend. This is NOT about taking over the window/event loop — Bevy STAYS the host, and bevy_egui REMAINS the leading UI.

---

## Goals

1. Add a Tauri-powered webview/backend to `fe-webview` (behind `backend-tauri` feature)
2. Replace raw wry FFI overlay with Tauri webview integration
3. Enable Tauri's IPC command system (`#[tauri::command]` + `invoke()`)
4. Leverage Tauri's custom protocol, multi-window, and plugin ecosystem
5. Maintain bevy_egui as the leading UI — Tauri integrates, doesn't replace

---

## Non-Goals (this track)

- **Full shell inversion** (Tauri hosts window/event loop) — that's track 4 (SPIKE)
- Replacing bevy_egui with web UI — egui remains the main UI
- Custom asset protocol implementation — covered in track 2
- Making Tauri the default entry point — covered in track 3
- Removing the raw wry backend — can be deprecated later

---

## Why Tauri's WebView Beats Raw wry

| Aspect | Raw wry (current) | Tauri WebView |
|--------|-------------------|---------------|
| IPC | `evaluate_script()` only | `#[tauri::command]` + typed `invoke()` |
| Custom Protocol | Manual implementation | Built-in via `register_asynchronous_protocol` |
| Plugins | None | shell, dialog, notification, etc. |
| Multi-window | Manual | Built-in window management |
| Window parenting | FFI tricks | `with_parent()` API |
| Events | Manual polling | Full event subscription |
| Security | Custom implementation | CSP, asset protocol scopes |

---

## Architecture

### fe-webview Backend Selection

```
fe-webview/Cargo.toml:

[features]
default = []
backend-servo = ["winit"]
backend-wry = []
backend-tauri = ["tauri", "tao"]  # NEW
```

### Bevy-as-Host Architecture (This Track)

```
┌─────────────────────────────────────────────────────────────┐
│                     Bevy App (HOST)                         │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ bevy_egui — Main UI panels (SELECT, INSPECTOR, etc) │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  fe-webview / PetalPortalPlugin                      │   │
│  │    - WebViewBackend trait (impl for Tauri)          │   │
│  └─────────────────────────────────────────────────────┘   │
│                            │                                 │
│                            ▼                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │  Tauri WebView (embedded via wry+tauri)             │   │
│  │    - Child window of Bevy                           │   │
│  │    - IPC via invoke()                               │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### vs. The SPIKE (Track 4)

Track 4 (SPIKE) explores the **opposite approach**: Tauri hosts, Bevy renders into it. This track keeps Bevy as host — the key distinction.

---

## Design Decision: Approaches A vs. B

### Approach A — Tauri WebView as Bevy Child (THIS TRACK)

Bevy owns window + event loop (via `bevy_winit`):
- Tauri `WebviewWindow` embedded as child via `tao::WebviewWindowBuilder::with_parent()`
- Change stays inside `fe-webview`
- Bevy_egui remains the leading UI

**Pros:**
- Minimal blast radius (stays in fe-webview)
- Bevy_winit continues to provide input handling
- egui works without changes
- Incremental migration

**Cons:**
- Tauri isn't "designed" for embed-only mode
- Some Tauri features may not work perfectly as child
- Less control over the webview lifecycle

### Approach B — Tauri Hosts Everything (SPIKE, track 4)

Full inversion:
- Tauri owns window + event loop
- Bevy renders into Tauri surface via custom renderer
- bevy_winit REMOVED

**Pros:**
- Full Tauri plugin ecosystem
- Clean separation: Tauri = window/UI, Bevy = renderer

**Cons:**
- Large blast radius
- Must implement input forwarding (critical for picking)
- bevy_egui compatibility uncertain
- Version reconciliation (bevy 0.15→0.18)

---

## FractalEngine Reality

- **Bevy version**: 0.18 (with `bevy_winit`, `3d_bevy_render`, `bevy_picking`)
- **bevy_egui**: 0.39
- **Existing P2P**: Rust-native libp2p DHT + iroh transport (mycelium)
- **Existing browser**: `fe-webview` crate, inline wry overlay child of Bevy window
- **wry version**: 0.54

---

## Functional Requirements

### FR-1: Tauri Backend Implementation

Add `backend-tauri` feature to `fe-webview` that implements `WebViewBackend`:

```rust
// fe-webview/src/backends/tauri.rs

pub struct TauriBackend {
    webview_window: WebviewWindow,
    app_handle: AppHandle,
}

impl WebViewBackend for TauriBackend {
    fn navigate(&mut self, url: &Url) -> Result<(), WebViewError> {
        // Use Tauri window API
        self.webview_window.eval(&format!(
            "window.location.href = '{}';",
            url.as_str()
        )).map_err(|e| WebViewError::Navigation(e.to_string()))
    }

    fn url(&self) -> Result<Url, WebViewError> {
        // Query current URL via invoke
        Ok(Url::parse(&current_url))
    }

    fn close(&mut self) -> Result<(), WebViewError> {
        // Close the webview window
        Ok(())
    }
}
```

### FR-2: Window Parenting

Use Tauri's parent-window API to embed in Bevy:

```rust
let webview_window = tauri::WebviewWindowBuilder::new(
    &app_handle,
    "petal-portal",
    tauri::WebviewUrl::App("index.html".into()),
)
.with_parent_window(bevy_window_hwnd)  // Embed in Bevy window
.build()?;
```

### FR-3: IPC Command System

Expose Tauri commands for PetalPortal interaction:

```rust
#[tauri::command]
fn get_node_data(node_id: String) -> Result<NodeData, String> {
    // Return shared node structure for bridging
}

#[tauri::command]
fn notify_interaction(interaction: WebViewInteraction) -> Result<(), String> {
    // Bridge webview events to Bevy
}
```

### FR-4: Transparent Overlay Support

Configure Tauri webview for transparent background (matching current wry):

```json
{
  "app": {
    "windows": [{
      "transparent": true,
      "decorations": false
    }]
  }
}
```

---

## Shared Node Data Structure (Track 2 Preview)

The key insight: **egui LEADS, Tauri integrates via commands**. Design a shared "node" data structure that bridges Tauri↔Bevy:

```rust
// Shared between Rust (Bevy) and JS (Tauri webview)
#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub enum WebViewInteraction {
    NodeSelected { node_id: String },
    NodeDeselected { node_id: String },
    TransformChanged { node_id: String, transform: TransformData },
    PropertyChanged { node_id: String, key: String, value: PropertyValue },
}
```

This shared structure is the "seam" that track 2 will formalize, and track 5 (Pear) will plug into.

---

## Testing Strategy

- **Compile test**: `cargo check -p fe-webview --features backend-tauri`
- **Backend trait test**: Verify `TauriBackend` implements `WebViewBackend`
- **Integration test**: Run PetalPortal with Tauri backend, verify navigation
- **Transparency test**: Verify webview has transparent background
- **IPC test**: Verify commands work via invoke

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Tauri webview not compatible as child window | Medium | High | Test early; fallback to wry if issues |
| bevy_egui input conflict with webview | Medium | Medium | Use pointer-events CSS to route input |
| Transparency not working | Low | Medium | Test on all platforms |
| Tauri API instability (v2) | Low | Medium | Pin tauri 2.x, test updates |

---

## Design Decisions

### DD-1: Backend Selection

**Chosen**: Add `backend-tauri` as new feature alongside existing `backend-wry`. Not default yet — that's track 3.

### DD-2: bevy_egui Priority

**Chosen**: bevy_egui remains the leading UI. Tauri webview is for PetalPortal only. This is the core principle: **egui LEADS**.

### DD-3: Reference

This track doesn't need the reference implementation (sunxfancy/BevyTauriExample) because we're NOT doing the full shell inversion. We're embedding Tauri as a child window, which is a simpler pattern.

### DD-4: Feature Flag Naming

**Chosen**: `backend-tauri` (not `app-shell-tauri`) because this is about the webview backend, not the app shell.

---

## Documentation Updates

This track updates docs to note Tauri as an option for fe-webview. Full docs updates are in track 3.
