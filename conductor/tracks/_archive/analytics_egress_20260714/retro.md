---
type: retro
track: analytics_egress_20260714
title: "Retro: Analytics BI Egress"
created: 2026-10-31
---

## Summary

**What shipped**: BI last-mile egress — GeoParquet export, CSV export with copy-paste UX for SQL/URLs, signed share URLs for PowerBI/spreadsheets. The track delivers on the P0 roadmap goal (spatial analytics engine, kill feature = copy-paste query into PowerBI).

**Implementation**:
- fe-query columnar/geoparquet/{mod,codec}.rs — GeoParquet writer/reader with arrow+parquet 54.x, WKB Point Z, CRS metadata. 5 tests incl. round-trip.
- fe-api export.rs, share.rs, crs.rs, query_guard.rs, limits.rs — DuckDB-httpfs export endpoints (GET /petals/:id/export.parquet|.csv), signed-URL share service (POST/GET /api/v1/query/share with ed25519 verify_strict), CRS resolution (three-branch: envelope + CSV comment + parquet geo metadata).
- fe-ui egress_card.rs, egress_strings.rs — GeoParquet panel card, pure tested builders (12 tests) for SQL/query+export URLs/DuckDB snippet/mint-share curl.

**Verification**: 
- Phases 1-5 code committed; fe-api/tests/export_share_test.rs integration suite (~20 tests). 
- Phase 6 e2e SUPERSEDED by scripts/bi-egress-verify.ps1 (live relay + real DuckDB 1.5.6): 31/31 → 48/48 → 49/49 across review saga (M4 FAIL → fix @6f23aa7 → re-review STILL-FAILING → remediation @236ae8e).
- In-app verification: user-gated, not completed.

**Left open / handed off**:
- ApiState.share_signer is ephemeral (persisted-key wiring handed to fe-api/AGENTS.md §share for integration).
- Task 6.1: fe-api/tests/export_e2e.rs never written; scripts/bi-egress-verify.ps1 is the acceptance equivalent.
- D1/D2 provisionals superseded by user-ratified mission decision (DuckDB-first + signed share URLs).
- GeoParquet crs metadata fixed post-ship (bd46845, spec-legal null, D-16 scope).
- Residuals (ledgered, not blocking closure): (a) /query and share fmt=json require db_reader (no channel fallback), (b) in-app share-link mint button (D-64 copyable curl default), (c) WKB local-frame axis order + f32 anchor quantization.

**Lessons**:
- GeoParquet is load-bearing: the round-trip tests caught a silent CRS-metadata encoding bug that would have shipped (only the parquet geo-key encoding needed the fix, DEC-C16).
- CRS conversion at API layer only proved correct — moving it to storage would couple the format to one project's grid, breaking portability.
- Copy-paste UX works: the three egress shapes (SQL string, query+export URL, DuckDB snippet) are high-leverage — users reach for one.
- Query-guard extraction (separate query_guard.rs reused by both execute and export) kept the two paths in sync.

**Commits**: 82c373f, 77f0061, 4345e13, 54fce7b, 0082c64, b2ca4a7 (Phases 1-5); F11 @73f4777 + F10 @7418a16 (Phase 5.1 readings export + live e2e).
