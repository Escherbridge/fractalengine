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

## [ ] Track: Headless Relay — Build Split, SecretStore Trait, Thin Client Surface

_Link: [./tracks/headless_relay_20260429/](./tracks/headless_relay_20260429/)_
_Depends on: Realtime API Gateway (complete) | Blocks: Release CI, Web Client SDK, IoT Integration, Docker Deployment, Mobile Client_
_Scope: Separate headless binary crate, SecretStore trait (OS/env/file backends), feature-gated Bevy headless mode, scene graph streaming over WS, asset delivery endpoint, relay hardening_
_Priority: P1 (enables server deployment, thin clients, and all non-desktop access patterns)_

## [ ] Track: Release CI — Cross-Compilation Pipeline, Artifact Publishing, Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/)_
_Depends on: Cross-Platform Desktop, Headless Relay | Blocks: none_
_Scope: GitHub Actions PR check (3 OS), release workflow (8 targets), sccache, cargo-zigbuild for musl, macOS universal binary, Docker image to GHCR_
_Priority: P2 (CI validates what we claim about cross-platform support)_
