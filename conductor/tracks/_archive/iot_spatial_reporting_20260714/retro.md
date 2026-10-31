---
type: retro
track: iot_spatial_reporting_20260714
title: "Retro: IoT Spatial Reporting"
created: 2026-10-31
---

## Summary

**What shipped**: Expose live IoT data as queryable spatial rows (spatial anchor + timestamp + metric + value) so /api/v1/query and analytics egress serve real-time IoT into PowerBI/spreadsheets. Includes capped node_log, time-series query support, batch ingestion hook.

**Implementation**:
- fe-database handlers/iot_reading.rs + tests — iot_reading table, insert_readings handler, read-back tests. Phase 0-1 delivered @4345e13.
- fe-query builder/timeseries.rs — time-series query builders. Phase 3 delivered @4345e13.
- fe-api iot.rs + tests/iot_ingest_test.rs — POST /petals/:id/iot/readings, query_guard whitelist. Phase 2-3 delivered @4345e13.
- fe-api export.rs (Phase 4) — reading-shaped parquet/CSV export via readings whitelist + batched anchor join + latlon + nullable geometry (@F11 73f4777).

**Verification**:
- Phases 1-3 code: @4e79780, @4345e13 on disk; tests passing.
- Phase 4 (readings export): gap flagged 2026-07-19; CLOSED by F11 @73f4777 (A23).
- Integration suite: iot_ingest_test.rs + export_share_test.rs readings tests.
- In-app verification: user-gated.

**Left open / handed off**:
- FR-5 (Phase 4, readings-shaped export) — handed to mission_prodready_continuation. CLOSED by F11 @73f4777.
- Task 4.2 (external API puller, poll-based ingestion connector catalog) — declared future work in plan.md Phase 4 note; v1 ingestion shape (authenticated batch POST, webhook-able) shipped complete.
- Task 4.3 (integrated sweep) — subsumed by F19 single end-of-mission workspace sweep.
- IoT row storage (SurrealDB vs columnar, D-5) and ingest defaults (D-57/D-58) await ratification.

**Lessons**:
- Bounded node_log cache (FR-2) proved essential: unbounded O(N) clone per frame is hostile to IoT-frequency updates. Local meters CRS seam respected throughout.
- Readings whitelist in export (classify_export_table) stays maintainable: new payload-shaped tables can register without touching core export logic.
- Timeseries semantics: union idempotence, no overwrites on conflict, order-independence allow eventually-consistent replication.

**Commits**: 4345e13 (Phases 1-3); F11 @73f4777 (Phase 4); F19 sweep. Residual: iot_reading retention/GC not wired (fe-database/src/AGENTS.md §replication-mode).
