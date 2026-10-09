---
type: Plan
track: mission_prodready_continuation_20261008
title: "Execution plan — F9–F19"
timestamp: 2026-10-08T23:00:00-07:00
---

# Plan — F9–F19 continuation

Per-feature status board. Statuses: `pending` | `in_progress` | `landed` (committed,
crate gates green) | `validated` (milestone review passed). Each landed feature gets
its commit hash + evidence line appended here.

## M3 — Simulation lab (finish)

- [ ] **F9** (A20+A21) — `in_progress`; **part A landed @ `1f2b6fd`** (factory's partial fe-sync diff kept, DEC-C2)
  - [x] fe-sync gossip-plane seam: `TopicSender`, `VirtualGossipTopic`, subscribe/pump wiring (inherited diff; compile fixes in part A)
  - [x] fe-sim hub-side gossip plane: DID-sorted deterministic fan-out, miss-forever semantics (`net.rs gossip_broadcast`)
  - [x] `SimTransportFactory::join_gossip_topic` wiring + identity tagging (`net.rs:960`)
  - [x] CI scenario 1: `fe-sim/scenarios/sharded_query.json` — merged aggregate == union oracle, full coverage, fingerprint-stable
  - [x] CI scenario 2: `fe-sim/scenarios/offline_degraded.json` — 6/3 missing shards w/ `missing_hosts=["alice"]` during outage, 18/0 after heal
  - [x] f64 1-ULP executed (DEC-C6): serde_json `float_roundtrip` workspace-wide; 0/200k deltas (was 5,395/200k); bit-exact replica asserts
  - [x] **part B landed @ `4854227`**: fe-runtime sim_control contract, fe-sim ScenarioSession (run_scenario = 3 lines over it; equivalence + live-fault-matches-scripted fingerprint tests), control bridge (concurrent start → Conflict before the lock), REST /api/v1/sim/* + 5 MCP tools (EXPECTED_TOOLS 11→16), relay-only default-off `sim-control` feature (dep-tree proof 0)
  - Part A gate (orchestrator-run, serial): fe-sim 33/33, fe-sync 221/221, fe-database 275 pass, clippy -D warnings + fmt clean
  - Part B gate (orchestrator-run, serial): fe-sim 37/37, fe-runtime 92, fe-api 165, harness 26; clippy default+feature clean; fmt clean
  - Part A extras: F8 bugfix (event at_ms now absolute — faults actually fire mid-run), determinism hardening (seeded identities, write-order replays, exact barriers)
  - Open items → F13: sim guard is role-only (add global/admin ScopeRule); fold sim_* arms into the ToolSpec table. → F17: relay README sim warning. Known: process-global HLC override on a sim-control relay (documented, loud log).
- [x] **M3 milestone review** (2026-10-09, adversarial opus, read-only): **PASS-WITH-FIXES** — findings 1-3 mandatory:
  1. MAJOR bounded-channel deadlock cycle: replica `ReplicatedRowApplied` results have no drainer while `TestPeer::send` blocks → fleets retaining ≳129 rows hang forever past the settle deadline (peer.rs:101/876-883, scenario.rs:632-636). CI scenarios stay under threshold — green CI masked it.
  2. MAJOR process-global HLC skew: sim `start_ms` in the future persists into production stamps across restarts (op_log forward-only + init_hlc(max_persisted)); past start_ms breaks monotonicity; ≥2^48 truncates; chrono-range panics abort the relay. Role guard = Owner of ANY verse.
  3. MAJOR F24 ingest correlation: late reply after 10s timeout delivers to the NEXT waiting client (app.rs:338-347 skips closed entries; no petal/correlation check in iot.rs fallback).
  - Minors batched into the fix pass: #4 validate caps (readings/peers), #6 self-echo drift vs real iroh-gossip (skip author; fix the gossip_deliveries>0 assert), #7 exact min/max/count compares + shard-membership assert, #8 merge_actions position test, #10 doc-comment hijack.
  - Deferred with triggers: #5 step idempotency (→ document; revisit if a real client retries), #9 F24 hydration/drain startup race (→ F19 sweep note), MCP sim tools advertised on GUI (→ F13 table refactor), auto_pump dead code (→ F13/F17 decide expose-or-remove), 1-ULP legacy rows (none in prod; no backfill).
  - Claims verified to hold: real-gossip path drift-free; float_roundtrip safe (no canonical-path float parsing); lock poisoning recoverable; teardown order correct; scenarios include_str-coupled (no drift).
  - **Fixes LANDED @ `2023809`** (combined with F11 — change sets interleave in relay main.rs). Verdict now: **M3 VALIDATED**. Gate: fe-runtime 93, fe-database 277, fe-sync 221, fe-sim 43, fe-query 123, fe-api all suites, harness 26 + 14/14 real-transport scenarios.
  - Residual risk (documented, accepted): co-resident production DB-thread stamps DURING a sim session read sim time — restore can't undo those; mitigations are the start_ms<=now validation + FE_SIM_ALLOW + lab-relay-only doctrine (F17 README).

> **Ops discipline (DEC-C12)**: every gate starts with a disk check; if C: free
> < 10 GB, prune `target/debug/incremental` first (29.7 GB reclaimed 2026-10-09;
> C: is a 1.9 TB drive running ~100% full from non-project data too — flagged
> to the user). Builds remain strictly serialized.
- [ ] **M3 milestone review** — adversarial pass over F24+F8+F9 (scrutiny pattern)

## M4 — BI egress (DuckDB-first) — order F11 → F10 → F12 (DEC-C11); map in m4-bi-egress-design.md

- [x] **F11 LANDED @ `2023809`** (A23+A24) — two-table export whitelist (classify_export_table), batched anchor join (node_id IN $ids, one query per page), latlon via petal projection on the anchor, nullable geometry for dead anchors, f64 bit-exact ReadingSnapshot; readings parquet writer/codec in fe-query; share_signer required ApiConfig field + dedicated keystore slot both binaries (DEC-C9), restart-persistence + foreign-key-401 tests. Open items for F10: nullable WKB cells, Float64 value / Int64 recorded_at_ms schema asserts.
- [x] **F10 LANDED @ `2a4026e`** (A22) — scripts/bi-egress-verify.ps1 + recorded log: 31/31 live checks (real relay + DuckDB 1.5.6 + curl). DEC-C10 → IMPLEMENTED single-range 206 (DuckDB httpfs issues real ranged GETs, observed live). Windows export 503 hole fixed (run_guarded_query_via_state channel fallback; /query + share json branch deferred w/ AGENTS.md note). Mint path: offline mint_api_token example + shared node-keypair seed env (no HTTP route by design). Fold-ins (a)-(e) done; (c) verified genuine misnomer and renamed. NEW: GeoParquet crs spec violation found live → DEC-C16 (PROJJSON-null fix pending); script uses enable_geoparquet_conversion=false meanwhile. Gate: fe-api 182, relay 7/7, clippy/fmt clean.
- [x] **F12 LANDED** (A25) — docs/bi-egress.md (240 lines): every command sourced from the 31/31 verified script/log; DuckDB leg marked live-verified, PowerBI/spreadsheet legs honestly marked derived-not-script-verified; troubleshooting covers the four real failure modes. (/api/v1/query distributed-shape doc fix landed with F10.) README cross-link deferred to F17 (owns README).
- [ ] **M4 milestone review** — adversarial pass over F11+F10+DEC-C16+F12 (A22-A25)
- [ ] **M4 milestone review**

## M5 — MCP + API integration

- [ ] **F13** (A26) — ToolSpec/ScopeRule dispatcher, 6→20 MCP tools, 3 weak-authz fixes, GLB upload API-side
- [ ] **F14** (A27) — harness suites: WS, hexon/tileset, IoT ingest+export, share mint→redeem, token lifecycle, cross-thread, MCP negatives; fix peer.rs:705/709 bare `.ok()`
- [ ] **M5 milestone review**

## M6 — GIS hexon examples

- [x] **F15 LANDED** (A28) — gis-tile-etl @ `33eff03`, main-repo provenance @ `dbb6461`: configs/intl-regions.toml (esri-world-imagery + aws-terrarium, DEC-C8), Zurich Alps (10+10 tiles, 1.48 MB) + Mount Fuji (8+8, 0.84 MB) built + `Verify: OK`; 17/17 tests; sink fixed for fe-format TilesetMeta drift (scale fields None → app-side backfill). Flag for fe-format owners: derive(Default) on TilesetMeta would stop sibling-repo struct-literal breakage on future field adds.
- [ ] **F16** (A29) — install dist/*.hexon via install_sample_hexons example + Hexon Manager semantics, serve via relay tile plane (petal-bound authz), scale-bar backfill assert; hexon_ids: tileset-switzerland-zurich-alps, tileset-japan-mount-fuji
- [ ] **M6 milestone review**

## M7 — Close-out

- [ ] **F17** (A30) — README/ops docs rewrite (P2P status, relay ops surface incl. Windows SurrealKV note + process-lifetime secrets note, sim usage)
- [ ] **F18** (A31) — conductor reconciliation: VALIDATED notes on absorbed tracks, new tracks (timeseries-fabric, simulation-lab, BI-verification), consolidation pointers from p2p track, tracks.md/roadmap.md refresh, THIS track retro + archive
- [ ] **F19** (A32+A33) — full sweep: fmt, clippy -D warnings, workspace tests (RUST_MIN_STACK), harness scenarios, relay release build, git hygiene check

## Verification evidence log

(appended as features land)
