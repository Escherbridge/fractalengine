---
type: Track Index
title: FractalEngine Project Tracks
timestamp: 2026-07-11T00:00:00Z
---

# Project Tracks

This file tracks all major tracks for the project. Each track has its own detailed plan in its respective folder.

> **Pruned + archived 2026-07-11 (hexon-p2p-commons pass):** completed `[x]` tracks are collapsed to
> one-liners AND their folders physically relocated into [`./tracks/_archive/`](./tracks/_archive/)
> (37 tracks) — full scope/verification detail lives in each moved folder; links above updated to
> `_archive/` paths. Open tracks keep their full entries and stay at `./tracks/<id>/`. Statuses
> reconciled against verified evidence from `research/hexon-p2p-commons/report.md`.
> **Execution order** for open tracks is encoded as an `execution_wave` field (0=in-flight/P0 →
> 5=release; `DEADLINE`=iroh 1.0 by 2026-12-31) plus `depends_on`/`blocks` edges in each
> `metadata.json` — query with `jq '.execution_wave' tracks/*/metadata.json`. Architecture
> decisions ratified this pass: [./decisions/hexon-p2p-commons-20260711.md](./decisions/hexon-p2p-commons-20260711.md).

---

## Wave 1: Core Infrastructure (Foundation)

Completed (collapsed — see folders):

- [x] Seed Runtime — three-thread topology + channel skeleton — [./tracks/_archive/seed_runtime_20260321/](./tracks/_archive/seed_runtime_20260321/)
- [x] Root Identity — ed25519 keypair, OS keychain, JWT + did:key — [./tracks/root_identity_20260321/](./tracks/root_identity_20260321/)
- [x] Petal Soil — SurrealDB schema, RBAC permissions, op-log — [./tracks/petal_soil_20260321/](./tracks/petal_soil_20260321/)
- [x] Mycelium Network — libp2p DHT + iroh data transport — [./tracks/mycelium_network_20260321/](./tracks/mycelium_network_20260321/)
- [x] Bloom Renderer — GLTF pipeline, content addressing, dead-reckoning — [./tracks/bloom_renderer_20260321/](./tracks/bloom_renderer_20260321/)
- [x] Petal Gate — auth handshake, session cache, role enforcement — [./tracks/petal_gate_20260321/](./tracks/petal_gate_20260321/)
- [x] Canopy View — wry WebView overlay + BrowserInteraction tabs — [./tracks/canopy_view_20260321/](./tracks/canopy_view_20260321/)
- [x] Fractal Mesh — multi-node sync, petal replication, offline cache — [./tracks/fractal_mesh_20260321/](./tracks/fractal_mesh_20260321/)
- [x] Gardener Console — node operator admin UI — [./tracks/gardener_console_20260321/](./tracks/gardener_console_20260321/)

## [ ] Track: Thorns and Shields — Security Hardening + Pre-Launch Documents

_Link: [./tracks/thorns_shields_20260321/](./tracks/thorns_shields_20260321/)_
_verified: genuinely open — `docs/webview-threat-model.md`, `docs/security-checklist.md`, `docs/unwrap-audit.md`, `scripts/audit.sh`, `fuzz/targets/{ed25519_verify,jwt_parse}.rs` are all thin Wave-1 scaffolds (8-21 lines); `unwrap-audit.md` still says "Status: PENDING — run scripts/audit.sh after Wave 6" and cites a `.expect("SurrealDB init")` that db_graceful (2026-04-30) already removed — audit was never actually run/updated_

---

## Chore / Exploration Tracks

- [x] `_archive/db_repository_pattern_20260407/` — Repository pattern for DB access
- [x] `_archive/glb_stability_20260405/` — GLB loading stability fixes
- [x] `_archive/mycelium_scaling_20260407/` — Mycelium network scaling research
- [x] `_archive/p2p_mycelium_20260405/` — P2P mycelium initial implementation
- [ ] `p2p_mycelium_completion_20260701/` — P2P mycelium completion (Real iroh-docs Engine + Gossip Router integration)
  _reconciled 2026-07-11 (hexon-p2p-commons report §8.1): phases 1-2 REOPENED in metadata.json — `IrohDocsEngineHolder::is_available()` hardcoded false, all replicators delegate to `MockVerseReplicator` (`fe-sync/src/replicator.rs:235-237,286-304`); gossip is send-only (broadcast at sync_thread.rs:538/678, no receive loop — added to Phase 3 scope). Prerequisites before wiring real docs: `p2p_unblock_now_20260711` FR-1 (try_send bridge) and the policy gate on the sync write path (see auth_policy_pattern amendment)_
- [x] `_archive/relay_data_horizon_20260407/` — Relay-based data horizon strategy
- [x] `_archive/render_distance_lod_20260407/` — Render distance and LOD system
- [x] `_archive/code_review_retro_20260701/` — System-wide code review & retrospective session. See `conductor/tracks/_archive/wave_retros_20260710/retro.md` for the consolidated follow-on retro.
- [x] `_archive/wave_retros_20260710/` — Consolidated Wave 1-3 + Tauri migration retrospective

---

## Chores & Refactors

- [x] UI Manager Architecture Refactor — UiSet ordering, UiAction queue, ActiveDialog enum — [./tracks/_archive/ui_manager_refactor_20260419/](./tracks/_archive/ui_manager_refactor_20260419/)

## [ ] Track: Code Review Cleanup — SSRF Fix, Dead Code Removal, Stale Docs, Quality Fixes

_Link: [./tracks/code_review_cleanup_20260419/](./tracks/code_review_cleanup_20260419/)_
_Scope: fe-webview security fix (P0), fe-webview + fe-ui dead code and quality cleanup | Blocks: none_
_Priority: P0 (contains critical SSRF vulnerability fix)_
_verified: partial — FR-1 (SSRF fix) done: `is_url_allowed()` wired into `navigation_handler` in both `fe-webview/src/plugin.rs:203` and `fe-webview/src/backends/tauri.rs:54`. FR-2 (dead guard/flush code) done. FR-3–FR-9 (stale docs, VerseManager UiSet ordering, URL persistence, hostname caching, context-menu close detection, sidebar toggle, dead `tag_filter_buf`) not verified — track stays open_

## [ ] Track: Build Size Optimization & Mobile Deployment Preparation

_Link: [./tracks/build_size_mobile_prep_20260508/](./tracks/build_size_mobile_prep_20260508/)_
_Scope: Tokio feature pruning, Bevy plugin slimming, mobile architecture strategy doc | Blocks: none_
_Priority: P1 (reduces 154 MB GUI / 106 MB relay binaries, documents mobile thin-client approach)_
_verified: genuinely open — `docs/mobile-architecture.md` exists (thin-client relay strategy documented), but metadata.json status is still "pending" and no Tokio-feature-pruning or Bevy-plugin-slimming evidence found in Cargo.toml files_

---

## Wave 2: Interactive Digital Twin Platform

Completed (collapsed — see folders):

- [x] Viewport Foundation — 3D camera, ground plane, Bevy scene setup — [./tracks/_archive/viewport_foundation_20260402/](./tracks/_archive/viewport_foundation_20260402/)
- [x] Scene Graph Bridge — DB entity ↔ Bevy ECS sync — [./tracks/_archive/scene_graph_bridge_20260402/](./tracks/_archive/scene_graph_bridge_20260402/)
- [x] Selection System — raycasting, highlighting, inspector sync — [./tracks/_archive/selection_system_20260402/](./tracks/_archive/selection_system_20260402/)
- [x] Transform Gizmos — move/rotate/scale handles — [./tracks/_archive/transform_gizmos_20260402/](./tracks/_archive/transform_gizmos_20260402/)
- [x] Shared Peer Infrastructure — NodeIdentity, PeerRegistry, presence, canonical DID — [./tracks/_archive/shared_peer_infra_20260419/](./tracks/_archive/shared_peer_infra_20260419/) _(note: fe-ui `peer_registry` dead_code warning — shipped but not fully consumed)_
- [x] Garden Console — live admin & space manager UI — [./tracks/_archive/garden_console_20260322/](./tracks/_archive/garden_console_20260322/)
- [x] Mycelium Live — peer discovery & node browsing — [./tracks/_archive/mycelium_live_20260322/](./tracks/_archive/mycelium_live_20260322/)
- [x] Bloom Stage — 3D scene rendering & object interaction — [./tracks/_archive/bloom_stage_20260322/](./tracks/_archive/bloom_stage_20260322/)
- [x] Petal Portal — digital twin browser overlay & IoT interaction — [./tracks/_archive/petal_portal_20260322/](./tracks/_archive/petal_portal_20260322/)
- [x] Fractal Atlas — space manager & metadata system — [./tracks/_archive/fractal_atlas_20260322/](./tracks/_archive/fractal_atlas_20260322/)

## [ ] Track: Light Box — Default Lighting Rig and Light Management System

_Link: [./tracks/light_box_20260402/](./tracks/light_box_20260402/)_
_Depends on: Viewport Foundation | Blocks: none_
_verified: genuinely open — no `DirectionalLight`/`PointLight`/`AmbientLight`/lighting-rig code found anywhere in the workspace_

## [ ] Track: Drag & Drop Asset Placement — File Drop + Scene Placement Flow

_Link: [./tracks/drag_drop_placement_20260402/](./tracks/drag_drop_placement_20260402/)_
_Depends on: Viewport Foundation, Scene Graph Bridge | Blocks: none_
_verified: genuinely open — no `FileDragAndDrop` event handling found anywhere in the workspace; GLTF import instead ships via a manual file-path text field (`ActiveDialog::GltfImport` in fe-ui/src/dialogs.rs), not OS drag-drop + placement-preview + Alt-Drag duplication + Asset Library panel as specced_

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

## [ ] Track: Petal Seed — GLTF Drag-and-Drop & Asset Seeding

_Link: [./tracks/petal_seed_20260322/](./tracks/petal_seed_20260322/)_
_Depends on: none | Blocks: Bloom Stage (shipped without it)_
_verified: genuinely open — no `AssetRegistry`/asset-browser-panel found; ingestion pipeline exists (fe-renderer/src/ingester.rs) but the drag-drop UI + asset library flow specced here was not built (superseded by manual `GltfImport` dialog)_

## [ ] Track: Seedling Onboarding — Local/Peer Instance Bootstrap + Entity CRUD

_Link: [./tracks/seedling_onboarding_20260327/](./tracks/seedling_onboarding_20260327/)_
_Depends on: Wave 1 complete_
_verified: genuinely open — no first-launch onboarding wizard or entity CRUD dialogs beyond `seed_default_data()` found; only generic libp2p "bootstrap" networking terminology present_

---

## Wave 3: External Access & IoT Platform

Completed (collapsed — see folders / `.omc/plans/skill-chain-prompts.md`):

- [x] Entity Data Layer Phases 1-5 — hierarchy/HLC/observability, direct API reads + transform op-log, custom properties + petal iroh replication, query endpoint + scene streaming, fe-format/fe-entity-store/node_log
- [x] Entity Data Layer Phase 6.1 — fe-query LINQ builder, GraphQL, GIS validation
- [x] Hexon Format (Phase 6.5) — universal .hexon package, amp.SDK addressing, signed manifests (`docs/hexon-format-spec.md`)
- [x] Terrain & GPX (Phase 7) — 3D map tiles, GPX, elevation mesh, petal-bound terrain — [./tracks/_archive/terrain_gpx_maps_20260508/](./tracks/_archive/terrain_gpx_maps_20260508/)
- [x] Hexon Registry (Phase 8) — P2P distribution, multi-format assets, marketplace — [./tracks/_archive/crate_registry_20260508/](./tracks/_archive/crate_registry_20260508/) _(known gaps: terrain auto-config not wired; registry RBAC gap → now closed-by-design via auth_policy_pattern amendment 2026-07-11)_
- [x] Plugin Host (Phase 9A) — Rhai + Wasmtime runtime — [./tracks/_archive/plugin_host_20260509/](./tracks/_archive/plugin_host_20260509/)
- [x] Extension SDK + UI Slots (Phase 9B) — [./tracks/_archive/extension_sdk_ui_20260509/](./tracks/_archive/extension_sdk_ui_20260509/)
- [x] Plugin Testing DX (Phase 9C) — [./tracks/_archive/plugin_testing_dx_20260509/](./tracks/_archive/plugin_testing_dx_20260509/) _(9C's "fe-plugin should depend on fe-sdk" gap was closed by analytics_extension_api_20260710)_
- [x] Realtime API Gateway — MCP + REST + WebSocket — [./tracks/_archive/realtime_api_mcp_20260427/](./tracks/_archive/realtime_api_mcp_20260427/)

## [ ] Track: Entity Data Layer — DataFusion + GeoParquet + Peer Compute (Phase 6.2)
_Depends on: Phase 6.1 | Blocks: Final Architecture Review_
_Scope: DataFusion execution engine, GeoParquet read/write, spatial UDFs, DuckDB compat layer, peer compute mesh, Arrow Flight endpoint_
_verified: intentionally deferred per MEMORY.md; no DataFusion/GeoParquet/Arrow Flight code found — correctly left open_

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
_Note (2026-07-11): the relay is also the designated always-on seeder / first "verse service" per decisions §D3 — see verse_services_20260711_

## [ ] Track: Release CI — Cross-Compilation Pipeline, Artifact Publishing, Docker Image

_Link: [./tracks/release_ci_20260429/](./tracks/release_ci_20260429/)_
_Depends on: Cross-Platform Desktop, Headless Relay | Blocks: none_
_Scope: GitHub Actions PR check (3 OS), release workflow (8 targets), sccache, cargo-zigbuild for musl, macOS universal binary, Docker image to GHCR_
_Priority: P2 (CI validates what we claim about cross-platform support)_
_verified: genuinely open — no `.github/` directory found in the workspace_

---

## Code Review 2026-04-30 — Quality & Performance Fixes

Completed (collapsed): `code_review_20260430_egui_deprecation` [x], `code_review_20260430_channel_errors` [x], `code_review_20260430_db_graceful` [x].

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
_Note (2026-07-11): related engine-side hot-path fixes (entity-store O(N) clone, replication bridge) are now scoped in p2p_unblock_now_20260711 — this track remains the fe-ui-side counterpart_

## [ ] Track: Clippy Warnings, Code Quality, and Polish

_Link: [./tracks/code_review_20260430_clippy_quality/](./tracks/code_review_20260430_clippy_quality/)_
_Scope: `fe-ui`, `fe-webview`, `fe-database` — dead code, clippy lints, logging consistency, missing Debug | Blocks: none_
_Priority: MEDIUM (developer experience)_
_verified: genuinely open — MEMORY.md records a pre-existing `fe-ui` `peer_registry` dead_code warning and an unused import in `fe-terrain/store.rs` still outstanding_

---

## Tauri WebView Migration (2026-06-30) — Browser-First Path

Completed (collapsed): Tauri WebView Backend [x], Tauri IPC/Asset Bridge [x], Tauri Backend Cutover [x] — Bevy stays host, bevy_egui leads, `backend-tauri` is fe-webview's default. Folders: [tauri_webview_backend_20260630](./tracks/_archive/tauri_webview_backend_20260630/), [tauri_ipc_asset_bridge_20260630](./tracks/_archive/tauri_ipc_asset_bridge_20260630/), [tauri_backend_cutover_20260630](./tracks/_archive/tauri_backend_cutover_20260630/).

## SPIKE / Research Tracks

### Track: Tauri-Host Shell SPIKE — Exploratory: Full Shell Inversion

_Link: [./tracks/tauri_host_shell_spike_20260630/](./tracks/tauri_host_shell_spike_20260630/)_
_Depends on: none | Blocks: none (SPIKE, not on critical path)_
_Scope: SPIKE — time-boxed exploration of full architecture inversion: Tauri owns window/event-loop, Bevy renders into Tauri surface (REPLACES bevy_winit). Reference: sunxfancy/BevyTauriExample._
_Priority: P2 (exploratory, informs future decisions)_
_Status: SPIKE — verified: metadata.json status "pending", all 4 phases pending — not started_

- [x] Pear Runtime P2P Layer SPIKE — completed 2026-07-01, recommendation: hybrid — [./tracks/_archive/pears_p2p_layer_spike_20260630/](./tracks/_archive/pears_p2p_layer_spike_20260630/)

---

## Wave: Analytics & Extension Platform

> Goal: 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting. Builds on Phase 9 (Plugin System) and Phase 6.1 (fe-query).

Completed (collapsed — see folders):

- [x] Analytics Extension API — storage/query capabilities, fail-closed, WIT query-api, fe-plugin unified onto fe-sdk — [./tracks/_archive/analytics_extension_api_20260710/](./tracks/_archive/analytics_extension_api_20260710/) _(residual open: production DB wiring of `ExtensionStorageApi`/`ExtensionQueryApi` into the running binary — see spec.md; also a prerequisite for verse_services_20260711)_
- [x] IoT Extension Slice — device ↔ node push/pull proof, 6 bridge-loop tests — [./tracks/_archive/iot_extension_slice_20260710/](./tracks/_archive/iot_extension_slice_20260710/)
- [x] fe-ui Decomposition — god-file breakup into domain modules — [./tracks/_archive/feui_decomposition_20260710/](./tracks/_archive/feui_decomposition_20260710/) _(follow-up second split pass noted in spec.md)_
- [x] Terrain Scale Controls — multi-scale space operation, per-petal `world_scale` — [./tracks/_archive/terrain_scale_controls_20260711/](./tracks/_archive/terrain_scale_controls_20260711/) _(follow-ons tracked in terrain_lod_hardening_20260711)_

## [ ] Track: Hexon Deltas — Replayable Op-Log Hexons over P2P (spec only)

_Link: [./tracks/hexon_delta_format_20260710/](./tracks/hexon_delta_format_20260710/)_
_Depends on: Hexon Format (6.5), fe-sync P2P | Blocks: none (design-only)_
_Scope: A delta-hexon manifest type over the existing op_log, enabling replay/materialization, time-travel checkpoints, sovereign-authored signature chains, compression, and content-addressed P2P distribution of op-log deltas_
_**AMENDED 2026-07-11** (decisions §D4/§D5): per-op signatures corrected to MISSING (13 placeholder sites — must be built first); container = HashSeq/manifest-of-blobs for streamable types (ZIP no-streaming disqualified); log-first write path (op-log as WAL, SurrealDB as realtime materialized view, fe-query intent routing) now IN implementation scope; signing schemes to be unified. See spec.md "Amendments" section_

## [ ] Track: Policy Engine — Unified Authorization for Layered Tokens and Entry Points (spec only)

_Link: [./tracks/auth_policy_pattern_20260710/](./tracks/auth_policy_pattern_20260710/)_
_Depends on: hexon_delta_format (per-op signing D5-1) | Blocks: p2p_mycelium_completion (sync write path must be gated before real replication)_
_Scope: A central `Policy` abstraction (`evaluate(subject, action, resource) -> Decision`) replacing scattered ad-hoc role checks across every entry point; deny-by-default; closes the fe-hexon registry RBAC enforcement gap (Phase 8.4)_
_**AMENDED 2026-07-11** (decisions §D1): 8th surface added — the P2P sync write path has ZERO authz (`handle_write_row_entry`, sync_thread.rs:345) and is the highest-priority adapter; membership/auth state is NEVER plain LWW — signed causal-DAG ops + strong-removal resolver (Matrix state-reset precedent). See spec.md "Amendment" section_

## [ ] Track: Hexon P2P Bucket — 3D Visual IPFS (spec only)

_Link: [./tracks/hexon_p2p_bucket_20260710/](./tracks/hexon_p2p_bucket_20260710/)_
_Depends on: Hexon Registry (Phase 8), Hexon Deltas (spec, content-vs-operations counterpart) | Blocks: none (design-only)_
_Scope: Generalize node assets from GLTF-only to any file type or directory (placeholder-rendering contract); download/upload endpoints; distribute the content-addressed, sovereign-authored bucket over iroh P2P. Confirmed gap: `fe-network/src/iroh_blobs.rs` is a 13-line unfilled Wave-1 stub_
_**AMENDED 2026-07-11** (decisions §D2): handshake-then-swarm topology (authorize once at membership boundary, swarm within); relay-as-seeder is design, not fallback (~70% NAT reality, browsers 100% relay); BBR congestion control required (30x lever); HashSeq container alignment; crypto-shred for erasable content. See spec.md "Amendments" section_

## [ ] Track: Terrain Splat View — Synthesized 3D Splats from Hexon Data

_Link: [./tracks/terrain_splat_view_20260711/](./tracks/terrain_splat_view_20260711/)_
_Depends on: Terrain Scale Controls | Blocks: none_
_Scope: One splat per elevation texel (position from tile geo + elevation, color from satellite, slope-aware anisotropy) rendered via instanced quads; `TerrainViewMode { Mesh, Splats, Hybrid }` toggle persisted per petal; phase 2 pre-bakes quantized splat buffers into hexon archives (additive entry type + `splat_ready` flag) with a gis-tile-etl bake stage. Photogrammetric 3DGS training is explicitly out of scope (single orthographic view)._
_Status: pending — queued behind terrain_lod_hardening_20260711 (same crates; user gated "if easy and convenient")_

## [ ] Track: Hexon Scale Orchestration + Rulers — Real-World Scale in the GIS Data Layer

_Link: [./tracks/hexon_scale_orchestration_20260712/](./tracks/hexon_scale_orchestration_20260712/)_
_Depends on: Terrain Scale Controls (done) | Coordinate with: Terrain LOD Hardening (in-flight, same crates) | Blocks: none_
_Scope: Push real-world scale into the **existing** hexon format (`TilesetMeta` in fe-format — additive serde fields `native_scale`/`ground_sample_distance_m`/`crs`/`scale_bounds` + Web-Mercator backfill so already-installed hexons upgrade on load, NOT a parallel format). Hexon-authoritative scale: `apply_terrain_assignments` sets `world_scale` from the hexon (mirroring the elevation-encoding override) and the per-petal user slider becomes a clamped nudge within hexon-declared bounds. `CompositeTileSource` carries per-source `TilesetMeta` and reconciles mixed-GSD sources into one common metric frame for LOD selection. Then a `fe-terrain::ruler` pure-math module + `RulerPlugin`: scale-bar HUD, measurement tools (tape/area/bearing + GPX path length), adaptive world grid graticule, and dimensioned annotations. fe-ui stays free of a fe-terrain dep (clamp bounds travel via terrain JSON)._
_Non-goals: 3DGS scale ingestion, DataFusion/GeoParquet, CRS reprojection beyond WGS84/Mercator (crs recorded, not reprojected), any edit to `fe-hexon/src/manifest.rs`._
_Status: pending — spec + plan ready (6 data-layer-first phases)_

## [ ] Track: Terrain LOD Hardening — Seams, Clipping, Close-Range Quality

_Link: [./tracks/terrain_lod_hardening_20260711/](./tracks/terrain_lod_hardening_20260711/)_
_Depends on: Terrain Scale Controls | Blocks: Terrain Splat View (same crates)_
_Scope: Fix inter-tile seam gaps (black vertical lines between chunks), zoom-out clipping/holes (fetch ring vs scaled far plane coherence, despawn hysteresis), and close-range quality via elevation interpolation + denser meshes when the camera outruns the tileset's max zoom. Honest limit documented: satellite texture resolution is capped by the hexon's max zoom — higher-zoom hexons come from gis-tile-etl, not the renderer._
_Status: in progress (2026-07-11 GIS hardening run, W1)_

## [ ] Track: Asset Download Fix — Save Dialog + E2E Resolution (bug)

_Link: [./tracks/asset_download_fix_20260711/](./tracks/asset_download_fix_20260711/)_
_Depends on: node asset download (commit `3a97fc1`/`19a2df2`) | Blocks: none_
_Scope: User report 2026-07-11: Download button in the inspector Asset card produces no visible result ("no glb download box"). Root-cause the queue→bridge→blob-store chain with an integration test against a real temp blob store, and replace silent-copy-to-Downloads UX with an rfd native save dialog (fe-ui already uses rfd for GLB import) + persistent status row in the card._
_Status: in progress (2026-07-11 GIS hardening run, W2)_

## [ ] Track: Petal GIS Endpoints — Petal-Scoped Geo Data over REST

_Link: [./tracks/petal_gis_endpoints_20260711/](./tracks/petal_gis_endpoints_20260711/)_
_Depends on: fe-query GIS (Phase 6.1), Terrain & GPX (Phase 7), fe-api gateway | Blocks: none_
_Scope: Petal-scoped read endpoints for GIS data — nodes with geo positions + annotations, GPX tracks, bbox/radius spatial queries — as additive fe-api modules (new `gis.rs`, wired in `server.rs`; `rest.rs`/`assets.rs`/`Cargo.toml` are under external-IDE quarantine). Bearer + scope RBAC per the assets.rs precedent. Shared data layer (fe-query gis builders + fe-database handler tests) delivered alongside (W5)._
_Status: in progress (2026-07-11 GIS hardening run, W3+W5)_

## [ ] Track: GIS Query & Annotation UI — Edit, Query, Orchestrate Geo Data

_Link: [./tracks/gis_query_ui_20260711/](./tracks/gis_query_ui_20260711/)_
_Depends on: fe-ui decomposition, Petal GIS Endpoints (shares the `gis.annotation.*` property contract) | Blocks: none_
_Scope: In-app surface for editing, querying, and orchestrating GIS data: annotation editor on selected nodes (`gis.annotation.*` properties), spatial/property query panel with results list + fly-to, and a layer manager (satellite/terrain/GPX/GeoJSON visibility + opacity). fe-ui must not depend on fe-terrain — lat/lon math stays API/terrain-side._
_Status: in progress (2026-07-11 GIS hardening run, W4)_

---

## Wave: P2P Commons Hardening (2026-07-11)

> Goal: close the gap between the platform premise (hexon as P2P digital-twin format;
> FractalEngine as browser + peer-server for a distributed, resilient, self-permissioned
> federated 3D commons) and the verified present. Evidence:
> [research/hexon-p2p-commons/report.md](../research/hexon-p2p-commons/report.md).
> Ratified decisions: [./decisions/hexon-p2p-commons-20260711.md](./decisions/hexon-p2p-commons-20260711.md)
> (D1 consistency tiers + auth never-LWW; D2 handshake-then-swarm; D3 accelerator-only
> verse services; D4 log-first WAL + SurrealDB operational view; D5 sequencing; D6 non-promises).

```
Sequencing (decisions §D5 — ordered to avoid rework):

  p2p_unblock_now ──► p2p_mycelium_completion ──► (real replication, policy-gated)
  (try_send, BBR,      (reopened ph.1-2 + gossip RX;
   node_log cap)        gated by policy evaluate())
                                                        ┌──► verse_services (spec)
  hexon_delta_format ──► auth_policy_pattern ───────────┤    (accelerator-only)
  (per-op signing D5-1,   (sync-path evaluate(),        └──► serverless materializer (§D4)
   HashSeq, log-first)     causal-DAG membership)
  iroh_1_0_upgrade — independent; HARD DEADLINE 2026-12-31 (0.35 relay EOL)
```

## [ ] Track: P2P Unblock-Now — Bridge Backpressure, BBR, Node-Log Cap, Sync-Thread Blocking Read

_Link: [./tracks/p2p_unblock_now_20260711/](./tracks/p2p_unblock_now_20260711/)_
_Depends on: none | Blocks: p2p_mycelium_completion (FR-1 must land before real iroh-docs replaces the mock)_
_Priority: P0 (decisions §D5-3 — ships first, independent of format/auth)_
_Scope: try_send + drop-metric on the two-hop replication bridge (`fe-database/src/lib.rs:155`, `main.rs:113-120`); ring-buffer cap on hot-cache `node_log` (`fe-entity-store/src/lib.rs:136,198-207`); `spawn_blocking` for the sync-thread `std::fs::read` (`sync_thread.rs:377`); verify + explicitly configure BBR (30x throughput lever, iroh#4286)_
_Status: spec + plan ready — implementation next session_

## [ ] Track: Verse Services — Opt-In Per-Verse Centralization as Accelerator-Only Plugins (spec only)

_Link: [./tracks/verse_services_20260711/](./tracks/verse_services_20260711/)_
_Depends on: auth_policy_pattern (services are policy subjects), hexon_delta_format (delta units + signing) | Blocks: none_
_Priority: P2 (queued behind auth + delta foundations)_
_Scope: Plugin service class (`service.host`/`service.seed`/`service.presence`/`service.materialize` capabilities) for seeder, presence host, serverless materializer (§D4), order-hinter. Invariant: signed op-log stays state of record; any member reconstructs without the service (testable). Relay + registry re-framed as first instances. Sequencer authority deferred._
_Status: spec-only (decisions §D3)_

## [ ] Track: iroh 1.0 Upgrade — 0.35 → 1.0 Behind the VerseReplicator Seam

_Link: [./tracks/iroh_1_0_upgrade_20260711/](./tracks/iroh_1_0_upgrade_20260711/)_
_Depends on: none | Coordinate with: p2p_mycelium_completion (wire real iroh-docs against 1.x directly if still in flight)_
_Priority: P1 — **HARD EXTERNAL DEADLINE 2026-12-31** (n0 hosted-relay support for the 0.35 wire protocol ends; iroh 1.0 shipped 2026-06-15)_
_Scope: Migrate fe-sync/fe-network to iroh 1.x behind the `VerseReplicator` trait seam; re-verify BBR on the 1.x API; evaluate self-hosted 1.x relay (the §D3 seam) as the resilient default_
_Status: pending_

---

## Execution Order (open tracks, 2026-07-11)

Machine source of truth is the `execution_wave` + `depends_on`/`blocks` fields in each
`metadata.json`; this table is the human-readable rendering. Waves gate on the ratified D5
sequencing ([decisions](./decisions/hexon-p2p-commons-20260711.md)). Within a wave, tracks
are independent and parallelizable.

| Wave | Tracks | Gate |
|---|---|---|
| **0 — in-flight / P0** | `terrain_lod_hardening`, `petal_gis_endpoints`, `gis_query_ui`, `asset_download_fix` (GIS hardening W1–W5, underway); `p2p_unblock_now` (P0 fixes) | finish what's started; unblock fixes ship first |
| **1 — foundations** | `hexon_delta_format` (per-op signing D5-1 + HashSeq + log-first); `terrain_splat_view` | signing is the prerequisite for auth |
| **2 — gated by wave 1** | `auth_policy_pattern` (policy engine + sync-path `evaluate()` + causal-DAG membership); `p2p_mycelium_completion` (real iroh-docs + gossip RX) | needs signing + the policy gate |
| **3 — gated by auth+delta** | `hexon_p2p_bucket` (handshake-then-swarm); `verse_services` (accelerator-only) | needs auth engine + delta units |
| **DEADLINE** | `iroh_1_0_upgrade` — **by 2026-12-31** (0.35 relay EOL) | independent; calendar-driven |
| **4 — backlog** | `code_review_*` (mega_function, perf_hotpaths, clippy, cleanup); UI (`light_box`, `drag_drop_placement`, `petal_seed`, `inspector_settings`, `profile_manager`, `seedling_onboarding`); `build_size_mobile_prep`, `sso_federation`, `tauri_host_shell_spike` | no blocking deps; opportunistic |
| **5 — release/platform** | `cross_platform_desktop`, `headless_relay` (also the D3 seeder), `release_ci` | platform readiness |

## Archived Tracks

**Convention (established 2026-07-10; extended 2026-07-11 to a physical move):** Archiving means "implementation complete, no longer active work." A track is archived by (1) its checkbox being `[x]` above, (2) its `metadata.json` carrying `"archived": true` + `"archived_at"`, and — **as of 2026-07-11** — (3) its folder living under `./tracks/_archive/<id>/` rather than `./tracks/<id>/`. The move is non-destructive (git-tracked rename; content identical) and links above were updated to the `_archive/` paths. Open/in-progress tracks stay at `./tracks/<id>/`. To un-archive, move the folder back and flip the flags.

Tracks archived in this pass (2026-07-10 reconciliation): `ui_manager_refactor_20260419`, `viewport_foundation_20260402`, `scene_graph_bridge_20260402`, `selection_system_20260402`, `transform_gizmos_20260402`, `shared_peer_infra_20260419`, `garden_console_20260322`, `mycelium_live_20260322`, `bloom_stage_20260322`, `petal_portal_20260322`, `fractal_atlas_20260322`, Entity Data Layer Phase 6.1 (fe-query), Hexon Format Phase 6.5, Terrain & GPX Phase 7, Hexon Registry Phase 8, Realtime API Gateway, `tauri_webview_backend_20260630`, `tauri_ipc_asset_bridge_20260630`, `tauri_backend_cutover_20260630`, `code_review_20260430_egui_deprecation`, `code_review_20260430_channel_errors`, `code_review_20260430_db_graceful`, `plugin_host_20260509`, `extension_sdk_ui_20260509`, `plugin_testing_dx_20260509`.

Tracks archived in the 2026-07-10/11 ultrapilot close-out pass: `analytics_extension_api_20260710` (residual DB-wiring item still open — see spec.md), `iot_extension_slice_20260710`, `feui_decomposition_20260710` (follow-up second split pass noted — see spec.md). `hexon_delta_format_20260710`, `auth_policy_pattern_20260710`, and `hexon_p2p_bucket_20260710` remain spec-only/pending — not archived.

**2026-07-11 hexon-p2p-commons pass:** this file was pruned (completed entries collapsed to one-liners — no track folders touched); `p2p_mycelium_completion_20260701` phases 1-2 were **reopened** with file:line evidence (see its metadata.json); the three 2026-07-10 spec-only tracks were amended against the ratified decision record; and the P2P Commons Hardening wave was registered (`p2p_unblock_now_20260711`, `verse_services_20260711`, `iroh_1_0_upgrade_20260711`). Wave 2's historical dependency graph was dropped from this index in the prune — it survives in the git history and the track folders.
