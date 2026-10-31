---
type: retro
track: simulation_lab_20261009
title: "Retro: Simulation Lab"
created: 2026-10-31
---

## Summary

**What shipped**: fe-sim crate (new workspace member): SimClock, virtual doc+gossip transport planes, deterministic sensor-fleet scenarios, ScenarioSession REST/MCP control surface, HLC-safety hardening from M3 review.

**Implementation**:
- fe-sim/src/clock.rs — SimClock (accelerable, stepped for scenarios); install/uninstall shim; SCENARIO_RUN_LOCK for test serialization.
- fe-sim/src/net.rs — SimNet hub (latest-per-key docs, min-heap deliveries by (due_ms, seq)); MockVerseReplicator for membership/churn/partitions/heal.
- fe-sim/src/fleet.rs — Deterministic sensors (sine/weather/random-walk); declarative JSON fleet config; plan_fleet pure planner.
- fe-sync virtual_transport.rs — VirtualReplica + VirtualTransportFactory traits; spawn_sync_thread_with_transport; no real endpoint (Started{online:true, node_addr:None}).
- fe-sim bin — `fe-sim [run|print] <script.json>`.

**Verification**:
- A18 (fleet model): scenario_run_cadence test — 3 models × 5 ticks = 15 real rows, exact values, on-cadence recorded_at, HLC wall bits == tick ms.
- A19 (determinism): scripted_scenario_is_deterministic_with_no_real_network — same script twice yields identical per-peer fingerprints, 21 readings + latency/churn/partition/heal faults.
- A20 (control surface REST+MCP): two_peer_scenario scenario; fe-api/src/sim_control.rs (F9 @545fa0b).
- A21 (virtual gossip + scenarios): bound_endpoint_count asserted unchanged; 14 harness scenarios green (FE_SYNC_RELAY=disabled).
- M3 adversarial review: PASS-WITH-FIXES (3 MAJOR) → fixes @73f4777 → VALIDATED.

**Left open / handed off**:
- Gossip plane not virtualized (VersePeers rosters empty); sharded/balanced placement sees only local peer; roster virtualization needed for sim scenarios to exercise distributed query.
- One-ULP observation on f64 round-trip (deterministic per peer; fingerprint unaffected).
- Co-resident production DB-thread stamps read sim time during session (fe-sim/src/AGENTS.md §hlc-sim; mitigated by start_ms<=now + FE_SIM_ALLOW lab-relay doctrine).
- Inbound DB-channel drop under burst (§honest-limits).
- Sim step idempotency undocumented-for-retry (M3 minor #5).
- MCP sim_* tools listed in GUI tools/list despite no GUI sim-control seam (@fe-api/src/mcp/mod.rs:158-166).
- auto_pump is demo-only.

**Lessons**:
- Deterministic clock (process-global override) requires test serialization (SCENARIO_RUN_LOCK) when parallel tests run.
- Virtual transport seam (VirtualReplica trait) ensures prod and sim sync code never drift by construction.
- Sensor models as keyed splitmix64 hashes (not RNG state) deliver determinism without per-seed setup.
- Scenario fingerprints exclude reading_id/source_did/HLC counter bits by design; cross-peer exact-equality needs epsilon for irrational inputs.

**Commits**: c90cd3f (F8 A18/A19); db945e4 (F9A A21); 545fa0b (F9B A20); 73f4777 (M3 fixes). Docs: docs/simulation-lab.md (@277edea F17).
