---
type: Track Spec
title: Per-Endpoint Read/Write API Surface — Every Object an Addressable, Queryable, Drivable Endpoint
description: Make every object (node, promoted stamp, earthwork region, path) a stable public endpoint with a full read + write API (D-A4) — GET its data (props, geometry, type-specific payload) and MUTATE it (move/scale/rotate/edit-props/delete/trigger-op) through the sync-safe ops, authorized by fe-policy, exposed over REST + the existing MCP tools so an external tool or agent can drive the scene. Also emits the copy-API-string / report seam the analyst persona (D-A3) and the context menu (T4) consume. Owns fe-api/fe-query; consumes the node spine (T1). Wave 1.
tags: [feature, endpoint_api_surface_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Per-Endpoint Read/Write API Surface

**Track ID:** `endpoint_api_surface_20260725`
**Type:** feature · **Wave:** 1 · **depends_on:** `node_lifecycle_addressing_20260725` · **coordinates:** `contextual_controls_20260725`
**Crates:** `fe-api/*`, `fe-query/*`, `fe-renderer/src/addressing.rs`

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md).
Foundation: [`../node_lifecycle_addressing_20260725/spec.md`](../node_lifecycle_addressing_20260725/spec.md)
(address FR-4, ops, lifecycle events FR-6), the existing egress backbone
(`fe-api`, `fe-query`, `fe-ui gis/egress_strings.rs`), and the existing 6 MCP
tools (`fe-api/src/mcp.rs`).

## Overview

The program's defining idea (D-A4, user's own words): "API-level interactions on
each endpoint." Every object is a stable, addressable endpoint you can **read**
(its data) and **write** (drive it) — turning the builder into a programmable
spatial backend an external tool or agent can operate, while the analyst (D-A3)
gets copy-paste egress strings and queries over the same addresses. This is the
"report on everything / drive everything" layer that reconciles the game-feel
builder with the analytics/BIM data underneath.

### Ground truth (2026-07-25)

- The egress backbone exists: `fe-api` (`rest.rs`, `server.rs`, `ws.rs`,
  `mcp.rs`, `query_guard.rs`, `limits.rs`), `fe-query` (builder, gis, graphql,
  columnar), and `fe-ui gis/egress_strings.rs` (copy-paste SQL/API strings).
  GeoParquet/DataFusion (Phase 6.2) is intentionally deferred — this extends the
  SQL/API path, not that.
- MCP already ships 6 primitives-only scene tools (fe-relay-hosted). D-A4 extends
  this to per-endpoint CRUD.
- Node addresses come from T1 FR-4 (stable internal key); T1 Q-1 assigns the
  **public URI projection to this track**. `fe-renderer/src/addressing.rs` holds
  the render-side addressing view to reconcile.

## Functional Requirements

- **FR-1 — Public endpoint addressing.** Project T1's stable node address into a
  public, resolvable endpoint identifier (URI). Reconcile with the render-side
  `addressing.rs`. *Acceptance:* every node/stamp/region/path resolves to a
  stable URI and back; the URI survives move/rename (rides T1 FR-4); documented.

- **FR-2 — Read endpoint per object.** GET returns an object's data: common
  (props, geometry, children, scope/role) + type-specific (stamp overrides;
  earthwork region footprint/material/cut-fill volume; path curve). Reads go
  through `fe-query` so they compose with existing filters/GIS. *Acceptance:* a
  read of each object type returns its full payload including the T2/T3 type-
  specific fields; unknown/unauthorized address → typed error, never a panic.

- **FR-3 — Write / mutate endpoint per object.** Mutations — move (where legal),
  scale, rotate, edit-props, delete, trigger-op (e.g. re-sculpt a region) — are
  applied through T1's **sync-safe ops** (N-4), authorized via `fe-policy`
  (N-5). The API is a *client* of the same ops the UI uses; it never bypasses
  them. *Acceptance:* each mutation produces the same op-log entry as the UI
  path; an unauthorized caller is rejected by fe-policy; a write survives P2P
  merge; position-locked objects (stamps) reject illegal free-translate writes.

- **FR-4 — MCP per-endpoint CRUD.** Extend the existing MCP tool surface
  (`fe-api/src/mcp.rs`) so an agent/external tool can read + drive any endpoint
  (create/read/update/delete/trigger). *Acceptance:* an MCP client can address an
  object, read it, mutate it, and delete it; ops are authorized + sync-safe
  (FR-3); primitives-only guarantees from the city-building slate are preserved
  or explicitly extended.

- **FR-5 — Copy-API-string / report seam (serves T4 + the analyst).** Every
  object emits a copy-paste API/SQL string (extends `gis/egress_strings.rs`) and
  a report view of its data. Expose this as the small seam T4's copy-API-string /
  report verbs call. *Acceptance:* the string round-trips (paste → same read);
  the seam is callable from T4; analyst can drop the string into an external
  tool and get the object's live data.

- **FR-6 — New object types are queryable (N-10).** Promoted stamp nodes and
  earthwork region nodes are first-class in `fe-query` (filter, GIS, egress) —
  read through the generic node abstraction + type tags so this track does not
  edit T2/T3 files. *Acceptance:* querying "all stamps on path X" and "total
  cut/fill in petal P" works; egress strings for both types are valid; the
  integration test runs against real T2/T3-produced data at join time.

## Non-Functional Requirements

Inherits the shared pool. Load-bearing: **N-4** (all writes are sync-safe T1
ops — the API never drops/mutates rows directly), **N-5** (authz in fe-policy,
API layer carries caller identity but does not decide policy; no `block_on` on
Bevy systems — API runs on its own tokio gateway thread), **N-10** (every object
reportable). Reuse `query_guard.rs`/`limits.rs` for read safety. No GeoParquet.

## Dependencies & concurrency

- **depends_on:** `node_lifecycle_addressing_20260725` (address FR-4, ops,
  events FR-6). **coordinates:** `contextual_controls_20260725` (provides the
  FR-5 seam it calls). Reads T2/T3 data **shapes** at integration time via the
  generic node abstraction (no file edits into T2/T3). **blocks:** none.
- **Owns (file partition):** `fe-api/*`, `fe-query/*`,
  `fe-renderer/src/addressing.rs`. The cleanest-isolated Wave-1 track — its own
  crates, safe to run fully parallel.

## Open questions (ratify before build)

- **Q-1 — URI scheme.** `fe://verse/fractal/petal/node` human-readable path
  (recommended — legible, matches the scope hierarchy) vs opaque stable ids?
- **Q-2 — Write auth for external/MCP callers.** Reuse the existing session/relay
  auth to derive a `RoleLevel` for API callers (recommended — one auth model),
  and document how an agent authenticates?
- **Q-3 — Read-first vs both.** Land read (FR-2) in an early phase, write (FR-3)
  in a later phase within this track (recommended — de-risk the mutate path),
  but both ship in-track (D-A4 requires read+write)?
- **Q-4 — MCP shape.** Extend the existing 6 primitives tools to per-endpoint
  CRUD (recommended — one coherent surface) vs a new generic endpoint tool?

## Ratified decisions (2026-07-25)

User ratified 2026-07-25 (Q-2 asked; Q-1 settled by the program address choice;
Q-3/Q-4 recommended defaults adopted).

- **Q-1 → RATIFIED: `fe://verse/fractal/petal/node` human-readable path** (not
  opaque ids). Settled by the program-level address decision — **T1 now defines
  this URI at the data layer; this track exposes it** over REST/MCP and
  reconciles `fe-renderer/src/addressing.rs`. FR-1 becomes *expose + reconcile*,
  not *project an internal key*. Gates FR-1.
- **Q-2 → RATIFIED: reuse the existing session/relay auth to derive a `RoleLevel`
  for API/MCP callers;** fe-policy enforces Editor+ per scope — one auth model for
  UI and API. Document how an agent authenticates. Gates FR-3/FR-4.
- **Q-3 → RATIFIED: land read (FR-2) in an early phase, write (FR-3) in a later
  phase — but both ship in-track** (D-A4 requires read+write). De-risks the
  mutate path. Gates phase order.
- **Q-4 → RATIFIED: extend the existing 6 primitives MCP tools to per-endpoint
  CRUD** (one coherent surface), not a new generic endpoint tool. Gates FR-4.

## Out of scope

- The address/op/event **primitives** (T1 owns them; this track projects +
  exposes them).
- The context-menu UI (T4 calls the FR-5 seam).
- Producing stamp/region **data** (T2/T3 produce it; this track serves it).
- GeoParquet/DataFusion egress (Phase 6.2, deferred).
