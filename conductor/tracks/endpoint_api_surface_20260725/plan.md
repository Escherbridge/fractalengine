---
type: Implementation Plan
title: "Implementation Plan: Per-Endpoint Read/Write API Surface"
tags: [endpoint_api_surface_20260725]
resource: ./spec.md
---

# Implementation Plan: Per-Endpoint Read/Write API Surface

Six phases: addressing → read → write → MCP → egress seam → new-type queries.
Read before write to de-risk the mutate path (ratified Q-3). TDD on the address
projection + query composition; single sweep at the end (N-6). The cleanest-
isolated Wave-1 track (own crates).

## Phase 1: Public endpoint addressing (FR-1) [P0]

- [ ] Task: project T1's stable address → public URI; resolve URI→object and
      back; reconcile `fe-renderer/src/addressing.rs`; stability-across-move test.

## Phase 2: Read endpoint per object (FR-2) [P1]

- [ ] Task: GET an object's data through `fe-query` (common + type-specific);
      typed error on unknown/unauthorized (no panic); per-type read tests
      (props/geometry/children).

## Phase 3: Write / mutate endpoint (FR-3) [P1]

- [ ] Task: mutations routed through T1's sync-safe ops (move/scale/rotate/
      edit-props/delete/trigger); same op-log entry as the UI path (test).
- [ ] Task: fe-policy authorization on writes (reject test); position-locked
      objects reject illegal free-translate (test).

## Phase 4: MCP per-endpoint CRUD (FR-4) [P1]

- [ ] Task: extend `fe-api/src/mcp.rs` so an MCP client can address/read/mutate/
      delete an endpoint; authz + sync-safety preserved; primitives-only
      guarantees preserved or explicitly extended.

## Phase 5: Copy-API-string / report seam (FR-5) [P1]

- [ ] Task: extend `gis/egress_strings.rs` — per-object string + report; expose
      the seam T4 calls; round-trip test (paste → same read).

## Phase 6: New-type queries + docs + sweep (FR-6) [P1]

- [ ] Task: make promoted stamp nodes + earthwork regions first-class in
      `fe-query` (filter/GIS/egress) via the generic node abstraction + type
      tags; integration test against real T2/T3 data at join time ("all stamps
      on path X", "total cut/fill in petal P").
- [ ] Task: `fe-api/AGENTS.md` + `fe-query` notes — URI scheme, read/write
      contract, MCP surface, auth model (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, workspace tests
      (N-6). Retro; in-app/endpoint verify user-gated.
