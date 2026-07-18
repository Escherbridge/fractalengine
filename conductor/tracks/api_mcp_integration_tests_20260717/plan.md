---
type: Implementation Plan
title: "Implementation Plan: API + MCP Integration Tests"
tags: [api_mcp_integration_tests_20260717]
resource: ./spec.md
---

# Implementation Plan: API + MCP Integration Tests

## Overview

Harness first (FR-1), then the two suites in parallel-safe order. Suites are
written against current behavior — including explicitly-marked KNOWN-WEAK authz
cases — so they are green at landing and act as a ratchet for
`mcp_scene_primitives_20260716`. Phase 4 (FR-5, user ask 2026-07-18) expands to
the remaining surface on the same harness. Full workspace sweep runs ONCE at
session end.

## Phase 1: Harness (FR-1)

- [ ] Task: Shared test-support module — in-mem SurrealDB bring-up, scope/role fixture seeding, real fe-api router construction, tower `oneshot` dispatch helpers, authed/unauthed request builders, JSON assertion helpers
- [ ] Task: Wire `fractalengine-test-harness` as the fixture/assertion provider (dev-dependency from fe-api) — first downstream consumer, SG-08 gap
- [ ] Verification: one smoke test (health/list endpoint) proves the harness end-to-end [checkpoint]

## Phase 2: api_integration.rs (FR-2)

- [ ] Task: Query-guard tests — cost/row/timeout limits, row-cap errors (not truncates)
- [ ] Task: Injection tests — SQL-injection attempts via params + raw-SQL endpoints rejected
- [ ] Task: RBAC negative tests — Viewer/None write denials, cross-scope denials
- [ ] Task: GIS tests — spatial queries round-trip against seeded geometry
- [ ] Task: Egress CSV tests — well-formed CSV, expected headers, guard limits applied

## Phase 3: mcp_integration.rs (FR-3)

- [ ] Task: MCP round-trip tests — list tools, call each existing tool, assert DB effect + response shape
- [ ] Task: Weak-authz KNOWN-WEAK markers — create_node / create_petal / update_transform current behavior pinned, named to reference mcp_scene_primitives_20260716
- [ ] Task: Cross-reference note added to mcp_scene_primitives_20260716 (flip markers to strict when dispatcher lands)

## Phase 4: Comprehensive expansion (FR-5, user ask 2026-07-18)

- [ ] Task: Endpoint-family inventory — enumerate the full fe-api router surface vs current coverage; record covered / to-cover / N/A (incl. whether a WS/realtime surface exists) [checkpoint: inventory in track folder]
- [ ] Task: Hexon/tileset endpoint tests (`/crates/*` as shipped; annotate hexon_unification_20260716 re-point dependency)
- [ ] Task: IoT tests — batch ingestion semantics + guard whitelist seam + reading-shaped export
- [ ] Task: Share-URL lifecycle tests — mint→redeem, scope ceiling, expired + tampered-signature negatives
- [ ] Task: Auth token lifecycle tests — issue, use, expiry, revocation (session-cache TTL path)
- [ ] Task: Cross-thread scenarios via fractalengine-test-harness — API write observed through the DB thread channel contract; sync-facing effect asserted at the seam (mock replicator boundary)
- [ ] Task: MCP negative/fuzz — malformed frames, unknown tools, wrong-typed/oversized args, missing scope; bounded seeded structured-fuzz over tool arg schemas (graceful errors, no panics)

## Phase 5: Close-out

- [ ] Task: Suites green in the single end-of-session workspace sweep (FR-4, acceptance gate)
- [ ] Task: Retro + archive per track-per-feature workflow
