---
type: track-spec
---

# Track: Tauri-Host Shell SPIKE — Exploratory: Tauri as Primary Window + Bevy Render-into-Surface

**Created:** 2026-06-30
**Status:** In progress (SPIKE / exploratory) — Phases 1, 2, and Task 4.1 covered by spike-exit-report.md (2026-07-01); deferred per 2026-07-14 alignment (OFF-STRATEGY / DEFER, P3)
**Priority:** P2
**Depends on:** none
**Blocks:** none (this is a SPIKE, not on the critical path)

---

## SPIKE Purpose

This is a **time-boxed exploratory track** to evaluate the feasibility of a full architecture inversion:
- **Current**: Bevy owns window + event loop (via bevy_winit), Tauri webview embedded as child
- **SPIKE**: Tauri owns window + event loop, Bevy renders INTO the Tauri surface

**This is NOT the committed architecture.** It may be adopted later if the spike proves viable. It must NOT block tracks 1-3 (the browser-first path).

---

## Problem Statement

The reference implementation (sunxfancy/BevyTauriExample) demonstrates a working technique:
- Tauri owns window and event loop
- Bevy renders into Tauri surface via custom renderer
- Uses `app.set_runner(run_tauri_app)`, not Bevy's default runner

However:
- Reference uses **bevy 0.15.1**, FractalEngine uses **bevy 0.18**
- Reference bridges resize/scale events but **NOT input** — critical for picking
- bevy_egui compatibility with custom renderer is unknown
- The technique requires significant API reconciliation

This spike evaluates whether this inversion is viable for FractalEngine.

---

## Goals (SPIKE)

1. Analyze the reference implementation in detail
2. Reconcile bevy 0.15 → 0.18 API differences
3. Implement a minimal proof-of-concept
4. Produce an exit report: go/no-go recommendation

---

## Non-Goals (SPIKE)

- Full production implementation (not the goal)
- Making this the default (would require track 3 revision)
- Replacing bevy_egui (compatibility question, not goal)

---

## Reference Implementation

The canonical reference is **sunxfancy/BevyTauriExample**:

### Key Files

| File | Purpose |
|------|---------|
| `src-tauri/src/tauri_plugin.rs` | Core: `TauriPlugin`, `CustomRendererPlugin`, `run_tauri_app` runner |
| `src-tauri/src/bevy.rs` | Bevy app setup with minimal plugins (no `WinitPlugin`) |
| `src-tauri/src/main.rs` | Entry point |
| `src-tauri/Cargo.toml` | Versions: `tauri = "2"`, `bevy = "0.15.1"`, `wgpu = "23.0.1"` |
| `tauri.conf.json` | Window config (`transparent: true`, `macos-private-api`) |

### Core APIs Used (bevy 0.15.1)

```rust
// Custom runner replaces Bevy's default runner
app.set_runner(run_tauri_app);

// Run loop pumps Tauri events then Bevy update
tauri_app.run_iteration(|app_handle, event| {
    handle_tauri_events(app_handle, event, app.borrow_mut());
});
app.update();

// Renderer initialization (bevy 0.15 style)
app.add_plugins(RenderPlugin {
    render_creation: RenderCreation::Manual(device, queue, adapter_info, adapter, RenderInstance(...)),
    ..default()
});

// Window handle bridging
RawHandleWrapper::new(WindowWrapper::new(tauri_window))
```

---

## The bevy_winit Question

**Is `bevy_winit` needed?**

Under full shell inversion, `bevy_winit` is **REMOVED**. Here's what it provides and how to replace it:

| winit Responsibility | Replacement in Full Inversion |
|----------------------|-------------------------------|
| Window creation | Tauri `WebviewWindow` |
| Event loop / app runner | `app.set_runner(run_tauri_app)` + `tauri_app.run_iteration()` |
| Window handle for renderer | `RawHandleWrapper::new(WindowWrapper::new(tauri_window))` |
| Resize / scale-factor events | Bridged from Tauri `WindowEvent::Resized/ScaleFactorChanged` |
| **INPUT (mouse/keyboard)** | **Must be explicitly forwarded** from Tauri/tao to Bevy input events |

**Key insight**: The reference implementation bridges resize/scale but NOT input. FractalEngine uses `bevy_picking` which absolutely requires mouse position and click events. This is the primary technical risk.

---

## The egui Reconciliation Question

**Does bevy_egui work when Tauri hosts?**

This is the **key spike question**. bevy_egui expects:
- A Bevy window with proper handle
- Input events from bevy_winit
- Render to the wgpu surface

When Tauri owns the window:
1. The Bevy window exists but with a different handle type
2. Input comes from Tauri, not winit
3. Render surface is created differently

**Unknown**: Does bevy_egui 0.39 work with this setup?

---

## Version Reconciliation (bevy 0.15 → 0.18)

The APIs have likely shifted:

| API (0.15) | Likely Status in 0.18 | Notes |
|------------|----------------------|-------|
| `RenderCreation::Manual` | Likely exists | Verify signature |
| `initialize_renderer()` | Likely exists | Check in `bevy::render::renderer` |
| `RawHandleWrapper::new()` | Verify | Bevy windowing API |
| `WindowWrapper::new()` | Verify | Bevy windowing API |
| `WgpuWrapper` | Likely renamed or changed | wgpu wrapper type |

**Risk**: Breaking changes between versions. This spike treats API verification as real work with unknowns.

---

## Functional Requirements (SPIKE)

### FR-1: Reference Analysis

Document:
- All APIs used in reference
- Which exist in bevy 0.18
- Which have changed signatures

### FR-2: Input Bridging Proof

Implement mouse/keyboard forwarding to verify picking works:
- `CursorMoved` → map to Bevy mouse motion
- `MouseInput` → map to Bevy mouse button events  
- `KeyboardInput` → map to Bevy keyboard events

### FR-3: bevy_egui Compatibility Test

Attempt to render an egui panel with the custom renderer:
- If works → note success
- If fails → document error, note as risk

### FR-4: Exit Report

Produce a decision document:
- **GO**: Adopt full inversion (requires track revision)
- **NO-GO**: Stick with browser-first path (tracks 1-3)
- **CONDITIONAL**: Partial adoption (e.g., only on certain platforms)

---

## Testing Strategy (SPIKE)

- **Compile test**: Can we compile a minimal POC?
- **Input test**: Does picking work with bridged input?
- **egui test**: Does egui render with custom renderer?
- **Decision**: Go/No-go based on evidence

---

## Risks and Mitigations (SPIKE)

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| bevy 0.18 API incompatibility | High | High | Treat as unknown, verify each API |
| Input bridging incomplete | High | High | Explicit task, test picking |
| bevy_egui incompatibility | Medium | High | Test early, fallback to browser-only |
| Time-box overrun | Medium | Medium | Strict 2-week limit |

---

## Design Decisions (Deferred)

This spike does NOT make design decisions. It produces recommendations:
- Recommend adopting full inversion (if viable)
- Recommend sticking with browser-first (if not viable)
- Recommend conditional approach (if partially viable)

---

## SPIKE Exit Criteria

This spike is complete when:
1. Reference implementation APIs are analyzed
2. bevy 0.18 API compatibility is verified
3. A minimal POC compiles and runs
4. Input bridging is tested with picking
5. bevy_egui compatibility is tested
6. Exit report with GO/NO-GO/CONDITIONAL recommendation exists
