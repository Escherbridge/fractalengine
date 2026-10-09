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
  - [ ] **part B**: sim control surface per DEC-C7 — fe-runtime contract, fe-sim `ScenarioSession`, relay `sim-control` feature, `POST /api/v1/sim/*` + MCP tools
  - Part A gate (orchestrator-run, serial): fe-sim 33/33, fe-sync 221/221, fe-database 275 pass, clippy -D warnings + fmt clean
  - Part A extras: F8 bugfix (event at_ms now absolute — faults actually fire mid-run), determinism hardening (seeded identities, write-order replays, exact barriers)
- [ ] **M3 milestone review** — adversarial pass over F24+F8+F9 (scrutiny pattern)

## M4 — BI egress (DuckDB-first) — order F11 → F10 → F12 (DEC-C11); map in m4-bi-egress-design.md

- [ ] **F11** (A23+A24) — iot_reading parquet/CSV export (anchor attach-join is NEW query-shape work; guards already readings-ready); share-signer keystore slot per DEC-C9
- [ ] **F10** (A22) — live relay + real DuckDB CLI e2e (nodes AND readings), checked-in script + recorded output; Range-gap resolution per DEC-C10; cosmetic fold-ins (a)–(e) — note (c) rename unconfirmed, verify vs F22 diff first; (d) is a real doc bug
- [ ] **F12** (A25) — docs/bi-egress.md with verified DuckDB/PowerBI/spreadsheet steps + fix the documented /api/v1/query distributed shape
- [ ] **M4 milestone review**

## M5 — MCP + API integration

- [ ] **F13** (A26) — ToolSpec/ScopeRule dispatcher, 6→20 MCP tools, 3 weak-authz fixes, GLB upload API-side
- [ ] **F14** (A27) — harness suites: WS, hexon/tileset, IoT ingest+export, share mint→redeem, token lifecycle, cross-thread, MCP negatives; fix peer.rs:705/709 bare `.ok()`
- [ ] **M5 milestone review**

## M6 — GIS hexon examples

- [ ] **F15** (A28) — ≥2 new region configs (≥1 non-US), ETL runs, provenance, README table
- [ ] **F16** (A29) — hexon install via Hexon Manager + relay tile plane verification, sample-hexons/ entries
- [ ] **M6 milestone review**

## M7 — Close-out

- [ ] **F17** (A30) — README/ops docs rewrite (P2P status, relay ops surface incl. Windows SurrealKV note + process-lifetime secrets note, sim usage)
- [ ] **F18** (A31) — conductor reconciliation: VALIDATED notes on absorbed tracks, new tracks (timeseries-fabric, simulation-lab, BI-verification), consolidation pointers from p2p track, tracks.md/roadmap.md refresh, THIS track retro + archive
- [ ] **F19** (A32+A33) — full sweep: fmt, clippy -D warnings, workspace tests (RUST_MIN_STACK), harness scenarios, relay release build, git hygiene check

## Verification evidence log

(appended as features land)
