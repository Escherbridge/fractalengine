---
type: Implementation Plan
---

# Implementation Plan: Tauri-Host Shell SPIKE

## Overview

**SPIKE / EXPLORATORY** — Time-boxed investigation, NOT production implementation.

Four-phase approach:
1. Analyze reference implementation
2. Reconcile bevy 0.18 API differences
3. Build minimal proof-of-concept
4. Produce exit report

**Time limit**: 2 weeks (SPIKE)

---

## Phase 1: Reference Implementation Analysis

**Goal:** Document what the reference does and what's needed for bevy 0.18.

---

### Task 1.1 — Analyze tauri_plugin.rs [ ]

Read and document:
- `TauriPlugin` struct and `build()` method
- `run_tauri_app` function
- `CustomRendererPlugin` struct
- Event bridging logic

**Verification:** Document all APIs used.

**Files:** Analysis document (new)

---

### Task 1.2 — Analyze Cargo.toml dependencies [ ]

Document versions used:
- tauri 2
- bevy 0.15.1
- wgpu 23.0.1

Compare to FractalEngine:
- bevy 0.18 (not 0.15.1)
- wgpu version from bevy 0.18

**Verification:** Version gap documented.

**Files:** Version comparison document

---

### Task 1.3 — Analyze window handle bridging [ ]

Document:
- `RawHandleWrapper::new()` usage
- `WindowWrapper::new()` usage
- How the handle is inserted onto Bevy window entity

**Verification:** Handle creation documented.

**Files:** Analysis document

---

### Task 1.4 — Analyze event bridging [ ]

Document what's bridged:
- `WindowEvent::Resized` → `WindowResized`
- `WindowEvent::ScaleFactorChanged` → `WindowScaleFactorChanged`

Note: **Input events are NOT bridged** in the reference — this is a gap for picking.

**Verification:** Event types documented.

**Files:** Analysis document

---

### Phase 1 Checkpoint

- All reference APIs documented
- Version gap identified
- Event bridging gap identified

---

## Phase 2: bevy 0.18 API Reconciliation

**Goal:** Verify which APIs exist in bevy 0.18 and what's changed.

---

### Task 2.1 — Check RenderCreation API [ ]

Search bevy 0.18 source for:
- `RenderCreation::Manual` — does it exist?
- `RenderCreation` enum variants
- Manual creation parameters

**Verification:** API status documented.

**Files:** API compatibility document

---

### Task 2.2 — Check window handle API [ ]

Search for:
- `RawHandleWrapper` — does it exist?
- `WindowWrapper` — does it exist?
- Handle creation pattern

**Verification:** API status documented.

**Files:** API compatibility document

---

### Task 2.3 — Check renderer initialization [ ]

Search for:
- `initialize_renderer()` function
- `RenderPlugin` configuration
- Surface creation from window

**Verification:** API status documented.

**Files:** API compatibility document

---

### Phase 2 Checkpoint

- All required APIs verified in bevy 0.18
- Changes/signature differences documented
- Unknowns identified

---

## Phase 3: Proof-of-Concept Implementation

**Goal:** Build minimal POC to test feasibility.

---

### Task 3.1 — Create minimal Cargo project [ ]

Create a test project with:
- tauri 2
- bevy 0.18
- Minimal window setup

```toml
[package]
name = "tauri-bevy-poc"
version = "0.1.0"

[dependencies]
tauri = { version = "2", features = ["macos-private-api"] }
bevy = "0.18"
wgpu = "23"  # Match bevy's internal wgpu
```

**Verification:** Project compiles.

**Files:** New `tauri-bevy-poc/` directory

---

### Task 3.2 — Implement custom runner [ ]

Port the reference's `run_tauri_app` to bevy 0.18:

```rust
fn run_tauri_app(app: App) -> AppExit {
    let app = Rc::new(RefCell::new(app));
    let mut tauri_app = app.borrow_mut()
        .world_mut()
        .remove_non_send_resource::<tauri::App>()
        .unwrap();

    loop {
        let app_clone = app.clone();
        tauri_app.run_iteration(move |app_handle, event| {
            handle_tauri_events(app_handle, event, app_clone.borrow_mut());
        });

        if tauri_app.webview_windows().is_empty() {
            break;
        }

        app.borrow_mut().update();
    }

    AppExit::Success
}
```

**Verification:** Runner compiles.

**Files:** POC project

---

### Task 3.3 — Implement input bridging [ ]

The critical task — forward input for picking:

```rust
fn handle_cursor_moved(position: PhysicalPosition<i32>, mut app: RefMut<'_, App>) {
    let mut system_state: SystemState<EventWriter<CursorMoved>> = SystemState::new(app.world_mut());
    let mut cursor_moved = system_state.get_mut(app.world_mut());

    cursor_moved.send(CursorMoved {
        window: /* get window entity */,
        position: Vec2::new(position.x as f32, position.y as f32),
    });
}

fn handle_mouse_input(button: MouseButton, state: ButtonState, mut app: RefMut<'_, App>) {
    // Map Tauri mouse button to Bevy
    // Send MouseButtonInput event
}
```

**Verification:** Input bridging compiles.

**Files:** POC project

---

### Task 3.4 — Test picking [ ]

Create a simple 3D scene with selectable entities:
1. Spawn a cube
2. Move mouse over cube
3. Verify picking system detects hover

**Verification:** Picking works (or doesn't).

**Files:** POC project test

---

### Phase 3 Checkpoint

- POC compiles
- Runner works
- Input bridging exists
- Picking tested (result documented)

---

## Phase 4: Exit Report + Decision

**Goal:** Produce recommendation based on evidence.

---

### Task 4.1 — Document findings [ ]

Create exit report:

```markdown
# SPIKE Exit Report: Tauri-Host Shell

## Summary
[One paragraph summary of findings]

## Evidence
### API Compatibility
- List of APIs verified working in bevy 0.18
- List of APIs with changes/signature differences

### Input Bridging
- Result of picking test
- Gaps identified

### bevy_egui Compatibility
- Test result (pass/fail/unknown)
- If fails, error details

## Recommendation

### Option A: GO
Full shell inversion is viable. Proceed with production implementation.

### Option B: NO-GO
Full shell inversion is not viable. Stick with browser-first path (tracks 1-3).

### Option C: CONDITIONAL
Partial adoption possible. [Specify conditions]

## Risk Assessment
[Document residual risks if adopted]
```

**Verification:** Report exists and is complete.

**Files:** `spike-exit-report.md`

---

### Task 4.2 — Update tracks if needed [ ]

If recommendation is GO:
- Note that tracks 1-3 should be revisited
- This spike's findings inform revision

If recommendation is NO-GO:
- No changes to other tracks
- Browser-first path remains

If recommendation is CONDITIONAL:
- Document what needs to change in tracks

**Verification:** Tracks status updated if needed.

**Files:** Tracks metadata (if changed)

---

### Phase 4 Checkpoint

- Exit report complete
- Recommendation clear
- Tracks updated if needed

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|--------------|
| 1 | Reference analysis | Document complete |
| 2 | API compatibility | All APIs verified |
| 3 | POC + picking test | Test result documented |
| 4 | Exit report | GO/NO-GO decision |

## SPIKE Criteria

- [ ] Reference implementation fully analyzed
- [ ] All APIs verified in bevy 0.18
- [ ] Minimal POC compiles
- [ ] Input bridging implemented
- [ ] Picking tested
- [x] Exit report with recommendation (spike-exit-report.md, 2026-07-01 — CONDITIONAL GO; shelved per 2026-07-14 alignment)

## Time Box

**2 weeks** from start to exit report. If not complete, default to NO-GO.
