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

> **Ops discipline (DEC-C12)**: every gate starts with a disk check; if C: free
> < 10 GB, prune `target/debug/incremental` first (29.7 GB reclaimed 2026-10-09;
> C: is a 1.9 TB drive running ~100% full from non-project data too — flagged
> to the user). Builds remain strictly serialized.
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
