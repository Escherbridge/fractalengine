---
type: Track Spec
title: Runtime Instance Guardrails — fix the GPU-OOM crash and cap every spawn path
description: Diagnose and fix the 2026-07-17 create_bind_group GPU-OOM crash (~10-20M mesh instances) via spawn caps, degenerate-scale guards, double-materialization elimination, and an instance-count watchdog with user-visible warning
tags: [bug, runtime_instance_guardrails_20260717, in_progress]
timestamp: 2026-07-17T00:00:00Z
resource: ./metadata.json
---

# Specification: Runtime Instance Guardrails

**Track ID:** `runtime_instance_guardrails_20260717`
**Priority:** P0 — user-reported crash, 2026-07-17
**Likely crates:** `fe-ui`, `fe-renderer`, `fe-terrain`, `fractalengine` (audit determines final set)

## Crash signature (verbatim, user run 2026-07-17)

```
Device::create_bind_group label='preprocess_gpu_indexed_frustum_culling_bind_group'
Buffer binding 6 range 2758820944 exceeds max_*_buffer_binding_size limit 2147483647
panics in bevy_pbr prepare_mesh_bind_groups + prepare_preprocess_bind_groups
then host 'memory allocation of 2703360 bytes failed' -> STATUS_STACK_BUFFER_OVERRUN.
```

2.75 GB of per-instance GPU data implies roughly **10–20 million mesh instances**
(at ~128–256 B/instance) — a runaway spawner or unbounded persisted data being
materialized wholesale, on a memory-constrained Windows machine. The crash fires
during render prepare, so the app dies before the user can see or fix anything.

## Diagnosis approach

Reproduce against the user's live database (`data/fractalengine.db` — **never
delete, reset, or overwrite anything under `data/`**; read-only inspection and
launching the app against it are allowed), then audit in four lanes:

1. **Spawn audit** — every code path that spawns renderable entities in a loop
   or per-DB-row (path-asset stamping, terrain tiles, GPX ribbons/points, node
   materialization, splat/petal reconcile, sample installers). For each: is it
   bounded? What is the bound? Can persisted data drive it unbounded?
2. **Terrain audit** — tile/LOD spawn counts, mesh instance fan-out per tile,
   degenerate `world_scale` interactions (a near-zero scale can explode
   spacing-derived stamp counts).
3. **Render audit** — what feeds bevy_pbr's per-instance buffers; whether GPU
   preprocessing (indexed frustum culling) can be bounded or disabled as a
   fallback on memory-constrained hardware.
4. **DB audit** — row counts per table in the live DB (read-only queries) to
   find whether persisted data (e.g. accumulated stamps, duplicated path
   assets, runaway node rows) is the instance source, and whether a reconcile
   loop re-materializes rows it already spawned (double-materialization).

Known prior art (build on, don't duplicate):
`fe-ui/src/verse_manager/path_asset_reconcile.rs` already has `MAX_STAMPS=4096`
and `sanitize_world_scale` guards — the crash proves at least one spawn path is
**not** behind such a cap, or a cap is applied per-frame/per-asset and
accumulates.

## Functional Requirements

- **FR-1 — Every spawn path capped.** Each renderable-entity spawn loop found
  by the audit gets an explicit upper bound (constant or config), applied
  cumulatively (not per-invocation), with a warn log naming the path when the
  cap truncates.
- **FR-2 — Degenerate-scale guarded.** Every spacing/count computation derived
  from `world_scale` (or any user/DB-sourced scale) sanitizes ≤0, non-finite,
  and near-zero values before use, mirroring `sanitize_world_scale`.
- **FR-3 — Double-materialization eliminated.** Reconcile/materialize systems
  are idempotent: re-running against unchanged DB state spawns zero new
  entities. Any found duplicate-spawn loop is fixed at the source.
- **FR-4 — Instance watchdog with user-visible warning.** A runtime system
  tracks total renderable instance count; crossing a soft threshold surfaces a
  user-visible in-app warning (not just a log line) identifying the dominant
  source; crossing a hard threshold stops further spawning before render
  prepare can OOM.
- **FR-5 — Verified live app run.** The app is launched against the user's
  existing `data/` and runs past startup + initial scene materialization
  without the create_bind_group panic.
- **FR-6 — Render-distance-gated spawning (user directive 2026-07-17).**
  Structural follow-on to the caps: materialize node entities only within a
  configurable camera radius and stream them in/out as the camera moves, so
  worst-case instance count is bounded by density × radius, not petal size.
  Prior art: archived tracks `render_distance_lod_20260407` and
  `relay_data_horizon_20260407` (read their specs before designing). The
  FR-1..4 caps/watchdog stay as the safety net underneath. Independently
  schedulable phase — the crash fix does not block on it.

## Acceptance criteria

- **The app launches against the user's existing `data/` without the
  `create_bind_group` panic** (primary gate — FR-5).
- Audit findings recorded (spawn-path inventory with per-path bound, DB row
  counts, root cause) in this track folder.
- FR-1..FR-4 each covered by unit/integration tests where testable without a
  GPU (cap math, sanitize guards, idempotent reconcile, watchdog thresholds).
- No writes of any kind to `data/` performed by the diagnosis workflow.

## Out of scope

- Renderer-level LOD/instancing performance work beyond the crash fix.
- DB compaction/migration of the user's data (any data-shape fix must be a
  code-side guard, not a data rewrite, unless the user explicitly ratifies).
- General memory budgeting for the whole app (separate future track).

## Root cause + resolution (2026-07-17)

**Diagnosed:** duck.glb (Khronos sample) carries an embedded glTF camera; the
stamp materializer legitimately stamps it 8,192x (2 tracks x MAX_STAMPS 4096 —
spacing-meters vs missing map scale made per-track counts saturate), and every
SceneRoot expansion spawned a real active Camera3d. 8,197 cameras -> 16,387
extracted views -> ~8,193 shadow views x ~1,913 visible instances =
15,675,110 MeshUniform slots x 176 B = the exact 2.76 GB create_bind_group
crash. Bevy render buffers never shrink, so one spiked frame poisoned the rest.
Four crash samples decoded to within 0.1% of each other; entity-count watchdog
was blind because views, not Mesh3d entities, were the multiplier.

**Fix:** fe-renderer/src/camera.rs deactivate_foreign_cameras — PostUpdate
system deactivating any Added<Camera> lacking OrbitCameraController before
extraction. Verified: 309 s run at full frame rate, 0 validation errors,
views max 3, data_buffer ~1.5 MB (was 2.76 GB). Silent stamp-cap saturation
now warns (path_asset_reconcile). DIAG-15M render-census scaffold kept at
debug level (fe-runtime/src/diag15m.rs).

**Follow-ups:** glb-embedded punctual lights could recreate the shape if
imported with shadows enabled (extend the foreign-scene-component guard);
stamp groups respawn every 30-60 s (churn worth a look); status-bar
"0 petals 0 models" counter reads wrong with content present.
