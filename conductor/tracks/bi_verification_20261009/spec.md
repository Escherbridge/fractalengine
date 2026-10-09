---
type: Spec
track: bi_verification_20261009
title: "BI egress verification — consolidated record"
timestamp: 2026-10-09T00:00:00Z
---

# BI egress verification — consolidated record

Record track created at mission close (F18, A31) for the M4 BI-egress
verification effort — the live-relay-plus-real-DuckDB proof and the
adversarial review saga it survived. Implementation lives in
`analytics_egress_20260714` (now `done`, see its dated note); this track is
the VERIFICATION record specifically, per the F18 deliverable list.

## Pointers

- `conductor/tracks/analytics_egress_20260714/metadata.json` —
  implementation history + this track's own dated closure note.
- `conductor/tracks/mission_prodready_continuation_20261008/plan.md` §M4.
- `conductor/tracks/mission_prodready_continuation_20261008/spec.md`
  DEC-C9/C10/C11/C16/C17/C18/C19/C20 (the full decision trail).
- `docs/bi-egress.md` (F12, operator-facing verified steps).
- `scripts/bi-egress-verify.ps1` — the standing, re-runnable live
  verification script (real relay + real DuckDB 1.5.6 + curl).

## What was verified (summary)

- **A22 (live DuckDB e2e, F10 `2a4026e`):** `scripts/bi-egress-verify.ps1`
  against a real running relay + real DuckDB CLI (fetched to `tools/`, no
  system install) — 31/31 checks at F10, 48/48 after the first remediation
  pass, 49/49 after the second. DEC-C10 implemented single-range 206
  (DuckDB httpfs genuinely issues ranged GETs, observed live) instead of
  the false `Accept-Ranges: bytes` advertisement.
- **A23/A24 (readings export + share-signer persistence, F11 `2023809`):**
  reading-shaped parquet/CSV export with the batched anchor join;
  share-signer moved to a dedicated keystore slot (DEC-C9) so share URLs
  survive restarts.
- **A25 (docs, F12 `a6d23f3`):** `docs/bi-egress.md`, every command sourced
  from the verified script/log; PowerBI/spreadsheet legs honestly marked
  derived-not-script-verified (only the DuckDB leg is live-verified).
- **M4 milestone review saga** (the reason this gets its own record):
  adversarial opus review verdict **FAIL** (2 BLOCKERs: bypassable
  scope-filter string-append, uncorrelated RawQuery/QueryResult reply
  routing) → fix pass @`667aa18` (DEC-C17: scope-source substitution +
  `correlation_id` + ETag/If-Range + SurrealQL `TIMEOUT 5s` + A24 honesty
  restated) → re-review verdict **STILL-FAILING narrowly** (1 new HIGH:
  `node_log` side-channel leaking unscoped `petal_id`/`name`/`position`) →
  remediation @`770acd3` (DEC-C19: `node_log` removed from
  `ALLOWED_TABLES`; DEC-C20 grammar audit found + fixed 1 critical
  comment-token-splitting bypass in `reject_record_constructors`, 17 other
  SurrealQL 3.0.5 form classes verified sound) → final gate green, e2e
  49/49.
- **DEC-C16:** GeoParquet `crs` metadata corrected to spec-legal `null`
  (petal-local frames have no PROJJSON CRS; the honest label moved to a
  custom `fe:crs` key) so stock DuckDB reads the parquet without the
  `enable_geoparquet_conversion=false` escape hatch.
- **DEC-C18:** A22's "immutable cache" expectation amended — query-driven
  exports are live data, so no-store/private + ETag/If-Range is the
  correct contract, not immutable caching.

## Known limitations (carried forward honestly, not fixed here)

- WKB local-frame axes `(x, elevation, z)` can be misread by GIS readers as
  `(x, north, z)` — predates M4, deferred until a real GIS-consumer
  integration forces the decision.
- `f32` anchor-position quantization (~1 m at lat/lon) — deferred until a
  consumer needs sub-meter anchors (the underlying `ReadingSnapshot` is
  `f64`).
- On Windows (per-handle-lock platforms), `/api/v1/query` distributed mode
  and the public JSON share-redemption path both ride the DB-thread
  channel fallback under a bounded semaphore (F23's 4-permit honest-503
  design) rather than the direct reader — documented in
  `fe-api/AGENTS.md §analytics-query` and the relay README (F17).

## Status

`done` — this is a closed verification record, not an active work item.
