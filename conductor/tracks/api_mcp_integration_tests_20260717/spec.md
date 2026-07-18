---
type: Track Spec
title: API + MCP Integration Tests — reusable harness and first downstream consumer of fractalengine-test-harness
description: In-mem SurrealDB + real fe-api router + tower oneshot harness, with api_integration.rs and mcp_integration.rs suites covering query guard, injection, RBAC negatives, GIS, egress CSV, and MCP round-trips with weak-authz markers
tags: [feature, api_mcp_integration_tests_20260717, in_progress]
timestamp: 2026-07-17T00:00:00Z
resource: ./metadata.json
---

# Specification: API + MCP Integration Tests

**Track ID:** `api_mcp_integration_tests_20260717`
**Priority:** P1 ENABLING
**Crates:** `fe-api` (tests), `fe-test-harness` (package `fractalengine-test-harness`)

## Overview

A reusable integration-test harness for the HTTP/MCP surface: an in-memory
SurrealDB instance, the **real** fe-api router (not a mock), and
`tower::ServiceExt::oneshot` request dispatch — so every endpoint test
exercises the same middleware, extractors, guards, and serialization the live
app runs. Two suites consume it: `api_integration.rs` and `mcp_integration.rs`.

This gives `fractalengine-test-harness` its **first downstream consumer**,
closing the styleguide SG-08 gap (the harness crate exists but nothing outside
it consumes it).

## Functional Requirements

- **FR-1 — Reusable harness.** A shared test-support module (in or consuming
  `fractalengine-test-harness`): spin up in-mem SurrealDB, seed
  verse/fractal/petal scope fixtures + roles, build the real router, dispatch
  via tower `oneshot`, helpers for authed/unauthed requests and JSON
  assertions.
- **FR-2 — `api_integration.rs` coverage:**
  - query guard: cost/row/timeout limits enforced (row-cap = error-not-truncate);
  - SQL-injection attempts through query params and raw-SQL endpoints rejected;
  - RBAC negatives: Viewer/None denied on write endpoints, cross-scope denied;
  - GIS: spatial query endpoints round-trip against seeded geometry;
  - egress CSV: export endpoint returns well-formed CSV with expected headers
    and guard limits applied.
- **FR-3 — `mcp_integration.rs` coverage:** MCP tool round-trips over the real
  `/mcp` surface (list tools, call each existing tool, verify DB effect +
  response shape), plus **weak-authz markers**: the known
  create_node / create_petal / update_transform scope-check gaps are asserted
  at their current (weak) behavior with explicit `KNOWN-WEAK` test names
  referencing `mcp_scene_primitives_20260716`, so that track's dispatcher flip
  turns them into failing-then-fixed strict assertions.
- **FR-4 — Sweep integration.** Both suites run as ordinary `cargo test -p
  fe-api` targets — no special runner, no network, no on-disk DB.
- **FR-5 — Comprehensive suite expansion (user ask 2026-07-18).** Grow the
  suites to the remaining fe-api surface, keeping the FR-1 harness as the
  shared substrate (no parallel fixtures):
  - **remaining endpoint families:** WS/realtime surface if present (inventory
    first — cover or record N/A), hexon/tileset endpoints (`/crates/*` as
    currently shipped; note re-point owned by `hexon_unification_20260716`),
    IoT ingestion (`POST /petals/:id/iot/readings` batch semantics + guard
    whitelist seam) and reading-shaped export, share-URL **mint → redeem
    lifecycle** (authed mint, public redemption, scope ceiling, expired/
    tampered-signature negatives), auth **token lifecycle** (issue, use,
    expiry, revocation via session cache TTL);
  - **cross-thread scenarios** via `fractalengine-test-harness`: DB↔API↔sync
    seams — a write through the API observed via the DB thread's channel
    contract, and sync-facing effects asserted at the seam (mock replicator
    boundary is fine; no live P2P);
  - **MCP negative/fuzz coverage:** malformed JSON-RPC frames, unknown tool
    names, wrong-typed/oversized arguments, missing-scope calls, and a
    bounded structured-fuzz pass over tool arg schemas — assert graceful
    errors, never panics or channel poisoning.

## Acceptance criteria

- Both suites green in the workspace sweep (single end-of-session run per the
  standing test-execution policy).
- Harness is reusable: adding a new endpoint test requires only fixtures +
  assertions, no per-test boilerplate for DB/router setup.
- `fractalengine-test-harness` appears as a dev-dependency of at least one
  downstream crate (SG-08 closed).
- Weak-authz markers documented and cross-referenced from
  `mcp_scene_primitives_20260716`.
- FR-5: endpoint-family inventory recorded (covered / N/A per family); fuzz
  pass bounded and deterministic (seeded) so the sweep stays reproducible.

## Out of scope

- Fixing the weak-authz gaps themselves (owned by `mcp_scene_primitives_20260716`).
- P2P/sync/relay integration tests (different thread/runtime seams).
- Load/perf testing of the API surface.
