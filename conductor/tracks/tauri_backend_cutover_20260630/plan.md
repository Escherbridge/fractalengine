---
type: track-plan
---

# Implementation Plan: Tauri Backend Cutover

## Overview

Three-phase implementation:
1. Make Tauri the default browser backend
2. Update documentation + add system dependencies
3. Cleanup + final verification

**TDD is mandatory** where applicable.

**Scope**: This is about the fe-webview browser backend ONLY. Bevy remains the host, bevy_egui remains the leading UI.

---

## Phase 1: Default Backend Migration

**Goal:** `backend-tauri` becomes the default for fe-webview.

---

### Task 1.1 — Update default feature [ ]

In `fe-webview/Cargo.toml`:

```toml
[features]
default = ["backend-tauri"]  # Changed from empty
backend-tauri = ["tauri", "tao"]
backend-wry = []
backend-servo = ["winit"]
# Legacy alias
webview = ["backend-tauri"]
```

**Verification:** Default feature compiles.

**Files:** `fe-webview/Cargo.toml`

---

### Task 1.2 — Update plugin default selection [ ]

In `fe-webview/src/plugin.rs`:

```rust
impl WebViewPlugin {
    pub fn new() -> Self {
        // Default to Tauri backend
        #[cfg(feature = "backend-tauri")]
        return Self {
            backend: BackendKind::Tauri(tauri::TauriBackend::new(/* ... */).unwrap()),
        };

        #[cfg(feature = "backend-wry")]
        return Self {
            backend: BackendKind::Wry(wry::WryBackend::new(/* ... */).unwrap()),
        };

        #[cfg(feature = "backend-servo")]
        return Self {
            backend: BackendKind::Servo(servo::ServoBackend::new(/* ... */).unwrap()),
        };

        // Fallback to stub if nothing selected
        Self {
            backend: BackendKind::Stub(stub::StubBackend::new()),
        }
    }
}
```

**Verification:** Plugin compiles with default selection.

**Files:** `fe-webview/src/plugin.rs`

---

### Task 1.3 — Deprecate wry backend [ ]

Mark the wry backend as deprecated:

```rust
#[deprecated(since = "2026-06-30", note = "Use backend-tauri instead")]
pub struct WryBackend { /* ... */ }
```

And in Cargo.toml:

```toml
[features]
# Deprecated: use backend-tauri instead
backend-wry = ["dep:wry"]
```

**Verification:** Deprecation warning appears.

**Files:** `fe-webview/src/backends/wry.rs`, `fe-webview/Cargo.toml`

---

### Task 1.4 — Verify default builds [ ]

```bash
cargo check -p fe-webview
# Should use Tauri backend by default

cargo check -p fe-webview --no-default-features
# Should compile but with no default backend
```

**Verification:** Both commands succeed.

**Files:** none (verification)

---

### Phase 1 Checkpoint

- `backend-tauri` is default
- Plugin selects Tauri by default
- Wry is deprecated
- Default build succeeds

---

## Phase 2: Documentation + System Deps

**Goal:** Documentation reflects Tauri as default, system deps documented.

---

### Task 2.1 — Update BUILDING.md [ ]

Add Linux system dependencies for Tauri webview:

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
  libssl-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev \
  libgtk-3-dev
```

Fedora:
```bash
sudo dnf install -y \
  pkg-config \
  dbus-devel \
  webkit2gtk4.1-devel \
  alsa-lib-devel \
  systemd-devel \
  openssl-devel \
  libxcb-devel \
  gtk3-devel
```

**Note**: `libwebkit2gtk-4.1-dev` is required for the Tauri webview backend (default since 2026-06-30).
```

**Verification:** Instructions are accurate.

**Files:** `BUILDING.md`

---

### Task 2.2 — Update AGENTS.md [ ]

Add fe-webview backend section:

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

**Verification:** Section renders correctly.

**Files:** `AGENTS.md`

---

### Task 2.3 — Update tech-stack.md [ ]

Add Tauri backend to the architecture diagram:

```
fe-webview
├── backend-tauri (default) ← NEW
├── backend-wry (deprecated)
└── backend-servo (optional)

PetalPortal → Tauri webview → IPC commands
```

**Verification:** Diagram is accurate.

**Files:** `tech-stack.md`

---

### Task 2.4 — Verify docs render [ ]

Open rendered docs and verify:
- BUILDING.md has Linux deps
- AGENTS.md has backend table
- tech-stack.md has updated diagram

**Verification:** All docs render correctly.

**Files:** none (verification)

---

### Phase 2 Checkpoint

- BUILDING.md has system deps
- AGENTS.md documents backends
- tech-stack.md updated
- All docs render

---

## Phase 3: Cleanup + Verification

**Goal:** Final verification and optional cleanup.

---

### Task 3.1 — Full workspace build [ ]

```bash
cargo check --workspace
```

**Verification:** Workspace compiles.

**Files:** none (verification)

---

### Task 3.2 — Test PetalPortal with default [ ]

1. Run fractalengine
2. Select node with webpage_url
3. Verify PetalPortal opens (Tauri backend)
4. Navigate, close, verify works

**Verification:** PetalPortal works with default backend.

**Files:** none (manual test)

---

### Task 3.3 — Document decisions [ ]

Create a decision record:

```markdown
## Decision: Tauri as Default Browser Backend

**Date**: 2026-06-30

**Decision**: backend-tauri is now the default for fe-webview

**Rationale**:
- More robust IPC via invoke()
- Custom protocol support
- Plugin ecosystem
- Better maintained than raw wry

**Alternatives considered**:
- Keep wry as default (rejected: less featureful)
- Full shell inversion (deferred to track 4 SPIKE)

**Deprecation**: backend-wry is deprecated but kept for debugging
```

**Verification:** Decision documented.

**Files:** New decision record (optional)

---

### Phase 3 Checkpoint

- Workspace compiles
- PetalPortal works with default
- Documentation is complete

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|--------------|
| 1 | Default backend change | `backend-tauri` is default |
| 2 | Docs + deps | BUILDING.md, AGENTS.md updated |
| 3 | Verification | Full test passes |

## Quality Gates

- [ ] `cargo check -p fe-webview` uses Tauri by default
- [ ] PetalPortal works with default backend
- [ ] BUILDING.md has Linux system deps
- [ ] AGENTS.md documents backends
- [ ] Workspace compiles

## Notes

- This track is about browser backend ONLY
- Full shell inversion is track 4 (SPIKE)
- Pear P2P is track 5 (SPIKE)
