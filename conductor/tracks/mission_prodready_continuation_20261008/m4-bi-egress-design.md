---
type: Design Note
track: mission_prodready_continuation_20261008
title: "M4 (F10/F11/F12) — BI egress implementation map (recon 2026-10-09)"
timestamp: 2026-10-09T08:30:00Z
---

# M4 — BI egress: implementation map

Recon output (sonnet explore, 2026-10-09). Line cites at recon time.

## Load-bearing facts

- **Export is NODE-only by one check**: `export.rs:125-131` rejects any
  non-`NODE` table; `share.rs:143-148` mirrors it. The guard pipeline is
  ALREADY readings-ready (`IOT_READING` whitelisted `query_guard.rs:25-38`,
  scope filter covers it `:215-244`). F11 = loosen the check + new row mapper +
  CSV shape; guards unchanged.
- **Readings have no geometry**: `iot_reading` schema has no position column
  (schema test `fe-database/src/schema.rs:706-708`). The documented pattern
  FILTERS by anchor (`anchors_within`, fe-query timeseries.rs:17-28); nothing
  ATTACHES node position to reading rows. F11's anchor join is new query-shape
  work (per-row subquery or two-pass Rust join). Projection reuse:
  `fe_terrain::projection::Projection` + `resolve_petal_crs` (crs.rs:48-85)
  unchanged.
- **Share signer is ephemeral per-process — proven**: hard-coded
  `share_signer: Arc::new(fe_identity::NodeKeypair::generate())` at
  `fe-api/src/lib.rs:111-113`; no ApiConfig field. Persistence template:
  `load_or_generate_keypair(&secret_store, ...)` (keychain.rs:39-62) with
  `OsKeystoreBackend` (GUI) / `EnvBackend` (relay; `set()` is in-process only —
  durable ONLY if the operator exports `FE_SECRET_*` every launch, same caveat
  as the relay node keypair).
- **Range gap (A22 risk)**: `body_response` (export.rs:284-302) advertises
  `Accept-Ranges: bytes`; grep finds ZERO Range/206 handling in fe-api. DuckDB
  httpfs probes parquet footers with ranged GETs. Bodies are fully buffered
  (≤128 MiB cap), so single-range 206 is cheap if needed. Cache-Control absent
  on exports (only assets.rs sets immutable — correct there only).
- **Phase 6 of analytics_egress is untouched**: `export_e2e.rs` absent; no
  scripts/ dir; no DuckDB run ever recorded. `fe-ui` egress_strings.rs builds
  the read_parquet snippet but nothing executes it.
- **Token mint over HTTP unverified**: README mentions `MintApiToken` DB
  command / MCP tool; no HTTP mint route located in recon. F10's script must
  resolve the mint path first (one grep) — fallback: MCP tool or seed example.
- Relay e2e env: `FE_BIND_ADDR`, `FE_DB_PATH`, `FE_P2P_DIR`,
  `FE_SYNC_RELAY=disabled`, `FE_SHUTDOWN_AFTER_SECS` (self-terminating runs),
  secrets `FE_SECRET_{SERVICE}_{ACCOUNT}`. Seed nodes/readings via REST
  (seed_join_verse only writes verse/role rows and needs the relay stopped).
- Fold-ins confirmed: (a) AGENTS.md says 11 distributed-query tests, suite has
  12; (b) replicas.rs:19-24 doc framing imprecise; (c) test rename NOT
  independently confirmed misnomeric — verify against F22 diff before renaming;
  (d) REAL doc bug: documented `/api/v1/query` distributed shape
  (`{"distributed": true, "query": {...}}`) ≠ shipped
  (`distributed: Option<TsQueryKind>` carrying the spec directly,
  types.rs:269-282, rest.rs:865-872) — live-verified 422 in M2; (e)
  seed_join_verse TABLES whitelist lacks iot_reading (example readback only).

## Decisions

- **DEC-C9 (2026-10-09)** — Share signer persistence (A24): dedicated keystore
  slot `load_or_generate_keypair(&secret_store, "share_signer")` on BOTH
  binaries (GUI: OsKeystoreBackend; relay: EnvBackend +
  `FE_SECRET_FRACTALENGINE_SHARE_SIGNER` documented with the
  operator-must-export caveat). NOT derived from the node identity seed:
  share-URL signing is a different capability domain — independent rotation,
  and a leaked share key must not equal node identity. `ApiConfig` gains
  `share_signer` (required field, both call sites construct it).
- **DEC-C10 (2026-10-09)** — Range honesty (A22): F10's e2e empirically probes
  DuckDB httpfs against the live relay. If ranged GETs are issued and fail →
  implement single-range 206 on export/shared responses (bodies already
  buffered; slice the Vec). If DuckDB succeeds via full-GET fallback → STILL
  resolve the false advertisement (implement Range anyway or drop
  `Accept-Ranges`). Advertising bytes without support never ships past F10.
- **DEC-C11 (2026-10-09)** — M4 execution order: **F11 → F10 → F12** (A22
  requires node AND reading parquet from the live relay, so the readings
  export must exist before the e2e can pass; docs last, written from verified
  output). Factory's numbering kept for identity, order re-sequenced.
