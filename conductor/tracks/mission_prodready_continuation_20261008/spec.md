---
type: Spec
track: mission_prodready_continuation_20261008
title: "Prod-ready mission continuation (factory handover F9–F19)"
timestamp: 2026-10-08T23:00:00-07:00
---

# Spec — Prod-ready mission continuation (F9–F19)

## Origin

Factory (droid) ran mission `mis_6c57917e` ("FractalEngine prod-ready: real P2P
replication + sharded timeseries fabric + verified BI egress") from 2026-10-07
and **paused on an unrecoverable usage-402** at 2026-10-09T04:41Z, mid-feature
F9. This track continues that mission to completion in Claude Code.

**Mission source of truth** (read these before working any feature):

- `C:\Users\atooz\.factory\missions\77c4c579-4f99-403f-8bcf-3b1ae3f0c718\mission.md` — full mission plan (M1–M7)
- `...\validation-contract.md` — acceptance assertions A1–A33 (adopted verbatim here)
- `...\features.json` — feature board with per-feature orchestrator notes (adopted verbatim)
- `...\library\decisions.md` — user-ratified decisions D1–D4 (LOCKED; do not deviate)
- `...\library\worker-notes-p2p.md` — accumulated worker gotchas (63KB; consult per-feature)
- `...\library\user-testing.md` + `...\library\findings.md` — validation evidence + pre-mission code findings
- `...\handoffs\*.json` — 19 per-feature handoffs with verification evidence

## State at handover (2026-10-08)

- **Complete & committed** (local main, 14 commits ahead of origin, NO pushes):
  M1 (F1–F4, F20, validators), M2 (F5–F7, F21–F23, validators), M3 partial
  (F24 live-surface restoration, F8 simulation lab @ `c0266e3`).
- **In flight (uncommitted)**: F9 worker's partial fe-sync diff — gossip/compute
  plane virtualization (A21 seam): `TopicSender {Real, Virtual}` enum,
  `VirtualGossipTopic`/`VirtualGossipMessage` trait surface on
  `VirtualTransportFactory`, virtual branch + pump in
  `subscribe_to_verse_gossip_topic`, seam tests. ~378 lines across
  `fe-sync/src/{distributed_query,sync_thread,virtual_transport}.rs`.
- **Remaining features**: F9 (M3), F10–F12 (M4 BI egress), F13–F14 (M5 MCP/API),
  F15–F16 (M6 GIS hexons), F17–F19 (M7 close-out).

## Acceptance

The remaining assertions of the mission validation contract: **A20–A33**
(sim control surface, CI scenarios, DuckDB e2e, readings export, share-key
persistence, egress docs, MCP 20 tools, harness suites, GIS regions, hexon
install, docs-current, conductor-reconciled, sweep-green, git-hygiene).
Milestone boundaries get an adversarial review pass (the factory pattern:
scrutiny validator per milestone), recorded in plan.md with a one-line verdict.

## Decision log (this continuation)

Decisions made autonomously during the continuation are recorded here, dated.
Factory-era decisions live in the mission library's `decisions.md` (D1–D4).

- **DEC-C1 (2026-10-08)** — Continue factory's mission in place: adopt its
  feature board (F9–F19), validation contract (A20–A33), and per-feature
  orchestrator notes verbatim. Rationale: the board is coherent, evidence-backed
  (19 handoffs), and the user asked to "pick up where factory left off."
- **DEC-C2 (2026-10-08)** — KEEP the F9 worker's uncommitted fe-sync diff and
  build on it. Reviewed in full: the `TopicSender` seam honors the F8 trait
  discipline (D3: prod/sim share the seam), carries F20 identity alignment and
  the F23 forged-attribution gate into sim, includes a loopback seam test, and
  default-`None` keeps gossip-less factories compiling. Reverting would redo
  identical work. Verification: compile+test gate before any new work stacks on it.
- **DEC-C3 (2026-10-08)** — Commit cadence: one commit per feature (factory's
  pattern), crate-scoped test gates per feature, ONE full workspace sweep at
  F19 (A32). Matches both the mission contract and the standing "one sweep at
  the end" preference.
- **DEC-C4 (2026-10-08)** — Builds/tests are serialized by the orchestrator
  (never two cargo invocations in parallel; Windows file-lock + OOM gotchas on
  record). Subagents implement; orchestrator runs gates.
- **DEC-C5 (2026-10-08)** — No pushes (A33). All commits stay on local `main`.
- **DEC-C6 (2026-10-09)** — f64 payload 1-ULP deltas across the replication
  bridge (F8's finding): root-cause first during F9 (bounded investigation).
  If the loss is a cheap, local fix (e.g. a lossy intermediate representation),
  fix it bit-exact; if structural, document the accepted delta + tolerance
  discipline in fe-sim/AGENTS.md. Scenario assertions use epsilon tolerance
  (1e-9) either way, as defense against float-summation order effects — never
  as a mask for transport lossiness. Outcome recorded in plan.md when executed.

- **DEC-C7 (2026-10-09)** — F9 sim control surface placement (A20): message
  contract (`SimControlCommand/Call/Result` + sender alias) lives in
  **fe-runtime** (sibling of `distributed_query.rs`, same seam pattern as
  `ApiConfig.distributed_tx`) so fe-api gains zero new dependencies; the
  backend is a new **`ScenarioSession`** in fe-sim (decomposes the monolithic
  blocking `run_scenario` into start/step/inject-fault/stop/status, holding
  the process-global `SCENARIO_RUN_LOCK` for the session lifetime and
  rejecting a concurrent start with a clean error BEFORE touching the lock);
  wiring is **relay-only behind a default-off `sim-control` cargo feature**
  (`fractalengine-relay`), GUI stays `None` — a sim session sharing the
  process-global HLC override with a live editing session is a correctness
  hazard, and default builds keep D3's "no new prod-path deps" literally true
  (fe-sim pulls in the test harness as a real dependency). Full design map:
  `f9-sim-control-design.md` in this track.

- **DEC-C8 (2026-10-09)** — M6 regions/sources: ESRI World Imagery as the new
  keyless global imagery source (`license_type = "attribution"` — honest ToS
  posture, unlike USGS public-domain); regions = Zurich/Alps (CH) + Mount Fuji
  (JP), CI-scale bboxes, full-region builds documented as operator-run.
  Map + rationale: `m6-gis-regions-design.md` in this track.

- **DEC-C9 (2026-10-09)** — Share signer persisted via a dedicated keystore
  slot (`"share_signer"`), NOT derived from the node identity seed (capability
  separation + independent rotation). Details: `m4-bi-egress-design.md`.
- **DEC-C10 (2026-10-09)** — `Accept-Ranges: bytes` is advertised with zero
  Range support; F10 resolves it empirically with DuckDB httpfs and the false
  advertisement never ships past F10 (implement single-range 206 or drop the
  header). Details: `m4-bi-egress-design.md`.
- **DEC-C11 (2026-10-09)** — M4 order re-sequenced F11 → F10 → F12 (A22 needs
  readings parquet live, so the readings export lands before the e2e; docs
  written last from verified output).

- **DEC-C13 (2026-10-09)** — M3 review fix scope: findings 1-3 mandatory +
  cheap same-area minors (#4, #6, #7-exact-compares, #8, #10-doc) in one fix
  pass; HLC protection = validate `start_ms` (≤ real now, < 2^48) + snapshot/
  restore HLC on session teardown (restore to max(snapshot, real now)) + a
  runtime env opt-in (`FE_SIM_ALLOW=1`) required for the relay's sim bridge to
  accept Start (defense beyond the compile feature); ingest correlation =
  proper `correlation_id` on the InsertIotReadings family + petal_id defense
  in the fallback match — the global pop-and-drop reply-routing change is NOT
  made (alters family-blind Error routing semantics; deferred with a note).
  Fix agents run NO builds (F11 holds the build lock); one serial gate after.

- **DEC-C14 (2026-10-09)** — A26 "20 tools" resolved as "the
  mcp_scene_primitives 20-name vocabulary fully present"; true tools/list
  length 29 (24 + 5 sim) recorded as a documented intentional superset.
  Details: `m5-mcp-harness-design.md`.
- **DEC-C15 (2026-10-09)** — M5: F13 (opus — security-authz refactor) then
  F14 (sonnet) strictly sequential; ToolSpec/ScopeRule shape ratified in
  `m5-mcp-harness-design.md`.

- **DEC-C16 (2026-10-09)** — GeoParquet CRS spec compliance: the geo-metadata
  `crs` key gets spec-legal `null` (petal-local frames genuinely have no
  PROJJSON CRS; null = "unspecified", never a silent EPSG:4326 claim), with
  the honest free-text label moved to a CUSTOM key (`fe:crs`) in the same
  geo metadata + the existing x-fe-crs header. Stock DuckDB then reads our
  parquet without `SET enable_geoparquet_conversion=false`; the escape hatch
  stays documented for older builds only. Implemented as a small dedicated
  pass before F12 so the egress docs state the simple truth.

- **DEC-C17 (2026-10-09)** — M4 review verdict **FAIL** → mandatory fix pass
  (after F13 lands; shared files): (B1) row-level petal_id post-filter in
  prepare_export + reject comments/nested-SELECTs in export/share SQL +
  whitespace normalization + 4-vector adversarial tests; (B2) correlation_id
  on RawQuery/QueryResult (M3 pattern) + db_reader:None cross-delivery test;
  (M1) ETag + If-Range on export/share bodies; (M2) SurrealQL `TIMEOUT 5s`
  appended in the guard; (M3) relay startup warning when share-signer env
  unset + EnvBackend round-trip test + A24 restated honestly (GUI: OS
  keystore; relay: operator-exported env or ephemeral); (M4) docs corrections
  (token extraction, step order, schema shape, 401-not-404, Windows json-503,
  drop "bundled duckdb" + "cannot widen" claims); cheap minors folded in
  (#5 HashSet dedupe, #6 null-anchor honesty — no fabricated origin, #7 latlon
  omits crs key → OGC:CRS84, #10 bytes=-0 → 416, #11 harness share_signer
  default decoupled from node keypair). Script re-run (with a new live latlon
  + geometry assert) is the acceptance gate.
- **DEC-C18 (2026-10-09)** — A22's "immutable cache" header expectation is
  AMENDED: query-driven exports are live data — immutable caching would be a
  correctness bug. Contract: no-store/private Cache-Control (or none) +
  strong ETag + If-Range for read-consistency across ranged GETs. (The
  assets endpoint keeps immutable — content-addressed.)
- **Deferred with triggers (M4 review minors)**: #4 WKB local-frame axes
  (x, elevation, z) misread by GIS readers as (x, north, z) — predates M4,
  semantic/breaking for consumers; revisit when a GIS consumer integration
  lands (own decision + migration note). #8 f32 anchor quantization (~1 m at
  latlon) — revisit if a consumer needs sub-meter anchors (ReadingSnapshot
  f64 position). #2 commit-msg drift (noted, harmless). #9 script
  port-collision hardening (partial: pre-check folded into fix pass).

- **DEC-C19 (2026-10-09)** — M4 re-review remediation (verdict STILL-FAILING,
  narrowly — original vectors all closed, one new HIGH): (N1) `NODE_LOG`
  REMOVED from ALLOWED_TABLES (no documented egress consumer; it is a
  denormalized copy of scoped node data — re-add only behind node-scoped
  substitution when a consumer appears) + PETAL/ROOM/MODEL/CRATE_REGISTRY
  added to PETAL_SCOPED_TABLES (they carry petal_id; the existing filter
  applies unchanged) + AGENTS.md §query-guard corrected; (N2)
  deliver_to_oldest skips correlated entries (typed replies make the
  wildcard unnecessary for them); (N3) global bounded concurrency on
  channel-fallback egress queries (semaphore) — the 6s/5s ordering claim was
  false under queueing; (N4) e2e asserts the relay log contains no
  "post-filter dropped rows" line (proves the SQL rewrite alone scoped the
  probes, not just the egress re-check); plus an independent read-only check
  of the reject_record_constructors denylist against the vendored
  surrealdb-core 3.0.5 parser grammar. Deferred w/ trigger: N5 user-TIMEOUT
  clause collision (fails closed; revisit on user report); verse.namespace_id
  read-capability question → fe-sync owners, next p2p track touch.

- **DEC-C20 (2026-10-09)** — Grammar audit verdict GAPS-FOUND (1 critical):
  comment tokens split the multi-token constructor needles in
  reject_record_constructors, and GuardMode::Query skips reject_comments →
  `type/**/::/**/thing('no'+'de', id).*` reads any petal's nodes via
  /api/v1/query + MCP query (no row backstop there). Fix (two independent
  layers): reject_comments unconditional in ALL guard modes (Query's doc
  justified subqueries, never comments) AND the needle scan excises comment
  spans before matching. All 17 other enumerated SurrealQL 3.0.5 form
  classes verified sound or harmless (record-id literals, escaped idents,
  raw strings, casts, unicode — the lexer's bare identifiers are ASCII-only,
  matching the guard's scanner by construction). Folded into the DEC-C19
  remediation pass as R5.

## Bounds

- Stale sibling forks (`fe-hermes/`, `fe-pi/`, `fe-pibridge/`, `servo/`) untouched.
- No new wire protocols; no Postgres-wire service (D1); iroh 1.0 upgrade out of scope.
- M6 full-region ETL builds are operator-run; CI-scale verification regions only.
