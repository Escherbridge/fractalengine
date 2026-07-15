---
type: Implementation Plan
title: Analytics Egress (BI last-mile) — implementation plan
tags: [plan, analytics, bi, egress, geoparquet, duckdb, analytics_egress_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./spec.md
---

# Plan: Analytics Egress (BI last-mile)

**Track:** `analytics_egress_20260714` · **P0 / CORE-ANALYTICS** · Crates: `fe-query`, `fe-api`, `fe-ui`
Spec: `./spec.md` · Roadmap: `conductor/roadmap.md §1` · Audit basis: `.omc/research/track-audit-20260714-work.json`

## Audit-verified starting state (2026-07-14 — do not re-derive)

- `fe-query/src/columnar/geoparquet.rs:40-56` — `write_nodes_parquet`/`read_nodes_parquet` are **stubs**; `fe-query/Cargo.toml` has **no parquet/geoarrow dependency**. `GeoParquetMeta` struct exists.
- `fe-api` has **zero export endpoints** (no parquet/CSV/OData hits anywhere in `fe-api/src`).
- `rest.rs::execute_query` (~784-926) ALREADY has: viewer role gate, per-DID rate limit (10 q/s, ~796), single-SELECT + keyword blocklist + table whitelist, scope-filter injection (`build_scope_filter`/`inject_scope_filter`, ~890), and a **5s statement timeout** (~902). **FR-4's real delta is only row-cap + result-size ceiling** — do NOT re-implement the timeout.
- **FR-6 is partially satisfied**: `inject_scope_filter` pre-filters node-table queries by token scope. The delta is a **shareable-URL scope ceiling + export pre-filtering**, not greenfield RBAC.
- CRS conversion machinery exists at the API layer: `fe-api/src/gis.rs::load_petal_terrain_origin` (~358), `Projection.wgs84_to_local`, `bbox_ll_to_local`. FR-5 coordinates with in-progress `hexon_scale_orchestration_20260712`.
- Anchor to **symbol names**, not line numbers — rest.rs line anchors will drift.

## Outstanding decisions (provisional — each marked below)

- **D1 — Egress shape:** parquet + CSV **download endpoints** first, served with correct content-type/range support so **DuckDB reads them via httpfs** (`read_parquet('https://.../export.parquet?...')`). A real DuckDB ATTACH-style native wire protocol is **out of scope**; OData deferred to a fast-follow track. (provisional — logged in outstanding decisions)
- **D2 — Shareable query URLs:** short-TTL **signed scoped URL** — HMAC or ed25519 signature over (query + scope ceiling + expiry), verified server-side; **NOT** long-lived embedded bearer tokens. (provisional — logged in outstanding decisions)
- **D3 — CRS metadata source (FR-5):** egress ships a `crs`/scale metadata field sourced from whatever `hexon_scale_orchestration_20260712` has landed; when the hexon lacks metadata, emit documented placeholder `"EPSG:4326 (petal-local meters, origin=<terrain origin>)"`. Do **not** hard-block on that track. (provisional — logged in outstanding decisions)
- **D4 — Limit values:** `/query` JSON row cap 10 000 rows / 8 MiB response ceiling; export endpoints 500 000 rows / 128 MiB, all named constants in one module. (provisional — logged in outstanding decisions)
- **D5 — Panel home (FR-3):** the egress copy-paste panel lives in the **GIS panel** (`fe-ui/src/panels/gis_panel.rs`), next to `query_tab.rs`. (provisional — logged in outstanding decisions)

Style constraints (all phases): files ~300 lines (split modules past that), terse one-line doc comments (rationale goes in the directory `AGENTS.md`), no ad-hoc role checks (reuse `require_role` / `build_scope_filter` / `inject_scope_filter`), no `.unwrap()`/`.expect()` in prod paths, read-back tests for anything persisted, single test sweep at the end of each phase.

---

## Phase 1 — GeoParquet real writer + round-trip test [fe-query only]

- [x] **Task 1.1 — Add parquet/arrow deps to fe-query.** *(Done 2026-07-15: plain `parquet`/`arrow` 54.x — matching datafusion 46's arrow line — behind a new lighter `parquet` feature; `datafusion` implies it. geoarrow-rs rejected: writer surface unstable + large dep tree. Not hoisted to workspace root — fe-query is the sole consumer. Bonus: fe-query dev-dep tokio switched to `workspace = true`, closing build_size_mobile_prep_20260508's last item.)*
  Files: `fe-query/Cargo.toml`, workspace `Cargo.toml`.
  Add `parquet` + `arrow` (or `geoarrow` if its writer is stable enough — decide at implementation, prefer plain `parquet` ArrowWriter + hand-written GeoParquet `geo` KV metadata to keep the dep surface small). Keep behind the existing `datafusion` feature (or a new lighter `parquet` feature so fe-api doesn't drag datafusion in).
  Accept: `cargo check -p fe-query` green with the feature on; no new default-features bloat for downstream crates.
- [x] **Task 1.2 — Replace `write_nodes_parquet` stub with a real writer.** *(Done 2026-07-15: split into `columnar/geoparquet/{mod,codec}.rs`; rows→RecordBatch→ArrowWriter with GeoParquet 1.0 `geo` footer metadata; position as ISO WKB Point Z; CRS default `"PETAL-LOCAL:meters;origin=unset"` — never silent EPSG:4326; `write_nodes_parquet_with_meta` is the FR-5 override seam. AGENTS.md §geoparquet added.)*
  Files: `fe-query/src/columnar/geoparquet.rs` (split into `geoparquet/{mod,schema,write,read}.rs` if >300 lines), `fe-query/src/AGENTS.md` (new §geoparquet section for the why).
  Map `EntitySnapshot` → RecordBatch (node_id, petal_id, position as geometry, rotation, scale, properties JSON, updated_at_ms); write via ArrowWriter with GeoParquet 1.0 file metadata (`geo` key) built from `GeoParquetMeta` (`primary_geometry_column`, `crs`, `encoding`).
  Accept: output file passes a parquet-footer sanity check in-test; `geo` metadata present and spec-shaped; returns real row count; no `.unwrap()` in the writer path.
- [x] **Task 1.3 — Replace `read_nodes_parquet` stub + round-trip test.** *(Done 2026-07-15: real reader incl. `read_geo_metadata`; stub tests replaced with 5 tests — round-trip field equality on 2 snapshots incl. properties + geo-metadata assertions, empty-slice write, corrupt-file Err-not-panic, custom-CRS honored, local-meters default.)*
  Files: `fe-query/src/columnar/geoparquet.rs` (+submodules).
  Read RecordBatches back into `Vec<EntitySnapshot>`; delete the stub tests (`write_stub_returns_count`, `read_stub_returns_empty`) and replace with: write→read round-trip equality on ≥2 snapshots incl. properties, empty-slice write, corrupt-file read returns Err (not panic), `GeoParquetMeta` custom CRS honored in file metadata.
  Accept: round-trip test green (this is the FR-1 acceptance test); read-back verifies every field written.
- [ ] **Task 1.4 — Phase sweep.** `cargo test -p fe-query` + clippy on fe-query, once, at end of phase. *(Deferred to the session-end integrated sweep per test policy; `cargo check -p fe-query --features parquet` green 2026-07-15.)*

## Phase 2 — Export endpoints: parquet + CSV reusing execute_query guards [fe-api]

- [x] **Task 2.1 — Extract execute_query's guard pipeline into a reusable helper.** *(Done 2026-07-15: new `fe-api/src/query_guard.rs` — `guard_and_prepare_query` (rate limit → `validate_select_sql` → scope-filter inject) + `run_guarded_query` (owns the pre-existing 5s timeout + row cap) + `enforce_byte_ceiling`; moved verbatim from rest.rs, error strings preserved, `execute_query` now consumes it. AGENTS.md §query-guard.)*
  Files: `fe-api/src/rest.rs` (extract into `fe-api/src/query_guard.rs` if rest.rs growth demands), `fe-api/src/AGENTS.md`.
  Factor the validation chain (single-SELECT, semicolon reject, `BLOCKED_KEYWORDS`, `ALLOWED_TABLES`, `build_scope_filter` + `inject_scope_filter`, rate limiter, 5s timeout) into `fn guard_and_prepare_query(...) -> Result<GuardedQuery, ApiError>` consumed by both `execute_query` and the new export handlers. No behavior change to `/api/v1/query`.
  Accept: existing query-endpoint tests still green; export handlers cannot bypass any guard by construction (one shared entry point).
- [x] **Task 2.2 — Row cap + result-size ceiling (FR-4 delta).** *(Done 2026-07-15: `fe-api/src/limits.rs` named constants per D4 (10k/8MiB query, 500k/128MiB export). Policy: **error, not truncate** — `row cap exceeded (limit N rows…)`; byte ceiling checked during serialization. Applied to /query, both exports, and share redemption; no new timeout added. Tests: row-cap error + byte-ceiling unit test.)*
  Files: `fe-api/src/rest.rs` / `query_guard.rs`, new `fe-api/src/limits.rs` (named constants per D4 (provisional — logged in outstanding decisions)).
  Enforce row cap by fetching `cap+1` (or LIMIT injection) and erroring/truncating-with-flag past the cap; enforce byte ceiling while serializing/streaming. Applies to `/query` (10k/8MiB) and exports (500k/128MiB). Do NOT add another timeout — the 5s `tokio::time::timeout` already exists.
  Accept: test — over-cap query returns a clear `row cap exceeded` error (or truncation marker, pick one and document it); test — oversized result hits the byte ceiling; existing rate-limit/timeout behavior untouched.
- [x] **Task 2.3 — `GET /api/v1/petals/:petal_id/export.parquet?query=...` endpoint.** *(Done 2026-07-15: `fe-api/src/export.rs` + route; Viewer+ + petal-scope RBAC with real HTTP statuses, shared guard pipeline, forced petal pre-filter, node-table-only queries, fe-query `write_nodes_parquet_bytes` shim (in-memory, no temp files), parquet content-type + Accept-Ranges for DuckDB httpfs. READ-BACK integration test incl. scope filtering + wrong-scope 403.)*
  Files: `fe-api/src/export.rs` (new), `fe-api/src/server.rs` (route), `fe-api/src/AGENTS.md`.
  Auth via the same `ApiClaims` extension + `require_role("viewer")`; run through `guard_and_prepare_query`; rows → `EntitySnapshot`/RecordBatch → Phase-1 writer → response body with `Content-Type: application/vnd.apache.parquet`, `Content-Length`, and `Accept-Ranges: bytes` so DuckDB httpfs can consume it (D1 (provisional — logged in outstanding decisions)).
  Accept: integration test — authed request round-trips to a parquet body that `read_nodes_parquet` parses (read-back test); unauthed/wrong-scope request rejected; scope filter provably applied (node outside token scope absent from export).
- [x] **Task 2.4 — `GET /api/v1/petals/:petal_id/export.csv?query=...` endpoint.** *(Done 2026-07-15: same guard path; RFC-4180 CSV with `# crs=` comment line + `X-FE-CRS` header, properties as one JSON-string column (documented in AGENTS.md §export). Tests: header/row parse-back, injection rejected identically to /query, caps enforced.)*
  Files: `fe-api/src/export.rs`, `fe-api/src/server.rs`.
  Same guard path; CSV serialization (header row, RFC-4180 quoting, properties flattened or JSON-string column — document choice in AGENTS.md), `Content-Type: text/csv`.
  Accept: test — CSV parses back with expected headers/rows; injection attempt (`;`, blocked keyword, off-whitelist table) rejected identically to `/query`; row/size caps enforced.
- [ ] **Task 2.5 — Phase sweep.** `cargo test -p fe-api` + clippy, once, at end of phase. *(Deferred to the session-end integrated sweep per test policy; `cargo check -p fe-api -j2` run once at end of the Phase-2/3/5 session.)*

## Phase 3 — Signed shareable query URL [fe-api]

- [x] **Task 3.1 — Signed-URL token format + signer/verifier.** *(Done 2026-07-15: `fe-api/src/share.rs` — `b64url(payload).b64url(sig)`, ed25519 over exact payload bytes verified with `verify_strict` (chose ed25519 over HMAC: identity stack is already ed25519 via `fe_identity::NodeKeypair`, no new secret type). TTL default 1h / max 24h; ceiling = issuer scope verbatim. Tests: valid/tampered-payload/tampered-sig/expired/wrong-key + ceiling preservation.)*
  Files: `fe-api/src/share.rs` (new), `fe-api/src/AGENTS.md` (§share: format rationale).
  Per D2 (provisional — logged in outstanding decisions): compact token encoding (query string or its hash, scope ceiling, expiry unix-ts) signed with HMAC-SHA256 (server secret) or ed25519 (reuse fe-identity keys if trivially available — decide at implementation, HMAC is the floor). Short TTL, default 1h, max 24h. No bearer JWT embedded.
  Accept: tests — valid token verifies; tampered query/scope/expiry rejected; expired token rejected; token grants NO scope wider than the issuing user's scope at signing time (ceiling = intersection).
- [x] **Task 3.2 — Issue + redeem endpoints.** *(Done 2026-07-15: `POST /api/v1/query/share` (authed; body sql+format+ttl_secs — matches the fe-ui egress-card contract) → `{url, token, expires_at}`; `GET /api/v1/shared/{token}` on the PUBLIC router (signature is the credential) re-runs the full guard pipeline with the token scope ceiling, per-token rate limit, dispatches json/parquet/csv. Expired → 410, invalid → 401. Integration tests: issue→redeem round-trip, narrow-scope ceiling enforced (verse-wide SELECT capped to petal), parquet redemption read-back. Signing key is per-process ephemeral — persistence wiring in main.rs left as integration request (AGENTS.md §share).)*
  Files: `fe-api/src/share.rs`, `fe-api/src/server.rs`, `fe-api/src/rest.rs` (reuse `guard_and_prepare_query`).
  `POST /api/v1/query/share` (authed; body = sql + format + ttl) → returns full shareable URL, e.g. `GET /api/v1/shared/{token}` (or `?sig=` on the export routes). Redemption runs the SAME guard pipeline with the token's scope ceiling substituted for live claims scope, then dispatches to JSON/parquet/CSV per requested format. Rate-limit redemption per token.
  Accept: integration test — issue→redeem round-trip returns scoped data; redeemed export is pre-filtered to the token's scope ceiling (FR-6 delta); a URL issued by a narrow-scope user cannot read outside that scope even if the verifier's server-wide key would allow it; expired URL → 401/410.
- [ ] **Task 3.3 — Phase sweep.** `cargo test -p fe-api` + clippy, once. *(Deferred to the session-end integrated sweep per test policy.)*

## Phase 4 — Copy-paste egress panel [fe-ui]

- [x] **Task 4.1 — Egress string builders (pure, testable).** *(Done 2026-07-15: `fe-ui/src/gis/egress_strings.rs` — pure, egui-free. Builders for petal-scope / node-selection / bbox SQL (bbox filters `position.coordinates[0|1]`, the `[x,z]` contract from `gis::query::extract_xz`; normalizes reversed bounds), hand-rolled RFC-3986 percent-encode (no new dep), `/api/v1/query` POST URL, `export.parquet`/`export.csv` GET URLs, DuckDB `read_parquet` snippet, and `mint_share_curl` for `POST /api/v1/query/share` (body sql+format+ttl_secs per Task 3.2). 12 unit tests assert exact strings incl. URL-encoding + SQL-quote escaping.)*
  Files: `fe-ui/src/gis/egress_strings.rs` (new, pure functions; no egui), `fe-ui/src/gis/mod.rs`.
  From current petal + query (or default node SELECT): build (a) the SQL string, (b) the API URL (`/query` POST or export GET incl. signed URL when present), (c) the DuckDB connection snippet — `SELECT * FROM read_parquet('<export-url>');` per D1 (provisional — logged in outstanding decisions).
  Accept: unit tests assert exact string output for a fixture petal/query, incl. URL-encoding of the query param.
- [x] **Task 4.2 — Panel UI in the GIS panel (D5 (provisional — logged in outstanding decisions)).** *(Done 2026-07-15: new **Export** tab on the GIS panel (`GisPanelTab::Export`) hosting `panels/egress_card.rs` — copy rows (egui builtin `ctx.copy_text` + toast) for SQL / query endpoint / parquet+csv export URLs / DuckDB snippet, all sourced from Task-4.1 builders. Query context is panel-local `EgressCardState` on `GisPanelState.egress` (Petal / Node / Bbox); node id fills only via an explicit "Use viewport selection" click, never implicitly from `NodeManager.selected`. Shareable-link section shows TTL field (default 3600, clamped ≤86400) + copyable mint curl — fe-ui has NO fe-api HTTP-client seam, so in-app minting is a recorded follow-up (open_decisions); manual in-app verify deferred to Phase 6. AGENTS.md §egress-card added.)*
  Files: `fe-ui/src/panels/gis_panel.rs` (host; split a `egress_card.rs` sibling if >300 lines), `fe-ui/src/panels/AGENTS.md`.
  "Copy for BI" section: three labeled rows (SQL / API URL / DuckDB snippet), each with one-click copy via egui clipboard; "Get shareable link" button calling the Phase-3 issue endpoint through the existing api-client seam; TTL indicator. Key the panel's query context on `PathEditorState.editing_track_id`-style panel-local state, NOT `NodeManager.selected` (see project memory: track-selection-two-concepts).
  Accept: strings rendered come from Task-4.1 builders (no inline formatting); copy sets clipboard; compile + existing fe-ui tests green. Manual in-app verify deferred to Phase 6.
- [ ] **Task 4.3 — Phase sweep.** `cargo test -p fe-ui` + clippy, once. *(Deferred to the session-end integrated sweep per test policy; `cargo check -p fe-ui -j2` run at end of Phase-4 work.)*

## Phase 5 — CRS metadata on egress + tests (FR-5)

- [x] **Task 5.1 — CRS/scale resolution helper at the API layer.** *(Done 2026-07-15: new `fe-api/src/crs.rs` (gis.rs is past the 300-line cap) — `resolve_petal_crs` with the three D3 branches (hexon `TilesetMeta.crs`/`native_scale` → terrain origin → placeholder `PETAL-LOCAL:meters;origin=unset`, matching fe-query's Phase-1 default rather than D3's provisional EPSG-prefixed string). Pure `crs_label` unit-tested on all three branches.)*
  Files: `fe-api/src/gis.rs` (extend `load_petal_terrain_origin` / `Projection` use), `fe-api/src/export.rs`.
  Per D3 (provisional — logged in outstanding decisions): resolve the petal's CRS/scale from `hexon_scale_orchestration_20260712`'s landed metadata when present; else fall back to the terrain origin; else emit documented placeholder `"EPSG:4326 (petal-local meters, origin=<terrain origin>)"`. Conversion stays at the API layer — do NOT push projection into fe-query/fe-database (`fe-query/src/AGENTS.md §local-coords`).
  Accept: unit tests for all three resolution branches (hexon metadata / terrain origin only / neither).
- [x] **Task 5.2 — Stamp CRS onto every egress path + lat/lon conversion option.** *(Done 2026-07-15: parquet `geo` metadata via `GeoParquetMeta.crs`; CSV `# crs=` first line + `X-FE-CRS` header; `/query` + shared-JSON envelopes gain optional `crs` (petal-scoped → resolved, broader → `origin=per-petal` marker). `?coords=latlon|local` on exports + redemption; latlon converts via petal `Projection` ([lon,lat,ele], EPSG:4326) and 400s without an origin. Landmine test green in `export_share_test.rs`: local output never labeled 4326; latlon output round-trips a known-origin fixture through `Projection`.)*
  Files: `fe-api/src/export.rs`, `fe-query/src/columnar/geoparquet.rs` (`GeoParquetMeta.crs` plumb-through), CSV header comment or sidecar column, JSON `/query` result envelope field.
  Parquet: `geo` metadata `crs` from Task 5.1. CSV/JSON: explicit `crs` field. Add `?coords=latlon|local` on export routes; `latlon` converts via the petal `Projection` (wgs84↔local, as `list_gis_nodes` does) and labels CRS EPSG:4326; `local` keeps meters and labels the local CRS string.
  Accept: **the landmine test** — an export in local meters is never labeled EPSG:4326 degrees: assert `coords=local` output carries the local/placeholder CRS string and `coords=latlon` output carries EPSG:4326 with actually-converted coordinates (round-trip a known origin fixture through `Projection`).
- [ ] **Task 5.3 — Phase sweep.** fe-api + fe-query tests + clippy, once. *(Deferred to the session-end integrated sweep per test policy.)*

## Phase 6 — Integration sweep + docs

- [ ] **Task 6.1 — End-to-end integration test.**
  Files: `fe-api/tests/export_e2e.rs` (new).
  Boot the test API state, seed nodes across two petals/scopes, then: `/query` (row-capped), `export.parquet` (read back via `read_nodes_parquet`, scope-filtered, CRS-stamped), `export.csv`, share-issue→redeem (scope ceiling honored, expiry honored). One test file, read-back assertions throughout.
  Accept: e2e green; covers FR-1/2/4/5/6 acceptance in one sweep.
- [ ] **Task 6.2 — Docs + AGENTS.md + spec reconcile.**
  Files: `fe-api/src/AGENTS.md` (§export, §share, §limits), `fe-query/src/AGENTS.md` (§geoparquet), `README.md` API section (paste-into-PowerBI/DuckDB quickstart: the three copy-paste strings), `./spec.md` (mark FR-4 timeout as pre-existing; record D1-D5 outcomes), `./metadata.json` notes.
  Accept: a cold reader can go from README to a working DuckDB `read_parquet` call; provisional decisions D1-D5 recorded as ratified-or-revised.
- [ ] **Task 6.3 — Full workspace sweep (once, per test policy).**
  `cargo test` on touched crates (fe-query, fe-api, fe-ui) + `cargo clippy -- -D warnings` on the same + fmt check. Fix fallout; then phase-completion checkpoint per `conductor/workflow.md`.

## Cross-track notes

- `hexon_scale_orchestration_20260712` (in progress): FR-5 consumes whatever CRS/scale metadata it has landed; D3 fallback means this track never hard-blocks on it. Re-check its status at Phase 5 start.
- `auth_policy_pattern_20260710` (spec_only): FR-6 minimal slice here is the scope-ceiling + export pre-filtering built on existing `inject_scope_filter`; full policy-engine RBAC row/column filtering migrates there later.
- OData feed, DuckDB native wire protocol, and export provenance (as-of HLC + authored-by DID) are explicit fast-follow/out-of-scope items — see roadmap §go-forward "hardening gaps".
