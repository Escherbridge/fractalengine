# Implementation Plan: Build Size Optimization & Mobile Deployment Preparation

## Overview

Four phases targeting binary size reduction and mobile architecture documentation. Phase 1 captures baseline measurements. Phase 2 prunes tokio features. Phase 3 slims Bevy plugins. Phase 4 documents the mobile thin-client strategy. Each phase ends with a verification checkpoint.

**Estimated total effort:** 4-6 hours
**Risk level:** Low (dependency feature pruning is additive removal, easily reverted)

## Phase 1: Baseline Measurements

Goal: Record current binary sizes and dependency counts so post-optimization deltas are quantifiable.

Tasks:
- [ ] Task 1.1: Build both binaries in release mode and record sizes. Run `cargo build --release -p fractalengine -p fractalengine-relay`, then record file sizes of `target/release/fractalengine.exe` and `target/release/fractalengine-relay.exe`. Also run `cargo tree --workspace --depth 0 | wc -l` to count direct dependencies and `cargo tree --workspace | wc -l` for total dependency tree lines.
- [ ] Task 1.2: Record `cargo tree -p tokio --depth 1` output to document which tokio sub-features are currently compiled. Save to a temporary baseline file or commit message.
- [ ] Verification: Baseline numbers documented. GUI ~154 MB, Relay ~106 MB expected. [checkpoint marker]

## Phase 2: Tokio Feature Pruning

Goal: Replace `tokio = ["full"]` with the minimal feature set. Expected savings: 1-3 MB (tokio sub-crates are relatively small, but reducing features also trims transitive deps).

Tasks:
- [ ] Task 2.1: Write a compile-test. Create a minimal test or build check that exercises each required tokio feature (`rt-multi-thread`, `macros`, `sync`, `time`, `signal`, `net`) to confirm they are sufficient. (TDD: write a test that imports `tokio::signal::ctrl_c`, `tokio::sync::broadcast`, `tokio::time::sleep`, `tokio::net::TcpListener` -- if it compiles, the features are correct.)
- [ ] Task 2.2: Update workspace Cargo.toml. Change `tokio = { version = "1", features = ["full"] }` to `tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "signal", "net"] }`.
- [ ] Task 2.3: Full workspace build and test. Run `cargo build --workspace` and `cargo test --workspace`. Fix any compilation errors from missing features (e.g., if any crate uses `tokio::fs` or `tokio::io` utilities not covered by `net`).
- [ ] Task 2.4: Record post-change binary sizes and dependency delta.
- [ ] Verification: `cargo test --workspace` passes. Binary sizes recorded. Commit with message `chore: prune tokio features — replace "full" with minimal set`. [checkpoint marker]

## Phase 3: Bevy Plugin Slimming

Goal: Exclude unused Bevy subsystems (audio, gamepad) from the workspace dependency. Expected savings: 2-8 MB (bevy_audio pulls in rodio/cpal/symphonia; bevy_gilrs pulls in gilrs).

Tasks:
- [ ] Task 3.1: Audit Bevy feature usage. Run `cargo tree -p bevy --depth 2 -f "{p} {f}"` to see which Bevy sub-crates are currently pulled. Identify which default features can be disabled. Check Bevy 0.18's Cargo.toml (or docs) for the exact feature flag names.
- [ ] Task 3.2: Write a build-verification test. Ensure the GUI binary still compiles and the renderer, gizmos, GLTF loading, and egui overlay work. This is a compile-time check plus a manual smoke test. (TDD: existing tests in fe-renderer and fe-ui should cover this.)
- [ ] Task 3.3: Update workspace Cargo.toml. Change `bevy = { version = "0.18" }` to `bevy = { version = "0.18", default-features = false, features = [...] }` with the curated feature list. The list should include at minimum: `bevy_asset`, `bevy_core_pipeline`, `bevy_pbr`, `bevy_render`, `bevy_winit`, `bevy_gltf`, `bevy_scene`, `bevy_gizmos`, `bevy_diagnostic`, `bevy_state`, `bevy_text`, `bevy_sprite`, `multi_threaded`, `png`, `x11`, `wayland`. Exclude: `bevy_audio`, `bevy_gilrs`.
  - **Note:** If Bevy 0.18 feature names differ from 0.15, adjust accordingly. The key principle is: disable audio and gamepad, keep everything else.
  - **Alternative approach:** If granular features prove fragile, keep `default-features = true` and instead call `.disable::<AudioPlugin>()` and `.disable::<GilrsPlugin>()` on `DefaultPlugins` in `fe-runtime/src/app.rs`. This is less impactful on binary size (the code is still compiled) but simpler to maintain.
- [ ] Task 3.4: Verify relay binary. The relay uses `MinimalPlugins` and should be unaffected, but confirm it still compiles: `cargo build --release -p fractalengine-relay`.
- [ ] Task 3.5: Full workspace test. Run `cargo test --workspace`. Fix any compilation errors.
- [ ] Task 3.6: Record post-change binary sizes and total dependency delta from baseline.
- [ ] Verification: Both binaries compile. All tests pass. Binary size delta documented. Commit with message `chore: slim Bevy plugins — exclude audio and gamepad subsystems`. [checkpoint marker]

## Phase 4: Mobile Architecture Strategy Document

Goal: Write a technical document describing the thin-client relay architecture for future mobile deployment. No code changes.

Tasks:
- [ ] Task 4.1: Draft document outline. Sections: Executive Summary, Architecture Overview (relay as backend, mobile as thin client), API Surface (REST/WS/MCP endpoints the mobile client consumes), Platform Considerations (Android APK limits, iOS constraints, keychain APIs), Security (JWT auth, TLS, credential storage), What Mobile Needs (HTTP client, WS client, WebView, local keychain), What Mobile Does NOT Need (SurrealDB, iroh, libp2p, Bevy), Timeline and Dependencies (v2 goal, depends on relay hardening).
- [ ] Task 4.2: Write the document at `docs/mobile-architecture.md`. Reference existing API endpoints from fe-api. Reference the SecretStore trait as the credential abstraction. Reference the relay binary as the deployment target for mobile backends.
- [ ] Task 4.3: Review document for accuracy against current codebase state.
- [ ] Verification: Document exists at `docs/mobile-architecture.md`. Content is accurate and actionable. Commit with message `docs: mobile architecture strategy — thin-client relay approach`. [checkpoint marker]

## Summary Checklist

| Phase | Key Deliverable | Acceptance |
|-------|----------------|------------|
| 1 | Baseline measurements | Sizes recorded |
| 2 | Tokio feature pruning | `["full"]` replaced, tests pass |
| 3 | Bevy plugin slimming | Audio + gamepad excluded, tests pass |
| 4 | Mobile architecture doc | `docs/mobile-architecture.md` written |

## Rollback Plan

All changes are in Cargo.toml and one documentation file. If any phase causes unexpected breakage, revert the Cargo.toml change. The `panic = "abort"` and LTO settings are already in place and unchanged by this track.
