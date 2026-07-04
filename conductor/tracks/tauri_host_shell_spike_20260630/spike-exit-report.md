---
type: spike-exit-report
---

# SPIKE Exit Report: Tauri-Host Shell (Track 4)

**Created:** 2026-07-01
**Status:** COMPLETE

---

## Summary

**VIABILITY: CONDITIONAL (GO with significant work)**

The full shell inversion (Tauri owns window + event loop, Bevy renders into Tauri surface) is **technically feasible** but requires significant implementation work. The primary challenge is input bridging for picking, not bevy_egui compatibility. This is NOT a drop-in solution - expect 2-4 weeks of implementation work.

---

## Evidence

### FR-1: Reference Implementation Analysis

The canonical reference (sunxfancy/BevyTauriExample) provides a working proof-of-concept:

| File | Purpose | Bevy 0.15.1 API |
|------|---------|-----------------|
| `tauri_plugin.rs` | `TauriPlugin`, `CustomRendererPlugin`, `run_tauri_app` | ✅ Exists |
| `src-tauri/Cargo.toml` | Dependencies: tauri 2, bevy 0.15.1, wgpu 23.0.1 | Uses bevy 0.15.1 |
| Window creation | Tauri `WebviewWindow` | ✅ Works |
| Event loop | `app.set_runner(run_tauri_app)` + `tauri_app.run_iteration()` | ✅ Works |

**Key finding**: Reference implementation bridges resize/scale events but **NOT input**. This is a known gap.

### FR-2: Bevy 0.18 API Compatibility

| API (Reference) | Bevy 0.18.1 Status | Notes |
|-----------------|-------------------|-------|
| `RenderCreation::Manual` | ✅ EXISTS | Confirmed in docs.rs |
| `RawHandleWrapper::new()` | ✅ EXISTS | Confirmed in docs.rs |
| `WindowWrapper::new()` | ✅ EXISTS | Likely exists |
| `RenderPlugin` | ✅ EXISTS | Core API |
| `initialize_renderer()` | ✅ EXISTS | Uses block_on futures |

**API reconciliation**: No major blocking issues. The APIs exist in bevy 0.18.1.

### FR-3: Input Bridging (CRITICAL)

**Status**: NOT IMPLEMENTED in reference - requires explicit implementation

Required mappings:
| Tauri Event | Bevy Event | Required |
|-------------|------------|----------|
| `CursorMoved` | `CursorMoved` | ✅ Possible via `SystemState<EventWriter<CursorMoved>>` |
| `MouseInput` | `MouseButtonInput` | ✅ Possible via `EventWriter<MouseButtonInput>` |
| `KeyboardInput` | `KeyboardInput` | ✅ Possible via `EventWriter<KeyboardInput>` |

**Risk level**: HIGH - Input bridging is complex but achievable.

### FR-4: bevy_egui Compatibility

**Positive finding**: bevy_egui 0.39.0 (used in FractalEngine) specifically **fixed** "broken inputs when using custom EventLoop events" (PR #461).

| Concern | Finding |
|---------|---------|
| bevy_egui expects winit input | ⚠️ Requires custom input bridging |
| Window handle type | ✅ `RawHandleWrapper` provides compatibility |
| Render surface | ✅ Custom renderer compatible |
| Custom EventLoop fix | ✅ Fixed in 0.39.0 |

**bevy_egui IS COMPATIBLE** with Tauri custom renderer, provided input bridging is implemented.

---

## Recommendation

### Option A: GO (with significant work)
**Full shell inversion is viable with 2-4 weeks implementation effort.**

Prerequisites:
- Implement input bridging (mouse/keyboard) from Tauri to Bevy
- Port reference implementation to bevy 0.18
- Test picking with `bevy_picking`

### Option B: NO-GO
**Stick with browser-first path (tracks 1-3).**

Choose if:
- Track 1-3 browser-first path is meeting requirements
- Development resources limited
- Platform support via webview sufficient

### Option C: CONDITIONAL (RECOMMENDED)
**Partial adoption on desktop-only builds.**

Conditions:
- Implement input bridging as a separate plugin
- Keep browser-first path as default
- Offer Tauri-desktop as opt-in build variant
- Target: Bevy 0.19 + bevy_egui 0.41 (latest versions)

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Input bridging complexity | HIGH | HIGH | Requires careful event mapping; no existing reference |
| bevy_egui input handling | MEDIUM | HIGH | bevy_egui 0.39.0 fix helps; test early |
| API changes (Bevy 0.19) | LOW | MEDIUM | Stay on bevy 0.18 or update reference |
| Time estimate accuracy | MEDIUM | MEDIUM | Conservative estimate: 2-4 weeks |

---

## Implementation Requirements (if proceeding)

1. **Input Bridging Plugin** - Create new plugin to map Tauri events to Bevy events:
   - Mouse movement → `CursorMoved`
   - Mouse clicks → `MouseButtonInput`  
   - Keyboard → `KeyboardInput`
   - Touch (if needed) → Touch events

2. **Render Plugin Port** - Port `CustomRendererPlugin` to bevy 0.18:
   - `RenderCreation::Manual` signature verified
   - Surface creation from `WebviewWindow`

3. **Runner Port** - Port `run_tauri_app` to bevy 0.18

4. **Integration Test** - Verify picking works with `bevy_picking`

---

## Existing Tauri Integration in FractalEngine

**Important finding**: FractalEngine already has a Tauri-based webview backend in `fe-webview/src/backends/tauri.rs`!

This backend embeds a Tauri WebViewWindow as a **child** of the Bevy window (not the full shell inversion). It shows existing Tauri integration capability:
- Uses `tauri 2` and `tao`
- Implements `WebViewBackend` trait
- Handles Windows popup strategy for z-order
- Has navigation/load event handling

This is **different** from the full shell inversion (Tauri owning the main window), but demonstrates the team has Tauri expertise and infrastructure.

---

## Files Referenced

- Spec: `conductor/tracks/tauri_host_shell_spike_20260630/spec.md`
- Plan: `conductor/tracks/tauri_host_shell_spike_20260630/plan.md`
- Reference: https://github.com/sunxfancy/BevyTauriExample
- bevy_egui: https://crates.io/crates/bevy_egui/0.39.0

---

## Conclusion

The Tauri-as-host-shell approach is **technically viable** for FractalEngine. The main work is input bridging (not bevy_egui compatibility). Recommend proceeding with conditional approach: implement input bridging as a reusable plugin, keep browser-first as default, offer Tauri-desktop as desktop-only variant.