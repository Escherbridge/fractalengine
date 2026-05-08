# Specification: Build Size Optimization & Mobile Deployment Preparation

## Overview

Reduce FractalEngine binary sizes to meet mobile deployment constraints and improve desktop distribution. The GUI binary is currently 154 MB and the relay binary is 106 MB. The Android APK limit is 150 MB; the target range for a mobile-viable binary is 50-100 MB. This track focuses on dependency pruning (tokio, Bevy plugins) and documenting the mobile thin-client architecture strategy.

**Track Type:** Chore
**Track ID:** `build_size_mobile_prep_20260508`

## Background

- Release profile optimizations (strip, LTO=fat, codegen-units=1, panic=abort) are **already applied** in the workspace Cargo.toml.
- `tokio` is declared with `features = ["full"]` at the workspace level but only 6 of 11 feature groups are actually used: `rt-multi-thread`, `macros`, `sync`, `time`, `signal`, `net`.
- Bevy is pulled with all default features (including `bevy_audio`, `bevy_gilrs` for gamepad input). Neither audio playback nor gamepad input are used by FractalEngine.
- Mobile is explicitly a v2 non-goal (product.md), so this track is **preparation** only: documenting the thin-client relay architecture, not implementing native mobile builds.
- 1,296 unique dependencies in Cargo.lock contribute to compile time and binary size.

## Functional Requirements

### FR-1: Tokio Feature Pruning

**Description:** Replace `tokio = { version = "1", features = ["full"] }` in workspace Cargo.toml with the minimal feature set actually used across all crates.

**Acceptance Criteria:**
- [ ] Workspace tokio dependency uses explicit features: `["rt-multi-thread", "macros", "sync", "time", "signal", "net"]`
- [ ] All workspace crates compile without errors (`cargo build --workspace`)
- [ ] All workspace tests pass (`cargo test --workspace`)
- [ ] No crate-level Cargo.toml overrides the workspace tokio features

**Priority:** P1

### FR-2: Bevy Plugin Slimming

**Description:** Replace `bevy = { version = "0.18" }` (all default features) with a curated feature set that excludes unused subsystems, or use `DefaultPlugins` with `.disable::<T>()` where appropriate.

**Acceptance Criteria:**
- [ ] `bevy_audio` plugin/feature is excluded from both GUI and relay binaries
- [ ] `bevy_gilrs` (gamepad) feature is excluded from both binaries
- [ ] GUI binary still renders 3D content, loads GLTF/GLB assets, and displays egui overlays
- [ ] Relay binary still runs headless with `MinimalPlugins`
- [ ] All existing tests pass

**Priority:** P1

### FR-3: Dependency Audit Report

**Description:** Generate a before/after comparison of binary sizes and dependency counts to verify the impact of FR-1 and FR-2.

**Acceptance Criteria:**
- [ ] Baseline measurements recorded before changes (GUI and relay binary sizes, `cargo tree` dependency count)
- [ ] Post-optimization measurements recorded
- [ ] Delta documented in a commit message or PR description
- [ ] Combined savings target: at least 5 MB reduction in GUI binary, at least 3 MB reduction in relay binary

**Priority:** P1

### FR-4: Mobile Architecture Strategy Document

**Description:** Write a technical document describing the thin-client relay architecture for future mobile deployment. This is documentation only -- no code changes.

**Acceptance Criteria:**
- [ ] Document covers: relay as backend, REST/WS API as the interface surface, no native SurrealDB/iroh/libp2p on mobile
- [ ] Document describes what a mobile client needs: HTTP client, WebSocket client, WebView for petal portals, local credential storage
- [ ] Document identifies platform-specific concerns: Android APK size limits, iOS App Store constraints, keychain APIs
- [ ] Document lives at `docs/mobile-architecture.md`
- [ ] Document explicitly states mobile is a v2 goal (aligns with product.md)

**Priority:** P2

## Non-Functional Requirements

### NFR-1: Build Time

- Build time for `cargo build --release --workspace` must not regress by more than 10% (LTO is already fat; no new cost expected).

### NFR-2: Runtime Performance

- No measurable runtime performance regression from feature pruning. Tokio feature pruning removes unused code paths. Bevy plugin exclusion removes unused subsystems.

### NFR-3: CI Compatibility

- Changes must not break existing CI scripts or the cross-platform desktop track's build targets (Linux, macOS, Windows, ARM64).

## User Stories

### US-1: Developer reducing binary size
**As a** developer preparing for distribution,
**I want** the release binaries to be as small as possible,
**So that** download times are reasonable and mobile deployment becomes feasible.

**Given** the current workspace Cargo.toml with `tokio = ["full"]`
**When** I replace it with the minimal feature set
**Then** the compiled binary excludes unused tokio subsystems (io-util, fs, process, tracing, parking_lot)

### US-2: Developer reviewing mobile readiness
**As a** developer planning the v2 mobile client,
**I want** a clear architecture document for mobile thin-client access,
**So that** I can design the mobile client without re-discovering the relay capabilities.

**Given** the relay binary already exposes REST/WS/MCP APIs
**When** I read the mobile architecture document
**Then** I understand what the mobile client needs to implement and what the relay handles

## Technical Considerations

1. **Bevy 0.18 feature granularity:** Bevy 0.18 supports `default-features = false` with granular feature flags. The key features needed are: `bevy_asset`, `bevy_core_pipeline`, `bevy_pbr`, `bevy_render`, `bevy_winit`, `bevy_gltf`, `bevy_ui`, `bevy_scene`, `bevy_gizmos`, `bevy_diagnostic`, `bevy_state`, `multi_threaded`, `png`, `x11`, `wayland`. The exact set needs validation.
2. **Workspace-level Bevy features:** Since `bevy = { workspace = true }` is used by 12 crates, the workspace definition must include all features needed by any crate. The relay binary will inherit features it does not use; this is acceptable because Cargo only links what is actually called.
3. **tokio feature "signal":** Only used in the relay binary for `ctrl_c()`. This feature is platform-specific (Unix signals vs Windows). Verify it works on all targets.
4. **libp2p features:** Already minimal (`kad`, `quic`, `tokio`, `macros`). No changes needed.
5. **Existing profile.release:** Already optimal (strip, fat LTO, codegen-units=1, panic=abort). No changes needed.

## Out of Scope

- Native mobile builds (Android/iOS) -- v2 goal
- WASM/browser client -- v2 goal
- Dependency vendoring or auditing for supply chain security
- Replacing SurrealDB, libp2p, or iroh with lighter alternatives
- Dynamic linking or shared library approaches
- UPX or other post-build binary compression

## Open Questions

1. **Bevy 0.18 exact feature flags:** The list of available features may have changed from 0.15. Need to verify against the actual Bevy 0.18 Cargo.toml or docs.
2. **bevy_gizmos dependency:** The `fe-renderer` crate uses axis gizmos. Confirm `bevy_gizmos` is included in the custom feature set.
3. **bevy_state dependency:** Some crates may use Bevy states. Verify before excluding.
