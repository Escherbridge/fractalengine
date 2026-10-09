---
type: Spec
track: simulation_lab_20261009
title: "Simulation lab (fe-sim) — consolidated record"
timestamp: 2026-10-09T00:00:00Z
---

# Simulation lab (fe-sim) — consolidated record

Record track created at mission close (F18, A31) for the `fe-sim` crate and
its control surface. Implementation evidence lives in the mission
continuation track; this file summarizes and points, it does not duplicate.

## Pointers

- `conductor/tracks/mission_prodready_continuation_20261008/plan.md` §M3
  (F8, F9 part A/B, the M3 milestone review PASS-WITH-FIXES → VALIDATED
  verdict).
- `conductor/tracks/mission_prodready_continuation_20261008/spec.md`
  DEC-C6, DEC-C7, DEC-C13 (f64 1-ULP tolerance discipline, sim-control
  placement/feature-gating, HLC protection design).
- `docs/simulation-lab.md` (F17, operator-facing usage doc).
- `fe-sim/src/AGENTS.md` (crate-local design rationale).

## What it is (summary)

- **fe-sim crate** (new workspace member, `c0266e3` / F8, A18+A19): virtual
  transport (`VirtualReplica`/`VirtualTransportFactory`, byte-identical
  command loop to production — no parallel sim-replication model, D3);
  `SimClock` (accelerable, pluggable wall-clock override, process-global
  `SCENARIO_RUN_LOCK`); `SimNet` hub (latest-per-key virtual docs,
  `(due_ms, seq)` min-heap delivery, scripted latency/partition/churn);
  pure sensor models (sine/weather/random-walk) feeding real `iot_reading`
  rows through the F5/F7 DB-thread ingestion seam — sim readings are real
  rows, not a parallel fixture format.
- **Virtual gossip plane** (`1f2b6fd` / F9 part A, A21): `TopicSender`
  `{Real,Virtual}`, DID-sorted deterministic fan-out, miss-forever
  semantics; ≥2 checked-in CI scenarios (`sharded_query.json`,
  `offline_degraded.json`); the f64 1-ULP delta was root-caused to
  serde_json float round-tripping and fixed workspace-wide (DEC-C6) — 0 of
  200k deltas, was 5,395 of 200k.
- **Control surface** (`4854227` / F9 part B, A20): `ScenarioSession`
  decomposing the blocking `run_scenario` into start/step/inject-fault/
  stop/status, `SimControlCommand/Call/Result` living in fe-runtime (zero
  new fe-api deps), `POST /api/v1/sim/*` + 5 MCP tools, relay-only behind a
  default-off `sim-control` cargo feature (GUI stays `None` — DEC-C7: a sim
  session sharing the process-global HLC override with a live editing
  session is a correctness hazard).
- **HLC protection lineage** (DEC-C13 → M3 review finding #2 fix):
  `start_ms` validated (≤ real now, < 2^48), snapshot/restore HLC on
  session teardown (restore to `max(snapshot, real now)`), `FE_SIM_ALLOW=1`
  runtime opt-in required beyond the compile feature.
- **M3 milestone review** (adversarial opus, 2026-10-09): PASS-WITH-FIXES
  on 3 MAJOR findings (bounded-channel deadlock, process-global HLC skew,
  F24 ingest correlation) — fixes landed @`2023809` (combined with F11);
  verdict then M3 VALIDATED.

## Status

`done` — this is a closed record of a delivered subsystem, not an active
work item.
