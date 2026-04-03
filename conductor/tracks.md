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
