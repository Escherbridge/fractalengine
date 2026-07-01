# Project Tracks

This file tracks all major tracks for the project. Each track has its own detailed plan in its respective folder.

---

## Wave 1: Core Infrastructure (Foundation)

## [x] Track: Seed Runtime — Three-Thread Topology and Channel Skeleton

_Link: [./tracks/seed_runtime_20260321/](./tracks/seed_runtime_20260321/)_

## [x] Track: Root Identity — Ed25519 Keypair, OS Keychain, JWT + did:key

_Link: [./tracks/root_identity_20260321/](./tracks/root_identity_20260321/)_

## [x] Track: Petal Soil — SurrealDB Schema, RBAC Permissions, Op-Log

_Link: [./tracks/petal_soil_20260321/](./tracks/petal_soil_20260321/)_

## [x] Track: Mycelium Network — libp2p DHT + iroh Data Transport

_Link: [./tracks/mycelium_network_20260321/](./tracks/mycelium_network_20260321/)_

## [x] Track: Bloom Renderer — GLTF Asset Pipeline, Content Addressing, Dead-Reckoning

_Link: [./tracks/bloom_renderer_20260321/](./tracks/bloom_renderer_20260321/)_

## [x] Track: Petal Gate — Auth Handshake, Session Cache, Role Enforcement

_Link: [./tracks/petal_gate_20260321/](./tracks/petal_gate_20260321/)_

## [x] Track: Canopy View — wry WebView Overlay + BrowserInteraction Tabs

_Link: [./tracks/canopy_view_20260321/](./tracks/canopy_view_20260321/)_

## [x] Track: Fractal Mesh — Multi-Node Sync, Petal Replication, Offline Cache

_Link: [./tracks/fractal_mesh_20260321/](./tracks/fractal_mesh_20260321/)_

## [x] Track: Gardener Console — Node Operator Admin UI

_Link: [./tracks/gardener_console_20260321/](./tracks/gardener_console_20260321/)_

## [ ] Track: Thorns and Shields — Security Hardening + Pre-Launch Documents

_Link: [./tracks/thorns_shields_20260321/](./tracks/thorns_shields_20260321/)_

---

## Chore / Exploration Tracks

- [x] `db_repository_pattern_20260407/` — Repository pattern for DB access
- [x] `glb_stability_20260405/` — GLB loading stability fixes
- [x] `mycelium_scaling_20260407/` — Mycelium network scaling research
- [x] `p2p_mycelium_20260405/` — P2P mycelium initial implementation
- [x] `relay_data_horizon_20260407/` — Relay-based data horizon strategy
- [x] `render_distance_lod_20260407/` — Render distance and LOD system

---

## Chores & Refactors

## [ ] Track: UI Manager Architecture Refactor — UiSet Ordering, UiAction Queue, ActiveDialog Enum, Selection Dedup

_Link: [./tracks/ui_manager_refactor_20260419/](./tracks/ui_manager_refactor_20260419/)_
_Scope: fe-ui internal refactor | Blocks: none_

## [ ] Track: Code Review Cleanup — SSRF Fix, Dead Code Removal, Stale Docs, Quality Fixes

_Link: [./tracks/code_review_cleanup_20260419/](./tracks/code_review_cleanup_20260419/)_
_Scope: fe-webview security fix (P0), fe-webview + fe-ui dead code and quality cleanup | Blocks: none_
_Priority: P0 (contains critical SSRF vulnerability fix)_

## [ ] Track: Build Size Optimization & Mobile Deployment Preparation

_Link: [./tracks/build_size_mobile_prep_20260508/](./tracks/build_size_mobile_prep_20260508/)_
_Scope: Tokio feature pruning, Bevy plugin slimming, mobile architecture strategy doc | Blocks: none_
_Priority: P1 (reduces 154 MB GUI / 106 MB relay binaries, documents mobile thin-client approach)_

---

## Wave 2: Interactive Digital Twin Platform

```
Dependency graph:

  Viewport Foundation ──┬──► Light Box
  (camera + grid)       │    (lighting rig)
                        │
                        ├──► Scene Graph Bridge ──┬──► Selection System ──► Transform Gizmos
                        │    (DB ↔ ECS sync)      │    (raycast + highlight)  (move/rotate/scale)
                        │                         │
                        └──► Drag & Drop ─────────┘
                             (file drop + placement)

  Petal Seed ──────► Bloom Stage ──────► Petal Portal
  (drag-drop)        (3D scene)          (browser overlay)

  Garden Console ──► Fractal Atlas
  (live admin UI)    (metadata/spaces)

  Mycelium Live      (independent)
  (peer discovery)

  Seedling Onboarding (independent — builds on Wave 1 infra)

  Hexon Format (6.5) ───────────┬──► fe-terrain (Phase 7) ──► fe-hexon (Phase 8)
  Entity Data Layer 6.1 (GIS) ─┤    (GPX + terrain +         (registry, P2P hosting,
  Viewport Foundation ──────────┘     map layers + IoT)        skybox/material/model hexons)
  Scene Graph Bridge ───────────┘

  Shared Peer Infra ──┬──► Inspector P1-P3  ──┐
  (NodeIdentity,       │   (tabs, hierarchy)   ├──► Coordinated P4
   PeerRegistry,       │                       │    (Access tab + P2P sync)
   presence)           └──► Profile P1-P3   ──┘
                            (display, edit, identity)
```

### 3D Editor Pipeline (new)

## [ ] Track: Viewport Foundation — 3D Camera, Infinite Ground Plane, and Bevy Scene Setup

_Link: [./tracks/viewport_foundation_20260402/](./tracks/viewport_foundation_20260402/)_
_Depends on: none | Blocks: Light Box, Scene Graph Bridge, Selection System, Transform Gizmos, Drag & Drop_

## [ ] Track: Light Box — Default Lighting Rig and Light Management System

_Link: [./tracks/light_box_20260402/](./tracks/light_box_20260402/)_
_Depends on: Viewport Foundation | Blocks: none_

## [ ] Track: Scene Graph Bridge — DB Entity ↔ Bevy ECS Synchronization

_Link: [./tracks/scene_graph_bridge_20260402/](./tracks/scene_graph_bridge_20260402/)_
_Depends on: Viewport Foundation | Blocks: Selection System, Drag & Drop_

## [ ] Track: Selection System — Raycasting, Highlighting, and Inspector Sync

_Link: [./tracks/selection_system_20260402/](./tracks/selection_system_20260402/)_
_Depends on: Viewport Foundation, Scene Graph Bridge | Blocks: Transform Gizmos_

## [ ] Track: Transform Gizmos — Blender-Style Move/Rotate/Scale Handles

_Link: [./tracks/transform_gizmos_20260402/](./tracks/transform_gizmos_20260402/)_
_Depends on: Selection System | Blocks: none_

## [ ] Track: Drag & Drop Asset Placement — File Drop + Scene Placement Flow

_Link: [./tracks/drag_drop_placement_20260402/](./tracks/drag_drop_placement_20260402/)_
_Depends on: Viewport Foundation, Scene Graph Bridge | Blocks: none_

### Shared Infrastructure

## [ ] Track: Shared Peer Infrastructure — NodeIdentity, PeerRegistry, Peer Presence, Canonical DID Format

_Link: [./tracks/shared_peer_infra_20260419/](./tracks/shared_peer_infra_20260419/)_
_Depends on: Root Identity (complete), Petal Gate (complete) | Blocks: Inspector Settings P4, Profile Manager P4_
_Scope: Resolves 3 BLOCKERs and 5 design decisions from cross-track alignment analysis_
_Priority: P0 (unblocks both Inspector Settings and Profile Manager Phase 4 integration)_

### UI & Configuration

## [ ] Track: Inspector Settings — Portal URL Persistence, Inspector Tabs, Hierarchy Inspection, Auth Settings UI

_Link: [./tracks/inspector_settings_20260419/](./tracks/inspector_settings_20260419/)_
_Depends on: Gardener Console (complete); Shared Peer Infrastructure (Phase 4 only) | Blocks: none_
_Scope: fe-ui inspector expansion, SurrealDB URL persistence, RBAC UI_
_Note: P1-P3 independent of shared infra; P4 (Access tab) requires PeerRegistry + LocalUserRole_

## [ ] Track: User Profile Manager — Identity Display, Profile Editing, Identity Management, P2P Profile Sync

_Link: [./tracks/profile_manager_20260419/](./tracks/profile_manager_20260419/)_
_Depends on: Root Identity (complete); Shared Peer Infrastructure (Phase 4 only) | Blocks: none_
_Scope: fe-ui profile panel, fe-identity multi-identity support, iroh-gossip profile broadcast_
_Note: P1-P3 independent of shared infra; P4 (P2P sync + PeerProfileCache) requires PeerRegistry_

### Existing Wave 2 Tracks

## [ ] Track: Petal Seed — GLTF Drag-and-Drop & Asset Seeding

_Link: [./tracks/petal_seed_20260322/](./tracks/petal_seed_20260322/)_
_Depends on: none | Blocks: Bloom Stage_

## [ ] Track: Garden Console — Live Admin & Space Manager UI

_Link: [./tracks/garden_console_20260322/](./tracks/garden_console_20260322/)_
_Depends on: none | Blocks: Fractal Atlas_

## [ ] Track: Mycelium Live — Peer Discovery & Node Browsing

_Link: [./tracks/mycelium_live_20260322/](./tracks/mycelium_live_20260322/)_
_Depends on: none_

## [ ] Track: Bloom Stage — 3D Scene Rendering & Object Interaction

_Link: [./tracks/bloom_stage_20260322/](./tracks/bloom_stage_20260322/)_
_Depends on: Petal Seed | Blocks: Petal Portal_

## [ ] Track: Petal Portal — Digital Twin Browser Overlay & IoT Interaction

_Link: [./tracks/petal_portal_20260322/](./tracks/petal_portal_20260322/)_
_Depends on: Bloom Stage_

## [ ] Track: Fractal Atlas — Space Manager & Metadata System

_Link: [./tracks/fractal_atlas_20260322/](./tracks/fractal_atlas_20260322/)_
_Depends on: Garden Console_

## [ ] Track: Seedling Onboarding — Local/Peer Instance Bootstrap + Entity CRUD

_Link: [./tracks/seedling_onboarding_20260327/](./tracks/seedling_onboarding_20260327/)_
_Depends on: Wave 1 complete (Root Identity, Petal Soil, Petal Gate, Gardener Console, Mycelium Network)_

---

## Wave 3: External Access & IoT Platform

### Entity Data Layer (Phases 1-5 complete, Phase 6 in progress)

## [x] Track: Entity Data Layer — Hierarchy Optimization, HLC, Observability (Phase 1)
_Scope: N+1→4 query hierarchy loader, HLC clock upgrade, #[instrument] on all handlers_

## [x] Track: Entity Data Layer — Direct API DB Reads, Transform Oplog (Phase 2)
_Scope: Read-only SurrealKV connection for API, transform mutations through op_log_

## [x] Track: Entity Data Layer — Custom Properties, Petal Iroh Replication (Phase 3)
_Scope: Node custom properties CRUD, field_def schema, SceneChange::PropertyChanged, petal replication_

## [x] Track: Entity Data Layer — Query Endpoint, Scene Streaming (Phase 4)
_Scope: POST /api/v1/query (scope-guarded SurrealQL), scene snapshot + delta streaming over WS_

## [x] Track: Entity Data Layer — Format, Entity Store, Node Log (Phase 5)
_Scope: fe-format crate (ZIP export/import), fe-entity-store crate (papaya lock-free cache), node_log table (append-only), elevated query endpoint_
_Crates: fe-format, fe-entity-store_

## [ ] Track: Entity Data Layer — fe-query LINQ Builder, GraphQL, GIS Validation (Phase 6.1)
_Link: [.omc/plans/skill-chain-prompts.md — Phase 6.1]_
_Depends on: Phase 5 complete | Blocks: Phase 6.2 (DataFusion + peer compute)_
_Scope: fe-query crate with LINQ-style QueryBuilder (parameterized, type-safe), async-graphql schema, GIS coordinate validation, spatial query filters_
_Crates: fe-query (new)_
_Priority: P1 (eliminates raw SQL strings, enables GraphQL + digital twin GIS queries)_

## [ ] Track: Entity Data Layer — DataFusion + GeoParquet + Peer Compute (Phase 6.2)
_Depends on: Phase 6.1 | Blocks: Final Architecture Review_
_Scope: DataFusion execution engine, GeoParquet read/write, spatial UDFs, DuckDB compat layer, peer compute mesh, Arrow Flight endpoint_

## [ ] Track: Hexon Format — Universal .hexon Package, amp.SDK Addressing, Signed Manifests (Phase 6.5)
_Link: [.omc/plans/skill-chain-prompts.md — Phase 6.5]_
_Depends on: Phase 5 (fe-format exists) | Blocks: Phase 7 (terrain), Phase 8 (hexon registry)_
_Scope: Rewrite fe-format as Hexon v1.0.0 — HexonManifest (hexon_type, publisher_did, version, signature, tags, platforms, amp-compatible address), entries.json (AssetEntry with amp EntryKind mapping), license.json, .hexon extension, ed25519 signing, hexon_ref property type, 3-level address system (NodeID/AttrID/ItemID). Spec: docs/hexon-format-spec.md_
_Priority: P0 (foundational — all subsequent tracks depend on the universal format)_
_Interop: amp.SDK (Go), plan.3D (Unity) — shared format spec_

### Terrain, GPX & Crate Registry

## [ ] Track: Terrain & GPX — 3D Map Tiles, GPX Tracks, Elevation Mesh, Petal-Bound Terrain (Phase 7)

_Link: [./tracks/terrain_gpx_maps_20260508/](./tracks/terrain_gpx_maps_20260508/)_
_Depends on: Phase 6.5 (Hexon format), Phase 6.1 (fe-query GIS), Viewport Foundation, Scene Graph Bridge | Blocks: Hexon Registry (terrain hexon type), IoT Path Tracking_
_Scope: Unified fe-terrain — GPX 1.0/1.1 parsing, terrain tile fetching (XYZ/TMS), elevation mesh from DEM, satellite draping, petal-scoped terrain config, layer stack (GPX tracks, GeoJSON overlays, heatmaps), waypoint interaction, IoT path tracking, .hexon terrain/ directory integration_
_Crates: fe-terrain (new — consolidates GPX + terrain + map layers + IoT path tracking)_
_Priority: P1 (enables outdoor digital twin, gpx.studio-style 3D visualization, IoT route tracking)_
_Key deps: gpx 0.10, geojson 1.0, flat_projection 0.4, image 0.25, reqwest 0.12_

## [ ] Track: Hexon Registry — P2P Distribution, Multi-Format Assets, Marketplace (Phase 8)

_Link: [./tracks/crate_registry_20260508/](./tracks/crate_registry_20260508/)_
_Depends on: Phase 6.5 (Hexon format), Headless Relay, Fractal Mesh (P2P), Terrain & GPX (terrain hexon type) | Blocks: Community Marketplace_
_Scope: fe-hexon handles registry + distribution (format in fe-format). Local registry (SurrealDB), install/uninstall, multi-format asset handlers (GLB, HDR/EXR skyboxes, PBR materials, terrain tilesets, GPX collections, sounds), P2P distribution via DHT+iroh, paywall (ChaCha20-Poly1305 encrypted blobs), publisher DID identity_
_Crates: fe-hexon (new — registry, P2P distribution, asset handlers, publisher tools)_
_Priority: P1 (enables community content ecosystem — any peer/relay can host hexons for all verses)_
_Key deps: chacha20poly1305 (paid hexon encryption), blake3 1, ed25519-dalek 2.2_
_Interop: amp.SDK (Go), plan.3D (Unity) — shared Hexon format (docs/hexon-format-spec.md)_

### External Access

## [ ] Track: Realtime API Gateway — MCP + REST + WebSocket for External Access

_Link: [./tracks/realtime_api_mcp_20260427/](./tracks/realtime_api_mcp_20260427/)_
_Depends on: Wave 1 complete (Root Identity, Petal Soil, Petal Gate, Fractal Mesh) | Blocks: IoT Integration, AI Agent Framework, External SDK, SSO Federation_
_Scope: New fe-api crate — axum HTTP/WS server, rmcp MCP tools, ApiClaims auth, transform streaming_
_Priority: P1 (first-of-kind: no Rust 3D engine exposes MCP/REST APIs)_

## [ ] Track: SSO Federation — OIDC Provider Integration for External Authentication

_Link: [./tracks/sso_federation_20260429/](./tracks/sso_federation_20260429/)_
_Depends on: Realtime API Gateway (complete) | Blocks: none_
_Scope: OIDC token exchange endpoint, provider management, identity mapping — supports Okta, Authentik, Google, LinkedIn, Azure AD, Keycloak, and any custom OIDC provider_
_Priority: P2 (enables enterprise SSO integration for verse access)_

## [~] Track: Cross-Platform Desktop — Linux + macOS + Windows ARM64 GUI Builds

_Link: [./tracks/cross_platform_desktop_20260429/](./tracks/cross_platform_desktop_20260429/)_
_Depends on: none | Blocks: Release CI_
_Scope: Multi-target .cargo/config.toml, Linux/macOS compile verification, platform #[cfg] audit + tests, BUILDING.md_
_Priority: P1 (validates that GUI binary compiles on all desktop platforms)_

## [~] Track: Headless Relay — Build Split, SecretStore Trait, Thin Client Surface

_Link: [./tracks/headless_relay_20260429/](./tracks/headless_relay_20260429/)_
_Depends on: Realtime API Gateway (complete) | Blocks: Release CI, Web Client SDK, IoT Integration, Docker Deployment, Mobile Client_
_Scope: Separate headless binary crate, SecretStore trait (OS/env/file backends), feature-gated Bevy headless mode, scene graph streaming over WS, asset delivery endpoint, relay hardening_
_Priority: P1 (enables server deployment, thin clients, and all non-desktop access patterns)_

## [ ] Track: Release CI — Cross-Compilation Pipeline, Artifact Publishing, Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/)_
_Depends on: Cross-Platform Desktop, Headless Relay | Blocks: none_
_Scope: GitHub Actions PR check (3 OS), release workflow (8 targets), sccache, cargo-zigbuild for musl, macOS universal binary, Docker image to GHCR_
_Priority: P2 (CI validates what we claim about cross-platform support)_

---

## Code Review 2026-04-30 — Quality & Performance Fixes

Comprehensive code review findings from 2026-04-30. Six tracks addressing 18 issues across `fe-ui`, `fe-webview`, `fe-database`, and `fractalengine`.

## [ ] Track: Replace Deprecated egui `screen_rect()` API

_Link: [./tracks/code_review_20260430_egui_deprecation/](./tracks/code_review_20260430_egui_deprecation/)_
_Scope: `fe-ui` — 3 call sites | Blocks: next egui bump_
_Priority: HIGH (will break on next dependency update)_

## [ ] Track: Fix Silent Channel Send Error Swallowing

_Link: [./tracks/code_review_20260430_channel_errors/](./tracks/code_review_20260430_channel_errors/)_
_Scope: `fe-ui` — `navigation_manager.rs`, `verse_manager.rs`, `node_manager.rs` | Blocks: none_
_Priority: HIGH (hides DB/sync thread crashes)_

## [ ] Track: Refactor `apply_db_results` Mega-Function

_Link: [./tracks/code_review_20260430_mega_function/](./tracks/code_review_20260430_mega_function/)_
_Scope: `fe-ui` — `verse_manager.rs` ~450-line function → thin dispatcher | Blocks: none_
_Priority: MEDIUM (maintainability, testability)_

## [ ] Track: Fix Hot-Path Performance Regressions

_Link: [./tracks/code_review_20260430_performance_hotpaths/](./tracks/code_review_20260430_performance_hotpaths/)_
_Scope: `fe-ui` — O(n³) node lookup, per-frame Vec allocation, full tree traversal | Blocks: none_
_Priority: HIGH (frame-time regression at scale)_

## [ ] Track: Clippy Warnings, Code Quality, and Polish

_Link: [./tracks/code_review_20260430_clippy_quality/](./tracks/code_review_20260430_clippy_quality/)_
_Scope: `fe-ui`, `fe-webview`, `fe-database` — dead code, clippy lints, logging consistency, missing Debug | Blocks: none_
_Priority: MEDIUM (developer experience)_

## [ ] Track: Graceful Degradation on DB Init Failure

_Link: [./tracks/code_review_20260430_db_graceful/](./tracks/code_review_20260430_db_graceful/)_
_Scope: `fe-database` + `fractalengine` — replace `.expect("SurrealDB init")` with `Result` | Blocks: none_
_Priority: MEDIUM (production robustness)_

---

## Tauri WebView Migration (2026-06-30) — Browser-First Path

### Track 1: Tauri WebView Backend — Robust Tauri Browser for fe-webview

_Link: [./tracks/tauri_webview_backend_20260630/](./tracks/tauri_webview_backend_20260630/)_
_Depends on: none | Blocks: Tauri IPC/Asset Bridge (track 2), Tauri Backend Cutover (track 3)_
_Scope: fe-webview browser backend — add Tauri-powered webview to replace raw wry FFI. Bevy STAYS host, bevy_egui REMAINS leading UI. Tauri integrates via commands, not replaces._
_Priority: P0 (primary deliverable: robust webview)_

### Track 2: Tauri IPC/Asset Bridge — Shared Node Structure + egui-Led Event Bridge

_Link: [./tracks/tauri_ipc_asset_bridge_20260630/](./tracks/tauri_ipc_asset_bridge_20260630/)_
_Depends on: Tauri WebView Backend (track 1) | Blocks: Tauri Backend Cutover (track 3)_
_Scope: IPC via `#[tauri::command]` + JS `invoke()`, shared "node" data structure bridging Tauri↔Bevy, custom `asset://` protocol. egui LEADS — Tauri integrates via commands._
_Priority: P1 (interop backbone, the seam Pear will plug into)_

### Track 3: Tauri Backend Cutover — Make Tauri Default Browser, Retire Raw wry

_Link: [./tracks/tauri_backend_cutover_20260630/](./tracks/tauri_backend_cutover_20260630/)_
_Depends on: Tauri IPC/Asset Bridge (track 2) | Blocks: none_
_Scope: Make `backend-tauri` default for fe-webview (BROWSER backend only, NOT app shell). Update docs (AGENTS.md, BUILDING.md, tech-stack.md). Bevy remains host._
_Priority: P1 (completes browser migration)_

---

## SPIKE / Research Tracks

### Track 4: Tauri-Host Shell SPIKE — Exploratory: Full Shell Inversion

_Link: [./tracks/tauri_host_shell_spike_20260630/](./tracks/tauri_host_shell_spike_20260630/)_
_Depends on: none | Blocks: none (SPIKE, not on critical path)_
_Scope: SPIKE — time-boxed exploration of full architecture inversion: Tauri owns window/event-loop, Bevy renders into Tauri surface (REPLACES bevy_winit). Reference: sunxfancy/BevyTauriExample. Tests: input bridging for picking, bevy_egui compatibility with custom renderer, bevy 0.18 API reconciliation._
_Priority: P2 (exploratory, informs future decisions)_
_Status: SPIKE_

### Track 5: Pear Runtime P2P Layer SPIKE — Research: JS-Native P2P

_Link: [./tracks/pears_p2p_layer_spike_20260630/](./tracks/pears_p2p_layer_spike_20260630/)_
_Depends on: none | Blocks: none (SPIKE, research only)_
_Scope: SPIKE — research Pear Runtime (pears.com, Holepunch) for P2P layer. Core tension: existing mycelium is Rust-native (libp2p + iroh); Pear is JS-native (Hypercore/Hyperswarm). Would run inside Tauri webview, bridge via shared node structure (track 2). Options: augment mycelium, replace, or hybrid._
_Priority: P2 (research, relates to tracks 1 & 2)_
_Status: SPIKE_

