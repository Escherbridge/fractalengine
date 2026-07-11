---
type: track-plan
---

# Implementation Plan: Tauri WebView Backend

## Overview

Three-phase implementation:
1. Integrate Tauri backend into fe-webview
2. Add feature flags and default selection logic
3. Verify PetalPortal parity with new backend

**TDD is mandatory** where applicable.

This track keeps Bevy as host — the webview is embedded as a child window, NOT a full shell inversion.

---

## Phase 1: Tauri Backend Integration

**Goal:** `backend-tauri` feature compiles and provides a working webview backend.

---

### Task 1.1 — Add Tauri dependencies [ ]

Add to `fe-webview/Cargo.toml`:

```toml
[features]
default = []
backend-servo = ["winit"]
backend-wry = []
backend-tauri = ["tauri", "tao"]  # NEW
webview = ["backend-wry"]

[dependencies]
# ... existing deps
tauri = { version = "2", optional = true }
tao = { version = "0.30", optional = true }
```

**Verification:** `cargo check -p fe-webview --features backend-tauri` compiles.

**Files:** `fe-webview/Cargo.toml`

---

### Task 1.2 — Create Tauri backend module [ ]

Create `fe-webview/src/backends/tauri.rs`:

```rust
use bevy::prelude::*;
use wry::WebView;

pub struct TauriBackend {
    // Tauri-specific fields
}

impl WebViewBackend for TauriBackend {
    fn navigate(&mut self, url: &Url) -> Result<(), WebViewError> {
        todo!()
    }

    fn url(&self) -> Result<Url, WebViewError> {
        todo!()
    }

    fn close(&mut self) -> Result<(), WebViewError> {
        todo!()
    }

    fn eval(&mut self, js: &str) -> Result<(), WebViewError> {
        todo!()
    }
}
```

**Verification:** Compiles with stub implementations.

**Files:** New `fe-webview/src/backends/tauri.rs`

---

### Task 1.3 — Implement window parenting [ ]

Implement embedding Tauri webview as child of Bevy window:

```rust
impl TauriBackend {
    pub fn new(parent_window: bevy::window::RawHandleWrapper) -> Result<Self, WebViewError> {
        let webview_window = tauri::WebviewWindowBuilder::new(
            &app_handle,
            "petal-portal",
            tauri::WebviewUrl::App("index.html".into()),
        )
        .with_parent_window(parent_window)  // Embed in Bevy window
        .build()?;

        Ok(Self { webview_window, app_handle })
    }
}
```

**Verification:** Creates webview as child window.

**Files:** `fe-webview/src/backends/tauri.rs`

---

### Task 1.4 — Implement WebViewBackend trait [ ]

Complete the trait implementation:

```rust
impl WebViewBackend for TauriBackend {
    fn navigate(&mut self, url: &Url) -> Result<(), WebViewError> {
        self.webview_window
            .eval(&format!("window.location.href = '{}';", url.as_str()))
            .map_err(|e| WebViewError::Navigation(e.to_string()))
    }

    fn url(&self) -> Result<Url, WebViewError> {
        // Use invoke to query URL from webview
        Ok(Url::parse("about:blank"))  // TODO: implement
    }

    fn close(&mut self) -> Result<(), WebViewError> {
        self.webview_window.close().map_err(|e| WebViewError::Close(e.to_string()))
    }

    fn eval(&mut self, js: &str) -> Result<(), WebViewError> {
        self.webview_window.eval(js).map_err(|e| WebViewError::Eval(e.to_string()))
    }
}
```

**Verification:** All trait methods compile.

**Files:** `fe-webview/src/backends/tauri.rs`

---

### Phase 1 Checkpoint

- `backend-tauri` feature compiles
- `TauriBackend` implements `WebViewBackend` trait
- Webview can be created as child of Bevy window

---

## Phase 2: Feature Flags + Default Selection

**Goal:** Proper feature gate and registry integration.

---

### Task 2.1 — Register Tauri backend in mod.rs [ ]

Update `fe-webview/src/backends/mod.rs`:

```rust
mod tauri;  // NEW
mod wry;
mod servo;
mod stub;

pub use tauri::TauriBackend;  // NEW

pub type ActiveBackend = BackendKind;

pub enum BackendKind {
    Wry(wry::WryBackend),
    Servo(servo::ServoBackend),
    Tauri(tauri::TauriBackend),  // NEW
    Stub(stub::StubBackend),
}
```

**Verification:** Backend enum compiles.

**Files:** `fe-webview/src/backends/mod.rs`

---

### Task 2.2 — Add backend selection to fe-webview [ ]

Update the main webview plugin to support Tauri:

```rust
// fe-webview/src/plugin.rs

#[derive(Resource)]
pub struct WebViewPlugin {
    pub backend: BackendKind,
}

impl WebViewPlugin {
    pub fn new() -> Self {
        // Select backend based on features
        #[cfg(feature = "backend-tauri")]
        let backend = BackendKind::Tauri(tauri::TauriBackend::new(/* ... */));

        #[cfg(feature = "backend-wry")]
        let backend = BackendKind::Wry(wry::WryBackend::new(/* ... */));

        // ... etc
    }
}
```

**Verification:** Plugin compiles with feature selection.

**Files:** `fe-webview/src/plugin.rs`

---

### Task 2.3 — Document backend selection [ ]

Add documentation to `fe-webview/README.md` or crate docs:

```markdown
## Available Backends

- `backend-wry` (default): Raw wry FFI overlay
- `backend-tauri`: Tauri-powered webview with IPC
- `backend-servo`: Servo browser engine (feature-gated)
- `stub`: No-op backend for testing
```

**Verification:** Docs render correctly.

**Files:** Documentation update

---

### Phase 2 Checkpoint

- Backend selection works via feature flags
- Plugin creates correct backend type
- Documentation describes options

---

## Phase 3: PetalPortal Parity

**Goal:** PetalPortal works identically with Tauri backend.

---

### Task 3.1 — Verify navigation works [ ]

Test PetalPortal navigation with Tauri backend:

```rust
#[test]
fn tauri_backend_navigation() {
    let mut backend = TauriBackend::new(parent_window);
    let url = Url::parse("https://example.com").unwrap();

    backend.navigate(&url).expect("Navigation should succeed");
}
```

**Verification:** Test passes.

**Files:** Test in `fe-webview/tests/`

---

### Task 3.2 — Verify transparency works [ ]

Ensure transparent overlay works with Tauri:

```json
// Tauri config
{
  "app": {
    "windows": [{
      "transparent": true,
      "decorations": false
    }]
  }
}
```

And in backend:

```rust
impl TauriBackend {
    fn configure_transparent(window: &mut tauri::WebviewWindowBuilder) {
        window.set_decorations(false);
    }
}
```

**Verification:** Webview has transparent background.

**Files:** Config + backend code

---

### Task 3.3 — Manual PetalPortal test [ ]

Manual verification:
1. Run fractalengine with `--features backend-tauri`
2. Select a node with `webpage_url`
3. Verify portal opens with Tauri backend
4. Navigate, close, verify all work

**Verification:** User confirms functionality.

**Files:** none (manual test)

---

### Phase 3 Checkpoint

- PetalPortal navigation works with Tauri backend
- Transparent overlay works
- Manual test passes

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|--------------|
| 1 | Tauri backend implementation | `backend-tauri` compiles, trait implemented |
| 2 | Feature flag integration | Backend selection works |
| 3 | PetalPortal parity | All functionality works |

## Quality Gates

- [ ] `cargo check -p fe-webview --features backend-tauri` passes
- [ ] `TauriBackend` implements `WebViewBackend` trait
- [ ] PetalPortal works with Tauri backend
- [ ] Transparency works correctly
- [ ] Manual test passes

## Next Steps

This track delivers the Tauri webview backend. Track 2 adds IPC commands and the shared node structure. Track 3 makes Tauri the default browser backend.
