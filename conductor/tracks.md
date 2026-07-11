---
type: Track Index
title: FractalEngine Project Tracks
timestamp: 2026-07-10T00:00:00Z
---

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
_verified: genuinely open — `docs/webview-threat-model.md`, `docs/security-checklist.md`, `docs/unwrap-audit.md`, `scripts/audit.sh`, `fuzz/targets/{ed25519_verify,jwt_parse}.rs` are all thin Wave-1 scaffolds (8-21 lines); `unwrap-audit.md` still says "Status: PENDING — run scripts/audit.sh after Wave 6" and cites a `.expect("SurrealDB init")` that db_graceful (2026-04-30) already removed — audit was never actually run/updated_

---

## Chore / Exploration Tracks

- [x] `db_repository_pattern_20260407/` — Repository pattern for DB access
- [x] `glb_stability_20260405/` — GLB loading stability fixes
- [x] `mycelium_scaling_20260407/` — Mycelium network scaling research
- [x] `p2p_mycelium_20260405/` — P2P mycelium initial implementation
- [ ] `p2p_mycelium_completion_20260701/` — P2P mycelium completion (Real iroh-docs Engine + Gossip Router integration)
  _verified: genuinely open — `fe-sync/src/replicator.rs` doc comment: "Whether a real iroh-docs Engine is available. Currently always `false`"; two `TODO(iroh-0.35)` markers remain for routing through the real Doc. Phases 3-5 (transform sync, gossip topics, tileset P2P) have code + tests in `fe-sync/src/sync_thread.rs`, but Phase 1 (real docs Engine) is still stubbed despite metadata.json marking it "completed"_
- [x] `relay_data_horizon_20260407/` — Relay-based data horizon strategy
- [x] `render_distance_lod_20260407/` — Render distance and LOD system
- [x] `code_review_retro_20260701/` — System-wide code review & retrospective session (spec.md + plan.md; not previously listed here). See `conductor/tracks/wave_retros_20260710/retro.md` for the consolidated follow-on retro.
- [x] `wave_retros_20260710/` — Consolidated Wave 1-3 + Tauri migration retrospective (this reconciliation pass's findings)

---

## Chores & Refactors

## [x] Track: UI Manager Architecture Refactor — UiSet Ordering, UiAction Queue, ActiveDialog Enum, Selection Dedup

_Link: [./tracks/ui_manager_refactor_20260419/](./tracks/ui_manager_refactor_20260419/)_
_Scope: fe-ui internal refactor | Blocks: none_
_verified: `UiSet` enum (fe-ui/src/plugin.rs:723), `UiAction` enum (plugin.rs:13) + `UiManager::push_action`/`drain_actions`, `ActiveDialog` enum (plugin.rs:168), `InspectorFormState` (renamed from `InspectorState`, no `selected_entity`/`selected_node_id` fields — `NodeManager::selected_entity()` is sole source) — FR-1 through FR-4 all present_

## [ ] Track: Code Review Cleanup — SSRF Fix, Dead Code Removal, Stale Docs, Quality Fixes

_Link: [./tracks/code_review_cleanup_20260419/](./tracks/code_review_cleanup_20260419/)_
_Scope: fe-webview security fix (P0), fe-webview + fe-ui dead code and quality cleanup | Blocks: none_
_Priority: P0 (contains critical SSRF vulnerability fix)_
_verified: partial — FR-1 (SSRF fix) done: `is_url_allowed()` wired into `navigation_handler` in both `fe-webview/src/plugin.rs:203` and `fe-webview/src/backends/tauri.rs:54`. FR-2 (dead guard/flush code) done: `tab_switch_guard_system`/`flush_browser_commands_system`/`PendingBrowserCommands` confirmed removed (comment at petal_portal.rs:328). FR-3–FR-9 (stale docs, VerseManager UiSet ordering, URL persistence, hostname caching, context-menu close detection, sidebar toggle, dead `tag_filter_buf`) not verified — track stays open_

## [ ] Track: Build Size Optimization & Mobile Deployment Preparation

_Link: [./tracks/build_size_mobile_prep_20260508/](./tracks/build_size_mobile_prep_20260508/)_
_Scope: Tokio feature pruning, Bevy plugin slimming, mobile architecture strategy doc | Blocks: none_
_Priority: P1 (reduces 154 MB GUI / 106 MB relay binaries, documents mobile thin-client approach)_
_verified: genuinely open — `docs/mobile-architecture.md` exists (thin-client relay strategy documented), but metadata.json status is still "pending" and no Tokio-feature-pruning or Bevy-plugin-slimming evidence found in Cargo.toml files_

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

## [x] Track: Viewport Foundation — 3D Camera, Infinite Ground Plane, and Bevy Scene Setup

_Link: [./tracks/viewport_foundation_20260402/](./tracks/viewport_foundation_20260402/)_
_Depends on: none | Blocks: Light Box, Scene Graph Bridge, Selection System, Transform Gizmos, Drag & Drop_
_verified: fe-renderer/src/camera.rs (orbit camera), grid.rs (ground grid), axis_gizmo.rs (axis gizmo) all exist_

## [ ] Track: Light Box — Default Lighting Rig and Light Management System

_Link: [./tracks/light_box_20260402/](./tracks/light_box_20260402/)_
_Depends on: Viewport Foundation | Blocks: none_
_verified: genuinely open — no `DirectionalLight`/`PointLight`/`AmbientLight`/lighting-rig code found anywhere in the workspace_

## [x] Track: Scene Graph Bridge — DB Entity ↔ Bevy ECS Synchronization

_Link: [./tracks/scene_graph_bridge_20260402/](./tracks/scene_graph_bridge_20260402/)_
_Depends on: Viewport Foundation | Blocks: Selection System, Drag & Drop_
_verified: `SceneChange` type wired through fe-sdk/src/scene.rs, fe-runtime/src/messages.rs, and consumed in fe-ui node_manager.rs/plugin.rs_

## [x] Track: Selection System — Raycasting, Highlighting, and Inspector Sync

_Link: [./tracks/selection_system_20260402/](./tracks/selection_system_20260402/)_
_Depends on: Viewport Foundation, Scene Graph Bridge | Blocks: Transform Gizmos_
_verified: `handle_viewport_click` raycast pick/deselect system in fe-ui/src/node_manager.rs:553, chained with `sync_manager_to_inspector`_

## [x] Track: Transform Gizmos — Blender-Style Move/Rotate/Scale Handles

_Link: [./tracks/transform_gizmos_20260402/](./tracks/transform_gizmos_20260402/)_
_Depends on: Selection System | Blocks: none_
_verified: fe-ui/src/gimbal.rs implements `Tool::Move`/`Tool::Rotate`/`Tool::Scale` handle rendering + drag_

## [ ] Track: Drag & Drop Asset Placement — File Drop + Scene Placement Flow

_Link: [./tracks/drag_drop_placement_20260402/](./tracks/drag_drop_placement_20260402/)_
_Depends on: Viewport Foundation, Scene Graph Bridge | Blocks: none_
_verified: genuinely open — no `FileDragAndDrop` event handling found anywhere in the workspace; GLTF import instead ships via a manual file-path text field (`ActiveDialog::GltfImport` in fe-ui/src/dialogs.rs), not OS drag-drop + placement-preview + Alt-Drag duplication + Asset Library panel as specced_

### Shared Infrastructure

## [x] Track: Shared Peer Infrastructure — NodeIdentity, PeerRegistry, Peer Presence, Canonical DID Format

_Link: [./tracks/shared_peer_infra_20260419/](./tracks/shared_peer_infra_20260419/)_
_Depends on: Root Identity (complete), Petal Gate (complete) | Blocks: Inspector Settings P4, Profile Manager P4_
_Scope: Resolves 3 BLOCKERs and 5 design decisions from cross-track alignment analysis_
_Priority: P0 (unblocks both Inspector Settings and Profile Manager Phase 4 integration)_
_verified: `PeerRegistry` in fe-runtime/src/peer_registry.rs, `NodeIdentity` in fe-identity/src/resource.rs (note: MEMORY.md records a pre-existing `peer_registry` dead_code warning in fe-ui — infra shipped but not fully consumed everywhere)_

### UI & Configuration

## [ ] Track: Inspector Settings — Portal URL Persistence, Inspector Tabs, Hierarchy Inspection, Auth Settings UI

_Link: [./tracks/inspector_settings_20260419/](./tracks/inspector_settings_20260419/)_
_Depends on: Gardener Console (complete); Shared Peer Infrastructure (Phase 4 only) | Blocks: none_
_Scope: fe-ui inspector expansion, SurrealDB URL persistence, RBAC UI_
_Note: P1-P3 independent of shared infra; P4 (Access tab) requires PeerRegistry + LocalUserRole_
_verified: genuinely open — inspector's `InspectorTab` enum (fe-ui/src/plugin.rs:379) shipped as `{Properties, ApiAccess, Query}`, not the specced `{Info, Settings, Access}` tabs with per-hierarchy-level (Node/Petal/Fractal/Verse) inspection; `RoleManager`/`RoleLevel` exist in fe-database but no Access-tab RBAC UI found_

## [ ] Track: User Profile Manager — Identity Display, Profile Editing, Identity Management, P2P Profile Sync

_Link: [./tracks/profile_manager_20260419/](./tracks/profile_manager_20260419/)_
_Depends on: Root Identity (complete); Shared Peer Infrastructure (Phase 4 only) | Blocks: none_
_Scope: fe-ui profile panel, fe-identity multi-identity support, iroh-gossip profile broadcast_
_Note: P1-P3 independent of shared infra; P4 (P2P sync + PeerProfileCache) requires PeerRegistry_
_verified: genuinely open — no `ProfilePanel`/`PeerProfileCache`/`UserProfile` implementation found anywhere_

### Existing Wave 2 Tracks

## [ ] Track: Petal Seed — GLTF Drag-and-Drop & Asset Seeding

_Link: [./tracks/petal_seed_20260322/](./tracks/petal_seed_20260322/)_
_Depends on: none | Blocks: Bloom Stage_
_verified: genuinely open — no `AssetRegistry`/asset-browser-panel found; ingestion pipeline exists (fe-renderer/src/ingester.rs) but the drag-drop UI + asset library flow specced here was not built (superseded by manual `GltfImport` dialog)_

## [x] Track: Garden Console — Live Admin & Space Manager UI

_Link: [./tracks/garden_console_20260322/](./tracks/garden_console_20260322/)_
_Depends on: none | Blocks: Fractal Atlas_
_verified: fe-database/src/space_manager.rs, handlers/rbac.rs, list_petals/list_rooms queries wired to live fe-ui panels_

## [x] Track: Mycelium Live — Peer Discovery & Node Browsing

_Link: [./tracks/mycelium_live_20260322/](./tracks/mycelium_live_20260322/)_
_Depends on: none_
_verified: Kademlia DHT in fe-network/src/swarm.rs + discovery.rs_

## [x] Track: Bloom Stage — 3D Scene Rendering & Object Interaction

_Link: [./tracks/bloom_stage_20260322/](./tracks/bloom_stage_20260322/)_
_Depends on: Petal Seed | Blocks: Petal Portal_
_verified: `GroundPlane` + orbit camera (fe-renderer/src/camera.rs) + raycast selection (fe-ui/src/node_manager.rs) all present_

## [x] Track: Petal Portal — Digital Twin Browser Overlay & IoT Interaction

_Link: [./tracks/petal_portal_20260322/](./tracks/petal_portal_20260322/)_
_Depends on: Bloom Stage_
_verified: fe-webview/src/petal_portal.rs — `TabVisibilityFilter`, `BrowserTab`, role-gated tab visibility_

## [x] Track: Fractal Atlas — Space Manager & Metadata System

_Link: [./tracks/fractal_atlas_20260322/](./tracks/fractal_atlas_20260322/)_
_Depends on: Garden Console_
_verified: fe-ui/src/atlas/{dashboard,model_editor,petal_wizard,room_editor,search_bar,tag_panel,visibility_control}.rs all present_

## [ ] Track: Seedling Onboarding — Local/Peer Instance Bootstrap + Entity CRUD

_Link: [./tracks/seedling_onboarding_20260327/](./tracks/seedling_onboarding_20260327/)_
_Depends on: Wave 1 complete (Root Identity, Petal Soil, Petal Gate, Gardener Console, Mycelium Network)_
_verified: genuinely open — no first-launch onboarding wizard or entity CRUD dialogs beyond `seed_default_data()` found; only generic libp2p "bootstrap" networking terminology present_

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

## [x] Track: Entity Data Layer — fe-query LINQ Builder, GraphQL, GIS Validation (Phase 6.1)
_Link: [.omc/plans/skill-chain-prompts.md — Phase 6.1]_
_Depends on: Phase 5 complete | Blocks: Phase 6.2 (DataFusion + peer compute)_
_Scope: fe-query crate with LINQ-style QueryBuilder (parameterized, type-safe), async-graphql schema, GIS coordinate validation, spatial query filters_
_Crates: fe-query (new)_
_Priority: P1 (eliminates raw SQL strings, enables GraphQL + digital twin GIS queries)_
_verified: fe-query crate exists with builder/, graphql/, geo/, duckdb_compat/, columnar/ modules; wired into fe-ui inspector's `InspectorTab::Query` tab_

## [ ] Track: Entity Data Layer — DataFusion + GeoParquet + Peer Compute (Phase 6.2)
_Depends on: Phase 6.1 | Blocks: Final Architecture Review_
_Scope: DataFusion execution engine, GeoParquet read/write, spatial UDFs, DuckDB compat layer, peer compute mesh, Arrow Flight endpoint_
_verified: intentionally deferred per MEMORY.md ("Phase 6.2: SKIPPED — DataFusion + GeoParquet (intentionally deferred)"); no DataFusion/GeoParquet/Arrow Flight code found — correctly left open_

## [x] Track: Hexon Format — Universal .hexon Package, amp.SDK Addressing, Signed Manifests (Phase 6.5)
_Link: [.omc/plans/skill-chain-prompts.md — Phase 6.5]_
_Depends on: Phase 5 (fe-format exists) | Blocks: Phase 7 (terrain), Phase 8 (hexon registry)_
_Scope: Rewrite fe-format as Hexon v1.0.0 — HexonManifest (hexon_type, publisher_did, version, signature, tags, platforms, amp-compatible address), entries.json (AssetEntry with amp EntryKind mapping), license.json, .hexon extension, ed25519 signing, hexon_ref property type, 3-level address system (NodeID/AttrID/ItemID). Spec: docs/hexon-format-spec.md_
_Priority: P0 (foundational — all subsequent tracks depend on the universal format)_
_Interop: amp.SDK (Go), plan.3D (Unity) — shared format spec_
_verified: fe-format crate rewritten — manifest.rs, entries.rs, license.rs, signature.rs, archive.rs; docs/hexon-format-spec.md present_

### Terrain, GPX & Crate Registry

## [x] Track: Terrain & GPX — 3D Map Tiles, GPX Tracks, Elevation Mesh, Petal-Bound Terrain (Phase 7)

_Link: [./tracks/terrain_gpx_maps_20260508/](./tracks/terrain_gpx_maps_20260508/)_
_Depends on: Phase 6.5 (Hexon format), Phase 6.1 (fe-query GIS), Viewport Foundation, Scene Graph Bridge | Blocks: Hexon Registry (terrain hexon type), IoT Path Tracking_
_Scope: Unified fe-terrain — GPX 1.0/1.1 parsing, terrain tile fetching (XYZ/TMS), elevation mesh from DEM, satellite draping, petal-scoped terrain config, layer stack (GPX tracks, GeoJSON overlays, heatmaps), waypoint interaction, IoT path tracking, .hexon terrain/ directory integration_
_Crates: fe-terrain (new — consolidates GPX + terrain + map layers + IoT path tracking)_
_Priority: P1 (enables outdoor digital twin, gpx.studio-style 3D visualization, IoT route tracking)_
_Key deps: gpx 0.10, geojson 1.0, flat_projection 0.4, image 0.25, reqwest 0.12_
_verified: fe-terrain crate complete per MEMORY.md (Phase 7.1-7.4, 65+ tests); fe-api/src/{gpx,terrain}.rs expose it over the API gateway; §petal-map flow documented in AGENTS.md_

## [x] Track: Hexon Registry — P2P Distribution, Multi-Format Assets, Marketplace (Phase 8)

_Link: [./tracks/crate_registry_20260508/](./tracks/crate_registry_20260508/)_
_Depends on: Phase 6.5 (Hexon format), Headless Relay, Fractal Mesh (P2P), Terrain & GPX (terrain hexon type) | Blocks: Community Marketplace_
_Scope: fe-hexon handles registry + distribution (format in fe-format). Local registry (SurrealDB), install/uninstall, multi-format asset handlers (GLB, HDR/EXR skyboxes, PBR materials, terrain tilesets, GPX collections, sounds), P2P distribution via DHT+iroh, paywall (ChaCha20-Poly1305 encrypted blobs), publisher DID identity_
_Crates: fe-hexon (new — registry, P2P distribution, asset handlers, publisher tools)_
_Priority: P1 (enables community content ecosystem — any peer/relay can host hexons for all verses)_
_Key deps: chacha20poly1305 (paid hexon encryption), blake3 1, ed25519-dalek 2.2_
_Interop: amp.SDK (Go), plan.3D (Unity) — shared Hexon format (docs/hexon-format-spec.md)_
_verified: fe-hexon + fe-hexon-registry crates exist, per MEMORY.md Phase 8 COMPLETE (review 5/7 PASS). Known gaps (unresolved): terrain crate auto-config not wired, RBAC not enforced in fe-hexon registry handlers (tracked below under auth_policy_pattern_20260710)_

### Plugin System (Phase 9)

> Track folders existed on disk (spec.md + plan.md) but were never registered in this file. Added here with the rest of the reconciliation pass — 2026-07-10.

## [x] Track: Plugin Host — Rhai + Wasmtime Extension Runtime (Phase 9A)

_Link: [./tracks/plugin_host_20260509/](./tracks/plugin_host_20260509/)_
_Depends on: Hexon Format (Phase 6.5) | Blocks: Extension SDK UI (Phase 9B)_
_Scope: fe-plugin crate — `FractalExtension` trait, `PluginRegistry`, `PluginTransaction`, `CapabilityManifest`, Rhai sandboxed engine (eval/import disabled, 1M op limit), Wasmtime engine (pooling, fuel metering, AOT cache), install/activate/deactivate/uninstall lifecycle with signature verification_
_Crates: fe-plugin (new)_
_verified: fe-plugin crate exists — capability.rs, context.rs, lifecycle.rs, registry.rs, transaction.rs, rhai/, wasm/; per MEMORY.md Phase 9A COMPLETE, 49 tests, QA 6/6 PASS_

## [x] Track: Extension SDK + UI Slots (Phase 9B)

_Link: [./tracks/extension_sdk_ui_20260509/](./tracks/extension_sdk_ui_20260509/)_
_Depends on: Plugin Host (Phase 9A) | Blocks: Plugin Testing DX (Phase 9C)_
_Scope: fe-sdk crate — stable serde-only API, `NodeSnapshot`, `PropertyValue`, `SceneChange`, `UiExtensionRegistry` (6 slots), `ApiExtensionHandle`; WIT interface (`fractalengine:plugin@1.0.0`); fe-terrain first-party extension with 3 UI contributions_
_Crates: fe-sdk (new)_
_verified: fe-sdk crate exists — api.rs, context.rs, events.rs, node.rs, property.rs, scene.rs, transaction.rs, ui/; fe-plugin/wit/hexon-plugin.wit present; per MEMORY.md Phase 9B COMPLETE, 11+16 tests, review 7/7 PASS_

## [x] Track: Plugin Testing DX — Mock Host, Fixtures, Rhai Test Runner (Phase 9C)

_Link: [./tracks/plugin_testing_dx_20260509/](./tracks/plugin_testing_dx_20260509/)_
_Depends on: Extension SDK UI (Phase 9B) | Blocks: none_
_Scope: fe-plugin-test crate — `MockHostEnv`, `SpyRecorder`, 3 fixtures (empty/terrain/sensor), assertion helpers, `RhaiTestRunner`_
_Crates: fe-plugin-test (new)_
_verified: fe-plugin-test crate exists — assertions.rs, fixtures.rs, mock_host.rs, rhai_runner.rs, spy.rs; per MEMORY.md Phase 9C COMPLETE, 28 tests, QA 6/6 PASS, final review 7/7 PASS (89 plugin-system tests total)_
_Known gap: fe-plugin does not yet depend on fe-sdk (parallel type definitions) — tracked as a prerequisite for analytics_extension_api_20260710_

### External Access

## [x] Track: Realtime API Gateway — MCP + REST + WebSocket for External Access

_Link: [./tracks/realtime_api_mcp_20260427/](./tracks/realtime_api_mcp_20260427/)_
_Depends on: Wave 1 complete (Root Identity, Petal Soil, Petal Gate, Fractal Mesh) | Blocks: IoT Integration, AI Agent Framework, External SDK, SSO Federation_
_Scope: New fe-api crate — axum HTTP/WS server, rmcp MCP tools, ApiClaims auth, transform streaming_
_Priority: P1 (first-of-kind: no Rust 3D engine exposes MCP/REST APIs)_
_verified: fe-api crate — rest.rs, ws.rs, mcp.rs (hand-rolled JSON-RPC 2.0 request/response types rather than the `rmcp` crate as specced, but MCP+REST+WS all delivered), auth.rs, server.rs_

## [ ] Track: SSO Federation — OIDC Provider Integration for External Authentication

_Link: [./tracks/sso_federation_20260429/](./tracks/sso_federation_20260429/)_
_Depends on: Realtime API Gateway (complete) | Blocks: none_
_Scope: OIDC token exchange endpoint, provider management, identity mapping — supports Okta, Authentik, Google, LinkedIn, Azure AD, Keycloak, and any custom OIDC provider_
_Priority: P2 (enables enterprise SSO integration for verse access)_
_verified: genuinely open — no OIDC/SSO code found anywhere in the workspace_

## [~] Track: Cross-Platform Desktop — Linux + macOS + Windows ARM64 GUI Builds

_Link: [./tracks/cross_platform_desktop_20260429/](./tracks/cross_platform_desktop_20260429/)_
_Depends on: none | Blocks: Release CI_
_Scope: Multi-target .cargo/config.toml, Linux/macOS compile verification, platform #[cfg] audit + tests, BUILDING.md_
_Priority: P1 (validates that GUI binary compiles on all desktop platforms)_
_Status: Phase 1-2 complete; Phase 3 in progress (needs cross-platform compile verification)_

## [~] Track: Headless Relay — Build Split, SecretStore Trait, Thin Client Surface

_Link: [./tracks/headless_relay_20260429/](./tracks/headless_relay_20260429/)_
_Depends on: Realtime API Gateway (complete) | Blocks: Release CI, Web Client SDK, IoT Integration, Docker Deployment, Mobile Client_
_Scope: Separate headless binary crate, SecretStore trait (OS/env/file backends), feature-gated Bevy headless mode, scene graph streaming over WS, asset delivery endpoint, relay hardening_
_Priority: P1 (enables server deployment, thin clients, and all non-desktop access patterns)_
_Status: Phase 1-2 complete; Phase 3-4 in progress (entity change broadcast & scene subscription handlers pending)_

## [ ] Track: Release CI — Cross-Compilation Pipeline, Artifact Publishing, Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/)_
_Depends on: Cross-Platform Desktop, Headless Relay | Blocks: none_
_Scope: GitHub Actions PR check (3 OS), release workflow (8 targets), sccache, cargo-zigbuild for musl, macOS universal binary, Docker image to GHCR_
_Priority: P2 (CI validates what we claim about cross-platform support)_
_verified: genuinely open — no `.github/` directory found in the workspace_

---

## Code Review 2026-04-30 — Quality & Performance Fixes

Comprehensive code review findings from 2026-04-30. Six tracks addressing 18 issues across `fe-ui`, `fe-webview`, `fe-database`, and `fractalengine`.

## [x] Track: Replace Deprecated egui `screen_rect()` API

_Link: [./tracks/code_review_20260430_egui_deprecation/](./tracks/code_review_20260430_egui_deprecation/)_
_Scope: `fe-ui` — 3 call sites | Blocks: next egui bump_
_Priority: HIGH (will break on next dependency update)_
_verified: no `screen_rect()` calls remain anywhere in the workspace_

## [x] Track: Fix Silent Channel Send Error Swallowing

_Link: [./tracks/code_review_20260430_channel_errors/](./tracks/code_review_20260430_channel_errors/)_
_Scope: `fe-ui` — `navigation_manager.rs`, `verse_manager.rs`, `node_manager.rs` | Blocks: none_
_Priority: HIGH (hides DB/sync thread crashes)_
_verified: named call sites in verse_manager.rs (Seeded/VerseJoined/DatabaseReset/revocation_tx/ListApiTokens) now use `.send(...).is_err()` + `bevy::log::error!(...)` instead of bare `.ok()`; navigation_manager.rs has no `.ok()` sends remaining_

## [ ] Track: Refactor `apply_db_results` Mega-Function

_Link: [./tracks/code_review_20260430_mega_function/](./tracks/code_review_20260430_mega_function/)_
_Scope: `fe-ui` — `verse_manager.rs` ~450-line function → thin dispatcher | Blocks: none_
_Priority: MEDIUM (maintainability, testability)_
_verified: genuinely open — `apply_db_results` in verse_manager.rs is still one ~430-line match block, unrefactored (see feui_decomposition_20260710 for the successor track)_

## [ ] Track: Fix Hot-Path Performance Regressions

_Link: [./tracks/code_review_20260430_performance_hotpaths/](./tracks/code_review_20260430_performance_hotpaths/)_
_Scope: `fe-ui` — O(n³) node lookup, per-frame Vec allocation, full tree traversal | Blocks: none_
_Priority: HIGH (frame-time regression at scale)_
_verified: genuinely open — `VerseManager::update_node_position`/`update_node_url` still walk the full verse/fractal/petal tree with nested loops (verse_manager.rs:103+), unchanged from the spec's "before" example_

## [ ] Track: Clippy Warnings, Code Quality, and Polish

_Link: [./tracks/code_review_20260430_clippy_quality/](./tracks/code_review_20260430_clippy_quality/)_
_Scope: `fe-ui`, `fe-webview`, `fe-database` — dead code, clippy lints, logging consistency, missing Debug | Blocks: none_
_Priority: MEDIUM (developer experience)_
_verified: genuinely open — MEMORY.md records a pre-existing `fe-ui` `peer_registry` dead_code warning and an unused import in `fe-terrain/store.rs` still outstanding_

## [x] Track: Graceful Degradation on DB Init Failure

_Link: [./tracks/code_review_20260430_db_graceful/](./tracks/code_review_20260430_db_graceful/)_
_Scope: `fe-database` + `fractalengine` — replace `.expect("SurrealDB init")` with `Result` | Blocks: none_
_Priority: MEDIUM (production robustness)_
_verified: `DbInitError` enum (fe-database/src/lib.rs) with variants for RuntimeBuild/SurrealOpen/SurrealNsDb/SchemaApply/ApiTokenSchemaApply replaces the old `.expect("SurrealDB init")`; no such `.expect()` call remains in the workspace_

---

## Tauri WebView Migration (2026-06-30) — Browser-First Path

### Track 1: Tauri WebView Backend — Robust Tauri Browser for fe-webview [x]

_Link: [./tracks/tauri_webview_backend_20260630/](./tracks/tauri_webview_backend_20260630/)_
_Depends on: none | Blocks: Tauri IPC/Asset Bridge (track 2), Tauri Backend Cutover (track 3)_
_Scope: fe-webview browser backend — add Tauri-powered webview to replace raw wry FFI. Bevy STAYS host, bevy_egui REMAINS leading UI. Tauri integrates via commands, not replaces._
_Priority: P0 (primary deliverable: robust webview)_
_verified: fe-webview/src/backends/tauri.rs implements the Tauri-powered backend_

### Track 2: Tauri IPC/Asset Bridge — Shared Node Structure + egui-Led Event Bridge [x]

_Link: [./tracks/tauri_ipc_asset_bridge_20260630/](./tracks/tauri_ipc_asset_bridge_20260630/)_
_Depends on: Tauri WebView Backend (track 1) | Blocks: Tauri Backend Cutover (track 3)_
_Scope: IPC via `#[tauri::command]` + JS `invoke()`, shared "node" data structure bridging Tauri↔Bevy, custom `asset://` protocol. egui LEADS — Tauri integrates via commands._
_Priority: P1 (interop backbone, the seam Pear will plug into)_
_verified: fe-webview/src/tauri_commands.rs implements the `#[tauri::command]` IPC surface_

### Track 3: Tauri Backend Cutover — Make Tauri Default Browser, Retire Raw wry [x]

_Link: [./tracks/tauri_backend_cutover_20260630/](./tracks/tauri_backend_cutover_20260630/)_
_Depends on: Tauri IPC/Asset Bridge (track 2) | Blocks: none_
_Scope: Make `backend-tauri` default for fe-webview (BROWSER backend only, NOT app shell). Update docs (AGENTS.md, BUILDING.md, tech-stack.md). Bevy remains host._
_Priority: P1 (completes browser migration)_
_verified: fe-webview/Cargo.toml has `default = ["backend-tauri"]`; AGENTS.md documents the cutover_

---

## SPIKE / Research Tracks

### Track 4: Tauri-Host Shell SPIKE — Exploratory: Full Shell Inversion

_Link: [./tracks/tauri_host_shell_spike_20260630/](./tracks/tauri_host_shell_spike_20260630/)_
_Depends on: none | Blocks: none (SPIKE, not on critical path)_
_Scope: SPIKE — time-boxed exploration of full architecture inversion: Tauri owns window/event-loop, Bevy renders into Tauri surface (REPLACES bevy_winit). Reference: sunxfancy/BevyTauriExample. Tests: input bridging for picking, bevy_egui compatibility with custom renderer, bevy 0.18 API reconciliation._
_Priority: P2 (exploratory, informs future decisions)_
_Status: SPIKE — verified: metadata.json status "pending", all 4 phases pending — not started_

### Track 5: Pear Runtime P2P Layer SPIKE — Research: JS-Native P2P

_Link: [./tracks/pears_p2p_layer_spike_20260630/](./tracks/pears_p2p_layer_spike_20260630/)_
_Depends on: none | Blocks: none (SPIKE, research only)_
_Scope: SPIKE — research Pear Runtime (pears.com, Holepunch) for P2P layer. Core tension: existing mycelium is Rust-native (libp2p + iroh); Pear is JS-native (Hypercore/Hyperswarm). Would run inside Tauri webview, bridge via shared node structure (track 2). Options: augment mycelium, replace, or hybrid._
_Priority: P2 (research, relates to tracks 1 & 2)_
_Status: SPIKE — verified: metadata.json status "completed" 2026-07-01, all 4 phases done, recommendation: hybrid_

---

## Wave: Analytics & Extension Platform

> Goal: 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting. Builds on Phase 9 (Plugin System) and Phase 6.1 (fe-query).

```
Dependency graph:

  Plugin Host (9A) ──┬──► Analytics Extension API ──┬──► IoT Extension Slice
  Extension SDK (9B) ┤    (storage+query for exts)   │    (proves the loop end-to-end)
  fe-query (6.1) ─────┘                              │
                                                      │
  Auth Policy Pattern (spec) ───────────────────────►┘ (capability gating depends on
                                                          the policy engine existing)

  feUI Decomposition (independent — successor to ui_manager_refactor_20260419 physical split)

  Hexon Delta Format (spec) ───┬─ depends conceptually on Hexon Format (6.5) + fe-sync P2P,
  Hexon P2P Bucket (spec)   ───┘  but both are design-only this round — no code dependency yet.
                                  Deltas = operations layer; Bucket = content layer (see
                                  hexon_p2p_bucket_20260710/spec.md "Relationship" section).
```

## [ ] Track: Analytics Extension API — Storage + Query API for Extensions

_Link: [./tracks/analytics_extension_api_20260710/](./tracks/analytics_extension_api_20260710/)_
_Depends on: Plugin Host (9A), Extension SDK UI (9B), fe-query (6.1) | Blocks: IoT Extension Slice_
_Scope: Unify fe-plugin/fe-sdk (fe-plugin depends on fe-sdk types instead of parallel definitions), capability-gated storage + query API surface exposed to extensions, WIT query-api addition_
_Status: in progress via ultrapilot_

## [ ] Track: IoT Extension Slice — Device ↔ Node Push/Pull Proof

_Link: [./tracks/iot_extension_slice_20260710/](./tracks/iot_extension_slice_20260710/)_
_Depends on: Analytics Extension API | Blocks: none_
_Scope: A real IoT bridge extension built on the Analytics Extension API, proving the push/pull device↔node loop end-to-end through the plugin host_
_Status: in progress via ultrapilot_

## [ ] Track: fe-ui Decomposition — God-File Breakup into Domain Modules

_Link: [./tracks/feui_decomposition_20260710/](./tracks/feui_decomposition_20260710/)_
_Depends on: none (successor to ui_manager_refactor_20260419's remaining physical-split work) | Blocks: none_
_Scope: Decompose fe-ui's largest files (panels.rs 1991 lines, plugin.rs 1397 lines, dialogs.rs 1183 lines, verse_manager.rs 914 lines, node_manager.rs 834 lines) into domain modules, soft-capped ~300 lines each_
_Status: in progress via ultrapilot_

## [ ] Track: Hexon Deltas — Replayable Op-Log Hexons over P2P (spec only)

_Link: [./tracks/hexon_delta_format_20260710/](./tracks/hexon_delta_format_20260710/)_
_Depends on: Hexon Format (6.5), fe-sync P2P | Blocks: none (design-only this round)_
_Scope: A delta-hexon manifest type over the existing op_log, enabling replay/materialization, time-travel checkpoints, sovereign-authored signature chains, compression, and content-addressed P2P distribution of op-log deltas_
_Status: spec-only via ultrapilot — no implementation this round_

## [ ] Track: Policy Engine — Unified Authorization for Layered Tokens and Entry Points (spec only)

_Link: [./tracks/auth_policy_pattern_20260710/](./tracks/auth_policy_pattern_20260710/)_
_Depends on: none (surveys fe-auth, fe-identity, fe-database RBAC, fe-api, fe-hexon, fe-plugin, fe-webview) | Blocks: none (design-only this round)_
_Scope: A central `Policy` abstraction (`evaluate(subject, action, resource) -> Decision`) replacing scattered ad-hoc role checks across every entry point; deny-by-default; closes the fe-hexon registry RBAC enforcement gap (Phase 8.4)_
_Status: spec-only via ultrapilot — no implementation this round_

## [ ] Track: Hexon P2P Bucket — 3D Visual IPFS (spec only)

_Link: [./tracks/hexon_p2p_bucket_20260710/](./tracks/hexon_p2p_bucket_20260710/)_
_Depends on: Hexon Registry (Phase 8), Hexon Deltas (spec, content-vs-operations counterpart) | Blocks: none (design-only this round)_
_Scope: Generalize node assets from GLTF-only to any file type or a directory of files (with a placeholder-rendering contract for content with no native 3D form); download/upload endpoints (relates to the concurrently in-progress `GET /nodes/{id}/asset`); distribute the resulting content-addressed, sovereign-authored bucket over iroh P2P — "3D visual IPFS." Confirmed gap: `fe-network/src/iroh_blobs.rs` is a 13-line unfilled Wave-1 stub — the P2P blob transport itself does not exist yet._
_Status: spec-only via ultrapilot — no implementation this round_

---

## Archived Tracks

**Convention (established 2026-07-10, no prior convention existed in PLAYBOOK.md/workflow.md):** Archiving here means "implementation complete, no longer active work" — it does **not** mean moved or deleted. Links above remain valid. A track is archived by (1) its checkbox being `[x]` above, and (2) its `metadata.json` carrying `"archived": true` + `"archived_at"`. This pass added the `archived` field only to the metadata.json files it created or repaired (the tracks newly flipped to `[x]` today); pre-existing `[x]` tracks from Waves 1-2 were left untouched (non-destructive — see notepad/report for the full list).

Tracks archived in this pass (2026-07-10 reconciliation): `ui_manager_refactor_20260419`, `viewport_foundation_20260402`, `scene_graph_bridge_20260402`, `selection_system_20260402`, `transform_gizmos_20260402`, `shared_peer_infra_20260419`, `garden_console_20260322`, `mycelium_live_20260322`, `bloom_stage_20260322`, `petal_portal_20260322`, `fractal_atlas_20260322`, Entity Data Layer Phase 6.1 (fe-query), Hexon Format Phase 6.5, Terrain & GPX Phase 7, Hexon Registry Phase 8, Realtime API Gateway, `tauri_webview_backend_20260630`, `tauri_ipc_asset_bridge_20260630`, `tauri_backend_cutover_20260630`, `code_review_20260430_egui_deprecation`, `code_review_20260430_channel_errors`, `code_review_20260430_db_graceful`, `plugin_host_20260509`, `extension_sdk_ui_20260509`, `plugin_testing_dx_20260509`.

