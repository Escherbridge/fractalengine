---
type: manifest
title: Campaign final-sweep manifest
created: 2026-10-13
---

# Campaign final-sweep manifest

Durable list of verification steps that the campaign's **final sweep** (RUNBOOK §7 step 6) must execute beyond `cargo test --workspace`.

This manifest captures everything that cannot be deferred, is not automated in CI, or requires live environment verification. It survives between sessions and the W1 closure rolls up into it.

## Core suite (mandatory, ~2 hours)

All of these must pass green before pushing the campaign to origin.

| Step | Command / Procedure | Evidence |
|---|---|---|
| **Unit + integration tests** | `cargo test --workspace` (3452 baseline) | Test summary with count + zero failures |
| **Harness scenario 16** | `cargo run -p fractalengine-test-harness` (real loopback iroh, all 15 scenarios ×2) | Terminal output: `15/15 ×2 scenarios pass` |
| **fe-sync store migration** | `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` | Test pass; `0.35 → 1.x store upgrade` confirmed |
| **fe-database ignored tests** | `cargo test -p fe-database -- --ignored` (15 deferred tests; judge each) | Judgment table: PASS / SKIP reason / FIX needed for each test |
| **BI egress e2e** | `scripts/bi-egress-verify.ps1` | Output pasted into `conductor/tracks/analytics_egress_20260714/evidence.md` §X with DuckDB rowcount + schema |
| **Hexon install e2e** | `scripts/hexon-install-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Hexon serve e2e** | `scripts/hexon-serve-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Relay health** | `scripts/relay-health-verify.ps1` (headless fe-relay 1.x) | Output pasted into `conductor/tracks/p2p_asset_streaming_20260718/evidence.md` §6 |
| **Packaged binary smoke** | Build → extract → launch from different CWD with `FE_SYNC_RELAY=disabled` (AC-9 procedure) | Window opens, data dir beside exe, no panic.log, exact PID tree killed |
| **Relay binary smoke** | Build fe-relay → temp CWD → launch with temp FE_P2P_DIR → `FE_SYNC_RELAY=disabled` (AC-15 procedure) | API server listening, zero ERROR lines, exact PID tree killed |

## W0 / W0b / W0c / W0d must-cover (verify from journals)

Grep the workflow journals for `test_sweep_must_cover` and `test_gate_commands` entries to assemble the exact crates/scopes tested by each workflow's gate. These are recorded at workflow close and form the evidence that the code path was exercised.

**Sources:** `.omc/state/sessions/*/subagents/workflows/*/journal.jsonl`

| Workflow | Scope | Evidence |
|---|---|---|
| **W0 (119dbd0)** | fixture CI gen + bookkeeping | List of test_sweep_must_cover crates from `wf_aa8dded9-e92` |
| **W0b (6a53a2b)** | lint fixes, licenses, runnable packages | List of test_sweep_must_cover crates from `wf_e727510a-f59` |
| **W0c (da3e6a1)** | KTX2 guard, fuzz, CI setup | List of test_sweep_must_cover crates from `wf_34174034-e00` |
| **W0d (558197c)** | shared SurrealKV handle, packaged re-smoke | List of test_sweep_must_cover crates from `wf_e724d2d0-2de` + push-gate logs |

## W1 PAS-P3P4 (append at W1 close)

*This section is appended by the W1 orchestrator at the W1→final-sweep boundary. It captures the must-cover list for the 8 parallel slices (S1..S8) + the sequential integration pass (S8b) + the reviewer corrections (S9).*

*Placeholder — leave empty until W1 closure.*

---

## Signoff checklist

- [ ] All core suite steps executed and passing
- [ ] W0/W0b/W0c/W0d must-cover lists reconciled and verified
- [ ] W1 PAS-P3P4 must-cover list appended and verified
- [ ] Zero test failures across all scopes
- [ ] No regressions in e2e scripts
- [ ] Packaged binaries (GUI + relay) launch and connect cleanly
- [ ] Campaign final sweep complete → ready to push

