---
type: Implementation Plan
title: "Implementation Plan: Runtime Instance Guardrails"
tags: [runtime_instance_guardrails_20260717]
resource: ./spec.md
---

# Implementation Plan: Runtime Instance Guardrails

## Overview

Diagnose-first: reproduce with the live DB and inventory the instance source
before writing guards, so caps land at the actual runaway path instead of
speculatively everywhere. Full workspace test sweep runs ONCE at the end.

**Hard rule throughout: no deletes/resets/overwrites under `data/`.**

## Phase 1: Repro + audits (diagnosis)

- [ ] Task: Launch the app against the user's existing `data/` and capture the crash log; confirm the signature matches (buffer binding 6, ~2.75 GB)
- [ ] Task: DB audit — read-only row counts per table (nodes, path assets, stamps, GPX points, terrain refs) to size the persisted-data candidate
- [ ] Task: Spawn audit — inventory every renderable-entity spawn loop across fe-ui/fe-terrain/fe-renderer/fractalengine with its current bound (or NONE)
- [ ] Task: Terrain + render audit — tile/LOD fan-out, per-instance buffer feeders, degenerate-scale interactions with spacing/count math
- [ ] Task: Root-cause writeup — name the runaway path(s) and the multiplier chain that reaches 10-20M instances [checkpoint: findings recorded in track folder]

## Phase 2: Guards (fix)

- [x] Task: FR-1 — cumulative caps on every uncapped spawn path found in Phase 1, warn log on truncation (TDD on cap math)
- [x] Task: FR-2 — degenerate-scale sanitization at every scale-derived spacing/count site, reusing/mirroring sanitize_world_scale (TDD)
- [x] Task: FR-3 — make reconcile/materialize idempotent; fix the double-materialization source (TDD: re-run against unchanged state spawns 0)
- [x] Task: FR-4 — instance watchdog resource + system: soft threshold -> user-visible in-app warning naming dominant source; hard threshold -> spawn stop (TDD on thresholds)

## Phase 3: Verification

- [x] Task: FR-5 — live app run against the user's existing `data/`: launches past startup + scene materialization without the create_bind_group panic (primary acceptance gate)
- [ ] Task: Single end-of-track workspace sweep (test/clippy/fmt) per standing directive
- [ ] Task: Retro + archive per track-per-feature workflow

## Phase 4: Render-distance streaming (FR-6, user directive 2026-07-17)

- [ ] Task: Read archived render_distance_lod_20260407 + relay_data_horizon_20260407 specs; design the camera-radius spawn gate over the Phase-2 allowance seams
- [ ] Task: Materialize-within-radius + stream-out on exit (hysteresis so edge nodes do not thrash); caps stay as safety net (TDD on gate math)
- [ ] Task: Live-run verification with a dense petal (instance count tracks camera locality, not petal size)
