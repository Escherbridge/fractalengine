---
type: retro
track: api_mcp_integration_tests_20260717
title: "Retro: API MCP Integration Tests"
created: 2026-10-31
---

## Summary

**What shipped**: Reusable API integration-test harness (in-mem SurrealDB + real fe-api router + tower oneshot) plus api_integration.rs and mcp_integration.rs suites. Closes SG-08 styleguide gap (harness existed but nothing consumed it).

**Implementation**:
- fe-test-harness/src/api.rs (311 lines) — reusable harness: in-mem DB + live fe-api router + tower oneshot.
- fe-api/tests/api_harness_smoke.rs — Phase 1 proof of concept.
- fe-api/tests/api_integration.rs (304 lines) — Phase 2: query-guard enforcement, SQL injection, RBAC negatives, GIS queries, CSV egress.
- fe-api/tests/mcp_integration.rs (741 lines) — Phase 3: 6-tool inventory, CRUD round-trip, weak-authz markers for mcp_scene_primitives flipping to strict.

**Verification**:
- Phases 1-3 code: @4e79780 (2026-07-17). Plan.md checkboxes stale despite code landing (pattern seen elsewhere).
- Phase 4 (comprehensive endpoint-family expansion): NOT started; Phase 4 sub-items landed later via mission F14 @917b502 as new files (ws_integration.rs, hexon_tileset_test.rs, token_lifecycle_test.rs, cross-thread scenario, MCP fuzz).
- Mission acceptance (A27) satisfied in aggregate: WS, hexon/tileset, IoT ingest+export, share mint→redeem, token lifecycle, cross-thread, MCP negatives green in single workspace sweep.
- In-app verification: not applicable.

**Left open / handed off**:
- Phase 4 endpoint-family inventory artifact (enumerate router surface vs coverage) — ~66 routes in fe-api/src/server.rs, partial substitute in tests/existence_oracle_test.rs; DECISION: not a closure blocker, ledgered as AGENT_DOABLE_RATIFIED size S.
- REST multipart POST /api/v1/petals/{p}/assets happy path untested (only MCP twin + denial rows tested).
- Weak-authz markers: create_node / create_petal / update_transform scope checks asserted KNOWN-WEAK, flipped strict in mcp_scene_primitives_20260716.

**Lessons**:
- In-mem harness is powerful: allows real router + SurrealDB semantics without spinning a process; enabled 19+ new test files downstream.
- Weak-authz markers are load-bearing: they front-load the knowledge that a gate is incomplete, preventing silent regressions.
- Integration tests do NOT become stale: the router surface changes frequently; test existence-oracle (static route table) is the cheaper refresh when coverage needs auditing.

**Commits**: 4e79780 (Phases 1-3); F14 @917b502 (Phase 4 items); F19 workspace sweep.
