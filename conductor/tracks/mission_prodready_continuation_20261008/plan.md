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

- [ ] **F9** (A20+A21) — `in_progress` (factory's partial fe-sync diff kept, DEC-C2)
  - [x] fe-sync gossip-plane seam: `TopicSender`, `VirtualGossipTopic`, subscribe/pump wiring (inherited diff)
  - [ ] fe-sim hub-side gossip plane: SimNet virtual topics w/ scripted latency/partition/churn parity
  - [ ] `SimTransportFactory::join_gossip_topic` wiring + identity tagging
  - [ ] CI scenario 1: 3-peer sharded distributed query (deterministic, repeat-run fingerprints)
  - [ ] CI scenario 2: peer-offline → degraded (honesty metadata) → return → convergence
  - [ ] Sim control surface: `POST /api/v1/sim/*` + MCP sim tools (start/stop/status/step/inject-fault)
  - [ ] f64 1-ULP decision executed (DEC-C6)
  - Gate: `cargo test -p fe-sim -p fe-sync` + touched-crate clippy/fmt
- [ ] **M3 milestone review** — adversarial pass over F24+F8+F9 (scrutiny pattern)

## M4 — BI egress (DuckDB-first)

- [ ] **F10** (A22) — live relay + real DuckDB CLI e2e, checked-in script + recorded output; cosmetic fold-ins (a)–(e) from M2 scrutiny
- [ ] **F11** (A23+A24) — iot_reading parquet/CSV export (anchor join, coords=latlon); share-signer key persistence (GUI + relay)
- [ ] **F12** (A25) — docs/bi-egress.md with verified DuckDB/PowerBI/spreadsheet steps
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
