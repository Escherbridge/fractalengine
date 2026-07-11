---
type: track-spec
---

# Track: Tauri Backend Cutover — Make Tauri Default Browser, Retire Raw wry

**Created:** 2026-06-30
**Status:** Draft
**Priority:** P1
**Depends on:** Tauri IPC/Asset Bridge (track 2)
**Blocks:** none

---

## Problem Statement

Tracks 1 and 2 established:
1. **Tauri webview backend** — robust, interoperable browser for fe-webview
2. **IPC command bridge** — typed commands + shared node structure
3. **Asset protocol** — secure local file serving

This track makes **Tauri the default browser backend** in fe-webview and optionally retires the raw wry FFI overlay. The key scope distinction: **this is about the browser backend only**, NOT about making Tauri the app shell.

**Bevy stays the host. bevy_egui remains the leading UI.**

---

## Goals

1. Make `backend-tauri` the default feature in `fe-webview`
2. Update `fe-webview` to use Tauri backend by default
3. Update documentation: `AGENTS.md`, `tech-stack.md`, `BUILDING.md`
4. Add system dependencies: webkit2gtk on Linux
5. Decide on raw wry backend: deprecate or remove

---

## Non-Goals (this track)

- **Full shell inversion** (Tauri hosts window/event loop) — track 4 (SPIKE)
- Replacing bevy_egui with web UI
- Changing the default app entry point
- Pear P2P integration — track 5 (SPIKE)

---

## Scope Clarification: Browser Backend vs. App Shell

| Aspect | This Track (Browser) | Track 4 SPIKE (App Shell) |
|--------|---------------------|---------------------------|
| What changes | fe-webview PetalPortal backend | fractalengine entry point |
| Bevy role | Host (unchanged) | Renderer into Tauri surface |
| bevy_egui | Leading UI (unchanged) | Uncertain compatibility |
| Window ownership | Bevy/winit | Tauri |
| Input handling | bevy_winit | Must forward manually |

This track is **incremental** within fe-webview. Track 4 is a **fundamental architecture change**.

---

## Architecture

### Default Feature Change

```toml
# fe-webview/Cargo.toml

[features]
# default = ["backend-tauri"]  # Make Tauri the default
backend-tauri = ["tauri", "tao"]
backend-wry = []  # Keep for debugging
backend-servo = ["winit"]  # Keep as option
```

### Plugin Configuration Change

```rust
// fe-webview/src/plugin.rs

impl WebViewPlugin {
    pub fn new() -> Self {
        // Default to Tauri backend
        #[cfg(feature = "backend-tauri")]
        return Self::tauri();

        #[cfg(feature = "backend-wry")]
        return Self::wry();

        #[cfg(feature = "backend-servo")]
        return Self::servo();

        // Fallback
        Self::stub()
    }
}
```

---

## Functional Requirements

### FR-1: Default Backend Selection

When no feature is explicitly specified, use Tauri:

```toml
[features]
default = ["backend-tauri"]  # Changed from empty
```

### FR-2: Backward Compatibility

Keep `webview` alias working:

```toml
[features]
# Legacy alias — now maps to Tauri
webview = ["backend-tauri"]
```

### FR-3: System Dependencies

**Linux**: Add webkit2gtk (required for Tauri webview)

```bash
# Ubuntu/Debian
sudo apt-get install libwebkit2gtk-4.1-dev

# Fedora
sudo dnf install webkit2gtk4.1-devel
```

**macOS**: WKWebView (built-in, no extra deps)

**Windows**: WebView2 (likely already installed)

### FR-4: Documentation Update

Update:
- `AGENTS.md` — Document new fe-webview architecture
- `tech-stack.md` — Show Tauri backend in diagram
- `BUILDING.md` — Add webkit2gtk to Linux deps

---

## Wry Backend Decision

Options for the raw wry backend:

| Option | Description | Recommendation |
|--------|-------------|----------------|
| A | Keep for debugging | **Recommended** — mark deprecated |
| B | Remove entirely | Deferred to future cleanup |

**Rationale**: The raw wry backend can be useful for debugging or if Tauri has issues. Deprecate it but don't remove yet.

---

## Testing Strategy

- **Compile test**: `cargo check -p fe-webview` (default features)
- **Backend test**: Verify Tauri backend is selected by default
- **Integration test**: Run PetalPortal with default backend
- **Documentation test**: Verify BUILDING.md instructions work

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| webkit2gtk missing on Linux | Medium | High | Document in BUILDING.md |
| Tauri backend has edge case | Low | Medium | Keep wry as fallback |
| Users need to update features | Low | Low | Default feature handles it |

---

## Design Decisions

### DD-1: Default Selection

**Chosen**: `backend-tauri` becomes default. Simple, non-breaking.

### DD-2: Wry Backend

**Chosen**: Keep but deprecate. Mark with `#[deprecated]` attribute.

### DD-3: Documentation Priority

**Chosen**: Update BUILDING.md first (users need system deps), then AGENTS.md.

---

## Documentation Updates Required

### AGENTS.md

Add section on fe-webview backends:

```markdown
## WebView / PetalPortal

fe-webview provides the browser overlay for PetalPortal. Available backends:

| Backend | Description | Default |
|---------|-------------|---------|
| `backend-tauri` | Tauri-powered webview with IPC | Yes |
| `backend-wry` | Raw wry FFI | Deprecated |
| `backend-servo` | Servo browser (feature-gated) | No |

Since 2026-06-30, Tauri is the default backend for robust IPC and custom protocol support.
```

### BUILDING.md

Add Linux system dependencies:

```markdown
## System Dependencies

### Linux (for Tauri webview)

Ubuntu/Debian:
```bash
sudo apt-get install -y \
  pkg-config \
  libdbus-1-dev \
  libwebkit2gtk-4.1-dev \
  libasound2-dev \
  libudev-dev \
  libssl-dev
```

Fedora:
```bash
sudo dnf install -y \
  pkg-config \
  dbus-devel \
  webkit2gtk4.1-devel \
  alsa-lib-devel \
  systemd-devel \
  openssl-devel
```
```

### tech-stack.md

Update diagram to show Tauri backend in fe-webview.
